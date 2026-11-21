//! Writes kept on disk while the server is unreachable.
//!
//! A `remember` or `correct` that fails with "server unavailable" is written here instead of
//! being lost (two writes of one week were lost when the server's Tailscale address timed
//! out). Entries are sent oldest first after the next successful write and at session start.
//! A process claims an entry by renaming it before sending, so two processes never send the
//! same one; a claim left by a killed process is given back after [`STALE_CLAIM`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{CorrectArgs, ForgetArgs};
use crate::client::RememberRequest;

const ENTRY: &str = "json";
const CLAIMED: &str = "sending";
const STALE_CLAIM: Duration = Duration::from_mins(10);
const STALE_TMP: Duration = Duration::from_hours(1);

static SEQ: AtomicU64 = AtomicU64::new(0);

/// One queued write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Entry {
    Remember {
        request: RememberRequest,
    },
    Correct {
        args: CorrectArgs,
    },
    /// A source left behind by `correct --cascade` when the server went away after the
    /// correction landed.
    Forget {
        args: ForgetArgs,
    },
}

/// An entry this process renamed so no other sends it.
#[derive(Debug)]
pub struct Claim {
    path: PathBuf,
    original: PathBuf,
}

/// Queued writes for one server.
#[derive(Debug, Clone)]
pub struct Outbox {
    dir: PathBuf,
}

impl Outbox {
    /// `<dir>/<server>`, one subdirectory per server URL so a write is never sent to another
    /// server. `dir` is `YREM_OUTBOX_DIR`, else `$XDG_STATE_HOME/yrem/outbox`
    /// (`~/.local/state/yrem/outbox`). `YREM_OUTBOX_DIR=off` (or `0`) turns the outbox off.
    #[must_use]
    pub fn from_env(env: &impl Fn(&str) -> Option<String>, server: &str) -> Option<Self> {
        let non_empty = |k: &str| {
            env(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let root = match non_empty("YREM_OUTBOX_DIR") {
            Some(v)
                if matches!(
                    v.to_ascii_lowercase().as_str(),
                    "off" | "0" | "false" | "no"
                ) =>
            {
                return None;
            }
            Some(v) => PathBuf::from(v),
            None => match non_empty("XDG_STATE_HOME") {
                Some(x) => PathBuf::from(x),
                None => non_empty("HOME")
                    .map(PathBuf::from)
                    .or_else(std::env::home_dir)?
                    .join(".local")
                    .join("state"),
            }
            .join("yrem")
            .join("outbox"),
        };
        Some(Self::new(root.join(format!("{:016x}", fnv1a(server)))))
    }

    /// A fresh idempotency key for a write that may be sent again.
    #[must_use]
    pub fn key() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        format!("yrem-{}", unique_name(now))
    }

    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes `entries` atomically, readable by the owner only (they hold memory text). When
    /// one cannot be written, the ones written before it are removed: the caller reports an
    /// error and the writer retries the whole call.
    ///
    /// # Errors
    /// The directory or a file cannot be written.
    pub fn push_all(&self, entries: &[Entry]) -> io::Result<()> {
        let mut written = Vec::new();
        for entry in entries {
            match self.push(entry) {
                Ok(path) => written.push(path),
                Err(e) => {
                    for path in &written {
                        let _ = fs::remove_file(path);
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Writes one entry. A queued `remember` keeps the time it was made: sent later without
    /// one, it would be stamped at sending time and could pass for newer than what followed.
    ///
    /// # Errors
    /// The directory or the file cannot be written.
    pub fn push(&self, entry: &Entry) -> io::Result<PathBuf> {
        create_private_dir(&self.dir)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let name = unique_name(now);
        let mut entry = entry.clone();
        if let Entry::Remember { request } = &mut entry {
            request.created_at.get_or_insert(now.as_secs_f64());
            request.idempotency_key.get_or_insert_with(Self::key);
        }
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let path = self.dir.join(format!("{name}.{ENTRY}"));
        write_new_private(&tmp, &serde_json::to_vec(&entry)?)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Queued entries not claimed by anyone, oldest first. Claims older than [`STALE_CLAIM`]
    /// are given back first. A file that is not an entry is left where it is.
    #[must_use]
    pub fn pending(&self) -> Vec<(PathBuf, Entry)> {
        self.recover();
        let mut paths = self.files(ENTRY);
        paths.sort();
        paths
            .into_iter()
            .filter_map(|p| {
                let entry = serde_json::from_slice(&fs::read(&p).ok()?).ok()?;
                Some((p, entry))
            })
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.files(ENTRY).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes `path` for this process; `None` when another process took it first.
    #[must_use]
    pub fn claim(path: &Path) -> Option<Claim> {
        let claimed = path.with_extension(format!("{ENTRY}.{}.{CLAIMED}", std::process::id()));
        fs::rename(path, &claimed).ok()?;
        Some(Claim {
            path: claimed,
            original: path.to_owned(),
        })
    }

    /// Gives `claim` back for a later run.
    pub fn release(claim: Claim) {
        let Claim { path, original } = claim;
        if let Err(e) = fs::rename(&path, &original) {
            eprintln!(
                "yrem: cannot return outbox entry {}: {e}",
                original.display()
            );
        }
    }

    /// Deletes a sent or refused entry.
    pub fn finish(claim: Claim) {
        let Claim { path, .. } = claim;
        if let Err(e) = fs::remove_file(&path)
            && e.kind() != io::ErrorKind::NotFound
        {
            eprintln!("yrem: cannot remove outbox entry {}: {e}", path.display());
        }
    }

    fn files(&self, extension: &str) -> Vec<PathBuf> {
        fs::read_dir(&self.dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == extension))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns claims of killed processes and deletes temporary files they left.
    fn recover(&self) {
        let marker = format!(".{ENTRY}.");
        for claimed in self.files(CLAIMED) {
            let stem = claimed
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.split_once(&marker))
                .map(|(stem, _)| stem.to_owned());
            if let Some(stem) = stem
                && older_than(&claimed, STALE_CLAIM)
            {
                let _ = fs::rename(&claimed, self.dir.join(format!("{stem}.{ENTRY}")));
            }
        }
        for tmp in self.files("tmp") {
            if older_than(&tmp, STALE_TMP) {
                let _ = fs::remove_file(tmp);
            }
        }
    }
}

/// Time, process and a per-process counter: two entries made within the clock's resolution
/// (a microsecond on macOS) still get two names.
fn unique_name(now: Duration) -> String {
    format!(
        "{:024}-{}-{}",
        now.as_nanos(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn write_new_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_new_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(bytes)
}

/// Stable across builds, so a server keeps its directory after an upgrade.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

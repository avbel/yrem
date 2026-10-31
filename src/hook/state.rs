//! Per-session hook state on disk, so separate hook processes agree on what is done.
//!
//! Hooks for one session run as separate, often concurrent processes (Claude Code runs a
//! session's hooks in parallel, plugins may overlap). Every decision is therefore an atomic
//! `create_new` claim of its own file, never a read-modify-write of shared state.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Two hooks for the same event fire within this window.
const SAME_EVENT: Duration = Duration::from_secs(30);
/// A briefing claim still pending after this long belongs to a killed process.
const ABANDONED: Duration = Duration::from_secs(30);
/// How long an unreachable server is skipped without a request.
const DOWN_FOR: Duration = Duration::from_mins(1);
const KEEP_FOR: Duration = Duration::from_hours(7 * 24);

const PENDING: &[u8] = b"pending";
const DONE: &[u8] = b"done";

/// Kinds of per-session claims, one directory each.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// Rids shown to the model.
    Seen,
    /// Transcript line hashes sent to (or deliberately kept out of) `session_capture`.
    Captured,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Self::Seen => "seen",
            Self::Captured => "captured",
        }
    }
}

/// State directory. Every method is best-effort: an unwritable directory degrades to no dedup.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `YREM_HOOK_STATE_DIR`, else `$XDG_CACHE_HOME/yrem/hooks`, else `~/.cache/yrem/hooks`.
    #[must_use]
    pub fn from_env(env: &impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |k| env(k).filter(|s: &String| !s.trim().is_empty());
        if let Some(d) = non_empty("YREM_HOOK_STATE_DIR") {
            return Some(Self::new(d.into()));
        }
        let cache = match non_empty("XDG_CACHE_HOME") {
            Some(x) => PathBuf::from(x),
            None => non_empty("HOME")
                .map(PathBuf::from)
                .or_else(std::env::home_dir)?
                .join(".cache"),
        };
        Some(Self::new(cache.join("yrem").join("hooks")))
    }

    /// `<dir>/<session>[/<sub>]`, created; `None` without a usable session id.
    fn session_dir(&self, session: &str, sub: Option<&str>) -> Option<PathBuf> {
        let mut dir = self.dir.join(safe_name(session)?);
        if let Some(s) = sub {
            dir.push(s);
        }
        fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    fn file(&self, session: &str, name: &str) -> Option<PathBuf> {
        Some(self.session_dir(session, None)?.join(name))
    }

    fn item(&self, session: &str, kind: Kind, key: &str) -> Option<PathBuf> {
        Some(
            self.session_dir(session, Some(kind.dir()))?
                .join(safe_name(key)?),
        )
    }

    #[must_use]
    pub fn has(&self, session: &str, kind: Kind, key: &str) -> bool {
        self.item(session, kind, key).is_some_and(|p| p.exists())
    }

    /// Claims `key`; false when another hook claimed it first. Without state, always true.
    #[must_use]
    pub fn claim(&self, session: &str, kind: Kind, key: &str) -> bool {
        self.item(session, kind, key)
            .is_none_or(|p| create_new(&p, b""))
    }

    pub fn release(&self, session: &str, kind: Kind, key: &str) {
        if let Some(p) = self.item(session, kind, key) {
            let _ = fs::remove_file(p);
        }
    }

    /// True when this process should inject the session briefing.
    ///
    /// The first call per session wins and stays pending until [`Store::finish_digest`]; a
    /// claim pending for longer than 30 s belongs to a killed process and is taken over. A
    /// finished claim blocks duplicates, except that after `compact` or `clear` the old
    /// briefing is gone from the context, so one later call may claim again.
    #[must_use]
    pub fn claim_digest(&self, session: &str, source: Option<&str>) -> bool {
        let Some(path) = self.file(session, "digest") else {
            return true;
        };
        if create_new(&path, PENDING) {
            return true;
        }
        let Some(age) = age(&path) else {
            return false;
        };
        let pending = fs::read(&path).is_ok_and(|b| b == PENDING);
        let reclaim = if pending {
            age > ABANDONED
        } else {
            age > SAME_EVENT && matches!(source, Some("compact" | "clear"))
        };
        // Only one concurrent caller can move the old claim away; it alone may re-claim.
        let stale = path.with_extension(std::process::id().to_string());
        if !reclaim || fs::rename(&path, &stale).is_err() {
            return false;
        }
        let _ = fs::remove_file(stale);
        create_new(&path, PENDING)
    }

    /// Marks the briefing as injected.
    pub fn finish_digest(&self, session: &str) {
        if let Some(p) = self.file(session, "digest") {
            let _ = fs::write(p, DONE);
        }
    }

    /// Gives up a claim whose briefing could not be fetched.
    pub fn release_digest(&self, session: &str) {
        if let Some(p) = self.file(session, "digest") {
            let _ = fs::remove_file(p);
        }
    }

    /// Claims the right to open the tracked server session (once per harness session).
    #[must_use]
    pub fn claim_tracking(&self, session: &str) -> bool {
        self.file(session, "tracking")
            .is_some_and(|p| create_new(&p, b""))
    }

    pub fn release_tracking(&self, session: &str) {
        if let Some(p) = self.file(session, "tracking") {
            let _ = fs::remove_file(p);
        }
    }

    pub fn set_server_session(&self, session: &str, id: &str) {
        if let Some(p) = self.file(session, "tracking") {
            let _ = fs::write(p, id);
        }
    }

    /// Takes the tracked server session id, so only one hook closes it.
    #[must_use]
    pub fn take_server_session(&self, session: &str) -> Option<String> {
        let path = self.file(session, "tracking")?;
        let taken = path.with_extension(std::process::id().to_string());
        fs::rename(&path, &taken).ok()?;
        let id = fs::read_to_string(&taken).ok();
        let _ = fs::remove_file(taken);
        id.filter(|s| !s.trim().is_empty())
    }

    #[must_use]
    pub fn server_down(&self) -> bool {
        age(&self.dir.join("down")).is_some_and(|a| a < DOWN_FOR)
    }

    pub fn mark_down(&self) {
        if fs::create_dir_all(&self.dir).is_ok() {
            let _ = File::create(self.dir.join("down"));
        }
    }

    /// Deletes sessions untouched for a week.
    pub fn sweep(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            if newest(&path).is_some_and(|a| a > KEEP_FOR) {
                let _ = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
            }
        }
    }
}

/// A path-safe file name, or `None` when nothing usable remains.
fn safe_name(s: &str) -> Option<String> {
    let safe: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    (!safe.is_empty()).then_some(safe)
}

fn create_new(path: &Path, content: &[u8]) -> bool {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .is_ok_and(|mut f| f.write_all(content).is_ok())
}

fn age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

/// Age of the most recently touched file under `path`.
fn newest(path: &Path) -> Option<Duration> {
    if !path.is_dir() {
        return age(path);
    }
    fs::read_dir(path)
        .ok()?
        .flatten()
        .filter_map(|e| newest(&e.path()))
        .chain(age(path))
        .min()
}

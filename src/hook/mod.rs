//! `yrem hook <event>`: agent lifecycle hooks shared by every harness plugin.
//!
//! Plugins only pipe the harness's event JSON to this command. Everything else lives here:
//! the briefing, the relevance gate, per-session dedup and capture. A hook never fails the
//! session: errors go to stderr and the exit code is always 0.

pub mod gate;
pub mod render;
pub mod state;
pub mod transcript;

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::ValueEnum;
use serde::Deserialize;
use serde_json::Value;

use crate::client::Client;
use crate::config::{Config, Overrides};
use crate::error::Error;
use crate::ops::{App, RecallArgs, SessionCaptureArgs, SessionDigestArgs, SessionEndArgs};
use crate::ops::{SessionStartArgs, ThinkArgs};
use crate::output;
use crate::secrets;
use gate::Gate;
use state::{Kind, Store};

const MIN_PROMPT_CHARS: usize = 24;
const QUERY_CHARS: usize = 400;
const CANDIDATES: u32 = 10;
const CAPTURE_LINES: usize = 40;
const CAPTURE_CHARS: usize = 6000;
/// Per-request timeout for capture and session end; the whole capture is bounded below.
const CAPTURE_REQUEST: Duration = Duration::from_secs(10);
const CAPTURE_BUDGET: Duration = Duration::from_secs(45);
/// A line whose best stored match is at least this similar is already in memory.
const DUPLICATE_SIMILARITY: f64 = 0.92;

/// Lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Event {
    /// Inject the briefing once per session and open a tracked server session.
    SessionStart,
    /// Inject relevant memories not shown yet in this session.
    Prompt,
    /// Capture user turns before compaction (when `YREM_HOOK_CAPTURE=1`).
    PreCompact,
    /// Capture, close the tracked session and run `think`.
    SessionEnd,
}

/// Output format of the calling harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    /// Claude Code hook JSON.
    Claude,
    /// Codex hook JSON.
    Codex,
    /// Plain context text, for plugins that inject it themselves.
    Plain,
}

impl Harness {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Plain => "plain",
        }
    }

    /// Hook output cap in bytes: Claude Code's 10,000 characters, Codex's documented 8,000.
    fn limit(self) -> usize {
        match self {
            Self::Codex => 8_000,
            Self::Claude | Self::Plain => 10_000,
        }
    }
}

/// The event JSON on stdin. Claude Code and Codex send these fields as they are.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub session_id: String,
    pub prompt: Option<String>,
    /// `startup`, `resume`, `clear` or `compact`.
    pub source: Option<String>,
    pub transcript_path: Option<String>,
    /// User turns, for harnesses without a transcript file.
    pub turns: Vec<Turn>,
}

/// One conversation turn.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Turn {
    pub role: String,
    pub text: String,
}

impl Input {
    /// Reads the event from stdin; empty when stdin is a terminal or not JSON.
    #[must_use]
    pub fn from_stdin() -> (Self, String) {
        let mut raw = String::new();
        if !std::io::stdin().is_terminal() {
            let _ = std::io::stdin().read_to_string(&mut raw);
        }
        (serde_json::from_str(&raw).unwrap_or_default(), raw)
    }
}

/// `YREM_HOOK_*` settings.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools, reason = "independent on/off switches")]
pub struct Settings {
    pub namespace: Option<String>,
    pub digest: bool,
    pub decisions: usize,
    pub maintenance: bool,
    pub recall: bool,
    pub top_k: usize,
    pub gate: Gate,
    pub capture: bool,
    pub think: bool,
    pub track_session: bool,
}

impl Settings {
    #[must_use]
    pub fn from_env(env: &impl Fn(&str) -> Option<String>) -> Self {
        let get = |k: &str| {
            env(&format!("YREM_HOOK_{k}"))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let flag = |k: &str, default: bool| {
            get(k).map_or(default, |v| {
                !matches!(
                    v.to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
        };
        let num = |k: &str, default: usize| get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
        let capture = flag("CAPTURE", false);
        Self {
            namespace: get("NAMESPACE"),
            digest: flag("DIGEST", true),
            decisions: num("DECISIONS", 5),
            maintenance: flag("MAINTENANCE", false),
            recall: flag("RECALL", true),
            top_k: num("TOP_K", 3),
            gate: Gate {
                min_similarity: get("MIN_SIMILARITY")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.50),
                include_captured: capture,
                excluded_namespaces: get("EXCLUDE_NAMESPACES")
                    .map(|v| {
                        v.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            capture,
            think: flag("THINK", true),
            track_session: flag("TRACK_SESSION", true),
        }
    }
}

/// Runs one hook. Never fails: diagnostics go to stderr.
pub async fn run(overrides: &Overrides, event: Event, harness: Harness, detach: bool) {
    let (input, raw) = Input::from_stdin();
    if detach {
        if let Err(e) = spawn_detached(overrides, event, harness, &raw) {
            eprintln!("yrem hook: cannot detach: {e}");
        }
        return;
    }
    let env = |k: &str| std::env::var(k).ok();
    let Some(store) = Store::from_env(&env) else {
        eprintln!("yrem hook: no state directory (set YREM_HOOK_STATE_DIR)");
        return;
    };
    let settings = Settings::from_env(&env);
    let hook = Hook {
        overrides,
        harness,
        settings,
        store,
        input,
    };
    match hook.handle(event).await {
        Ok(Some(text)) => output::print_raw(&hook.format(event, &text)),
        Ok(None) => {}
        Err(e) => {
            if matches!(e.downcast_ref::<Error>(), Some(Error::Unavailable(_))) {
                hook.store.mark_down();
            }
            eprintln!("yrem hook: {e:#}");
        }
    }
}

struct Hook<'a> {
    overrides: &'a Overrides,
    harness: Harness,
    settings: Settings,
    store: Store,
    input: Input,
}

impl Hook<'_> {
    fn format(&self, event: Event, text: &str) -> String {
        let name = match event {
            Event::SessionStart => "SessionStart",
            Event::Prompt => "UserPromptSubmit",
            Event::PreCompact => "PreCompact",
            Event::SessionEnd => "SessionEnd",
        };
        match self.harness {
            Harness::Plain => text[..text.floor_char_boundary(self.harness.limit())].to_owned(),
            Harness::Claude | Harness::Codex => render::envelope(name, text, self.harness.limit()),
        }
    }

    fn app(&self, total: Duration) -> anyhow::Result<App> {
        let config = Config::load(self.overrides)?;
        Ok(App::new(Client::with_timeouts(
            &config,
            Duration::from_secs(2),
            total,
        )?))
    }

    fn session(&self) -> &str {
        &self.input.session_id
    }

    async fn handle(&self, event: Event) -> anyhow::Result<Option<String>> {
        if self.store.server_down() {
            return Ok(None);
        }
        match event {
            Event::SessionStart => self.session_start().await,
            Event::Prompt => self.prompt().await,
            Event::PreCompact => {
                self.capture(&self.app(CAPTURE_REQUEST)?).await?;
                Ok(None)
            }
            Event::SessionEnd => self.session_end().await.map(|()| None),
        }
    }

    async fn session_start(&self) -> anyhow::Result<Option<String>> {
        self.store.sweep();
        let app = self.app(Duration::from_secs(8))?;
        let track = async {
            if self.settings.track_session {
                self.track(&app).await;
            }
        };
        let (text, ()) = tokio::join!(self.briefing(&app), track);
        text
    }

    async fn briefing(&self, app: &App) -> anyhow::Result<Option<String>> {
        let s = &self.settings;
        if !s.digest
            || !self
                .store
                .claim_digest(self.session(), self.input.source.as_deref())
        {
            return Ok(None);
        }
        let args = SessionDigestArgs {
            namespace: s.namespace.clone(),
            include_gaps: s.maintenance,
        };
        let digest = match app.session_digest(args).await {
            Ok(d) => d,
            Err(e) => {
                self.store.release_digest(self.session());
                return Err(e.into());
            }
        };
        let excluded = &s.gate.excluded_namespaces;
        let text = render::digest_text(&digest, s.decisions, s.maintenance, excluded);
        for d in render::live_decisions(&digest, s.decisions, excluded) {
            if let Some(rid) = d.get("rid").and_then(Value::as_str) {
                let _ = self.store.claim(self.session(), Kind::Seen, rid);
            }
        }
        self.store.finish_digest(self.session());
        Ok(text)
    }

    /// Opens a tracked server session, keyed by the harness session so each owns its slot.
    async fn track(&self, app: &App) {
        if !self.store.claim_tracking(self.session()) {
            return;
        }
        let base = format!("{}-{}", self.harness.name(), self.session());
        // The id is generated by the harness, not user data; OpenCode's ids trip the
        // high-entropy secret detector.
        let start = |client_id: String| {
            app.session_start(SessionStartArgs {
                namespace: self.settings.namespace.clone(),
                client_id: Some(client_id),
                force: true,
            })
        };
        let mut resp = start(base.clone()).await;
        // A resumed session whose previous run never ended still holds the slot.
        if let Err(Error::Http { status: 409, .. }) = resp {
            resp = start(format!("{base}-{}", std::process::id())).await;
        }
        match resp.map(|v| {
            v.get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }) {
            Ok(Some(id)) => self.store.set_server_session(self.session(), &id),
            Ok(None) => self.store.release_tracking(self.session()),
            Err(e) => {
                self.store.release_tracking(self.session());
                eprintln!("yrem hook: session start: {e}");
            }
        }
    }

    async fn prompt(&self) -> anyhow::Result<Option<String>> {
        let s = &self.settings;
        let prompt = self.input.prompt.as_deref().unwrap_or_default().trim();
        if !s.recall || prompt.chars().count() < MIN_PROMPT_CHARS || prompt.starts_with('/') {
            return Ok(None);
        }
        let app = self.app(Duration::from_secs(5))?;
        let resp = app
            .recall(RecallArgs {
                query: render::clip(prompt, QUERY_CHARS),
                top_k: Some(CANDIDATES),
                namespace: s.namespace.clone(),
                ..RecallArgs::default()
            })
            .await?;
        // Claiming each rid makes concurrent prompt hooks split the hits instead of
        // injecting the same one twice.
        let hits: Vec<&Value> = resp
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|h| s.gate.reject(h).is_none())
            .filter(|h| {
                let rid = h.get("rid").and_then(Value::as_str).unwrap_or_default();
                !self.store.has(self.session(), Kind::Seen, rid)
            })
            .take(s.top_k)
            .filter(|h| {
                let rid = h.get("rid").and_then(Value::as_str).unwrap_or_default();
                self.store.claim(self.session(), Kind::Seen, rid)
            })
            .collect();
        Ok(render::recall_text(&hits))
    }

    async fn session_end(&self) -> anyhow::Result<()> {
        let app = self.app(CAPTURE_REQUEST)?;
        if let Err(e) = self.capture(&app).await {
            eprintln!("yrem hook: capture: {e:#}");
        }
        if let Some(id) = self.store.take_server_session(self.session()) {
            // No summary: the server would draft memories from it, a second capture path.
            let end = app.session_end(SessionEndArgs {
                session_id: id,
                summary: None,
                force: false,
            });
            if let Err(e) = end.await {
                eprintln!("yrem hook: session end: {e}");
            }
        }
        if self.settings.think {
            app.think(ThinkArgs {}).await?;
        }
        Ok(())
    }

    /// Sends the newest user lines not captured before and not already in memory to
    /// `session_capture`.
    ///
    /// A line is claimed (watermarked) only once it is sent, found stored, or can never be
    /// sent (oversized, looks like a secret). Lines left out by the size cap or the deadline
    /// stay unclaimed for the next run.
    async fn capture(&self, app: &App) -> anyhow::Result<()> {
        if !self.settings.capture {
            return Ok(());
        }
        let sid = self.session();
        let deadline = Instant::now() + CAPTURE_BUDGET;
        let fresh: Vec<(String, String)> = self
            .user_lines()
            .into_iter()
            .rev()
            .take(CAPTURE_LINES)
            .map(|l| (format!("{:016x}", fnv1a(&l)), l))
            .filter(|(h, _)| !self.store.has(sid, Kind::Captured, h))
            .collect();
        let mut picked: Vec<(String, String)> = Vec::new();
        let mut size = 0;
        for (hash, line) in fresh {
            if line.len() >= CAPTURE_CHARS
                || !secrets::scan(&Value::String(line.clone())).is_empty()
            {
                let _ = self.store.claim(sid, Kind::Captured, &hash);
                continue;
            }
            if size + line.len() >= CAPTURE_CHARS || Instant::now() >= deadline {
                break;
            }
            if self.already_stored(app, &line).await? {
                let _ = self.store.claim(sid, Kind::Captured, &hash);
            } else if self.store.claim(sid, Kind::Captured, &hash) {
                size += line.len() + 1;
                picked.push((hash, line));
            }
        }
        if picked.is_empty() {
            return Ok(());
        }
        let summary = picked.iter().rev().fold(String::new(), |mut acc, (_, l)| {
            acc.push_str(l);
            acc.push('\n');
            acc
        });
        let sent = app
            .session_capture(SessionCaptureArgs {
                summary,
                namespace: self.settings.namespace.clone(),
                ..SessionCaptureArgs::default()
            })
            .await;
        if sent.is_err() {
            for (hash, _) in &picked {
                self.store.release(sid, Kind::Captured, hash);
            }
        }
        sent.map(drop).map_err(Into::into)
    }

    fn user_lines(&self) -> Vec<String> {
        match &self.input.transcript_path {
            Some(p) if !p.is_empty() => transcript::user_lines(Path::new(p)),
            _ => self
                .input
                .turns
                .iter()
                .filter(|t| t.role == "user")
                .filter_map(|t| transcript::clean(&t.text))
                .collect(),
        }
    }

    /// Whether `line` is already in memory. An unreachable server stops the capture; any
    /// other failure counts as "not stored".
    async fn already_stored(&self, app: &App, line: &str) -> crate::Result<bool> {
        let resp = app
            .recall(RecallArgs {
                query: render::clip(line, QUERY_CHARS),
                top_k: Some(1),
                namespace: self.settings.namespace.clone(),
                ..RecallArgs::default()
            })
            .await;
        match resp {
            Ok(resp) => Ok(resp
                .get("results")
                .and_then(Value::as_array)
                .and_then(|r| r.first())
                .and_then(gate::similarity)
                .is_some_and(|s| s >= DUPLICATE_SIMILARITY)),
            Err(e @ Error::Unavailable(_)) => Err(e),
            Err(_) => Ok(false),
        }
    }
}

/// Stable across builds, unlike `DefaultHasher`, so watermarks survive an upgrade.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Re-runs this hook in the background and returns, for hosts that clamp handler time.
#[allow(clippy::zombie_processes, reason = "the parent exits right away")]
fn spawn_detached(
    overrides: &Overrides,
    event: Event,
    harness: Harness,
    input: &str,
) -> std::io::Result<()> {
    let event = event
        .to_possible_value()
        .map(|v| v.get_name().to_owned())
        .unwrap_or_default();
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["hook", &event, "--harness", harness.name()]);
    if let Some(c) = &overrides.config {
        cmd.arg("--config").arg(c);
    }
    if let Some(u) = &overrides.url {
        cmd.env("YREM_URL", u);
    }
    if let Some(t) = &overrides.token {
        cmd.env("YREM_TOKEN", t);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }
    Ok(())
}

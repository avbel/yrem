//! Hooks: `metadata.harness` on captured lines, and the `home` project of sessions started in
//! the home directory.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};

const BIN: &str = env!("CARGO_BIN_EXE_yrem");
const USER_LINE: &str = "The billing service runs on PostgreSQL 17";

struct Env {
    dir: PathBuf,
    url: String,
    vars: Vec<(String, String)>,
}

impl Env {
    fn new(url: &str, name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("yrem-hh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            url: url.to_owned(),
            vars: vec![
                ("YREM_HOOK_TRACK_SESSION".into(), "0".into()),
                ("YREM_HOOK_THINK".into(), "0".into()),
                ("YREM_HOOK_MAINTENANCE".into(), "0".into()),
                ("YREM_HOOK_CAPTURE".into(), "1".into()),
                ("YREM_OUTBOX_DIR".into(), "off".into()),
            ],
            dir,
        }
    }

    fn set(mut self, k: &str, v: &str) -> Self {
        self.vars.retain(|(key, _)| key != k);
        self.vars.push((k.to_owned(), v.to_owned()));
        self
    }

    fn home(&self) -> PathBuf {
        let home = self.dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    fn hook(&self, event: &str, harness: &str, stdin: &Value) -> String {
        let mut c = Command::new(BIN);
        for (k, _) in std::env::vars() {
            if k.starts_with("YREM_")
                || k.starts_with("YANTRIKDB_")
                || k.starts_with("CODEX_")
                || k == "CLAUDE_CODE_ENTRYPOINT"
            {
                c.env_remove(k);
            }
        }
        c.env("XDG_CONFIG_HOME", "/nonexistent")
            .env("YREM_URL", &self.url)
            .env("YREM_TOKEN", TOKEN)
            .env("YREM_HOOK_STATE_DIR", self.dir.join("state"))
            .envs(self.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .args(["hook", event, "--harness", harness])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "hooks always exit 0");
        String::from_utf8(out.stdout).unwrap()
    }

    fn transcript(&self) -> PathBuf {
        let path = self.dir.join("transcript.jsonl");
        let line = json!({"type": "user", "message": {"role": "user", "content": USER_LINE}});
        std::fs::write(&path, line.to_string()).unwrap();
        path
    }

    fn capture(&self, harness: &str, cwd: &Path) {
        let ev = json!({"session_id": "s1", "cwd": cwd, "transcript_path": self.transcript()});
        self.hook("pre-compact", harness, &ev);
    }
}

fn capture_steps() -> Vec<Step> {
    vec![
        Step::post("/v1/recall").json(json!({"results": []})),
        Step::post("/v1/session/end").json(json!({"count": 1})),
    ]
}

fn captured(srv: &FakeServer) -> Value {
    srv.assert_done();
    srv.requests().last().unwrap().body.clone()
}

#[test]
fn capture_is_stamped_with_the_hook_harness() {
    for harness in ["claude", "codex", "antigravity"] {
        let srv = FakeServer::start(capture_steps());
        let env = Env::new(&srv.url, harness);
        env.capture(harness, &env.dir);
        assert_eq!(captured(&srv)["metadata"], json!({"harness": harness}));
    }
}

#[test]
fn plain_plugins_name_themselves_through_the_environment() {
    let srv = FakeServer::start(capture_steps());
    let env = Env::new(&srv.url, "plain-env").set("YREM_HARNESS", "opencode");
    env.capture("plain", &env.dir);
    assert_eq!(captured(&srv)["metadata"], json!({"harness": "opencode"}));
}

#[test]
fn plain_without_a_name_sends_no_metadata() {
    let srv = FakeServer::start(capture_steps());
    let env = Env::new(&srv.url, "plain-bare");
    env.capture("plain", &env.dir);
    assert!(captured(&srv).get("metadata").is_none());
}

#[test]
fn capture_from_the_home_directory_goes_to_home() {
    let srv = FakeServer::start(capture_steps());
    let env = Env::new(&srv.url, "home-capture");
    let home = env.home();
    let env = env.set("HOME", home.to_str().unwrap());
    env.capture("claude", &home);
    assert_eq!(captured(&srv)["namespace"], "home");
}

#[test]
fn the_home_namespace_can_be_renamed() {
    let srv = FakeServer::start(capture_steps());
    let env = Env::new(&srv.url, "home-renamed");
    let home = env.home();
    let env = env
        .set("HOME", home.to_str().unwrap())
        .set("YREM_HOME_NAMESPACE", "personal");
    env.capture("claude", &home);
    assert_eq!(captured(&srv)["namespace"], "personal");
}

#[test]
fn a_directory_below_home_has_no_project() {
    let srv = FakeServer::start(capture_steps());
    let env = Env::new(&srv.url, "home-below");
    let home = env.home();
    let below = home.join("notes");
    std::fs::create_dir_all(&below).unwrap();
    let env = env.set("HOME", home.to_str().unwrap());
    env.capture("claude", &below);
    assert!(captured(&srv).get("namespace").is_none());
}

#[test]
fn a_home_session_is_briefed_and_tracked_in_the_home_namespace() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest?namespace=home").json(json!({})),
        Step::get("/v1/memories?limit=100&order=desc").json(json!({"records": []})),
        Step::get("/v1/memories?limit=50&namespace=home&order=desc").json(json!({
            "records": [{"rid": "h1", "text": "The user keeps dotfiles in a bare repository",
                         "namespace": "home", "source": "user", "created_at": 1.0e12,
                         "metadata": {}}],
        })),
        Step::post("/v1/sessions")
            .body(|b| b["namespace"] == "home")
            .json(json!({"session_id": "srv-1"})),
    ]);
    let env = Env::new(&srv.url, "home-brief").set("YREM_HOOK_TRACK_SESSION", "1");
    let home = env.home();
    let env = env.set("HOME", home.to_str().unwrap());
    let ev = json!({"session_id": "s1", "cwd": home});
    let out = env.hook("session-start", "claude", &ev);
    assert!(out.contains("dotfiles in a bare repository"), "{out}");
    srv.assert_done();
}

#[test]
fn outside_home_and_git_there_is_still_no_briefing() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "no-project");
    let home = env.home();
    let elsewhere = env.dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let env = env.set("HOME", home.to_str().unwrap());
    let ev = json!({"session_id": "s1", "cwd": elsewhere});
    assert_eq!(env.hook("session-start", "claude", &ev), "");
    srv.assert_done();
}

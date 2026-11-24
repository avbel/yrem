//! `yrem hook --harness antigravity` against the fake server: Antigravity's camelCase input,
//! its `injectSteps` output, `{}` whenever there is nothing to inject, and the
//! `PreInvocation` gate that keeps recall to the first model call of a user turn.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};

const BIN: &str = env!("CARGO_BIN_EXE_yrem");
const PROMPT: &str = "which database does the payments service use now?";

struct Env {
    dir: PathBuf,
    url: String,
    vars: Vec<(&'static str, String)>,
}

impl Env {
    fn new(url: &str, name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("yrem-agy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self {
            url: url.to_owned(),
            vars: vec![
                ("YREM_HOOK_TRACK_SESSION", "0".into()),
                ("YREM_HOOK_THINK", "0".into()),
                ("YREM_HOOK_MAINTENANCE", "0".into()),
                ("YREM_OUTBOX_DIR", dir.join("outbox").display().to_string()),
            ],
            dir,
        }
    }

    fn set(mut self, k: &'static str, v: &str) -> Self {
        self.vars.retain(|(key, _)| *key != k);
        self.vars.push((k, v.to_owned()));
        self
    }

    fn hook(&self, args: &[&str], stdin: &Value) -> String {
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
            .env("YREM_HOOK_STATE_DIR", &self.dir)
            .envs(self.vars.iter().map(|(k, v)| (*k, v.as_str())))
            .arg("hook")
            .args(args)
            .args(["--harness", "antigravity"])
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
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    }

    fn repo(&self, name: &str) -> PathBuf {
        let dir = self.dir.join("repos").join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }
}

/// The text of the single system message an Antigravity hook injects.
fn injected(out: &str) -> String {
    let v: Value = serde_json::from_str(out).unwrap_or_else(|e| panic!("{e}: {out:?}"));
    let steps = v["injectSteps"].as_array().unwrap();
    assert_eq!(steps.len(), 1, "{out}");
    assert!(steps[0].get("userMessage").is_none(), "{out}");
    steps[0]["systemMessage"]["systemMessage"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn hit(rid: &str, text: &str, sim: f64) -> Value {
    json!({
        "rid": rid, "text": text, "score": 0.5, "namespace": "default", "source": "user",
        "metadata": {}, "why_retrieved": [format!("semantically similar ({sim})")],
    })
}

/// Antigravity's common input fields. Hooks run in the plugin's directory, so `cwd` is never
/// the project.
fn event(conversation: &str, workspace: &Path, extra: &Value) -> Value {
    let mut ev = json!({
        "conversationId": conversation,
        "workspacePaths": [workspace],
        "cwd": "/Users/me/.gemini/config/plugins/yrem",
        "modelName": "gemini-3.6-flash-medium",
    });
    ev.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    ev
}

#[test]
fn session_start_briefs_the_first_workspace_as_one_system_message() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest?namespace=fluidd").json(json!({})),
        Step::get("/v1/memories?limit=100&order=desc").json(json!({"records": []})),
        Step::get("/v1/memories?limit=50&namespace=fluidd&order=desc").json(json!({
            "records": [{"rid": "p1", "text": "Fluidd valuation rule", "namespace": "fluidd",
                         "source": "user", "created_at": 1.0e12, "metadata": {}}],
        })),
    ]);
    let env = Env::new(&srv.url, "start").set("YREM_HOOK_TRACK_SESSION", "1");
    let ev = event("conv-1", &env.repo("fluidd"), &json!({}));
    let text = injected(&env.hook(&["session-start"], &ev));
    assert!(text.contains("Fluidd valuation rule"), "{text}");
    assert_eq!(
        env.hook(&["session-start"], &ev),
        "{}",
        "the digest is shown once per conversation"
    );
    srv.assert_done();
    assert!(
        !srv.calls().iter().any(|c| c.contains("/v1/session/start")),
        "no session-end event would ever close it: {:?}",
        srv.calls()
    );
}

#[test]
fn recall_runs_only_on_the_first_model_call_of_a_turn() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .body(|b| b["query"] == PROMPT)
            .json(json!({"results": [hit("good", "Payments uses PostgreSQL", 0.91)]})),
    ]);
    let env = Env::new(&srv.url, "recall");
    let ws = env.repo("payments");
    let wrapped = format!(
        "<USER_REQUEST>\n{PROMPT}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local \
         time is: 2026-11-24T15:07:56+04:00.\n</ADDITIONAL_METADATA>"
    );
    let later = event(
        "conv-2",
        &ws,
        &json!({"invocationNum": 3, "lastUserInput": wrapped}),
    );
    assert_eq!(env.hook(&["prompt"], &later), "{}");

    let first = event(
        "conv-2",
        &ws,
        &json!({"invocationNum": 0, "lastUserInput": wrapped}),
    );
    let text = injected(&env.hook(&["prompt"], &first));
    assert!(text.ends_with("\n- Payments uses PostgreSQL"), "{text}");
    srv.assert_done();
}

#[test]
fn nothing_to_inject_and_a_dead_server_still_print_an_empty_object() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(json!({"results": []}))]);
    let env = Env::new(&srv.url, "empty");
    let ev = event(
        "conv-3",
        &env.repo("x"),
        &json!({"invocationNum": 0, "lastUserInput": PROMPT}),
    );
    assert_eq!(env.hook(&["prompt"], &ev), "{}");
    srv.assert_done();

    let dead = Env::new("http://127.0.0.1:9", "dead");
    assert_eq!(dead.hook(&["prompt"], &ev), "{}");
    assert_eq!(
        dead.hook(&["pre-compact", "--detach"], &ev),
        "{}",
        "Stop runs detached and must still answer"
    );
}

#[test]
fn stop_captures_typed_lines_from_the_antigravity_transcript() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(json!({"results": []})),
        Step::post("/v1/session/end")
            .body(|b| b["summary"] == "The billing service runs on PostgreSQL 17.\n")
            .json(json!({"count": 1})),
    ]);
    let env = Env::new(&srv.url, "capture").set("YREM_HOOK_CAPTURE", "1");
    let record = |source: &str, kind: &str, content: &str| {
        json!({"step_index": 0, "source": source, "type": kind, "status": "DONE",
               "content": content})
    };
    let lines = [
        record(
            "USER_EXPLICIT",
            "USER_INPUT",
            "<USER_REQUEST>\nThe billing service runs on PostgreSQL 17\n</USER_REQUEST>\n\
             <ADDITIONAL_METADATA>\nThe current local time is: 2026-11-24.\n</ADDITIONAL_METADATA>\n\
             <USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection`.\n</USER_SETTINGS_CHANGE>",
        ),
        record(
            "SYSTEM_SDK",
            "SYSTEM_MESSAGE",
            "The following system message was injected by a PreInvocation hook: Payments uses MySQL",
        ),
        record("MODEL", "PLANNER_RESPONSE", "I think billing runs on MySQL"),
    ];
    std::fs::create_dir_all(&env.dir).unwrap();
    let path = env.dir.join("transcript_full.jsonl");
    let body: Vec<String> = lines.iter().map(Value::to_string).collect();
    std::fs::write(&path, body.join("\n")).unwrap();
    let ev = event(
        "conv-4",
        &env.repo("billing"),
        &json!({"transcriptPath": path, "fullyIdle": true}),
    );
    assert_eq!(env.hook(&["pre-compact"], &ev), "{}");
    srv.assert_done();
}

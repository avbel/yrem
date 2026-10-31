//! `yrem hook <event>` end to end against the fake server: injection, dedup across hook
//! processes, capture watermarks and failure isolation.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};

const BIN: &str = env!("CARGO_BIN_EXE_yrem");

#[derive(Clone)]
struct Env {
    dir: PathBuf,
    url: String,
    vars: Vec<(&'static str, String)>,
}

impl Env {
    fn new(url: &str, name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("yrem-hook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self {
            dir,
            url: url.to_owned(),
            vars: vec![
                ("YREM_HOOK_TRACK_SESSION", "0".into()),
                ("YREM_HOOK_THINK", "0".into()),
            ],
        }
    }

    fn set(mut self, k: &'static str, v: &str) -> Self {
        self.vars.retain(|(key, _)| *key != k);
        self.vars.push((k, v.to_owned()));
        self
    }

    fn run(&self, args: &[&str], stdin: &Value) -> (String, String) {
        let mut c = Command::new(BIN);
        for (k, _) in std::env::vars() {
            if k.starts_with("YREM_") || k.starts_with("YANTRIKDB_") {
                c.env_remove(k);
            }
        }
        c.env("XDG_CONFIG_HOME", "/nonexistent")
            .env("YREM_URL", &self.url)
            .env("YREM_TOKEN", TOKEN)
            .env("YREM_HOOK_STATE_DIR", &self.dir)
            .envs(self.vars.iter().map(|(k, v)| (*k, v.as_str())))
            .args(args)
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
        (
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    }

    fn hook(&self, event: &str, harness: &str, stdin: &Value) -> String {
        self.run(&["hook", event, "--harness", harness], stdin).0
    }

    /// The on-disk state as `{seen, captured, server_session}`; `null` when absent.
    fn state(&self, session: &str) -> Value {
        let dir = self.dir.join(session);
        if !dir.exists() {
            return Value::Null;
        }
        let names = |sub: &str| {
            let mut v: Vec<String> = std::fs::read_dir(dir.join(sub))
                .map(|d| {
                    d.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        };
        let session = std::fs::read_to_string(dir.join("tracking"))
            .ok()
            .filter(|s| !s.is_empty());
        json!({"seen": names("seen"), "captured": names("captured"), "server_session": session})
    }
}

fn context(out: &str, event: &str) -> String {
    let v: Value = serde_json::from_str(out).unwrap_or_else(|e| panic!("{e}: {out:?}"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], event);
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn digest() -> Value {
    json!({
        "narrative_head": {"text": "Payments moved to PostgreSQL last week."},
        "open_conflict_count": 2,
        "pending_trigger_count": 6,
        "knowledge_gaps": [{"query": "quantum gardening"}],
        "top_decisions": [
            {"rid": "d1", "snippet": "Payments uses PostgreSQL", "current_status": "active",
             "created_at": 1_793_392_814.9},
            {"rid": "d2", "snippet": "Payments uses MySQL", "current_status": "superseded"},
        ],
    })
}

fn hit(rid: &str, text: &str, sim: f64) -> Value {
    json!({
        "rid": rid, "text": text, "score": 0.5, "namespace": "default", "source": "user",
        "metadata": {}, "why_retrieved": [format!("semantically similar ({sim})"), "recent"],
    })
}

fn recall(hits: &[Value]) -> Value {
    json!({ "results": hits })
}

const PROMPT: &str = "which database does the payments service use now?";

#[test]
fn session_start_injects_text_briefing_once() {
    let srv = FakeServer::start(vec![Step::get("/v1/session/digest").json(digest())]);
    let env = Env::new(&srv.url, "once");
    let ev = json!({"session_id": "s1", "source": "startup"});
    let ctx = context(&env.hook("session-start", "claude", &ev), "SessionStart");
    assert!(ctx.contains("do not call session_digest"), "{ctx}");
    assert!(ctx.contains("Where things stood: Payments moved to PostgreSQL"));
    assert!(
        ctx.contains("- [2026-10-30] Payments uses PostgreSQL"),
        "{ctx}"
    );
    assert!(!ctx.contains("MySQL"), "superseded decisions are dropped");
    assert!(!ctx.contains("conflicts"), "maintenance is off by default");
    assert_eq!(env.state("s1")["seen"], json!(["d1"]));

    assert_eq!(
        env.hook("session-start", "codex", &ev),
        "",
        "a second plugin stays silent"
    );
    srv.assert_done();
}

#[test]
fn compact_reinjects_after_the_duplicate_window() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/session/digest").json(digest()),
        Step::get("/v1/session/digest?include_gaps=true").json(digest()),
    ]);
    let env = Env::new(&srv.url, "compact");
    let start = json!({"session_id": "s3", "source": "startup"});
    let compact = json!({"session_id": "s3", "source": "compact"});
    assert_ne!(env.hook("session-start", "plain", &start), "");
    assert_eq!(
        env.hook("session-start", "plain", &compact),
        "",
        "same event burst"
    );

    let claim = std::fs::File::options()
        .write(true)
        .open(env.dir.join("s3/digest"))
        .unwrap();
    claim
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    assert_eq!(
        env.hook("session-start", "plain", &start),
        "",
        "resume keeps the briefing"
    );
    let env = env.set("YREM_HOOK_MAINTENANCE", "1");
    let text = env.hook("session-start", "plain", &compact);
    assert!(text.starts_with("Memory briefing"), "{text}");
    assert!(text.contains("Open conflicts: 2"));
    assert!(text.contains("Pending triggers: 6."));
    assert!(text.contains("Known gaps: quantum gardening."));
    srv.assert_done();
}

#[test]
fn failed_digest_releases_the_claim() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/session/digest").status(500).raw("boom"),
        Step::get("/v1/session/digest").json(json!({})),
    ]);
    let env = Env::new(&srv.url, "release");
    let ev = json!({"session_id": "s4"});
    let (out, err) = env.run(&["hook", "session-start"], &ev);
    assert_eq!(out, "");
    assert!(err.contains("yrem hook"), "{err}");
    assert_eq!(
        env.hook("session-start", "plain", &ev),
        "",
        "empty digest: nothing to say"
    );
    srv.assert_done();
}

#[test]
fn tracked_session_opens_once_and_retries_a_held_slot() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/sessions")
            .body(|b| b["client_id"] == "codex-s5")
            .status(409)
            .json(json!({"error": "session conflict"})),
        Step::post("/v1/sessions")
            .body(|b| b["client_id"].as_str().unwrap().starts_with("codex-s5-"))
            .json(json!({"session_id": "srv-1"})),
        Step::new("DELETE", "/v1/sessions/srv-1").json(json!({})),
        Step::post("/v1/think").json(json!({})),
    ]);
    let env = Env::new(&srv.url, "track")
        .set("YREM_HOOK_TRACK_SESSION", "1")
        .set("YREM_HOOK_DIGEST", "0");
    let ev = json!({"session_id": "s5"});
    assert_eq!(env.hook("session-start", "codex", &ev), "");
    assert_eq!(env.state("s5")["server_session"], "srv-1");
    assert_eq!(
        env.hook("session-start", "codex", &ev),
        "",
        "already tracked"
    );
    let env = env.set("YREM_HOOK_THINK", "1");
    assert_eq!(env.hook("session-end", "codex", &ev), "");
    assert_eq!(env.state("s5")["server_session"], Value::Null);
    srv.assert_done();
}

#[test]
fn tracking_failure_is_logged_not_fatal() {
    let srv = FakeServer::start(vec![Step::post("/v1/sessions").status(500).raw("no")]);
    let env = Env::new(&srv.url, "trackfail")
        .set("YREM_HOOK_TRACK_SESSION", "1")
        .set("YREM_HOOK_DIGEST", "0");
    let (out, err) = env.run(&["hook", "session-start"], &json!({"session_id": "s6"}));
    assert_eq!(out, "");
    assert!(err.contains("session start"), "{err}");
    srv.assert_done();
}

#[test]
fn prompt_injects_gated_unseen_hits() {
    let merge = json!({
        "rid": "m", "text": "commit and push | fix it", "score": 0.9, "namespace": "default",
        "metadata": "{\"consolidated_from\": [\"a\", \"b\"]}",
        "why_retrieved": ["semantically similar (0.95)"],
    });
    let hits = vec![
        hit("weak", "Payments has a dashboard", 0.31),
        json!({"rid": "kw", "text": "keyword only", "score": 0.9, "why_retrieved": ["keyword_match"]}),
        json!({"rid": "cap", "text": "captured prompt", "score": 0.9, "source": "session_auto_capture",
               "why_retrieved": ["semantically similar (0.99)"]}),
        json!({"rid": "ns", "text": "other agent chat", "score": 0.9, "namespace": "hermes:chat",
               "why_retrieved": ["semantically similar (0.99)"]}),
        json!({"rid": "low", "text": "low score", "score": 0.01,
               "why_retrieved": ["semantically similar (0.99)"]}),
        merge,
        hit("d1", "Decision already in the briefing", 0.99),
        hit("good", "Payments uses PostgreSQL", 0.91),
        hit("second", "Payments DB is managed by Alice", 0.8),
    ];
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .body(|b| b["query"] == PROMPT && b["top_k"] == 10)
            .json(recall(&hits)),
        Step::post("/v1/recall").json(recall(&hits)),
    ]);
    let env = Env::new(&srv.url, "prompt")
        .set("YREM_HOOK_EXCLUDE_NAMESPACES", "hermes:, ")
        .set("YREM_HOOK_TOP_K", "1");
    std::fs::create_dir_all(&env.dir).unwrap();
    std::fs::create_dir_all(env.dir.join("s7/seen")).unwrap();
    std::fs::write(env.dir.join("s7/seen/d1"), "").unwrap();
    let ev = json!({"session_id": "s7", "prompt": PROMPT});
    let ctx = context(&env.hook("prompt", "claude", &ev), "UserPromptSubmit");
    assert!(
        ctx.starts_with("Memories relevant to this message"),
        "{ctx}"
    );
    assert!(ctx.ends_with("\n- Payments uses PostgreSQL"), "{ctx}");

    let text = env.hook("prompt", "plain", &ev);
    assert!(
        text.trim_end()
            .ends_with("\n- Payments DB is managed by Alice"),
        "{text}"
    );
    assert_eq!(env.state("s7")["seen"], json!(["d1", "good", "second"]));
    srv.assert_done();
}

#[test]
fn prompt_skips_short_slash_and_disabled() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "skip");
    for p in ["short one", "/commit and push everything to the remote"] {
        assert_eq!(
            env.hook("prompt", "claude", &json!({"session_id": "s", "prompt": p})),
            ""
        );
    }
    let env = env.set("YREM_HOOK_RECALL", "off");
    assert_eq!(
        env.hook(
            "prompt",
            "claude",
            &json!({"session_id": "s", "prompt": PROMPT})
        ),
        ""
    );
    assert_eq!(env.hook("prompt", "claude", &json!("not an object")), "");
    srv.assert_done();
}

#[test]
fn prompt_without_relevant_hits_prints_nothing() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&[]))]);
    let env = Env::new(&srv.url, "none");
    assert_eq!(env.hook("prompt", "codex", &json!({"prompt": PROMPT})), "");
    srv.assert_done();
}

#[test]
fn codex_output_respects_its_smaller_cap() {
    let long = "word ".repeat(80);
    let hits: Vec<Value> = (0..40).map(|i| hit(&format!("r{i}"), &long, 0.9)).collect();
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&hits))]);
    let env = Env::new(&srv.url, "cap").set("YREM_HOOK_TOP_K", "40");
    let out = env.hook(
        "prompt",
        "codex",
        &json!({"session_id": "s8", "prompt": PROMPT}),
    );
    assert!(out.trim_end().len() <= 8_000, "{}", out.len());
    assert!(context(&out, "UserPromptSubmit").ends_with("…[truncated]"));
    srv.assert_done();
}

#[test]
fn unreachable_server_is_skipped_for_a_while() {
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let env = Env::new(&dead, "down");
    let ev = json!({"session_id": "s9", "prompt": PROMPT});
    let (out, err) = env.run(&["hook", "prompt"], &ev);
    assert_eq!(out, "");
    assert!(err.contains("unavailable"), "{err}");
    assert!(env.dir.join("down").exists());
    let (out, err) = env.run(&["hook", "prompt"], &ev);
    assert_eq!(
        (out.as_str(), err.as_str()),
        ("", ""),
        "no request while marked down"
    );
}

#[test]
fn missing_config_is_reported_on_stderr() {
    let env = Env::new("", "noconf");
    let (out, err) = env.run(&["hook", "prompt"], &json!({"prompt": PROMPT}));
    assert_eq!(out, "");
    assert!(err.contains("no server URL"), "{err}");
}

fn write_transcript(dir: &Path, lines: &[Value]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("transcript.jsonl");
    let body: Vec<String> = lines.iter().map(Value::to_string).collect();
    std::fs::write(&path, body.join("\n") + "\nnot json\n").unwrap();
    path
}

#[test]
fn capture_sends_new_user_lines_once_and_skips_stored_ones() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .body(|b| b["top_k"] == 1 && b["query"] == "The billing service runs on PostgreSQL 17.")
            .json(recall(&[hit("y", "unrelated", 0.4)])),
        Step::post("/v1/recall")
            .body(|b| b["query"] == "We deploy on Fridays.")
            .json(recall(&[hit("x", "We deploy on Fridays", 0.97)])),
        Step::post("/v1/session/end")
            .body(|b| b["summary"] == "The billing service runs on PostgreSQL 17.\n")
            .json(json!({"count": 1})),
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(|b| b["summary"] == "Carol is on call this week.\n")
            .json(json!({"count": 1})),
    ]);
    let env = Env::new(&srv.url, "capture").set("YREM_HOOK_CAPTURE", "1");
    let user = |t: &str| json!({"type": "user", "message": {"role": "user", "content": t}});
    let lines = vec![
        user("We deploy on Fridays"),
        json!({"type": "assistant", "message": {"role": "assistant", "content": "I think it is MySQL"}}),
        user("<system-reminder>noise</system-reminder>"),
        json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "meta turn text"}}),
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "text", "text": "The billing service runs on PostgreSQL 17"},
            {"type": "tool_result", "content": "ignored"},
        ]}}),
        user("ok"),
    ];
    let path = write_transcript(&env.dir, &lines);
    let ev = json!({"session_id": "c1", "transcript_path": path});
    assert_eq!(env.hook("pre-compact", "claude", &ev), "");

    let mut more = lines;
    more.push(user("Carol is on call this week"));
    write_transcript(&env.dir, &more);
    assert_eq!(
        env.hook("session-end", "claude", &ev),
        "",
        "earlier lines are watermarked"
    );
    assert_eq!(env.state("c1")["captured"].as_array().unwrap().len(), 3);
    assert_eq!(
        env.hook("session-end", "claude", &ev),
        "",
        "nothing new: no request"
    );
    srv.assert_done();
}

#[test]
fn capture_reads_codex_rollouts_and_plain_turns() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(|b| b["summary"] == "Use the staging cluster for load tests.\n")
            .json(json!({})),
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(|b| b["summary"] == "Billing owns the invoice service!\n")
            .json(json!({})),
    ]);
    let env = Env::new(&srv.url, "codex").set("YREM_HOOK_CAPTURE", "true");
    let item = |role: &str, text: &str| {
        json!({"type": "response_item", "payload": {"type": "message", "role": role,
               "content": [{"type": "input_text", "text": text}]}})
    };
    let path = write_transcript(
        &env.dir,
        &[
            item("user", "<environment_context>cwd</environment_context>"),
            item("user", "Use the staging cluster for load tests"),
            item("assistant", "Sure, the staging cluster it is"),
            json!({"type": "response_item", "payload": {"type": "function_call"}}),
        ],
    );
    let ev = json!({"session_id": "c2", "transcript_path": path});
    assert_eq!(env.hook("pre-compact", "codex", &ev), "");

    let ev = json!({"session_id": "c3", "turns": [
        {"role": "assistant", "text": "Billing owns the payments service"},
        {"role": "user", "text": "Billing owns the invoice service!"},
    ]});
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    srv.assert_done();
}

#[test]
fn headless_transcripts_are_not_captured() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "headless").set("YREM_HOOK_CAPTURE", "1");
    let path = write_transcript(
        &env.dir,
        &[json!({"type": "user", "entrypoint": "sdk-cli",
                   "message": {"role": "user", "content": "Review the plugin for bugs please"}})],
    );
    let ev = json!({"session_id": "c4", "transcript_path": path});
    assert_eq!(env.hook("pre-compact", "claude", &ev), "");
    let ev = json!({"session_id": "c4", "transcript_path": env.dir.join("missing.jsonl")});
    assert_eq!(env.hook("pre-compact", "claude", &ev), "");
    srv.assert_done();
}

#[test]
fn secret_lines_are_kept_out_without_a_request() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "secret").set("YREM_HOOK_CAPTURE", "1");
    let ev = json!({"session_id": "c5", "turns": [
        {"role": "user", "text": "the key is ghp_0123456789abcdefghijklmnopqrstuvwxyzAB"},
    ]});
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    assert_eq!(env.state("c5")["captured"].as_array().unwrap().len(), 1);
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    srv.assert_done();
}

#[test]
fn capture_is_kept_for_retry_when_the_server_is_down() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end").status(503).raw("busy"),
    ]);
    let env = Env::new(&srv.url, "retry").set("YREM_HOOK_CAPTURE", "1");
    let ev = json!({"session_id": "c6", "turns": [{"role": "user", "text": "Releases ship on Tuesdays"}]});
    let (_, err) = env.run(&["hook", "session-end"], &ev);
    assert!(err.contains("capture"), "{err}");
    assert_eq!(env.state("c6")["captured"], json!([]), "watermark released");
    srv.assert_done();
}

#[test]
fn detach_returns_at_once_and_finishes_in_the_background() {
    let srv = FakeServer::start(vec![Step::post("/v1/think").json(json!({}))]);
    let env = Env::new(&srv.url, "detach").set("YREM_HOOK_THINK", "1");
    let (out, _) = env.run(
        &[
            "--config",
            "/dev/null",
            "hook",
            "session-end",
            "--harness",
            "codex",
            "--detach",
        ],
        &json!({"session_id": "d1"}),
    );
    assert_eq!(out, "");
    srv.wait_for(1);
    srv.assert_done();
}

#[test]
fn detach_forwards_url_and_token_flags() {
    let srv = FakeServer::start(vec![Step::post("/v1/think").json(json!({}))]);
    let env = Env::new("http://127.0.0.1:9", "detachflags").set("YREM_HOOK_THINK", "1");
    env.run(
        &[
            "--url",
            &srv.url,
            "--token",
            TOKEN,
            "hook",
            "session-end",
            "--detach",
        ],
        &json!({}),
    );
    srv.wait_for(1);
    srv.assert_done();
}

#[test]
fn capture_keeps_the_newest_lines_and_leaves_the_rest_for_later() {
    let line = |c: char| format!("{}.", c.to_string().repeat(2_500));
    let (a, b, c) = (line('a'), line('b'), line('c'));
    let (first, second) = (format!("{b}\n{c}\n"), format!("{a}\n"));
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").status(500).raw("boom"),
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(move |body| body["summary"] == first)
            .json(json!({})),
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(move |body| body["summary"] == second)
            .json(json!({})),
    ]);
    let env = Env::new(&srv.url, "cap-capture").set("YREM_HOOK_CAPTURE", "1");
    let turns: Vec<Value> = [&a, &b, &c]
        .iter()
        .map(|t| json!({"role": "user", "text": t}))
        .collect();
    let ev = json!({"session_id": "c7", "turns": turns});
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    assert_eq!(env.state("c7")["captured"].as_array().unwrap().len(), 2);
    assert_eq!(
        env.hook("pre-compact", "plain", &ev),
        "",
        "the cut line goes next"
    );
    assert_eq!(env.state("c7")["captured"].as_array().unwrap().len(), 3);
    srv.assert_done();
}

#[test]
fn an_oversized_line_is_skipped_not_blocking_the_batch() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(|body| body["summary"] == "Releases ship on Tuesdays.\n")
            .json(json!({})),
    ]);
    let env = Env::new(&srv.url, "oversize").set("YREM_HOOK_CAPTURE", "1");
    let ev = json!({"session_id": "c8", "turns": [
        {"role": "user", "text": "Releases ship on Tuesdays"},
        {"role": "user", "text": "log ".repeat(2_000)},
    ]});
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    assert_eq!(env.state("c8")["captured"].as_array().unwrap().len(), 2);
    srv.assert_done();
}

#[test]
fn unreachable_server_stops_capture_and_releases_nothing_it_did_not_send() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").status(503).raw("busy")]);
    let env = Env::new(&srv.url, "cap-down").set("YREM_HOOK_CAPTURE", "1");
    let ev = json!({"session_id": "c9", "turns": [
        {"role": "user", "text": "Releases ship on Tuesdays"},
        {"role": "user", "text": "Billing owns the invoice service"},
    ]});
    let (_, err) = env.run(&["hook", "pre-compact"], &ev);
    assert!(err.contains("unavailable"), "{err}");
    assert_eq!(env.state("c9")["captured"], json!([]));
    srv.assert_done();
}

fn spawn_hook(env: &Env, args: &[&str], stdin: &Value) -> std::thread::JoinHandle<String> {
    let (dir, url, vars) = (env.dir.clone(), env.url.clone(), env.vars.clone());
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let stdin = stdin.clone();
    std::thread::spawn(move || {
        let env = Env { dir, url, vars };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        env.run(&args, &stdin).0
    })
}

#[test]
fn concurrent_prompt_hooks_never_inject_the_same_memory_twice() {
    let hits = vec![hit("a", "Payments uses PostgreSQL", 0.9)];
    let srv = FakeServer::start(
        (0..6)
            .map(|_| Step::post("/v1/recall").json(recall(&hits)))
            .collect(),
    );
    let env = Env::new(&srv.url, "race-prompt");
    let ev = json!({"session_id": "r1", "prompt": PROMPT});
    let outs: Vec<String> = (0..6)
        .map(|_| spawn_hook(&env, &["hook", "prompt"], &ev))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert_eq!(outs.iter().filter(|o| !o.is_empty()).count(), 1, "{outs:?}");
    srv.assert_done();
}

#[test]
fn concurrent_session_starts_open_one_tracked_session() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/sessions").json(json!({"session_id": "srv-9"})),
    ]);
    let env = Env::new(&srv.url, "race-track")
        .set("YREM_HOOK_TRACK_SESSION", "1")
        .set("YREM_HOOK_DIGEST", "0");
    let ev = json!({"session_id": "r2"});
    let handles: Vec<_> = (0..6)
        .map(|_| spawn_hook(&env, &["hook", "session-start"], &ev))
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(env.state("r2")["server_session"], "srv-9");
    srv.assert_done();
}

#[test]
fn opencode_session_ids_are_not_refused_as_secrets() {
    let id = "ses_2a1ac3785ffeaGVGrhb2KRRyhH";
    let srv = FakeServer::start(vec![
        Step::post("/v1/sessions")
            .body(move |b| b["client_id"] == format!("plain-{id}"))
            .json(json!({"session_id": "srv-oc"})),
    ]);
    let env = Env::new(&srv.url, "oc-id")
        .set("YREM_HOOK_TRACK_SESSION", "1")
        .set("YREM_HOOK_DIGEST", "0");
    env.hook("session-start", "plain", &json!({"session_id": id}));
    assert_eq!(env.state(id)["server_session"], "srv-oc");
    srv.assert_done();
}

#[test]
fn a_briefing_claim_left_by_a_killed_hook_is_taken_over() {
    let srv = FakeServer::start(vec![Step::get("/v1/session/digest").json(digest())]);
    let env = Env::new(&srv.url, "abandoned");
    std::fs::create_dir_all(env.dir.join("k1")).unwrap();
    let claim = env.dir.join("k1/digest");
    std::fs::write(&claim, "pending").unwrap();
    let ev = json!({"session_id": "k1", "source": "startup"});
    assert_eq!(
        env.hook("session-start", "plain", &ev),
        "",
        "a fresh pending claim is live"
    );
    std::fs::File::options()
        .write(true)
        .open(&claim)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(60))
        .unwrap();
    assert!(
        env.hook("session-start", "plain", &ev)
            .starts_with("Memory briefing")
    );
    assert_eq!(std::fs::read_to_string(&claim).unwrap(), "done");
    srv.assert_done();
}

#[test]
fn think_runs_even_when_closing_the_session_fails() {
    let srv = FakeServer::start(vec![
        Step::new("DELETE", "/v1/sessions/gone")
            .status(404)
            .json(json!({"error": "no"})),
        Step::post("/v1/think").json(json!({})),
    ]);
    let env = Env::new(&srv.url, "end-fail").set("YREM_HOOK_THINK", "1");
    std::fs::create_dir_all(env.dir.join("e1")).unwrap();
    std::fs::write(env.dir.join("e1/tracking"), "gone").unwrap();
    let (_, err) = env.run(&["hook", "session-end"], &json!({"session_id": "e1"}));
    assert!(err.contains("session end"), "{err}");
    assert_eq!(env.state("e1")["server_session"], Value::Null);
    srv.assert_done();
}

#[test]
fn detach_forwards_the_event_on_stdin() {
    let srv = FakeServer::start(vec![
        Step::new("DELETE", "/v1/sessions/srv-d").json(json!({})),
    ]);
    let env = Env::new(&srv.url, "detach-stdin");
    std::fs::create_dir_all(env.dir.join("d2")).unwrap();
    std::fs::write(env.dir.join("d2/tracking"), "srv-d").unwrap();
    env.run(
        &["hook", "session-end", "--detach"],
        &json!({"session_id": "d2"}),
    );
    srv.wait_for(1);
    srv.assert_done();
}

#[test]
fn codex_subagents_and_injected_user_blocks_are_not_captured() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[])),
        Step::post("/v1/session/end")
            .body(|b| b["summary"] == "Keep the staging cluster for load tests.\n")
            .json(json!({})),
    ]);
    let env = Env::new(&srv.url, "codex-kinds").set("YREM_HOOK_CAPTURE", "1");
    let msg = |content: Value, kinds: Value| {
        json!({"type": "response_item", "payload": {"type": "message", "role": "user",
               "content": content,
               "internal_chat_message_metadata_passthrough": {"content_item_kinds": kinds}}})
    };
    let text = |t: &str| json!({"type": "input_text", "text": t});
    let rollout = |name: &str, meta: Value| {
        let dir = env.dir.join(name);
        write_transcript(
            &dir,
            &[
                json!({"type": "session_meta", "payload": meta}),
                msg(
                    json!([text("A skill body injected under the user role")]),
                    json!(["skill.body"]),
                ),
                msg(
                    json!([
                        text("Keep the staging cluster for load tests"),
                        text("app page context here")
                    ]),
                    json!(["user.input", "app.context"]),
                ),
            ],
        )
    };
    let main = rollout("main", json!({"originator": "codex_cli_rs"}));
    let sub = rollout("sub", json!({"source": {"subagent": {"thread_spawn": {}}}}));
    let exec = rollout("exec", json!({"originator": "codex_exec"}));
    for (sid, path) in [("x1", &main), ("x2", &sub), ("x3", &exec)] {
        env.hook(
            "pre-compact",
            "codex",
            &json!({"session_id": sid, "transcript_path": path}),
        );
    }
    srv.assert_done();
}

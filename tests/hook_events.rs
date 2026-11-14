//! `yrem hook <event>` end to end against the fake server: injection, dedup across hook
//! processes, capture watermarks and failure isolation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::needless_pass_by_value
)]

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
            // The suite may itself run inside an SDK-driven agent, whose entrypoint would
            // make every hook count as headless.
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
        "narrative_head": {"rid": "n0", "snippet": "Payments moved to PostgreSQL last week.",
                           "namespace": "default", "current_status": "active"},
        "open_conflict_count": 2,
        "pending_trigger_count": 6,
        "knowledge_gaps": [{"query": "quantum gardening"}],
        // The server's all-time top decisions are never shown.
        "top_decisions": [{"rid": "static", "snippet": "Static top decision", "current_status": "active"}],
    })
}

const LIST_ALL: &str = "/v1/memories?limit=100&order=desc";

fn list_project(project: &str) -> String {
    format!("/v1/memories?limit=50&namespace={project}&order=desc")
}

/// A `memory list --desc` row created `hours_ago`.
fn row(rid: &str, namespace: &str, text: &str, hours_ago: f64) -> Value {
    let at = chrono::Utc::now().timestamp() as f64 - hours_ago * 3600.0;
    let iso = chrono::DateTime::from_timestamp(at as i64, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    json!({"rid": rid, "text": text, "namespace": namespace, "source": "user",
           "created_at": at, "created_at_iso": iso, "metadata": {}, "importance": 0.5})
}

fn records(rows: &[Value]) -> Value {
    json!({"records": rows, "limit": 200, "next_cursor": null})
}

/// The listed row the digest's narrative head points at.
fn narrative_row() -> Value {
    row(
        "n0",
        "default",
        "Payments moved to PostgreSQL last week.",
        4.0,
    )
}

/// The two requests of a briefing without a project.
fn briefing_steps(rows: &[Value]) -> Vec<Step> {
    let mut listed = rows.to_vec();
    listed.push(narrative_row());
    vec![
        Step::get("/v1/session/digest").json(digest()),
        Step::get(LIST_ALL).json(records(&listed)),
    ]
}

/// What `memory get` answers for a stored memory.
fn stored(rid: &str, metadata: Value) -> Step {
    Step::get(&format!("/v1/memory/{rid}"))
        .json(json!({"rid": rid, "text": "t", "metadata_json": metadata}))
}

/// A repository directory under the test's state dir; returns the path inside it to use as `cwd`.
fn repo(env: &Env, name: &str) -> PathBuf {
    let dir = env.dir.join("repos").join(name);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir.join("src")
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
    let mut captured = row("cap", "default", "Captured prompt", 1.0);
    captured["source"] = json!("session_auto_capture");
    let mut secret = row("pin", "default", "Offline code is <redacted>", 1.0);
    secret["metadata"] = json!({"sensitive": true});
    let mut replaced = row("old", "default", "Payments uses MySQL", 2.0);
    replaced["current_status"] = json!("superseded");
    let srv = FakeServer::start_unordered(briefing_steps(&[
        row("d1", "default", "Payments uses PostgreSQL", 3.0),
        row("hermes", "hermes:chat", "Another agent's chat", 1.0),
        captured,
        secret,
        replaced,
        row("stale", "default", "Last month's decision", 24.0 * 30.0),
    ]));
    let env = Env::new(&srv.url, "once").set("YREM_HOOK_EXCLUDE_NAMESPACES", "hermes:");
    let ev = json!({"session_id": "s1", "source": "startup"});
    let ctx = context(&env.hook("session-start", "claude", &ev), "SessionStart");
    assert!(ctx.contains("do not call session_digest"), "{ctx}");
    assert!(ctx.contains("Where things stood: Payments moved to PostgreSQL"));
    assert!(ctx.contains("] Payments uses PostgreSQL"), "{ctx}");
    for gone in [
        "MySQL",
        "chat",
        "Captured",
        "redacted",
        "Last month",
        "Static",
    ] {
        assert!(!ctx.contains(gone), "{gone}: {ctx}");
    }
    assert!(
        ctx.contains("Open conflicts: 2"),
        "the first briefing of the day carries maintenance: {ctx}"
    );
    assert!(!ctx.contains("trigger"), "{ctx}");
    assert_eq!(
        ctx.matches("Payments moved to PostgreSQL last week")
            .count(),
        1,
        "the narrative is not repeated as a decision: {ctx}"
    );
    assert_eq!(env.state("s1")["seen"], json!(["d1", "n0"]));

    assert_eq!(
        env.hook("session-start", "codex", &ev),
        "",
        "a second plugin stays silent"
    );
    srv.assert_done();
}

#[test]
fn compact_reinjects_after_the_duplicate_window() {
    let rows = [row("d1", "default", "Payments uses PostgreSQL", 3.0)];
    let mut steps = briefing_steps(&rows);
    steps.extend([
        Step::get("/v1/session/digest?include_gaps=true").json(digest()),
        Step::get(LIST_ALL).json(records(&[rows[0].clone(), narrative_row()])),
    ]);
    let srv = FakeServer::start_unordered(steps);
    let env = Env::new(&srv.url, "compact").set("YREM_HOOK_MAINTENANCE", "0");
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
    assert!(
        !text.contains("Pending triggers"),
        "triggers are no longer shown"
    );
    assert!(text.contains("Known gaps: quantum gardening."));
    srv.assert_done();
}

#[test]
fn failed_digest_releases_the_claim() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").status(500).raw("boom"),
        Step::get(LIST_ALL).json(records(&[])),
        Step::get("/v1/session/digest").json(json!({})),
        Step::get(LIST_ALL).json(records(&[])),
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
    let srv = FakeServer::start_unordered(briefing_steps(&[]));
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

// ---------------------------------------------------------------- project namespace

fn hit_in(rid: &str, ns: &str, text: &str, why: &[&str]) -> Value {
    json!({"rid": rid, "text": text, "score": 0.5, "namespace": ns, "source": "user",
           "metadata": {}, "why_retrieved": why})
}

fn cos(c: &str) -> String {
    format!("semantically similar ({c})")
}

#[test]
fn the_project_from_cwd_scopes_the_briefing() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest?namespace=fluidd").json(digest()),
        Step::get(LIST_ALL).json(records(&[
            row(
                "n1",
                "default",
                "Fluidd decision kept in the default namespace",
                2.0,
            ),
            row("x1", "sui", "Sui notes from today", 1.0),
            row("x2", "sui", "More Sui notes", 3.0),
            row("x3", "sui", "Third Sui note", 4.0),
            narrative_row(),
        ])),
        Step::get(&list_project("fluidd")).json(records(&[
            row("p1", "fluidd", "Fluidd valuation rule", 50.0),
            row("p2", "fluidd", "Fluidd board rule", 60.0),
        ])),
    ]);
    let env = Env::new(&srv.url, "proj").set("YREM_HOOK_MAINTENANCE", "0");
    let cwd = repo(&env, "fluidd");
    let ctx = context(
        &env.hook(
            "session-start",
            "claude",
            &json!({"session_id": "j1", "cwd": cwd}),
        ),
        "SessionStart",
    );
    let lines: Vec<&str> = ctx.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(lines.len(), 5, "{ctx}");
    assert!(
        lines[0].ends_with("Fluidd decision kept in the default namespace"),
        "{ctx}"
    );
    assert!(lines[1].ends_with("Fluidd valuation rule"), "{ctx}");
    assert!(lines[2].ends_with("Fluidd board rule"), "{ctx}");
    assert!(lines[3].ends_with("Sui notes from today"), "{ctx}");
    assert!(
        lines[4].ends_with("More Sui notes"),
        "two recent rows from elsewhere: {ctx}"
    );
    assert_eq!(
        env.state("j1")["seen"],
        json!(["n0", "n1", "p1", "p2", "x1", "x2"]),
        "every shown row counts as seen"
    );
    srv.assert_done();
}

#[test]
fn a_worktree_and_an_alias_map_to_the_project() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest?namespace=vaultkeeper").json(json!({})),
        Step::get(LIST_ALL).json(records(&[])),
        Step::get(&list_project("vaultkeeper")).json(records(&[row(
            "v1",
            "vaultkeeper",
            "Vaultkeeper rule",
            5.0,
        )])),
    ]);
    let env = Env::new(&srv.url, "alias")
        .set("YREM_HOOK_NAMESPACE_ALIASES", "bw-lite-desktop=vaultkeeper")
        .set("YREM_HOOK_MAINTENANCE", "0");
    let main = env.dir.join("repos/bw-lite-desktop");
    std::fs::create_dir_all(main.join(".git/worktrees/agent-7")).unwrap();
    let wt = env.dir.join("repos/vk-wt-1");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}/.git/worktrees/agent-7\n", main.display()),
    )
    .unwrap();
    let out = env.hook(
        "session-start",
        "plain",
        &json!({"session_id": "j2", "cwd": wt}),
    );
    assert!(out.contains("Vaultkeeper rule"), "{out}");
    srv.assert_done();
}

#[test]
fn an_explicit_namespace_is_the_project_and_filters_recall() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest?namespace=payments").json(json!({})),
        Step::get(LIST_ALL).json(records(&[])),
        Step::get(&list_project("payments")).json(records(&[row(
            "p",
            "payments",
            "Payments rule",
            2.0,
        )])),
        Step::post("/v1/recall")
            .body(|b| b["namespace"] == "payments")
            .json(recall(&[hit_in(
                "h",
                "payments",
                "Payments uses PostgreSQL",
                &[&cos("0.5")],
            )])),
    ]);
    let env = Env::new(&srv.url, "explicit")
        .set("YREM_HOOK_NAMESPACE", "payments")
        .set("YREM_HOOK_MAINTENANCE", "0");
    let cwd = repo(&env, "fluidd");
    let start = env.hook(
        "session-start",
        "plain",
        &json!({"session_id": "j3", "cwd": cwd}),
    );
    assert!(start.contains("Payments rule"), "{start}");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "j3", "cwd": cwd, "prompt": PROMPT}),
    );
    assert!(out.contains("Payments uses PostgreSQL"), "{out}");
    srv.assert_done();
}

#[test]
fn zero_decisions_skip_the_listing() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").json(digest()),
        stored("n0", json!({})),
    ]);
    let env = Env::new(&srv.url, "nodec")
        .set("YREM_HOOK_DECISIONS", "0")
        .set("YREM_HOOK_MAINTENANCE", "0");
    let out = env.hook("session-start", "plain", &json!({"session_id": "j4"}));
    assert!(out.contains("Where things stood"), "{out}");
    assert!(!out.contains("Live decisions"), "{out}");
    srv.assert_done();
}

#[test]
fn a_failing_listing_leaves_out_the_decisions_only() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").json(digest()),
        Step::get(LIST_ALL).status(500).raw("boom"),
        stored("n0", json!({})),
    ]);
    let env = Env::new(&srv.url, "listfail").set("YREM_HOOK_MAINTENANCE", "0");
    let (out, err) = env.run(
        &["hook", "session-start", "--harness", "plain"],
        &json!({"session_id": "j5"}),
    );
    assert!(out.contains("Where things stood"), "{out}");
    assert!(!out.contains("Live decisions"), "{out}");
    assert!(err.contains("decisions"), "{err}");
    srv.assert_done();
}

#[test]
fn an_unreachable_listing_does_not_mark_the_server_down() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").json(digest()),
        Step::get(LIST_ALL).status(503).raw("busy"),
        stored("n0", json!({})),
    ]);
    let env = Env::new(&srv.url, "listdown").set("YREM_HOOK_MAINTENANCE", "0");
    let (out, err) = env.run(
        &["hook", "session-start", "--harness", "plain"],
        &json!({"session_id": "j6"}),
    );
    assert!(out.contains("Where things stood"), "{out}");
    assert!(err.contains("decisions"), "{err}");
    assert!(
        !env.dir.join("down").exists(),
        "a listing is not worth a minute of silence"
    );
    assert_eq!(env.state("j6")["seen"], json!(["n0"]));
    srv.assert_done();
}

#[test]
fn an_unreachable_digest_fails_the_briefing_and_releases_the_claim() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").status(503).raw("busy"),
        Step::get(LIST_ALL).json(records(&[])),
        Step::get("/v1/session/digest").json(digest()),
        Step::get(LIST_ALL).json(records(&[
            row("a", "x", "Recent note", 1.0),
            narrative_row(),
        ])),
    ]);
    let env = Env::new(&srv.url, "digestdown").set("YREM_HOOK_MAINTENANCE", "0");
    let ev = json!({"session_id": "j7"});
    let (out, err) = env.run(&["hook", "session-start", "--harness", "plain"], &ev);
    assert_eq!(out, "");
    assert!(err.contains("unavailable"), "{err}");
    assert!(env.dir.join("down").exists());
    std::fs::remove_file(env.dir.join("down")).unwrap();
    let out = env.hook("session-start", "plain", &ev);
    assert!(out.contains("Recent note"), "the claim was released: {out}");
    srv.assert_done();
}

#[test]
fn the_briefing_requests_run_at_once() {
    let delay = Duration::from_millis(600);
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest?namespace=fluidd")
            .delay(delay)
            .json(digest()),
        Step::get(LIST_ALL)
            .delay(delay)
            .json(records(&[narrative_row()])),
        Step::get(&list_project("fluidd"))
            .delay(delay)
            .json(records(&[row("p1", "fluidd", "Fluidd rule", 3.0)])),
    ]);
    let env = Env::new(&srv.url, "parallel").set("YREM_HOOK_MAINTENANCE", "0");
    let cwd = repo(&env, "fluidd");
    let started = std::time::Instant::now();
    let out = env.hook(
        "session-start",
        "plain",
        &json!({"session_id": "j8", "cwd": cwd}),
    );
    assert!(out.contains("Fluidd rule"), "{out}");
    assert!(
        started.elapsed() < delay * 2,
        "three requests of {delay:?} ran one after the other: {:?}",
        started.elapsed()
    );
    srv.assert_done();
}

// ---------------------------------------------------------------- maintenance

#[test]
fn maintenance_appears_once_a_day_across_sessions() {
    let mut steps = vec![];
    for _ in 0..3 {
        steps.extend(briefing_steps(&[]));
    }
    let srv = FakeServer::start_unordered(steps);
    let env = Env::new(&srv.url, "daily");
    let first = env.hook("session-start", "plain", &json!({"session_id": "m1"}));
    assert!(first.contains("Open conflicts: 2"), "{first}");
    assert!(
        !first.contains("Known gaps"),
        "daily mode asks for no gaps: {first}"
    );
    let second = env.hook("session-start", "plain", &json!({"session_id": "m2"}));
    assert!(!second.contains("conflicts"), "{second}");
    age(&env.dir.join(".maintenance"), 25 * 3600);
    let third = env.hook("session-start", "plain", &json!({"session_id": "m3"}));
    assert!(third.contains("Open conflicts: 2"), "a day later: {third}");
    srv.assert_done();
}

fn age(path: &Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn maintenance_off_and_always() {
    let listed = || Step::get(LIST_ALL).json(records(&[narrative_row()]));
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").json(digest()),
        listed(),
        Step::get("/v1/session/digest?include_gaps=true").json(digest()),
        listed(),
        Step::get("/v1/session/digest?include_gaps=true").json(digest()),
        listed(),
    ]);
    let off = Env::new(&srv.url, "m-off").set("YREM_HOOK_MAINTENANCE", "0");
    let text = off.hook("session-start", "plain", &json!({"session_id": "m4"}));
    assert!(
        !text.contains("conflicts") && !text.contains("gaps"),
        "{text}"
    );
    assert!(!off.dir.join(".maintenance").exists());

    let always = Env::new(&srv.url, "m-always").set("YREM_HOOK_MAINTENANCE", "1");
    for sid in ["m5", "m6"] {
        let text = always.hook("session-start", "plain", &json!({"session_id": sid}));
        assert!(
            text.contains("Open conflicts: 2") && text.contains("Known gaps: quantum"),
            "{text}"
        );
        assert!(!text.contains("Pending triggers"), "{text}");
    }
    assert!(
        !always.dir.join(".maintenance").exists(),
        "always needs no claim"
    );
    srv.assert_done();
}

#[test]
fn daily_maintenance_is_not_spent_on_a_briefing_with_nothing_to_report() {
    let quiet = json!({"open_conflict_count": 0, "pending_trigger_count": 4});
    let mut steps = vec![
        Step::get("/v1/session/digest").json(quiet),
        Step::get(LIST_ALL).json(records(&[])),
    ];
    steps.extend(briefing_steps(&[]));
    let srv = FakeServer::start_unordered(steps);
    let env = Env::new(&srv.url, "m-quiet");
    env.hook("session-start", "plain", &json!({"session_id": "m8"}));
    assert!(
        !env.dir.join(".maintenance").exists(),
        "nothing was shown, nothing is claimed"
    );
    let text = env.hook("session-start", "plain", &json!({"session_id": "m9"}));
    assert!(text.contains("Open conflicts: 2"), "{text}");
    srv.assert_done();
}

#[test]
fn a_failed_briefing_leaves_the_maintenance_slot_alone() {
    let mut steps = vec![
        Step::get("/v1/session/digest").status(500).raw("boom"),
        Step::get(LIST_ALL).json(records(&[])),
    ];
    steps.extend(briefing_steps(&[]));
    let srv = FakeServer::start_unordered(steps);
    let env = Env::new(&srv.url, "m-fail");
    env.run(&["hook", "session-start"], &json!({"session_id": "m7"}));
    assert!(!env.dir.join(".maintenance").exists());
    let text = env.hook("session-start", "plain", &json!({"session_id": "m7"}));
    assert!(text.contains("Open conflicts: 2"), "{text}");
    srv.assert_done();
}

#[test]
fn an_unknown_maintenance_value_is_reported_and_means_daily() {
    let srv = FakeServer::start_unordered(briefing_steps(&[]));
    let env = Env::new(&srv.url, "m-junk").set("YREM_HOOK_MAINTENANCE", "weekly");
    let (out, err) = env.run(
        &["hook", "session-start", "--harness", "plain"],
        &json!({"session_id": "m10"}),
    );
    assert!(err.contains("YREM_HOOK_MAINTENANCE"), "{err}");
    assert!(out.contains("Open conflicts: 2"), "{out}");
    srv.assert_done();
}

// ---------------------------------------------------------------- headless

#[test]
fn sdk_sessions_get_no_briefing_and_no_recall() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "sdk").set("CLAUDE_CODE_ENTRYPOINT", "sdk-cli");
    assert_eq!(
        env.hook("session-start", "claude", &json!({"session_id": "h1"})),
        ""
    );
    assert_eq!(
        env.hook(
            "prompt",
            "claude",
            &json!({"session_id": "h1", "prompt": PROMPT})
        ),
        ""
    );
    srv.assert_done();
}

#[test]
fn an_interactive_entrypoint_is_not_headless() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&[]))]);
    let env = Env::new(&srv.url, "cli").set("CLAUDE_CODE_ENTRYPOINT", "cli");
    env.hook(
        "prompt",
        "claude",
        &json!({"session_id": "h2", "prompt": PROMPT}),
    );
    srv.assert_done();
}

#[test]
fn codex_exec_and_subagent_rollouts_get_no_briefing_and_no_recall() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&[]))]);
    let env = Env::new(&srv.url, "codex-headless");
    let rollout = |name: &str, meta: Value| {
        write_transcript(
            &env.dir.join(name),
            &[json!({"type": "session_meta", "payload": meta})],
        )
    };
    let exec = rollout("exec", json!({"originator": "codex_exec"}));
    let sub = rollout("sub", json!({"source": {"subagent": {}}}));
    let tui = rollout("tui", json!({"originator": "codex-tui"}));
    for (sid, path) in [("h3", &exec), ("h4", &sub)] {
        let ev = json!({"session_id": sid, "transcript_path": path, "prompt": PROMPT});
        assert_eq!(env.hook("session-start", "codex", &ev), "");
        assert_eq!(env.hook("prompt", "codex", &ev), "");
    }
    let ev = json!({"session_id": "h5", "transcript_path": tui, "prompt": PROMPT});
    env.hook("prompt", "codex", &ev);
    srv.assert_done();
}

// ---------------------------------------------------------------- prompt hygiene

#[test]
fn the_query_has_no_ide_blocks_and_no_secrets_and_asks_not_to_reinforce() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .body(|b| {
                b["query"] == "how do we deploy <redacted> to prod?"
                    && b["skip_reinforce"] == true
                    && b["top_k"] == 10
            })
            .json(recall(&[])),
    ]);
    let env = Env::new(&srv.url, "hygiene");
    let prompt = "<ide_selection>The user selected lines 1 to 1 from /x/.sessions/a.credentials.json:\nhunter2</ide_selection>\n\
                  how do we deploy ghp_0123456789abcdefghijklmnopqrstuvwxyzAB to prod?";
    env.hook(
        "prompt",
        "claude",
        &json!({"session_id": "q1", "prompt": prompt}),
    );
    srv.assert_done();
}

#[test]
fn harness_prompts_and_unclosed_ide_blocks_send_no_request() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "no-query");
    for p in [
        "<task-notification><task-id>b3tr1jdn2</task-id> finished with a long enough body</task-notification>",
        "Another Claude session sent a message: <agent-message from=\"a\"> [Subagent hand-back] text",
        "<ide_selection>never closed, followed by a perfectly normal question",
        "# Overview\nGenerate 0 to 3 hyperpersonalized suggestions for the next prompt please",
    ] {
        assert_eq!(
            env.hook(
                "prompt",
                "claude",
                &json!({"session_id": "q2", "prompt": p})
            ),
            "",
            "{p}"
        );
    }
    srv.assert_done();
}

// ---------------------------------------------------------------- project-aware gate

#[test]
fn prompt_gate_prefers_the_project_and_asks_more_of_the_rest() {
    let hits = vec![
        hit_in("off55", "sui", "Sui notes about commits", &[&cos("0.55")]),
        hit_in("off60", "sui", "Sui deploy notes", &[&cos("0.60")]),
        hit_in(
            "kw",
            "fluidd",
            "Fluidd board rule",
            &["keyword_match", "multi-lane agreement (2 lanes)"],
        ),
        hit_in(
            "named",
            "default",
            "The Fluidd valuation rule",
            &[&cos("0.50")],
        ),
        hit_in(
            "graph",
            "fluidd",
            "Fluidd graph-only note",
            &[
                "keyword_match",
                "multi-lane agreement (2 lanes)",
                "graph-connected via API",
            ],
        ),
        hit_in("cos", "fluidd", "Fluidd semantic note", &[&cos("0.70")]),
    ];
    let step = || {
        Step::post("/v1/recall")
            .body(|b| b.get("namespace").is_none() && b["skip_reinforce"] == true)
            .json(recall(&hits))
    };
    let srv = FakeServer::start(vec![step(), step()]);
    let env = Env::new(&srv.url, "gate-proj");
    let cwd = repo(&env, "fluidd");
    let ask = |env: &Env, sid: &str| {
        let out = env.hook(
            "prompt",
            "plain",
            &json!({"session_id": sid, "cwd": cwd, "prompt": PROMPT}),
        );
        out.lines()
            .filter(|l| l.starts_with("- "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ask(&env, "g1"),
        [
            "- Fluidd board rule",
            "- The Fluidd valuation rule",
            "- Fluidd semantic note"
        ],
        "project hits first, cut at top 3"
    );
    // With room for all of them, what the gate lets through shows: the 0.60 hit of another
    // namespace, but neither the 0.55 one nor the keyword hit that only the graph agreed on.
    std::fs::remove_dir_all(env.dir.join(".cooldown")).unwrap();
    let wide = env.clone().set("YREM_HOOK_TOP_K", "10");
    assert_eq!(
        ask(&wide, "g2"),
        [
            "- Fluidd board rule",
            "- The Fluidd valuation rule",
            "- Fluidd semantic note",
            "- Sui deploy notes"
        ]
    );
    srv.assert_done();
}

#[test]
fn prompt_gate_without_a_project_behaves_as_before() {
    let hits = vec![
        hit_in("a", "sui", "Sui note", &[&cos("0.55")]),
        hit_in(
            "b",
            "fluidd",
            "Fluidd keyword note",
            &["keyword_match", "multi-lane agreement (2 lanes)"],
        ),
    ];
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&hits))]);
    let env = Env::new(&srv.url, "gate-none");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "g2", "prompt": PROMPT}),
    );
    assert!(out.trim_end().ends_with("\n- Sui note"), "{out}");
    srv.assert_done();
}

// ---------------------------------------------------------------- cooldown

#[test]
fn a_memory_is_not_reinjected_in_the_same_project_within_a_day() {
    let hits = || {
        recall(&[
            hit_in("a", "fluidd", "Fluidd board rule", &[&cos("0.64")]),
            hit_in("b", "fluidd", "Fluidd strong match", &[&cos("0.65")]),
            hit_in("c", "fluidd", "Fluidd fresh note", &[&cos("0.55")]),
        ])
    };
    let srv = FakeServer::start(
        (0..4)
            .map(|_| Step::post("/v1/recall").json(hits()))
            .collect(),
    );
    let env = Env::new(&srv.url, "cool");
    let cwd = repo(&env, "fluidd");
    let other = repo(&env, "auction-bot");
    let ask = |sid: &str, cwd: &Path| {
        env.hook(
            "prompt",
            "plain",
            &json!({"session_id": sid, "cwd": cwd, "prompt": PROMPT}),
        )
    };
    let first = ask("k1", &cwd);
    for t in ["board rule", "strong match", "fresh note"] {
        assert!(first.contains(t), "{first}");
    }
    let second = ask("k2", &cwd);
    assert!(
        second.contains("strong match"),
        "cosine 0.65 passes the cooldown: {second}"
    );
    assert!(
        !second.contains("board rule") && !second.contains("fresh note"),
        "{second}"
    );
    let elsewhere = ask("k3", &other);
    assert!(
        elsewhere.contains("board rule"),
        "another project has its own cooldown: {elsewhere}"
    );

    for rid in ["a", "b", "c"] {
        age(&env.dir.join(".cooldown/fluidd").join(rid), 25 * 3600);
    }
    let later = ask("k4", &cwd);
    assert!(
        later.contains("board rule") && later.contains("fresh note"),
        "a day later: {later}"
    );
    srv.assert_done();
}

#[test]
fn without_a_project_there_is_no_cooldown() {
    let hits = recall(&[hit_in("a", "x", "Some note", &[&cos("0.55")])]);
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(hits.clone()),
        Step::post("/v1/recall").json(hits),
    ]);
    let env = Env::new(&srv.url, "no-cool");
    for sid in ["k5", "k6"] {
        let out = env.hook(
            "prompt",
            "plain",
            &json!({"session_id": sid, "prompt": PROMPT}),
        );
        assert!(out.contains("Some note"), "{out}");
    }
    assert!(!env.dir.join(".cooldown").exists());
    srv.assert_done();
}

// ---------------------------------------------------------------- capture and tracking

#[test]
fn capture_goes_to_the_project_namespace() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .body(|b| b["namespace"] == "fluidd" && b["skip_reinforce"] == true && b["top_k"] == 1)
            .json(recall(&[])),
        Step::post("/v1/session/end")
            .body(|b| b["namespace"] == "fluidd" && b["summary"] == "Releases ship on Tuesdays.\n")
            .json(json!({})),
    ]);
    let env = Env::new(&srv.url, "cap-proj").set("YREM_HOOK_CAPTURE", "1");
    let cwd = repo(&env, "fluidd");
    let ev = json!({"session_id": "c20", "cwd": cwd,
                    "turns": [{"role": "user", "text": "Releases ship on Tuesdays"}]});
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    srv.assert_done();
}

#[test]
fn headless_sessions_are_not_captured_from_the_environment_either() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "cap-sdk")
        .set("YREM_HOOK_CAPTURE", "1")
        .set("CLAUDE_CODE_ENTRYPOINT", "sdk-ts");
    let ev = json!({"session_id": "c21", "turns": [{"role": "user", "text": "Releases ship on Tuesdays"}]});
    assert_eq!(env.hook("pre-compact", "plain", &ev), "");
    srv.assert_done();
}

#[test]
fn turning_capture_on_does_not_inject_captured_rows() {
    let mut captured = hit_in("cap", "default", "Captured prompt text", &[&cos("0.9")]);
    captured["source"] = json!("session_auto_capture");
    let hits = recall(&[
        captured,
        hit_in("ok", "default", "A curated fact", &[&cos("0.9")]),
    ]);
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(hits.clone()),
        Step::post("/v1/recall").json(hits),
    ]);
    let env = Env::new(&srv.url, "inject-cap").set("YREM_HOOK_CAPTURE", "1");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "c22", "prompt": PROMPT}),
    );
    assert!(
        out.trim_end().ends_with("\n- A curated fact") && !out.contains("Captured"),
        "{out}"
    );
    let env = env.set("YREM_HOOK_INJECT_CAPTURED", "1");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "c23", "prompt": PROMPT}),
    );
    assert!(out.contains("Captured prompt text"), "{out}");
    srv.assert_done();
}

#[test]
fn sensitive_rows_are_never_injected() {
    let mut secret = hit_in("pin", "default", "Offline code <redacted>", &[&cos("0.99")]);
    secret["metadata"] = json!({"sensitive": true});
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&[
        secret,
        hit_in("ok", "default", "A curated fact", &[&cos("0.9")]),
    ]))]);
    let env = Env::new(&srv.url, "sens").set("YREM_HOOK_INJECT_CAPTURED", "1");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "c24", "prompt": PROMPT}),
    );
    assert!(
        out.trim_end().ends_with("\n- A curated fact") && !out.contains("Offline"),
        "{out}"
    );
    srv.assert_done();
}

#[test]
fn the_tracked_session_carries_the_project_namespace() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/sessions")
            .body(|b| b["namespace"] == "fluidd" && b["client_id"] == "codex-t1")
            .json(json!({"session_id": "srv-t"})),
    ]);
    let env = Env::new(&srv.url, "track-proj")
        .set("YREM_HOOK_TRACK_SESSION", "1")
        .set("YREM_HOOK_DIGEST", "0");
    let cwd = repo(&env, "fluidd");
    env.hook(
        "session-start",
        "codex",
        &json!({"session_id": "t1", "cwd": cwd}),
    );
    assert_eq!(env.state("t1")["server_session"], "srv-t");
    srv.assert_done();
}

// ---------------------------------------------------------------- merges of sensitive memories

fn merge_hit(rid: &str, label: &str, sources: &[&str]) -> Value {
    let mut h = hit_in(
        rid,
        "default",
        &format!("{label}. {}", "Merged note. ".repeat(60)),
        &[&cos("0.9")],
    );
    h["metadata"] = json!({"consolidated_from": sources});
    h
}

fn sensitive_memory(rid: &str) -> Step {
    stored(rid, json!({"sensitive": true}))
}

#[test]
fn a_merge_of_a_sensitive_memory_is_not_injected() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[
            merge_hit("mbad", "Poisoned merge", &["s1", "s2"]),
            merge_hit("mgood", "Clean merge", &["s3"]),
            hit_in("plain", "default", "A plain fact", &[&cos("0.8")]),
        ])),
        stored("s1", json!({})),
        sensitive_memory("s2"),
        stored("s3", json!({})),
    ]);
    let env = Env::new(&srv.url, "lineage");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l1", "prompt": PROMPT}),
    );
    assert!(
        out.contains("Clean merge") && out.contains("A plain fact"),
        "{out}"
    );
    assert!(!out.contains("Poisoned"), "{out}");
    assert_eq!(env.state("l1")["seen"], json!(["mgood", "plain"]));
    srv.assert_done();
}

#[test]
fn sources_are_followed_to_any_depth() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[merge_hit("m", "Merge of merges", &["s1"])])),
        stored("s1", json!({"consolidated_from": ["s0"]})),
        sensitive_memory("s0"),
    ]);
    let env = Env::new(&srv.url, "lineage-deep");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l2", "prompt": PROMPT}),
    );
    assert_eq!(out, "", "{out}");
    srv.assert_done();
}

#[test]
fn a_source_that_cannot_be_read_counts_as_sensitive() {
    for failure in [
        Step::get("/v1/memory/s1").status(500).raw("boom"),
        Step::get("/v1/memory/s1")
            .status(404)
            .json(json!({"error": {"code": "memory_not_found", "message": "gone"}})),
        Step::get("/v1/memory/s1").status(503).raw("busy"),
        Step::get("/v1/memory/s1").raw("not json"),
    ] {
        let srv = FakeServer::start(vec![
            Step::post("/v1/recall").json(recall(&[
                merge_hit("m", "Unverifiable merge", &["s1"]),
                hit_in("plain", "default", "A plain fact", &[&cos("0.8")]),
            ])),
            failure,
        ]);
        let env = Env::new(&srv.url, "lineage-fail");
        let out = env.hook(
            "prompt",
            "plain",
            &json!({"session_id": "l3", "prompt": PROMPT}),
        );
        assert!(
            out.contains("A plain fact") && !out.contains("Unverifiable"),
            "{out}"
        );
        srv.assert_done();
    }
}

#[test]
fn a_source_that_answers_too_late_counts_as_sensitive() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[
            merge_hit("m", "Slow merge", &["s1"]),
            hit_in("plain", "default", "A plain fact", &[&cos("0.8")]),
        ])),
        stored("s1", json!({})).delay(Duration::from_secs(5)),
    ]);
    let env = Env::new(&srv.url, "lineage-slow");
    let started = std::time::Instant::now();
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l4", "prompt": PROMPT}),
    );
    assert!(
        out.contains("A plain fact") && !out.contains("Slow merge"),
        "{out}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the prompt did not wait for it"
    );
}

#[test]
fn a_merge_with_too_many_sources_is_not_worth_checking() {
    let names: Vec<String> = (0..31).map(|i| format!("s{i}")).collect();
    let sources: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut steps =
        vec![Step::post("/v1/recall").json(recall(&[merge_hit("m", "Huge merge", &sources)]))];
    steps.extend(names[..30].iter().map(|n| stored(n, json!({}))));
    let srv = FakeServer::start(steps);
    let env = Env::new(&srv.url, "lineage-wide");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l5", "prompt": PROMPT}),
    );
    assert_eq!(out, "");
    srv.assert_done();

    let few: Vec<&str> = sources[..30].to_vec();
    let mut steps =
        vec![Step::post("/v1/recall").json(recall(&[merge_hit("m", "Wide merge", &few)]))];
    steps.extend(names[..30].iter().map(|n| stored(n, json!({}))));
    let srv = FakeServer::start(steps);
    let env = Env::new(&srv.url, "lineage-30");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l6", "prompt": PROMPT}),
    );
    assert!(
        out.contains("Wide merge"),
        "thirty sources are checked: {out}"
    );
    srv.assert_done();
}

#[test]
fn a_shared_source_is_read_once() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(recall(&[
            merge_hit("m1", "First merge", &["s1"]),
            merge_hit("m2", "Second merge", &["s1", "s1"]),
        ])),
        stored("s1", json!({})),
    ]);
    let env = Env::new(&srv.url, "lineage-cache");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l7", "prompt": PROMPT}),
    );
    assert!(
        out.contains("First merge") && out.contains("Second merge"),
        "{out}"
    );
    srv.assert_done();
}

#[test]
fn only_the_rows_about_to_be_injected_are_checked() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(recall(&[
        hit_in("a", "default", "First fact", &[&cos("0.9")]),
        merge_hit("m", "Unchecked merge", &["s1"]),
    ]))]);
    let env = Env::new(&srv.url, "lineage-lazy").set("YREM_HOOK_TOP_K", "1");
    let out = env.hook(
        "prompt",
        "plain",
        &json!({"session_id": "l8", "prompt": PROMPT}),
    );
    assert!(
        out.contains("First fact") && !out.contains("Unchecked"),
        "{out}"
    );
    srv.assert_done();
}

#[test]
fn a_decision_merged_from_a_sensitive_memory_is_replaced_by_the_next_row() {
    let mut steps = vec![
        Step::get("/v1/session/digest").json(json!({})),
        Step::get(LIST_ALL).json(records(&[
            {
                let mut m = row(
                    "m",
                    "default",
                    &format!("Poisoned merge. {}", "Merged note. ".repeat(60)),
                    1.0,
                );
                m["metadata"] = json!({"consolidated_from": ["s1"]});
                m
            },
            row("d2", "default", "Older clean decision", 2.0),
            {
                let mut m = row(
                    "m3",
                    "default",
                    &format!("Unreadable merge. {}", "Merged note. ".repeat(60)),
                    1.5,
                );
                m["metadata"] = json!({"consolidated_from": ["s9"]});
                m
            },
        ])),
        sensitive_memory("s1"),
    ];
    steps.push(Step::get("/v1/memory/s9").status(500).raw("boom"));
    let srv = FakeServer::start_unordered(steps);
    let env = Env::new(&srv.url, "lineage-dec")
        .set("YREM_HOOK_MAINTENANCE", "0")
        .set("YREM_HOOK_DECISIONS", "1");
    let out = env.hook("session-start", "plain", &json!({"session_id": "l9"}));
    assert!(out.contains("Older clean decision"), "{out}");
    assert!(
        !out.contains("Poisoned") && !out.contains("Unreadable"),
        "{out}"
    );
    assert_eq!(env.state("l9")["seen"], json!(["d2"]));
    srv.assert_done();
}

// ---------------------------------------------------------------- the narrative head

fn narrative_briefing(
    name: &str,
    digest: Value,
    listed: &[Value],
    reads: Vec<Step>,
) -> (String, Env) {
    let mut steps = vec![
        Step::get("/v1/session/digest").json(digest),
        Step::get(LIST_ALL).json(records(listed)),
    ];
    steps.extend(reads);
    let srv = FakeServer::start_unordered(steps);
    let env = Env::new(&srv.url, name).set("YREM_HOOK_MAINTENANCE", "0");
    let out = env.hook("session-start", "plain", &json!({"session_id": "h"}));
    srv.assert_done();
    (out, env)
}

fn head(rid: Option<&str>, snippet: &str) -> Value {
    let mut h = json!({"snippet": snippet, "namespace": "default", "current_status": "active"});
    if let Some(r) = rid {
        h["rid"] = json!(r);
    }
    json!({"narrative_head": h})
}

#[test]
fn a_listed_sensitive_head_is_not_shown() {
    let mut secret = row("n0", "default", "Offline code is <redacted>", 3.0);
    secret["metadata"] = json!({"sensitive": true});
    let (out, env) = narrative_briefing(
        "nh-listed",
        head(Some("n0"), "Offline code is 12345678"),
        &[secret, row("d1", "default", "An ordinary decision", 1.0)],
        vec![],
    );
    assert!(
        !out.contains("Where things stood") && !out.contains("12345678"),
        "{out}"
    );
    assert!(out.contains("An ordinary decision"), "{out}");
    assert_eq!(env.state("h")["seen"], json!(["d1"]));
}

#[test]
fn an_unlisted_head_is_read_before_it_is_shown() {
    let (out, _) = narrative_briefing(
        "nh-clean",
        head(Some("n9"), "Payments moved to PostgreSQL"),
        &[],
        vec![stored("n9", json!({}))],
    );
    assert!(
        out.contains("Where things stood: Payments moved to PostgreSQL"),
        "{out}"
    );

    let (out, _) = narrative_briefing(
        "nh-secret",
        head(Some("n9"), "Offline code is 12345678"),
        &[],
        vec![sensitive_memory("n9")],
    );
    assert!(
        !out.contains("12345678") && !out.contains("Where things stood"),
        "{out}"
    );

    for failure in [
        Step::get("/v1/memory/n9")
            .status(404)
            .json(json!({"error": "gone"})),
        Step::get("/v1/memory/n9").status(500).raw("boom"),
    ] {
        let (out, _) = narrative_briefing(
            "nh-fail",
            head(Some("n9"), "Unverified text"),
            &[],
            vec![failure],
        );
        assert!(!out.contains("Unverified"), "{out}");
    }
}

#[test]
fn a_head_without_a_rid_or_of_another_shape_is_dropped() {
    for digest in [
        head(None, "Nobody knows where this came from"),
        json!({"narrative_head": "A plain string"}),
        json!({"narrative_head": {"rid": "", "snippet": "Empty rid"}}),
        json!({"narrative_head": {"rid": "n9"}}),
        json!({"narrative_head": 3}),
        json!({"narrative_head": null}),
    ] {
        let (out, _) = narrative_briefing(
            "nh-shape",
            digest.clone(),
            &[row("d1", "default", "A decision", 1.0)],
            vec![],
        );
        assert!(!out.contains("Where things stood"), "{digest}: {out}");
        assert!(out.contains("A decision"), "{out}");
    }
}

#[test]
fn a_head_that_is_a_merge_of_a_sensitive_memory_is_dropped() {
    let mut merged = row("n0", "default", "Merged head text", 3.0);
    merged["metadata"] = json!({"consolidated_from": ["s1"]});
    let open = |name: &str, listed: &[Value], reads: Vec<Step>| {
        let mut steps = vec![
            Step::get("/v1/session/digest").json(head(Some("n0"), "Merged head text")),
            Step::get(LIST_ALL).json(records(listed)),
        ];
        steps.extend(reads);
        let srv = FakeServer::start_unordered(steps);
        let env = Env::new(&srv.url, name)
            .set("YREM_HOOK_MAINTENANCE", "0")
            .set("YREM_HOOK_INJECT_CAPTURED", "1");
        let out = env.hook("session-start", "plain", &json!({"session_id": "h"}));
        srv.assert_done();
        out
    };
    let out = open("nh-merge-listed", &[merged], vec![sensitive_memory("s1")]);
    assert!(!out.contains("Merged head"), "{out}");
    let out = open(
        "nh-merge-read",
        &[],
        vec![
            stored("n0", json!({"consolidated_from": ["s1"]})),
            sensitive_memory("s1"),
        ],
    );
    assert!(!out.contains("Merged head"), "{out}");
    let out = open(
        "nh-merge-clean",
        &[],
        vec![
            stored("n0", json!({"consolidated_from": ["s1"]})),
            stored("s1", json!({})),
        ],
    );
    assert!(
        out.contains("Where things stood: Merged head text"),
        "{out}"
    );
}

#[test]
fn a_superseded_or_excluded_head_is_dropped() {
    for (field, value) in [
        ("current_status", json!("superseded")),
        ("namespace", json!("hermes:chat")),
    ] {
        let mut digest = head(Some("n0"), "Stale head text");
        digest["narrative_head"][field] = value;
        let srv = FakeServer::start_unordered(vec![
            Step::get("/v1/session/digest").json(digest),
            Step::get(LIST_ALL).json(records(&[row("n0", "default", "Stale head text", 3.0)])),
        ]);
        let env = Env::new(&srv.url, "nh-ineligible")
            .set("YREM_HOOK_MAINTENANCE", "0")
            .set("YREM_HOOK_EXCLUDE_NAMESPACES", "hermes:");
        let out = env.hook("session-start", "plain", &json!({"session_id": "h"}));
        assert!(!out.contains("Where things stood"), "{field}: {out}");
        srv.assert_done();
    }
}

// ---------------------------------------------------------------- prompts that cannot be trusted

#[test]
fn a_selection_that_quotes_its_own_closing_tag_sends_no_request() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "ide-probe");
    let prompt = "<ide_selection>The user selected lines 10 to 12 from /x/y.rs:\n\
        assert!(s.contains(\"</ide_selection>\")); let pw = \"hunter2xyz secret line\";\n\
        </ide_selection>\nwhy does this assertion fail on the second run?";
    assert_eq!(
        env.hook(
            "prompt",
            "claude",
            &json!({"session_id": "ip", "prompt": prompt})
        ),
        ""
    );
    srv.assert_done();
}

// ---------------------------------------------------------------- headless: no session, no think

#[test]
fn headless_sessions_open_no_tracked_session_and_run_no_think() {
    let srv = FakeServer::start(vec![]);
    let env = Env::new(&srv.url, "sdk-track")
        .set("CLAUDE_CODE_ENTRYPOINT", "sdk-cli")
        .set("YREM_HOOK_TRACK_SESSION", "1")
        .set("YREM_HOOK_THINK", "1")
        .set("YREM_HOOK_DIGEST", "0");
    let ev = json!({"session_id": "hs1"});
    assert_eq!(env.hook("session-start", "claude", &ev), "");
    assert_eq!(env.state("hs1")["server_session"], Value::Null);
    assert_eq!(env.hook("session-end", "claude", &ev), "");
    srv.assert_done();
    assert!(srv.requests().is_empty(), "{:?}", srv.calls());
}

#[test]
fn a_listing_that_never_answers_costs_the_decisions_not_the_briefing() {
    let srv = FakeServer::start_unordered(vec![
        Step::get("/v1/session/digest").json(digest()),
        Step::get(LIST_ALL)
            .delay(Duration::from_secs(12))
            .json(records(&[narrative_row()])),
        stored("n0", json!({})).delay(Duration::from_secs(12)),
    ]);
    let env = Env::new(&srv.url, "late-list");
    let started = std::time::Instant::now();
    let (out, err) = env.run(
        &["hook", "session-start", "--harness", "plain"],
        &json!({"session_id": "j10"}),
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(7) && took < Duration::from_secs(11),
        "{took:?}"
    );
    assert!(
        out.contains("Open conflicts: 2"),
        "what the digest gave is printed: {out}"
    );
    assert!(
        !out.contains("Live decisions") && !out.contains("Where things stood"),
        "{out}"
    );
    assert!(err.contains("timed out"), "{err}");
    assert!(
        !env.dir.join("down").exists(),
        "a late listing is not a dead server"
    );
}

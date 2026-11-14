//! The `yrem` binary end to end: every command against the fake server, stdin handling,
//! output formats, exit codes, `--help` goldens, `doctor` and config files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::assert_is_empty
)]

mod support;

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};

const BIN: &str = env!("CARGO_BIN_EXE_yrem");
const OLD_RID: &str = "01a18e8a-aa4d-786a-a95e-2944a7bb906a";
const NEW_RID: &str = "01a18d90-0000-7000-8000-000000000001";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not JSON ({e}): {:?}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }
}

/// A `yrem` command with a clean, hermetic environment pointing at `url`.
fn base(url: Option<&str>) -> Command {
    let mut c = Command::new(BIN);
    c.env("XDG_CONFIG_HOME", "/nonexistent")
        .env_remove("YANTRIKDB_SERVER_URL")
        .env_remove("YANTRIKDB_TOKEN")
        .env_remove("YREM_CONFIG")
        .env_remove("YREM_URL")
        .env_remove("YREM_TOKEN")
        .env_remove("RUST_LOG")
        .env("NO_COLOR", "1");
    if let Some(url) = url {
        c.env("YREM_URL", url).env("YREM_TOKEN", TOKEN);
    }
    c
}

fn run(mut cmd: Command, stdin: Option<&str>) -> Out {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(input.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    Out {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn yrem(srv: &FakeServer, args: &[&str]) -> Out {
    let mut c = base(Some(&srv.url));
    c.args(args);
    run(c, None)
}

fn yrem_stdin(srv: &FakeServer, args: &[&str], stdin: &str) -> Out {
    let mut c = base(Some(&srv.url));
    c.args(args);
    run(c, Some(stdin))
}

/// A URL nothing listens on.
fn dead_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

fn ok(out: &Out) -> Value {
    assert_eq!(
        out.code, 0,
        "stdout: {}\nstderr: {}",
        out.stdout, out.stderr
    );
    out.json()
}

fn step_ok(srv: &FakeServer, args: &[&str]) -> Value {
    let out = yrem(srv, args);
    let v = ok(&out);
    srv.assert_done();
    v
}

// ------------------------------------------------------------ commands

#[test]
fn remember() {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").fixture("remember")]);
    let v = step_ok(
        &srv,
        &[
            "remember",
            "Payments service uses PostgreSQL",
            "--memory-type",
            "semantic",
            "--importance",
            "0.8",
            "--valence",
            "-0.5",
            "--certainty",
            "0.9",
            "--domain",
            "work",
            "--source",
            "user",
            "--namespace",
            "payments",
            "--created-at",
            "2026-01-02",
            "--idempotency-key",
            "k1",
        ],
    );
    assert_eq!(v["rid"], "01a18e8a-aa4d-786a-a95e-2944a7bb906a");
    let b = &srv.requests()[0].body;
    assert_eq!(b["text"], "Payments service uses PostgreSQL");
    assert_eq!(b["memory_type"], "semantic");
    assert_eq!(b["valence"], -0.5);
    assert_eq!(b["namespace"], "payments");
    assert_eq!(b["created_at"].as_f64(), Some(1_767_312_000.0));
    assert_eq!(b["idempotency_key"], "k1");
}

#[test]
fn remember_stdin() {
    let srv = FakeServer::start(vec![
        Step::no_duplicate(),
        Step::post("/v1/remember").fixture("remember_minimal"),
    ]);
    let out = yrem_stdin(
        &srv,
        &["remember", "--stdin"],
        "multi word\nsecond line é🦀\n\n",
    );
    ok(&out);
    srv.assert_done();
    assert_eq!(
        srv.requests()[1].body["text"],
        "multi word\nsecond line é🦀"
    );
}

#[test]
fn remember_stdin_conflicts_with_text() {
    let srv = FakeServer::start(vec![]);
    let out = yrem_stdin(&srv, &["remember", "x", "--stdin"], "y");
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stdout.is_empty());
    assert!(srv.calls().is_empty());
}

#[test]
fn remember_requires_text() {
    let srv = FakeServer::start(vec![]);
    let out = yrem(&srv, &["remember"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("required"), "{}", out.stderr);
}

#[test]
fn recall() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").fixture("recall")]);
    let v = step_ok(
        &srv,
        &[
            "recall",
            "which database?",
            "--top-k",
            "5",
            "--namespace",
            "payments",
            "--domain",
            "work",
            "--memory-type",
            "semantic",
            "--order",
            "recency",
            "--min-score-ratio",
            "0.1",
        ],
    );
    let hits = v["results"].as_array().unwrap();
    assert!(hits.len() >= 2);
    assert!(
        hits[0]["created_at"].as_f64() >= hits[1]["created_at"].as_f64(),
        "recency order"
    );
    let top = hits[0]["score"].as_f64().unwrap();
    assert_eq!(top, (top * 1e4).round() / 1e4);
    let b = &srv.requests()[0].body;
    assert_eq!(b["query"], "which database?");
    assert_eq!(b["top_k"], 5);
}

#[test]
fn recall_filters_from_cli() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(json!({"results": [
        {"rid": "a", "score": 0.9, "created_at": 1_767_312_000.0},
        {"rid": "b", "score": 0.8, "created_at": 1_700_000_000.0},
        {"rid": "c", "score": 0.7, "created_at": 1_767_312_001.0},
    ]}))]);
    let v = step_ok(
        &srv,
        &[
            "recall",
            "q",
            "--since",
            "2026-01-01",
            "--until",
            "2026-02-01",
            "--refine-exclude",
            "c",
        ],
    );
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert_eq!(v["filtered_by_time_window"], 1);
    assert_eq!(v["filtered_by_refine_exclude"], 1);
}

#[test]
fn recall_stdin() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").fixture("recall_empty")]);
    let out = yrem_stdin(&srv, &["recall", "--stdin"], "what is the database\n");
    ok(&out);
    assert_eq!(srv.requests()[0].body["query"], "what is the database");
}

#[test]
fn recall_empty_query_is_usage_error() {
    let srv = FakeServer::start(vec![]);
    let out = yrem(&srv, &["recall"]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["exit_code"], 2);
    assert!(out.stderr.starts_with("yrem:"), "{}", out.stderr);
    assert!(srv.calls().is_empty());
}

#[test]
fn forget() {
    let srv = FakeServer::start(vec![Step::post("/v1/forget").fixture("forget")]);
    let v = step_ok(&srv, &["forget", "rid-1", "--reason", "dup"]);
    assert_eq!(v["found"], true);
    let b = &srv.requests()[0].body;
    assert_eq!(b["rid"], "rid-1");
    assert_eq!(b["reason"], "dup");
}

#[test]
fn correct_native() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"new_rid": "n"})),
    ]);
    let v = step_ok(
        &srv,
        &[
            "correct",
            OLD_RID,
            "--reason",
            "changed",
            "--new-text",
            "t",
            "--new-importance",
            "0.9",
            "--new-valence",
            "-0.3",
        ],
    );
    assert_eq!(v["emulated"], false);
    let b = &srv.requests()[0].body;
    assert_eq!(b["new_importance"], 0.9);
    assert_eq!(b["new_valence"], -0.3);
}

#[test]
fn correct_emulated() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&format!("/v1/memory/{OLD_RID}")).fixture("memory_get"),
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").json(json!({"found": true})),
    ]);
    let v = step_ok(
        &srv,
        &[
            "correct",
            OLD_RID,
            "--reason",
            "upgraded",
            "--new-text",
            "PG 16",
        ],
    );
    assert_eq!(
        v,
        json!({"old_rid": OLD_RID, "new_rid": NEW_RID, "emulated": true})
    );
}

#[test]
fn correct_partial_exits_6() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&format!("/v1/memory/{OLD_RID}")).fixture("memory_get"),
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").status(500).raw("disk full"),
    ]);
    let out = yrem(&srv, &["correct", OLD_RID, "--reason", "r"]);
    assert_eq!(out.code, 6, "{}", out.stderr);
    let v = out.json();
    assert_eq!(v["exit_code"], 6);
    let msg = v["error"].as_str().unwrap();
    assert!(msg.contains(OLD_RID) && msg.contains(NEW_RID), "{msg}");
    assert!(out.stderr.starts_with("yrem:"), "{}", out.stderr);
}

#[test]
fn think() {
    let srv = FakeServer::start(vec![Step::post("/v1/think").fixture("think")]);
    let v = step_ok(&srv, &["think"]);
    assert_eq!(v["conflicts_found"], 1);
}

#[test]
fn graph_relate() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/relate")
            .body(|b| {
                b["entity"] == "Alice" && b["target"] == "Payments" && b["relationship"] == "leads"
            })
            .fixture("relate"),
    ]);
    let v = step_ok(
        &srv,
        &[
            "graph",
            "relate",
            "--src",
            "Alice",
            "--dst",
            "Payments",
            "--rel-type",
            "leads",
            "--weight",
            "0.9",
        ],
    );
    assert!(v["edge_id"].is_string());
}

#[test]
fn graph_edges_unsupported_exits_4() {
    let srv = FakeServer::start(vec![Step::post("/v1/graph").fixture("graph_edges")]);
    let out = yrem(&srv, &["graph", "edges", "--entity", "Alice"]);
    assert_eq!(out.code, 4, "{}", out.stderr);
    let v = out.json();
    assert_eq!(v["exit_code"], 4);
    assert!(v["error"].as_str().unwrap().contains("graph_edges"));
    assert!(out.stderr.starts_with("yrem:"));
    srv.assert_done();
}

#[test]
fn conflict_commands() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/conflicts?limit=3&status=open").fixture("conflicts"),
        Step::post("/v1/conflicts/c1/resolve")
            .body(|b| b["strategy"] == "keep_b" && b["resolution_note"] == "n")
            .fixture("conflict_resolve"),
        Step::post("/v1/conflicts/c2/resolve")
            .body(|b| b["strategy"] == "merge" && b["new_text"] == "merged")
            .json(json!({"conflict_id": "c2", "strategy": "merge"})),
        Step::post("/v1/conflicts/c3/resolve")
            .body(|b| b["strategy"] == "keep_both" && b["resolution_note"] == "dismissed")
            .json(json!({"conflict_id": "c3", "strategy": "keep_both"})),
    ]);
    let v = ok(&yrem(
        &srv,
        &["conflict", "list", "--status", "open", "--limit", "3"],
    ));
    assert_eq!(v["conflicts"].as_array().unwrap().len(), 1);
    ok(&yrem(
        &srv,
        &[
            "conflict",
            "resolve",
            "c1",
            "--strategy",
            "keep_b",
            "--note",
            "n",
        ],
    ));
    ok(&yrem(
        &srv,
        &[
            "conflict",
            "resolve",
            "c2",
            "--strategy",
            "merge",
            "--new-text",
            "merged",
        ],
    ));
    let v = ok(&yrem(&srv, &["conflict", "dismiss", "c3"]));
    assert_eq!(v["strategy"], "keep_both");
    srv.assert_done();
}

#[test]
fn conflict_merge_without_text_is_usage_error() {
    let srv = FakeServer::start(vec![]);
    let out = yrem(&srv, &["conflict", "resolve", "c1", "--strategy", "merge"]);
    assert_eq!(out.code, 2);
    assert!(srv.calls().is_empty());
}

#[test]
fn session_digest_and_alias() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/session/digest?include_gaps=true&namespace=payments")
            .fixture("session_digest"),
        Step::get("/v1/session/digest").fixture("session_digest_default"),
    ]);
    let v = ok(&yrem(
        &srv,
        &[
            "session",
            "digest",
            "--namespace",
            "payments",
            "--include-gaps",
        ],
    ));
    assert!(v["knowledge_gaps"].is_array());
    let v = ok(&yrem(&srv, &["digest"]));
    assert!(v["pending_triggers"].is_array());
    srv.assert_done();
}

#[test]
fn session_capture_arg_and_stdin() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/session/end").fixture("session_end"),
        Step::post("/v1/session/end").fixture("session_end"),
    ]);
    let v = ok(&yrem(
        &srv,
        &[
            "session",
            "capture",
            "We reviewed.",
            "--namespace",
            "payments",
            "--domain",
            "work",
        ],
    ));
    assert_eq!(v["count"], 2);
    ok(&yrem_stdin(
        &srv,
        &["session", "capture", "--stdin"],
        "from stdin\n",
    ));
    srv.assert_done();
    let reqs = srv.requests();
    assert_eq!(reqs[0].body["summary"], "We reviewed.");
    assert_eq!(reqs[0].body["namespace"], "payments");
    assert_eq!(reqs[1].body["summary"], "from stdin");
}

#[test]
fn session_start_and_end() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/sessions")
            .body(|b| b["namespace"] == "payments" && b["client_id"] == "cli")
            .fixture("session_start"),
        Step::delete("/v1/sessions/01a18e8a-af32-70a3-b823-f881004b4a9d")
            .body(|b| b["summary"] == "done")
            .fixture("session_delete"),
    ]);
    let v = ok(&yrem(
        &srv,
        &[
            "session",
            "start",
            "--namespace",
            "payments",
            "--client-id",
            "cli",
        ],
    ));
    assert_eq!(v["session_id"], "01a18e8a-af32-70a3-b823-f881004b4a9d");
    let v = ok(&yrem(
        &srv,
        &[
            "session",
            "end",
            "01a18e8a-af32-70a3-b823-f881004b4a9d",
            "--summary",
            "done",
        ],
    ));
    assert_eq!(v["memory_count"], 2);
    srv.assert_done();
}

#[test]
fn memory_commands() {
    let srv = FakeServer::start(vec![
        Step::get(&format!("/v1/memory/{OLD_RID}")).fixture("memory_get"),
        Step::get("/v1/current?namespace=payments").fixture("current"),
        Step::get("/v1/memories?domain=work&limit=2&order=desc").fixture("memories_desc"),
        Step::get("/v1/insights/gaps?namespace=payments").fixture("gaps"),
        Step::get("/v1/stats").fixture("stats"),
    ]);
    let v = ok(&yrem(&srv, &["memory", "get", OLD_RID]));
    assert_eq!(v["memory_type"], "semantic");
    assert!(v.get("metadata_json").is_none());
    let v = ok(&yrem(
        &srv,
        &["memory", "current", "--namespace", "payments"],
    ));
    assert_eq!(v["found"], true);
    let v = ok(&yrem(
        &srv,
        &[
            "memory", "list", "--domain", "work", "--limit", "2", "--desc",
        ],
    ));
    assert_eq!(v["records"].as_array().unwrap().len(), 2);
    let v = ok(&yrem(&srv, &["memory", "gaps", "--namespace", "payments"]));
    assert_eq!(v["count"], 1);
    let v = ok(&yrem(&srv, &["memory", "stats"]));
    assert_eq!(v["active_memories"], 0);
    srv.assert_done();
}

// ------------------------------------------------------------ call

#[test]
fn call_with_json_argument_and_batch() {
    let srv = FakeServer::start(vec![
        Step::no_duplicate(),
        Step::post("/v1/remember").fixture("remember"),
        Step::post("/v1/remember/batch").fixture("remember_batch"),
    ]);
    let v = ok(&yrem(
        &srv,
        &["call", "remember", r#"{"text":"via call","importance":2}"#],
    ));
    assert!(v["rid"].is_string());
    let batch = r#"{"memories":[{"text":"a"},{"text":"b","memory_type":"episodic"}]}"#;
    let v = ok(&yrem(&srv, &["call", "remember", batch]));
    assert_eq!(v["count"], 4);
    srv.assert_done();
    let reqs = srv.requests();
    assert_eq!(reqs[1].body["importance"], 1.0);
    assert_eq!(reqs[2].body["memories"].as_array().unwrap().len(), 2);
}

#[test]
fn call_recall_prints_what_recall_prints() {
    let answer = json!({"results": [{"rid": "r1", "text": "t".repeat(500), "score": 0.5,
                                     "metadata": {"a": 1}}]});
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").json(answer.clone()),
        Step::post("/v1/recall").json(answer.clone()),
        Step::post("/v1/recall").json(answer),
    ]);
    let direct = ok(&yrem(&srv, &["recall", "q"]));
    let via_call = ok(&yrem(&srv, &["call", "recall", r#"{"query":"q"}"#]));
    assert_eq!(via_call, direct, "only the MCP tool shortens hits");
    assert_eq!(direct["results"][0]["text"].as_str().unwrap().len(), 500);
    let full = ok(&yrem(
        &srv,
        &["call", "recall", r#"{"query":"q","full":true}"#],
    ));
    assert_eq!(full, direct, "`full` is accepted and changes nothing here");
    srv.assert_done();
}

#[test]
fn call_json_on_stdin() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").fixture("recall_empty")]);
    let out = yrem_stdin(&srv, &["call", "recall"], r#"{"query": "from stdin"}"#);
    ok(&out);
    assert_eq!(srv.requests()[0].body["query"], "from stdin");
}

#[test]
fn call_without_args_uses_empty_object() {
    let srv = FakeServer::start(vec![Step::post("/v1/think").fixture("think")]);
    let out = yrem_stdin(&srv, &["call", "think"], "   \n");
    ok(&out);
}

#[test]
fn call_metadata_merge_via_json() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"new_rid": "n"})),
    ]);
    ok(&yrem(
        &srv,
        &[
            "call",
            "correct",
            r#"{"rid":"r","reason":"x","metadata_merge":{"k":[1,2]}}"#,
        ],
    ));
    assert_eq!(
        srv.requests()[0].body["metadata_merge"],
        json!({"k": [1, 2]})
    );
}

#[test]
fn call_errors_are_usage_errors() {
    let srv = FakeServer::start(vec![]);
    for args in [
        vec!["call", "nope", "{}"],
        vec!["call", "recall", "not json"],
        vec!["call", "remember", r#"{"text": 5}"#],
        vec!["call", "remember", r#"{"text": "x", "memories": []}"#],
    ] {
        let out = yrem(&srv, &args);
        assert_eq!(out.code, 2, "{args:?}: {}", out.stderr);
        assert_eq!(out.json()["exit_code"], 2);
        assert!(out.stderr.starts_with("yrem:"), "{}", out.stderr);
    }
    assert!(srv.calls().is_empty());
}

// ------------------------------------------------------------ output format

fn one_line(s: &str) -> bool {
    s.ends_with('\n') && !s.trim_end().contains('\n')
}

#[test]
fn piped_output_is_compact() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    let out = yrem(&srv, &["memory", "stats"]);
    assert!(one_line(&out.stdout), "{:?}", out.stdout);
    ok(&out);
}

#[test]
fn pretty_flag_is_multiline() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    let out = yrem(&srv, &["--pretty", "memory", "stats"]);
    assert!(
        out.stdout.trim_end().lines().count() > 3,
        "{:?}",
        out.stdout
    );
    assert!(out.stdout.contains("\n  \""), "{:?}", out.stdout);
    ok(&out);
}

#[test]
fn compact_flag_and_conflict_with_pretty() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    let out = yrem(&srv, &["memory", "stats", "--compact"]);
    assert!(one_line(&out.stdout));
    let out = yrem(&srv, &["--pretty", "--compact", "memory", "stats"]);
    assert_eq!(out.code, 2);
}

fn script_available() -> bool {
    Command::new("script")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Runs `yrem args` with a pseudo-terminal as stdout.
fn tty(srv: &FakeServer, args: &str) -> Option<String> {
    if !script_available() {
        eprintln!("skipping: util-linux `script` not available");
        return None;
    }
    // `script` runs the command in a pty and passes its environment through.
    let mut c = Command::new("script");
    c.args(["-qec", &format!("{BIN} {args}"), "/dev/null"])
        .env("YREM_URL", &srv.url)
        .env("YREM_TOKEN", TOKEN)
        .env("XDG_CONFIG_HOME", "/nonexistent")
        .env("NO_COLOR", "1")
        .env_remove("YANTRIKDB_SERVER_URL")
        .env_remove("YANTRIKDB_TOKEN")
        .env_remove("YREM_CONFIG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = c.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    if text.trim().is_empty() {
        eprintln!(
            "skipping: `script` produced no output: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(text)
}

#[test]
fn tty_output_is_pretty_and_compact_flag_overrides() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/stats").fixture("stats"),
        Step::get("/v1/stats").fixture("stats"),
    ]);
    let Some(text) = tty(&srv, "memory stats") else {
        return;
    };
    assert!(text.trim_end().lines().count() > 3, "{text:?}");
    serde_json::from_str::<Value>(&text).unwrap();
    let Some(text) = tty(&srv, "memory stats --compact") else {
        return;
    };
    assert!(one_line(&text), "{text:?}");
    srv.assert_done();
}

// ------------------------------------------------------------ exit codes

fn error_case(status: u16, body: &str, args: &[&str], code: i32) {
    let srv = FakeServer::start(vec![
        Step::get("/v1/stats").status(status).raw(body),
        Step::get("/v1/stats").status(status).raw(body),
    ]);
    let out = yrem(&srv, args);
    assert_eq!(
        out.code, code,
        "stdout {} stderr {}",
        out.stdout, out.stderr
    );
    let v = out.json();
    assert_eq!(v["exit_code"], code);
    assert!(v["error"].is_string());
    assert!(out.stderr.starts_with("yrem:"), "{}", out.stderr);
    assert!(one_line(&out.stdout));
}

#[test]
fn exit_code_0() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    assert_eq!(yrem(&srv, &["memory", "stats"]).code, 0);
}

#[test]
fn exit_code_1_server_error() {
    error_case(500, r#"{"error":"boom"}"#, &["memory", "stats"], 1);
}

#[test]
fn exit_code_1_not_found() {
    let srv = FakeServer::start(vec![Step::get("/v1/memory/nope").fixture("memory_get_404")]);
    let out = yrem(&srv, &["memory", "get", "nope"]);
    assert_eq!(out.code, 1);
    assert!(out.json()["error"].as_str().unwrap().contains("not found"));
}

#[test]
fn exit_code_2_clap_usage() {
    let srv = FakeServer::start(vec![]);
    for args in [
        vec![],
        vec!["bogus"],
        vec!["recall", "q", "--top-k", "many"],
        vec!["recall", "q", "--order", "sideways"],
        vec!["graph", "relate", "--src", "a"],
        vec!["--version-nope"],
    ] {
        let out = yrem(&srv, &args);
        assert_eq!(out.code, 2, "{args:?}");
        assert!(
            out.stdout.is_empty(),
            "clap errors go to stderr only: {:?}",
            out.stdout
        );
        assert!(
            out.stderr.contains("Usage") || out.stderr.contains("error"),
            "{}",
            out.stderr
        );
    }
    assert!(srv.calls().is_empty());
}

#[test]
fn exit_code_2_ops_validation() {
    let srv = FakeServer::start(vec![]);
    for args in [
        vec!["recall", "q", "--top-k", "0"],
        vec!["recall", "q", "--min-score-ratio", "0"],
        vec![
            "recall",
            "q",
            "--since",
            "2026-02-01",
            "--until",
            "2026-01-01",
        ],
        vec!["recall", "q", "--since", "whenever"],
        vec!["remember", "  "],
        vec!["forget", " "],
        vec!["correct", "r", "--reason", " "],
        vec!["memory", "current", "--namespace", ""],
    ] {
        let out = yrem(&srv, &args);
        assert_eq!(out.code, 2, "{args:?}: {} / {}", out.stdout, out.stderr);
        let v = out.json();
        assert_eq!(v["exit_code"], 2);
        assert!(v["error"].as_str().unwrap().starts_with("invalid argument"));
        assert!(out.stderr.starts_with("yrem:"));
    }
    assert!(srv.calls().is_empty());
}

#[test]
fn exit_code_3_auth() {
    error_case(
        401,
        r#"{"error":{"code":"unauthenticated","message":"invalid or unknown token"}}"#,
        &["memory", "stats"],
        3,
    );
}

#[test]
fn exit_code_4_unsupported() {
    let srv = FakeServer::start(vec![Step::post("/v1/think").status(501).json(json!({
        "error": "not_yet_available_over_http", "operation": "think", "reason": "later"
    }))]);
    let out = yrem(&srv, &["think"]);
    assert_eq!(out.code, 4);
    assert!(out.json()["error"].as_str().unwrap().contains("think"));
}

#[test]
fn exit_code_5_connection_refused() {
    let url = dead_url();
    let mut c = base(Some(&url));
    c.args(["memory", "stats"]);
    let out = run(c, None);
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert_eq!(out.json()["exit_code"], 5);
    assert!(out.stderr.starts_with("yrem:"));
}

#[test]
fn exit_code_5_service_unavailable() {
    error_case(
        503,
        r#"{"error":"read-only: not the leader"}"#,
        &["memory", "stats"],
        5,
    );
}

// ------------------------------------------------------------ v0.2 commands

#[test]
fn temporal_commands() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/temporal/stale?days=0&limit=2&namespace=ops")
            .fixture("temporal_stale_params"),
        Step::get("/v1/temporal/upcoming?days=30").fixture("temporal_upcoming"),
        Step::post("/v1/temporal/as_of")
            .body(|b| b["as_of"] == 4_000_000_000.0 && b["top_k"] == 2 && b["query"] == "q1")
            .fixture("temporal_as_of"),
        Step::post("/v1/temporal/as_of")
            .body(|b| b["query"] == "from stdin" && b["memory_type"] == "procedural")
            .fixture("temporal_as_of_empty"),
    ]);
    let v = ok(&yrem(
        &srv,
        &[
            "temporal",
            "stale",
            "--days",
            "0",
            "--limit",
            "2",
            "--namespace",
            "ops",
        ],
    ));
    assert_eq!(v["count"], 2);
    let v = ok(&yrem(&srv, &["temporal", "upcoming", "--days", "30"]));
    assert_eq!(v["count"], 0);
    let v = ok(&yrem(
        &srv,
        &[
            "temporal",
            "as-of",
            "q1",
            "--at",
            "4000000000",
            "--top-k",
            "2",
        ],
    ));
    assert_eq!(v["count"], 2);
    let out = yrem_stdin(
        &srv,
        &[
            "temporal",
            "as-of",
            "--stdin",
            "--at",
            "2026-01-02",
            "--memory-type",
            "procedural",
        ],
        "from stdin\n",
    );
    ok(&out);
    srv.assert_done();
}

#[test]
fn temporal_as_of_usage_errors() {
    let srv = FakeServer::start(vec![]);
    assert_eq!(yrem(&srv, &["temporal", "as-of", "q"]).code, 2);
    let out = yrem(&srv, &["temporal", "as-of", "q", "--at", "soon"]);
    assert_eq!(out.code, 2);
    let out = yrem(
        &srv,
        &["temporal", "as-of", "q", "--at", "7d", "--top-k", "0"],
    );
    assert_eq!(out.code, 2);
    assert!(srv.calls().is_empty());
}

#[test]
fn trigger_commands() {
    let id = "01a18e8a-a8c7-7a8f-b1df-ff839152012f";
    let srv = FakeServer::start(vec![
        Step::get("/v1/triggers?limit=1").fixture("triggers_limit"),
        Step::get("/v1/triggers/history?limit=5&trigger_type=decay_review")
            .fixture("triggers_history_filtered"),
        Step::post(&format!("/v1/triggers/{id}/deliver")).fixture("trigger_deliver"),
        Step::post(&format!("/v1/triggers/{id}/acknowledge")).fixture("trigger_acknowledge"),
        Step::post(&format!("/v1/triggers/{id}/act")).fixture("trigger_act"),
        Step::post(&format!("/v1/triggers/{id}/dismiss")).fixture("trigger_dismiss"),
        Step::post("/v1/triggers/nope/act").fixture("trigger_act_404"),
    ]);
    assert_eq!(
        ok(&yrem(&srv, &["trigger", "list", "--limit", "1"]))["count"],
        1
    );
    let v = ok(&yrem(
        &srv,
        &[
            "trigger",
            "history",
            "--limit",
            "5",
            "--trigger-type",
            "decay_review",
        ],
    ));
    assert_eq!(v["count"], 1);
    for action in ["deliver", "acknowledge", "act", "dismiss"] {
        let v = ok(&yrem(&srv, &["trigger", action, id]));
        assert_eq!(v["changed"], true, "{action}");
    }
    let out = yrem(&srv, &["trigger", "act", "nope"]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    srv.assert_done();
}

#[test]
fn claim_and_procedure_and_task_commands() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/claim")
            .body(|b| {
                b["src"] == "Bob"
                    && b["rel_type"] == "reviews"
                    && b["dst"] == "Zephyr"
                    && b["valid_from"] == 1_790_000_000.0
                    && b["polarity"] == -1
                    && b["weight"] == 0.8
                    && b["confidence_band"] == "high"
            })
            .fixture("claim"),
        Step::get("/v1/claims?entity=Bob&namespace=ops").fixture("claims"),
        Step::post("/v1/procedures/surface")
            .body(|b| b["query"] == "roll back" && b["top_k"] == 3 && b["domain"] == "deploy")
            .fixture("procedures_surface"),
        Step::get("/v1/tasks?namespace=ops&status=pending").fixture("tasks_filtered"),
    ]);
    let v = ok(&yrem(
        &srv,
        &[
            "claim",
            "add",
            "Bob",
            "reviews",
            "Zephyr",
            "--valid-from",
            "1790000000",
            "--polarity",
            "-1",
            "--weight",
            "0.8",
            "--confidence-band",
            "high",
        ],
    ));
    assert!(v["claim_id"].is_string());
    let v = ok(&yrem(&srv, &["claim", "list", "Bob", "--namespace", "ops"]));
    assert_eq!(v["claims"][0]["src"], "Bob");
    let v = ok(&yrem(
        &srv,
        &[
            "procedure",
            "surface",
            "roll back",
            "--top-k",
            "3",
            "--domain",
            "deploy",
        ],
    ));
    assert_eq!(v["count"], 1);
    let v = ok(&yrem(
        &srv,
        &["task", "list", "--namespace", "ops", "--status", "pending"],
    ));
    assert_eq!(v["count"], 0);
    srv.assert_done();
}

// ------------------------------------------------------------ help goldens

fn golden(name: &str, args: &[&str]) {
    let mut c = base(None);
    c.args(args);
    let out = run(c, None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "golden", name]
        .iter()
        .collect();
    if std::env::var_os("YREM_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &out.stdout).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with YREM_BLESS=1", path.display()));
    assert_eq!(
        out.stdout, expected,
        "{name} changed; rerun with YREM_BLESS=1"
    );
}

#[test]
fn help_snapshot() {
    golden("help.txt", &["--help"]);
}

#[test]
fn help_recall_snapshot() {
    golden("help_recall.txt", &["recall", "--help"]);
}

#[test]
fn every_argument_and_subcommand_is_documented() {
    use clap::CommandFactory;

    fn walk(cmd: &clap::Command, path: &str, missing: &mut Vec<String>) {
        for arg in cmd.get_arguments() {
            let id = arg.get_id().as_str();
            if matches!(id, "help" | "version") {
                continue;
            }
            if arg.get_help().is_none() && arg.get_long_help().is_none() {
                missing.push(format!("{path} argument `{id}`"));
            }
        }
        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            let sub_path = format!("{path} {name}");
            if name != "help" && sub.get_about().is_none() && sub.get_long_about().is_none() {
                missing.push(format!("{sub_path} (no about)"));
            }
            walk(sub, &sub_path, missing);
        }
    }

    let mut missing = Vec::new();
    walk(&yrem::cli::Cli::command(), "yrem", &mut missing);
    assert!(missing.is_empty(), "undocumented: {missing:#?}");
}

// ------------------------------------------------------------ doctor

#[test]
fn doctor_reports_unsupported_and_redacts_token() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/health/deep").fixture("health_deep"),
        Step::get("/v1/conflicts?limit=1").fixture("conflicts"),
        Step::post("/v1/correct").fixture("correct"),
        Step::post("/v1/graph").fixture("graph_edges"),
    ]);
    let out = yrem(&srv, &["doctor"]);
    let v = ok(&out);
    srv.assert_done();
    assert!(!out.stdout.contains(TOKEN), "token leaked: {}", out.stdout);
    assert!(!out.stderr.contains(TOKEN));
    assert_eq!(v["ok"], true);
    assert_eq!(v["config"]["token"], "<redacted>");
    assert_eq!(v["config"]["url"], srv.url);
    assert_eq!(v["connectivity"]["ok"], true);
    assert_eq!(v["auth"]["ok"], true);
    assert_eq!(v["server_version"]["build_id"], "0.20.0");
    let unsupported: Vec<&str> = v["unsupported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(unsupported.contains(&"correct"), "{unsupported:?}");
    assert!(unsupported.contains(&"graph_edges"), "{unsupported:?}");
}

#[test]
fn doctor_all_supported() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/health/deep").fixture("health_deep"),
        Step::get("/v1/conflicts?limit=1").fixture("conflicts"),
        Step::post("/v1/correct")
            .status(400)
            .json(json!({"error": "unknown rid"})),
        Step::post("/v1/graph").json(json!({"edges": []})),
    ]);
    let v = ok(&yrem(&srv, &["doctor"]));
    assert_eq!(v["unsupported"], json!([]));
    assert_eq!(v["ok"], true);
}

#[test]
fn doctor_bad_token_reports_auth_not_ok() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/health/deep").fixture("health_deep"),
        Step::get("/v1/conflicts?limit=1").fixture("unauthorized"),
        Step::post("/v1/correct").fixture("correct"),
        Step::post("/v1/graph").fixture("graph_edges"),
    ]);
    let out = yrem(&srv, &["doctor"]);
    let v = ok(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["connectivity"]["ok"], true);
    assert_eq!(v["auth"]["ok"], false);
}

#[test]
fn doctor_server_down_is_ok_false_exit_0() {
    let url = dead_url();
    let mut c = base(Some(&url));
    c.arg("doctor");
    let out = run(c, None);
    let v = ok(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["connectivity"]["ok"], false);
    assert_eq!(v["unsupported"], json!([]));
    assert!(!out.stdout.contains(TOKEN));
}

// ------------------------------------------------------------ config file

fn temp_dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "yrem-cli-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn with_config(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn config_file_with_token_cmd() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    let dir = temp_dir("tokencmd");
    let path = with_config(
        &dir,
        &format!("url = \"{}\"\ntoken_cmd = \"printf {TOKEN}\"\n", srv.url),
    );
    let mut c = base(None);
    c.arg("--config").arg(&path).args(["memory", "stats"]);
    let out = run(c, None);
    ok(&out);
    srv.assert_done();
    assert_eq!(
        srv.requests()[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn config_file_plain_token_and_doctor_origin() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/health/deep").fixture("health_deep"),
        Step::get("/v1/conflicts?limit=1").fixture("conflicts"),
        Step::post("/v1/correct").fixture("correct"),
        Step::post("/v1/graph").fixture("graph_edges"),
    ]);
    let dir = temp_dir("plain");
    let path = with_config(
        &dir,
        &format!("url = \"{}\"\ntoken = \"{TOKEN}\"\n", srv.url),
    );
    let mut c = base(None);
    c.arg("--config").arg(&path).arg("doctor");
    let out = run(c, None);
    let v = ok(&out);
    assert!(!out.stdout.contains(TOKEN));
    assert_eq!(v["config"]["url_from"], "file");
    assert_eq!(v["config"]["token_from"], "file");
    assert_eq!(v["config"]["file"], path.display().to_string());
    srv.assert_done();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn config_file_via_env_var() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    let dir = temp_dir("envcfg");
    let path = with_config(
        &dir,
        &format!("url = \"{}\"\ntoken = \"{TOKEN}\"\n", srv.url),
    );
    let mut c = base(None);
    c.env("YREM_CONFIG", &path).args(["memory", "stats"]);
    ok(&run(c, None));
    srv.assert_done();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn flags_override_config_file() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats")]);
    let dir = temp_dir("override");
    let path = with_config(
        &dir,
        &format!("url = \"{}\"\ntoken = \"wrong\"\n", dead_url()),
    );
    let mut c = base(None);
    c.arg("--config")
        .arg(&path)
        .args(["--url", &srv.url, "--token", TOKEN, "memory", "stats"]);
    ok(&run(c, None));
    srv.assert_done();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn config_unknown_key_is_rejected() {
    let dir = temp_dir("unknown");
    let path = with_config(&dir, "url = \"http://127.0.0.1:1\"\nbogus_key = 1\n");
    let mut c = base(None);
    c.arg("--config").arg(&path).args(["memory", "stats"]);
    let out = run(c, None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    let v = out.json();
    assert_eq!(v["exit_code"], 1);
    assert!(v["error"].as_str().unwrap().contains("bogus_key"), "{v}");
    assert!(out.stderr.starts_with("yrem:"), "{}", out.stderr);
    assert!(out.stderr.contains("bogus_key"), "{}", out.stderr);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_explicit_config_and_missing_url_exit_1() {
    let mut c = base(None);
    c.args(["--config", "/nonexistent/yrem.toml", "memory", "stats"]);
    let out = run(c, None);
    assert_eq!(out.code, 1);
    assert!(out.stderr.starts_with("yrem:"));

    let mut c = base(None);
    c.args(["memory", "stats"]);
    let out = run(c, None);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no server URL"), "{}", out.stderr);
    assert_eq!(out.json()["exit_code"], 1);
    assert!(
        out.json()["error"]
            .as_str()
            .unwrap()
            .contains("no server URL")
    );
}

fn fake_aws() -> String {
    format!("AKIA{}", "Z".repeat(16))
}

#[test]
fn secret_refusal_exit_code_findings_and_no_leak() {
    let srv = FakeServer::start(vec![]);
    let secret = fake_aws();
    let out = yrem(&srv, &["remember", &format!("key {secret} end")]);
    assert_eq!(out.code, 7, "{}", out.stderr);
    let v = out.json();
    assert_eq!(v["exit_code"], 7);
    assert_eq!(
        v["findings"],
        json!([{"kind": "aws_access_key", "path": "/text", "start": 4, "end": 24}])
    );
    assert!(v["error"].as_str().unwrap().contains("force=true"));
    assert!(!out.stdout.contains(&secret) && !out.stderr.contains(&secret));
    assert!(srv.requests().is_empty());
}

#[test]
fn secret_force_flag_stdin_and_call() {
    let secret = fake_aws();
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember").fixture("remember_minimal"),
        Step::post("/v1/remember").fixture("remember_minimal"),
        Step::post("/v1/remember").fixture("remember_minimal"),
    ]);
    ok(&yrem(
        &srv,
        &["remember", &format!("k {secret}"), "--force"],
    ));
    ok(&yrem_stdin(
        &srv,
        &["remember", "--stdin", "--force"],
        &format!("k {secret}\n"),
    ));
    let args = json!({"text": format!("k {secret}"), "force": true}).to_string();
    ok(&yrem(&srv, &["call", "remember", &args]));
    srv.assert_done();
    assert!(srv.requests().iter().all(|r| r.body.get("force").is_none()));

    let none = FakeServer::start(vec![]);
    let out = yrem_stdin(&none, &["remember", "--stdin"], &format!("k {secret}\n"));
    assert_eq!(out.code, 7);
    let args = json!({"text": format!("k {secret}")}).to_string();
    let out = yrem(&none, &["call", "remember", &args]);
    assert_eq!(out.code, 7);
    assert!(!out.stdout.contains(&secret));
    assert!(none.requests().is_empty());
}

#[test]
fn secret_force_stores_private_keys_and_ydb_tokens() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember").fixture("remember_minimal"),
        Step::post("/v1/remember").fixture("remember_minimal"),
    ]);
    let pem = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----";
    let ydb = format!("ydb_{}", "k".repeat(64));
    let refused = yrem_stdin(&srv, &["remember", "--stdin"], pem);
    assert_eq!(refused.code, 7);
    assert_eq!(refused.json()["findings"][0]["kind"], "private_key");
    ok(&yrem_stdin(&srv, &["remember", "--stdin", "--force"], pem));
    ok(&yrem(&srv, &["remember", &ydb, "--force"]));
    srv.assert_done();
    let reqs = srv.requests();
    assert_eq!(reqs[0].body["text"], pem);
    assert_eq!(reqs[1].body["text"], ydb.as_str());
    assert!(reqs.iter().all(|r| r.body.get("force").is_none()));
}

#[test]
fn secret_guard_on_cli_only_writes() {
    let srv = FakeServer::start(vec![]);
    let s = format!("k {}", fake_aws());
    for args in [
        vec!["claim", "add", "svc", "uses", &s],
        vec!["session", "end", "sid", "--summary", &s],
        vec!["forget", "r1", "--reason", &s],
        vec!["conflict", "dismiss", "c1", "--note", &s],
    ] {
        assert_eq!(yrem(&srv, &args).code, 7, "{args:?}");
    }
    assert!(srv.requests().is_empty());
}

#[test]
fn session_start_client_id_is_guarded() {
    let secret = fake_aws();
    let none = FakeServer::start(vec![]);
    let out = yrem(&none, &["session", "start", "--client-id", &secret]);
    assert_eq!(out.code, 7, "{}", out.stderr);
    assert!(none.requests().is_empty());
    let srv = FakeServer::start(vec![Step::post("/v1/sessions").fixture("session_start")]);
    ok(&yrem(
        &srv,
        &["session", "start", "--client-id", &secret, "--force"],
    ));
    srv.assert_done();
}

#[test]
fn call_type_errors_do_not_echo_the_value() {
    let srv = FakeServer::start(vec![]);
    let secret = format!("ghp_{}", "Z".repeat(36));
    for args in [
        json!({"text": "x", "importance": secret}),
        json!({"text": "x", "memory_type": secret}),
        json!({"text": "x", secret.clone(): 1}),
    ] {
        let out = yrem(&srv, &["call", "remember", &args.to_string()]);
        assert_eq!(out.code, 2, "{}", out.stderr);
        assert!(!out.stdout.contains(&secret) && !out.stderr.contains(&secret));
        assert!(out.json()["error"].as_str().unwrap().contains("remember: "));
    }
    let out = yrem(&srv, &["call", "remember", r#"{"text": "x", "bogus": 1}"#]);
    assert!(
        out.stderr.contains("unknown field `…`, expected one of"),
        "{}",
        out.stderr
    );
    let out = yrem(&srv, &["call", "remember", r#"{"text": 5}"#]);
    assert!(out.stderr.contains("expected a string"), "{}", out.stderr);
    assert!(srv.requests().is_empty());
}

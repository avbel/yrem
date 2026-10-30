//! `yrem mcp` end to end: an rmcp client (or raw JSON-RPC) driving the real binary
//! against the fake server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::process::Stdio;
use std::time::{Duration, Instant};

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn command(url: &str) -> tokio::process::Command {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_yrem")).configure(|c| {
        c.arg("mcp")
            .env("YREM_URL", url)
            .env("YREM_TOKEN", TOKEN)
            .env("XDG_CONFIG_HOME", "/nonexistent")
            .env_remove("YANTRIKDB_SERVER_URL")
            .env_remove("YANTRIKDB_TOKEN")
            .env_remove("YREM_CONFIG");
    })
}

async fn client(url: &str) -> RunningService<RoleClient, ()> {
    ().serve(TokioChildProcess::new(command(url)).unwrap())
        .await
        .unwrap()
}

fn params(tool: &'static str, args: Value) -> CallToolRequestParams {
    let Value::Object(map) = args else {
        panic!("args must be an object")
    };
    CallToolRequestParams::new(tool).with_arguments(map)
}

fn text(r: &CallToolResult) -> String {
    serde_json::to_value(&r.content).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "current_thread")]
async fn tools_list_golden_and_budgets() {
    let srv = FakeServer::start(vec![]);
    let c = client(&srv.url).await;
    let instructions = c.peer_info().and_then(|i| i.instructions.clone()).unwrap();
    assert!(
        instructions.len() / 4 <= 150,
        "instructions ~{} tokens",
        instructions.len() / 4
    );
    let tools = c.list_all_tools().await.unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    let mut expected = yrem::ops::TOOLS.to_vec();
    names.sort_unstable();
    expected.sort_unstable();
    assert_eq!(names, expected);
    let compact = serde_json::to_string(&tools).unwrap();
    assert!(
        compact.len() / 4 <= 2000,
        "tools/list ~{} tokens",
        compact.len() / 4
    );
    for t in &tools {
        let d = t.description.as_deref().unwrap();
        assert!(d.split_whitespace().count() <= 15, "{}: {d}", t.name);
        let a = t.annotations.as_ref().unwrap();
        assert_eq!(a.open_world_hint, Some(false), "{}", t.name);
        let read_only = matches!(
            t.name.as_ref(),
            "recall" | "conflict_list" | "session_digest" | "memory_get" | "memory_current"
        );
        assert_eq!(a.read_only_hint, Some(read_only), "{}", t.name);
        let destructive = matches!(t.name.as_ref(), "forget" | "correct" | "conflict_resolve");
        if !read_only {
            assert_eq!(a.destructive_hint, Some(destructive), "{}", t.name);
        }
        let schema = serde_json::to_string(&t.input_schema).unwrap();
        for noise in ["\"title\"", "$schema", "\"null\"", "\"format\""] {
            assert!(!schema.contains(noise), "{}: {noise} in {schema}", t.name);
        }
    }

    let golden = json!({ "instructions": instructions, "tools": tools });
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/tools_list.json");
    let rendered = serde_json::to_string_pretty(&golden).unwrap() + "\n";
    if std::env::var_os("YREM_BLESS").is_some() {
        std::fs::write(path, &rendered).unwrap();
    }
    let expected = std::fs::read_to_string(path).expect("run with YREM_BLESS=1 to create");
    assert_eq!(
        rendered, expected,
        "tools/list changed; rerun with YREM_BLESS=1"
    );
    c.cancel().await.unwrap();
}

/// Each tool once successfully and once failing with `isError`.
#[tokio::test(flavor = "current_thread")]
async fn every_tool_success_and_error() {
    let unsupported =
        json!({"error": "not_yet_available_over_http", "operation": "correct", "reason": "r"});
    let cases: Vec<(&'static str, Value, Vec<Step>, Vec<Step>)> = vec![
        (
            "remember",
            json!({"text": "Payments uses PostgreSQL", "importance": 2.0}),
            vec![Step::post("/v1/remember")
                .body(|b| b["importance"] == 1.0)
                .json(json!({"rid": "r1", "log_index": 3}))],
            vec![Step::post("/v1/remember").status(429).json(json!({"error": "quota"}))],
        ),
        (
            "recall",
            json!({"query": "database"}),
            vec![Step::post("/v1/recall").json(json!({"results": [{"rid": "r1", "score": 0.5}]}))],
            vec![Step::post("/v1/recall").status(500).raw("boom")],
        ),
        (
            "forget",
            json!({"rid": "r1"}),
            vec![Step::post("/v1/forget").json(json!({"rid": "r1", "found": true}))],
            vec![Step::post("/v1/forget").status(401).json(json!({"error": "bad token"}))],
        ),
        (
            "correct",
            json!({"rid": "r1", "reason": "changed", "new_text": "MySQL"}),
            vec![Step::post("/v1/correct").json(json!({"rid": "r2"}))],
            vec![Step::post("/v1/correct").status(501).json(unsupported.clone()),
                 Step::get("/v1/memory/r1").status(404).json(json!({"error": {"code": "memory_not_found", "message": "memory not found: r1"}}))],
        ),
        (
            "think",
            json!({}),
            vec![Step::post("/v1/think").json(json!({"conflicts_found": 0}))],
            vec![Step::post("/v1/think").status(503).json(json!({"error": "read-only"}))],
        ),
        (
            "graph_relate",
            json!({"src": "payments", "dst": "postgres", "rel_type": "uses"}),
            vec![Step::post("/v1/relate")
                .body(|b| b["entity"] == "payments" && b["target"] == "postgres" && b["relationship"] == "uses")
                .json(json!({"edge_id": "e1"}))],
            vec![Step::post("/v1/relate").status(400).json(json!({"error": "missing 'entity'"}))],
        ),
        (
            "conflict_list",
            json!({"status": "open"}),
            vec![Step::get("/v1/conflicts?status=open").json(json!({"conflicts": []}))],
            vec![Step::get("/v1/conflicts?status=open").status(403).json(json!({"error": "forbidden"}))],
        ),
        (
            "conflict_resolve",
            json!({"id": "c1", "strategy": "keep_b", "note": "newer"}),
            vec![Step::post("/v1/conflicts/c1/resolve")
                .body(|b| b["strategy"] == "keep_b" && b["resolution_note"] == "newer")
                .json(json!({"conflict_id": "c1", "strategy": "keep_b"}))],
            vec![Step::post("/v1/conflicts/c1/resolve").status(404).json(json!({"error": "no such conflict"}))],
        ),
        (
            "session_digest",
            json!({"include_gaps": true}),
            vec![Step::get("/v1/session/digest?include_gaps=true").json(json!({"decisions": []}))],
            vec![Step::get("/v1/session/digest?include_gaps=true").status(500).json(json!({"error": "x"}))],
        ),
        (
            "session_capture",
            json!({"summary": "We moved to PostgreSQL."}),
            vec![Step::post("/v1/session/end").json(json!({"drafted": ["r9"], "count": 1}))],
            vec![Step::post("/v1/session/end").status(409).json(json!({"error": "follower"}))],
        ),
        (
            "memory_get",
            json!({"rid": "r1"}),
            vec![Step::get("/v1/memory/r1").json(json!({"rid": "r1", "text": "x", "type": "semantic"}))],
            vec![Step::get("/v1/memory/r1").status(404).json(json!({"error": {"code": "memory_not_found", "message": "gone"}}))],
        ),
        (
            "memory_current",
            json!({"namespace": "payments"}),
            vec![Step::get("/v1/current?namespace=payments").json(json!({"found": true}))],
            vec![Step::get("/v1/current?namespace=payments").status(404).json(json!({"error": "no chain head"}))],
        ),
    ];
    assert_eq!(cases.len(), yrem::ops::TOOLS.len());

    let steps: Vec<Step> = cases
        .iter()
        .flat_map(|(_, _, ok, err)| ok.iter().chain(err).cloned())
        .collect();
    let srv = FakeServer::start(steps);
    let c = client(&srv.url).await;
    for (tool, args, _, _) in &cases {
        let ok = c.call_tool(params(tool, args.clone())).await.unwrap();
        assert_eq!(ok.is_error, Some(false), "{tool}: {}", text(&ok));
        let body: Value = serde_json::from_str(&text(&ok)).unwrap();
        assert!(body.is_object(), "{tool}");
        assert!(ok.structured_content.is_none(), "{tool}");
        let err = c.call_tool(params(tool, args.clone())).await.unwrap();
        assert_eq!(err.is_error, Some(true), "{tool}: {}", text(&err));
        assert_ne!(text(&err), "");
    }
    srv.assert_done();
    c.cancel().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_arguments_and_unknown_tool() {
    let srv = FakeServer::start(vec![]);
    let c = client(&srv.url).await;
    // Unknown field: rejected by serde before any request.
    let r = c
        .call_tool(params("forget", json!({"rid": "r1", "claims": []})))
        .await;
    assert!(r.is_err() || r.unwrap().is_error == Some(true));
    // Validation error from ops: empty text.
    let r = c
        .call_tool(params("remember", json!({"text": "  "})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(text(&r).contains("must not be empty"));
    // CLI-only fields are not part of the MCP schema.
    let r = c
        .call_tool(params("recall", json!({"query": "x", "stdin": true})))
        .await;
    assert!(r.is_err() || r.unwrap().is_error == Some(true));
    assert!(
        c.call_tool(params("graph_edges", json!({"entity": "x"})))
            .await
            .is_err()
    );
    assert!(srv.requests().is_empty());
    c.cancel().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn emulated_correct_partial_mentions_both_rids() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").status(501).json(
            json!({"error": "not_yet_available_over_http", "operation": "correct", "reason": "r"}),
        ),
        Step::get("/v1/memory/old").json(json!({"rid": "old", "text": "x"})),
        Step::post("/v1/remember").json(json!({"rid": "new"})),
        Step::post("/v1/forget")
            .status(503)
            .json(json!({"error": "down"})),
    ]);
    let c = client(&srv.url).await;
    let r = c
        .call_tool(params(
            "correct",
            json!({"rid": "old", "reason": "changed"}),
        ))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    let t = text(&r);
    assert!(
        t.contains("old") && t.contains("new") && t.contains("forget"),
        "{t}"
    );
    srv.assert_done();
    c.cancel().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_aborts_http_request() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall").delay(Duration::from_secs(10)),
    ]);
    let c = client(&srv.url).await;
    let peer = c.peer().clone();
    let call = tokio::spawn(async move {
        peer.call_tool(params("recall", json!({"query": "slow"})))
            .await
    });
    for _ in 0..500 {
        if !srv.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(srv.requests().len(), 1);
    // Dropping the request future sends notifications/cancelled.
    call.abort();
    let started = Instant::now();
    while !srv.requests()[0].aborted {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "HTTP request not aborted"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    c.cancel().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_calls_run_concurrently() {
    let delay = Duration::from_millis(400);
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .delay(delay)
            .json(json!({"results": []})),
        Step::post("/v1/recall")
            .delay(delay)
            .json(json!({"results": []})),
    ]);
    let c = client(&srv.url).await;
    let started = Instant::now();
    let (a, b) = tokio::join!(
        c.call_tool(params("recall", json!({"query": "a"}))),
        c.call_tool(params("recall", json!({"query": "b"}))),
    );
    assert_eq!(a.unwrap().is_error, Some(false));
    assert_eq!(b.unwrap().is_error, Some(false));
    assert!(
        started.elapsed() < delay * 2,
        "calls ran serially: {:?}",
        started.elapsed()
    );
    srv.assert_done();
    c.cancel().await.unwrap();
}

/// Raw JSON-RPC over stdio: both protocol eras, and stdout carries only JSON-RPC frames.
struct Raw {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

impl Raw {
    fn spawn(url: &str) -> Self {
        let mut child = command(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            stdin,
            lines,
        }
    }

    async fn send(&mut self, v: &Value) {
        self.stdin
            .write_all(format!("{v}\n").as_bytes())
            .await
            .unwrap();
    }

    /// Next frame; panics if a stdout line is not a JSON-RPC 2.0 message.
    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {line}: {e}"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
        v
    }

    async fn response(&mut self, id: i64) -> Value {
        loop {
            let v = self.recv().await;
            if v["id"] == id {
                return v;
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_era() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(json!({"results": []}))]);
    let mut raw = Raw::spawn(&srv.url);
    raw.send(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "t", "version": "0"}}}),
    )
    .await;
    let init = raw.response(1).await;
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["serverInfo"]["name"], "yrem");
    assert!(init["result"]["instructions"].is_string());
    raw.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    raw.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .await;
    let list = raw.response(2).await;
    assert_eq!(
        list["result"]["tools"].as_array().unwrap().len(),
        yrem::ops::TOOLS.len()
    );
    raw.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "recall", "arguments": {"query": "x"}}}))
        .await;
    let call = raw.response(3).await;
    assert_eq!(call["result"]["isError"], false);
    drop(raw.stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), raw.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn discover_era() {
    let srv = FakeServer::start(vec![]);
    let mut raw = Raw::spawn(&srv.url);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    raw.send(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": {"_meta": meta}}),
    )
    .await;
    let disc = raw.response(1).await;
    assert!(disc.get("result").is_some(), "{disc}");
    let result = disc["result"].to_string();
    assert!(result.contains("yrem"), "{result}");
    raw.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list",
        "params": {"_meta": meta}}))
        .await;
    let list = raw.response(2).await;
    assert_eq!(
        list["result"]["tools"].as_array().map(Vec::len),
        Some(yrem::ops::TOOLS.len()),
        "{list}"
    );
    let _: JsonObject = serde_json::from_value(list["result"].clone()).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn missing_config_fails_fast() {
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_yrem"))
        .arg("mcp")
        .env_remove("YREM_URL")
        .env_remove("YANTRIKDB_SERVER_URL")
        .env_remove("YREM_CONFIG")
        .env("XDG_CONFIG_HOME", "/nonexistent")
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.stdout, b"");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no server URL"));
}

#[tokio::test(flavor = "current_thread")]
async fn secret_guard_refuses_and_force_overrides() {
    let secret = format!("AKIA{}", "Z".repeat(16));
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember")
            .body(|b| b.get("force").is_none())
            .json(json!({"rid": "r1", "log_index": 1})),
        Step::post("/v1/remember")
            .body(|b| b.get("force").is_none())
            .json(json!({"rid": "r2", "log_index": 2})),
    ]);
    let c = client(&srv.url).await;
    let r = c
        .call_tool(params("remember", json!({"text": format!("key {secret}")})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    let t = text(&r);
    assert!(t.contains("force=true") && t.contains("aws_access_key at /text[4..24]"));
    assert!(!t.contains(&secret));
    assert!(srv.requests().is_empty());

    let r = c
        .call_tool(params(
            "remember",
            json!({"text": format!("key {secret}"), "force": true}),
        ))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(false), "{}", text(&r));

    let pem = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----";
    let r = c
        .call_tool(params("remember", json!({"text": pem, "force": true})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(false), "{}", text(&r));
    assert_eq!(srv.requests()[1].body["text"], pem);
    srv.assert_done();
    c.cancel().await.unwrap();
}

//! `metadata.harness` on every write, and the project namespace of MCP writes that name none.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};

const BIN: &str = env!("CARGO_BIN_EXE_yrem");
const OLD_RID: &str = "01a18e8a-aa4d-786a-a95e-2944a7bb906a";

fn hermetic(url: &str) -> Command {
    let mut c = Command::new(BIN);
    for (k, _) in std::env::vars() {
        if k.starts_with("YREM_") || k.starts_with("YANTRIKDB_") {
            c.env_remove(k);
        }
    }
    c.env("XDG_CONFIG_HOME", "/nonexistent")
        .env("YREM_URL", url)
        .env("YREM_TOKEN", TOKEN)
        .env("YREM_OUTBOX_DIR", "off")
        .env("NO_COLOR", "1");
    c
}

fn cli(srv: &FakeServer, env: &[(&str, &str)], args: &[&str]) {
    let out = hermetic(&srv.url)
        .envs(env.iter().copied())
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    srv.assert_done();
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yrem-harness-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn body(srv: &FakeServer, index: usize) -> Value {
    srv.requests()[index].body.clone()
}

#[test]
fn remember_is_stamped_cli() {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").json(json!({"rid": "r"}))]);
    cli(&srv, &[], &["remember", "A fact", "--force"]);
    assert_eq!(body(&srv, 0)["metadata"], json!({"harness": "cli"}));
}

#[test]
fn env_harness_names_the_cli_caller() {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").json(json!({"rid": "r"}))]);
    cli(
        &srv,
        &[("YREM_HARNESS", " Codex ")],
        &["remember", "A fact", "--force"],
    );
    assert_eq!(body(&srv, 0)["metadata"]["harness"], "codex");
}

#[test]
fn a_harness_the_caller_set_is_kept() {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").json(json!({"rid": "r"}))]);
    cli(
        &srv,
        &[],
        &[
            "call",
            "remember",
            r#"{"text":"A fact","force":true,"metadata":{"harness":"mine","k":1}}"#,
        ],
    );
    assert_eq!(
        body(&srv, 0)["metadata"],
        json!({"harness": "mine", "k": 1})
    );
}

#[test]
fn every_memory_of_a_batch_is_stamped_and_keeps_its_own_harness() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember/batch").json(json!({"count": 3})),
    ]);
    cli(
        &srv,
        &[],
        &[
            "call",
            "remember",
            r#"{"force":true,"metadata":{"k":1},"memories":[
                {"text":"a"},
                {"text":"b","metadata":{"j":2}},
                {"text":"c","metadata":{"harness":"mine"}}]}"#,
        ],
    );
    let memories = body(&srv, 0)["memories"].clone();
    assert_eq!(memories[0]["metadata"], json!({"harness": "cli", "k": 1}));
    assert_eq!(memories[1]["metadata"], json!({"harness": "cli", "j": 2}));
    assert_eq!(memories[2]["metadata"], json!({"harness": "mine"}));
}

#[test]
fn native_correct_merges_the_harness() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"new_rid": "n"})),
    ]);
    cli(
        &srv,
        &[],
        &["correct", OLD_RID, "--reason", "changed", "--new-text", "t"],
    );
    assert_eq!(body(&srv, 0)["metadata_merge"], json!({"harness": "cli"}));
}

#[test]
fn emulated_correct_replaces_the_old_harness() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&format!("/v1/memory/{OLD_RID}")).json(json!({
            "rid": OLD_RID, "text": "old", "metadata": {"harness": "claude", "k": 1},
        })),
        Step::post("/v1/remember").json(json!({"rid": "new"})),
        Step::post("/v1/forget").json(json!({"found": true})),
    ]);
    cli(
        &srv,
        &[("YREM_HARNESS", "pi")],
        &["correct", OLD_RID, "--reason", "r", "--new-text", "t"],
    );
    let metadata = &body(&srv, 2)["metadata"];
    assert_eq!(metadata["harness"], "pi");
    assert_eq!(metadata["k"], 1);
    assert_eq!(metadata["corrects"], OLD_RID);
}

#[test]
fn session_capture_sends_the_harness_as_metadata() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/session/end").json(json!({"count": 1})),
    ]);
    cli(
        &srv,
        &[],
        &["session", "capture", "We reviewed the payments service."],
    );
    assert_eq!(body(&srv, 0)["metadata"], json!({"harness": "cli"}));
}

/// A `yrem mcp` child driven over stdio with one initialize and one tool call.
struct Mcp {
    child: std::process::Child,
}

impl Mcp {
    fn call(
        srv: &FakeServer,
        client: &str,
        cwd: &Path,
        env: &[(&str, &str)],
        tool: &str,
        args: &Value,
    ) {
        let mut c = hermetic(&srv.url);
        c.current_dir(cwd)
            .env_remove("HOME")
            .envs(env.iter().copied())
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut mcp = Self {
            child: c.spawn().unwrap(),
        };
        let mut stdin = mcp.child.stdin.take().unwrap();
        let mut lines = BufReader::new(mcp.child.stdout.take().unwrap()).lines();
        let mut send = |v: Value| writeln!(stdin, "{v}").unwrap();
        send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": client, "version": "0"}}}),
        );
        send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": {"name": tool, "arguments": args}}));
        let answer = loop {
            let line = lines.next().unwrap().unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == 2 {
                break v;
            }
        };
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        drop(stdin);
        mcp.child.wait().unwrap();
        srv.assert_done();
    }
}

fn remember_via_mcp(client: &str, env: &[(&str, &str)]) -> Value {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").json(json!({"rid": "r"}))]);
    let cwd = scratch("mcp-harness");
    Mcp::call(
        &srv,
        client,
        &cwd,
        env,
        "remember",
        &json!({"text": "A fact", "force": true}),
    );
    body(&srv, 0)
}

#[test]
fn mcp_harness_comes_from_the_client_name() {
    for (client, harness) in [
        ("claude-code", "claude"),
        ("Claude Code", "claude"),
        ("codex-mcp-client", "codex"),
        ("antigravity-client", "antigravity"),
        ("gemini-cli-mcp-client", "antigravity"),
        ("agy", "antigravity"),
        ("opencode", "opencode"),
        ("pi-coding-agent", "pi"),
        ("Some-Client", "some-client"),
    ] {
        let sent = remember_via_mcp(client, &[]);
        assert_eq!(sent["metadata"]["harness"], harness, "{client}");
    }
}

#[test]
fn mcp_env_harness_wins_over_the_client_name() {
    let sent = remember_via_mcp("claude-code", &[("YREM_HARNESS", "custom")]);
    assert_eq!(sent["metadata"]["harness"], "custom");
}

#[test]
fn mcp_capture_and_correct_are_stamped_too() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/session/end").json(json!({"count": 1})),
    ]);
    let cwd = scratch("mcp-capture");
    let args = json!({"summary": "We reviewed the payments service."});
    Mcp::call(
        &srv,
        "codex-mcp-client",
        &cwd,
        &[],
        "session_capture",
        &args,
    );
    assert_eq!(body(&srv, 0)["metadata"], json!({"harness": "codex"}));

    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"new_rid": "n"})),
    ]);
    let args = json!({"rid": OLD_RID, "reason": "r", "new_text": "t"});
    Mcp::call(&srv, "claude-code", &cwd, &[], "correct", &args);
    assert_eq!(
        body(&srv, 0)["metadata_merge"],
        json!({"harness": "claude"})
    );
}

fn namespace_sent(cwd: &Path, env: &[(&str, &str)], args: &Value) -> Option<String> {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").json(json!({"rid": "r"}))]);
    Mcp::call(&srv, "claude-code", cwd, env, "remember", args);
    body(&srv, 0)["namespace"].as_str().map(str::to_owned)
}

fn fact() -> Value {
    json!({"text": "A fact", "force": true})
}

fn repo(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir.join("src")
}

#[test]
fn mcp_writes_default_to_the_repository_name() {
    let root = scratch("ns-repo");
    let cwd = repo(&root, "billing");
    assert_eq!(
        namespace_sent(&cwd, &[], &fact()).as_deref(),
        Some("billing")
    );
}

#[test]
fn an_explicit_namespace_is_kept() {
    let root = scratch("ns-explicit");
    let cwd = repo(&root, "billing");
    let args = json!({"text": "A fact", "force": true, "namespace": "payments"});
    assert_eq!(
        namespace_sent(&cwd, &[], &args).as_deref(),
        Some("payments")
    );
}

#[test]
fn aliases_apply_to_the_default_namespace() {
    let root = scratch("ns-alias");
    let cwd = repo(&root, "bw-lite-desktop");
    let env = [("YREM_HOOK_NAMESPACE_ALIASES", "bw-lite-desktop=vaultkeeper")];
    assert_eq!(
        namespace_sent(&cwd, &env, &fact()).as_deref(),
        Some("vaultkeeper")
    );
}

#[test]
fn excluded_projects_get_no_default_namespace() {
    let root = scratch("ns-excluded");
    let cwd = repo(&root, "private-notes");
    let env = [("YREM_HOOK_EXCLUDE_NAMESPACES", "private")];
    assert_eq!(namespace_sent(&cwd, &env, &fact()), None);
}

#[test]
fn nothing_resolving_keeps_the_server_default() {
    let root = scratch("ns-none");
    assert_eq!(namespace_sent(&root, &[], &fact()), None);
}

#[test]
fn the_home_directory_defaults_to_home() {
    let root = scratch("ns-home");
    let home = root.to_str().unwrap();
    assert_eq!(
        namespace_sent(&root, &[("HOME", home)], &fact()).as_deref(),
        Some("home")
    );
    let env = [("HOME", home), ("YREM_HOME_NAMESPACE", "me")];
    assert_eq!(namespace_sent(&root, &env, &fact()).as_deref(), Some("me"));
}

#[test]
fn a_subdirectory_of_home_is_not_home() {
    let root = scratch("ns-home-sub");
    let sub = root.join("notes");
    std::fs::create_dir_all(&sub).unwrap();
    let env = [("HOME", root.to_str().unwrap())];
    assert_eq!(namespace_sent(&sub, &env, &fact()), None);
}

#[test]
fn session_capture_and_graph_relate_default_too() {
    let root = scratch("ns-others");
    let cwd = repo(&root, "billing");
    let srv = FakeServer::start(vec![
        Step::post("/v1/session/end").json(json!({"count": 1})),
    ]);
    let args = json!({"summary": "We reviewed the payments service."});
    Mcp::call(&srv, "claude-code", &cwd, &[], "session_capture", &args);
    assert_eq!(body(&srv, 0)["namespace"], "billing");

    let srv = FakeServer::start(vec![Step::post("/v1/relate").json(json!({}))]);
    let args = json!({"src": "a", "dst": "b", "rel_type": "uses"});
    Mcp::call(&srv, "claude-code", &cwd, &[], "graph_relate", &args);
    assert_eq!(body(&srv, 0)["namespace"], "billing");
}

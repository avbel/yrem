//! The real `yrem` binary against a real `yantrikdb-server`. Skipped unless `YREM_CONTRACT=1`;
//! needs `YREM_URL` and `YREM_TOKEN` for a scratch database (never the real store).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_yrem");
const MISSING_ID: &str = "00000000-0000-7000-8000-000000000000";

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

    fn ok(&self) -> Value {
        assert_eq!(
            self.code, 0,
            "stderr: {}\nstdout: {}",
            self.stderr, self.stdout
        );
        self.json()
    }
}

fn enabled() -> bool {
    if std::env::var("YREM_CONTRACT").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!("skipped: set YREM_CONTRACT=1");
    false
}

fn spawn(envs: &[(&str, &str)], args: &[&str], stdin: Option<&str>) -> Out {
    let mut cmd = Command::new(BIN);
    cmd.arg("--compact")
        .args(args)
        .env("YREM_CONFIG", "/dev/null")
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    Out {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn yrem(args: &[&str]) -> Out {
    spawn(&[], args, None)
}

fn yrem_stdin(args: &[&str], input: &str) -> Out {
    spawn(&[], args, Some(input))
}

fn rid(v: &Value) -> String {
    v["rid"]
        .as_str()
        .unwrap_or_else(|| panic!("{v}"))
        .to_owned()
}

#[test]
fn lifecycle() {
    if !enabled() {
        return;
    }
    let ns = format!("yrem-cli-contract-{}", std::process::id());

    let stored = yrem(&[
        "remember",
        "The CLI contract service stores data in PostgreSQL",
        "--namespace",
        &ns,
        "--domain",
        "work",
        "--importance",
        "0.7",
    ])
    .ok();
    let first = rid(&stored);

    let piped = yrem_stdin(
        &["remember", "--stdin", "--namespace", &ns],
        "The CLI contract team meets on Mondays",
    )
    .ok();
    let piped_rid = rid(&piped);

    let batch = yrem(&[
        "call",
        "remember",
        &format!(
            r#"{{"memories":[{{"text":"The CLI contract owner is Alice","namespace":"{ns}"}},{{"text":"The CLI contract runner is nightly","namespace":"{ns}","created_at":"2026-01-02"}}]}}"#
        ),
    ])
    .ok();
    assert_eq!(batch["count"], 2, "{batch}");

    let hits = yrem(&[
        "recall",
        "where does the CLI contract service store data",
        "--namespace",
        &ns,
        "--top-k",
        "5",
        "--since",
        "1d",
        "--order",
        "relevance",
    ])
    .ok();
    assert!(
        hits["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["rid"] == first.as_str()),
        "{hits}"
    );

    let got = yrem(&["memory", "get", &first]).ok();
    assert_eq!(got["memory_type"], "semantic", "{got}");
    assert_eq!(got["namespace"], ns.as_str());

    let listed = yrem(&["memory", "list", "--namespace", &ns, "--limit", "10"]).ok();
    assert!(listed.is_object(), "{listed}");
    assert!(
        yrem(&["memory", "current", "--namespace", &ns])
            .ok()
            .is_object()
    );
    assert!(yrem(&["memory", "stats"]).ok()["active_memories"].is_u64());
    assert!(yrem(&["memory", "gaps", "--limit", "5"]).ok().is_object());

    let corrected = yrem(&[
        "correct",
        &first,
        "--reason",
        "migrated",
        "--new-text",
        "The CLI contract service stores data in MySQL",
    ])
    .ok();
    eprintln!("correct emulated: {}", corrected["emulated"]);
    assert!(corrected["new_rid"].is_string(), "{corrected}");

    let related = yrem(&[
        "graph",
        "relate",
        "--src",
        "cli-contract-svc",
        "--dst",
        "postgres",
        "--rel-type",
        "uses",
        "--namespace",
        &ns,
    ])
    .ok();
    assert!(related.is_object(), "{related}");

    assert!(yrem(&["conflict", "list"]).ok()["conflicts"].is_array());
    assert!(yrem(&["think"]).ok().is_object());

    let captured = yrem(&[
        "session",
        "capture",
        "We decided the CLI contract runs nightly. Bob owns the CLI contract runner.",
        "--namespace",
        &ns,
    ])
    .ok();
    assert!(captured["count"].is_u64(), "{captured}");

    let session = yrem(&[
        "session",
        "start",
        "--namespace",
        &ns,
        "--client-id",
        "yrem-cli",
    ])
    .ok();
    let sid = session["session_id"].as_str().unwrap().to_owned();
    assert!(yrem(&["session", "end", &sid]).ok().is_object());

    assert!(yrem(&["digest"]).ok().is_object());
    assert!(
        yrem(&["session", "digest", "--include-gaps"])
            .ok()
            .is_object()
    );
    let hook = yrem(&["digest", "--hook"]).ok();
    assert_eq!(
        hook["hookSpecificOutput"]["hookEventName"], "SessionStart",
        "{hook}"
    );
    assert!(hook["hookSpecificOutput"]["additionalContext"].is_string());

    let doctor = yrem(&["doctor"]).ok();
    assert_eq!(doctor["ok"], true, "{doctor}");

    for r in [
        first,
        piped_rid,
        corrected["new_rid"].as_str().unwrap().to_owned(),
    ] {
        assert!(
            yrem(&["forget", &r, "--reason", "contract cleanup"])
                .ok()
                .is_object()
        );
    }
}

#[test]
fn v02_commands() {
    if !enabled() {
        return;
    }
    let ns = format!("yrem-cli-contract-v02-{}", std::process::id());

    yrem(&[
        "call",
        "remember",
        &format!(
            r#"{{"text":"The v02 CLI ledger runbook lives in the archive","namespace":"{ns}","created_at":"2026-01-02"}}"#
        ),
    ])
    .ok();
    yrem(&[
        "remember",
        "To roll back a v02 CLI deploy, redeploy the previous tag",
        "--namespace",
        &ns,
        "--memory-type",
        "procedural",
        "--domain",
        "deploy",
    ])
    .ok();

    let stale = yrem(&[
        "temporal",
        "stale",
        "--days",
        "0",
        "--limit",
        "10",
        "--namespace",
        &ns,
    ])
    .ok();
    assert!(stale["count"].as_u64().unwrap() >= 2, "{stale}");
    let upcoming = yrem(&["temporal", "upcoming", "--namespace", &ns]).ok();
    assert!(upcoming["count"].is_u64(), "{upcoming}");
    let before = yrem(&[
        "temporal",
        "as-of",
        "ledger runbook",
        "--at",
        "2025-01-01",
        "--namespace",
        &ns,
    ])
    .ok();
    assert_eq!(before["count"], 0, "{before}");
    let after = yrem(&[
        "temporal",
        "as-of",
        "ledger runbook",
        "--at",
        "1h",
        "--namespace",
        &ns,
    ])
    .ok();
    assert!(after["count"].as_u64().unwrap() >= 1, "{after}");

    assert!(yrem(&["trigger", "list"]).ok()["triggers"].is_array());
    assert!(yrem(&["trigger", "history", "--limit", "5"]).ok()["triggers"].is_array());

    let claim = yrem(&[
        "claim",
        "add",
        "cli-bob",
        "reviews",
        "cli-ledger",
        "--namespace",
        &ns,
        "--valid-from",
        "2026-01-01",
        "--weight",
        "0.8",
        "--confidence-band",
        "high",
    ])
    .ok();
    assert!(claim["claim_id"].is_string(), "{claim}");
    let claims = yrem(&["claim", "list", "cli-bob", "--namespace", &ns]).ok();
    assert_eq!(claims["claims"][0]["dst"], "cli-ledger", "{claims}");

    let surfaced = yrem(&[
        "procedure",
        "surface",
        "roll back a deploy",
        "--namespace",
        &ns,
        "--domain",
        "deploy",
    ])
    .ok();
    assert!(surfaced["count"].as_u64().unwrap() >= 1, "{surfaced}");
    assert_eq!(yrem(&["task", "list", "--namespace", &ns]).ok()["count"], 0);
}

#[test]
fn error_exit_codes() {
    if !enabled() {
        return;
    }
    let bad = spawn(&[("YREM_TOKEN", "ydb_invalid")], &["think"], None);
    assert_eq!(bad.code, 3, "{}", bad.stderr);
    assert!(bad.json()["error"].is_string());

    let edges = yrem(&["graph", "edges", "--entity", "cli-contract-svc"]);
    assert_eq!(edges.code, 4, "{}", edges.stderr);
    assert!(edges.json()["error"].is_string());

    let down = spawn(&[("YREM_URL", "http://127.0.0.1:9")], &["think"], None);
    assert_eq!(down.code, 5, "{}", down.stderr);

    let missing = yrem(&["trigger", "act", MISSING_ID]);
    assert_eq!(missing.code, 1, "{}", missing.stderr);
    assert_eq!(missing.json()["exit_code"], 1);
}

#[test]
fn secret_guard() {
    if !enabled() {
        return;
    }
    let ns = format!("yrem-cli-contract-secret-{}", std::process::id());
    let secret = format!("AKIA{}", "Q7W8E9R1T2Y3U4I5");
    let text = format!("The CLI contract deploy key is {secret}");

    let refused = yrem(&["remember", &text, "--namespace", &ns]);
    assert_eq!(refused.code, 7, "{}", refused.stderr);
    let body = refused.json();
    assert_eq!(body["findings"][0]["kind"], "aws_access_key", "{body}");
    assert!(!refused.stdout.contains(&secret) && !refused.stderr.contains(&secret));

    let forced = yrem(&["remember", &text, "--namespace", &ns, "--force"]).ok();
    let stored = rid(&forced);
    let got = yrem(&["memory", "get", &stored]).ok();
    assert!(got["text"].as_str().unwrap().contains(&secret), "{got}");
    assert!(yrem(&["forget", &stored]).ok().is_object());
}

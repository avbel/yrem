#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use yrem::cli::hook::{LIMIT, render};

const MARKER: &str = "…[truncated]";

fn context(out: &str) -> String {
    let v: Value = serde_json::from_str(out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn small_digest_passes_through() {
    let digest = json!({"summary": "hello", "n": 3});
    let out = render(&digest, LIMIT);
    let ctx = context(&out);
    assert!(ctx.contains(&digest.to_string()), "{ctx}");
    assert!(ctx.starts_with("Memory briefing"));
    assert!(!ctx.contains(MARKER));
}

fn big_digest() -> Value {
    let chunk = "é🦀 \"quoted\"\nline \\ back\t";
    json!({"text": chunk.repeat(50_000 / chunk.chars().count() + 1)})
}

#[test]
fn big_digest_is_truncated_within_limit() {
    let digest = big_digest();
    assert!(digest.to_string().chars().count() > 50_000);
    let out = render(&digest, LIMIT);
    assert!(out.len() <= LIMIT, "{}", out.len());
    let ctx = context(&out);
    assert!(ctx.ends_with(MARKER), "{ctx:?}");
}

#[test]
fn big_escape_heavy_digest_keeps_most_of_the_budget() {
    let out = render(&big_digest(), LIMIT);
    let ctx = context(&out);
    assert!(ctx.starts_with("Memory briefing"), "{ctx:?}");
    assert!(out.len() > LIMIT / 2, "only {} bytes kept", out.len());
}

#[test]
fn many_limits_never_panic() {
    let digest = big_digest();
    for limit in (200..260).chain([0, 1, 10, 50, 100, 150, 199, 1000, 4097]) {
        let out = render(&digest, limit);
        if limit >= 200 {
            assert!(out.len() <= limit, "limit {limit}: {}", out.len());
            let ctx = context(&out);
            assert!(ctx.ends_with(MARKER), "limit {limit}");
        } else if let Ok(v) = serde_json::from_str::<Value>(&out) {
            assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        }
    }
}

#[test]
fn multibyte_only_digest_at_all_boundaries() {
    let digest = json!("🦀".repeat(2000));
    for limit in 150..400 {
        let out = render(&digest, limit);
        assert!(out.len() <= limit, "limit {limit}: {}", out.len());
        context(&out);
    }
}

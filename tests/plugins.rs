//! The harness plugins in `plugins/`: manifests that only call `yrem hook`, bundled skills
//! that match `skills/yrem`, and versions that match the crate.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use serde_json::Value;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const EVENTS: &[&str] = &["session-start", "prompt", "pre-compact", "session-end"];

fn json(path: &str) -> Value {
    let text = std::fs::read_to_string(Path::new(ROOT).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn hook_manifests_only_call_yrem_hook_and_never_fail() {
    for (path, harness) in [
        ("plugins/claude/hooks/hooks.json", "claude"),
        ("plugins/codex/hooks/hooks.json", "codex"),
    ] {
        let hooks = json(path)["hooks"].as_object().unwrap().clone();
        assert_eq!(hooks.len(), 4, "{path}");
        for groups in hooks.values() {
            for group in groups.as_array().unwrap() {
                for hook in group["hooks"].as_array().unwrap() {
                    let cmd = hook["command"].as_str().unwrap();
                    let words: Vec<&str> = cmd.split_whitespace().collect();
                    assert_eq!(&words[..2], ["yrem", "hook"], "{cmd}");
                    assert!(EVENTS.contains(&words[2]), "{cmd}");
                    assert_eq!(&words[3..5], ["--harness", harness], "{cmd}");
                    // An older yrem without `hook` exits 2, which drops a Claude prompt.
                    assert!(cmd.ends_with(" || true"), "{cmd}");
                }
            }
        }
    }
}

#[test]
fn bundled_skills_match_the_canonical_one() {
    let canonical = std::fs::read_to_string(Path::new(ROOT).join("skills/yrem/SKILL.md")).unwrap();
    for h in ["claude", "codex", "pi"] {
        let copy = std::fs::read_to_string(
            Path::new(ROOT).join(format!("plugins/{h}/skills/yrem/SKILL.md")),
        )
        .unwrap();
        assert_eq!(
            copy, canonical,
            "plugins/{h}/skills/yrem/SKILL.md: copy skills/yrem/SKILL.md"
        );
    }
}

#[test]
fn plugin_versions_match_the_crate() {
    let version = env!("CARGO_PKG_VERSION");
    for (path, pointer) in [
        ("plugins/claude/.claude-plugin/plugin.json", "/version"),
        ("plugins/codex/.codex-plugin/plugin.json", "/version"),
        ("plugins/opencode/package.json", "/version"),
        ("plugins/pi/package.json", "/version"),
        (".claude-plugin/marketplace.json", "/plugins/0/version"),
    ] {
        assert_eq!(
            json(path).pointer(pointer).and_then(Value::as_str),
            Some(version),
            "{path}"
        );
    }
}

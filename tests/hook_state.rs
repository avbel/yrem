//! Project resolution from `cwd`, the cooldown and maintenance claims, and the harness
//! filter shared with capture.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::json;
use yrem::hook::project::resolve;
use yrem::hook::state::{Kind, Store};
use yrem::hook::transcript::{clean, is_headless_session, strip_harness};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("yrem-hs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    // Resolved paths are canonical; /tmp is a symlink on macOS.
    fs::canonicalize(d).unwrap()
}

fn aliases(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

fn mkdir(p: &Path) -> &Path {
    fs::create_dir_all(p).unwrap();
    p
}

#[test]
fn the_project_is_the_repository_directory_name() {
    let root = temp("proj");
    let repo = root.join("fluidd");
    mkdir(&repo.join(".git"));
    mkdir(&repo.join("packages/api/src"));
    assert_eq!(resolve(&repo, &[]).as_deref(), Some("fluidd"));
    assert_eq!(
        resolve(&repo.join("packages/api/src"), &[]).as_deref(),
        Some("fluidd")
    );
    assert_eq!(
        resolve(&repo.join("deleted/dir"), &[]).as_deref(),
        Some("fluidd"),
        "a directory that is gone still sits in the repository"
    );
    let loose = mkdir(&root.join("loose/dir")).to_path_buf();
    assert_eq!(resolve(&loose, &[]), None, "no repository, no project");
    assert_eq!(resolve(Path::new("/"), &[]), None);
}

#[test]
fn a_linked_worktree_belongs_to_its_main_repository() {
    let root = temp("wt");
    let main = root.join("fluidd");
    mkdir(&main.join(".git/worktrees/fluidd-wt-306"));
    for wt in [
        root.join("fluidd-wt-306"),
        main.join(".claude/worktrees/agent-1"),
    ] {
        mkdir(&wt.join("src"));
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}/.git/worktrees/x\n", main.display()),
        )
        .unwrap();
        assert_eq!(
            resolve(&wt.join("src"), &[]).as_deref(),
            Some("fluidd"),
            "{wt:?}"
        );
    }
}

#[test]
fn a_relative_gitdir_is_resolved_against_the_worktree() {
    let root = temp("rel");
    mkdir(&root.join("main/.git/worktrees/w"));
    let wt = mkdir(&root.join("elsewhere/w")).to_path_buf();
    fs::write(wt.join(".git"), "gitdir: ../../main/.git/worktrees/w\n").unwrap();
    assert_eq!(resolve(&wt, &[]).as_deref(), Some("main"));
}

#[test]
fn a_git_file_that_is_not_a_worktree_names_its_own_directory() {
    let root = temp("sub");
    for (name, content) in [
        ("submodule", "gitdir: ../.git/modules/submodule\n"),
        ("junk", "not a gitdir line\n"),
        ("empty", ""),
        ("bare", "gitdir: ../bare.git/worktrees/bare\n"),
        ("odd", "gitdir: /nowhere/worktrees/x\n"),
    ] {
        let dir = mkdir(&root.join(name)).to_path_buf();
        fs::write(dir.join(".git"), content).unwrap();
        assert_eq!(resolve(&dir, &[]).as_deref(), Some(name), "{name}");
    }
    let unreadable = mkdir(&root.join("unreadable")).to_path_buf();
    mkdir(&unreadable.join(".git/"));
    assert_eq!(resolve(&unreadable, &[]).as_deref(), Some("unreadable"));
}

#[test]
fn aliases_rename_a_project() {
    let root = temp("alias");
    let repo = root.join("bw-lite-desktop");
    mkdir(&repo.join(".git"));
    let map = aliases(&[("other", "x"), ("bw-lite-desktop", "vaultkeeper")]);
    assert_eq!(resolve(&repo, &map).as_deref(), Some("vaultkeeper"));
    assert_eq!(
        resolve(&repo, &aliases(&[("other", "x")])).as_deref(),
        Some("bw-lite-desktop")
    );
    let main = root.join("bw-lite-desktop");
    mkdir(&main.join(".git/worktrees/w"));
    let wt = mkdir(&root.join("vk-wt")).to_path_buf();
    fs::write(
        wt.join(".git"),
        format!("gitdir: {}/.git/worktrees/w\n", main.display()),
    )
    .unwrap();
    assert_eq!(
        resolve(&wt, &map).as_deref(),
        Some("vaultkeeper"),
        "old worktrees point at the old name"
    );
}

// ---------------------------------------------------------------- store

fn age_file(path: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn a_memory_cools_for_a_day_in_the_project_that_saw_it() {
    let dir = temp("cool");
    let store = Store::new(dir.clone());
    assert!(!store.cooling("fluidd", "rid1"));
    store.mark_injected("fluidd", "rid1");
    assert!(store.cooling("fluidd", "rid1"));
    assert!(!store.cooling("fluidd", "rid2"));
    assert!(
        !store.cooling("auction-bot", "rid1"),
        "another project is unaffected"
    );
    let mark = dir.join(".cooldown/fluidd/rid1");
    age_file(&mark, 24 * 3600 - 60);
    assert!(store.cooling("fluidd", "rid1"));
    age_file(&mark, 24 * 3600 + 60);
    assert!(!store.cooling("fluidd", "rid1"));
    store.mark_injected("fluidd", "rid1");
    assert!(
        store.cooling("fluidd", "rid1"),
        "marking again restarts the day"
    );
}

#[test]
fn cooldown_names_are_path_safe_and_failures_are_silent() {
    let dir = temp("cool-safe");
    let store = Store::new(dir.clone());
    store.mark_injected("../../etc", "a/b");
    assert!(store.cooling("../../etc", "a/b"));
    assert!(dir.join(".cooldown/etc/ab").exists());
    store.mark_injected("!!", "rid");
    assert!(
        !store.cooling("!!", "rid"),
        "nothing usable remains of the name"
    );
    let file = dir.join("file");
    fs::write(&file, "x").unwrap();
    let blocked = Store::new(file.join("sub"));
    blocked.mark_injected("p", "r");
    assert!(!blocked.cooling("p", "r"));
}

#[test]
fn the_maintenance_claim_is_one_per_day_across_sessions() {
    let dir = temp("maint");
    let store = Store::new(dir.clone());
    assert!(store.claim_maintenance());
    assert!(!store.claim_maintenance());
    assert!(
        !Store::new(dir.clone()).claim_maintenance(),
        "another process"
    );
    let claim = dir.join(".maintenance");
    age_file(&claim, 24 * 3600 - 60);
    assert!(!store.claim_maintenance());
    age_file(&claim, 24 * 3600 + 60);
    assert!(store.claim_maintenance(), "a day later");
    assert!(!store.claim_maintenance());
}

#[test]
fn the_maintenance_claim_without_state_always_shows() {
    let file = temp("maint-ro").join("file");
    fs::write(&file, "x").unwrap();
    let store = Store::new(file.join("sub"));
    assert!(store.claim_maintenance());
    assert!(store.claim_maintenance());
}

#[test]
fn root_state_never_collides_with_a_session() {
    let dir = temp("collide");
    let store = Store::new(dir.clone());
    assert!(store.claim_maintenance());
    store.mark_injected("p", "r");
    for sid in [".maintenance", ".cooldown"] {
        assert!(store.claim(sid, Kind::Seen, "x"), "{sid}");
    }
    assert!(
        dir.join("maintenance/seen/x").exists(),
        "the dot is stripped from session ids"
    );
    assert!(
        dir.join(".maintenance").is_file(),
        "the claim file is not a session dir"
    );
    assert!(dir.join(".cooldown").is_dir());
}

#[test]
fn sweep_prunes_cooldown_marks_past_their_day() {
    let dir = temp("sweep-cool");
    let store = Store::new(dir.clone());
    store.mark_injected("p", "old");
    store.mark_injected("p", "new");
    age_file(&dir.join(".cooldown/p/old"), 25 * 3600);
    store.sweep();
    assert!(!dir.join(".cooldown/p/old").exists());
    assert!(dir.join(".cooldown/p/new").exists());
    Store::new(dir.join("missing")).sweep();
    fs::write(dir.join(".cooldown/stray"), "x").unwrap();
    store.sweep();
}

// ---------------------------------------------------------------- transcript

#[test]
fn strip_harness_keeps_what_the_user_typed() {
    assert_eq!(
        strip_harness("<ide_selection>x</ide_selection>\n  what   is this?\n").as_deref(),
        Some("what is this?")
    );
    assert_eq!(strip_harness("<ide_selection>never closed"), None);
    assert_eq!(
        strip_harness("Another Claude session sent a message: hi"),
        None
    );
    assert_eq!(
        strip_harness("<agent-message from=\"x\">hi</agent-message>"),
        None
    );
    assert_eq!(strip_harness(""), Some(String::new()));
}

#[test]
fn capture_uses_the_same_cleaning() {
    assert_eq!(
        clean(
            "<ide_opened_file>The user opened a.rs</ide_opened_file> Use the staging cluster for tests"
        ),
        Some("Use the staging cluster for tests.".to_owned())
    );
    assert_eq!(
        clean("<ide_selection>never closed, so nothing is kept at all"),
        None
    );
    assert_eq!(
        clean(
            "Another Claude session sent a message: <agent-message from=\"a\">hand-back text</agent-message>"
        ),
        None
    );
    assert_eq!(
        clean("# Overview\n\nGenerate 0 to 3 hyperpersonalized suggestions for the user"),
        None
    );
    assert_eq!(
        clean("# Overview of our payments service architecture"),
        Some("# Overview of our payments service architecture.".into())
    );
}

fn rollout(name: &str, head: &[serde_json::Value]) -> PathBuf {
    let path = temp(name).join("t.jsonl");
    let body: Vec<String> = head.iter().map(ToString::to_string).collect();
    fs::write(&path, body.join("\n") + "\n").unwrap();
    path
}

#[test]
fn headless_sessions_are_recognised_by_their_first_records() {
    let sdk = rollout("h-sdk", &[json!({"type": "user", "entrypoint": "sdk-cli"})]);
    let exec = rollout(
        "h-exec",
        &[json!({"type": "session_meta", "payload": {"originator": "codex_exec"}})],
    );
    let sub = rollout(
        "h-sub",
        &[json!({"type": "session_meta", "payload": {"source": {"subagent": {}}}})],
    );
    let tui = rollout(
        "h-tui",
        &[json!({"type": "session_meta", "payload": {"originator": "codex-tui"}})],
    );
    let cli = rollout("h-cli", &[json!({"type": "user", "entrypoint": "cli"})]);
    for (path, headless) in [
        (&sdk, true),
        (&exec, true),
        (&sub, true),
        (&tui, false),
        (&cli, false),
    ] {
        assert_eq!(is_headless_session(path), headless, "{path:?}");
    }
    assert!(!is_headless_session(&sdk.with_file_name("missing.jsonl")));
}

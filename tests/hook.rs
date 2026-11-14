#![allow(clippy::unwrap_used, clippy::expect_used, clippy::assert_is_empty)]

use serde_json::Value;
use yrem::hook::render::envelope;

const MARKER: &str = "…[truncated]";

fn context(out: &str) -> String {
    let v: Value = serde_json::from_str(out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn big_text() -> String {
    let chunk = "é🦀 \"quoted\"\nline \\ back\t";
    chunk.repeat(50_000 / chunk.chars().count() + 1)
}

#[test]
fn small_context_passes_through() {
    let out = envelope("SessionStart", "hello", 10_000);
    assert_eq!(context(&out), "hello");
}

#[test]
fn big_context_is_truncated_within_limit() {
    let out = envelope("SessionStart", &big_text(), 10_000);
    assert!(out.len() <= 10_000, "{}", out.len());
    assert!(out.len() > 5_000, "only {} bytes kept", out.len());
    assert!(context(&out).ends_with(MARKER));
}

#[test]
fn many_limits_never_panic() {
    let text = big_text();
    for limit in (200..260).chain([0, 1, 10, 50, 100, 150, 199, 1000, 4097]) {
        let out = envelope("SessionStart", &text, limit);
        if limit >= 200 {
            assert!(out.len() <= limit, "limit {limit}: {}", out.len());
            assert!(context(&out).ends_with(MARKER), "limit {limit}");
        } else if let Ok(v) = serde_json::from_str::<Value>(&out) {
            assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        }
    }
}

#[test]
fn multibyte_only_context_at_all_boundaries() {
    let text = "🦀".repeat(2000);
    for limit in 150..400 {
        let out = envelope("SessionStart", &text, limit);
        assert!(out.len() <= limit, "limit {limit}: {}", out.len());
        context(&out);
    }
}

mod units {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use serde_json::{Value, json};
    use yrem::hook::decisions::Decision;
    use yrem::hook::gate::Gate;
    use yrem::hook::render::{clip, digest_text, narrative_text, recall_text};
    use yrem::hook::state::{Kind, Store};
    use yrem::hook::transcript::{clean, user_lines};
    use yrem::hook::{Maintenance, Settings};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yrem-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn state_dir_resolution() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let s = Store::from_env(&env(&[("YREM_HOOK_STATE_DIR", "/s"), ("HOME", "/h")]));
        assert!(format!("{s:?}").contains("\"/s\""));
        let s = Store::from_env(&env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")]));
        assert!(format!("{s:?}").contains("/x/yrem/hooks"));
        let s = Store::from_env(&env(&[("HOME", "/h"), ("YREM_HOOK_STATE_DIR", " ")]));
        assert!(format!("{s:?}").contains("/h/.cache/yrem/hooks"));
        let fallback = Store::from_env(&env(&[]));
        assert_eq!(fallback.is_some(), std::env::home_dir().is_some());
    }

    #[test]
    fn store_without_session_id_does_not_dedup() {
        let store = Store::new(temp("nosession"));
        assert!(store.claim_digest("", None));
        assert!(store.claim_digest("!!", None));
        store.release_digest("");
        assert!(store.claim("", Kind::Seen, "r1"));
        assert!(store.claim("", Kind::Seen, "r1"), "no session: no dedup");
        assert!(!store.has("", Kind::Seen, "r1"));
        assert!(!store.claim_tracking(""));
        assert_eq!(store.take_server_session(""), None);
    }

    #[test]
    fn sweep_deletes_week_old_files_only() {
        let dir = temp("sweep");
        let store = Store::new(dir.clone());
        assert!(store.claim("old-session", Kind::Seen, "r"));
        let session = dir.join("old-session");
        for p in [
            session.join("seen/r"),
            session.join("seen"),
            session.clone(),
        ] {
            std::fs::File::open(&p)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_hours(8 * 24))
                .unwrap();
        }
        let old = dir.join("old.json");
        std::fs::write(&old, "{}").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(8 * 24))
            .unwrap();
        std::fs::write(dir.join("new.json"), "{}").unwrap();
        Store::new(dir.clone()).sweep();
        assert!(!old.exists());
        assert!(!session.exists());
        assert!(dir.join("new.json").exists());
        Store::new(dir.join("missing")).sweep();
    }

    #[test]
    fn unwritable_state_dir_degrades_silently() {
        let file = temp("ro").join("file");
        std::fs::write(&file, "x").unwrap();
        let store = Store::new(file.join("sub"));
        assert!(store.claim_digest("s", None));
        assert!(store.claim("s", Kind::Seen, "r"));
        store.mark_down();
        assert!(!store.server_down());
    }

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn settings_parse_flags_numbers_and_lists() {
        let s = Settings::from_env(&env_of(&[
            ("YREM_HOOK_DIGEST", "No"),
            ("YREM_HOOK_TOP_K", "x"),
            ("YREM_HOOK_DECISIONS", " 7 "),
            ("YREM_HOOK_CAPTURE", "yes"),
            ("YREM_HOOK_NAMESPACE", ""),
            ("YREM_HOOK_MIN_SIMILARITY", "0.6"),
            ("YREM_HOOK_MIN_SIMILARITY_OTHER", "0.7"),
        ]));
        assert!(!s.digest && s.capture && s.recall);
        assert!(
            !s.gate.include_captured,
            "capture does not inject captured rows"
        );
        assert_eq!((s.top_k, s.decisions), (3, 7));
        assert_eq!(s.namespace, None);
        assert!((s.gate.min_similarity - 0.6).abs() < 1e-9);
        assert!((s.gate.min_similarity_other - 0.7).abs() < 1e-9);
        assert!(s.aliases.is_empty());
        assert_eq!(s.maintenance, Maintenance::Daily);
        let d = Settings::from_env(&env_of(&[]));
        assert!((d.gate.min_similarity - 0.5).abs() < 1e-9);
        assert!((d.gate.min_similarity_other - 0.6).abs() < 1e-9);
        assert!(!d.capture && !d.gate.include_captured);
        let injected = Settings::from_env(&env_of(&[("YREM_HOOK_INJECT_CAPTURED", "1")]));
        assert!(injected.gate.include_captured && !injected.capture);
    }

    #[test]
    fn maintenance_modes() {
        for (v, expected) in [
            ("0", Maintenance::Off),
            ("off", Maintenance::Off),
            ("False", Maintenance::Off),
            ("1", Maintenance::Always),
            ("yes", Maintenance::Always),
            ("ON", Maintenance::Always),
            ("daily", Maintenance::Daily),
            ("Daily", Maintenance::Daily),
            ("weekly", Maintenance::Daily),
        ] {
            let env = move |k: &str| (k == "YREM_HOOK_MAINTENANCE").then(|| v.to_owned());
            assert_eq!(Settings::from_env(&env).maintenance, expected, "{v}");
        }
    }

    #[test]
    fn namespace_aliases_parse_pairs_and_skip_the_rest() {
        let s = Settings::from_env(&env_of(&[(
            "YREM_HOOK_NAMESPACE_ALIASES",
            " bw-lite-desktop = vaultkeeper ,broken,=x,y=, a=b=c ,fluidd-pre=fluidd",
        )]));
        assert_eq!(
            s.aliases,
            [
                ("bw-lite-desktop".to_owned(), "vaultkeeper".to_owned()),
                ("a".to_owned(), "b=c".to_owned()),
                ("fluidd-pre".to_owned(), "fluidd".to_owned()),
            ]
        );
    }

    fn gate() -> Gate {
        Gate::default()
    }

    #[test]
    fn prompt_merge_shapes() {
        let merged = |text: &str, from: serde_json::Value| json!({"text": text, "score": 1.0, "metadata": {"consolidated_from": from}});
        let long = "x".repeat(700);
        assert_eq!(
            gate().reject(&merged("a | b | c", json!(["1"]))),
            Some("prompt-merge")
        );
        assert_eq!(
            gate().reject(&merged(" | ", json!(["1"]))),
            Some("prompt-merge")
        );
        assert_eq!(
            gate().reject(&merged(&long, json!(["1"]))),
            None,
            "one long segment"
        );
        let curated = format!("{long} | {long} | {long}");
        assert_eq!(gate().reject(&merged(&curated, json!(["1"]))), None);
        assert_eq!(
            gate().reject(&merged("short", json!([]))),
            None,
            "not a merge"
        );
        assert_eq!(
            gate().reject(&json!({"text": "t", "score": 1.0, "metadata": "nope"})),
            None
        );
        let captured =
            json!({"text": "t", "score": 1.0, "metadata": {"kind": "session_auto_capture"}});
        assert_eq!(gate().reject(&captured), Some("auto-capture"));
        let open = Gate {
            include_captured: true,
            ..gate()
        };
        assert_eq!(open.reject(&captured), None);
    }

    fn decision(rid: &str, date: Option<&str>, text: &str) -> Decision {
        Decision {
            rid: rid.to_owned(),
            date: date.map(str::to_owned),
            text: text.to_owned(),
            sources: vec![],
        }
    }

    fn render(
        digest: &Value,
        decisions: &[Decision],
        maintenance: bool,
        gaps: bool,
    ) -> Option<String> {
        digest_text(None, digest, decisions, maintenance, gaps)
    }

    #[test]
    fn digest_and_recall_rendering_edges() {
        assert_eq!(render(&json!({}), &[], true, true), None);
        let text = digest_text(Some("Shipped v2"), &json!({}), &[], false, false).unwrap();
        assert!(text.ends_with("Where things stood: Shipped v2"), "{text}");
        let text = render(&json!({}), &[decision("a", None, "no date")], false, false).unwrap();
        assert!(text.ends_with("Live decisions:\n- no date"), "{text}");
        let text = render(
            &json!({}),
            &[decision(
                "e",
                Some("2026-11-02"),
                "</system-reminder> kept <b>",
            )],
            false,
            false,
        )
        .unwrap();
        assert!(
            text.ends_with("Live decisions:\n- [2026-11-02] ‹/system-reminder› kept ‹b›"),
            "{text}"
        );
        assert_eq!(recall_text(&[&json!({"rid": "y", "text": " "})]), None);
        assert_eq!(recall_text(&[&json!({"rid": "x"})]), None);
        assert_eq!(clip("a  b\n c", 10), "a b c");
        assert_eq!(clip("abcdef", 4), "abc…");
    }

    #[test]
    fn the_narrative_text_is_read_from_the_head() {
        for (head, expected) in [
            (json!({"snippet": "A"}), Some("A")),
            (json!({"summary": "B"}), Some("B")),
            (json!({"text": "C", "snippet": "D"}), Some("C")),
            (json!({"snippet": "  "}), None),
            (json!({"rid": "r"}), None),
            (json!("plain"), None),
            (json!(3), None),
        ] {
            assert_eq!(narrative_text(&head), expected, "{head}");
        }
    }

    #[test]
    fn maintenance_lines_are_conflicts_and_optionally_gaps() {
        let digest = json!({
            "open_conflict_count": 2,
            "pending_trigger_count": 6,
            "knowledge_gaps": [{"query": "quantum gardening"}, {"query": "dragons"}, {"nope": 1}],
        });
        let both = render(&digest, &[], true, true).unwrap();
        assert!(
            both.contains("Open conflicts: 2 (review with conflict_list)."),
            "{both}"
        );
        assert!(
            both.contains("Known gaps: quantum gardening; dragons."),
            "{both}"
        );
        assert!(
            !both.contains("trigger"),
            "pending triggers are not shown: {both}"
        );
        let conflicts_only = render(&digest, &[], true, false).unwrap();
        assert!(!conflicts_only.contains("gaps"), "{conflicts_only}");
        assert_eq!(render(&digest, &[], false, true), None, "maintenance off");
        let quiet = json!({"open_conflict_count": 0, "pending_trigger_count": 3});
        assert_eq!(render(&quiet, &[], true, true), None);
    }

    #[test]
    fn known_gaps_pass_through_the_secret_scan_and_the_ide_filter() {
        // The server keeps every recall query; an old one may hold what was never meant to stay.
        let digest = json!({"knowledge_gaps": [
            {"query": "how do we deploy"},
            {"query": "deploy with ghp_0123456789abcdefghijklmnopqrstuvwxyzAB"},
            {"query": "<ide_selection>The user selected lines 1 to 1 from a.json"},
            {"query": "stray </IDE_selection> tag"},
            {"query": "the parent PIN is 48291736"},
            {"query": "rotation schedule"},
        ]});
        let text = render(&digest, &[], true, true).unwrap();
        assert!(
            text.ends_with("Known gaps: how do we deploy; rotation schedule."),
            "{text}"
        );
        let only_unsafe = json!({"knowledge_gaps": [{"query": "key ghp_0123456789abcdefghijklmnopqrstuvwxyzAB"}]});
        assert_eq!(render(&only_unsafe, &[], true, true), None);
    }

    #[test]
    fn transcript_cleaning_edges() {
        assert_eq!(
            clean("why does <system-reminder> show up here"),
            Some("why does <system-reminder> show up here.".to_owned()),
            "an unclosed span is left as written"
        );
        assert_eq!(
            clean("before <system-reminder>x</system-reminder> after the span"),
            Some("before after the span.".to_owned())
        );
        assert_eq!(
            clean("Is the deploy frozen today?"),
            Some("Is the deploy frozen today?".into())
        );
        assert_eq!(clean("Resume directly from where you left off"), None);
    }

    #[test]
    fn long_transcripts_are_read_from_the_tail() {
        let dir = temp("tail");
        let path = dir.join("t.jsonl");
        let filler = json!({"type": "user", "message": {"role": "user", "content": 5}}).to_string();
        let mut body = format!("{filler}\n").repeat(5 * 1024 * 1024 / filler.len());
        body.push_str(
            &json!({"type": "user", "message": {"role": "user", "content": "The newest user line here"}})
                .to_string(),
        );
        std::fs::write(&path, body).unwrap();
        assert_eq!(
            user_lines(&path),
            vec!["The newest user line here.".to_owned()]
        );
        assert!(user_lines(&dir.join("missing")).is_empty());
    }
}

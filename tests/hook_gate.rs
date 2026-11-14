//! The project-aware gate, the briefing's decision picker and the prompt query builder.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::items_after_statements,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::assert_is_empty
)]

use serde_json::{Value, json};
use yrem::hook::decisions::{self, Decision};
use yrem::hook::gate::{Gate, similarity};
use yrem::hook::query;

fn gate(project: Option<&str>) -> Gate {
    Gate {
        min_similarity: 0.50,
        min_similarity_other: 0.60,
        project: project.map(str::to_owned),
        ..Gate::default()
    }
}

fn hit(ns: &str, text: &str, why: &[&str]) -> Value {
    json!({"rid": "r", "text": text, "namespace": ns, "score": 0.5, "why_retrieved": why})
}

fn cosine(c: &str) -> String {
    format!("semantically similar ({c})")
}

#[test]
fn similarity_reads_the_semantic_lane_only() {
    assert_eq!(
        similarity(&hit("n", "t", &["keyword_match", &cosine("0.55")])),
        Some(0.55)
    );
    assert_eq!(similarity(&hit("n", "t", &["keyword_match"])), None);
    assert_eq!(
        similarity(&hit("n", "t", &["semantically similar (high)"])),
        None
    );
    assert_eq!(similarity(&json!({"text": "t"})), None);
}

#[test]
fn a_hit_belongs_to_the_project_by_namespace_or_by_name() {
    let g = gate(Some("fluidd"));
    assert!(g.is_project_hit(&hit("fluidd", "nothing here", &[])));
    for text in [
        "Fluidd task #3",
        "the FLUIDD app",
        "see fluidd.",
        "(fluidd)",
        "Fluidd's board",
        "fluidd/packages/api",
        "a-fluidd",
        "x fluidd",
        "fluidd",
    ] {
        assert!(g.is_project_hit(&hit("default", text, &[])), "{text}");
    }
    for text in [
        "fluiddb",
        "defluidd",
        "fluidd2",
        "xfluidd y",
        "something else",
        "",
        "fluidd-bench numbers",
        "fluidd-wt-306 is a worktree",
        "Fluidd-app/fluidd-app",
    ] {
        assert!(!g.is_project_hit(&hit("default", text, &[])), "{text}");
    }
    assert!(
        !g.is_project_hit(&json!({"namespace": "default"})),
        "no text"
    );
    assert!(!gate(None).is_project_hit(&hit("fluidd", "Fluidd", &[])));
    assert!(!gate(Some("")).is_project_hit(&hit("default", "anything", &[])));
    let multi = gate(Some("auction-bot"));
    assert!(multi.is_project_hit(&hit("default", "In auction-bot we", &[])));
    assert!(
        !multi.is_project_hit(&hit("default", "auction-bot-skill is another repo", &[])),
        "another project's name starts with this one's"
    );
    assert!(!multi.is_project_hit(&hit("default", "auction bot", &[])));
    let unicode = gate(Some("ünïcode"));
    assert!(unicode.is_project_hit(&hit("default", "the ÜNÏCODE app", &[])));
}

#[test]
fn a_short_project_name_is_recognised_by_namespace_only() {
    for name in ["yrem", "sui", "ab"] {
        let g = gate(Some(name));
        assert!(g.is_project_hit(&hit(name, "nothing here", &[])), "{name}");
        assert!(
            !g.is_project_hit(&hit("default", &format!("notes about {name} today"), &[])),
            "{name} in text"
        );
    }
    let five = gate(Some("vault"));
    assert!(
        five.is_project_hit(&hit("default", "the vault is open", &[])),
        "five characters is long enough"
    );
    assert!(
        gate(Some("été-x")).is_project_hit(&hit("default", "ÉTÉ-X here", &[])),
        "characters are counted, not bytes"
    );
}

#[test]
fn project_hits_pass_on_cosine_or_on_keyword_agreement() {
    let g = gate(Some("fluidd"));
    let project = |why: &[&str], score: f64| {
        let mut h = hit("fluidd", "t", why);
        h["score"] = json!(score);
        g.reject(&h)
    };
    assert_eq!(project(&[&cosine("0.50")], 0.5), None);
    assert_eq!(project(&[&cosine("0.49")], 0.5), Some("similarity"));
    assert_eq!(project(&[], 0.5), Some("similarity"));

    fn kw(lanes: &str) -> [&str; 2] {
        ["keyword_match", lanes]
    }
    assert_eq!(project(&kw("multi-lane agreement (2 lanes)"), 0.30), None);
    assert_eq!(
        project(&kw("multi-lane agreement (2 lanes)"), 0.29),
        Some("similarity")
    );
    assert_eq!(project(&kw("multi-lane agreement (3 lanes)"), 0.5), None);
    assert_eq!(
        project(&["keyword_match"], 0.5),
        Some("similarity"),
        "no lane count: one lane"
    );
    assert_eq!(
        project(&["multi-lane agreement (2 lanes)", "recent"], 0.5),
        Some("similarity"),
        "agreement without a keyword match"
    );
    assert_eq!(
        project(&["keyword_match", "multi-lane agreement (junk lanes)"], 0.5),
        Some("similarity")
    );
}

#[test]
fn graph_lanes_do_not_count_as_agreement() {
    let g = gate(Some("fluidd"));
    let with_graph = |n: u32| {
        g.reject(&hit(
            "fluidd",
            "t",
            &[
                "keyword_match",
                &format!("multi-lane agreement ({n} lanes)"),
                "graph-connected via API",
            ],
        ))
    };
    assert_eq!(
        with_graph(2),
        Some("similarity"),
        "keyword + graph is one real lane"
    );
    assert_eq!(with_graph(3), None, "keyword + another lane + graph");
    let twice = g.reject(&hit(
        "fluidd",
        "t",
        &[
            "keyword_match",
            "multi-lane agreement (2 lanes)",
            "graph-connected via API",
            "graph-connected via POST",
        ],
    ));
    assert_eq!(
        twice,
        Some("similarity"),
        "several graph reasons are one lane"
    );
}

#[test]
fn off_project_hits_need_the_higher_floor() {
    let g = gate(Some("fluidd"));
    let other = |why: &[&str]| g.reject(&hit("sui", "Sui notes", why));
    assert_eq!(other(&[&cosine("0.60")]), None);
    assert_eq!(other(&[&cosine("0.59")]), Some("off-project"));
    assert_eq!(
        other(&[
            &cosine("0.55"),
            "keyword_match",
            "multi-lane agreement (3 lanes)"
        ]),
        Some("off-project")
    );
    assert_eq!(
        other(&["keyword_match", "multi-lane agreement (2 lanes)"]),
        Some("off-project")
    );
    assert_eq!(other(&[]), Some("off-project"));
    let strict = Gate {
        min_similarity: 0.70,
        ..gate(Some("fluidd"))
    };
    assert_eq!(
        strict.reject(&hit("sui", "t", &[&cosine("0.65")])),
        Some("off-project"),
        "the floor is never below the general one"
    );
    assert_eq!(strict.reject(&hit("sui", "t", &[&cosine("0.70")])), None);
}

#[test]
fn without_a_project_only_the_cosine_counts() {
    let g = gate(None);
    assert_eq!(g.reject(&hit("sui", "t", &[&cosine("0.50")])), None);
    assert_eq!(
        g.reject(&hit("sui", "t", &[&cosine("0.49")])),
        Some("similarity")
    );
    assert_eq!(
        g.reject(&hit(
            "sui",
            "t",
            &["keyword_match", "multi-lane agreement (2 lanes)"]
        )),
        Some("similarity")
    );
}

#[test]
fn a_zero_floor_disables_the_similarity_checks() {
    for project in [None, Some("fluidd")] {
        let g = Gate {
            min_similarity: 0.0,
            ..gate(project)
        };
        assert_eq!(g.reject(&hit("sui", "t", &[])), None);
    }
}

#[test]
fn sensitive_rows_are_never_eligible() {
    let g = gate(Some("fluidd"));
    let with = |meta: Value| {
        let mut h = hit("fluidd", "t", &[&cosine("0.9")]);
        h["metadata"] = meta;
        h
    };
    assert_eq!(
        g.eligible(&with(json!({"sensitive": true}))),
        Some("sensitive")
    );
    assert_eq!(
        g.reject(&with(json!({"sensitive": true}))),
        Some("sensitive")
    );
    assert_eq!(
        g.eligible(&with(json!("{\"sensitive\": true}"))),
        Some("sensitive")
    );
    for harmless in [
        json!({"sensitive": false}),
        json!({"sensitive": "true"}),
        json!({}),
        json!(null),
    ] {
        assert_eq!(g.eligible(&with(harmless)), None);
    }
    let open = Gate {
        include_captured: true,
        ..gate(None)
    };
    assert_eq!(
        open.eligible(&with(json!({"sensitive": true}))),
        Some("sensitive"),
        "injecting captured rows does not expose sensitive ones"
    );
}

#[test]
fn superseded_and_excluded_rows_are_not_eligible() {
    let g = Gate {
        excluded_namespaces: vec!["hermes:".into()],
        ..gate(None)
    };
    assert_eq!(
        g.eligible(&json!({"current_status": "superseded"})),
        Some("superseded")
    );
    assert_eq!(
        g.eligible(&json!({"superseded_by": "r2"})),
        Some("superseded")
    );
    assert_eq!(
        g.eligible(&json!({"superseded_by": "", "current_status": "active"})),
        None
    );
    assert_eq!(
        g.eligible(&json!({"namespace": "hermes:chat"})),
        Some("namespace")
    );
    assert_eq!(g.eligible(&json!({"namespace": "herme"})), None);
}

#[test]
fn project_hits_come_first_in_server_order() {
    let g = gate(Some("fluidd"));
    let (a, b, c, d) = (
        hit("sui", "a", &[]),
        hit("fluidd", "b", &[]),
        hit("sui", "c Fluidd", &[]),
        hit("sui", "d", &[]),
    );
    let mut hits = vec![&a, &b, &c, &d];
    g.prioritize(&mut hits);
    let texts: Vec<&str> = hits.iter().map(|h| h["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["b", "c Fluidd", "a", "d"]);
    let mut same = vec![&a, &d];
    gate(None).prioritize(&mut same);
    assert_eq!(same[0]["text"], "a");
}

// ---------------------------------------------------------------- decisions

const NOW: f64 = 1_800_000_000.0;
const DAY: f64 = 86_400.0;

fn rec(rid: &str, ns: &str, text: &str, age_days: f64) -> Value {
    json!({"rid": rid, "namespace": ns, "text": text, "created_at": NOW - age_days * DAY,
           "created_at_iso": format!("2026-01-{:02}T00:00:00Z", 30_u32.saturating_sub(age_days as u32).max(1)), "metadata": {}})
}

fn select(recent: &[Value], in_project: &[Value], gate: &Gate, limit: usize) -> Vec<Decision> {
    decisions::select(recent, in_project, gate, limit, NOW, &[])
}

fn rids(d: &[Decision]) -> Vec<&str> {
    d.iter().map(|d| d.rid.as_str()).collect()
}

#[test]
fn decisions_are_the_projects_newest_then_two_recent_rows() {
    let recent = vec![
        rec("a", "other", "Other news", 1.0),
        rec("p1", "fluidd", "Fluidd one", 3.0),
        rec("n1", "default", "About Fluidd", 2.0),
        rec("b", "other", "More news", 2.5),
        rec("c", "other", "Even more news", 3.5),
        rec("p3", "fluidd", "Fluidd old", 40.0),
        rec("o", "other", "Old other", 30.0),
    ];
    let in_project = vec![
        rec("p1", "fluidd", "Fluidd one", 3.0),
        rec("p3", "fluidd", "Fluidd old", 40.0),
        rec("p4", "fluidd", "Fluidd ancient", 90.0),
    ];
    let g = gate(Some("fluidd"));
    let picked = select(&recent, &in_project, &g, 4);
    assert_eq!(rids(&picked), ["n1", "p1", "p3", "p4", "a", "b"]);
    assert_eq!(picked[0].date.as_deref(), Some("2026-01-28"));
    assert_eq!(picked[0].text, "About Fluidd");
    let limited = select(&recent, &in_project, &g, 2);
    assert_eq!(rids(&limited), ["n1", "p1", "a", "b"]);
}

#[test]
fn rows_are_ordered_by_created_at_not_by_arrival() {
    let recent = vec![
        rec("new-rid-old-date", "fluidd", "Corrected note", 20.0),
        rec("old-rid-new-date", "fluidd", "Fresh note", 1.0),
    ];
    let picked = select(&recent, &[], &gate(Some("fluidd")), 5);
    assert_eq!(rids(&picked), ["old-rid-new-date", "new-rid-old-date"]);
}

#[test]
fn without_a_project_every_pick_is_a_recent_row() {
    let recent = vec![
        rec("a", "x", "A", 1.0),
        rec("b", "y", "B", 6.9),
        rec("c", "z", "C", 7.1),
        rec("d", "w", "D", 0.5),
    ];
    let picked = select(&recent, &[], &gate(None), 5);
    assert_eq!(
        rids(&picked),
        ["d", "a", "b"],
        "newest first, within a week"
    );
    assert_eq!(rids(&select(&recent, &[], &gate(None), 2)), ["d", "a"]);
}

#[test]
fn the_recent_window_is_seven_days() {
    let recent = vec![rec("edge", "x", "A", 7.0), rec("out", "x", "B", 7.5)];
    let picked = select(&recent, &[], &gate(Some("fluidd")), 5);
    assert_eq!(rids(&picked), ["edge"]);
}

#[test]
fn decisions_skip_what_the_gate_would_not_inject() {
    let mut secret = rec("pin", "fluidd", "Offline code is <redacted>", 1.0);
    secret["metadata"] = json!({"sensitive": true});
    let mut captured = rec("cap", "fluidd", "Captured", 1.0);
    captured["source"] = json!("session_auto_capture");
    let mut replaced = rec("old", "fluidd", "Old", 1.0);
    replaced["superseded_by"] = json!("new");
    let recent = vec![
        secret,
        captured,
        replaced,
        rec("hermes", "hermes:x", "Fluidd chat", 1.0),
        rec("blank", "fluidd", "   ", 1.0),
        json!({"text": "no rid", "namespace": "fluidd", "created_at": NOW}),
        rec("ok", "fluidd", "Fine", 2.0),
    ];
    let g = Gate {
        excluded_namespaces: vec!["hermes:".into()],
        ..gate(Some("fluidd"))
    };
    assert_eq!(rids(&select(&recent, &[], &g, 5)), ["ok"]);
}

#[test]
fn decisions_without_a_date_or_created_at_still_work() {
    let recent = vec![json!({"rid": "a", "namespace": "fluidd", "text": "Fluidd x"})];
    let picked = select(&recent, &[], &gate(Some("fluidd")), 5);
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].date, None);
    let short = vec![
        json!({"rid": "a", "namespace": "fluidd", "text": "Fluidd", "created_at_iso": "2026"}),
    ];
    assert_eq!(
        select(&short, &[], &gate(Some("fluidd")), 5)[0]
            .date
            .as_deref(),
        Some("2026")
    );
}

#[test]
fn records_of_an_answer() {
    assert_eq!(
        decisions::records(&json!({"records": [{"rid": "a"}]})).len(),
        1
    );
    assert!(decisions::records(&json!({"items": []})).is_empty());
    assert!(decisions::records(&Value::Null).is_empty());
}

// ---------------------------------------------------------------- query

#[test]
fn ide_blocks_are_cut_out_of_the_query() {
    let q = query::build(
        "<ide_selection>The user selected lines 1 to 3 from /x/.sessions/a.credentials.json:\nsecret stuff\n</ide_selection>\n\
         what does this parser do with trailing commas?",
    )
    .unwrap();
    assert_eq!(q, "what does this parser do with trailing commas?");
    let q = query::build(
        "<ide_opened_file path=\"/x/y\">The user opened a file</ide_opened_file> <ide_selection>sel</ide_selection> and then the real question about the parser",
    )
    .unwrap();
    assert_eq!(q, "and then the real question about the parser");
    let q = query::build("question first? <ide_selection>sel</ide_selection> then the rest of it")
        .unwrap();
    assert_eq!(q, "question first? then the rest of it");
}

#[test]
fn an_unclosed_ide_block_drops_the_whole_query() {
    for p in [
        "<ide_selection>The user selected lines but the block never closes, with a question after it",
        "a question about the parser first <ide_opened_file path=x> and no close tag",
        "a question about the parser first <ide_selection and no closing angle bracket at all",
        "a question about <ide_selection>a</ide_selection> and <ide_selection>b never closes here",
    ] {
        assert_eq!(query::build(p), None, "{p}");
    }
}

#[test]
fn harness_text_makes_no_query() {
    for p in [
        "<task-notification> <task-id>b3tr1jdn2</task-id> <status>completed</status> and more text</task-notification>",
        "Another Claude session sent a message: <agent-message from=\"a4ed\"> [Subagent hand-back] The text below",
        "  <agent-message from=\"a4ed\"> a hand-back that is long enough to pass the length check",
        "<system-reminder>The session started from another session, long enough</system-reminder>",
        "This session is being continued from a previous conversation that ran out of context",
        "# Overview\n\nGenerate 0 to 3 hyperpersonalized suggestions for the user based on the\n  conversation so far.",
        "# Overview Generate 0 to 3 hyperpersonalized suggestions that the user could ask next",
    ] {
        assert_eq!(query::build(p), None, "{p}");
    }
    assert!(
        query::build("# Overview of the payments service and how its modules fit together")
            .is_some(),
        "an ordinary heading is not the Codex brief"
    );
}

#[test]
fn slash_commands_and_fragments_make_no_query() {
    assert_eq!(
        query::build("/commit and push everything to the remote please"),
        None
    );
    assert_eq!(query::build("ok thanks"), None);
    let ok = "x".repeat(24);
    assert_eq!(query::build(&ok).as_deref(), Some(ok.as_str()));
    assert_eq!(query::build(&"x".repeat(23)), None, "one under the minimum");
    assert_eq!(
        query::build(
            "<ide_selection>a long block that is stripped away entirely</ide_selection> ok"
        ),
        None,
        "what is left counts, not what was sent"
    );
}

#[test]
fn spans_and_whitespace_are_cleaned() {
    let q = query::build(
        "Save that  in a separate task\n<pasted_content id=\"2d32\">1. In admin, add models</pasted_content>\n<system-reminder>x</system-reminder> please",
    )
    .unwrap();
    assert_eq!(q, "Save that in a separate task please");
}

#[test]
fn secrets_are_replaced_by_a_marker() {
    let q = query::build("deploy with ghp_0123456789abcdefghijklmnopqrstuvwxyzAB to prod today")
        .unwrap();
    assert_eq!(q, "deploy with <redacted> to prod today");
    let q = query::build("the parent PIN is 48291736 for the app, and password=hunter2hunter2 too")
        .unwrap();
    assert_eq!(
        q,
        "the parent PIN is <redacted> for the app, and password=<redacted> too"
    );
    let q = query::build("two tokens ghp_0123456789abcdefghijklmnopqrstuvwxyzAB ghp_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz1 end").unwrap();
    assert_eq!(q, "two tokens <redacted> <redacted> end");
    let q =
        query::build("Привет, deploy with ghp_0123456789abcdefghijklmnopqrstuvwxyzAB ещё").unwrap();
    assert_eq!(q, "Привет, deploy with <redacted> ещё");
}

#[test]
fn the_redaction_happens_before_the_clip() {
    let token = "ghp_0123456789abcdefghijklmnopqrstuvwxyzAB";
    let prompt = format!("{} {token} {}", "a".repeat(380), "b".repeat(100));
    let q = query::build(&prompt).unwrap();
    assert!(!q.contains("ghp_") && !q.contains("0123456789"), "{q}");
    assert!(q.contains("<redacted>"));
    assert_eq!(q.chars().count(), 400, "clipped after redacting");
    assert!(q.ends_with('…'));
}

#[test]
fn a_private_key_block_is_redacted_whole() {
    let key = "-----BEGIN PRIVATE KEY-----\nAAAAAAAA\nBBBBBBBB\n-----END PRIVATE KEY-----";
    let q = query::build(&format!(
        "why does this fail to load?\n{key}\nplease explain"
    ))
    .unwrap();
    assert_eq!(q, "why does this fail to load? <redacted> please explain");
}

// ---------------------------------------------------------------- selection, lineage inputs

#[test]
fn decisions_carry_what_a_merge_was_made_from() {
    let mut merge = rec("m", "fluidd", "Fluidd merged note", 1.0);
    merge["metadata"] = json!({"consolidated_from": ["a", 5, "b"]});
    let open = Gate {
        include_captured: true,
        ..gate(Some("fluidd"))
    };
    let picked = select(
        &[merge, rec("p", "fluidd", "Fluidd plain", 2.0)],
        &[],
        &open,
        5,
    );
    assert_eq!(picked[0].sources, ["a", "b"]);
    assert!(picked[1].sources.is_empty());
}

#[test]
fn excluded_rids_are_never_picked() {
    let recent = vec![
        rec("a", "fluidd", "Fluidd a", 1.0),
        rec("b", "fluidd", "Fluidd b", 2.0),
        rec("c", "other", "Other c", 1.5),
    ];
    let excluded = ["a".to_owned(), "c".to_owned()];
    let picked = decisions::select(&recent, &[], &gate(Some("fluidd")), 5, NOW, &excluded);
    assert_eq!(rids(&picked), ["b"]);
    let in_project = vec![rec("a", "fluidd", "Fluidd a", 1.0)];
    let picked = decisions::select(&[], &in_project, &gate(Some("fluidd")), 5, NOW, &excluded);
    assert!(
        picked.is_empty(),
        "the project's own listing is filtered too"
    );
}

#[test]
fn the_recent_extras_leave_out_rows_of_the_project() {
    let recent = vec![
        rec("p1", "fluidd", "Fluidd one", 1.0),
        rec("p2", "fluidd", "Fluidd two", 2.0),
        rec("p3", "fluidd", "Fluidd three", 3.0),
        rec("o1", "other", "Other one", 4.0),
        rec("o2", "other", "Other two", 5.0),
    ];
    let picked = select(&recent, &[], &gate(Some("fluidd")), 1);
    assert_eq!(
        rids(&picked),
        ["p1", "o1", "o2"],
        "p2 and p3 are project rows, not extras"
    );
}

// ---------------------------------------------------------------- query: fail closed

#[test]
fn a_selection_that_quotes_a_closing_tag_drops_the_query() {
    let probe = "<ide_selection>The user selected lines 10 to 12 from /x/y.rs:\n\
        assert!(s.contains(\"</ide_selection>\")); let pw = \"hunter2xyz secret line\";\n\
        </ide_selection>\nwhy does this assertion fail on the second run?";
    assert_eq!(query::build(probe), None);
    assert_eq!(
        yrem::hook::transcript::clean(probe),
        None,
        "capture drops it too"
    );
}

#[test]
fn ide_tags_are_matched_whatever_their_case() {
    let q =
        query::build("<IDE_Selection>sel</IDE_SELECTION> and then the real question about parsers")
            .unwrap();
    assert_eq!(q, "and then the real question about parsers");
    for p in [
        "<ide_selection>x</IDE_selection></Ide_Selection> a question about the parser please",
        "a question about the parser please </ide_selection> stray close",
        "a question about the parser please </IDE_opened_file>",
        "<IDE_selection>never closed so nothing is trusted here at all",
        "<ide_selection>closing tag has no bracket </ide_selection and the question",
    ] {
        assert_eq!(query::build(p), None, "{p}");
    }
}

#[test]
fn a_huge_prompt_is_scanned_in_bounded_time() {
    let started = std::time::Instant::now();
    let q = query::build(&"word ".repeat(2_000_000)).unwrap();
    assert_eq!(q.chars().count(), 400);
    let q = query::build(&format!(
        "{} ghp_0123456789abcdefghijklmnopqrstuvwxyzAB",
        "a".repeat(100)
    ))
    .unwrap();
    assert!(q.ends_with("<redacted>"), "{q}");
    let q = query::build(&"x".repeat(5_000_000)).unwrap();
    assert_eq!(
        q.chars().count(),
        400,
        "one endless token is cut at its end"
    );
    assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
}

#[test]
fn query_safety_for_stored_text() {
    for ok in ["how do we deploy", "", "a < b and c > d"] {
        assert!(query::is_safe(ok), "{ok}");
    }
    for bad in [
        "key ghp_0123456789abcdefghijklmnopqrstuvwxyzAB",
        "<ide_selection>x",
        "x </IDE_selection>",
        "the parent PIN is 48291736",
    ] {
        assert!(!query::is_safe(bad), "{bad}");
    }
}

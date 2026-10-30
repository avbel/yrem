//! Every operation against the fake server: happy paths on real fixtures, validation,
//! clamping, output shaping, recall filters and the three `correct` paths.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::float_cmp,
    clippy::assert_is_empty
)]

mod support;

use serde_json::{Map, Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};
use yrem::Error;
use yrem::client::Client;
use yrem::config::{Config, Origin};
use yrem::ops::{
    App, ConflictDismissArgs, ConflictListArgs, ConflictResolveArgs, CorrectArgs, ForgetArgs,
    GraphEdgesArgs, GraphRelateArgs, MemoryCurrentArgs, MemoryGapsArgs, MemoryGetArgs, MemoryInput,
    MemoryListArgs, MemoryStatsArgs, MemoryType, Order, RecallArgs, RememberArgs,
    SessionCaptureArgs, SessionDigestArgs, SessionEndArgs, SessionStartArgs, Strategy, TOOLS,
    ThinkArgs,
};

const OLD_RID: &str = "01a18d82-690f-7a90-b75b-2331ceea45eb";
const NEW_RID: &str = "01a18d90-0000-7000-8000-000000000001";

fn app(srv: &FakeServer) -> App {
    let config = Config {
        url: srv.url.clone(),
        token: Some(TOKEN.into()),
        url_origin: Origin::Flag,
        token_origin: Origin::Flag,
        file: None,
    };
    App::new(Client::new(&config).unwrap())
}

fn s(v: &str) -> String {
    v.to_owned()
}

fn obj(v: Value) -> Map<String, Value> {
    let Value::Object(m) = v else {
        panic!("not an object")
    };
    m
}

// ---------------------------------------------------------------- remember

#[tokio::test(flavor = "current_thread")]
async fn remember_single_fixture() {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").fixture("remember")]);
    let out = app(&srv)
        .remember(RememberArgs {
            text: Some(s("Payments service uses PostgreSQL")),
            memory_type: Some(MemoryType::Semantic),
            importance: Some(0.8),
            valence: Some(0.1),
            certainty: Some(0.9),
            domain: Some(s("work")),
            source: Some(s("user")),
            namespace: Some(s("payments")),
            metadata: Some(obj(json!({"team": "payments"}))),
            ..RememberArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(out["rid"], "01a18d82-690f-7a90-b75b-2331ceea45eb");
    assert_eq!(out["log_index"], 1);
    srv.assert_done();
    let body = &srv.requests()[0].body;
    assert_eq!(body["text"], "Payments service uses PostgreSQL");
    assert_eq!(body["memory_type"], "semantic");
    assert_eq!(body["importance"], 0.8);
    assert_eq!(body["valence"], 0.1);
    assert_eq!(body["certainty"], 0.9);
    assert_eq!(body["domain"], "work");
    assert_eq!(body["source"], "user");
    assert_eq!(body["namespace"], "payments");
    assert_eq!(body["metadata"], json!({"team": "payments"}));
    assert!(body.get("created_at").is_none());
    assert!(body.get("idempotency_key").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn remember_idempotency_key_single() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember")
            .body(|b| b["idempotency_key"] == "k1")
            .fixture("remember_idem"),
    ]);
    let out = app(&srv)
        .remember(RememberArgs {
            text: Some(s("once")),
            idempotency_key: Some(s("k1")),
            ..RememberArgs::default()
        })
        .await
        .unwrap();
    assert!(out["rid"].is_string());
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn remember_batch_fixture() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember/batch").fixture("remember_batch"),
    ]);
    let memories = vec![
        MemoryInput {
            text: s("Alice leads the payments team"),
            namespace: Some(s("payments")),
            importance: Some(7.0),
            ..MemoryInput::default()
        },
        MemoryInput {
            text: s("Bob maintains the billing dashboard"),
            memory_type: Some(MemoryType::Episodic),
            created_at: Some(s("2026-01-02")),
            ..MemoryInput::default()
        },
    ];
    let out = app(&srv)
        .remember(RememberArgs {
            memories: Some(memories),
            ..RememberArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 4);
    assert_eq!(out["rids"].as_array().unwrap().len(), 4);
    srv.assert_done();
    let body = &srv.requests()[0].body;
    let items = body["memories"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["importance"], 1.0, "clamped inside batch");
    assert_eq!(items[1]["memory_type"], "episodic");
    assert_eq!(items[1]["created_at"].as_f64(), Some(1_767_312_000.0));
}

#[tokio::test(flavor = "current_thread")]
async fn remember_clamps_and_parses_created_at() {
    let srv = FakeServer::start(vec![Step::post("/v1/remember").fixture("remember_minimal")]);
    app(&srv)
        .remember(RememberArgs {
            text: Some(s("x")),
            importance: Some(2.0),
            valence: Some(-5.0),
            certainty: Some(-1.0),
            created_at: Some(s("2026-01-02")),
            ..RememberArgs::default()
        })
        .await
        .unwrap();
    srv.assert_done();
    let body = &srv.requests()[0].body;
    assert_eq!(body["importance"], 1.0);
    assert_eq!(body["valence"], -1.0);
    assert_eq!(body["certainty"], 0.0);
    assert_eq!(body["created_at"].as_f64(), Some(1_767_312_000.0));
}

#[tokio::test(flavor = "current_thread")]
async fn remember_created_at_forms() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember").json(json!({"rid": "a"})),
        Step::post("/v1/remember").json(json!({"rid": "b"})),
    ]);
    let a = app(&srv);
    for t in ["2026-01-02T00:00:00Z", "1767312000"] {
        a.remember(RememberArgs {
            text: Some(s("x")),
            created_at: Some(s(t)),
            ..RememberArgs::default()
        })
        .await
        .unwrap();
    }
    for r in srv.requests() {
        assert_eq!(r.body["created_at"].as_f64(), Some(1_767_312_000.0));
    }
}

// ---------------------------------------------------------------- recall

#[tokio::test(flavor = "current_thread")]
async fn recall_fixture_shaping() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/recall")
            .body(|b| {
                b["query"] == "which database does the payments service use?" && b["top_k"] == 5
            })
            .fixture("recall"),
    ]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("which database does the payments service use?"),
            top_k: Some(5),
            namespace: Some(s("payments")),
            domain: Some(s("work")),
            memory_type: Some(MemoryType::Semantic),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    srv.assert_done();
    let body = &srv.requests()[0].body;
    assert_eq!(body["namespace"], "payments");
    assert_eq!(body["domain"], "work");
    assert_eq!(body["memory_type"], "semantic");
    assert!(body.get("time_window").is_none());
    let hits = out["results"].as_array().unwrap();
    assert!(!hits.is_empty());
    let first = &hits[0];
    assert_eq!(first["score"], 0.5646, "rounded to 4 dp");
    assert_eq!(first["created_at_iso"], "2026-10-30T15:51:36Z");
    assert!(first["created_at"].as_f64().unwrap() > 1.7e9);
    assert!(first["why_retrieved"].is_array());
    assert_eq!(out["fallback"], "fts5_keyword");
    for k in [
        "filtered_by_time_window",
        "filtered_by_min_score_ratio",
        "filtered_by_refine_exclude",
    ] {
        assert!(
            out.get(k).is_none(),
            "{k} must be absent when nothing was filtered"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn recall_empty_prunes_null() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").fixture("recall_empty")]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("zebra migration schedule"),
            namespace: Some(s("nonexistent")),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(out["results"], json!([]));
    assert!(out.get("fallback").is_none(), "null pruned");
    assert_eq!(out["total"], 0);
}

fn hits(v: Value) -> Step {
    Step::post("/v1/recall").json(json!({ "results": v }))
}

fn rids(out: &Value) -> Vec<&str> {
    out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["rid"].as_str().unwrap())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn recall_time_window_filter() {
    let srv = FakeServer::start(vec![hits(json!([
        {"rid": "in", "score": 0.9, "created_at": 1_767_312_000.0},
        {"rid": "old", "score": 0.8, "created_at": 1_700_000_000.0},
        {"rid": "none", "score": 0.7},
        {"rid": "edge", "score": 0.6, "created_at": 1_769_904_000.0},
        {"rid": "future", "score": 0.5, "created_at": 1_800_000_000.0},
    ]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            since: Some(s("2026-01-01T00:00:00Z")),
            until: Some(s("2026-02-01T00:00:00Z")),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["in", "edge"]);
    assert_eq!(out["filtered_by_time_window"], 3);
    let tw = &srv.requests()[0].body["time_window"];
    assert_eq!(tw[0].as_f64(), Some(1_767_225_600.0));
    assert_eq!(tw[1].as_f64(), Some(1_769_904_000.0));
}

#[tokio::test(flavor = "current_thread")]
async fn recall_since_only_span() {
    let srv = FakeServer::start(vec![hits(json!([
        {"rid": "recent", "created_at": chrono_now() - 3600.0},
        {"rid": "old", "created_at": chrono_now() - 3.0 * 86_400.0},
    ]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            since: Some(s("1d")),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["recent"]);
    assert_eq!(out["filtered_by_time_window"], 1);
}

#[allow(clippy::cast_precision_loss)]
fn chrono_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as f64
}

#[tokio::test(flavor = "current_thread")]
async fn recall_min_score_ratio_filter() {
    let srv = FakeServer::start(vec![hits(json!([
        {"rid": "top", "score": 1.0},
        {"rid": "mid", "score": 0.6},
        {"rid": "low", "score": 0.4},
        {"rid": "noscore"},
    ]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            min_score_ratio: Some(0.5),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["top", "mid"]);
    assert_eq!(out["filtered_by_min_score_ratio"], 2);
}

// top < 1 tells `ratio * top` from `ratio / top`; 0.4 is exactly 0.5 * 0.8 and must be kept.
#[tokio::test(flavor = "current_thread")]
async fn recall_min_score_ratio_scales_with_top_and_keeps_the_boundary() {
    let srv = FakeServer::start(vec![hits(json!([
        {"rid": "top", "score": 0.8},
        {"rid": "mid", "score": 0.5},
        {"rid": "edge", "score": 0.4},
        {"rid": "low", "score": 0.3},
    ]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            min_score_ratio: Some(0.5),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["top", "mid", "edge"]);
    assert_eq!(out["filtered_by_min_score_ratio"], 1);
}

#[tokio::test(flavor = "current_thread")]
async fn recall_filter_that_removes_nothing_reports_no_count() {
    let srv = FakeServer::start(vec![hits(json!([{"rid": "a", "score": 0.9}]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            refine_exclude: Some(vec![s("zzz")]),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["a"]);
    assert!(out.get("filtered_by_refine_exclude").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn recall_min_score_ratio_without_any_score_keeps_all() {
    let srv = FakeServer::start(vec![hits(json!([{"rid": "a"}, {"rid": "b"}]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            min_score_ratio: Some(1.0),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["a", "b"]);
    assert!(out.get("filtered_by_min_score_ratio").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn recall_refine_exclude_filter() {
    let srv = FakeServer::start(vec![hits(json!([
        {"rid": "a", "score": 0.9},
        {"rid": "b", "score": 0.8},
        {"rid": "c", "score": 0.7},
    ]))]);
    let out = app(&srv)
        .recall(RecallArgs {
            query: s("q"),
            refine_exclude: Some(vec![s("a"), s("c"), s("zzz")]),
            ..RecallArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(rids(&out), ["b"]);
    assert_eq!(out["filtered_by_refine_exclude"], 2);
}

#[tokio::test(flavor = "current_thread")]
async fn recall_order_variants() {
    let data = json!([
        {"rid": "a", "score": 0.9, "created_at": 200.0, "certainty": 0.5},
        {"rid": "b", "score": 0.8, "created_at": 300.0, "certainty": 0.9},
        {"rid": "c", "score": 0.7, "created_at": 100.0, "certainty": 0.7},
        {"rid": "d", "score": 0.6},
    ]);
    let cases = [
        (None, vec!["a", "b", "c", "d"]),
        (Some(Order::Relevance), vec!["a", "b", "c", "d"]),
        (Some(Order::Recency), vec!["b", "a", "c", "d"]),
        (Some(Order::FirstMention), vec!["d", "c", "a", "b"]),
        (Some(Order::Certainty), vec!["b", "c", "a", "d"]),
    ];
    let srv = FakeServer::start(cases.iter().map(|_| hits(data.clone())).collect());
    let a = app(&srv);
    for (order, expected) in cases {
        let out = a
            .recall(RecallArgs {
                query: s("q"),
                order,
                ..RecallArgs::default()
            })
            .await
            .unwrap();
        assert_eq!(rids(&out), expected, "{order:?}");
    }
    srv.assert_done();
}

// ---------------------------------------------------------------- simple ops

#[tokio::test(flavor = "current_thread")]
async fn forget_fixture() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/forget")
            .body(|b| b["rid"] == "r1" && b["reason"] == "dup")
            .fixture("forget"),
    ]);
    let out = app(&srv)
        .forget(ForgetArgs {
            force: false,
            rid: s("r1"),
            reason: Some(s("dup")),
        })
        .await
        .unwrap();
    assert_eq!(out["found"], true);
    assert_eq!(out["rid"], "01a18d82-6965-7f1f-99b3-1e170db6afe8");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn think_fixture_rounds_floats() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/think")
            .body(|b| b == &json!({}))
            .fixture("think"),
    ]);
    let out = app(&srv).think(ThinkArgs {}).await.unwrap();
    srv.assert_done();
    assert_eq!(out["conflicts_found"], 1);
    assert_eq!(out["triggers"][0]["urgency"], 1.0);
    assert_eq!(out["triggers"][1]["urgency"], 0.7603);
}

#[tokio::test(flavor = "current_thread")]
async fn graph_relate_wire_fields() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/relate")
            .body(|b| {
                b["entity"] == "Alice"
                    && b["target"] == "Payments"
                    && b["relationship"] == "leads"
                    && b["weight"] == 1.0
                    && b["namespace"] == "payments"
                    && b.get("src").is_none()
                    && b.get("dst").is_none()
                    && b.get("rel_type").is_none()
            })
            .fixture("relate"),
    ]);
    let out = app(&srv)
        .graph_relate(GraphRelateArgs {
            force: false,
            src: s("Alice"),
            dst: s("Payments"),
            rel_type: s("leads"),
            weight: Some(9.0),
            namespace: Some(s("payments")),
        })
        .await
        .unwrap();
    assert!(out["edge_id"].is_string());
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn graph_edges_404_is_unsupported() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/graph")
            .body(|b| b["entity"] == "Alice" && b["action"] == "query")
            .fixture("graph_edges"),
    ]);
    let err = app(&srv)
        .graph_edges(GraphEdgesArgs { entity: s("Alice") })
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Unsupported { operation, .. } if operation == "graph_edges"),
        "{err:?}"
    );
    assert_eq!(err.exit_code(), 4);
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn graph_edges_success_passthrough() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/graph").json(json!({"edges": [{"to": "x", "w": 0.123_456}]})),
    ]);
    let out = app(&srv)
        .graph_edges(GraphEdgesArgs { entity: s("Alice") })
        .await
        .unwrap();
    assert_eq!(out["edges"][0]["w"], 0.1235);
}

#[tokio::test(flavor = "current_thread")]
async fn conflict_list_query_string_and_fixture() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/conflicts?entity=Alice&limit=5&namespace=payments&status=open")
            .fixture("conflicts"),
        Step::get("/v1/conflicts").fixture("conflicts"),
    ]);
    let a = app(&srv);
    let out = a
        .conflict_list(ConflictListArgs {
            status: Some(s("open")),
            entity: Some(s("Alice")),
            namespace: Some(s("payments")),
            limit: Some(5),
        })
        .await
        .unwrap();
    let c = &out["conflicts"][0];
    assert_eq!(c["priority"], "high");
    assert!(c.get("entity").is_none(), "null entity pruned");
    assert_eq!(c["detected_at_iso"], "2026-10-30T15:51:43Z");
    a.conflict_list(ConflictListArgs::default()).await.unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn conflict_resolve_fixture() {
    let id = "01a18d82-83dc-7e2d-b7c2-f390a01a2994";
    let srv = FakeServer::start(vec![
        Step::post(&format!("/v1/conflicts/{id}/resolve"))
            .body(|b| b["strategy"] == "keep_b" && b["resolution_note"] == "PostgreSQL confirmed")
            .fixture("conflict_resolve"),
        Step::post("/v1/conflicts/c2/resolve")
            .body(|b| b["strategy"] == "merge" && b["new_text"] == "merged")
            .json(json!({"conflict_id": "c2", "strategy": "merge"})),
    ]);
    let a = app(&srv);
    let out = a
        .conflict_resolve(ConflictResolveArgs {
            force: false,
            id: s(id),
            strategy: Strategy::KeepB,
            new_text: None,
            note: Some(s("PostgreSQL confirmed")),
        })
        .await
        .unwrap();
    assert_eq!(out["strategy"], "keep_b");
    a.conflict_resolve(ConflictResolveArgs {
        force: false,
        id: s("c2"),
        strategy: Strategy::Merge,
        new_text: Some(s("merged")),
        note: None,
    })
    .await
    .unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn conflict_dismiss_uses_dismiss_strategy() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/conflicts/c1/resolve")
            .body(|b| b["strategy"] == "dismiss" && b["resolution_note"] == "noise")
            .json(json!({"conflict_id": "c1", "strategy": "dismiss"})),
    ]);
    let out = app(&srv)
        .conflict_dismiss(ConflictDismissArgs {
            force: false,
            id: s("c1"),
            note: Some(s("noise")),
        })
        .await
        .unwrap();
    assert_eq!(out["strategy"], "dismiss");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn session_digest_include_gaps() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/session/digest?include_gaps=true&namespace=payments")
            .fixture("session_digest"),
        Step::get("/v1/session/digest").fixture("session_digest_default"),
    ]);
    let a = app(&srv);
    let out = a
        .session_digest(SessionDigestArgs {
            namespace: Some(s("payments")),
            include_gaps: true,
        })
        .await
        .unwrap();
    assert!(out["knowledge_gaps"].is_array());
    assert!(out.get("narrative_head").is_none(), "null pruned");
    assert_eq!(out["pending_triggers"][1]["urgency"], 0.7603);
    a.session_digest(SessionDigestArgs::default())
        .await
        .unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn session_digest_decodes_stringified_maintenance() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/session/digest").json(json!({
            "last_maintenance": "{\"ran_at\":1793378517.90742,\"think_conflicts_found\":0}"
        })),
        Step::get("/v1/session/digest").json(json!({ "last_maintenance": "not json" })),
    ]);
    let a = app(&srv);
    let out = a
        .session_digest(SessionDigestArgs::default())
        .await
        .unwrap();
    assert_eq!(out["last_maintenance"]["think_conflicts_found"], 0);
    assert_eq!(
        out["last_maintenance"]["ran_at_iso"],
        "2026-10-30T16:41:57Z"
    );
    let out = a
        .session_digest(SessionDigestArgs::default())
        .await
        .unwrap();
    assert_eq!(out["last_maintenance"], "not json");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn session_capture_fixture() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/session/end")
            .body(|b| {
                b["summary"] == "We reviewed."
                    && b["namespace"] == "payments"
                    && b["domain"] == "work"
            })
            .fixture("session_end"),
    ]);
    let out = app(&srv)
        .session_capture(SessionCaptureArgs {
            force: false,
            summary: s("We reviewed."),
            namespace: Some(s("payments")),
            domain: Some(s("work")),
            stdin: false,
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 2);
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn session_start_fixture() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/sessions")
            .body(|b| b["namespace"] == "payments" && b["client_id"] == "fixture-client")
            .fixture("session_start"),
    ]);
    let out = app(&srv)
        .session_start(SessionStartArgs {
            namespace: Some(s("payments")),
            client_id: Some(s("fixture-client")),
            ..SessionStartArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(out["session_id"], "01a18d83-61a3-7c63-9b60-158c6e776718");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn session_end_is_delete() {
    let srv = FakeServer::start(vec![
        Step::delete("/v1/sessions/01a18d83-61a3-7c63-9b60-158c6e776718")
            .body(|b| b["summary"] == "Reviewed payments architecture")
            .fixture("session_delete"),
    ]);
    let out = app(&srv)
        .session_end(SessionEndArgs {
            force: false,
            session_id: s("01a18d83-61a3-7c63-9b60-158c6e776718"),
            summary: Some(s("Reviewed payments architecture")),
        })
        .await
        .unwrap();
    assert_eq!(out["memory_count"], 2);
    assert_eq!(out["duration_secs"], 7.8134);
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn memory_get_renames_type_and_metadata() {
    let srv = FakeServer::start(vec![
        Step::get(&format!("/v1/memory/{OLD_RID}")).fixture("memory_get"),
    ]);
    let out = app(&srv)
        .memory_get(MemoryGetArgs { rid: s(OLD_RID) })
        .await
        .unwrap();
    srv.assert_done();
    assert_eq!(out["memory_type"], "semantic");
    assert_eq!(out["metadata"], json!({"kind": "fact", "team": "payments"}));
    assert!(out.get("type").is_none());
    assert!(out.get("metadata_json").is_none());
    assert!(out.get("updated_at").is_none(), "null pruned");
    assert_eq!(out["text"], "Payments service uses PostgreSQL");
    assert_eq!(out["created_at_iso"], "2026-10-30T15:51:36Z");
    assert!(out["last_access_iso"].is_string());
}

#[tokio::test(flavor = "current_thread")]
async fn memory_get_404() {
    let srv = FakeServer::start(vec![Step::get("/v1/memory/nope").fixture("memory_get_404")]);
    let err = app(&srv)
        .memory_get(MemoryGetArgs { rid: s("nope") })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn memory_current_fixture() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/current?namespace=payments").fixture("current"),
    ]);
    let out = app(&srv)
        .memory_current(MemoryCurrentArgs {
            namespace: s("payments"),
        })
        .await
        .unwrap();
    assert_eq!(out["found"], true);
    assert_eq!(
        out["record"]["text"],
        "Carol is the on-call engineer this week"
    );
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn memory_list_desc_and_legacy() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/memories?domain=work&limit=2&order=desc").fixture("memories_desc"),
        Step::get("/v1/memories?limit=3&namespace=payments").fixture("memories"),
        Step::get("/v1/memories?memory_type=episodic&offset=4").json(json!({"items": []})),
    ]);
    let a = app(&srv);
    let out = a
        .memory_list(MemoryListArgs {
            domain: Some(s("work")),
            limit: Some(2),
            desc: true,
            ..MemoryListArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(out["records"].as_array().unwrap().len(), 2);
    assert_eq!(out["records"][0]["created_at_iso"], "2026-10-30T15:51:36Z");
    let out = a
        .memory_list(MemoryListArgs {
            namespace: Some(s("payments")),
            limit: Some(3),
            ..MemoryListArgs::default()
        })
        .await
        .unwrap();
    assert!(out["items"].is_array());
    a.memory_list(MemoryListArgs {
        memory_type: Some(MemoryType::Episodic),
        offset: Some(4),
        ..MemoryListArgs::default()
    })
    .await
    .unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn memory_gaps_fixture() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/insights/gaps?limit=3&namespace=payments").fixture("gaps"),
    ]);
    let out = app(&srv)
        .memory_gaps(MemoryGapsArgs {
            namespace: Some(s("payments")),
            limit: Some(3),
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 1);
    assert_eq!(out["gaps"][0]["avg_top_score"], 0.1329);
    assert_eq!(out["gaps"][0]["last_seen_iso"], "2026-10-30T15:52:47Z");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn memory_stats_fixture() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").fixture("stats_after")]);
    let out = app(&srv).memory_stats(MemoryStatsArgs {}).await.unwrap();
    assert!(out["active_memories"].is_number());
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn output_shaping_rounds_prunes_and_adds_iso() {
    let srv = FakeServer::start(vec![Step::get("/v1/stats").json(json!({
        "a": null,
        "x": 0.123_456_789,
        "i": 7,
        "big": 1.0e-9,
        "detected_at": 1_793_375_503.324_312_4,
        "last_access": 1_793_375_496.0,
        "nested": [{"n": null, "f": 1.000_04, "keep": "s", "list": [null, 0.55555]}],
    }))]);
    let out = app(&srv).memory_stats(MemoryStatsArgs {}).await.unwrap();
    assert!(out.get("a").is_none());
    assert_eq!(out["x"], 0.1235);
    assert_eq!(out["i"], 7);
    assert_eq!(out["big"], 0.0);
    assert_eq!(
        out["detected_at"], 1_793_375_503.324_312_4,
        "timestamps keep full precision"
    );
    assert_eq!(out["detected_at_iso"], "2026-10-30T15:51:43Z");
    assert_eq!(out["last_access_iso"], "2026-10-30T15:51:36Z");
    assert!(out["nested"][0].get("n").is_none());
    assert_eq!(out["nested"][0]["f"], 1.0);
    assert_eq!(out["nested"][0]["keep"], "s");
    assert_eq!(out["nested"][0]["list"], json!([null, 0.5556]));
}

// ---------------------------------------------------------------- call()

#[tokio::test(flavor = "current_thread")]
async fn call_every_tool_by_name() {
    let cases: Vec<(&str, Value, Step)> = vec![
        (
            "remember",
            json!({"text": "x"}),
            Step::post("/v1/remember").fixture("remember"),
        ),
        (
            "recall",
            json!({"query": "q"}),
            Step::post("/v1/recall").fixture("recall_empty"),
        ),
        (
            "forget",
            json!({"rid": "r"}),
            Step::post("/v1/forget").fixture("forget"),
        ),
        (
            "correct",
            json!({"rid": "r", "reason": "why", "new_text": "t"}),
            Step::post("/v1/correct").json(json!({"rid": "r2"})),
        ),
        (
            "think",
            Value::Null,
            Step::post("/v1/think").fixture("think"),
        ),
        (
            "graph_relate",
            json!({"src": "a", "dst": "b", "rel_type": "uses"}),
            Step::post("/v1/relate").fixture("relate"),
        ),
        (
            "conflict_list",
            json!({}),
            Step::get("/v1/conflicts").fixture("conflicts"),
        ),
        (
            "conflict_resolve",
            json!({"id": "c1", "strategy": "keep_a"}),
            Step::post("/v1/conflicts/c1/resolve").fixture("conflict_resolve"),
        ),
        (
            "session_digest",
            json!({}),
            Step::get("/v1/session/digest").fixture("session_digest_default"),
        ),
        (
            "session_capture",
            json!({"summary": "s"}),
            Step::post("/v1/session/end").fixture("session_end"),
        ),
        (
            "memory_get",
            json!({"rid": "r"}),
            Step::get("/v1/memory/r").fixture("memory_get"),
        ),
        (
            "memory_current",
            json!({"namespace": "payments"}),
            Step::get("/v1/current?namespace=payments").fixture("current"),
        ),
        (
            "graph_edges",
            json!({"entity": "Alice"}),
            Step::post("/v1/graph").fixture("graph_edges"),
        ),
    ];
    for t in TOOLS {
        assert!(cases.iter().any(|(n, ..)| *n == t), "no case for {t}");
    }
    let srv = FakeServer::start(cases.iter().map(|(.., st)| st.clone()).collect());
    let a = app(&srv);
    for (name, args, _) in cases {
        let r = a.call(name, args).await;
        if name == "graph_edges" {
            assert!(matches!(r, Err(Error::Unsupported { .. })), "{r:?}");
        } else {
            assert!(r.is_ok(), "{name}: {r:?}");
        }
    }
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn call_unknown_tool_and_bad_args_are_invalid() {
    let srv = FakeServer::start(vec![]);
    let a = app(&srv);
    let e = a.call("nope", json!({})).await.unwrap_err();
    assert!(
        matches!(&e, Error::Invalid(m) if m.contains("nope") && m.contains("remember")),
        "{e:?}"
    );
    let e = a.call("memory_stats", json!({})).await.unwrap_err();
    assert!(
        matches!(e, Error::Invalid(_)),
        "memory_stats is not an MCP tool"
    );
    for (tool, args) in [
        ("remember", json!({"text": 5})),
        ("remember", json!({"text": "x", "bogus": 1})),
        ("recall", json!("not an object")),
        ("recall", json!({"query": "q", "order": "sideways"})),
        ("forget", json!({})),
        ("correct", json!({"rid": "r"})),
        ("conflict_resolve", json!({"id": "c", "strategy": "nope"})),
        ("memory_current", Value::Null),
    ] {
        let e = a.call(tool, args.clone()).await.unwrap_err();
        assert!(
            matches!(&e, Error::Invalid(m) if m.starts_with(tool)),
            "{tool} {args}: {e:?}"
        );
        assert_eq!(e.exit_code(), 2);
    }
    assert!(srv.calls().is_empty());
}

// ---------------------------------------------------------------- validation

fn invalid(what: &str, r: yrem::Result<Value>) {
    match r {
        Err(e @ Error::Invalid(_)) => assert_eq!(e.exit_code(), 2, "{what}"),
        other => panic!("{what}: expected Invalid, got {other:?}"),
    }
}

fn mem(text: &str) -> MemoryInput {
    MemoryInput {
        text: s(text),
        ..MemoryInput::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn validation_errors_send_nothing() {
    let srv = FakeServer::start(vec![]);
    let a = app(&srv);
    let rem = |text: Option<&str>| RememberArgs {
        text: text.map(s),
        ..RememberArgs::default()
    };

    invalid("remember empty text", a.remember(rem(Some(""))).await);
    invalid("remember blank text", a.remember(rem(Some("  \n\t"))).await);
    invalid("remember neither", a.remember(rem(None)).await);
    invalid(
        "remember text+memories",
        a.remember(RememberArgs {
            memories: Some(vec![mem("a")]),
            ..rem(Some("x"))
        })
        .await,
    );
    invalid(
        "remember empty memories",
        a.remember(RememberArgs {
            memories: Some(vec![]),
            ..rem(None)
        })
        .await,
    );
    invalid(
        "remember batch idempotency",
        a.remember(RememberArgs {
            memories: Some(vec![mem("a")]),
            idempotency_key: Some(s("k")),
            ..rem(None)
        })
        .await,
    );
    invalid(
        "remember batch empty text",
        a.remember(RememberArgs {
            memories: Some(vec![mem("a"), mem(" ")]),
            ..rem(None)
        })
        .await,
    );
    invalid(
        "remember batch bad created_at",
        a.remember(RememberArgs {
            memories: Some(vec![MemoryInput {
                created_at: Some(s("yesterday-ish")),
                ..mem("a")
            }]),
            ..rem(None)
        })
        .await,
    );
    invalid(
        "remember batch NaN",
        a.remember(RememberArgs {
            memories: Some(vec![MemoryInput {
                valence: Some(f64::NAN),
                ..mem("a")
            }]),
            ..rem(None)
        })
        .await,
    );
    for (name, args) in [
        (
            "importance NaN",
            RememberArgs {
                importance: Some(f64::NAN),
                ..rem(Some("x"))
            },
        ),
        (
            "importance inf",
            RememberArgs {
                importance: Some(f64::INFINITY),
                ..rem(Some("x"))
            },
        ),
        (
            "valence -inf",
            RememberArgs {
                valence: Some(f64::NEG_INFINITY),
                ..rem(Some("x"))
            },
        ),
        (
            "certainty NaN",
            RememberArgs {
                certainty: Some(f64::NAN),
                ..rem(Some("x"))
            },
        ),
        (
            "bad created_at",
            RememberArgs {
                created_at: Some(s("soon")),
                ..rem(Some("x"))
            },
        ),
    ] {
        invalid(name, a.remember(args).await);
    }

    let rec = |q: &str| RecallArgs {
        query: s(q),
        ..RecallArgs::default()
    };
    invalid("recall empty query", a.recall(rec("")).await);
    invalid("recall blank query", a.recall(rec("   ")).await);
    invalid(
        "top_k 0",
        a.recall(RecallArgs {
            top_k: Some(0),
            ..rec("q")
        })
        .await,
    );
    for r in [0.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
        invalid(
            "min_score_ratio",
            a.recall(RecallArgs {
                min_score_ratio: Some(r),
                ..rec("q")
            })
            .await,
        );
    }
    invalid(
        "since > until",
        a.recall(RecallArgs {
            since: Some(s("2026-02-01")),
            until: Some(s("2026-01-01")),
            ..rec("q")
        })
        .await,
    );
    invalid(
        "bad since",
        a.recall(RecallArgs {
            since: Some(s("banana")),
            ..rec("q")
        })
        .await,
    );
    invalid(
        "bad until",
        a.recall(RecallArgs {
            until: Some(s("13/13/13")),
            ..rec("q")
        })
        .await,
    );

    invalid(
        "forget empty rid",
        a.forget(ForgetArgs {
            force: false,
            rid: s(" "),
            reason: None,
        })
        .await,
    );

    let cor = || CorrectArgs {
        rid: s("r"),
        reason: s("why"),
        ..CorrectArgs::default()
    };
    invalid(
        "correct empty rid",
        a.correct(CorrectArgs {
            rid: s(""),
            ..cor()
        })
        .await,
    );
    invalid(
        "correct empty reason",
        a.correct(CorrectArgs {
            reason: s(" "),
            ..cor()
        })
        .await,
    );
    invalid(
        "correct blank new_text",
        a.correct(CorrectArgs {
            new_text: Some(s("  ")),
            ..cor()
        })
        .await,
    );
    invalid(
        "correct importance NaN",
        a.correct(CorrectArgs {
            new_importance: Some(f64::NAN),
            ..cor()
        })
        .await,
    );
    invalid(
        "correct valence inf",
        a.correct(CorrectArgs {
            new_valence: Some(f64::INFINITY),
            ..cor()
        })
        .await,
    );

    let rel = || GraphRelateArgs {
        src: s("a"),
        dst: s("b"),
        rel_type: s("uses"),
        ..GraphRelateArgs::default()
    };
    invalid(
        "relate src",
        a.graph_relate(GraphRelateArgs {
            src: s(""),
            ..rel()
        })
        .await,
    );
    invalid(
        "relate dst",
        a.graph_relate(GraphRelateArgs {
            dst: s(""),
            ..rel()
        })
        .await,
    );
    invalid(
        "relate rel_type",
        a.graph_relate(GraphRelateArgs {
            rel_type: s(" "),
            ..rel()
        })
        .await,
    );
    invalid(
        "relate weight",
        a.graph_relate(GraphRelateArgs {
            weight: Some(f64::NAN),
            ..rel()
        })
        .await,
    );
    invalid(
        "edges entity",
        a.graph_edges(GraphEdgesArgs { entity: s("") }).await,
    );

    let res = |id: &str, strategy, new_text: Option<&str>| ConflictResolveArgs {
        force: false,
        id: s(id),
        strategy,
        new_text: new_text.map(s),
        note: None,
    };
    invalid(
        "resolve empty id",
        a.conflict_resolve(res("", Strategy::KeepA, None)).await,
    );
    invalid(
        "merge without new_text",
        a.conflict_resolve(res("c", Strategy::Merge, None)).await,
    );
    invalid(
        "merge blank new_text",
        a.conflict_resolve(res("c", Strategy::Merge, Some(" ")))
            .await,
    );
    invalid(
        "dismiss empty id",
        a.conflict_dismiss(ConflictDismissArgs {
            force: false,
            id: s(""),
            note: None,
        })
        .await,
    );

    invalid(
        "capture empty summary",
        a.session_capture(SessionCaptureArgs::default()).await,
    );
    invalid(
        "capture blank summary",
        a.session_capture(SessionCaptureArgs {
            summary: s("  "),
            ..SessionCaptureArgs::default()
        })
        .await,
    );
    invalid(
        "session_end empty id",
        a.session_end(SessionEndArgs::default()).await,
    );
    invalid(
        "memory_get empty rid",
        a.memory_get(MemoryGetArgs { rid: s("") }).await,
    );
    invalid(
        "memory_current empty ns",
        a.memory_current(MemoryCurrentArgs { namespace: s(" ") })
            .await,
    );

    assert!(
        srv.calls().is_empty(),
        "requests were sent: {:?}",
        srv.calls()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn clamping_other_ops() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"rid": "n"})),
        Step::post("/v1/relate").json(json!({"edge_id": "e"})),
    ]);
    let a = app(&srv);
    a.correct(CorrectArgs {
        rid: s("r"),
        reason: s("why"),
        new_importance: Some(3.0),
        new_valence: Some(-9.0),
        ..CorrectArgs::default()
    })
    .await
    .unwrap();
    a.graph_relate(GraphRelateArgs {
        force: false,
        src: s("a"),
        dst: s("b"),
        rel_type: s("t"),
        weight: Some(-3.0),
        namespace: None,
    })
    .await
    .unwrap();
    let reqs = srv.requests();
    assert_eq!(reqs[0].body["new_importance"], 1.0);
    assert_eq!(reqs[0].body["new_valence"], -1.0);
    assert_eq!(reqs[1].body["weight"], 0.0);
}

// ---------------------------------------------------------------- correct

fn correct_args() -> CorrectArgs {
    CorrectArgs {
        rid: s(OLD_RID),
        reason: s("database upgraded"),
        ..CorrectArgs::default()
    }
}

fn memory_path() -> String {
    format!("/v1/memory/{OLD_RID}")
}

#[tokio::test(flavor = "current_thread")]
async fn correct_native_success() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct")
            .body(|b| {
                b["rid"] == OLD_RID
                    && b["reason"] == "database upgraded"
                    && b["new_text"] == "PG 16"
                    && b["metadata_merge"] == json!({"k": 1})
            })
            .json(json!({"new_rid": "n1", "score": 0.123_456})),
    ]);
    let out = app(&srv)
        .correct(CorrectArgs {
            new_text: Some(s("PG 16")),
            metadata_merge: Some(obj(json!({"k": 1}))),
            ..correct_args()
        })
        .await
        .unwrap();
    assert_eq!(out["emulated"], false);
    assert_eq!(out["new_rid"], "n1");
    assert_eq!(out["score"], 0.1235);
    assert_eq!(srv.calls(), ["POST /v1/correct"]);
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn correct_native_other_error_is_not_emulated() {
    let srv = FakeServer::start(vec![Step::post("/v1/correct").status(500).raw("boom")]);
    let err = app(&srv).correct(correct_args()).await.unwrap_err();
    assert!(matches!(err, Error::Http { status: 500, .. }), "{err:?}");
    assert_eq!(srv.calls().len(), 1);
}

fn emulation_steps(remember: Step, forget: Step) -> Vec<Step> {
    vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).fixture("memory_get"),
        remember,
        forget,
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_success_with_overrides() {
    let srv = FakeServer::start(emulation_steps(
        Step::post("/v1/remember").json(json!({"rid": NEW_RID, "log_index": 9})),
        Step::post("/v1/forget").json(json!({"rid": OLD_RID, "found": true})),
    ));
    let out = app(&srv)
        .correct(CorrectArgs {
            new_text: Some(s("Payments service uses PostgreSQL 16")),
            new_importance: Some(5.0),
            new_valence: Some(-0.25),
            metadata_merge: Some(obj(json!({"team": "core", "extra": 1}))),
            ..correct_args()
        })
        .await
        .unwrap();
    assert_eq!(
        out,
        json!({"old_rid": OLD_RID, "new_rid": NEW_RID, "emulated": true})
    );
    srv.assert_done();
    assert_eq!(
        srv.calls(),
        [
            "POST /v1/correct".to_owned(),
            format!("GET {}", memory_path()),
            "POST /v1/remember".to_owned(),
            "POST /v1/forget".to_owned(),
        ]
    );
    let reqs = srv.requests();
    let new = &reqs[2].body;
    assert_eq!(new["text"], "Payments service uses PostgreSQL 16");
    assert_eq!(new["memory_type"], "semantic");
    assert_eq!(new["domain"], "work");
    assert_eq!(new["source"], "user");
    assert_eq!(new["namespace"], "payments");
    assert_eq!(new["certainty"], 0.9);
    assert_eq!(new["emotional_state"], "neutral");
    assert_eq!(new["importance"], 1.0, "override clamped");
    assert_eq!(new["valence"], -0.25);
    let md = &new["metadata"];
    assert_eq!(md["kind"], "fact", "old metadata kept");
    assert_eq!(md["team"], "core", "merge wins over old");
    assert_eq!(md["extra"], 1);
    assert_eq!(md["corrects"], OLD_RID);
    assert_eq!(md["correct_reason"], "database upgraded");
    let at = md["corrected_at"].as_str().unwrap();
    assert!(
        at.ends_with('Z') && at.len() == 20 && at.starts_with("20"),
        "{at}"
    );
    assert!(new.get("created_at").is_none());
    assert!(new.get("idempotency_key").is_none());
    let forget = &reqs[3].body;
    assert_eq!(forget["rid"], OLD_RID);
    assert!(forget["reason"].as_str().unwrap().contains(NEW_RID));
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_keeps_old_text_and_values() {
    let srv = FakeServer::start(emulation_steps(
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").json(json!({"found": true})),
    ));
    let out = app(&srv).correct(correct_args()).await.unwrap();
    assert_eq!(out["emulated"], true);
    let new = &srv.requests()[2].body;
    assert_eq!(new["text"], "Payments service uses PostgreSQL");
    assert_eq!(new["importance"], 0.8);
    assert_eq!(new["valence"], 0.1);
    assert_eq!(new["metadata"]["team"], "payments");
    assert_eq!(new["metadata"]["corrects"], OLD_RID);
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_parses_string_metadata() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).json(json!({
            "rid": OLD_RID, "text": "old", "type": "episodic",
            "metadata_json": "{\"a\": 1, \"b\": \"two\"}",
            "domain": "", "source": "", "namespace": "",
        })),
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").json(json!({"found": true})),
    ]);
    app(&srv).correct(correct_args()).await.unwrap();
    let new = &srv.requests()[2].body;
    assert_eq!(new["metadata"]["a"], 1);
    assert_eq!(new["metadata"]["b"], "two");
    assert_eq!(new["metadata"]["corrects"], OLD_RID);
    assert_eq!(new["memory_type"], "episodic");
    for k in [
        "domain",
        "source",
        "namespace",
        "importance",
        "valence",
        "certainty",
    ] {
        assert!(new.get(k).is_none(), "{k} should be omitted: {new}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_unparseable_or_missing_metadata() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path())
            .json(json!({"rid": OLD_RID, "text": "old", "metadata_json": "not json"})),
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").json(json!({"found": true})),
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).json(json!({"rid": OLD_RID, "text": "old"})),
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").json(json!({"found": true})),
    ]);
    let a = app(&srv);
    a.correct(correct_args()).await.unwrap();
    a.correct(correct_args()).await.unwrap();
    for i in [2, 6] {
        let md = srv.requests()[i].body["metadata"]
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(md.len(), 3, "{md:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_get_404_sends_no_writes() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).fixture("memory_get_404"),
    ]);
    let err = app(&srv).correct(correct_args()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert_eq!(
        srv.calls(),
        [
            "POST /v1/correct".to_owned(),
            format!("GET {}", memory_path())
        ]
    );
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_remember_failure_sends_no_forget() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).fixture("memory_get"),
        Step::post("/v1/remember").status(500).raw("embedder down"),
    ]);
    let err = app(&srv).correct(correct_args()).await.unwrap_err();
    assert!(matches!(err, Error::Http { status: 500, .. }), "{err:?}");
    assert_eq!(srv.calls().len(), 3);
    assert!(!srv.calls().iter().any(|c| c.contains("forget")));
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_forget_failure_is_partial() {
    let srv = FakeServer::start(emulation_steps(
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").status(500).raw("disk full"),
    ));
    let err = app(&srv).correct(correct_args()).await.unwrap_err();
    assert_eq!(err.exit_code(), 6);
    match &err {
        Error::PartialCorrect {
            old_rid,
            new_rid,
            source,
        } => {
            assert_eq!(old_rid, OLD_RID);
            assert_eq!(new_rid, NEW_RID);
            assert!(matches!(**source, Error::Http { status: 500, .. }));
        }
        other => panic!("{other:?}"),
    }
    let text = err.to_string();
    assert!(text.contains(OLD_RID) && text.contains(NEW_RID), "{text}");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn correct_emulated_remember_without_rid_is_decode_error() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).fixture("memory_get"),
        Step::post("/v1/remember").json(json!({"log_index": 1})),
    ]);
    let err = app(&srv).correct(correct_args()).await.unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
    assert_eq!(
        srv.calls().len(),
        3,
        "no forget after an unreadable remember"
    );
}

// ---------------------------------------------------------------- v0.2 endpoints

use yrem::ops::{
    ClaimAddArgs, ClaimListArgs, ProcedureSurfaceArgs, TaskListArgs, TemporalAsOfArgs,
    TemporalListArgs, TriggerActionArgs, TriggerHistoryArgs, TriggerListArgs,
};

/// Path of a captured fixture, e.g. `/v1/triggers/<id>/act`.
fn fixture_path(name: &str) -> String {
    support::fake_server::fixture(name)["path"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn trigger_id(name: &str) -> String {
    fixture_path(name).split('/').nth(3).unwrap().to_owned()
}

fn assert_invalid(err: &Error, needle: &str) {
    assert!(
        matches!(err, Error::Invalid(m) if m.contains(needle)),
        "{err:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn temporal_stale_query_and_shaping() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/temporal/stale?days=0&limit=2&namespace=ops")
            .fixture("temporal_stale_params"),
        Step::get("/v1/temporal/stale").fixture("temporal_stale"),
    ]);
    let a = app(&srv);
    let out = a
        .temporal_stale(TemporalListArgs {
            days: Some(0),
            limit: Some(2),
            namespace: Some(s("ops")),
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 2);
    assert_eq!(out["memories"][0]["created_at_iso"], "2026-08-01T17:14:48Z");
    assert!(out["memories"][0].get("due_at").is_none(), "null pruned");
    a.temporal_stale(TemporalListArgs::default()).await.unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn temporal_upcoming_query() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/temporal/upcoming?days=30&namespace=ops").fixture("temporal_upcoming"),
    ]);
    let out = app(&srv)
        .temporal_upcoming(TemporalListArgs {
            days: Some(30),
            limit: None,
            namespace: Some(s("ops")),
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 0);
    srv.assert_done();
}

fn as_of_args(at: &str) -> TemporalAsOfArgs {
    TemporalAsOfArgs {
        query: s("ledger migration"),
        at: s(at),
        ..TemporalAsOfArgs::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn temporal_as_of_sends_unix_seconds() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/temporal/as_of").fixture("temporal_as_of"),
        Step::post("/v1/temporal/as_of").fixture("temporal_as_of_empty"),
        Step::post("/v1/temporal/as_of").fixture("temporal_as_of_empty"),
    ]);
    let a = app(&srv);
    let out = a
        .temporal_as_of(TemporalAsOfArgs {
            top_k: Some(2),
            namespace: Some(s("ops")),
            memory_type: Some(MemoryType::Episodic),
            ..as_of_args("4000000000")
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 2);
    a.temporal_as_of(as_of_args("2026-01-02")).await.unwrap();
    a.temporal_as_of(as_of_args("2026-01-02T01:00:00+01:00"))
        .await
        .unwrap();
    srv.assert_done();
    let reqs = srv.requests();
    let b = &reqs[0].body;
    assert_eq!(b["as_of"], 4_000_000_000.0);
    assert_eq!(b["query"], "ledger migration");
    assert_eq!(b["top_k"], 2);
    assert_eq!(b["namespace"], "ops");
    assert_eq!(b["memory_type"], "episodic");
    assert_eq!(reqs[1].body["as_of"], 1_767_312_000.0);
    assert!(reqs[1].body.get("top_k").is_none());
    assert_eq!(reqs[2].body["as_of"], 1_767_312_000.0);
}

#[tokio::test(flavor = "current_thread")]
async fn temporal_as_of_validation() {
    let srv = FakeServer::start(vec![]);
    let a = app(&srv);
    let err = a
        .temporal_as_of(TemporalAsOfArgs {
            query: s("  "),
            ..as_of_args("2026-01-02")
        })
        .await
        .unwrap_err();
    assert_invalid(&err, "query");
    let err = a
        .temporal_as_of(TemporalAsOfArgs {
            top_k: Some(0),
            ..as_of_args("2026-01-02")
        })
        .await
        .unwrap_err();
    assert_invalid(&err, "top_k");
    let err = a.temporal_as_of(as_of_args("yesterday")).await.unwrap_err();
    assert_invalid(&err, "time `yesterday`");
    assert!(srv.calls().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn temporal_as_of_server_errors() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/temporal/as_of").fixture("temporal_as_of_topk_cap"),
        Step::post("/v1/temporal/as_of").fixture("temporal_as_of_missing_query"),
    ]);
    let a = app(&srv);
    let err = a
        .temporal_as_of(TemporalAsOfArgs {
            top_k: Some(100_000),
            ..as_of_args("4000000000")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 400, .. }), "{err:?}");
    let err = a
        .temporal_as_of(as_of_args("4000000000"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 400, .. }), "{err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn trigger_list_and_history_queries() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/triggers").fixture("triggers"),
        Step::get("/v1/triggers?limit=1").fixture("triggers_limit"),
        Step::get("/v1/triggers/history?limit=5").fixture("triggers_history"),
        Step::get("/v1/triggers/history?trigger_type=decay_review")
            .fixture("triggers_history_filtered"),
    ]);
    let a = app(&srv);
    let out = a.trigger_list(TriggerListArgs::default()).await.unwrap();
    assert_eq!(out["count"], 2);
    assert!(out["triggers"][0]["created_at_iso"].is_string());
    let out = a
        .trigger_list(TriggerListArgs { limit: Some(1) })
        .await
        .unwrap();
    assert_eq!(out["count"], 1);
    let out = a
        .trigger_history(TriggerHistoryArgs {
            limit: Some(5),
            trigger_type: None,
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 2);
    let out = a
        .trigger_history(TriggerHistoryArgs {
            limit: None,
            trigger_type: Some(s("decay_review")),
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 1);
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn trigger_actions_post_to_the_action_path() {
    let srv = FakeServer::start(vec![
        Step::post(&fixture_path("trigger_deliver")).fixture("trigger_deliver"),
        Step::post(&fixture_path("trigger_acknowledge")).fixture("trigger_acknowledge"),
        Step::post(&fixture_path("trigger_act")).fixture("trigger_act"),
        Step::post(&fixture_path("trigger_dismiss")).fixture("trigger_dismiss"),
    ]);
    let a = app(&srv);
    let id = |name: &str| TriggerActionArgs {
        id: trigger_id(name),
    };
    let out = a.trigger_deliver(id("trigger_deliver")).await.unwrap();
    assert_eq!(out["action"], "deliver");
    assert_eq!(out["changed"], true);
    let out = a
        .trigger_acknowledge(id("trigger_acknowledge"))
        .await
        .unwrap();
    assert_eq!(out["action"], "acknowledge");
    let out = a.trigger_act(id("trigger_act")).await.unwrap();
    assert_eq!(out["action"], "act");
    let out = a.trigger_dismiss(id("trigger_dismiss")).await.unwrap();
    assert_eq!(out["action"], "dismiss");
    srv.assert_done();
    assert!(srv.requests().iter().all(|r| r.body == json!({})));
}

#[tokio::test(flavor = "current_thread")]
async fn trigger_action_encodes_id_and_maps_errors() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/triggers/a%2Fb/act").json(json!({"changed": true})),
        Step::post(&fixture_path("trigger_act_404")).fixture("trigger_act_404"),
    ]);
    let a = app(&srv);
    a.trigger_act(TriggerActionArgs { id: s("a/b") })
        .await
        .unwrap();
    let err = a
        .trigger_act(TriggerActionArgs {
            id: trigger_id("trigger_act_404"),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::NotFound(m) if m.contains("not found or not in a state")),
        "{err:?}"
    );
    let err = a
        .trigger_dismiss(TriggerActionArgs { id: s(" ") })
        .await
        .unwrap_err();
    assert_invalid(&err, "id");
    srv.assert_done();
}

fn claim_args() -> ClaimAddArgs {
    ClaimAddArgs {
        src: s("Bob"),
        rel_type: s("reviews"),
        dst: s("Zephyr ledger migration"),
        ..ClaimAddArgs::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn claim_add_full_and_minimal() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/claim").fixture("claim"),
        Step::post("/v1/claim").fixture("claim_minimal"),
    ]);
    let a = app(&srv);
    let out = a
        .claim_add(ClaimAddArgs {
            namespace: Some(s("ops")),
            polarity: Some(-1),
            modality: Some(s("asserted")),
            valid_from: Some(s("1790000000")),
            valid_to: Some(s("2026-12-31")),
            confidence_band: Some(s("high")),
            source_memory_rid: Some(s("01a18dcf-1ebd-78ea-9981-9fabefe5fc93")),
            weight: Some(0.8),
            ..claim_args()
        })
        .await
        .unwrap();
    assert_eq!(out["claim_id"], "01a18dcf-1ed2-7ef7-9ef2-124133e63777");
    a.claim_add(claim_args()).await.unwrap();
    srv.assert_done();
    let reqs = srv.requests();
    let b = &reqs[0].body;
    assert_eq!(b["src"], "Bob");
    assert_eq!(b["rel_type"], "reviews");
    assert_eq!(b["dst"], "Zephyr ledger migration");
    assert_eq!(b["namespace"], "ops");
    assert_eq!(b["polarity"], -1);
    assert_eq!(b["modality"], "asserted");
    assert_eq!(b["valid_from"], 1_790_000_000.0);
    assert_eq!(b["valid_to"], 1_798_675_200.0);
    assert_eq!(b["confidence_band"], "high");
    assert_eq!(
        b["source_memory_rid"],
        "01a18dcf-1ebd-78ea-9981-9fabefe5fc93"
    );
    assert_eq!(b["weight"], 0.8);
    assert_eq!(
        reqs[1].body,
        json!({"src": "Bob", "rel_type": "reviews", "dst": "Zephyr ledger migration"})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn claim_add_validation() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/claim")
            .body(|b| b["weight"] == 1.0)
            .json(json!({"claim_id": "c"})),
    ]);
    let a = app(&srv);
    for (args, needle) in [
        (
            ClaimAddArgs {
                src: s(" "),
                ..claim_args()
            },
            "src",
        ),
        (
            ClaimAddArgs {
                rel_type: s(""),
                ..claim_args()
            },
            "rel_type",
        ),
        (
            ClaimAddArgs {
                dst: s(""),
                ..claim_args()
            },
            "dst",
        ),
        (
            ClaimAddArgs {
                weight: Some(f64::NAN),
                ..claim_args()
            },
            "weight",
        ),
        (
            ClaimAddArgs {
                valid_from: Some(s("2026-02-01")),
                valid_to: Some(s("2026-01-01")),
                ..claim_args()
            },
            "later than",
        ),
        (
            ClaimAddArgs {
                valid_to: Some(s("never")),
                ..claim_args()
            },
            "time `never`",
        ),
    ] {
        assert_invalid(&a.claim_add(args).await.unwrap_err(), needle);
    }
    assert!(srv.calls().is_empty());
    a.claim_add(ClaimAddArgs {
        weight: Some(5.0),
        valid_from: Some(s("2026-01-01")),
        valid_to: Some(s("2026-01-01")),
        ..claim_args()
    })
    .await
    .unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn claim_list_queries() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/claims?entity=Bob&namespace=ops").fixture("claims"),
        Step::get("/v1/claims?entity=Carol").fixture("claims_default_ns"),
        Step::get("/v1/claims?entity=Nobody").fixture("claims_empty"),
    ]);
    let a = app(&srv);
    let out = a
        .claim_list(ClaimListArgs {
            entity: s("Bob"),
            namespace: Some(s("ops")),
        })
        .await
        .unwrap();
    assert_eq!(out["claims"][0]["src"], "Bob");
    assert!(out["claims"][0]["created_at_iso"].is_string());
    let out = a
        .claim_list(ClaimListArgs {
            entity: s("Carol"),
            namespace: None,
        })
        .await
        .unwrap();
    assert!(!out["claims"].as_array().unwrap().is_empty());
    let out = a
        .claim_list(ClaimListArgs {
            entity: s("Nobody"),
            namespace: None,
        })
        .await
        .unwrap();
    assert_eq!(out["claims"], json!([]));
    let err = a
        .claim_list(ClaimListArgs {
            entity: s(" "),
            namespace: None,
        })
        .await
        .unwrap_err();
    assert_invalid(&err, "entity");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn procedure_surface_body_and_validation() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/procedures/surface").fixture("procedures_surface"),
        Step::post("/v1/procedures/surface").fixture("procedures_surface_unrelated"),
        Step::post("/v1/procedures/surface").fixture("procedures_surface_missing"),
    ]);
    let a = app(&srv);
    let out = a
        .procedure_surface(ProcedureSurfaceArgs {
            query: s("roll back a deploy"),
            top_k: Some(3),
            domain: Some(s("deploy")),
            namespace: Some(s("ops")),
        })
        .await
        .unwrap();
    assert_eq!(out["count"], 1);
    a.procedure_surface(ProcedureSurfaceArgs {
        query: s("quantum gardening"),
        ..ProcedureSurfaceArgs::default()
    })
    .await
    .unwrap();
    let err = a
        .procedure_surface(ProcedureSurfaceArgs {
            query: s("x"),
            ..ProcedureSurfaceArgs::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 400, .. }), "{err:?}");
    srv.assert_done();
    let reqs = srv.requests();
    assert_eq!(
        reqs[0].body,
        json!({"query": "roll back a deploy", "top_k": 3, "domain": "deploy", "namespace": "ops"})
    );
    assert_eq!(reqs[1].body, json!({"query": "quantum gardening"}));

    let srv = FakeServer::start(vec![]);
    let a = app(&srv);
    assert_invalid(
        &a.procedure_surface(ProcedureSurfaceArgs::default())
            .await
            .unwrap_err(),
        "query",
    );
    assert_invalid(
        &a.procedure_surface(ProcedureSurfaceArgs {
            query: s("x"),
            top_k: Some(0),
            ..ProcedureSurfaceArgs::default()
        })
        .await
        .unwrap_err(),
        "top_k",
    );
    assert!(srv.calls().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn task_list_queries() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/tasks").fixture("tasks"),
        Step::get("/v1/tasks?namespace=ops&status=pending").fixture("tasks_filtered"),
    ]);
    let a = app(&srv);
    let out = a.task_list(TaskListArgs::default()).await.unwrap();
    assert_eq!(out["count"], 0);
    let out = a
        .task_list(TaskListArgs {
            namespace: Some(s("ops")),
            status: Some(s("pending")),
        })
        .await
        .unwrap();
    assert_eq!(out["tasks"], json!([]));
    srv.assert_done();
}

// ---------------------------------------------------------------- secret guard

fn aws() -> String {
    format!("AKIA{}", "Z".repeat(16))
}

fn pem() -> String {
    "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----".to_owned()
}

fn refused(r: yrem::Result<Value>, kind: &str, path: &str) -> String {
    let Err(e @ Error::Sensitive { .. }) = r else {
        panic!("expected Sensitive, got {r:?}");
    };
    let msg = e.to_string();
    assert!(msg.contains(&format!("{kind} at {path}[")), "{msg}");
    assert!(!msg.contains("ZZZZ"), "{msg}");
    msg
}

#[tokio::test(flavor = "current_thread")]
async fn remember_refuses_secrets_without_a_request() {
    let srv = FakeServer::start(vec![]);
    let a = app(&srv);
    let r = a
        .remember(RememberArgs {
            text: Some(format!("key {} here", aws())),
            ..RememberArgs::default()
        })
        .await;
    let msg = refused(r, "aws_access_key", "/text");
    assert!(msg.starts_with("remember refused") && msg.contains("force=true"));
    let r = a
        .remember(RememberArgs {
            text: Some(s("fine")),
            metadata: Some(obj(json!({"db": {"password": "Abcdef12"}}))),
            ..RememberArgs::default()
        })
        .await;
    refused(r, "credential_field", "/metadata/db/password");
    assert!(srv.requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn remember_batch_refuses_the_whole_batch() {
    let srv = FakeServer::start(vec![]);
    let r = app(&srv)
        .remember(RememberArgs {
            memories: Some(vec![mem("fine"), mem(&format!("k {}", aws()))]),
            ..RememberArgs::default()
        })
        .await;
    refused(r, "aws_access_key", "/memories/1/text");
    assert!(srv.requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn remember_force_sends_the_text_verbatim_without_a_force_key() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember").json(json!({"rid": "r1"})),
        Step::post("/v1/remember/batch").json(json!({"count": 2})),
    ]);
    let a = app(&srv);
    let text = format!("key {} here", aws());
    a.remember(RememberArgs {
        text: Some(text.clone()),
        force: true,
        ..RememberArgs::default()
    })
    .await
    .unwrap();
    a.remember(RememberArgs {
        memories: Some(vec![mem(&text), mem("b")]),
        force: true,
        ..RememberArgs::default()
    })
    .await
    .unwrap();
    srv.assert_done();
    let reqs = srv.requests();
    assert_eq!(reqs[0].body["text"], text.as_str());
    assert!(reqs[0].body.get("force").is_none());
    assert_eq!(reqs[1].body["memories"][0]["text"], text.as_str());
    assert!(reqs[1].body.get("force").is_none());
    assert!(reqs[1].body["memories"][0].get("force").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn force_stores_private_keys_and_ydb_tokens_verbatim() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/remember").json(json!({"rid": "r1"})),
        Step::post("/v1/remember/batch").json(json!({"count": 2})),
    ]);
    let a = app(&srv);
    let ydb = format!("ydb_{}", "k".repeat(64));
    a.remember(RememberArgs {
        text: Some(pem()),
        force: true,
        ..RememberArgs::default()
    })
    .await
    .unwrap();
    a.remember(RememberArgs {
        memories: Some(vec![mem("a"), mem(&ydb)]),
        force: true,
        ..RememberArgs::default()
    })
    .await
    .unwrap();
    srv.assert_done();
    let reqs = srv.requests();
    assert_eq!(reqs[0].body["text"], pem().as_str());
    assert_eq!(reqs[1].body["memories"][1]["text"], ydb.as_str());
    assert!(reqs.iter().all(|r| r.body.get("force").is_none()));
}

#[tokio::test(flavor = "current_thread")]
async fn private_key_is_refused_without_force() {
    let srv = FakeServer::start(vec![]);
    let r = app(&srv)
        .remember(RememberArgs {
            text: Some(pem()),
            ..RememberArgs::default()
        })
        .await;
    let msg = refused(r, "private_key", "/text");
    assert!(msg.contains("force=true"));
    assert!(srv.requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn guarded_ops_refuse_before_any_request() {
    let srv = FakeServer::start(vec![]);
    let a = app(&srv);
    let secret = format!("token {}", aws());
    refused(
        a.correct(CorrectArgs {
            new_text: Some(secret.clone()),
            ..correct_args()
        })
        .await,
        "aws_access_key",
        "/new_text",
    );
    refused(
        a.correct(CorrectArgs {
            metadata_merge: Some(obj(json!({"api_key": "Abcdef12"}))),
            ..correct_args()
        })
        .await,
        "credential_field",
        "/metadata_merge/api_key",
    );
    refused(
        a.session_capture(SessionCaptureArgs {
            summary: secret.clone(),
            ..SessionCaptureArgs::default()
        })
        .await,
        "aws_access_key",
        "/summary",
    );
    refused(
        a.graph_relate(GraphRelateArgs {
            src: s("svc"),
            dst: secret.clone(),
            rel_type: s("uses"),
            ..GraphRelateArgs::default()
        })
        .await,
        "aws_access_key",
        "/target",
    );
    refused(
        a.conflict_resolve(ConflictResolveArgs {
            id: s("c1"),
            strategy: Strategy::Merge,
            new_text: Some(secret.clone()),
            note: None,
            force: false,
        })
        .await,
        "aws_access_key",
        "/new_text",
    );
    refused(
        a.conflict_dismiss(ConflictDismissArgs {
            id: s("c1"),
            note: Some(secret.clone()),
            force: false,
        })
        .await,
        "aws_access_key",
        "/resolution_note",
    );
    refused(
        a.forget(ForgetArgs {
            rid: s("r1"),
            reason: Some(secret.clone()),
            force: false,
        })
        .await,
        "aws_access_key",
        "/reason",
    );
    refused(
        a.session_end(SessionEndArgs {
            session_id: s("sid"),
            summary: Some(secret.clone()),
            force: false,
        })
        .await,
        "aws_access_key",
        "/summary",
    );
    refused(
        a.claim_add(yrem::ops::ClaimAddArgs {
            src: s("svc"),
            rel_type: s("uses"),
            dst: secret,
            ..yrem::ops::ClaimAddArgs::default()
        })
        .await,
        "aws_access_key",
        "/dst",
    );
    assert!(srv.requests().is_empty(), "{:?}", srv.calls());
}

#[tokio::test(flavor = "current_thread")]
async fn force_lets_guarded_ops_through() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"rid": NEW_RID})),
        Step::post("/v1/session/end").json(json!({})),
        Step::post("/v1/relate").json(json!({})),
        Step::post("/v1/forget").json(json!({})),
    ]);
    let a = app(&srv);
    let secret = format!("token {}", aws());
    a.correct(CorrectArgs {
        new_text: Some(secret.clone()),
        force: true,
        ..correct_args()
    })
    .await
    .unwrap();
    a.session_capture(SessionCaptureArgs {
        summary: secret.clone(),
        force: true,
        ..SessionCaptureArgs::default()
    })
    .await
    .unwrap();
    a.graph_relate(GraphRelateArgs {
        src: s("svc"),
        dst: secret.clone(),
        rel_type: s("uses"),
        force: true,
        ..GraphRelateArgs::default()
    })
    .await
    .unwrap();
    a.forget(ForgetArgs {
        rid: s("r1"),
        reason: Some(secret),
        force: true,
    })
    .await
    .unwrap();
    srv.assert_done();
    assert!(srv.requests().iter().all(|r| r.body.get("force").is_none()));
}

#[tokio::test(flavor = "current_thread")]
async fn emulated_correct_does_not_scan_the_old_record() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").fixture("correct"),
        Step::get(&memory_path()).json(json!({
            "rid": OLD_RID,
            "text": format!("leaked {}", aws()),
            "metadata": {"password": "Abcdef12"}
        })),
        Step::post("/v1/remember").json(json!({"rid": NEW_RID})),
        Step::post("/v1/forget").json(json!({"found": true})),
    ]);
    let out = app(&srv)
        .correct(CorrectArgs {
            new_text: None,
            ..correct_args()
        })
        .await
        .unwrap();
    assert_eq!(out["emulated"], true);
    srv.assert_done();
    assert_eq!(
        srv.requests()[2].body["text"],
        format!("leaked {}", aws()).as_str()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn correct_redacting_a_secret_is_allowed() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/correct").json(json!({"rid": NEW_RID})),
    ]);
    app(&srv)
        .correct(CorrectArgs {
            new_text: Some(s("key was <redacted>")),
            ..correct_args()
        })
        .await
        .unwrap();
    srv.assert_done();
}

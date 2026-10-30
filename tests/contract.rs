//! Contract tests against a real `yantrikdb-server`. Skipped unless `YREM_CONTRACT=1`;
//! needs `YREM_URL` and `YREM_TOKEN` for a scratch database (never the real store).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use serde_json::{Value, json};
use yrem::Error;
use yrem::client::Client;
use yrem::config::{Config, Overrides};
use yrem::ops::*;

fn app() -> Option<App> {
    if std::env::var("YREM_CONTRACT").ok().as_deref() != Some("1") {
        eprintln!("skipped: set YREM_CONTRACT=1");
        return None;
    }
    let config = Config::load(&Overrides::default()).unwrap();
    Some(App::new(Client::new(&config).unwrap()))
}

fn call<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

/// Unique per test run so consolidation can never merge texts across runs or suites.
fn run_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

#[tokio::test(flavor = "current_thread")]
async fn lifecycle() {
    let Some(app) = app() else { return };
    let ns = format!("yrem-contract-{}", run_id());

    let stored = app
        .remember(call(json!({
            "text": format!("The contract test service stores data in PostgreSQL [{ns}]"),
            "namespace": ns, "domain": "work", "importance": 0.7,
            "metadata": {"suite": "contract"}
        })))
        .await
        .unwrap();
    let rid = stored["rid"].as_str().unwrap().to_owned();

    let batch = app
        .remember(call(json!({"memories": [
            {"text": format!("The contract test team meets on Mondays [{ns}]"), "namespace": ns},
            {"text": format!("The contract test owner is Alice [{ns}]"), "namespace": ns, "created_at": "2026-01-02"}
        ]})))
        .await
        .unwrap();
    assert_eq!(batch["count"], 2, "{batch}");

    let hits = app
        .recall(call(json!({"query": "where does the contract test service store data", "namespace": ns, "top_k": 5})))
        .await
        .unwrap();
    assert!(
        hits["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["rid"] == rid.as_str()),
        "{hits}"
    );

    let got = app
        .memory_get(MemoryGetArgs { rid: rid.clone() })
        .await
        .unwrap();
    assert_eq!(got["memory_type"], "semantic");
    assert_eq!(got["metadata"]["suite"], "contract");

    let corrected = app
        .correct(CorrectArgs {
            rid: rid.clone(),
            reason: "migrated".into(),
            new_text: Some(format!(
                "The contract test service stores data in MySQL [{ns}]"
            )),
            ..CorrectArgs::default()
        })
        .await
        .unwrap();
    eprintln!("correct emulated: {}", corrected["emulated"]);
    if corrected["emulated"] == true {
        let new_rid = corrected["new_rid"].as_str().unwrap();
        let new = app
            .memory_get(MemoryGetArgs {
                rid: new_rid.into(),
            })
            .await
            .unwrap();
        assert_eq!(new["metadata"]["corrects"], rid.as_str());
        assert_eq!(new["metadata"]["suite"], "contract");
        assert_eq!(new["namespace"], ns.as_str());
        assert_eq!(new["domain"], "work");
    }

    app.think(ThinkArgs {}).await.unwrap();
    app.conflict_list(ConflictListArgs::default())
        .await
        .unwrap();
    app.session_digest(SessionDigestArgs {
        namespace: None,
        include_gaps: true,
    })
    .await
    .unwrap();
    app.memory_current(MemoryCurrentArgs {
        namespace: ns.clone(),
    })
    .await
    .unwrap();
    app.memory_list(MemoryListArgs {
        namespace: Some(ns.clone()),
        ..MemoryListArgs::default()
    })
    .await
    .unwrap();
    app.memory_gaps(MemoryGapsArgs::default()).await.unwrap();
    app.memory_stats(MemoryStatsArgs {}).await.unwrap();
    app.graph_relate(call(
        json!({"src": "contract-svc", "dst": "postgres", "rel_type": "uses", "namespace": ns}),
    ))
    .await
    .unwrap();
    match app
        .graph_edges(GraphEdgesArgs {
            entity: "contract-svc".into(),
        })
        .await
    {
        Ok(v) => eprintln!("graph_edges served: {v}"),
        Err(Error::Unsupported { .. }) => eprintln!("graph_edges: unsupported"),
        Err(e) => panic!("graph_edges: {e}"),
    }
    let captured = app
        .session_capture(SessionCaptureArgs {
            summary: format!(
                "We decided the contract test runs nightly. Bob owns the contract runner [{ns}]."
            ),
            namespace: Some(ns.clone()),
            ..SessionCaptureArgs::default()
        })
        .await
        .unwrap();
    assert!(captured["count"].as_u64().is_some(), "{captured}");
    let session = app
        .session_start(SessionStartArgs {
            namespace: Some(ns.clone()),
            client_id: Some("yrem-contract".into()),
            ..SessionStartArgs::default()
        })
        .await
        .unwrap();
    let sid = session["session_id"].as_str().unwrap().to_owned();
    app.session_end(SessionEndArgs {
        force: false,
        session_id: sid,
        summary: None,
    })
    .await
    .unwrap();

    app.forget(ForgetArgs {
        force: false,
        rid,
        reason: Some("contract cleanup".into()),
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn v02_endpoints() {
    let Some(app) = app() else { return };
    let ns = format!("yrem-contract-v02-{}", run_id());

    let old = app
        .remember(call(json!({
            "text": format!("The v02 contract ledger runbook lives in the archive [{ns}]"),
            "namespace": ns, "created_at": "2026-01-02"
        })))
        .await
        .unwrap();
    let procedure = app
        .remember(call(json!({
            "text": format!("To roll back a v02 contract deploy, redeploy the previous tag [{ns}]"),
            "namespace": ns, "memory_type": "procedural", "domain": "deploy"
        })))
        .await
        .unwrap();
    assert!(old["rid"].is_string() && procedure["rid"].is_string());

    let stale = app
        .temporal_stale(TemporalListArgs {
            days: Some(0),
            limit: Some(10),
            namespace: Some(ns.clone()),
        })
        .await
        .unwrap();
    assert!(stale["count"].as_u64().unwrap() >= 2, "{stale}");
    let upcoming = app
        .temporal_upcoming(TemporalListArgs {
            namespace: Some(ns.clone()),
            ..TemporalListArgs::default()
        })
        .await
        .unwrap();
    assert!(upcoming["count"].is_u64(), "{upcoming}");

    let before = app
        .temporal_as_of(call(json!({
            "query": "ledger runbook", "at": "2025-01-01", "namespace": ns
        })))
        .await
        .unwrap();
    assert_eq!(before["count"], 0, "{before}");
    let after = app
        .temporal_as_of(call(json!({
            "query": "ledger runbook", "at": "1h", "namespace": ns, "top_k": 5
        })))
        .await
        .unwrap();
    assert!(after["count"].as_u64().unwrap() >= 1, "{after}");

    let triggers = app.trigger_list(TriggerListArgs::default()).await.unwrap();
    assert!(triggers["triggers"].is_array(), "{triggers}");
    let history = app
        .trigger_history(TriggerHistoryArgs::default())
        .await
        .unwrap();
    assert!(history["triggers"].is_array(), "{history}");

    let claim = app
        .claim_add(call(json!({
            "src": "contract-bob", "rel_type": "reviews", "dst": "contract-ledger",
            "namespace": ns, "valid_from": "2026-01-01", "weight": 0.8,
            "confidence_band": "high"
        })))
        .await
        .unwrap();
    assert!(claim["claim_id"].is_string(), "{claim}");
    let claims = app
        .claim_list(ClaimListArgs {
            entity: "contract-bob".into(),
            namespace: Some(ns.clone()),
        })
        .await
        .unwrap();
    assert_eq!(claims["claims"][0]["dst"], "contract-ledger", "{claims}");

    let surfaced = app
        .procedure_surface(call(json!({
            "query": "roll back a deploy", "namespace": ns, "domain": "deploy", "top_k": 3
        })))
        .await
        .unwrap();
    assert!(surfaced["count"].as_u64().unwrap() >= 1, "{surfaced}");

    let tasks = app
        .task_list(TaskListArgs {
            namespace: Some(ns.clone()),
            ..TaskListArgs::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks["count"], 0, "{tasks}");

    let missing = app
        .trigger_act(TriggerActionArgs {
            id: "00000000-0000-7000-8000-000000000000".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(missing, Error::NotFound(_)), "{missing}");
}

#[tokio::test(flavor = "current_thread")]
async fn errors() {
    let Some(app) = app() else { return };
    let missing = app
        .memory_get(MemoryGetArgs {
            rid: "00000000-0000-7000-8000-000000000000".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(missing, Error::NotFound(_)), "{missing}");
    let mut config = Config::load(&Overrides::default()).unwrap();
    config.token = Some("ydb_invalid".into());
    let bad = App::new(Client::new(&config).unwrap());
    let err = bad.think(ThinkArgs {}).await.unwrap_err();
    assert!(matches!(err, Error::Auth { status: 401, .. }), "{err}");
}

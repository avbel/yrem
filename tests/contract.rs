//! Contract tests against a real `yantrikdb-server`. Skipped unless `YREM_CONTRACT=1`;
//! needs `YREM_URL` and `YREM_TOKEN` for a scratch database (never the real store).
#![allow(clippy::unwrap_used, clippy::expect_used)]

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

#[tokio::test(flavor = "current_thread")]
async fn lifecycle() {
    let Some(app) = app() else { return };
    let ns = format!("yrem-contract-{}", std::process::id());

    let stored = app
        .remember(call(json!({
            "text": "The contract test service stores data in PostgreSQL",
            "namespace": ns, "domain": "work", "importance": 0.7,
            "metadata": {"suite": "contract"}
        })))
        .await
        .unwrap();
    let rid = stored["rid"].as_str().unwrap().to_owned();

    let batch = app
        .remember(call(json!({"memories": [
            {"text": "The contract test team meets on Mondays", "namespace": ns},
            {"text": "The contract test owner is Alice", "namespace": ns, "created_at": "2026-01-02"}
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
            new_text: Some("The contract test service stores data in MySQL".into()),
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
            summary: "We decided the contract test runs nightly. Bob owns the contract runner."
                .into(),
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
        })
        .await
        .unwrap();
    let sid = session["session_id"].as_str().unwrap().to_owned();
    app.session_end(SessionEndArgs {
        session_id: sid,
        summary: None,
    })
    .await
    .unwrap();

    app.forget(ForgetArgs {
        rid,
        reason: Some("contract cleanup".into()),
    })
    .await
    .unwrap();
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

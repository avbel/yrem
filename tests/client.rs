#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::fake_server::{FakeServer, Step, TOKEN};
use yrem::Error;
use yrem::client::{Client, ConflictsQuery, RecallRequest};
use yrem::config::{Config, Origin};

fn config(url: &str) -> Config {
    Config {
        url: url.to_owned(),
        token: Some(TOKEN.to_owned()),
        url_origin: Origin::Flag,
        token_origin: Origin::Flag,
        file: None,
    }
}

fn client(srv: &FakeServer) -> Client {
    Client::new(&config(&srv.url)).unwrap()
}

async fn err_for(status: u16, body: Value) -> Error {
    let srv = FakeServer::start(vec![Step::get("/v1/health").status(status).json(body)]);
    let e = client(&srv).health().await.unwrap_err();
    srv.assert_done();
    e
}

#[tokio::test(flavor = "current_thread")]
async fn status_to_error_table() {
    assert!(matches!(
        err_for(401, json!({"error": "bad token"})).await,
        Error::Auth { status: 401, ref message } if message == "bad token"
    ));
    assert!(matches!(
        err_for(403, json!({})).await,
        Error::Auth { status: 403, .. }
    ));
    assert!(
        matches!(err_for(404, json!({"error": "nope"})).await, Error::NotFound(m) if m == "nope")
    );
    assert!(matches!(err_for(429, json!({})).await, Error::Quota(_)));
    assert!(
        matches!(err_for(503, json!({"error": "ro"})).await, Error::Unavailable(m) if m == "ro")
    );
    assert!(matches!(
        err_for(500, json!({"x": 1})).await,
        Error::Http { status: 500, ref body } if body.contains("\"x\"")
    ));
    assert!(matches!(
        err_for(418, json!({})).await,
        Error::Http { status: 418, .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_501() {
    let e = err_for(
        501,
        json!({"error": "not_yet_available_over_http", "operation": "correct", "reason": "r"}),
    )
    .await;
    assert!(
        matches!(&e, Error::Unsupported { operation, reason } if operation == "correct" && reason == "r"),
        "{e:?}"
    );
    let e = err_for(501, json!({"error": "nope"})).await;
    assert!(
        matches!(&e, Error::Unsupported { operation, reason } if operation == "health" && reason == "nope"),
        "{e:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn both_error_envelopes() {
    let e = err_for(
        404,
        json!({"error": {"code": "x", "message": "nested msg"}}),
    )
    .await;
    assert!(matches!(e, Error::NotFound(m) if m == "nested msg"));
    let e = err_for(404, json!({"message": "top msg"})).await;
    assert!(matches!(e, Error::NotFound(m) if m == "top msg"));
    // Non-JSON body falls back to the trimmed text.
    let srv = FakeServer::start(vec![Step::get("/v1/health").status(404).raw("  plain \n")]);
    let e = client(&srv).health().await.unwrap_err();
    assert!(matches!(e, Error::NotFound(m) if m == "plain"));
}

#[tokio::test(flavor = "current_thread")]
async fn connection_refused_is_unavailable() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let c = Client::new(&config(&format!("http://127.0.0.1:{port}"))).unwrap();
    let e = c.health().await.unwrap_err();
    assert!(
        matches!(&e, Error::Unavailable(m) if m.to_lowercase().contains("connection refused")),
        "the OS cause must reach the message: {e}"
    );
    assert!(e.is_retryable());
}

#[tokio::test(flavor = "current_thread")]
async fn timeout_is_unavailable() {
    let srv = FakeServer::start(vec![Step::get("/v1/health").delay(Duration::from_secs(2))]);
    let c = Client::with_timeouts(
        &config(&srv.url),
        Duration::from_secs(1),
        Duration::from_millis(200),
    )
    .unwrap();
    let e = c.health().await.unwrap_err();
    assert!(
        matches!(&e, Error::Unavailable(m) if m.contains("timed out")),
        "{e:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_json_is_decode_and_empty_is_null() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/health").raw("not json"),
        Step::get("/v1/health").raw(""),
        Step::get("/v1/health").raw("  \n"),
    ]);
    let c = client(&srv);
    assert!(matches!(c.health().await.unwrap_err(), Error::Decode(_)));
    assert_eq!(c.health().await.unwrap(), Value::Null);
    assert_eq!(c.health().await.unwrap(), Value::Null);
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn recall_decode_is_lenient() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(json!({
        "results": [{"rid": "r1", "text": "t", "score": 0.5, "created_at": 1.0, "weird": {"a": 1}}],
        "engine": "v2",
    }))]);
    let r = client(&srv)
        .recall(&RecallRequest {
            query: "q".into(),
            ..RecallRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].rid, "r1");
    assert_eq!(r.results[0].score, Some(0.5));
    assert_eq!(r.results[0].extra["weird"], json!({"a": 1}));
    assert_eq!(r.extra["engine"], "v2");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn recall_wrong_shape_is_decode() {
    let srv = FakeServer::start(vec![Step::post("/v1/recall").json(json!({"results": 5}))]);
    let e = client(&srv)
        .recall(&RecallRequest::default())
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Decode(_)), "{e:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn headers_sent() {
    let srv = FakeServer::start(vec![Step::get("/v1/health").json(json!({"ok": true}))]);
    assert_eq!(client(&srv).health().await.unwrap(), json!({"ok": true}));
    let reqs = srv.requests();
    assert_eq!(
        reqs[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(reqs[0].header("user-agent").unwrap().starts_with("yrem/"));
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn no_token_sends_no_auth_header() {
    let srv = FakeServer::start(vec![Step::get("/v1/health").auth(None)]);
    let mut cfg = config(&srv.url);
    cfg.token = None;
    Client::new(&cfg).unwrap().health().await.unwrap();
    assert!(srv.requests()[0].header("authorization").is_none());
}

#[test]
fn debug_does_not_leak_token() {
    let c = Client::new(&config("http://127.0.0.1:1")).unwrap();
    assert!(!format!("{c:?}").contains(TOKEN));
}

#[test]
fn invalid_token_characters() {
    let mut cfg = config("http://127.0.0.1:1");
    cfg.token = Some("bad\ntoken".into());
    assert!(matches!(Client::new(&cfg), Err(Error::Invalid(_))));
}

#[test]
fn invalid_urls() {
    for url in ["ftp://host", "not a url", "", "file:///x"] {
        let r = Client::new(&config(url));
        assert!(matches!(r, Err(Error::Invalid(_))), "{url}");
    }
}

#[cfg(not(feature = "tls"))]
#[test]
fn https_without_tls_is_invalid() {
    let e = Client::new(&config("https://host")).unwrap_err();
    assert!(
        matches!(&e, Error::Invalid(m) if m.contains("TLS")),
        "{e:?}"
    );
}

#[cfg(feature = "tls")]
#[test]
fn https_accepted_with_tls() {
    assert!(Client::new(&config("https://host")).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn conflicts_query_string() {
    // serde_json::Map is alphabetical, so `limit` precedes `status`; nulls are skipped.
    let srv = FakeServer::start(vec![
        Step::get("/v1/conflicts?limit=5&status=open").json(json!({"conflicts": []})),
        Step::get("/v1/conflicts"),
    ]);
    let c = client(&srv);
    c.conflicts(&ConflictsQuery {
        status: Some("open".into()),
        limit: Some(5),
        ..ConflictsQuery::default()
    })
    .await
    .unwrap();
    c.conflicts(&ConflictsQuery::default()).await.unwrap();
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn path_segments_are_percent_encoded() {
    let srv = FakeServer::start(vec![
        Step::get("/v1/memory/a%20b%2Fc").json(json!({"rid": "a b/c"})),
    ]);
    let m = client(&srv).memory_get("a b/c").await.unwrap();
    assert_eq!(m.rid, "a b/c");
    srv.assert_done();
}

#[tokio::test(flavor = "current_thread")]
async fn graph_edges_404_is_unsupported() {
    let srv = FakeServer::start(vec![
        Step::post("/v1/graph").status(404),
        Step::post("/v1/graph").status(405).raw("no"),
        Step::post("/v1/graph").json(json!({"edges": []})),
    ]);
    let c = client(&srv);
    for _ in 0..2 {
        let e = c.graph_edges("x").await.unwrap_err();
        assert!(
            matches!(&e, Error::Unsupported { operation, .. } if operation == "graph_edges"),
            "{e:?}"
        );
    }
    assert_eq!(c.graph_edges("x").await.unwrap(), json!({"edges": []}));
}

#[tokio::test(flavor = "current_thread")]
async fn base_url_with_path_prefix() {
    let srv = FakeServer::start(vec![
        Step::get("/prefix/v1/health"),
        Step::get("/prefix/v1/conflicts?limit=1"),
    ]);
    let c = Client::new(&config(&format!("{}/prefix", srv.url))).unwrap();
    c.health().await.unwrap();
    c.conflicts(&ConflictsQuery {
        limit: Some(1),
        ..ConflictsQuery::default()
    })
    .await
    .unwrap();
    srv.assert_done();
    assert_eq!(c.base().path(), "/prefix");
}

#[test]
fn exit_codes_and_retryable() {
    fn raw_transport() -> Error {
        // Builder errors surface as reqwest::Error without needing the network.
        let e = reqwest::Client::new().get("http://[").build().unwrap_err();
        Error::Transport(e)
    }
    let partial = |inner: Error| Error::PartialCorrect {
        old_rid: "o".into(),
        new_rid: "n".into(),
        source: Box::new(inner),
    };
    // (error, exit code, retryable)
    let cases: Vec<(Error, u8, bool)> = vec![
        (Error::Invalid("x".into()), 2, false),
        (
            Error::Auth {
                status: 401,
                message: String::new(),
            },
            3,
            false,
        ),
        (
            Error::Unsupported {
                operation: "o".into(),
                reason: "r".into(),
            },
            4,
            false,
        ),
        (Error::Unavailable("x".into()), 5, true),
        (partial(Error::NotFound("x".into())), 6, false),
        (Error::NotFound("x".into()), 1, false),
        (Error::Quota("x".into()), 1, true),
        (
            Error::Http {
                status: 500,
                body: String::new(),
            },
            1,
            true,
        ),
        (
            Error::Http {
                status: 502,
                body: String::new(),
            },
            1,
            true,
        ),
        (
            Error::Http {
                status: 400,
                body: String::new(),
            },
            1,
            false,
        ),
        (Error::Decode("x".into()), 1, false),
        (raw_transport(), 1, true),
    ];
    for (e, code, retry) in cases {
        assert_eq!(e.exit_code(), code, "{e:?}");
        assert_eq!(e.is_retryable(), retry, "{e:?}");
        assert_ne!(e.to_string(), "");
    }
}

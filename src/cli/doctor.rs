//! `yrem doctor`: resolved config, connectivity, auth, server version, 501 operations.

use serde_json::{Value, json};

use crate::client::{Client, ConflictsQuery, CorrectRequest};
use crate::config::Config;
use crate::error::Error;

/// Builds the doctor report. Problems are reported in the JSON, not as errors.
pub async fn run(config: &Config) -> Value {
    let mut report = json!({
        "config": {
            "url": config.url,
            "url_from": config.url_origin,
            "token": config.token.as_ref().map(|_| "<redacted>"),
            "token_from": config.token_origin,
            "file": config.file.as_ref().map(|p| p.display().to_string()),
        },
    });
    let client = match Client::new(config) {
        Ok(c) => c,
        Err(e) => {
            report["ok"] = false.into();
            report["error"] = e.to_string().into();
            return report;
        }
    };
    let health = client.health_deep().await;
    report["connectivity"] = match &health {
        Ok(_) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    if let Ok(h) = &health {
        report["server_version"] = h["version"].clone();
        report["health"] = h.clone();
    }
    let auth = client
        .conflicts(&ConflictsQuery {
            limit: Some(1),
            ..Default::default()
        })
        .await;
    report["auth"] = match &auth {
        Ok(_) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    let mut unsupported = Vec::new();
    // An empty rid can never be a real record: the probe has no side effect.
    let probe = CorrectRequest {
        rid: String::new(),
        reason: "yrem doctor probe".into(),
        ..Default::default()
    };
    if let Err(Error::Unsupported { operation, .. }) = client.correct(&probe).await {
        unsupported.push(operation);
    }
    if let Err(Error::Unsupported { operation, .. }) = client.graph_edges("yrem-doctor-probe").await
    {
        unsupported.push(operation);
    }
    report["unsupported"] = unsupported.into();
    report["ok"] = (health.is_ok() && auth.is_ok()).into();
    report
}

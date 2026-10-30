//! HTTP client for `yantrikdb-server`.

mod types;

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;
use crate::error::{Error, Result};

pub use types::*;

/// Thin typed wrapper over `reqwest::Client`. Cheap to share; it pools connections.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
}

impl Client {
    /// Builds a client for `config`.
    ///
    /// # Errors
    /// Invalid URL, an `https` URL in a build without TLS, or a token that is not a valid header.
    pub fn new(config: &Config) -> Result<Self> {
        Self::with_timeouts(config, Duration::from_secs(3), Duration::from_secs(15))
    }

    /// Builds a client with explicit connect / total timeouts.
    ///
    /// # Errors
    /// See [`Client::new`].
    pub fn with_timeouts(config: &Config, connect: Duration, total: Duration) -> Result<Self> {
        let base = Url::parse(&config.url)
            .map_err(|e| Error::Invalid(format!("server URL `{}`: {e}", config.url)))?;
        check_scheme(&base)?;
        let mut headers = HeaderMap::new();
        if let Some(token) = &config.token {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| Error::Invalid("token contains invalid characters".into()))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(total)
            .user_agent(concat!("yrem/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .build()?;
        Ok(Self { http, base })
    }

    /// Server base URL.
    #[must_use]
    pub fn base(&self) -> &Url {
        &self.base
    }

    fn url(&self, path: &str, query: Option<&Value>) -> Url {
        let mut url = self.base.clone();
        let prefix = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{prefix}{path}"));
        if let Some(Value::Object(map)) = query {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in map {
                match v {
                    Value::Null => {}
                    Value::String(s) => {
                        pairs.append_pair(k, s);
                    }
                    other => {
                        pairs.append_pair(k, &other.to_string());
                    }
                }
            }
        }
        if url.query() == Some("") {
            url.set_query(None);
        }
        url
    }

    /// Sends a request and decodes the JSON answer. `op` names the operation in errors.
    async fn send(
        &self,
        op: &str,
        method: Method,
        path: &str,
        query: Option<&Value>,
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut req = self.http.request(method, self.url(path, query));
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.map_err(transport)?;
        let status = resp.status();
        let text = resp.text().await.map_err(transport)?;
        if !status.is_success() {
            return Err(map_status(op, status, &text));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| Error::Decode(format!("{op}: {e}")))
    }

    async fn get(&self, op: &str, path: &str, query: Option<&Value>) -> Result<Value> {
        self.send(op, Method::GET, path, query, None).await
    }

    async fn post<B: Serialize + ?Sized>(&self, op: &str, path: &str, body: &B) -> Result<Value> {
        let body = to_value(body)?;
        self.send(op, Method::POST, path, None, Some(&body)).await
    }

    pub async fn health(&self) -> Result<Value> {
        self.get("health", "/v1/health", None).await
    }

    pub async fn health_deep(&self) -> Result<Value> {
        self.get("health_deep", "/v1/health/deep", None).await
    }

    pub async fn stats(&self) -> Result<Value> {
        self.get("stats", "/v1/stats", None).await
    }

    pub async fn remember(&self, req: &RememberRequest) -> Result<Value> {
        self.post("remember", "/v1/remember", req).await
    }

    pub async fn remember_batch(&self, memories: &[RememberRequest]) -> Result<Value> {
        self.post("remember", "/v1/remember/batch", &BatchRequest { memories })
            .await
    }

    pub async fn recall(&self, req: &RecallRequest) -> Result<RecallResponse> {
        decode("recall", self.post("recall", "/v1/recall", req).await?)
    }

    pub async fn forget(&self, req: &ForgetRequest) -> Result<Value> {
        self.post("forget", "/v1/forget", req).await
    }

    pub async fn correct(&self, req: &CorrectRequest) -> Result<Value> {
        self.post("correct", "/v1/correct", req).await
    }

    pub async fn think(&self, req: &ThinkRequest) -> Result<Value> {
        self.post("think", "/v1/think", req).await
    }

    pub async fn relate(&self, req: &RelateRequest) -> Result<Value> {
        self.post("graph_relate", "/v1/relate", req).await
    }

    /// Edges of an entity. Servers without an HTTP graph endpoint answer 404 / 405,
    /// which is reported as `Unsupported`.
    pub async fn graph_edges(&self, entity: &str) -> Result<Value> {
        let body = GraphQuery {
            action: "query",
            entity,
        };
        match self.post("graph_edges", "/v1/graph", &body).await {
            Err(Error::NotFound(_) | Error::Http { status: 405, .. }) => Err(Error::Unsupported {
                operation: "graph_edges".into(),
                reason: "this server has no HTTP graph query endpoint".into(),
            }),
            other => other,
        }
    }

    pub async fn conflicts(&self, query: &ConflictsQuery) -> Result<Value> {
        let q = to_value(query)?;
        self.get("conflict_list", "/v1/conflicts", Some(&q)).await
    }

    pub async fn resolve_conflict(&self, id: &str, req: &ResolveRequest) -> Result<Value> {
        let path = format!("/v1/conflicts/{}/resolve", segment(id));
        self.post("conflict_resolve", &path, req).await
    }

    pub async fn session_digest(&self, query: &DigestQuery) -> Result<Value> {
        let q = to_value(query)?;
        self.get("session_digest", "/v1/session/digest", Some(&q))
            .await
    }

    pub async fn session_capture(&self, req: &CaptureRequest) -> Result<Value> {
        self.post("session_capture", "/v1/session/end", req).await
    }

    pub async fn session_start(&self, req: &SessionStartRequest) -> Result<Value> {
        self.post("session_start", "/v1/sessions", req).await
    }

    pub async fn session_end(&self, id: &str, req: &SessionEndRequest) -> Result<Value> {
        let path = format!("/v1/sessions/{}", segment(id));
        let body = to_value(req)?;
        self.send("session_end", Method::DELETE, &path, None, Some(&body))
            .await
    }

    pub async fn memory_get(&self, rid: &str) -> Result<Memory> {
        let path = format!("/v1/memory/{}", segment(rid));
        decode("memory_get", self.get("memory_get", &path, None).await?)
    }

    pub async fn memories(&self, query: &MemoriesQuery) -> Result<Value> {
        let q = to_value(query)?;
        self.get("memory_list", "/v1/memories", Some(&q)).await
    }

    pub async fn current(&self, namespace: &str) -> Result<Value> {
        let q = serde_json::json!({ "namespace": namespace });
        self.get("memory_current", "/v1/current", Some(&q)).await
    }

    pub async fn gaps(&self, query: &GapsQuery) -> Result<Value> {
        let q = to_value(query)?;
        self.get("memory_gaps", "/v1/insights/gaps", Some(&q)).await
    }
}

/// Accepts `http`, and `https` only in builds with the `tls` feature.
fn check_scheme(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" => Ok(()),
        "https" => cfg_select! {
            feature = "tls" => { Ok(()) }
            _ => {
                Err(Error::Invalid(
                    "https URL, but yrem was built without TLS; rebuild with `--features tls`"
                        .into(),
                ))
            }
        },
        s => Err(Error::Invalid(format!("unsupported URL scheme `{s}`"))),
    }
}

/// Percent-encodes one path segment.
fn segment(s: &str) -> String {
    let mut url = Url::parse("http://x/").unwrap_or_else(|_| unreachable!());
    url.path_segments_mut()
        .unwrap_or_else(|()| unreachable!())
        .push(s);
    url.path()[1..].to_owned()
}

fn to_value<T: Serialize + ?Sized>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Invalid(e.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(op: &str, v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::Decode(format!("{op}: {e}")))
}

fn transport(e: reqwest::Error) -> Error {
    if e.is_timeout() {
        Error::Unavailable(format!("request timed out: {e}"))
    } else if e.is_connect() {
        Error::Unavailable(format!("cannot connect: {e}"))
    } else {
        Error::Transport(e)
    }
}

/// Maps a non-2xx answer to an [`Error`].
pub(crate) fn map_status(op: &str, status: StatusCode, body: &str) -> Error {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = error_message(&json).unwrap_or_else(|| body.trim().to_owned());
    match status.as_u16() {
        s @ (401 | 403) => Error::Auth { status: s, message },
        404 => Error::NotFound(message),
        429 => Error::Quota(message),
        501 if let Some(operation) = json["operation"].as_str() => Error::Unsupported {
            operation: operation.to_owned(),
            reason: json["reason"].as_str().unwrap_or(&message).to_owned(),
        },
        501 => Error::Unsupported {
            operation: op.to_owned(),
            reason: message,
        },
        503 => Error::Unavailable(message),
        s => Error::Http {
            status: s,
            body: body.to_owned(),
        },
    }
}

/// Extracts the message from either error envelope the server uses:
/// `{"error": "msg"}` or `{"error": {"code", "message"}}`.
fn error_message(json: &Value) -> Option<String> {
    match &json["error"] {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) if let Some(m) = o.get("message").and_then(Value::as_str) => {
            Some(m.to_owned())
        }
        _ => json["message"].as_str().map(str::to_owned),
    }
}

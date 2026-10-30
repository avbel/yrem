//! Scripted fake `yantrikdb-server`: a queue of expected requests and canned answers.
//! Every request is recorded so tests can assert which calls happened.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The token the fake server expects unless a step says otherwise.
pub const TOKEN: &str = "ydb_test_token";

/// One scripted exchange.
#[derive(Clone)]
pub struct Step {
    pub method: &'static str,
    /// Path, optionally with `?query`; matched exactly.
    pub path: String,
    /// Expected `Authorization` header value; `None` skips the check.
    pub auth: Option<String>,
    /// Body predicate.
    pub body: Option<Arc<dyn Fn(&Value) -> bool + Send + Sync>>,
    pub status: u16,
    pub response: String,
    pub delay: Option<Duration>,
}

impl Step {
    pub fn new(method: &'static str, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            auth: Some(format!("Bearer {TOKEN}")),
            body: None,
            status: 200,
            response: "{}".into(),
            delay: None,
        }
    }
    pub fn get(path: &str) -> Self {
        Self::new("GET", path)
    }
    pub fn post(path: &str) -> Self {
        Self::new("POST", path)
    }
    pub fn delete(path: &str) -> Self {
        Self::new("DELETE", path)
    }
    pub fn status(mut self, s: u16) -> Self {
        self.status = s;
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.response = v.to_string();
        self
    }
    pub fn raw(mut self, s: &str) -> Self {
        self.response = s.to_owned();
        self
    }
    /// Answers with the `response` of `tests/fixtures/<name>.json`.
    pub fn fixture(mut self, name: &str) -> Self {
        let f = fixture(name);
        self.status = f["status"].as_u64().map_or(200, |s| s as u16);
        self.response = match &f["response"] {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        self
    }
    pub fn body(mut self, f: impl Fn(&Value) -> bool + Send + Sync + 'static) -> Self {
        self.body = Some(Arc::new(f));
        self
    }
    pub fn auth(mut self, a: Option<&str>) -> Self {
        self.auth = a.map(str::to_owned);
        self
    }
    pub fn delay(mut self, d: Duration) -> Self {
        self.delay = Some(d);
        self
    }
}

/// Loads `tests/fixtures/<name>.json`.
pub fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// A request the server received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    /// The client closed the connection before the answer was sent.
    pub aborted: bool,
    /// Why the request did not match its step, if it didn't.
    pub mismatch: Option<String>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Default)]
struct State {
    steps: VecDeque<Step>,
    log: Vec<Recorded>,
}

/// Handle to a running fake server. It lives on its own thread and runtime, so tests
/// may block (e.g. on a child process) while it serves.
#[derive(Clone)]
pub struct FakeServer {
    pub url: String,
    state: Arc<Mutex<State>>,
}

impl FakeServer {
    pub fn start(steps: Vec<Step>) -> Self {
        let state = Arc::new(Mutex::new(State {
            steps: steps.into(),
            log: Vec::new(),
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        let st = state.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                loop {
                    let Ok((sock, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(serve(sock, st.clone()));
                }
            });
        });
        let addr = rx.recv().unwrap();
        Self {
            url: format!("http://{addr}"),
            state,
        }
    }

    /// Requests received so far.
    pub fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().log.clone()
    }

    /// `METHOD path` of every request, in order.
    pub fn calls(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect()
    }

    /// Steps not yet consumed.
    pub fn remaining(&self) -> usize {
        self.state.lock().unwrap().steps.len()
    }

    /// Panics if any request mismatched its step or steps remain.
    pub fn assert_done(&self) {
        for r in self.requests() {
            assert!(
                r.mismatch.is_none(),
                "{} {}: {:?}",
                r.method,
                r.path,
                r.mismatch
            );
        }
        assert_eq!(
            self.remaining(),
            0,
            "unconsumed steps; calls: {:?}",
            self.calls()
        );
    }

    /// Waits until `n` requests were recorded (or 5 s pass).
    pub fn wait_for(&self, n: usize) {
        for _ in 0..500 {
            if self.requests().len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

async fn serve(mut sock: TcpStream, state: Arc<Mutex<State>>) {
    loop {
        let Some((method, path, headers, body)) = read_request(&mut sock).await else {
            return;
        };
        let step = state.lock().unwrap().steps.pop_front();
        let mut rec = Recorded {
            method: method.clone(),
            path: path.clone(),
            headers: headers.clone(),
            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
            aborted: false,
            mismatch: None,
        };
        let (status, response) = match &step {
            None => {
                rec.mismatch = Some("no step left".into());
                (599, "unexpected request".to_owned())
            }
            Some(s) => {
                let auth = rec.header("authorization").map(str::to_owned);
                if s.method != method || s.path != path {
                    rec.mismatch = Some(format!("expected {} {}", s.method, s.path));
                } else if s.auth.is_some() && s.auth != auth {
                    rec.mismatch = Some(format!("auth {auth:?}"));
                } else if let Some(f) = &s.body
                    && !f(&rec.body)
                {
                    rec.mismatch = Some(format!("body {}", rec.body));
                }
                (s.status, s.response.clone())
            }
        };
        let idx = {
            let mut st = state.lock().unwrap();
            st.log.push(rec);
            st.log.len() - 1
        };
        if let Some(d) = step.as_ref().and_then(|s| s.delay) {
            let mut probe = [0u8; 1];
            tokio::select! {
                () = tokio::time::sleep(d) => {}
                r = sock.read(&mut probe) => {
                    if matches!(r, Ok(0) | Err(_)) {
                        state.lock().unwrap().log[idx].aborted = true;
                        return;
                    }
                }
            }
        }
        let reason = if status < 300 { "OK" } else { "ERR" };
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            response.len()
        );
        if sock.write_all(head.as_bytes()).await.is_err()
            || sock.write_all(response.as_bytes()).await.is_err()
        {
            return;
        }
    }
}

type Request = (String, String, Vec<(String, String)>, Vec<u8>);

async fn read_request(sock: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, headers, body))
}

//! Shared HTTP helpers for talking to the (forked) local backend: connection defaults,
//! agent construction and the raw `POST /api/action` call (used by phase 1 and phase 2).

use std::{fs, time::{Duration, Instant}};

use serde_json::{json, Value};

use crate::{bench_dir, short};

#[derive(Clone)]
pub struct Conn {
    pub url: String,
    pub admin_key: String,
}

pub fn default_url() -> String {
    std::env::var("CONVEX_SELF_HOSTED_URL").unwrap_or_else(|_| "http://127.0.0.1:3220".into())
}

pub fn default_admin_key() -> String {
    std::env::var("CONVEX_SELF_HOSTED_ADMIN_KEY")
        .ok()
        .or_else(|| fs::read_to_string(data_dir(false).join("admin-key")).ok().map(|s| s.trim().to_string()))
        .unwrap_or_default()
}

/// Data dir of the main (`native = false`) or the rust-native backend. Mirrors
/// `scripts/start-backend.sh`: `$BENCH_DATA_DIR` (default `bench/.backend-data`), plus `-native`.
pub fn data_dir(native: bool) -> std::path::PathBuf {
    let base = std::env::var("BENCH_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| bench_dir().join(".backend-data"));
    if native {
        let mut s = base.into_os_string();
        s.push("-native");
        s.into()
    } else {
        base
    }
}

/// Admin key of the rust-native backend (written by its start script), falling back to the main one.
pub fn native_admin_key(main_key: &str) -> String {
    fs::read_to_string(data_dir(true).join("admin-key")).map(|s| s.trim().to_string()).unwrap_or_else(|_| main_key.to_string())
}

/// Port offset of the rust-native backend relative to the main one.
pub const NATIVE_PORT_OFFSET: u16 = 10;

/// `http://host:port` with the port moved by `NATIVE_PORT_OFFSET`.
pub fn native_url_of(url: &str) -> String {
    let t = url.trim_end_matches('/');
    match t.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => format!("{h}:{}", p.parse::<u16>().unwrap() + NATIVE_PORT_OFFSET),
        _ => t.to_string(),
    }
}

/// Agent with a connection pool big enough for `max_conns` calls in flight.
pub fn agent(timeout: Duration, max_conns: usize) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .max_idle_connections(max_conns.max(8))
        .max_idle_connections_per_host(max_conns.max(8))
        .build()
}

/// A finished HTTP round trip: wall time (send .. body fully read) and the body, or a
/// transport / HTTP-status failure description.
pub struct Raw {
    pub ns: u64,
    pub body: Result<String, String>,
}

pub fn post_action(c: &Conn, agent: &ureq::Agent, path: &str, args: &Value, request_id: Option<&str>) -> Raw {
    let body = json!({"path": path, "args": args, "format": "json"}).to_string();
    let mut req = agent
        .post(&format!("{}/api/action", c.url.trim_end_matches('/')))
        .set("Authorization", &format!("Convex {}", c.admin_key))
        .set("Content-Type", "application/json");
    if let Some(id) = request_id {
        req = req.set("Convex-Request-Id", id);
    }
    let t = Instant::now();
    let resp = req.send_string(&body);
    let out = match resp {
        Ok(r) => read_all(r).map_err(|e| format!("read body: {e}")),
        Err(ureq::Error::Status(code, r)) => {
            let b = read_all(r).unwrap_or_default();
            Err(format!("HTTP {code}: {}", short(&b)))
        },
        Err(e) => Err(format!("transport: {e}")),
    };
    Raw { ns: t.elapsed().as_nanos() as u64, body: out }
}

/// `Ok(true)` = action success, `Ok(false)` = action (user) error, `Err` = unparseable
/// or unexpected body.
pub fn action_status(text: &str) -> Result<bool, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("bad JSON ({e}): {}", short(text)))?;
    match v["status"].as_str() {
        Some("success") => Ok(true),
        Some("error") => Ok(false),
        _ => Err(format!("unexpected response: {}", short(text))),
    }
}

/// Like `action_status`, plus the user-visible error message of a failed action.
pub fn action_info(text: &str) -> Result<(bool, Option<String>), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("bad JSON ({e}): {}", short(text)))?;
    match v["status"].as_str() {
        Some("success") => Ok((true, None)),
        Some("error") => Ok((false, Some(v["errorMessage"].as_str().unwrap_or("").to_string()))),
        _ => Err(format!("unexpected response: {}", short(text))),
    }
}

/// Backend overload responses that come back as a well-formed action error. They are
/// rejections, not results: a rejected call did no work and must not count as a success
/// (nor feed latency or throughput).
pub fn is_rejection(msg: &str) -> bool {
    ["TooManyConcurrentRequests", "Too many concurrent requests", "Rate limit", "rate limited", "overloaded"]
        .iter()
        .any(|p| msg.contains(p))
}

/// One execution-completion entry from `GET /api/stream_function_logs`.
#[derive(Clone, Debug)]
pub struct Completion {
    pub request_id: String,
    /// Backend-recorded wall time of the whole function execution, seconds.
    pub execution_time: f64,
    /// Time spent inside user code (Rust runtime only), seconds.
    pub user_execution_time: Option<f64>,
    /// `parentExecutionId`: set for child UDFs (runQuery / runMutation / runAction made by an
    /// action). They share the request id of the top-level call, so only entries WITHOUT a
    /// parent are the measured call.
    pub parent_execution_id: Option<String>,
    /// Function identifier (`rust/cb:cb_001`), used as a sanity check on the root entry.
    pub identifier: String,
    /// `environment` as reported by the backend (e.g. isolate / node / rust).
    pub environment: String,
    /// Error text of the execution, if it failed.
    pub error: Option<String>,
}

impl Completion {
    pub fn is_root(&self) -> bool {
        self.parent_execution_id.is_none()
    }
}

/// `GET /api/rust_native/info`: `Ok(None)` when the endpoint answers 404 or `enabled:false`.
pub fn native_info(c: &Conn, agent: &ureq::Agent) -> Result<Option<Value>, String> {
    let r = agent
        .get(&format!("{}/api/rust_native/info", c.url.trim_end_matches('/')))
        .set("Authorization", &format!("Convex {}", c.admin_key))
        .call();
    match r {
        Ok(r) => {
            let v: Value = serde_json::from_str(&read_all(r).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            Ok((v["enabled"] == true).then_some(v))
        },
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Polls the log stream once. Returns `(completions, new_cursor)`; a read timeout (no new
/// entries; the endpoint long-polls) yields an empty list and the same cursor.
pub fn poll_logs(c: &Conn, agent: &ureq::Agent, cursor: f64) -> Result<(Vec<Completion>, f64), String> {
    let r = agent
        .get(&format!("{}/api/stream_function_logs", c.url.trim_end_matches('/')))
        .query("cursor", &format!("{cursor}"))
        .set("Authorization", &format!("Convex {}", c.admin_key))
        .call();
    let text = match r {
        Ok(r) => read_all(r).map_err(|e| e.to_string())?,
        Err(ureq::Error::Transport(t)) if t.kind() == ureq::ErrorKind::Io => return Ok((vec![], cursor)),
        Err(e) => return Err(e.to_string()),
    };
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let new_cursor = v["newCursor"].as_f64().unwrap_or(cursor);
    let mut out = vec![];
    for e in v["entries"].as_array().into_iter().flatten() {
        if e["kind"] != "Completion" {
            continue;
        }
        out.push(Completion {
            request_id: e["requestId"].as_str().unwrap_or("").to_string(),
            execution_time: e["executionTime"].as_f64().unwrap_or(f64::NAN),
            user_execution_time: e["userExecutionTime"].as_f64(),
            parent_execution_id: e["parentExecutionId"].as_str().map(str::to_string),
            identifier: e["identifier"].as_str().unwrap_or("").to_string(),
            environment: e["environment"].as_str().unwrap_or("").to_string(),
            error: e["error"].as_str().map(str::to_string),
        });
    }
    Ok((out, new_cursor))
}

/// `Response::into_string` caps bodies at 10 MB; log-stream pages and big returns exceed it.
fn read_all(r: ureq::Response) -> std::io::Result<String> {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader().take(512 << 20).read_to_string(&mut s)?;
    Ok(s)
}

/// Polls the log stream until the root entries (no `parentExecutionId`) of all `want` request ids
/// have shown up or progress stalls for 4 s. Starts at cursor 0.
pub fn resolve_roots(
    c: &Conn,
    agent: &ureq::Agent,
    mut want: std::collections::HashSet<String>,
) -> std::collections::HashMap<String, Completion> {
    let mut got = std::collections::HashMap::new();
    let (mut cursor, mut last) = (0.0, Instant::now());
    while !want.is_empty() && last.elapsed() < Duration::from_secs(4) {
        match poll_logs(c, agent, cursor) {
            Ok((comps, nc)) => {
                if !comps.is_empty() {
                    last = Instant::now();
                }
                cursor = nc;
                for e in comps {
                    if e.is_root() && want.remove(&e.request_id) {
                        got.insert(e.request_id.clone(), e);
                    }
                }
            },
            Err(_) => std::thread::sleep(Duration::from_millis(300)),
        }
    }
    got
}

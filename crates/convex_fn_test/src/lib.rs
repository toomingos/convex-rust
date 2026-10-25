//! Run `convex_fn` actions natively under `cargo test`, against an in-process
//! mock host (same idea as `convex-test` for JS). Skeleton from W0-09; W1-00
//! adds the in-memory database, scheduler, storage and fetch stubs.

/// Calls an `#[action]` with Convex-JSON args and returns the ABI result
/// envelope (`{"ok": …}` or `{"error": …}`):
/// `convex_fn_test::call!(host, base_001, json!({"seed": 1}))`.
#[macro_export]
macro_rules! call {
    ($host:expr, $f:path, $args:expr) => {
        $host.run_envelope(&$args, |ctx, a| async move { $f(&ctx, a).await })
    };
}

/// Calls an `#[http_action]` handler: `convex_fn_test::call_http!(host, handler, request)`
/// returns `Result<HttpTestResponse, Value>` (see [`TestHost::run_http`]).
#[macro_export]
macro_rules! call_http {
    ($host:expr, $f:path, $req:expr) => {
        $host.run_http($req, |ctx, r| async move { $f(&ctx, r).await })
    };
}

pub mod compare;

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    future::Future,
    rc::Rc,
};

use base64::Engine;
use convex_fn::{
    __wire::{self as wire, Event, Request},
    testing, Ctx, Value,
};
use serde_json::json;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// Number of chunks a streamed body is split into (at most).
const CHUNKS: usize = 3;
/// Default virtual clock start (ms since epoch).
const DEFAULT_NOW_MS: f64 = 1_700_000_000_000.0;

/// An error returned by a mock function handler. Plain `String`/`&str` become
/// a message-only error; use [`HostError::convex`] for a `ConvexError` with
/// data, which round-trips to `convex_fn::Error::Convex`.
#[derive(Clone, Debug, PartialEq)]
pub struct HostError {
    pub message: String,
    pub data: Option<Value>,
}

impl HostError {
    pub fn convex(data: Value) -> Self {
        let message = match &data {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        Self {
            message,
            data: Some(data),
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            data: None,
        }
    }

    fn to_json(&self) -> Value {
        match &self.data {
            Some(d) => json!({ "message": self.message, "data": d }),
            None => json!({ "message": self.message }),
        }
    }
}

impl From<String> for HostError {
    fn from(s: String) -> Self {
        Self::message(s)
    }
}

impl From<&str> for HostError {
    fn from(s: &str) -> Self {
        Self::message(s)
    }
}

/// A request seen by an `on_fetch` handler.
#[derive(Clone, Debug)]
pub struct FetchStubRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A response produced by an `on_fetch` handler.
#[derive(Clone, Debug)]
pub struct FetchStubResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// When set, the body is delivered as exactly these chunks instead of
    /// `body` (see [`FetchStubResponse::chunked`]).
    pub chunks: Option<Vec<Vec<u8>>>,
    /// When set, the stream ends with this error instead of `end`
    /// (see [`FetchStubResponse::fail_after`]).
    pub stream_error: Option<String>,
    /// When set, the fetch task never completes (see [`FetchStubResponse::never`]).
    pub hang: bool,
}

impl FetchStubResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![],
            body: body.into(),
            chunks: None,
            stream_error: None,
            hang: false,
        }
    }

    /// A response whose body arrives as exactly these `chunk` events.
    pub fn chunked(status: u16, chunks: impl IntoIterator<Item = impl Into<Vec<u8>>>) -> Self {
        let chunks: Vec<Vec<u8>> = chunks.into_iter().map(Into::into).collect();
        Self {
            body: chunks.concat(),
            chunks: Some(chunks),
            ..Self::new(status, vec![])
        }
    }

    /// A fetch that never completes (a hung connection): only a timeout or a
    /// dropped future ends it.
    pub fn never() -> Self {
        Self {
            hang: true,
            ..Self::new(0, vec![])
        }
    }

    /// Ends the body stream with an `error` chunk (after the chunks already
    /// configured) instead of a normal `end`.
    pub fn fail_after(mut self, message: impl Into<String>) -> Self {
        self.stream_error = Some(message.into());
        self
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

/// The streamed response an HTTP action produced (`httpStart` / `httpWrite` / `httpEnd`).
struct HttpOut {
    status: u16,
    headers: Vec<(String, String)>,
    chunks: Vec<Vec<u8>>,
    ended: bool,
}

/// A request for [`TestHost::run_http`].
#[derive(Clone, Debug, Default)]
pub struct HttpTestRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Body delivered as stream chunks (none = no body stream).
    pub body: Option<Vec<Vec<u8>>>,
}

impl HttpTestRequest {
    pub fn new(method: &str, url: &str) -> Self {
        Self { method: method.into(), url: url.into(), ..Default::default() }
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    /// Sets the body, split into the given chunks.
    pub fn body_chunks(mut self, chunks: impl IntoIterator<Item = impl Into<Vec<u8>>>) -> Self {
        self.body = Some(chunks.into_iter().map(Into::into).collect());
        self
    }
}

/// What an HTTP action answered.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpTestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// The whole body.
    pub body: Vec<u8>,
    /// The writes of a streamed body (empty when buffered).
    pub chunks: Vec<Vec<u8>>,
    pub streamed: bool,
}

/// A recorded function call made through `ctx.run_query/mutation/action`.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// `"query"`, `"mutation"` or `"action"`.
    pub kind: String,
    pub name: String,
    pub args: Value,
}

/// A recorded `ctx.schedule_*` call.
#[derive(Clone, Debug, PartialEq)]
pub struct Scheduled {
    pub id: String,
    pub name: String,
    /// The `ts` argument exactly as sent on the wire (seconds since epoch).
    pub ts: f64,
    pub args: Value,
    pub cancelled: bool,
}

type SyscallHandler = Box<dyn FnMut(&Value) -> Result<Value, String>>;
type FnHandler = Box<dyn FnMut(&Value) -> Result<Value, HostError>>;
type FetchHandler = Box<dyn FnMut(&FetchStubRequest) -> FetchStubResponse>;

struct StoredFile {
    content_type: Option<String>,
    data: Vec<u8>,
}

struct Inner {
    syscalls: HashMap<String, SyscallHandler>,
    functions: HashMap<(&'static str, String), FnHandler>,
    fetch: Option<FetchHandler>,
    next_id: i64,
    next_uuid: u64,
    ready: VecDeque<Vec<u8>>,
    reverse: bool,
    logs: Vec<(i32, String)>,
    calls: Vec<Call>,
    scheduled: Vec<Scheduled>,
    storage: HashMap<String, StoredFile>,
    identity: Value,
    sleep_total_ms: u64,
    now_ms: f64,
    component_args: HashMap<String, Value>,
    http_out: Option<HttpOut>,
    /// `net/caps` answers (cancel + bodyRef) when set.
    net_caps: bool,
    /// Task ids passed to `net/cancel`.
    cancelled: Vec<i64>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            syscalls: HashMap::new(),
            functions: HashMap::new(),
            fetch: None,
            next_id: 0,
            next_uuid: 0,
            ready: VecDeque::new(),
            reverse: false,
            logs: vec![],
            calls: vec![],
            scheduled: vec![],
            storage: HashMap::new(),
            identity: Value::Null,
            sleep_total_ms: 0,
            now_ms: DEFAULT_NOW_MS,
            component_args: HashMap::new(),
            http_out: None,
            net_caps: false,
            cancelled: vec![],
        }
    }
}

impl Inner {
    fn new_stream_id(&mut self) -> String {
        self.next_uuid += 1;
        format!("00000000-0000-4000-8000-{:012x}", self.next_uuid)
    }

    fn push(&mut self, ev: Event) {
        self.ready.push_back(ev.to_json().to_string().into_bytes());
    }

    /// Queues `chunk` events for `data` (split into a few pieces) and `end`.
    fn push_stream(&mut self, id: &str, data: &[u8]) {
        if !data.is_empty() {
            let size = data.len().div_ceil(CHUNKS);
            for part in data.chunks(size) {
                self.push(Event::chunk_data(id, part));
            }
        }
        self.push(Event::chunk_end(id));
    }

    fn put(&mut self, data: Vec<u8>, content_type: Option<String>) -> String {
        let id = format!("kg2test{:08}", self.storage.len() + 1);
        self.storage
            .insert(id.clone(), StoredFile { content_type, data });
        id
    }

    fn handle_syscall(&mut self, name: &str, args: &Value) -> Result<Value, HostError> {
        if let Some(h) = self.syscalls.get_mut(name) {
            return h(args).map_err(HostError::message);
        }
        let kind = match name {
            "1.0/actions/query" => Some("query"),
            "1.0/actions/mutation" => Some("mutation"),
            "1.0/actions/action" => Some("action"),
            _ => None,
        };
        if let Some(kind) = kind {
            let fname = address_of(args);
            let fargs = args["args"].clone();
            self.calls.push(Call {
                kind: kind.to_string(),
                name: fname.clone(),
                args: fargs.clone(),
            });
            return match self.functions.get_mut(&(kind, fname.clone())) {
                Some(h) => h(&fargs),
                None => Err(HostError::message(format!(
                    "convex_fn_test: no {kind} registered for {fname}"
                ))),
            };
        }
        match name {
            "1.0/actions/schedule" => {
                let id = format!("scheduled_{}", self.scheduled.len() + 1);
                self.scheduled.push(Scheduled {
                    id: id.clone(),
                    name: address_of(args),
                    ts: args["ts"].as_f64().unwrap_or_default(),
                    args: args["args"].clone(),
                    cancelled: false,
                });
                Ok(Value::String(id))
            }
            "1.0/actions/cancel_job" => {
                let id = args["id"].as_str().unwrap_or_default();
                match self.scheduled.iter_mut().find(|s| s.id == id) {
                    Some(s) => {
                        s.cancelled = true;
                        Ok(Value::Null)
                    }
                    None => Err(HostError::message(format!(
                        "convex_fn_test: unknown scheduled id {id}"
                    ))),
                }
            }
            "1.0/getUserIdentity" => Ok(self.identity.clone()),
            _ => Err(HostError::message(format!(
                "convex_fn_test: no handler for syscall {name}"
            ))),
        }
    }

    fn handle_task(&mut self, id: i64, req: &Value) {
        let result: Result<Value, HostError> = match Request::parse(req) {
            Request::Syscall { name, args } => self.handle_syscall(&name, &args),
            Request::Sleep { ms } => {
                self.sleep_total_ms += ms;
                self.now_ms += ms as f64;
                Ok(Value::Null)
            }
            Request::StorageStore { content_type, body } => match decode_body(&body) {
                Ok(data) => Ok(Value::String(self.put(data, content_type))),
                Err(e) => Err(HostError::message(e)),
            },
            Request::StorageGet { storage_id } => match self.storage.get(&storage_id) {
                None => Ok(Value::Null),
                Some(f) => {
                    let (data, ct) = (f.data.clone(), f.content_type.clone());
                    let stream = self.new_stream_id();
                    let head = wire::storage_file_head(&stream, data.len(), ct.as_deref());
                    self.push(Event::done_ok(id, head));
                    self.push_stream(&stream, &data);
                    return;
                }
            },
            Request::Fetch {
                url,
                method,
                headers,
                body,
            } => {
                let body = decode_body(&body).unwrap_or_default();
                let fr = FetchStubRequest {
                    url,
                    method,
                    headers,
                    body,
                };
                match self.fetch.as_mut() {
                    None => Err(HostError::message(format!(
                        "convex_fn_test: no fetch handler registered (fetch {})",
                        fr.url
                    ))),
                    Some(h) => {
                        let resp = h(&fr);
                        if resp.hang {
                            return;
                        }
                        let has_body = !resp.body.is_empty()
                            || resp.chunks.as_ref().is_some_and(|c| !c.is_empty())
                            || resp.stream_error.is_some();
                        let stream = has_body.then(|| self.new_stream_id());
                        let head = wire::fetch_head(
                            resp.status,
                            status_text(resp.status),
                            &resp.headers,
                            &fr.url,
                            stream.as_deref(),
                        );
                        self.push(Event::done_ok(id, head));
                        if let Some(s) = stream {
                            if resp.chunks.is_some() || resp.stream_error.is_some() {
                                for part in resp.chunks.as_deref().unwrap_or_default() {
                                    self.push(Event::chunk_data(&s, part));
                                }
                                match &resp.stream_error {
                                    Some(e) => self.push(Event::chunk_error(&s, e)),
                                    None => self.push(Event::chunk_end(&s)),
                                }
                            } else {
                                self.push_stream(&s, &resp.body);
                            }
                        }
                        return;
                    }
                }
            }
            Request::HttpStart { status, headers } => {
                if self.http_out.is_some() {
                    Err(HostError::message("httpStart called twice"))
                } else {
                    self.http_out = Some(HttpOut {
                        status,
                        headers,
                        chunks: vec![],
                        ended: false,
                    });
                    Ok(Value::Null)
                }
            }
            Request::HttpWrite { body } => match (self.http_out.as_mut(), B64.decode(body)) {
                (Some(out), Ok(data)) if !out.ended => {
                    out.chunks.push(data);
                    Ok(Value::Null)
                }
                (Some(_), Ok(_)) => Err(HostError::message("httpWrite after httpEnd")),
                (None, _) => Err(HostError::message("httpWrite before httpStart")),
                (_, Err(e)) => Err(HostError::message(format!("invalid base64 body: {e}"))),
            },
            Request::HttpEnd => match self.http_out.as_mut() {
                Some(out) => {
                    out.ended = true;
                    Ok(Value::Null)
                }
                None => Err(HostError::message("httpEnd before httpStart")),
            },
            Request::Other(other) => Err(HostError::message(format!(
                "convex_fn_test: task type {other:?} not mocked"
            ))),
        };
        self.push(match result {
            Ok(v) => Event::done_ok(id, v),
            Err(e) => Event::done_err(id, e.to_json()),
        });
    }
}

/// Decodes a request body. A `{ptr,len}` reference is a real address here: the
/// guest is native code in this process and its buffer is alive during submit.
fn decode_body(body: &wire::WireBody) -> Result<Vec<u8>, String> {
    match body {
        wire::WireBody::None | wire::WireBody::Stream => Ok(vec![]),
        wire::WireBody::Base64(s) => B64
            .decode(s)
            .map_err(|e| format!("invalid base64 body: {e}")),
        wire::WireBody::Ref { ptr, len } => {
            // SAFETY: produced by `convex_fn` from a live slice (see above).
            Ok(unsafe { std::slice::from_raw_parts(*ptr as *const u8, *len) }.to_vec())
        }
    }
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "",
    }
}

/// The function address of a syscall's args: `name`, `functionHandle` or `reference`.
fn address_of(args: &Value) -> String {
    ["name", "functionHandle", "reference"]
        .iter()
        .find_map(|k| args[*k].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A mock Convex host. Register handlers, then `run` an action.
#[derive(Clone, Default)]
pub struct TestHost {
    inner: Rc<RefCell<Inner>>,
}

struct Installed(Rc<RefCell<Inner>>);

impl testing::Host for Installed {
    fn task_submit(&mut self, request: &[u8]) -> i64 {
        let Ok(req) = serde_json::from_slice::<Value>(request) else {
            return -1;
        };
        let mut inner = self.0.borrow_mut();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.handle_task(id, &req);
        id
    }

    fn task_poll(&mut self, _block: bool) -> Option<Vec<u8>> {
        let mut inner = self.0.borrow_mut();
        if inner.reverse {
            inner.ready.pop_back()
        } else {
            inner.ready.pop_front()
        }
    }

    fn sync_syscall(&mut self, op: &str, args: &[u8]) -> Vec<u8> {
        let mut inner = self.0.borrow_mut();
        let out = match op {
            "net/caps" if inner.net_caps => {
                json!({ "ok": { "version": 1, "ops": ["cancel", "bodyRef"], "maxBodyRef": 64 << 20 } })
            }
            "net/cancel" if inner.net_caps => {
                let args: Value = serde_json::from_slice(args).unwrap_or(Value::Null);
                if let Some(id) = args["taskId"].as_i64() {
                    inner.cancelled.push(id);
                }
                json!({ "ok": true })
            }
            "now" => json!({ "ok": inner.now_ms }),
            "componentArgument" => {
                let args: Value = serde_json::from_slice(args).unwrap_or(Value::Null);
                let name = args["name"].as_str().unwrap_or_default();
                json!({ "ok": inner.component_args.get(name).cloned().unwrap_or(Value::Null) })
            }
            _ => {
                json!({ "error": { "message": format!("convex_fn_test: sync op {op} not mocked") } })
            }
        };
        out.to_string().into_bytes()
    }

    fn log(&mut self, level: i32, msg: &str) {
        self.0.borrow_mut().logs.push((level, msg.to_string()));
    }
}

impl TestHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle an async syscall such as `"1.0/actions/query"`. The handler
    /// receives the syscall args JSON (`{"name": …, "args": …}`). Takes
    /// precedence over the built-in mocks.
    pub fn on_syscall(
        self,
        name: &str,
        handler: impl FnMut(&Value) -> Result<Value, String> + 'static,
    ) -> Self {
        self.inner
            .borrow_mut()
            .syscalls
            .insert(name.to_string(), Box::new(handler));
        self
    }

    fn register<E: Into<HostError>>(
        self,
        kind: &'static str,
        name: &str,
        mut handler: impl FnMut(&Value) -> Result<Value, E> + 'static,
    ) -> Self {
        self.inner.borrow_mut().functions.insert(
            (kind, name.to_string()),
            Box::new(move |a| handler(a).map_err(Into::into)),
        );
        self
    }

    /// Register a query, e.g. `on_query("messages:list", |args| Ok::<_, String>(json!([])))`.
    /// The error type may be `String`, `&str` or [`HostError`].
    pub fn on_query<E: Into<HostError>>(
        self,
        name: &str,
        handler: impl FnMut(&Value) -> Result<Value, E> + 'static,
    ) -> Self {
        self.register("query", name, handler)
    }

    pub fn on_mutation<E: Into<HostError>>(
        self,
        name: &str,
        handler: impl FnMut(&Value) -> Result<Value, E> + 'static,
    ) -> Self {
        self.register("mutation", name, handler)
    }

    pub fn on_action<E: Into<HostError>>(
        self,
        name: &str,
        handler: impl FnMut(&Value) -> Result<Value, E> + 'static,
    ) -> Self {
        self.register("action", name, handler)
    }

    /// Stub `fetch`. Without a stub, fetches fail the task.
    pub fn on_fetch(
        self,
        handler: impl FnMut(&FetchStubRequest) -> FetchStubResponse + 'static,
    ) -> Self {
        self.inner.borrow_mut().fetch = Some(Box::new(handler));
        self
    }

    /// Value returned by `ctx.user_identity()` (default: no identity).
    pub fn with_identity(self, identity: Value) -> Self {
        self.inner.borrow_mut().identity = identity;
        self
    }

    /// Starting value of the virtual clock (sync op `now`), in ms.
    pub fn with_now_ms(self, now_ms: f64) -> Self {
        self.inner.borrow_mut().now_ms = now_ms;
        self
    }

    /// Value returned by sync op `componentArgument` for `name`.
    pub fn with_component_arg(self, name: &str, value: Value) -> Self {
        self.inner
            .borrow_mut()
            .component_args
            .insert(name.to_string(), value);
        self
    }

    /// Deliver queued events newest-first (`done` events out of submission
    /// order, stream chunks before their `done`), to stress concurrency.
    pub fn with_reverse_delivery(self) -> Self {
        self.inner.borrow_mut().reverse = true;
        self
    }

    /// Advertise `net/caps` (cancel on drop, `{ptr,len}` bodies); without
    /// this the guest uses the older inline-base64, no-cancel behaviour.
    pub fn with_net_caps(self) -> Self {
        self.inner.borrow_mut().net_caps = true;
        self
    }

    /// Task ids the guest cancelled (`net/cancel`), in order.
    pub fn cancelled_tasks(&self) -> Vec<i64> {
        self.inner.borrow().cancelled.clone()
    }

    /// Put a file in mock storage and return its storage id.
    pub fn storage_put(&self, data: impl Into<Vec<u8>>, content_type: Option<&str>) -> String {
        self.inner
            .borrow_mut()
            .put(data.into(), content_type.map(str::to_string))
    }

    /// Bytes of a stored file, if present.
    pub fn storage_read(&self, id: &str) -> Option<Vec<u8>> {
        self.inner.borrow().storage.get(id).map(|f| f.data.clone())
    }

    /// Every `run_query/mutation/action` call, in submission order.
    pub fn calls(&self) -> Vec<Call> {
        self.inner.borrow().calls.clone()
    }

    /// Every scheduled function, in order.
    pub fn scheduled(&self) -> Vec<Scheduled> {
        self.inner.borrow().scheduled.clone()
    }

    /// Total virtual milliseconds slept via `ctx.sleep`.
    pub fn slept_ms(&self) -> u64 {
        self.inner.borrow().sleep_total_ms
    }

    /// Current virtual clock (ms since epoch).
    pub fn now_ms(&self) -> f64 {
        self.inner.borrow().now_ms
    }

    /// Runs an action body to completion with this host installed.
    pub fn run<F, Fut, R>(&self, f: F) -> R
    where
        F: FnOnce(Ctx) -> Fut,
        Fut: Future<Output = R>,
    {
        let prev = testing::install_host(Box::new(Installed(self.inner.clone())));
        convex_fn::__rt::install_logging();
        let ctx = convex_fn::__rt::test_ctx();
        let out = convex_fn::block_on(f(ctx));
        match prev {
            Some(p) => {
                testing::install_host(p);
            }
            None => {
                testing::uninstall_host();
            }
        }
        out
    }

    /// Runs an action from raw Convex-JSON args and returns the ABI result
    /// envelope as JSON, exactly as the wasm export would.
    pub fn run_envelope<A, R, F, Fut>(&self, args: &Value, f: F) -> Value
    where
        A: serde::de::DeserializeOwned,
        R: serde::Serialize,
        F: FnOnce(Ctx, A) -> Fut,
        Fut: Future<Output = convex_fn::Result<R>>,
    {
        let prev = testing::install_host(Box::new(Installed(self.inner.clone())));
        let out = convex_fn::__rt::run_to_envelope(args.to_string().as_bytes(), f);
        match prev {
            Some(p) => {
                testing::install_host(p);
            }
            None => {
                testing::uninstall_host();
            }
        }
        serde_json::from_str(&out).expect("envelope is JSON")
    }

    /// Runs an `#[http_action]` handler against `req` and returns what the host would have
    /// answered the client, or the error JSON of the result envelope.
    /// `convex_fn_test::call_http!(host, handler, request)` wraps a handler fn.
    pub fn run_http<F, Fut>(&self, req: HttpTestRequest, f: F) -> Result<HttpTestResponse, Value>
    where
        F: FnOnce(Ctx, convex_fn::HttpRequest) -> Fut + 'static,
        Fut: Future<Output = convex_fn::Result<convex_fn::HttpResponse>> + 'static,
    {
        let stream = req.body.as_ref().map(|chunks| {
            let mut inner = self.inner.borrow_mut();
            let id = inner.new_stream_id();
            for c in chunks {
                inner.push(Event::chunk_data(&id, c));
            }
            inner.push(Event::chunk_end(&id));
            id
        });
        self.inner.borrow_mut().http_out = None;
        let args = json!({
            "method": req.method,
            "url": req.url,
            "headers": req.headers,
            "bodyStreamId": stream,
        });
        let envelope = self.run_envelope(&args, convex_fn::__rt::http_adapter(f));
        let Some(ok) = envelope.get("ok") else {
            return Err(envelope["error"].clone());
        };
        if ok["streamed"] == Value::Bool(true) {
            let out = self.inner.borrow_mut().http_out.take().expect("streamed without httpStart");
            assert!(out.ended, "streamed response never ended");
            return Ok(HttpTestResponse {
                status: out.status,
                headers: out.headers,
                body: out.chunks.concat(),
                chunks: out.chunks,
                streamed: true,
            });
        }
        let body = match ok["body"].as_str() {
            Some(b) => B64.decode(b).expect("body is base64"),
            None => vec![],
        };
        Ok(HttpTestResponse {
            status: ok["status"].as_u64().expect("status") as u16,
            headers: serde_json::from_value(ok["headers"].clone()).expect("headers"),
            body,
            chunks: vec![],
            streamed: false,
        })
    }

    /// Log lines written by the action, as `(level, message)`.
    pub fn logs(&self) -> Vec<(i32, String)> {
        self.inner.borrow().logs.clone()
    }
}

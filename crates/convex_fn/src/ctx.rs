//! `Ctx`: typed wrappers over the existing V8 action task names (D-008).

use std::future::Future;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

use crate::{
    error::Error,
    executor::{abandon_stream, submit, submit_cancellable, NextChunk, StreamFuture},
    function_ref::{address_json, FunctionAddress, FunctionTarget},
    net, sys,
    wire::{self, BodyRef},
    Result,
};

/// The action context, equivalent to `ctx` in a JS action.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    _private: (),
}

#[derive(Clone, Debug)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Redirect handling; follows redirects like Node's `fetch` by default.
    pub redirect: Redirect,
}

/// `RequestInit.redirect`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Redirect {
    /// Follow up to 20 redirects (the default).
    #[default]
    Follow,
    /// Return the 3xx response as is.
    Manual,
}

/// Maximum redirect hops, as in the fetch standard.
const MAX_REDIRECTS: usize = 20;

/// Headers dropped when a redirect rewrites the request to a body-less GET.
const BODY_HEADERS: [&str; 5] = [
    "content-type",
    "content-length",
    "content-encoding",
    "content-language",
    "content-location",
];

/// Headers dropped when a redirect crosses origins.
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "cookie", "proxy-authorization"];

impl FetchRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "GET".into(),
            headers: vec![],
            body: None,
            redirect: Redirect::Follow,
        }
    }

    pub fn post(url: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            url: url.into(),
            method: "POST".into(),
            headers: vec![],
            body: Some(body.into()),
            redirect: Redirect::Follow,
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the HTTP method (for example `PUT` before [`Ctx::fetch_upload`]).
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// `redirect: "manual"` returns 3xx responses instead of following them.
    pub fn redirect(mut self, redirect: Redirect) -> Self {
        self.redirect = redirect;
        self
    }
}

/// A source of request-body chunks for [`Ctx::fetch_upload`] /
/// [`Ctx::fetch_stream_upload`]: `Ok(None)` ends the body, `Err` fails the
/// request. A [`FetchStream`] is one (proxying a download into an upload);
/// [`Chunks`] wraps an iterator.
pub trait ChunkSource {
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>>>;
}

impl ChunkSource for FetchStream {
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>>> {
        self.chunk()
    }
}

/// A [`ChunkSource`] over an iterator of byte chunks.
pub struct Chunks<I>(pub I);

impl<I: Iterator<Item = Vec<u8>>> ChunkSource for Chunks<I> {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(self.0.next())
    }
}

/// Slices one big body into [`net::STREAM_SLICE`] chunks.
struct SliceSource<'a>(&'a [u8]);

impl ChunkSource for SliceSource<'_> {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.0.is_empty() {
            return Ok(None);
        }
        let n = self.0.len().min(net::STREAM_SLICE);
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(Some(head.to_vec()))
    }
}

/// Source for the plain (non-uploading) fetch path.
struct NoUpload;

impl ChunkSource for NoUpload {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

#[derive(Clone, Debug)]
pub struct FetchResponse {
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl FetchResponse {
    pub fn text(&self) -> Result<String> {
        Ok(String::from_utf8(self.body.clone())?)
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

/// A fetch response whose body is read incrementally, chunk by chunk, as the
/// host delivers `chunk` events (contracts/ABI.md "Events"). Created by
/// [`Ctx::fetch_stream`]. Dropping it early discards the rest of the body.
#[derive(Debug)]
pub struct FetchStream {
    /// HTTP status code.
    pub status: u16,
    /// HTTP status text.
    pub status_text: String,
    /// Final URL of the response.
    pub url: String,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    stream_id: Option<String>,
}

impl FetchStream {
    /// The next body chunk, `Ok(None)` once the body ended, or `Err` if the
    /// stream failed (chunks received before the failure are returned first).
    /// After `None` or `Err`, further calls return `Ok(None)`.
    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(id) = self.stream_id.as_deref() else {
            return Ok(None);
        };
        let r = NextChunk { stream_id: id }.await;
        if !matches!(r, Ok(Some(_))) {
            self.stream_id = None;
        }
        r
    }

    /// Reads the remaining body into one buffer.
    pub async fn bytes(&mut self) -> Result<Vec<u8>> {
        let mut out = vec![];
        while let Some(c) = self.chunk().await? {
            out.extend_from_slice(&c);
        }
        Ok(out)
    }
}

impl Drop for FetchStream {
    fn drop(&mut self) {
        if let Some(id) = self.stream_id.take() {
            abandon_stream(&id);
        }
    }
}

/// Arguments of `ctx.vectorSearch`.
#[derive(Clone, Debug, Default)]
pub struct VectorSearchQuery {
    pub vector: Vec<f64>,
    /// 1..=256; the backend default (10) if `None`.
    pub limit: Option<u32>,
    pub filter: Option<VectorFilter>,
}

/// A vector search filter: `q.eq(field, value)` combined with `q.or(...)`.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorFilter {
    Eq(String, Value),
    Or(Vec<VectorFilter>),
}

impl VectorFilter {
    pub fn eq(field: impl Into<String>, value: impl Serialize) -> Self {
        VectorFilter::Eq(
            field.into(),
            serde_json::to_value(value).unwrap_or(Value::Null),
        )
    }

    pub fn or(filters: impl IntoIterator<Item = VectorFilter>) -> Self {
        VectorFilter::Or(filters.into_iter().collect())
    }

    /// The expression JSON the runtime sends: `{"$eq": [{"$field": f}, v]}` / `{"$or": [...]}`.
    pub fn to_json(&self) -> Value {
        match self {
            VectorFilter::Eq(f, v) => json!({ "$eq": [{ "$field": f }, v] }),
            VectorFilter::Or(fs) => {
                json!({ "$or": fs.iter().map(Self::to_json).collect::<Vec<_>>() })
            }
        }
    }
}

/// One `ctx.vectorSearch` result.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct VectorHit {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_score")]
    pub score: f64,
}

#[derive(Clone, Debug)]
pub struct StorageFile {
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

/// Metadata of a stored file (`ctx.storage.getMetadata`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageMetadata {
    pub storage_id: String,
    pub sha256: String,
    pub size: u64,
    pub content_type: Option<String>,
}

/// `ctx.storage`: see [`Ctx::storage`].
#[derive(Clone, Copy, Debug)]
pub struct Storage<'a> {
    ctx: &'a Ctx,
}

impl Storage<'_> {
    /// `ctx.storage.store(blob)`.
    pub async fn store(&self, data: &[u8], content_type: Option<&str>) -> Result<String> {
        self.ctx.storage_store(data, content_type).await
    }

    /// `ctx.storage.get(id)`; `None` if the file does not exist.
    pub async fn get(&self, id: &str) -> Result<Option<StorageFile>> {
        self.ctx.storage_get(id).await
    }

    /// `ctx.storage.getUrl(id)`; `None` if the file does not exist.
    pub async fn get_url(&self, id: &str) -> Result<Option<String>> {
        let v = self
            .ctx
            .syscall("1.0/storageGetUrl", json!({ "storageId": id }))
            .await?;
        Ok(v.as_str().map(str::to_string))
    }

    /// `ctx.storage.getMetadata(id)`; `None` if the file does not exist.
    pub async fn get_metadata(&self, id: &str) -> Result<Option<StorageMetadata>> {
        let v = self
            .ctx
            .syscall("1.0/storageGetMetadata", json!({ "storageId": id }))
            .await?;
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(StorageMetadata {
            storage_id: str_field(&v, "storageId"),
            sha256: str_field(&v, "sha256"),
            size: v.get("size").and_then(Value::as_u64).unwrap_or(0),
            content_type: v
                .get("contentType")
                .and_then(Value::as_str)
                .map(str::to_string),
        }))
    }

    /// `ctx.storage.generateUploadUrl()`: a short-lived URL a client can POST a file to.
    pub async fn generate_upload_url(&self) -> Result<String> {
        let v = self
            .ctx
            .syscall("1.0/storageGenerateUploadUrl", json!({}))
            .await?;
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::msg("storage.generateUploadUrl returned no url"))
    }

    /// `ctx.storage.delete(id)`.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.ctx
            .syscall("1.0/storageDelete", json!({ "storageId": id }))
            .await?;
        Ok(())
    }
}

/// `ctx.scheduler`: see [`Ctx::scheduler`].
#[derive(Clone, Copy, Debug)]
pub struct Scheduler<'a> {
    ctx: &'a Ctx,
}

impl Scheduler<'_> {
    /// `ctx.scheduler.runAfter(ms, fn, args)`; `name` is a path, a typed
    /// reference, a [`FunctionHandle`](crate::FunctionHandle) or a component reference.
    pub async fn run_after<A: Serialize>(
        &self,
        delay_ms: f64,
        name: impl FunctionAddress,
        args: A,
    ) -> Result<String> {
        self.ctx.schedule_after(delay_ms, name, args).await
    }

    /// `ctx.scheduler.runAt(timestamp_ms, fn, args)`.
    pub async fn run_at<A: Serialize>(
        &self,
        ts_ms: f64,
        name: impl FunctionAddress,
        args: A,
    ) -> Result<String> {
        self.ctx.schedule_at(ts_ms, name, args).await
    }

    /// `ctx.scheduler.cancel(id)`.
    pub async fn cancel(&self, id: &str) -> Result<()> {
        self.ctx
            .syscall("1.0/actions/cancel_job", json!({ "id": id }))
            .await?;
        Ok(())
    }
}

impl Ctx {
    /// `ctx.storage`.
    pub fn storage(&self) -> Storage<'_> {
        Storage { ctx: self }
    }

    /// `ctx.scheduler`.
    pub fn scheduler(&self) -> Scheduler<'_> {
        Scheduler { ctx: self }
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Runs any async syscall by name with the exact JSON the V8 runtime sends.
    pub async fn syscall(&self, name: &str, args: Value) -> Result<Value> {
        submit(&wire::syscall_request(name, args)).await
    }

    /// `ctx.runQuery(api.<path>, args)`. `name` is a function path such as
    /// `"messages:list"` or `"rust/cpu:cpu_001"`; `args` is a Convex JSON object.
    pub async fn run_query<A: Serialize, R: DeserializeOwned>(
        &self,
        name: impl FunctionTarget<A, R>,
        args: A,
    ) -> Result<R> {
        self.call("1.0/actions/query", &name, args).await
    }

    /// `ctx.runMutation(api.<path>, args)`.
    pub async fn run_mutation<A: Serialize, R: DeserializeOwned>(
        &self,
        name: impl FunctionTarget<A, R>,
        args: A,
    ) -> Result<R> {
        self.call("1.0/actions/mutation", &name, args).await
    }

    /// `ctx.runAction(api.<path>, args)`; the target may be in any runtime.
    pub async fn run_action<A: Serialize, R: DeserializeOwned>(
        &self,
        name: impl FunctionTarget<A, R>,
        args: A,
    ) -> Result<R> {
        self.call("1.0/actions/action", &name, args).await
    }

    async fn call<A: Serialize, R: DeserializeOwned>(
        &self,
        syscall: &str,
        target: &impl FunctionAddress,
        args: A,
    ) -> Result<R> {
        let mut req = address_json(target);
        req["args"] = serde_json::to_value(args)?;
        let v = self.syscall(syscall, req).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Use [`Ctx::scheduler`] (`run_after`) instead.
    #[doc(hidden)]
    pub async fn schedule_after<A: Serialize>(
        &self,
        delay_ms: f64,
        name: impl FunctionAddress,
        args: A,
    ) -> Result<String> {
        let now = self.now_ms();
        self.schedule_at(now + delay_ms, name, args).await
    }

    /// Use [`Ctx::scheduler`] (`run_at`) instead.
    #[doc(hidden)]
    pub async fn schedule_at<A: Serialize>(
        &self,
        ts_ms: f64,
        name: impl FunctionAddress,
        args: A,
    ) -> Result<String> {
        let mut req = address_json(&name);
        req["ts"] = json!(ts_ms / 1000.0);
        req["args"] = serde_json::to_value(args)?;
        let v = self.syscall("1.0/actions/schedule", req).await?;
        Ok(serde_json::from_value(v)?)
    }

    /// `ctx.vectorSearch(table, index, query)`: nearest documents with their scores.
    pub async fn vector_search(
        &self,
        table: &str,
        index: &str,
        query: VectorSearchQuery,
    ) -> Result<Vec<VectorHit>> {
        let mut q = json!({
            "indexName": format!("{table}.{index}"),
            "vector": query.vector,
            "expressions": query.filter.map_or(Value::Null, |f| f.to_json()),
        });
        if let Some(limit) = query.limit {
            q["limit"] = json!(limit);
        }
        let v = self
            .syscall("1.0/actions/vectorSearch", json!({ "query": q }))
            .await?;
        let results = v.get("results").cloned().unwrap_or(json!([]));
        Ok(serde_json::from_value(results)?)
    }

    /// The value of a component argument (`componentArgument` sync op); `None`
    /// if unset. Only meaningful inside a component.
    pub fn component_argument(&self, name: &str) -> Result<Option<Value>> {
        let v = self.sync_syscall("componentArgument", json!({ "name": name }))?;
        Ok((!v.is_null()).then_some(v))
    }

    /// `ctx.auth.getUserIdentity()`.
    pub async fn user_identity(&self) -> Result<Option<Value>> {
        let v = self.syscall("1.0/getUserIdentity", json!({})).await?;
        Ok((!v.is_null()).then_some(v))
    }

    /// `fetch(url, init)`; reads the whole body.
    ///
    /// Dropping the future (a timeout, [`race`](crate::race)) aborts the
    /// request on hosts that support `net/cancel`.
    pub async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse> {
        let head = self.fetch_head(req, None::<NoUpload>).await?;
        Self::read_response(head).await
    }

    /// `fetch(url, init)` with a streaming body: resolves as soon as the
    /// response head arrives; read the body with [`FetchStream::chunk`].
    pub async fn fetch_stream(&self, req: FetchRequest) -> Result<FetchStream> {
        let head = self.fetch_head(req, None::<NoUpload>).await?;
        Self::stream_response(head)
    }

    /// `fetch(url, { method, headers, body: <stream> })`: the request body is
    /// pulled from `source` chunk by chunk, so it never has to fit in memory.
    /// `req.body` must be `None`. On hosts without streamed request bodies the
    /// chunks are buffered and sent as one body. A streamed body cannot be
    /// replayed, so a 307/308 redirect (or any redirect that keeps the body)
    /// is an error.
    pub async fn fetch_upload<S: ChunkSource>(
        &self,
        req: FetchRequest,
        source: S,
    ) -> Result<FetchResponse> {
        let head = self.fetch_head(req, Some(source)).await?;
        Self::read_response(head).await
    }

    /// [`Ctx::fetch_upload`] with a streamed response body.
    pub async fn fetch_stream_upload<S: ChunkSource>(
        &self,
        req: FetchRequest,
        source: S,
    ) -> Result<FetchStream> {
        let head = self.fetch_head(req, Some(source)).await?;
        Self::stream_response(head)
    }

    async fn read_response(head: Value) -> Result<FetchResponse> {
        let head = FetchHead::parse(&head)?;
        let body = read_body(head.stream_id.as_deref()).await?;
        Ok(FetchResponse {
            status: head.status,
            status_text: head.status_text,
            url: head.url,
            headers: head.headers,
            body,
        })
    }

    fn stream_response(head: Value) -> Result<FetchStream> {
        let head = FetchHead::parse(&head)?;
        Ok(FetchStream {
            status: head.status,
            status_text: head.status_text,
            url: head.url,
            headers: head.headers,
            stream_id: head.stream_id,
        })
    }

    /// Submits the fetch and follows redirects (unless `Manual`); returns the
    /// raw response head. `upload` supplies a streamed request body.
    async fn fetch_head<S: ChunkSource>(
        &self,
        req: FetchRequest,
        mut upload: Option<S>,
    ) -> Result<Value> {
        if upload.is_some() && req.body.is_some() {
            return Err(Error::msg(
                "fetch_upload: `body` must be unset, the chunk source is the body",
            ));
        }
        let FetchRequest {
            url,
            method,
            headers,
            body,
            redirect,
        } = req;
        let mut cur = RedirectState {
            url,
            method,
            headers,
            body,
        };
        let mut hops = 0;
        // Probe `net/*` once (cancel on drop, body references, streamed bodies).
        let caps = net::caps();
        loop {
            let mut streamed = false;
            let mut head = match upload.take() {
                Some(mut src) if caps.request_stream => {
                    streamed = true;
                    self.send_streamed(&cur, &mut src).await?
                }
                Some(mut src) => {
                    // Old host: buffer the chunks, send one body.
                    let mut all = vec![];
                    while let Some(c) = src.next_chunk().await? {
                        all.extend_from_slice(&c);
                    }
                    cur.body = Some(all);
                    self.send_buffered(&cur, caps).await?
                }
                None => match cur.body.as_deref() {
                    Some(b) if caps.request_stream && b.len() > caps.max_body_ref => {
                        self.send_streamed(&cur, &mut SliceSource(b)).await?
                    }
                    _ => self.send_buffered(&cur, caps).await?,
                },
            };
            if redirect == Redirect::Manual {
                return Ok(head);
            }
            let status = head.get("status").and_then(Value::as_u64).unwrap_or(0);
            let location = match status {
                301 | 302 | 303 | 307 | 308 => response_header(&head, "location"),
                _ => None,
            };
            let Some(location) = location else {
                head["url"] = Value::String(cur.url);
                return Ok(head);
            };
            if let Some(id) = wire::body_stream_id(&head) {
                abandon_stream(id);
            }
            hops += 1;
            let was_upload = streamed;
            cur = next_redirect(cur, status, &location, hops)?;
            if was_upload && cur.method != "GET" && cur.method != "HEAD" {
                return Err(Error::msg(format!(
                    "fetch failed: cannot follow a {status} redirect after a streamed request body"
                )));
            }
        }
    }

    /// One fetch with the body inline, as a `{ptr,len}` reference, or none.
    async fn send_buffered(&self, cur: &RedirectState, caps: net::Caps) -> Result<Value> {
        let body = match cur.body.as_deref() {
            None => BodyRef::None,
            Some(b)
                if caps.body_ref
                    && b.len() >= net::REF_THRESHOLD
                    && b.len() <= caps.max_body_ref =>
            {
                BodyRef::Ref(b)
            }
            Some(b) => BodyRef::Inline(b),
        };
        submit_cancellable(&wire::fetch_request_with(
            &cur.url,
            &cur.method,
            &cur.headers,
            body,
        ))
        .await
    }

    /// One fetch whose body is pulled from `src` and written chunk by chunk.
    async fn send_streamed<S: ChunkSource>(
        &self,
        cur: &RedirectState,
        src: &mut S,
    ) -> Result<Value> {
        let task = submit_cancellable(&wire::fetch_request_with(
            &cur.url,
            &cur.method,
            &cur.headers,
            BodyRef::Stream,
        ));
        let id = task.id();
        if id < 0 {
            return task.await;
        }
        loop {
            match src.next_chunk().await {
                Ok(Some(c)) if c.is_empty() => {}
                Ok(Some(c)) => {
                    if net::body_write(id, &c).is_err() {
                        // The request already finished (the server answered early) or failed:
                        // its own result says which.
                        net::body_abort(id, "upload stopped");
                        break;
                    }
                }
                Ok(None) => {
                    if net::body_end(id).is_err() {
                        net::body_abort(id, "upload stopped");
                    }
                    break;
                }
                Err(e) => {
                    net::body_abort(id, &e.to_string());
                    let _ = task.await;
                    return Err(e);
                }
            }
        }
        task.await
    }

    /// `await new Promise(r => setTimeout(r, ms))`.
    pub async fn sleep(&self, ms: u64) -> Result<()> {
        // Cancellable: dropping the sleep (a lost `select`/timeout race) frees the host timer.
        submit_cancellable(&wire::sleep_request(ms)).await?;
        Ok(())
    }

    /// Use [`Ctx::storage`] (`store`) instead.
    #[doc(hidden)]
    pub async fn storage_store(&self, data: &[u8], content_type: Option<&str>) -> Result<String> {
        let by_ref = data.len() >= net::REF_THRESHOLD && {
            let caps = net::caps();
            caps.body_ref && data.len() <= caps.max_body_ref
        };
        let body = if by_ref {
            BodyRef::Ref(data)
        } else {
            BodyRef::Inline(data)
        };
        let v = submit(&wire::storage_store_request_with(body, content_type)).await?;
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::msg("storage.store returned no id"))
    }

    /// Use [`Ctx::storage`] (`get`) instead.
    #[doc(hidden)]
    pub async fn storage_get(&self, storage_id: &str) -> Result<Option<StorageFile>> {
        let v = submit(&wire::storage_get_request(storage_id)).await?;
        if v.is_null() {
            return Ok(None);
        }
        let data = read_body(wire::body_stream_id(&v)).await?;
        Ok(Some(StorageFile {
            content_type: opt_str(&v, "contentType"),
            data,
        }))
    }

    /// Milliseconds since the Unix epoch, from the host clock.
    /// Host time (sync op `now`), falling back to the WASI clock.
    pub fn now_ms(&self) -> f64 {
        self.sync_syscall("now", json!({}))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64() * 1000.0)
                    .unwrap_or(0.0)
            })
    }

    /// Synchronous syscall (contracts/ABI.md "Sync syscalls").
    pub fn sync_syscall(&self, op: &str, args: Value) -> Result<Value> {
        let out = sys::sync(op, args.to_string().as_bytes());
        let v: Value = serde_json::from_slice(&out)?;
        match v.get("error") {
            Some(err) => Err(Error::from_json(err)),
            None => Ok(v.get("ok").cloned().unwrap_or(Value::Null)),
        }
    }
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn str_field(v: &Value, k: &str) -> String {
    opt_str(v, k).unwrap_or_default()
}

/// Reads a host body stream to the end; no stream id means an empty body.
async fn read_body(stream_id: Option<&str>) -> Result<Vec<u8>> {
    match stream_id {
        Some(id) => {
            StreamFuture::new(id.to_string()).await
        }
        None => Ok(vec![]),
    }
}

/// The parsed response head of a `fetch` task.
struct FetchHead {
    status: u16,
    status_text: String,
    url: String,
    headers: Vec<(String, String)>,
    stream_id: Option<String>,
}

impl FetchHead {
    fn parse(head: &Value) -> Result<Self> {
        Ok(Self {
            status: head.get("status").and_then(Value::as_u64).unwrap_or(0) as u16,
            status_text: str_field(head, "statusText"),
            url: str_field(head, "url"),
            headers: serde_json::from_value(head.get("headers").cloned().unwrap_or(json!([])))?,
            stream_id: wire::body_stream_id(head).map(str::to_string),
        })
    }
}

/// The request to send for the next hop of a redirect chain.
#[derive(Clone, Debug, PartialEq)]
struct RedirectState {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

/// Pure redirect step: given the request that produced a 3xx `status` with
/// `location`, returns the request for the next hop. `hop` is the 1-based
/// number of this redirect; exceeding [`MAX_REDIRECTS`] is an error.
fn next_redirect(
    cur: RedirectState,
    status: u64,
    location: &str,
    hop: usize,
) -> Result<RedirectState> {
    if hop > MAX_REDIRECTS {
        return Err(Error::msg(format!(
            "fetch failed: maximum redirect ({MAX_REDIRECTS}) reached for {}",
            cur.url
        )));
    }
    let next = resolve_url(&cur.url, location).ok_or_else(|| {
        Error::msg(format!(
            "fetch failed: invalid redirect location {location:?}"
        ))
    })?;
    let RedirectState {
        url,
        mut method,
        mut headers,
        mut body,
    } = cur;
    let to_get = status == 303 && method != "GET" && method != "HEAD"
        || (status == 301 || status == 302) && method == "POST";
    if to_get {
        method = "GET".into();
        body = None;
        headers.retain(|(k, _)| !BODY_HEADERS.iter().any(|h| k.eq_ignore_ascii_case(h)));
    }
    if origin(&next) != origin(&url) {
        headers.retain(|(k, _)| !CREDENTIAL_HEADERS.iter().any(|h| k.eq_ignore_ascii_case(h)));
    }
    Ok(RedirectState {
        url: next,
        method,
        headers,
        body,
    })
}

fn response_header(head: &Value, name: &str) -> Option<String> {
    head.get("headers")?
        .as_array()?
        .iter()
        .filter_map(|p| Some((p.get(0)?.as_str()?, p.get(1)?.as_str()?)))
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.to_string())
}

/// `scheme://authority` of an absolute URL.
fn origin(url: &str) -> &str {
    let Some(i) = url.find("://") else { return url };
    let rest = &url[i + 3..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    &url[..i + 3 + end]
}

/// Resolves a `Location` header against the request URL (RFC 3986 subset:
/// absolute, scheme-relative, origin-relative, path-relative, query-only),
/// dropping the fragment and removing dot segments.
pub(crate) fn resolve_url(base: &str, location: &str) -> Option<String> {
    let location = location.trim().split('#').next().unwrap_or("");
    let org = origin(base);
    let scheme_end = org.find("://")? + 1;
    if location.contains("://") {
        let o = origin(location);
        return Some(format!("{o}{}", normalize(&location[o.len()..])));
    }
    if let Some(rest) = location.strip_prefix("//") {
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        return Some(format!(
            "{}//{}{}",
            &org[..scheme_end],
            &rest[..end],
            normalize(&rest[end..])
        ));
    }
    let base_rest = base[org.len()..].split('#').next().unwrap_or("");
    let base_path = base_rest.split('?').next().unwrap_or("");
    let path = if location.starts_with('/') {
        location.to_string()
    } else if location.is_empty() {
        base_rest.to_string()
    } else if location.starts_with('?') {
        format!(
            "{}{location}",
            if base_path.is_empty() { "/" } else { base_path }
        )
    } else {
        let dir = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
        format!("{}{location}", if dir.is_empty() { "/" } else { dir })
    };
    Some(format!("{org}{}", normalize(&path)))
}

/// Removes `.` / `..` segments from the path part, keeping the query.
fn normalize(path_and_query: &str) -> String {
    let (path, query) = match path_and_query.find('?') {
        Some(i) => (&path_and_query[..i], &path_and_query[i..]),
        None => (path_and_query, ""),
    };
    let mut out: Vec<&str> = vec![];
    let segs: Vec<&str> = path.split('/').collect();
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        match *seg {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let p = out.join("/");
    let p = if p.is_empty() { "/".to_string() } else { p };
    format!("{p}{query}")
}

#[cfg(test)]
mod tests {
    use super::{next_redirect, resolve_url, RedirectState, VectorFilter, MAX_REDIRECTS};
    use serde_json::json;

    #[test]
    fn vector_filter_json() {
        let f = VectorFilter::or([VectorFilter::eq("a", 1), VectorFilter::eq("b", "x")]);
        assert_eq!(
            f.to_json(),
            json!({"$or": [{"$eq": [{"$field": "a"}, 1]}, {"$eq": [{"$field": "b"}, "x"]}]})
        );
    }

    #[test]
    fn resolves_locations() {
        let b = "http://h:1/a/b/c?x=1";
        assert_eq!(resolve_url(b, "http://o/z?q#f").unwrap(), "http://o/z?q");
        assert_eq!(resolve_url(b, "/r?t=3").unwrap(), "http://h:1/r?t=3");
        assert_eq!(resolve_url(b, "d").unwrap(), "http://h:1/a/b/d");
        assert_eq!(resolve_url(b, "../d").unwrap(), "http://h:1/a/d");
        assert_eq!(resolve_url(b, "?y=2").unwrap(), "http://h:1/a/b/c?y=2");
        assert_eq!(resolve_url(b, "//o2/p").unwrap(), "http://o2/p");
        assert_eq!(resolve_url("http://h", "x").unwrap(), "http://h/x");
    }

    fn st(method: &str) -> RedirectState {
        RedirectState {
            url: "http://a.test/x".into(),
            method: method.into(),
            headers: vec![
                ("Content-Type".into(), "text/plain".into()),
                ("Content-Length".into(), "2".into()),
                ("Authorization".into(), "Bearer t".into()),
                ("X-Keep".into(), "1".into()),
            ],
            body: Some(b"hi".to_vec()),
        }
    }

    fn names(s: &RedirectState) -> Vec<&str> {
        s.headers.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn post_becomes_get_on_301_302_303() {
        for status in [301, 302, 303] {
            let n = next_redirect(st("POST"), status, "/y", 1).unwrap();
            assert_eq!(n.method, "GET", "{status}");
            assert_eq!(n.body, None);
            assert_eq!(n.url, "http://a.test/y");
            assert_eq!(names(&n), ["Authorization", "X-Keep"]);
        }
    }

    #[test]
    fn method_rewrite_edge_cases() {
        // 301/302 keep non-POST methods and their body.
        let n = next_redirect(st("PUT"), 302, "/y", 1).unwrap();
        assert_eq!((n.method.as_str(), n.body.is_some()), ("PUT", true));
        // 303 rewrites everything except GET/HEAD.
        let n = next_redirect(st("PUT"), 303, "/y", 1).unwrap();
        assert_eq!((n.method.as_str(), n.body.is_some()), ("GET", false));
        let n = next_redirect(st("HEAD"), 303, "/y", 1).unwrap();
        assert_eq!(n.method, "HEAD");
    }

    #[test]
    fn redirects_307_308_preserve_method_and_body() {
        for status in [307, 308] {
            let n = next_redirect(st("POST"), status, "/y", 1).unwrap();
            assert_eq!(n.method, "POST");
            assert_eq!(n.body.as_deref(), Some(&b"hi"[..]));
            assert_eq!(n.headers.len(), 4);
        }
    }

    #[test]
    fn cross_origin_strips_credentials() {
        let mut s = st("GET");
        s.headers.push(("cookie".into(), "a=b".into()));
        s.headers.push(("Proxy-Authorization".into(), "x".into()));
        let n = next_redirect(s.clone(), 307, "http://b.test/z", 1).unwrap();
        assert_eq!(names(&n), ["Content-Type", "Content-Length", "X-Keep"]);
        // Same origin keeps them.
        let n = next_redirect(s, 307, "http://a.test/z", 1).unwrap();
        assert_eq!(n.headers.len(), 6);
    }

    #[test]
    fn max_redirects() {
        assert!(next_redirect(st("GET"), 302, "/y", MAX_REDIRECTS).is_ok());
        let e = next_redirect(st("GET"), 302, "/y", MAX_REDIRECTS + 1).unwrap_err();
        assert!(e.message().contains("maximum redirect (20)"), "{e}");
    }
}

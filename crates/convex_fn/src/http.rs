//! HTTP actions: `#[http_action]` handlers take an [`HttpRequest`] and return an
//! [`HttpResponse`] (contracts/V8HOST.md "HTTP actions").
//!
//! The request head arrives as the call's arguments; its body is a host stream read
//! chunk by chunk ([`HttpRequest::chunk`]) or whole ([`HttpRequest::bytes`]). A response
//! is either a buffered body or a streaming one: [`HttpResponse::streaming`] runs a body
//! producer after the head was sent, writing chunks with [`BodyWriter::write`] while the
//! client receives them (server-sent events, relayed LLM tokens, ...).

use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc};

use base64::Engine;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    error::Error,
    executor::{abandon_stream, submit, NextChunk, StreamFuture},
    wire::{self, B64},
    Ctx, Result,
};

/// Convex limits an HTTP action's response body to 20 MiB.
const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;

/// Largest piece sent to the host per `BodyWriter::write` task.
const WRITE_PIECE_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct RequestWire {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default, rename = "bodyStreamId")]
    body_stream_id: Option<String>,
    /// Whole request body, base64 (hosts without request body streams: the `rustHttpAction` bridge
    /// of the native / wasmtime hosts). Sent together with `buffered`.
    #[serde(default)]
    body: Option<String>,
    /// The host cannot stream a response: a streaming body is collected and returned whole.
    #[serde(default)]
    buffered: bool,
}

/// An incoming HTTP request (the `Request` of a Convex `httpAction`).
#[derive(Debug, Deserialize)]
#[serde(from = "RequestWire")]
pub struct HttpRequest {
    /// HTTP method, upper case (`"GET"`, `"POST"`, ...).
    pub method: String,
    /// The full request URL, e.g. `https://happy-animal-123.convex.site/hook?x=1`.
    pub url: String,
    /// Request headers; names are lower case, as `Headers` yields them.
    pub headers: Vec<(String, String)>,
    stream_id: Option<String>,
    /// The not yet read inline body (`RequestWire::body`).
    inline: Option<Vec<u8>>,
    buffered: bool,
}

impl From<RequestWire> for HttpRequest {
    fn from(w: RequestWire) -> Self {
        // An undecodable inline body reads as an empty one; the bridge always sends valid base64.
        let inline = w.body.map(|b| B64.decode(b).unwrap_or_default());
        HttpRequest {
            method: w.method,
            url: w.url,
            headers: w.headers,
            stream_id: w.body_stream_id,
            inline,
            buffered: w.buffered,
        }
    }
}

impl HttpRequest {
    /// Builds a request value (for tests); `body_stream_id` names a host stream.
    #[doc(hidden)]
    pub fn from_parts(
        method: &str,
        url: &str,
        headers: Vec<(String, String)>,
        body_stream_id: Option<String>,
    ) -> Self {
        RequestWire {
            method: method.to_string(),
            url: url.to_string(),
            headers,
            body_stream_id,
            body: None,
            buffered: false,
        }
        .into()
    }

    /// The first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The URL path (without query string and fragment).
    pub fn path(&self) -> &str {
        let rest = self.url.split_once("://").map_or(&*self.url, |(_, r)| r);
        let rest = rest.find('/').map_or("/", |i| &rest[i..]);
        rest.split(['?', '#']).next().unwrap_or("/")
    }

    /// The raw query string (without the `?`), if any.
    pub fn query(&self) -> Option<&str> {
        let (_, q) = self.url.split_once('?')?;
        Some(q.split('#').next().unwrap_or(q))
    }

    /// Does the request have a body stream (false for `GET`/`HEAD`)?
    pub fn has_body(&self) -> bool {
        self.stream_id.is_some() || self.inline.is_some()
    }

    /// The next body chunk, `Ok(None)` once the body ended or if there is none.
    /// After `None` or `Err`, further calls return `Ok(None)`.
    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if let Some(body) = self.inline.take() {
            return Ok((!body.is_empty()).then_some(body));
        }
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
        if let Some(body) = self.inline.take() {
            return Ok(body);
        }
        match self.stream_id.take() {
            Some(stream_id) => StreamFuture::new(stream_id).await,
            None => Ok(vec![]),
        }
    }

    /// Reads the remaining body as UTF-8 text.
    pub async fn text(&mut self) -> Result<String> {
        Ok(String::from_utf8(self.bytes().await?)?)
    }

    /// Reads the remaining body and parses it as JSON.
    pub async fn json<T: DeserializeOwned>(&mut self) -> Result<T> {
        Ok(serde_json::from_slice(&self.bytes().await?)?)
    }
}

impl Drop for HttpRequest {
    fn drop(&mut self) {
        if let Some(id) = self.stream_id.take() {
            abandon_stream(&id);
        }
    }
}

type BodyFuture = Pin<Box<dyn Future<Output = Result<()>>>>;
type BodyFn = Box<dyn FnOnce(BodyWriter) -> BodyFuture>;

enum Body {
    Bytes(Vec<u8>),
    Stream(BodyFn),
}

/// The response of an HTTP action: status, headers and a buffered or streaming body.
pub struct HttpResponse {
    /// Status code, 200..=599.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    body: Body,
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("HttpResponse");
        d.field("status", &self.status).field("headers", &self.headers);
        match &self.body {
            Body::Bytes(b) => d.field("body", &format_args!("{} bytes", b.len())),
            Body::Stream(_) => d.field("body", &format_args!("<stream>")),
        };
        d.finish()
    }
}

impl HttpResponse {
    /// An empty response with `status`.
    pub fn new(status: u16) -> Self {
        HttpResponse {
            status,
            headers: vec![],
            body: Body::Bytes(vec![]),
        }
    }

    /// An empty `200 OK` response.
    pub fn ok() -> Self {
        Self::new(200)
    }

    /// A `200` response with a `text/plain; charset=utf-8` body.
    pub fn text(body: impl Into<String>) -> Self {
        Self::ok()
            .header("content-type", "text/plain; charset=utf-8")
            .body(body.into().into_bytes())
    }

    /// A `200` response with `value` as an `application/json` body.
    pub fn json<T: Serialize>(value: &T) -> Result<Self> {
        Ok(Self::ok()
            .header("content-type", "application/json")
            .body(serde_json::to_vec(value)?))
    }

    /// Sets the status code.
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Adds a header (repeated names are sent as repeated headers).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets a buffered body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Body::Bytes(body.into());
        self
    }

    /// Makes the body a stream: after the head is sent, `produce` runs with a
    /// [`BodyWriter`]; the body ends when the future finishes (an `Err` aborts the stream).
    ///
    /// ```ignore
    /// HttpResponse::ok()
    ///     .header("content-type", "text/event-stream")
    ///     .streaming(|w| async move {
    ///         for i in 0..3 { w.write_str(&format!("data: {i}\n\n")).await?; }
    ///         Ok(())
    ///     })
    /// ```
    pub fn streaming<F, Fut>(mut self, produce: F) -> Self
    where
        F: FnOnce(BodyWriter) -> Fut + 'static,
        Fut: Future<Output = Result<()>> + 'static,
    {
        self.body = Body::Stream(Box::new(move |w| Box::pin(produce(w))));
        self
    }

    /// The buffered body, `None` for a streaming response.
    pub fn body_bytes(&self) -> Option<&[u8]> {
        match &self.body {
            Body::Bytes(b) => Some(b),
            Body::Stream(_) => None,
        }
    }
}

/// Writes the body of a streaming [`HttpResponse`]. `write` resolves once the host
/// accepted the bytes (it waits while the client is slower than the producer) and fails
/// once the client went away. On a host that cannot stream (`buffered`), the writes are
/// collected and sent as one body when the producer finishes.
#[derive(Debug)]
pub struct BodyWriter {
    collected: Option<Rc<RefCell<Vec<u8>>>>,
}

impl BodyWriter {
    /// Sends `data` to the client.
    pub async fn write(&self, data: impl AsRef<[u8]>) -> Result<()> {
        if let Some(buf) = &self.collected {
            let mut buf = buf.borrow_mut();
            if buf.len() + data.as_ref().len() > MAX_BODY_BYTES {
                return Err(Error::msg(format!(
                    "HTTP response exceeds the {MAX_BODY_BYTES} byte limit"
                )));
            }
            buf.extend_from_slice(data.as_ref());
            return Ok(());
        }
        for piece in data.as_ref().chunks(WRITE_PIECE_BYTES) {
            submit(&wire::http_write_request(piece)).await?;
        }
        Ok(())
    }

    /// Sends `text` to the client.
    pub async fn write_str(&self, text: &str) -> Result<()> {
        self.write(text.as_bytes()).await
    }
}

/// Sends the response to the host and returns the `ok` value of the result envelope:
/// the buffered response (`{status, headers, body}`), or `{"streamed": true}` after a
/// streaming body was written.
pub(crate) async fn respond(resp: HttpResponse, buffered: bool) -> Result<Value> {
    if !(200..=599).contains(&resp.status) {
        return Err(Error::msg(format!(
            "invalid HTTP status {} (expected 200..=599)",
            resp.status
        )));
    }
    let envelope = |status: u16, headers: &[(String, String)], b: &[u8]| {
        json!({
            "status": status,
            "headers": headers,
            "body": (!b.is_empty()).then(|| B64.encode(b)),
        })
    };
    match resp.body {
        Body::Bytes(b) => Ok(envelope(resp.status, &resp.headers, &b)),
        Body::Stream(produce) if buffered => {
            let buf = Rc::new(RefCell::new(Vec::new()));
            produce(BodyWriter { collected: Some(buf.clone()) }).await?;
            let body = buf.take();
            Ok(envelope(resp.status, &resp.headers, &body))
        }
        Body::Stream(produce) => {
            submit(&wire::http_start_request(resp.status, &resp.headers)).await?;
            produce(BodyWriter { collected: None }).await?;
            submit(&wire::http_end_request()).await?;
            Ok(json!({ "streamed": true }))
        }
    }
}

/// Adapter used by `#[http_action]`: runs `f` and sends its response.
#[doc(hidden)]
pub fn adapter<F, Fut>(
    f: F,
) -> impl FnOnce(Ctx, HttpRequest) -> Pin<Box<dyn Future<Output = Result<Value>>>>
where
    F: FnOnce(Ctx, HttpRequest) -> Fut + 'static,
    Fut: Future<Output = Result<HttpResponse>> + 'static,
{
    move |ctx, req| {
        let buffered = req.buffered;
        Box::pin(async move { respond(f(ctx, req).await?, buffered).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str) -> HttpRequest {
        HttpRequest::from_parts("GET", url, vec![("Content-Type".into(), "text/x".into())], None)
    }

    #[test]
    fn path_and_query() {
        assert_eq!(req("https://a.convex.site/hook?x=1#f").path(), "/hook");
        assert_eq!(req("https://a.convex.site/hook?x=1#f").query(), Some("x=1"));
        assert_eq!(req("https://a.convex.site").path(), "/");
        assert_eq!(req("https://a.convex.site/a/b").query(), None);
    }

    #[test]
    fn header_is_case_insensitive() {
        assert_eq!(req("https://a/").header("content-TYPE"), Some("text/x"));
        assert_eq!(req("https://a/").header("x"), None);
    }

    #[test]
    fn request_deserializes_from_the_host_head() {
        let r: HttpRequest = serde_json::from_value(json!({
            "method": "POST", "url": "https://a/b", "headers": [["x", "y"]], "bodyStreamId": "s0"
        }))
        .unwrap();
        assert_eq!((r.method.as_str(), r.header("x"), r.has_body()), ("POST", Some("y"), true));
        let r: HttpRequest =
            serde_json::from_value(json!({ "method": "GET", "url": "https://a/", "bodyStreamId": null }))
                .unwrap();
        assert!(!r.has_body() && r.headers.is_empty());
    }

    #[test]
    fn inline_body_is_read_once_and_marks_a_buffered_host() {
        let head = json!({
            "method": "POST", "url": "https://a/b", "headers": [], "body": B64.encode(b"hello"), "buffered": true
        });
        let mut r: HttpRequest = serde_json::from_value(head.clone()).unwrap();
        assert!(r.has_body() && r.buffered);
        assert_eq!(crate::executor::block_on(r.bytes()).unwrap(), b"hello");
        assert_eq!(crate::executor::block_on(r.bytes()).unwrap(), b"");
        let mut r: HttpRequest = serde_json::from_value(head).unwrap();
        assert_eq!(crate::executor::block_on(r.chunk()).unwrap(), Some(b"hello".to_vec()));
        assert_eq!(crate::executor::block_on(r.chunk()).unwrap(), None);
        let mut r: HttpRequest =
            serde_json::from_value(json!({ "method": "POST", "url": "https://a/", "body": null, "buffered": true })).unwrap();
        assert!(!r.has_body() && crate::executor::block_on(r.text()).unwrap().is_empty());
    }

    #[test]
    fn buffered_host_collects_a_streaming_body() {
        let resp = HttpResponse::ok().header("content-type", "text/plain").streaming(|w| async move {
            w.write_str("ab").await?;
            w.write(b"cd").await?;
            Ok(())
        });
        let v = crate::executor::block_on(respond(resp, true)).unwrap();
        assert_eq!(v["status"], 200);
        assert_eq!(v["headers"], json!([["content-type", "text/plain"]]));
        assert_eq!(v["body"], B64.encode(b"abcd"));
        // A failing producer fails the call (the buffered host has no truncated bodies).
        let failing = HttpResponse::ok().streaming(|w| async move {
            w.write_str("x").await?;
            Err(Error::msg("boom"))
        });
        assert!(crate::executor::block_on(respond(failing, true)).is_err());
        // An empty collected body is `null`.
        let empty = HttpResponse::ok().streaming(|_| async { Ok(()) });
        assert!(crate::executor::block_on(respond(empty, true)).unwrap()["body"].is_null());
    }

    #[test]
    fn builders() {
        let r = HttpResponse::text("hi").status(201).header("x-a", "b");
        assert_eq!(r.status, 201);
        assert_eq!(r.body_bytes(), Some(&b"hi"[..]));
        assert_eq!(r.headers[0].0, "content-type");
        let j = HttpResponse::json(&json!({"a": 1})).unwrap();
        assert_eq!(j.body_bytes(), Some(&br#"{"a":1}"#[..]));
        assert!(HttpResponse::ok().streaming(|_| async { Ok(()) }).body_bytes().is_none());
    }
}

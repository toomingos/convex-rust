use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use bytes::Bytes;
use convex_fn::{select, Ctx, Either, FetchStream};
use futures_core::Stream;
use http::{header::CONTENT_LENGTH, HeaderMap, StatusCode, Version};
use serde::de::DeserializeOwned;

use url::Url;

use crate::{
    body::Body,
    error::{Error, Kind, Result},
};

/// A Response to a submitted `Request`.
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    url: Box<Url>,
    version: Version,
    extensions: http::Extensions,
    body: RespBody,
    deadline: Option<Instant>,
}

enum RespBody {
    Stream(FetchStream),
    Bytes(Option<Bytes>),
    /// Taken by `bytes_stream`.
    Gone,
}

impl Response {
    pub(crate) fn from_fetch(
        stream: FetchStream,
        status: StatusCode,
        headers: HeaderMap,
        url: Url,
        timeout: Option<Duration>,
    ) -> Response {
        Response {
            status,
            headers,
            url: Box::new(url),
            version: Version::HTTP_11,
            extensions: http::Extensions::new(),
            body: RespBody::Stream(stream),
            deadline: timeout.map(|t| Instant::now() + t),
        }
    }

    /// Get the `StatusCode` of this `Response`.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Get the HTTP `Version` of this `Response`.
    pub fn version(&self) -> Version {
        self.version
    }

    /// Get the `Headers` of this `Response`.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get a mutable reference to the `Headers` of this `Response`.
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    /// Get the content length of the response, if the header says.
    pub fn content_length(&self) -> Option<u64> {
        self.headers
            .get(CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .parse()
            .ok()
    }

    /// Get the final `Url` of this `Response`.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Get the remote address (never known: the host owns the socket).
    pub fn remote_addr(&self) -> Option<std::net::SocketAddr> {
        None
    }

    /// Returns a reference to the associated extensions.
    pub fn extensions(&self) -> &http::Extensions {
        &self.extensions
    }

    /// Returns a mutable reference to the associated extensions.
    pub fn extensions_mut(&mut self) -> &mut http::Extensions {
        &mut self.extensions
    }

    /// Get the full response text (lossy UTF-8).
    pub async fn text(self) -> Result<String> {
        self.text_with_charset("utf-8").await
    }

    /// Get the full response text; only UTF-8 (lossy) is decoded, other
    /// charsets are read as UTF-8 too.
    pub async fn text_with_charset(self, _default_encoding: &str) -> Result<String> {
        let bytes = self.bytes().await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Try to deserialize the response body as JSON.
    pub async fn json<T: DeserializeOwned>(self) -> Result<T> {
        let url = (*self.url).clone();
        let full = self.bytes().await?;
        serde_json::from_slice(&full).map_err(|e| Error::decode(e).with_url(url))
    }

    /// Get the full response body as `Bytes`.
    pub async fn bytes(mut self) -> Result<Bytes> {
        let mut out: Vec<u8> = Vec::new();
        let mut first: Option<Bytes> = None;
        while let Some(c) = self.chunk().await? {
            if first.is_none() && out.is_empty() {
                first = Some(c);
            } else {
                if let Some(f) = first.take() {
                    out.extend_from_slice(&f);
                }
                out.extend_from_slice(&c);
            }
        }
        Ok(match first {
            Some(f) => f,
            None => Bytes::from(out),
        })
    }

    /// Stream a chunk of the response body; `None` when the body ended.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        let deadline = self.deadline;
        let url = (*self.url).clone();
        match &mut self.body {
            RespBody::Gone => Ok(None),
            RespBody::Bytes(b) => Ok(b.take().filter(|b| !b.is_empty())),
            RespBody::Stream(s) => {
                let r = match deadline {
                    Some(d) => {
                        let left = d.saturating_duration_since(Instant::now());
                        match select(s.chunk(), Ctx::default().sleep(crate::client::millis(left))).await {
                            Either::Left(r) => r,
                            Either::Right(_) => return Err(Error::timeout(Kind::Body).with_url(url)),
                        }
                    }
                    None => s.chunk().await,
                };
                match r {
                    Ok(c) => Ok(c.map(Bytes::from)),
                    Err(e) => Err(Error::body(e.to_string()).with_url(url)),
                }
            }
        }
    }

    /// Convert the response into a `Stream` of `Bytes` from the body.
    pub fn bytes_stream(mut self) -> impl Stream<Item = Result<Bytes>> {
        let body = std::mem::replace(&mut self.body, RespBody::Gone);
        BytesStream {
            resp: Some(Box::new(Response { body, ..self })),
            fut: None,
        }
    }

    /// Turn a response into an error if the server returned an error.
    pub fn error_for_status(self) -> Result<Self> {
        let status = self.status;
        if status.is_client_error() || status.is_server_error() {
            Err(Error::status_code(*self.url, status))
        } else {
            Ok(self)
        }
    }

    /// Turn a reference to a response into an error if the server returned an error.
    pub fn error_for_status_ref(&self) -> Result<&Self> {
        let status = self.status;
        if status.is_client_error() || status.is_server_error() {
            Err(Error::status_code((*self.url).clone(), status))
        } else {
            Ok(self)
        }
    }
}

type ChunkFut = Pin<Box<dyn Future<Output = (Box<Response>, Result<Option<Bytes>>)> + Send>>;

struct BytesStream {
    resp: Option<Box<Response>>,
    fut: Option<ChunkFut>,
}

impl Stream for BytesStream {
    type Item = Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.fut.is_none() {
            let Some(mut resp) = self.resp.take() else {
                return Poll::Ready(None);
            };
            self.fut = Some(Box::pin(async move {
                let r = resp.chunk().await;
                (resp, r)
            }));
        }
        let fut = self.fut.as_mut().expect("set above");
        match fut.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready((resp, r)) => {
                self.fut = None;
                match r {
                    Ok(Some(c)) => {
                        self.resp = Some(resp);
                        Poll::Ready(Some(Ok(c)))
                    }
                    Ok(None) => Poll::Ready(None),
                    Err(e) => Poll::Ready(Some(Err(e))),
                }
            }
        }
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("url", &self.url.as_str())
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish()
    }
}

impl<T: Into<Body>> From<http::Response<T>> for Response {
    fn from(r: http::Response<T>) -> Response {
        let (mut parts, body) = r.into_parts();
        let body: Body = body.into();
        let bytes = match body.as_bytes() {
            Some(b) => Bytes::copy_from_slice(b),
            None => Bytes::new(),
        };
        let url = parts
            .extensions
            .remove::<Url>()
            .unwrap_or_else(|| Url::parse("http://no.url.provided.local").expect("static url"));
        Response {
            status: parts.status,
            headers: parts.headers,
            url: Box::new(url),
            version: parts.version,
            extensions: parts.extensions,
            body: RespBody::Bytes(Some(bytes)),
            deadline: None,
        }
    }
}

impl From<Response> for http::Response<Body> {
    fn from(r: Response) -> http::Response<Body> {
        let (status, headers, url, version, extensions) = (
            r.status,
            r.headers.clone(),
            (*r.url).clone(),
            r.version,
            r.extensions.clone(),
        );
        let body = match &r.body {
            RespBody::Bytes(Some(b)) => Body::from(b.clone()),
            _ => Body::wrap_stream(r.bytes_stream()),
        };
        let mut out = http::Response::new(body);
        *out.status_mut() = status;
        *out.headers_mut() = headers;
        *out.version_mut() = version;
        *out.extensions_mut() = extensions;
        out.extensions_mut().insert(url);
        out
    }
}

/// Extension trait for `http::response::Builder` objects: records the final
/// URL so `Response::from(http::Response)` keeps it.
pub trait ResponseBuilderExt {
    /// Adds a `Url` to the `http::Response` being built.
    fn url(self, url: Url) -> Self;
}

impl ResponseBuilderExt for http::response::Builder {
    fn url(self, url: Url) -> Self {
        self.extension(url)
    }
}

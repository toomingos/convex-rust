use std::{fmt, pin::Pin, sync::Mutex, task::Poll};

use bytes::Bytes;
use futures_core::Stream;
use futures_util::TryStream;

use crate::error::{BoxError, Error};

/// An asynchronous request body.
pub struct Body {
    inner: Inner,
}

type BoxStream = Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send>>;

enum Inner {
    Bytes(Bytes),
    Stream(Mutex<BoxStream>),
}

impl Body {
    /// Returns a reference to the internal data of the `Body`, or `None` for a
    /// streaming body.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.inner {
            Inner::Bytes(b) => Some(b),
            Inner::Stream(_) => None,
        }
    }

    /// Wraps a futures `Stream` in a box inside `Body`.
    pub fn wrap_stream<S>(stream: S) -> Body
    where
        S: TryStream + Send + 'static,
        S::Error: Into<BoxError>,
        Bytes: From<S::Ok>,
    {
        let mut stream = Box::pin(stream);
        let s = futures_util::stream::poll_fn(move |cx| {
            stream
                .as_mut()
                .try_poll_next(cx)
                .map(|o| o.map(|r| r.map(Bytes::from).map_err(Into::into)))
        });
        Body {
            inner: Inner::Stream(Mutex::new(Box::pin(s))),
        }
    }

    /// Splits into the bytes of an in-memory body or the stream of a streaming one.
    pub(crate) fn into_source(self) -> Result<Bytes, StreamSource> {
        match self.inner {
            Inner::Bytes(b) => Ok(b),
            Inner::Stream(s) => Err(StreamSource(
                s.into_inner().unwrap_or_else(|e| e.into_inner()),
            )),
        }
    }

    pub(crate) fn try_clone(&self) -> Option<Body> {
        match &self.inner {
            Inner::Bytes(b) => Some(Body::from(b.clone())),
            Inner::Stream(_) => None,
        }
    }
}

/// A streaming request body as a `convex_fn` chunk source (a streamed upload).
pub(crate) struct StreamSource(BoxStream);

impl convex_fn::ChunkSource for StreamSource {
    async fn next_chunk(&mut self) -> convex_fn::Result<Option<Vec<u8>>> {
        let next = std::future::poll_fn(|cx| self.0.as_mut().poll_next(cx)).await;
        match next {
            Some(Ok(c)) => Ok(Some(c.to_vec())),
            Some(Err(e)) => Err(convex_fn::Error::msg(e.to_string())),
            None => Ok(None),
        }
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Body");
        match &self.inner {
            Inner::Bytes(b) => d.field("bytes", &b.len()),
            Inner::Stream(_) => d.field("stream", &"..."),
        };
        d.finish()
    }
}

impl Default for Body {
    fn default() -> Body {
        Body::from(Bytes::new())
    }
}

impl From<Bytes> for Body {
    fn from(b: Bytes) -> Body {
        Body {
            inner: Inner::Bytes(b),
        }
    }
}
impl From<Vec<u8>> for Body {
    fn from(v: Vec<u8>) -> Body {
        Bytes::from(v).into()
    }
}
impl From<&'static [u8]> for Body {
    fn from(v: &'static [u8]) -> Body {
        Bytes::from_static(v).into()
    }
}
impl From<String> for Body {
    fn from(v: String) -> Body {
        Bytes::from(v).into()
    }
}
impl From<&'static str> for Body {
    fn from(v: &'static str) -> Body {
        Bytes::from_static(v.as_bytes()).into()
    }
}

impl http_body::Body for Body {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, Error>>> {
        let this = self.get_mut();
        match &mut this.inner {
            Inner::Bytes(b) => {
                if b.is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(http_body::Frame::data(std::mem::take(b)))))
                }
            }
            Inner::Stream(s) => {
                let s = s.get_mut().unwrap_or_else(|e| e.into_inner());
                s.as_mut()
                    .poll_next(cx)
                    .map(|o| o.map(|r| r.map(http_body::Frame::data).map_err(Error::body)))
            }
        }
    }
}

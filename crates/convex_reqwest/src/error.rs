use std::{error::Error as StdError, fmt};

use http::StatusCode;
use url::Url;

pub type BoxError = Box<dyn StdError + Send + Sync>;

/// `Result<T, reqwest::Error>`.
pub type Result<T> = std::result::Result<T, Error>;

/// The errors that may occur when processing a `Request`.
pub struct Error {
    inner: Box<Inner>,
}

struct Inner {
    kind: Kind,
    source: Option<BoxError>,
    url: Option<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Builder,
    Request,
    #[allow(dead_code)]
    Redirect,
    Status(StatusCode),
    Body,
    Decode,
}

/// Marker source for deadline expiry; `Error::is_timeout` looks for it.
#[derive(Debug)]
pub(crate) struct TimedOut;

impl fmt::Display for TimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("operation timed out")
    }
}
impl StdError for TimedOut {}

/// Marker source for a failed host fetch (the closest thing to a connect error).
#[derive(Debug)]
pub(crate) struct FetchFailed(pub(crate) String);

impl fmt::Display for FetchFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl StdError for FetchFailed {}

#[derive(Debug)]
struct Message(String);
impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl StdError for Message {}

impl Error {
    pub(crate) fn new<E: Into<BoxError>>(kind: Kind, source: Option<E>) -> Error {
        Error {
            inner: Box::new(Inner {
                kind,
                source: source.map(Into::into),
                url: None,
            }),
        }
    }

    pub(crate) fn builder<E: Into<BoxError>>(e: E) -> Error {
        Error::new(Kind::Builder, Some(e))
    }

    pub(crate) fn builder_msg(m: impl Into<String>) -> Error {
        Error::builder(Message(m.into()))
    }

    pub(crate) fn request<E: Into<BoxError>>(e: E) -> Error {
        Error::new(Kind::Request, Some(e))
    }

    pub(crate) fn body<E: Into<BoxError>>(e: E) -> Error {
        Error::new(Kind::Body, Some(e))
    }

    pub(crate) fn decode<E: Into<BoxError>>(e: E) -> Error {
        Error::new(Kind::Decode, Some(e))
    }

    pub(crate) fn timeout(kind: Kind) -> Error {
        Error::new(kind, Some(TimedOut))
    }

    pub(crate) fn status_code(url: Url, status: StatusCode) -> Error {
        Error::new(Kind::Status(status), None::<BoxError>).with_url(url)
    }

    /// Returns a possible URL related to this error.
    pub fn url(&self) -> Option<&Url> {
        self.inner.url.as_ref()
    }

    /// Returns a mutable reference to the URL related to this error.
    pub fn url_mut(&mut self) -> Option<&mut Url> {
        self.inner.url.as_mut()
    }

    /// Adds a url related to this error (overwriting any existing).
    pub fn with_url(mut self, url: Url) -> Self {
        self.inner.url = Some(url);
        self
    }

    /// Strips the related url from this error (if, for example, it contains
    /// sensitive information).
    pub fn without_url(mut self) -> Self {
        self.inner.url = None;
        self
    }

    /// Returns true if the error is from a type `Builder`.
    pub fn is_builder(&self) -> bool {
        matches!(self.inner.kind, Kind::Builder)
    }

    /// Returns true if the error is from a `RedirectPolicy`.
    pub fn is_redirect(&self) -> bool {
        matches!(self.inner.kind, Kind::Redirect)
    }

    /// Returns true if the error is from `Response::error_for_status`.
    pub fn is_status(&self) -> bool {
        matches!(self.inner.kind, Kind::Status(_))
    }

    /// Returns true if the error is related to a timeout.
    pub fn is_timeout(&self) -> bool {
        let mut source = self.inner.source.as_deref().map(|e| e as &dyn StdError);
        while let Some(err) = source {
            if err.is::<TimedOut>() {
                return true;
            }
            source = err.source();
        }
        false
    }

    /// Returns true if the error is related to the request.
    pub fn is_request(&self) -> bool {
        matches!(self.inner.kind, Kind::Request)
    }

    /// Returns true if the error is related to connect (here: the host's
    /// fetch task failed before a response head arrived).
    pub fn is_connect(&self) -> bool {
        let mut source = self.inner.source.as_deref().map(|e| e as &dyn StdError);
        while let Some(err) = source {
            if err.is::<FetchFailed>() {
                return true;
            }
            source = err.source();
        }
        false
    }

    /// Returns true if the error is related to the request or response body.
    pub fn is_body(&self) -> bool {
        matches!(self.inner.kind, Kind::Body)
    }

    /// Returns true if the error is related to decoding the response's body.
    pub fn is_decode(&self) -> bool {
        matches!(self.inner.kind, Kind::Decode)
    }

    /// Returns the status code, if the error was generated from a response.
    pub fn status(&self) -> Option<StatusCode> {
        match self.inner.kind {
            Kind::Status(code) => Some(code),
            _ => None,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut b = f.debug_struct("reqwest::Error");
        b.field("kind", &self.inner.kind);
        if let Some(url) = &self.inner.url {
            b.field("url", &url.as_str());
        }
        if let Some(source) = &self.inner.source {
            b.field("source", source);
        }
        b.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner.kind {
            Kind::Builder => f.write_str("builder error")?,
            Kind::Request => f.write_str("error sending request")?,
            Kind::Body => f.write_str("request or response body error")?,
            Kind::Decode => f.write_str("error decoding response body")?,
            Kind::Redirect => f.write_str("error following redirect")?,
            Kind::Status(code) => {
                let prefix = if code.is_client_error() {
                    "HTTP status client error"
                } else {
                    "HTTP status server error"
                };
                write!(f, "{prefix} ({code})")?;
            }
        }
        if let Some(url) = &self.inner.url {
            write!(f, " for url ({url})")?;
        }
        if let Some(e) = &self.inner.source {
            write!(f, ": {e}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.inner.source.as_ref().map(|e| &**e as _)
    }
}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        std::io::Error::other(e)
    }
}

use std::{fmt, future::Future, time::Duration};

use base64::Engine;
use http::{
    header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Method, Version,
};
use serde::Serialize;
use url::Url;

use crate::{
    body::Body,
    client::Client,
    error::{Error, Result},
    multipart,
    response::Response,
};

/// A request which can be executed with `Client::execute()`.
pub struct Request {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Option<Body>,
    timeout: Option<Duration>,
    version: Version,
}

impl Request {
    /// Constructs a new request.
    pub fn new(method: Method, url: Url) -> Self {
        Request {
            method,
            url,
            headers: HeaderMap::new(),
            body: None,
            timeout: None,
            version: Version::default(),
        }
    }

    pub fn method(&self) -> &Method {
        &self.method
    }
    pub fn method_mut(&mut self) -> &mut Method {
        &mut self.method
    }
    pub fn url(&self) -> &Url {
        &self.url
    }
    pub fn url_mut(&mut self) -> &mut Url {
        &mut self.url
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }
    pub fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }
    pub fn body_mut(&mut self) -> &mut Option<Body> {
        &mut self.body
    }
    pub fn timeout(&self) -> Option<&Duration> {
        self.timeout.as_ref()
    }
    pub fn timeout_mut(&mut self) -> &mut Option<Duration> {
        &mut self.timeout
    }
    pub fn version(&self) -> Version {
        self.version
    }
    pub fn version_mut(&mut self) -> &mut Version {
        &mut self.version
    }

    /// Attempts to clone the request; `None` if the body is a stream.
    pub fn try_clone(&self) -> Option<Request> {
        let body = match &self.body {
            Some(b) => Some(b.try_clone()?),
            None => None,
        };
        Some(Request {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            body,
            timeout: self.timeout,
            version: self.version,
        })
    }

    pub(crate) fn into_parts(self) -> (Method, Url, HeaderMap, Option<Body>, Option<Duration>) {
        (self.method, self.url, self.headers, self.body, self.timeout)
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url.as_str())
            .field("headers", &self.headers)
            .finish()
    }
}

impl<T: Into<Body>> TryFrom<http::Request<T>> for Request {
    type Error = Error;

    fn try_from(req: http::Request<T>) -> Result<Self> {
        let (parts, body) = req.into_parts();
        let url = Url::parse(&parts.uri.to_string()).map_err(Error::builder)?;
        let mut r = Request::new(parts.method, url);
        r.headers = parts.headers;
        r.version = parts.version;
        r.body = Some(body.into());
        Ok(r)
    }
}

impl From<Request> for http::Request<Body> {
    fn from(req: Request) -> Self {
        let mut r = http::Request::new(req.body.unwrap_or_default());
        *r.method_mut() = req.method;
        *r.uri_mut() = req.url.as_str().parse().expect("url is a valid uri");
        *r.headers_mut() = req.headers;
        *r.version_mut() = req.version;
        r
    }
}

/// A builder to construct the properties of a `Request`.
#[must_use = "RequestBuilder does nothing until you 'send' it"]
pub struct RequestBuilder {
    client: Client,
    request: Result<Request>,
}

impl RequestBuilder {
    pub(crate) fn new(client: Client, request: Result<Request>) -> RequestBuilder {
        RequestBuilder { client, request }
    }

    /// Assemble a builder starting from an existing `Client` and a `Request`.
    pub fn from_parts(client: Client, request: Request) -> RequestBuilder {
        RequestBuilder {
            client,
            request: Ok(request),
        }
    }

    /// Add a `Header` to this Request.
    pub fn header<K, V>(self, key: K, value: V) -> RequestBuilder
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.header_sensitive(key, value, false)
    }

    fn header_sensitive<K, V>(mut self, key: K, value: V, sensitive: bool) -> RequestBuilder
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        let mut error = None;
        if let Ok(req) = &mut self.request {
            match <HeaderName as TryFrom<K>>::try_from(key) {
                Ok(key) => match <HeaderValue as TryFrom<V>>::try_from(value) {
                    Ok(mut value) => {
                        value.set_sensitive(sensitive);
                        req.headers_mut().append(key, value);
                    }
                    Err(e) => error = Some(Error::builder(e.into())),
                },
                Err(e) => error = Some(Error::builder(e.into())),
            }
        }
        if let Some(e) = error {
            self.request = Err(e);
        }
        self
    }

    /// Add a set of Headers to the existing ones on this Request.
    pub fn headers(mut self, headers: HeaderMap) -> RequestBuilder {
        if let Ok(req) = &mut self.request {
            for (k, v) in headers.iter() {
                req.headers_mut().insert(k.clone(), v.clone());
            }
        }
        self
    }

    /// Enable HTTP basic authentication.
    pub fn basic_auth<U, P>(self, username: U, password: Option<P>) -> RequestBuilder
    where
        U: fmt::Display,
        P: fmt::Display,
    {
        let creds = match password {
            Some(p) => format!("{username}:{p}"),
            None => format!("{username}:"),
        };
        let value = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(creds)
        );
        self.header_sensitive(AUTHORIZATION, value, true)
    }

    /// Enable HTTP bearer authentication.
    pub fn bearer_auth<T: fmt::Display>(self, token: T) -> RequestBuilder {
        self.header_sensitive(AUTHORIZATION, format!("Bearer {token}"), true)
    }

    /// Set the request body.
    pub fn body<T: Into<Body>>(mut self, body: T) -> RequestBuilder {
        if let Ok(req) = &mut self.request {
            *req.body_mut() = Some(body.into());
        }
        self
    }

    /// Enables a request timeout (covers the response body too).
    pub fn timeout(mut self, timeout: Duration) -> RequestBuilder {
        if let Ok(req) = &mut self.request {
            *req.timeout_mut() = Some(timeout);
        }
        self
    }

    /// Sends a multipart/form-data body.
    pub fn multipart(self, form: multipart::Form) -> RequestBuilder {
        let ct = format!("multipart/form-data; boundary={}", form.boundary());
        let mut b = self.header(CONTENT_TYPE, ct);
        if let Ok(req) = &mut b.request {
            *req.body_mut() = Some(Body::from(form.into_bytes()));
        }
        b
    }

    /// Modify the query string of the URL.
    pub fn query<T: Serialize + ?Sized>(mut self, query: &T) -> RequestBuilder {
        let mut error = None;
        if let Ok(req) = &mut self.request {
            let url = req.url_mut();
            let mut pairs = url.query_pairs_mut();
            let serializer = serde_urlencoded::Serializer::new(&mut pairs);
            if let Err(err) = query.serialize(serializer) {
                error = Some(Error::builder(err));
            }
        }
        if let Ok(req) = &mut self.request {
            if let Some("") = req.url().query() {
                req.url_mut().set_query(None);
            }
        }
        if let Some(e) = error {
            self.request = Err(e);
        }
        self
    }

    /// Set HTTP version (accepted, ignored: the host decides).
    pub fn version(mut self, version: Version) -> RequestBuilder {
        if let Ok(req) = &mut self.request {
            *req.version_mut() = version;
        }
        self
    }

    /// Send a form body (`application/x-www-form-urlencoded`).
    pub fn form<T: Serialize + ?Sized>(mut self, form: &T) -> RequestBuilder {
        let mut error = None;
        if let Ok(req) = &mut self.request {
            match serde_urlencoded::to_string(form) {
                Ok(body) => {
                    req.headers_mut().insert(
                        CONTENT_TYPE,
                        HeaderValue::from_static("application/x-www-form-urlencoded"),
                    );
                    *req.body_mut() = Some(body.into());
                }
                Err(err) => error = Some(Error::builder(err)),
            }
        }
        if let Some(e) = error {
            self.request = Err(e);
        }
        self
    }

    /// Send a JSON body.
    pub fn json<T: Serialize + ?Sized>(mut self, json: &T) -> RequestBuilder {
        let mut error = None;
        if let Ok(req) = &mut self.request {
            match serde_json::to_vec(json) {
                Ok(body) => {
                    if !req.headers().contains_key(CONTENT_TYPE) {
                        req.headers_mut()
                            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                    }
                    *req.body_mut() = Some(body.into());
                }
                Err(err) => error = Some(Error::builder(err)),
            }
        }
        if let Some(e) = error {
            self.request = Err(e);
        }
        self
    }

    /// Build a `Request`, which can be inspected, modified and executed.
    pub fn build(self) -> Result<Request> {
        self.request
    }

    /// Build a `Request`, returning the `Client` as well.
    pub fn build_split(self) -> (Client, Result<Request>) {
        (self.client, self.request)
    }

    /// Constructs the Request and sends it to the target URL.
    pub fn send(self) -> impl Future<Output = Result<Response>> {
        let client = self.client;
        let request = self.request;
        async move { client.execute(request?).await }
    }

    /// Attempt to clone the RequestBuilder; `None` for a streaming body.
    pub fn try_clone(&self) -> Option<RequestBuilder> {
        let req = self.request.as_ref().ok()?.try_clone()?;
        Some(RequestBuilder {
            client: self.client.clone(),
            request: Ok(req),
        })
    }
}

impl fmt::Debug for RequestBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("RequestBuilder");
        match &self.request {
            Ok(r) => d.field("request", r),
            Err(e) => d.field("error", e),
        };
        d.finish()
    }
}

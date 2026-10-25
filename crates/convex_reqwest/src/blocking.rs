//! A blocking `Client` API, built on `convex_fn::block_on`.
//!
//! Works from plain synchronous code and from inside a running action:
//! `convex_fn::block_on` detects the nested case and drives the host task
//! directly (submit + blocking `task_poll`) instead of starting a second tokio
//! runtime. Because the guest is single-threaded, tasks spawned on the tokio
//! `LocalSet` (and tokio timers) do not progress while a blocking call is in
//! flight; do not block on something that needs them.

use std::{io, time::Duration};

use bytes::Bytes;
use http::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, StatusCode, Version,
};
use serde::{de::DeserializeOwned, Serialize};
use url::Url;

use crate::{body::Body, error::Result, into_url::IntoUrl, multipart, redirect, Proxy, Request};

fn wait<F: std::future::Future>(f: F) -> F::Output {
    convex_fn::block_on(f)
}

/// Shortcut method to quickly make a blocking `GET` request.
pub fn get<T: IntoUrl>(url: T) -> Result<Response> {
    Client::new().get(url).send()
}

/// A blocking `Client` to make Requests with.
#[derive(Clone, Debug)]
pub struct Client {
    inner: crate::Client,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Client {
        ClientBuilder::new().build().expect("default client")
    }
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }
    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url)
    }
    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url)
    }
    pub fn put<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url)
    }
    pub fn patch<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PATCH, url)
    }
    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url)
    }
    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::HEAD, url)
    }
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        RequestBuilder {
            inner: self.inner.request(method, url),
        }
    }
    pub fn execute(&self, request: Request) -> Result<Response> {
        wait(self.inner.execute(request)).map(Response::new)
    }
}

/// A `ClientBuilder` can be used to create a blocking `Client`.
#[derive(Debug)]
pub struct ClientBuilder {
    inner: crate::ClientBuilder,
    timeout: Option<Duration>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! passthrough {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            pub fn $name(mut self $(, $arg: $ty)*) -> ClientBuilder {
                self.inner = self.inner.$name($($arg),*);
                self
            }
        )*
    };
}

impl ClientBuilder {
    pub fn new() -> ClientBuilder {
        ClientBuilder {
            inner: crate::ClientBuilder::new(),
            // reqwest::blocking defaults to a 30 s timeout.
            timeout: Some(Duration::from_secs(30)),
        }
    }

    pub fn build(self) -> Result<Client> {
        let b = match self.timeout {
            Some(t) => self.inner.timeout(t),
            None => self.inner,
        };
        Ok(Client { inner: b.build()? })
    }

    /// Enables a request timeout (`None` disables it).
    pub fn timeout<T: Into<Option<Duration>>>(mut self, timeout: T) -> ClientBuilder {
        self.timeout = timeout.into();
        self
    }

    pub fn user_agent<V>(mut self, value: V) -> ClientBuilder
    where
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.inner = self.inner.user_agent(value);
        self
    }

    passthrough! {
        default_headers(headers: HeaderMap);
        https_only(enabled: bool);
        redirect(policy: redirect::Policy);
        proxy(proxy: Proxy);
        no_proxy();
        connect_timeout(timeout: Duration);
        pool_max_idle_per_host(max: usize);
        gzip(enable: bool);
        brotli(enable: bool);
        deflate(enable: bool);
        danger_accept_invalid_certs(accept: bool);
        use_native_tls();
        use_rustls_tls();
        http1_only();
        tcp_nodelay(enabled: bool);
    }
}

/// A builder to construct the properties of a blocking `Request`.
#[derive(Debug)]
#[must_use = "RequestBuilder does nothing until you 'send' it"]
pub struct RequestBuilder {
    inner: crate::RequestBuilder,
}

impl RequestBuilder {
    pub fn header<K, V>(mut self, key: K, value: V) -> RequestBuilder
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.inner = self.inner.header(key, value);
        self
    }
    pub fn headers(mut self, headers: HeaderMap) -> RequestBuilder {
        self.inner = self.inner.headers(headers);
        self
    }
    pub fn basic_auth<U, P>(mut self, username: U, password: Option<P>) -> RequestBuilder
    where
        U: std::fmt::Display,
        P: std::fmt::Display,
    {
        self.inner = self.inner.basic_auth(username, password);
        self
    }
    pub fn bearer_auth<T: std::fmt::Display>(mut self, token: T) -> RequestBuilder {
        self.inner = self.inner.bearer_auth(token);
        self
    }
    pub fn body<T: Into<Body>>(mut self, body: T) -> RequestBuilder {
        self.inner = self.inner.body(body);
        self
    }
    pub fn timeout(mut self, timeout: Duration) -> RequestBuilder {
        self.inner = self.inner.timeout(timeout);
        self
    }
    pub fn multipart(mut self, form: multipart::Form) -> RequestBuilder {
        self.inner = self.inner.multipart(form);
        self
    }
    pub fn query<T: Serialize + ?Sized>(mut self, query: &T) -> RequestBuilder {
        self.inner = self.inner.query(query);
        self
    }
    pub fn form<T: Serialize + ?Sized>(mut self, form: &T) -> RequestBuilder {
        self.inner = self.inner.form(form);
        self
    }
    pub fn json<T: Serialize + ?Sized>(mut self, json: &T) -> RequestBuilder {
        self.inner = self.inner.json(json);
        self
    }
    pub fn build(self) -> Result<Request> {
        self.inner.build()
    }
    pub fn send(self) -> Result<Response> {
        wait(self.inner.send()).map(Response::new)
    }
    pub fn try_clone(&self) -> Option<RequestBuilder> {
        self.inner.try_clone().map(|inner| RequestBuilder { inner })
    }
}

/// A blocking `Response`.
#[derive(Debug)]
pub struct Response {
    inner: crate::Response,
    pending: Bytes,
}

impl Response {
    fn new(inner: crate::Response) -> Response {
        Response {
            inner,
            pending: Bytes::new(),
        }
    }
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }
    pub fn version(&self) -> Version {
        self.inner.version()
    }
    pub fn headers(&self) -> &HeaderMap {
        self.inner.headers()
    }
    pub fn url(&self) -> &Url {
        self.inner.url()
    }
    pub fn content_length(&self) -> Option<u64> {
        self.inner.content_length()
    }
    pub fn text(self) -> Result<String> {
        wait(self.inner.text())
    }
    pub fn json<T: DeserializeOwned>(self) -> Result<T> {
        wait(self.inner.json())
    }
    pub fn bytes(self) -> Result<Bytes> {
        wait(self.inner.bytes())
    }
    pub fn error_for_status(self) -> Result<Self> {
        let pending = self.pending;
        self.inner
            .error_for_status()
            .map(|inner| Response { inner, pending })
    }
    pub fn error_for_status_ref(&self) -> Result<&Self> {
        self.inner.error_for_status_ref().map(|_| self)
    }
    /// Copies the body into `w`; returns the number of bytes copied.
    pub fn copy_to<W: io::Write + ?Sized>(&mut self, w: &mut W) -> Result<u64> {
        io::copy(self, w).map_err(crate::error::Error::body)
    }
}

impl io::Read for Response {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            match wait(self.inner.chunk())? {
                Some(c) => self.pending = c,
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending.split_to(n));
        Ok(n)
    }
}

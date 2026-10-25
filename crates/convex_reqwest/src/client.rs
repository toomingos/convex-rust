use std::{fmt, future::Future, net::SocketAddr, sync::Arc, time::Duration};

use convex_fn::{select, Ctx, Either, FetchRequest, Redirect};
use http::{
    header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT},
    Method, StatusCode,
};
use url::Url;

use crate::{
    body::Body,
    error::{Error, Kind, Result},
    into_url::IntoUrl,
    redirect, tls, Proxy, Request, RequestBuilder, Response,
};

/// An asynchronous `Client` to make Requests with. Every request becomes a
/// Convex `Fetch` task, so egress policy, limits, metering and logs are the
/// host's, exactly as for `fetch` in a JS action.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Config>,
}

#[derive(Default)]
struct Config {
    headers: HeaderMap,
    timeout: Option<Duration>,
    https_only: bool,
    no_redirect: bool,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Constructs a new `Client`.
    pub fn new() -> Client {
        Client {
            inner: Arc::new(Config::default()),
        }
    }

    /// Creates a `ClientBuilder` to configure a `Client`.
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

    /// Start building a `Request` with the `Method` and `Url`.
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let req = url.into_url().map(|url| Request::new(method, url));
        RequestBuilder::new(self.clone(), req)
    }

    /// Executes a `Request`.
    pub fn execute(&self, request: Request) -> impl Future<Output = Result<Response>> {
        let this = self.clone();
        async move { this.execute_inner(request).await }
    }

    async fn execute_inner(&self, request: Request) -> Result<Response> {
        let (method, url, mut headers, body, timeout) = request.into_parts();
        if self.inner.https_only && url.scheme() != "https" {
            return Err(Error::builder_msg("URL scheme is not allowed").with_url(url));
        }
        let timeout = timeout.or(self.inner.timeout);
        for (k, v) in &self.inner.headers {
            if !headers.contains_key(k) {
                headers.append(k.clone(), v.clone());
            }
        }
        // An in-memory body is sent as is; a streaming one is uploaded chunk by chunk
        // (`Ctx::fetch_upload`), so it never has to fit in memory.
        let mut upload = None;
        let body = match body.map(Body::into_source) {
            Some(Ok(b)) => Some(b),
            Some(Err(src)) => {
                upload = Some(src);
                None
            }
            None => None,
        };
        let mut fr = FetchRequest {
            url: url.to_string(),
            method: method.as_str().to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), latin1(v.as_bytes())))
                .collect(),
            body: body.map(|b| b.to_vec()),
            redirect: if self.inner.no_redirect {
                Redirect::Manual
            } else {
                Redirect::Follow
            },
        };
        if matches!(method, Method::GET | Method::HEAD)
            && fr.body.as_ref().is_some_and(Vec::is_empty)
        {
            fr.body = None;
        }

        let ctx = Ctx::default();
        // Dropping `send` (the timeout below, or the caller dropping the request future)
        // aborts the host fetch.
        let send = async {
            match upload {
                Some(src) => ctx.fetch_stream_upload(fr, src).await,
                None => ctx.fetch_stream(fr).await,
            }
        };
        let stream = match timeout {
            // The host `sleep` task, not a tokio timer: inside `block_on_direct` (a blocking client
            // in an action) no tokio timer is ever driven. Dropping `send` cancels the fetch.
            Some(t) => match select(send, ctx.sleep(millis(t))).await {
                Either::Left(r) => r,
                Either::Right(_) => return Err(Error::timeout(Kind::Request).with_url(url)),
            },
            None => send.await,
        }
        .map_err(|e| {
            Error::request(crate::error::FetchFailed(e.to_string())).with_url(url.clone())
        })?;

        let status = StatusCode::from_u16(stream.status)
            .map_err(|e| Error::request(e).with_url(url.clone()))?;
        let mut resp_headers = HeaderMap::new();
        for (k, v) in &stream.headers {
            if let (Ok(k), Ok(v)) = (
                HeaderName::from_bytes(k.as_bytes()),
                header_value(v),
            ) {
                resp_headers.append(k, v);
            }
        }
        let final_url = Url::parse(&stream.url).unwrap_or(url);
        Ok(Response::from_fetch(
            stream,
            status,
            resp_headers,
            final_url,
            timeout,
        ))
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").finish()
    }
}

/// A `ClientBuilder` can be used to create a `Client` with custom
/// configuration. Options with no equivalent for host-side fetch (TLS,
/// proxies, pooling, redirects, ...) are accepted and ignored.
#[must_use]
pub struct ClientBuilder {
    config: Config,
    error: Option<Error>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! ignored {
    ($($(#[$m:meta])* $name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            $(#[$m])*
            pub fn $name(self $(, $arg: $ty)*) -> ClientBuilder {
                $(let _ = $arg;)*
                self
            }
        )*
    };
}

impl ClientBuilder {
    pub fn new() -> ClientBuilder {
        ClientBuilder {
            config: Config::default(),
            error: None,
        }
    }

    /// Returns a `Client` that uses this `ClientBuilder` configuration.
    pub fn build(self) -> Result<Client> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(Client {
                inner: Arc::new(self.config),
            }),
        }
    }

    /// Sets the default headers for every request.
    pub fn default_headers(mut self, headers: HeaderMap) -> ClientBuilder {
        for (k, v) in headers.iter() {
            self.config.headers.insert(k.clone(), v.clone());
        }
        self
    }

    /// Sets the `User-Agent` header to be used by this client.
    pub fn user_agent<V>(mut self, value: V) -> ClientBuilder
    where
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        match HeaderValue::try_from(value) {
            Ok(v) => {
                self.config.headers.insert(USER_AGENT, v);
            }
            Err(e) => self.error = Some(Error::builder(e.into())),
        }
        self
    }

    /// Enables a total request timeout.
    pub fn timeout(mut self, timeout: Duration) -> ClientBuilder {
        self.config.timeout = Some(timeout);
        self
    }

    /// Only use HTTPS URLs.
    pub fn https_only(mut self, enabled: bool) -> ClientBuilder {
        self.config.https_only = enabled;
        self
    }

    /// Sets the redirect policy. `Policy::none()` returns 3xx responses as
    /// is; any other policy follows redirects (the host's limit applies).
    pub fn redirect(mut self, policy: redirect::Policy) -> ClientBuilder {
        self.config.no_redirect = policy.is_none();
        self
    }

    /// Add a proxy (accepted, ignored).
    pub fn proxy(self, proxy: Proxy) -> ClientBuilder {
        let _ = proxy;
        self
    }

    /// Add a root certificate (accepted, ignored).
    pub fn add_root_certificate(self, cert: tls::Certificate) -> ClientBuilder {
        let _ = cert;
        self
    }

    /// Sets the identity to be used for client certificate authentication (ignored).
    pub fn identity(self, identity: tls::Identity) -> ClientBuilder {
        let _ = identity;
        self
    }

    ignored! {
        connect_timeout(timeout: Duration);
        read_timeout(timeout: Duration);
        no_proxy();
        pool_idle_timeout(val: impl Into<Option<Duration>>);
        pool_max_idle_per_host(max: usize);
        http1_only();
        http1_title_case_headers();
        http2_prior_knowledge();
        http2_adaptive_window(enabled: bool);
        http2_keep_alive_interval(interval: impl Into<Option<Duration>>);
        http2_keep_alive_timeout(timeout: Duration);
        http2_keep_alive_while_idle(enabled: bool);
        tcp_nodelay(enabled: bool);
        tcp_keepalive(val: impl Into<Option<Duration>>);
        local_address(addr: impl Into<Option<std::net::IpAddr>>);
        referer(enable: bool);
        gzip(enable: bool);
        brotli(enable: bool);
        zstd(enable: bool);
        deflate(enable: bool);
        no_gzip();
        no_brotli();
        no_zstd();
        no_deflate();
        cookie_store(enable: bool);
        hickory_dns(enable: bool);
        trust_dns(enable: bool);
        no_hickory_dns();
        no_trust_dns();
        danger_accept_invalid_certs(accept: bool);
        danger_accept_invalid_hostnames(accept: bool);
        tls_built_in_root_certs(enabled: bool);
        tls_built_in_webpki_certs(enabled: bool);
        tls_built_in_native_certs(enabled: bool);
        tls_sni(enabled: bool);
        min_tls_version(version: tls::Version);
        max_tls_version(version: tls::Version);
        use_native_tls();
        use_rustls_tls();
        tls_info(enabled: bool);
        connection_verbose(enabled: bool);
    }

    /// Override DNS resolution for a domain (ignored).
    pub fn resolve(self, domain: &str, addr: SocketAddr) -> ClientBuilder {
        let _ = (domain, addr);
        self
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder").finish()
    }
}

/// A timeout as whole host-sleep milliseconds (rounded up, at least 1).
pub(crate) fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_nanos().div_ceil(1_000_000))
        .unwrap_or(u64::MAX)
        .max(1)
}

/// A header value's bytes as the ByteString `fetch` takes (one char per byte), so values that
/// are not UTF-8 survive the round trip instead of being dropped.
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// The inverse of [`latin1`] for response headers (fetch hands out ByteStrings); a value with
/// chars above U+00FF is taken as UTF-8.
fn header_value(s: &str) -> std::result::Result<HeaderValue, http::header::InvalidHeaderValue> {
    if s.chars().all(|c| (c as u32) < 256) {
        let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
        HeaderValue::from_bytes(&bytes)
    } else {
        HeaderValue::from_str(s)
    }
}

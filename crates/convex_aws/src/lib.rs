//! AWS SDK for Rust support for `"use rust"` actions (DX-06c).
//!
//! The AWS SDK's default HTTP stack (hyper + rustls + `aws-lc-sys`/`ring`) cannot
//! be built for `wasm32-wasip1`. Turn it off (`default-features = false`) and plug
//! this crate in instead:
//!
//! ```ignore
//! let cfg = convex_aws::config_from_env().await;
//! let s3 = aws_sdk_s3::Client::new(&cfg);
//! ```
//!
//! Requests go through `ctx.fetch` (egress policy, limits and metering are the
//! host's, as for `fetch` in a JS action); sleeping/timeouts use tokio's timer
//! on the `convex_fn` `tokio-rt` runtime; SigV4 signing is pure Rust (sha2/hmac).
//! Request and response bodies are buffered (v1).

use std::{future::Future, pin::Pin, time::Duration};

use aws_credential_types::{provider::SharedCredentialsProvider, Credentials};
use aws_smithy_async::rt::sleep::{AsyncSleep, SharedAsyncSleep, Sleep};
use aws_smithy_runtime_api::{
    box_error::BoxError,
    client::{
        http::{
            HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings,
            SharedHttpClient, SharedHttpConnector,
        },
        orchestrator::{HttpRequest, HttpResponse},
        result::ConnectorError,
        runtime_components::RuntimeComponents,
    },
};
use aws_smithy_types::{body::SdkBody, byte_stream::ByteStream};
use aws_types::{region::Region, SdkConfig};
use convex_fn::{Ctx, FetchRequest, Redirect};

/// Wasm guests are single-threaded; the smithy traits demand `Send` futures
/// though, so assert it.
struct AssertSend<F>(F);
// SAFETY: wasm32-wasip1 actions run on a single thread (current_thread
// runtime); native tests drive the future on the thread that created it.
unsafe impl<F> Send for AssertSend<F> {}
impl<F: Future> Future for AssertSend<F> {
    type Output = F::Output;
    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: structural pin projection of the only field.
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}

/// A smithy [`HttpClient`] that sends requests with `ctx.fetch`.
#[derive(Clone, Debug, Default)]
pub struct ConvexHttpClient {
    _private: (),
}

impl ConvexHttpClient {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HttpClient for ConvexHttpClient {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(ConvexConnector)
    }
}

#[derive(Clone, Debug)]
struct ConvexConnector;

impl HttpConnector for ConvexConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        HttpConnectorFuture::new(AssertSend(send(request)))
    }
}

fn io_err(e: impl std::fmt::Display) -> ConnectorError {
    ConnectorError::io(BoxError::from(e.to_string()))
}

/// Converts the smithy request to a [`FetchRequest`]. Redirects are returned
/// as is (the SDK handles S3 region redirects itself).
async fn to_fetch(request: HttpRequest) -> Result<FetchRequest, ConnectorError> {
    let method = request.method().to_string();
    let url = request.uri().to_string();
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let body = request.into_body();
    let bytes = match body.bytes() {
        Some(b) => b.to_vec(),
        None => ByteStream::new(body)
            .collect()
            .await
            .map_err(io_err)?
            .to_vec(),
    };
    Ok(FetchRequest {
        url,
        method,
        headers,
        body: (!bytes.is_empty()).then_some(bytes),
        redirect: Redirect::Manual,
    })
}

fn to_response(
    status: u16,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<HttpResponse, ConnectorError> {
    let mut resp = HttpResponse::new(
        status.try_into().map_err(|e| io_err(format!("{e:?}")))?,
        SdkBody::from(body),
    );
    for (k, v) in headers {
        resp.headers_mut().append(k.clone(), v.clone());
    }
    Ok(resp)
}

async fn send(request: HttpRequest) -> Result<HttpResponse, ConnectorError> {
    let fr = to_fetch(request).await?;
    let r = Ctx::default().fetch(fr).await.map_err(io_err)?;
    to_response(r.status, &r.headers, r.body)
}

/// The smithy HTTP client backed by `ctx.fetch`.
pub fn http_client() -> SharedHttpClient {
    SharedHttpClient::new(ConvexHttpClient::new())
}

/// `AsyncSleep` on the host `sleep` task (not tokio's timer, which is not driven inside a
/// nested blocking call), cancelled when the future is dropped.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConvexSleep;

impl AsyncSleep for ConvexSleep {
    fn sleep(&self, duration: Duration) -> Sleep {
        let ms = u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX);
        Sleep::new(async move {
            let _ = Ctx::default().sleep(ms).await;
        })
    }
}

pub fn sleep_impl() -> SharedAsyncSleep {
    SharedAsyncSleep::new(ConvexSleep)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Builds an [`SdkConfig`] from the deployment env: `AWS_ACCESS_KEY_ID`,
/// `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` (optional), `AWS_REGION` (or
/// `AWS_DEFAULT_REGION`, default `us-east-1`), `AWS_ENDPOINT_URL` (optional,
/// for S3-compatible services and mocks; also set `force_path_style(true)` on
/// the S3 config). Without credentials in the env the config has none and
/// requests fail at signing time with an SDK error.
/// Only env-var static credentials are supported (no web-identity / AssumeRole provider).
pub async fn config_from_env() -> SdkConfig {
    let mut b = SdkConfig::builder()
        .behavior_version(aws_smithy_runtime_api::client::behavior_version::BehaviorVersion::latest())
        .http_client(http_client())
        .sleep_impl(sleep_impl())
        .region(Region::new(
            env("AWS_REGION")
                .or_else(|| env("AWS_DEFAULT_REGION"))
                .unwrap_or_else(|| "us-east-1".into()),
        ));
    if let (Some(id), Some(secret)) = (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
        let creds = Credentials::new(id, secret, env("AWS_SESSION_TOKEN"), None, "convex_aws-env");
        b = b.credentials_provider(SharedCredentialsProvider::new(creds));
    }
    if let Some(url) = env("AWS_ENDPOINT_URL") {
        b = b.endpoint_url(url);
    }
    b.build()
}

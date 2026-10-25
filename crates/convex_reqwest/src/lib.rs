//! `reqwest`-compatible HTTP client for `"use rust"` actions (DX-06, D-010).
//!
//! This crate is named `reqwest` and is swapped in with
//! `[patch.crates-io] reqwest = { path = ... }` (the Convex CLI writes that
//! into `convex/rust/.cargo/config.toml`). Vendor crates built on reqwest then
//! send their requests through `convex_fn`'s `Fetch` task, so they keep the
//! host's egress policy, limits, metering and logs.
//!
//! Notes on fidelity:
//! - Needs the SDK's tokio runtime (`convex_fn` feature `tokio-rt`, enabled by
//!   this crate): timeouts use `tokio::time`.
//! - TLS, proxies, pooling, compression, cookies and redirects are the host's;
//!   the builder options for them are accepted and ignored.
//! - Response bodies arrive in the chunks the host delivers.
//! - Only UTF-8 text decoding is supported.

pub mod blocking;
mod body;
mod client;
mod config_types;
mod error;
mod into_url;
pub mod multipart;
mod request;
mod response;

pub use bytes::Bytes;
pub use http::{header, Method, StatusCode, Version};
pub use url::Url;

pub use self::body::Body;
pub use self::client::{Client, ClientBuilder};
pub use self::config_types::{redirect, tls, Proxy};
pub use self::error::{Error, Result};
pub use self::into_url::IntoUrl;
pub use self::request::{Request, RequestBuilder};
pub use self::response::{Response, ResponseBuilderExt};

/// Shortcut method to quickly make a `GET` request.
pub async fn get<T: IntoUrl>(url: T) -> Result<Response> {
    Client::builder().build()?.get(url).send().await
}

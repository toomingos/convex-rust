//! DX-06 wasm probe: `reqwest` (the Convex shim) used directly and through
//! two real vendor-style crates (oauth2, reqwest-middleware).

use std::time::Duration;

use convex_fn::{action, json, Ctx, Error, Result, Value};
use futures_util::StreamExt;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

fn e<E: std::fmt::Display>(x: E) -> Error {
    Error::msg(x.to_string())
}

/// GET + headers + json decode.
#[action]
pub async fn basic(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let resp = reqwest::get("https://api.test/echo?x=1").await?;
    let status = resp.status().as_u16();
    let xecho = resp.headers()["x-echo"].to_str().map_err(e)?.to_string();
    let v: Value = resp.json().await?;
    Ok(json!({"status": status, "xEcho": xecho, "echo": v}))
}

/// Client with defaults, bearer auth, JSON POST, form, concurrent requests.
#[action]
pub async fn post(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let client = reqwest::Client::builder()
        .user_agent("probe/1")
        .timeout(Duration::from_secs(5))
        .build()?;
    let one: Value = client
        .post("https://api.test/items")
        .bearer_auth("tok")
        .json(&json!({"a": 1}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let (f, g) = tokio::join!(
        client.post("https://api.test/f").form(&[("k", "v")]).send(),
        client.get("https://api.test/g").query(&[("q", "1")]).send(),
    );
    let f: Value = f?.json().await?;
    let g: Value = g?.json().await?;
    Ok(json!({"json": one, "form": f, "get": g}))
}

/// Per-request and client timeouts fire (host answers "slow" after 300 ms).
#[action]
pub async fn timeout(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(60))
        .build()?;
    let err = client.get("https://api.test/slow").send().await.unwrap_err();
    let err2 = reqwest::Client::new()
        .get("https://api.test/slow")
        .timeout(Duration::from_millis(60))
        .send()
        .await
        .unwrap_err();
    let ok = reqwest::Client::new()
        .get("https://api.test/fast")
        .timeout(Duration::from_secs(2))
        .send()
        .await?
        .status()
        .as_u16();
    Ok(json!({"clientTimeout": err.is_timeout(), "requestTimeout": err2.is_timeout(), "fast": ok}))
}

/// error_for_status, bytes_stream, chunk.
#[action]
pub async fn body(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let err = reqwest::get("https://api.test/status/404")
        .await?
        .error_for_status()
        .unwrap_err();
    let mut stream = Box::pin(reqwest::get("https://api.test/chunks").await?.bytes_stream());
    let mut parts = vec![];
    while let Some(c) = stream.next().await {
        parts.push(String::from_utf8_lossy(&c?).into_owned());
    }
    let mut r = reqwest::get("https://api.test/chunks").await?;
    let first = r.chunk().await?.map(|b| b.len());
    Ok(json!({"isStatus": err.is_status(), "status": err.status().map(|s| s.as_u16()),
              "parts": parts, "firstChunkLen": first}))
}

/// Vendor crate 1: oauth2 (reqwest 0.12 line) client-credentials exchange.
#[action]
pub async fn oauth(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    use oauth2::{basic::BasicClient, AuthUrl, ClientId, ClientSecret, TokenResponse, TokenUrl};
    let http = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let client = BasicClient::new(ClientId::new("id".into()))
        .set_client_secret(ClientSecret::new("secret".into()))
        .set_auth_uri(AuthUrl::new("https://auth.test/authorize".into()).map_err(e)?)
        .set_token_uri(TokenUrl::new("https://auth.test/token".into()).map_err(e)?);
    let tok = client
        .exchange_client_credentials()
        .request_async(&http)
        .await
        .map_err(e)?;
    Ok(json!({"accessToken": tok.access_token().secret(), "expiresIn": tok.expires_in().map(|d| d.as_secs())}))
}

struct AddHeader;

// reqwest-middleware declares its trait `?Send` on wasm32.
#[async_trait::async_trait(?Send)]
impl reqwest_middleware::Middleware for AddHeader {
    async fn handle(
        &self,
        mut req: reqwest13::Request,
        ext: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest13::Response> {
        req.headers_mut().insert("x-mw", "yes".parse().unwrap());
        next.run(req, ext).await
    }
}

/// Vendor crate 2: reqwest-middleware 0.5 on the reqwest 0.13 line.
#[action]
pub async fn middleware(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let client = reqwest_middleware::ClientBuilder::new(reqwest13::Client::new())
        .with(AddHeader)
        .build();
    let v: Value = client
        .get("https://api.test/mw")
        .send()
        .await
        .map_err(e)?
        .json()
        .await
        .map_err(e)?;
    Ok(json!({"echo": v}))
}

/// `reqwest::blocking` inside an action (no nested tokio runtime).
#[action]
pub async fn blocking_get(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let resp = reqwest::blocking::get("https://api.test/blocking?x=1")?;
    let status = resp.status().as_u16();
    let v: Value = resp.json()?;
    Ok(json!({"status": status, "echo": v}))
}

/// An async fetch, then a blocking one, then an async one again, then a
/// spawned task (which can only run once the blocking call has returned).
#[action]
pub async fn mixed(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let a: Value = reqwest::get("https://api.test/async-1").await?.json().await?;
    let b: Value = reqwest::blocking::Client::new()
        .post("https://api.test/blocking-2")
        .body("payload")
        .send()?
        .json()?;
    let c: Value = reqwest::get("https://api.test/async-3").await?.json().await?;
    let spawned = tokio::task::spawn_local(async { 7 });
    let n = spawned.await.map_err(e)?;
    Ok(json!({"a": a["url"], "b": b["url"], "bBody": b["body"], "c": c["url"], "spawned": n}))
}

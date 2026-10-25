//! Actions using the real `reqwest` API; the Convex CLI's managed
//! `[patch.crates-io]` block swaps the shim in at build time.
use convex_fn::{action, json, Ctx, Error, Result, Value};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Base {
    /// Mock server (bench/mock-server) base URL.
    pub base: String,
    /// Header-echo server base URL.
    #[serde(default)]
    pub echo: String,
}

#[derive(Serialize, Deserialize)]
struct Item {
    i: u32,
    name: String,
    score: u32,
}

fn err(e: reqwest::Error) -> Error {
    Error::msg(format!("reqwest: {e}"))
}

/// GET json into a typed Vec.
#[action]
pub async fn get_json(_ctx: &Ctx, args: Base) -> Result<Value> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/json/5", args.base))
        .query(&[("seed", "7")])
        .send()
        .await
        .map_err(err)?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let items: Vec<Item> = resp.json().await.map_err(err)?;
    Ok(json!({ "status": status, "contentType": content_type, "items": items }))
}

/// POST a json body; the mock server answers with its length and sha256.
#[action]
pub async fn post_json(_ctx: &Ctx, args: Base) -> Result<Value> {
    let client = reqwest::Client::new();
    let payload = json!({ "hello": "world", "n": [1, 2, 3] });
    let echo: Value = client
        .post(format!("{}/echo", args.base))
        .json(&payload)
        .send()
        .await
        .map_err(err)?
        .error_for_status()
        .map_err(err)?
        .json()
        .await
        .map_err(err)?;
    Ok(json!({ "sent": payload.to_string().len(), "echo": echo }))
}

/// Request headers (default + per-request + bearer) reach the server.
#[action]
pub async fn headers(_ctx: &Ctx, args: Base) -> Result<Value> {
    let mut defaults = HeaderMap::new();
    defaults.insert("x-default", HeaderValue::from_static("d"));
    let client = reqwest::Client::builder()
        .user_agent("convex-reqwest-e2e/1")
        .default_headers(defaults)
        .build()
        .map_err(err)?;
    let resp = client
        .get(format!("{}/headers", args.echo))
        .header("x-req", "r")
        .header(AUTHORIZATION, "Bearer tok")
        .send()
        .await
        .map_err(err)?;
    let reply = resp
        .headers()
        .get("x-echo-reply")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let seen: Value = resp.json().await.map_err(err)?;
    Ok(json!({ "reply": reply, "seen": seen }))
}

/// `error_for_status` turns a 404 into an error carrying the status.
#[action]
pub async fn not_found(_ctx: &Ctx, args: Base) -> Result<Value> {
    let url = format!("{}/nope", args.base);
    let resp = reqwest::get(&url).await.map_err(err)?;
    let plain_status = resp.status().as_u16();
    match resp.error_for_status() {
        Ok(_) => Ok(json!({ "ok": true })),
        Err(e) => Ok(json!({
            "ok": false,
            "plainStatus": plain_status,
            "status": e.status().map(|s| s.as_u16()),
            "isStatus": e.is_status(),
            "url": e.url().map(|u| u.path().to_string()),
        })),
    }
}

/// Fails the action itself via `?` on an `error_for_status` error.
#[action]
pub async fn not_found_fails(_ctx: &Ctx, args: Base) -> Result<()> {
    reqwest::get(format!("{}/nope", args.base))
        .await
        .map_err(err)?
        .error_for_status()
        .map_err(err)?;
    Ok(())
}

/// Streams `n` bytes with `bytes_stream` and compares them with a buffered GET.
#[action]
pub async fn stream(_ctx: &Ctx, args: StreamArgs) -> Result<Value> {
    let client = reqwest::Client::new();
    let mut streamed: Vec<u8> = Vec::new();
    let mut chunks = 0u32;
    let mut s = client
        .get(format!("{}/stream/{}?seed=3", args.base, args.n))
        .send()
        .await
        .map_err(err)?
        .bytes_stream();
    while let Some(chunk) = s.next().await {
        streamed.extend_from_slice(&chunk.map_err(err)?);
        chunks += 1;
    }
    let buffered = client
        .get(format!("{}/bytes/{}?seed=3", args.base, args.n))
        .send()
        .await
        .map_err(err)?
        .bytes()
        .await
        .map_err(err)?;
    Ok(json!({
        "len": streamed.len(),
        "chunks": chunks,
        "equal": streamed == buffered.as_ref(),
    }))
}

#[derive(Deserialize)]
pub struct StreamArgs {
    pub base: String,
    pub n: usize,
}

//! Network cases (net-*). Mirrors bench/convex/node/net.ts: fetch from
//! bench/mock-server (env `MOCK_URL`, default http://127.0.0.1:8787) and return
//! digests, never bodies.

use std::{future::Future, pin::pin, task::Poll};

use convex_fn::{action, join_all, json, Ctx, FetchRequest, Result, Value};
use serde::{Deserialize, Serialize};

use crate::prng::Prng;

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

fn base() -> String {
    std::env::var("MOCK_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string())
}

/// SHA-256: the host's `crypto/digest` on wasm32 (no in-guest fallback), `sha2` natively.
#[cfg(target_arch = "wasm32")]
fn sha256(b: &[u8]) -> Result<String> {
    Ok(hex::encode(convex_fn::crypto::digest("SHA-256", b)?))
}

#[cfg(not(target_arch = "wasm32"))]
fn sha256(b: &[u8]) -> Result<String> {
    use sha2::Digest;
    Ok(hex::encode(sha2::Sha256::digest(b)))
}

#[derive(Serialize)]
pub struct Digest1 {
    pub status: u16,
    pub length: usize,
    pub sha256: String,
}

#[derive(Serialize)]
pub struct Many {
    pub count: usize,
    #[serde(rename = "totalBytes")]
    pub total_bytes: usize,
    pub sha256s: Vec<String>,
}

async fn get_bytes(ctx: &Ctx, n: usize, seed: u32) -> Result<Digest1> {
    let res = ctx
        .fetch(FetchRequest::get(format!("{}/bytes/{n}?seed={seed}", base())))
        .await?;
    Ok(Digest1 {
        status: res.status,
        length: res.body.len(),
        sha256: sha256(&res.body)?,
    })
}

fn summarize(rs: Vec<Digest1>) -> Many {
    Many {
        count: rs.len(),
        total_bytes: rs.iter().map(|r| r.length).sum(),
        sha256s: rs.into_iter().map(|r| r.sha256).collect(),
    }
}

/// net-001: GET 1 KB.
#[action]
pub async fn net_001(ctx: &Ctx, args: SeedArgs) -> Result<Digest1> {
    get_bytes(ctx, 1024, args.seed).await
}

/// net-002: GET 10 MB.
#[action]
pub async fn net_002(ctx: &Ctx, args: SeedArgs) -> Result<Digest1> {
    get_bytes(ctx, 10 * 1024 * 1024, args.seed).await
}

/// net-003: POST a 1 MiB JSON body; the server echoes its length and sha256.
#[action]
pub async fn net_003(ctx: &Ctx, args: SeedArgs) -> Result<Digest1> {
    let body = format!(
        r#"{{"seed":{},"data":"{}"}}"#,
        args.seed,
        Prng::new(args.seed).word(1 << 20)
    );
    let res = ctx
        .fetch(
            FetchRequest::post(format!("{}/echo", base()), body)
                .header("content-type", "application/json"),
        )
        .await?;
    let echo: Value = res.json()?;
    Ok(Digest1 {
        status: res.status,
        length: echo["length"].as_u64().unwrap_or_default() as usize,
        sha256: echo["sha256"].as_str().unwrap_or_default().to_string(),
    })
}

/// net-004: 20 GETs in parallel.
#[action]
pub async fn net_004(ctx: &Ctx, args: SeedArgs) -> Result<Many> {
    let rs = join_all((0..20).map(|i| get_bytes(ctx, 1024, args.seed + i))).await;
    Ok(summarize(rs.into_iter().collect::<Result<Vec<_>>>()?))
}

/// net-005: 20 GETs sequential.
#[action]
pub async fn net_005(ctx: &Ctx, args: SeedArgs) -> Result<Many> {
    let mut rs = Vec::new();
    for i in 0..20 {
        rs.push(get_bytes(ctx, 1024, args.seed + i).await?);
    }
    Ok(summarize(rs))
}

/// net-006: chunked 1 MiB body, hashed. The SDK's `fetch` delivers the whole
/// body at once, so the hash is computed over it after the stream ends (same
/// bytes, one digest call).
#[action]
pub async fn net_006(ctx: &Ctx, args: SeedArgs) -> Result<Digest1> {
    let res = ctx
        .fetch(FetchRequest::get(format!(
            "{}/stream/{}?seed={}",
            base(),
            1 << 20,
            args.seed
        )))
        .await?;
    let digest = sha256(&res.body)?;
    Ok(Digest1 {
        status: res.status,
        length: res.body.len(),
        sha256: digest,
    })
}

#[derive(Serialize)]
pub struct Stored {
    pub id: String,
    pub size: usize,
}

/// net-007: fetch 64 KiB, store it.
#[action]
pub async fn net_007(ctx: &Ctx, args: SeedArgs) -> Result<Stored> {
    let res = ctx
        .fetch(FetchRequest::get(format!(
            "{}/bytes/65536?seed={}",
            base(),
            args.seed
        )))
        .await?;
    let id = ctx
        .storage_store(&res.body, Some("application/octet-stream"))
        .await?;
    ctx.storage().delete(&id).await?;
    Ok(Stored {
        id,
        size: res.body.len(),
    })
}

#[derive(Serialize)]
pub struct Recorded {
    pub count: usize,
    pub doc: Value,
}

/// net-008: fetch a JSON array, then record its length through a mutation.
#[action]
pub async fn net_008(ctx: &Ctx, args: SeedArgs) -> Result<Recorded> {
    let res = ctx
        .fetch(FetchRequest::get(format!(
            "{}/json/10?seed={}",
            base(),
            args.seed
        )))
        .await?;
    let items: Vec<Value> = res.json()?;
    let doc: Value = ctx
        .run_mutation(
            "cbTargets:insertDoc",
            json!({ "key": args.seed, "value": format!("n{}", items.len()) }),
        )
        .await?;
    Ok(Recorded {
        count: items.len(),
        doc,
    })
}

/// `Ok` with the output of `a` if it finishes first, else `Err` with `b`'s
/// (`a` wins ties). The SDK has no `select`, so this is a local one.
async fn race<A: Future, B: Future>(a: A, b: B) -> std::result::Result<A::Output, B::Output> {
    let (mut a, mut b) = (pin!(a), pin!(b));
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(Ok(v));
        }
        if let Poll::Ready(v) = b.as_mut().poll(cx) {
            return Poll::Ready(Err(v));
        }
        Poll::Pending
    })
    .await
}

#[derive(Serialize)]
pub struct TimedOut {
    #[serde(rename = "timedOut")]
    pub timed_out: bool,
}

/// net-009: the server sleeps 5 s; give up after 200 ms.
#[action]
pub async fn net_009(ctx: &Ctx, _args: SeedArgs) -> Result<TimedOut> {
    let fetch = ctx.fetch(FetchRequest::get(format!("{}/sleep/5000", base())));
    match race(ctx.sleep(200), fetch).await {
        Ok(timer) => {
            timer?;
            Ok(TimedOut { timed_out: true })
        }
        Err(res) => {
            res?;
            Ok(TimedOut { timed_out: false })
        }
    }
}

#[derive(Serialize)]
pub struct Redirected {
    pub status: u16,
    pub done: bool,
    pub hops: u64,
}

/// net-010: follow 5 redirects (`ctx.fetch` follows them, like web fetch).
#[action]
pub async fn net_010(ctx: &Ctx, _args: SeedArgs) -> Result<Redirected> {
    let res = ctx.fetch(FetchRequest::get(format!("{}/redirect/5", base()))).await?;
    let j: Value = res.json()?;
    Ok(Redirected {
        status: res.status,
        done: j["done"].as_bool().unwrap_or(false),
        hops: j["hops"].as_u64().unwrap_or_default(),
    })
}

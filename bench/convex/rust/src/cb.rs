//! Backend-callback cases (cb-*). Twin of node/cb.ts; targets are the
//! cbTargets.ts functions, addressed by the same paths.

use convex_fn::{action, join_all, json, Ctx, Result, Value};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

// Numbers are f64: the backend returns every JS number as a float (1.0), which i64 refuses.
const SQUARE: &str = "cbTargets:square";
const INSERT_DOC: &str = "cbTargets:insertDoc";
const INC_ACTION: &str = "cbTargets:incAction";
const TOUCH: &str = "cbTargets:touch";
const BLOB_LEN: usize = 1 << 20;

fn range(n: u32, seed: u32) -> impl Iterator<Item = f64> {
    (0..n).map(move |i| f64::from(seed) + f64::from(i))
}

async fn seq(ctx: &Ctx, n: u32, seed: u32) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n as usize);
    for k in range(n, seed) {
        out.push(ctx.run_query(SQUARE, json!({ "n": k })).await?);
    }
    Ok(out)
}

async fn par(ctx: &Ctx, n: u32, seed: u32) -> Result<Vec<f64>> {
    join_all(range(n, seed).map(|k| ctx.run_query::<_, f64>(SQUARE, json!({ "n": k }))))
        .await
        .into_iter()
        .collect()
}

/// 1 MB deterministic payload (same formula as node/cb.ts).
pub fn payload(seed: u32) -> Vec<u8> {
    (0..BLOB_LEN)
        .map(|i| ((i as u64 * 31 + u64::from(seed)) & 255) as u8)
        .collect()
}

/// cb-001: one runQuery.
#[action]
pub async fn cb_001(ctx: &Ctx, a: SeedArgs) -> Result<f64> {
    ctx.run_query(SQUARE, json!({ "n": a.seed })).await
}

/// cb-002: 10 sequential runQuery.
#[action]
pub async fn cb_002(ctx: &Ctx, a: SeedArgs) -> Result<Vec<f64>> {
    seq(ctx, 10, a.seed).await
}

/// cb-003: 10 parallel runQuery (join_all <-> Promise.all).
#[action]
pub async fn cb_003(ctx: &Ctx, a: SeedArgs) -> Result<Vec<f64>> {
    par(ctx, 10, a.seed).await
}

/// cb-004: 100 sequential runQuery.
#[action]
pub async fn cb_004(ctx: &Ctx, a: SeedArgs) -> Result<Vec<f64>> {
    seq(ctx, 100, a.seed).await
}

/// cb-005: 100 parallel runQuery.
#[action]
pub async fn cb_005(ctx: &Ctx, a: SeedArgs) -> Result<Vec<f64>> {
    par(ctx, 100, a.seed).await
}

/// cb-006: one runMutation (returns the inserted doc).
#[action]
pub async fn cb_006(ctx: &Ctx, a: SeedArgs) -> Result<Value> {
    ctx.run_mutation(
        INSERT_DOC,
        json!({ "key": a.seed, "value": format!("v{}", a.seed) }),
    )
    .await
}

/// cb-007: 10 sequential runMutation.
#[action]
pub async fn cb_007(ctx: &Ctx, a: SeedArgs) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(10);
    for k in range(10, a.seed) {
        out.push(
            ctx.run_mutation(INSERT_DOC, json!({ "key": k, "value": format!("v{k}") }))
                .await?,
        );
    }
    Ok(out)
}

/// cb-008: runAction against a default-runtime (isolate) action.
#[action]
pub async fn cb_008(ctx: &Ctx, a: SeedArgs) -> Result<f64> {
    ctx.run_action(INC_ACTION, json!({ "n": a.seed })).await
}

/// cb-009: three chained runAction, each fed the previous result.
#[action]
pub async fn cb_009(ctx: &Ctx, a: SeedArgs) -> Result<f64> {
    let mut n = f64::from(a.seed);
    for _ in 0..3 {
        n = ctx.run_action(INC_ACTION, json!({ "n": n })).await?;
    }
    Ok(n)
}

/// cb-010: scheduler.runAfter(0, ...).
#[action]
pub async fn cb_010(ctx: &Ctx, a: SeedArgs) -> Result<String> {
    ctx.schedule_after(0.0, TOUCH, json!({ "n": a.seed })).await
}

/// cb-011: scheduler.runAt x10, one second apart.
#[action]
pub async fn cb_011(ctx: &Ctx, a: SeedArgs) -> Result<Vec<String>> {
    let base = ctx.now_ms();
    let mut ids = Vec::with_capacity(10);
    for i in 0..10u32 {
        let ts = base + f64::from(i + 1) * 1000.0;
        ids.push(
            ctx.schedule_at(ts, TOUCH, json!({ "n": a.seed + i }))
                .await?,
        );
    }
    Ok(ids)
}

#[derive(Serialize)]
pub struct Stored {
    pub id: String,
    pub size: usize,
}

/// cb-012: storage.store of a 1 MB blob.
#[action]
pub async fn cb_012(ctx: &Ctx, a: SeedArgs) -> Result<Stored> {
    let data = payload(a.seed);
    let id = ctx
        .storage_store(&data, Some("application/octet-stream"))
        .await?;
    Ok(Stored {
        id,
        size: data.len(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fetched {
    pub size: usize,
    pub sum: u64,
    pub content_type: Option<String>,
}

/// cb-013: store (setup) then storage.get a 1 MB blob; returns a checksum.
#[action]
pub async fn cb_013(ctx: &Ctx, a: SeedArgs) -> Result<Fetched> {
    let id = ctx
        .storage_store(&payload(a.seed), Some("application/octet-stream"))
        .await?;
    let f = ctx
        .storage_get(&id)
        .await?
        .ok_or_else(|| convex_fn::Error::msg("stored blob missing"))?;
    Ok(Fetched {
        size: f.data.len(),
        sum: f.data.iter().map(|&b| u64::from(b)).sum(),
        content_type: f.content_type.filter(|s| !s.is_empty()),
    })
}

/// cb-014: auth.getUserIdentity -> {subject, issuer} or null.
#[action]
pub async fn cb_014(ctx: &Ctx, _a: SeedArgs) -> Result<Value> {
    Ok(match ctx.user_identity().await? {
        None => Value::Null,
        Some(id) => json!({ "subject": id["subject"], "issuer": id["issuer"] }),
    })
}

/// cb-015: read CB_ENV_0..CB_ENV_9 (null when unset).
#[action]
pub async fn cb_015(_ctx: &Ctx, _a: SeedArgs) -> Result<Vec<Option<String>>> {
    Ok((0..10)
        .map(|i| std::env::var(format!("CB_ENV_{i}")).ok())
        .collect())
}

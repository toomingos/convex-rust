//! DX-05: env vars, stdout/stderr, `log`, `tracing`, clock and random in an action.
use convex_fn::{action, json, Ctx, Result, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[action]
pub async fn probe(ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    log::info!("dx05 log info");
    log::warn!("dx05 log warn");
    tracing::warn!(user = "ann", n = 3, "dx05 tracing warn");
    println!("dx05 stdout");
    eprintln!("dx05 stderr");

    let env = std::env::var("SPEC_ENV_UNI").ok();

    let wall0 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let t0 = std::time::Instant::now();
    ctx.sleep(30).await?;
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    let mut a = [0u8; 16];
    getrandom02::getrandom(&mut a).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    let mut b = [0u8; 16];
    getrandom03::fill(&mut b).map_err(|e| convex_fn::Error::msg(e.to_string()))?;

    Ok(json!({
        "env": env,
        "wallMsPlausible": wall0 > 1_600_000_000_000,
        "elapsedMs": elapsed_ms,
        "random02Nonzero": a.iter().any(|x| *x != 0),
        "random03Nonzero": b.iter().any(|x| *x != 0),
        "randomDiffer": a != b,
    }))
}

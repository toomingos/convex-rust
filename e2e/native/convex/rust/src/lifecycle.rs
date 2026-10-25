//! Code identity and slow calls for the redeploy, reaping and concurrency tests (suite 4).
//! The version is baked in at build time: `NATIVE_E2E_VERSION=v2 convex dev --once ...`.
use convex_fn::{action, json, Ctx, Result, Value};
use serde::Deserialize;

const VERSION: &str = match option_env!("NATIVE_E2E_VERSION") {
    Some(v) => v,
    None => "none",
};

#[derive(Deserialize)]
pub struct NoArgs {}

#[action]
pub async fn version(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    // wasm has no processes (std::process::id panics there): report 0.
    #[cfg(target_arch = "wasm32")]
    let pid = 0u32;
    #[cfg(not(target_arch = "wasm32"))]
    let pid = std::process::id();
    Ok(json!({ "version": VERSION, "pid": pid }))
}

#[derive(Deserialize)]
pub struct SlowArgs {
    ms: f64,
}
/// Sleeps `ms` (a host timer, so the call stays in flight), then reports its version.
#[action]
pub async fn slow_version(ctx: &Ctx, a: SlowArgs) -> Result<Value> {
    ctx.sleep(a.ms as u64).await?;
    Ok(json!({ "version": VERSION, "pid": std::process::id() }))
}

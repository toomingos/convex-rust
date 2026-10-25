//! Baseline overhead cases (base-*).

use convex_fn::{action, json, log, ConvexError, Ctx, Result};
use serde::{Deserialize, Serialize};

use crate::prng::Prng;

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

/// base-001: noop.
#[action]
pub async fn base_001(_ctx: &Ctx, _args: SeedArgs) -> Result<()> {
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct EchoArgs {
    pub seed: u32,
    pub msg: String,
}

/// base-002: echo small args.
#[action]
pub async fn base_002(_ctx: &Ctx, args: EchoArgs) -> Result<EchoArgs> {
    Ok(args)
}

/// base-003: return a 1 MiB string.
#[action]
pub async fn base_003(_ctx: &Ctx, args: SeedArgs) -> Result<String> {
    Ok(Prng::new(args.seed).text(1024 * 1024))
}

/// base-004: throw ConvexError with structured data.
#[action]
pub async fn base_004(_ctx: &Ctx, _args: SeedArgs) -> Result<()> {
    Err(ConvexError::new(
        json!({ "code": 42, "detail": "base-004" }),
    ))
}

/// base-005: 100 log lines.
#[action]
pub async fn base_005(_ctx: &Ctx, _args: SeedArgs) -> Result<()> {
    for i in 0..100 {
        log::info(&format!("base-005 log line {i}"));
    }
    Ok(())
}

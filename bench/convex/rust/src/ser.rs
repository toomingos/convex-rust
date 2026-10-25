//! Serialization cases (ser-*): each action receives generated args and echoes
//! them back (Convex-JSON decode + encode of the value).

use convex_fn::{action, Bytes, Ctx, Float64, Int64, Result, Value};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Args<T> {
    pub seed: u32,
    pub payload: T,
}

#[derive(Serialize, Deserialize)]
pub struct Item {
    pub id: u32,
    pub name: String,
    pub score: u32,
    pub x: f64,
}

/// ser-001: 1 KB object.
#[action]
pub async fn ser_001(_ctx: &Ctx, args: Args<Value>) -> Result<Args<Value>> {
    Ok(args)
}

/// ser-002: 100 KB object.
#[action]
pub async fn ser_002(_ctx: &Ctx, args: Args<Value>) -> Result<Args<Value>> {
    Ok(args)
}

/// ser-003: 1 MB object.
#[action]
pub async fn ser_003(_ctx: &Ctx, args: Args<Value>) -> Result<Args<Value>> {
    Ok(args)
}

/// ser-004: 5 MB object.
#[action]
pub async fn ser_004(_ctx: &Ctx, args: Args<Value>) -> Result<Args<Value>> {
    Ok(args)
}

/// ser-005: 64-level nested object.
#[action]
pub async fn ser_005(_ctx: &Ctx, args: Args<Value>) -> Result<Args<Value>> {
    Ok(args)
}

/// ser-006: array of 10k objects.
#[action]
pub async fn ser_006(_ctx: &Ctx, args: Args<Vec<Item>>) -> Result<Args<Vec<Item>>> {
    Ok(args)
}

/// ser-007: 1 MB of bytes.
#[action]
pub async fn ser_007(_ctx: &Ctx, args: Args<Bytes>) -> Result<Args<Bytes>> {
    Ok(args)
}

/// ser-008: int64 values.
#[action]
pub async fn ser_008(_ctx: &Ctx, args: Args<Vec<Int64>>) -> Result<Args<Vec<Int64>>> {
    Ok(args)
}

/// ser-009: float specials.
#[action]
pub async fn ser_009(_ctx: &Ctx, args: Args<Vec<Float64>>) -> Result<Args<Vec<Float64>>> {
    Ok(args)
}

/// ser-010: Unicode-heavy strings.
#[action]
pub async fn ser_010(_ctx: &Ctx, args: Args<Vec<String>>) -> Result<Args<Vec<String>>> {
    Ok(args)
}

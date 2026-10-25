use convex_fn::{action, json, Ctx, Result, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[action]
pub async fn probe(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    Ok(json!({}))
}

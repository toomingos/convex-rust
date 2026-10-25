use convex_fn::{action, Ctx, Result, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Args {}

#[action]
pub async fn shared(_ctx: &Ctx, _a: Args) -> Result<String> {
    Ok("alpha".into())
}

#[action]
pub async fn only_alpha(_ctx: &Ctx, a: Value) -> Result<String> {
    // Big enough that the code is visible in the size.
    let v: Vec<serde_json::Value> = serde_json::from_value(a).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
}

use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Args {}

#[action]
pub async fn only_gamma(_ctx: &Ctx, _a: Args) -> Result<String> {
    Ok("gamma".into())
}

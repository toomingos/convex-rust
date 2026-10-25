use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[action]
pub async fn hi(_ctx: &Ctx, _a: NoArgs) -> Result<String> {
    Ok("hi".into())
}

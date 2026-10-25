use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Args {}

// Same name as alpha::shared: both stay in each other's build; the CLI keeps only its own module's.
#[action]
pub async fn shared(_ctx: &Ctx, _a: Args) -> Result<String> {
    Ok("beta".into())
}

#[action(internal)]
pub async fn only_beta(_ctx: &Ctx, _a: Args) -> Result<f64> {
    Ok(2.0)
}

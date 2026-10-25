use convex_fn::{action, log, ConvexArgs, ConvexValue, Ctx, Error, Result};
use serde::{Deserialize, Serialize};

use crate::api;

#[derive(Deserialize, ConvexArgs)]
pub struct NoArgs {}

/// Counts the documents returned by the `messages:list` query, through the
/// generated, typed `api` reference.
#[action]
pub async fn hello(ctx: &Ctx, _args: NoArgs) -> Result<String> {
    let docs = ctx
        .run_query(api::messages::list, api::messages::ListArgs {})
        .await?;
    let n = docs.as_array().map_or(0, Vec::len);
    log::info(&format!("hello: runQuery returned {n} docs"));
    Ok(format!("hello from rust: {n} messages"))
}

#[derive(Deserialize, ConvexArgs)]
pub struct FailArgs {
    pub reason: String,
}

/// Always fails, to exercise the error path.
#[action]
pub async fn fail(_ctx: &Ctx, args: FailArgs) -> Result<()> {
    log::error(&format!("about to fail: {}", args.reason));
    Err(Error::msg(format!("boom: {}", args.reason)))
}

#[derive(Deserialize, ConvexArgs)]
#[serde(rename_all = "camelCase")]
pub struct GreetArgs {
    pub name: String,
    pub shout_times: Option<f64>,
    #[convex(id = "messages")]
    pub about_message: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Serialize, ConvexValue)]
#[serde(rename_all = "camelCase")]
pub struct Greeting {
    pub text: String,
    pub length: f64,
}

/// Typed args and a typed return value; the manifest carries both validators.
#[action]
pub async fn greet(_ctx: &Ctx, args: GreetArgs) -> Result<Greeting> {
    let times = args.shout_times.unwrap_or(1.0) as usize;
    let text = format!("hello {}{}", args.name, "!".repeat(times));
    Ok(Greeting {
        length: text.len() as f64,
        text,
    })
}

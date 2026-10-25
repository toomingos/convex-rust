//! ctx round trips from a native worker: runQuery, runMutation, fetch, storage, scheduler.
use convex_fn::{action, json, Ctx, Error, FetchRequest, Result, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Args {
    tag: String,
    #[serde(rename = "mockUrl")]
    mock_url: String,
}

/// Exercises every ctx surface once and reports what came back.
#[action]
pub async fn all(ctx: &Ctx, a: Args) -> Result<Value> {
    let doubled: Value = ctx.run_query("data:double", json!({ "n": 21 })).await?;
    let _id: Value = ctx.run_mutation("data:addNote", json!({ "tag": a.tag, "from": "native" })).await?;
    let notes: Value = ctx.run_query("data:notes", json!({ "tag": a.tag })).await?;

    let f = ctx.fetch(FetchRequest::get(format!("{}/bytes/4096?seed=7", a.mock_url))).await?;

    let id = ctx.storage_store(b"native storage payload", Some("text/plain")).await?;
    let got = ctx.storage_get(&id).await?.ok_or_else(|| Error::msg("storage.get returned null"))?;
    let url = ctx.storage().get_url(&id).await?;
    let via_url = match &url {
        Some(u) => Some(ctx.fetch(FetchRequest::get(u.clone())).await?.body.len()),
        None => None,
    };
    ctx.storage().delete(&id).await?;
    let after = ctx.storage_get(&id).await?;

    let sched = ctx
        .scheduler()
        .run_after(50.0, "data:addNote", json!({ "tag": a.tag, "from": "scheduled-native" }))
        .await?;

    Ok(json!({
        "doubled": doubled,
        "notes": notes,
        "fetchStatus": f.status,
        "fetchLen": f.body.len(),
        "storageLen": got.data.len(),
        "storageType": got.content_type,
        "viaUrlLen": via_url,
        "afterDelete": after.map(|_| 1),
        "scheduledIdIsString": !sched.is_empty(),
    }))
}

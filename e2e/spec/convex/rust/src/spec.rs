//! Rust twins of convex/nodeSpec.ts: same names, same argument and result shapes.
use base64::Engine;
use convex_fn::{action, json, log, ConvexError, Ctx, Error, FetchRequest, Result, Value};
use serde::Deserialize;

fn fnv(b: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for x in b {
        h = (h ^ *x as u32).wrapping_mul(0x01000193);
    }
    h
}
fn unb64(s: &str) -> Result<Vec<u8>> {
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}
fn header(h: &[(String, String)], name: &str) -> Value {
    h.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| Value::String(v.clone()))
        .unwrap_or(Value::Null)
}

#[derive(Deserialize)]
pub struct EchoArgs {
    value: Value,
}
#[action]
pub async fn echo(_ctx: &Ctx, a: EchoArgs) -> Result<Value> {
    Ok(json!({ "value": a.value }))
}

#[derive(Deserialize)]
pub struct NoArgs {}
#[action]
pub async fn ping(_ctx: &Ctx, _a: NoArgs) -> Result<String> {
    Ok("pong".into())
}
#[action(internal)]
pub async fn internal_ping(_ctx: &Ctx, _a: NoArgs) -> Result<String> {
    Ok("pong".into())
}

#[derive(Deserialize, Clone)]
pub struct CallArgs {
    kind: String,
    name: String,
    args: Value,
}
async fn do_call(ctx: &Ctx, a: &CallArgs) -> Result<Value> {
    match a.kind.as_str() {
        "query" => ctx.run_query(&a.name, a.args.clone()).await,
        "mutation" => ctx.run_mutation(&a.name, a.args.clone()).await,
        _ => ctx.run_action(&a.name, a.args.clone()).await,
    }
}
#[action]
pub async fn call(ctx: &Ctx, a: CallArgs) -> Result<Value> {
    do_call(ctx, &a).await
}
#[action]
pub async fn call_catch(ctx: &Ctx, a: CallArgs) -> Result<Value> {
    // Goes through an action hop like the node twin, so both see the same error shape.
    Ok(match ctx.run_action::<_, Value>("rust/spec:call", json!({"kind": a.kind, "name": a.name, "args": a.args})).await {
        Ok(v) => json!({ "ok": true, "value": v }),
        Err(Error::Convex { message, data }) => {
            json!({ "ok": false, "isConvexError": true, "data": data, "message": message })
        }
        Err(Error::Message(message)) => {
            json!({ "ok": false, "isConvexError": false, "data": null, "message": message })
        }
    })
}

#[derive(Deserialize)]
pub struct FailArgs {
    message: String,
}
#[action]
pub async fn fail(_ctx: &Ctx, a: FailArgs) -> Result<()> {
    Err(Error::msg(a.message))
}
#[derive(Deserialize)]
pub struct FailConvexArgs {
    data: Value,
}
#[action]
pub async fn fail_convex(_ctx: &Ctx, a: FailConvexArgs) -> Result<()> {
    Err(ConvexError::new(a.data))
}
#[action]
pub async fn crash(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    panic!("kaboom");
}
#[derive(Deserialize)]
pub struct StrictArgs {
    n: f64,
}
#[action]
pub async fn strict_args(_ctx: &Ctx, a: StrictArgs) -> Result<f64> {
    Ok(a.n * 2.0)
}

#[derive(Deserialize)]
pub struct ScheduleArgs {
    kind: String,
    target: String,
    args: Value,
    #[serde(rename = "delayMs")]
    delay_ms: f64,
}
#[action]
pub async fn schedule(ctx: &Ctx, a: ScheduleArgs) -> Result<Value> {
    let id = if a.kind == "at" {
        ctx.scheduler().run_at(ctx.now_ms() + a.delay_ms, &a.target, a.args).await?
    } else {
        ctx.scheduler().run_after(a.delay_ms, &a.target, a.args).await?
    };
    Ok(json!({ "idIsString": !id.is_empty() }))
}
#[derive(Deserialize)]
pub struct RecordArgs {
    tag: String,
}
#[action]
pub async fn record(ctx: &Ctx, a: RecordArgs) -> Result<()> {
    let _: Value = ctx
        .run_mutation("data:addNote", json!({ "tag": a.tag, "from": "rust" }))
        .await?;
    Ok(())
}

#[action]
pub async fn whoami(ctx: &Ctx, _a: NoArgs) -> Result<Option<Value>> {
    ctx.user_identity().await
}

#[derive(Deserialize)]
pub struct StorageArgs {
    data: String,
    #[serde(rename = "contentType")]
    content_type: String,
}
#[action]
pub async fn storage_roundtrip(ctx: &Ctx, a: StorageArgs) -> Result<Value> {
    let bytes = unb64(&a.data)?;
    let id = ctx.storage_store(&bytes, Some(&a.content_type)).await?;
    let storage = ctx.storage();
    let url = storage.get_url(&id).await?.map_or(Value::Null, Value::String);
    let meta = storage.get_metadata(&id).await?;
    let meta = match &meta {
        Some(m) => json!({ "size": m.size, "contentType": m.content_type, "sha256": m.sha256, "storageId": m.storage_id }),
        None => Value::Null,
    };
    let got = ctx.storage_get(&id).await?.ok_or_else(|| Error::msg("storage.get returned null"))?;
    let via_url = ctx.fetch(FetchRequest::get(url.as_str().unwrap_or_default())).await?;
    storage.delete(&id).await?;
    let after_get = ctx.storage_get(&id).await?;
    let after_url = storage.get_url(&id).await?.map_or(Value::Null, Value::String);
    let after_meta = storage.get_metadata(&id).await?.map_or(Value::Null, |_| json!(1));
    Ok(json!({
        "idIsString": true,
        "urlIsString": url.is_string(),
        "meta": {
            "size": meta["size"],
            "contentType": meta["contentType"],
            "sha256Len": meta["sha256"].as_str().map_or(0, str::len),
            "keys": meta.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
        },
        "gotLen": got.data.len(),
        "gotSum": fnv(&got.data),
        // Node's Blob.type is "" when there is no content type.
        "gotContentType": got.content_type.unwrap_or_default(),
        "viaUrlSum": fnv(&via_url.body),
        "afterDelete": { "get": after_get.map(|_| 1), "url": after_url, "meta": after_meta },
    }))
}

#[derive(Deserialize)]
pub struct UrlArgs {
    url: String,
}
#[action]
pub async fn fetch_get(ctx: &Ctx, a: UrlArgs) -> Result<Value> {
    let r = ctx.fetch(FetchRequest::get(a.url)).await?;
    Ok(json!({
        "status": r.status, "ok": (200..300).contains(&r.status), "url": r.url,
        "len": r.body.len(), "sum": fnv(&r.body), "contentType": header(&r.headers, "content-type"),
    }))
}
#[derive(Deserialize)]
pub struct PostArgs {
    url: String,
    body: String,
}
#[action]
pub async fn fetch_post(ctx: &Ctx, a: PostArgs) -> Result<Value> {
    let r = ctx
        .fetch(FetchRequest::post(a.url, unb64(&a.body)?).header("x-test", "1"))
        .await?;
    Ok(json!({ "status": r.status, "json": r.json::<Value>()? }))
}
#[action]
pub async fn fetch_stream(ctx: &Ctx, a: UrlArgs) -> Result<Value> {
    let mut s = ctx.fetch_stream(FetchRequest::get(a.url)).await?;
    let (mut len, mut chunks, mut h) = (0usize, 0usize, 0x811c9dc5u32);
    while let Some(c) = s.chunk().await? {
        chunks += 1;
        len += c.len();
        for x in &c {
            h = (h ^ *x as u32).wrapping_mul(0x01000193);
        }
    }
    Ok(json!({ "status": s.status, "len": len, "sum": h, "multiChunk": chunks > 1 }))
}
#[action]
pub async fn fetch_fail(ctx: &Ctx, a: UrlArgs) -> Result<Value> {
    Ok(json!({ "threw": ctx.fetch(FetchRequest::get(a.url)).await.is_err() }))
}

#[derive(Deserialize)]
pub struct EnvArgs {
    name: String,
}
#[action]
pub async fn read_env(_ctx: &Ctx, a: EnvArgs) -> Result<Option<String>> {
    log::info(&format!("read_env {}", a.name));
    Ok(std::env::var(&a.name).ok())
}

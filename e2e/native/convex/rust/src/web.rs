//! `#[http_action]` handlers on the native runtime (routed from convex/http.ts via `rustHttpAction`).
use convex_fn::{http_action, Ctx, Error, HttpRequest, HttpResponse, Result, Value};

/// JSON route: echoes method, path, query and a header.
#[http_action]
pub async fn info(_ctx: &Ctx, req: HttpRequest) -> Result<HttpResponse> {
    HttpResponse::json(&serde_json::json!({
        "method": req.method,
        "path": req.path(),
        "query": req.query(),
        "x": req.header("x-test"),
    }))
    .map(|r| r.status(201).header("x-reply", "native"))
}

/// POST echo: the body bytes come back as they went in.
#[http_action]
pub async fn echo(_ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
    let ct = req.header("content-type").unwrap_or("application/octet-stream").to_string();
    let body = req.bytes().await?;
    Ok(HttpResponse::ok().header("content-type", ct).body(body))
}

/// Calls back into the deployment from an HTTP action.
#[http_action]
pub async fn double(ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
    let v: Value = req.json().await?;
    let n = v["n"].as_i64().unwrap_or(0);
    let out: Value = ctx.run_query("data:double", serde_json::json!({ "n": n })).await?;
    HttpResponse::json(&out)
}

/// Streaming response; the native runtime collects it and sends it whole.
#[http_action]
pub async fn stream(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::ok().header("content-type", "text/plain").streaming(|w| async move {
        for i in 0..3 {
            w.write_str(&format!("tick {i}\n")).await?;
        }
        Ok(())
    }))
}

/// Handler error.
#[http_action]
pub async fn fail(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
    Err(Error::msg("http boom"))
}

/// Handler panic.
#[http_action]
pub async fn panics(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
    panic!("http panic marker");
}

//! HTTP action fixtures for the host tests (test/http.test.ts).
pub mod h {
    use convex_fn::{http_action, json, Ctx, HttpRequest, HttpResponse, Result, Value};

    /// Buffered: echoes method, URL, a header and the whole body.
    #[http_action]
    pub async fn echo(_ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
        let head = format!(
            "{} {} ct={} body=",
            req.method,
            req.url,
            req.header("content-type").unwrap_or("-")
        );
        let body = req.bytes().await?;
        let mut out = head.into_bytes();
        out.extend_from_slice(&body);
        Ok(HttpResponse::new(201).header("x-echo", "1").body(out))
    }

    /// Buffered JSON that goes through `ctx`.
    #[http_action]
    pub async fn whoami(ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
        let who: Value = ctx.run_query("users:me", json!({})).await?;
        HttpResponse::json(&json!({ "who": who }))
    }

    /// Streams: upper-cases request chunks as they arrive, then a trailer.
    #[http_action]
    pub async fn shout(_ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
        Ok(HttpResponse::ok()
            .header("content-type", "text/plain")
            .streaming(move |w| async move {
                w.write_str("start\n").await?;
                while let Some(chunk) = req.chunk().await? {
                    w.write(String::from_utf8_lossy(&chunk).to_uppercase()).await?;
                }
                w.write_str("\nend").await
            }))
    }

    /// Starts a stream, then fails.
    #[http_action]
    pub async fn broken(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
        Ok(HttpResponse::ok().streaming(|w| async move {
            w.write_str("partial").await?;
            Err(convex_fn::Error::msg("upstream died"))
        }))
    }

    /// Fails before answering.
    #[http_action]
    pub async fn fails(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
        Err(convex_fn::Error::msg("handler failed"))
    }

    /// 204 with no body.
    #[http_action]
    pub async fn no_content(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
        Ok(HttpResponse::new(204))
    }
}

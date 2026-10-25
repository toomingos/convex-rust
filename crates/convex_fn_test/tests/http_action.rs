use convex_fn::{http_action, json, Ctx, HttpRequest, HttpResponse, Result, Value};
use convex_fn_test::{call_http, HttpTestRequest, TestHost};

#[http_action]
async fn echo(_ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
    let ct = req.header("content-type").unwrap_or("none").to_string();
    let path = req.path().to_string();
    let body = req.bytes().await?;
    Ok(HttpResponse::ok()
        .header("x-path", path)
        .header("x-ct", ct)
        .body(body))
}

#[http_action]
async fn relay(ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
    let who: Value = ctx.run_query("users:me", json!({})).await?;
    let mut first = vec![];
    while let Some(c) = req.chunk().await? {
        first.push(c);
        if first.len() == 2 {
            break;
        }
    }
    Ok(HttpResponse::ok()
        .header("content-type", "text/event-stream")
        .streaming(move |w| async move {
            w.write_str(&format!("hello {who}\n")).await?;
            for c in first {
                w.write(c).await?;
            }
            w.write_str("bye\n").await
        }))
}

#[http_action]
async fn failing_stream(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::ok().streaming(|w| async move {
        w.write_str("partial").await?;
        Err(convex_fn::Error::msg("upstream died"))
    }))
}

#[http_action]
async fn bad_status(_ctx: &Ctx, _req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::new(42))
}

#[test]
fn buffered_response_echoes_the_request() {
    let host = TestHost::new();
    let req = HttpTestRequest::new("POST", "https://x.convex.site/e/f?q=1")
        .header("content-type", "text/plain")
        .body_chunks([b"ab".to_vec(), b"cd".to_vec()]);
    let r = call_http!(host, echo, req).unwrap();
    assert!(!r.streamed);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"abcd");
    assert!(r.headers.contains(&("x-path".into(), "/e/f".into())));
    assert!(r.headers.contains(&("x-ct".into(), "text/plain".into())));
}

#[test]
fn get_without_body_is_empty() {
    let host = TestHost::new();
    let r = call_http!(host, echo, HttpTestRequest::new("GET", "https://x/")).unwrap();
    assert_eq!(r.body, b"");
}

#[test]
fn streaming_response_uses_ctx_and_chunks() {
    let host = TestHost::new().on_query("users:me", |_| Ok::<_, String>(json!("ann")));
    let req = HttpTestRequest::new("POST", "https://x/relay")
        .body_chunks([b"1".to_vec(), b"2".to_vec(), b"3".to_vec()]);
    let r = call_http!(host, relay, req).unwrap();
    assert!(r.streamed);
    assert_eq!(r.chunks.len(), 4);
    assert_eq!(String::from_utf8(r.body).unwrap(), "hello \"ann\"\n12bye\n");
    assert!(r.headers.contains(&("content-type".into(), "text/event-stream".into())));
}

#[test]
fn stream_error_surfaces_as_an_error_envelope() {
    let host = TestHost::new();
    let e = call_http!(host, failing_stream, HttpTestRequest::new("GET", "https://x/")).unwrap_err();
    assert_eq!(e["message"], "upstream died");
}

#[test]
fn invalid_status_is_an_error() {
    let host = TestHost::new();
    let e = call_http!(host, bad_status, HttpTestRequest::new("GET", "https://x/")).unwrap_err();
    assert!(e["message"].as_str().unwrap().contains("invalid HTTP status 42"), "{e}");
}

#[test]
fn manifest_symbol_marks_the_kind() {
    // The native manifest static is `#[export_name]`d; the wasm custom section has the same bytes.
    let m = convex_fn::__rt::manifest_string("app::m", "echo", "public", "http", None, None);
    assert!(m.contains("\"kind\":\"http\""), "{m}");
}

use std::{cell::RefCell, rc::Rc};

use convex_fn::{action, join_all, json, ConvexError, Ctx, FetchRequest, Result, Value};
use convex_fn_test::{call, FetchStubResponse, HostError, TestHost};

#[action]
async fn three_queries(ctx: &Ctx, _args: Value) -> Result<Vec<i64>> {
    let futs = (1..=3).map(|i| ctx.run_query::<_, i64>("m:double", json!({ "n": i })));
    join_all(futs).await.into_iter().collect()
}

#[action]
async fn mixed(ctx: &Ctx, _args: Value) -> Result<Value> {
    let q: Value = ctx.run_query("q:get", json!({"a": 1})).await?;
    let m: Value = ctx.run_mutation("m:set", json!({"b": 2})).await?;
    let a: Value = ctx.run_action("a:go", json!({"c": 3})).await?;
    Ok(json!([q, m, a]))
}

#[action]
async fn failing(ctx: &Ctx, _args: Value) -> Result<Value> {
    ctx.run_mutation("m:fail", json!({})).await
}

#[action]
async fn plain_failing(ctx: &Ctx, _args: Value) -> Result<Value> {
    ctx.run_query("q:fail", json!({})).await
}

#[action]
async fn unregistered(ctx: &Ctx, _args: Value) -> Result<Value> {
    ctx.run_query("q:nope", json!({})).await
}

#[action]
async fn throws_convex(_ctx: &Ctx, _args: Value) -> Result<Value> {
    Err(ConvexError::new(json!({"code": 7})))
}

#[action]
async fn scheduling(ctx: &Ctx, _args: Value) -> Result<Vec<String>> {
    let a = ctx.schedule_at(5_000.0, "s:a", json!({"x": 1})).await?;
    let b = ctx.schedule_after(250.0, "s:b", json!({})).await?;
    Ok(vec![a, b])
}

#[action]
async fn scheduling_ms(ctx: &Ctx, _args: Value) -> Result<String> {
    ctx.scheduler().run_at(1_729_700_000_500.0, "s:ms", json!({})).await
}

#[action]
async fn storage_roundtrip(ctx: &Ctx, args: Value) -> Result<Value> {
    let data = vec![7u8; 100];
    let id = ctx.storage_store(&data, Some("application/x-test")).await?;
    let back = ctx.storage_get(&id).await?.expect("stored");
    let missing = ctx.storage_get("nope").await?;
    let preset = match args["preset"].as_str() {
        Some(p) => ctx.storage_get(p).await?.map(|f| (f.content_type, f.data)),
        None => None,
    };
    Ok(json!({
        "same": back.data == data,
        "ct": back.content_type,
        "missing": missing.is_none(),
        "preset": preset.map(|(ct, d)| json!({"ct": ct, "len": d.len()})),
    }))
}

#[action]
async fn fetching(ctx: &Ctx, _args: Value) -> Result<Value> {
    let r = ctx
        .fetch(FetchRequest::post("https://example.test/echo", b"ping".to_vec()).header("x-a", "b"))
        .await?;
    Ok(json!({ "status": r.status, "text": r.text()?, "headers": r.headers, "url": r.url }))
}

#[action]
async fn fetch_nobody(ctx: &Ctx, _args: Value) -> Result<u16> {
    Ok(ctx
        .fetch(FetchRequest::get("https://example.test/empty"))
        .await?
        .status)
}

#[action]
async fn sleeper(ctx: &Ctx, _args: Value) -> Result<Value> {
    let t0 = ctx.sync_syscall("now", json!({}))?;
    ctx.sleep(100).await?;
    ctx.sleep(50).await?;
    let t1 = ctx.sync_syscall("now", json!({}))?;
    Ok(json!([t0, t1]))
}

#[action]
async fn identity(ctx: &Ctx, _args: Value) -> Result<Option<Value>> {
    ctx.user_identity().await
}

#[action]
async fn component(ctx: &Ctx, _args: Value) -> Result<Value> {
    Ok(json!([
        ctx.sync_syscall("componentArgument", json!({"name": "k"}))?,
        ctx.sync_syscall("componentArgument", json!({"name": "other"}))?,
    ]))
}

#[action]
async fn bad_sync(ctx: &Ctx, _args: Value) -> Result<Value> {
    ctx.sync_syscall("bogus", json!({}))
}

#[action]
async fn custom_syscall(ctx: &Ctx, _args: Value) -> Result<Value> {
    ctx.syscall("1.0/custom", json!({"v": 1})).await
}

fn doubler(host: TestHost) -> TestHost {
    host.on_query("m:double", |a| {
        Ok::<_, String>(json!(a["n"].as_i64().unwrap() * 2))
    })
}

#[test]
fn join_all_of_three_queries() {
    let host = doubler(TestHost::new());
    let out = call!(host, three_queries, json!({}));
    assert_eq!(out, json!({"ok": [2, 4, 6]}));
    let calls = host.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls
        .iter()
        .all(|c| c.kind == "query" && c.name == "m:double"));
}

#[test]
fn join_all_with_reverse_delivery() {
    let host = doubler(TestHost::new()).with_reverse_delivery();
    assert_eq!(
        call!(host, three_queries, json!({})),
        json!({"ok": [2, 4, 6]})
    );
}

#[test]
fn registry_kinds_and_call_log() {
    let host = TestHost::new()
        .on_query("q:get", |a| Ok::<_, String>(json!({"q": a["a"]})))
        .on_mutation("m:set", |a| Ok::<_, String>(json!({"m": a["b"]})))
        .on_action("a:go", |a| Ok::<_, String>(json!({"a": a["c"]})));
    let out = call!(host, mixed, json!({}));
    assert_eq!(out, json!({"ok": [{"q": 1}, {"m": 2}, {"a": 3}]}));
    let kinds: Vec<_> = host
        .calls()
        .into_iter()
        .map(|c| (c.kind, c.name, c.args))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("query".into(), "q:get".into(), json!({"a": 1})),
            ("mutation".into(), "m:set".into(), json!({"b": 2})),
            ("action".into(), "a:go".into(), json!({"c": 3})),
        ]
    );
}

#[test]
fn convex_error_data_round_trips() {
    let host = TestHost::new()
        .on_mutation("m:fail", |_| {
            Err::<Value, _>(HostError::convex(json!({"code": "nope", "n": 3})))
        })
        .on_query("q:fail", |_| Err::<Value, _>("boom"));
    let out = call!(host, failing, json!({}));
    assert_eq!(out["error"]["kind"], "convex");
    assert_eq!(out["error"]["data"], json!({"code": "nope", "n": 3}));
    let out = call!(host, plain_failing, json!({}));
    assert_eq!(out["error"]["message"], "boom");
    assert_eq!(out["error"]["kind"], "user");
    let out = call!(host, throws_convex, json!({}));
    assert_eq!(out["error"]["data"], json!({"code": 7}));
}

#[test]
fn unregistered_function_errors() {
    let out = call!(TestHost::new(), unregistered, json!({}));
    assert!(out["error"]["message"].as_str().unwrap().contains("q:nope"));
}

/// F4: `run_at` keeps milliseconds; the wire `ts` is fractional seconds (what JS `runAt(ms)` sends).
#[test]
fn run_at_preserves_milliseconds_on_the_wire() {
    let host = TestHost::new();
    let out = call!(host, scheduling_ms, json!({}));
    assert_eq!(out, json!({"ok": "scheduled_1"}));
    assert_eq!(host.scheduled()[0].ts, 1_729_700_000.5);
}

#[test]
fn scheduler_records_and_returns_ids() {
    let host = TestHost::new();
    let out = call!(host, scheduling, json!({}));
    assert_eq!(out, json!({"ok": ["scheduled_1", "scheduled_2"]}));
    let s = host.scheduled();
    assert_eq!(s.len(), 2);
    assert_eq!(
        (s[0].name.as_str(), s[0].ts, s[0].args.clone()),
        ("s:a", 5.0, json!({"x": 1}))
    );
    // `Ctx::schedule_after` reads the real system clock (SDK `now_ms`), not the
    // virtual `now` sync op, so only check it is in the future of "now" here.
    assert_eq!(s[1].name, "s:b");
    assert!(s[1].ts > 1.0e9);
}

#[test]
fn storage_store_and_get() {
    let host = TestHost::new();
    let preset = host.storage_put(vec![1u8; 10], Some("text/plain"));
    let out = call!(host, storage_roundtrip, json!({ "preset": preset }));
    assert_eq!(
        out,
        json!({"ok": {
            "same": true, "ct": "application/x-test", "missing": true,
            "preset": {"ct": "text/plain", "len": 10},
        }})
    );
}

#[test]
fn fetch_body_arrives_in_multiple_chunks() {
    let seen = Rc::new(RefCell::new(vec![]));
    let seen2 = seen.clone();
    let host = TestHost::new().on_fetch(move |req| {
        seen2.borrow_mut().push((
            req.url.clone(),
            req.method.clone(),
            req.headers.clone(),
            req.body.clone(),
        ));
        FetchStubResponse::new(200, "0123456789abcdef-multi-chunk-body")
            .header("content-type", "text/plain")
    });
    let out = call!(host, fetching, json!({}));
    assert_eq!(
        out,
        json!({"ok": {
            "status": 200,
            "text": "0123456789abcdef-multi-chunk-body",
            "headers": [["content-type", "text/plain"]],
            "url": "https://example.test/echo",
        }})
    );
    let seen = seen.borrow();
    assert_eq!(seen[0].0, "https://example.test/echo");
    assert_eq!(seen[0].1, "POST");
    assert_eq!(seen[0].2, vec![("x-a".to_string(), "b".to_string())]);
    assert_eq!(seen[0].3, b"ping");
}

#[test]
fn fetch_chunks_are_split_and_reverse_delivery_works() {
    let host = TestHost::new()
        .on_fetch(|_| FetchStubResponse::new(200, vec![b'x'; 30]))
        .with_reverse_delivery();
    let out = call!(host, fetching, json!({}));
    assert_eq!(out["ok"]["text"].as_str().unwrap().len(), 30);
}

#[test]
fn fetch_empty_body_and_unhandled() {
    let host = TestHost::new().on_fetch(|_| FetchStubResponse::new(204, vec![]));
    assert_eq!(call!(host, fetch_nobody, json!({})), json!({"ok": 204}));
    let out = call!(TestHost::new(), fetch_nobody, json!({}));
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no fetch handler"));
}

#[test]
fn sleep_is_virtual_and_advances_now() {
    let host = TestHost::new().with_now_ms(1000.0);
    let out = call!(host, sleeper, json!({}));
    assert_eq!(out, json!({"ok": [1000.0, 1150.0]}));
    assert_eq!(host.slept_ms(), 150);
    assert_eq!(host.now_ms(), 1150.0);
}

#[test]
fn identity_and_component_args() {
    let host = TestHost::new();
    assert_eq!(call!(host, identity, json!({})), json!({"ok": null}));
    let host = TestHost::new().with_identity(json!({"subject": "u1"}));
    assert_eq!(
        call!(host, identity, json!({})),
        json!({"ok": {"subject": "u1"}})
    );
    let host = TestHost::new().with_component_arg("k", json!("v"));
    assert_eq!(
        call!(host, component, json!({})),
        json!({"ok": ["v", null]})
    );
}

#[test]
fn unknown_sync_op_errors_and_on_syscall_still_works() {
    let out = call!(TestHost::new(), bad_sync, json!({}));
    assert!(out["error"]["message"].as_str().unwrap().contains("bogus"));
    let host = TestHost::new().on_syscall("1.0/custom", |a| Ok(json!({"echo": a["v"]})));
    assert_eq!(
        call!(host, custom_syscall, json!({})),
        json!({"ok": {"echo": 1}})
    );
    let out = call!(TestHost::new(), custom_syscall, json!({}));
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("1.0/custom"));
}

#[action]
async fn redirecting(ctx: &Ctx, args: Value) -> Result<Value> {
    let mut req = match args["method"].as_str() {
        Some("POST") => FetchRequest::post(args["url"].as_str().unwrap(), b"payload".to_vec())
            .header("authorization", "secret")
            .header("content-type", "text/plain"),
        _ => FetchRequest::get(args["url"].as_str().unwrap()),
    };
    if args["manual"].as_bool() == Some(true) {
        req = req.redirect(convex_fn::Redirect::Manual);
    }
    let r = ctx.fetch(req).await?;
    Ok(json!({"status": r.status, "url": r.url, "text": r.text()?}))
}

type Seen = Rc<RefCell<Vec<(String, String, usize, bool)>>>;

fn redirect_host(seen: Seen) -> TestHost {
    TestHost::new().on_fetch(move |req| {
        let has_auth = req.headers.iter().any(|(k, _)| k == "authorization");
        seen.borrow_mut().push((
            req.url.clone(),
            req.method.clone(),
            req.body.len(),
            has_auth,
        ));
        let path = req
            .url
            .split("example.test")
            .nth(1)
            .unwrap_or("")
            .to_string();
        let redir = |status, loc: &str| FetchStubResponse::new(status, "").header("Location", loc);
        match path.as_str() {
            "/r302" => redir(302, "/final?x=1"),
            "/r307" => redir(307, "/final"),
            "/cross" => redir(307, "https://other.test/final"),
            "/loop" => redir(302, "/loop"),
            _ => FetchStubResponse::new(200, "done"),
        }
    })
}

#[test]
fn fetch_follows_redirects_like_node() {
    let seen: Seen = Rc::new(RefCell::new(vec![]));
    let host = redirect_host(seen.clone());
    let out = call!(
        host,
        redirecting,
        json!({"url": "https://example.test/r302", "method": "POST"})
    );
    assert_eq!(
        out,
        json!({"ok": {"status": 200, "url": "https://example.test/final?x=1", "text": "done"}})
    );
    // 302 on POST becomes GET without a body; same-origin keeps authorization.
    assert_eq!(
        seen.borrow()[1],
        (
            "https://example.test/final?x=1".into(),
            "GET".into(),
            0,
            true
        )
    );

    let seen: Seen = Rc::new(RefCell::new(vec![]));
    let host = redirect_host(seen.clone());
    call!(
        host,
        redirecting,
        json!({"url": "https://example.test/r307", "method": "POST"})
    );
    assert_eq!(
        seen.borrow()[1],
        ("https://example.test/final".into(), "POST".into(), 7, true)
    );

    // Cross-origin hops drop credentials.
    let seen: Seen = Rc::new(RefCell::new(vec![]));
    let host = redirect_host(seen.clone());
    call!(
        host,
        redirecting,
        json!({"url": "https://example.test/cross", "method": "POST"})
    );
    assert_eq!(
        seen.borrow()[1],
        ("https://other.test/final".into(), "POST".into(), 7, false)
    );
}

#[test]
fn fetch_redirect_manual_and_limit() {
    let seen: Seen = Rc::new(RefCell::new(vec![]));
    let host = redirect_host(seen.clone());
    let out = call!(
        host,
        redirecting,
        json!({"url": "https://example.test/r302", "manual": true})
    );
    assert_eq!(out["ok"]["status"], 302);
    assert_eq!(seen.borrow().len(), 1);

    let seen: Seen = Rc::new(RefCell::new(vec![]));
    let host = redirect_host(seen.clone());
    let out = call!(
        host,
        redirecting,
        json!({"url": "https://example.test/loop"})
    );
    assert!(
        out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("maximum redirect"),
        "{out}"
    );
    assert_eq!(seen.borrow().len(), 21);
}

#[action]
async fn host_verify(_ctx: &Ctx, _args: Value) -> Result<Value> {
    Ok(json!({
        "supported": convex_fn::crypto::host_supports("verify", "RS256"),
        "verified": convex_fn::crypto::verify("RS256", b"k", b"m", b"s")
            .map_err(|e| e.message().to_string()),
    }))
}

/// A host without the `crypto/*` ops (here: the mock) has no fallback: the
/// guest gets a clear "update the host" error.
#[test]
fn crypto_errors_clearly_without_host_support() {
    let host = TestHost::new();
    let out = call!(host, host_verify, json!({}));
    assert_eq!(out["ok"]["supported"], false);
    let err = out["ok"]["verified"]["Err"].as_str().unwrap();
    assert!(
        err.contains("host lacks crypto/verify RS256") && err.contains("update convex-rust"),
        "{err}"
    );
}

#[action]
async fn addressed(ctx: &Ctx, _args: Value) -> Result<Value> {
    use convex_fn::{ComponentFunctionRef, FunctionHandle};
    let a: Value = ctx
        .run_query(FunctionHandle::new("function://h1"), json!({}))
        .await?;
    let b: Value = ctx
        .run_mutation(
            ComponentFunctionRef::new("_reference/childComponent/c/f"),
            json!({}),
        )
        .await?;
    let id = ctx
        .scheduler()
        .run_after(0.0, FunctionHandle::new("function://h2"), json!({}))
        .await?;
    Ok(json!([a, b, id]))
}

#[test]
fn handles_and_component_refs_are_addressed() {
    let host = TestHost::new()
        .on_query("function://h1", |_| Ok::<_, String>(json!("q")))
        .on_mutation("_reference/childComponent/c/f", |_| {
            Ok::<_, String>(json!("m"))
        });
    let out = call!(host, addressed, json!({}));
    assert_eq!(out, json!({"ok": ["q", "m", "scheduled_1"]}));
    assert_eq!(host.scheduled()[0].name, "function://h2");
}

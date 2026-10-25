use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc};

use convex_fn::{action, json, race, Chunks, Ctx, FetchRequest, Result, Value};
use convex_fn_test::{call, FetchStubResponse, TestHost};

fn big() -> Vec<u8> {
    (0..100_000u32).map(|i| (i % 251) as u8).collect()
}

#[action]
async fn post_big(ctx: &Ctx, _args: Value) -> Result<usize> {
    let r = ctx
        .fetch(FetchRequest::post("https://example.test/up", big()))
        .await?;
    Ok(r.body.len())
}

#[action]
async fn upload_chunks(ctx: &Ctx, _args: Value) -> Result<u16> {
    let parts = vec![b"ab".to_vec(), b"cd".to_vec(), b"ef".to_vec()];
    let r = ctx
        .fetch_upload(
            FetchRequest::get("https://example.test/up").method("PUT"),
            Chunks(parts.into_iter()),
        )
        .await?;
    Ok(r.status)
}

#[action]
async fn store_big(ctx: &Ctx, _args: Value) -> Result<String> {
    ctx.storage_store(&big(), Some("application/octet-stream"))
        .await
}

type Job<'a> = Pin<Box<dyn Future<Output = Result<usize>> + 'a>>;

#[action]
async fn drop_fetch(ctx: &Ctx, _args: Value) -> Result<usize> {
    let quick: Job = Box::pin(async {
        ctx.sleep(1).await?;
        Ok(0)
    });
    let slow: Job = Box::pin(async {
        ctx.fetch(FetchRequest::get("https://example.test/slow"))
            .await
            .map(|_| 1)
    });
    // The first future submitted is delivered first by the mock; the loser is dropped.
    let (_, out) = race([quick, slow]).await;
    out
}

#[test]
fn big_bodies_travel_intact_with_and_without_caps() {
    for caps in [false, true] {
        let seen = Rc::new(RefCell::new(vec![]));
        let s = seen.clone();
        let mut host = TestHost::new().on_fetch(move |req| {
            *s.borrow_mut() = req.body.clone();
            FetchStubResponse::new(200, b"ok".to_vec())
        });
        if caps {
            host = host.with_net_caps();
        }
        assert_eq!(call!(host, post_big, json!({})), json!({"ok": 2}));
        assert_eq!(*seen.borrow(), big());
        let out = call!(host, store_big, json!({}));
        let id = out["ok"].as_str().unwrap().to_string();
        assert_eq!(host.storage_read(&id).unwrap(), big());
    }
}

#[test]
fn upload_without_stream_support_is_buffered() {
    for caps in [false, true] {
        let seen = Rc::new(RefCell::new(vec![]));
        let s = seen.clone();
        let mut host = TestHost::new().on_fetch(move |req| {
            assert_eq!(req.method, "PUT");
            *s.borrow_mut() = req.body.clone();
            FetchStubResponse::new(201, vec![])
        });
        if caps {
            host = host.with_net_caps();
        }
        assert_eq!(call!(host, upload_chunks, json!({})), json!({"ok": 201}));
        assert_eq!(*seen.borrow(), b"abcdef");
    }
}

#[test]
fn dropping_a_fetch_cancels_it_only_when_the_host_supports_it() {
    let host = TestHost::new()
        .with_net_caps()
        .on_fetch(|_| FetchStubResponse::new(200, vec![]));
    assert_eq!(call!(host, drop_fetch, json!({})), json!({"ok": 0}));
    assert_eq!(host.cancelled_tasks().len(), 1);

    let host = TestHost::new().on_fetch(|_| FetchStubResponse::new(200, vec![]));
    assert_eq!(call!(host, drop_fetch, json!({})), json!({"ok": 0}));
    assert!(host.cancelled_tasks().is_empty());
}

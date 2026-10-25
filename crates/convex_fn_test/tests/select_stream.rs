use convex_fn::{json, race, select, Either, FetchRequest, Value};
use convex_fn_test::{FetchStubResponse, TestHost};

#[test]
fn select_first_wins_loser_completes_later() {
    let host = TestHost::new().on_query("q:get", |_| Ok::<_, String>(json!(42)));
    let out = host.run(|ctx| async move {
        // `slow` needs two sequential host round trips, `fast` one: FIFO
        // delivery makes `fast` win, and `slow`'s second task completes later.
        let slow = async {
            ctx.sleep(10).await?;
            ctx.sleep(10).await?;
            Ok::<_, convex_fn::Error>("slow")
        };
        let fast = async {
            ctx.sleep(1).await?;
            Ok::<_, convex_fn::Error>("fast")
        };
        let winner = match select(slow, fast).await {
            Either::Left(v) => v.unwrap(),
            Either::Right(v) => v.unwrap(),
        };
        // The stale completion of the loser is still queued; later awaits must be unaffected.
        let v: Value = ctx.run_query("q:get", json!({})).await.unwrap();
        (winner, v)
    });
    assert_eq!(out, ("fast", json!(42)));
}

#[test]
fn select_left_wins_ties() {
    let host = TestHost::new();
    let out = host.run(|_ctx| async move { select(async { 1 }, async { 2 }).await });
    assert_eq!(out, Either::Left(1));
}

#[test]
fn race_returns_index_and_ignores_losers() {
    let host = TestHost::new()
        .on_query("q:a", |_| Ok::<_, String>(json!("a")))
        .on_query("q:b", |_| Ok::<_, String>(json!("b")));
    let out = host.run(|ctx| async move {
        let ctx = &ctx;
        let futs = vec![
            Box::pin(async move {
                ctx.sleep(1).await.unwrap();
                ctx.sleep(1).await.unwrap();
                ctx.sleep(1).await.unwrap();
                "three".to_string()
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = String>>>,
            Box::pin(async move { ctx.run_query::<_, String>("q:b", json!({})).await.unwrap() }),
        ];
        let (i, v) = race(futs).await;
        // Executor still works afterwards.
        let a: String = ctx.run_query("q:a", json!({})).await.unwrap();
        (i, v, a)
    });
    assert_eq!(out, (1, "b".to_string(), "a".to_string()));
}

#[test]
fn fetch_stream_yields_chunks_in_order() {
    let host = TestHost::new().on_fetch(|_| {
        FetchStubResponse::chunked(200, ["one ", "two ", "three"]).header("x-a", "b")
    });
    let (status, chunks, after) = host.run(|ctx| async move {
        let mut s = ctx
            .fetch_stream(FetchRequest::get("https://x.test/"))
            .await
            .unwrap();
        let mut chunks = vec![];
        while let Some(c) = s.chunk().await.unwrap() {
            chunks.push(String::from_utf8(c).unwrap());
        }
        let after = s.chunk().await.unwrap();
        (s.status, chunks, after)
    });
    assert_eq!(status, 200);
    assert_eq!(chunks, ["one ", "two ", "three"]);
    assert_eq!(after, None);
}

#[test]
fn fetch_stream_error_mid_stream() {
    let host = TestHost::new()
        .on_fetch(|_| FetchStubResponse::chunked(200, ["ab", "cd"]).fail_after("connection reset"));
    let (got, err, after) = host.run(|ctx| async move {
        let mut s = ctx
            .fetch_stream(FetchRequest::get("https://x.test/"))
            .await
            .unwrap();
        let mut got = vec![];
        let err = loop {
            match s.chunk().await {
                Ok(Some(c)) => got.push(c),
                Ok(None) => panic!("expected an error"),
                Err(e) => break e.to_string(),
            }
        };
        (got, err, s.chunk().await.unwrap())
    });
    assert_eq!(got, [b"ab".to_vec(), b"cd".to_vec()]);
    assert!(err.contains("connection reset"), "{err}");
    assert_eq!(after, None);
}

#[test]
fn stream_error_fails_buffered_fetch_too() {
    let host =
        TestHost::new().on_fetch(|_| FetchStubResponse::chunked(200, ["x"]).fail_after("boom"));
    let err = host.run(|ctx| async move {
        ctx.fetch(FetchRequest::get("https://x.test/"))
            .await
            .unwrap_err()
            .to_string()
    });
    assert!(err.contains("boom"), "{err}");
}

#[test]
fn dropping_a_stream_early_does_not_disturb_later_fetches() {
    let host = TestHost::new().on_fetch(|req| {
        if req.url.ends_with("/big") {
            FetchStubResponse::chunked(200, ["1", "2", "3", "4"])
        } else {
            FetchStubResponse::new(200, "small")
        }
    });
    let text = host.run(|ctx| async move {
        let mut s = ctx
            .fetch_stream(FetchRequest::get("https://x.test/big"))
            .await
            .unwrap();
        assert_eq!(s.chunk().await.unwrap(), Some(b"1".to_vec()));
        drop(s);
        ctx.fetch(FetchRequest::get("https://x.test/small"))
            .await
            .unwrap()
            .text()
            .unwrap()
    });
    assert_eq!(text, "small");
}

#[test]
fn fetch_stream_bytes_and_empty_body() {
    let host = TestHost::new().on_fetch(|req| {
        if req.url.ends_with("/empty") {
            FetchStubResponse::new(204, "")
        } else {
            FetchStubResponse::chunked(200, ["ab", "c"])
        }
    });
    let (all, empty) = host.run(|ctx| async move {
        let all = ctx
            .fetch_stream(FetchRequest::get("https://x.test/full"))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let mut e = ctx
            .fetch_stream(FetchRequest::get("https://x.test/empty"))
            .await
            .unwrap();
        (all, e.chunk().await.unwrap())
    });
    assert_eq!(all, b"abc");
    assert_eq!(empty, None);
}

//! DX-06a spike action: tokio APIs inside a `convex_fn` action.
//!
//! Exercised natively (tests/native.rs, mock host) and as wasm32-wasip1
//! (harness/, wasmtime host with real timers).

use std::time::{Duration, Instant};

use convex_fn::{action, json, Ctx, FetchRequest, Result, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

/// `tokio::join!` of two host sleeps, a mock fetch, a `tokio::spawn`ed task and
/// a `tokio::time::sleep`; then `select!`/`timeout` races between a fetch and a
/// tokio timer, and a `spawn_local` + mpsc pipeline.
#[action]
pub async fn probe(ctx: &Ctx, _args: NoArgs) -> Result<Value> {
    let t0 = Instant::now();

    // 1. Two host Sleep tasks, concurrently (wall time ~ max, not sum).
    let (a, b) = tokio::join!(ctx.sleep(60), ctx.sleep(120));
    a?;
    b?;
    let after_sleeps = t0.elapsed();

    // 2. Mock fetch + tokio::spawn'd task + tokio::time::sleep, all at once.
    let spawned = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        7u32
    });
    let (resp, spawned, ()) = tokio::join!(
        ctx.fetch(FetchRequest::get("https://mock.test/fast")),
        spawned,
        tokio::time::sleep(Duration::from_millis(30)),
    );
    let resp = resp?;
    let spawned = spawned.map_err(|e| convex_fn::Error::msg(format!("join: {e}")))?;

    // 3. select!: a fast fetch beats a 500 ms tokio timer.
    let fast = tokio::select! {
        r = ctx.fetch(FetchRequest::get("https://mock.test/fast")) => format!("fetch:{}", r?.status),
        () = tokio::time::sleep(Duration::from_millis(500)) => "timer".to_string(),
    };

    // 4. tokio::time::timeout around a slow fetch (host delays "/slow" by 300 ms;
    //    the native mock completes instantly).
    let slow = match tokio::time::timeout(
        Duration::from_millis(60),
        ctx.fetch(FetchRequest::get("https://mock.test/slow")),
    )
    .await
    {
        Ok(r) => format!("fetch:{}", r?.status),
        Err(_) => "timeout".to_string(),
    };

    // 5. spawn_local + mpsc channel + oneshot.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(4);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<u32>();
    tokio::task::spawn_local(async move {
        let mut sum = 0;
        while let Some(v) = rx.recv().await {
            sum += v;
        }
        let _ = done_tx.send(sum);
    });
    for v in 1..=3 {
        tx.send(v).await.map_err(|e| convex_fn::Error::msg(e.to_string()))?;
        tokio::task::yield_now().await;
    }
    drop(tx);
    let chan_sum = done_rx.await.map_err(|e| convex_fn::Error::msg(e.to_string()))?;

    Ok(json!({
        "afterSleepsMs": after_sleeps.as_millis() as u64,
        "fetchStatus": resp.status,
        "fetchBody": String::from_utf8_lossy(&resp.body),
        "spawned": spawned,
        "fastRace": fast,
        "slowRace": slow,
        "chanSum": chan_sum,
        "totalMs": t0.elapsed().as_millis() as u64,
    }))
}

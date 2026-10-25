//! Fan-out: run many Convex actions (typically internal Rust actions) in parallel with a concurrency limit.
//!
//! ```ignore
//! // 200 chunks, at most 16 actions running at once; results come back in input order.
//! let sums: Vec<f64> = ctx.run_action_each(api::work::sum_chunk, chunks, 16).await?;
//! ```
//!
//! Each element is a separate action execution (`ctx.runAction`), so it runs in its own isolate and gets
//! its own 64 MiB / time budget; the calling action waits for all of them and its wall time counts. Send
//! small inputs as arguments (16 MiB per call at most); put large ones in storage
//! ([`Ctx::storage`]) and pass the storage id. See docs/rust/cloud.md "Fan-out".

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{Ctx, FunctionTarget, Result};

/// Maps `items` through `f`, keeping at most `limit` (at least 1) futures in flight, and returns the outputs
/// in input order. Futures start as earlier ones finish, in input order. Single-threaded (like [`crate::join_all`]).
pub async fn par_map<T, R, Fut>(
    items: impl IntoIterator<Item = T>,
    limit: usize,
    mut f: impl FnMut(T) -> Fut,
) -> Vec<R>
where
    Fut: Future<Output = R>,
{
    let mut iter = items.into_iter();
    let limit = limit.max(1);
    let mut out: Vec<Option<R>> = Vec::new();
    let mut flight: Vec<(usize, Pin<Box<Fut>>)> = Vec::new();
    let mut exhausted = false;
    std::future::poll_fn(|cx| loop {
        while !exhausted && flight.len() < limit {
            match iter.next() {
                Some(t) => {
                    out.push(None);
                    flight.push((out.len() - 1, Box::pin(f(t))));
                }
                None => exhausted = true,
            }
        }
        let mut finished = false;
        let mut i = 0;
        while i < flight.len() {
            match flight[i].1.as_mut().poll(cx) {
                Poll::Ready(v) => {
                    let (idx, _) = flight.swap_remove(i);
                    out[idx] = Some(v);
                    finished = true;
                }
                Poll::Pending => i += 1,
            }
        }
        if flight.is_empty() && exhausted {
            return Poll::Ready(());
        }
        if !finished {
            return Poll::Pending;
        }
    })
    .await;
    out.into_iter()
        .map(|o| o.expect("polled to completion"))
        .collect()
}

impl Ctx {
    /// Runs `target` once per element of `args`, at most `limit` at a time, and returns every outcome in input
    /// order (one failure does not stop the others).
    pub async fn run_action_each_settled<A, R>(
        &self,
        target: impl FunctionTarget<A, R> + Copy,
        args: impl IntoIterator<Item = A>,
        limit: usize,
    ) -> Vec<Result<R>>
    where
        A: Serialize,
        R: DeserializeOwned,
    {
        par_map(args, limit, |a| self.run_action(target, a)).await
    }

    /// Like [`Ctx::run_action_each_settled`], but fails with the first error (in input order) after all
    /// started calls have finished. Calls not yet started when an error occurs are still started: use
    /// the `settled` form and stop yourself if that matters.
    pub async fn run_action_each<A, R>(
        &self,
        target: impl FunctionTarget<A, R> + Copy,
        args: impl IntoIterator<Item = A>,
        limit: usize,
    ) -> Result<Vec<R>>
    where
        A: Serialize,
        R: DeserializeOwned,
    {
        self.run_action_each_settled(target, args, limit)
            .await
            .into_iter()
            .collect()
    }
}

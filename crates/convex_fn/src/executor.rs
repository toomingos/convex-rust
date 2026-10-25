//! Single-threaded executor driven by `task_poll` (contracts/ABI.md).
//!
//! Every pending future is re-polled after each host event, which is simple
//! and correct for the one-call-per-instance model. DX-06a replaces this with
//! a tokio current_thread runtime.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use serde_json::Value;

use crate::{error::Error, net, sys, wire::Incoming, Result};

#[derive(Default)]
struct Stream {
    chunks: VecDeque<Vec<u8>>,
    done: Option<std::result::Result<(), String>>,
}

#[derive(Default)]
struct State {
    results: HashMap<i64, Result<Value>>,
    streams: HashMap<String, Stream>,
    /// Task ids whose future was dropped before completion; late results are discarded.
    abandoned_tasks: HashSet<i64>,
    /// Stream ids whose reader was dropped before the stream ended; late chunks are discarded.
    abandoned_streams: HashSet<String>,
    /// `tokio-rt`: wakers of futures awaiting a task result / stream event.
    #[cfg(feature = "tokio-rt")]
    wakers: HashMap<WakeKey, Waker>,
    /// `tokio-rt`: the host pump task, woken when a waiter registers.
    #[cfg(feature = "tokio-rt")]
    pump: Option<Waker>,
}

#[cfg(feature = "tokio-rt")]
#[derive(PartialEq, Eq, Hash)]
enum WakeKey {
    Task(i64),
    Stream(String),
}

#[cfg(feature = "tokio-rt")]
fn register(s: &mut State, key: WakeKey, cx: &Context<'_>) {
    s.wakers.insert(key, cx.waker().clone());
    if let Some(p) = s.pump.take() {
        p.wake();
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn reset() {
    reset_state();
    net::reset();
}

/// Forgets every task result, stream buffer and abandoned id. The wasm entry
/// point calls this at the start of each call: the host restarts task and
/// stream ids at 0 per call, so on a reused instance anything left over from
/// the previous call (a dropped stream, a late result) must not be matched by
/// a new call's ids. The `net/*` capabilities stay probed (they are per instance).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reset_state() {
    // Take the old state out first so its wakers drop outside the borrow.
    let old = STATE.with(|s| std::mem::take(&mut *s.borrow_mut()));
    drop(old);
}

/// Submits a task request and returns a future for its result.
pub(crate) fn submit(request: &Value) -> TaskFuture {
    let id = sys::submit(request.to_string().as_bytes());
    TaskFuture {
        id,
        finished: false,
        cancel: false,
    }
}

/// Like [`submit`], but dropping the future before it completes also cancels
/// the work on the host (`net/cancel`) when the host supports it.
pub(crate) fn submit_cancellable(request: &Value) -> TaskFuture {
    let mut t = submit(request);
    t.cancel = net::caps().cancel;
    t
}

/// A submitted task. Dropping it before it completes abandons the task: the
/// host result, when it arrives, is discarded instead of piling up.
pub(crate) struct TaskFuture {
    id: i64,
    finished: bool,
    /// Cancel the host task on drop.
    cancel: bool,
}

impl TaskFuture {
    pub(crate) fn id(&self) -> i64 {
        self.id
    }
}

impl Drop for TaskFuture {
    fn drop(&mut self) {
        if self.finished || self.id < 0 {
            return;
        }
        let _ = STATE.try_with(|s| {
            let mut s = s.borrow_mut();
            #[cfg(feature = "tokio-rt")]
            s.wakers.remove(&WakeKey::Task(self.id));
            if s.results.remove(&self.id).is_none() {
                s.abandoned_tasks.insert(self.id);
            }
        });
        if self.cancel {
            net::cancel_task(self.id);
        }
    }
}

impl Future for TaskFuture {
    type Output = Result<Value>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.id < 0 {
            self.finished = true;
            return Poll::Ready(Err(Error::msg("host rejected the task request")));
        }
        let id = self.id;
        match STATE.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.results.remove(&id);
            #[cfg(feature = "tokio-rt")]
            if r.is_none() {
                register(&mut s, WakeKey::Task(id), _cx);
            }
            r
        }) {
            Some(r) => {
                self.finished = true;
                Poll::Ready(r)
            }
            None => Poll::Pending,
        }
    }
}

/// Resolves to the full body of a host stream once its `end` chunk arrives.
/// Dropping it before it completes abandons the stream (late chunks are discarded).
pub(crate) struct StreamFuture {
    stream_id: String,
    finished: bool,
}

impl StreamFuture {
    pub(crate) fn new(stream_id: String) -> Self {
        Self {
            stream_id,
            finished: false,
        }
    }
}

impl Drop for StreamFuture {
    fn drop(&mut self) {
        if !self.finished {
            abandon_stream(&self.stream_id);
        }
    }
}

impl Future for StreamFuture {
    type Output = Result<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let r = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let finished = s
                .streams
                .get(&self.stream_id)
                .is_some_and(|st| st.done.is_some());
            if !finished {
                #[cfg(feature = "tokio-rt")]
                register(&mut s, WakeKey::Stream(self.stream_id.clone()), _cx);
                return Poll::Pending;
            }
            let st = s.streams.remove(&self.stream_id).unwrap_or_default();
            Poll::Ready(match st.done {
                Some(Err(e)) => Err(Error::msg(e)),
                _ => {
                    let mut chunks = st.chunks;
                    if chunks.len() == 1 {
                        Ok(chunks.pop_front().unwrap_or_default())
                    } else {
                        let total: usize = chunks.iter().map(Vec::len).sum();
                        let mut out = Vec::with_capacity(total);
                        for c in &chunks {
                            out.extend_from_slice(c);
                        }
                        Ok(out)
                    }
                }
            })
        });
        if r.is_ready() {
            self.finished = true;
        }
        r
    }
}

/// Resolves to the next chunk of a host stream: `Some(bytes)`, `None` at the
/// end, or the stream error (after all chunks that preceded it).
pub(crate) struct NextChunk<'a> {
    pub(crate) stream_id: &'a str,
}

impl Future for NextChunk<'_> {
    type Output = Result<Option<Vec<u8>>>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let Some(st) = s.streams.get_mut(self.stream_id) else {
                #[cfg(feature = "tokio-rt")]
                register(&mut s, WakeKey::Stream(self.stream_id.to_string()), _cx);
                return Poll::Pending;
            };
            if let Some(chunk) = st.chunks.pop_front() {
                return Poll::Ready(Ok(Some(chunk)));
            }
            match st.done.take() {
                None => {
                    #[cfg(feature = "tokio-rt")]
                    register(&mut s, WakeKey::Stream(self.stream_id.to_string()), _cx);
                    Poll::Pending
                }
                Some(r) => {
                    s.streams.remove(self.stream_id);
                    Poll::Ready(match r {
                        Ok(()) => Ok(None),
                        Err(e) => Err(Error::msg(e)),
                    })
                }
            }
        })
    }
}

/// Stops tracking a stream that will not be read any further; chunks the host
/// still sends for it are discarded.
pub(crate) fn abandon_stream(stream_id: &str) {
    let ended = STATE
        .try_with(|s| {
            let mut s = s.borrow_mut();
            let ended = s
                .streams
                .remove(stream_id)
                .is_some_and(|st| st.done.is_some());
            if !ended {
                s.abandoned_streams.insert(stream_id.to_string());
            }
            ended
        })
        .unwrap_or(true);
    if !ended {
        // Stop the download on the host too (best effort).
        net::cancel_stream(stream_id);
    }
}

fn dispatch(event: Vec<u8>) {
    let Some(ev) = Incoming::parse_owned(event) else {
        return;
    };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        match ev {
            Incoming::Done {
                task_id: id,
                result,
            } => {
                if s.abandoned_tasks.remove(&id) {
                    return;
                }
                let result = result.map_err(|err| Error::from_json(&err));
                s.results.insert(id, result);
                #[cfg(feature = "tokio-rt")]
                if let Some(w) = s.wakers.remove(&WakeKey::Task(id)) {
                    w.wake();
                }
            }
            Incoming::Chunk {
                stream_id: id,
                data,
                error,
                terminal,
            } => {
                if s.abandoned_streams.contains(&id) {
                    if terminal {
                        s.abandoned_streams.remove(&id);
                    }
                    return;
                }
                #[cfg(feature = "tokio-rt")]
                if let Some(w) = s.wakers.remove(&WakeKey::Stream(id.clone())) {
                    w.wake();
                }
                let st = s.streams.entry(id).or_default();
                if let Some(data) = data {
                    st.chunks.push_back(data);
                }
                if let Some(err) = error {
                    st.done = Some(Err(err));
                } else if terminal {
                    st.done = Some(Ok(()));
                }
            }
        }
    });
}

/// Drives `fut` to completion, blocking on the host for task events.
#[cfg(not(feature = "tokio-rt"))]
pub fn block_on<F: Future>(fut: F) -> F::Output {
    block_on_direct(fut)
}

/// Polls `fut` with a no-op waker and blocks in `task_poll(1)` between polls,
/// dispatching every host event to its result/stream buffer (and waking any
/// registered waker). No runtime is involved.
fn block_on_direct<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        match sys::poll(true) {
            Some(event) => {
                dispatch(event);
                // Everything already delivered, before re-polling once.
                while let Some(event) = sys::poll(false) {
                    dispatch(event);
                }
            }
            None => panic!("action is waiting on a future that no host task will complete"),
        }
    }
}

/// Runs futures concurrently and collects their outputs in order
/// (the equivalent of `Promise.all`).
pub async fn join_all<F: Future>(futs: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut futs: Vec<Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = futs.iter().map(|_| None).collect();
    std::future::poll_fn(move |cx| {
        let mut pending = false;
        for (f, slot) in futs.iter_mut().zip(out.iter_mut()) {
            if slot.is_none() {
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => *slot = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(
                out.iter_mut()
                    .map(|s| s.take().expect("polled to completion"))
                    .collect(),
            )
        }
    })
    .await
}

/// The result of [`select`]: which of the two futures finished first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Either<A, B> {
    /// The first future finished first.
    Left(A),
    /// The second future finished first.
    Right(B),
}

/// Races two futures (like `Promise.race`). Resolves with the output of the
/// first one to finish; if both are ready on the same poll, `a` wins. The
/// loser is dropped; any host task it had in flight is abandoned and its late
/// result is discarded, so it cannot wedge or confuse later awaits.
pub async fn select<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let mut a = Box::pin(a);
    let mut b = Box::pin(b);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(v));
        }
        if let Poll::Ready(v) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(v));
        }
        Poll::Pending
    })
    .await
}

/// Races any number of futures. Resolves with `(index, output)` of the first
/// one to finish (lowest index wins ties); all the others are dropped as in
/// [`select`].
///
/// # Panics
///
/// Panics if `futs` is empty, since no future could ever finish.
pub async fn race<F: Future>(futs: impl IntoIterator<Item = F>) -> (usize, F::Output) {
    let mut futs: Vec<Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    assert!(!futs.is_empty(), "race() needs at least one future");
    std::future::poll_fn(move |cx| {
        for (i, f) in futs.iter_mut().enumerate() {
            if let Poll::Ready(v) = f.as_mut().poll(cx) {
                return Poll::Ready((i, v));
            }
        }
        Poll::Pending
    })
    .await
}

/// `tokio-rt`: drives `fut` on a tokio `current_thread` runtime (+ `LocalSet`),
/// so `tokio::spawn`, `spawn_local`, `tokio::time::*`, `select!`, `join!` and
/// tokio channels work inside actions.
///
/// Host events are fed in by a "pump" future running inside the runtime: it
/// drains `task_poll(0)` and, while something awaits a host result, re-checks
/// every `tick` (1 ms backing off to 8 ms) because tokio's own idle park
/// (`poll_oneoff` on the WASI clock) cannot be interrupted by a host event.
///
/// # Nested (blocking) calls
///
/// Called from inside a running action (e.g. `reqwest::blocking` in an
/// `#[action]`), `block_on` cannot start a second tokio runtime, so it drives
/// `fut` directly: submit + `task_poll(block = 1)` in a loop, dispatching all
/// events to their buffers/wakers. The guest is single-threaded, so while a
/// blocking call runs, tasks spawned on the `LocalSet` (and timers) do NOT
/// progress; they resume when the call returns. `fut` must therefore only
/// depend on host tasks (fetch, run_*, sleep...), never on a spawned tokio task
/// or a tokio timer/channel; such a future would wait forever and the call
/// panics ("no host task will complete") once no host task is outstanding.
#[cfg(feature = "tokio-rt")]
pub fn block_on<F: Future>(fut: F) -> F::Output {
    use std::time::Duration;

    thread_local! {
        static IN_RT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if IN_RT.with(|f| f.get()) {
        return block_on_direct(fut);
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IN_RT.with(|f| f.set(false));
        }
    }
    IN_RT.with(|f| f.set(true));
    let _reset = Reset;

    const MIN_TICK: Duration = Duration::from_millis(1);
    const MAX_TICK: Duration = Duration::from_millis(8);

    /// Blocks in `task_poll(1)` until a host event arrives or a host `sleep(tick)` fires,
    /// dispatching what arrived. `Some(true)` if an event came before the sleep, `Some(false)`
    /// if the sleep ended the wait, `None` if the host rejected the sleep.
    fn block_on_host(tick: Duration) -> Option<bool> {
        let mut sleep =
            submit_cancellable(&crate::wire::sleep_request(tick.as_millis().max(1) as u64));
        if sleep.id() < 0 {
            return None;
        }
        if let Some(event) = sys::poll(true) {
            dispatch(event);
        }
        while let Some(event) = sys::poll(false) {
            dispatch(event);
        }
        // Done: consumes the result. Still pending: dropping `sleep` cancels it on the host.
        let mut cx = Context::from_waker(Waker::noop());
        Some(Pin::new(&mut sleep).poll(&mut cx).is_pending())
    }

    async fn pump() {
        let mut tick = MIN_TICK;
        let mut yielded = false;
        loop {
            let mut got = false;
            while let Some(event) = sys::poll(false) {
                dispatch(event);
                got = true;
            }
            if got {
                tick = MIN_TICK;
                yielded = false;
                // Let woken tasks run before we look again.
                tokio::task::yield_now().await;
                continue;
            }
            let waiting = STATE.with(|s| !s.borrow().wakers.is_empty());
            if waiting {
                if net::caps().cancel {
                    // Let ready tasks run once, then block on the host: it wakes us the moment a
                    // result arrives instead of up to `tick` later. The bounded host sleep keeps
                    // tokio timers / spawned tasks served at `tick` granularity (tokio's own park
                    // cannot be interrupted by a host event, so it is not used while waiting).
                    if !yielded {
                        yielded = true;
                        tokio::task::yield_now().await;
                        continue;
                    }
                    yielded = false;
                    if let Some(early) = block_on_host(tick) {
                        tick = if early {
                            MIN_TICK
                        } else {
                            (tick * 2).min(MAX_TICK)
                        };
                        tokio::task::yield_now().await;
                        continue;
                    }
                }
                tokio::time::sleep(tick).await;
                tick = (tick * 2).min(MAX_TICK);
            } else {
                // Nothing awaits the host: sleep until a future registers.
                tick = MIN_TICK;
                std::future::poll_fn(|cx| {
                    STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        if s.wakers.is_empty() {
                            s.pump = Some(cx.waker().clone());
                            Poll::Pending
                        } else {
                            Poll::Ready(())
                        }
                    })
                })
                .await;
            }
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let mut fut = std::pin::pin!(fut);
        let mut pump = std::pin::pin!(pump());
        std::future::poll_fn(|cx| {
            if let Poll::Ready(v) = fut.as_mut().poll(cx) {
                return Poll::Ready(v);
            }
            let _ = pump.as_mut().poll(cx);
            Poll::Pending
        })
        .await
    })
}

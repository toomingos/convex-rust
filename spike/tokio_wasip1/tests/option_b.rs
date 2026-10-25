//! Option B evidence: what works / breaks when futures are polled by an
//! executor that is NOT a tokio runtime (no runtime context, no time driver).

use std::{
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::pin,
    task::{Context, Poll, Waker},
};

fn poll_once<F: Future>(f: F) -> Poll<F::Output> {
    let mut f = pin!(f);
    f.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

fn panic_msg(f: impl FnOnce()) -> Option<String> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(prev);
    r.err().map(|e| {
        e.downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    })
}

#[test]
fn needs_runtime_apis_panic_without_tokio() {
    let spawn = panic_msg(|| {
        let _ = tokio::spawn(async {});
    });
    println!("tokio::spawn      -> {spawn:?}");
    assert!(spawn.unwrap().contains("no reactor running"));

    let sleep = panic_msg(|| {
        let _ = poll_once(tokio::time::sleep(std::time::Duration::from_millis(1)));
    });
    println!("tokio::time::sleep -> {sleep:?}");
    assert!(sleep.unwrap().contains("no reactor running"));

    let timeout = panic_msg(|| {
        let _ = poll_once(tokio::time::timeout(std::time::Duration::from_millis(1), async {}));
    });
    println!("tokio::time::timeout -> {timeout:?}");
    assert!(timeout.unwrap().contains("no reactor running"));

    let local = panic_msg(|| {
        let _ = tokio::task::spawn_local(async {});
    });
    println!("tokio::task::spawn_local -> {local:?}");
    assert!(local.is_some());

    let interval = panic_msg(|| {
        let _ = tokio::time::interval(std::time::Duration::from_millis(1));
    });
    println!("tokio::time::interval -> {interval:?}");
    assert!(interval.is_some());
}

#[test]
fn pure_future_apis_work_without_tokio() {
    // join!/select!/oneshot/mpsc/Mutex/Notify/Semaphore/watch need no runtime.
    let (tx, rx) = tokio::sync::oneshot::channel::<u32>();
    tx.send(5).unwrap();
    let (a, b) = match poll_once(async { tokio::join!(async { 1 }, async { rx.await.unwrap() }) }) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("pending"),
    };
    assert_eq!((a, b), (1, 5));

    let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(2);
    tx.try_send(9).unwrap();
    assert_eq!(poll_once(async { rx.recv().await }), Poll::Ready(Some(9)));

    let m = tokio::sync::Mutex::new(1);
    assert!(matches!(poll_once(async { *m.lock().await }), Poll::Ready(1)));

    let sem = tokio::sync::Semaphore::new(1);
    assert!(matches!(poll_once(async { sem.acquire().await.is_ok() }), Poll::Ready(true)));

    let s = poll_once(async {
        tokio::select! { v = async { 1 } => v, v = std::future::pending::<i32>() => v }
    });
    assert_eq!(s, Poll::Ready(1));
}

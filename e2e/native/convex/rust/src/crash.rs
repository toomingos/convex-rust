//! One action per failure mode of DESIGN section 8.2 (suite 2) plus the limits cases.
//! Everything here is meant to hurt the worker process. Do not run it outside the e2e suite.
use convex_fn::{action, json, Ctx, Result, Value};
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Deserialize)]
pub struct NoArgs {}

static CALLS: AtomicU64 = AtomicU64::new(0);

/// Healthy call: reports which process served it and how many calls that process has served.
/// A crash must be followed by a call that reports a different pid (a fresh worker).
#[action]
pub async fn probe(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let n = CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    Ok(json!({ "pid": std::process::id(), "calls": n }))
}

#[action]
pub async fn panic_plain(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    panic!("native-e2e-panic");
}

/// Stand-in for a `panic = "abort"` profile: the CLI forces `panic = "unwind"` on every native
/// build, so the variant is emulated with a hook that aborts. The process dies by SIGABRT.
#[action]
pub async fn panic_abort_hook(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    std::panic::set_hook(Box::new(|_| std::process::abort()));
    panic!("native-e2e-panic-abort");
}

#[action]
pub async fn abort(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    std::process::abort();
}

#[action]
pub async fn segfault(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    unsafe {
        let p: *mut u8 = std::hint::black_box(std::ptr::null_mut());
        p.write_volatile(1);
    }
    Ok(())
}

#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(n: u64, pad: &mut [u8; 512]) -> u64 {
    pad[(n % 512) as usize] = n as u8;
    let r = recurse(std::hint::black_box(n + 1), pad);
    r + pad[0] as u64
}
#[action]
pub async fn stack_overflow(_ctx: &Ctx, _a: NoArgs) -> Result<u64> {
    let mut pad = [0u8; 512];
    Ok(recurse(0, &mut pad))
}

#[derive(Deserialize)]
pub struct OomArgs {
    bytes: usize,
}
/// Allocates and touches `bytes` (default test value: 2 GiB, above the 512 MiB limit).
#[action]
pub async fn oom(_ctx: &Ctx, a: OomArgs) -> Result<u64> {
    let mut v: Vec<u8> = Vec::new();
    v.try_reserve_exact(0).ok();
    let mut v2 = vec![1u8; a.bytes];
    v2[a.bytes - 1] = 2;
    v.push(v2[a.bytes / 2]);
    Ok(std::hint::black_box(v2.iter().step_by(4096).map(|x| *x as u64).sum()))
}

#[action]
pub async fn infinite_loop(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    let mut x = 0u64;
    loop {
        x = std::hint::black_box(x.wrapping_add(1));
    }
}

#[action]
pub async fn exit3(_ctx: &Ctx, _a: NoArgs) -> Result<()> {
    std::process::exit(3);
}

/// Returns normally while a detached thread keeps spinning: the worker must be recycled, not
/// reused with a runaway thread inside.
#[action]
pub async fn spin_thread(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    std::thread::spawn(|| loop {
        std::hint::black_box(0u64);
    });
    Ok(json!({ "pid": std::process::id() }))
}

#[derive(Deserialize)]
pub struct FloodArgs {
    mib: usize,
}
/// Prints `mib` MiB to stdout in 1 KiB lines (the host caps captured stdio at 16 MiB per call).
#[action]
pub async fn println_flood(_ctx: &Ctx, a: FloodArgs) -> Result<Value> {
    let line = "x".repeat(1023);
    for _ in 0..(a.mib * 1024) {
        println!("{line}");
    }
    Ok(json!({ "printed_mib": a.mib }))
}

#[derive(Deserialize)]
pub struct LogArgs {
    n: usize,
}
/// `n` ctx log lines (the backend caps log lines per execution).
#[action]
pub async fn log_lines(_ctx: &Ctx, a: LogArgs) -> Result<Value> {
    for i in 0..a.n {
        convex_fn::log::info(&format!("native-e2e-log {i}"));
    }
    Ok(json!({ "logged": a.n }))
}

#[derive(Deserialize)]
pub struct BigArgs {
    bytes: usize,
}
/// Returns a string of `bytes` characters (limit: 16 MiB result).
#[action]
pub async fn big_result(_ctx: &Ctx, a: BigArgs) -> Result<String> {
    Ok("a".repeat(a.bytes))
}

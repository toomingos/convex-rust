//! Native ABI v1 (contracts/NATIVE.md §2.4): the exports a `--cfg convex_native`
//! cdylib offers to the worker, and the host vtable it calls back through.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::RefCell;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use serde::{de::DeserializeOwned, Serialize};

use crate::Ctx;

/// Native ABI version returned by `convex_native_init`.
pub const NATIVE_ABI_VERSION: u32 = 1;

/// Host vtable (contracts/NATIVE.md §2.4). Prefix-compatible growth via `size`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostVtableV1 {
    pub size: usize,
    pub version: u32,
    pub task_submit: extern "C" fn(ptr: *const u8, len: usize) -> i64,
    pub task_poll: extern "C" fn(block: i32, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32,
    pub sync_syscall: extern "C" fn(
        op: *const u8,
        op_len: usize,
        args: *const u8,
        args_len: usize,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> i32,
    pub log: extern "C" fn(level: i32, ptr: *const u8, len: usize),
    pub oom: extern "C" fn(limit: usize, requested: usize),
}

pub(crate) type Vt = HostVtableV1;

/// Result handle: `convex_run:*` returns a pointer to a boxed `ConvexBuf`.
#[repr(C)]
pub struct ConvexBuf {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

static VTABLE: AtomicPtr<HostVtableV1> = AtomicPtr::new(std::ptr::null_mut());

pub(crate) fn vtable() -> Option<&'static Vt> {
    // SAFETY: only ever set to a leaked, never-freed copy of the host's vtable.
    unsafe { VTABLE.load(Ordering::Acquire).as_ref() }
}

/// Stores the host vtable (copied, so the host may free its own) and returns
/// the native ABI version, or 0 when the vtable is unusable.
///
/// # Safety
/// `vt` must be null or point to a valid `HostVtableV1` of `(*vt).size` bytes.
#[no_mangle]
pub unsafe extern "C" fn convex_native_init(vt: *const HostVtableV1) -> u32 {
    if vt.is_null() {
        return 0;
    }
    let vt = &*vt;
    if vt.size < std::mem::size_of::<HostVtableV1>() || vt.version != NATIVE_ABI_VERSION {
        return 0;
    }
    let copy = Box::into_raw(Box::new(*vt));
    VTABLE.store(copy, Ordering::Release);
    NATIVE_ABI_VERSION
}

/// Sets the allocator limit in bytes (default: unlimited). Not in the frozen
/// vtable, so the worker calls this export after `convex_native_init`.
#[no_mangle]
pub extern "C" fn convex_native_set_memory_limit(limit: usize) {
    LIMIT.store(limit, Ordering::Relaxed);
}

/// Optional idle hook (contracts/NATIVE.md §2.4), called by the worker on the
/// call thread after a call's reply was sent. `check_only != 0` returns 1 when
/// the last call's peak live bytes reached 8 MiB (a trim is worthwhile), else
/// 0, without touching the allocator. `check_only == 0` runs `mi_collect(true)`
/// so freed pages go back to the OS and returns 1. The worker checks after
/// every reply but collects only after ~100 ms without a new call, so
/// back-to-back calls neither wait for the trim nor re-fault purged pages.
/// mimalloc only purges while the allocator is active, so without this an
/// idle worker keeps its high-water memory. `mi_collect` acts on the calling
/// thread's heap (plus abandoned segments), which is where guest allocations
/// happen: the guest runs on the call thread.
#[no_mangle]
pub extern "C" fn convex_native_trim(check_only: i32) -> i32 {
    if PEAK.load(Ordering::Relaxed) < TRIM_PEAK_BYTES {
        return 0;
    }
    if check_only != 0 {
        return 1;
    }
    flush_credit();
    // SAFETY: mi_collect has no preconditions.
    unsafe { libmimalloc_sys::mi_collect(true) };
    1
}

/// Same as the wasm export, with `usize` lengths.
#[no_mangle]
pub extern "C" fn convex_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len.max(1));
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Frees a handle returned by a `convex_run:*` export.
#[no_mangle]
pub extern "C" fn convex_free_result(handle: i64) {
    if handle == 0 {
        return;
    }
    // SAFETY: the handle came from `into_handle` and is freed once.
    unsafe {
        let b = Box::from_raw(handle as usize as *mut ConvexBuf);
        drop(Vec::from_raw_parts(b.ptr, b.len, b.cap));
    }
}

fn into_handle(bytes: String) -> i64 {
    let mut v = std::mem::ManuallyDrop::new(bytes.into_bytes());
    let buf = Box::new(ConvexBuf {
        ptr: v.as_mut_ptr(),
        len: v.len(),
        cap: v.capacity(),
    });
    Box::into_raw(buf) as usize as i64
}

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Called by the panic hook with `<msg> at <file:line:col>`.
pub(crate) fn record_panic(msg: String) {
    LAST_PANIC.with(|p| *p.borrow_mut() = Some(msg));
}

/// Body of every native `convex_run:*` export. Never unwinds.
pub fn run_export_native<A, R, F, Fut>(args_ptr: *mut u8, args_len: usize, f: F) -> i64
where
    A: DeserializeOwned,
    R: Serialize,
    F: FnOnce(Ctx, A) -> Fut,
    Fut: Future<Output = crate::Result<R>>,
{
    crate::error::install_panic_hook();
    crate::executor::reset();
    flush_credit();
    LAST_PANIC.with(|p| p.borrow_mut().take());
    let args = crate::sys::take_buf(args_ptr, args_len);
    let envelope = match catch_unwind(AssertUnwindSafe(|| {
        crate::__rt::run_to_envelope(&args, f)
    })) {
        Ok(env) => env,
        Err(payload) => {
            let message = LAST_PANIC
                .with(|p| p.borrow_mut().take())
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                })
                .unwrap_or_else(|| "Box<dyn Any>".to_string());
            serde_json::json!({ "error": { "message": message, "kind": "panic" } }).to_string()
        }
    };
    into_handle(envelope)
}

// ---- counting allocator ---------------------------------------------------

use std::cell::Cell;

use mimalloc::MiMalloc;

/// Peak of `LIVE` since the last `flush_credit` (updated on the slow path
/// only, so it is a lower bound within one batch per thread).
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// A call whose peak reached this many bytes makes `convex_native_trim`
/// collect the allocator (smaller calls leave the heap alone).
const TRIM_PEAK_BYTES: usize = 8 << 20;

/// Bytes a thread reserves from the shared counter at once.
const BATCH: usize = 256 * 1024;
/// A thread hands credit back once it holds more than this.
const MAX_CREDIT: usize = 2 * BATCH;

/// Bytes reserved against the limit (live bytes plus every thread's credit).
static LIVE: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

thread_local! {
    // Reserved in `LIVE` but not in use by this thread. Const-init and no
    // destructor, so it is safe to touch from inside the global allocator.
    static CREDIT: Cell<usize> = const { Cell::new(0) };
}

/// Returns the calling thread's unused credit to the shared counter (called
/// at the start of each run so a long-lived worker does not drift).
pub(crate) fn flush_credit() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let _ = CREDIT.try_with(|c| {
        let n = c.replace(0);
        if n > 0 {
            LIVE.fetch_sub(n, Ordering::Relaxed);
        }
    });
}

/// mimalloc with a per-call memory limit. Failing allocations are reported
/// through `vt.oom`.
///
/// Accounting is batched per thread: a thread reserves `BATCH` (256 KiB) at a
/// time from `LIVE` and serves small allocations from that credit without
/// touching a shared atomic; frees refill the credit and anything above
/// `MAX_CREDIT` (512 KiB) is returned. Allocations of `BATCH` or more go to
/// `LIVE` exactly. The limit check is therefore exact for large allocations
/// and conservative otherwise: an allocation can fail up to `MAX_CREDIT` per
/// thread (<= 512 KiB, ~1 MiB with the two threads a guest normally has)
/// before the true live size reaches the limit, but never succeeds beyond it.
/// A thread that exits holding credit leaks at most `MAX_CREDIT` until the
/// worker process is recycled.
///
/// mimalloc keeps its default purge delay: `purge_delay = 0` was measured
/// (macOS, L3-01) to double the time of allocation-heavy calls without
/// lowering worker RSS (freed pages stay resident under MADV_FREE).
pub struct CountingAlloc;

impl CountingAlloc {
    /// Adds `n` to `LIVE` if that stays within the limit.
    fn take_global(n: usize) -> bool {
        let limit = LIMIT.load(Ordering::Relaxed);
        let live = LIVE.fetch_add(n, Ordering::Relaxed);
        if live.saturating_add(n) > limit {
            LIVE.fetch_sub(n, Ordering::Relaxed);
            return false;
        }
        PEAK.fetch_max(live.saturating_add(n), Ordering::Relaxed);
        true
    }

    #[cold]
    fn reserve_slow(extra: usize) -> bool {
        // Try a whole batch first (small requests only), then the exact size.
        if extra < BATCH && Self::take_global(BATCH) {
            let _ = CREDIT.try_with(|c| c.set(c.get() + BATCH - extra));
            return true;
        }
        if Self::take_global(extra) {
            return true;
        }
        if let Some(vt) = vtable() {
            (vt.oom)(LIMIT.load(Ordering::Relaxed), extra);
        }
        false
    }

    /// Reserves `extra` bytes against the limit; false (after reporting) if over.
    #[inline]
    fn reserve(extra: usize) -> bool {
        if extra < BATCH {
            let hit = CREDIT
                .try_with(|c| {
                    let have = c.get();
                    if have >= extra {
                        c.set(have - extra);
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if hit {
                return true;
            }
        }
        Self::reserve_slow(extra)
    }

    /// Gives `n` bytes back.
    #[inline]
    fn release(n: usize) {
        let done = CREDIT
            .try_with(|c| {
                let have = c.get() + n;
                if have > MAX_CREDIT {
                    c.set(BATCH);
                    LIVE.fetch_sub(have - BATCH, Ordering::Relaxed);
                } else {
                    c.set(have);
                }
            })
            .is_ok();
        if !done {
            LIVE.fetch_sub(n, Ordering::Relaxed);
        }
    }
}

// SAFETY: delegates to mimalloc; counters are atomics/thread-locals and never allocate.
unsafe impl GlobalAlloc for CountingAlloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = MiMalloc.alloc(layout);
        if p.is_null() {
            Self::release(layout.size());
        }
        p
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !Self::reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = MiMalloc.alloc_zeroed(layout);
        if p.is_null() {
            Self::release(layout.size());
        }
        p
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        MiMalloc.dealloc(ptr, layout);
        Self::release(layout.size());
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size();
        if new_size > old && !Self::reserve(new_size - old) {
            return std::ptr::null_mut();
        }
        let p = MiMalloc.realloc(ptr, layout, new_size);
        if p.is_null() {
            if new_size > old {
                Self::release(new_size - old);
            }
        } else if new_size < old {
            Self::release(old - new_size);
        }
        p
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

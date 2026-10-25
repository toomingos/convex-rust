//! Write Convex actions in Rust (`"use rust"`).
//!
//! ```ignore
//! use convex_fn::{action, Ctx, Result};
//!
//! #[action]
//! async fn hello(ctx: &Ctx, args: serde_json::Value) -> Result<String> {
//!     let msgs = ctx.run_query("messages:list", serde_json::json!({})).await?;
//!     Ok(format!("{} messages", msgs.as_array().map_or(0, Vec::len)))
//! }
//! ```
//!
//! # Logging, clock, random, env
//!
//! - `println!` goes to WASI stdout and `eprintln!` to stderr; the host turns
//!   each line into a function log line (stdout at log/info level, stderr at
//!   error level). No setup needed.
//! - Feature `log`: a `log::Log` implementation is installed by the `#[action]`
//!   entry unless a logger is already set (`error`->3, `warn`->2, `info`->1,
//!   `debug`/`trace`->0). Messages are written as-is (no target prefix).
//! - Feature `tracing`: a minimal subscriber is installed the same way; events
//!   are written as `span{fields}: message key=value`, at the mapped level.
//! - `std::env::var`, `SystemTime::now`, `Instant` and `getrandom` (0.2 and 0.3,
//!   hence `rand`) work through WASI.
//! - With `tokio-rt`, blocking calls (`block_on`, `reqwest::blocking`) made
//!   inside an action drive the host task directly; tokio tasks spawned on the
//!   `LocalSet` do not progress until the blocking call returns.
//!
//! Host interface: `contracts/ABI.md`. Function metadata: `contracts/MANIFEST.md`.

mod args;
pub mod crypto;
mod ctx;
pub mod dns;
mod error;
mod executor;
mod fanout;
pub mod fs;
pub mod intl;
mod function_ref;
mod http;
mod logging;
#[cfg(convex_native)]
pub mod native;
mod net;
mod sys;
pub mod values;
#[doc(hidden)]
pub mod wire;

pub use args::{ConvexType, Field, Validator};
pub use convex_fn_macros::{action, http_action, ConvexArgs, ConvexValue};
pub use ctx::{
    ChunkSource, Chunks, Ctx, FetchRequest, FetchResponse, FetchStream, Redirect, Scheduler,
    Storage, StorageFile, StorageMetadata, VectorFilter, VectorHit, VectorSearchQuery,
};
pub use error::{ConvexError, Error, Result};
pub use executor::{block_on, join_all, race, select, Either};
pub use fanout::par_map;
pub use function_ref::{
    ComponentFunctionRef, FunctionAddress, FunctionHandle, FunctionRef, FunctionTarget,
};
pub use http::{BodyWriter, HttpRequest, HttpResponse};
pub use serde;
pub use serde_json::{json, Value};
pub use values::{Bytes, Float64, Int64};

/// Wire format shared with `convex_fn_test`; not public API.
#[doc(hidden)]
pub use wire as __wire;

/// Write one line to the function log at the given level.
pub mod log {
    use crate::wire::LogLevel;

    pub fn debug(msg: &str) {
        crate::sys::log_line(LogLevel::Debug, msg)
    }
    pub fn info(msg: &str) {
        crate::sys::log_line(LogLevel::Info, msg)
    }
    pub fn warn(msg: &str) {
        crate::sys::log_line(LogLevel::Warn, msg)
    }
    pub fn error(msg: &str) {
        crate::sys::log_line(LogLevel::Error, msg)
    }
}

/// Hooks for running actions natively (used by `convex_fn_test`).
#[cfg(all(not(target_arch = "wasm32"), not(convex_native)))]
pub mod testing {
    /// A host implementing contracts/ABI.md in-process. Byte payloads are the
    /// same JSON the wasm host exchanges.
    pub trait Host {
        fn task_submit(&mut self, request: &[u8]) -> i64;
        /// `None` means "nothing ready" (`block = false`) or "no outstanding
        /// tasks" (`block = true`).
        fn task_poll(&mut self, block: bool) -> Option<Vec<u8>>;
        fn sync_syscall(&mut self, op: &str, args: &[u8]) -> Vec<u8>;
        fn log(&mut self, level: i32, msg: &str);
    }

    /// Install `host` for the current thread, returning the previous one.
    pub fn install_host(host: Box<dyn Host>) -> Option<Box<dyn Host>> {
        crate::executor::reset();
        crate::sys::install(host)
    }

    /// Remove and return the current thread's host.
    pub fn uninstall_host() -> Option<Box<dyn Host>> {
        crate::sys::uninstall()
    }
}

/// Optional wasm export: frees the result buffer of a `convex_run:*` call once the host has read it
/// (contracts/V8HOST.md). `ptr`/`len` are the two halves of the packed value the call returned; that
/// buffer is an exactly sized boxed slice (see `run_export`). Hosts that do not know it just leak the buffer.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn convex_free(ptr: *mut u8, len: usize) {
    if len == 0 || ptr.is_null() {
        return;
    }
    // SAFETY: `run_export` returned `Box<[u8]>::into_raw` parts (ptr, len); the host frees them once.
    unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len))) };
}

#[doc(hidden)]
pub mod __rt {
    use std::future::Future;

    use serde::{de::DeserializeOwned, Serialize};

    use crate::{error::Error, Ctx};

    /// A fresh `Ctx` for native test hosts (`convex_fn_test`).
    #[cfg(all(not(target_arch = "wasm32"), not(convex_native)))]
    pub fn test_ctx() -> Ctx {
        Ctx::new()
    }

    pub use crate::http::adapter as http_adapter;

    pub use crate::args::{manifest_bytes, manifest_len, manifest_string, NoValidator, Probe};

    /// Stand-in return type with no validator.
    pub struct NoReturnType;

    pub const fn const_bytes<const N: usize>(s: &str) -> [u8; N] {
        let b = s.as_bytes();
        let mut out = [0u8; N];
        let mut i = 0;
        while i < N {
            out[i] = b[i];
            i += 1;
        }
        out
    }

    /// Installs the `log` / `tracing` bridges (features `log`, `tracing`) unless
    /// the user already installed their own. No-op without those features.
    pub fn install_logging() {
        crate::logging::install();
    }

    /// Body of every `convex_run:*` export.
    #[cfg(target_arch = "wasm32")]
    pub fn run_export<A, R, F, Fut>(args_ptr: *mut u8, args_len: usize, f: F) -> i64
    where
        A: DeserializeOwned,
        R: Serialize,
        F: FnOnce(Ctx, A) -> Fut,
        Fut: Future<Output = crate::Result<R>>,
    {
        crate::error::install_panic_hook();
        // A reused instance must not see the previous call's tasks/streams (the host restarts ids at 0).
        crate::executor::reset_state();
        // SAFETY: the host wrote `args_len` bytes into a buffer from `convex_alloc`.
        let args = unsafe { Vec::from_raw_parts(args_ptr, args_len, args_len.max(1)) };
        let envelope = run_to_envelope(&args, f);
        let out = std::mem::ManuallyDrop::new(envelope.into_bytes().into_boxed_slice());
        ((out.as_ptr() as u64) << 32 | out.len() as u64) as i64
    }

    /// Body of every `convex_run:*` export in native (`--cfg convex_native`) builds.
    #[cfg(convex_native)]
    pub fn run_export<A, R, F, Fut>(args_ptr: *mut u8, args_len: usize, f: F) -> i64
    where
        A: DeserializeOwned,
        R: Serialize,
        F: FnOnce(Ctx, A) -> Fut,
        Fut: Future<Output = crate::Result<R>>,
    {
        crate::native::run_export_native(args_ptr, args_len, f)
    }

    /// Plain native builds (`cargo test`) never call the exports.
    #[cfg(all(not(target_arch = "wasm32"), not(convex_native)))]
    pub fn run_export<A, R, F, Fut>(_args_ptr: *mut u8, _args_len: usize, _f: F) -> i64
    where
        A: DeserializeOwned,
        R: Serialize,
        F: FnOnce(Ctx, A) -> Fut,
        Fut: Future<Output = crate::Result<R>>,
    {
        unreachable!("convex_run exports are only callable on wasm32 or with --cfg convex_native")
    }

    /// Body of every `#[http_action]` `convex_run:*` export: the call's args are the request head.
    pub fn run_http_export<F, Fut>(args_ptr: *mut u8, args_len: usize, f: F) -> i64
    where
        F: FnOnce(Ctx, crate::HttpRequest) -> Fut + 'static,
        Fut: Future<Output = crate::Result<crate::HttpResponse>> + 'static,
    {
        run_export(args_ptr, args_len, crate::http::adapter(f))
    }

    /// Runs an action on raw Convex-JSON args and returns the result envelope
    /// (contracts/ABI.md "Result envelope").
    pub fn run_to_envelope<A, R, F, Fut>(args: &[u8], f: F) -> String
    where
        A: DeserializeOwned,
        R: Serialize,
        F: FnOnce(Ctx, A) -> Fut,
        Fut: Future<Output = crate::Result<R>>,
    {
        install_logging();
        let result = serde_json::from_slice::<A>(args)
            .map_err(|e| Error::msg(format!("invalid arguments: {e}")))
            .and_then(|a| crate::executor::block_on(f(Ctx::new(), a)))
            .and_then(|r| {
                serde_json::to_value(r)
                    .map_err(|e| Error::msg(format!("invalid return value: {e}")))
            });
        let envelope = match result {
            Ok(v) => serde_json::json!({ "ok": v }),
            Err(e) => serde_json::json!({ "error": e.to_json() }),
        };
        envelope.to_string()
    }
}

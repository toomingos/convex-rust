//! Optional `net/*` sync ops (contracts/ABI.md "Optional sync ops: `net/*`"):
//! task / stream cancellation, `{ptr,len}` request bodies and guest-fed
//! (streamed) request bodies.
//!
//! The ops ride on `sync_syscall` (no new wasm import). The guest probes
//! `net/caps` once per instance; a host without them keeps the older
//! behaviour (inline base64 bodies, no cancellation, a streamed upload is
//! buffered), so a module still works on an older host.

use std::cell::Cell;

use serde_json::{json, Value};

use crate::{error::Error, Result};

/// Request bodies at least this large go as `{ptr,len}` references.
pub(crate) const REF_THRESHOLD: usize = 8 * 1024;
/// Size of the slices a body bigger than the host's single-buffer cap is cut into.
pub(crate) const STREAM_SLICE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Caps {
    pub cancel: bool,
    pub body_ref: bool,
    pub request_stream: bool,
    /// Largest `{ptr,len}` body one task may carry.
    pub max_body_ref: usize,
}

thread_local! {
    static CAPS: Cell<Option<Caps>> = const { Cell::new(None) };
}

pub(crate) fn reset() {
    CAPS.with(|c| c.set(None));
}

/// The host's `net/*` capabilities, probed on first use (per instance).
pub(crate) fn caps() -> Caps {
    if let Some(c) = CAPS.with(Cell::get) {
        return c;
    }
    let c = probe();
    CAPS.with(|cell| cell.set(Some(c)));
    c
}

/// The capabilities if they were already probed; never calls the host.
pub(crate) fn known() -> Caps {
    CAPS.with(Cell::get).unwrap_or_default()
}

fn probe() -> Caps {
    let Ok(v) = call("net/caps", json!({ "version": 1 })) else {
        return Caps::default();
    };
    let has = |op: &str| {
        v.get("ops")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(op)))
    };
    Caps {
        cancel: has("cancel"),
        body_ref: has("bodyRef"),
        request_stream: has("requestStream"),
        max_body_ref: v
            .get("maxBodyRef")
            .and_then(Value::as_u64)
            .map_or(64 << 20, |n| n as usize),
    }
}

fn call(op: &str, args: Value) -> Result<Value> {
    let out = crate::sys::sync(op, args.to_string().as_bytes());
    let v: Value = serde_json::from_slice(&out)?;
    match v.get("error") {
        Some(e) => Err(Error::from_json(e)),
        None => Ok(v.get("ok").cloned().unwrap_or(Value::Null)),
    }
}

/// A byte slice as a guest-memory reference.
pub(crate) fn bytes_ref(b: &[u8]) -> Value {
    json!({ "ptr": b.as_ptr() as usize, "len": b.len() })
}

/// Aborts the host work of a task (its fetch, and a streamed request body).
/// Best effort; only called for tasks submitted under `cancel` support.
pub(crate) fn cancel_task(id: i64) {
    let _ = call("net/cancel", json!({ "taskId": id }));
}

/// Aborts a body stream the guest stopped reading (the rest of the download).
pub(crate) fn cancel_stream(stream_id: &str) {
    if known().cancel {
        let _ = call("net/cancel", json!({ "streamId": stream_id }));
    }
}

/// Appends a chunk to the streamed request body of task `id`. The host copies
/// the bytes and, when its buffer is full, suspends the guest until the
/// network has drained it (backpressure).
pub(crate) fn body_write(id: i64, data: &[u8]) -> Result<()> {
    call(
        "net/body_write",
        json!({ "taskId": id, "data": bytes_ref(data) }),
    )
    .map(|_| ())
}

/// Ends the streamed request body of task `id`.
pub(crate) fn body_end(id: i64) -> Result<()> {
    call("net/body_end", json!({ "taskId": id })).map(|_| ())
}

/// Fails the streamed request body of task `id`, which fails the request.
pub(crate) fn body_abort(id: i64, message: &str) {
    let _ = call(
        "net/body_abort",
        json!({ "taskId": id, "message": message }),
    );
}

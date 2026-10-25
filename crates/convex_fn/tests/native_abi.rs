//! Drives the native exports with a fake host vtable, in-process.
#![cfg(convex_native)]

use std::collections::VecDeque;
use std::sync::Mutex;

use convex_fn::native::{
    convex_alloc, convex_free_result, convex_native_init, convex_native_set_memory_limit,
    run_export_native, ConvexBuf, HostVtableV1,
};
use convex_fn::{Ctx, Result};
use serde_json::{json, Value};

#[derive(Default)]
struct Fake {
    submits: Vec<Vec<u8>>,
    events: VecDeque<Vec<u8>>,
    syncs: Vec<(String, Vec<u8>)>,
    logs: Vec<(i32, String)>,
    ooms: Vec<(usize, usize)>,
}

static FAKE: Mutex<Option<Fake>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Fake) -> R) -> R {
    f(FAKE.lock().unwrap().get_or_insert_with(Fake::default))
}

fn give(bytes: &[u8], out_ptr: *mut *mut u8, out_len: *mut usize) {
    let p = convex_alloc(bytes.len());
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        *out_ptr = p;
        *out_len = bytes.len();
    }
}

extern "C" fn task_submit(ptr: *const u8, len: usize) -> i64 {
    let req = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    with(|f| {
        f.submits.push(req);
        // Answer each submit with a done event for its (implicit) id.
        let id = f.submits.len() as i64 - 1;
        f.events.push_back(
            json!({"type": "done", "taskId": id, "ok": {"echo": id}})
                .to_string()
                .into_bytes(),
        );
        id
    })
}

extern "C" fn task_poll(block: i32, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32 {
    match with(|f| f.events.pop_front()) {
        Some(ev) => {
            give(&ev, out_ptr, out_len);
            1
        }
        None if block != 0 => -1,
        None => 0,
    }
}

extern "C" fn sync_syscall(
    op: *const u8,
    op_len: usize,
    args: *const u8,
    args_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    let op = String::from_utf8(unsafe { std::slice::from_raw_parts(op, op_len) }.to_vec()).unwrap();
    let args = unsafe { std::slice::from_raw_parts(args, args_len) }.to_vec();
    with(|f| f.syncs.push((op, args)));
    give(br#"{"ok":1234.5}"#, out_ptr, out_len);
    1
}

extern "C" fn log(level: i32, ptr: *const u8, len: usize) {
    let s = String::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()).unwrap();
    with(|f| f.logs.push((level, s)));
}

extern "C" fn oom(limit: usize, requested: usize) {
    with(|f| f.ooms.push((limit, requested)));
}

/// Calls an export body the way a `convex_run:*` symbol does and returns the envelope.
fn call<F, Fut>(args: &str, f: F) -> Value
where
    F: FnOnce(Ctx, Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let p = convex_alloc(args.len());
    unsafe { std::ptr::copy_nonoverlapping(args.as_ptr(), p, args.len()) };
    let handle = run_export_native(p, args.len(), f);
    assert_ne!(handle, 0);
    let buf = unsafe { &*(handle as usize as *const ConvexBuf) };
    let bytes = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
    let v: Value = serde_json::from_slice(bytes).unwrap();
    convex_free_result(handle);
    v
}

#[test]
fn native_abi() {
    let vt = HostVtableV1 {
        size: std::mem::size_of::<HostVtableV1>(),
        version: 1,
        task_submit,
        task_poll,
        sync_syscall,
        log,
        oom,
    };
    assert_eq!(unsafe { convex_native_init(&vt) }, 1);
    let bad = HostVtableV1 { version: 2, ..vt };
    assert_eq!(unsafe { convex_native_init(&bad) }, 0);
    assert_eq!(unsafe { convex_native_init(std::ptr::null()) }, 0);

    // Task submit + blocking poll + sync + log, through the real executor.
    let env = call(r#"{"x":1}"#, |ctx, args| async move {
        assert_eq!(args["x"], 1);
        let r: Value = ctx.run_query("m:f", json!({"a": 1})).await?;
        let now = ctx.now_ms();
        convex_fn::log::info("hello");
        Ok(json!({"r": r, "now": now}))
    });
    assert_eq!(
        env,
        json!({"ok": {"r": {"echo": 0}, "now": 1234.5}}),
        "envelope"
    );
    with(|f| {
        assert_eq!(f.submits.len(), 1);
        let req: Value = serde_json::from_slice(&f.submits[0]).unwrap();
        assert_eq!(req["type"], "syscall");
        assert_eq!(f.syncs.len(), 1);
        assert_eq!(f.syncs[0].0, "now");
        assert!(f.logs.contains(&(1, "hello".to_string())));
    });

    // Errors keep the ABI v1 envelope.
    let env = call("{}", |_, _| async { Err(convex_fn::Error::msg("nope")) });
    assert_eq!(env, json!({"error": {"message": "nope", "kind": "user"}}));

    // Bad args.
    let env = call("not json", |_, _| async { Ok(Value::Null) });
    assert!(env["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("invalid arguments"));

    // Panic: caught, envelope has `<msg> at <file:line:col>`, hook logs the wasm-format line.
    let env = call("{}", |_, _| async {
        if true {
            panic!("kaboom");
        }
        Ok(Value::Null)
    });
    let msg = env["error"]["message"].as_str().unwrap();
    assert!(
        msg.starts_with("kaboom at ") && msg.contains("native_abi.rs:"),
        "{msg}"
    );
    assert_eq!(env["error"]["kind"], "panic");
    with(|f| {
        assert!(f
            .logs
            .iter()
            .any(|(l, s)| *l == 3 && s.starts_with("Uncaught panic: kaboom at ")));
    });

    // The worker is reusable after a panic.
    let env = call("{}", |_, _| async { Ok(json!(7)) });
    assert_eq!(env, json!({"ok": 7}));

    // Allocator: over the limit returns null (try_reserve fails) and reports via vt.oom.
    let limit = 1usize << 40;
    convex_native_set_memory_limit(limit);
    let mut v: Vec<u8> = Vec::new();
    assert!(v.try_reserve_exact(1usize << 41).is_err());
    convex_native_set_memory_limit(usize::MAX);
    with(|f| assert_eq!(f.ooms, vec![(limit, 1usize << 41)]));
}

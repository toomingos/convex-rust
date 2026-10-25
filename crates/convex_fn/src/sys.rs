//! The four host imports of contracts/ABI.md, plus a native stand-in so the
//! same code runs under `cargo test` against `convex_fn_test`.

use crate::wire::LogLevel;

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::LogLevel;

    #[link(wasm_import_module = "convex")]
    extern "C" {
        fn task_submit(ptr: *const u8, len: usize) -> i64;
        fn task_poll(block: i32) -> i64;
        fn sync_syscall(
            op_ptr: *const u8,
            op_len: usize,
            args_ptr: *const u8,
            args_len: usize,
        ) -> i64;
        fn log(level: i32, ptr: *const u8, len: usize);
    }

    /// Host-side allocation entry point (contracts/ABI.md "Memory protocol").
    #[no_mangle]
    pub extern "C" fn convex_alloc(len: usize) -> *mut u8 {
        let mut buf = Vec::<u8>::with_capacity(len.max(1));
        let ptr = buf.as_mut_ptr();
        std::mem::forget(buf);
        ptr
    }

    /// Takes ownership of a packed `(ptr << 32) | len` buffer written by the host.
    pub(crate) fn take_packed(packed: i64) -> Vec<u8> {
        let ptr = (packed as u64 >> 32) as usize as *mut u8;
        let len = (packed as u64 & 0xffff_ffff) as usize;
        // SAFETY: the host allocated `len` bytes via `convex_alloc(len)`, which
        // created a Vec with capacity `len.max(1)`, and handed it to us once.
        unsafe { Vec::from_raw_parts(ptr, len, len.max(1)) }
    }

    pub fn submit(req: &[u8]) -> i64 {
        unsafe { task_submit(req.as_ptr(), req.len()) }
    }

    pub fn poll(block: bool) -> Option<Vec<u8>> {
        let packed = unsafe { task_poll(block as i32) };
        // 0 = nothing ready, -1 = no tasks outstanding. A pointer >= 2 GiB makes `packed` negative, so
        // only these two values mean "no event".
        (packed != 0 && packed != -1).then(|| take_packed(packed))
    }

    pub fn sync(op: &str, args: &[u8]) -> Vec<u8> {
        let packed = unsafe { sync_syscall(op.as_ptr(), op.len(), args.as_ptr(), args.len()) };
        take_packed(packed)
    }

    pub fn log_line(level: LogLevel, msg: &str) {
        unsafe { log(level.as_i32(), msg.as_ptr(), msg.len()) }
    }

    /// Presence of this export tells the host that a real panic is confirmed by a level-4 `log`
    /// call right after the `Uncaught panic: ` line, so a user log line with that prefix is not
    /// mistaken for one. Hosts that do not know it keep using the prefix line alone.
    #[no_mangle]
    pub extern "C" fn convex_panic_marker() -> i32 {
        1
    }

    /// Confirms the panic line just logged (level 4, empty message).
    pub fn panic_marker() {
        unsafe { log(4, "".as_ptr(), 0) }
    }
}

#[cfg(all(not(target_arch = "wasm32"), convex_native))]
mod imp {
    use super::LogLevel;
    use crate::native::{vtable, Vt};

    /// Takes ownership of a buffer the host allocated with `convex_alloc(len)`.
    pub(crate) fn take_buf(ptr: *mut u8, len: usize) -> Vec<u8> {
        if ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: `convex_alloc(len)` created a Vec with capacity `len.max(1)`
        // and the host handed it to us exactly once.
        unsafe { Vec::from_raw_parts(ptr, len, len.max(1)) }
    }

    fn vt() -> &'static Vt {
        vtable().expect("convex_native_init has not been called")
    }

    pub fn submit(req: &[u8]) -> i64 {
        (vt().task_submit)(req.as_ptr(), req.len())
    }

    pub fn poll(block: bool) -> Option<Vec<u8>> {
        let mut ptr = std::ptr::null_mut();
        let mut len = 0usize;
        let r = (vt().task_poll)(block as i32, &mut ptr, &mut len);
        (r == 1).then(|| take_buf(ptr, len))
    }

    pub fn sync(op: &str, args: &[u8]) -> Vec<u8> {
        let mut ptr = std::ptr::null_mut();
        let mut len = 0usize;
        (vt().sync_syscall)(
            op.as_ptr(),
            op.len(),
            args.as_ptr(),
            args.len(),
            &mut ptr,
            &mut len,
        );
        take_buf(ptr, len)
    }

    pub fn log_line(level: LogLevel, msg: &str) {
        match vtable() {
            Some(vt) => (vt.log)(level.as_i32(), msg.as_ptr(), msg.len()),
            None => eprintln!("{msg}"),
        }
    }

    pub fn panic_marker() {}
}

#[cfg(all(not(target_arch = "wasm32"), not(convex_native)))]
mod imp {
    use std::cell::RefCell;

    use super::LogLevel;
    use crate::testing::Host;

    thread_local! {
        static HOST: RefCell<Option<Box<dyn Host>>> = const { RefCell::new(None) };
    }

    pub(crate) fn install(host: Box<dyn Host>) -> Option<Box<dyn Host>> {
        HOST.with(|h| h.borrow_mut().replace(host))
    }

    pub(crate) fn uninstall() -> Option<Box<dyn Host>> {
        HOST.with(|h| h.borrow_mut().take())
    }

    fn with_host<R>(f: impl FnOnce(&mut dyn Host) -> R) -> R {
        HOST.with(|h| {
            let mut h = h.borrow_mut();
            let host = h.as_deref_mut().expect(
                "no Convex host installed: use convex_fn_test::TestHost to run actions natively",
            );
            f(host)
        })
    }

    pub fn submit(req: &[u8]) -> i64 {
        with_host(|h| h.task_submit(req))
    }

    pub fn poll(block: bool) -> Option<Vec<u8>> {
        with_host(|h| h.task_poll(block))
    }

    pub fn sync(op: &str, args: &[u8]) -> Vec<u8> {
        with_host(|h| h.sync_syscall(op, args))
    }

    pub fn log_line(level: LogLevel, msg: &str) {
        HOST.with(|h| match h.borrow_mut().as_deref_mut() {
            Some(host) => host.log(level.as_i32(), msg),
            None => eprintln!("{msg}"),
        })
    }
}

pub(crate) use imp::*;

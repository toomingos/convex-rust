# Native ABI v1 and worker protocol (rust-native mode)

Status: **frozen at WN-00** (decision D-101). Source: the internal design notes, §2, §4.3–4.4, §5.1–5.2, §5.6, copied verbatim. Implementation references in it (`file:line`, [V]/[I] marks) are context, not contract.
JSON payloads are byte-identical to [ABI.md](ABI.md) (guest ABI v1). This document adds only the transport, the native symbol table, the artifact format, error strings and knobs.
Golden frame fixtures: `contracts/native/frames/*.hex` (WN-03 produces them from this table; any change here needs a DECISIONS entry).

## 2. IPC protocol (host ⇄ worker)

### 2.1 Transport

| fd (worker) | Type | Direction | Content |
|---|---|---|---|
| 0 | `/dev/null` | — | — |
| 1, 2 | pipe | W→H | user `print!`/`eprint!`, std's panic and stack-overflow messages |
| 3 | `socketpair(AF_UNIX, SOCK_STREAM)` | both | frames (below) |
| 4 | pipe | W→H | fatal status notes, written with raw `write(2)` from signal handlers or the allocator; no allocation |

Why not `SOCK_SEQPACKET`: macOS does not support it for `AF_UNIX` [I]. A stream with length prefixes is portable.

### 2.2 Framing

Every frame is `u32 len (LE, bytes after this field) | u8 kind | body`.

- `len ≤ RUST_NATIVE_MAX_FRAME_BYTES`, which is `max(max_task_request_bytes, max_result_bytes) + 64 KiB`. These are 64 MiB and 16 MiB + 64 KiB (`core.rs:91-114`) [V].
- A longer frame is a protocol error, and the host kills the worker. Exception: a `Result` longer than `max_result_bytes` becomes `CallFailure::ResultTooLarge`, the same as wasm (`core.rs:640-648`) [V]. The host checks the length **before** reading the body, then kills the worker.
- Integers are LE. `bytes` means the rest of the frame. `str16` means `u16 len + UTF-8`.

| kind | Dir | Name | Body | Notes |
|---|---|---|---|---|
| 0x01 | W→H | `Ready` | `u16 proto, u32 pid, u32 resolved_exports, u64 init_us, u64 rss_bytes` | once, after dlopen, `convex_native_init` and the seccomp install |
| 0x02 | H→W | `Call` | `u64 call_id, u32 limits_len, limits JSON, str16 export, bytes args_json` | limits = `{maxResultBytes, maxTaskRequestBytes, maxOutstandingTasks, maxLogMessageBytes}` from `CoreLimits` |
| 0x03 | W→H | `Submit` | `bytes request_json` | fire-and-forget; the id is implicit (§2.3) |
| 0x04 | H→W | `Event` | `bytes event_json` | exactly the `PollOutcome::Event(String)` text (`core.rs:155-162`) [V], with no re-serialisation |
| 0x05 | W→H | `Poll` | `u32 seq` | sent only when the worker would block (`task_poll(1)` with an empty local queue) |
| 0x06 | H→W | `NoTasks` | `u32 seq` | answers the `Poll` with the same `seq`; the worker ignores a stale `seq` |
| 0x07 | W→H | `Sync` | `str16 op, bytes args_json` | the worker blocks until `SyncResult` |
| 0x08 | H→W | `SyncResult` | `bytes result_json` | `{"ok":…}` / `{"error":…}` as in ABI v1 |
| 0x09 | W→H | `Log` | `i32 level, bytes utf8` | levels 0–3 as in ABI v1 |
| 0x0A | W→H | `Result` | `u64 call_id, u64 guest_us, bytes envelope_json` | ABI v1 result envelope |
| 0x0B | H→W | `Shutdown` | — | the worker calls `_exit(0)` |
| 0x0C | H→W | `Chunk` | `bytes event` | proto 2. A stream chunk without base64 or JSON: `0x01 \| u8 id_len \| stream id \| raw bytes` (ABI.md "Binary chunk events"). The worker queues it as is when the cdylib exports `convex_binary_events`, otherwise it converts to the JSON `chunk` event with base64 before the guest sees it. Only data chunks use it; `end`/`error` chunks and `done` stay `Event`. Adjacent chunks of one stream are merged by `ActionBackend` (up to 1 MiB). |

The status pipe (fd 4) carries fixed-size 64-byte records `u8 code | u8[63] detail`, NUL-padded:

| code | Meaning | detail |
|---|---|---|
| 1 | `Oom` | `u64 limit, u64 requested` |
| 2 | `SigSys` | `u32 syscall_nr` |
| 3 | `StackOverflow` | — |
| 4 | `InitFailed` | first 62 bytes of the dlerror text |

`Ready.proto` is 2 since the `Chunk` frame (L3-02); host and worker are one binary, so the bump only guards a stale binary.

**Batched events (L3-06).** Whenever the pump sends an event (in answer to `Poll` or from the armed `poll(true)`), it then drains `backend.poll(false)` and queues every other ready event too, up to the 1 MiB write backlog, so the worker finds them in one read. Submits were already coalesced: the worker buffers `Submit` frames and flushes on `task_poll` or at 64 KiB. Before a blocking `task_poll` sends `Poll` and parks, the worker spins on non-blocking reads for `RUST_NATIVE_SPIN_US` (default 25, 0 = off) when tasks are outstanding; an event that lands in that window needs no `Poll` at all.

The kind numbering and fd numbers are internal. Host and worker are the same binary (D2), so there is no compatibility promise. `proto` in `Ready` is a sanity check against a stale binary on disk, for example when the worker path resolves to an older binary after an in-place upgrade.

### 2.3 Semantics vs ABI v1

| ABI v1 import | Native | Divergence |
|---|---|---|
| `task_submit` → id or `-1` | The worker assigns `id = submits_sent` (0,1,2…). It checks the size and outstanding limits **locally** and returns `-1` exactly as `core.rs:773-790` does [V], without sending anything. Otherwise it buffers a `Submit` frame and returns the id. | Host-side `ActionBackend::submit` errors (an unknown type, or the executor being gone) arrive later as an `Event` `{"type":"done","taskId":id,"error":{"message":"host rejected the task request: <msg>"}}` instead of a synchronous `-1`. The SDK message for `-1` is `"host rejected the task request"` (`convex_fn/src/executor.rs:99-102`) [V], so the text matches. **Required change**: `ActionBackend::submit` (`backend.rs:374-389`) [V] must increment `next_task_id` *before* `parse_request`, so that every `Submit` frame consumes exactly one host id. The wasm path never sees the skipped ids, because gaps are harmless: ids are map keys. The host passes `JsonValue::Null` for an unparsable frame so that it still consumes an id. |
| `task_poll(0)` | Non-blocking drain of fd 3 (`MSG_DONTWAIT`) into a local queue; return the front item or `0`. | none |
| `task_poll(1)` | If the local queue is non-empty, return. Otherwise flush the writes, send `Poll{seq}`, and block on reads until an `Event` arrives (return it) or `NoTasks{seq == current}` arrives (return `-1`). | none |
| `sync_syscall` | One round trip (`Sync` → `SyncResult`). | `now` stays host-answered for parity (`backend.rs:427-431`) [V]. An optional later optimisation is a local `SystemTime`. |
| `log` | Buffered `Log` frame, thread-safe (a mutex around the writer), callable from user threads. | Better than wasm, where user threads do not exist. |
| WASI `fd_write` 1/2 | Real fds 1/2 are pipes. The host splits on `\n` and emits level 1 (stdout) and level 3 (stderr), with a 16 MiB cap per call. This reuses `StdioSink` and `STDIO_CAPACITY` (`core.rs:80-90, 183-310`) [V]. | Ordering between pipe lines and `Log` frames is not guaranteed. Document it. |
| WASI `environ` | The real process env equals the deployment env (D3). | Exact `std::env` semantics. |
| Instances | wasm uses a fresh instance per call (ABI.md "One instance runs one call") [V]. Native reuses a process for up to 1000 calls. | **Statics persist across calls.** Document it, and offer `RUST_NATIVE_MAX_CALLS_PER_WORKER=1` for strict parity. A worker is always recycled after a panic or timeout. |

**Host pump.** Each call gets one tokio task, `CallPump::run`, which owns the `ActionBackend`.

```text
state: armed=false, waiting: Option<u32>=None, stdout/err sinks, deadline future
loop select! (biased):
  _ = deadline                         => kill worker; return Timeout
  frame = sock.read_frame()            => match frame {
        Submit(b)  => backend.submit(parse_or_null(b)); armed = true; (Err => push done-error Event for that id)
        Poll(seq)  => if let Some(ev)=drain_ready(backend.poll(false)) { push(ev) }
                      else if backend.outstanding_tasks()==0 && !has_open_streams { send NoTasks(seq) }
                      else { waiting=Some(seq); armed=true }
        Sync(op,a) => send SyncResult(backend.sync_syscall(op, a))
        Log(l,m)   => backend.log(l, m)
        Result(..) => drain fd1/fd2 non-blocking; return Ok(envelope)
        EOF/err    => read fd4 + exit status; return classified failure (§4.4) }
  ev = backend.poll(true), if armed    => match ev { Event(e) => { push(e); waiting=None }
                                          NoTasks => { armed=false; if let Some(s)=waiting.take() { send NoTasks(s) } }
                                          Empty => unreachable }
  line = stdout/stderr pipes           => sinks → backend.log
```

`backend.poll(true)` is cancel-safe:

- it only awaits `task_responses.recv()` (a tokio mpsc, which is cancel-safe);
- `translate` and the ready-queue push are synchronous (`backend.rs:391-421`) [V].

Use `HostBackend::outstanding_tasks()` plus a new `has_open_streams()`, or reuse the private `outstanding()` (`backend.rs:127-129`) [V] by making it `pub(super)`.

Events are **pushed eagerly** while the guest runs. A later `task_poll(0)` or `task_poll(1)` therefore usually finds the event already in the socket buffer, with no round trip.

### 2.4 Native guest ABI v1 (worker ⇄ cdylib, in-process inside the worker)

The cdylib exports these, all `extern "C"`. Exported dynamic symbols are GC roots, so the linker keeps them [I].

| Symbol | Signature | Meaning |
|---|---|---|
| `convex_native_init` | `(vt: *const HostVtableV1) -> u32` | stores the vtable; returns the native ABI version (1); called once after dlopen |
| `convex_alloc` | `(len: usize) -> *mut u8` | the same as wasm (`sys.rs:20-26`) [V], with `usize` lengths |
| `convex_free_result` | `(handle: i64)` | frees a result handle |
| `convex_native_set_memory_limit` | `(limit: usize)` | optional (D-103); the worker calls it right after a successful `convex_native_init`. The SDK counting allocator then fails allocations above `limit` and reports through `vt.oom`. Missing export or never called means unlimited (the RSS watchdog still applies). |
| `convex_native_trim` | `(check_only: i32) -> i32` | optional (L4-04). After every reply the worker calls it with `check_only=1` on the call thread; the SDK returns 1 when that call's peak live bytes reached 8 MiB, else 0 (no allocator work). On 1 the worker waits for 100 ms without a new frame and then calls it with `0`, which runs `mi_collect(true)` so freed pages go back to the OS (back-to-back calls are never delayed and do not re-fault purged pages). mimalloc only purges while active, so this is what shrinks an idle worker. Missing export: nothing happens. |
| `convex_run:<modulePath>:<name>` | `(args_ptr: *mut u8, args_len: usize) -> i64` | the same name and arguments as wasm (`convex_fn_macros/src/lib.rs:52-58`) [V]; **returns a handle**: a pointer to a boxed `#[repr(C)] ConvexBuf{ptr,len,cap}` holding the envelope, never 0 |
| `convex_mf:<modulePath>:<name>` | `static [u8; N]` | manifest entry bytes, the same content as the wasm `convex:manifest` section; one exported symbol per function (§3.2) |

```rust
#[repr(C)]
pub struct HostVtableV1 {
    pub size: usize,          // size_of::<HostVtableV1>(), for prefix-compatible growth
    pub version: u32,         // 1
    pub task_submit: extern "C" fn(ptr: *const u8, len: usize) -> i64,
    /// 1 = event in (*out_ptr,*out_len) (allocated with the guest's convex_alloc; guest owns it),
    /// 0 = empty (block=0), -1 = no tasks (block=1)
    pub task_poll: extern "C" fn(block: i32, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32,
    pub sync_syscall: extern "C" fn(op: *const u8, op_len: usize, args: *const u8, args_len: usize,
                                    out_ptr: *mut *mut u8, out_len: *mut usize) -> i32,  // 1 always
    pub log: extern "C" fn(level: i32, ptr: *const u8, len: usize),
    /// Called by the SDK counting allocator before it returns null: records OOM on fd 4.
    pub oom: extern "C" fn(limit: usize, requested: usize),
}
```

Buffer ownership follows ABI v1: the host allocates *into the guest* with `convex_alloc`, and the guest frees with its own allocator. No Rust types cross the boundary, so two std copies in one process are fine [I].

### 4.3 How limits map to existing Convex limits and errors

| Limit | Source of truth | Enforcement | User-visible error (exact) |
|---|---|---|---|
| Action timeout | `V8_ACTION_USER_TIMEOUT` (`knobs.rs:209`, used at `rust/mod.rs` before `core.call`) [V] | Host deadline future in `CallPump`, then `SIGKILL` of the process group | `Function execution timed out (maximum duration: {timeout:?})`. This is **identical** to the wasm mapping in `rust/mod.rs` (`CallFailure::Timeout`) [V] |
| Memory | `CoreLimits.memory_bytes` (512 MiB, `core.rs:91-114`; `docs/rust/limits.md:5`) [V], overridable by `RUST_NATIVE_MEMORY_BYTES` | Counting allocator (Rust allocations, exact). RSS watchdog (all memory, ≤ 5 ms lag). `RLIMIT_DATA` or cgroup `memory.max` (hard) | `Uncaught Error: Rust execution ran out of memory (maximum memory usage: 512 MiB)`, modelled on V8's `JavaScript execution ran out of memory (maximum memory usage: 64 MB)` (`isolate/src/termination.rs:346`) [V]. Wasm today surfaces a trap plus std's `memory allocation of N bytes failed` log. Aligning wasm is optional task WN-15 |
| Result size | `max_result_bytes` | Worker check plus host frame-length check | the existing `Function result is too large (...)` [V] |
| Task request size, outstanding tasks | `RUST_MAX_TASK_REQUEST_BYTES`, `RUST_MAX_OUTSTANDING_TASKS` (`knobs.rs:466-479`) [V] | Worker-local, returns `-1` like wasm | same SDK error as wasm |
| Log lines | `MAX_LOG_LINES_V8_ACTIONS` in `ActionBackend::log` (`backend.rs:451-478`) [V]; 32 KiB per message (`CoreLimits`) | Host (unchanged) | unchanged |
| stdout/stderr volume | `STDIO_CAPACITY` 16 MiB | Host sinks | unchanged |
| Concurrency | `RUST_MAX_CONCURRENT_ACTIONS` (`knobs.rs:477`) [V] | Existing semaphore | queueing, unchanged |

### 4.4 Crash classification

The `CallPump` reads the status pipe, the exit status and the last stderr lines.

| Observation | `CallFailure` | Message |
|---|---|---|
| status `Oom`, or watchdog `_exit(137)`, or cgroup `oom_kill` incremented, or `SIGABRT` plus stderr line `memory allocation of` | `OutOfMemory{limit}` | see §4.3 |
| status `StackOverflow`, or stderr `has overflowed its stack` plus `SIGABRT`/`SIGSEGV` | `StackOverflow{stack}` | `Uncaught Error: Rust action overflowed its stack (stack size: 8 MiB)` |
| status `SigSys{nr}` | `Crashed` | `Uncaught Error: Rust action was stopped by the sandbox (disallowed system call {name or nr})` |
| `SIGSEGV`/`SIGBUS`/`SIGILL`/`SIGABRT`, other | `Crashed` | `Uncaught Error: Rust action crashed the worker process (signal {SIGNAME})`; stderr lines were already forwarded as level-3 logs |
| exit code without `Result` | `Crashed` | `Uncaught Error: Rust action exited the worker process (exit status {n})` |
| `SIGKILL` after our deadline | `Timeout` | timeout message |
| a panic envelope | `GuestResult::Error{kind:Panic}` | `Uncaught Error: <msg> at <file:line:col>`, the same as wasm's panic mapping (`guest_error_to_js_error`) [V] |
| bad frame or envelope | `Protocol` | existing `The "use rust" module broke the guest ABI: …` [V] |
| spawn, dlopen or Ready failure | developer error (not a system error) | `Could not start the native Rust worker: <reason>` (breaker applies) |

---

### 5.1 Backend capability discovery

`GET /api/rust_native/info` (admin key) is implemented in `fork/crates/local_backend/src/rust_native.rs`, using `native::target::host_target()`. Response:

```json
{"enabled": true, "mode": "worker", "nativeAbi": [1], "abi": [1],
 "target": {"triple": "aarch64-unknown-linux-gnu", "os": "linux", "arch": "aarch64", "env": "gnu", "glibc": "2.39"},
 "sandbox": {"level": "best-effort", "seccomp": true, "landlockAbi": 4, "netns": false, "cgroup": false, "dropUid": false},
 "pools": [{"artifact": "ab12…", "live": 2, "idle": 1, "busy": 1, "spawned": 9, "crashes": 1, "lastCrash": "SIGSEGV"}]}
```

- glibc comes from `libc::gnu_get_libc_version()` at runtime.
- The triple comes from `std::env::consts::{ARCH, OS}` plus `cfg!(target_env)` [I].
- When the mode is off: `{"enabled": false}`.
- When the endpoint is missing (an older fork or upstream): HTTP 404. The CLI then treats native as unsupported.

### 5.2 CLI build (`npm-packages/convex/src/cli/lib/rust/native.ts`, new)

1. **Selection.** `--rust-runtime wasm|native|auto` (flag) or `convex.json` `{"rust": {"runtime": "native"}}`. The default is `wasm`. With `auto`, use native if `info.enabled` and the target is buildable, otherwise wasm with a gray note.
2. **Target choice.**
   - If the backend target equals the host target (Mac local dev, or Linux on Linux): plain `cargo build`.
   - Else if Linux gnu: `cargo zigbuild --target <triple>.<glibc>`. The glibc floor is the backend's glibc, so `aarch64-unknown-linux-gnu.2.39` for the noble image. zigbuild supports that suffix [V, research report]. When `cargo-zigbuild` or `zig` is missing, give an actionable error: `cargo install cargo-zigbuild` plus `brew install zig`, or `--rust-native-build docker`, which runs `docker run --rm -v … rust:<pinned> cargo build` (option b).
   - `rustup target add <triple>` goes through `toolchain.ts`.
3. **Command.**
   - `cargo build --lib --manifest-path <rustDir>/Cargo.toml --target <t> <profile args>`;
   - `--config 'profile.<p>.panic="unwind"'` and `--config 'profile.<p>.strip="debuginfo"'`;
   - `RUSTFLAGS="--cfg convex_native"`, or `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` to avoid cache churn for host build scripts;
   - `CARGO_TARGET_DIR` unchanged, so the wasm and native artifacts coexist under different triple dirs.
4. **Locate** `target/<triple>/<profile>/lib<crate>.so` (or `.dylib`).
5. **Local manifest read.** A small TS ELF64 and Mach-O 64 reader walks the dynamic symbol table, finds `convex_mf:*` symbols, and reads their bytes from the file. This feeds the same `parseManifest` entry list as wasm (`rust.ts:101`) [V], so codegen (`rust_api.ts`) is unchanged.
6. **Module configs** (replacing `rustModuleConfigs`, `rust.ts:157-169`) [V].
   - The **carrier** is the first module path after sorting. Its source is:
     ```
     //convex-rust-native v1\n
     {"format":1,"nativeAbi":1,"abi":1,"triple":"…","glibcMax":"2.34"|null,"sha256":"<hex of binary>","size":N,
      "functions":{"cpu:cpu_001":"convex_run:bench_rust::cpu:cpu_001",…},"sdk":"<convex_fn version>"}\n
     <base64(binary)>
     ```
   - **Refs** (every other module):
     ```
     //convex-rust-native-ref v1\n{"sha256":"<hex>","carrier":"rust/base.js"}
     ```
   - `environment: "rust"` is unchanged.
   - The size goes into the existing size report (`rust/size.ts`). Warn above 100 MB, error above 180 MB, against the 200 MB `MAX_PUSH_BYTES`.
   - Optional `runtime: "native+wasm"` adds `"wasm": "<base64>"` to the header for fallback (§6).

### 5.6 Knobs (`common/src/knobs.rs`, one block)

| Knob | Default | Meaning |
|---|---|---|
| `RUST_NATIVE_MODE` | `off` | `off` or `worker` |
| `RUST_NATIVE_SANDBOX` | `best-effort` (Linux), `off` (macOS) | `strict`, `best-effort` or `off` |
| `RUST_NATIVE_ALLOW_NETWORK` | `false` | allows the socket syscalls |
| `RUST_NATIVE_DIR` | `convex_local_storage/rust-native` | where artifacts and tmp dirs live |
| `RUST_NATIVE_MEMORY_BYTES` | 512 MiB | per worker |
| `RUST_NATIVE_RSS_SLACK_BYTES` | 64 MiB | watchdog slack above the memory limit |
| `RUST_NATIVE_STACK_BYTES` | 8 MiB | call-thread stack |
| `RUST_NATIVE_MIN_IDLE` | 1 | idle workers kept per pool |
| `RUST_NATIVE_MAX_IDLE` | 4 | total idle workers |
| `RUST_NATIVE_IDLE_TIMEOUT_SECS` | 300 | idle reaping |
| `RUST_NATIVE_MAX_CALLS_PER_WORKER` | 1000 | recycle threshold |
| `RUST_NATIVE_RECYCLE_RSS_BYTES` | 256 MiB | recycle threshold |
| `RUST_NATIVE_IDLE_RECYCLE_RSS_BYTES` | 24 MiB | the reaper (10 s) recycles an idle worker above this RSS, one per key per tick, only while the key has another idle worker (0 disables); backstop for guests without `convex_native_trim`. On macOS the RSS probe (`proc_pidinfo` resident size) keeps counting pages the allocator already released with `MADV_FREE_REUSABLE` (`footprint` shows them as reclaimable), so a trimmed worker can still read high and this backstop is what brings the number down there. When the key's only idle worker is above the threshold the reaper spawns a spare first, then recycles it on the next tick. |
| `RUST_NATIVE_SPAWN_TIMEOUT_MS` | 5000 | wait for `Ready` |
| `RUST_NATIVE_MAX_ARTIFACT_BYTES` | 180 MB | push-time size limit |
| `RUST_NATIVE_UID`, `RUST_NATIVE_GID` | unset | privilege drop |
| `RUST_NATIVE_CGROUP` | unset | delegated cgroup subtree |
| `RUST_NATIVE_EXTRA_LIBS` | empty | extends the shared-library allowlist |
| `RUST_NATIVE_FALLBACK_WASM` | `true` | use an embedded wasm when native is unavailable |

`RustActionRuntime::new` constructs a `NativeRuntime` only when the mode is `worker`. Funrun or Cloud would never set it.

---


# Guest ABI v1 (frozen at W0-13)

Contract between a `"use rust"` module (`wasm32-wasip1`, core module, reactor)
and the host (`RustActionRunner` in the backend, the phase-0 runner, and the
native mock host in `convex_fn_test`). Decision: D-008.

All structured data crossing the boundary is UTF-8 JSON. Convex values use the
**Convex JSON export format** (the same one `UdfArgsJson` and the V8 syscalls
use): `int64` → `{"$integer": "<base64 LE 8 bytes>"}`, `bytes` →
`{"$bytes": "<base64>"}`, non-finite / `-0` floats → `{"$float": "<base64 LE 8 bytes>"}`.

## Memory protocol

- Guest → host: the guest passes `(ptr, len)` into its own linear memory.
- Host → guest: the host calls the guest export `convex_alloc(len) -> ptr`,
  writes the bytes, and returns a **packed** `i64`: `(ptr << 32) | len`.
  The guest takes ownership and frees with its own allocator.
- A packed value of `0` means "nothing" (only `task_poll` with `block = 0`).

## Exports (guest)

| Export | Signature | Meaning |
|---|---|---|
| `memory` | memory | linear memory |
| `_initialize` | `() -> ()` | optional; if present the host calls it once before any other export (WASI reactor init) |
| `convex_alloc` | `(len: i32) -> i32` | allocate `len` bytes, alignment 1 |
| `convex_free` | `(ptr: i32, len: i32) -> ()` | optional (additive, still `abiVersion: 1`); frees a result envelope returned by `convex_run:*` once the host has read it. Hosts that discard the instance after the call may skip it |
| `convex_run:<modulePath>:<name>` | `(args_ptr, args_len: i32) -> i64` | one export per function, e.g. `convex_run:bench_rust::cpu:cpu_001` (`modulePath` = Rust `module_path!()`, see MANIFEST.md). `args` is a Convex JSON **object**. Returns packed result envelope (below). |

One instance runs one `convex_run:*` call. Instances are never reused across
calls in v1.

### Result envelope (returned by `convex_run:*`)

```json
{"ok": <ConvexJSON value>}
{"error": {"message": "…", "data": <ConvexJSON value, optional>, "location": "src/foo.rs:12:5", "kind": "convex" | "panic" | "user"}}
```

`kind = "convex"` and `data` present ⇒ host surfaces it as a `ConvexError`
with that data (same as `throw new ConvexError(data)` in JS).

**Panics.** A panic aborts (trap), so no envelope is returned. Convention on the
existing `log` import: before trapping, the guest panic hook logs one level-3 line
`Uncaught panic: <message> at <file>:<line>:<col>`. If the call traps, the host
uses the last such line as the error (`Uncaught Error: <message> at <file>:<line>`)
instead of the raw trap text. Not a new ABI element; v1-compatible.

Optional hardening: a guest that exports `convex_panic_marker() -> i32` follows the panic line with
`log(4, ptr, 0)`. A host that knows the export counts a prefixed line as a panic only after that
level-4 call (user log lines cannot send level 4); other hosts ignore it (level 4 is just an unknown
level, treated as info) and keep using the prefixed line alone.

## Imports (module `convex`)

| Import | Signature | Meaning |
|---|---|---|
| `task_submit` | `(ptr, len: i32) -> i64` | submit a task request (JSON, below). Returns task id `>= 0`, or `-1` if the request was malformed (host also logs). |
| `task_poll` | `(block: i32) -> i64` | next event (JSON, below), packed. `block = 1`: wait until an event is available (host suspends the wasm fiber; time spent counts against the action timeout). `block = 0`: return `0` if none ready. Returns `-1` if no tasks are outstanding and `block = 1`. |
| `sync_syscall` | `(op_ptr, op_len, args_ptr, args_len: i32) -> i64` | synchronous host call, returns packed JSON `{"ok": …}` / `{"error": {"message": …}}`. |
| `log` | `(level: i32, ptr, len: i32) -> ()` | one log line, UTF-8. Levels: 0 debug, 1 log/info, 2 warn, 3 error. |

WASI (wasip1) imports are also provided: `environ_*` (deployment env vars,
D-009), `clock_*`, `random_get`, `fd_write` for fds 1/2 (→ log lines at level
1/3, split on `\n`), `proc_exit`. No sockets. No filesystem except the optional in-memory `/tmp` preopen (below).

### Task requests (`task_submit`)

`type` selects the variant. Shapes mirror the existing `TaskRequestEnum`.

```jsonc
// AsyncSyscall: name and args are byte-for-byte what the V8 runtime sends.
{"type": "syscall", "name": "1.0/actions/query",    "args": {"name": "messages:list", "args": {…}}}
{"type": "syscall", "name": "1.0/actions/mutation", "args": {"name": "…", "args": {…}}}
{"type": "syscall", "name": "1.0/actions/action",   "args": {"name": "…", "args": {…}}}
{"type": "syscall", "name": "1.0/actions/schedule", "args": {"name": "…", "ts": 1729700000.0, "args": {…}}}
{"type": "syscall", "name": "1.0/getUserIdentity",  "args": {}}
{"type": "syscall", "name": "1.0/actions/vectorSearch", "args": {"query": {"indexName": "table.index", "vector": [0.1], "limit": 5, "expressions": null}}}
{"type": "syscall", "name": "1.0/storageGenerateUploadUrl", "args": {}}
// … any other name accepted by isolate/src/environment/action/async_syscall.rs

// Function address (additive): run/schedule syscalls take exactly one of
//   "name": "messages:list"               a UDF path
//   "functionHandle": "function://…"      a function handle (createFunctionHandle)
//   "reference": "<reference path>"       a component function (JS Symbol.for("toReferencePath"))
// in place of "name" above. The V8 host passes them to ctx.run*/ctx.scheduler as
// getFunctionAddress does; the wasmtime host forwards them to the real syscall.

// AsyncOp
{"type": "fetch", "url": "https://…", "method": "GET", "headers": [["k","v"]], "body": "<base64>" | null}
{"type": "sleep", "ms": 10}
{"type": "storageStore", "contentType": "image/png" | null, "body": "<base64>"}
{"type": "storageGet", "storageId": "kg2…"}
```

Notes on the syscalls above (additive, still `abiVersion: 1`):

- `1.0/actions/vectorSearch`: args are the real syscall's without `requestId` and `version` (the host adds
  them). `indexName` is `"<table>.<index>"`; `expressions` is the filter JSON (`{"$eq": [{"$field": f}, v]}`,
  `{"$or": [...]}`) or `null`. Result `{"results": [{"_id", "_score"}]}`. The V8 host calls `ctx.vectorSearch`
  and rebuilds the filter with the public `q.eq` / `q.or` builder, so only those two forms exist there.
- `1.0/storageGenerateUploadUrl`: result is the URL string (V8 host: `ctx.storage.generateUploadUrl()`).
- Any other syscall name: the V8 host forwards it to the runtime's `Convex.asyncSyscall(name, JSON.stringify(args))`
  and returns the parsed result; it can only succeed for syscalls that need no per-request `requestId`, since
  the action's request id is not visible to user code. Without a `Convex` global it is `Unsupported syscall`.


Request bodies are inline base64 by default. The optional `net/*` ops below add three more forms
(additive, only used after the guest probed `net/caps`; a host without them keeps the inline form):

- `"body": {"ptr": <u32>, "len": <u32>}`: a reference into guest memory (fetch and `storageStore`). The
  host copies the bytes during `task_submit`, so the guest may reuse the buffer as soon as it returns.
  The SDK uses this from 8 KiB up to `maxBodyRef`; no base64 and no JSON copy of the payload.
- `"body": {"stream": true}` (fetch only): the body is fed by the guest with `net/body_write` /
  `net/body_end` after `task_submit` returned (see below); the host builds a `ReadableStream`
  (`duplex: "half"`) from the chunks. Bodies larger than `maxBodyRef` are sent this way in slices.
- Cancellation: a task submitted under `cancel` support is aborted with `net/cancel`.

HTTP action tasks (additive, still `abiVersion: 1`; V8 host only). `#[http_action]` exports receive
`{"method","url","headers":[[k,v]],"bodyStreamId":<id>|null}` as their args (the request body is a host stream read
with the usual chunk events) and answer in one of two ways:

```jsonc
{"type": "httpStart", "status": 200, "headers": [["k","v"]]}   // starts a streamed response; status 200..599
{"type": "httpWrite", "body": "<base64>"}                      // one body piece; completes once the client has room
{"type": "httpEnd"}                                            // ends the body
```

A buffered response is the result envelope `{"ok": {"status", "headers", "body": "<base64>" | null}}`. After
`httpStart`/`httpEnd` the guest returns `{"ok": {"streamed": true}}`. An error after `httpStart` errors the response
body (truncated response); before it, the call throws. `httpWrite` fails once the client closed the response. Both
bodies are limited to 20 MiB (Convex's HTTP action limit); the buffered envelope is also bound by the 16 MiB result
limit, so stream larger bodies. The wasmtime and native hosts do not implement these tasks. Instead the request head
may carry `"body": "<base64>" | null` (the whole request body inline, no `bodyStreamId`) and `"buffered": true`; with
`buffered` a streaming response is collected in the guest (20 MiB cap) and returned as the buffered envelope.

### Events (`task_poll`)

```jsonc
{"type": "done", "taskId": 3, "ok": <result>}
{"type": "done", "taskId": 3, "error": {"message": "…", "data": <ConvexJSON, optional>}}
{"type": "chunk", "streamId": "uuid", "data": "<base64>"}
{"type": "chunk", "streamId": "uuid", "end": true}
{"type": "chunk", "streamId": "uuid", "error": "…"}
```

`ok` results per request type:

| Request | `ok` |
|---|---|
| `syscall` | the syscall's JSON result, parsed (e.g. ConvexJSON value for `actions/query`) |
| `fetch` | `{"status": 200, "statusText": "OK", "headers": [["k","v"]], "url": "…", "bodyStreamId": "uuid" \| null}`; body follows as `chunk` events |
| `sleep` | `null` |
| `storageStore` | `"<storage id>"` |
| `storageGet` | `null` or `{"bodyStreamId": "uuid", "contentLength": 123, "contentType": "…" \| null}`; body follows as `chunk` events |

Chunks for a stream may arrive before or after the `done` event that names the
stream; the guest buffers by `streamId`.

#### Binary chunk events (additive, still `abiVersion: 1`)

A module that exports `convex_binary_events: () -> i32` (any value) parses an
extra event form, and a host may then deliver stream data as

```text
0x01 | u8 id_len | stream id (UTF-8, id_len bytes) | raw chunk bytes (the rest)
```

instead of the JSON `chunk` with base64 `data`. JSON events always start with
`{`, so the first byte tells the forms apart. `end` and `error` chunks and all
`done` events stay JSON. A host sends the binary form only to modules that
export `convex_binary_events`; modules built before it get JSON exactly as
before, and a new module still parses JSON from an older host. Independent of
the encoding, a host may merge adjacent chunks of one stream into one event
(up to about 1 MiB), so a guest must not rely on the chunk boundaries of the
upstream response.

### Sync syscalls (`sync_syscall`)

| op | args | ok |
|---|---|---|
| `componentArgument` | `{"name": "…"}` | ConvexJSON value or `null` (V8 host: internal sync syscall `1.0/componentArgument`, `{value}` / `{}`) |
| `now` | `{}` | ms since epoch as f64 (same clock as `Date.now()` in actions) |

### Optional sync ops: `crypto/*` (additive, still `abiVersion: 1`; crypto contract version 2)

A wasm32 guest carries **no** signature, AEAD, digest or HMAC code (D-L4a): the host
runs them natively (WebCrypto on the V8 host, aws-lc-rs on the wasmtime host). They
are ordinary `sync_syscall` ops, **not new imports**, so a module still links on an
older host and a host with them still runs modules that never call them. There is no
pure-Rust fallback in wasm32: the guest probes `crypto/caps` once per instance
(`convex_fn::crypto::host_supports` / `require`) and a missing op or algorithm is the
error `host lacks crypto/<op> <alg>: update convex-rust to >= X (V8 host) / the backend (wasmtime host)`.
MD5 (no WebCrypto equivalent) stays in the guest.

Byte arguments are `{"ptr", "len"}` references into the guest's linear memory (a
base64 string is accepted too). The host reads them in place; `crypto/aead` transforms
`data` in place. A 10 MB payload is therefore never copied through JSON or base64.
Small results (signature, digest, MAC, tag, public key) are base64 in the JSON reply.
An out-of-range reference is an error reply.

| op | args | ok |
|---|---|---|
| `crypto/caps` | `{"version": 2}` | `{"version": 2, "ops": {"verify": [alg...], "sign": [...], "pubkey": [...], "aead": [...], "digest": [...], "hmac": [...]}}` |
| `crypto/verify` | `{alg, key, msg, sig}` | `true` / `false` |
| `crypto/sign` | `{alg, key, msg}` | base64 signature |
| `crypto/pubkey` | `{alg, key}` | base64 public key (ES256/ES384/EdDSA only) |
| `crypto/aead` | `{alg, dir: "seal"\|"open", key, nonce, aad, data, tag?}` | seal: `{"tag": base64}`; open: `true` / `false` |
| `crypto/digest` | `{alg, data}` | base64 digest |
| `crypto/hmac` | `{alg, key, data}` | base64 MAC |

- verify/sign `alg`: `RS256/384/512` (PKCS#1 v1.5), `PS256/384/512` (salt length = digest length), `ES256` (P-256), `ES384` (P-384), `EdDSA` (Ed25519).
- verify `key`: PKCS#1 `RSAPublicKey` DER (RS*/PS*); SEC1 uncompressed point `04 || X || Y` (ES*); 32 raw bytes (EdDSA). sign `key`: PKCS#8 DER private key. `sig`: raw RSA signature; ECDSA fixed `r || s` (not DER).
- RSA modulus policy on both hosts: 2048..8192 bits, otherwise an error (not "invalid").
- aead `alg`: `A128GCM` / `A256GCM`; 12-byte `nonce`, 16-byte `tag`; the key size must match. `open` leaves `data` untouched and answers `false` when the tag does not authenticate.
- digest `alg`: `SHA-1`, `SHA-256`, `SHA-384`, `SHA-512`; hmac `alg` names the hash, any key length (an empty key is the same as one zero byte on V8).
- Guests may compute small digests locally: `convex_fn::crypto::{digest,hmac}` run SHA-256/384/512 (and HMAC over them) in the guest when the input is <= 2 KiB (key + message); larger inputs go to the host.
- Errors: `{"error": {"message": "crypto/<op>: ..."}}`; an unknown op answers `unknown sync op crypto/<op>`.

Hosts: V8 (`packages/convex-rust/src/runtime/crypto.ts`, `crypto.subtle`; the call is a JSPI
`Suspending`, so the guest sees it synchronously; Ed25519 is probed live for `caps`, as stock
Convex V8 may lack it) and wasmtime (`isolate/src/environment/rust/host_crypto.rs`, aws-lc-rs).
SDK: `convex_fn::crypto`; `bench/convex/rust/src/jwt_host.rs` plugs it into jsonwebtoken 10 as a
`CryptoProvider` on wasm32.

### Optional WASI: in-memory `/tmp` (additive, still `abiVersion: 1`)

The V8 host (`packages/convex-rust/src/runtime/tmpfs.ts`) preopens one directory, fd 3 = `/tmp`
(`fd_prestat_get` / `fd_prestat_dir_name`), backed by an in-memory file system. A module that imports none of
the `path_*` / `fd_*` file functions is unaffected; a host without it answers `EBADF` to `fd_prestat_get`, so
libc sees no preopens and `std::fs` fails with an I/O error.

- Per call: empty when a call starts, discarded when it ends (instance reuse does not carry files over).
- Implemented: `path_open`, `fd_read`, `fd_pread`, `fd_write`, `fd_pwrite`, `fd_seek`, `fd_tell`, `fd_close`,
  `fd_readdir`, `path_filestat_get`, `fd_filestat_get`, `fd_filestat_set_size`, `fd_filestat_set_times`,
  `path_filestat_set_times`, `path_create_directory`, `path_unlink_file`, `path_remove_directory`,
  `path_rename`, `fd_fdstat_get`, `fd_fdstat_set_flags`, `fd_sync`, `fd_datasync`, `fd_advise`,
  `fd_allocate`, `fd_prestat_get`, `fd_prestat_dir_name`. Symlinks and hard links answer `EPERM`; `..` out of
  `/tmp` answers `ENOTCAPABLE`. Unlinked files that are still open stay readable until closed.
- Storage: file data is held in 4 KiB..1 MiB segments allocated with the guest's `convex_alloc` and returned with
  `convex_free` when the call ends, so it lives in wasm linear memory and not in the host's JS heap (an isolate
  in the default Convex runtime has a 64 MiB limit on that). A module without those exports gets plain
  `Uint8Array` segments.
- Cap on the allocated bytes of all files: 64 MiB by default, `RunRustOptions.tmpBytes` or the deployment variable
  `CONVEX_RUST_TMP_BYTES` (`0` = no preopen). Beyond it, writes fail with `ENOSPC`.
- `std::env::temp_dir()` panics on `wasm32-wasip1` regardless of `TMPDIR`; the SDK offers `convex_fn::fs::temp_dir()`
  (`/tmp`).
- The wasmtime host does not implement it (no preopens there).

### Optional sync ops: `intl/*` (additive, still `abiVersion: 1`)

Time zones and locale formatting from the host runtime's `Intl` (a wasm32 guest has no tz database). Ordinary
`sync_syscall` ops. An unknown zone or locale is an error reply, never UTC; a host without the op answers
`unknown sync op intl/<op>`, which `convex_fn::intl` turns into `host lacks intl/<op>: update convex-rust`.

| op | args | ok |
|---|---|---|
| `intl/tzOffset` | `{"tz": "Europe/Lisbon", "epochMs": 1721044800000}` | offset from UTC in seconds (integer, east positive, DST included) |
| `intl/format` | `{"kind": "date" \| "number", "value": <epoch ms \| number>, "locale": "en-GB" \| "", "options": {…Intl options…}}` | formatted string (`Intl.DateTimeFormat` / `Intl.NumberFormat`) |

SDK: `convex_fn::intl::{tz_offset_secs, local_to_epoch_ms, format_date, format_number}`. Hosts: V8 only
(`packages/convex-rust/src/runtime/intl.ts`). Alternatively `jiff` (bundled tzdb) or `chrono-tz` run in the guest
without host support, at a cost in wasm size.

### Optional sync ops: `net/*` (additive, still `abiVersion: 1`; net contract version 1)

Fetch cancellation, memory references for large request bodies and guest-fed (streamed) request bodies.
Like `crypto/*` they ride on `sync_syscall`; the guest probes `net/caps` once per instance and falls
back to the old behaviour on `unknown sync op net/caps` (inline base64 bodies, no cancellation, a
streamed upload buffered into one body). All replies are `{"ok": ...}` / `{"error": {"message": ...}}`.

| op | args | `ok` |
|---|---|---|
| `net/caps` | `{"version": 1}` | `{"version": 1, "ops": ["cancel", "bodyRef", "requestStream"], "maxBodyRef": <bytes>}` |
| `net/cancel` | `{"taskId": n}` or `{"streamId": "s…"}` | `true` if something was cancelled, `false` for an unknown / finished id |
| `net/body_write` | `{"taskId": n, "data": {"ptr", "len"}}` | `true` (see backpressure) |
| `net/body_end` | `{"taskId": n}` | `true` |
| `net/body_abort` | `{"taskId": n, "message": "…"}` | `true` |

- `net/cancel {taskId}`: aborts the host work (the fetch's `AbortController`, and a streamed request body).
  Also cancels a `sleep` task (the host timer is dropped), which is how timeouts and `select` losers stay clean.
  The task's `done` event is **not** delivered (the guest dropped the future); a body stream of the same
  request errors on the host and the guest discards its events. The guest SDK calls it when a fetch future is dropped
  (a `reqwest` timeout, `race`, `select`), so a timed-out request really stops.
- `net/cancel {streamId}`: aborts a response body the guest stopped reading (the rest of the download is
  not fetched); its `chunk` / `end` events stop. The SDK calls it when a `FetchStream` is dropped before the
  end. The same abort ends the whole request: a stream cancel and a task cancel are equivalent for the
  underlying fetch.
- `net/body_write` copies `data` out of guest memory and enqueues it on the request stream. It is a
  JSPI-suspending call: when more than 1 MiB is queued, the host does not answer until the network has
  drained the queue, so the guest is paced by the upload (backpressure). It fails once the request body is
  closed (the request finished, failed or was cancelled); the SDK then stops writing and returns the
  request's own result.
- `net/body_end` closes the stream cleanly; `net/body_abort` errors it, which fails the fetch with
  `message`. A fetch that rejects aborts its request body so a blocked writer is released.
- `maxBodyRef` is the largest single `{ptr,len}` body (64 MiB on V8: one JS `ArrayBuffer`); the SDK
  streams anything larger in 1 MiB slices.
- A streamed request body cannot be replayed: the SDK fails a redirect that would resend it
  (307 / 308 / a `POST` kept as `POST`); 301 / 302 / 303 downgrade to a body-less `GET` as usual.

Hosts: V8 (`packages/convex-rust/src/runtime/host.ts`; relies on `fetch` accepting an `AbortSignal` and a
`ReadableStream` request body with `duplex: "half"`, which the Convex runtime documents but which has
not been exercised on a real cloud deployment yet). The wasmtime host in `isolate/` answers
`unknown sync op net/caps`, which keeps the previous behaviour.

## Limits (host-enforced)

- Memory: configurable cap (default 512 MiB, same as Node action default).
- Time: epoch interruption; same user timeout as V8 actions (knob `V8_ACTION_USER_TIMEOUT`, default 1800 s).
- Result envelope size: same limit as action return values.

## Versioning

The manifest carries `abiVersion: 1`. A host rejects modules with an unknown
`abiVersion` at push time.

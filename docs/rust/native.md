# Native mode (experimental, opt-in)

> **Status: experimental, not yet released.** The default and launch runtime for "use rust" is WebAssembly
> (everything else in these docs). Native mode is an opt-in alternative for self-hosted backends, being built
> now and proposed upstream. It is off by default, and the details here (flags, knobs, messages) may still
> change before release.

## Read this first: trust model

**Native Rust code runs as an operating-system process on the backend host.** Wasm runs inside a sandbox that
cannot touch the host unless the runtime allows it; native code is ordinary compiled machine code, restricted
only by what the operating system is told to enforce. A bug or malicious dependency in your crate is therefore
as serious as one in any program you run on that machine.

- Use native mode only on **trusted, single-tenant, self-hosted** backends, where you control every crate that
  is pushed. Do not use it where untrusted people can push code.
- The sandbox is strong only on Linux, and even there it is "best effort" by default (it applies what the
  kernel offers; a warning at startup and the `sandbox.missing` / `sandbox.degraded` fields of
  `/api/rust_native/info` list what is not enforced; use `RUST_NATIVE_SANDBOX=strict` in production). On
  macOS there is **no sandbox**, and the backend refuses to start native mode unless you set
  `RUST_NATIVE_ALLOW_UNSANDBOXED=true`. See the [platform matrix](#platform-matrix).
- The stock image runs the backend as root. Without `RUST_NATIVE_UID` and `RUST_NATIVE_GID`, workers run as
  root (confined by Landlock and seccomp); a warning is logged. Set both to an unprivileged user that can read
  `RUST_NATIVE_DIR/bin` (the artifacts are mode `0500`, owned by the backend user, so chown the directory or
  run the backend as that user).
- With `RUST_NATIVE_ALLOW_NETWORK`, a worker can open TCP connections to **any host on ports 80 and 443**,
  including private, link-local and cloud-metadata addresses (`169.254.169.254`), and UDP (DNS) is not
  restricted. seccomp cannot inspect socket addresses and the design has no network namespace or egress proxy.
  If you allow the network, block those destinations at the host or network level (egress firewall, metadata
  service hop limit or token requirement).
- A native worker's environment (the deployment env vars) is readable through `/proc/<pid>/environ` by any
  process with the same uid. Use a dedicated worker uid (`RUST_NATIVE_UID`) so nothing else shares it.
- Wasm remains the right choice whenever isolation matters more than raw speed.

What native mode applies on Linux: a cleared environment holding only the deployment env vars, resource
limits, `no_new_privs`, Landlock file-system rules (no access to the backend's data directory), a seccomp
syscall filter (no `execve`, `fork`, `ptrace`, `mount`, ... and no sockets unless allowed), an optional
privilege drop and cgroup memory limit, and a host-side `SIGKILL` for timeouts. Each worker runs one call at a
time.

## When to use native or wasm

| Choose | When |
|---|---|
| **wasm** (default) | Production, shared or untrusted code, portable artifact, no build-host constraints. |
| **native** | A trusted self-hosted backend, where your actions are limited by wasm: they need real threads (for example `rayon`), C libraries that do not build for wasm, or native `std` behaviour that wasm lacks. |

## Measured performance (local, native vs "use node")

Final benchmark, 100 cases, same fork backend, macOS aarch64, release builds, end-to-end client time
(`client_ns`). Full tables, per-case numbers, fairness notes and caveats: [bench/reports/final-2026-10-25/report.html](../../bench/reports/final-2026-10-25/report.html).

| Median of per-case ratios native / node | ratio | cases faster / slower (5% band) |
|---|---|---|
| warm p50 (100 iterations) | 0.43 | 94 / 2 |
| warm p95 | 0.41 | 92 / 3 |
| cold p50 (only 2 rounds, indicative) | 0.37 | 94 / 4 |
| concurrency 10, inverse throughput | 0.18 | 91 / 7 |
| concurrency 50, inverse throughput | 0.22 | 73 / 23 |

A ratio below 1 means native is faster. Native wins most on compute- and data-heavy cases; the gap is smallest
for `net` (0.76 warm) and for cases dominated by Convex round trips (`runMutation`, storage). At concurrency 50 the
cheap cases (`base`, some `cb`/`ser`/`net`) are slower than node in throughput (ratio above 1). Numbers depend on
the workload and the machine; two gates were not met in the run: idle worker RSS (32.7 MB at the end of the long run,
17 MB when measured alone; a test-timing artifact being fixed) and spawn-path overhead (0.22 ms against the 0.2 ms
limit). Some pairs differ intrinsically (for example sharp is multithreaded, Rust single-threaded), noted per case in the report.

### Native vs wasm vs node, same machine

Same final benchmark, one fork backend serving all three runtimes (node, Rust wasm on V8, Rust native), 100 cases,
50 iterations, concurrency 1, macOS aarch64, release builds. End-to-end client time; local numbers have no network
and are not comparable with the cloud numbers in [cloud.md](cloud.md). Native is self-hosted only. Median of per-case
ratios, numerator time / denominator time (above 1 = the denominator is faster); the last column counts cases where
native is more than 5% slower than wasm.

| Median of per-case ratios | node / wasm | node / native | wasm / native | native slower than wasm |
|---|---|---|---|---|
| warm p50 (client) | 1.86 | 2.43 | 1.39 | 12 of 100 |
| cold p50 (client, 2 rounds, indicative) | 2.30 | 2.90 | 1.35 | 21 of 100 |
| warm server exec p50 | 2.11 | 2.73 | 1.43 | 12 of 100 |
| cold server exec p50 | 2.54 | 3.13 | 1.40 | 19 of 100 |

Median over all cases of the per-case p50 (ms): warm node 38.6, wasm 21.0, native 12.2; cold node 52.8, wasm 21.8,
native 13.6. Native wins most on `data`, `mem`, `cpu` and `media` (wasm/native 1.5 to 1.8 warm). It does not win on
cheap, call-dominated cases: `cb` (callbacks into Convex) is on par warm (3.38 ms native vs 3.30 ms wasm) and slower
cold in this run (5.94 ms vs 3.68 ms), and `net` is on par. The cold `cb` gap did not reproduce in three reruns
(native 2.9 to 3.4 ms, wasm 3.6 to 4.2 ms): with 2 cold rounds, whichever runtime's first call after a restart lands
on a `cb` case pays the one-off spawn and first-touch cost (about 200 ms), which skews that category's median.
Per-case numbers:
the "Local: node vs Rust wasm vs Rust native" section of the report.

## Platform matrix

| Backend host | Native mode | Sandbox |
|---|---|---|
| Linux x86_64 or aarch64, glibc (for example the `ubuntu:noble` backend image) | Supported | `best-effort` by default: rlimits, Landlock, seccomp, `no_new_privs`; optional cgroup v2 and uid/gid drop. `strict` refuses to start a worker if Landlock or seccomp is missing. |
| macOS (local development) | Supported for dev | None (`RUST_NATIVE_SANDBOX=off`). Requires `RUST_NATIVE_ALLOW_UNSANDBOXED=true`, otherwise the backend refuses to start; a warning is logged. Only a cleared env, resource limits, a memory watchdog and host `SIGKILL`. |
| Linux with musl (Alpine) | Not supported | The artifact must link against glibc. |
| Windows | Not supported | Use wasm. |
| Convex Cloud | Not available | Native mode is not enabled there. |

The library must be built for the backend's own OS, CPU and a glibc no newer than the backend's. The backend
checks this at push time (see [errors](#errors)). Only system libraries (libc, libm, libgcc_s and a few
others) may be dynamically linked; extend the list with `RUST_NATIVE_EXTRA_LIBS` if you must.

## Enabling it

1. Start the backend with the mode on:

   ```sh
   RUST_NATIVE_MODE=worker convex-local-backend ...
   ```

   Check it with `GET /api/rust_native/info` (admin key). It reports `enabled`, the host target (triple,
   glibc), the sandbox level that is actually active, and the live worker pools. A backend without the
   endpoint (HTTP 404) does not support native mode.

2. Push with the native runtime:

   ```sh
   npx convex dev --rust-runtime native
   npx convex deploy --rust-runtime native
   ```

   `--rust-runtime wasm|native|auto`; `auto` uses native when the backend has it enabled and the target can be
   built, otherwise wasm. It can also be set in `convex.json`: `{"rust": {"runtime": "native"}}`. The default
   is `wasm`.

Pushing a native-only build to a backend that has the mode off is an error at push time; it never silently
switches runtime. The runtime is chosen per push for the whole crate. Your crate stays a `cdylib` (the same
`crate-type` as for wasm), and `#[action]` code does not change.

When the backend and your machine have the same OS and architecture (a Mac with a local Mac backend, or Linux
to Linux), the CLI runs a plain `cargo build`.

## Cross-compiling from macOS to a Linux backend

To push from a Mac to a Linux backend (for example the Docker image), the CLI builds with
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild), which uses `zig` as the linker and lets you
target an older glibc:

```sh
cargo install cargo-zigbuild
brew install zig
rustup target add aarch64-unknown-linux-gnu      # or x86_64-unknown-linux-gnu; the CLI does this too
```

The CLI reads the backend's target from `/api/rust_native/info` and builds `<triple>.<glibc>`, for example
`aarch64-unknown-linux-gnu.2.39` for the `ubuntu:noble` image. If `cargo-zigbuild` or `zig` is missing, the
CLI stops with the install commands above. An alternative is `--rust-native-build docker`, which builds inside
a pinned `rust` container.

The artifact size appears in the size report. The CLI warns above 100 MB and errors above 180 MB.

## Knobs

Set these as environment variables on the backend.

| Knob | Default | Meaning |
|---|---|---|
| `RUST_NATIVE_MODE` | `off` | `off` or `worker` |
| `RUST_NATIVE_SANDBOX` | `best-effort` (Linux), `off` (macOS) | `strict`, `best-effort` or `off` |
| `RUST_NATIVE_ALLOW_UNSANDBOXED` | `false` | required to run with sandbox level `off` (the only level outside Linux); development only |
| `RUST_NATIVE_ALLOW_NETWORK` | `false` | allows the socket syscalls (TCP to ports 80 and 443 on any address; see the trust model) |
| `RUST_NATIVE_DIR` | `convex_local_storage/rust-native` | where artifacts and tmp dirs live (must not be on a `noexec` mount) |
| `RUST_NATIVE_MEMORY_BYTES` | 512 MiB | memory limit per worker |
| `RUST_NATIVE_RSS_SLACK_BYTES` | 64 MiB | watchdog slack above the memory limit |
| `RUST_NATIVE_STACK_BYTES` | 8 MiB | call-thread stack |
| `RUST_NATIVE_MIN_IDLE` | 1 | idle workers kept per pool |
| `RUST_NATIVE_MAX_IDLE` | 4 | total idle workers |
| `RUST_NATIVE_IDLE_TIMEOUT_SECS` | 300 | idle reaping |
| `RUST_NATIVE_MAX_CALLS_PER_WORKER` | 1000 | recycle threshold; `1` gives wasm-like isolation between calls |
| `RUST_NATIVE_RECYCLE_RSS_BYTES` | 256 MiB | recycle threshold |
| `RUST_NATIVE_IDLE_RECYCLE_RSS_BYTES` | 24 MiB | an idle worker above this RSS is recycled, while the same function set keeps another idle worker (`0` disables) |
| `RUST_NATIVE_SPAWN_TIMEOUT_MS` | 5000 | wait for the worker to become ready |
| `RUST_NATIVE_MAX_ARTIFACT_BYTES` | 180 MB | push-time size limit |
| `RUST_NATIVE_UID`, `RUST_NATIVE_GID` | unset | drop privileges to this user and group (the stock image runs the backend as root; a warning is logged while unset). With a drop and no cgroup, `RLIMIT_NPROC` (4096 per uid) applies |
| `RUST_NATIVE_CGROUP` | unset | delegated cgroup v2 subtree, for a hard memory limit and exact OOM detection. Without it, workers get `RLIMIT_CPU` (4 h of CPU time) as a backstop |
| `RUST_NATIVE_EXTRA_LIBS` | empty | extends the shared-library allowlist |
| `RUST_NATIVE_FALLBACK_WASM` | `true` | use an embedded wasm when native is unavailable (only for pushes built with both) |

A guest built with the current `convex_fn` also exports `convex_native_trim`: after a call that peaked at 8 MiB or more of live heap, an idle worker returns freed pages to the OS.

The time limit is the same `V8_ACTION_USER_TIMEOUT` as for wasm and TS actions; see [limits.md](limits.md).

## Differences from wasm

| Topic | Wasm | Native |
|---|---|---|
| Isolation | Language-level sandbox | A process; the sandbox depends on the OS (see above) |
| State between calls | Fresh instance per call | A process serves up to 1000 calls, so **`static` and `thread_local` values persist across calls**. Do not keep per-request state in them. Set `RUST_NATIVE_MAX_CALLS_PER_WORKER=1` for wasm-like behaviour. A worker is always discarded after a panic, timeout or crash. |
| Environment | Deployment env vars via the host | The real process environment, holding exactly the deployment env vars: normal `std::env` behaviour |
| Threads | Cannot start | Work (for example `rayon`); they share the call's memory and time budget |
| Log ordering | One ordered stream | `print!`/`eprint!` and `ctx` logging travel on different channels, so their relative order is **not guaranteed** |
| Task submit errors | A rejected `ctx` request fails at once | Rejections that come from the host arrive slightly later, with the same message |
| Timeout | Interrupts the instance | The worker process is killed; the error text is identical |
| Out of memory | A trap, plus std's `memory allocation of N bytes failed` log | A clear error (below) |
| Filesystem and sockets | None | Limited by the sandbox: a private per-worker temp directory (removed when the worker exits) only, and no sockets unless `RUST_NATIVE_ALLOW_NETWORK` is set. Use `ctx.fetch` and storage as in wasm. |
| Build | `wasm32-wasip1` | Host or cross-target `cdylib` with `panic = "unwind"`; the CLI sets this |
| Artifact | Portable | Tied to one OS, CPU and glibc range |

Queries, mutations and components are still not supported in Rust ([limits.md](limits.md)). HTTP actions work, in a buffered form (below).
Result, argument and size limits are the same as for wasm.

## HTTP actions

`#[http_action]` functions run on the native runtime (and on the fork's wasmtime runtime) through a small V8 shim. The
fork CLI generates `convex/_generated/rustHttp` with `rustHttpAction("<module>:<fn>")`; use it as the route handler:

```ts
import { httpRouter } from "convex/server";
import { rustHttpAction } from "./_generated/rustHttp";
const http = httpRouter();
http.route({ path: "/hook", method: "POST", handler: rustHttpAction("web:hook") });
export default http;
```

The shim reads the request, calls the Rust function as an internal action (`rust/web:hook`; the function is registered
`internal`, so clients cannot call it with a forged request) and builds the `Response` from the returned status, headers
and body. Status, headers, body, errors (a failed handler or a panic is a 500, as in the wasm path) and the `Ctx` are the
same as in [cloud.md](cloud.md#http-actions). Differences:

- Buffered both ways. `req.chunk()`/`bytes()`/`text()`/`json()` work, but the body arrives whole, base64 encoded inside
  the action argument, so a request body must stay under about 12 MiB (16 MiB argument limit). A `.streaming(..)`
  response is collected inside the guest and sent whole (20 MiB cap); the client does not see it tick by tick.
- The route needs the generated helper, so `convex/http.ts` imports `./_generated/rustHttp` instead of `./rust/<module>`.

## Errors

| Situation | Message |
|---|---|
| Timeout | `Function execution timed out (maximum duration: {timeout})` (same as wasm) |
| Memory limit | `Uncaught Error: Rust execution ran out of memory (maximum memory usage: 512 MiB)` |
| Stack overflow | `Uncaught Error: Rust action overflowed its stack (stack size: 8 MiB)` |
| Blocked by the sandbox | `Uncaught Error: Rust action was stopped by the sandbox (disallowed system call {name or nr})` |
| Segfault, abort or similar | `Uncaught Error: Rust action crashed the worker process (signal {SIGNAME})`; the last stderr lines are in the function logs |
| Process exits on its own | `Uncaught Error: Rust action exited the worker process (exit status {n})` |
| Panic | `Uncaught Error: <msg> at <file:line:col>` (same as wasm) |
| Worker could not start | `Could not start the native Rust worker: <reason>` |
| Backend has native mode off | `This backend does not run native Rust actions (RUST_NATIVE_MODE is off). Push with --rust-runtime wasm, or enable it.` |
| Wrong glibc | `the Rust library needs glibc 2.40 but the backend has 2.39; rebuild with --target <triple>.2.39` |
| Artifact dir not executable | `RUST_NATIVE_DIR is on a noexec mount` |

A crash fails only the call that caused it. Repeated failures to start a worker open a circuit breaker for
that artifact (calls fail fast with the last start error; retries back off from 1 s, doubling up to 5 minutes).

See also: [limits.md](limits.md), [crates.md](crates.md), [self-hosted-fork.md](self-hosted-fork.md).

# Native Rust worker process for self-hosted "use rust" actions (opt-in, experimental)

> Draft status: this pull request is stacked on the wasm "use rust" runtime patch (`convex-rust-wasm.patch`; its
> own description is `PR-BODY-wasm.md`). The first commit on the branch is that runtime; the second commit is
> the native worker. Please review the second commit. Numbers marked "at preparation" were measured on the patch set prepared locally; regenerate
> them with `release/make-patches.sh` before filing if the fork has moved.

## Motivation

The wasm runtime for Rust actions is portable and sandboxed by construction. It also has limits that some
self-hosters hit:

- no real threads (for example `rayon`);
- C libraries and crates that do not target `wasm32-wasip1` cannot be used;
- every call crosses the wasm boundary and pays for host callbacks;
- `std` behaviour that wasm lacks.

This adds an optional second runtime. The same `#[action]` code is built as a native shared library and run in
a separate, sandboxed worker process that the backend manages. It is off by default and changes nothing while
off.

## Design summary

Full document: `docs/rust-native-runtime-design.md`.

- The user crate stays a `cdylib`. A generic worker `dlopen`s it in a separate process.
- The worker is the backend binary re-executed with a hidden first argument (`__rust_native_worker`), so there
  is no new binary to package and no host/worker version skew.
- One call at a time per worker. A warm pool per (artifact hash, environment hash) with recycling, an idle
  reaper and a spawn-failure breaker.
- Host and worker talk over a Unix socketpair with length-prefixed frames carrying the same JSON as the wasm
  ABI; stdout and stderr are separate pipes; a status pipe carries fatal notes.
- Timeouts are a host-side `SIGKILL`. Memory is enforced by a counting allocator, an RSS watchdog and an
  rlimit or cgroup backstop. Error texts match the wasm runtime where one exists.
- The artifact travels in the existing module-source channel (a carrier module plus references), so the push
  protocol is unchanged. Push-time validation parses the ELF or Mach-O (never executes it): format, dependencies,
  glibc floor, exports, and manifest.
- `GET /api/rust_native/info` lets the CLI discover whether native is enabled and which target to build for.

## Trust model (please read)

Native mode runs user-supplied machine code as an OS process on the backend host. It is intended only for
trusted, single-tenant, self-hosted backends where the operator controls every pushed crate. It is not a
multi-tenant isolation boundary and is never enabled on Convex Cloud.

- Linux: cleared environment (deployment variables only), rlimits, `no_new_privs`, Landlock (no access to the
  backend data directory), seccomp-bpf (no `execve`, `fork`, `ptrace`, `mount`, `bpf`, `io_uring`; no sockets
  unless allowed), optional uid/gid drop and cgroup v2 limit. Level is `best-effort` by default (applies what
  the kernel offers) or `strict` (refuse to spawn without Landlock and seccomp).
- macOS (development): no sandbox beyond a cleared environment, rlimits, a memory watchdog and host kill.
  The backend refuses to start native mode there unless `RUST_NATIVE_ALLOW_UNSANDBOXED=true` is set.
- Library constructors (`.init_array`) run at `dlopen`, after Landlock and seccomp are applied (the worker
  applies the sandbox before loading the library; type-enforced). Whether `dlopen` succeeds under the real
  filter is not yet verified on a Linux kernel. On macOS there is no sandbox.
- Statics persist across calls in one worker (unlike wasm); `RUST_NATIVE_MAX_CALLS_PER_WORKER=1` restores
  per-call isolation.

The same exposure class as the existing self-hosted `"use node"` executor, with a stronger sandbox.

## Upstream footprint

Almost everything is new files under `crates/isolate/src/environment/rust/native/`, `rust_analyze/native.rs`,
`local_backend/src/rust_native.rs` and `npm-packages/convex/src/cli/lib/rust/native.ts`.

Edits to files that exist in upstream `get-convex/convex-backend` (the native commit only, patch generated from
fork head `a5d46a5`):

| File | Change |
|---|---|
| `crates/local_backend/src/main.rs` | +3 lines at the top of `main`: hand over to the worker entry when `argv[1] == __rust_native_worker` (must precede arg parsing and tracing) |
| `crates/local_backend/src/router.rs` | +1 line: the info route |
| `crates/local_backend/src/lib.rs` | +1 line: `mod rust_native;` |
| `crates/common/src/knobs.rs` | +104 lines: the `RUST_NATIVE_*` knob block |
| `crates/{isolate,application,local_backend}/Cargo.toml` | +2 / +3 / +1 dependency lines (`libloading` in isolate; `object` and `sha2` in application; `isolate` in local_backend). seccomp and Landlock are raw syscalls, no extra crates |
| `Cargo.lock` | +4 lines |
| `npm-packages/convex/src/cli/{deploy,dev}.ts`, `lib/config.ts`, `schemas/convex.schema.json` | `--rust-runtime` flag, `rust.runtime` setting, backend capability lookup (+22, +22/-1, +7, +13) |

Totals for the native commit: 35 files; 12 touch files that exist upstream (+183/-1 lines, of which Rust source
lines outside `Cargo.*` are 109, 104 of them the knob block); 16 are new files (+13,290 lines). The remaining 7
modify files that the wasm commit introduced (see `SUMMARY.md`, rows marked "ours (modified)").
The wasm commit itself (the stacked base) touches 25 upstream files (+881/-56, of which 691/29 are
`Cargo.lock`) and adds 25 new files (+8,152 lines). Patch sizes: wasm 365,531 bytes (340,994 without
`Cargo.lock`), native 579,546 bytes (578,813 without).

## HTTP actions on the native runtime

`#[http_action]` functions run on the native runtime (and on the wasm runtime in this stack), in a buffered form.
The function manifest marks them `kind:"http"`; the analyzer registers them as *internal* actions (never public,
so a client cannot forge a request head), and the CLI generates `convex/_generated/rustHttp` with
`rustHttpAction("<module>:<fn>")`, a V8 `httpAction` that calls the internal action with the request (head, base64
body, `buffered`) and builds the `Response` from the returned `{status, headers, body}`. Request bodies must stay
under about 12 MiB (argument limit); a streaming response is collected in the guest and sent whole (20 MiB cap).
Streaming HTTP actions need the stock V8 host. This part (analyzer registration and `rustHttp` codegen, about 230
lines including tests) is in the first commit; the native commit adds nothing to it.

## Platform status

- macOS arm64: the full native path runs (build, push, calls, failure modes) with the reduced macOS sandbox.
- Linux x86_64 and aarch64: `isolate` cross-compiles for `x86_64-unknown-linux-gnu` and `clippy` is clean (zig
  cross build; this found and fixed a Linux-only build error in the sandbox module, `a5d46a5`). That is a compile
  check only.
- Known gap: the Linux sandbox (Landlock, seccomp-bpf, rlimits, cgroup, uid drop) has not been exercised on a
  real kernel in this preparation. Its plan construction and the BPF program are unit-tested, but the end-to-end
  isolation checks listed under Testing still need a run on Linux (a Docker container on a Linux host
  or a VM). Please treat Linux sandbox behaviour as unverified until
  that run is reported in this thread.

## Testing

Run on the prepared patch set (fill in the results before filing):

- `cargo test -p isolate rust` (wasm and native unit tests: frames, pump with a fake worker, pool with a fake
  spawner, worker loop, sandbox plan, crash classification).
- `cargo test -p application rust_analyze` (push-time validation with synthetic ELF and Mach-O files).
- `cd npm-packages/convex && npx vitest run src/cli/lib/rust` and
  `npx tsc --noEmit --emitDeclarationOnly false`.
- End-to-end spec suite re-run with the native runtime, failure-mode crate (panic, abort, segfault, stack
  overflow, OOM, timeout, `process::exit`, output floods), isolation checks and lifecycle checks as listed in
  the design, on macOS arm64 (local backend) and Linux (Docker `ubuntu:noble`, arm64 and x86_64).

Verified for this preparation: the wasm patch applies cleanly to the upstream base `342530f`, and the native
patch applies cleanly on top of it (`git apply --check` then `git apply`; the same with the `-nolock` pair). The
wasm tree is exactly the fork head with the native commits reverted and the mixed HTTP-action commit re-applied
(`release/native-commits.txt`, `release/mixed-commits.txt`), and the wasm patch contains no native-only path.
`cargo check -p isolate -p application` passes with no warnings on the wasm-only tree and on the stacked
tree (`release/verify-patches.sh -c`). On current `upstream/main` (`bb31399`) both patches apply as-is and the
stacked tree checks after one added match arm for upstream commit 868554b (`SUBMIT.md` step 3). On the stacked tree (identical to the fork head
`a40937f`): `cargo test -p isolate -p application --lib rust` 128 + 32 passed, CLI vitest 104 passed, `tsc --noEmit`
clean; the end-to-end spec and parity suite passed 47/47 on the fork backend built from it (macOS arm64). Linux
sandbox checks are still open (see Platform status).

## How to try it

```sh
# backend (Linux glibc or macOS), mode is off unless set
RUST_NATIVE_MODE=worker ./convex-local-backend --instance-name x --instance-secret ... ...

# check
curl -H "Authorization: Convex $ADMIN_KEY" http://127.0.0.1:3210/api/rust_native/info

# app, using the CLI from this branch
npx convex dev --rust-runtime native        # or: deploy --rust-runtime native
# or in convex.json: {"rust": {"runtime": "native"}}
```

Cross-building from macOS to a Linux backend uses `cargo-zigbuild` (`cargo install cargo-zigbuild`,
`brew install zig`); the CLI builds for `<triple>.<backend glibc>`. User documentation: `docs/rust/native.md` in
the SDK repository.

## Open questions

1. Separate static-musl executable variant for distroless or older-glibc hosts?
2. Default for network access: through the host (consistent) or real sockets behind a knob?
3. Answer `now` inside the worker to save a round trip, at the cost of clock parity under test runtimes?
4. Prewarm a pool when push analysis accepts a new artifact?
5. Is the RSS watchdog plus `RLIMIT_DATA` enough without a delegated cgroup in Docker?
6. Move the artifact to file storage (like external packages) instead of base64 module source? That touches
   upstream push code, so it is deferred.
7. Pool keyed per environment hash means any env edit recycles workers; acceptable?
8. Preferred support and licence wording for "executes native code with the backend's kernel privileges, minus
   the sandbox".
9. Is a stacked review (wasm runtime first, native second) acceptable, or should the wasm runtime be filed as a
   separate pull request first?

https://claude.ai/code/session_01QtCEDMqQnUwastSLyVXX3S

By submitting this pull request, I confirm that you can use, modify, copy, and redistribute this contribution, under the terms of your choice.

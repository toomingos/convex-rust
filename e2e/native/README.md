# e2e/native

End-to-end suites for the rust-native runtime (`--rust-runtime native`, backend `RUST_NATIVE_MODE=worker`).
Implements the native runtime design (`docs/rust-native-runtime-design.md`). Contract: `contracts/NATIVE.md`; user docs: `docs/rust/native.md`.

```sh
cd e2e/native
npm install                      # once: links the fork CLI (file:)
node run.mjs                     # all suites + spec parity; prints a PASS/FAIL/SKIP table; exit 1 on any FAIL
node run.mjs --only 01           # one suite file (substring of its name); --only parity for the parity run
node run.mjs --skip-parity       # fixtures suites only
node run.mjs --parity-only
node run.mjs --keep              # keep the temp data dir ($E2E_NATIVE_TMP) for debugging (backend.log inside)
```

Needs: `target-backend/debug/convex-local-backend` + `generate_key` (built per `docs/rust/self-hosted-fork.md`
if missing), the fork CLI bundle (`npm-packages/convex/dist/cli.bundle.cjs`, built if missing), toolchain
`nightly-2026-06-28`, Node 22/24 on the machine (backend's node executor; override with `SPEC_NODE22`).

## What the runner does

- Starts its own backend on ports **3360/3361** (mock server 3368; `NATIVE_PORT`, `NATIVE_MOCK_PORT` override),
  one fresh backend per suite file, data dir under `$E2E_NATIVE_TMP` (default `<tmpdir>/convex-rs-e2e-native`),
  deleted at the end. It only kills processes it started (the backend, the mock server, and native workers that
  are children of its backend).
- Default backend env: `RUST_NATIVE_MODE=worker`, `V8_ACTION_USER_TIMEOUT=5`, plus `CONVEX_E2E_SECRET` and
  `INSTANCE_SECRET` set to markers the worker must never see. Suites override knobs per profile.
- `NATIVE_CLI=<path to cli.bundle.cjs>` runs a different CLI build than the fork package.

## Suites (`test/`)

| File | DESIGN 8.2 | Backend profile |
|---|---|---|
| `01-crash` | suite 2: panic, panic-abort stand-in, abort, segfault, stack overflow, OOM (2 GiB), `loop {}` timeout, `exit(3)`, spinning thread (recycle), flood; every case: exact message (NATIVE.md 4.4) + next call on a fresh pid | default |
| `02-isolation` | suite 3: deployment env visible, backend secrets absent, cleared env, Landlock EACCES, socket EPERM, `sh` stopped by sandbox, tmp write, crash does not hit a concurrent call | default |
| `03-network` | suite 3: `RUST_NATIVE_ALLOW_NETWORK=1` allows `TcpStream::connect` | network |
| `04-lifecycle` | suite 4: info endpoint, 32 parallel calls, idle reaping (`IDLE_TIMEOUT_SECS=3`), redeploy v1 -> v2 with a v1 call in flight, `kill -9` leaves no workers, breaker with a corrupt artifact | lifecycle |
| `05-limits` | suite 2: result > 16 MiB, log-line cap, stdout cap | default |
| `06-ctx` | ctx: runQuery, runMutation, fetch, storage, scheduler | default |
| `07-modes` | mode fallback (auto -> wasm + note; native -> clear error; mode turned off after a native push), strict sandbox, push validation | modes-off / modes-later / strict |
| `08-sandbox-levels` | `RUST_NATIVE_SANDBOX=off` | sandbox-off |

Fixture project: `convex/` (TS: schema, `data.ts`) and `convex/rust/` (crate `native_e2e`, modules `crash`,
`isolation`, `lifecycle`, `ctx_calls`). `NATIVE_E2E_VERSION=v2` at push time bakes a version into
`lifecycle:version`, which the redeploy test uses.

## Spec parity (`parity/`)

`e2e/spec` is **not copied or edited**. `node run.mjs` runs `e2e/spec/test/spec.test.ts` with
`--import parity/preload.mjs`, which (a) appends `--rust-runtime native` to every `convex dev` the spec harness
runs and (b) starts the harness's backend with `RUST_NATIVE_MODE=worker` and cwd = its own data dir. Ports come
from `SPEC_PORT`/`SPEC_MOCK_PORT` (set to 3360/3368). Target: 45/47, i.e. at most 2 allowed differences listed in
`parity/allowed.json` with their reason (shown as `ALLOWED-DIFF`). **The two entries are provisional**: DESIGN does
not name them; confirm after the first full native run. Side effects of the unmodified harness (the runner undoes the Cargo.lock/.cargo ones): it recreates
`e2e/spec/.backend-data` (admin key + node symlink only; removed afterwards when it did not exist before).

## Linux-only checks

These mean nothing on macOS and show as **SKIP** with the reason (`linuxOnly()` in `lib/harness.mjs`):
Landlock (data dir EACCES), seccomp (socket EPERM, `sh` SIGSYS), rlimits (`/proc/self/limits`), strict
sandbox success. cgroup (`RUST_NATIVE_CGROUP`, exact OOM) and uid/gid drop are not covered here. On macOS
the `strict` level is expected to fail with "Could not start the native Rust worker".

## Not covered / known gaps

- Docker `linux/arm64` and Linux x86_64 backends: the runner only drives a local backend binary (a Linux run
  needs the Linux backend binary and `cargo-zigbuild` when cross-pushing; not wired).
- Push validation for wrong triple and glibc-too-new needs crafted artifacts; covered by the unit tests in
  `rust_analyze/native.rs`.
- A real `panic = "abort"` build: the CLI forces `panic = "unwind"` for native builds, so the case is a panic
  hook that calls `abort()`.

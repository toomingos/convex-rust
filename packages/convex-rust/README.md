# convex-rust

Run a `"use rust"` crate on stock Convex (the Convex cloud or any self-hosted backend) with no backend or
CLI changes. The crate is compiled to `wasm32-wasip1`; `convex-rust build` generates ordinary Convex
`action` / `internalAction` wrappers that run the wasm inside the default Convex runtime, using WebAssembly
JSPI to await `fetch`, `ctx.runQuery`, `ctx.runMutation`, timers and storage from Rust.

Spec: `contracts/V8HOST.md` (guest ABI: `contracts/ABI.md`, manifest: `contracts/MANIFEST.md`).

## Quickstart

In an existing Convex project (`npx convex dev` has been run once, so `convex/_generated` and a deployment exist),
with [rustup](https://rustup.rs) installed:

```sh
npm install convex-rust
npx convex-rust init                                  # scaffolds convex/rust, prints the next steps
npx convex-rust build                                 # compiles the crate to wasm, writes the TS wrappers
npx convex dev --once                                 # push (or: npx convex deploy)
npx convex run rust/example:greet '{"name":"you"}'    # => { "text": "Hello, you!" }
```

Verified (stable 1.99.1) from a packed tarball on a fresh project against a stock local backend (actions, `ctx.run_query` into a
TS query, and an `#[http_action]` route). Stable Rust is enough (the scaffolded `rust-toolchain.toml` says
`stable`; set `CONVEX_RUST_CHANNEL=nightly-...` for `init` if you want a pinned one). The `wasm32-wasip1` target
comes from that file; if toolchain or target is missing `build` shows the exact `rustup` command and, in a terminal,
asks `Run ... now? (Y/n)` (never silently; with no TTY it just fails with the command).
`npx convex-rust doctor` checks cargo, rustup, toolchain + target, the shipped `convex_fn` crate and `wasm-opt`.
`wasm-opt` (binaryen, `brew install binaryen`) is optional: it makes release wasm about 30% smaller (`-O3`).
Wasm SIMD is on by default (`--no-simd` to opt out). Instance reuse, `CONVEX_RUST_TRACE` and build flags: docs/rust/cloud.md.

The scaffolded crate depends on `convex_fn` by path into `node_modules/convex-rust/rust/convex_fn` (shipped in this
package; nothing is fetched from crates.io for it). Commit `convex/rust/Cargo.lock`, `src/_generated/api.rs` and
the generated `*.ts` wrappers; `_wasm/` is git-ignored, so CI runs `npx convex-rust build` before `npx convex deploy`.

## Build

```sh
npx convex-rust build [--watch] [--out convex/rust] [--profile dev|release] [--crate <dir>] [--env A,B] [--no-simd] [--single] [--modules] [--group name=a,b] [--split] [--wasi-sdk | --no-wasi-sdk]
npx convex-rust dev       # watch loop next to `npx convex dev` (debounced; dev profile)
npx convex-rust doctor    # check cargo / rustup / target / wasm-opt, with fix commands
npx convex-rust codegen   # only refresh src/_generated/api.rs (typed refs to your TS functions)
npx convex-rust symbolize [file] [--module m]   # turn wasm-function[N]:0x.. in pasted Convex logs into Rust names
npx convex-rust wasi-sdk  # install (if missing) and print the wasi-sdk path used for C dependencies
```

C-wrapping crates (zstd-sys, bundled SQLite, ring) build with a wasi-sdk: found in `WASI_SDK_PATH` or
`~/.cache/convex-rust/wasi-sdk-34.0`, downloaded once (about 180 MB) when a C build script is detected
(`--wasi-sdk` forces, `--no-wasi-sdk` disables). The build also warns about wasm imports that fail on the host
(sockets, filesystem, threads). See docs/rust/crates.md and docs/rust/cloud.md.

1. `cargo build --target wasm32-wasip1` of the crate (default `convex/rust`, or `--crate`).
2. Reads the `convex:manifest` custom section of the wasm.
   In the release profile the crate is built once per Convex module with no setup in the crate: after a first
   build learns the modules, `cargo rustc --lib -- --cfg convex_split --cfg 'convex_keep="<fn>"'` rebuilds only
   your crate per module (dependencies are compiled once), the `#[action]` macro drops the other modules'
   exports and the linker removes their code. Each module imports its own, smaller wasm
   (`<out>/_wasm/<crate>.<module>.wasm`). Over 28 MiB in total (Convex bundles at most 32 MiB) the smallest
   modules are merged with a warning. `convex-rust dev` (dev profile) builds one shared wasm for faster
   rebuilds; `--modules` forces per-module there. Crates with two or more `m-<module>` cargo features use
   those instead. `--group name=a,b` overrides the grouping, `--single` builds one shared wasm. See docs/rust/cloud.md.
3. Writes `<out>/_wasm/<crate>.wasm` and one TypeScript module per Rust module
   (`crate::a::b` -> `<out>/a/b.ts`, crate root -> `<out>/lib.ts`), plus `_wasm.d.ts`.
   Files carry a `@generated` header; stale generated files are removed, hand-written files are kept.

`--out` must be inside the Convex functions directory. Then push with the stock CLI:

```sh
npx convex dev --once        # or: npx convex deploy
npx convex run rust/hello:hello
```

Generated wrapper (args/returns validators come from the Rust types; omitted args default to `v.any()`):

```ts
import { action } from "../_generated/server";
import { runRust } from "convex-rust/runtime";
import wasm from "./_wasm/hello_rust.wasm";
export const hello = action({
  args: {},
  returns: v.string(),
  handler: (ctx, args) => runRust(ctx, wasm, "convex_run:hello_rust::hello:hello", args),
});
```

`#[action(internal)]` functions become `internalAction`.

## Runtime

`convex-rust/runtime` exports `runRust(ctx, module, exportName, args)` and `runRustHttp(ctx, module, exportName, request)`
(the handler body of the `httpAction` generated for `#[http_action]` functions: streamed request and response bodies, 20 MiB
limits; V8 host only, see `docs/rust/cloud.md#http-actions`). It has no Node built-ins and no
dependencies besides `convex/values`; it needs `WebAssembly.Suspending` / `WebAssembly.promising` (JSPI),
which the Convex default runtime provides. The guest's own linear memory is used as is (no extra cap).

Supported guest operations: `fetch` (streamed response and request bodies, manual redirects handled by the SDK;
dropping the Rust future aborts the request; large bodies pass as `{ptr,len}` references instead of base64), `sleep`,
Convex syscalls (`runQuery`, `runMutation`, `runAction` and `schedule` by path, function handle or component
reference; `getUserIdentity`; storage incl. `generateUploadUrl`; `vectorSearch`), `componentArgument`, `now`,
logging (levels map to `console.debug|log|warn|error`) and WASI `fd_write`, `environ_*`,
`clock_time_get`, `random_get`. Any other syscall name is forwarded to the runtime's `Convex.asyncSyscall`
(it fails with `Unsupported syscall in the V8 host: <name>` if that is missing); other WASI calls return `ENOSYS`.

Errors: Rust errors throw `Error(message)`; `Error::convex(data)` throws `ConvexError(data)`; a panic
surfaces as `Uncaught Error: <panic message> at file:line`.

## Typed `api.rs`, reqwest, env vars

- `build` / `codegen` generate `src/_generated/api.rs` (typed `FunctionRef`s for `ctx.run_query` & co.) by
  bundling your `convex/*.ts` with esbuild and reading the registered functions' validators in a Node
  subprocess. No backend needed; `convex/_generated` must exist.
- `.cargo/config.toml` holds a managed `[patch.crates-io]` block that swaps `reqwest` for the host-`fetch`
  shim when a dependency uses it. The shim crates ship in `rust/` (copied from the monorepo's `crates/` by
  `npm run vendor-crates`, run on `prepack`); `CONVEX_REQWEST_PATH` / `CONVEX_FN_PATH` override.
- `[build] target-dir` points outside `convex/` so the stock CLI does not walk cargo output.
- The default runtime cannot enumerate `process.env`: list the variables Rust may read with
  `build --env A,B` or the deployment variable `CONVEX_RUST_ENV=A,B`.

- Crypto (signatures, AES-GCM, SHA-1/SHA-2, HMAC) is provided by the host and required: there is no pure-Rust fallback in wasm32. Keep this package up to date; a host without an operation fails with `host lacks crypto/<op> <alg>`.

- `/tmp` is a writable in-memory directory (per call, discarded after it; file data lives in the guest's linear memory; 64 MiB cap by default, `CONVEX_RUST_TMP_BYTES` or `runRust(..., { tmpBytes })`, `0` removes it). Use `convex_fn::fs::temp_dir()`: `std::env::temp_dir()` panics on `wasm32-wasip1`.
- Time zones and locale formatting come from the runtime's `Intl` via `convex_fn::intl` (`tz_offset_secs`, `local_to_epoch_ms`, `format_date`, `format_number`); an unknown zone is an error, never UTC. `jiff` / `chrono-tz` with a bundled tzdb also work, at a size cost.
- Fan-out: `ctx.run_action_each(target, args, limit)` / `convex_fn::par_map` run many actions in parallel with a concurrency limit (each is a separate billed action execution).

See `docs/rust/cloud.md` for the full guide.

## Limitations

- Convex default-runtime limits apply (30 minute actions, 64 MiB RAM documented, code size per the [limits page](https://docs.convex.dev/production/state/limits), actions only, no `"use node"`).

## Development

```sh
npm run build   # tsc -> dist
npm test        # vitest: host import semantics (hand-written wasm guest) + generator output
```

`test/fixtures/gen-guest.mjs` regenerates the test guest (needs `wasm-opt`).

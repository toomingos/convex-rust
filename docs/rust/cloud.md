# "use rust" on the Convex cloud (and stock self-hosted)

The `convex-rust` npm package runs the same Rust crate on a **stock** Convex deployment (the Convex cloud, or a
self-hosted backend without this repo's fork). No backend or CLI changes: your crate is compiled to
`wasm32-wasip1`, and `convex-rust build` writes ordinary Convex `action` / `internalAction` wrappers plus the
`.wasm`. The stock `npx convex dev` / `npx convex deploy` bundle and push them like any other TypeScript.
Inside the action, a small JS host (`convex-rust/runtime`, using WebAssembly JSPI) gives the wasm `fetch`,
`ctx.run_query`/`run_mutation`/`run_action`, scheduling, storage, timers and logging.

Package reference: [packages/convex-rust/README.md](../../packages/convex-rust/README.md). Host contract:
`contracts/V8HOST.md`. The Rust API (`#[action]`, `Ctx`) is the one described in the other pages here.

## Getting started

Prerequisites: a Convex project (`npx convex dev` has been run once, so `convex/_generated` exists), `rustup`
(stable Rust is enough; the `wasm32-wasip1` target comes from the `rust-toolchain.toml` that init writes, and `build`
offers to install a missing toolchain or target with the exact `rustup` command), and optionally `wasm-opt`
(binaryen) for smaller modules. `npx convex-rust doctor` checks all of it.

```sh
npm install convex-rust
npx convex-rust init        # scaffolds convex/rust (crate, rust-toolchain.toml, .cargo/config.toml, .gitignore)
npx convex-rust build       # cargo build + typed api.rs + TS wrappers + .wasm
npx convex dev --once       # or: npx convex deploy
npx convex run rust/example:greet '{"name":"you"}'
```

Day to day, run the watch loop next to `npx convex dev`:

```sh
npx convex-rust dev         # terminal 1
npx convex dev              # terminal 2
```

`convex-rust dev` rebuilds (dev profile) and regenerates on `.rs`/`Cargo.*` changes, debounced, and re-runs the
`api.rs` codegen when your TypeScript functions change. Its own outputs (`target`, `_wasm`, `src/_generated`,
`Cargo.lock`, the managed config block) do not retrigger it. A change to the wasm also touches the generated
`.ts` (it embeds a content hash), because `convex dev` does not watch `.wasm` files by itself.

### Layout

```
convex/rust/
  Cargo.toml  rust-toolchain.toml  .gitignore
  .cargo/config.toml          # [build] target-dir outside convex/, managed reqwest [patch]
  src/lib.rs  src/example.rs  # your Rust (ignored by the Convex bundler)
  src/_generated/api.rs       # typed refs to your TS functions (generated)
  example.ts                  # generated wrapper -> rust/example:greet   (generated, small, may be committed)
  _wasm/convex_rust.wasm      # generated, git-ignored
  _wasm.d.ts                  # generated
```

The wrappers land at `rust/<module>:<name>` (`src/example.rs` -> `api.rust.example.greet`; the crate root maps to
`rust/lib`). The stock CLI only bundles `.js`/`.ts` files; `Cargo.toml`, `src/*.rs`, `.cargo` and the `.wasm`
are not entry points. Cargo's `target/` would hold thousands of files that the CLI walks, so init sets
`[build] target-dir = "../../.convex-rust/target"` in `.cargo/config.toml` (outside `convex/`) and adds
`.convex-rust/` to your `.gitignore`. `build` warns if the target directory is inside `convex/`. Setting
`CARGO_TARGET_DIR` also works.

### Calling your TypeScript from Rust

`src/_generated/api.rs` is generated from your TS functions without a backend: `convex-rust build` bundles
`convex/*.ts` with esbuild, imports them in a short Node process and reads each function's visibility and
validators (the same data the server derives). Include it once in `lib.rs` (init does):

```rust
#[path = "_generated/api.rs"]
pub mod api;
// ctx.run_query(api::messages::list, api::messages::ListArgs { limit: Some(10.0) }).await?
```

Needs `convex/_generated` to exist. Run `npx convex-rust codegen` to refresh only this file (no cargo), for
example after `npx convex codegen`. Rust functions are included from the previous build, so one Rust action can
call another.

### Crates that use `reqwest`

`cargo add reqwest` works: the managed block of `.cargo/config.toml` (between the `convex-managed:patch`
markers) patches `reqwest` with the built-in shim that sends requests through the host `fetch`. It is filled in
by the next `convex-rust build` when a dependency uses reqwest; edit outside the markers only. The shim crates
ship in the npm package (`node_modules/convex-rust/rust/*`, copied from `crates/` at pack time). Override with
`CONVEX_REQWEST_PATH` / `CONVEX_FN_PATH` (also for `init`).

### Environment variables (difference from self-hosted)

In the default Convex runtime `process.env` can be read by name but not listed, so the guest cannot see
"all" variables. Name the ones the Rust code reads (`std::env::var`):

```sh
npx convex-rust build --env OPENAI_API_KEY,STRIPE_KEY   # baked into the generated wrappers
npx convex env set CONVEX_RUST_ENV EXTRA_A,EXTRA_B      # or: add names at run time, no rebuild
```

Values are read at call time from the deployment's environment variables, so `npx convex env set` takes effect
without a rebuild. A variable that is not named is not visible to Rust.

### Instance reuse and statics

The compiled `WebAssembly.Module` comes from `import wasm from "./x.wasm"`, evaluated once per isolate
module load. `runRust` keeps the **instance** too and hands it to the next call in the same isolate after a
call that finished cleanly (it returned a value or an error result, with no task or stream left running).

- **Rust statics persist between calls in the same isolate**, exactly like JavaScript module globals do in
  Convex (`static`, `thread_local!`, `OnceLock`, allocator state). Treat them as a cache that may or may not be
  there: a new isolate, a trap, a panic, an out-of-memory, a changed environment variable (WASI `environ` is cached by libc
  at instantiation) or a memory size above 128 MiB (wasm linear memory never shrinks) all start from a
  fresh instance. Never keep per-request or per-user data in a static.
- If a call is still using the instance (JSPI suspended, e.g. awaiting `fetch`), an overlapping call builds its
  own instance. Convex runs one action per isolate at a time, so this is a guard.
- Whether an isolate is reused between calls is up to the platform. **The Convex cloud does not keep module
  state between calls** (measured 2026-10-24 on a dev deployment, 100 sequential calls: a module-scope counter
  is 1 on every call and every trace line says `reused=false`), so there every call imports the module again,
  compiles the wasm and instantiates it, exactly like the local backend. Instance reuse only helps where the
  platform keeps module state.

Per-call phase timings (encode, imports, instantiate, `_initialize`, the `convex_run:*` call, result decode,
total) are logged when the deployment variable `CONVEX_RUST_TRACE=1` is set (off by default):

```sh
npx convex env set CONVEX_RUST_TRACE 1     # then: npx convex logs
# [convex-rust] convex_run:app::noop:noop reused=true ok=true encodeArgs=0.01 prepare=0.01 call=0.20 decode=0.02 total=0.31
```

`imports`/`instantiate`/`init` appear only when the instance was created for that call. The module compile
happens in `import wasm` before `runRust` runs and is not part of these numbers.

### Fixed cost per call on the cloud (and what to do about it)

Because the cloud re-evaluates the module on every call, a Rust action pays for the **size of the wasm it
imports** on every call, whether or not it uses all of it. Server `executionTime` p50 of a no-op action on a
dev deployment (100 sequential calls each; client round trip is ~115-135 ms from the test machine, so use
`executionTime`):

| action | exec p50 |
|---|---|
| plain TypeScript no-op | 3.1 ms |
| Rust no-op, 190 KB wasm | 4.1 ms (+1.0) |
| TypeScript that only imports a 10 MB wasm (never uses it) | 11.4 ms (+8.3) |
| same 10 MB wasm, through `runRust` (no-op) | 14.9 ms (+11.8: 8.3 load + 2.2 instantiate + 0.4 host) |
| 6 MB wasm (the `base` module of the bench crate after the old `--split`) through `runRust` | 12.2 ms |
| TypeScript with `await import("./x.wasm")` on a branch not taken | 3.2 ms (no cost) |

So the cost is about 0.8-0.9 ms per MB of wasm loaded per call (a synthetic 10 MB data-only wasm was worse,
19.5 ms), plus an instantiate that grows with the data segments (0.2 ms for 190 KB, ~2 ms for 6-10 MB); the
host's own work (encode, imports, call, decode) is about 0.3-0.5 ms. This is the platform's per-call module
load, not something the host can cache: a module-scope compile cache or `WebAssembly.compile` has nothing
to hit, since module scope is fresh every call. It is why a no-op in the large cloud bench crate (10 MB
wasm) a few ms slower in server `executionTime` than a TypeScript no-op (final run: no-op p50 5.3 ms rust-v8
vs 3.9 ms ts-v8 vs 27 ms "use node"), while compute-heavy cases are faster in Rust.

### Measured performance (cloud)

Final benchmark on a dev deployment (`bench/reports/final-2026-10-25/report.html`), end-to-end client time,
median of per-case ratios (below 1 = numerator faster). R = rust-v8 (wasm), T = ts-v8 (default Convex runtime),
J = "use node".

| | R/J | T/J | R/T |
|---|---|---|---|
| cold p50 (2 rounds, indicative) | 0.34 (86 cases) | 0.55 (57) | 0.98 (58) |
| warm p50 | 0.55 (62) | 0.69 (51) | 0.99 (51) |
| concurrency 10, inverse throughput | 0.35 (61) | 0.45 (51) | 1.01 (51) |
| warm server `executionTime` p50 | 0.18 (57) | 0.21 (46) | 1.04 (46) |

Client time includes about 110 ms of network to the region, which compresses client ratios; server execution
time is the cleaner view. Rust and plain TypeScript in the default runtime are on par; both beat "use node",
mostly because Node actions carry a heavy per-call overhead. Caveats: the deployment was disabled mid-run for
exceeding free-plan limits, so warm and concurrency numbers cover only the cases before `data-014` (no
`data-014/015`, `crypto`, `media`, `mem`; cold covers all); `net` was not run on the cloud; 37 cases cannot run
as TypeScript in the default runtime; cold is the median of 2 rounds.

What helps, cheapest first:

- Keep the wasm small: fewer/lighter dependencies, `opt-level = "s"`, `lto = true`, `panic = "abort"`. `build`
  prints the per-call load estimate when the wasm is over 1 MB. (`-Oz` instead of `-O3` changed a 6 MB module by
  1.5%, so it is not worth the slower code.)
- **One wasm per module (the default, no setup in the crate).** A plain crate of `#[action]` functions spread over
  several Rust modules gets one wasm per Convex module from `convex-rust build` (release profile). It is built
  once to learn the modules, then once per module with
  `cargo rustc --lib --release --target wasm32-wasip1 -- --cfg convex_split --cfg 'convex_keep="<fn>"' ...`
  (plus `--check-cfg`, so no `unexpected_cfgs` warnings). The `#[action]` macro only emits a function's
  `convex_run:` export and `convex:manifest` entry when `convex_split` is unset or the function is kept; exports
  are the linker's roots, so wasm-ld drops the code only the other modules use. Only your crate is recompiled per
  module (dependencies are compiled once, no `RUSTFLAGS`), and each generated wrapper imports only its own
  module's wasm, so it works on Convex cloud as is. Functions with the same name in two modules are kept in both
  builds; the manifest is filtered to the module. If the total exceeds 28 MiB (Convex's limit is 32 MiB of
  bundled code), the smallest modules are merged with a warning. Dependencies are still unified across the
  crate (features of every module's dependencies are on), so a module's wasm can be a little larger than with
  hand-written cargo features.
  Measured on the bench crate built as a plain crate: base 240 KB, cb 305, cpu 1.4 MB, crypto 581, data 4.1,
  media 4.1, mem 253, net 328, ser 219 KB (11.5 MB in total), versus 8.5 MB for the shared wasm.
  The release build takes about 3 min cold and 2 min after an edit (one crate compile and wasm-opt per module).
  `convex-rust dev` (dev profile) keeps building one shared wasm: the per-module build made a dev rebuild 45 s
  instead of 5.5 s. `--modules` (or `CONVEX_RUST_MODULES=1`) forces per-module there.
- **`m-<module>` cargo features (manual override).** If the crate defines two or more features named `m-<module>`
  (`m-a-b` is `a/b`), `convex-rust build` runs `cargo build --no-default-features --features m-<module>` once per
  module instead (dependencies behind `optional = true` are then excluded entirely, which is the smallest
  result). `--group light=a,b --group heavy=cpu` (or `CONVEX_RUST_GROUPS="light=a,b;heavy=cpu"`) merges modules
  into fewer wasm. `--single` (or `CONVEX_RUST_MODULES=0`) builds one shared wasm.
- `--split` (or `CONVEX_RUST_SPLIT=1`, only with `--single`) is a legacy wasm-opt cut of the shared wasm; the
  function table keeps most code alive, so the smallest result for the bench crate is still 6.2 MB.
- Do not use a static `import` of a wasm in your own TypeScript modules unless the function needs it; use
  `await import("./x.wasm")` there (lazy, not compiled until the branch runs).

### Build flags: optimizer and SIMD

- `wasm-opt` runs with `-O3` and without `-g` (the name section is dropped, panic messages keep their
  `file:line`, which comes from the `Location` data). Set `CONVEX_RUST_WASM_OPT_FLAGS="-Oz -g"` for the old
  small/named output. A 25 KB-class crate is within 1 KB of its `-Oz` size either way.
- `convex-rust build` compiles with `-C target-feature=+simd128` (WebAssembly SIMD, so crates such as `memchr`,
  `base64`, hashing and JSON parsers can vectorize). Opt out with `--no-simd` (or `CONVEX_RUST_SIMD=0`). It is
  added as `--config target.wasm32-wasip1.rustflags` (a `build.rustflags` in `.cargo/config.toml` is then
  shadowed; an existing `RUSTFLAGS` is appended to instead).
  SIMD works on the local stock-path backend's default runtime (V8 validates and compiles `v128` code). It has
  **not** been verified on the Convex cloud: if the cloud rejects the module (`CompileError` mentioning
  `simd`/`v128` when the deployment loads), rebuild with `--no-simd`.

### Build-time hints and symbolizing traps

After each wasm is built, `convex-rust build` scans its imports and memories and prints a `warning:` with a one-line
`fix:` for things that fail at runtime on the host: `sock_*` (raw sockets), `path_open`/`fd_readdir`/`fd_prestat_*`
(no filesystem), `proc_raise`, threads or shared memory, 64-bit memory, `convex.*` imports the installed runtime lacks,
and imports from other modules (wasm-bindgen/emscripten style). What counts as implemented is read from the runtime's
WASI table, so it follows the runtime. C crates: see [crates.md](crates.md#c-dependencies-and-the-wasi-sdk).

The deployed wasm has no names, so a trap in the logs reads `wasm-function[123]:0x4a5b`. The build keeps the function
names (demangled) in `<target-dir>/convex-rust-symbols/<wasm>.json`, outside the bundle. Paste the log:

```sh
pbpaste | npx convex-rust symbolize [--module cpu] [--symbols <dir>] [file]
```

Each frame becomes `core::fmt::write (wasm-function[123]:0x4a5b)`. With one wasm no flag is needed; with per-module
wasms pass `--module`, or have exactly one `rust/<module>` in the pasted text. Rebuild before symbolizing logs from a
different build: indexes change with every build. The legacy `--split` path writes no maps.

### Crypto is provided by the host

Signing, verification, public-key derivation, AES-GCM, SHA-1/SHA-2 digests and HMAC run on the host, not in the wasm
(`convex_fn::crypto`, crypto/caps version 2): RS/PS 256-512, ES256/384, EdDSA, A128GCM/A256GCM and SHA-1/SHA-2. There is
no pure-Rust fallback on `wasm32`; a host that lacks an operation fails the call with
`host lacks crypto/<op> <alg>: update convex-rust to >= ...`. Keep `convex-rust` up to date for the V8 host. SHA-2
digests and HMACs over inputs of up to 2 KiB are computed inside the guest, larger inputs and SHA-1 go to the host.
See [ABI.md](../../contracts/ABI.md).

### Temporary files: in-memory `/tmp`

The guest has a writable, preopened `/tmp`, so `std::fs` and crates that write temporary files work. It is
empty when a call starts and discarded when it ends (also when the instance is reused), and nothing else on the
file system exists.

```rust
let dir = convex_fn::fs::temp_dir();           // "/tmp"
std::fs::write(dir.join("in.bin"), &bytes)?;
```

- **Use `convex_fn::fs::temp_dir()`, not `std::env::temp_dir()`**: the latter panics on `wasm32-wasip1`
  ("not supported by WASI yet"), whatever `TMPDIR` says. For crates that default to it, use the `*_in` forms
  (`tempfile::Builder::new().tempfile_in(convex_fn::fs::temp_dir())`).
- Where the bytes live: in the **guest's linear memory**, allocated through the module's own `convex_alloc` in
  4 KiB..1 MiB segments and returned when the call ends. They do not count toward the JS heap of the isolate
  (64 MiB per the limits page), but they do count toward the wasm memory, which the host does not cap
  (see [Limits](#limits)), and the instance is not reused if its memory ends above 128 MiB.
- Cap: 64 MiB of allocated file data by default; writes beyond it fail with `ENOSPC`. Raise or lower it with the
  deployment variable `CONVEX_RUST_TMP_BYTES` (bytes; `0` removes `/tmp`) or `runRust(..., { tmpBytes })`.
  Small files cost at least a 4 KiB segment each.
- Not supported: symlinks, hard links, permissions. Directory listings are in creation order.
- Scope: V8 host only. The wasmtime host (fork) has no preopens. Details: [ABI.md](../../contracts/ABI.md).
- Verified on a Convex cloud dev deployment (2026-10-24: write and read back 300 KB through `std::fs`). The platform's own 64 MiB
  isolate accounting for wasm memory remains the unknown for large files, as for any large allocation.

### Time zones and locale formatting (Intl)

`wasm32` has no time zone database: `chrono::Local`, `std` and friends know only UTC. The host serves the
runtime's `Intl` through sync ops, wrapped in `convex_fn::intl`:

```rust
use convex_fn::{intl, json};
let off = intl::tz_offset_secs("Europe/Lisbon", ctx.now_ms())?;                 // seconds east of UTC, DST included
let utc = intl::local_to_epoch_ms("America/New_York", wall_clock_ms_as_if_utc)?; // wall clock -> instant
let s = intl::format_date(ms, "en-GB", json!({ "timeZone": "Asia/Tokyo", "dateStyle": "full" }))?;
let n = intl::format_number(1234567.891, "de-DE", json!({ "style": "currency", "currency": "EUR" }))?;
```

An unknown zone or locale, or a host that lacks the op (older `convex-rust`, or the wasmtime host), is an error
(`host lacks intl/tzOffset: update convex-rust ...`); the helpers never fall back to UTC. Offsets are exact
for the instant asked; for a repeating or future schedule ask again per instant (a zone's rules can change).
`Intl` data is the runtime's ICU, which may differ in version from your local machine.

Alternative with no host dependency: `jiff` (with `jiff-tzdb` bundled, `features = ["tzdb-bundle-always"]`) or
`chrono-tz` run in the guest. They add the whole tz database to the wasm, roughly 0.4-0.5 MB, which the cloud
loads on every call (about 0.4 ms at the ~0.9 ms/MB measured above); `jiff` can also read `/usr/share/zoneinfo`,
which does not exist here. Pick the host ops for small modules and the bundled tzdb when you need
`Zoned` arithmetic in Rust.

### Fan-out: many Rust actions in parallel

```rust
// 200 chunks, at most 16 actions at a time; results in input order, first error (in input order) fails.
let sums: Vec<f64> = ctx.run_action_each(api::work::sum_chunk, chunks, 16).await?;
// or: every outcome, none aborts the rest
let all: Vec<Result<f64>> = ctx.run_action_each_settled(api::work::sum_chunk, chunks, 16).await;
// any async work with a concurrency limit:
let out = convex_fn::par_map(ids, 8, |id| async move { ctx.run_query(api::x::get, G { id }).await }).await;
```

`par_map` keeps at most `limit` futures in flight, starting the next as one finishes; outputs are in input order.
`run_action_each*` use it with `ctx.run_action` (so the targets can be internal Rust actions in other modules:
one wasm each, see above).

What it costs and where it stops (limits page, https://docs.convex.dev/production/state/limits, checked 2026-10-24):

- Every element is a separate action execution: a function call (counted for billing), its own isolate with
  its own **64 MiB** and **30 minutes**, and its own per-call wasm load (about 0.9 ms per MB of wasm, see above).
- Billing is action compute (GB-hours of the Convex runtime) for each child **and** for the caller, which is
  billed while it waits. A fan-out of N children of t seconds costs N*t plus the caller's wall time.
- Concurrency: the deployment runs 64 (S16) to 2048 (D2048) Convex-runtime actions at once, shared with
  everything else; the caller occupies one slot. Excess calls queue, and the caller's wall time grows. Keep
  `limit` well under your plan's number, and note the caller's own 30 minute limit covers the whole fan-out.
- An action may do up to 1000 concurrent operations (queries, mutations, fetches, `runAction`).
- Data: arguments and return values are limited to **16 MiB** each. Small inputs go in the arguments; for
  large ones store once (`ctx.storage().store`) and pass the storage id, each child then `storage().get`s its
  slice (or the whole blob). Return small summaries, or storage ids, not payloads.
- Convex advises `ctx.runAction` from an action only to cross runtimes (it adds a hop); here it is the
  only way to get parallelism across isolates, since a wasm instance is single-threaded.
- Failures: a child that fails or times out returns an error to its slot; there is no automatic retry. Make the
  children idempotent if you re-run the fan-out.

### CI

```sh
npm ci
npx convex-rust build --env ...    # release profile by default, runs wasm-opt when installed
npx convex deploy
```

The generated `_wasm/` is git-ignored, so CI must run the build before `convex deploy`.

## HTTP actions

```rust
use convex_fn::{http_action, Ctx, HttpRequest, HttpResponse, Result};

#[http_action]
pub async fn chat(ctx: &Ctx, mut req: HttpRequest) -> Result<HttpResponse> {
    let prompt = req.text().await?;                      // or .json::<T>(), .bytes(), .chunk() (streaming)
    let ctx = ctx.clone();
    Ok(HttpResponse::ok()
        .header("content-type", "text/event-stream")
        .streaming(move |w| async move {                 // relay tokens as they arrive
            let mut up = ctx.fetch_stream(/* LLM request built from `prompt` */).await?;
            while let Some(chunk) = up.chunk().await? {
                w.write(chunk).await?;
            }
            Ok(())
        }))
}
```

`build` generates `export const chat = httpAction(...)` in `convex/rust/<module>.ts`; route it in `convex/http.ts`:

```ts
import { httpRouter } from "convex/server";
import * as rust from "./rust/chat";
const http = httpRouter();
http.route({ path: "/chat", method: "POST", handler: rust.chat });
export default http;
```

- `HttpRequest` has `method`, `url`, `headers`, `header(name)`, `path()`, `query()` and the body as `chunk()` / `bytes()` / `text()` / `json()`. The body is delivered as a stream, so a large upload is not buffered unless you ask for `bytes()`.
- `HttpResponse::new(status)` / `ok()` / `text()` / `json()` with `.header()` and `.body()`, or `.streaming(|w| async { w.write(..) })` to send the body while the guest runs (the `Response` is returned as soon as the stream starts). The same `Ctx` as an action is available.
- Limits: 20 MiB for the request and for the response (Convex's HTTP action limit). A buffered response also has to fit the ~16 MiB result limit, so stream big ones. If the handler fails after streaming started the client sees a truncated body; before that, a 500.
- Streaming HTTP actions need this V8 host (cloud / stock self-hosted). The fork's native and wasmtime runtimes run them buffered through `rustHttpAction` ([native.md](native.md#http-actions)).
- Verified on a Convex cloud dev deployment (2026-10-24): a 5-tick streamed response arrives tick by tick (300 ms apart), a 1 MiB streamed download, a 100 KB request body. Still unverified: backpressure and client-disconnect behaviour of streamed responses.

## Limits

These are the limits of Convex's **default runtime** on the cloud, not the fork's:

| Limit | Value |
|---|---|
| Bundle size | The limits page currently lists **230 MB of code per deployment** on all plans (https://docs.convex.dev/production/state/limits, read 2026-10-24); the bundling page and older copies of the docs say 32 MiB, so check the live page. `convex-rust build` still budgets the total wasm at 28 MiB (to stay under the old 32 MiB) and merges modules beyond it; that is conservative and not changed here. The size limit is not what costs you: the `.wasm` is embedded in the bundle (base64) next to your TS and the **cloud loads it on every call** (about 0.9 ms per MB, see [Fixed cost per call](#fixed-cost-per-call-on-the-cloud-and-what-to-do-about-it)), so keep the module small: `build` reports the size, release + `wasm-opt` helps, and per-module wasm keeps each call's load small. |
| Function kinds | Actions (`#[action]`) and HTTP actions (`#[http_action]`, see [HTTP actions](#http-actions)). Queries, mutations and the schema stay in TypeScript. |
| Runtime | The default runtime only. `"use node"` files have no JSPI, so the generated wrappers must not be in a `"use node"` module and Rust cannot be called from Node's runtime directly (call it with `ctx.runAction`). |
| Time | **30 minutes** for an action in the default (Convex) runtime (Node runtime: 10 minutes), per https://docs.convex.dev/production/state/limits ("Convex runtime action execution time"). Time awaiting `fetch`, `run_*` and `sleep` counts. Not measured on the cloud here. |
| Memory | Convex documents 64 MiB RAM for the Convex runtime (limits page). The wasm linear memory is **not currently capped by the 64 MB JS heap** (on 2026-10-24 a default-runtime action on a Convex cloud deployment grew and touched 1000 MiB of wasm memory; this is observed behaviour, not a documented limit, and Convex may change it). Do not rely on it; keep working sets modest. The host adds no cap of its own. |
| Concurrency | One call at a time per wasm instance (see [Instance reuse](#instance-reuse-and-statics)); an overlapping call gets its own instance. |
| Network I/O | `fetch` is cancellable (dropping the future, a `reqwest` timeout, `race`/`select` abort the request), request bodies can stream (`Ctx::fetch_upload`, `reqwest::Body::wrap_stream`, with backpressure), and bodies of 8 KiB or more go through guest memory instead of base64 (up to 64 MiB in one piece; bigger ones stream). Needs `convex-rust` with the `net/*` ops; an older host keeps inline bodies and no cancellation. Exercised on a Convex cloud dev deployment (2026-10-24): plain GET, 200 KB body, streamed request body (also via `reqwest::Body::wrap_stream`), response streaming, a 1 s timeout on a 10 s endpoint returning in about 1.1 s, DoH. Convex logs a "1 unawaited operation: [fetch]" warning when a request was cancelled by dropping its future. Raw TCP/UDP does not exist; use `convex_fn::dns::resolve` for DNS lookups (DNS over HTTPS). |
| Not supported | threads, sockets, a file system other than the in-memory [`/tmp`](#temporary-files-in-memory-tmp), anything needing the network except through the host `fetch`. |

Everything else about writing Rust actions ([writing-actions.md](writing-actions.md), [ctx-api.md](ctx-api.md),
[crates.md](crates.md)) applies unchanged.

## Differences from the self-hosted fork

| | Convex cloud / stock (`convex-rust`) | Fork (`npx convex rust`) |
|---|---|---|
| CLI | stock `convex` + `npx convex-rust` | forked `convex` CLI |
| Where wasm runs | JS host inside a normal action (JSPI) | wasmtime in the backend |
| Env vars | named (`--env`, `CONVEX_RUST_ENV`) | all deployment env vars |
| Limits | default runtime: bundle size per the limits page, no memory knob | backend knobs ([limits.md](limits.md)) |
| `api.rs` | `convex-rust build/codegen` (local analysis) | `npx convex dev` |
| Native worker mode | not available | opt-in ([native.md](native.md)) |
| Rust actions in `api.d.ts` | normal modules, typed by the generated wrappers | merged by the forked codegen |

Status: verified against stock `convex` 1.48 and the local backend (push, call, `run_query`/`run_mutation`
round trip, reqwest through host fetch), and exercised on a Convex cloud dev deployment for the measurements
quoted above (per-call load cost, no module state kept between calls, wasm memory growth to 1000 MiB). Also verified on the cloud (2026-10-24 smoke): function handles and names for `run_*` and the scheduler, vector search, storage
(upload URL, store/get/delete), pass-through syscalls, `/tmp`, `intl/*`, `par_map`, DoH, panics and `ConvexError` (with data and null data)
reaching the client, HTTP actions with streamed responses. Not yet verified on the cloud: the 30 minute action limit.

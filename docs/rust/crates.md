# Using crates

```sh
cd convex/rust
cargo add image          # or serde, regex, chrono, sha2, ...
```

Pure-Rust crates just work. `npx convex dev` builds with `--target wasm32-wasip1`, so `cargo add` and plain
`cargo test` / `cargo check` run on your host target; use `npx convex dev` to build the wasm. To have
rust-analyzer check for wasm, set `rust-analyzer.cargo.target` to `wasm32-wasip1` in your editor.
Commit `Cargo.lock`.

Checked in this repo: `image` (default features: png, jpeg, gif, webp, avif, ... builds and resizes on the
backend, ~4.2 MB release wasm), `serde`/`serde_json`, `rand`, `jsonwebtoken` (with the `rust_crypto`
backend), `oauth2`.

Cryptography: the `convex_fn::crypto` module (signatures, AES-GCM, SHA-1/SHA-2, HMAC) calls the host, which is much
faster than bignum code on `wasm32` and is **required**: there is no pure-Rust fallback, and a host that lacks an operation
fails with `host lacks crypto/<op> <alg>: update convex-rust to >= ...` (details in [cloud.md](cloud.md)). RustCrypto
crates used directly in your own code still run in the guest.

This page describes the default wasm runtime. In the experimental [native mode](native.md) C libraries and
threads can work, because the code is built for the host rather than wasm; the `reqwest` shim and `convex_aws`
still apply to wasm builds.

## HTTP: `reqwest` is built in

Crates that call `reqwest` (API clients, SDKs, your own `reqwest::Client`) work without any setup. The CLI
ships a drop-in `reqwest` replacement inside the `convex` npm package and keeps this block up to date in
`convex/rust/.cargo/config.toml`:

```toml
# >>> convex-managed:patch (begin)
[patch.crates-io]
reqwest = { path = ".../node_modules/convex/rust/convex_reqwest" }
# <<< convex-managed:patch (end)
```

- **Do not edit the block.** `npx convex dev` and `deploy` rewrite it on every run, so paths stay right after
  a CLI upgrade or on another machine/CI. Anything outside the markers is yours.
- Requests go through the host's `fetch` (`ctx.fetch`): same egress rules and limits as TS actions, no TLS
  stack in your wasm. All reqwest feature names (`rustls-tls`, `native-tls`, `json`, `stream`, `gzip`, ...)
  are accepted and ignored, so `default-features = false, features = ["rustls-tls"]` in a dependency resolves.
- Works: `Client`, `ClientBuilder` (timeouts, default headers), `RequestBuilder` (`json`, `form`, `query`,
  `bearer_auth`, `basic_auth`, `multipart`), `Response` (`status`, `headers`, `text`, `json`, `bytes`,
  `bytes_stream`, `error_for_status`), `reqwest::blocking` (see the blocking caveat in
  [ctx-api.md](ctx-api.md#tokio-rt-feature)), `reqwest-middleware`.
- Only the **reqwest 0.12** line is patched automatically today. A crate that depends on reqwest 0.11 or
  0.13 gets the real reqwest, which does not build for wasm (raw sockets).
- `cargo` may print `warning: patch reqwest ... was not used in the crate graph` when nothing uses reqwest.
  Harmless; the CLI filters it from its own builds.

Compatibility, probed against mock servers (`crates/convex_reqwest/COMPAT.md` has the full matrix):

| Works | Does not build (reason) |
|---|---|
| `oauth2`, `reqwest-middleware`, `jsonwebtoken` + JWKS fetch, `anthropic-sdk`, `slack-morphism`, plain `reqwest` | `async-openai` default features (hyper/socket2), `async-stripe` (openssl), `octocrab` (ring), `resend-rs` default TLS (openssl), `twilio`, `sendgrid`, `google-cloud-storage`, `sentry` default transport (hyper/socket2, aws-lc), `reqwest-retry` (tokio net) |

The "does not build" crates usually have a feature set that avoids the blocker (turn off the built-in
transport, pick a `rustls`-free/pure-Rust backend, inject a `reqwest::Client`); try
`default-features = false` and read the hint the CLI prints. Often it is simpler to call the service's HTTP
API with `ctx.fetch`.

## AWS: `convex_aws`

The AWS SDK's default HTTP client (hyper + rustls + aws-lc) cannot target wasm. Turn it off and use
`convex_aws`, which sends SDK requests through `ctx.fetch`, signs with pure-Rust SigV4, and reads
credentials from the environment:

```toml
[dependencies]
aws-sdk-s3 = { version = "1", default-features = false, features = ["behavior-version-latest"] }
convex_aws = { path = "/path/to/convex-rs/crates/convex_aws" }   # not published yet
```
```rust
let cfg = convex_aws::config_from_env().await;   // AWS_REGION, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_ENDPOINT_URL...
let s3 = aws_sdk_s3::Client::new(&cfg);
s3.put_object().bucket("b").key("k").body(bytes.into()).send().await?;
```

Set the credentials as deployment env vars (`npx convex env set AWS_ACCESS_KEY_ID ...`). Proven for S3
put/get/list and `NoSuchKey` errors; bodies are buffered. Other AWS service crates follow the same pattern but
are not individually tested.

## What does not work, and what to do

When a build fails, the CLI appends a `Hint:` section after cargo's own output with the cause, who depends on
the crate (`cargo tree -i`), and alternatives. The common cases:

| Problem | Typical crates | Instead |
|---|---|---|
| C libraries that need a compiler for wasm | `openssl-sys`, `aws-lc-sys`, `lzma-sys`, `bzip2-sys`, `libz-sys` (not verified) | Turn on a pure-Rust feature/backend: `jsonwebtoken` `rust_crypto`; `flate2` `rust_backend`; `ruzstd`/`lz4_flex`; RustCrypto (`sha2`, `hmac`, `aes-gcm`). HTTP needs no TLS crate (the host does TLS). |
| C libraries that build with the wasi-sdk | `zstd-sys` (`zstd` 0.13), `libsqlite3-sys` (`rusqlite` 0.32, `bundled`), `ring` 0.17 | See "C dependencies and the wasi-sdk" below. Without a wasi-sdk `ring` fails with `No available targets are compatible with triple "wasm32-unknown-wasip1"`. |
| Raw sockets and DB drivers | `sqlx`, `tokio-postgres`, `postgres`, `mongodb`, `redis`, `mysql`, `diesel`, `lettre`, `rdkafka`, `lapin`, `async-nats`, `tiberius`, `scylla`; anything using `socket2`/`mio`/`tokio::net` | Use the vendor's HTTP API or an HTTP driver (Neon/PlanetScale serverless, Supabase REST, Upstash Redis REST, an email provider's HTTP API). Calls to sockets fail at runtime, not silently. |
| Threads | `rayon`, `std::thread::spawn` | Code using them may compile, but threads cannot start at runtime (the error is recognised and explained in the CLI). Use `iter()` instead of `par_iter()`, or move the parallel work to a `"use node"` action. |
| `getrandom` "target not supported" | old `getrandom` | Usually means the build is not for `wasm32-wasip1` (build with `npx convex dev`, which passes `--target wasm32-wasip1`; a host `cargo build` doesn't target wasm); `cargo update -p getrandom`. |
| `psm` / `stacker` "section too large" | crates using `stacker` | Pin a working version of the dependent crate, or drop it. |

If you cannot avoid an incompatible crate, write that part as a `"use node"` action in TypeScript and call it
from Rust with `ctx.run_action("module:fn", args)`.

## C dependencies and the wasi-sdk

Crates whose build script compiles C (via `cc`/`cmake`) need a clang that targets wasm. `convex-rust build` looks for a
[wasi-sdk](https://github.com/WebAssembly/wasi-sdk) in `WASI_SDK_PATH`, then in `~/.cache/convex-rust/wasi-sdk-34.0`
(`CONVEX_RUST_CACHE_DIR` / `XDG_CACHE_HOME` move it), and sets `CC_wasm32_wasip1`, `CXX_wasm32_wasip1`,
`AR_wasm32_wasip1` and `CFLAGS_wasm32_wasip1=--sysroot=<sdk>/share/wasi-sysroot` for cargo. Variables you set yourself win.

- Default (`auto`): if no SDK is installed and `cargo metadata` shows a dependency with a `cc`/`cmake` build script, the
  pinned wasi-sdk 34.0 is downloaded once (about 180 MB; about 600 MB on disk; sha256 checked) and the build says so.
  A failed download is a warning, not an error.
- `--wasi-sdk` (or `CONVEX_RUST_WASI_SDK=1`) installs it even without a detected C crate; `--no-wasi-sdk`
  (`CONVEX_RUST_WASI_SDK=0`) never downloads or uses one. `convex-rust wasi-sdk` just installs and prints the path.

Verified on aarch64-apple-darwin with the repo's pinned nightly (build, and the function run through the host runtime):

| Crate | Result |
|---|---|
| `zstd` 0.13 (`zstd-sys`) | builds (+~0.8 MB wasm); compress/decompress round trip runs |
| `rusqlite` 0.32 `bundled` (`libsqlite3-sys`) | builds (+~1.4 MB wasm); `sqlite_version()` runs (3.46.0). In-memory only: the host has no filesystem, so the build warns about `path_open` and friends |
| `ring` 0.17.14 | builds (+~0.2 MB wasm); SHA-256 runs |

Other platforms use the same pinned release but were not tested. Other C crates are not verified.

Build scripts that only generate Rust code (no C) work. Proc macros work (they run on your machine).

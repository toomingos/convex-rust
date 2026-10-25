# convex_reqwest compatibility matrix (DX-06b)

Each crate has a probe under `compat/<name>/` that depends on the vendor crate
and routes `reqwest` to the shim with `[patch.crates-io] reqwest = { path = "../.." }`.
Build: `compat/build.sh [name...]` (target `wasm32-wasip1`, `--cfg tokio_unstable`
from `compat/.cargo/config.toml`). Run: `wasm/harness/tests/compat.rs` (wasmtime
host with mock responses) plus the existing `wasm/harness/tests/wasm.rs`.

Versions are the latest on crates.io on 2026-10-23. Probes use each crate's
default features unless noted; a "builds" failure can often be avoided by
choosing different features. Failure reasons come from the first error in each build log (`compat/results/<name>.build.log`, git-ignored); only the first blocker was identified, others may follow.

| Crate | Version | Builds? | Runs? | Notes |
|---|---|---|---|---|
| async-openai | 0.42.2 | No | n/a | 🚫 default features pull `tokio` net via `socket2` (hyper); `socket2` has no wasm target. Needs a feature set without the built-in transport. |
| async-stripe | 0.41.0 | No | n/a | 🚫 `runtime-tokio-hyper` pulls `openssl-sys` (C). Other runtimes not tried. |
| octocrab | 0.54.5 | No | n/a | 🚫 `ring` build script needs a C compiler for wasm32 (via `jsonwebtoken`/rustls). Uses hyper directly. |
| resend-rs | 0.35.0 | No | n/a | 🚫 default TLS is `openssl-sys` (C). Not tried with `rustls` feature. |
| aws-sdk-s3 | 1.155.0 | Yes (via `convex_aws`) | Yes | Not via the reqwest shim (smithy uses hyper): use `aws-sdk-s3 = { default-features = false, features = ["behavior-version-latest"] }` plus `convex_aws::config_from_env().await`, which plugs a smithy `HttpClient` over `ctx.fetch`, a tokio `AsyncSleep` and env credentials. SigV4 is pure Rust (sha2/hmac): no `ring`/`aws-lc-sys` in the wasm graph. put/get/list_objects_v2 + NoSuchKey proven on wasmtime against an S3 mock (`crates/convex_aws/probe`); 4.7 MB wasm. Bodies are buffered (v1). Use `force_path_style(true)` with `AWS_ENDPOINT_URL` mocks. |
| google-cloud-storage | 1.21.0 | No | n/a | 🚫 `socket2` (hyper) and `aws-lc-sys`/`ring` C builds. |
| sendgrid | 0.27.2 | No | n/a | 🚫 `socket2` (hyper) and C TLS dependencies. |
| twilio | 1.1.0 | No | n/a | 🚫 `openssl-sys` (C) and hyper. |
| anthropic-sdk | 0.1.5 | Yes | Not run | Compiles with the patch. No mock probe written; request/response paths are plain reqwest. |
| slack-morphism | 2.31.1 | Yes | Not run | Compiles with the patch (default features). No mock probe written. |
| reqwest-middleware | 0.5.2 (+ reqwest-retry 0.9.1) | Middleware: yes. Retry: No | Middleware: yes | `reqwest-middleware` alone runs (`wasm/harness/tests/wasm.rs::middleware`). `reqwest-retry` fails: it pulls `tokio` net / `socket2`. |
| oauth2 | 5.0.0 | Yes | Yes | Token exchange through the shim (`wasm/harness/tests/wasm.rs::oauth`). |
| jsonwebtoken + reqwest (JWKS) | 11.1.0 | Yes | Yes | HS256 sign/verify with `rust_crypto` backend plus a JWKS-style GET through the shim (`compat.rs`). Backends needing C (`aws_lc_rs`, `ring`) will not build. |
| url-shortener SDK (ezlime-rs) | 0.2.0 | Yes | Not run | "url-shortener SDK" is not a real crate name; `ezlime-rs` is a stand-in. Compiles with the patch. No mock probe written. |
| sentry | 0.49.3 | No | n/a | 🚫 `socket2` (hyper) and `aws-lc-sys` C build via the default transport. Try `default-features = false` with only the `reqwest` transport feature. |

## Takeaways

- The shim's API surface was sufficient for every crate that reached type-checking
  against it. No shim API gaps were found by this pass; the failures happen in
  dependencies below reqwest.
- Failures are dominated by three causes: `tokio` net/`socket2` through hyper
  (crates that use hyper directly or enable reqwest's default transport),
  C dependencies (`ring`, `aws-lc-sys`, `openssl-sys`) and `reqwest::blocking`
  (needs an SDK change, out of scope).
- Next steps for the "No" rows: retry with `default-features = false` and a
  `rustls`-only or custom-client feature set; many of these crates expose a
  way to inject a `reqwest::Client`, which would reach the shim.
- "Not run" rows only prove compilation. They were not exercised against mock
  responses in this pass.

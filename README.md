# convex-rust

Write Convex actions in Rust. The crate is compiled to `wasm32-wasip1`, and the generated wrappers run it
inside an ordinary Convex `action` / `internalAction` on **stock Convex** (the Convex cloud or any self-hosted
backend), with no backend or CLI changes. The wasm host uses WebAssembly JSPI to await `fetch`, `ctx.runQuery`,
`ctx.runMutation`, timers and storage from Rust. Queries, mutations and the schema stay in TypeScript.

For self-hosters there are also optional in-backend runtimes (a wasmtime wasm runtime and an opt-in native
worker mode) that need a fork of the Convex backend. They are not needed for the npm package.

Status: **experimental, version 0.1.0.** The npm package `convex-rust` is published from
[packages/convex-rust](packages/convex-rust). Convex default-runtime limits apply (actions only, no
`"use node"`); see [docs/rust/limits.md](docs/rust/limits.md).

## Quick start

In an existing Convex project (`npx convex dev` has been run once), with [rustup](https://rustup.rs) installed:

```sh
npm install convex convex-rust
npx convex-rust init                                  # scaffolds convex/rust, prints the next steps
npx convex-rust build                                 # compiles the crate to wasm, writes the TS wrappers
npx convex dev                                        # push (or: npx convex deploy)
npx convex run rust/example:greet '{"name":"you"}'    # => { "text": "Hello, you!" }
```

`npx convex-rust doctor` checks cargo, rustup, the toolchain and target, and `wasm-opt` (optional). Full guide:
[packages/convex-rust/README.md](packages/convex-rust/README.md) and [docs/rust/cloud.md](docs/rust/cloud.md).

## Repository layout

| Path | Contents |
|---|---|
| `crates/` | Rust crates: `convex_fn` (+ `convex_fn_macros`, the `#[action]` SDK), `convex_fn_test` (mock host for unit tests), `convex_reqwest` (reqwest-compatible shim over host fetch), `convex_aws` |
| `packages/convex-rust/` | The npm package: CLI (`init`, `build`, `dev`, `doctor`, ...) and the `convex-rust/runtime` wasm host |
| `docs/rust/` | User documentation (index below) |
| `docs/rust-native-runtime-design.md` | Design of the native runtime proposed upstream, including its safety assessment |
| `contracts/` | Specs: guest ABI, manifest, V8 host, native protocol |
| `backend-patches/` | The three patches of upstream PR [#620](https://github.com/convex/convex-backend/pull/620) (see its README) |
| `e2e/` | End-to-end suites (hello app, spec, reqwest, v8 host, native, deploy) |
| `bench/` | Benchmark runner, cases, and the report in `bench/reports/` |
| `release/` | Release tooling and the upstream PR drafts (`release/native-pr/`) |
| `spike/` | Early experiments (tokio on wasm32-wasip1) |

## Documentation

- [Overview](docs/rust/README.md), [getting started](docs/rust/getting-started.md), [writing actions](docs/rust/writing-actions.md)
- [Ctx API](docs/rust/ctx-api.md), [crates](docs/rust/crates.md), [testing](docs/rust/testing.md)
- [Convex cloud / stock self-hosted](docs/rust/cloud.md), [deploy and CI](docs/rust/deploy-ci.md), [limits](docs/rust/limits.md)
- [Self-hosted fork](docs/rust/self-hosted-fork.md), [native mode](docs/rust/native.md) (experimental, opt-in, trusted single-tenant backends only)

## Backend fork

The in-backend runtimes live in a fork of the Convex backend:
[github.com/toomingos/convex-backend](https://github.com/toomingos/convex-backend), branch `rust-native-runtime`
(upstream PR [convex/convex-backend#620](https://github.com/convex/convex-backend/pull/620)). It is not included
here. Scripts and e2e suites that need it look for a clone at `vendor/convex-backend` or at `$CONVEX_BACKEND_DIR`.

## Development

```sh
cargo check -p convex_fn -p convex_fn_macros -p convex_fn_test
cd packages/convex-rust && npm install && npm run build && npm test
```

The toolchain is pinned in `rust-toolchain.toml`.

## License

Apache-2.0, Copyright 2026 Tomás Domingos. See [LICENSE](LICENSE).

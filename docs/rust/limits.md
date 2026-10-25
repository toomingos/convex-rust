# Limits and unsupported features

These are the limits of the default wasm runtime. The experimental, opt-in native mode has different
memory, concurrency and sandbox rules: see [native.md](native.md).

| Limit | Value |
|---|---|
| Memory per invocation | 512 MiB of wasm linear memory by default (same as the Node action default). Exceeding it fails the call. |
| Time | The same user timeout as TS actions (`V8_ACTION_USER_TIMEOUT`, default 1800 s on self-hosted). Time spent waiting on `fetch`, `run_*` and `sleep` counts. |
| Return value | Same size limit as any action return value; larger results fail the call. Store big data with `ctx.storage()` and return the id. |
| Arguments | Same limits as any action call. |
| HTTP and storage bodies | Buffered in memory (request and response, `store`/`get`); `fetch_stream` streams responses only. Keep them well under the memory limit. |
| Wasm module size | The CLI warns above 20 MB ([deploy-ci.md](deploy-ci.md)). Size drives cold start. |
| Concurrency | One wasm instance per invocation; no shared state between calls. |

Performance numbers (cloud wasm and local native versus "use node") are in [cloud.md](cloud.md#measured-performance-cloud) and [native.md](native.md#measured-performance-local-native-vs-use-node); the full report is `bench/reports/final-2026-10-25/report.html`.

## Not supported

- **Queries and mutations in Rust.** Only actions (`#[action]`). Write queries, mutations and the schema in
  TypeScript and call them with `ctx.run_query` / `ctx.run_mutation`.
- **`httpAction` in Rust.** Supported on cloud / stock self-hosted (V8 host, streaming) and on the fork's native and
  wasmtime runtimes (buffered, request body under about 12 MiB; see [native.md](native.md#http-actions)).
- **Components.** A Rust crate inside a component directory is rejected at push. Rust actions live in the app's
  `convex/rust` only.
- **Importing across the boundary.** TS cannot import Rust modules; call them through `api.rust.*` /
  `internal.rust.*`.
- **`#[action]` in the crate root** (`lib.rs`) is rejected at push; put actions in modules.
- **Sockets, threads, C libraries.** See [crates.md](crates.md#what-does-not-work-and-what-to-do).
- **Streaming request bodies**; the file system (nothing persistent, use storage); anything needing the
  network other than through the host `fetch`.
- **Convex "use node" packages and Node APIs.** Rust actions run in wasm, not Node.

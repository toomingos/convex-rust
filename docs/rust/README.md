# "use rust": Convex actions in Rust

Write Convex **actions** in Rust. They compile to WebAssembly (`wasm32-wasip1`), are pushed by
`npx convex dev` / `npx convex deploy` next to your TypeScript, and show up in the generated
client API as `api.rust.<module>.<function>`.

```rust
// convex/rust/src/images.rs
#[action]
pub async fn thumbnail(ctx: &Ctx, args: ThumbnailArgs) -> Result<Thumbnail> { /* ... */ }
```
```ts
// React / TS: fully typed from the Rust signature
const thumbnail = useAction(api.rust.images.thumbnail);
```

Rust functions are actions and HTTP actions only (on the fork runtimes HTTP actions are buffered). Queries, mutations and the schema stay in TypeScript;
call them from Rust with `ctx.run_query` / `ctx.run_mutation` using typed `api::` references.

Runs on the **fork** of the self-hosted backend in this repo, or on stock Convex / the Convex cloud through the
`convex-rust` npm package (see [cloud.md](cloud.md)).
See [self-hosted-fork.md](self-hosted-fork.md) for how to get the backend and CLI today.

| Page | What is in it |
|---|---|
| [getting-started.md](getting-started.md) | Prerequisites, `npx convex rust init`, first action, dev loop, calling from React |
| [writing-actions.md](writing-actions.md) | `#[action]`, args and return types, errors and panics, internal actions, module paths |
| [ctx-api.md](ctx-api.md) | `Ctx`: run_query/mutation/action, scheduler, storage, auth, fetch, env, logging, time, async |
| [crates.md](crates.md) | `cargo add`, the built-in `reqwest`, AWS, what does not compile and what to do instead |
| [testing.md](testing.md) | Unit tests with the `convex_fn_test` mock host |
| [deploy-ci.md](deploy-ci.md) | `convex deploy`, release builds, wasm size, GitHub Actions ([ci/convex-deploy.yml](ci/convex-deploy.yml)) |
| [limits.md](limits.md) | Memory, time, sizes, and what is not supported |
| [cloud.md](cloud.md) | Convex cloud / stock self-hosted via the `convex-rust` npm package: setup, dev loop, env vars, limits, differences |
| [self-hosted-fork.md](self-hosted-fork.md) | Building the fork backend and CLI, linking the CLI package |
| [native.md](native.md) | Experimental, opt-in native worker mode: trust model, platforms, knobs, differences from wasm |

Benchmarks (node vs ts-v8 vs rust-v8 on the cloud, node vs native locally): `bench/reports/final-2026-10-25/report.html`,
summarised in [cloud.md](cloud.md#measured-performance-cloud) and [native.md](native.md#measured-performance-local-native-vs-use-node).

Working examples in this repo: `e2e/hello` (smallest app), `e2e/spec/convex/rust` (every `Ctx` feature),
`bench/convex/rust`.

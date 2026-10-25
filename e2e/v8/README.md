# e2e/v8: spec suite on the rust-v8 runtime

Runs the unmodified `e2e/spec` suite (47 tests, node vs Rust) with the Rust side hosted by the
`convex-rust` package inside a normal default-runtime Convex action (D-104, `contracts/V8HOST.md`).

```sh
node e2e/v8/run.mjs [--keep] [--skip-build]
```

Prints a PASS/FAIL/ALLOWED-DIFF table; exit 1 on any non-allowed FAIL. Ports 3380/3381, mock 3388
(`SPEC_PORT` / `SPEC_MOCK_PORT`); backend data in `$V8_TMP` (default `<tmpdir>/convex-rs-e2e-v8`).
Needs the debug backend in `target-backend/debug`, a built `packages/convex-rust/dist`, rustup nightly +
`wasm32-wasip1`, `wasm-opt` (optional). Rust builds use `CARGO_TARGET_DIR` (default `target-e2e`), 4 jobs.

## How it works

- `e2e/v8/app/` (generated, git-ignored): the spec's TS sources (`data`, `nodeSpec`, `schema`, `http`)
  copied from `e2e/spec/convex` each run, plus `convex/rust/` produced by
  `convex-rust build --crate e2e/spec/convex/rust --out app/convex/rust`. Function paths are
  `rust/<module>:<name>`, exactly as on the backend-hosted runtime, so the spec runs as is.
- The push uses the **stock npm CLI** (`convex@1.48.0`, `e2e/v8/node_modules`), no fork code.
  The fork CLI would auto-build `convex/rust` and overwrite the generated wrappers.
- `parity/preload.mjs` (`node --import`, same idea as `e2e/native/parity/preload.mjs`) redirects the
  harness' CLI calls (cwd `e2e/spec`) to the stock CLI in `app/`, and redirects `e2e/spec/.backend-data`
  to `$V8_TMP/data`. `e2e/spec` is not edited; the runner restores `Cargo.lock`.
- `convex-rust` is installed with `install-links=true` (a copy, like a published install). A symlink makes
  esbuild bundle the package's own devDependency `convex`, so `instanceof ConvexError` in the host fails
  (ConvexError raised by `ctx.runAction` is then not recognised, 1 spec failure). Hardening idea for the
  host: also accept `e?.name === "ConvexError"` with a `data` property.
- The component test (`W3-02 components`) uses the fork CLI (it tests the fork's push check), unchanged.

## Env vars (the one open difference)

`process.env` in the Convex default runtime is a `Proxy` with only a `get` trap, so it cannot be
enumerated; the host builds WASI `environ` from `Object.entries(process.env)` and so gives the guest an
empty environment. Affects 3 tests: "deployment env vars", "updated var" (both in `allowed.json`) and the
DX-05 probe (its env assertion is first; its log/clock/random assertions pass, verified by hand).
Fix for `packages/convex-rust` (not done here): let the user name the variables, e.g.
`convex-rust build --env A,B` emitted into the wrapper as `runRust(ctx, wasm, name, args, { env: [...] })`
(or a `CONVEX_RUST_ENV=A,B` deployment variable read by name), and have the host build `environ` from
`process.env[name]` for those names. With that the suite should be 47/47.

# Deploying and CI

## `npx convex deploy`

```sh
export CONVEX_SELF_HOSTED_URL=https://your-backend.example.com
export CONVEX_SELF_HOSTED_ADMIN_KEY=...
npx convex deploy
```

Same command as for TS-only projects. For `convex/rust` it additionally:

1. checks the toolchain (`rust-toolchain.toml`) and the `wasm32-wasip1` target (fails with the fix command
   when there is no TTY),
2. refreshes the managed block in `.cargo/config.toml`,
3. runs `cargo build --release --target wasm32-wasip1` (the dev command uses a faster
   `convex-dev` profile instead: opt-level 1, incremental, no LTO),
4. runs `wasm-opt -Oz` **if `wasm-opt` is on PATH** (below),
5. prints the size report, reads the function manifest from the wasm, regenerates `api.rs` / `api.d.ts`,
6. pushes the wasm together with your TS modules.

No `-y`/`--yes` is needed; there are no prompts when the toolchain is in place.

Build time: the first build compiles every dependency (about a minute for `image`); later builds only compile your crate. Cache `convex/rust/target` and `~/.cargo` in CI.

### Size report and the 20 MB warning

```
Rust wasm: 4.2 MB (code 3.2 MB, data 639.8 KB, names 416.8 KB, other 9.3 KB)
```

Above 20 MB the CLI warns, names the biggest section, and suggests fewer or lighter dependencies,
`opt-level = "z"`, `lto = true` or installing `wasm-opt`. The warning does not stop the deploy; the hard
limits are in [limits.md](limits.md). Size and cold start go together (below), so keep it small.

Release profile knobs go in `convex/rust/Cargo.toml`:

```toml
[profile.release]
opt-level = "z"     # smaller; "3" is faster
lto = true
codegen-units = 1
strip = false       # keep it: panic locations rely on the name section
```

### wasm-opt (binaryen)

Optional but recommended for deploys: a smaller module. Install `binaryen`
(`brew install binaryen`, `apt-get install binaryen`). The CLI runs
`wasm-opt -Oz -g --enable-bulk-memory --enable-sign-ext --enable-mutable-globals --enable-nontrapping-float-to-int --enable-multivalue`,
keeps the result only if it is smaller and still has the manifest, and caches by content hash in
`convex/rust/target/wasm32-wasip1/release/convex-wasm-opt-cache`, so an unchanged redeploy skips it. If `wasm-opt` is missing
or fails, the unoptimized wasm is deployed (nothing breaks, it is just bigger). It is only used for
`deploy`, not for `dev`.

### Cold start

A module is compiled the first time it runs on a backend and kept in a compiled-module cache keyed by the
wasm hash, so the first call after a deploy or restart pays compile time that grows with module size;
subsequent calls reuse it. To keep the first call fast: keep dependencies lean, use `opt-level = "z"` /
`wasm-opt`, and check the size report.

## GitHub Actions

[ci/convex-deploy.yml](ci/convex-deploy.yml) is a ready workflow. Copy it to `.github/workflows/`. It:

- installs the exact toolchain and `wasm32-wasip1` target pinned in `convex/rust/rust-toolchain.toml`,
- caches `~/.cargo` registry and `convex/rust/target`,
- installs `binaryen` so `wasm-opt` is used,
- runs `cargo test` on the host target, then `npx convex deploy`.

Secrets:

| Secret | When |
|---|---|
| `CONVEX_SELF_HOSTED_URL`, `CONVEX_SELF_HOSTED_ADMIN_KEY` | self-hosted backend (the fork; the normal case) |
| `CONVEX_DEPLOY_KEY` | a deployment that accepts a deploy key; set it instead of the two above (never both) |

Things the pre-publication setup needs in CI:

1. **The fork CLI as the `convex` dependency.** `npm ci` must install the fork package, not npm's `convex`
   ([self-hosted-fork.md](self-hosted-fork.md)). Build it in CI or vendor it; the workflow has a marked step.
2. **`convex_fn` on disk.** `init` wrote a relative path in `convex/rust/Cargo.toml`; that path has to exist
   on the runner (a git submodule or a second checkout at the same relative location, see the marked step).
   Once `convex_fn` is on crates.io this step disappears.
3. Commit `convex/rust/Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml` and
   `src/_generated/api.rs`. The managed patch block is rewritten on the runner, so absolute paths inside it
   do not matter.

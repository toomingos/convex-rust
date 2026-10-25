# Self-hosted: applying the wasm patch to upstream Convex

"use rust" needs two changes to stock Convex: a backend that runs the wasm, and a `convex` CLI that builds and
pushes it. Instead of running our whole fork, you can apply a patch to the upstream
[`get-convex/convex-backend`](https://github.com/get-convex/convex-backend) source at the version you run, and
build both yourself. The alternative is building the fork directly; see [self-hosted-fork.md](self-hosted-fork.md).

The patch covers the wasm runtime only. The experimental native worker ([native.md](native.md)) is a separate
patch and is not needed.

## What you get

`convex-rust-wasm.patch` is one `git diff` against upstream commit `342530f`. It adds the Rust runtime in
`crates/isolate`, the push-time analyzer in `crates/application`, a few knobs and plumbing lines, and the CLI
(`convex rust init`, the wasm build and push, generated `api.rust.*`). It touches 25 upstream files
(+881/-56 lines including `Cargo.lock`, which is regenerated; see `SUMMARY.md`) and adds 25 new files (about 8,150 lines). The exact numbers
for each release are in `SUMMARY.md` next to the patch.

Files: `convex-rust-wasm.patch` (full, including `Cargo.lock`) and `convex-rust-wasm-nolock.patch` (same without
`Cargo.lock`). Both come from `release/make-patches.sh` in the `convex-rs` repository, which writes into
`release/out/`.

## 1. Apply it

Use the upstream commit the patch was made against when you can; it applies exactly.

```sh
git clone https://github.com/get-convex/convex-backend.git && cd convex-backend
git switch -c use-rust 342530f
git apply --check /path/to/convex-rust-wasm.patch      # must print nothing
git apply --index /path/to/convex-rust-wasm.patch
git commit -m "use rust: wasm runtime"
```

To run a different upstream version (a release tag or newer `main`), branch from that version and use the
no-lock patch with a three-way merge:

```sh
git switch -c use-rust <your-upstream-tag-or-commit>
git apply --3way --index /path/to/convex-rust-wasm-nolock.patch
```

If `--3way` reports conflicts, resolve them the usual way (`git status`, edit, `git add`). The patch is
designed so that edits to upstream files are small and isolated; the usual conflict sites are the
`application_function_runner/mod.rs` dispatch, `knobs.rs`, and `cli/dev.ts` or `deploy.ts` hunks. Everything
else is new files. Then refresh the lock file with `cargo check -p isolate -p application` and commit
`Cargo.lock`.

## 2. Build the backend

Same prerequisites as upstream (see its `self-hosting` docs): the Rust nightly pinned in
`rust-toolchain.toml`, `clang`, `cmake`, `protoc`, and a working V8 build setup.

```sh
export CARGO_TARGET_DIR=$PWD/target SODIUM_USE_PKG_CONFIG=1
cargo build --release -p local_backend --bin convex-local-backend -p keybroker --bin generate_key
```

For Docker, build the image from the patched tree with upstream's own Dockerfile and self-hosting instructions.

Optional knobs are listed in [limits.md](limits.md) and `crates/common/src/knobs.rs` (search for `RUST_`).

## 3. Build the CLI

```sh
cd npm-packages/convex
npm ci                                   # or: just rush install, per upstream's instructions
node scripts/build.cjs standalone-cli tempDir=dist
# produces dist/cli.bundle.cjs and the package files
```

Link it into your app instead of the npm `convex` package:

```sh
cd /path/to/your-app
npm install /path/to/convex-backend/npm-packages/convex     # or "convex": "file:..." in package.json
```

`npx convex rust init`, `npx convex dev` and `npx convex deploy` then understand `convex/rust`.

## 4. Run and use it

Start the patched backend as in [self-hosted-fork.md](self-hosted-fork.md) section 3, point the CLI at it
(`CONVEX_SELF_HOSTED_URL`, `CONVEX_SELF_HOSTED_ADMIN_KEY`), and follow [getting-started.md](getting-started.md).
You also need the SDK crates (`convex_fn` and friends) from the `convex-rs` repository, referenced by path via
`CONVEX_FN_PATH`.

## Upgrading to a new upstream release

1. Fetch the new upstream tag and branch from it.
2. `git apply --3way --index convex-rust-wasm-nolock.patch`; resolve conflicts.
3. `cargo check -p isolate -p application`, rebuild the backend and the CLI.
4. Re-run your push; the manifest and ABI contract are versioned (ABI v1), so existing Rust modules keep working
   unless the release notes of the patch say otherwise.

## Caveats

- This patch is not part of upstream Convex and is not supported by Convex. Stock Convex and the npm `convex`
  CLI do not understand `convex/rust`; Convex Cloud cannot run it.
- Apply the CLI and backend from the same patch version; the wasm ABI and the manifest format are checked at
  push time and a mismatch is rejected with an explicit message.
- `Cargo.lock` in the full patch is tied to the exact base commit. Use the no-lock patch on any other version.

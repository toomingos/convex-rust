# e2e/hello

First end-to-end "use rust" action on the forked self-hosted backend.

- `convex/messages.ts`: `list` query, `add` mutation.
- `convex/rust/`: standalone crate (`hello_rust`), module `hello.rs` with
  `hello` (runs `messages:list`, logs, returns a string) and `fail` (returns `Err`).

All commands from the repo root unless noted. `vendor/convex-backend` is a clone of
`github.com/toomingos/convex-backend` (branch `rust-native-runtime`, not included in this repo). Rust toolchain
is `nightly-2026-06-28` with `wasm32-wasip1` (root `rust-toolchain.toml`, copied here).

## 1. Build the backend and key generator

```sh
cd vendor/convex-backend
export CARGO_TARGET_DIR=<repo>/target-backend \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 SODIUM_USE_PKG_CONFIG=1 CONVEX_PREBUILT_JS=1
cargo build -p local_backend --bin convex-local-backend -p keybroker --bin generate_key -j 4
```

Note: the `v8` build script downloads `gen/src_binding_simdutf_release_aarch64-apple-darwin.rs`
into the cargo git checkout of rusty_v8. If that checkout gets re-created the file can go
missing ("couldn't read .../gen/src_binding_..."); re-download it from
`https://github.com/convex/rusty_v8/releases/download/v149.2.11/`.

## 2. Build the fork's convex CLI from source

```sh
cd vendor/convex-backend/npm-packages/convex
node scripts/build.cjs standalone-cli tempDir=dist   # writes dist/cli.bundle.cjs
cd e2e/hello && npm install   # links the fork via file:
```

## 3. Start the backend

```sh
cd e2e/hello && mkdir -p .backend-data && cd .backend-data
B=<repo>/target-backend/debug
openssl rand -hex 32 > secret
$B/generate_key convex-rs-e2e $(cat secret) | tail -1 > admin-key
$B/convex-local-backend --instance-name convex-rs-e2e --instance-secret $(cat secret) \
  --interface 127.0.0.1 --port 3210 --site-proxy-port 3211 convex_local_backend.sqlite3 &
echo $! > backend.pid
```

## 4. Push, seed, run

```sh
cd e2e/hello
export CONVEX_SELF_HOSTED_URL=http://127.0.0.1:3210
export CONVEX_SELF_HOSTED_ADMIN_KEY=$(cat .backend-data/admin-key)
export CARGO_TARGET_DIR=<repo>/target-e2e   # optional: wasm build dir
npx convex dev --once                       # builds the wasm, pushes rust/hello.js
npx convex run messages:add '{"body":"one"}'
npx convex run messages:add '{"body":"two"}'
npx convex run rust/hello:hello '{}'        # -> "hello from rust: 2 messages" (+ [LOG] line)
npx convex run rust/hello:fail '{"reason":"nope"}'   # -> Server Error / Uncaught Error: boom: nope
```

## 5. Stop

```sh
kill $(cat .backend-data/backend.pid)
```

## Types

`convex dev --once` generates `api.rust.*` in `convex/_generated/api.d.ts`.
`convex/typecheck_rust.ts` checks it: run `tsc --noEmit -p convex` with the fork's
`npm-packages/convex/node_modules/.bin/tsc`.

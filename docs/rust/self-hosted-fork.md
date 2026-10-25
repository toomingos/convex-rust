# Self-hosted fork: building and linking today

"use rust" needs a fork of the self-hosted Convex backend (it runs the wasm) and the fork's `convex` CLI
(it builds and pushes the wasm). Neither is published yet. Until they are, build both from the
`convex-rs` checkout (`vendor/convex-backend` is the fork). Stock Convex and the npm `convex` CLI do not
understand `convex/rust`.

Prefer to stay on stock upstream? Apply the wasm patch to the Convex version you run instead: see
[self-hosted-patch.md](self-hosted-patch.md).

Paths below assume `CONVEX_RS=/path/to/convex-rs`.

## 1. Build the backend

```sh
cd $CONVEX_RS/vendor/convex-backend
export CARGO_TARGET_DIR=$CONVEX_RS/target-backend \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 SODIUM_USE_PKG_CONFIG=1 CONVEX_PREBUILT_JS=1
cargo build -p local_backend --bin convex-local-backend -p keybroker --bin generate_key -j 4
```

## 2. Build the CLI and link it into your app

```sh
cd $CONVEX_RS/vendor/convex-backend/npm-packages/convex
node scripts/build.cjs standalone-cli tempDir=dist      # writes dist/cli.bundle.cjs
cd /path/to/your-app
npm install $CONVEX_RS/vendor/convex-backend/npm-packages/convex      # or "convex": "file:..." in package.json
```

## 3. Run a backend

```sh
mkdir -p .backend-data && cd .backend-data
B=$CONVEX_RS/target-backend/debug
openssl rand -hex 32 > secret
$B/generate_key my-instance $(cat secret) | tail -1 > admin-key   # prints "Admin key:" then the key; keep the last line
$B/convex-local-backend --instance-name my-instance --instance-secret $(cat secret) \
  --interface 127.0.0.1 --port 3210 --site-proxy-port 3211 convex_local_backend.sqlite3 &
```

```sh
export CONVEX_SELF_HOSTED_URL=http://127.0.0.1:3210
export CONVEX_SELF_HOSTED_ADMIN_KEY=$(cat .backend-data/admin-key)
```

## 4. Use it

```sh
CONVEX_FN_PATH=$CONVEX_RS/crates/convex_fn npx convex rust init
npx convex dev
```

`CONVEX_FN_PATH` tells `init` where the unpublished `convex_fn` crate is; it is written into
`convex/rust/Cargo.toml` as a relative path. Other `init` overrides: `CONVEX_RUST_CHANNEL` (toolchain
channel to pin) and `CONVEX_REQWEST_PATH` (the reqwest shim; normally found inside the CLI package). Add
`convex_fn_test` and `convex_aws` to `Cargo.toml` by path the same way ([testing.md](testing.md),
[crates.md](crates.md)).

Environment variables you will use: `CONVEX_SELF_HOSTED_URL`, `CONVEX_SELF_HOSTED_ADMIN_KEY` (or
`CONVEX_DEPLOY_KEY`). For production, run the backend behind your usual TLS proxy and raise
`V8_ACTION_USER_TIMEOUT` only if you need to (see [limits.md](limits.md)).

## Optional: native mode

The fork also contains an experimental, opt-in worker-process mode (`RUST_NATIVE_MODE=worker`). It is off by
default and not yet released; read the trust-model warning in [native.md](native.md) before enabling it.

## Known rough edges

- `convex_fn` is referenced by path, so the app and the checkout need a stable relative layout (also in CI).

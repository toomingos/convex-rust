#!/bin/sh
# Runs the fork `convex` CLI against the local bench backend: bench/scripts/cvx.sh dev --once | run ... | env set ...
# Env: BENCH_PORT (default 3220), BENCH_DATA_DIR (default bench/.backend-data), CARGO_TARGET_DIR (wasm build dir),
#      CONVEX_RUST_PROFILE=release|dev (default release here: full release build + `wasm-opt -O3`, no -g;
#      the CLI default without this variable is the fast dev profile, which must not be benchmarked).
#      BENCH_NATIVE=1: talk to the rust-native backend (port BENCH_PORT+10, data dir "$BENCH_DATA_DIR-native") and
#      make `dev` / `deploy` build and push the native cdylib (`--rust-runtime native`, release profile).
# After a `dev` push, bench/convex/.build-info.json records the build profile, wasm-opt version and wasm sizes.
cd "$(dirname "$0")/.." || exit 1
PORT="${BENCH_PORT:-3220}"
DATA="${BENCH_DATA_DIR:-.backend-data}"
EXTRA=""
CMD="$1"
NATIVE_FLAG=""
if [ "${BENCH_NATIVE:-0}" = 1 ]; then
  NATIVE_FLAG="--native"
  PORT=$((PORT + 10))
  DATA="$DATA-native"
  case "$1" in dev | deploy) EXTRA="--rust-runtime native" ;; esac
fi
export CONVEX_SELF_HOSTED_URL="http://127.0.0.1:$PORT"
export CONVEX_SELF_HOSTED_ADMIN_KEY="$(cat "$DATA/admin-key")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-../target-e2e}"
export CONVEX_RUST_PROFILE="${CONVEX_RUST_PROFILE:-release}"
if [ -n "$EXTRA" ]; then
  shift
  npx convex "$CMD" $EXTRA "$@"
else
  npx convex "$@"
fi
rc=$?
if [ "$CMD" = dev ] && [ $rc -eq 0 ]; then
  # shellcheck disable=SC2086
  node scripts/build-info.mjs $NATIVE_FLAG || echo "warning: could not write bench/convex/.build-info.json" >&2
fi
exit $rc

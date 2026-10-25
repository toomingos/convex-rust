#!/bin/sh
# Runs (foreground, exec) a local forked backend for the bench app in bench/.backend-data.
# Launch it detached, e.g. `bench/scripts/start-backend.sh [port] [site-proxy-port] > log 2>&1 &`.
# BACKEND_PROFILE=debug|release picks target-backend/<profile> (or BACKEND_BIN_DIR). Benchmarks must use release.
# BENCH_DATA_DIR overrides the data dir (default bench/.backend-data).
# BENCH_NATIVE=1 starts the rust-native instance instead: data dir "<data dir>-native" (own secret and admin key),
#   RUST_NATIVE_MODE=worker. The runner starts it on port+10 (e.g. 3380/3381 for 3370/3371); one deployment holds
#   only one Rust artifact (wasm or native), so rust-native needs its own backend.
# Needs Node 22/24 on PATH for the node executor (the fork rejects v26).
PORT=${1:-3220}
SITE=${2:-3221}
BIN=${BACKEND_BIN_DIR:-$(cd "$(dirname "$0")/../.." && pwd)/target-backend/${BACKEND_PROFILE:-debug}}
D="${BENCH_DATA_DIR:-$(cd "$(dirname "$0")/.." && pwd)/.backend-data}"
if [ "${BENCH_NATIVE:-0}" = 1 ]; then
  D="$D-native"
  export RUST_NATIVE_MODE=worker
  # macOS has no worker sandbox; the backend refuses native mode without this opt-in (trusted local bench).
  [ "$(uname)" = Linux ] || export RUST_NATIVE_ALLOW_UNSANDBOXED=true
else
  unset RUST_NATIVE_MODE
fi
mkdir -p "$D"
cd "$D" || exit 1
[ -f secret ] || openssl rand -hex 32 > secret
"$BIN/generate_key" convex-rs-bench "$(cat secret)" | tail -1 > admin-key
export PATH="${NODE_BIN_DIR:-/usr/local/bin}:$PATH"
# Keep every UDF completion in the in-memory function log (default 1000) so the phase-2
# runner can read per-call execution times from /api/stream_function_logs.
export MAX_UDF_EXECUTION="${MAX_UDF_EXECUTION:-500000}"
exec "$BIN/convex-local-backend" --instance-name convex-rs-bench --instance-secret "$(cat secret)" \
  --interface 127.0.0.1 --port "$PORT" --site-proxy-port "$SITE" convex_local_backend.sqlite3

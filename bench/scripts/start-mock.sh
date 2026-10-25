#!/bin/sh
# Runs (foreground, exec) the net-* mock server on MOCK_PORT (default 8787). Needs node >= 22.6 for type stripping.
cd "$(dirname "$0")/.." || exit 1
export PATH="${NODE_BIN_DIR:-/usr/local/bin}:$PATH"
export MOCK_PORT="${MOCK_PORT:-8787}"
exec node --experimental-strip-types mock-server/server.ts

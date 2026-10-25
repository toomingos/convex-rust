#!/bin/sh
# usage: redeploy.sh [nonce]  -- regenerates convex/{node,v8} (with the nonce) and pushes with the stock CLI.
set -e
cd "$(dirname "$0")"
BENCH_NONCE="$1" node gen.mjs
exec ./push.sh

#!/bin/sh
# Rebuilds http-guest.wasm (the HTTP action fixture of test/http.test.ts) from http-crate.
set -e
cd "$(dirname "$0")/http-crate"
cargo build --release --target wasm32-wasip1
cp "${CARGO_TARGET_DIR:-target}/wasm32-wasip1/release/http_fixture.wasm" ../http-guest.wasm
ls -l ../http-guest.wasm

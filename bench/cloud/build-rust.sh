#!/bin/sh
# Builds the bench Rust crate with the convex-rust package (V8 host) into convex/rustv8 (functions rustv8/<module>:<name>).
set -e
cd "$(dirname "$0")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-../../target-e2e}" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
# node_modules/convex-rust is an installed copy (npm file: dep); refresh it from the package build so the
# bundle always carries the current runtime.
(cd ../../packages/convex-rust && npm run build >/dev/null)
rsync -a --delete ../../packages/convex-rust/dist/ node_modules/convex-rust/dist/
rsync -a --delete --exclude target ../../packages/convex-rust/rust/ node_modules/convex-rust/rust/
cp ../../packages/convex-rust/package.json node_modules/convex-rust/package.json && chmod +x node_modules/convex-rust/dist/cli/bin.js
# Packaged default (D-L4b): a plain crate, one wasm per Convex module via the zero-config split. No groups/features.
npx --no-install convex-rust build --crate ../convex/rust --out convex/rustv8 --profile release

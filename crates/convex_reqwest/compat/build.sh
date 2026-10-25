#!/bin/bash
# Build compat probes for wasm32-wasip1 against the shim. Usage: build.sh [name...]
ROOT=$(cd "$(dirname "$0")" && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/../../../target-e2e}
mkdir -p $ROOT/results
names=("$@")
if [ ${#names[@]} -eq 0 ]; then names=($(cd $ROOT && ls -d */ | tr -d / | grep -v -e results)); fi
for n in "${names[@]}"; do
  start=$(date +%s)
  (cd $ROOT/$n && cargo build --release --target wasm32-wasip1 -j 4) > $ROOT/results/$n.build.log 2>&1
  rc=$?
  echo "$n rc=$rc $(( $(date +%s) - start ))s"
done

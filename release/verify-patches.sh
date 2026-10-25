#!/usr/bin/env bash
# Apply the patches to a clean upstream-base worktree (git apply --check, then apply), optionally cargo check.
# Usage: release/verify-patches.sh [-s SCRATCH_DIR] [-c]   (-c = also run cargo check; needs >= 8 GB free, override with VERIFY_MIN_GB)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/.." && pwd)"
FORK="$ROOT/vendor/convex-backend"; BASE=342530f; HEADREF=use-rust; SCRATCH="${TMPDIR:-/tmp}"; CHECK=0
while getopts "s:c" o; do case $o in s) SCRATCH=$OPTARG;; c) CHECK=1;; *) exit 2;; esac; done
OUT="$HERE/out"; UP="$SCRATCH/up"
cleanup() { git -C "$FORK" worktree remove --force "$UP" >/dev/null 2>&1 || true; }
trap cleanup EXIT
run_case() { # name patches...
  local name=$1; shift
  rm -rf "$UP"; git -C "$FORK" worktree prune
  git -C "$FORK" worktree add --detach "$UP" "$BASE" >/dev/null 2>&1
  for p in "$@"; do
    git -C "$UP" apply --check "$OUT/$p" && echo "[$name] apply --check $p: ok"
    git -C "$UP" apply "$OUT/$p" && echo "[$name] apply $p: ok"
  done
  git -C "$UP" status --short | wc -l | xargs echo "[$name] changed paths:"
  if [ "$CHECK" = 1 ] && [[ "$name" != *nolock* ]]; then
    avail=$(df -g /System/Volumes/Data 2>/dev/null | awk 'NR==2{print $4}' || echo 0)
    if [ "${avail:-0}" -lt "${VERIFY_MIN_GB:-8}" ]; then echo "[$name] cargo check SKIPPED: only ${avail}G free"; else
      # cargo check uses the fork's own lock (crates already in the local cargo cache; --offline, no network).
      # Cargo prunes the entries a wasm-only tree does not need. The lock inside the patch is checked by apply only.
      git -C "$FORK" show "$HEADREF:Cargo.lock" > "$UP/Cargo.lock"
      # isolate/build.rs bundles the system UDFs even with CONVEX_PREBUILT_JS; borrow the fork's built JS.
      for d in "$FORK"/npm-packages/node_modules "$FORK"/npm-packages/*/node_modules "$FORK"/npm-packages/*/dist; do
        rel=${d#"$FORK"/}; [ -d "$UP/$(dirname "$rel")" ] && [ ! -e "$UP/$rel" ] && ln -s "$d" "$UP/$rel"
      done
      (cd "$UP" && CARGO_TARGET_DIR="$ROOT/target-backend" CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
        SODIUM_USE_PKG_CONFIG=1 CONVEX_PREBUILT_JS=1 CARGO_NET_OFFLINE=true cargo check --offline -p isolate -p application -j 4 2>&1 | tail -25)
    fi
  fi
  git -C "$FORK" worktree remove --force "$UP"
}
run_case wasm convex-rust-wasm.patch
run_case wasm+native convex-rust-wasm.patch convex-rust-native.patch
run_case wasm-nolock+native-nolock convex-rust-wasm-nolock.patch convex-rust-native-nolock.patch

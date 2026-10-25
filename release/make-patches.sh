#!/usr/bin/env bash
# Produce the upstream patches from the fork's COMMITTED history. Read-only on the fork checkout:
# it only creates (and removes) a detached scratch worktree. Never pushes or contacts a remote.
#
#   convex-rust-wasm.patch         base -> wasm-only tree (wasm runtime + CLI), incl. Cargo.lock
#   convex-rust-wasm-nolock.patch  same without Cargo.lock (use on other upstream versions; then `cargo update -w`)
#   convex-rust-native-nolock.patch  same without Cargo.lock
#   convex-rust-native.patch       wasm-only tree -> use-rust (native worker on top of the wasm patch)
#   SUMMARY.md                     diffstats, upstream-touched files, budget comparison
#
# Note: write-tree leaves one dangling tree object in the fork object store (harmless; gc removes it).
# Method: in a scratch worktree of the head ref, `git revert --no-commit` the native commits listed in
# release/native-commits.txt. The resulting index is the wasm-only tree, so both patches are exact diffs
# between real trees and (a)+(b) == base->head by construction.
#
# Usage: release/make-patches.sh [-f FORK] [-b BASE] [-h HEADREF] [-o OUTDIR]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
FORK="$ROOT/vendor/convex-backend"
BASE=342530f
HEADREF=use-rust
OUT="$HERE/out"
while getopts "f:b:h:o:" o; do
  case $o in f) FORK=$OPTARG;; b) BASE=$OPTARG;; h) HEADREF=$OPTARG;; o) OUT=$OPTARG;; *) exit 2;; esac
done
# Native-only paths (wasm patch must contain none of these).
NATIVE_PATHS_RE='^(crates/isolate/src/environment/rust/native/|crates/application/src/application_function_runner/rust_analyze/native\.rs|npm-packages/convex/src/cli/lib/rust/native(\.test)?\.ts|crates/local_backend/src/rust_native\.rs)'
# Budget from DESIGN §1.4: upstream lines the native worker may add outside Cargo/knobs.
BUDGET_NATIVE_UPSTREAM_SRC="main.rs +3, router.rs +1, lib.rs +1 (local_backend), plus Cargo.toml dep lines"

mkdir -p "$OUT"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/make-patches.XXXXXX")"
WT="$TMP/wt"
cleanup() { git -C "$FORK" worktree remove --force "$WT" >/dev/null 2>&1 || true; rm -rf "$TMP"; }
trap cleanup EXIT

BASE_SHA="$(git -C "$FORK" rev-parse "$BASE^{commit}")"
HEAD_SHA="$(git -C "$FORK" rev-parse "$HEADREF^{commit}")"
git -C "$FORK" worktree add --detach "$WT" "$HEAD_SHA" >/dev/null 2>&1
G=(git -C "$WT" -c user.name=release -c user.email=release@localhost)

# Guard: every commit in range that touches a native-only path must be listed.
LIST="$(grep -vE '^\s*(#|$)' "$HERE/native-commits.txt" | awk '{print $1}')"
for c in $(git -C "$FORK" rev-list "$BASE_SHA..$HEAD_SHA"); do
  if git -C "$FORK" diff-tree --no-commit-id --name-only -r "$c" | grep -qE "$NATIVE_PATHS_RE"; then
    short="$(git -C "$FORK" rev-parse --short "$c")"
    echo "$LIST" | grep -q "^${short:0:7}" || { echo "ERROR: $short touches native paths but is not in native-commits.txt" >&2; exit 1; }
  fi
done

# Revert newest-first (history order, not file order).
REVERT=()
for c in $(git -C "$FORK" rev-list "$BASE_SHA..$HEAD_SHA"); do
  s="$(git -C "$FORK" rev-parse --short=7 "$c")"
  echo "$LIST" | grep -q "^$s" && REVERT+=("$c")
done
for c in "${REVERT[@]}"; do
  "${G[@]}" revert --no-commit "$c" >/dev/null 2>&1 || { echo "ERROR: revert of $c conflicts; native-commits.txt needs a manual split" >&2; exit 1; }
done

# Mixed commits (release/mixed-commits.txt): reverted above with the native ones, re-applied here.
MIXED="$(grep -vE '^\s*(#|$)' "$HERE/mixed-commits.txt" 2>/dev/null | awk '{print $1}' || true)"
MIXED_PICKED=0
for c in $(git -C "$FORK" rev-list --reverse "$BASE_SHA..$HEAD_SHA"); do
  s="$(git -C "$FORK" rev-parse --short=7 "$c")"
  echo "$MIXED" | grep -q "^$s" || continue
  # Re-apply only the commit's shared files (its native-only paths stay reverted) with a 3-way apply. A file that
  # still conflicts (its hunks sit next to native lines that were reverted, e.g. tests appended after native
  # tests) keeps the wasm tree's version and takes the commit's hunks with one line of context.
  shared="$(git -C "$FORK" diff-tree --no-commit-id --name-only -r "$c" | grep -vE "$NATIVE_PATHS_RE" || true)"
  # shellcheck disable=SC2086
  git -C "$FORK" diff --binary --full-index "$c^" "$c" -- $shared | "${G[@]}" apply --index --3way >/dev/null 2>&1 || true
  for f in $("${G[@]}" diff --name-only --diff-filter=U); do
    "${G[@]}" checkout --ours -- "$f" && "${G[@]}" add -- "$f"
    git -C "$FORK" diff --full-index "$c^" "$c" -- "$f" | "${G[@]}" apply --index -C1 >/dev/null 2>&1 \
      || { echo "ERROR: mixed commit $s: $f does not re-apply on the wasm tree; needs a manual split" >&2; exit 1; }
    echo "note: mixed commit $s: $f re-applied with reduced context" >&2
  done
  if "${G[@]}" ls-files -u | grep -q .; then echo "ERROR: mixed commit $s left conflicts" >&2; exit 1; fi
  MIXED_PICKED=$((MIXED_PICKED+1))
done

WASM_TREE="$("${G[@]}" write-tree)"
DIFFOPTS=(--binary --full-index)
"${G[@]}" diff --cached "${DIFFOPTS[@]}" "$BASE_SHA" > "$OUT/convex-rust-wasm.patch"
"${G[@]}" diff --cached "${DIFFOPTS[@]}" "$BASE_SHA" -- . ':!Cargo.lock' > "$OUT/convex-rust-wasm-nolock.patch"
"${G[@]}" diff "${DIFFOPTS[@]}" "$WASM_TREE" "$HEAD_SHA" > "$OUT/convex-rust-native.patch"

"${G[@]}" diff "${DIFFOPTS[@]}" "$WASM_TREE" "$HEAD_SHA" -- . ':!Cargo.lock' > "$OUT/convex-rust-native-nolock.patch"

# Sanity: wasm patch has no native-only path.
if grep -E '^diff --git a/' "$OUT/convex-rust-wasm.patch" | sed 's|^diff --git a/\([^ ]*\) .*|\1|' | grep -E "$NATIVE_PATHS_RE"; then
  echo "ERROR: native-only files leaked into the wasm patch" >&2; exit 1
fi

# ---- summary ----
files() { grep -E '^diff --git a/' "$1" | sed 's|^diff --git a/\([^ ]*\) .*|\1|'; }
numstat() { "${G[@]}" diff --cached --numstat "$@"; }
is_upstream() { git -C "$FORK" cat-file -e "$BASE_SHA:$1" 2>/dev/null; }

{
echo "# Patch summary"
echo
echo "- base: \`$(git -C "$FORK" rev-parse --short "$BASE_SHA")\` ($(git -C "$FORK" log -1 --format=%s "$BASE_SHA"))"
echo "- head: \`$(git -C "$FORK" rev-parse --short "$HEAD_SHA")\` ($HEADREF)"
echo "- native commits reverted for the wasm tree: ${#REVERT[@]} (of which $MIXED_PICKED mixed, re-applied: see release/mixed-commits.txt)"
echo "- generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo
for pair in "wasm:base-to-wasm" "native:wasm-to-head"; do
  name="${pair%%:*}"
  if [ "$name" = wasm ]; then NS="$(numstat "$BASE_SHA")"; else NS="$("${G[@]}" diff --numstat "$WASM_TREE" "$HEAD_SHA")"; fi
  echo "## convex-rust-$name.patch"
  echo
  echo "Patch file size: $(wc -c < "$OUT/convex-rust-$name.patch" | tr -d ' ') bytes"
  echo
  echo "| file | + | - | kind |"; echo "|---|---:|---:|---|"
  up_files=0; up_add=0; up_del=0; new_files=0; new_add=0
  while IFS=$'\t' read -r a d f; do
    [ -z "$f" ] && continue
    if [ "$name" = wasm ]; then ref="$BASE_SHA"; else ref="$WASM_TREE"; fi
    if git -C "$FORK" cat-file -e "$ref:$f" 2>/dev/null; then
      if is_upstream "$f"; then k="upstream-touched"; else k="ours (modified)"; fi
    else k="new"; fi
    echo "| \`$f\` | $a | $d | $k |"
    if [ "$k" = "upstream-touched" ]; then
      up_files=$((up_files+1)); [ "$a" != - ] && up_add=$((up_add+a)); [ "$d" != - ] && up_del=$((up_del+d))
    elif [ "$k" = new ]; then new_files=$((new_files+1)); [ "$a" != - ] && new_add=$((new_add+a))
    fi
  done <<< "$NS"
  echo
  echo "Totals: $(echo "$NS" | wc -l | tr -d ' ') files; upstream-touched $up_files files (+$up_add/-$up_del); new files $new_files (+$new_add)."
  echo
  echo "Upstream-touched, excluding Cargo.lock and tests:"
  echo
  echo '```'
  echo "$NS" | awk -F'\t' '{print $3"\t+"$1"/-"$2}' | while IFS=$'\t' read -r f c; do
    [ "$f" = Cargo.lock ] && continue
    is_upstream "$f" || continue
    case "$f" in *test*|*/tests.rs) continue;; esac
    echo "$c  $f"
  done
  echo '```'
  echo
done
echo "## Budget (DESIGN §1.4)"
echo
echo "Native design budget for upstream source lines: $BUDGET_NATIVE_UPSTREAM_SRC."
echo
echo "Native patch, upstream-touched source files (non-Cargo, non-lock, non-test):"
echo
echo '```'
"${G[@]}" diff --numstat "$WASM_TREE" "$HEAD_SHA" | while IFS=$'\t' read -r a d f; do
  case "$f" in Cargo.*|*/Cargo.toml|*test*|*/tests.rs) continue;; esac
  git -C "$FORK" cat-file -e "$BASE_SHA:$f" 2>/dev/null || continue
  # files that exist in base (true upstream) or only in the wasm tree (ours)
  echo "+$a/-$d  $f"
done
echo '```'
echo
echo "Files changed by BOTH patches (wasm and native code mixed; see docs/rust/self-hosted-patch.md):"
echo
echo '```'
comm -12 <(files "$OUT/convex-rust-wasm.patch" | sort) <(files "$OUT/convex-rust-native.patch" | sort)
echo '```'
} > "$OUT/SUMMARY.md"

echo "wrote $OUT:"; ls -l "$OUT"

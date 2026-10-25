# Filing the upstream pull request (you run these; nothing here has been pushed)


Prerequisites: `git`, `gh` (logged in), a GitHub fork of `get-convex/convex-backend` under your account, and the
patches built by `release/make-patches.sh` (they land in `release/out/`). Regenerate first if the fork has
moved:

```sh
cd /path/to/convex-rs
release/make-patches.sh
cat release/out/SUMMARY.md          # check the numbers in native-pr/PR-BODY.md still match
release/verify-patches.sh -s /some/scratch/dir    # git apply --check/apply of each patch on a clean 342530f worktree
```

`make-patches.sh` builds the wasm tree by reverting the commits in `release/native-commits.txt` and then
re-applying the ones in `release/mixed-commits.txt` (currently the HTTP-action commit `ffa6f94`, which is wasm
and native). If a new fork commit touches native-only paths the script stops and asks for it to be listed; if a
mixed commit no longer re-applies it stops and a manual split is needed. `verify-patches.sh -c` also runs
`cargo check` (needs 8 GB free).

## 1. Fork and clone

```sh
export GH_USER=<your-github-login>
gh repo fork get-convex/convex-backend --clone=false          # once
git clone git@github.com:$GH_USER/convex-backend.git /path/to/pr-work
cd /path/to/pr-work
git remote add upstream https://github.com/get-convex/convex-backend.git
git fetch upstream
```

## 2. Branch from the base the patches were made against

The patches are exact diffs against upstream commit `342530f`. Start there (it applies without fuzz), then rebase
onto current `main` as a separate step so conflicts are visible:

```sh
git switch -c rust-native-worker 342530f
export P=/path/to/convex-rs/release/out

git apply --index --check "$P/convex-rust-wasm.patch" && git apply --index "$P/convex-rust-wasm.patch"
git commit -m "isolate, application, cli: wasm runtime for \"use rust\" actions"

git apply --index --check "$P/convex-rust-native.patch" && git apply --index "$P/convex-rust-native.patch"
git commit -m "isolate, application, local_backend, cli: opt-in native worker process for \"use rust\" actions"
```

Two commits on purpose: the second one is the review target.

## 3. Optional: move to current upstream main

```sh
git rebase upstream/main
# Cargo.lock conflicts: take upstream's, then `cargo update -w --offline=false` (or plain `cargo check`) and amend.
# Use the -nolock patches instead if you prefer to avoid lock conflicts:
#   convex-rust-wasm-nolock.patch and convex-rust-native-nolock.patch, then run `cargo check -p isolate` once to
#   refresh Cargo.lock and commit it.
```

Checked on 2026-10-25 against `upstream/main` `bb31399` (16 commits after the base): both `-nolock` patches apply
with plain `git apply` (no 3-way needed), and `cargo check -p isolate -p application -p local_backend` passes after
**one manual fix**. Upstream commit 868554b ("Add function.runtime to log stream events") added a `match` on `ModuleEnvironment`
that does not know the new variant. Add this arm to `crates/common/src/log_streaming.rs` (`FunctionEventSource::to_json_map`)
in the wasm commit:

```rust
            ModuleEnvironment::Rust => Some("rust"),
```

## 4. Build and test before pushing

```sh
export CONVEX_PREBUILT_JS=1 SODIUM_USE_PKG_CONFIG=1
cargo check -p isolate -p application -p local_backend
cargo test -p isolate rust
cargo test -p application rust_analyze
(cd npm-packages/convex && npx vitest run src/cli/lib/rust && npx tsc --noEmit --emitDeclarationOnly false)
```

Put the results into the Testing section of `release/native-pr/PR-BODY.md`.
Also run the Linux sandbox checks (the known gap listed in the PR body) if you can reach a Linux host, and update
the "Platform status" section accordingly. If `git apply` of the patches on current `upstream/main` fails in step 3,
the usual sites are `knobs.rs`, `application_function_runner/mod.rs` and the CLI `dev.ts`/`deploy.ts`/`codegen.ts`
hunks; the `-nolock` patches plus `git apply --3way` localise it.

## 5. Push to your fork and open the PR

```sh
git push -u origin rust-native-worker

gh pr create \
  --repo get-convex/convex-backend \
  --base main \
  --head "$GH_USER:rust-native-worker" \
  --draft \
  --title "Opt-in native worker process for \"use rust\" actions (stacked on wasm runtime)" \
  --body-file /path/to/convex-rs/release/native-pr/PR-BODY.md
```

Then attach `docs/rust-native-runtime-design.md`: either paste it as the first comment, or commit it as
`docs/rust-native-design.md` on the branch (one more commit) and link it from the description.

## Alternative: file the wasm runtime first

If maintainers would rather review the wasm runtime on its own, push a branch with only the first commit, open a
PR with it, and file the native PR later with the second commit rebased on top:

```sh
git switch -c rust-wasm-runtime rust-native-worker~1
git push -u origin rust-wasm-runtime
gh pr create --repo get-convex/convex-backend --base main --head "$GH_USER:rust-wasm-runtime" --draft \
  --title "\"use rust\" actions: wasm runtime" --body-file /path/to/convex-rs/release/native-pr/PR-BODY-wasm.md
```

Remember the PR bodies (which end with the session link) is the file to edit if any of the numbers change.

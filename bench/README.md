# bench

Scripts in `scripts/`:

- `start-backend.sh [port] [site-port]`: local fork backend (data dir `bench/.backend-data`, override with `BENCH_DATA_DIR`).
- `cvx.sh dev --once | run ... | env set ...`: fork CLI against that backend.

## Rust build profile

Benchmarks must run a release wasm. `cvx.sh` sets `CONVEX_RUST_PROFILE=release` by default, which makes
`convex dev` build the full release profile and run `wasm-opt -O3` (no `-g`; panic file:line comes from
`core::panic::Location` data, not the name section). Without the variable the CLI uses the fast
`convex-dev` profile (opt-level 1), which is not representative. Set `CONVEX_RUST_PROFILE=dev` to opt out.

After every `cvx.sh dev ...` the script writes `bench/convex/.build-info.json` (gitignored): profile,
wasm-opt version and flags, wasm size before and after wasm-opt. Copy it into a run's meta.json.

## rust-native runtime (WN-13)

A third runtime next to `node` and `rust-wasm`: the same `rust/...` function paths, pushed with
`--rust-runtime native` and `CONVEX_RUST_PROFILE=release`, served by a second backend started with
`RUST_NATIVE_MODE=worker`. node and rust-wasm share backend A (port P); rust-native runs on backend B
(port P+10, data dir `<BENCH_DATA_DIR>-native`, own secret and admin key). `BENCH_NATIVE=1` makes
`start-backend.sh` and `cvx.sh` target B; the runner sets it itself.

- `phase1 [--runtimes node,rust,rust-native] [--native-url U] [--native-admin-key K]`: three-way equality. A
  failing native call (for example before the native runtime exists) is reported per case in the table
  (`ERROR`/`MISMATCH` plus the reason) and never aborts the run. Columns: node, wasm, nat.
- `phase2 --runtimes ... [--push] [--native-url U]`: adds native rows to phase2.jsonl/rss.jsonl (runtime name
  `rust-native`; wasm keeps the name `rust`) and a `native-worker` RSS class (descendants of backend B that
  run the backend binary). The summary prints W/J (wasm/node), N/J (native/node), N/W (native/wasm) ratios.
- meta.json gains `build.wasm` (profile, wasm-opt, sizes), `build.native` (profile, triple, bytes, sha256,
  panic, strip; from `.build-info-native.json`) and `backend_native`.
- `gates [--only overhead,spawn,idle-rss,crypto,orphans] [--calls N] [--spawns N] [--push] [--external]`: WN-14
  gate measurements, written to `results/<run-id>/gates.json` with a PASS/FAIL/NO DATA verdict each:
  base-001 runtime overhead (exec - user_exec) p50 <= 0.1 ms Linux / 0.2 ms macOS, spawn to Ready (first call after SIGKILL of the workers minus warm p50)
  <= 30 ms Linux / 50 ms macOS, idle worker RSS <= 20 MB, crypto-003 and AES-GCM native/node <= 0.7, no
  orphans after SIGTERM/SIGKILL of the backend.

Report (`bench_report <run-dir>`): a "Gates" section after Headline shows `gates.json` (gate, value, threshold,
PASS/FAIL/NO DATA; without the file it says "gates not measured"). For runs with rust-native rows the Headline
and the category/library tables gain Native/Node and Native/Wasm geometric means (warm and cold, no CI), and
the per-case tables show native latency, ratios, worker RSS and failures. Concurrency tables stay node vs wasm.

Measurement fixes versus v1: log entries are matched by request id on the root execution only (entries with
a `parentExecutionId`, i.e. child UDFs, are ignored); concurrency counts overload rejections
(`TooManyConcurrentRequests`) separately from failures; meta.json records the real build profile.

## Convex cloud target (WV-04)

Compares three runtimes on a real Convex cloud deployment: `node` ("use node" TS cases), `ts-v8` (the same TS
case code in the default runtime) and `rust-v8` (the Rust cases as wasm via the `convex-rust` package).
Project: `bench/cloud/` (its own `package.json`, stock published `convex`, `convex-rust` installed from an
`npm pack` tarball with `--install-links`).

```sh
export CONVEX_DEPLOY_KEY=$(cat <file>)            # never commit or print it
cd bench/cloud && npm install
./build-rust.sh                                    # convex-rust build -> convex/rustv8 (rustv8/<module>:<name>)
node gen.mjs && ./push.sh                          # convex/node (copy), convex/v8 (generated), then npx convex deploy
cd .. && bench_runner phase1 --target cloud        # node vs ts-v8 vs rust-v8 equality
bench_runner phase2 --target cloud --quick --run-id cloud-quick-<date> --iters 20 --conc-min 15
bench_report results/cloud-quick-<date>            # report.md / report.html / summary.json
```

* No local backend is started or stopped. POST /api/action with the deploy key as admin key.
* Timing: client round trip, plus server-side `executionTime` from `GET /api/stream_function_logs` (Completion
  entries matched by `Convex-Request-Id`). Both are reported.
* `net` is skipped (needs a localhost mock server); recorded in `meta.json` (`cloud.skipped_categories`).
* Cold = first call per function after a fresh deploy: `redeploy.sh <nonce>` appends a nonce constant to every
  function module (new module hashes, new isolates), once per `--cold` round (about 2 minutes each).
* ts-v8 is generated by `gen.mjs` from the node modules (TypeScript AST closure extraction, one source per case).
  Cases that need Node built-ins/native addons/Buffer, exceed the 64 MB isolate limit or need `new Function` are
  n/a (`cloud/v8-na.json`, reasons in the report); `cloud/overrides/*.ts` supply web-standard replacements
  (crypto.subtle etc.) where trivial. Cases where the Node action itself OOMs at 512 MB are n/a for node
  (`cloud/node-manual-na.json`).

## Final report (2026-10-25)

`bench/reports/final-2026-10-25/report.html` (self-contained; `report.md` is the short summary) merges the final
runs: local node vs rust-native (`final2-local-*`, the `nodena` reruns fill node rows that were skipped in the chunk
runs) and cloud node vs ts-v8 vs rust-v8 (`final2-cloud`; warm and concurrency from `data-014` onward are "not measured"
because the deployment was disabled for exceeding free-plan limits). Primary metric is end-to-end client time;
server exec is a secondary cloud view. Regenerate:

```sh
bench_report final --out bench/reports/final-2026-10-25 \
  --local-warm results/final2-local-base ... --local-warm results/final2-local-nodena \
  --local-cold results/final2-local-cold --local-cold results/final2-local-nodena-cold \
  --local-phase1 results/final2-local-phase1 \
  --local3 results/final3-local-base ... --local3 results/final3-local-net \
  --local3-cold results/final3-local-cold --local3-phase1 results/final3-local-phase1 \
  --cloud results/final2-cloud \
  --cloud-phase1 results/phase1-cloud-1792882868 --gates results/final2-local-base/gates.json \
  --gates-recheck results/final2-idlerss-1 ...
```

Later `--local-warm` / `--local-cold` dirs supersede earlier ones per cell when they hold valid samples. Headline: native/node
median per-case ratio 0.43 warm, 0.37 cold; cloud rust-v8/node 0.55 warm, 0.34 cold; rust-v8 and ts-v8 are on par.

`--local3`, `--local3-cold`, `--local3-phase1` add the same-machine three-way section (node vs Rust wasm vs Rust
native, `final3-local-*`: one backend serving all three runtimes, warm concurrency 1 with 50 iterations, cold 2 rounds).
Its ratios are numerator/denominator times, so above 1 means the denominator is faster. Median per-case client-time
ratios over 100 cases: node/wasm 1.86 warm, 2.30 cold; node/native 2.43 warm, 2.90 cold; wasm/native 1.39 warm, 1.35 cold.
Native is more than 5% slower than wasm on 12 cases warm and 21 cold (mostly `cb` and `net`).

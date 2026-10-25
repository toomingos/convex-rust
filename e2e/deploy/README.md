# e2e/deploy

Deploy-path tests for "use rust": what a developer hits when pushing.

```sh
cd e2e/deploy
npm test                              # all scenarios (starts/kills its own backend on 3240/3241)
npm test -- test/04-05.test.mjs       # one file
DEPLOY_DUMP=1 npm test -- test/06-09.test.mjs   # print the full CLI output of each push
```

`run.mjs` starts the prebuilt backend with a fresh data dir under `.run/`, runs `node --test`
serially, then kills the backend. Each scenario copies `fixtures/base` to a temp dir, links the
fork `convex` package into `node_modules`, runs `convex rust init` (with `CONVEX_FN_PATH`) and
then the fork CLI (`bin/main.js`, the bundled CLI) with `dev --once` / `run` / `deploy`.
Cargo output goes to `.cache/target` (shared, so dependencies compile once).

Scenarios: 1 fresh init, 2 compile error, 3 redeploy, 4 add/remove/rename action+module,
5 internal action, 6 crate-root action, 7 import allow-list, 8 missing toolchain/target/cargo,
9 TS importing `./rust/...`, 10 bench-crate push size/time (measurement only; slow, cold cargo
cache = many minutes; writes `.run/bench-push.json`), 1b rust-toolchain.toml honoured (cwd = crate dir).


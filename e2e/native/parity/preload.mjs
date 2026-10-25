// Preload (node --import) that runs the unmodified e2e/spec suite in native mode:
//  - every `convex dev` the spec harness runs gets `--rust-runtime native`;
//  - the backend the harness spawns gets RUST_NATIVE_MODE=worker and runs with cwd = $PARITY_DATA
//    (its own data dir, so sqlite + storage + rust-native artifacts never touch e2e/spec).
// e2e/spec is not edited. Only the tiny admin-key/bin dir the harness itself creates lands there.
import cp from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { mkdirSync } from "node:fs";

const data = process.env.PARITY_DATA;
if (!data) throw new Error("PARITY_DATA not set");
mkdirSync(data, { recursive: true });

const isCli = (cmd, args) => cmd === "node" && typeof args?.[0] === "string" && args[0].endsWith("bin/main.js");
const origSync = cp.spawnSync;
cp.spawnSync = function (cmd, args, opts) {
  if (isCli(cmd, args) && args.includes("dev") && !args.includes("--rust-runtime")) {
    args = [...args, "--rust-runtime", "native"];
  }
  return origSync.call(this, cmd, args, opts);
};
const origSpawn = cp.spawn;
cp.spawn = function (cmd, args, opts) {
  if (typeof cmd === "string" && cmd.endsWith("convex-local-backend")) {
    opts = {
      ...opts,
      cwd: data,
      env: { ...opts.env, RUST_NATIVE_MODE: "worker", ...(process.platform === "linux" ? {} : { RUST_NATIVE_ALLOW_UNSANDBOXED: "true" }), ...(process.env.PARITY_BACKEND_ENV ? JSON.parse(process.env.PARITY_BACKEND_ENV) : {}) },
    };
  }
  return origSpawn.call(this, cmd, args, opts);
};
syncBuiltinESMExports();

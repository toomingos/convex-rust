// Preload (node --import) that runs the unmodified e2e/spec suite against the rust-v8 runtime.
//  - the harness' `convex ...` CLI calls (cwd = e2e/spec) run the STOCK npm CLI (convex@1.48.0) in
//    $V8_APP (the generated app: spec TS sources + convex-rust generated wrappers in convex/rust);
//  - the harness' backend data dir (e2e/spec/.backend-data) is redirected to $V8_DATA, so nothing is
//    created, removed or read in e2e/spec;
//  - the backend runs with cwd = $V8_DATA.
// e2e/spec is not edited.
import cp from "node:child_process";
import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { join, resolve } from "node:path";

const app = process.env.V8_APP;
const data = process.env.V8_DATA;
if (!app || !data) throw new Error("V8_APP / V8_DATA not set");
fs.mkdirSync(data, { recursive: true });

const specDir = resolve(import.meta.dirname, "../../spec");
const origData = join(specDir, ".backend-data");
const mapPath = (p) => {
  if (typeof p !== "string") return p;
  const r = resolve(p);
  return r === origData || r.startsWith(origData + "/") ? data + r.slice(origData.length) : p;
};
for (const fn of ["rmSync", "mkdirSync", "writeFileSync", "symlinkSync", "readFileSync", "existsSync"]) {
  const orig = fs[fn];
  fs[fn] = function (p, ...rest) {
    // symlinkSync(target, path): the second argument is the link location.
    if (fn === "symlinkSync") return orig.call(this, p, mapPath(rest[0]), ...rest.slice(1));
    return orig.call(this, mapPath(p), ...rest);
  };
}

const stockCli = join(app, "node_modules/convex/bin/main.js");
const origSync = cp.spawnSync;
cp.spawnSync = function (cmd, args, opts) {
  if (cmd === "node" && typeof args?.[0] === "string" && args[0].endsWith("bin/main.js") && opts?.cwd === specDir) {
    args = [stockCli, ...args.slice(1)];
    opts = { ...opts, cwd: app };
  }
  return origSync.call(this, cmd, args, opts);
};
const origSpawn = cp.spawn;
cp.spawn = function (cmd, args, opts) {
  if (typeof cmd === "string" && cmd.endsWith("convex-local-backend")) {
    opts = { ...opts, cwd: data, env: { ...opts.env, PATH: String(opts.env.PATH).replace(origData, data) } };
  }
  return origSpawn.call(this, cmd, args, opts);
};
syncBuiltinESMExports();

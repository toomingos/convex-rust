#!/usr/bin/env node
// Runner for the rust-v8 runtime ("use rust" in a normal Convex action via the convex-rust package).
// Runs the unmodified e2e/spec suite (47 tests) against it. Usage: node run.mjs [--keep] [--skip-build]
//   1. app/ = spec TS sources (data, nodeSpec, schema, http) + convex-rust generated wrappers in convex/rust
//      (stock CLI convex@1.48.0, no fork code involved in the push)
//   2. `convex-rust build --crate e2e/spec/convex/rust --out app/convex/rust --env SPEC_ENV,SPEC_ENV_UNI` (env vars are passed by name)
//   3. node --test e2e/spec/test/spec.test.ts with parity/preload.mjs (stock CLI + own data dir)
// Ports 3380/3381, mock 3388, data dir $V8_TMP (default <os tmpdir>/convex-rs-e2e-v8).
import { spawn, spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const V8_DIR = dirname(fileURLToPath(import.meta.url));
const REPO = join(V8_DIR, "../..");
const SPEC_DIR = join(REPO, "e2e/spec");
const APP = join(V8_DIR, "app");
const PORT = Number(process.env.SPEC_PORT ?? 3380);
const MOCK_PORT = Number(process.env.SPEC_MOCK_PORT ?? 3388);
const TMP = process.env.V8_TMP ?? join(tmpdir(), "convex-rs-e2e-v8");
const argv = process.argv.slice(2);
const sh = (cmd, args, opts = {}) => spawnSync(cmd, args, { stdio: "inherit", ...opts });
const brief = (e) => {
  const lines = String(e).split("\n").map((l) => l.trim()).filter(Boolean);
  const marked = lines.filter((l) => /^✖|Error|error:/.test(l) && !/^at /.test(l));
  return (marked.length ? marked : lines).slice(0, 4).join(" | ").slice(0, 400);
};

function setup() {
  if (!existsSync(join(REPO, "target-backend/debug/convex-local-backend"))) throw new Error("debug backend not built (target-backend/debug)");
  if (!existsSync(join(REPO, "packages/convex-rust/dist/cli/bin.js"))) throw new Error("packages/convex-rust is not built (npm run build)");
  if (!existsSync(join(V8_DIR, "node_modules/convex/package.json")) || !existsSync(join(V8_DIR, "node_modules/convex-rust/package.json"))) {
    console.log("== npm install (e2e/v8) ==");
    if (sh("npm", ["install", "--no-audit", "--no-fund"], { cwd: V8_DIR }).status !== 0) throw new Error("npm install failed");
  }
  // Refresh the installed copy of convex-rust from the package's current dist (npm install-links copies
  // instead of symlinking, so the bundle has ONE convex/values; a symlink would pull in the package's
  // own devDependency copy and `instanceof ConvexError` in the host would fail).
  for (const f of ["dist", "package.json"]) {
    rmSync(join(V8_DIR, "node_modules/convex-rust", f), { recursive: true, force: true });
    cpSync(join(REPO, "packages/convex-rust", f), join(V8_DIR, "node_modules/convex-rust", f), { recursive: true });
  }
  // App dir: spec TS sources are the source of truth (copied each run); rust wrappers are generated.
  rmSync(join(APP, "convex"), { recursive: true, force: true });
  mkdirSync(APP, { recursive: true });
  cpSync(join(SPEC_DIR, "convex"), join(APP, "convex"), {
    recursive: true,
    filter: (s) => !/\/convex\/(rust|_generated)(\/|$)/.test(s),
  });
  writeFileSync(join(APP, "package.json"), JSON.stringify({ name: "e2e-v8-app", private: true, type: "module", dependencies: { convex: "1.48.0", "convex-rust": "*" } }));
  // Resolve convex / convex-rust from e2e/v8/node_modules (one install).
  if (!existsSync(join(APP, "node_modules"))) {
    mkdirSync(APP, { recursive: true });
    spawnSync("ln", ["-s", join(V8_DIR, "node_modules"), join(APP, "node_modules")]);
  }
}

function build() {
  // cargo may rewrite the spec crate's Cargo.lock; restore it afterwards.
  const lock = join(SPEC_DIR, "convex/rust/Cargo.lock");
  const before = existsSync(lock) ? readFileSync(lock) : null;
  process.once("exit", () => before && writeFileSync(lock, before));
  console.log("== convex-rust build (wasm32-wasip1) ==");
  const env = { ...process.env, CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? join(REPO, "target-e2e"), CARGO_BUILD_JOBS: "4" };
  const r = sh("node", [join(REPO, "packages/convex-rust/dist/cli/bin.js"), "build", "--crate", join(SPEC_DIR, "convex/rust"), "--out", join(APP, "convex/rust"), "--env", "SPEC_ENV,SPEC_ENV_UNI"], { cwd: APP, env });
  if (r.status !== 0) throw new Error("convex-rust build failed");
}

function runSpec() {
  return new Promise((resolve) => {
    const results = [];
    let tail = "";
    const p = spawn("node", [
      "--import", join(V8_DIR, "parity/preload.mjs"), "--test", "--test-concurrency=1", "--test-timeout=1500000",
      `--test-reporter=${join(REPO, "e2e/native/lib/reporter.mjs")}`, "test/spec.test.ts",
    ], {
      cwd: SPEC_DIR,
      env: { ...process.env, V8_APP: APP, V8_DATA: join(TMP, "data"), SPEC_PORT: String(PORT), SPEC_MOCK_PORT: String(MOCK_PORT) },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let buf = "";
    p.stdout.on("data", (d) => {
      buf += d;
      let i;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        if (line.startsWith("@@RESULT ")) {
          const r = JSON.parse(line.slice(9));
          results.push(r);
          process.stdout.write(`  ${r.status.padEnd(4)} ${r.name}${r.status === "FAIL" ? `\n         ${brief(r.error)}` : ""}\n`);
        }
      }
    });
    p.stderr.on("data", (d) => (tail = (tail + d).slice(-4000)));
    p.on("close", (code) => {
      if (results.length === 0 || (code !== 0 && !results.some((r) => r.status === "FAIL")))
        results.push({ name: "(suite setup / crash)", status: "FAIL", error: `exit ${code}; ${tail.trim().split("\n").slice(-6).join(" | ")}` });
      const sf = results.find((r) => r.name.startsWith("(suite)") && r.status === "FAIL");
      if (sf) for (const r of results) if (r !== sf && r.status === "FAIL" && /cancelled/.test(String(r.error))) r.error = "not run (suite setup failed)";
      resolve(results);
    });
  });
}

let table = [];
try {
  if (!argv.includes("--skip-build")) { setup(); build(); } else setup();
  mkdirSync(TMP, { recursive: true });
  console.log(`rust-v8; ports ${PORT}/${PORT + 1}, mock ${MOCK_PORT}; tmp ${TMP}`);
  const hadData = existsSync(join(SPEC_DIR, ".backend-data"));
  const allowed = JSON.parse(readFileSync(join(V8_DIR, "parity/allowed.json"), "utf8")).allowed;
  for (const r of await runSpec()) {
    const a = r.status === "FAIL" && allowed.find((x) => r.name.includes(x.match));
    table.push(a ? { ...r, allowed: a.reason } : r);
  }
  void hadData;
} catch (e) {
  console.error("RUNNER ERROR:", e.message);
  table.push({ name: "runner", status: "FAIL", error: e.message });
}
console.log("\n" + "=".repeat(100) + "\nSTATUS        TEST\n" + "-".repeat(100));
for (const r of table) console.log(`${(r.allowed ? "ALLOWED-DIFF" : r.status).padEnd(12)}  ${r.name}${r.status === "SKIP" ? `   [${r.skip}]` : ""}`);
console.log("-".repeat(100));
const c = (s) => table.filter((r) => (s === "ALLOWED-DIFF" ? r.allowed : !r.allowed && r.status === s)).length;
const ok = c("PASS");
console.log(`PASS ${c("PASS")}  FAIL ${c("FAIL")}  SKIP ${c("SKIP")}  ALLOWED-DIFF ${c("ALLOWED-DIFF")}   (total ${table.length}; target 45/47 pass)`);
const first = table.find((r) => r.status === "FAIL" && !r.allowed);
if (first) console.log(`\nFIRST FAILURE: ${first.name}\n  ${brief(first.error)}`);
for (const r of table.filter((r) => r.allowed)) console.log(`ALLOWED-DIFF [${r.name}]: ${r.allowed}`);
if (!argv.includes("--keep")) rmSync(TMP, { recursive: true, force: true });
process.exit(table.some((r) => r.status === "FAIL" && !r.allowed) ? 1 : 0);

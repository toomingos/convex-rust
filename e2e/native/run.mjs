#!/usr/bin/env node
// Runner for the rust-native e2e suites (WN-12). Usage:
//   node run.mjs [--only <substr>] [--parity-only] [--skip-parity] [--list] [--keep]
// Starts its own backends (ports 3360/3361, mock 3368), one per suite, in its own temp data dir
// ($E2E_NATIVE_TMP, default <os tmpdir>/convex-rs-e2e-native), and removes it at the end.
// Prints a PASS/FAIL/SKIP table; exits 1 on any FAIL.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { BACKEND_BIN, MOCK_PORT, NATIVE_DIR, PORT, REPO, TMP_ROOT } from "./lib/harness.mjs";

const argv = process.argv.slice(2);
const flag = (n) => argv.includes(n);
const only = argv.includes("--only") ? argv[argv.indexOf("--only") + 1] : null;
const SPEC_DIR = join(REPO, "e2e/spec");
const FORK = process.env.CONVEX_BACKEND_DIR ?? join(REPO, "vendor/convex-backend"); // the Convex backend fork clone
const sh = (cmd, args, opts = {}) => spawnSync(cmd, args, { stdio: "inherit", ...opts });

// ---- prerequisites ---------------------------------------------------------
function prereqs() {
  const df = spawnSync("df", ["-g", "/System/Volumes/Data"], { encoding: "utf8" }).stdout?.split("\n")[1]?.split(/\s+/)[3];
  if (df && Number(df) < 5) throw new Error(`only ${df} GB free; stopping (< 5 GB)`);
  if (!existsSync(join(BACKEND_BIN, "convex-local-backend")) || !existsSync(join(BACKEND_BIN, "generate_key"))) {
    console.log("== building the backend (docs/rust/self-hosted-fork.md) ==");
    const r = sh("cargo", ["build", "-p", "local_backend", "--bin", "convex-local-backend", "-p", "keybroker", "--bin", "generate_key", "-j", "4"], {
      cwd: FORK,
      env: { ...process.env, CARGO_TARGET_DIR: join(REPO, "target-backend"), CARGO_PROFILE_DEV_DEBUG: "0", CARGO_INCREMENTAL: "0", SODIUM_USE_PKG_CONFIG: "1", CONVEX_PREBUILT_JS: "1" },
    });
    if (r.status !== 0) throw new Error("backend build failed");
  }
  const bundle = join(FORK, "npm-packages/convex/dist/cli.bundle.cjs");
  if (!existsSync(bundle)) {
    console.log("== building the fork CLI ==");
    if (sh("node", ["scripts/build.cjs", "standalone-cli", "tempDir=dist"], { cwd: join(FORK, "npm-packages/convex") }).status !== 0)
      throw new Error("CLI build failed");
  }
  if (!existsSync(join(NATIVE_DIR, "node_modules/convex/package.json"))) {
    console.log("== npm install (e2e/native) ==");
    if (sh("npm", ["install", "--no-audit", "--no-fund"], { cwd: NATIVE_DIR }).status !== 0) throw new Error("npm install failed");
  }
}

const brief = (e) => {
  const lines = String(e).split("\n").map((l) => l.trim()).filter(Boolean);
  const marked = lines.filter((l) => /^✖|Error|error:/.test(l) && !/^at /.test(l));
  return (marked.length ? marked : lines).slice(0, 4).join(" | ").slice(0, 400);
};

// ---- running a node:test file ------------------------------------------------
function runFile(cwd, file, { env = {}, nodeArgs = [], label }) {
  return new Promise((resolve) => {
    const results = [];
    let tail = "";
    const p = spawn("node", [
      ...nodeArgs, "--test", "--test-concurrency=1", "--test-timeout=1500000",
      `--test-reporter=${join(NATIVE_DIR, "lib/reporter.mjs")}`, file,
    ], { cwd, env: { ...process.env, E2E_NATIVE_TMP: TMP_ROOT, ...env }, stdio: ["ignore", "pipe", "pipe"] });
    let buf = "";
    p.stdout.on("data", (d) => {
      buf += d;
      let i;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        if (line.startsWith("@@RESULT ")) {
          const r = JSON.parse(line.slice(9));
          results.push({ ...r, suite: label });
          process.stdout.write(`  ${r.status.padEnd(4)} ${r.name}${r.status === "FAIL" ? `\n         ${brief(r.error)}` : ""}\n`);
        }
      }
    });
    p.stderr.on("data", (d) => (tail = (tail + d).slice(-4000)));
    p.on("close", (code) => {
      if (results.length === 0 || (code !== 0 && !results.some((r) => r.status === "FAIL")))
        results.push({ suite: label, name: "(suite setup / crash)", status: "FAIL", error: `exit ${code}; ${tail.trim().split("\n").slice(-6).join(" | ")}`, ms: 0 });
      // A failed suite (before-hook) first, then its cancelled children with a short reason.
      const suiteFail = results.find((r) => r.name.startsWith("(suite)") && r.status === "FAIL");
      if (suiteFail) {
        for (const r of results) if (r !== suiteFail && r.status === "FAIL" && /cancelled/.test(String(r.error))) r.error = "not run (suite setup failed)";
        results.sort((a, b) => (b === suiteFail) - (a === suiteFail));
      }
      resolve(results);
    });
  });
}

function print(table, allowed) {
  const w = Math.max(...table.map((r) => r.suite.length), 5);
  console.log("\n" + "=".repeat(100));
  console.log(`${"SUITE".padEnd(w)}  STATUS        TEST`);
  console.log("-".repeat(100));
  for (const r of table) {
    const st = r.allowed ? "ALLOWED-DIFF" : r.status;
    console.log(`${r.suite.padEnd(w)}  ${st.padEnd(12)}  ${r.name}${r.status === "SKIP" ? `   [${r.skip}]` : ""}`);
  }
  console.log("-".repeat(100));
  const c = (s) => table.filter((r) => (s === "ALLOWED-DIFF" ? r.allowed : !r.allowed && r.status === s)).length;
  console.log(`PASS ${c("PASS")}  FAIL ${c("FAIL")}  SKIP ${c("SKIP")}  ALLOWED-DIFF ${c("ALLOWED-DIFF")}   (total ${table.length})`);
  const first = table.find((r) => r.status === "FAIL" && !r.allowed);
  if (first) console.log(`\nFIRST FAILURE: [${first.suite}] ${first.name}\n  ${brief(first.error)}`);
  for (const r of table.filter((r) => r.allowed)) console.log(`ALLOWED-DIFF [${r.name}]: ${r.allowed}`);
}

// ---- main ------------------------------------------------------------------
const table = [];
let code = 0;
try {
  if (flag("--list")) {
    for (const f of readdirSync(join(NATIVE_DIR, "test")).filter((f) => f.endsWith(".test.mjs"))) console.log(f);
    process.exit(0);
  }
  prereqs();
  mkdirSync(TMP_ROOT, { recursive: true });
  console.log(`platform ${process.platform}/${process.arch}; ports ${PORT}/${PORT + 1}, mock ${MOCK_PORT}; tmp ${TMP_ROOT}`);

  if (!flag("--parity-only")) {
    const files = readdirSync(join(NATIVE_DIR, "test")).filter((f) => f.endsWith(".test.mjs")).sort().filter((f) => !only || f.includes(only));
    for (const f of files) {
      console.log(`\n== ${f} ==`);
      table.push(...(await runFile(NATIVE_DIR, join("test", f), { label: f.replace(/\.test\.mjs$/, "") })));
    }
  }

  if (!flag("--skip-parity") && (!only || "parity".includes(only))) {
    console.log("\n== spec parity: e2e/spec in native mode (unmodified; parity/preload.mjs) ==");
    const hadData = existsSync(join(SPEC_DIR, ".backend-data"));
    // The CLI rewrites Cargo.lock and creates .cargo/ in the spec crate on every push; undo that.
    const lockPath = join(SPEC_DIR, "convex/rust/Cargo.lock");
    const lockBefore = existsSync(lockPath) ? readFileSync(lockPath) : null;
    const hadCargoDir = existsSync(join(SPEC_DIR, "convex/rust/.cargo"));
    const allowed = JSON.parse(readFileSync(join(NATIVE_DIR, "parity/allowed.json"), "utf8")).allowed;
    const rs = await runFile(SPEC_DIR, "test/spec.test.ts", {
      label: "parity",
      nodeArgs: ["--import", join(NATIVE_DIR, "parity/preload.mjs")],
      env: { PARITY_DATA: join(TMP_ROOT, "parity"), SPEC_PORT: String(PORT), SPEC_MOCK_PORT: String(MOCK_PORT) },
    });
    if (!hadData) rmSync(join(SPEC_DIR, ".backend-data"), { recursive: true, force: true });
    if (lockBefore) writeFileSync(lockPath, lockBefore);
    if (!hadCargoDir) rmSync(join(SPEC_DIR, "convex/rust/.cargo"), { recursive: true, force: true });
    for (const r of rs) {
      const a = r.status === "FAIL" && allowed.find((x) => r.name.includes(x.match));
      table.push(a ? { ...r, allowed: a.reason } : r);
    }
    const par = table.filter((r) => r.suite === "parity" && r.name !== "(suite setup / crash)");
    const ok = par.filter((r) => r.status === "PASS").length;
    console.log(`  spec parity: ${ok}/${par.length} pass (target 45/47: at most 2 allowed differences)`);
  }
} catch (e) {
  console.error("RUNNER ERROR:", e.message);
  table.push({ suite: "runner", name: "runner", status: "FAIL", error: e.message });
}
print(table);
if (table.some((r) => r.status === "FAIL" && !r.allowed)) code = 1;
if (!flag("--keep")) rmSync(TMP_ROOT, { recursive: true, force: true });
process.exit(code);

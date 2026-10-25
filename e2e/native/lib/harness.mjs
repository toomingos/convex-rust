// Shared helpers for the rust-native suites: backend lifecycle (own ports, own data dir), the fork
// CLI, calling actions over HTTP, process inspection. Everything started here is killed by stop().
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const NATIVE_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
export const REPO = process.env.CONVEX_RS_MAIN ?? join(NATIVE_DIR, "..", "..");
export const BACKEND_BIN = join(REPO, "target-backend/debug");
export const PORT = Number(process.env.NATIVE_PORT ?? 3360);
export const SITE_PORT = PORT + 1;
export const MOCK_PORT = Number(process.env.NATIVE_MOCK_PORT ?? 3368);
export const URL_ = `http://127.0.0.1:${PORT}`;
export const MOCK = `http://127.0.0.1:${MOCK_PORT}`;
/** Root of every data dir this suite creates; the runner deletes it at the end. */
export const TMP_ROOT = process.env.E2E_NATIVE_TMP ?? join(tmpdir(), "convex-rs-e2e-native");

export const IS_LINUX = process.platform === "linux";
/** Use as `it(name, { skip: linuxOnly("seccomp") }, ...)`: a visible SKIP on macOS. */
export const linuxOnly = (what) => (IS_LINUX ? false : `Linux only (${what}); no sandbox on ${process.platform}`);

export const INSTANCE_SECRET_ENV = "native-e2e-instance-secret-env-marker";
export const E2E_SECRET = "native-e2e-backend-secret-marker";

const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;
export const strip = (s) => s.replace(ANSI, "");
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const children = [];
let state = null; // { adminKey, dataDir, backend, profile }

function findNode() {
  const cands = [process.env.SPEC_NODE22 ?? ""];
  const nvm = join(homedir(), ".nvm/versions/node");
  if (existsSync(nvm)) for (const d of readdirSync(nvm)) cands.push(join(nvm, d, "bin/node"));
  cands.push("/usr/local/bin/node", "/opt/homebrew/opt/node@22/bin/node", "/opt/homebrew/opt/node@24/bin/node", "/usr/bin/node", process.execPath);
  for (const c of cands) {
    if (!c || !existsSync(c)) continue;
    const v = spawnSync(c, ["--version"], { encoding: "utf8" }).stdout ?? "";
    if (v.startsWith("v22.") || v.startsWith("v24.")) return c;
  }
  throw new Error("no Node 22/24 found for the backend's node executor (set SPEC_NODE22)");
}

async function waitFor(fn, what, ms = 60_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      if (await fn()) return;
    } catch {}
    await sleep(200);
  }
  throw new Error(`timed out waiting for ${what}`);
}

/**
 * Start a backend (and the mock server). `profile` names the data dir. `env` is added to the
 * backend environment; keys set to null are removed. Default: RUST_NATIVE_MODE=worker.
 * `keepData` reuses the data dir and admin key of the previous start with the same profile.
 */
export async function startBackend(profile, env = {}, { keepData = false } = {}) {
  const dataDir = join(TMP_ROOT, profile);
  const keyFile = join(dataDir, "admin-key");
  const secretFile = join(dataDir, "secret");
  let adminKey, secret;
  if (keepData && existsSync(keyFile)) {
    adminKey = readFileText(keyFile);
    secret = readFileText(secretFile);
  } else {
    rmSync(dataDir, { recursive: true, force: true });
    mkdirSync(dataDir, { recursive: true });
    secret = spawnSync("openssl", ["rand", "-hex", "32"], { encoding: "utf8" }).stdout.trim();
    adminKey = spawnSync(join(BACKEND_BIN, "generate_key"), [`native-e2e-${profile}`, secret], { encoding: "utf8" })
      .stdout.trim().split("\n").pop();
    writeFileSync(keyFile, adminKey);
    writeFileSync(secretFile, secret);
    mkdirSync(join(dataDir, "bin"), { recursive: true });
    symlinkSync(findNode(), join(dataDir, "bin", "node"));
  }
  const benv = { ...process.env, PATH: `${join(dataDir, "bin")}:${process.env.PATH}` };
  for (const k of Object.keys(benv)) if (k.startsWith("RUST_NATIVE_") || k === "V8_ACTION_USER_TIMEOUT_SECS") delete benv[k];
  Object.assign(benv, {
    RUST_NATIVE_MODE: "worker",
    // No sandbox outside Linux: the backend refuses to start native mode without this opt-in.
    ...(process.platform === "linux" ? {} : { RUST_NATIVE_ALLOW_UNSANDBOXED: "true" }),
    V8_ACTION_USER_TIMEOUT_SECS: "5",
    // Secrets the worker must never see (suite 3).
    CONVEX_E2E_SECRET: E2E_SECRET,
    INSTANCE_SECRET: INSTANCE_SECRET_ENV,
  }, env);
  for (const k of Object.keys(benv)) if (benv[k] === null) delete benv[k];
  const backend = spawn(
    join(BACKEND_BIN, "convex-local-backend"),
    ["--instance-name", `native-e2e-${profile}`, "--instance-secret", secret, "--interface", "127.0.0.1",
      "--port", String(PORT), "--site-proxy-port", String(SITE_PORT), "convex_local_backend.sqlite3"],
    { cwd: dataDir, env: benv, stdio: ["ignore", openLog(dataDir, "backend.log"), openLog(dataDir, "backend.log")] },
  );
  children.push(backend);
  if (!state || !state.mock) {
    const mock = spawn("node", [join(REPO, "bench/mock-server/server.ts")], {
      env: { ...process.env, MOCK_PORT: String(MOCK_PORT) },
      stdio: ["ignore", "ignore", "inherit"],
    });
    children.push(mock);
    state = { ...(state ?? {}), mock };
  }
  state = { ...state, adminKey, dataDir, backend, profile };
  await waitFor(async () => (await fetch(`${URL_}/version`)).ok, "backend");
  await waitFor(async () => (await fetch(`${MOCK}/bytes/1`)).ok, "mock server");
  return { adminKey, dataDir, pid: backend.pid };
}

import { openSync, readFileSync } from "node:fs";
const readFileText = (f) => readFileSync(f, "utf8").trim();
const openLog = (dir, name) => openSync(join(dir, name), "a");

export const backendPid = () => state?.backend?.pid;
export const dataDir = () => state.dataDir;
export const adminKey = () => state.adminKey;

/** Stop only the backend (keeps the mock server and the data dir). Returns when it is gone. */
export async function stopBackend(signal = "SIGTERM") {
  const b = state?.backend;
  if (!b || b.exitCode !== null || b.signalCode !== null) return;
  const kids = descendants(b.pid);
  const done = new Promise((r) => b.once("exit", r));
  try { b.kill(signal); } catch {}
  await Promise.race([done, sleep(10_000)]);
  if (b.exitCode === null && b.signalCode === null) { try { b.kill("SIGKILL"); } catch {} }
  await sleep(500);
  // Workers we started (children of our backend) must not outlive it.
  for (const p of kids) if (alive(p) && isWorker(p)) { try { process.kill(p, "SIGKILL"); } catch {} }
}

/** Kill everything this process started and delete this profile's data dir. */
export async function stop({ deleteData = true } = {}) {
  await stopBackend();
  for (const c of children.splice(0)) {
    try { c.kill("SIGTERM"); } catch {}
  }
  if (deleteData && state?.dataDir) rmSync(state.dataDir, { recursive: true, force: true });
  state = null;
}
function killAllSync() {
  for (const c of children.splice(0)) {
    try { c.kill("SIGKILL"); } catch {}
  }
}
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => { killAllSync(); process.exit(1); });
process.on("exit", killAllSync);

// ---- processes ------------------------------------------------------------

function ps(args) {
  return spawnSync("ps", args, { encoding: "utf8" }).stdout ?? "";
}
export const alive = (pid) => { try { process.kill(pid, 0); return true; } catch { return false; } };
function childrenOf(pid) {
  const r = spawnSync("pgrep", ["-P", String(pid)], { encoding: "utf8" });
  return (r.stdout ?? "").split("\n").filter(Boolean).map(Number);
}
function descendants(pid) {
  const out = [];
  for (const c of childrenOf(pid)) out.push(c, ...descendants(c));
  return out;
}
const commandOf = (pid) => ps(["-o", "command=", "-p", String(pid)]).trim();
const isWorker = (pid) => /rust[_-]native[_-]worker/.test(commandOf(pid));
/** Native worker processes of the running backend (its descendants whose command names the worker entry). */
export function workerPids() {
  const b = backendPid();
  return b ? descendants(b).filter(isWorker) : [];
}

// ---- CLI ------------------------------------------------------------------

export function cliEnv(extra = {}) {
  const env = { ...process.env };
  delete env.CONVEX_DEPLOYMENT;
  delete env.CONVEX_DEPLOY_KEY;
  const merged = {
    ...env,
    CONVEX_SELF_HOSTED_URL: URL_,
    CONVEX_SELF_HOSTED_ADMIN_KEY: state?.adminKey ?? "",
    CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? join(REPO, "target-e2e"),
    CI: "1",
    NO_COLOR: "1",
    ...extra,
  };
  for (const k of Object.keys(merged)) if (merged[k] === null) delete merged[k];
  return merged;
}

/** Run the fork CLI in `cwd` (default: the fixture project). Output has ANSI stripped. */
export function convexCli(args, { cwd = NATIVE_DIR, env = {}, timeout = 1_200_000 } = {}) {
  // NATIVE_CLI overrides the CLI entry (e.g. a locally patched bundle); default: the fork package.
  const r = spawnSync("node", [process.env.NATIVE_CLI ?? join(NATIVE_DIR, "node_modules/convex/bin/main.js"), ...args], {
    cwd, env: cliEnv(env), encoding: "utf8", timeout, maxBuffer: 64 * 1024 * 1024,
  });
  return { status: r.status, out: strip(`${r.stdout ?? ""}${r.stderr ?? ""}`) };
}
/** `convex dev --once --rust-runtime <runtime>` */
export const push = (runtime, env = {}) => convexCli(["dev", "--once", "--rust-runtime", runtime], { env });
export const setEnv = (k, v) => convexCli(["env", "set", k, v]);

// ---- calling actions ------------------------------------------------------

/** Calls an action over HTTP. { ok, value, message, logLines, ms }; never throws on a failed call. */
export async function call(path, args = {}, kind = "action") {
  const t0 = Date.now();
  const res = await fetch(`${URL_}/api/${kind}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Convex ${state.adminKey}` },
    body: JSON.stringify({ path, args, format: "json" }),
  });
  const text = await res.text();
  let body;
  try { body = JSON.parse(text); } catch { return { ok: false, message: `HTTP ${res.status}: ${text.slice(0, 500)}`, logLines: [], ms: Date.now() - t0 }; }
  const ms = Date.now() - t0;
  if (body.status === "success") return { ok: true, value: body.value, logLines: body.logLines ?? [], ms };
  return { ok: false, message: String(body.errorMessage ?? JSON.stringify(body)), logLines: body.logLines ?? [], ms };
}
export const query = (path, args = {}) => call(path, args, "query");

/** GET /api/rust_native/info (admin key). */
export async function nativeInfo() {
  const res = await fetch(`${URL_}/api/rust_native/info`, { headers: { authorization: `Convex ${state.adminKey}` } });
  return { status: res.status, body: res.ok ? await res.json() : null };
}

export async function pollUntil(fn, what, ms = 20_000, every = 150) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    last = await fn();
    if (last !== undefined && last !== false) return last;
    await sleep(every);
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** Exact messages from contracts/NATIVE.md section 4.3/4.4. */
export const MSG = {
  oom: /Rust execution ran out of memory \(maximum memory usage: 512 MiB\)/,
  stack: /Rust action overflowed its stack \(stack size: 8 MiB\)/,
  sandbox: /Rust action was stopped by the sandbox \(disallowed system call .+\)/,
  signal: (name) => new RegExp(`Rust action crashed the worker process \\(signal ${name}\\)`),
  exit: (n) => new RegExp(`Rust action exited the worker process \\(exit status ${n}\\)`),
  timeout: /Function execution timed out \(maximum duration: 5(\.0+)?s\)/,
  tooLarge: /Function result is too large/,
};

/** startBackend + native push. Throws with the CLI output when the push fails. */
export async function setup(profile, env = {}, pushEnv = {}) {
  await startBackend(profile, env);
  const p = push("native", pushEnv);
  if (p.status !== 0) throw new Error(`native push failed (exit ${p.status}):\n${p.out.slice(-3000)}`);
  return p;
}

// Shared helpers: fixture apps in temp dirs, the fork CLI, and assertions on its output.
import { spawn } from "node:child_process";
import {
  cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
export const DEPLOY_DIR = path.resolve(here, "..");
export const ROOT = path.resolve(here, "..", "..", "..");
// The Convex backend fork (github.com/toomingos/convex-backend, branch rust-native-runtime): clone it and
// set CONVEX_BACKEND_DIR, or put it at <repo>/vendor/convex-backend.
export const BACKEND_DIR = process.env.CONVEX_BACKEND_DIR ?? `${ROOT}/vendor/convex-backend`;
export const FORK_CONVEX = `${BACKEND_DIR}/npm-packages/convex`;
export const CLI = `${FORK_CONVEX}/bin/main.js`;
export const CONVEX_FN_PATH = `${ROOT}/crates/convex_fn`;
export const TOOLCHAIN = "nightly-2026-06-28";
export const CARGO_TARGET_DIR = path.join(DEPLOY_DIR, ".cache", "target");

export const URL_ = process.env.DEPLOY_URL;
export const ADMIN_KEY = process.env.DEPLOY_ADMIN_KEY;

const apps = [];
const STRIP_ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;
export const strip = (s) => s.replace(STRIP_ANSI, "");

export function baseEnv(extra = {}) {
  const env = { ...process.env };
  delete env.CONVEX_DEPLOYMENT;
  delete env.CONVEX_DEPLOY_KEY;
  const merged = {
    ...env,
    CONVEX_SELF_HOSTED_URL: URL_,
    CONVEX_SELF_HOSTED_ADMIN_KEY: ADMIN_KEY,
    CARGO_TARGET_DIR,
    CONVEX_FN_PATH,
    CI: "1", // never prompt
    NO_COLOR: "1",
    ...extra,
  };
  for (const k of Object.keys(merged)) if (merged[k] === null) delete merged[k]; // null = unset
  return merged;
}

/** Run a command; resolves with {code, stdout, stderr, out (both, ansi stripped), ms}. */
export function exec(cmd, args, { cwd, env = {}, timeout = 600_000 } = {}) {
  return new Promise((resolve) => {
    const t0 = Date.now();
    const p = spawn(cmd, args, { cwd, env: baseEnv(env), stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    p.stdout.on("data", (d) => (stdout += d));
    p.stderr.on("data", (d) => (stderr += d));
    const timer = setTimeout(() => p.kill("SIGKILL"), timeout);
    p.on("close", (code) => {
      clearTimeout(timer);
      stdout = strip(stdout);
      stderr = strip(stderr);
      resolve({ code, stdout, stderr, out: stdout + stderr, ms: Date.now() - t0 });
    });
  });
}

export function cli(app, args, opts = {}) {
  return exec(process.execPath, [CLI, ...args], { cwd: app.dir, ...opts });
}

/**
 * Create a fixture app in a temp dir: copy fixtures/base, optionally run
 * `convex rust init`, then overlay fixtures/<name> directories in order.
 */
export async function makeApp({ init = true, overlays = [] } = {}) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "convex-deploy-e2e-"));
  cpSync(path.join(DEPLOY_DIR, "fixtures", "base"), dir, { recursive: true });
  mkdirSync(path.join(dir, "node_modules"), { recursive: true });
  symlinkSync(FORK_CONVEX, path.join(dir, "node_modules", "convex"));
  const app = { dir, rust: path.join(dir, "convex", "rust") };
  apps.push(app);
  if (init) {
    const r = await cli(app, ["rust", "init"]);
    if (r.code !== 0) throw new Error(`rust init failed:\n${r.out}`);
    app.initOut = r.out;
  }
  for (const o of overlays) overlay(app, o);
  return app;
}

/** Copy fixtures/<name>/** over the app (so a fixture ships only the files it changes). */
export function overlay(app, name) {
  cpSync(path.join(DEPLOY_DIR, "fixtures", name), app.dir, { recursive: true });
}

export const write = (app, rel, text) => {
  const f = path.join(app.dir, rel);
  mkdirSync(path.dirname(f), { recursive: true });
  writeFileSync(f, text);
};
export const read = (app, rel) => readFileSync(path.join(app.dir, rel), "utf8");
export const remove = (app, rel) => rmSync(path.join(app.dir, rel), { force: true, recursive: true });
export const exists = (app, rel) => existsSync(path.join(app.dir, rel));

/** `convex dev --once`: build + push. */
export const push = (app, opts) => cli(app, ["dev", "--once"], opts);
/** `convex run <fn> [json]` */
export const callRun = (app, fn, args = {}, opts) => cli(app, ["run", fn, JSON.stringify(args)], opts);

/** Source for a Rust module with one #[action] returning a constant string. */
export function constAction(name, value, vis = "") {
  return `use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[action${vis}]
pub async fn ${name}(_ctx: &Ctx, _args: NoArgs) -> Result<String> {
    Ok(${JSON.stringify(value)}.to_string())
}
`;
}

/** Plain HTTP call to the public function API (what a client sees; no admin key). */
export async function clientCall(kind, fnPath, args = {}) {
  const r = await fetch(`${URL_}/api/${kind}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path: fnPath, args, format: "json" }),
  });
  return { status: r.status, body: await r.json().catch(() => null) };
}

export function cleanup() {
  for (const a of apps) rmSync(a.dir, { recursive: true, force: true });
}

export { readdirSync };

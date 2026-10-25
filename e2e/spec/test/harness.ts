// Starts the forked backend + the bench mock server, pushes the app, and
// exposes helpers to run a spec on both runtimes. Everything is torn down by stop().
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ConvexHttpClient } from "convex/browser";
import { ConvexError } from "convex/values";
import { makeFunctionReference } from "convex/server";

export const SPEC_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
export const REPO = join(SPEC_DIR, "..", "..");
const REPO_MAIN = process.env.CONVEX_RS_MAIN ?? REPO;
// Prebuilt artefacts live in the main checkout (default: this repo; set CONVEX_RS_MAIN to share another).
const BACKEND_BIN = join(REPO_MAIN, "target-backend/debug");
export const PORT = Number(process.env.SPEC_PORT ?? 3230);
export const SITE_PORT = PORT + 1;
export const MOCK_PORT = Number(process.env.SPEC_MOCK_PORT ?? 3238);
export const URL_ = `http://127.0.0.1:${PORT}`;
export const SITE_URL = `http://127.0.0.1:${SITE_PORT}`;
export const MOCK = `http://127.0.0.1:${MOCK_PORT}`;
const DATA = join(SPEC_DIR, ".backend-data");

const children: ChildProcess[] = [];
export let adminKey = "";

export function cliEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    CONVEX_SELF_HOSTED_URL: URL_,
    CONVEX_SELF_HOSTED_ADMIN_KEY: adminKey,
    CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? join(REPO_MAIN, "target-e2e"),
    CI: "1",
  };
}

export function convexCli(cwd: string, args: string[], timeout = 1_200_000) {
  const r = spawnSync("node", [join(SPEC_DIR, "node_modules/convex/bin/main.js"), ...args], {
    cwd,
    env: cliEnv(),
    encoding: "utf8",
    timeout,
    maxBuffer: 64 * 1024 * 1024,
  });
  return { status: r.status, out: `${r.stdout ?? ""}${r.stderr ?? ""}` };
}

async function waitFor(fn: () => Promise<boolean>, what: string, ms = 60_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      if (await fn()) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`timed out waiting for ${what}`);
}

function findSupportedNode(): string {
  const cands = [process.env.SPEC_NODE22 ?? ""];
  const nvm = join(homedir(), ".nvm/versions/node");
  if (existsSync(nvm)) for (const d of readdirSync(nvm)) cands.push(join(nvm, d, "bin/node"));
  cands.push("/usr/local/bin/node", "/opt/homebrew/opt/node@22/bin/node", "/opt/homebrew/opt/node@24/bin/node", process.execPath);
  for (const c of cands) {
    if (!c || !existsSync(c)) continue;
    const v = spawnSync(c, ["--version"], { encoding: "utf8" }).stdout ?? "";
    if (v.startsWith("v22.") || v.startsWith("v24.")) return c;
  }
  throw new Error("no Node 22/24 found for the backend's node executor (set SPEC_NODE22)");
}

export async function start() {
  rmSync(DATA, { recursive: true, force: true });
  mkdirSync(DATA, { recursive: true });
  const secret = spawnSync("openssl", ["rand", "-hex", "32"], { encoding: "utf8" }).stdout.trim();
  const key = spawnSync(join(BACKEND_BIN, "generate_key"), ["convex-rs-e2e-spec", secret], {
    encoding: "utf8",
  }).stdout.trim().split("\n").pop()!;
  adminKey = key;
  writeFileSync(join(DATA, "admin-key"), key);

  // The backend's "use node" executor needs Node 22/24 as `node` on its PATH.
  const nodeBin = findSupportedNode();
  mkdirSync(join(DATA, "bin"), { recursive: true });
  symlinkSync(nodeBin, join(DATA, "bin", "node"));

  const backend = spawn(
    join(BACKEND_BIN, "convex-local-backend"),
    [
      "--instance-name", "convex-rs-e2e-spec",
      "--instance-secret", secret,
      "--interface", "127.0.0.1",
      "--port", String(PORT),
      "--site-proxy-port", String(SITE_PORT),
      "convex_local_backend.sqlite3",
    ],
    {
      cwd: DATA,
      env: { ...process.env, PATH: `${join(DATA, "bin")}:${process.env.PATH}` },
      stdio: ["ignore", "ignore", "ignore"],
    },
  );
  children.push(backend);
  const mock = spawn("node", [join(REPO, "bench/mock-server/server.ts")], {
    env: { ...process.env, MOCK_PORT: String(MOCK_PORT) },
    stdio: ["ignore", "ignore", "inherit"],
  });
  children.push(mock);

  await waitFor(async () => (await fetch(`${URL_}/version`)).ok, "backend");
  await waitFor(async () => (await fetch(`${MOCK}/bytes/1`)).ok, "mock server");
}

export function stop() {
  for (const c of children.splice(0)) {
    try {
      c.kill("SIGTERM");
    } catch {}
  }
}
for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    stop();
    process.exit(1);
  });
}
process.on("exit", stop);

// ---- running specs -------------------------------------------------------

export type Outcome =
  | { ok: true; value: any }
  | { ok: false; convex: boolean; data?: any; message: string };

export type Runtime = "node" | "rust";
const snake = (s: string) => s.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase());
export const fnPath = (rt: Runtime, name: string) =>
  rt === "node" ? `nodeSpec:${name}` : `rust/spec:${snake(name)}`;

export function client(identity?: Record<string, unknown>) {
  const c = new ConvexHttpClient(URL_, { logger: false });
  c.setAdminAuth(adminKey, identity as any);
  return c;
}

export async function runPath(
  kind: "query" | "mutation" | "action",
  path: string,
  args: any,
  identity?: Record<string, unknown>,
  anonymous = false,
): Promise<Outcome> {
  const c = anonymous ? new ConvexHttpClient(URL_, { logger: false }) : client(identity);
  try {
    const ref = makeFunctionReference<any>(path);
    const value =
      kind === "query" ? await c.query(ref, args) : kind === "mutation" ? await c.mutation(ref, args) : await c.action(ref, args);
    return { ok: true, value };
  } catch (e: any) {
    if (e instanceof ConvexError) return { ok: false, convex: true, data: e.data, message: String(e.message) };
    return { ok: false, convex: false, message: String(e?.message ?? e) };
  }
}

/** Runs an action over the raw HTTP API and returns its value plus the function's log lines. */
export async function runWithLogs(path: string, args: any = {}): Promise<{ value: any; logLines: string[] }> {
  const res = await fetch(`${URL_}/api/action`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Convex ${adminKey}` },
    body: JSON.stringify({ path, args, format: "json" }),
  });
  const body: any = await res.json();
  if (body.status !== "success") throw new Error(`action ${path} failed: ${JSON.stringify(body)}`);
  return { value: body.value, logLines: body.logLines ?? [] };
}

export const run = (rt: Runtime, name: string, args: any = {}, identity?: Record<string, unknown>) =>
  runPath("action", fnPath(rt, name), args, identity);

export async function pollUntil<T>(fn: () => Promise<T | undefined>, what: string, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const v = await fn();
    if (v !== undefined) return v;
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`timed out waiting for ${what}`);
}

export const readAdminKey = () => readFileSync(join(DATA, "admin-key"), "utf8");

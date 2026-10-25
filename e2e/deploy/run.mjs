// Starts a private backend (ports 3240/3241), runs the node:test suite against it, kills the backend.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, rmSync, openSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const B = path.join(here, "..", "..", "target-backend", "debug");
const PORT = Number(process.env.DEPLOY_PORT ?? 3240);
const SITE = PORT + 1;
const run = path.join(here, ".run");
const data = path.join(run, "backend-data");
rmSync(data, { recursive: true, force: true });
mkdirSync(data, { recursive: true });

const secret = spawnSync("openssl", ["rand", "-hex", "32"], { encoding: "utf8" }).stdout.trim();
const adminKey = spawnSync(`${B}/generate_key`, ["convex-rs-e2e-deploy", secret], { encoding: "utf8" })
  .stdout.trim().split("\n").pop();

const logFd = openSync(path.join(run, "backend.log"), "w");
const be = spawn(
  `${B}/convex-local-backend`,
  ["--instance-name", "convex-rs-e2e-deploy", "--instance-secret", secret, "--interface", "127.0.0.1",
   "--port", String(PORT), "--site-proxy-port", String(SITE), "convex_local_backend.sqlite3"],
  { cwd: data, stdio: ["ignore", logFd, logFd] },
);
let stopped = false;
function stop() {
  if (!stopped) {
    stopped = true;
    try { be.kill("SIGTERM"); } catch {}
  }
}
process.on("exit", stop);
for (const s of ["SIGINT", "SIGTERM"]) process.on(s, () => { stop(); process.exit(130); });

async function ready() {
  for (let i = 0; i < 120; i++) {
    try { const r = await fetch(`http://127.0.0.1:${PORT}/version`); if (r.ok) return; } catch {}
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("backend did not start");
}
let code = 1;
try {
  await ready();
  const child = spawn(process.execPath, ["--test", "--test-concurrency=1", "--test-timeout=1800000", ...(process.argv.length > 2 ? process.argv.slice(2) : ["test/"])], {
    cwd: here, stdio: "inherit",
    env: { ...process.env, DEPLOY_URL: `http://127.0.0.1:${PORT}`, DEPLOY_ADMIN_KEY: adminKey },
  });
  code = await new Promise((r) => child.on("exit", (c) => r(c ?? 1)));
} finally {
  stop();
}
process.exit(code);

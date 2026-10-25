// End-to-end: a Rust action using the real `reqwest` crate API, deployed with
// the fork CLI and NO manual patch config. The CLI-managed [patch.crates-io]
// block (created on build) swaps in the shim, so requests go through the
// backend's Fetch task. Everything started here is torn down in after().
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ConvexHttpClient } from "convex/browser";
import { makeFunctionReference } from "convex/server";

const DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
const MAIN = process.env.CONVEX_RS_MAIN ?? join(DIR, "..", "..");
const BIN = join(MAIN, "target-backend/debug");
const PORT = Number(process.env.E2E_PORT ?? 3250);
const MOCK_PORT = Number(process.env.E2E_MOCK_PORT ?? 3258);
const ECHO_PORT = Number(process.env.E2E_ECHO_PORT ?? MOCK_PORT + 1);
const URL_ = `http://127.0.0.1:${PORT}`;
const MOCK = `http://127.0.0.1:${MOCK_PORT}`;
const ECHO = `http://127.0.0.1:${ECHO_PORT}`;
const DATA = join(DIR, ".backend-data");
const CONFIG = join(DIR, "convex/rust/.cargo/config.toml");

const children = [];
let echoServer;
let adminKey = "";
let pushOutput = "";

async function waitFor(fn, what, ms = 60_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      if (await fn()) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`timed out waiting for ${what}`);
}

const call = async (name, args = {}) => {
  const c = new ConvexHttpClient(URL_, { logger: false });
  c.setAdminAuth(adminKey);
  return await c.action(makeFunctionReference(`rust/http:${name}`), { base: MOCK, echo: ECHO, ...args });
};

before(async () => {
  // Start from a crate with no patch config: the CLI must create it.
  rmSync(join(DIR, "convex/rust/.cargo"), { recursive: true, force: true });
  rmSync(DATA, { recursive: true, force: true });
  mkdirSync(DATA, { recursive: true });
  const secret = spawnSync("openssl", ["rand", "-hex", "32"], { encoding: "utf8" }).stdout.trim();
  adminKey = spawnSync(join(BIN, "generate_key"), ["convex-rs-e2e-reqwest", secret], { encoding: "utf8" })
    .stdout.trim()
    .split("\n")
    .pop();
  writeFileSync(join(DATA, "admin-key"), adminKey);

  children.push(
    spawn(
      join(BIN, "convex-local-backend"),
      [
        "--instance-name", "convex-rs-e2e-reqwest",
        "--instance-secret", secret,
        "--interface", "127.0.0.1",
        "--port", String(PORT),
        "--site-proxy-port", String(PORT + 1),
        "convex_local_backend.sqlite3",
      ],
      { cwd: DATA, stdio: "ignore" },
    ),
  );
  children.push(
    spawn("node", [join(MAIN, "bench/mock-server/server.ts")], {
      env: { ...process.env, MOCK_PORT: String(MOCK_PORT) },
      stdio: ["ignore", "ignore", "inherit"],
    }),
  );
  // Tiny header-echo server (the bench mock server has no such endpoint).
  echoServer = createServer((req, res) => {
    const body = JSON.stringify({ method: req.method, url: req.url, headers: req.headers });
    res.writeHead(200, { "content-type": "application/json", "x-echo-reply": "pong" });
    res.end(body);
  });
  await new Promise((r) => echoServer.listen(ECHO_PORT, "127.0.0.1", r));

  await waitFor(async () => (await fetch(`${URL_}/version`)).ok, "backend");
  await waitFor(async () => (await fetch(`${MOCK}/bytes/1`)).ok, "mock server");

  // Deploy with the fork CLI. No CONVEX_REQWEST_PATH: the shim must be found in
  // the CLI package (rust/convex_reqwest).
  const env = {
    ...process.env,
    CONVEX_SELF_HOSTED_URL: URL_,
    CONVEX_SELF_HOSTED_ADMIN_KEY: adminKey,
    CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? join(MAIN, "target-e2e"),
    CI: "1",
  };
  delete env.CONVEX_REQWEST_PATH;
  const r = spawnSync("node", [join(DIR, "node_modules/convex/bin/main.js"), "dev", "--once"], {
    cwd: DIR,
    env,
    encoding: "utf8",
    timeout: 1_200_000,
    maxBuffer: 64 * 1024 * 1024,
  });
  pushOutput = `${r.stdout ?? ""}${r.stderr ?? ""}`;
  assert.equal(r.status, 0, `convex dev --once failed:\n${pushOutput}`);
});

after(() => {
  for (const c of children.splice(0)) {
    try {
      c.kill("SIGTERM");
    } catch {}
  }
  echoServer?.close();
});

describe("reqwest in a Rust action (managed patch)", () => {
  it("the CLI wrote the managed patch block", () => {
    assert.ok(existsSync(CONFIG), "convex/rust/.cargo/config.toml was not created");
    const toml = readFileSync(CONFIG, "utf8");
    assert.match(toml, /convex-managed:patch \(begin\)/);
    assert.match(toml, /\[patch\.crates-io\]\nreqwest = \{ path = ".*convex_reqwest" \}/);
  });

  it("the unused-patch warning is filtered and the build output was streamed", () => {
    assert.doesNotMatch(pushOutput, /was not used in the crate graph/);
    assert.match(pushOutput, /Compiling|Finished/);
  });

  it("GET json", async () => {
    const v = await call("get_json");
    assert.equal(v.status, 200);
    assert.match(v.contentType, /application\/json/);
    const expected = await (await fetch(`${MOCK}/json/5?seed=7`)).json();
    assert.deepEqual(v.items, expected);
  });

  it("POST json", async () => {
    const v = await call("post_json");
    assert.equal(v.echo.length, v.sent);
    assert.match(v.echo.sha256, /^[0-9a-f]{64}$/);
  });

  it("request and response headers", async () => {
    const v = await call("headers");
    assert.equal(v.reply, "pong");
    assert.equal(v.seen.method, "GET");
    assert.equal(v.seen.headers["x-default"], "d");
    assert.equal(v.seen.headers["x-req"], "r");
    assert.equal(v.seen.headers["authorization"], "Bearer tok");
    assert.equal(v.seen.headers["user-agent"], "convex-reqwest-e2e/1");
  });

  it("error_for_status on 404", async () => {
    const v = await call("not_found");
    assert.deepEqual(v, { ok: false, plainStatus: 404, status: 404, isStatus: true, url: "/nope" });
  });

  it("error_for_status error fails the action", async () => {
    await assert.rejects(call("not_found_fails"), /404/);
  });

  it("bytes_stream", async () => {
    const n = 100_000;
    const v = await call("stream", { n });
    assert.equal(v.len, n);
    assert.equal(v.equal, true);
    assert.ok(v.chunks >= 1);
  });
});

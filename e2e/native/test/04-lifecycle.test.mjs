// DESIGN 8.2 suite 4: lifecycle. Redeploy under load, kill -9, parallelism, idle reaping, breaker.
import assert from "node:assert/strict";
import { chmodSync, existsSync, readdirSync, statSync, truncateSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import {
  alive, backendPid, call, dataDir, nativeInfo, pollUntil, push, setup, sleep, stop, stopBackend,
  startBackend, workerPids,
} from "../lib/harness.mjs";

const V = (n) => `rust/lifecycle:${n}`;

function filesUnder(dir) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...filesUnder(p));
    else out.push(p);
  }
  return out;
}

describe("native lifecycle", () => {
  before(() => setup("lifecycle", { RUST_NATIVE_IDLE_TIMEOUT_SECS: "3", RUST_NATIVE_MIN_IDLE: "0", V8_ACTION_USER_TIMEOUT_SECS: "60" }, { NATIVE_E2E_VERSION: "v1" }));
  after(() => stop());

  it("info endpoint: enabled, worker mode, pool list", async () => {
    const { status, body } = await nativeInfo();
    assert.equal(status, 200);
    assert.equal(body.enabled, true);
    assert.equal(body.mode, "worker");
    assert.ok(Array.isArray(body.pools));
    assert.ok(body.target?.triple, JSON.stringify(body.target));
  });

  it("32 parallel calls all succeed", async () => {
    const rs = await Promise.all(Array.from({ length: 32 }, () => call(V("slow_version"), { ms: 300 })));
    const bad = rs.filter((r) => !r.ok);
    assert.equal(bad.length, 0, `failures: ${bad.map((r) => r.message).slice(0, 3).join(" | ")}`);
    assert.ok(new Set(rs.map((r) => r.value.pid)).size > 1, "expected several workers");
  });

  it("idle workers are reaped after RUST_NATIVE_IDLE_TIMEOUT_SECS", async () => {
    await Promise.all(Array.from({ length: 4 }, () => call(V("slow_version"), { ms: 500 })));
    const before = workerPids();
    assert.ok(before.length >= 1, "no worker processes visible after calls");
    await pollUntil(async () => (workerPids().length === 0 ? true : undefined), "all idle workers reaped", 30_000, 500);
    const { body } = await nativeInfo();
    assert.equal(body.pools.reduce((n, p) => n + (p.live ?? 0), 0), 0, JSON.stringify(body.pools));
    const again = await call(V("version"));
    assert.ok(again.ok, `call after reaping failed: ${again.message}`);
  });

  it("redeploy swaps code; a v1 call in flight finishes with v1 output", async () => {
    const v1 = await call(V("version"));
    assert.equal(v1.value?.version, "v1", JSON.stringify(v1));
    const inflight = call(V("slow_version"), { ms: 20000 }); // spans the rebuild + push of v2
    await sleep(300);
    const p = push("native", { NATIVE_E2E_VERSION: "v2" });
    assert.equal(p.status, 0, p.out.slice(-2000));
    const v2 = await call(V("version"));
    assert.equal(v2.value?.version, "v2", `new calls must run v2: ${JSON.stringify(v2)}`);
    const old = await inflight;
    assert.ok(old.ok, `in-flight call was killed by the swap: ${old.message}`);
    assert.equal(old.value.version, "v1", "in-flight call changed code mid-call");
  });

  it("kill -9 of the backend leaves no worker processes", async () => {
    await Promise.all([call(V("slow_version"), { ms: 400 }), call(V("slow_version"), { ms: 400 })]);
    const pids = workerPids();
    assert.ok(pids.length >= 1, "no workers to orphan");
    process.kill(backendPid(), "SIGKILL");
    await sleep(2000);
    const left = pids.filter(alive);
    assert.deepEqual(left, [], `orphaned workers: ${left}`);
  });

  it("breaker: a corrupt artifact on disk fails fast with the start error", async () => {
    await stopBackend("SIGKILL");
    await startBackend("lifecycle", { RUST_NATIVE_IDLE_TIMEOUT_SECS: "3", RUST_NATIVE_MIN_IDLE: "0", V8_ACTION_USER_TIMEOUT_SECS: "60" }, { keepData: true });
    // Warm the artifact cache (a cold resolve would rewrite a corrupt file: the store self-heals),
    // then corrupt the file on disk once the idle worker is reaped.
    const warm = await call(V("version"));
    assert.ok(warm.ok, warm.message);
    await pollUntil(() => workerPids().length === 0, "idle workers reaped", 20_000);
    const files = filesUnder(join(dataDir(), "convex_local_storage", "rust-native")).filter((f) => statSync(f).size > 4096);
    assert.ok(files.length >= 1, `no artifact files under rust-native dir (${dataDir()})`);
    for (const f of files) {
      chmodSync(f, 0o600); // artifacts are stored read-only on purpose
      truncateSync(f, 64);
      writeFileSync(f, Buffer.alloc(64, 0x41));
    }
    const r1 = await call(V("version"));
    assert.equal(r1.ok, false);
    assert.match(r1.message, /Could not start the native Rust worker/);
    const t0 = Date.now();
    const r2 = await call(V("version"));
    assert.match(r2.message, /Could not start the native Rust worker/);
    assert.ok(Date.now() - t0 < 2000, "second failure was not fast (breaker open expected)");
  });
});

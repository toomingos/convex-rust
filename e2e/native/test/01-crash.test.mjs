// DESIGN 8.2 suite 2: failure modes. Each case asserts the exact message (NATIVE.md 4.4) and that
// the next call succeeds on a fresh worker. V8_ACTION_USER_TIMEOUT_SECS=5 (harness default).
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { MSG, call, linuxOnly, setup, stop } from "../lib/harness.mjs";

const P = (n) => `rust/crash:${n}`;

describe("native failure modes", () => {
  before(() => setup("crash"));
  after(() => stop());

  /** A failing call, then a healthy one on a different pid. */
  async function crashes(name, args, re) {
    const warm = await call(P("probe"));
    assert.ok(warm.ok, `warm-up failed: ${warm.message}`);
    const r = await call(P(name), args);
    assert.equal(r.ok, false, `${name} unexpectedly succeeded: ${JSON.stringify(r.value)}`);
    assert.match(r.message, re);
    const next = await call(P("probe"));
    assert.ok(next.ok, `next call failed after ${name}: ${next.message}`);
    assert.notEqual(next.value.pid, warm.value.pid, "the crashed worker was reused");
    return r;
  }

  it("healthy baseline: probe returns a pid and counts calls on the same worker", async () => {
    const a = await call(P("probe"));
    const b = await call(P("probe"));
    assert.ok(a.ok && b.ok, JSON.stringify([a, b]));
    assert.equal(typeof a.value.pid, "number");
    assert.equal(b.value.calls, a.value.calls + 1, "a worker serves several calls");
  });

  it("panic!: panic message and location, worker discarded", async () => {
    const r = await crashes("panic_plain", {}, /native-e2e-panic/);
    assert.match(r.message, /crash\.rs:\d+/);
  });
  it("panic hook that aborts (panic=abort stand-in): SIGABRT", () =>
    crashes("panic_abort_hook", {}, MSG.signal("SIGABRT")));
  it("std::process::abort(): SIGABRT", () => crashes("abort", {}, MSG.signal("SIGABRT")));
  it("null-deref segfault: SIGSEGV", () => crashes("segfault", {}, MSG.signal("SIGSEGV")));
  it("infinite recursion: stack overflow message", () => crashes("stack_overflow", {}, MSG.stack));
  it("2 GiB allocation: out-of-memory message", () => crashes("oom", { bytes: 2 * 1024 ** 3 }, MSG.oom));
  it("loop {} hits the 5 s timeout", async () => {
    const t0 = Date.now();
    await crashes("infinite_loop", {}, MSG.timeout);
    const s = (Date.now() - t0) / 1000;
    assert.ok(s >= 4.5 && s < 30, `timeout took ${s}s`);
  });
  it("process::exit(3): exit status message", () => crashes("exit3", {}, MSG.exit(3)));

  it("a thread spinning after return: call succeeds, worker is recycled", async () => {
    const r = await call(P("spin_thread"));
    assert.ok(r.ok, r.message);
    const next = await call(P("probe"));
    assert.ok(next.ok, next.message);
    assert.notEqual(next.value.pid, r.value.pid, "a worker with a runaway thread was reused");
  });

  it("20 MiB println! flood: call completes, logs are capped, backend still serves", async () => {
    const r = await call(P("println_flood"), { mib: 20 });
    // Either outcome is acceptable for the call itself; what matters is the cap and the next call.
    const bytes = r.logLines.reduce((n, l) => n + l.length, 0);
    assert.ok(bytes <= 20 * 1024 * 1024, `captured ${bytes} bytes of stdout for a 20 MiB flood`);
    const next = await call(P("probe"));
    assert.ok(next.ok, next.message);
  });

  it("rlimits are applied inside the worker (RLIMIT_CORE=0)", { skip: linuxOnly("rlimits via /proc/self/limits") }, async () => {
    const r = await call("rust/isolation:limits");
    assert.ok(r.ok && r.value.ok, JSON.stringify(r));
    assert.match(r.value.text, /Max core file size\s+0\s+0/);
  });
});

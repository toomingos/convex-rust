// DESIGN 8.2 mode fallback + suite 5 (push validation, the cases reachable from the CLI).
// RUST_NATIVE_MODE off: auto -> wasm with a note; native -> a clear push error.
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { IS_LINUX, call, nativeInfo, push, setup, startBackend, stop, stopBackend } from "../lib/harness.mjs";

describe("mode fallback (RUST_NATIVE_MODE off)", () => {
  before(() => startBackend("modes-off", { RUST_NATIVE_MODE: null }));
  after(() => stop());

  it("info endpoint reports enabled:false (or 404 on a backend without it)", async () => {
    const { status, body } = await nativeInfo();
    assert.ok(status === 404 || body?.enabled === false, `status ${status} ${JSON.stringify(body)}`);
  });

  it("--rust-runtime auto falls back to wasm with a note, and the action runs", async () => {
    const p = push("auto");
    assert.equal(p.status, 0, p.out.slice(-2000));
    assert.match(p.out, /Rust: using wasm \(native unavailable: .*RUST_NATIVE_MODE is off/);
    const r = await call("rust/lifecycle:version");
    assert.ok(r.ok, r.message);
    assert.equal(typeof r.value.pid, "number");
  });

  it("--rust-runtime native fails the push with a clear error", async () => {
    const p = push("native");
    assert.notEqual(p.status, 0, `push unexpectedly succeeded:\n${p.out.slice(-1500)}`);
    assert.match(p.out, /Cannot use --rust-runtime native: .*RUST_NATIVE_MODE is off/);
  });
});

describe("native artifact on a backend whose mode was turned off later", () => {
  before(() => setup("modes-later"));
  after(() => stop());

  it("calls fail with a clear message, never silently switch runtime", async () => {
    const ok = await call("rust/lifecycle:version");
    assert.ok(ok.ok, ok.message);
    await stopBackend();
    await startBackend("modes-later", { RUST_NATIVE_MODE: null }, { keepData: true });
    const r = await call("rust/lifecycle:version");
    assert.equal(r.ok, false, "a native-only artifact ran with the mode off");
    assert.match(r.message, /RUST_NATIVE_MODE|native/i);
  });
});

describe("sandbox level strict", () => {
  before(() => startBackend("strict", { RUST_NATIVE_SANDBOX: "strict" }).then(() => push("native")).then((p) => assert.equal(p.status, 0, p.out.slice(-2000))));
  after(() => stop());

  it("macOS: strict refuses to start a worker (spawn error)", { skip: IS_LINUX ? "Linux: strict works where Landlock and seccomp exist; covered by the default suites on a Linux backend" : false }, async () => {
    const r = await call("rust/lifecycle:version");
    assert.equal(r.ok, false);
    assert.match(r.message, /Could not start the native Rust worker/);
  });
  it("Linux: strict runs actions", { skip: IS_LINUX ? false : "Linux only (strict needs Landlock + seccomp)" }, async () => {
    const r = await call("rust/lifecycle:version");
    assert.ok(r.ok, r.message);
  });
});

describe("push validation (suite 5)", () => {
  it("wrong triple", { skip: "needs a crafted artifact; covered by rust_analyze/native.rs unit tests (WN-08)" }, () => {});
  it("glibc too new", { skip: "needs a crafted artifact; covered by rust_analyze/native.rs unit tests (WN-08)" }, () => {});
  it("knob off", { skip: "covered by the mode-fallback tests above (CLI error + backend-side message)" }, () => {});
});

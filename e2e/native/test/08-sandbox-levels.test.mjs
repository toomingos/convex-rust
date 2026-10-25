// Sandbox level "off" (explicit) works everywhere: actions run and the sandbox-only probes are open.
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { call, dataDir, setup, stop } from "../lib/harness.mjs";

describe("sandbox level off", () => {
  before(() => setup("sandbox-off", { RUST_NATIVE_SANDBOX: "off", RUST_NATIVE_ALLOW_UNSANDBOXED: "true" }));
  after(() => stop());

  it("actions run", async () => {
    const r = await call("rust/crash:probe");
    assert.ok(r.ok, r.message);
  });
  it("no Landlock: the backend data dir is readable (proves the level is really off)", async () => {
    const r = await call("rust/isolation:list_dir", { path: dataDir() });
    assert.ok(r.ok, r.message);
    assert.equal(r.value.ok, true, JSON.stringify(r.value));
  });
});

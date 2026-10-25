// DESIGN 8.2 suite 2 (limits part): result too large, log cap.
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { MSG, call, setup, stop } from "../lib/harness.mjs";

describe("native limits", () => {
  before(() => setup("limits"));
  after(() => stop());

  it("result of 1 MiB is fine", async () => {
    const r = await call("rust/crash:big_result", { bytes: 1024 * 1024 });
    assert.ok(r.ok, r.message);
    assert.equal(r.value.length, 1024 * 1024);
  });

  it("result above 16 MiB: 'Function result is too large', next call fine", async () => {
    const r = await call("rust/crash:big_result", { bytes: 17 * 1024 * 1024 });
    assert.equal(r.ok, false);
    assert.match(r.message, MSG.tooLarge);
    const next = await call("rust/crash:probe");
    assert.ok(next.ok, next.message);
  });

  it("log cap: 20000 ctx log lines are truncated, call still completes", async () => {
    const r = await call("rust/crash:log_lines", { n: 20_000 });
    assert.ok(r.ok, r.message);
    assert.ok(r.logLines.length < 20_000, `no cap: ${r.logLines.length} lines`);
    assert.ok(r.logLines.length > 0);
    console.log(`  log lines kept: ${r.logLines.length} of 20000`);
  });

  it("stdout flood of 20 MiB is capped (16 MiB per call)", async () => {
    const r = await call("rust/crash:println_flood", { mib: 20 });
    const bytes = r.logLines.reduce((n, l) => n + l.length, 0);
    assert.ok(bytes <= 17 * 1024 * 1024, `captured ${bytes} bytes`);
    console.log(`  flood call ok=${r.ok}; captured ${bytes} bytes in ${r.logLines.length} lines`);
  });
});

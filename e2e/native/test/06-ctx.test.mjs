// DESIGN 8.2 (ctx calls): runQuery, runMutation, fetch, storage, scheduler from a native worker.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { after, before, describe, it } from "node:test";
import { MOCK, call, pollUntil, query, setup, stop } from "../lib/harness.mjs";

describe("native ctx calls", () => {
  before(() => setup("ctx"));
  after(() => stop());

  it("runQuery, runMutation, fetch, storage and scheduler round trips", async () => {
    const tag = `ctx-${randomUUID()}`;
    const r = await call("rust/ctx_calls:all", { tag, mockUrl: MOCK });
    assert.ok(r.ok, r.message);
    const v = r.value;
    assert.equal(v.doubled, 42);
    assert.deepEqual(v.notes, [{ tag, from: "native" }]);
    assert.equal(v.fetchStatus, 200);
    assert.equal(v.fetchLen, 4096);
    assert.equal(v.storageLen, "native storage payload".length);
    assert.equal(v.viaUrlLen, "native storage payload".length);
    assert.equal(v.afterDelete, null);
    assert.equal(v.scheduledIdIsString, true);
    const rows = await pollUntil(async () => {
      const q = await query("data:notes", { tag });
      return q.ok && q.value.length === 2 ? q.value : undefined;
    }, "scheduled mutation to run");
    assert.deepEqual(rows.map((n) => n.from).sort(), ["native", "scheduled-native"]);
  });
});

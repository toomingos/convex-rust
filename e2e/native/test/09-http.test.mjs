// F-03: #[http_action] on the native runtime (V8 rustHttpAction -> internal native action call).
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { SITE_PORT, setup, stop } from "../lib/harness.mjs";

const SITE = `http://127.0.0.1:${SITE_PORT}`;

describe("native http actions", () => {
  before(() => setup("http"));
  after(() => stop());

  it("JSON route: status, headers, body", async () => {
    const res = await fetch(`${SITE}/info?a=1`, { headers: { "x-test": "hello" } });
    assert.equal(res.status, 201);
    assert.equal(res.headers.get("x-reply"), "native");
    assert.match(res.headers.get("content-type"), /application\/json/);
    assert.deepEqual(await res.json(), { method: "GET", path: "/info", query: "a=1", x: "hello" });
  });

  it("POST body echo (binary safe, large)", async () => {
    const bin = Buffer.from(Array.from({ length: 256 }, (_, i) => i));
    let res = await fetch(`${SITE}/echo`, { method: "POST", headers: { "content-type": "application/x-bin" }, body: bin });
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("content-type"), "application/x-bin");
    assert.deepEqual(Buffer.from(await res.arrayBuffer()), bin);
    const big = Buffer.alloc(3 * 1024 * 1024, "ab");
    res = await fetch(`${SITE}/echo`, { method: "POST", body: big });
    assert.deepEqual(Buffer.from(await res.arrayBuffer()), big);
    res = await fetch(`${SITE}/echo`, { method: "POST" });
    assert.equal((await res.arrayBuffer()).byteLength, 0);
  });

  it("calls back into the deployment (ctx.run_query)", async () => {
    const res = await fetch(`${SITE}/double`, { method: "POST", body: JSON.stringify({ n: 21 }) });
    assert.equal(await res.json(), 42);
  });

  it("streaming handler comes back whole", async () => {
    const res = await fetch(`${SITE}/stream`, { method: "POST" });
    assert.equal(res.status, 200);
    assert.equal(await res.text(), "tick 0\ntick 1\ntick 2\n");
  });

  it("handler error and panic give a 500", async () => {
    for (const [name, marker] of [["fail", "http boom"], ["panics", "http panic marker"]]) {
      const res = await fetch(`${SITE}/${name}`, { method: "POST" });
      assert.equal(res.status, 500, name);
      await res.text();
    }
    // The worker still serves after a panic.
    const ok = await fetch(`${SITE}/echo`, { method: "POST", body: "still alive" });
    assert.equal(await ok.text(), "still alive");
  });
});

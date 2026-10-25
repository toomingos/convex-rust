// RUST_NATIVE_ALLOW_NETWORK=1 lets a native action open sockets (DESIGN 8.2 suite 3).
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { MOCK_PORT, call, setup, stop } from "../lib/harness.mjs";

describe("native network allowed", () => {
  before(() => setup("network", { RUST_NATIVE_ALLOW_NETWORK: "1" }));
  after(() => stop());

  it("TcpStream::connect to the mock server works", async () => {
    const r = await call("rust/isolation:tcp_connect", { addr: `127.0.0.1:${MOCK_PORT}` });
    assert.ok(r.ok, r.message);
    assert.equal(r.value.ok, true, JSON.stringify(r.value));
  });
});

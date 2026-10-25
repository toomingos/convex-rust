import test, { after } from "node:test";
import assert from "node:assert/strict";
import { cleanup, makeApp, push, callRun, write, read, constAction, cli, clientCall } from "../lib/harness.mjs";

after(cleanup);

test("1. fresh `convex rust init` app deploys and its example action runs", async () => {
  const app = await makeApp();
  assert.match(app.initOut, /created\s+convex\/rust\/Cargo\.toml/);
  const p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.match(p.out, /Building Rust functions/);
  const r = await callRun(app, "rust/example:greet", { name: "Ada" });
  assert.equal(r.code, 0, r.out);
  assert.match(r.stdout, /Hello, Ada!/);
});

test("2. cargo compile error: push fails with file:line, previous deployment keeps serving", async () => {
  const app = await makeApp({ overlays: [] });
  write(app, "convex/rust/src/example.rs", constAction("greet", "v1"));
  let p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.match((await callRun(app, "rust/example:greet", {})).stdout, /v1/);

  // type error inside the #[action]: returns i32 where String is expected
  const broken = constAction("greet", "v2").replace('Ok("v2".to_string())', "Ok(42)");
  write(app, "convex/rust/src/example.rs", broken);
  p = await push(app);
  assert.notEqual(p.code, 0, "push must fail");
  assert.match(p.out, /error\[E0308\]: mismatched types/);
  assert.match(p.out, /src\/example\.rs:\d+:\d+/, "cargo's file:line must be shown");
  assert.match(p.out, /Rust build failed/);
  // nothing deployed: the old module still answers v1 (not v2, not an error)
  const r = await callRun(app, "rust/example:greet", {});
  assert.equal(r.code, 0, r.out);
  assert.match(r.stdout, /v1/);
  const c = await clientCall("action", "rust/example:greet", {});
  assert.equal(c.body?.value, "v1", JSON.stringify(c));
});

test("3. redeploy after changing a return value serves the new value (no stale module)", async () => {
  const app = await makeApp();
  const seen = [];
  for (const v of ["alpha", "beta", "gamma"]) {
    write(app, "convex/rust/src/example.rs", constAction("greet", v));
    const p = await push(app);
    assert.equal(p.code, 0, p.out);
    const r = await callRun(app, "rust/example:greet", {});
    assert.equal(r.code, 0, r.out);
    seen.push(r.stdout.trim());
    assert.match(r.stdout, new RegExp(v));
  }
  // flip back to an earlier value (hash seen before) -> still correct
  write(app, "convex/rust/src/example.rs", constAction("greet", "alpha"));
  assert.equal((await push(app)).code, 0);
  assert.match((await callRun(app, "rust/example:greet", {})).stdout, /alpha/);
  assert.equal(new Set(seen).size, 3);
  void read; void cli;
});

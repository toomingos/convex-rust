import test, { after } from "node:test";
import assert from "node:assert/strict";
import { cleanup, makeApp, push, read, write } from "../lib/harness.mjs";

after(cleanup);

test("11. DX-04: a C dependency (openssl-sys) fails the push with cargo's output plus an actionable hint", async () => {
  const app = await makeApp();
  const cargo = read(app, "convex/rust/Cargo.toml");
  write(app, "convex/rust/Cargo.toml", cargo.replace("[dependencies]\n", '[dependencies]\nopenssl-sys = "0.9"\n'));
  const p = await push(app);
  if (process.env.DEPLOY_DUMP) console.log(p.out);
  assert.notEqual(p.code, 0, "push must fail");
  // cargo's own output is kept
  assert.match(p.out, /failed to run custom build command for `openssl-sys/);
  assert.match(p.out, /Rust build failed/);
  // the hint
  assert.match(p.out, /Hint: `openssl-sys` needs a C library \(OpenSSL\)/);
  assert.match(p.out, /rustls/);
  assert.match(p.out, /"use node"/);
  // names the crate chain (cargo tree -i): the user's crate depends on it directly
  assert.match(p.out, /openssl-sys v0\.9\.\d+/);
  assert.match(p.out, /convex_rust/);
});

// DESIGN 8.2 suite 3: isolation. Cleared env, filesystem, network, process creation.
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import {
  E2E_SECRET, INSTANCE_SECRET_ENV, MSG, call, dataDir, linuxOnly, setEnv, setup, sleep, stop,
} from "../lib/harness.mjs";

describe("native isolation", () => {
  before(async () => {
    await setup("isolation");
    const e = setEnv("NATIVE_E2E_DEPLOY_VAR", "deploy-value");
    assert.equal(e.status, 0, e.out);
  });
  after(() => stop());

  it("deployment env var is visible", async () => {
    const r = await call("rust/isolation:env_probe", { names: ["NATIVE_E2E_DEPLOY_VAR"] });
    assert.ok(r.ok, r.message);
    assert.equal(r.value.values[0], "deploy-value");
  });

  it("backend secrets are absent (names and values)", async () => {
    const r = await call("rust/isolation:env_probe", { names: ["CONVEX_E2E_SECRET", "INSTANCE_SECRET"] });
    assert.ok(r.ok, r.message);
    assert.deepEqual(r.value.values, [null, null]);
    for (const secret of [E2E_SECRET, INSTANCE_SECRET_ENV])
      assert.ok(!r.value.allValues.some((v) => v.includes(secret)), "a secret leaked into another variable");
  });

  it("host shell environment is not inherited (cleared env)", async () => {
    const r = await call("rust/isolation:env_probe", { names: ["HOME", "USER", "SSH_AUTH_SOCK"] });
    assert.ok(r.ok, r.message);
    // Cleared env: only deployment vars (plus whatever the worker itself needs, e.g. TMPDIR).
    assert.ok(r.value.names.length < 20, `worker env has ${r.value.names.length} vars: ${r.value.names.join(",")}`);
    assert.equal(r.value.values[2], null, "SSH_AUTH_SOCK leaked");
  });

  it("backend data dir is not readable (Landlock EACCES)", { skip: linuxOnly("Landlock") }, async () => {
    const r = await call("rust/isolation:list_dir", { path: dataDir() });
    assert.ok(r.ok, r.message);
    assert.equal(r.value.ok, false, `data dir listed: ${JSON.stringify(r.value)}`);
    assert.equal(r.value.errno, 13, JSON.stringify(r.value));
    const f = await call("rust/isolation:read_path", { path: `${dataDir()}/secret` });
    assert.equal(f.value.ok, false, "backend secret file readable");
  });

  it("TcpStream::connect is refused by default (EPERM)", { skip: linuxOnly("seccomp socket filter") }, async () => {
    const r = await call("rust/isolation:tcp_connect", { addr: "127.0.0.1:3368" });
    // seccomp returns EPERM; a stricter profile may kill with SIGSYS instead. Both are "blocked".
    if (r.ok) {
      assert.equal(r.value.ok, false);
      assert.equal(r.value.errno, 1, JSON.stringify(r.value));
    } else assert.match(r.message, MSG.sandbox);
  });

  it("Command::new(\"sh\") is stopped by the sandbox", { skip: linuxOnly("seccomp execve filter") }, async () => {
    const r = await call("rust/isolation:spawn_process", { cmd: "sh" });
    if (r.ok) assert.equal(r.value.ok, false, `sh ran: ${JSON.stringify(r.value)}`);
    else assert.match(r.message, MSG.sandbox);
    await sleep(100);
    const next = await call("rust/crash:probe");
    assert.ok(next.ok, next.message);
  });

  it("a write in the temp dir works", async () => {
    const r = await call("rust/isolation:tmp_write");
    assert.ok(r.ok, r.message);
    assert.deepEqual([r.value.ok, r.value.content], [true, "hello tmp"], JSON.stringify(r.value));
  });

  it("a crash does not affect a concurrent call on another worker", async () => {
    const slow = call("rust/lifecycle:slow_version", { ms: 2500 });
    await sleep(300);
    const crash = await call("rust/crash:segfault");
    assert.equal(crash.ok, false);
    assert.match(crash.message, MSG.signal("SIGSEGV"));
    const r = await slow;
    assert.ok(r.ok, `concurrent call died with the crashed one: ${r.message}`);
  });
});

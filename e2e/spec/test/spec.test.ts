// Dual-runtime spec tests: every behaviour is implemented as a "use node" action
// (convex/nodeSpec.ts) and a Rust #[action] (convex/rust/src/spec.rs); each test
// runs both against the real forked backend and asserts identical observable results.
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { createHash, randomUUID } from "node:crypto";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { existsSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import {
  MOCK, SITE_URL, SPEC_DIR, convexCli, fnPath, pollUntil, run, runPath, runWithLogs, start, stop,
  type Outcome, type Runtime,
} from "./harness.ts";
import { Prng } from "../../../bench/shared/prng.ts";

const RTS: Runtime[] = ["node", "rust"];
const fnv = (b: Uint8Array) => {
  let h = 0x811c9dc5;
  for (const x of b) h = Math.imul(h ^ x, 0x01000193) >>> 0;
  return h;
};

/** Run on both runtimes, assert success + identical result, return it. */
async function same(name: string, args: any, identity?: Record<string, unknown>) {
  const [n, r] = await Promise.all(RTS.map((rt) => run(rt, name, args, identity)));
  assert.ok(n.ok, `node failed: ${JSON.stringify(n)}`);
  assert.ok(r.ok, `rust failed: ${JSON.stringify(r)}`);
  assert.deepEqual((r as any).value, (n as any).value, "rust and node results differ");
  return (n as any).value;
}
/** Run on both, assert both fail; returns the two failures. */
async function bothFail(name: string, args: any) {
  const [n, r] = (await Promise.all(RTS.map((rt) => run(rt, name, args)))) as Outcome[];
  assert.equal(n.ok, false, `node unexpectedly succeeded: ${JSON.stringify(n)}`);
  assert.equal(r.ok, false, `rust unexpectedly succeeded: ${JSON.stringify(r)}`);
  return [n, r] as [Extract<Outcome, { ok: false }>, Extract<Outcome, { ok: false }>];
}

describe("dual-runtime specs", () => {
before(async () => {
  await start();
  const push = convexCli(SPEC_DIR, ["dev", "--once"]);
  assert.equal(push.status, 0, `push failed:\n${push.out}`);
  for (const [k, v] of [
    ["SPEC_ENV", "hello-env"],
    ["SPEC_ENV_UNI", "héllo wörld ✓ a=b"],
  ]) {
    const e = convexCli(SPEC_DIR, ["env", "set", k, v]);
    assert.equal(e.status, 0, `env set failed:\n${e.out}`);
  }
});
after(() => stop());

describe("W3-01 actions calling functions", () => {
  it("echo: arguments and results round-trip identically", async () => {
    const value = { s: "héllo", n: 1.5, i: 3, b: true, z: null, arr: [1, "a", [2]], o: { x: { y: [] } } };
    const out = await same("echo", { value });
    assert.deepEqual(out, { value });
  });

  it("runQuery / runMutation from both runtimes", async () => {
    for (const rt of RTS) {
      const q = await run(rt, "call", { kind: "query", name: "data:double", args: { n: 21 } });
      assert.deepEqual(q, { ok: true, value: 42 }, rt);
      const tag = `rm-${rt}-${randomUUID()}`;
      const m = await run(rt, "call", { kind: "mutation", name: "data:addNote", args: { tag } });
      assert.ok(m.ok && typeof m.value === "string", `${rt} mutation id`);
      const notes = await runPath("query", "data:notes", { tag });
      assert.deepEqual(notes, { ok: true, value: [{ tag, from: null }] }, rt);
    }
  });

  for (const from of RTS)
    for (const to of RTS)
      it(`runAction: ${from} action calls ${to} action`, async () => {
        const r = await run(from, "call", { kind: "action", name: fnPath(to, "ping"), args: {} });
        assert.deepEqual(r, { ok: true, value: "pong" });
      });

  it("runAction forwards args and results across runtimes", async () => {
    const value = { k: [1, 2, { z: "y" }] };
    for (const from of RTS)
      for (const to of RTS) {
        const r = await run(from, "call", { kind: "action", name: fnPath(to, "echo"), args: { value } });
        assert.deepEqual(r, { ok: true, value: { value } }, `${from}->${to}`);
      }
  });
});

describe("W3-01 errors", () => {
  it("thrown Error: client sees a failure and the message", async () => {
    const [n, r] = await bothFail("fail", { message: "boom-msg-123" });
    assert.equal(n.convex, false);
    assert.equal(r.convex, false);
    assert.match(n.message, /boom-msg-123/);
    assert.match(r.message, /boom-msg-123/);
  });

  it("ConvexError with data reaches the client as ConvexError(data)", async () => {
    const data = { code: "E_BAD", details: [1, 2, { x: "y" }], n: 3.5 };
    const [n, r] = await bothFail("failConvex", { data });
    assert.equal(n.convex, true);
    assert.equal(r.convex, true);
    assert.deepEqual(n.data, data);
    assert.deepEqual(r.data, data);
  });

  it("ConvexError with a string payload", async () => {
    const [n, r] = await bothFail("failConvex", { data: "plain string" });
    assert.deepEqual([n.convex, n.data], [true, "plain string"]);
    assert.deepEqual([r.convex, r.data], [true, "plain string"]);
  });

  it("uncaught exception (node) / panic (rust): plain server error, not a ConvexError", async () => {
    const [n, r] = await bothFail("crash", {});
    assert.equal(n.convex, false);
    assert.equal(r.convex, false);
    assert.match(n.message, /kaboom/);
    console.log("  panic client message:", JSON.stringify(r.message));
    // DX-08: the panic message and location reach the client, not a bare wasm trap.
    assert.match(r.message, /kaboom/);
    assert.match(r.message, /spec\.rs:\d+/);
    assert.doesNotMatch(r.message, /unreachable|WebAssembly trap/);
  });

  it("argument validation failure: both reject before running", async () => {
    for (const args of [{ n: "not a number" }, {}]) {
      const [n, r] = await bothFail("strictArgs", args);
      assert.equal(n.convex, false);
      assert.equal(r.convex, false);
    }
    assert.deepEqual(await same("strictArgs", { n: 21 }), 42);
  });

  it("errors from a called function can be caught and inspected (ConvexError data, plain Error, query)", async () => {
    const data = { code: "inner", list: [1, 2] };
    const cases: [string, string, any, boolean, any, RegExp | null][] = [
      ["action", "nodeSpec:failConvex", { data }, true, data, null],
      ["action", "rust/spec:fail_convex", { data }, true, data, null],
      ["action", "nodeSpec:fail", { message: "inner-boom" }, false, null, /inner-boom/],
      ["action", "rust/spec:fail", { message: "inner-boom" }, false, null, /inner-boom/],
      ["query", "data:boom", {}, true, { code: "boom", n: 7 }, null],
      ["action", "nope:doesNotExist", {}, false, null, /nope|not find|Could not/i],
    ];
    for (const rt of RTS)
      for (const [kind, name, args, isCE, ceData, re] of cases) {
        const out = await run(rt, "callCatch", { kind, name, args });
        assert.ok(out.ok, `${rt} ${name}: ${JSON.stringify(out)}`);
        const v = out.value;
        assert.equal(v.ok, false, `${rt} ${name}`);
        assert.equal(v.isConvexError, isCE, `${rt} ${name} isConvexError`);
        assert.deepEqual(v.data, ceData, `${rt} ${name} data`);
        if (re) assert.match(v.message, re, `${rt} ${name} message`);
      }
  });
});

describe("W3-01 visibility", () => {
  it("internal actions are not callable from the client (either runtime)", async () => {
    for (const rt of RTS) {
      // No admin auth: an admin key may call internal functions (dashboard behaviour).
      const out = await runPath("action", fnPath(rt, "internalPing"), {}, undefined, true);
      assert.equal(out.ok, false, `${rt} internal fn was callable from client`);
      assert.match((out as any).message, /internal|not find|public/i);
      const pub = await runPath("action", fnPath(rt, "ping"), {}, undefined, true);
      assert.deepEqual(pub, { ok: true, value: "pong" }, `${rt} public fn via anonymous client`);
    }
  });
  it("admin key can call internal actions (both runtimes)", async () => {
    for (const rt of RTS) assert.deepEqual(await runPath("action", fnPath(rt, "internalPing"), {}), { ok: true, value: "pong" });
  });
  it("public actions are callable; internal actions are callable from other actions", async () => {
    for (const rt of RTS) assert.deepEqual(await run(rt, "ping"), { ok: true, value: "pong" });
    for (const from of RTS)
      for (const to of RTS) {
        const r = await run(from, "call", { kind: "action", name: fnPath(to, "internalPing"), args: {} });
        assert.deepEqual(r, { ok: true, value: "pong" }, `${from} -> internal ${to}`);
      }
  });
});

describe("W3-01 scheduler", () => {
  const targets: [string, (rt: Runtime) => string, string | null][] = [
    ["mutation", () => "data:addNote", null],
    ["node action", () => "nodeSpec:record", "node"],
    ["rust action", () => "rust/spec:record", "rust"],
  ];
  for (const rt of RTS)
    for (const kind of ["after", "at"])
      for (const [label, target, from] of targets)
        it(`${rt}: ctx.scheduler.run${kind === "at" ? "At" : "After"} -> ${label}`, async () => {
          const tag = `sch-${rt}-${kind}-${randomUUID()}`;
          const t = target(rt);
          const sched = await run(rt, "schedule", { kind, target: t, args: { tag }, delayMs: 300 });
          assert.deepEqual(sched, { ok: true, value: { idIsString: true } });
          const notes = await pollUntil(async () => {
            const n = await runPath("query", "data:notes", { tag });
            return n.ok && n.value.length > 0 ? n.value : undefined;
          }, `scheduled ${t}`);
          assert.deepEqual(notes, [{ tag, from }]);
          const states = await runPath("query", "data:scheduledStates", {});
          assert.ok(states.ok);
          assert.ok(
            // The system table stores the module file name ("data.js:addNote").
            (states.value as any[]).some((s) => s.name.replace(".js:", ":") === t && s.state === "success"),
            `no successful _scheduled_functions row for ${t}: ${JSON.stringify(states)}`,
          );
        });

  it("scheduled in the future does not run early", async () => {
    for (const rt of RTS) {
      const tag = `fut-${rt}-${randomUUID()}`;
      await run(rt, "schedule", { kind: "after", target: "data:addNote", args: { tag }, delayMs: 60_000 });
      await new Promise((r) => setTimeout(r, 700));
      assert.deepEqual(await runPath("query", "data:notes", { tag }), { ok: true, value: [] }, rt);
    }
  });
});

describe("W3-01 auth", () => {
  it("no identity -> null in both runtimes", async () => {
    assert.equal(await same("whoami", {}), null);
  });
  it("acting-as identity (admin key + identity, as `convex run --identity`) is visible identically", async () => {
    const identity = { subject: "user-42", issuer: "https://e2e.test", name: "Ada", email: "ada@e2e.test" };
    const v = await same("whoami", {}, identity);
    assert.equal(v.subject, "user-42");
    assert.equal(v.issuer, "https://e2e.test");
    assert.equal(v.name, "Ada");
    assert.equal(v.email, "ada@e2e.test");
    assert.equal(v.tokenIdentifier, "https://e2e.test|user-42");
  });
});

describe("W3-02 storage", () => {
  it("store / getUrl / getMetadata / get / delete behave identically", async () => {
    const bytes = new Prng(11).bytes(5000);
    const data = Buffer.from(bytes).toString("base64");
    const contentType = "application/x-spec";
    const v = await same("storageRoundtrip", { data, contentType });
    assert.equal(v.meta.size, 5000);
    assert.equal(v.meta.contentType, contentType);
    assert.equal(v.meta.sha256Len > 0, true);
    assert.deepEqual(v.meta.keys, ["contentType", "sha256", "size", "storageId"]);
    assert.equal(v.gotLen, 5000);
    assert.equal(v.gotSum, fnv(bytes));
    assert.equal(v.viaUrlSum, fnv(bytes));
    assert.equal(v.gotContentType, contentType);
    assert.deepEqual(v.afterDelete, { get: null, url: null, meta: null });
  });
  it("empty blob", async () => {
    const v = await same("storageRoundtrip", { data: "", contentType: "text/plain" });
    assert.equal(v.meta.size, 0);
    assert.equal(v.gotLen, 0);
  });
});

describe("W3-02 fetch (bench mock server)", () => {
  it("GET bytes", async () => {
    const bytes = new Prng(7).bytes(4096);
    const v = await same("fetchGet", { url: `${MOCK}/bytes/4096?seed=7` });
    assert.equal(v.status, 200);
    assert.equal(v.len, 4096);
    assert.equal(v.sum, fnv(bytes));
    assert.equal(v.contentType, "application/octet-stream");
  });
  it("POST echo", async () => {
    const body = Buffer.from(new Prng(5).bytes(20_000));
    const v = await same("fetchPost", { url: `${MOCK}/echo`, body: body.toString("base64") });
    assert.deepEqual(v, { status: 200, json: { length: 20_000, sha256: createHash("sha256").update(body).digest("hex") } });
  });
  it("redirects are followed (up to 20 hops, final URL reported)", async () => {
    const v = await same("fetchGet", { url: `${MOCK}/redirect/3` });
    assert.equal(v.status, 200);
    assert.equal(v.url, `${MOCK}/redirect/0?t=3`);
  });
  it("non-2xx status is returned, not thrown", async () => {
    const v = await same("fetchGet", { url: `${MOCK}/nothere` });
    assert.equal(v.status, 404);
    assert.equal(v.ok, false);
  });
  it("network error rejects", async () => {
    assert.deepEqual(await same("fetchFail", { url: "http://127.0.0.1:1/" }), { threw: true });
  });
  it("streaming body (ctx.fetch_stream in rust, ReadableStream in node)", async () => {
    const bytes = new Prng(3).bytes(3_000_000);
    const url = `${MOCK}/stream/3000000?seed=3`;
    const [n, r] = await Promise.all(RTS.map((rt) => run(rt, "fetchStream", { url })));
    assert.ok(n.ok && r.ok, JSON.stringify([n, r]));
    for (const o of [n, r] as any[]) {
      assert.equal(o.value.status, 200);
      assert.equal(o.value.len, 3_000_000);
      assert.equal(o.value.sum, fnv(bytes));
    }
    // the host may merge adjacent chunks (up to 1 MiB), so the body must exceed that to arrive in several
    assert.equal((r as any).value.multiChunk, true, "rust stream arrived in a single chunk");
  });
  it("large body via ctx.fetch", async () => {
    const bytes = new Prng(9).bytes(3_000_000);
    const v = await same("fetchGet", { url: `${MOCK}/bytes/3000000?seed=9` });
    assert.equal(v.len, 3_000_000);
    assert.equal(v.sum, fnv(bytes));
  });
});

describe("W3-02 environment variables", () => {
  it("deployment env vars (convex env set) are visible in both runtimes", async () => {
    assert.equal(await same("readEnv", { name: "SPEC_ENV" }), "hello-env");
    assert.equal(await same("readEnv", { name: "SPEC_ENV_UNI" }), "héllo wörld ✓ a=b");
  });
  it("unset var -> null/undefined in both", async () => {
    assert.equal(await same("readEnv", { name: "SPEC_DEFINITELY_UNSET" }), null);
  });
  it("updated var is seen by the next call", async () => {
    assert.equal(convexCli(SPEC_DIR, ["env", "set", "SPEC_ENV", "second"]).status, 0);
    assert.equal(await same("readEnv", { name: "SPEC_ENV" }), "second");
  });
});

describe("W3-02 http actions", () => {
  for (const [path, label] of [["/run-node", "node"], ["/run-rust", "rust"]])
    it(`TS httpAction runs a ${label} action`, async () => {
      const value = { a: 1, b: ["x"] };
      const res = await fetch(`${SITE_URL}${path}`, { method: "POST", body: JSON.stringify({ value }) });
      assert.equal(res.status, 200);
      assert.deepEqual(await res.json(), { value });
    });
});

describe("DX-05 logging, env, clock, random", () => {
  it("log/tracing/stdout/stderr become function log lines; env, clock and random work", async () => {
    const { value, logLines } = await runWithLogs("rust/dx05:probe");
    assert.equal(value.env, "héllo wörld ✓ a=b");
    assert.equal(value.wallMsPlausible, true);
    assert.ok(value.elapsedMs >= 25, `Instant elapsed ${value.elapsedMs}ms across a 30ms sleep`);
    assert.equal(value.random02Nonzero, true);
    assert.equal(value.random03Nonzero, true);
    assert.equal(value.randomDiffer, true);
    console.log("  dx05 log lines:", JSON.stringify(logLines));
    const has = (level: string, text: string) =>
      logLines.some((l) => l.includes(text) && l.startsWith(`[${level}]`));
    assert.ok(has("LOG", "dx05 log info"), "log::info! -> [LOG]");
    assert.ok(has("WARN", "dx05 log warn"), "log::warn! -> [WARN]");
    assert.ok(has("WARN", 'dx05 tracing warn user="ann" n=3'), "tracing::warn! with fields -> [WARN]");
    assert.ok(logLines.some((l) => l.includes("dx05 stdout")), "println!");
    assert.ok(logLines.some((l) => l.includes("dx05 stderr")), "eprintln!");
  });
});

describe("W3-02 components", () => {
  it("a Rust module inside a component is rejected on push", () => {
    const app = join(SPEC_DIR, "component-app");
    const inst = spawnSync("npm", ["install", "--no-audit", "--no-fund"], { cwd: app, encoding: "utf8" });
    assert.equal(inst.status, 0, inst.stderr);
    // The relative file: link only resolves in the main checkout; in a worktree link the same package.
    const linked = join(app, "node_modules/convex");
    if (!existsSync(join(linked, "package.json"))) {
      rmSync(linked, { recursive: true, force: true });
      symlinkSync(realpathSync(join(SPEC_DIR, "node_modules/convex")), linked);
    }
    const push = convexCli(app, ["dev", "--once"]);
    assert.notEqual(push.status, 0, `push unexpectedly succeeded:\n${push.out}`);
    assert.match(push.out, /Rust actions are not supported in components/);
    console.log("  component push output tail:", push.out.trim().split("\n").slice(-4).join(" | "));
  });
});
});

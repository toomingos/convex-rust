import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createServer } from "node:http";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConvexError } from "convex/values";
import { getFunctionAddress } from "convex/server";
import { runRust } from "../src/runtime/index.js";
import { vectorFilter } from "../src/runtime/host.js";
import { decodeBase64, encodeBase64 } from "../src/runtime/base64.js";
import { LineBuffer, makeWasi } from "../src/runtime/wasi.js";

const wasm = readFileSync(fileURLToPath(new URL("./fixtures/guest.wasm", import.meta.url)));
// The fixture's allocator is a bump allocator that never frees: give each test a fresh instance.
let module = new WebAssembly.Module(wasm);

function fakeCtx(over: Record<string, any> = {}) {
  const calls: any[] = [];
  const ctx: any = {
    runQuery: vi.fn(async (ref: any, args: any) => {
      calls.push(["query", ref, args]);
      return [{ _id: "a" }, { _id: "b" }];
    }),
    runMutation: vi.fn(async (ref: any, args: any) => {
      calls.push(["mutation", ref, args]);
      return null;
    }),
    runAction: vi.fn(async () => "act"),
    scheduler: { runAt: vi.fn(async () => "job1"), cancel: vi.fn(async () => undefined) },
    auth: { getUserIdentity: vi.fn(async () => ({ subject: "u1" })) },
    storage: {
      store: vi.fn(async (b: Blob) => `stored:${b.size}:${b.type}`),
      get: vi.fn(async (id: string) => (id === "none" ? null : new Blob(["file-bytes"], { type: "text/plain" }))),
      getUrl: vi.fn(async () => "http://x/y"),
      delete: vi.fn(async () => undefined),
    },
    ...over,
  };
  return { ctx, calls };
}

const run = (ctx: any, name: string, args: any = {}) =>
  runRust(ctx, module, `convex_run:t::${name}`, args);
const proxy = (ctx: any, req: any) => run(ctx, "proxy", req);

let logs: Record<string, string[]>;
beforeEach(() => {
  module = new WebAssembly.Module(wasm);
  logs = { log: [], debug: [], warn: [], error: [] };
  for (const k of Object.keys(logs)) {
    vi.spyOn(console, k as any).mockImplementation((...a: any[]) => void logs[k].push(a.join(" ")));
  }
});
afterEach(() => vi.restoreAllMocks());

describe("JSPI available in the test runtime", () => {
  it("has WebAssembly.Suspending", () => {
    expect(typeof (WebAssembly as any).Suspending).toBe("function");
  });
});

describe("envelope handling", () => {
  it("returns ok values through jsonToConvex (int64, bytes)", async () => {
    const { ctx } = fakeCtx();
    const out = await run(ctx, "echo", { a: 1n, b: new Uint8Array([1, 2, 3]).buffer });
    expect(out).toEqual({ a: 1n, b: new Uint8Array([1, 2, 3]).buffer });
  });
  it("calls _initialize first", async () => {
    expect(await run(fakeCtx().ctx, "inited")).toBe(true);
  });
  it("user error -> Error(message)", async () => {
    await expect(run(fakeCtx().ctx, "err")).rejects.toThrow(/^bad$/);
  });
  it("convex error -> ConvexError(data)", async () => {
    const e: any = await run(fakeCtx().ctx, "cerr").catch((x) => x);
    expect(e).toBeInstanceOf(ConvexError);
    expect(e.data).toEqual({ code: 42 });
  });
  it("invalid envelope -> ABI error", async () => {
    await expect(run(fakeCtx().ctx, "notjson")).rejects.toThrow(/broke the guest ABI/);
    await expect(run(fakeCtx().ctx, "neither")).rejects.toThrow(/neither `ok` nor `error`/);
  });
  it("missing export -> Could not find function", async () => {
    await expect(runRust(fakeCtx().ctx, module, "convex_run:t::a:nope", {})).rejects.toThrow(
      "Could not find function nope in module rust/a.js",
    );
  });
  it("panic convention: last 'Uncaught panic:' line becomes the error", async () => {
    const { ctx } = fakeCtx();
    await expect(run(ctx, "panic")).rejects.toThrow(/^boom at src\/x.rs:1:2$/);
    expect(logs.error).toContain("Uncaught panic: boom at src/x.rs:1:2");
    expect(logs.log).toContain("out1\npartial");
  });
  it("trap without panic line -> trap message", async () => {
    await expect(run(fakeCtx().ctx, "trap")).rejects.toThrow(/WebAssembly trap/);
  });
});

describe("log and WASI", () => {
  it("log levels map to console.*", async () => {
    await run(fakeCtx().ctx, "print");
    expect(logs.debug).toEqual(["now"]);
    expect(logs.log[0]).toBe("now");
    expect(logs.warn).toEqual(["now"]);
  });
  it("fd_write splits lines; a trailing partial line is flushed at the end", async () => {
    await run(fakeCtx().ctx, "print");
    expect(logs.log).toContain("out1");
    expect(logs.log).toContain("partial");
    expect(logs.error).toContain("err1");
  });
  it("unimplemented WASI functions return ENOSYS", async () => {
    // fd_close is a real call while the in-memory /tmp is on; with /tmp off it is not implemented.
    expect(await runRust(fakeCtx().ctx, module, "convex_run:t::enosys", {}, { tmpBytes: 0 })).toBe(52);
  });
  it("environ, clock, random", () => {
    const memory = new WebAssembly.Memory({ initial: 8 });
    const lb = new LineBuffer(() => {});
    const w = makeWasi({ memory: () => memory, environ: ["A=1", "BB=22"], line: () => {} }, { 1: lb, 2: lb });
    const dv = new DataView(memory.buffer);
    expect(w.environ_sizes_get(0, 4)).toBe(0);
    expect(dv.getUint32(0, true)).toBe(2);
    expect(dv.getUint32(4, true)).toBe(4 + 6);
    w.environ_get(16, 64);
    const p0 = dv.getUint32(16, true);
    const p1 = dv.getUint32(20, true);
    const bytes = new Uint8Array(memory.buffer);
    expect(new TextDecoder().decode(bytes.subarray(p0, p0 + 3))).toBe("A=1");
    expect(new TextDecoder().decode(bytes.subarray(p1, p1 + 5))).toBe("BB=22");
    expect(w.clock_time_get(0, 0n, 200)).toBe(0);
    const ms = Number(dv.getBigUint64(200, true) / 1_000_000n);
    expect(Math.abs(ms - Date.now())).toBeLessThan(5000);
    expect(w.random_get(300, 200000)).toBe(0);
    expect(bytes.subarray(300, 300 + 200000).some((b) => b !== 0)).toBe(true);
    expect(() => w.proc_exit(3)).toThrow("proc_exit(3)");
  });
  it("pointers at or above 2 GiB (negative i32) and out of range do not read or write elsewhere", () => {
    const memory = new WebAssembly.Memory({ initial: 1 });
    const lines: string[] = [];
    const lb = new LineBuffer((l) => lines.push(l));
    const w = makeWasi({ memory: () => memory, environ: [], line: () => {} }, { 1: lb, 2: lb });
    expect(w.random_get(-16, 8)).toBe(28); // 0xfffffff0, outside memory
    expect(w.random_get(65530, 100)).toBe(28);
    expect(w.fd_write(1, -8, 1, 0)).toBe(28);
    new DataView(memory.buffer).setUint32(0, 0x80000000, true); // iovec base >= 2 GiB
    new DataView(memory.buffer).setUint32(4, 4, true);
    expect(w.fd_write(1, 0, 1, 16)).toBe(28);
    expect(lines).toEqual([]);
  });
  it("LineBuffer flushes unbounded output without a newline and keeps lines intact", () => {
    const lines: string[] = [];
    const lb = new LineBuffer((l) => lines.push(l));
    const big = new Uint8Array(40 * 1024).fill(97);
    lb.write(big);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toHaveLength(40 * 1024);
    lb.write(new TextEncoder().encode("ab"));
    lb.write(new TextEncoder().encode("c\nd"));
    lb.finish();
    expect(lines.slice(1)).toEqual(["abc", "d"]);
  });
});

describe("sync_syscall", () => {
  it("now -> Date.now()", async () => {
    const r: any = await run(fakeCtx().ctx, "sync");
    expect(Math.abs(r.ok - Date.now())).toBeLessThan(5000);
  });
  it("unknown op -> error reply", async () => {
    const r: any = await run(fakeCtx().ctx, "syncunknown");
    expect(r.error.message).toBe("unknown sync op {}");
  });
});

describe("function addresses", () => {
  const sys = (ctx: any, name: string, args: any) => proxy(ctx, { type: "syscall", name, args });
  it("functionHandle goes through as the handle string (query/mutation/action/schedule)", async () => {
    const { ctx } = fakeCtx();
    const h = "function://abc";
    await sys(ctx, "1.0/actions/query", { functionHandle: h, args: {} });
    await sys(ctx, "1.0/actions/mutation", { functionHandle: h, args: {} });
    await sys(ctx, "1.0/actions/action", { functionHandle: h, args: {} });
    await sys(ctx, "1.0/actions/schedule", { functionHandle: h, ts: 1, args: {} });
    expect(ctx.runQuery.mock.calls[0][0]).toBe(h);
    expect(ctx.runMutation.mock.calls[0][0]).toBe(h);
    expect(ctx.runAction.mock.calls[0][0]).toBe(h);
    expect(ctx.scheduler.runAt.mock.calls[0][0]).toBe(1000);
    expect(ctx.scheduler.runAt.mock.calls[0][1]).toBe(h);
  });
  it("reference becomes an object carrying Symbol.for(toReferencePath)", async () => {
    const { ctx } = fakeCtx();
    await sys(ctx, "1.0/actions/query", { reference: "_reference/childComponent/c/f", args: {} });
    await sys(ctx, "1.0/actions/schedule", { reference: "_reference/childComponent/c/g", ts: 2, args: {} });
    const sym = Symbol.for("toReferencePath");
    expect(ctx.runQuery.mock.calls[0][0][sym]).toBe("_reference/childComponent/c/f");
    expect(ctx.scheduler.runAt.mock.calls[0][1][sym]).toBe("_reference/childComponent/c/g");
  });
  it("name still builds a function reference; no address is an error", async () => {
    const { ctx } = fakeCtx();
    await sys(ctx, "1.0/actions/query", { name: "m:l", args: {} });
    expect(getFunctionAddress(ctx.runQuery.mock.calls[0][0])).toEqual({ name: "m:l" });
    const r: any = await sys(ctx, "1.0/actions/query", { args: {} });
    expect(r[0].error.message).toMatch(/missing `name`/);
  });
});

describe("vectorSearch / generateUploadUrl / passthrough / componentArgument", () => {
  const sys = (ctx: any, name: string, args: any) => proxy(ctx, { type: "syscall", name, args });
  afterEach(() => {
    delete (globalThis as any).Convex;
  });
  it("vectorSearch maps indexName and limit", async () => {
    const vectorSearch = vi.fn(async (..._a: any[]) => [{ _id: "d1", _score: 0.5 }]);
    const { ctx } = fakeCtx({ vectorSearch });
    const r: any = await sys(ctx, "1.0/actions/vectorSearch", {
      query: { indexName: "docs.by_emb", limit: 5, vector: [1, 2], expressions: null },
    });
    expect(r[0].ok).toEqual({ results: [{ _id: "d1", _score: 0.5 }] });
    expect(vectorSearch.mock.calls[0].slice(0, 2)).toEqual(["docs", "by_emb"]);
    expect(vectorSearch.mock.calls[0][2]).toEqual({ vector: [1, 2], limit: 5 });
  });
  it("vectorFilter rebuilds $eq / $or with the public builder", () => {
    const b = { eq: (f: string, v: any) => ["eq", f, v], or: (...x: any[]) => ["or", ...x] };
    const expr = { $or: [{ $eq: [{ $field: "a" }, { $integer: "AQAAAAAAAAA=" }] }, { $eq: [{ $field: "b" }, "x"] }] };
    expect(vectorFilter(b, expr)).toEqual(["or", ["eq", "a", 1n], ["eq", "b", "x"]]);
    expect(() => vectorFilter(b, { $and: [] })).toThrow(/unsupported filter/);
  });
  it("storage.generateUploadUrl", async () => {
    const { ctx } = fakeCtx();
    ctx.storage.generateUploadUrl = vi.fn(async () => "http://up/1");
    const r: any = await sys(ctx, "1.0/storageGenerateUploadUrl", {});
    expect(r[0].ok).toBe("http://up/1");
  });
  it("unknown syscalls pass through to Convex.asyncSyscall", async () => {
    const asyncSyscall = vi.fn(async () => JSON.stringify({ hello: 1 }));
    (globalThis as any).Convex = { asyncSyscall };
    const r: any = await sys(fakeCtx().ctx, "1.0/something", { a: 2 });
    expect(r[0].ok).toEqual({ hello: 1 });
    expect(asyncSyscall).toHaveBeenCalledWith("1.0/something", '{"a":2}');
  });
  it("componentArgument uses Convex.syscall 1.0/componentArgument", async () => {
    const syscall = vi.fn((_op: string, a: string) =>
      JSON.stringify(JSON.parse(a).name === "k" ? { value: { $integer: "AQAAAAAAAAA=" } } : {}),
    );
    (globalThis as any).Convex = { syscall };
    const r: any = await run(fakeCtx().ctx, "synccomp");
    expect(r.ok).toBe(1n);
    expect(syscall).toHaveBeenCalledWith("1.0/componentArgument", '{"name":"k"}');
    syscall.mockImplementation(() => "{}");
    expect((await run(fakeCtx().ctx, "synccomp") as any).ok).toBeNull();
  });
  it("componentArgument without a Convex global is an error reply", async () => {
    const r: any = await run(fakeCtx().ctx, "synccomp");
    expect(r.error.message).toMatch(/componentArgument/);
  });
});

describe("task_submit / task_poll", () => {
  it("malformed JSON -> -1 and a log line", async () => {
    expect(await proxy(fakeCtx().ctx, { type: "bogus" })).toBe("submit-failed");
    expect(logs.error.join("\n")).toContain('task_submit: unknown request type "bogus"');
  });
  it("poll(0) is empty and poll(1) with nothing outstanding is -1", async () => {
    expect(await run(fakeCtx().ctx, "idle")).toBe(true);
  });
  it("runQuery/runMutation/runAction syscalls map to ctx", async () => {
    const { ctx, calls } = fakeCtx();
    const q: any = await proxy(ctx, {
      type: "syscall",
      name: "1.0/actions/query",
      args: { name: "messages:list", args: { n: 1n } },
    });
    expect(q).toEqual([{ type: "done", taskId: 0, ok: [{ _id: "a" }, { _id: "b" }] }]);
    expect(calls[0][2]).toEqual({ n: 1n });
    await proxy(ctx, { type: "syscall", name: "1.0/actions/mutation", args: { name: "m:add", args: {} } });
    expect(ctx.runMutation).toHaveBeenCalled();
    const a: any = await proxy(ctx, { type: "syscall", name: "1.0/actions/action", args: { name: "m:x", args: {} } });
    expect(a[0].ok).toBe("act");
  });
  it("a failing syscall becomes a done/error event (ConvexError keeps data)", async () => {
    const { ctx } = fakeCtx({
      runQuery: async () => {
        throw new ConvexError({ why: "nope" });
      },
    });
    const r: any = await proxy(ctx, { type: "syscall", name: "1.0/actions/query", args: { name: "a:b", args: {} } });
    expect(r[0].error.data).toEqual({ why: "nope" });
    const { ctx: c2 } = fakeCtx({ runQuery: async () => { throw new Error("kaboom"); } });
    const r2: any = await proxy(c2, { type: "syscall", name: "1.0/actions/query", args: { name: "a:b", args: {} } });
    expect(r2[0].error).toEqual({ message: "kaboom" });
  });
  it("schedule converts seconds to ms; identity; unsupported syscall", async () => {
    const { ctx } = fakeCtx();
    const s: any = await proxy(ctx, { type: "syscall", name: "1.0/actions/schedule", args: { name: "m:x", ts: 1729700000.5, args: {} } });
    expect(s[0].ok).toBe("job1");
    expect((ctx.scheduler.runAt as any).mock.calls[0][0]).toBe(1729700000500);
    const i: any = await proxy(ctx, { type: "syscall", name: "1.0/getUserIdentity", args: {} });
    expect(i[0].ok).toEqual({ subject: "u1" });
    const u: any = await proxy(ctx, { type: "syscall", name: "1.0/nope", args: {} });
    expect(u[0].error.message).toBe("Unsupported syscall in the V8 host: 1.0/nope");
  });
  it("sleep", async () => {
    const t = Date.now();
    const r: any = await proxy(fakeCtx().ctx, { type: "sleep", ms: 30 });
    expect(r).toEqual([{ type: "done", taskId: 0, ok: null }]);
    expect(Date.now() - t).toBeGreaterThanOrEqual(25);
  });
  it("storageStore / storageGet (with chunk events)", async () => {
    const { ctx } = fakeCtx();
    const st: any = await proxy(ctx, { type: "storageStore", contentType: "text/plain", body: encodeBase64(new Uint8Array([1, 2, 3])) });
    expect(st[0].ok).toBe("stored:3:text/plain");
    const g: any = await proxy(ctx, { type: "storageGet", storageId: "abc" });
    expect(g[0].ok).toMatchObject({ contentLength: 10, contentType: "text/plain" });
    const streamId = g[0].ok.bodyStreamId;
    const chunks = g.filter((e: any) => e.type === "chunk");
    expect(chunks[0]).toEqual({ type: "chunk", streamId, data: encodeBase64(new TextEncoder().encode("file-bytes")) });
    expect(chunks.at(-1)).toEqual({ type: "chunk", streamId, end: true });
    const none: any = await proxy(ctx, { type: "storageGet", storageId: "none" });
    expect(none).toEqual([{ type: "done", taskId: 0, ok: null }]);
  });
  it("fetch: head, then body chunks, request body and headers", async () => {
    const seen: any[] = [];
    const server = createServer((req, res) => {
      const parts: Buffer[] = [];
      req.on("data", (d) => parts.push(d));
      req.on("end", () => {
        seen.push({ method: req.method, h: req.headers["x-t"], body: Buffer.concat(parts).toString() });
        res.writeHead(200, { "x-reply": "yes", "content-type": "text/plain" });
        res.end("hello fetch");
      });
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    const port = (server.address() as any).port;
    try {
      const r: any = await proxy(fakeCtx().ctx, {
        type: "fetch",
        url: `http://127.0.0.1:${port}/p`,
        method: "POST",
        headers: [["x-t", "1"]],
        body: encodeBase64(new TextEncoder().encode("req-body")),
      });
      expect(seen[0]).toEqual({ method: "POST", h: "1", body: "req-body" });
      const head = r.find((e: any) => e.type === "done").ok;
      expect(head.status).toBe(200);
      expect(head.headers).toContainEqual(["x-reply", "yes"]);
      const body = r
        .filter((e: any) => e.type === "chunk" && e.data)
        .map((e: any) => new TextDecoder().decode(decodeBase64(e.data)))
        .join("");
      expect(body).toBe("hello fetch");
      expect(r.at(-1)).toEqual({ type: "chunk", streamId: head.bodyStreamId, end: true });
    } finally {
      server.close();
    }
  });
  it("fetch network failure -> done/error", async () => {
    const r: any = await proxy(fakeCtx().ctx, { type: "fetch", url: "http://127.0.0.1:1/", method: "GET", headers: [], body: null });
    expect(r[0].type).toBe("done");
    expect(typeof r[0].error.message).toBe("string");
  });
  it("concurrent calls on one ctx are independent", async () => {
    const { ctx } = fakeCtx();
    const rs = await Promise.all(
      [1, 2, 3].map((n) => proxy(ctx, { type: "sleep", ms: 10 * n })),
    );
    for (const r of rs) expect(r).toEqual([{ type: "done", taskId: 0, ok: null }]);
  });
});

describe("base64", () => {
  it("round trips all lengths", () => {
    for (let n = 0; n < 40; n++) {
      const b = Uint8Array.from({ length: n }, (_, i) => (i * 37 + 11) & 255);
      expect(encodeBase64(b)).toBe(Buffer.from(b).toString("base64"));
      expect(Array.from(decodeBase64(encodeBase64(b)))).toEqual(Array.from(b));
    }
  });
});

describe("runtime has no Node built-ins", () => {
  it("sources do not import node:*", async () => {
    const { readdirSync } = await import("node:fs");
    const dir = fileURLToPath(new URL("../src/runtime/", import.meta.url));
    for (const f of readdirSync(dir)) {
      const text = readFileSync(dir + f, "utf8");
      expect(text, f).not.toMatch(/from ["'](node:|fs|path|crypto|child_process)/);
      expect(text.replace(/\/\/.*$/gm, ""), f).not.toMatch(/\bBuffer\b/);
    }
  });
});

describe("instance reuse", () => {
  const traced = async (m: WebAssembly.Module, name: string, args: any = {}) => {
    process.env.CONVEX_RUST_TRACE = "1";
    try {
      logs.log.length = 0;
      let err: unknown;
      try {
        await runRust(fakeCtx().ctx, m, `convex_run:t::${name}`, args);
      } catch (e) {
        err = e;
      }
      const line = logs.log.find((l) => l.startsWith("[convex-rust]")) ?? "";
      return { reused: /reused=true/.test(line), line, err };
    } finally {
      delete process.env.CONVEX_RUST_TRACE;
    }
  };

  it("reuses after a clean call, drops the instance after a trap", async () => {
    const m = new WebAssembly.Module(wasm);
    expect((await traced(m, "echo")).reused).toBe(false);
    const second = await traced(m, "echo");
    expect(second.reused).toBe(true);
    expect(second.line).toMatch(/total=/);
    expect((await traced(m, "err")).reused).toBe(true); // an error result is a clean finish
    const t = await traced(m, "trap");
    expect(t.reused).toBe(true);
    expect(String(t.err)).toMatch(/trap/);
    expect((await traced(m, "echo")).reused).toBe(false); // fresh after the trap
    expect((await traced(m, "echo")).reused).toBe(true);
  });

  it("uses a fresh instance while another call is in flight", async () => {
    const m = new WebAssembly.Module(wasm);
    await traced(m, "echo");
    process.env.CONVEX_RUST_TRACE = "1";
    try {
      logs.log.length = 0;
      const ctx = fakeCtx().ctx;
      const a = runRust(ctx, m, "convex_run:t::proxy", { type: "sleep", ms: 30 });
      const b = runRust(ctx, m, "convex_run:t::proxy", { type: "sleep", ms: 1 });
      const [ra, rb] = await Promise.all([a, b]);
      expect(ra).toEqual(rb);
    } finally {
      delete process.env.CONVEX_RUST_TRACE;
    }
    const lines = logs.log.filter((l) => l.startsWith("[convex-rust]"));
    expect(lines.length).toBe(2);
    // the first call took the warm instance; the overlapping one had to build its own
    expect(lines.filter((l) => /reused=true/.test(l)).length).toBe(1);
    expect(lines.filter((l) => /reused=false/.test(l)).length).toBe(1);
  });
});

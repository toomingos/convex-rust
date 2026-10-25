import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runRust } from "../src/runtime/index.js";

// A scriptable guest (test/fixtures/net-guest): one command per line, see net_guest.rs.
const wasm = readFileSync(fileURLToPath(new URL("./fixtures/net-guest.wasm", import.meta.url)));

function fakeCtx(over: Record<string, any> = {}) {
  return {
    storage: { store: vi.fn(async (b: Blob) => `stored:${b.size}:${b.type}`) },
    ...over,
  } as any;
}

/** Runs the script and returns the printed lines. */
async function script(ctx: any, lines: string[]): Promise<string[]> {
  const module = new WebAssembly.Module(wasm);
  const out = (await runRust(ctx, module, "convex_run:t::script", lines.join("\n") as any)) as string;
  return out.split("\n");
}

const pattern = (n: number) => Uint8Array.from({ length: n }, (_, i) => i % 251);
const event = (line: string) => JSON.parse(line.slice(2));

beforeEach(() => {
  for (const k of ["log", "debug", "warn", "error"]) vi.spyOn(console, k as any).mockImplementation(() => {});
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const fetchReq = (body: string) =>
  `submit {"type":"fetch","url":"http://x.test/p","method":"POST","headers":[["x-a","b"]],"body":${body}}`;

/** A fetch mock that records what it was given and answers 200 "ok". */
function mockFetch(reply: (url: string, init: any) => Promise<Response> | Response) {
  const calls: { url: string; init: any }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: any) => {
      calls.push({ url, init });
      return reply(url, init);
    }),
  );
  return calls;
}

describe("net/caps", () => {
  it("advertises cancel, bodyRef and requestStream", async () => {
    const [r] = await script(fakeCtx(), ["sync net/caps {\"version\":1}"]);
    const caps = JSON.parse(r.slice(2)).ok;
    expect(caps.ops).toEqual(["cancel", "bodyRef", "requestStream"]);
    expect(caps.maxBodyRef).toBe(64 * 1024 * 1024);
  });
  it("net ops other than the known ones are an error reply", async () => {
    const [r] = await script(fakeCtx(), ["sync net/nope {}"]);
    expect(JSON.parse(r.slice(2)).error.message).toContain("unknown sync op net/nope");
  });
});

describe("{ptr,len} request bodies", () => {
  it("fetch: the host copies the bytes out of guest memory at submit", async () => {
    const calls = mockFetch(() => new Response("ok"));
    const lines = await script(fakeCtx(), ["fill 20000", fetchReq('{"ptr":@PTR@,"len":@LEN@}'), "poll", "poll", "poll"]);
    expect(lines[0]).toBe("s:0");
    expect(event(lines[1]).type).toBe("done");
    expect(calls).toHaveLength(1);
    expect(calls[0].init.method).toBe("POST");
    expect(calls[0].init.body).toBeInstanceOf(Uint8Array);
    expect(Buffer.from(calls[0].init.body)).toEqual(Buffer.from(pattern(20000)));
    // a copy, not a view that memory.grow could detach
    expect(calls[0].init.body.buffer.byteLength).toBe(20000);
  });

  it("storageStore: the Blob is built straight from the reference", async () => {
    const ctx = fakeCtx();
    const lines = await script(ctx, [
      "fill 50000",
      'submit {"type":"storageStore","contentType":"image/png","body":{"ptr":@PTR@,"len":@LEN@}}',
      "poll",
    ]);
    expect(event(lines[1])).toEqual({ type: "done", taskId: 0, ok: "stored:50000:image/png" });
    const blob: Blob = ctx.storage.store.mock.calls[0][0];
    expect(Buffer.from(await blob.arrayBuffer())).toEqual(Buffer.from(pattern(50000)));
  });

  it("an out-of-bounds reference fails the submit with a log line", async () => {
    mockFetch(() => new Response("ok"));
    const lines = await script(fakeCtx(), [fetchReq('{"ptr":4000000000,"len":10}')]);
    expect(lines[0]).toBe("s:-1");
  });

  it("base64 bodies still work", async () => {
    const calls = mockFetch(() => new Response("ok"));
    await script(fakeCtx(), [fetchReq('"aGk="'), "poll"]);
    expect(Buffer.from(calls[0].init.body).toString()).toBe("hi");
  });
});

describe("cancel", () => {
  /** A fetch that never answers; rejects with an AbortError when its signal fires. */
  const hang = (_u: string, init: any) =>
    new Promise<Response>((_, reject) => {
      init.signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
    });

  it("net/cancel {taskId} aborts the fetch and suppresses its done event", async () => {
    const calls = mockFetch(hang);
    const lines = await script(fakeCtx(), [fetchReq("null"), 'sync net/cancel {"taskId":0}', "poll"]);
    expect(lines[0]).toBe("s:0");
    expect(lines[1]).toBe('r:{"ok":true}');
    expect(calls[0].init.signal.aborted).toBe(true);
    // nothing outstanding any more: poll(1) reports idle instead of an AbortError event
    expect(lines[2]).toBe("e:none");
  });

  it("cancelling an unknown or finished task is a harmless false", async () => {
    mockFetch(() => new Response("ok"));
    const lines = await script(fakeCtx(), [fetchReq("null"), "poll", "poll", 'sync net/cancel {"taskId":0}', 'sync net/cancel {"taskId":99}']);
    expect(lines.slice(-2)).toEqual(['r:{"ok":false}', 'r:{"ok":false}']);
  });

  it("net/cancel {streamId} aborts the response body mid-download and drops its end event", async () => {
    let signal: AbortSignal | undefined;
    mockFetch((_u, init) => {
      signal = init.signal;
      return new Response(
        new ReadableStream({
          start(c) {
            // like a real fetch body, the stream errors when the request is aborted
            init.signal.addEventListener("abort", () => c.error(new DOMException("aborted", "AbortError")));
          },
          pull(c) {
            c.enqueue(new TextEncoder().encode("x".repeat(100)));
            return new Promise((r) => setTimeout(r, 5));
          },
        }),
      );
    });
    const lines = await script(fakeCtx(), [
      fetchReq("null"),
      "poll",
      'sync net/cancel {"streamId":"s0"}',
      "poll", "poll", "poll", "poll", "poll", "poll",
    ]);
    expect(event(lines[1]).ok.bodyStreamId).toBe("s0");
    expect(lines[2]).toBe('r:{"ok":true}');
    expect(signal?.aborted).toBe(true);
    // chunks that were already queued may still arrive, but there is no end/error event and the poll goes idle
    expect(lines.at(-1)).toBe("e:none");
    expect(lines.slice(3).filter((l) => l.includes('"end"') || l.includes('"error"'))).toEqual([]);
  });
});

describe("streamed request bodies", () => {
  it("net/body_write / body_end feed the request stream and the fetch sees every byte", async () => {
    let got = new Uint8Array(0);
    const calls = mockFetch(async (_u, init) => {
      const chunks: Uint8Array[] = [];
      for await (const c of init.body as any) chunks.push(c);
      got = Buffer.concat(chunks);
      return new Response("done");
    });
    const lines = await script(fakeCtx(), [
      "fill 300",
      fetchReq('{"stream":true}'),
      'sync net/body_write {"taskId":0,"data":{"ptr":@PTR@,"len":100}}',
      'sync net/body_write {"taskId":0,"data":{"ptr":@PTR+100@,"len":200}}',
      'sync net/body_end {"taskId":0}',
      "poll",
    ]);
    expect(lines.slice(1, 4)).toEqual(['r:{"ok":true}', 'r:{"ok":true}', 'r:{"ok":true}']);
    expect(event(lines[4]).type).toBe("done");
    expect(calls[0].init.duplex).toBe("half");
    expect(Buffer.from(got)).toEqual(Buffer.from(pattern(300)));
  });

  it("a chunk is copied: later writes to the buffer do not change what was sent", async () => {
    let got = new Uint8Array(0);
    mockFetch(async (_u, init) => {
      // read slowly, after the guest finished writing
      await new Promise((r) => setTimeout(r, 20));
      const chunks: Uint8Array[] = [];
      for await (const c of init.body as any) chunks.push(c);
      got = Buffer.concat(chunks);
      return new Response("done");
    });
    await script(fakeCtx(), [
      "fill 64",
      fetchReq('{"stream":true}'),
      'sync net/body_write {"taskId":0,"data":{"ptr":@PTR@,"len":64}}',
      "fill 64",
      'sync net/body_end {"taskId":0}',
      "poll",
    ]);
    expect(Buffer.from(got)).toEqual(Buffer.from(pattern(64)));
  });

  it("backpressure: a write beyond the high-water mark waits until the network reads", async () => {
    const order: string[] = [];
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    mockFetch(async (_u, init) => {
      await gate;
      order.push("read");
      let n = 0;
      for await (const c of init.body as any) n += c.byteLength;
      order.push(`read ${n}`);
      return new Response("done");
    });
    setTimeout(() => {
      order.push("release");
      release();
    }, 30);
    const lines = await script(fakeCtx(), [
      `fill ${3 * 1024 * 1024}`,
      fetchReq('{"stream":true}'),
      'sync net/body_write {"taskId":0,"data":{"ptr":@PTR@,"len":@LEN@}}',
      'sync net/body_end {"taskId":0}',
      "poll",
    ]);
    expect(lines[1]).toBe('r:{"ok":true}');
    // the guest was suspended in body_write until the consumer started reading
    expect(order.slice(0, 2)).toEqual(["release", "read"]);
    expect(order.at(-1)).toBe(`read ${3 * 1024 * 1024}`);
  });

  it("net/body_abort fails the request; cancel unblocks a suspended writer", async () => {
    let err: unknown;
    mockFetch(async (_u, init) => {
      try {
        for await (const _ of init.body as any) {
          // drain
        }
      } catch (e) {
        err = e;
        throw e;
      }
      return new Response("done");
    });
    const lines = await script(fakeCtx(), [
      fetchReq('{"stream":true}'),
      'sync net/body_abort {"taskId":0,"message":"guest gave up"}',
      "poll",
    ]);
    expect(lines[1]).toBe('r:{"ok":true}');
    const done = event(lines[2]);
    expect(done.error.message).toContain("guest gave up");
    expect(String(err)).toContain("guest gave up");
  });

  it("body_write for a task without a streamed body is an error reply", async () => {
    mockFetch(() => new Response("ok"));
    const [r] = await script(fakeCtx(), ['sync net/body_write {"taskId":7,"data":{"ptr":0,"len":0}}']);
    expect(JSON.parse(r.slice(2)).error.message).toContain("no streamed request body for task 7");
  });
});

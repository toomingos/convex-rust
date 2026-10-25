// HTTP actions written in Rust: runRustHttp against test/fixtures/http-guest.wasm (built from
// fixtures/http-crate by fixtures/build-http-guest.sh with the real #[http_action] macro).
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { runRustHttp } from "../src/runtime/index.js";
import { generateFiles } from "../src/cli/generate.js";
import { rustEntriesAsModules } from "../src/cli/rustApi.js";

const wasm = readFileSync(fileURLToPath(new URL("./fixtures/http-guest.wasm", import.meta.url)));

const call = (ctx: any, name: string, req: Request) =>
  runRustHttp(ctx, new WebAssembly.Module(wasm), `convex_run:http_fixture::h:${name}`, req);

const ctx = (over: Record<string, any> = {}): any => ({
  runQuery: vi.fn(async () => "ann"),
  runMutation: vi.fn(async () => null),
  runAction: vi.fn(async () => null),
  ...over,
});

function chunked(parts: string[], delayMs = 0): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  let i = 0;
  return new ReadableStream({
    async pull(c) {
      if (delayMs) await new Promise((r) => setTimeout(r, delayMs));
      if (i < parts.length) c.enqueue(enc.encode(parts[i++]));
      else c.close();
    },
  });
}

describe("runRustHttp", () => {
  it("buffered: passes method, url, headers and the body, returns status and headers", async () => {
    const res = await call(
      ctx(),
      "echo",
      new Request("https://x.convex.site/e?q=1", {
        method: "POST",
        headers: { "content-type": "text/plain" },
        body: "hello",
      }),
    );
    expect(res.status).toBe(201);
    expect(res.headers.get("x-echo")).toBe("1");
    expect(await res.text()).toBe("POST https://x.convex.site/e?q=1 ct=text/plain body=hello");
  });

  it("GET without a body", async () => {
    const res = await call(ctx(), "echo", new Request("https://x/"));
    expect(await res.text()).toBe("GET https://x/ ct=- body=");
  });

  it("buffered JSON through ctx", async () => {
    const c = ctx();
    const res = await call(c, "whoami", new Request("https://x/"));
    expect(res.headers.get("content-type")).toContain("application/json");
    expect(await res.json()).toEqual({ who: "ann" });
    expect(c.runQuery).toHaveBeenCalledTimes(1);
  });

  it("204 has no body", async () => {
    const res = await call(ctx(), "no_content", new Request("https://x/"));
    expect(res.status).toBe(204);
    expect(await res.text()).toBe("");
  });

  it("streams the response while consuming the request stream", async () => {
    const req = new Request("https://x/", {
      method: "POST",
      body: chunked(["ab", "cd", "ef"], 5),
      // @ts-expect-error node needs duplex for stream bodies
      duplex: "half",
    });
    const res = await call(ctx(), "shout", req);
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/plain");
    const reader = res.body!.getReader();
    const pieces: string[] = [];
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      pieces.push(new TextDecoder().decode(value));
    }
    expect(pieces.join("")).toBe("start\nABCDEF\nend");
    expect(pieces.length).toBeGreaterThan(1); // delivered incrementally, not as one buffer
  });

  it("an error before answering rejects", async () => {
    await expect(call(ctx(), "fails", new Request("https://x/"))).rejects.toThrow("handler failed");
  });

  it("an error after the stream started errors the body (truncated response)", async () => {
    const res = await call(ctx(), "broken", new Request("https://x/"));
    expect(res.status).toBe(200);
    await expect(res.text()).rejects.toThrow("upstream died");
  });

  it("cancelling the response stops the guest", async () => {
    const res = await call(ctx(), "shout", new Request("https://x/", { method: "POST", body: "z" }));
    await res.body!.cancel();
    // no throw / hang: the guest observes the closed response on its next write
  });
});

describe("codegen for http actions", () => {
  const entries = [
    { abiVersion: 1, modulePath: "app::chat", name: "stream", visibility: "public", kind: "http", args: null, returns: null },
    { abiVersion: 1, modulePath: "app::chat", name: "ask", visibility: "public", args: null, returns: null },
  ] as any[];

  it("exports httpAction wrappers next to actions", () => {
    const files = generateFiles({ entries, wasmFile: "x.wasm", outRelToFunctions: "rust" } as any);
    const src = [...files.entries()].find(([k]) => k.startsWith("app") || k.includes("chat"))![1];
    expect(src).toContain('import { action, httpAction } from');
    expect(src).toContain('import { runRust, runRustHttp } from "convex-rust/runtime";');
    expect(src).toMatch(/export const stream = httpAction\(\(ctx, request\) => runRustHttp\(ctx, wasm, "convex_run:app::chat:stream", request\)\);/);
    expect(src).toContain("export const ask = action(");
  });

  it("http entries are not part of the typed api", () => {
    const mods = rustEntriesAsModules(entries, "rust");
    const names = Object.values(mods).flatMap((m) => m.functions.map((f) => f.name));
    expect(names).toEqual(["ask"]);
  });
});

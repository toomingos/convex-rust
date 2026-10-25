"use node";
import { action, internalAction } from "./_generated/server";
import { ConvexError, v } from "convex/values";
import { makeFunctionReference } from "convex/server";

const fnv = (b: Uint8Array) => {
  let h = 0x811c9dc5;
  for (const x of b) h = Math.imul(h ^ x, 0x01000193) >>> 0;
  return h;
};
const unb64 = (s: string) => new Uint8Array(Buffer.from(s, "base64"));

export const echo = action({
  args: { value: v.any() },
  handler: async (_ctx, a) => ({ value: a.value }),
});

export const ping = action({ args: {}, handler: async () => "pong" });
export const internalPing = internalAction({ args: {}, handler: async () => "pong" });

export const call = action({
  args: { kind: v.string(), name: v.string(), args: v.any() },
  handler: async (ctx, a) => {
    if (a.kind === "query") return await ctx.runQuery(makeFunctionReference<"query">(a.name), a.args);
    if (a.kind === "mutation") return await ctx.runMutation(makeFunctionReference<"mutation">(a.name), a.args);
    return await ctx.runAction(makeFunctionReference<"action">(a.name), a.args);
  },
});

export const callCatch = action({
  args: { kind: v.string(), name: v.string(), args: v.any() },
  handler: async (ctx, a) => {
    try {
      const r = await ctx.runAction(makeFunctionReference<"action">("nodeSpec:call"), a);
      return { ok: true, value: r };
    } catch (e: any) {
      const isConvexError = e instanceof ConvexError;
      return { ok: false, isConvexError, data: isConvexError ? e.data : null, message: String(e?.message ?? e) };
    }
  },
});

export const fail = action({
  args: { message: v.string() },
  handler: async (_ctx, a) => {
    throw new Error(a.message);
  },
});
export const failConvex = action({
  args: { data: v.any() },
  handler: async (_ctx, a) => {
    throw new ConvexError(a.data);
  },
});
export const crash = action({
  args: {},
  handler: async () => {
    throw new Error("kaboom");
  },
});
export const strictArgs = action({
  args: { n: v.number() },
  handler: async (_ctx, a) => a.n * 2,
});

export const schedule = action({
  args: { kind: v.string(), target: v.string(), args: v.any(), delayMs: v.number() },
  handler: async (ctx, a) => {
    const ref = makeFunctionReference<"action">(a.target);
    const id =
      a.kind === "at"
        ? await ctx.scheduler.runAt(Date.now() + a.delayMs, ref, a.args)
        : await ctx.scheduler.runAfter(a.delayMs, ref, a.args);
    return { idIsString: typeof id === "string" && id.length > 0 };
  },
});
export const record = action({
  args: { tag: v.string() },
  handler: async (ctx, a) => {
    await ctx.runMutation(makeFunctionReference<"mutation">("data:addNote"), { tag: a.tag, from: "node" });
  },
});

export const whoami = action({
  args: {},
  handler: async (ctx) => await ctx.auth.getUserIdentity(),
});

export const storageRoundtrip = action({
  args: { data: v.string(), contentType: v.string() },
  handler: async (ctx, a) => {
    const bytes = unb64(a.data);
    const id = await ctx.storage.store(new Blob([bytes], { type: a.contentType }));
    const url = await ctx.storage.getUrl(id);
    const meta = await ctx.storage.getMetadata(id);
    const blob = await ctx.storage.get(id);
    const got = new Uint8Array(await blob!.arrayBuffer());
    const viaUrl = new Uint8Array(await (await fetch(url!)).arrayBuffer());
    await ctx.storage.delete(id);
    const afterGet = await ctx.storage.get(id);
    const afterUrl = await ctx.storage.getUrl(id);
    const afterMeta = await ctx.storage.getMetadata(id);
    return {
      idIsString: typeof id === "string",
      urlIsString: typeof url === "string",
      meta: {
        size: meta!.size,
        contentType: meta!.contentType,
        sha256Len: meta!.sha256.length,
        keys: Object.keys(meta!).sort(),
      },
      gotLen: got.length,
      gotSum: fnv(got),
      gotContentType: blob!.type,
      viaUrlSum: fnv(viaUrl),
      afterDelete: { get: afterGet, url: afterUrl, meta: afterMeta },
    };
  },
});

async function summarise(r: Response) {
  const b = new Uint8Array(await r.arrayBuffer());
  return {
    status: r.status,
    ok: r.ok,
    url: r.url,
    len: b.length,
    sum: fnv(b),
    contentType: r.headers.get("content-type"),
  };
}
export const fetchGet = action({
  args: { url: v.string() },
  handler: async (_ctx, a) => await summarise(await fetch(a.url)),
});
export const fetchPost = action({
  args: { url: v.string(), body: v.string() },
  handler: async (_ctx, a) => {
    const r = await fetch(a.url, { method: "POST", body: unb64(a.body), headers: { "x-test": "1" } });
    return { status: r.status, json: await r.json() };
  },
});
export const fetchStream = action({
  args: { url: v.string() },
  handler: async (_ctx, a) => {
    const r = await fetch(a.url);
    const reader = r.body!.getReader();
    let len = 0,
      chunks = 0,
      h = 0x811c9dc5;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks++;
      len += value.length;
      for (const x of value) h = Math.imul(h ^ x, 0x01000193) >>> 0;
    }
    return { status: r.status, len, sum: h, multiChunk: chunks > 1 };
  },
});
export const fetchFail = action({
  args: { url: v.string() },
  handler: async (_ctx, a) => {
    try {
      await fetch(a.url);
      return { threw: false };
    } catch {
      return { threw: true };
    }
  },
});

export const readEnv = action({
  args: { name: v.string() },
  handler: async (_ctx, a) => process.env[a.name] ?? null,
});

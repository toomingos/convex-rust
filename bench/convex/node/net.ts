"use node";
// Network cases (net-*). Each exports the pure implementation (`net001`) and
// the Convex action (`net_001`). They fetch from bench/mock-server
// (MOCK_URL, default http://127.0.0.1:8787) and return digests, never bodies.
// The Rust twin is rust/src/net.rs.
import { createHash } from "node:crypto";
import type { GenericActionCtx } from "convex/server";
import { seedAction } from "./_case.ts";
import { api } from "../_generated/api.js";
import { Prng } from "../../shared/prng.ts";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Ctx = GenericActionCtx<any>;
type Args = { seed: number };

const base = (): string => process.env.MOCK_URL ?? "http://127.0.0.1:8787";
const sha256 = (b: Uint8Array): string => createHash("sha256").update(b).digest("hex");

/** GET /bytes/n and digest the body. */
async function getBytes(n: number, seed: number): Promise<{ status: number; length: number; sha256: string }> {
  const res = await fetch(`${base()}/bytes/${n}?seed=${seed}`);
  const body = new Uint8Array(await res.arrayBuffer());
  return { status: res.status, length: body.length, sha256: sha256(body) };
}

export const net001 = (_ctx: Ctx | null, a: Args) => getBytes(1024, a.seed);
export const net_001 = seedAction(net001);

export const net002 = (_ctx: Ctx | null, a: Args) => getBytes(10 * 1024 * 1024, a.seed);
export const net_002 = seedAction(net002);

export async function net003(_ctx: Ctx | null, a: Args): Promise<{ status: number; length: number; sha256: string }> {
  const body = `{"seed":${a.seed},"data":"${new Prng(a.seed).word(1 << 20)}"}`;
  const res = await fetch(`${base()}/echo`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
  });
  const echo = (await res.json()) as { length: number; sha256: string };
  return { status: res.status, length: echo.length, sha256: echo.sha256 };
}
export const net_003 = seedAction(net003);

type Many = { count: number; totalBytes: number; sha256s: string[] };
function summarize(rs: { length: number; sha256: string }[]): Many {
  return { count: rs.length, totalBytes: rs.reduce((s, r) => s + r.length, 0), sha256s: rs.map((r) => r.sha256) };
}

export async function net004(_ctx: Ctx | null, a: Args): Promise<Many> {
  return summarize(await Promise.all(Array.from({ length: 20 }, (_, i) => getBytes(1024, a.seed + i))));
}
export const net_004 = seedAction(net004);

export async function net005(_ctx: Ctx | null, a: Args): Promise<Many> {
  const rs = [];
  for (let i = 0; i < 20; i++) rs.push(await getBytes(1024, a.seed + i));
  return summarize(rs);
}
export const net_005 = seedAction(net005);

// Chunked 1 MiB body hashed incrementally while reading.
export async function net006(_ctx: Ctx | null, a: Args): Promise<{ status: number; length: number; sha256: string }> {
  const res = await fetch(`${base()}/stream/${1 << 20}?seed=${a.seed}`);
  const hash = createHash("sha256");
  let length = 0;
  const reader = res.body!.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    hash.update(value);
    length += value.length;
  }
  return { status: res.status, length, sha256: hash.digest("hex") };
}
export const net_006 = seedAction(net006);

// Fetch 64 KiB, store it in file storage. Result: {id, size} (shape compare).
export async function net007(ctx: Ctx, a: Args): Promise<{ id: string; size: number }> {
  const res = await fetch(`${base()}/bytes/65536?seed=${a.seed}`);
  const data = new Uint8Array(await res.arrayBuffer());
  const id = await ctx.storage.store(new Blob([data], { type: "application/octet-stream" }));
  return { id, size: data.length };
}
export const net_007 = seedAction(net007);

// Fetch a JSON array, then record its length through a mutation.
export async function net008(ctx: Ctx, a: Args): Promise<{ count: number; doc: unknown }> {
  const res = await fetch(`${base()}/json/10?seed=${a.seed}`);
  const items = (await res.json()) as unknown[];
  const doc = await ctx.runMutation(api.cbTargets.insertDoc, { key: a.seed, value: `n${items.length}` });
  return { count: items.length, doc };
}
export const net_008 = seedAction(net008);

// Server sleeps 5 s; the client gives up after 200 ms.
export async function net009(_ctx: Ctx | null, _a: Args): Promise<{ timedOut: boolean }> {
  try {
    await fetch(`${base()}/sleep/5000`, { signal: AbortSignal.timeout(200) });
    return { timedOut: false };
  } catch (e) {
    if ((e as Error).name === "TimeoutError") return { timedOut: true };
    throw e;
  }
}
export const net_009 = seedAction(net009);

// 5 redirect hops, followed automatically.
export async function net010(_ctx: Ctx | null, _a: Args): Promise<{ status: number; done: boolean; hops: number }> {
  const res = await fetch(`${base()}/redirect/5`);
  const j = (await res.json()) as { done: boolean; hops: number };
  return { status: res.status, done: j.done, hops: j.hops };
}
export const net_010 = seedAction(net010);

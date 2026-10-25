"use node";
// Backend-callback cases (cb-*). Each exports the pure implementation
// (`cb001`) and the Convex action (`cb_001`). Callback targets live in
// ../cbTargets.ts (plain V8). The Rust twin is rust/src/cb.rs.
import type { GenericActionCtx } from "convex/server";
import { seedAction } from "./_case.ts";
import { api, internal } from "../_generated/api.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Ctx = GenericActionCtx<any>;
type Args = { seed: number };

const range = (n: number, seed: number): number[] =>
  Array.from({ length: n }, (_, i) => seed + i);

async function seq(ctx: Ctx, n: number, seed: number): Promise<number[]> {
  const out: number[] = [];
  for (const k of range(n, seed)) out.push(await ctx.runQuery(api.cbTargets.square, { n: k }));
  return out;
}

function par(ctx: Ctx, n: number, seed: number): Promise<number[]> {
  return Promise.all(range(n, seed).map((k) => ctx.runQuery(api.cbTargets.square, { n: k })));
}

/** 1 MB deterministic payload. */
function payload(seed: number): Uint8Array<ArrayBuffer> {
  const b = new Uint8Array(1 << 20);
  for (let i = 0; i < b.length; i++) b[i] = (i * 31 + seed) & 255;
  return b;
}

export async function cb001(ctx: Ctx, a: Args): Promise<number> {
  return await ctx.runQuery(api.cbTargets.square, { n: a.seed });
}
export const cb_001 = seedAction(cb001);

export const cb002 = (ctx: Ctx, a: Args) => seq(ctx, 10, a.seed);
export const cb_002 = seedAction(cb002);

export const cb003 = (ctx: Ctx, a: Args) => par(ctx, 10, a.seed);
export const cb_003 = seedAction(cb003);

export const cb004 = (ctx: Ctx, a: Args) => seq(ctx, 100, a.seed);
export const cb_004 = seedAction(cb004);

export const cb005 = (ctx: Ctx, a: Args) => par(ctx, 100, a.seed);
export const cb_005 = seedAction(cb005);

export async function cb006(ctx: Ctx, a: Args): Promise<unknown> {
  return await ctx.runMutation(api.cbTargets.insertDoc, { key: a.seed, value: `v${a.seed}` });
}
export const cb_006 = seedAction(cb006);

export async function cb007(ctx: Ctx, a: Args): Promise<unknown[]> {
  const out: unknown[] = [];
  for (const k of range(10, a.seed)) {
    out.push(await ctx.runMutation(api.cbTargets.insertDoc, { key: k, value: `v${k}` }));
  }
  return out;
}
export const cb_007 = seedAction(cb007);

// runAction against a default-runtime (V8 isolate) action.
export async function cb008(ctx: Ctx, a: Args): Promise<number> {
  return await ctx.runAction(api.cbTargets.incAction, { n: a.seed });
}
export const cb_008 = seedAction(cb008);

// Three chained runActions, each fed the previous result: seed + 3.
export async function cb009(ctx: Ctx, a: Args): Promise<number> {
  let n = a.seed;
  for (let i = 0; i < 3; i++) n = await ctx.runAction(api.cbTargets.incAction, { n });
  return n;
}
export const cb_009 = seedAction(cb009);

export async function cb010(ctx: Ctx, a: Args): Promise<string> {
  return await ctx.scheduler.runAfter(0, internal.cbTargets.touch, { n: a.seed });
}
export const cb_010 = seedAction(cb010);

export async function cb011(ctx: Ctx, a: Args): Promise<string[]> {
  const base = Date.now();
  const ids: string[] = [];
  for (let i = 0; i < 10; i++) {
    ids.push(
      await ctx.scheduler.runAt(base + (i + 1) * 1000, internal.cbTargets.touch, { n: a.seed + i }),
    );
  }
  return ids;
}
export const cb_011 = seedAction(cb011);

export async function cb012(ctx: Ctx, a: Args): Promise<{ id: string; size: number }> {
  const data = payload(a.seed);
  const id = await ctx.storage.store(new Blob([data], { type: "application/octet-stream" }));
  // Clean up so repeated runs don't fill file storage (the free plan has 1 GB).
  await ctx.storage.delete(id);
  return { id, size: data.length };
}
export const cb_012 = seedAction(cb012);

// Stores the blob first (setup), then reads it back; result is a checksum.
export async function cb013(
  ctx: Ctx,
  a: Args,
): Promise<{ size: number; sum: number; contentType: string | null }> {
  const id = await ctx.storage.store(
    new Blob([payload(a.seed)], { type: "application/octet-stream" }),
  );
  const blob = await ctx.storage.get(id);
  if (blob === null) throw new Error("stored blob missing");
  await ctx.storage.delete(id);
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let sum = 0;
  for (let i = 0; i < bytes.length; i++) sum += bytes[i];
  return { size: bytes.length, sum, contentType: blob.type || null };
}
export const cb_013 = seedAction(cb013);

export async function cb014(
  ctx: Ctx,
  _a: Args,
): Promise<{ subject: string; issuer: string } | null> {
  const id = await ctx.auth.getUserIdentity();
  return id === null ? null : { subject: id.subject, issuer: id.issuer };
}
export const cb_014 = seedAction(cb014);

// Reads CB_ENV_0..CB_ENV_9 (null when unset); W3 sets them on the deployment.
export async function cb015(_ctx: Ctx, _a: Args): Promise<(string | null)[]> {
  return Array.from({ length: 10 }, (_, i) => process.env[`CB_ENV_${i}`] ?? null);
}
export const cb_015 = seedAction(cb015);

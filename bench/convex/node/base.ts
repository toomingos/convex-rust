"use node";
// Baseline overhead cases (base-*). Each case exports the pure implementation
// (`base001`, used by the phase-0 runner) and the Convex action (`base_001`).
import { ConvexError, v } from "convex/values";
import { action } from "../_generated/server.js";
import { seedAction } from "./_case.ts";
import { Prng } from "../../shared/prng.ts";

export async function base001(_ctx: unknown, _args: { seed: number }): Promise<null> {
  return null;
}

export const base_001 = seedAction(base001);

/** base-002: echo small args. */
export async function base002(_ctx: unknown, args: { seed: number; msg: string }) {
  return args;
}

export const base_002 = action({
  args: { seed: v.number(), msg: v.string() },
  handler: (ctx, args) => base002(ctx, args),
});

/** base-003: return a 1 MiB string (deterministic from seed). */
export async function base003(_ctx: unknown, args: { seed: number }): Promise<string> {
  return new Prng(args.seed).text(1024 * 1024);
}

export const base_003 = seedAction(base003);

/** base-004: throw ConvexError with structured data. */
export async function base004(_ctx: unknown, _args: { seed: number }): Promise<never> {
  throw new ConvexError({ code: 42, detail: "base-004" });
}

export const base_004 = seedAction(base004);

/** base-005: 100 log lines. */
export async function base005(_ctx: unknown, _args: { seed: number }): Promise<null> {
  for (let i = 0; i < 100; i++) console.log(`base-005 log line ${i}`);
  return null;
}

export const base_005 = seedAction(base005);

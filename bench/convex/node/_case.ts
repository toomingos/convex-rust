// Shared scaffolding for the node cases (no timed code lives here).
//
// `seedAction(fn)` registers the Convex action twin of a pure case function
// `fn(ctx, {seed})`. The handler wrapper `(ctx, a) => fn(ctx, a)` is the same
// one every case file used to spell out by hand. Exported function names and
// paths (cases.json references them) are unchanged.
import { v } from "convex/values";
import { action, type ActionCtx } from "../_generated/server.js";

export type SeedArgs = { seed: number };

/** Validator for the `{ seed }` args shared by (almost) all cases. */
export const seedArgs = { seed: v.number() };

/** 10 MiB, the payload size used by the cpu-* and crypto-* cases. */
export const TEN_MB = 10 * 1024 * 1024;

export function seedAction<R>(fn: (ctx: ActionCtx, a: SeedArgs) => R | Promise<R>) {
  return action({ args: seedArgs, handler: (ctx, a) => fn(ctx, a) });
}

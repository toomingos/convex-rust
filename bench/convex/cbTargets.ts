// Plain V8 functions called back by the cb-* cases (no "use node").
// Both the Node and the Rust side address them by the same path, e.g.
// "cbTargets:square".
import { v } from "convex/values";
import {
  action,
  internalMutation,
  mutation,
  query,
} from "./_generated/server.js";

/** Pure query: n -> n*n. */
export const square = query({
  args: { n: v.number() },
  handler: async (_ctx, { n }) => n * n,
});

/** Reads a doc by key (null when absent). */
export const getDoc = query({
  args: { key: v.number() },
  handler: async (ctx, { key }) =>
    await ctx.db
      .query("cbDocs")
      .withIndex("by_key", (q) => q.eq("key", key))
      .first(),
});

/** Inserts a doc and returns it (with `_id`/`_creationTime`). */
export const insertDoc = mutation({
  args: { key: v.number(), value: v.string() },
  handler: async (ctx, args) => {
    const id = await ctx.db.insert("cbDocs", args);
    return await ctx.db.get(id);
  },
});

/** Scheduled-function target. */
export const touch = internalMutation({
  args: { n: v.number() },
  handler: async (_ctx, { n }) => n,
});

/** Action running in the default (V8 isolate) runtime: n -> n+1. */
export const incAction = action({
  args: { n: v.number() },
  handler: async (_ctx, { n }) => n + 1,
});

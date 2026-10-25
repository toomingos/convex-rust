import { query, mutation } from "./_generated/server";
import { ConvexError, v } from "convex/values";

export const addNote = mutation({
  args: { tag: v.string(), from: v.optional(v.string()) },
  handler: async (ctx, a) => await ctx.db.insert("notes", a),
});

export const notes = query({
  args: { tag: v.string() },
  handler: async (ctx, a) => {
    const rows = await ctx.db
      .query("notes")
      .withIndex("by_tag", (q) => q.eq("tag", a.tag))
      .collect();
    return rows.map((r) => ({ tag: r.tag, from: r.from ?? null }));
  },
});

export const double = query({
  args: { n: v.number() },
  handler: async (_ctx, a) => a.n * 2,
});

export const boom = query({
  args: {},
  handler: async () => {
    throw new ConvexError({ code: "boom", n: 7 });
  },
});

// Scheduled-function states by name (system table), for the scheduler specs.
export const scheduledStates = query({
  args: {},
  handler: async (ctx) => {
    const rows = await ctx.db.system.query("_scheduled_functions").collect();
    return rows.map((r) => ({ name: r.name, state: r.state.kind }));
  },
});

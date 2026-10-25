import { query, mutation } from "./_generated/server";
import { v } from "convex/values";

export const list = query({
  args: {},
  handler: async (ctx) => {
    return await ctx.db.query("messages").collect();
  },
});

export const add = mutation({
  args: { body: v.string() },
  handler: async (ctx, args) => {
    return await ctx.db.insert("messages", { body: args.body });
  },
});

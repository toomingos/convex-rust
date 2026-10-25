import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  notes: defineTable({ tag: v.string(), from: v.optional(v.string()) }).index("by_tag", ["tag"]),
});

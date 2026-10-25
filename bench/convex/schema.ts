import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  // Target table for the backend-callback cases (cb-006, cb-007).
  cbDocs: defineTable({
    key: v.number(),
    value: v.string(),
  }).index("by_key", ["key"]),
});

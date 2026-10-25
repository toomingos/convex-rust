"use node";
// Serialization cases (ser-*): each action receives generated args (see
// generateArgs in shared/cases.ts) and echoes them back, so the cost measured
// is Convex-JSON decode + encode of the value. The payload validator is
// v.any() since the shape is generated (see argsGen in cases.json).
import { v } from "convex/values";
import { action } from "../_generated/server.js";

type SerArgs = { seed: number; payload: any };

const echo = async (_ctx: unknown, args: SerArgs): Promise<SerArgs> => args;
const validators = { seed: v.number(), payload: v.any() };

export const ser001 = echo;
export const ser002 = echo;
export const ser003 = echo;
export const ser004 = echo;
export const ser005 = echo;
export const ser006 = echo;
export const ser007 = echo;
export const ser008 = echo;
export const ser009 = echo;
export const ser010 = echo;

export const ser_001 = action({ args: validators, handler: (ctx, a) => ser001(ctx, a) });
export const ser_002 = action({ args: validators, handler: (ctx, a) => ser002(ctx, a) });
export const ser_003 = action({ args: validators, handler: (ctx, a) => ser003(ctx, a) });
export const ser_004 = action({ args: validators, handler: (ctx, a) => ser004(ctx, a) });
export const ser_005 = action({ args: validators, handler: (ctx, a) => ser005(ctx, a) });
export const ser_006 = action({ args: validators, handler: (ctx, a) => ser006(ctx, a) });
export const ser_007 = action({ args: validators, handler: (ctx, a) => ser007(ctx, a) });
export const ser_008 = action({ args: validators, handler: (ctx, a) => ser008(ctx, a) });
export const ser_009 = action({ args: validators, handler: (ctx, a) => ser009(ctx, a) });
export const ser_010 = action({ args: validators, handler: (ctx, a) => ser010(ctx, a) });

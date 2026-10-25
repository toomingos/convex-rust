/* eslint-disable */
/**
 * Generated `api` utility.
 *
 * THIS CODE IS AUTOMATICALLY GENERATED.
 *
 * To regenerate, run `npx convex dev`.
 * @module
 */

import type * as cbTargets from "../cbTargets.js";
import type * as node__case from "../node/_case.js";
import type * as node_base from "../node/base.js";
import type * as node_cb from "../node/cb.js";
import type * as node_cpu from "../node/cpu.js";
import type * as node_crypto from "../node/crypto.js";
import type * as node_data from "../node/data.js";
import type * as node_media from "../node/media.js";
import type * as node_mem from "../node/mem.js";
import type * as node_net from "../node/net.js";
import type * as node_ser from "../node/ser.js";

import type {
  ApiFromModules,
  FilterApi,
  FunctionReference,
} from "convex/server";

declare const fullApi: ApiFromModules<{
  cbTargets: typeof cbTargets;
  "node/_case": typeof node__case;
  "node/base": typeof node_base;
  "node/cb": typeof node_cb;
  "node/cpu": typeof node_cpu;
  "node/crypto": typeof node_crypto;
  "node/data": typeof node_data;
  "node/media": typeof node_media;
  "node/mem": typeof node_mem;
  "node/net": typeof node_net;
  "node/ser": typeof node_ser;
}> & {
  rust: {
    base: {
      base_001: FunctionReference<"action", "public", any, null>;
      base_002: FunctionReference<"action", "public", any, any>;
      base_003: FunctionReference<"action", "public", any, string>;
      base_004: FunctionReference<"action", "public", any, null>;
      base_005: FunctionReference<"action", "public", any, null>;
    };
    cb: {
      cb_001: FunctionReference<"action", "public", any, number>;
      cb_002: FunctionReference<"action", "public", any, Array<number>>;
      cb_003: FunctionReference<"action", "public", any, Array<number>>;
      cb_004: FunctionReference<"action", "public", any, Array<number>>;
      cb_005: FunctionReference<"action", "public", any, Array<number>>;
      cb_006: FunctionReference<"action", "public", any, any>;
      cb_007: FunctionReference<"action", "public", any, Array<any>>;
      cb_008: FunctionReference<"action", "public", any, number>;
      cb_009: FunctionReference<"action", "public", any, number>;
      cb_010: FunctionReference<"action", "public", any, string>;
      cb_011: FunctionReference<"action", "public", any, Array<string>>;
      cb_012: FunctionReference<"action", "public", any, any>;
      cb_013: FunctionReference<"action", "public", any, any>;
      cb_014: FunctionReference<"action", "public", any, any>;
      cb_015: FunctionReference<"action", "public", any, Array<string | null>>;
    };
    cpu: {
      cpu_001: FunctionReference<"action", "public", any, string>;
      cpu_002: FunctionReference<"action", "public", any, string>;
      cpu_003: FunctionReference<"action", "public", any, string>;
      cpu_004: FunctionReference<"action", "public", any, string>;
      cpu_005: FunctionReference<"action", "public", any, number>;
      cpu_006: FunctionReference<"action", "public", any, number>;
      cpu_007: FunctionReference<"action", "public", any, any>;
      cpu_008: FunctionReference<"action", "public", any, any>;
      cpu_009: FunctionReference<"action", "public", any, any>;
      cpu_010: FunctionReference<"action", "public", any, any>;
      cpu_011: FunctionReference<"action", "public", any, Array<number>>;
      cpu_012: FunctionReference<"action", "public", any, number>;
      cpu_013: FunctionReference<"action", "public", any, any>;
      cpu_014: FunctionReference<"action", "public", any, any>;
      cpu_015: FunctionReference<"action", "public", any, any>;
      cpu_016: FunctionReference<"action", "public", any, any>;
      cpu_017: FunctionReference<"action", "public", any, any>;
      cpu_018: FunctionReference<"action", "public", any, any>;
      cpu_019: FunctionReference<"action", "public", any, any>;
      cpu_020: FunctionReference<"action", "public", any, any>;
    };
    crypto: {
      crypto_001: FunctionReference<"action", "public", any, string>;
      crypto_002: FunctionReference<"action", "public", any, any>;
      crypto_003: FunctionReference<"action", "public", any, any>;
      crypto_004: FunctionReference<"action", "public", any, any>;
      crypto_005: FunctionReference<"action", "public", any, any>;
      crypto_006: FunctionReference<"action", "public", any, any>;
      crypto_007: FunctionReference<"action", "public", any, any>;
      crypto_008: FunctionReference<"action", "public", any, any>;
      crypto_009: FunctionReference<"action", "public", any, any>;
      crypto_010: FunctionReference<"action", "public", any, any>;
    };
    data: {
      data_001: FunctionReference<"action", "public", any, any>;
      data_002: FunctionReference<"action", "public", any, any>;
      data_003: FunctionReference<
        "action",
        "public",
        any,
        Record<string, number>
      >;
      data_004: FunctionReference<"action", "public", any, any>;
      data_005: FunctionReference<"action", "public", any, any>;
      data_006: FunctionReference<"action", "public", any, any>;
      data_007: FunctionReference<"action", "public", any, any>;
      data_008: FunctionReference<"action", "public", any, any>;
      data_009: FunctionReference<"action", "public", any, any>;
      data_010: FunctionReference<"action", "public", any, any>;
      data_011: FunctionReference<"action", "public", any, any>;
      data_012: FunctionReference<"action", "public", any, any>;
      data_013: FunctionReference<"action", "public", any, any>;
      data_014: FunctionReference<"action", "public", any, any>;
      data_015: FunctionReference<"action", "public", any, any>;
    };
    media: {
      media_001: FunctionReference<"action", "public", any, any>;
      media_002: FunctionReference<"action", "public", any, any>;
      media_003: FunctionReference<"action", "public", any, any>;
      media_004: FunctionReference<"action", "public", any, any>;
      media_005: FunctionReference<"action", "public", any, any>;
      media_006: FunctionReference<"action", "public", any, any>;
      media_007: FunctionReference<"action", "public", any, any>;
      media_008: FunctionReference<"action", "public", any, any>;
      media_009: FunctionReference<"action", "public", any, any>;
      media_010: FunctionReference<"action", "public", any, any>;
    };
    mem: {
      mem_001: FunctionReference<"action", "public", any, number>;
      mem_002: FunctionReference<"action", "public", any, number>;
      mem_003: FunctionReference<"action", "public", any, Array<number>>;
      mem_004: FunctionReference<"action", "public", any, any>;
      mem_005: FunctionReference<"action", "public", any, any>;
    };
    net: {
      net_001: FunctionReference<"action", "public", any, any>;
      net_002: FunctionReference<"action", "public", any, any>;
      net_003: FunctionReference<"action", "public", any, any>;
      net_004: FunctionReference<"action", "public", any, any>;
      net_005: FunctionReference<"action", "public", any, any>;
      net_006: FunctionReference<"action", "public", any, any>;
      net_007: FunctionReference<"action", "public", any, any>;
      net_008: FunctionReference<"action", "public", any, any>;
      net_009: FunctionReference<"action", "public", any, any>;
      net_010: FunctionReference<"action", "public", any, any>;
    };
    ser: {
      ser_001: FunctionReference<"action", "public", any, any>;
      ser_002: FunctionReference<"action", "public", any, any>;
      ser_003: FunctionReference<"action", "public", any, any>;
      ser_004: FunctionReference<"action", "public", any, any>;
      ser_005: FunctionReference<"action", "public", any, any>;
      ser_006: FunctionReference<"action", "public", any, any>;
      ser_007: FunctionReference<"action", "public", any, any>;
      ser_008: FunctionReference<"action", "public", any, any>;
      ser_009: FunctionReference<"action", "public", any, any>;
      ser_010: FunctionReference<"action", "public", any, any>;
    };
  };
};

/**
 * A utility for referencing Convex functions in your app's public API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = api.myModule.myFunction;
 * ```
 */
export declare const api: FilterApi<
  typeof fullApi,
  FunctionReference<any, "public">
>;

/**
 * A utility for referencing Convex functions in your app's internal API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = internal.myModule.myFunction;
 * ```
 */
export declare const internal: FilterApi<
  typeof fullApi,
  FunctionReference<any, "internal">
>;

export declare const components: {};

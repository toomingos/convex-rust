// Fast Convex-JSON for the host's args / result. Semantically identical to
// `JSON.stringify(convexToJson(v))` and `jsonToConvex(JSON.parse(t))`: the fast paths only handle the
// common, valid shapes; anything else (a validation failure, an unsupported type, an odd encoding) is
// handed to the `convex/values` functions so the result or the error is exactly theirs.
import { convexToJson, jsonToConvex } from "convex/values";
import { decodeBase64, encodeBase64 } from "./base64.js";

const MAX_INT64 = 0x7fffffffffffffffn;
const MIN_INT64 = -0x8000000000000000n;
const f64 = new Float64Array(1);
const f64b = new Uint8Array(f64.buffer);
const i64 = new BigInt64Array(1);
const i64b = new Uint8Array(i64.buffer);
// The typed-array views are in platform byte order; Convex wants little endian.
const LITTLE = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;
// Valid field names: <= 1024 chars, non-control ASCII, no leading `$`.
const VALID_FIELD = /^(?:[ -#%-~][ -~]*)?$/;
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;

class Fallback extends Error {}
const FALLBACK = new Fallback();

function validField(k: string): boolean {
  return k.length <= 1024 && VALID_FIELD.test(k);
}

function replacer(this: any, key: string, v: any): any {
  switch (typeof v) {
    case "string":
      // A `toJSON` (Date) turned an object into a string: not a Convex value.
      if (this[key] !== v) throw FALLBACK;
      return v;
    case "boolean":
      return v;
    case "number":
      if (Number.isFinite(v) && !Object.is(v, -0)) return v;
      f64[0] = v;
      return { $float: encodeBase64(f64b) };
    case "bigint":
      if (v < MIN_INT64 || MAX_INT64 < v) throw FALLBACK;
      i64[0] = v;
      return { $integer: encodeBase64(i64b) };
    case "object": {
      if (v === null) return v;
      // `toJSON` already ran (Date, the commit-ts placeholder, ...): not a plain Convex value.
      if (this[key] !== v) throw FALLBACK;
      if (v instanceof ArrayBuffer) return { $bytes: encodeBase64(new Uint8Array(v)) };
      if (Array.isArray(v)) {
        for (let i = 0; i < v.length; i++) if (v[i] === undefined) throw FALLBACK;
        return v;
      }
      const proto = Object.getPrototypeOf(v);
      if (proto !== Object.prototype && proto !== null) throw FALLBACK;
      const keys = Object.keys(v);
      let sorted = true;
      for (let i = 0; i < keys.length; i++) {
        const k = keys[i];
        if (v[k] !== undefined && !validField(k)) throw FALLBACK;
        if (i > 0 && keys[i - 1] > k) sorted = false;
      }
      if (sorted) return v;
      // convexToJson emits keys sorted by UTF-16 code units; keep that order on the wire.
      keys.sort();
      const out: Record<string, unknown> = {};
      for (const k of keys) if (v[k] !== undefined) out[k] = v[k];
      return out;
    }
    default:
      // undefined, function, symbol
      throw FALLBACK;
  }
}

export function stringifyConvex(value: unknown): string {
  if (LITTLE) {
    try {
      const s = JSON.stringify(value, replacer);
      if (typeof s === "string") return s;
    } catch (e) {
      // FALLBACK or any error raised while walking: the reference conversion decides.
      void e;
    }
  }
  return JSON.stringify(convexToJson(value as any));
}

function bytes8(s: string): Uint8Array | null {
  if (typeof s !== "string" || s.length !== 12 || !BASE64.test(s)) return null;
  try {
    const b = decodeBase64(s);
    return b.length === 8 ? b : null;
  } catch {
    return null;
  }
}

/** Decodes Convex JSON in place; anything irregular is handed to `jsonToConvex` (same value or same error). */
function fix(v: any): any {
  if (v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) {
    for (let i = 0; i < v.length; i++) {
      const x = v[i];
      if (x !== null && typeof x === "object") v[i] = fix(x);
    }
    return v;
  }
  const keys = Object.keys(v);
  if (keys.length === 1 && keys[0].charCodeAt(0) === 36 /* $ */) {
    const k = keys[0];
    const s = v[k];
    if (k === "$bytes" && typeof s === "string" && s.length % 4 === 0 && BASE64.test(s)) {
      try {
        const b = decodeBase64(s);
        // Own, exactly sized buffer (like Base64.toByteArray(..).buffer).
        return b.byteOffset === 0 && b.byteLength === b.buffer.byteLength
          ? b.buffer
          : b.slice().buffer;
      } catch {
        // fall through to the reference decoder
      }
    } else if (k === "$integer") {
      const b = bytes8(s);
      if (b !== null) {
        i64b.set(b);
        return i64[0];
      }
    } else if (k === "$float") {
      const b = bytes8(s);
      if (b !== null) {
        f64b.set(b);
        const f = f64[0];
        if (Number.isNaN(f) || !Number.isFinite(f) || Object.is(f, -0)) return f;
      }
    }
    return jsonToConvex(v);
  }
  for (let i = 0; i < keys.length; i++) {
    const k = keys[i];
    if (!validField(k)) return jsonToConvex(v); // throws the reference error
    const x = v[k];
    if (x !== null && typeof x === "object") v[k] = fix(x);
  }
  return v;
}

/** `jsonToConvex(JSON.parse(text))`, converting the freshly parsed tree in place. */
export function parseConvexValue(parsed: unknown): any {
  if (!LITTLE) return jsonToConvex(parsed as any);
  return fix(parsed);
}

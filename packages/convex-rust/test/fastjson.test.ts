import { describe, expect, it } from "vitest";
import { convexToJson, jsonToConvex } from "convex/values";
import { parseConvexValue, stringifyConvex } from "../src/runtime/fastjson.js";

function ref(v: any): { ok: string } | { err: string } {
  try {
    return { ok: JSON.stringify(convexToJson(v)) };
  } catch (e: any) {
    return { err: e.message };
  }
}
function fast(v: any): { ok: string } | { err: string } {
  try {
    return { ok: stringifyConvex(v) };
  } catch (e: any) {
    return { err: e.message };
  }
}
const buf = (...b: number[]) => new Uint8Array(b).buffer;

const encodeCases: [string, any][] = [
  ["null", null],
  ["scalars", { a: 1, b: "x", c: true, d: null, e: 1.5 }],
  ["unsorted keys", { z: 1, a: { y: 1, b: 2 }, m: [{ q: 1, b: 2 }] }],
  ["int-like keys", { "10": 1, "2": 2, a: 3 }],
  ["specials", [NaN, Infinity, -Infinity, -0, 0, 1e308]],
  ["bigint", { a: 5n, b: -5n, c: 2n ** 63n - 1n, d: -(2n ** 63n) }],
  ["bigint out of range", { a: 2n ** 63n }],
  ["bytes", { a: buf(1, 2, 3), b: buf(), c: [buf(255)] }],
  ["undefined field", { a: undefined, b: 1 }],
  ["undefined $ field", { $a: undefined, b: 1 }],
  ["undefined top", undefined],
  ["undefined in array", [1, undefined]],
  ["nested undefined array", { a: [[undefined]] }],
  ["$ key", { $a: 1 }],
  ["$ key nested", { a: { b: { $x: 1 } } }],
  ["non-ascii key", { é: 1 }],
  ["control key", { "a\nb": 1 }],
  ["long key", { ["k".repeat(1025)]: 1 }],
  ["max key", { ["k".repeat(1024)]: 1 }],
  ["empty key", { "": 1 }],
  ["Date", { a: new Date(0) }],
  ["Map", { a: new Map() }],
  ["Set", new Set([1])],
  ["typed array", { a: new Uint8Array(2) }],
  ["class", new (class Foo { x = 1 })()],
  ["function", { a: () => 1 }],
  ["symbol", { a: Symbol("x") }],
  ["null proto", Object.assign(Object.create(null), { b: 1, a: 2 })],
  ["unicode string", { a: "héllo   \ud800" }],
];

describe("stringifyConvex matches JSON.stringify(convexToJson(v))", () => {
  for (const [name, v] of encodeCases) {
    it(name, () => {
      expect(fast(v)).toEqual(ref(v));
    });
  }
});

describe("parseConvexValue matches jsonToConvex", () => {
  const texts: [string, string][] = [
    ["plain", '{"a":1,"b":[1,2,{"c":"x"}],"d":null}'],
    ["bytes", '{"a":{"$bytes":"AQID"},"b":{"$bytes":""}}'],
    ["integer", '{"a":{"$integer":"BQAAAAAAAAA="},"b":{"$integer":"/////////38="}}'],
    ["float nan", '[{"$float":"AAAAAAAA+H8="},{"$float":"AAAAAAAA8H8="},{"$float":"AAAAAAAAAIA="}]'],
    ["float not special", '{"$float":"AAAAAAAA8D8="}'],
    ["bad bytes", '{"$bytes":5}'],
    ["unpadded bytes", '{"$bytes":"AQI"}'],
    ["bad integer length", '{"$integer":"AQID"}'],
    ["$ key", '{"$a":1}'],
    ["$ key nested", '{"a":[{"b":{"$x":1}}]}'],
    ["$bytes + other", '{"$bytes":"AQID","x":1}'],
    ["$set", '{"$set":[]}'],
    ["$map", '{"$map":[]}'],
    ["$commitTs", '{"$commitTs":null}'],
    ["bad $commitTs", '{"$commitTs":1}'],
    ["non-ascii key", '{"é":1}'],
    ["control key", '{"a\\nb":1}'],
    ["long key", JSON.stringify({ ["k".repeat(1025)]: 1 })],
    ["empty key", '{"":1}'],
    ["scalar", "3"],
    ["string", '"$x"'],
    ["value with $ text", '{"a":"\\"$bytes\\""}'],
  ];
  const run = (f: (v: any) => any, t: string) => {
    try {
      const r = f(JSON.parse(t));
      return { ok: r instanceof ArrayBuffer ? Array.from(new Uint8Array(r)) : r };
    } catch (e: any) {
      return { err: e.message };
    }
  };
  for (const [name, t] of texts) {
    it(name, () => {
      const a = run(parseConvexValue, t);
      const b = run(jsonToConvex, t);
      expect(a).toEqual(b);
    });
  }
  it("returns exactly sized ArrayBuffers and bigints", () => {
    const v: any = parseConvexValue(JSON.parse('{"a":{"$bytes":"AQID"},"b":{"$integer":"/////////38="}}'));
    expect(v.a).toBeInstanceOf(ArrayBuffer);
    expect(v.a.byteLength).toBe(3);
    expect(v.b).toBe(2n ** 63n - 1n);
  });
  it("round trips", () => {
    const v = { a: [1n, buf(9, 8), NaN, -0], s: "x" };
    const back: any = parseConvexValue(JSON.parse(stringifyConvex(v)));
    expect(back.a[0]).toBe(1n);
    expect(Array.from(new Uint8Array(back.a[1]))).toEqual([9, 8]);
    expect(Object.is(back.a[3], -0)).toBe(true);
  });
});

describe("prototype-ish keys", () => {
  it("keeps __proto__ and constructor as own fields", () => {
    const text = '{"__proto__":{"a":{"$integer":"AQAAAAAAAAA="}},"constructor":{"b":2}}';
    const back: any = parseConvexValue(JSON.parse(text));
    expect(Object.getPrototypeOf(back)).toBe(Object.prototype);
    expect(Object.keys(back)).toEqual(["__proto__", "constructor"]);
    expect(Object.getOwnPropertyDescriptor(back, "__proto__")!.value.a).toBe(1n);
    expect(back.constructor.b).toBe(2);
  });
});

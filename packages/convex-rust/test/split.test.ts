import { describe, expect, it } from "vitest";
import { filterExports, keepsExport } from "../src/cli/split.js";
import { generateFiles } from "../src/cli/generate.js";

// (module (func $a) (func $b) (memory 1) (export "keep" (func 0)) (export "drop" (func 1)) (export "mem" (memory 0)))
function tiny(): Uint8Array {
  const enc = (s: string) => [s.length, ...Buffer.from(s)];
  const exportsBody = [3, ...enc("keep"), 0, 0, ...enc("drop"), 0, 1, ...enc("mem"), 2, 0];
  return new Uint8Array([
    0, 0x61, 0x73, 0x6d, 1, 0, 0, 0,
    1, 4, 1, 0x60, 0, 0, // type () -> ()
    3, 3, 2, 0, 0, // two functions
    5, 3, 1, 0, 1, // memory 1 page
    7, exportsBody.length, ...exportsBody,
    10, 7, 2, 2, 0, 0x0b, 2, 0, 0x0b, // bodies
  ]);
}

describe("filterExports", () => {
  it("keeps only the wanted exports and leaves a valid module", () => {
    const out = filterExports(tiny(), (n) => n !== "drop");
    const m = new WebAssembly.Module(out as BufferSource);
    expect(WebAssembly.Module.exports(m).map((e) => e.name).sort()).toEqual(["keep", "mem"]);
  });
  it("is the identity when everything is kept", () => {
    const m = new WebAssembly.Module(filterExports(tiny(), () => true) as BufferSource);
    expect(WebAssembly.Module.exports(m)).toHaveLength(3);
  });
});

describe("generateFiles with per-module wasm", () => {
  const e = (modulePath: string, name: string) => ({
    abiVersion: 1, modulePath, name, visibility: "public" as const, args: null, returns: null,
  });
  it("imports each module's own wasm", () => {
    const files = generateFiles({
      entries: [e("c::a", "f"), e("c::b", "g")],
      wasmFile: "c.wasm",
      moduleWasm: new Map([
        ["a", { file: "c.a.wasm", hash: "aaaa" }],
        ["b", { file: "c.b.wasm", hash: "bbbb" }],
      ]),
    });
    expect(files.get("a.ts")).toContain('import wasm from "./_wasm/c.a.wasm"');
    expect(files.get("a.ts")).toContain("// wasm: aaaa");
    expect(files.get("b.ts")).toContain('import wasm from "./_wasm/c.b.wasm"');
  });
});

describe("keepsExport (exact module match)", () => {
  const prefixes = ["convex_run:c::a:"];
  it("keeps the module's own functions and non-convex_run exports", () => {
    expect(keepsExport("convex_run:c::a:f", prefixes)).toBe(true);
    expect(keepsExport("memory", prefixes)).toBe(true);
  });
  it("drops nested child modules and siblings", () => {
    expect(keepsExport("convex_run:c::a::b:f", prefixes)).toBe(false);
    expect(keepsExport("convex_run:c::ab:f", prefixes)).toBe(false);
    expect(keepsExport("convex_run:c::a::b:f", ["convex_run:c::a::b:"])).toBe(true);
  });
});

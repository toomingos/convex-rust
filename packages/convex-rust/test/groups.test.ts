import { describe, expect, it } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { BuildError, groupFeatureArgs, modulesFromFeatures, parseGroups, perModuleGroups, writeOutput } from "../src/cli/build.js";

describe("parseGroups", () => {
  it("parses name=modules specs", () => {
    expect(parseGroups(["light=base, cb,ser", "heavy=cpu"])).toEqual([
      { name: "light", modules: ["base", "cb", "ser"] },
      { name: "heavy", modules: ["cpu"] },
    ]);
  });
  it("rejects malformed, duplicate and overlapping specs", () => {
    expect(() => parseGroups(["nope"])).toThrow(BuildError);
    expect(() => parseGroups(["a=x", "a=y"])).toThrow(/twice/);
    expect(() => parseGroups(["a=x,y", "b=y"])).toThrow(/both group/);
  });
});

describe("groupFeatureArgs", () => {
  it("disables defaults and enables m-<module>", () => {
    expect(groupFeatureArgs({ name: "l", modules: ["base", "a/b"] })).toEqual([
      "--no-default-features", "--features", "m-base,m-a-b",
    ]);
    expect(groupFeatureArgs({ name: "l", modules: ["x"] }, "mod_")[2]).toBe("mod_x");
  });
});

describe("per-module default", () => {
  it("derives modules from m-<module> features (dash = path separator)", () => {
    expect(modulesFromFeatures(["default", "m-cpu", "m-a-b", "m-base", "serde"])).toEqual(["a/b", "base", "cpu"]);
    expect(modulesFromFeatures(["m-"," default"])).toEqual([]);
    expect(modulesFromFeatures(["mod_x", "mod_y"], "mod_")).toEqual(["x", "y"]);
  });
  it("needs at least two features to split", () => {
    expect(modulesFromFeatures(["default", "m-only"])).toEqual([]);
    expect(modulesFromFeatures(["default"])).toEqual([]);
  });
  it("makes one group per module with a file-safe name", () => {
    const g = perModuleGroups(["a/b", "cpu"]);
    expect(g).toEqual([{ name: "a__b", modules: ["a/b"] }, { name: "cpu", modules: ["cpu"] }]);
    expect(groupFeatureArgs(g[0])[2]).toBe("m-a-b");
  });
});

describe("writeOutput with groups", () => {
  it("writes one wasm per group and points each module at its group's file", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "grp-"));
    try {
      const fns = path.join(dir, "convex");
      const out = path.join(fns, "rust");
      const e = (modulePath: string, name: string) => ({
        abiVersion: 1, modulePath, name, visibility: "public" as const, args: null, returns: null,
      });
      const res = writeOutput(
        out, "c.wasm", new Uint8Array([1]), [e("c::a", "f"), e("c::b", "g"), e("c::d", "h")], fns, [], undefined,
        {
          wasm: new Map([["light", new Uint8Array([1, 2])], ["heavy", new Uint8Array([3])]]),
          moduleGroup: new Map([["a", "light"], ["b", "light"], ["d", "heavy"]]),
        },
      );
      expect(res.written).toContain("_wasm/c.light.wasm");
      expect(res.written).toContain("_wasm/c.heavy.wasm");
      expect(fs.readdirSync(path.join(out, "_wasm")).filter((f) => f.endsWith(".wasm")).sort()).toEqual(["c.heavy.wasm", "c.light.wasm"]);
      for (const [m, f] of [["a", "c.light.wasm"], ["b", "c.light.wasm"], ["d", "c.heavy.wasm"]]) {
        expect(fs.readFileSync(path.join(out, `${m}.ts`), "utf8")).toContain(`_wasm/${f}`);
      }
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("wasm file name collisions", () => {
  it("rejects a/b together with a real module a__b", () => {
    expect(() => perModuleGroups(["a/b", "a__b"])).toThrow(/both map to the wasm file name/);
    expect(perModuleGroups(["a/b", "a/c"]).map((g) => g.name)).toEqual(["a__b", "a__c"]);
  });
});

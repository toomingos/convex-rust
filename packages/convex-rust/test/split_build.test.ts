import { describe, expect, it } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { buildOnce, smallestTwo, splitRustcArgs } from "../src/cli/build.js";
import { parseManifest } from "../src/cli/manifest.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");

describe("splitRustcArgs", () => {
  it("declares the cfgs and keeps the given functions (sorted, deduplicated)", () => {
    expect(splitRustcArgs(["b", "a", "b"])).toEqual([
      "--check-cfg", "cfg(convex_split)", "--check-cfg", "cfg(convex_keep, values(any()))",
      "--cfg", "convex_split", "--cfg", 'convex_keep="a"', "--cfg", 'convex_keep="b"',
    ]);
  });
});

describe("smallestTwo", () => {
  it("picks the two smallest", () => {
    expect(smallestTwo(new Map([["a", 5], ["b", 1], ["c", 3]]))).toEqual(["b", "c"]);
    expect(smallestTwo(new Map([["a", 5]]))).toBeUndefined();
  });
});

const haveToolchain = spawnSync("rustup", ["target", "list", "--installed"], {
  encoding: "utf8",
  cwd: path.join(repo, "e2e/spec/convex/rust"),
}).stdout?.includes("wasm32-wasip1");

function setupProject(): { proj: string; crate: string } {
  const proj = fs.mkdtempSync(path.join(os.tmpdir(), "split-fixture-"));
  const crate = path.join(proj, "convex", "rust");
  fs.cpSync(path.join(here, "fixtures/split-crate"), crate, { recursive: true });
  const toml = path.join(crate, "Cargo.toml");
  fs.writeFileSync(toml, fs.readFileSync(toml, "utf8").replace("CONVEX_FN_PATH", path.join(repo, "crates/convex_fn")));
  fs.copyFileSync(path.join(repo, "e2e/spec/rust-toolchain.toml"), path.join(crate, "rust-toolchain.toml"));
  fs.copyFileSync(path.join(repo, "e2e/spec/convex/rust/Cargo.lock"), path.join(crate, "Cargo.lock"));
  return { proj, crate };
}

describe.skipIf(!haveToolchain)("zero-config per-module build (real cargo)", () => {
  it("builds one wasm per module of a plain crate, each with only its functions", async () => {
    const { proj, crate } = setupProject();
    const prevTarget = process.env.CARGO_TARGET_DIR;
    process.env.CARGO_TARGET_DIR ??= path.join(repo, "target-e2e");
    try {
      const logs: string[] = [];
      const res = await buildOnce({ cwd: proj, profile: "release", log: (m) => logs.push(m) });
      expect(logs.join("\n")).toMatch(/one wasm per module \(3 modules/);
      const dir = path.join(crate, "_wasm");
      const wasms = fs.readdirSync(dir).filter((f) => f.endsWith(".wasm")).sort();
      expect(wasms.map((f) => f.replace(/^.*?\.(alpha|beta|nested__gamma)\.wasm$/, "$1"))).toEqual(["alpha", "beta", "nested__gamma"]);
      const fnsOf = (w: string) =>
        parseManifest(new Uint8Array(fs.readFileSync(path.join(dir, w)))).map((e) => `${e.modulePath.split("::").slice(1).join("/")}:${e.name}`).sort();
      const exportsOf = (w: string) =>
        WebAssembly.Module.exports(new WebAssembly.Module(fs.readFileSync(path.join(dir, w))))
          .map((e) => e.name).filter((n) => n.startsWith("convex_run:")).sort();
      const [a, b, g] = wasms;
      // `shared` exists in alpha and beta, so each of those builds keeps both (name-level keep); the other
      // modules' unique functions are gone from the exports.
      expect(exportsOf(a).some((n) => n.endsWith(":only_beta") || n.endsWith(":only_gamma"))).toBe(false);
      expect(exportsOf(b).some((n) => n.endsWith(":only_alpha") || n.endsWith(":only_gamma"))).toBe(false);
      expect(exportsOf(g)).toHaveLength(1);
      expect(fnsOf(g)).toEqual(["nested/gamma:only_gamma"]);
      expect(fnsOf(a)).toContain("alpha:only_alpha");
      expect(fnsOf(b)).toContain("beta:only_beta");
      const size = (w: string) => fs.statSync(path.join(dir, w)).size;
      expect(size(g)).toBeLessThan(size(a));
      expect(res.functions.sort()).toEqual(["alpha:only_alpha", "alpha:shared", "beta:only_beta", "beta:shared", "nested/gamma:only_gamma"]);
    } finally {
      if (prevTarget === undefined) delete process.env.CARGO_TARGET_DIR;
      else process.env.CARGO_TARGET_DIR = prevTarget;
      fs.rmSync(proj, { recursive: true, force: true });
    }
  }, 600_000);

  it("merges the smallest modules when over the size budget, and --single builds one wasm", async () => {
    const { proj, crate } = setupProject();
    const prevTarget = process.env.CARGO_TARGET_DIR;
    process.env.CARGO_TARGET_DIR ??= path.join(repo, "target-e2e");
    try {
      const logs: string[] = [];
      await buildOnce({ cwd: proj, profile: "release", wasmBudget: 1, log: (m) => logs.push(m) });
      expect(logs.join("\n")).toMatch(/over the .* budget/);
      const dir = path.join(crate, "_wasm");
      expect(fs.readdirSync(dir).filter((f) => f.endsWith(".wasm"))).toHaveLength(1);
      await buildOnce({ cwd: proj, profile: "release", perModule: false, log: () => {} });
      expect(fs.readdirSync(dir).filter((f) => f.endsWith(".wasm"))).toHaveLength(1);
    } finally {
      if (prevTarget === undefined) delete process.env.CARGO_TARGET_DIR;
      else process.env.CARGO_TARGET_DIR = prevTarget;
      fs.rmSync(proj, { recursive: true, force: true });
    }
  }, 600_000);

  it("builds one shared wasm in the dev profile unless per-module is forced", async () => {
    const { proj, crate } = setupProject();
    const prevTarget = process.env.CARGO_TARGET_DIR;
    process.env.CARGO_TARGET_DIR ??= path.join(repo, "target-e2e");
    try {
      const dir = path.join(crate, "_wasm");
      const count = () => fs.readdirSync(dir).filter((f) => f.endsWith(".wasm")).length;
      await buildOnce({ cwd: proj, profile: "dev", log: () => {} });
      expect(count()).toBe(1);
      await buildOnce({ cwd: proj, profile: "dev", perModule: true, log: () => {} });
      expect(count()).toBe(3);
    } finally {
      if (prevTarget === undefined) delete process.env.CARGO_TARGET_DIR;
      else process.env.CARGO_TARGET_DIR = prevTarget;
      fs.rmSync(proj, { recursive: true, force: true });
    }
  }, 600_000);
});

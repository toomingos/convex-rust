import { describe, expect, it } from "vitest";
import { collectEnviron } from "../src/runtime/host.js";
import { generateFiles } from "../src/cli/generate.js";
import { rustApiCodegen, parseAnalysisOutput } from "../src/cli/rustApi.js";
import { patchBlockBody, MANAGED_BEGIN, MANAGED_END, managedConfigToml } from "../src/cli/shim.js";
import { isWatchedChange, isTsSourceChange } from "../src/cli/build.js";

describe("env", () => {
  it("reads named vars from a get-only Proxy, plus CONVEX_RUST_ENV", () => {
    const store: Record<string, string> = { A: "1", B: "2", C: "3", CONVEX_RUST_ENV: "C, MISSING" };
    (globalThis as any).process = { env: new Proxy({}, { get: (_t, k) => store[k as string] }) };
    expect(collectEnviron(["A"]).sort()).toEqual(["A=1", "C=3"]);
    expect(collectEnviron([])).toEqual(["C=3"]);
  });
});

describe("generator env", () => {
  it("emits the env option and wasm hash", () => {
    const e: any = { abiVersion: 1, modulePath: "app::m", name: "f", visibility: "public", args: null, returns: null };
    const t = generateFiles({ entries: [e], wasmFile: "a.wasm", env: ["X"], wasmHash: "abc" }).get("m.ts")!;
    expect(t).toContain('args, { env: ["X"] })');
    expect(t).toContain("// wasm: abc");
  });
});

describe("api.rs", () => {
  it("generates typed refs", () => {
    const f = (t: any, optional = false) => ({ fieldType: t, optional });
    const out = rustApiCodegen({
      "messages.js": { functions: [{ name: "list", udfType: "Query", visibility: { kind: "public" },
        args: JSON.stringify({ type: "object", value: { limit: f({ type: "number" }, true) } }), returns: null },
        { name: "sec", udfType: "Query", visibility: { kind: "internal" }, args: null, returns: null }] },
    });
    expect(out).toContain("pub mod messages");
    expect(out).toContain('FunctionRef::new("messages:list")');
    expect(out).toContain("pub limit: Option<f64>");
    expect(out).toContain("pub mod internal");
  });
  it("parses analysis output", () => {
    expect(parseAnalysisOutput('x\n@@convex-analysis@@{"a.js":[]}\n')).toEqual({ "a.js": { functions: [] } });
  });
});

describe("shim block", () => {
  it("patches reqwest only when used", () => {
    expect(patchBlockBody("/s", undefined, true)).toContain('reqwest = { path = "/s" }');
    expect(patchBlockBody("/s", undefined, false)).not.toContain("[patch");
    const c = managedConfigToml({ targetDir: "../../.convex-rust/target" });
    expect(c).toContain('target-dir = "../../.convex-rust/target"');
    expect(c).toContain(MANAGED_BEGIN);
    expect(c).toContain(MANAGED_END);
  });
});

describe("watch filters", () => {
  it("ignores outputs", () => {
    expect(isWatchedChange("src/lib.rs")).toBe(true);
    expect(isWatchedChange("src/_generated/api.rs")).toBe(false);
    expect(isWatchedChange("target/x.rs")).toBe(false);
    expect(isTsSourceChange("messages.ts")).toBe(true);
    expect(isTsSourceChange("_generated/api.d.ts")).toBe(false);
    expect(isTsSourceChange("rust/.x.ts")).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { scaffoldFiles } from "../src/cli/scaffold.js";

describe("scaffold", () => {
  it("documents m-<module> features without enabling any (single wasm until the user opts in)", () => {
    const toml = scaffoldFiles("/tmp/x/rust", { convexFnPath: "/tmp/convex_fn" })["Cargo.toml"];
    expect(toml).toContain("m-<module>");
    expect(toml.split("\n").some((l) => l.startsWith("[features]"))).toBe(false);
  });

  it("pins stable (not a nightly) and the wasm target in rust-toolchain.toml", () => {
    const files = scaffoldFiles("/tmp/x/rust", { convexFnPath: "/tmp/convex_fn" });
    expect(files["rust-toolchain.toml"]).toContain('channel = "stable"');
    expect(files["rust-toolchain.toml"]).toContain("wasm32-wasip1");
    expect(scaffoldFiles("/tmp/x/rust", { channel: "nightly-2026-06-28" })["rust-toolchain.toml"]).toContain("nightly-2026-06-28");
  });
});

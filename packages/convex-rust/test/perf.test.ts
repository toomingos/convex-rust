import { describe, expect, it } from "vitest";
import { simdCargoConfig } from "../src/cli/build.js";
import { decodeBase64, decodeBase64Js, encodeBase64, encodeBase64Js } from "../src/runtime/base64.js";

describe("simd build option", () => {
  it("adds target.<triple>.rustflags by default", () => {
    const c = simdCargoConfig(true, {});
    expect(c.args).toEqual(["--config", 'target.wasm32-wasip1.rustflags=["-C","target-feature=+simd128"]']);
  });
  it("appends to RUSTFLAGS and is off with --no-simd", () => {
    expect(simdCargoConfig(true, { RUSTFLAGS: "-Dwarnings" }).env.RUSTFLAGS).toBe("-Dwarnings -C target-feature=+simd128");
    expect(simdCargoConfig(false, {})).toEqual({ args: [], env: {} });
  });
});

describe("base64 builtin and fallback agree", () => {
  it("round trips", () => {
    for (let n = 0; n < 70; n++) {
      const b = Uint8Array.from({ length: n }, (_, i) => (i * 91 + 3) & 255);
      expect(encodeBase64(b)).toBe(encodeBase64Js(b));
      expect(Array.from(decodeBase64(encodeBase64(b)))).toEqual(Array.from(decodeBase64Js(encodeBase64Js(b))));
    }
    expect(() => decodeBase64("a$b")).toThrow(/invalid base64/);
  });
});

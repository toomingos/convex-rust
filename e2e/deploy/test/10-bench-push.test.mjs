// Measurement only: never fails on numbers. Pushes the bench crate with `convex deploy`
// (release build + wasm-opt, i.e. what a real deploy does) and reports size and time.
import test, { after } from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { cleanup, makeApp, cli, DEPLOY_DIR, ROOT, CONVEX_FN_PATH, read, write } from "../lib/harness.mjs";

after(cleanup);

/** Inline the workspace-inherited keys so the bench crate builds as a standalone crate. */
function standaloneCargoToml(src) {
  let t = src.split("[dev-dependencies]")[0];
  t = t
    .replace(/^edition\.workspace = true$/m, 'edition = "2021"')
    .replace(/^license\.workspace = true$/m, 'license = "Apache-2.0"')
    .replace(/^version\.workspace = true$/m, 'version = "0.1.0"')
    .replace(/^convex_fn\.workspace = true$/m, `convex_fn = { path = ${JSON.stringify(CONVEX_FN_PATH)} }`)
    .replace(/^serde\.workspace = true$/m, 'serde = { version = "1", features = ["derive"] }')
    .replace(/^serde_json\.workspace = true$/m, 'serde_json = { version = "1", features = ["float_roundtrip"] }')
    .replace(/^base64\.workspace = true$/m, 'base64 = "0.22"');
  assert.doesNotMatch(t, /workspace = true/);
  return `${t}
[workspace]

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
panic = "abort"
debug = false
`;
}

test("10. large wasm: push size and time for the bench crate (measure only)", async () => {
  const benchRust = path.join(DEPLOY_DIR, "..", "..", "bench", "convex", "rust");
  const app = await makeApp();
  rmSync(path.join(app.rust, "src"), { recursive: true, force: true });
  cpSync(path.join(benchRust, "src"), path.join(app.rust, "src"), { recursive: true });
  write(app, "convex/rust/Cargo.toml", standaloneCargoToml(readFileSync(path.join(benchRust, "Cargo.toml"), "utf8")));

  // pin dependency versions to the workspace lock (unlocked resolution pulls `psm`, which does not build for wasm on macOS)
  cpSync(path.join(DEPLOY_DIR, "..", "..", "Cargo.lock"), path.join(app.rust, "Cargo.lock"));

  const cold = await cli(app, ["deploy", "--yes"], { timeout: 1_700_000 });
  console.log(`# bench deploy (cold cache) exit=${cold.code} wall=${(cold.ms / 1000).toFixed(1)}s`);
  const sizeLine = cold.out.split("\n").find((l) => l.startsWith("Rust wasm:"));
  console.log(`# ${sizeLine ?? "(no size line)"}`);
  if (cold.code !== 0) console.log(cold.out.split("\n").slice(-25).join("\n"));
  assert.equal(cold.code, 0, "bench crate should deploy");

  const wasmDir = path.join(DEPLOY_DIR, ".cache", "target", "wasm32-wasip1", "release");
  let bytes = null;
  try { bytes = statSync(path.join(wasmDir, "convex_rust.wasm")).size; } catch { /* name differs */ }
  const modules = new Set((cold.out.match(/rust\/\w+/g) ?? []));

  // identical re-deploy: cargo is a no-op, the backend already holds the wasm by hash
  const warm = await cli(app, ["deploy", "--yes"], { timeout: 1_700_000 });
  console.log(`# bench deploy (unchanged re-deploy) exit=${warm.code} wall=${(warm.ms / 1000).toFixed(1)}s`);

  const report = {
    coldWallS: cold.ms / 1000,
    warmWallS: warm.ms / 1000,
    wasmBytesOnDisk: bytes,
    sizeLine,
    modulesMentioned: modules.size,
    note: "10 modules each carry the whole wasm; backend dedups by hash",
  };
  mkdirSync(path.join(DEPLOY_DIR, ".run"), { recursive: true });
  writeFileSync(path.join(DEPLOY_DIR, ".run", "bench-push.json"), JSON.stringify(report, null, 2));
  console.log(`# ${JSON.stringify(report)}`);
  void read; void ROOT;
});

// Writes bench/convex/.build-info.json (wasm) or, with --native, bench/convex/.build-info-native.json.
//
// wasm: from the CLI's sidecar (<wasm>.buildinfo.json, written for release builds). Profile is "dev" when the
//   sidecar is missing or older than the wasm.
// native: the CLI writes no sidecar, so this script inspects the cdylib the CLI just built:
//   <CARGO_TARGET_DIR>/<triple>/<release|convex-dev>/libbench_rust.{dylib,so}. Records profile, triple, size,
//   sha256 and mtime. The CLI builds native with panic=unwind and strip=debuginfo (NATIVE.md 5.2); there is no wasm-opt.
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const bench = join(dirname(fileURLToPath(import.meta.url)), "..");
const target = process.env.CARGO_TARGET_DIR ?? join(bench, "..", "target-e2e");
const native = process.argv.includes("--native");
const release = process.env.CONVEX_RUST_PROFILE === "release";
const out = { profileEnv: process.env.CONVEX_RUST_PROFILE ?? null, builtAt: new Date().toISOString() };
let file;

if (native) {
  const profileDir = release ? "release" : "convex-dev";
  // Newest artifact across triple dirs (host build: exactly one triple).
  let best = null;
  for (const t of readdirSync(target)) {
    for (const ext of ["dylib", "so"]) {
      const f = join(target, t, profileDir, `libbench_rust.${ext}`);
      if (existsSync(f) && (best === null || statSync(f).mtimeMs > best.mtime)) {
        best = { f, triple: t, mtime: statSync(f).mtimeMs };
      }
    }
  }
  file = join(bench, "convex", ".build-info-native.json");
  if (best === null) {
    Object.assign(out, { runtime: "native", profile: release ? "release" : "dev", artifact: null, bytes: null });
  } else {
    const bytes = readFileSync(best.f);
    Object.assign(out, {
      runtime: "native",
      profile: release ? "release" : "dev",
      cargoProfileDir: profileDir,
      triple: best.triple,
      artifact: best.f,
      bytes: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      artifactMtime: new Date(best.mtime).toISOString(),
      panic: "unwind",
      strip: "debuginfo",
      wasmOpt: null,
    });
  }
} else {
  const rel = join(target, "wasm32-wasip1", "release", "bench_rust.wasm");
  const dev = join(target, "wasm32-wasip1", "convex-dev", "bench_rust.wasm");
  const info = rel + ".buildinfo.json";
  file = join(bench, "convex", ".build-info.json");
  if (release && existsSync(info)) {
    Object.assign(out, JSON.parse(readFileSync(info, "utf8")), { runtime: "wasm", wasm: rel });
  } else {
    Object.assign(out, { runtime: "wasm", profile: "dev", wasmOpt: null, wasm: dev, bytes: existsSync(dev) ? statSync(dev).size : null });
  }
}
writeFileSync(file, JSON.stringify(out, null, 2) + "\n");
console.log("build info:", JSON.stringify(out));

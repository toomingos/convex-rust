// Per-module wasm: one copy of the module per Convex module, with only that module's `convex_run:*`
// exports, so wasm-opt can drop the code (and its dependencies) the other modules need. A stock Convex
// deployment compiles every wasm a module imports on EVERY call (module scope is not kept between calls),
// at roughly 0.8 ms per MB, so a small module should not pay for the whole crate.
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { convexModuleName } from "./manifest.js";

function readLeb(b: Uint8Array, pos: number): [number, number] {
  let r = 0;
  let shift = 0;
  while (pos < b.length) {
    const x = b[pos++];
    r += (x & 0x7f) * 2 ** shift;
    if ((x & 0x80) === 0) return [r, pos];
    shift += 7;
  }
  throw new Error("truncated LEB128");
}
function lebBytes(n: number): number[] {
  const out: number[] = [];
  do {
    let b = n % 128;
    n = Math.floor(n / 128);
    if (n > 0) b |= 0x80;
    out.push(b);
  } while (n > 0);
  return out;
}

/** Rewrites the export section, keeping only the exports for which `keep(name)` is true. */
export function filterExports(wasm: Uint8Array, keep: (name: string) => boolean): Uint8Array {
  const dec = new TextDecoder();
  let pos = 8;
  while (pos < wasm.length) {
    const start = pos;
    const id = wasm[pos++];
    let size: number;
    [size, pos] = readLeb(wasm, pos);
    const payload = pos;
    const end = pos + size;
    if (id === 7) {
      let count: number;
      let p: number;
      [count, p] = readLeb(wasm, payload);
      const kept: number[][] = [];
      for (let i = 0; i < count; i++) {
        const s = p;
        let len: number;
        [len, p] = readLeb(wasm, p);
        const name = dec.decode(wasm.subarray(p, p + len));
        p += len + 1; // kind
        [, p] = readLeb(wasm, p); // index
        if (keep(name)) kept.push(Array.from(wasm.subarray(s, p)));
      }
      const body = [...lebBytes(kept.length), ...kept.flat()];
      const out = new Uint8Array(start + 1 + lebBytes(body.length).length + body.length + (wasm.length - end));
      out.set(wasm.subarray(0, start), 0);
      let o = start;
      out[o++] = 7;
      for (const x of lebBytes(body.length)) out[o++] = x;
      out.set(body, o);
      o += body.length;
      out.set(wasm.subarray(end), o);
      return out;
    }
    pos = end;
  }
  return wasm;
}

/**
 * Whether an export stays in a module's wasm. `prefixes` are `convex_run:<module_path>:`. The match is
 * exact: what follows the prefix is the function name (no `:`), so `convex_run:c::a::b:f` is not kept in
 * the wasm of module `c::a`.
 */
export function keepsExport(exportName: string, prefixes: string[]): boolean {
  return (
    !exportName.startsWith("convex_run:") ||
    prefixes.some((p) => exportName.startsWith(p) && !exportName.includes(":", p.length))
  );
}

export interface SplitOptions {
  /** wasm-opt flags (same as the whole-module optimization). */
  wasmOptArgs: string[];
  cacheDir: string;
  log?: (m: string) => void;
}

/**
 * One wasm per Convex module name (`cpu`, `a/b`), keyed by that name. `modulePaths` are the Rust
 * `module_path!()` values of the functions. Returns null when `wasm-opt` is unavailable or fails
 * (callers fall back to the single shared wasm).
 */
export function splitWasmByModule(
  wasm: Uint8Array,
  modulePaths: string[],
  opts: SplitOptions,
): Map<string, Uint8Array> | null {
  const log = opts.log ?? (() => {});
  const ver = spawnSync("wasm-opt", ["--version"], { encoding: "utf8" });
  if (ver.status !== 0) {
    log("--split needs wasm-opt (binaryen) on PATH; using one shared wasm");
    return null;
  }
  const modules = new Map<string, Set<string>>();
  for (const mp of modulePaths) {
    const m = convexModuleName(mp);
    if (!modules.has(m)) modules.set(m, new Set());
    modules.get(m)!.add(mp);
  }
  const prefix = (mp: string) => `convex_run:${mp}:`;
  const out = new Map<string, Uint8Array>();
  fs.mkdirSync(opts.cacheDir, { recursive: true });
  for (const [name, mps] of modules) {
    const prefixes = [...mps].map(prefix);
    const filtered = filterExports(
      wasm,
      (e) => keepsExport(e, prefixes),
    );
    const key = createHash("sha256")
      .update(ver.stdout + "\0" + opts.wasmOptArgs.join(" ") + "\0split")
      .update(filtered)
      .digest("hex");
    const cached = path.join(opts.cacheDir, `${key}.wasm`);
    if (fs.existsSync(cached)) {
      out.set(name, new Uint8Array(fs.readFileSync(cached)));
      continue;
    }
    const src = path.join(opts.cacheDir, `${key}.${process.pid}.in`);
    const dst = path.join(opts.cacheDir, `${key}.${process.pid}.tmp`);
    fs.writeFileSync(src, filtered);
    try {
      const r = spawnSync("wasm-opt", [...opts.wasmOptArgs, "--remove-unused-module-elements", src, "-o", dst], {
        stdio: ["ignore", "ignore", "pipe"],
        encoding: "utf8",
      });
      if (r.status !== 0) {
        log(`wasm-opt failed splitting ${name} (${r.status}); using one shared wasm`);
        return null;
      }
      const bytes = new Uint8Array(fs.readFileSync(dst));
      fs.renameSync(dst, cached);
      out.set(name, bytes);
    } finally {
      fs.rmSync(src, { force: true });
      fs.rmSync(dst, { force: true });
    }
  }
  return out;
}

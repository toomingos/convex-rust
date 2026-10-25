/**
 * Function-name map for the deployed wasm. The shipped wasm has its name section stripped (wasm-opt without -g),
 * so the names are captured at build time and kept next to cargo's target dir, never in the deployed bundle.
 * `convex-rust symbolize` uses them to rewrite `wasm-function[123]:0x...` frames from Convex logs.
 */
import fs from "node:fs";
import path from "node:path";
import { readLeb } from "./size.js";
import { demangle } from "./demangle.js";

export interface SymbolMap {
  version: 1;
  /** File name under `_wasm/`. */
  wasm: string;
  /** Convex modules (`cpu`, `a/b`) whose functions this wasm exports. */
  modules: string[];
  /** Function index (imports first, as in `wasm-function[N]`) to demangled name. */
  names: Record<string, string>;
}

const dec = new TextDecoder();

/** Function names (subsection 1) of the "name" custom section, by function index. */
export function parseNameSection(wasm: Uint8Array): Map<number, string> {
  const out = new Map<number, string>();
  let pos = 8;
  while (pos < wasm.length) {
    const id = wasm[pos++];
    let size: number;
    [size, pos] = readLeb(wasm, pos);
    const end = pos + size;
    if (id === 0) {
      let len: number;
      let p: number;
      [len, p] = readLeb(wasm, pos);
      if (dec.decode(wasm.subarray(p, p + len)) === "name") {
        p += len;
        while (p < end) {
          const sub = wasm[p++];
          let subSize: number;
          [subSize, p] = readLeb(wasm, p);
          const subEnd = p + subSize;
          if (sub === 1) {
            let n: number;
            [n, p] = readLeb(wasm, p);
            for (let i = 0; i < n; i++) {
              let idx: number;
              let l: number;
              [idx, p] = readLeb(wasm, p);
              [l, p] = readLeb(wasm, p);
              out.set(idx, dec.decode(wasm.subarray(p, p + l)));
              p += l;
            }
          }
          p = subEnd;
        }
      }
    }
    pos = end;
  }
  return out;
}

/** Parses `wasm-opt --print-function-map` output (`index:name` per line). */
export function parseFunctionMap(text: string): Map<number, string> {
  const out = new Map<number, string>();
  for (const line of text.split("\n")) {
    const m = line.match(/^(\d+):(.+)$/);
    if (m) out.set(Number(m[1]), m[2].trim());
  }
  return out;
}

export function buildSymbolMap(
  names: Map<number, string>,
  info: { wasm: string; modules: string[] },
): SymbolMap {
  const out: Record<string, string> = {};
  for (const [idx, n] of [...names].sort((a, b) => a[0] - b[0])) out[String(idx)] = demangle(n);
  return { version: 1, wasm: info.wasm, modules: info.modules, names: out };
}

export function symbolsDir(targetDir: string): string {
  return path.join(targetDir, "convex-rust-symbols");
}

/** Replaces the maps in `dir` with `maps` (stale ones removed). */
export function writeSymbolMaps(dir: string, maps: SymbolMap[]): string[] {
  fs.mkdirSync(dir, { recursive: true });
  const keep = new Set(maps.map((m) => `${m.wasm}.json`));
  for (const f of fs.readdirSync(dir)) {
    if (f.endsWith(".wasm.json") && !keep.has(f)) fs.rmSync(path.join(dir, f), { force: true });
  }
  return maps.map((m) => {
    const file = path.join(dir, `${m.wasm}.json`);
    fs.writeFileSync(file, JSON.stringify(m));
    return file;
  });
}

export function readSymbolMaps(dir: string): SymbolMap[] {
  let files: string[] = [];
  try {
    files = fs.readdirSync(dir).filter((f) => f.endsWith(".wasm.json")).sort();
  } catch {
    return [];
  }
  return files.map((f) => JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")) as SymbolMap);
}

const FRAME = /wasm-function\[(\d+)\](:0x[0-9a-fA-F]+)?/g;

/**
 * Rewrites `wasm-function[N]:0xOFF` into `name (wasm-function[N]:0xOFF)`. With several maps, `module` picks one;
 * otherwise a module named like `rust/<module>` in the text does, if exactly one does.
 */
export function symbolize(
  text: string,
  maps: SymbolMap[],
  module?: string,
): { text: string; replaced: number; note?: string } {
  if (maps.length === 0) return { text, replaced: 0, note: "no symbol maps found (run `convex-rust build` first)" };
  let map: SymbolMap | undefined;
  let note: string | undefined;
  if (maps.length === 1) map = maps[0];
  else if (module) {
    map = maps.find(
      (m) => m.modules.includes(module) || m.wasm.replace(/\.wasm$/, "").endsWith(`.${module.replace(/\//g, "__")}`),
    );
    if (!map) note = `no symbol map for module "${module}" (have: ${maps.flatMap((m) => m.modules).join(", ")})`;
  } else {
    const seen = new Set<SymbolMap>();
    for (const m of maps) for (const mod of m.modules) if (text.includes(`rust/${mod}`)) seen.add(m);
    if (seen.size === 1) map = [...seen][0];
    else note = `several wasm modules were built; pass --module <name> (one of: ${maps.flatMap((m) => m.modules).join(", ")})`;
  }
  if (!map) return { text, replaced: 0, ...(note ? { note } : {}) };
  let replaced = 0;
  const out = text.replace(FRAME, (all, idx: string) => {
    const n = map!.names[idx];
    if (!n) return all;
    replaced++;
    return `${n} (${all})`;
  });
  return { text: out, replaced };
}

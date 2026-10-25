/**
 * Build-time scan of the compiled wasm's imports and memories for things that fail (or silently
 * answer ENOSYS) on the Convex host. Only reads the binary format; nothing is instantiated.
 */
import { readLeb } from "./size.js";
import { makeWasi, LineBuffer } from "../runtime/wasi.js";
import { TmpFs } from "../runtime/tmpfs.js";

export interface WasmImport {
  module: string;
  name: string;
  kind: "func" | "table" | "memory" | "global" | "tag";
  /** memory/table imports: limits flags (bit 1 shared, bit 2 64-bit). */
  flags?: number;
}

export interface WasmMemory {
  flags: number;
  imported: boolean;
}

export interface WasmInfo {
  imports: WasmImport[];
  memories: WasmMemory[];
  exports: string[];
}

const KINDS = ["func", "table", "memory", "global", "tag"] as const;
const dec = new TextDecoder();

function readName(b: Uint8Array, pos: number): [string, number] {
  let len: number;
  [len, pos] = readLeb(b, pos);
  return [dec.decode(b.subarray(pos, pos + len)), pos + len];
}

/** Reads a `limits` entry; returns [flags, newPos]. */
function readLimits(b: Uint8Array, pos: number): [number, number] {
  let flags: number;
  [flags, pos] = readLeb(b, pos);
  [, pos] = readLeb(b, pos);
  if (flags & 1) [, pos] = readLeb(b, pos);
  return [flags, pos];
}

export function scanWasm(wasm: Uint8Array): WasmInfo {
  const info: WasmInfo = { imports: [], memories: [], exports: [] };
  let pos = 8;
  while (pos < wasm.length) {
    const id = wasm[pos++];
    let size: number;
    [size, pos] = readLeb(wasm, pos);
    const end = pos + size;
    let p = pos;
    let count: number;
    if (id === 2) {
      [count, p] = readLeb(wasm, p);
      for (let i = 0; i < count; i++) {
        let module: string;
        let name: string;
        [module, p] = readName(wasm, p);
        [name, p] = readName(wasm, p);
        const kind = KINDS[wasm[p++]] ?? "func";
        const imp: WasmImport = { module, name, kind };
        if (kind === "func") [, p] = readLeb(wasm, p);
        else if (kind === "tag") {
          p++;
          [, p] = readLeb(wasm, p);
        } else if (kind === "table") {
          p++; // reftype
          [imp.flags, p] = readLimits(wasm, p);
        } else if (kind === "memory") {
          [imp.flags, p] = readLimits(wasm, p);
          info.memories.push({ flags: imp.flags, imported: true });
        } else p += 2; // global: valtype, mutability
        info.imports.push(imp);
      }
    } else if (id === 5) {
      [count, p] = readLeb(wasm, p);
      for (let i = 0; i < count; i++) {
        let flags: number;
        [flags, p] = readLimits(wasm, p);
        info.memories.push({ flags, imported: false });
      }
    } else if (id === 7) {
      [count, p] = readLeb(wasm, p);
      for (let i = 0; i < count; i++) {
        let name: string;
        [name, p] = readName(wasm, p);
        p++;
        [, p] = readLeb(wasm, p);
        info.exports.push(name);
      }
    }
    pos = end;
  }
  return info;
}

export interface Hint {
  id: string;
  title: string;
  /** One-line fix. */
  fix: string;
}

/** WASI functions the host implements (everything else answers ENOSYS), read from the runtime itself. */
export function hostWasiFunctions(): Set<string> {
  const lb = new LineBuffer(() => {});
  // The in-memory /tmp is part of the host's table (path_open, fd_prestat_*, fd_readdir ...).
  const memory = () => undefined as any;
  const fns = makeWasi({ memory, environ: [], line: () => {}, tmp: new TmpFs({ memory }) }, { 1: lb, 2: lb });
  return new Set(Object.keys(fns));
}

/** Functions of the `convex` import module the host provides (src/runtime/host.ts). */
export const HOST_CONVEX_IMPORTS = new Set(["task_submit", "task_poll", "sync_syscall", "log"]);

const SOCKETS = /^sock_/;
// Only calls that open or change paths: wasi-libc/std import fd_close, fd_fdstat_get, fd_seek ... even for stdio.
const FS = /^(path_|fd_readdir$|fd_prestat_)/;

export function wasmHints(wasm: Uint8Array, implemented: Set<string> = hostWasiFunctions()): Hint[] {
  const info = scanWasm(wasm);
  const hints: Hint[] = [];
  const wasi = info.imports.filter((i) => i.module === "wasi_snapshot_preview1" && i.kind === "func");
  const missing = (re: RegExp) => wasi.filter((i) => re.test(i.name) && !implemented.has(i.name)).map((i) => i.name);

  const sock = missing(SOCKETS);
  if (sock.length) {
    hints.push({
      id: "wasi-sockets",
      title: `imports ${sock.join(", ")}: raw sockets (TCP/UDP/DNS) return ENOSYS on the Convex host`,
      fix: 'use HTTP through reqwest/ctx.fetch instead of a socket driver, or run that part in a "use node" action (ctx.run_action).',
    });
  }
  const fs = missing(FS);
  if (fs.length) {
    hints.push({
      id: "wasi-fs",
      title: `imports ${fs.join(", ")}: std::fs and opening paths return ENOSYS (the host has no filesystem or VFS for them)`,
      fix: "keep data in memory or in Convex tables/storage (ctx.storage); check dependencies that read config or temp files at runtime.",
    });
  }
  if (wasi.some((i) => i.name === "proc_raise") && !implemented.has("proc_raise")) {
    hints.push({
      id: "wasi-proc-raise",
      title: "imports proc_raise: raise()/abort-by-signal returns ENOSYS and the call keeps running",
      fix: 'build with `panic = "abort"` and avoid C code that calls abort()/raise(); report errors as Result instead.',
    });
  }
  const threads = info.imports.filter((i) => i.module === "wasi" && /thread/.test(i.name));
  const shared = info.memories.some((m) => (m.flags & 2) !== 0);
  if (threads.length || shared) {
    hints.push({
      id: "threads",
      title: shared
        ? "uses shared memory (threads / atomics build): the host cannot start threads"
        : `imports ${threads.map((t) => `${t.module}.${t.name}`).join(", ")}: the host cannot start threads`,
      fix: 'remove `-C target-feature=+atomics` / wasi-threads settings and replace rayon/std::thread::spawn with sequential code, or use a "use node" action.',
    });
  }
  if (info.memories.some((m) => (m.flags & 4) !== 0)) {
    hints.push({
      id: "memory64",
      title: "uses a 64-bit memory (memory64), which the Convex runtime does not support",
      fix: "build for wasm32-wasip1 only (no memory64 target features).",
    });
  }
  const badHost = info.imports.filter((i) => i.module === "convex" && !HOST_CONVEX_IMPORTS.has(i.name)).map((i) => i.name);
  if (badHost.length) {
    hints.push({
      id: "host-imports",
      title: `needs host import(s) convex.${badHost.join(", convex.")} that the installed convex-rust runtime does not provide`,
      fix: "the convex_fn crate is newer than the npm package: update convex-rust (npm i convex-rust@latest) or pin the crate to the matching version.",
    });
  }
  const foreign = info.imports.filter(
    (i) => i.kind !== "memory" && i.module !== "convex" && i.module !== "wasi_snapshot_preview1" && i.module !== "wasi",
  );
  if (foreign.length) {
    const names = foreign.slice(0, 6).map((i) => `${i.module}.${i.name}`);
    hints.push({
      id: "foreign-imports",
      title: `imports ${names.join(", ")}${foreign.length > names.length ? ` (+${foreign.length - names.length} more)` : ""}, which the host does not provide: the module will fail to instantiate`,
      fix: "these come from wasm-bindgen / emscripten / wasm32-unknown-unknown style crates: build only for wasm32-wasip1 and drop the dependency or its `js`/`wasm-bindgen` feature (`cargo tree -i <crate> --target wasm32-wasip1`).",
    });
  }
  if (info.imports.some((i) => i.kind === "memory")) {
    hints.push({
      id: "imported-memory",
      title: "imports its linear memory (env.memory), which the host does not provide",
      fix: "do not pass `--import-memory` to the linker (check RUSTFLAGS / .cargo/config.toml).",
    });
  }
  return hints;
}

export function formatWasmHints(hints: Hint[], label = ""): string[] {
  const l = label.trim().replace(/^\[|\]$/g, "");
  return hints.map((h) => `warning:${l ? ` [${l}]` : ""} the wasm ${h.title}.\n  fix: ${h.fix}`);
}

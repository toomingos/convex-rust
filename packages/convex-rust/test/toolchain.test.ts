import { describe, expect, it } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { buildOnce } from "../src/cli/build.js";
import { demangle } from "../src/cli/demangle.js";
import { formatWasmHints, scanWasm, wasmHints } from "../src/cli/wasmHints.js";
import {
  buildSymbolMap,
  parseFunctionMap,
  parseNameSection,
  readSymbolMaps,
  symbolize,
  writeSymbolMaps,
} from "../src/cli/symbols.js";
import {
  assetForPlatform,
  cachedSdkDir,
  crateNamesWithCBuild,
  findWasiSdk,
  prepareWasiSdk,
  wasiSdkEnv,
  wasiSdkMode,
} from "../src/cli/wasisdk.js";
import { runRust } from "../src/runtime/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");

// --- a tiny wasm writer: imports, memories, optional name section -----------------------------------------------

const u32 = (n: number): number[] => {
  const out: number[] = [];
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n) b |= 0x80;
    out.push(b);
  } while (n);
  return out;
};
const str = (s: string): number[] => [...u32(Buffer.byteLength(s)), ...Buffer.from(s)];
const section = (id: number, body: number[]): number[] => [id, ...u32(body.length), ...body];

interface Imp {
  module: string;
  name: string;
  /** "func" (type 0) or a memory with these limits flags. */
  memFlags?: number;
}

function makeWasm(opts: { imports?: Imp[]; memories?: number[]; names?: [number, string][] }): Uint8Array {
  const bytes: number[] = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
  bytes.push(...section(1, [1, 0x60, 0, 0])); // one type: () -> ()
  const imports = opts.imports ?? [];
  if (imports.length) {
    const body = [...u32(imports.length)];
    for (const i of imports) {
      body.push(...str(i.module), ...str(i.name));
      if (i.memFlags !== undefined) body.push(2, i.memFlags, 1, ...(i.memFlags & 1 ? [2] : []));
      else body.push(0, 0);
    }
    bytes.push(...section(2, body));
  }
  if (opts.memories?.length) {
    const body = [...u32(opts.memories.length)];
    for (const f of opts.memories) body.push(f, 1, ...(f & 1 ? [2] : []));
    bytes.push(...section(5, body));
  }
  if (opts.names) {
    const sub = [...u32(opts.names.length)];
    for (const [i, n] of opts.names) sub.push(...u32(i), ...str(n));
    bytes.push(...section(0, [...str("name"), 1, ...u32(sub.length), ...sub]));
  }
  return new Uint8Array(bytes);
}

describe("wasm build-time hints", () => {
  it("scans imports and memories", () => {
    const w = makeWasm({
      imports: [{ module: "wasi_snapshot_preview1", name: "fd_write" }, { module: "env", name: "memory", memFlags: 3 }],
    });
    const info = scanWasm(w);
    expect(info.imports.map((i) => `${i.module}.${i.name}:${i.kind}`)).toEqual([
      "wasi_snapshot_preview1.fd_write:func",
      "env.memory:memory",
    ]);
    expect(info.memories).toEqual([{ flags: 3, imported: true }]);
  });

  it("is quiet for what the host implements", () => {
    const w = makeWasm({
      imports: ["fd_write", "clock_time_get", "random_get", "environ_get", "proc_exit", "fd_close", "fd_fdstat_get", "fd_seek"].map((name) => ({
        module: "wasi_snapshot_preview1",
        name,
      })).concat([{ module: "convex", name: "log" }]),
      memories: [1],
    });
    expect(wasmHints(w)).toEqual([]);
  });

  it("flags sockets, paths, proc_raise, host imports and foreign imports with a fix each", () => {
    const w = makeWasm({
      imports: [
        { module: "wasi_snapshot_preview1", name: "sock_accept" },
        { module: "wasi_snapshot_preview1", name: "sock_send" },
        { module: "wasi_snapshot_preview1", name: "proc_raise" },
        { module: "convex", name: "teleport" },
        { module: "wbg", name: "__wbindgen_throw" },
      ],
    });
    const hints = wasmHints(w);
    expect(hints.map((h) => h.id)).toEqual(["wasi-sockets", "wasi-proc-raise", "host-imports", "foreign-imports"]);
    expect(hints[0].title).toContain("sock_accept, sock_send");
    for (const h of hints) expect(h.fix.length).toBeGreaterThan(10);
    expect(formatWasmHints(hints, " [cpu]")[0]).toMatch(/^warning: \[cpu\] the wasm imports sock_accept/);
  });

  it("flags threads, shared and imported memory, memory64", () => {
    expect(wasmHints(makeWasm({ imports: [{ module: "wasi", name: "thread-spawn" }] })).map((h) => h.id)).toEqual(["threads"]);
    expect(wasmHints(makeWasm({ memories: [3] })).map((h) => h.id)).toEqual(["threads"]);
    expect(wasmHints(makeWasm({ memories: [4] })).map((h) => h.id)).toEqual(["memory64"]);
    expect(wasmHints(makeWasm({ imports: [{ module: "env", name: "memory", memFlags: 0 }] })).map((h) => h.id)).toEqual(["imported-memory"]);
  });

  it("does not flag the in-memory /tmp calls (std::fs on /tmp works on the host)", () => {
    const names = ["path_open", "fd_prestat_get", "fd_prestat_dir_name", "fd_readdir", "path_create_directory", "path_unlink_file"];
    const w = makeWasm({ imports: names.map((name) => ({ module: "wasi_snapshot_preview1", name })) });
    expect(wasmHints(w)).toEqual([]);
  });

  it("still flags path calls for a host that implements none of them", () => {
    const w = makeWasm({ imports: [{ module: "wasi_snapshot_preview1", name: "path_open" }] });
    expect(wasmHints(w, new Set()).map((h) => h.id)).toEqual(["wasi-fs"]);
  });

  it("treats a WASI function the runtime gains as supported", () => {
    const w = makeWasm({ imports: [{ module: "wasi_snapshot_preview1", name: "path_open" }] });
    expect(wasmHints(w, new Set(["path_open"]))).toEqual([]);
  });
});

describe("demangle", () => {
  it("demangles v0 symbols", () => {
    expect(demangle("_RNvCsjDvmBtyXmUz_9convex_fn3foo")).toBe("convex_fn::foo");
    expect(
      demangle("_RINvNtCskedA98lqrHQ_4core3ptr9drop_glueNtNtCshQ60cCGDwVI_10serde_json5error5ErrorECs3OacLBr8vRA_14c_deps_fixture"),
    ).toBe("core::ptr::drop_glue::<serde_json::error::Error>");
    expect(
      demangle("_RNvMs3_NtCshQ60cCGDwVI_10serde_json2deINtB5_12DeserializerNtNtB7_4read9SliceReadE12parse_numberCs3OacLBr8vRA_14c_deps_fixture"),
    ).toBe("<serde_json::de::Deserializer::<serde_json::read::SliceRead>>::parse_number");
    expect(
      demangle("_RNvXs1i_NtCskedA98lqrHQ_4core3fmtRDNtNtCshxISO5biF4o_10serde_core2de8ExpectedEL_NtB6_7Display3fmtCs3OacLBr8vRA_14c_deps_fixture.llvm.8376296816953989302"),
    ).toBe("<&dyn serde_core::de::Expected as core::fmt::Display>::fmt");
    expect(demangle("_RNCNCNvNtCsjDvmBtyXmUz_9convex_fn5error18install_panic_hook00B7_.llvm.1")).toBe(
      "convex_fn::error::install_panic_hook::{closure}::{closure}",
    );
  });
  it("demangles legacy symbols and leaves other names alone", () => {
    expect(demangle("_ZN4core3fmt5write17h0123456789abcdefE")).toBe("core::fmt::write");
    expect(demangle("convex_alloc")).toBe("convex_alloc");
    expect(demangle("_RNvCs")).toBe("_RNvCs");
  });
});

describe("symbols", () => {
  it("reads the name section and wasm-opt's function map", () => {
    const w = makeWasm({ names: [[0, "a"], [2, "_RNvCsjDvmBtyXmUz_9convex_fn3foo"]] });
    expect(parseNameSection(w)).toEqual(new Map([[0, "a"], [2, "_RNvCsjDvmBtyXmUz_9convex_fn3foo"]]));
    expect(parseFunctionMap("0:a\n1:b c\nnot a line\n2:d")).toEqual(new Map([[0, "a"], [1, "b c"], [2, "d"]]));
  });

  it("rewrites wasm-function frames from Convex logs", () => {
    const map = buildSymbolMap(new Map([[123, "_RNvCsjDvmBtyXmUz_9convex_fn3foo"], [7, "bar"]]), { wasm: "x.cpu.wasm", modules: ["cpu"] });
    const log = [
      "RuntimeError: unreachable",
      "    at wasm-function[123]:0x4a5b",
      "    at wasm://wasm/abc:wasm-function[7]:0x10",
      "    at wasm-function[999]:0x1",
    ].join("\n");
    const res = symbolize(log, [map]);
    expect(res.replaced).toBe(2);
    expect(res.text).toContain("at convex_fn::foo (wasm-function[123]:0x4a5b)");
    expect(res.text).toContain("bar (wasm-function[7]:0x10)");
    expect(res.text).toContain("at wasm-function[999]:0x1");
  });

  it("picks the wasm by --module or by a rust/<module> mention, and asks otherwise", () => {
    const a = buildSymbolMap(new Map([[1, "from_a"]]), { wasm: "r.a.wasm", modules: ["a"] });
    const b = buildSymbolMap(new Map([[1, "from_b"]]), { wasm: "r.nested__b.wasm", modules: ["nested/b"] });
    const frame = "at wasm-function[1]:0x0";
    expect(symbolize(frame, [a, b], "nested/b").text).toContain("from_b");
    expect(symbolize(`[rust/a.js:run] ${frame}`, [a, b]).text).toContain("from_a");
    const amb = symbolize(frame, [a, b]);
    expect(amb.replaced).toBe(0);
    expect(amb.note).toMatch(/--module/);
    expect(symbolize(frame, []).note).toMatch(/build/);
  });

  it("writes maps next to the target dir and drops stale ones", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "symbols-"));
    try {
      const a = buildSymbolMap(new Map([[0, "x"]]), { wasm: "a.wasm", modules: ["a"] });
      const b = buildSymbolMap(new Map([[0, "y"]]), { wasm: "b.wasm", modules: ["b"] });
      writeSymbolMaps(dir, [a, b]);
      expect(readSymbolMaps(dir).map((m) => m.wasm)).toEqual(["a.wasm", "b.wasm"]);
      writeSymbolMaps(dir, [b]);
      expect(readSymbolMaps(dir).map((m) => m.wasm)).toEqual(["b.wasm"]);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("wasi-sdk", () => {
  const fakeSdk = (dir: string) => {
    fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
    fs.mkdirSync(path.join(dir, "share", "wasi-sysroot"), { recursive: true });
    fs.writeFileSync(path.join(dir, "bin", process.platform === "win32" ? "clang.exe" : "clang"), "");
  };

  it("finds WASI_SDK_PATH before the cache dir", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "sdk-"));
    try {
      const env = { CONVEX_RUST_CACHE_DIR: path.join(root, "cache") };
      expect(findWasiSdk(env)).toBeNull();
      fakeSdk(cachedSdkDir(env));
      expect(findWasiSdk(env)).toEqual({ dir: cachedSdkDir(env), source: "cache" });
      fakeSdk(path.join(root, "own"));
      expect(findWasiSdk({ ...env, WASI_SDK_PATH: path.join(root, "own") })?.source).toBe("env");
      expect(findWasiSdk({ ...env, WASI_SDK_PATH: path.join(root, "missing") })?.source).toBe("cache");
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it("sets the cc-rs variables for wasm32-wasip1 and respects the user's own", () => {
    const e = wasiSdkEnv("/sdk", {});
    expect(e.CC_wasm32_wasip1).toMatch(/clang(\.exe)?$/);
    expect(e.AR_wasm32_wasip1).toMatch(/llvm-ar(\.exe)?$/);
    expect(e.CFLAGS_wasm32_wasip1).toBe(`--sysroot=${path.join("/sdk", "share", "wasi-sysroot")}`);
    const mine = wasiSdkEnv("/sdk", { CC_wasm32_wasip1: "/my/clang", "CFLAGS_wasm32-wasip1": "-O1" });
    expect(mine.CC_wasm32_wasip1).toBeUndefined();
    expect(mine.CFLAGS_wasm32_wasip1).toBeUndefined();
    expect(mine.AR_wasm32_wasip1).toBeDefined();
  });

  it("pins an archive with a sha256 for each platform", () => {
    for (const [p, a] of [["darwin", "arm64"], ["darwin", "x64"], ["linux", "x64"], ["linux", "arm64"], ["win32", "x64"]]) {
      expect(assetForPlatform(p, a)?.sha256).toMatch(/^[0-9a-f]{64}$/);
    }
    expect(assetForPlatform("freebsd", "x64")).toBeNull();
  });

  it("detects crates whose build script uses cc", () => {
    const meta = {
      packages: [
        { id: "zstd-sys", name: "zstd-sys" },
        { id: "cc", name: "cc" },
        { id: "serde", name: "serde" },
        { id: "app", name: "app" },
      ],
      resolve: {
        nodes: [
          { id: "zstd-sys", deps: [{ name: "cc", pkg: "cc", dep_kinds: [{ kind: "build" }] }] },
          { id: "serde", deps: [] },
          { id: "app", deps: [{ name: "cc", pkg: "cc", dep_kinds: [{ kind: null }] }] },
        ],
      },
    };
    expect(crateNamesWithCBuild(meta)).toEqual(["zstd-sys"]);
  });

  it("downloads only when needed (auto), always (always) or never", async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "sdk-"));
    try {
      const env = { CONVEX_RUST_CACHE_DIR: path.join(root, "cache") };
      const logs: string[] = [];
      let installs = 0;
      const install = async () => {
        installs++;
        fakeSdk(cachedSdkDir(env));
        return cachedSdkDir(env);
      };
      const base = { env, log: (m: string) => logs.push(m), install: install as any };
      expect(await prepareWasiSdk({ ...base, mode: "auto", cNames: () => [] })).toEqual({});
      expect(await prepareWasiSdk({ ...base, mode: "never", cNames: () => ["zstd-sys"] })).toEqual({});
      expect(installs).toBe(0);
      const got = await prepareWasiSdk({ ...base, mode: "auto", cNames: () => ["zstd-sys"] });
      expect(installs).toBe(1);
      expect(Object.keys(got).sort()).toEqual(["AR_wasm32_wasip1", "CC_wasm32_wasip1", "CFLAGS_wasm32_wasip1", "CXXFLAGS_wasm32_wasip1", "CXX_wasm32_wasip1"]);
      expect(logs.join("\n")).toMatch(/zstd-sys/);
      // Installed now: used without asking the crate graph again.
      const again = await prepareWasiSdk({ ...base, mode: "auto", cNames: () => { throw new Error("not needed"); } });
      expect(again.CC_wasm32_wasip1).toBeDefined();
      expect(installs).toBe(1);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it("maps flags and env to a mode", () => {
    expect(wasiSdkMode({}, {})).toBe("auto");
    expect(wasiSdkMode({ wasiSdk: true }, {})).toBe("always");
    expect(wasiSdkMode({}, { CONVEX_RUST_WASI_SDK: "1" })).toBe("always");
    expect(wasiSdkMode({ wasiSdk: true, noWasiSdk: true }, {})).toBe("never");
    expect(wasiSdkMode({}, { CONVEX_RUST_WASI_SDK: "0" })).toBe("never");
  });
});

// --- real build: a C-wrapping crate (zstd-sys) for wasm32-wasip1 with a wasi-sdk --------------------------------

const haveToolchain = spawnSync("rustup", ["target", "list", "--installed"], {
  encoding: "utf8",
  cwd: path.join(repo, "e2e/spec/convex/rust"),
}).stdout?.includes("wasm32-wasip1");
const haveSdk = findWasiSdk() !== null;

describe.skipIf(!haveToolchain || !haveSdk)("C dependencies with wasi-sdk (real cargo)", () => {
  it("builds zstd-sys, keeps symbols outside the bundle, and the function runs", async () => {
    const proj = fs.mkdtempSync(path.join(os.tmpdir(), "cdeps-"));
    const crate = path.join(proj, "convex", "rust");
    fs.cpSync(path.join(here, "fixtures/c-deps-crate"), crate, { recursive: true });
    const toml = path.join(crate, "Cargo.toml");
    fs.writeFileSync(
      toml,
      fs
        .readFileSync(toml, "utf8")
        .replace("CONVEX_FN_PATH", path.join(repo, "crates/convex_fn"))
        .replace("[features]", '[features]\ndefault = ["zstd"]'),
    );
    fs.copyFileSync(path.join(repo, "e2e/spec/rust-toolchain.toml"), path.join(crate, "rust-toolchain.toml"));
    fs.copyFileSync(path.join(repo, "e2e/spec/convex/rust/Cargo.lock"), path.join(crate, "Cargo.lock"));
    const prevTarget = process.env.CARGO_TARGET_DIR;
    process.env.CARGO_TARGET_DIR ??= path.join(repo, "target-e2e");
    try {
      const logs: string[] = [];
      await buildOnce({ cwd: proj, profile: "release", wasiSdk: "auto", log: (m) => logs.push(m) });
      expect(logs.join("\n")).toMatch(/using wasi-sdk at /);
      const dir = path.join(crate, "_wasm");
      const wasmFile = fs.readdirSync(dir).find((f) => f.endsWith(".wasm"))!;
      const bytes = fs.readFileSync(path.join(dir, wasmFile));
      // No name section in the deployed wasm; the symbols are kept elsewhere.
      expect(parseNameSection(bytes).size).toBe(0);
      const line = logs.find((l) => l.includes("convex-rust symbolize"))!;
      const symDir = line.slice(line.lastIndexOf(": ") + 2);
      expect(symDir.startsWith(proj)).toBe(false);
      const maps = readSymbolMaps(symDir);
      expect(Object.values(maps[0].names).some((n) => n.includes("zstd"))).toBe(true);
      const ctx: any = {};
      const out = await runRust(ctx, new WebAssembly.Module(bytes), "convex_run:c_deps_fixture:zstd_roundtrip", { text: "hello hello hello hello" });
      expect(out).toMatch(/^23 -> \d+ bytes -> hello hello hello hello$/);
    } finally {
      if (prevTarget === undefined) delete process.env.CARGO_TARGET_DIR;
      else process.env.CARGO_TARGET_DIR = prevTarget;
      fs.rmSync(proj, { recursive: true, force: true });
    }
  }, 900_000);
});

describe("toolchain onboarding helpers", async () => {
  const { offerToolchainFix, doctorReport, wasmOptInstallHint } = await import("../src/cli/toolchain.js");
  const noTarget = {
    ok: false as const,
    kind: "no-target" as const,
    message: "The wasm32-wasip1 Rust target is not installed. Run:\n  rustup target add wasm32-wasip1",
    fix: ["rustup", "target", "add", "wasm32-wasip1"],
  };
  const exec = (map: Record<string, { status: number; stdout?: string }>) => (cmd: string, args: string[]) => {
    const r = map[`${cmd} ${args.join(" ")}`] ?? map[cmd] ?? { status: 1 };
    return { status: r.status, stdout: r.stdout ?? "", stderr: "" };
  };

  it("never installs without a TTY", async () => {
    let ran = false;
    const r = await offerToolchainFix("/x", noTarget, { interactive: false, exec: () => ((ran = true), { status: 0, stdout: "", stderr: "" }) });
    expect(r).toBe(noTarget);
    expect(ran).toBe(false);
  });

  it("runs the fix after a yes and re-checks; a no leaves the failure", async () => {
    const calls: string[] = [];
    const ok = (cmd: string, args: string[]) => {
      calls.push(`${cmd} ${args.join(" ")}`);
      return { status: 0, stdout: "wasm32-wasip1\n", stderr: "" };
    };
    expect((await offerToolchainFix("/x", noTarget, { interactive: true, ask: async () => "", exec: ok })).ok).toBe(true);
    expect(calls[0]).toBe("rustup target add wasm32-wasip1");
    calls.length = 0;
    const declined = await offerToolchainFix("/x", noTarget, { interactive: true, ask: async () => "n", exec: ok });
    expect(declined.ok).toBe(false);
    expect(calls).toEqual([]);
  });

  it("doctor reports missing required tools and treats wasm-opt as optional", () => {
    const rep = doctorReport(null, exec({ cargo: { status: 0, stdout: "cargo 1.88.0\n" }, rustup: { status: 0, stdout: "rustup 1.28.2 (abc)\n" } }));
    expect(rep.ok).toBe(true);
    expect(rep.lines.find((l) => l.text.startsWith("wasm-opt"))?.optional).toBe(true);
    const bad = doctorReport(null, exec({}));
    expect(bad.ok).toBe(false);
    expect(bad.lines[0].text).toMatch(/rustup\.rs/);
    expect(wasmOptInstallHint("darwin")).toBe("brew install binaryen");
  });
});

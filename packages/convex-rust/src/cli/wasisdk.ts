/**
 * wasi-sdk discovery / install, so crates that compile C (zstd-sys, libsqlite3-sys, ...) build for
 * wasm32-wasip1. cc-rs reads `CC_wasm32_wasip1`, `AR_wasm32_wasip1` and `CFLAGS_wasm32_wasip1`
 * (https://docs.rs/cc/latest/cc/#external-configuration-via-environment-variables).
 */
import path from "node:path";
import fs from "node:fs";
import os from "node:os";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { RUST_TARGET } from "./toolchain.js";

export const WASI_SDK_VERSION = "34.0";
const RELEASE_BASE = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-34";

/** Pinned release archives (sha256 as published by GitHub for the release assets). */
export const WASI_SDK_ASSETS: Record<string, { file: string; sha256: string; size: number }> = {
  "darwin-arm64": {
    file: "wasi-sdk-34.0-arm64-macos.tar.gz",
    sha256: "9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9",
    size: 180322002,
  },
  "darwin-x64": {
    file: "wasi-sdk-34.0-x86_64-macos.tar.gz",
    sha256: "87d27fa8adc68dee59bfbf2e22a6d34ef717c34d6bf1d8af2a56fc929d9ce0eb",
    size: 182668326,
  },
  "linux-arm64": {
    file: "wasi-sdk-34.0-arm64-linux.tar.gz",
    sha256: "f7e243dff54d60bcc576e94d6166b69f410f2500ae4a9ceef34315be10e77971",
    size: 192255247,
  },
  "linux-x64": {
    file: "wasi-sdk-34.0-x86_64-linux.tar.gz",
    sha256: "b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4",
    size: 192383077,
  },
  "win32-x64": {
    file: "wasi-sdk-34.0-x86_64-windows.tar.gz",
    sha256: "cccb5c323a9b34f0349a9b09e8804a0a7632c68c3310f4b5f437ed57d7e71d8f",
    size: 619003408,
  },
  "win32-arm64": {
    file: "wasi-sdk-34.0-arm64-windows.tar.gz",
    sha256: "45e1c71f3e965621e7b98ebe1d37b0e4b1f77f3e8072113ffb4534e67b1a4b7c",
    size: 620307231,
  },
};

type Env = Record<string, string | undefined>;

/** `~/.cache/convex-rust` (override: `CONVEX_RUST_CACHE_DIR`; honours `XDG_CACHE_HOME`). */
export function cacheRoot(env: Env = process.env): string {
  if (env.CONVEX_RUST_CACHE_DIR) return env.CONVEX_RUST_CACHE_DIR;
  const base = env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache");
  return path.join(base, "convex-rust");
}

export function cachedSdkDir(env: Env = process.env): string {
  return path.join(cacheRoot(env), `wasi-sdk-${WASI_SDK_VERSION}`);
}

const exe = process.platform === "win32" ? ".exe" : "";

/** True when `dir` looks like a wasi-sdk (clang + sysroot). */
export function isWasiSdk(dir: string): boolean {
  return (
    fs.existsSync(path.join(dir, "bin", `clang${exe}`)) &&
    fs.existsSync(path.join(dir, "share", "wasi-sysroot"))
  );
}

export type SdkSource = "env" | "cache";

/** `WASI_SDK_PATH` first, then the cache dir. Null when neither holds a usable SDK. */
export function findWasiSdk(env: Env = process.env): { dir: string; source: SdkSource } | null {
  if (env.WASI_SDK_PATH && isWasiSdk(env.WASI_SDK_PATH)) return { dir: env.WASI_SDK_PATH, source: "env" };
  const c = cachedSdkDir(env);
  if (isWasiSdk(c)) return { dir: c, source: "cache" };
  return null;
}

/**
 * Environment that points cc-rs at the SDK. Variables the user already set are left alone.
 * `clang` is used as `ar` fallback only when `llvm-ar` is missing.
 */
export function wasiSdkEnv(sdkDir: string, env: Env = process.env): Record<string, string> {
  const t = RUST_TARGET.replace(/-/g, "_");
  const out: Record<string, string> = {};
  const bin = (n: string) => path.join(sdkDir, "bin", n + exe);
  const sysroot = path.join(sdkDir, "share", "wasi-sysroot");
  const set = (k: string, v: string) => {
    if (!env[k] && !env[k.replace(t, RUST_TARGET)]) out[k] = v;
  };
  set(`CC_${t}`, bin("clang"));
  set(`CXX_${t}`, bin("clang++"));
  set(`AR_${t}`, bin("llvm-ar"));
  set(`CFLAGS_${t}`, `--sysroot=${sysroot}`);
  set(`CXXFLAGS_${t}`, `--sysroot=${sysroot}`);
  return out;
}

/** Subset of `cargo metadata` used to spot C build scripts. */
export interface MetadataLite {
  packages: { id: string; name: string }[];
  resolve?: {
    nodes: {
      id: string;
      deps: { name: string; pkg: string; dep_kinds?: { kind: string | null }[] }[];
    }[];
  } | null;
}

/**
 * Crates in the resolved (target-filtered) graph whose build script uses a C compiler driver
 * (`cc`, `cmake`, `bindgen` is not counted). Their names, sorted.
 */
export function crateNamesWithCBuild(meta: MetadataLite): string[] {
  const name = new Map(meta.packages.map((p) => [p.id, p.name]));
  const out = new Set<string>();
  for (const node of meta.resolve?.nodes ?? []) {
    for (const d of node.deps) {
      const dep = name.get(d.pkg);
      if (dep !== "cc" && dep !== "cmake") continue;
      if (!(d.dep_kinds ?? []).some((k) => k.kind === "build")) continue;
      const n = name.get(node.id);
      if (n) out.add(n);
    }
  }
  return [...out].sort();
}

export function assetForPlatform(
  platform: string = process.platform,
  arch: string = process.arch,
): { file: string; sha256: string; size: number } | null {
  return WASI_SDK_ASSETS[`${platform}-${arch}`] ?? null;
}

async function sha256File(file: string): Promise<string> {
  const h = createHash("sha256");
  await pipeline(fs.createReadStream(file), h);
  return h.digest("hex");
}

export interface InstallOptions {
  env?: Env;
  log?: (m: string) => void;
  /** Test hook: replaces `fetch`. */
  fetchImpl?: typeof fetch;
}

/**
 * Downloads the pinned wasi-sdk into the cache dir (sha256 checked) and returns its directory.
 * An archive already present in `<cache>/dl` with the right hash is reused.
 */
export async function installWasiSdk(opts: InstallOptions = {}): Promise<string> {
  const env = opts.env ?? process.env;
  const log = opts.log ?? ((m: string) => console.error(m));
  const dest = cachedSdkDir(env);
  if (isWasiSdk(dest)) return dest;
  const asset = assetForPlatform();
  if (!asset) {
    throw new Error(
      `No pinned wasi-sdk build for ${process.platform}/${process.arch}. Download one from https://github.com/WebAssembly/wasi-sdk/releases and set WASI_SDK_PATH.`,
    );
  }
  const dlDir = path.join(cacheRoot(env), "dl");
  fs.mkdirSync(dlDir, { recursive: true });
  const archive = path.join(dlDir, asset.file);
  let have = fs.existsSync(archive) && (await sha256File(archive)) === asset.sha256;
  if (!have) {
    const url = `${RELEASE_BASE}/${asset.file}`;
    log(`Downloading wasi-sdk ${WASI_SDK_VERSION} (${(asset.size / 1024 / 1024).toFixed(0)} MB) from ${url} ...`);
    const res = await (opts.fetchImpl ?? fetch)(url);
    if (!res.ok || !res.body) throw new Error(`download failed: HTTP ${res.status} for ${url}`);
    const tmp = `${archive}.${process.pid}.part`;
    try {
      await pipeline(Readable.fromWeb(res.body as any), fs.createWriteStream(tmp));
      const got = await sha256File(tmp);
      if (got !== asset.sha256) throw new Error(`checksum mismatch for ${asset.file}: expected ${asset.sha256}, got ${got}`);
      fs.renameSync(tmp, archive);
    } finally {
      fs.rmSync(tmp, { force: true });
    }
    have = true;
  }
  const stage = fs.mkdtempSync(path.join(cacheRoot(env), ".wasi-sdk-"));
  try {
    log(`Extracting wasi-sdk to ${dest} ...`);
    const r = spawnSync("tar", ["-xzf", archive, "-C", stage], { encoding: "utf8" });
    if (r.status !== 0) throw new Error(`tar failed: ${r.stderr || r.error?.message || r.status}`);
    const top = fs.readdirSync(stage).find((n) => n.startsWith("wasi-sdk"));
    if (!top || !isWasiSdk(path.join(stage, top))) throw new Error("the archive does not contain a wasi-sdk");
    fs.rmSync(dest, { recursive: true, force: true });
    fs.renameSync(path.join(stage, top), dest);
  } finally {
    fs.rmSync(stage, { recursive: true, force: true });
  }
  fs.rmSync(archive, { force: true });
  return dest;
}

export type WasiSdkMode = "auto" | "always" | "never";

/** `--wasi-sdk` / `--no-wasi-sdk` / `CONVEX_RUST_WASI_SDK=1|0`. Default: auto (download only if a C build is detected). */
export function wasiSdkMode(flags: { wasiSdk?: boolean; noWasiSdk?: boolean }, env: Env = process.env): WasiSdkMode {
  if (flags.noWasiSdk || env.CONVEX_RUST_WASI_SDK === "0") return "never";
  if (flags.wasiSdk || env.CONVEX_RUST_WASI_SDK === "1") return "always";
  return "auto";
}

/**
 * Decides the cargo environment for C compilation. Uses an SDK that is already there (always, it is free);
 * downloads one only when `mode` is "always", or "auto" and `cNames` (crates with a C build script) is non-empty.
 */
export async function prepareWasiSdk(args: {
  mode: WasiSdkMode;
  cNames: () => Promise<string[]> | string[];
  env?: Env;
  log: (m: string) => void;
  install?: typeof installWasiSdk;
}): Promise<Record<string, string>> {
  const env = args.env ?? process.env;
  if (args.mode === "never") return {};
  let sdk = findWasiSdk(env);
  if (!sdk) {
    if (args.mode === "auto") {
      let names: string[] = [];
      try {
        names = await args.cNames();
      } catch {
        // detection is best effort
      }
      if (names.length === 0) return {};
      args.log(`C build scripts detected (${names.join(", ")}) and no wasi-sdk found; installing wasi-sdk ${WASI_SDK_VERSION} (disable with --no-wasi-sdk).`);
    }
    try {
      sdk = { dir: await (args.install ?? installWasiSdk)({ env, log: args.log }), source: "cache" };
    } catch (e: any) {
      args.log(`warning: could not install wasi-sdk: ${e?.message ?? e}`);
      return {};
    }
  }
  const out = wasiSdkEnv(sdk.dir, env);
  if (Object.keys(out).length > 0) args.log(`using wasi-sdk at ${sdk.dir} for C dependencies`);
  return out;
}

import path from "node:path";
import fs from "node:fs";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { GENERATED_HEADER, generateFiles } from "./generate.js";
import { convexModuleName, parseManifest, RustManifestEntry } from "./manifest.js";
import { RUST_TARGET, checkRustToolchain, offerToolchainFix, wasmOptInstallHint } from "./toolchain.js";
import { describeWasmSize, formatBytes } from "./size.js";
import { splitWasmByModule } from "./split.js";
import { updateApiRs, readRustCache, writeRustCache } from "./rustApi.js";
import { syncManagedPatch, usesReqwest } from "./shim.js";
import { cargoTreeArgs, diagnoseCargoFailure, formatFindings } from "./diagnostics.js";
import { WasiSdkMode, crateNamesWithCBuild, prepareWasiSdk } from "./wasisdk.js";
import { formatWasmHints, wasmHints } from "./wasmHints.js";
import { SymbolMap, buildSymbolMap, parseFunctionMap, parseNameSection, symbolsDir, writeSymbolMaps } from "./symbols.js";

export type Profile = "dev" | "release";

export interface BuildOptions {
  /** Project root (default: cwd). */
  cwd: string;
  /** Crate directory (default `<functions>/rust`). */
  crate?: string;
  /** Output directory (default `<functions>/rust`). */
  out?: string;
  profile: Profile;
  /** Env var names the guest may read (emitted into the wrappers). */
  env?: string[];
  /** Compile with `-C target-feature=+simd128` (default true; `--no-simd` / `CONVEX_RUST_SIMD=0` opt out). */
  simd?: boolean;
  /**
   * One wasm per Convex module (`--split` / `CONVEX_RUST_SPLIT=1`): wasm-opt drops the code only the other
   * modules need, so each action loads (and the platform compiles, on every call) less. Needs wasm-opt.
   */
  split?: boolean;
  /**
   * Module groups (`--group name=modA,modB`): the crate is built once per group with
   * `--no-default-features --features <prefix><module>,...` (see {@link GROUP_FEATURE_PREFIX}), so a group's
   * wasm contains only its modules' functions and dependencies. Each module imports its group's wasm.
   */
  groups?: ModuleGroup[];
  /** Cargo feature name prefix for the group builds (default `m-`). */
  groupFeaturePrefix?: string;
  /**
   * Default true: with no explicit groups, build one wasm per Convex module: through the crate's `m-<module>`
   * features when it defines two or more (see {@link modulesFromFeatures}), else zero-config through the
   * `#[action]` macro's `convex_split` cfg (see {@link splitRustcArgs}). `false` (`--single` / `CONVEX_RUST_MODULES=0`)
   * builds one shared wasm with the default features. The zero-config split (no `m-` features) is skipped in the
   * dev profile (`convex-rust dev`: ~8x slower rebuilds) unless this is `true` (`--modules` / `CONVEX_RUST_MODULES=1`).
   */
  perModule?: boolean;
  /**
   * C compiler for crates that build C code: "auto" (default) uses a wasi-sdk found via `WASI_SDK_PATH` or the cache dir
   * and downloads one only when a dependency has a C build script; "always" downloads it; "never" leaves cc-rs alone.
   */
  wasiSdk?: WasiSdkMode;
  /** Total wasm size above which the smallest zero-config modules are merged (default {@link WASM_BUDGET_BYTES}). */
  wasmBudget?: number;
  log?: (msg: string) => void;
}

export class BuildError extends Error {}

export interface ModuleGroup {
  name: string;
  /** Convex module names (`cpu`, `a/b`). */
  modules: string[];
}

export const GROUP_FEATURE_PREFIX = "m-";

/** Parses `--group name=modA,modB` values. Throws {@link BuildError} on a malformed or overlapping spec. */
export function parseGroups(specs: string[]): ModuleGroup[] {
  const groups: ModuleGroup[] = [];
  const seenModule = new Map<string, string>();
  for (const spec of specs) {
    const m = /^([A-Za-z0-9_-]+)=(.+)$/.exec(spec.trim());
    if (!m) throw new BuildError(`--group expects name=module,module (got "${spec}").`);
    const modules = m[2].split(",").map((x) => x.trim()).filter(Boolean);
    if (modules.length === 0) throw new BuildError(`--group ${m[1]} lists no modules.`);
    if (groups.some((g) => g.name === m[1])) throw new BuildError(`--group ${m[1]} given twice.`);
    for (const mod of modules) {
      const prev = seenModule.get(mod);
      if (prev) throw new BuildError(`module ${mod} is in both group ${prev} and group ${m[1]}.`);
      seenModule.set(mod, m[1]);
    }
    groups.push({ name: m[1], modules });
  }
  return groups;
}

/**
 * Convex module names for the crate's `<prefix><module>` features (`m-a-b` -> `a/b`: Rust module names have
 * no dashes, so a dash is a path separator). Sorted; empty when the crate has fewer than two such features
 * (one feature gates nothing worth splitting).
 */
export function modulesFromFeatures(features: string[], prefix = GROUP_FEATURE_PREFIX): string[] {
  const mods = features
    .filter((f) => f.startsWith(prefix) && f.length > prefix.length)
    .map((f) => f.slice(prefix.length).replace(/-/g, "/"))
    .sort();
  return mods.length >= 2 ? mods : [];
}

/** File-name stem of a module's wasm: the module with `/` as `__`. */
export function moduleFileName(module: string): string {
  return module.replace(/\//g, "__");
}

/** Rejects modules whose wasm file names collide (`a/b` and a real module `a__b` would overwrite each other). */
export function assertDistinctFileNames(modules: Iterable<string>): void {
  const seen = new Map<string, string>();
  for (const m of modules) {
    const f = moduleFileName(m);
    const prev = seen.get(f);
    if (prev !== undefined && prev !== m) {
      throw new BuildError(`Rust modules \`${prev}\` and \`${m}\` both map to the wasm file name \`${f}\`; rename one of them.`);
    }
    seen.set(f, m);
  }
}

/** One group per module; the group (file) name is the module with `/` as `__`. */
export function perModuleGroups(modules: string[]): ModuleGroup[] {
  assertDistinctFileNames(modules);
  return modules.map((m) => ({ name: moduleFileName(m), modules: [m] }));
}

/** Cargo feature args that build only a group's modules. Module `a/b` maps to feature `<prefix>a-b`. */
export function groupFeatureArgs(group: ModuleGroup, prefix = GROUP_FEATURE_PREFIX): string[] {
  return ["--no-default-features", "--features", group.modules.map((m) => prefix + m.replace(/\//g, "-")).join(",")];
}

/** Wasm size budget for all per-module wasm together: Convex bundles at most 32 MiB of code per deployment. */
export const WASM_BUDGET_BYTES = 28 * 1024 * 1024;

/**
 * Zero-config per-module build: the extra `rustc` arguments (after `cargo rustc ... --`) that make the
 * `#[action]` macro keep only the exports of `fns` (see `crates/convex_fn_macros`). `--check-cfg` keeps
 * the `unexpected_cfgs` lint quiet for crates that do not otherwise declare these cfgs.
 */
export function splitRustcArgs(fns: string[]): string[] {
  const out = ["--check-cfg", "cfg(convex_split)", "--check-cfg", "cfg(convex_keep, values(any()))", "--cfg", "convex_split"];
  for (const f of [...new Set(fns)].sort()) out.push("--cfg", `convex_keep="${f}"`);
  return out;
}

/** The two smallest entries of `sizes` (the next merge candidates when over the size budget), or undefined. */
export function smallestTwo(sizes: Map<string, number>): [string, string] | undefined {
  if (sizes.size < 2) return undefined;
  const [a, b] = [...sizes].sort((x, y) => x[1] - y[1]);
  return [a[0], b[0]];
}

/** Convex functions directory: `functions` in convex.json, default `convex`. */
export function resolveFunctionsDir(cwd: string): string {
  try {
    const cfg = JSON.parse(fs.readFileSync(path.join(cwd, "convex.json"), "utf8"));
    if (typeof cfg.functions === "string" && cfg.functions) {
      return path.resolve(cwd, cfg.functions);
    }
  } catch {
    // no convex.json
  }
  return path.resolve(cwd, "convex");
}

export function cargoProfileDir(profile: Profile): string {
  return profile === "dev" ? "convex-dev" : "release";
}

/** Cargo args and env for a profile; the dev profile is defined via env. */
export function cargoBuildConfig(profile: Profile): { args: string[]; env: Record<string, string> } {
  if (profile === "release") return { args: ["--release"], env: {} };
  return {
    args: ["--profile", "convex-dev"],
    env: {
      CARGO_PROFILE_CONVEX_DEV_INHERITS: "release",
      CARGO_PROFILE_CONVEX_DEV_OPT_LEVEL: "1",
      CARGO_PROFILE_CONVEX_DEV_INCREMENTAL: "true",
      CARGO_PROFILE_CONVEX_DEV_LTO: "false",
      CARGO_PROFILE_CONVEX_DEV_CODEGEN_UNITS: "16",
      CARGO_PROFILE_CONVEX_DEV_DEBUG: "false",
    },
  };
}

/** Rust flag pair that turns on wasm SIMD. */
export const SIMD_RUSTFLAG = "target-feature=+simd128";

/**
 * Extra cargo args / env for the SIMD option. With RUSTFLAGS (or CARGO_ENCODED_RUSTFLAGS) set the flag is
 * appended to it; otherwise it goes in as `--config target.<triple>.rustflags`, which cargo joins with the
 * rustflags of every `.cargo/config.toml` `target.<triple>.rustflags` (a config `build.rustflags` is then
 * shadowed, as for any target-specific rustflags).
 */
export function simdCargoConfig(
  simd: boolean,
  env: Record<string, string | undefined> = process.env,
): { args: string[]; env: Record<string, string> } {
  if (!simd) return { args: [], env: {} };
  if (env.CARGO_ENCODED_RUSTFLAGS) {
    return { args: [], env: { CARGO_ENCODED_RUSTFLAGS: `${env.CARGO_ENCODED_RUSTFLAGS}\x1f-C\x1f${SIMD_RUSTFLAG}` } };
  }
  if (env.RUSTFLAGS) return { args: [], env: { RUSTFLAGS: `${env.RUSTFLAGS} -C ${SIMD_RUSTFLAG}` } };
  return { args: ["--config", `target.${RUST_TARGET}.rustflags=["-C","${SIMD_RUSTFLAG}"]`], env: {} };
}

interface CargoResult {
  status: number | null;
  error?: NodeJS.ErrnoException | undefined;
  stdout: string;
  stderr: string;
}

function runCargo(
  cwd: string,
  args: string[],
  inherit: boolean,
  env: Record<string, string> = {},
): Promise<CargoResult> {
  return new Promise((resolve) => {
    const child = spawn("cargo", args, {
      cwd,
      env: { ...process.env, ...env },
      stdio: ["ignore", inherit ? "inherit" : "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let error: NodeJS.ErrnoException | undefined;
    child.stdout?.setEncoding("utf8");
    child.stdout?.on("data", (d: string) => (stdout += d));
    child.stderr?.setEncoding("utf8");
    child.stderr?.on("data", (d: string) => {
      process.stderr.write(d);
      if (stderr.length < 8 * 1024 * 1024) stderr += d;
    });
    child.on("error", (e) => (error = e));
    child.on("close", (status) => resolve({ status, error, stdout, stderr }));
  });
}

/** Cargo target directory and library name of the crate, via `cargo metadata`. */
export async function findCrateLayout(
  crateDir: string,
): Promise<{ targetDir: string; libName: string; features: string[] }> {
  const manifestPath = path.join(crateDir, "Cargo.toml");
  const r = await runCargo(
    crateDir,
    ["metadata", "--format-version", "1", "--no-deps", "--manifest-path", manifestPath],
    false,
  );
  if (r.error || r.status !== 0) {
    throw new BuildError(`\`cargo metadata\` failed for ${manifestPath}.`);
  }
  const meta = JSON.parse(r.stdout);
  const manifestAbs = path.resolve(manifestPath);
  const pkg =
    meta.packages.find((p: any) => path.resolve(p.manifest_path) === manifestAbs) ??
    meta.packages[0];
  const lib =
    pkg.targets.find((t: any) => t.kind.includes("cdylib")) ??
    pkg.targets.find((t: any) => t.kind.includes("lib")) ??
    pkg.targets[0];
  return {
    targetDir: meta.target_directory,
    libName: String(lib?.name ?? pkg.name).replace(/-/g, "_"),
    features: Object.keys(pkg.features ?? {}),
  };
}

/** Crates of the wasm32-wasip1 dependency graph whose build script drives a C compiler (`cc` / `cmake`). */
export async function cBuildCrates(crateDir: string): Promise<string[]> {
  const r = await runCargo(
    crateDir,
    ["metadata", "--format-version", "1", "--filter-platform", RUST_TARGET, "--manifest-path", path.join(crateDir, "Cargo.toml")],
    false,
  );
  if (r.error || r.status !== 0) return [];
  return crateNamesWithCBuild(JSON.parse(r.stdout));
}

const WASM_OPT_FEATURES = [
  "--enable-bulk-memory",
  "--enable-sign-ext",
  "--enable-mutable-globals",
  "--enable-nontrapping-float-to-int",
  "--enable-multivalue",
];
// Speed over size: -O3, no -g (the name section is dropped; panic messages keep their file:line, which
// comes from the Location data in .rodata). CONVEX_RUST_WASM_OPT_FLAGS (space separated) replaces
// the optimization flags, e.g. "-Oz -g" for the old behaviour.
const WASM_OPT_FEATURES_SIMD = ["--enable-simd"];
export function wasmOptFlags(): string[] {
  const o = process.env.CONVEX_RUST_WASM_OPT_FLAGS?.trim();
  const opt = o ? o.split(/\s+/) : ["-O3"];
  return [...opt, ...WASM_OPT_FEATURES, ...WASM_OPT_FEATURES_SIMD];
}

function wasmOptVersion(): string | null {
  const r = spawnSync("wasm-opt", ["--version"], { encoding: "utf8" });
  return r.status === 0 ? r.stdout.trim() : null;
}

/**
 * `wasm-opt -O3` when available, with a content-addressed cache in
 * `cacheDir`. Returns the bytes to ship (the input when wasm-opt is missing,
 * fails, does not shrink the module or drops the manifest). `onNames` receives the function names
 * (index in the returned module -> raw name) for the symbol map.
 */
export function optimizeWasm(
  input: Uint8Array,
  cacheDir: string,
  log: (m: string) => void = () => {},
  onNames: (names: Map<number, string>) => void = () => {},
): Uint8Array {
  const version = wasmOptVersion();
  if (version === null) {
    log(`wasm-opt not found on PATH; shipping the unoptimized wasm (smaller with binaryen: ${wasmOptInstallHint()})`);
    return input;
  }
  const key = createHash("sha256")
    .update(version + "\0" + wasmOptFlags().join(" "))
    .update(input)
    .digest("hex");
  const cached = path.join(cacheDir, `${key}.wasm`);
  const cachedMap = path.join(cacheDir, `${key}.fnmap`);
  if (fs.existsSync(cached) && fs.existsSync(cachedMap)) {
    log("wasm-opt: cache hit");
    onNames(parseFunctionMap(fs.readFileSync(cachedMap, "utf8")));
    return new Uint8Array(fs.readFileSync(cached));
  }
  fs.mkdirSync(cacheDir, { recursive: true });
  const src = path.join(cacheDir, `${key}.${process.pid}.in`);
  const dst = path.join(cacheDir, `${key}.${process.pid}.tmp`);
  fs.writeFileSync(src, input);
  try {
    // --print-function-map last: it lists the function indexes of the optimized module (wasm-opt renumbers them).
    const r = spawnSync("wasm-opt", [...wasmOptFlags(), "--print-function-map", src, "-o", dst], {
      stdio: ["ignore", "pipe", "pipe"],
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
    });
    if (r.status !== 0) {
      log(`wasm-opt failed (${r.status}); using the unoptimized wasm`);
      return input;
    }
    const after = new Uint8Array(fs.readFileSync(dst));
    if (after.length >= input.length || parseManifest(after).length !== parseManifest(input).length) {
      log("wasm-opt output not smaller or lost the manifest; ignoring");
      return input;
    }
    fs.writeFileSync(cachedMap, r.stdout);
    fs.renameSync(dst, cached);
    onNames(parseFunctionMap(r.stdout));
    log(`wasm-opt: ${input.length} -> ${after.length} bytes`);
    return after;
  } finally {
    fs.rmSync(src, { force: true });
    fs.rmSync(dst, { force: true });
  }
}

async function buildFailureHints(crateDir: string, stderr: string): Promise<string> {
  try {
    const findings = diagnoseCargoFailure(stderr);
    const trees: Record<string, string> = {};
    for (const crate of new Set(findings.map((f) => f.crate))) {
      if (!crate || Object.keys(trees).length >= 3) continue;
      const r = await runCargo(crateDir, [...cargoTreeArgs(crate)], false);
      if (r.status === 0) trees[crate] = r.stdout;
    }
    const text = formatFindings(findings, trees);
    return text === "" ? "" : "\n" + text;
  } catch {
    return "";
  }
}

/** Resolve Cargo.lock (so a transitive reqwest is seen) and sync the managed reqwest patch. */
async function prepareCrate(crateDir: string, log: (m: string) => void): Promise<void> {
  if (!fs.existsSync(path.join(crateDir, "Cargo.lock"))) {
    await runCargo(crateDir, ["metadata", "--format-version", "1", "--manifest-path", path.join(crateDir, "Cargo.toml")], false);
  }
  try {
    if (syncManagedPatch(crateDir)) {
      log("updated the managed [patch] block in .cargo/config.toml");
      // A Cargo.lock written before the patch pins the registry reqwest; re-resolve so the patch applies.
      if (usesReqwest(crateDir)) {
        await runCargo(crateDir, ["update", "--manifest-path", path.join(crateDir, "Cargo.toml"), "reqwest"], false);
      }
    }
  } catch (e: any) {
    log(`warning: could not update .cargo/config.toml: ${e?.message ?? e}`);
  }
}

/** The stock bundler walks every file of the functions directory; cargo output there is a hazard. */
export function warnIfTargetInsideFunctions(targetDir: string, functionsDir: string, log: (m: string) => void): void {
  const rel = path.relative(functionsDir, targetDir);
  if (!rel.startsWith("..") && !path.isAbsolute(rel)) {
    log(
      `warning: cargo's target directory (${targetDir}) is inside the Convex functions directory, where \`convex dev\` ` +
        `would walk it and may bundle stray .js files. Set \`[build] target-dir\` in ${"`.cargo/config.toml`"} to a path outside it ` +
        "(`convex-rust init` does this for new crates).",
    );
  }
}

function writeIfChanged(file: string, data: string | Uint8Array): boolean {
  try {
    const cur = fs.readFileSync(file);
    if (Buffer.compare(cur, Buffer.from(data as any)) === 0) return false;
  } catch {
    // new file
  }
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data);
  return true;
}

const SKIP_DIRS = new Set(["node_modules", "target", "src", "_wasm", ".cargo", ".git"]);

/** Previously generated `.ts` files below `dir` (those starting with the header). */
function findGenerated(dir: string, base = dir, out: string[] = []): string[] {
  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const e of entries) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (!SKIP_DIRS.has(e.name)) findGenerated(p, base, out);
    } else if (e.name.endsWith(".ts")) {
      try {
        if (fs.readFileSync(p, "utf8").startsWith(GENERATED_HEADER)) {
          out.push(path.relative(base, p).split(path.sep).join("/"));
        }
      } catch {
        // unreadable
      }
    }
  }
  return out;
}

export interface WriteResult {
  written: string[];
  removed: string[];
}

/** Writes the generated files and the wasm; removes stale generated files. */
export function writeOutput(
  outDir: string,
  wasmFile: string,
  wasm: Uint8Array,
  entries: RustManifestEntry[],
  functionsDir: string,
  env: string[] = [],
  split?: Map<string, Uint8Array>,
  grouped?: { wasm: Map<string, Uint8Array>; moduleGroup: Map<string, string> },
): WriteResult {
  const rel = path.relative(functionsDir, outDir).split(path.sep).join("/");
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    throw new BuildError(
      `--out ${outDir} must be inside the Convex functions directory (${functionsDir}).`,
    );
  }
  const wasmHash = createHash("sha256").update(wasm).digest("hex").slice(0, 16);
  const moduleWasm = new Map<string, { file: string; hash: string }>();
  const wasmFiles = new Map<string, Uint8Array>([[wasmFile, wasm]]);
  if (split) {
    const stem = wasmFile.replace(/\.wasm$/, "");
    wasmFiles.clear();
    assertDistinctFileNames(split.keys());
    for (const [module, bytes] of split) {
      const file = `${stem}.${moduleFileName(module)}.wasm`;
      moduleWasm.set(module, { file, hash: createHash("sha256").update(bytes).digest("hex").slice(0, 16) });
      wasmFiles.set(file, bytes);
    }
  }
  if (grouped) {
    const stem = wasmFile.replace(/\.wasm$/, "");
    wasmFiles.clear();
    const info = new Map<string, { file: string; hash: string }>();
    for (const [group, bytes] of grouped.wasm) {
      const file = `${stem}.${group}.wasm`;
      info.set(group, { file, hash: createHash("sha256").update(bytes).digest("hex").slice(0, 16) });
      wasmFiles.set(file, bytes);
    }
    for (const [module, group] of grouped.moduleGroup) moduleWasm.set(module, info.get(group)!);
  }
  const files = generateFiles({ entries, wasmFile, outRelToFunctions: rel === "" ? "." : rel, wasmHash, env, moduleWasm });
  const written: string[] = [];
  for (const [name, text] of files) {
    if (writeIfChanged(path.join(outDir, name), text)) written.push(name);
  }
  for (const [file, bytes] of wasmFiles) {
    if (writeIfChanged(path.join(outDir, "_wasm", file), bytes)) written.push(`_wasm/${file}`);
  }
  const removed: string[] = [];
  try {
    // Stale wasm (a previous build, or the other of split / shared): the bundler only ships imported ones, but keep the dir tidy.
    for (const f of fs.readdirSync(path.join(outDir, "_wasm"))) {
      if (f.endsWith(".wasm") && !wasmFiles.has(f)) {
        fs.rmSync(path.join(outDir, "_wasm", f), { force: true });
        removed.push(`_wasm/${f}`);
      }
    }
  } catch {
    // no _wasm dir yet
  }
  for (const old of findGenerated(outDir)) {
    if (!files.has(old)) {
      fs.rmSync(path.join(outDir, old), { force: true });
      removed.push(old);
    }
  }
  return { written, removed };
}

export interface BuildResult extends WriteResult {
  wasmFile: string;
  functions: string[];
  outDir: string;
}

/** Builds the crate and writes the generated files. Throws {@link BuildError}. */
export async function buildOnce(opts: BuildOptions): Promise<BuildResult> {
  const log = opts.log ?? ((m: string) => console.error(m));
  const functionsDir = resolveFunctionsDir(opts.cwd);
  const crateDir = path.resolve(opts.cwd, opts.crate ?? path.join(functionsDir, "rust"));
  const outDir = path.resolve(opts.cwd, opts.out ?? path.join(functionsDir, "rust"));
  const manifestPath = path.join(crateDir, "Cargo.toml");
  if (!fs.existsSync(manifestPath)) {
    throw new BuildError(
      `No Rust crate found: ${manifestPath} does not exist. Run \`npx convex-rust init\` to create one.`,
    );
  }
  const tc = await offerToolchainFix(crateDir, checkRustToolchain(crateDir), { log });
  if (!tc.ok) throw new BuildError(`Rust toolchain check failed.\n${tc.message}`);

  const cfg = cargoBuildConfig(opts.profile);
  const simdCfg = simdCargoConfig(opts.simd ?? true);
  const layout0 = await findCrateLayout(crateDir);
  const cacheFile = path.join(layout0.targetDir, "convex-rust-modules.json");
  warnIfTargetInsideFunctions(layout0.targetDir, functionsDir, log);
  await prepareCrate(crateDir, log);
  try {
    await updateApiRs({
      projectRoot: opts.cwd, functionsDir, crateDir, outDir,
      rustEntries: readRustCache(cacheFile), log,
    });
  } catch (e: any) {
    log(`warning: could not generate src/_generated/api.rs: ${e?.message ?? e}`);
  }
  const prefix = opts.groupFeaturePrefix ?? GROUP_FEATURE_PREFIX;
  let groups = opts.groups ?? [];
  // Default: one wasm per module when the crate gates its modules with m-<module> features.
  let auto = false;
  if (groups.length === 0 && opts.perModule !== false) {
    const mods = modulesFromFeatures(layout0.features, prefix);
    if (mods.length > 0) {
      groups = perModuleGroups(mods);
      auto = true;
      log(`one wasm per module (${mods.length} ${prefix}<module> features); --single builds one shared wasm`);
    }
  }
  let targetDir = layout0.targetDir;
  let libName = layout0.libName;
  const sdkEnv = await prepareWasiSdk({
    mode: opts.wasiSdk ?? "auto",
    cNames: () => cBuildCrates(crateDir),
    log,
  });
  /** One cargo build; `featureArgs` select the cargo features, `rustcArgs` (zero-config split) switch to `cargo rustc`. */
  const compileWasm = async (label: string, featureArgs: string[], rustcArgs?: string[]): Promise<Uint8Array> => {
    log(`Building Rust functions${label} (${manifestPath}, ${opts.profile === "dev" ? "dev profile" : "release"})...`);
    const r = await runCargo(
      crateDir,
      [
        rustcArgs ? "rustc" : "build", ...(rustcArgs ? ["--lib"] : []), ...cfg.args, ...simdCfg.args, ...featureArgs,
        "--target", RUST_TARGET, "--manifest-path", manifestPath, ...(rustcArgs ? ["--", ...rustcArgs] : []),
      ],
      true,
      { ...cfg.env, ...simdCfg.env, ...sdkEnv },
    );
    if ((r.error as any)?.code === "ENOENT") {
      throw new BuildError(
        `\`cargo\` was not found. Install Rust from https://rustup.rs, then run:\n  rustup target add ${RUST_TARGET}`,
      );
    }
    if (r.status !== 0) {
      const hints = await buildFailureHints(crateDir, r.stderr);
      throw new BuildError(
        `Rust build failed (cargo exited with ${r.status}).\nIf the error mentions a missing target, run:\n  rustup target add ${RUST_TARGET}${hints}`,
      );
    }
    ({ targetDir, libName } = await findCrateLayout(crateDir));
    const built = path.join(targetDir, RUST_TARGET, cargoProfileDir(opts.profile), `${libName}.wasm`);
    if (!fs.existsSync(built)) {
      throw new BuildError(
        `Build succeeded but ${built} was not found. Make sure Cargo.toml has crate-type = ["cdylib"] in [lib].`,
      );
    }
    return new Uint8Array(fs.readFileSync(built));
  };
  /** wasm-opt (release) and the size report for a compiled wasm. */
  const fnNames = new Map<Uint8Array, Map<number, string>>();
  const hintsSeen = new Set<string>();
  const finishWasm = (label: string, raw: Uint8Array): Uint8Array => {
    let bytes = raw;
    let names = new Map<number, string>();
    try {
      names = parseNameSection(raw);
    } catch {
      // best effort
    }
    if (opts.profile === "release") {
      bytes = optimizeWasm(bytes, path.join(targetDir, "convex-rust-wasm-opt-cache"), (m) => log(m), (n) => (names = n));
    }
    fnNames.set(bytes, names);
    try {
      for (const msg of formatWasmHints(wasmHints(bytes), label)) {
        const key = msg.replace(/^warning: \[[^\]]*\]/, "warning:");
        if (!hintsSeen.has(key)) {
          hintsSeen.add(key);
          log(msg);
        }
      }
    } catch {
      // hints are best effort
    }
    try {
      const { message, warning } = describeWasmSize(bytes);
      log(`${label.trim() ? label.trim() + ": " : ""}${message}`);
      if (warning) log(`warning: ${warning}`);
    } catch {
      // size report is best effort
    }
    return bytes;
  };
  const buildWasm = async (label: string, featureArgs: string[]): Promise<Uint8Array> =>
    finishWasm(label, await compileWasm(label, featureArgs));
  const parse = (bytes: Uint8Array): RustManifestEntry[] => {
    try {
      return parseManifest(bytes);
    } catch (e: any) {
      throw new BuildError(`Rust build failed: ${e?.message ?? e}`);
    }
  };
  let wasm: Uint8Array = new Uint8Array();
  let entries: RustManifestEntry[] = [];
  let grouped: { wasm: Map<string, Uint8Array>; moduleGroup: Map<string, string> } | undefined;
  let fullRaw: Uint8Array | undefined;
  if (groups.length === 0 && opts.perModule !== false && (opts.profile === "release" || opts.perModule === true)) {
    // Zero-config split: build everything once to learn the modules, then once per module with only that
    // module's exports kept (cfg from the #[action] macro); the linker drops the rest.
    fullRaw = await compileWasm("", []);
    const all = parse(fullRaw);
    const byModule = new Map<string, RustManifestEntry[]>();
    for (const e of all) {
      const m = convexModuleName(e.modulePath);
      byModule.set(m, [...(byModule.get(m) ?? []), e]);
    }
    assertDistinctFileNames(byModule.keys());
    if (byModule.size >= 2) {
      const t0 = Date.now();
      log(`one wasm per module (${byModule.size} modules, no cargo features needed); --single builds one shared wasm`);
      grouped = { wasm: new Map(), moduleGroup: new Map() };
      const members = new Map<string, string[]>(); // group name -> modules
      const sizes = new Map<string, number>();
      const build = async (name: string, mods: string[]): Promise<void> => {
        const fns = mods.flatMap((m) => byModule.get(m)!.map((e) => e.name));
        const bytes = finishWasm(` [${name}]`, await compileWasm(` [${name}]`, [], splitRustcArgs(fns)));
        const got = new Set(parse(bytes).map((e) => `${convexModuleName(e.modulePath)}:${e.name}`));
        for (const m of mods) {
          for (const e of byModule.get(m)!) {
            if (!got.has(`${m}:${e.name}`)) throw new BuildError(`split build [${name}]: ${m}:${e.name} is missing from its wasm.`);
          }
        }
        grouped!.wasm.set(name, bytes);
        members.set(name, mods);
        sizes.set(name, bytes.length);
      };
      for (const m of [...byModule.keys()].sort()) await build(moduleFileName(m), [m]);
      let pair: [string, string] | undefined;
      let warned = false;
      while (
        [...sizes.values()].reduce((a, b) => a + b, 0) > (opts.wasmBudget ?? WASM_BUDGET_BYTES) &&
        (pair = smallestTwo(sizes))
      ) {
        if (!warned) {
          log(`warning: the per-module wasm total ${formatBytes([...sizes.values()].reduce((a, b) => a + b, 0))}, over the ${formatBytes(opts.wasmBudget ?? WASM_BUDGET_BYTES)} budget (Convex allows 32 MiB of bundled code); merging the smallest modules`);
          warned = true;
        }
        const [a, b] = pair;
        const mods = [...members.get(a)!, ...members.get(b)!];
        for (const n of [a, b]) {
          grouped.wasm.delete(n);
          members.delete(n);
          sizes.delete(n);
        }
        await build(`${a}_${b}`.replace(/[^A-Za-z0-9_.-]/g, "_").slice(0, 60), mods);
      }
      for (const [name, mods] of members) for (const m of mods) grouped.moduleGroup.set(m, name);
      entries = all;
      log(`built ${grouped.wasm.size} wasm in ${((Date.now() - t0) / 1000).toFixed(1)} s`);
      const total = [...grouped.wasm.values()].reduce((a, b) => a + b.length, 0);
      log(`modules: ${[...grouped.wasm].map(([n, b]) => `${n} ${formatBytes(b.length)}`).join(", ")}, ${formatBytes(total)} in total`);
      wasm = grouped.wasm.values().next().value!;
    }
  }
  if (grouped) {
    // zero-config split done
  } else if (groups.length > 0) {
    grouped = { wasm: new Map(), moduleGroup: new Map() };
    const t0 = Date.now();
    for (const g of groups) {
      const bytes = await buildWasm(` [${g.name}]`, groupFeatureArgs(g, prefix));
      let ge = parse(bytes);
      const have = new Set(ge.map((e) => convexModuleName(e.modulePath)));
      const missing = g.modules.filter((m) => !have.has(m));
      if (auto) {
        // A feature without functions (a helper) gets no wasm. Modules the features do not gate show up in
        // every build; the first build that has them keeps them.
        if (missing.length === g.modules.length) {
          log(`${g.name}: no functions; skipping its wasm`);
          continue;
        }
        ge = ge.filter((e) => {
          const m = convexModuleName(e.modulePath);
          return g.modules.includes(m) || !grouped!.moduleGroup.has(m);
        });
        for (const e of ge) {
          const m = convexModuleName(e.modulePath);
          if (!grouped.moduleGroup.has(m)) grouped.moduleGroup.set(m, g.name);
        }
      } else {
        if (missing.length) {
          throw new BuildError(
            `group ${g.name}: no functions for module(s) ${missing.join(", ")}; check the crate has features ${missing.map((m) => prefix + m.replace(/\//g, "-")).join(", ")}.`,
          );
        }
        for (const m of have) {
          if (!g.modules.includes(m)) {
            throw new BuildError(`group ${g.name}: its wasm also contains module ${m}, which is not listed in the group (feature ${prefix}${m} should gate it).`);
          }
        }
        for (const m of g.modules) grouped.moduleGroup.set(m, g.name);
      }
      grouped.wasm.set(g.name, bytes);
      entries.push(...ge);
    }
    log(`built ${grouped.wasm.size} wasm in ${((Date.now() - t0) / 1000).toFixed(1)} s`);
    const total = [...grouped.wasm.values()].reduce((a, b) => a + b.length, 0);
    log(`groups: ${[...grouped.wasm].map(([n, b]) => `${n} ${formatBytes(b.length)}`).join(", ")}, ${formatBytes(total)} in total`);
    wasm = grouped.wasm.values().next().value!;
  } else if (fullRaw) {
    wasm = finishWasm("", fullRaw);
    entries = parse(wasm);
  } else {
    wasm = await buildWasm("", []);
    entries = parse(wasm);
  }
  writeRustCache(cacheFile, entries);
  try {
    await updateApiRs({ projectRoot: opts.cwd, functionsDir, crateDir, outDir, rustEntries: entries, log });
  } catch {
    // best effort: the pre-build pass already ran
  }
  const wasmFile = `${libName}.wasm`;
  let split: Map<string, Uint8Array> | undefined;
  if (opts.split && grouped) {
    log("--split is ignored when building one wasm per module / group");
  } else if (opts.split) {
    if (opts.profile !== "release") {
      log("--split only applies to the release profile; using one shared wasm");
    } else {
      const modulePaths = [...new Set(entries.map((e) => e.modulePath))];
      const res = splitWasmByModule(wasm, modulePaths, {
        wasmOptArgs: wasmOptFlags(),
        cacheDir: path.join(targetDir, "convex-rust-wasm-opt-cache"),
        log,
      });
      if (res && res.size > 0) {
        split = res;
        const total = [...res.values()].reduce((a, b) => a + b.length, 0);
        log(
          `split: ${res.size} wasm (${[...res].map(([m, b]) => `${m} ${formatBytes(b.length)}`).join(", ")}), ${formatBytes(total)} in total`,
        );
      }
    }
  }
  let result: WriteResult;
  try {
    result = writeOutput(outDir, wasmFile, wasm, entries, functionsDir, opts.env, split, grouped);
  } catch (e: any) {
    if (e instanceof BuildError) throw e;
    throw new BuildError(`Could not generate the wrappers: ${e?.message ?? e}`);
  }
  try {
    const stem = wasmFile.replace(/\.wasm$/, "");
    const maps: SymbolMap[] = [];
    if (grouped) {
      for (const [group, bytes] of grouped.wasm) {
        const modules = [...grouped.moduleGroup].filter(([, g]) => g === group).map(([m]) => m);
        const names = fnNames.get(bytes);
        if (names?.size) maps.push(buildSymbolMap(names, { wasm: `${stem}.${group}.wasm`, modules }));
      }
    } else if (!split) {
      const names = fnNames.get(wasm);
      const modules = [...new Set(entries.map((e) => convexModuleName(e.modulePath)))];
      if (names?.size) maps.push(buildSymbolMap(names, { wasm: wasmFile, modules }));
    }
    if (maps.length > 0) {
      writeSymbolMaps(symbolsDir(targetDir), maps);
      log(`function names for \`convex-rust symbolize\` (not deployed): ${symbolsDir(targetDir)}`);
    }
  } catch (e: any) {
    log(`warning: could not write the symbol map: ${e?.message ?? e}`);
  }
  const functions = entries.map((e) => `${e.modulePath.split("::").slice(1).join("/") || "lib"}:${e.name}`);
  log(
    `convex-rust: ${functions.length} function(s) -> ${path.relative(opts.cwd, outDir) || "."} ` +
      `(${result.written.length} file(s) written${result.removed.length ? `, ${result.removed.length} removed` : ""})`,
  );
  return { ...result, wasmFile, functions, outDir };
}

const WATCH_FILES = new Set(["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", "config.toml"]);
const IGNORED_PARTS = new Set(["target", "_wasm", "node_modules", ".git", "_generated"]);

/** Should a change to this crate-relative path trigger a rebuild? */
export function isWatchedChange(relPath: string): boolean {
  const parts = relPath.split(/[\\/]/);
  if (parts.some((p) => IGNORED_PARTS.has(p))) return false;
  const base = parts[parts.length - 1];
  return base.endsWith(".rs") || WATCH_FILES.has(base);
}

/** Should a change to this functions-dir-relative path re-run the api.rs codegen? */
export function isTsSourceChange(relPath: string): boolean {
  const parts = relPath.split(/[\\/]/);
  if (parts.some((p) => IGNORED_PARTS.has(p) || p.startsWith("."))) return false;
  const base = parts[parts.length - 1];
  return /\.(ts|tsx|js|jsx|mjs|mts)$/.test(base) && !base.endsWith(".d.ts");
}

function hashFile(p: string): string | null {
  try {
    return createHash("sha1").update(fs.readFileSync(p)).digest("hex");
  } catch {
    return null;
  }
}

/** Content hashes of every watched file below `dir` (crate-relative keys). */
function snapshot(dir: string, base = dir, out = new Map<string, string>()): Map<string, string> {
  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const e of entries) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (!IGNORED_PARTS.has(e.name)) snapshot(p, base, out);
    } else {
      const rel = path.relative(base, p);
      if (isWatchedChange(rel)) {
        const h = hashFile(p);
        if (h) out.set(rel, h);
      }
    }
  }
  return out;
}

/**
 * Initial build, then rebuild on changes to the crate sources (and re-run the api.rs codegen on
 * TypeScript changes). Events are debounced, and only a real content change versus the state
 * after the last build triggers a rebuild, so the build's own writes (Cargo.lock, the managed
 * config block, api.rs) never loop. Never resolves.
 */
export async function watch(opts: BuildOptions): Promise<never> {
  const log = opts.log ?? ((m: string) => console.error(m));
  const functionsDir = resolveFunctionsDir(opts.cwd);
  const crateDir = path.resolve(opts.cwd, opts.crate ?? path.join(functionsDir, "rust"));
  const outDir = path.resolve(opts.cwd, opts.out ?? path.join(functionsDir, "rust"));
  let snap = new Map<string, string>();
  const build = async () => {
    try {
      await buildOnce(opts);
    } catch (e: any) {
      log(`error: ${e?.message ?? e}`);
    }
    snap = snapshot(crateDir);
    log("Watching for changes...");
  };
  const crateChanged = (): boolean => {
    const now = snapshot(crateDir);
    if (now.size !== snap.size) return true;
    for (const [k, h] of now) if (snap.get(k) !== h) return true;
    return false;
  };
  await build();
  let timer: NodeJS.Timeout | null = null;
  let running = false;
  let tsDirty = false;
  let pending = false;
  const cacheFile = async () =>
    path.join((await findCrateLayout(crateDir)).targetDir, "convex-rust-modules.json");
  const cycle = async () => {
    if (running) {
      pending = true;
      return;
    }
    running = true;
    try {
      do {
        pending = false;
        let rebuild = crateChanged();
        if (tsDirty) {
          tsDirty = false;
          try {
            const changed = await updateApiRs({
              projectRoot: opts.cwd, functionsDir, crateDir, outDir,
              rustEntries: readRustCache(await cacheFile()), log,
            });
            if (changed) {
              log("src/_generated/api.rs updated from the TypeScript functions");
              rebuild = true;
            }
          } catch (e: any) {
            log(`warning: codegen failed: ${e?.message ?? e}`);
          }
        }
        if (rebuild) {
          log("Change detected, rebuilding...");
          await build();
        }
      } while (pending);
    } finally {
      running = false;
    }
  };
  const trigger = () => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void cycle(), 300);
  };
  const within = (p: string, dir: string) => {
    const r = path.relative(dir, p);
    return !r.startsWith("..") && !path.isAbsolute(r);
  };
  fs.watch(crateDir, { recursive: true }, (_event, name) => {
    if (name && isWatchedChange(name.toString())) trigger();
  });
  {
    try {
      fs.watch(functionsDir, { recursive: true }, (_event, name) => {
        if (!name) return;
        const abs = path.join(functionsDir, name.toString());
        if (within(abs, crateDir) || within(abs, outDir)) return;
        if (isTsSourceChange(name.toString())) {
          tsDirty = true;
          trigger();
        }
      });
    } catch {
      // functions dir watch is best effort
    }
  }
  return await new Promise<never>(() => {});
}

/** Regenerates only `src/_generated/api.rs` (no cargo). Needs a previous build for the Rust functions. */
export async function codegenOnly(opts: BuildOptions): Promise<boolean> {
  const log = opts.log ?? ((m: string) => console.error(m));
  const functionsDir = resolveFunctionsDir(opts.cwd);
  const crateDir = path.resolve(opts.cwd, opts.crate ?? path.join(functionsDir, "rust"));
  const outDir = path.resolve(opts.cwd, opts.out ?? path.join(functionsDir, "rust"));
  if (!fs.existsSync(path.join(crateDir, "Cargo.toml"))) {
    throw new BuildError(`No Rust crate found: ${path.join(crateDir, "Cargo.toml")} does not exist.`);
  }
  const { targetDir } = await findCrateLayout(crateDir);
  const file = path.join(crateDir, "src", "_generated", "api.rs");
  const before = fs.existsSync(file);
  const changed = await updateApiRs({
    projectRoot: opts.cwd, functionsDir, crateDir, outDir,
    rustEntries: readRustCache(path.join(targetDir, "convex-rust-modules.json")), log,
  });
  log(changed ? "convex-rust: updated src/_generated/api.rs" : before ? "convex-rust: src/_generated/api.rs is up to date" : "convex-rust: nothing to generate (no api.rs and no `api::` use)");
  return changed;
}

#!/usr/bin/env node
import path from "node:path";
import { parseArgs } from "node:util";
import fs from "node:fs";
import { BuildError, Profile, buildOnce, codegenOnly, parseGroups, resolveFunctionsDir, watch } from "./build.js";
import { defaultTargetDir, scaffoldRust } from "./scaffold.js";
import { findCrateLayout } from "./build.js";
import { findWasiSdk, installWasiSdk, wasiSdkMode } from "./wasisdk.js";
import { checkRustToolchain, doctorReport } from "./toolchain.js";
import { resolveConvexFnPath } from "./shim.js";
import { readSymbolMaps, symbolize, symbolsDir } from "./symbols.js";

const USAGE = `convex-rust: "use rust" for stock Convex

Usage:
  convex-rust build [--watch] [--out <dir>] [--crate <dir>] [--profile release|dev] [--env A,B] [--no-simd] [--single] [--modules] [--split] [--group name=mod,mod ...]
  convex-rust dev [--out <dir>] [--crate <dir>] [--profile release|dev] [--env A,B] [--no-simd] [--split]
  convex-rust codegen [--out <dir>] [--crate <dir>]
  convex-rust init [--crate <dir>] [--convex-fn-path <dir>]
  convex-rust doctor [--crate <dir>]
  convex-rust symbolize [file] [--module <name>] [--crate <dir>] [--symbols <dir>]
  convex-rust wasi-sdk

build   Builds the Rust crate (default convex/rust) for wasm32-wasip1 and writes the
        wasm and the TypeScript action wrappers into --out (default convex/rust).
dev     Watch loop for use next to \`npx convex dev\`: rebuilds and regenerates on .rs / Cargo changes
        (debounced), and refreshes src/_generated/api.rs when your TypeScript functions change.
        Same as \`build --watch\`, with the dev profile unless --profile is given.
codegen Regenerates only src/_generated/api.rs (typed Rust references to your TS functions), no cargo.
--env    Env var names the Rust code may read (Convex env vars cannot be enumerated in the default
        runtime). More can be added at runtime with the deployment variable CONVEX_RUST_ENV=A,B.
--no-simd Build without wasm SIMD (-C target-feature=+simd128 is on by default). Use it if the runtime
        rejects SIMD wasm (CompileError mentioning simd). Also: CONVEX_RUST_SIMD=0.
One wasm per Convex module is the default, with no setup in the crate: it is compiled once, then once per
        module with \`--cfg convex_split --cfg convex_keep="<fn>"\` for that module's functions (the #[action] macro
        drops the other exports, so the linker removes their code). Dependencies are compiled once. If the crate
        defines cargo features m-<module> (two or more), those are used instead. Each module's wrapper imports its
        own wasm. A stock deployment compiles every imported wasm on every call (~0.85 ms per MB). If the wasm add
        up to more than 28 MiB (Convex bundles at most 32 MiB), the smallest modules are merged, with a warning.
--modules Also build one wasm per module in the dev profile (\`dev\` builds one shared wasm by default: the per-module
        build recompiles the crate once per module, ~8x slower rebuilds). Also: CONVEX_RUST_MODULES=1.
--single Build one shared wasm with the default features instead. Also: CONVEX_RUST_MODULES=0.
--split  Legacy, only with --single: cut per-module wasm from the shared wasm with wasm-opt (keeps everything the
        function table reaches, so it shrinks far less than the default). Also: CONVEX_RUST_SPLIT=1.
--group name=modA,modB  (repeatable) Build the crate once per group with cargo features m-<module> (no default
        features), one wasm per group (overrides the per-module default, e.g. to merge tiny modules into one light
        wasm); each module imports its group's wasm. Modules in no group are not built.
        Also: CONVEX_RUST_GROUPS="light=base,cb;heavy=cpu,data". Feature prefix: --group-prefix (default m-).
init    Creates a starter crate (never overwrites existing files) and prints the next steps.
doctor  Checks cargo, rustup, the toolchain + wasm32-wasip1 target, the shipped convex_fn crate and wasm-opt,
        with the command that fixes each problem. Exit code 1 when something required is missing.
--wasi-sdk / --no-wasi-sdk  C dependencies (zstd-sys, libsqlite3-sys, ...) are compiled with a wasi-sdk clang: WASI_SDK_PATH, or
        ~/.cache/convex-rust/wasi-sdk-34.0. By default (auto) it is downloaded (~180 MB, sha256-pinned) only when a
        dependency has a C build script and none is installed; --wasi-sdk always installs it, --no-wasi-sdk never does.
        Also: CONVEX_RUST_WASI_SDK=1|0.
symbolize  Rewrites wasm-function[123]:0x... frames from pasted Convex logs (file or stdin) into Rust function names, using
        the name map that build keeps in cargo's target dir (not deployed). --module picks the wasm when several were built.
wasi-sdk   Installs the pinned wasi-sdk into the cache dir (if missing) and prints its path.
`;

/** Appends `line` to the project's .gitignore unless it is already there. */
function ensureGitignored(cwd: string, line: string): void {
  const file = path.join(cwd, ".gitignore");
  let cur = "";
  try {
    cur = fs.readFileSync(file, "utf8");
  } catch {
    // no .gitignore yet
  }
  if (cur.split(/\r?\n/).some((l) => l.trim() === line || l.trim() === line.replace(/\/$/, ""))) return;
  fs.writeFileSync(file, cur + (cur === "" || cur.endsWith("\n") ? "" : "\n") + line + "\n");
  console.error(`updated .gitignore (${line})`);
}

async function main(argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  if (!command || command === "--help" || command === "-h" || command === "help") {
    console.log(USAGE);
    return command ? 0 : 1;
  }
  const { values, positionals } = parseArgs({
    args: rest,
    options: {
      watch: { type: "boolean", default: false },
      out: { type: "string" },
      crate: { type: "string" },
      env: { type: "string" },
      "no-simd": { type: "boolean", default: false },
      "wasi-sdk": { type: "boolean", default: false },
      "no-wasi-sdk": { type: "boolean", default: false },
      module: { type: "string" },
      symbols: { type: "string" },
      split: { type: "boolean", default: false },
      single: { type: "boolean", default: false },
      modules: { type: "boolean", default: false },
      group: { type: "string", multiple: true },
      "group-prefix": { type: "string" },
      profile: { type: "string" },
      "convex-fn-path": { type: "string" },
      help: { type: "boolean", short: "h", default: false },
    },
    allowPositionals: command === "symbolize",
  });
  if (values.help) {
    console.log(USAGE);
    return 0;
  }
  const cwd = process.cwd();
  try {
    if (command === "codegen") {
      await codegenOnly({ cwd, crate: values.crate, out: values.out, profile: "release" });
      return 0;
    }
    if (command === "symbolize") {
      let dir = values.symbols;
      if (!dir) {
        const crate = path.resolve(cwd, values.crate ?? path.join(resolveFunctionsDir(cwd), "rust"));
        dir = symbolsDir((await findCrateLayout(crate)).targetDir);
      }
      const input = positionals[0] ? fs.readFileSync(positionals[0], "utf8") : fs.readFileSync(0, "utf8");
      const res = symbolize(input, readSymbolMaps(dir), values.module);
      process.stdout.write(res.text);
      if (res.note) console.error(`convex-rust symbolize: ${res.note}`);
      else if (res.replaced === 0) console.error("convex-rust symbolize: no wasm-function[N] frames matched the symbol map");
      return res.note ? 1 : 0;
    }
    if (command === "wasi-sdk") {
      const dir = findWasiSdk()?.dir ?? (await installWasiSdk());
      console.log(dir);
      return 0;
    }
    if (command === "build" || command === "dev") {
      const profile =
        values.profile ??
        (command === "dev" || process.env.CONVEX_RUST_PROFILE === "dev" ? "dev" : "release");
      if (profile !== "release" && profile !== "dev") {
        console.error(`error: --profile must be release or dev (got ${profile}).`);
        return 2;
      }
      const groupSpecs = values.group?.length ? values.group : (process.env.CONVEX_RUST_GROUPS ?? "").split(";").filter((x) => x.trim());
      const opts = { wasiSdk: wasiSdkMode({ wasiSdk: values["wasi-sdk"], noWasiSdk: values["no-wasi-sdk"] }), groups: parseGroups(groupSpecs), groupFeaturePrefix: values["group-prefix"], perModule: values.single || process.env.CONVEX_RUST_MODULES === "0" ? false : values.modules || process.env.CONVEX_RUST_MODULES === "1" ? true : undefined, cwd, crate: values.crate, out: values.out, profile: profile as Profile, split: values.split || process.env.CONVEX_RUST_SPLIT === "1", simd: !values["no-simd"] && process.env.CONVEX_RUST_SIMD !== "0", env: (values.env ?? process.env.CONVEX_RUST_BUILD_ENV ?? "").split(",").map((s) => s.trim()).filter(Boolean) };
      if (values.watch || command === "dev") await watch(opts);
      else await buildOnce(opts);
      return 0;
    }
    if (command === "init") {
      const crate = path.resolve(cwd, values.crate ?? path.join(resolveFunctionsDir(cwd), "rust"));
      const res = scaffoldRust(crate, {
        convexFnPath: values["convex-fn-path"] ?? process.env.CONVEX_FN_PATH,
        channel: process.env.CONVEX_RUST_CHANNEL,
        targetDir: defaultTargetDir(cwd, crate),
      });
      ensureGitignored(cwd, ".convex-rust/");
      for (const f of res.created) console.error(`created ${path.relative(cwd, path.join(crate, f))}`);
      for (const f of res.skipped) console.error(`skipped ${path.relative(cwd, path.join(crate, f))} (exists)`);
      const tc = checkRustToolchain(crate);
      if (!tc.ok) console.error(`\nwarning: ${tc.message}`);
      const rel = path.relative(cwd, crate) || ".";
      console.error(
        [
          "",
          "Next:",
          "  npx convex-rust build        # compiles the crate, writes the wasm + TS wrappers (the first build downloads crates and can take a few minutes; later builds take seconds)",
          "  npx convex dev --once        # pushes it (or: npx convex deploy). Needs a configured Convex project: run `npx convex dev` once (creates/links one), or set CONVEX_DEPLOY_KEY (CI).",
          `  npx convex run rust/example:greet '{"name":"you"}'`,
          "",
          `While editing Rust, run \`npx convex-rust dev\` next to \`npx convex dev\`. Commit ${rel}/Cargo.lock and ${rel}/src/_generated/api.rs.`,
          `Check your setup any time with \`npx convex-rust doctor\`.`,
        ].join("\n"),
      );
      return 0;
    }
    if (command === "doctor") {
      const crate = path.resolve(cwd, values.crate ?? path.join(resolveFunctionsDir(cwd), "rust"));
      const hasCrate = fs.existsSync(path.join(crate, "Cargo.toml"));
      const rep = doctorReport(hasCrate ? crate : null, undefined, { convexFnPath: resolveConvexFnPath(crate) ?? "<not found>" });
      for (const l of rep.lines) console.log(`${l.ok ? "ok  " : l.optional ? "warn" : "FAIL"} ${l.text}`);
      if (!hasCrate) console.log(`note ${path.join(crate, "Cargo.toml")} not found: run \`npx convex-rust init\``);
      return rep.ok ? 0 : 1;
    }
    console.error(`error: unknown command ${command}\n\n${USAGE}`);
    return 2;
  } catch (e: any) {
    if (e instanceof BuildError) {
      console.error(`error: ${e.message}`);
      return 1;
    }
    throw e;
  }
}

main(process.argv.slice(2)).then(
  (code) => process.exit(code),
  (e) => {
    console.error(e);
    process.exit(1);
  },
);

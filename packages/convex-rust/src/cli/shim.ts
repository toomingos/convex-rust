// Managed `.cargo/config.toml` patch block: swaps `reqwest` for the built-in shim
// (crates/convex_reqwest), which sends requests through the host fetch. Port of the fork
// CLI's lib/rust/scaffold.ts.
import fs from "node:fs";
import path from "node:path";

export const MANAGED_BEGIN = "# >>> convex-managed:patch (begin)";
export const MANAGED_END = "# <<< convex-managed:patch (end)";

const toToml = (x: string) => JSON.stringify(x.split(path.sep).join("/"));

/** `node_modules/<pkg>/rust/<crate>` found by walking up from `fromDir`. */
function findInNodeModules(fromDir: string, pkg: string, crate: string): string | undefined {
  let dir = path.resolve(fromDir);
  for (;;) {
    const p = path.join(dir, "node_modules", pkg, "rust", crate);
    if (fs.existsSync(path.join(p, "Cargo.toml"))) return p;
    const up = path.dirname(dir);
    if (up === dir) return undefined;
    dir = up;
  }
}

/** The package's own copy (`<package>/rust/<crate>`, created by `npm run vendor-crates`). */
function ownCopy(crate: string): string | undefined {
  const here = path.dirname(new URL(import.meta.url).pathname);
  const p = path.resolve(here, "..", "..", "rust", crate);
  return fs.existsSync(path.join(p, "Cargo.toml")) ? p : undefined;
}

/**
 * Where the reqwest shim lives: `CONVEX_REQWEST_PATH`, else `node_modules/convex-rust/rust/convex_reqwest`
 * (or the fork's `node_modules/convex/rust/convex_reqwest`) above the crate, else this package's own copy.
 */
export function resolveReqwestShimPath(
  rustDir?: string,
  env: Record<string, string | undefined> = process.env,
): string | undefined {
  if (env.CONVEX_REQWEST_PATH) return path.resolve(env.CONVEX_REQWEST_PATH);
  if (rustDir) {
    for (const pkg of ["convex-rust", "convex"]) {
      const p = findInNodeModules(rustDir, pkg, "convex_reqwest");
      if (p) return p;
    }
  }
  return ownCopy("convex_reqwest");
}

/** Where the `convex_fn` crate lives (`CONVEX_FN_PATH`, else shipped copy), if anywhere. */
export function resolveConvexFnPath(
  rustDir?: string,
  env: Record<string, string | undefined> = process.env,
): string | undefined {
  if (env.CONVEX_FN_PATH) return path.resolve(env.CONVEX_FN_PATH);
  if (rustDir) {
    for (const pkg of ["convex-rust", "convex"]) {
      const p = findInNodeModules(rustDir, pkg, "convex_fn");
      if (p) return p;
    }
  }
  return ownCopy("convex_fn");
}

/** Portable when inside node_modules (relative), absolute otherwise. */
export function displayPath(rustDir: string, p: string): string {
  if (!p.split(path.sep).includes("node_modules")) return p;
  const rel = path.relative(path.resolve(rustDir), p);
  return path.isAbsolute(rel) ? p : rel;
}

export function patchBlockBody(shimPath?: string, convexFnPath?: string, withReqwest = true): string {
  if (!withReqwest) {
    return "# Nothing to patch: no dependency uses reqwest. When one is added (cargo add reqwest),\n# the next `convex-rust build` fills in the built-in reqwest replacement here.\n";
  }
  if (!shimPath) {
    return "# The built-in reqwest shim was not found (set CONVEX_REQWEST_PATH);\n# crates that depend on reqwest will not be able to make requests.\n";
  }
  // The shim depends on the registry `convex_fn`; with a path dependency, patch the
  // registry one to the same path so both resolve to one crate.
  const fnPatch = convexFnPath ? `convex_fn = { path = ${toToml(convexFnPath)} }\n` : "";
  return `[patch.crates-io]\nreqwest = { path = ${toToml(shimPath)} }\n${fnPatch}`;
}

/** Absolute path of the user's `convex_fn` path dependency in `<rustDir>/Cargo.toml`. */
export function userConvexFnPath(rustDir: string): string | undefined {
  let toml: string;
  try {
    toml = fs.readFileSync(path.join(rustDir, "Cargo.toml"), "utf8");
  } catch {
    return undefined;
  }
  const m = /^convex_fn\s*=\s*\{[^}\n]*\bpath\s*=\s*"([^"]+)"/m.exec(toml);
  return m ? path.resolve(rustDir, m[1]) : undefined;
}

export interface ConfigOptions {
  shimPath?: string | undefined;
  convexFnPath?: string | undefined;
  withReqwest?: boolean;
  /** `[build] target-dir`, relative to the crate directory (the parent of `.cargo`). */
  targetDir?: string | undefined;
}

export function managedConfigToml(o: ConfigOptions = {}): string {
  const build = o.targetDir
    ? `# Keeps cargo's output (target/) out of the Convex functions directory, whose bundler\n# would otherwise walk it.\n[build]\ntarget-dir = ${toToml(o.targetDir)}\n\n`
    : "";
  // No `[build] target`: plain `cargo test` / `cargo check` run on the host; convex-rust passes --target.
  return `# Managed by \`convex-rust\`: edit with care, the block below is regenerated.\n${build}${MANAGED_BEGIN}\n${patchBlockBody(o.shimPath, o.convexFnPath, o.withReqwest ?? false)}${MANAGED_END}\n`;
}

/** Does the crate depend on reqwest (directly or transitively)? */
export function usesReqwest(rustDir: string): boolean {
  const read = (f: string) => {
    try {
      return fs.readFileSync(path.join(rustDir, f), "utf8");
    } catch {
      return "";
    }
  };
  if (/^\[\[package\]\]\s*\nname\s*=\s*"reqwest"\s*$/m.test(read("Cargo.lock"))) return true;
  const toml = read("Cargo.toml");
  return /^\s*reqwest\s*=/m.test(toml) || /^\s*\[[^\]]*\bdependencies\.reqwest\]/m.test(toml);
}

/**
 * Rewrites the managed block of `<rustDir>/.cargo/config.toml`; everything outside the
 * markers is left alone. Creates the file when missing (and `Cargo.toml` exists); a file
 * without markers is left alone. Returns true if the file changed.
 */
export function syncManagedPatch(
  rustDir: string,
  shimPath: string | undefined = resolveReqwestShimPath(rustDir),
  withReqwest: boolean = usesReqwest(rustDir),
): boolean {
  const shim = shimPath ? displayPath(rustDir, shimPath) : undefined;
  const rawFn = shim ? userConvexFnPath(rustDir) : undefined;
  const fnPath = rawFn ? displayPath(rustDir, rawFn) : undefined;
  const file = path.join(rustDir, ".cargo", "config.toml");
  let cur: string;
  try {
    cur = fs.readFileSync(file, "utf8");
  } catch {
    if (!fs.existsSync(path.join(rustDir, "Cargo.toml"))) return false;
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, managedConfigToml({ shimPath: shim, convexFnPath: fnPath, withReqwest }));
    return true;
  }
  const b = cur.indexOf(MANAGED_BEGIN);
  const e = cur.indexOf(MANAGED_END);
  if (b < 0 || e < b) return false;
  const next =
    cur.slice(0, b) + MANAGED_BEGIN + "\n" + patchBlockBody(shim, fnPath, withReqwest) + cur.slice(e);
  if (next === cur) return false;
  fs.writeFileSync(file, next);
  return true;
}

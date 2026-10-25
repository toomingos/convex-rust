// Copies the Rust support crates (convex_fn, convex_fn_macros, convex_reqwest) from the monorepo's
// crates/ into ./rust so they ship inside the npm package. `convex-rust init` points the new
// crate's convex_fn dependency, and the managed reqwest [patch], at node_modules/convex-rust/rust/*.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const crates = path.resolve(root, "..", "..", "crates");
const dest = path.join(root, "rust");
if (!fs.existsSync(crates)) {
  if (!fs.existsSync(path.join(dest, "convex_fn", "Cargo.toml"))) {
    console.error("vendor-crates: ../../crates not found and ./rust is empty; refusing to pack a package without the Rust crates");
    process.exit(1);
  }
  console.error("vendor-crates: ../../crates not found (not in the monorepo); keeping ./rust as is");
  process.exit(0);
}

// `[workspace.package]` / `[workspace.dependencies]` of the monorepo root, to inline `x.workspace = true`.
const rootToml = fs.readFileSync(path.join(crates, "..", "Cargo.toml"), "utf8");
function section(name) {
  const m = new RegExp(`^\\[${name.replace(".", "\\.")}\\]\\n([\\s\\S]*?)(?=^\\[|$(?![\\s\\S]))`, "m").exec(rootToml);
  const out = {};
  for (const line of (m ? m[1] : "").split("\n")) {
    const kv = /^([A-Za-z0-9_-]+)\s*=\s*(.+)$/.exec(line.trim());
    if (kv) out[kv[1]] = kv[2];
  }
  return out;
}
const wsPackage = section("workspace.package");
const wsDeps = section("workspace.dependencies");
function inline(text) {
  return text
    .replace(/^(edition|license|version)\.workspace\s*=\s*true$/gm, (_m, k) => `${k} = ${wsPackage[k]}`)
    .replace(/^([A-Za-z0-9_-]+)\.workspace\s*=\s*true$/gm, (_m, k) => {
      let v = wsDeps[k];
      if (v === undefined) throw new Error(`workspace dependency ${k} not found`);
      // path = "crates/x" -> sibling directory
      v = v.replace(/path\s*=\s*"crates\/([^"]+)"/, 'path = "../$1"');
      return `${k} = ${v}`;
    });
}
fs.rmSync(dest, { recursive: true, force: true });
for (const name of ["convex_fn", "convex_fn_macros", "convex_reqwest"]) {
  const src = path.join(crates, name);
  const out = path.join(dest, name);
  fs.mkdirSync(out, { recursive: true });
  for (const f of ["Cargo.toml", "build.rs", "LICENSE", "README.md"]) {
    if (fs.existsSync(path.join(src, f))) fs.copyFileSync(path.join(src, f), path.join(out, f));
  }
  if (!fs.existsSync(path.join(out, "LICENSE"))) fs.copyFileSync(path.join(root, "LICENSE"), path.join(out, "LICENSE"));
  fs.cpSync(path.join(src, "src"), path.join(out, "src"), { recursive: true });
  // dev-dependencies point at monorepo-only crates (convex_fn_test); drop them.
  const toml = path.join(out, "Cargo.toml");
  const text = fs.readFileSync(toml, "utf8").replace(/\n\[dev-dependencies\][\s\S]*?(?=\n\[|$)/, "\n");
  fs.writeFileSync(toml, inline(text));
}
console.error(`vendor-crates: copied to ${path.relative(process.cwd(), dest) || dest}`);

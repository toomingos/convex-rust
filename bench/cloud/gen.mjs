// Generates the cloud bench project's convex/ tree from ../convex (single source of truth).
//
//   node gen.mjs            -> convex/{schema.ts,cbTargets.ts,node/*,v8/*}, shared/prng.ts, v8-na.json
//
// * convex/node/*  : verbatim copy of ../convex/node ("use node" actions).
// * convex/v8/*    : the SAME case code for the default (V8) runtime. For every case the exported action
//                    plus the transitive closure of the top-level declarations it references is extracted
//                    from the node module (TypeScript AST); "use node" is dropped. A case whose closure
//                    needs Node built-ins / native addons / Buffer is "n/a" for ts-v8 (reason recorded in
//                    v8-na.json) unless overrides/<module>.ts supplies a web-standard replacement for the
//                    declarations involved (same timed work, e.g. crypto.subtle instead of createHash).
// * convex/rust/*  : produced by build-rust.sh (convex-rust build), not here.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const here = path.dirname(fileURLToPath(import.meta.url));
const bench = path.join(here, "..");
const src = path.join(bench, "convex");
const dst = path.join(here, "convex");

// Modules / globals that do not exist in the default Convex runtime.
const BAD_MODULE = /^(node:|sharp$|argon2$|bcrypt$|@mongodb-js\/zstd$|@resvg\/resvg-js$|pdf-parse)/;
const BAD_GLOBAL = new Set(["Buffer", "require", "__dirname", "__filename", "setImmediate"]);

// Cases whose ts-v8 variant cannot run for a reason found by running it (not by static analysis).
// Filled from observation; id -> reason.
const MANUAL_NA = JSON.parse(fs.readFileSync(path.join(here, "v8-manual-na.json"), "utf8"));

for (const d of ["node", "v8"]) fs.rmSync(path.join(dst, d), { recursive: true, force: true }); // rustv8/ is owned by build-rust.sh
fs.mkdirSync(path.join(dst, "node"), { recursive: true });
fs.mkdirSync(path.join(dst, "v8"), { recursive: true });
fs.mkdirSync(path.join(here, "shared"), { recursive: true });
for (const f of ["schema.ts", "cbTargets.ts"]) fs.copyFileSync(path.join(src, f), path.join(dst, f));
for (const f of fs.readdirSync(path.join(src, "node"))) fs.copyFileSync(path.join(src, "node", f), path.join(dst, "node", f));
fs.copyFileSync(path.join(bench, "shared/prng.ts"), path.join(here, "shared/prng.ts"));

const cases = JSON.parse(fs.readFileSync(path.join(bench, "cases.json"), "utf8")).cases;
const byModule = new Map();
for (const c of cases) {
  const m = /^node\/([^:]+):(.+)$/.exec(c.node.path);
  if (!m) throw new Error("bad node path " + c.node.path);
  if (!byModule.has(m[1])) byModule.set(m[1], []);
  byModule.get(m[1]).push({ id: c.id, exp: m[2] });
}

/** Parse a file into { imports: Map<local, {decl, spec}>, decls: Map<name, stmt>, order: stmt[] }. */
function parseModule(file) {
  const text = fs.readFileSync(file, "utf8");
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TS);
  const imports = new Map(); // local name -> { decl, kind, imported, typeOnly }
  const decls = new Map(); // name -> statement
  const order = [];
  for (const st of sf.statements) {
    if (ts.isImportDeclaration(st)) {
      const c = st.importClause;
      if (!c) continue;
      const mod = st.moduleSpecifier.text;
      const clauseType = c.isTypeOnly;
      if (c.name) imports.set(c.name.text, { mod, kind: "default", typeOnly: clauseType });
      if (c.namedBindings) {
        if (ts.isNamespaceImport(c.namedBindings)) imports.set(c.namedBindings.name.text, { mod, kind: "ns", typeOnly: clauseType });
        else
          for (const e of c.namedBindings.elements)
            imports.set(e.name.text, { mod, kind: "named", imported: (e.propertyName ?? e.name).text, typeOnly: clauseType || e.isTypeOnly });
      }
      continue;
    }
    if (ts.isExpressionStatement(st) && ts.isStringLiteral(st.expression)) continue; // "use node"
    const names = [];
    if (ts.isVariableStatement(st)) for (const d of st.declarationList.declarations) if (ts.isIdentifier(d.name)) names.push(d.name.text);
    if ((ts.isFunctionDeclaration(st) || ts.isClassDeclaration(st) || ts.isTypeAliasDeclaration(st) || ts.isInterfaceDeclaration(st) || ts.isEnumDeclaration(st)) && st.name) names.push(st.name.text);
    const rec = { st, text: st.getText(sf), names, refs: refsOf(st) };
    order.push(rec);
    for (const n of names) decls.set(n, rec);
  }
  return { imports, decls, order };
}

function refsOf(root) {
  const out = new Set();
  const visit = (n) => {
    if (ts.isIdentifier(n)) {
      const p = n.parent;
      const isProp =
        (ts.isPropertyAccessExpression(p) && p.name === n) ||
        (ts.isPropertyAssignment(p) && p.name === n) ||
        (ts.isPropertySignature(p) && p.name === n) ||
        (ts.isMethodDeclaration(p) && p.name === n) ||
        (ts.isQualifiedName(p) && p.right === n) ||
        (ts.isBindingElement(p) && p.propertyName === n);
      if (!isProp) out.add(n.text);
    }
    ts.forEachChild(n, visit);
  };
  visit(root);
  return out;
}

function importText(local, imp) {
  const t = imp.typeOnly ? "type " : "";
  if (imp.kind === "default") return { mod: imp.mod, text: `import ${t}${local} from ${JSON.stringify(imp.mod)};` };
  if (imp.kind === "ns") return { mod: imp.mod, text: `import ${t}* as ${local} from ${JSON.stringify(imp.mod)};` };
  const spec = imp.imported === local ? local : `${imp.imported} as ${local}`;
  return { mod: imp.mod, text: `import ${t}{ ${spec} } from ${JSON.stringify(imp.mod)};` };
}

const na = {}; // id -> reason
const generated = [];

for (const [mod, list] of byModule) {
  const base = parseModule(path.join(src, "node", mod + ".ts"));
  const ovFile = path.join(here, "overrides", mod + ".ts");
  const ov = fs.existsSync(ovFile) ? parseModule(ovFile) : null;
  const imports = new Map(base.imports);
  const decls = new Map(base.decls);
  const overridden = new Set();
  if (ov) {
    for (const [k, v] of ov.imports) imports.set(k, v);
    for (const [k, v] of ov.decls) {
      decls.set(k, v);
      overridden.add(k);
    }
  }
  const kept = []; // { rec, id }
  const keptImports = new Map();
  let anyOk = false;
  const emitted = new Set();
  const emitRecs = [];
  for (const { id, exp } of list) {
    if (MANUAL_NA[id]) {
      na[id] = MANUAL_NA[id];
      continue;
    }
    const root = decls.get(exp);
    if (!root) {
      na[id] = `no top-level declaration ${exp} in ${mod}.ts`;
      continue;
    }
    // closure
    const seen = new Set();
    const recs = [];
    const impUsed = new Map();
    const bad = new Set();
    const stack = [root];
    while (stack.length) {
      const r = stack.pop();
      if (seen.has(r)) continue;
      seen.add(r);
      recs.push(r);
      for (const name of r.refs) {
        if (decls.has(name) && !r.names.includes(name)) stack.push(decls.get(name));
        else if (imports.has(name) && !decls.has(name)) {
          const imp = imports.get(name);
          impUsed.set(name, imp);
          if (BAD_MODULE.test(imp.mod) && !imp.typeOnly) bad.add(imp.mod);
        } else if (BAD_GLOBAL.has(name)) bad.add(name);
      }
    }
    if (bad.size) {
      na[id] = `uses ${[...bad].join(", ")}`;
      continue;
    }
    anyOk = true;
    for (const r of recs) if (!emitted.has(r)) { emitted.add(r); emitRecs.push(r); }
    for (const [k, v] of impUsed) keptImports.set(k, v);
  }
  if (!anyOk) continue;
  // original order: base statements first (replaced in place by their override), then override-only ones
  const baseOrder = base.order.map((r) => (r.names.length && overridden.has(r.names[0]) ? decls.get(r.names[0]) : r));
  const full = [...baseOrder, ...(ov ? ov.order.filter((r) => !baseOrder.includes(r)) : [])];
  const body = full.filter((r) => emitted.has(r)).map((r) => r.text);
  const impLines = [...keptImports].map(([k, v]) => {
    const t = importText(k, { ...v, mod: v.mod.startsWith("./") ? "../node/" + v.mod.slice(2) : v.mod });
    return t.text;
  });
  const header = `// @generated by cloud/gen.mjs from convex/node/${mod}.ts (same case code, default runtime). Do not edit.\n`;
  fs.writeFileSync(path.join(dst, "v8", mod + ".ts"), header + impLines.join("\n") + "\n\n" + body.join("\n\n") + "\n");
  generated.push(mod);
}

// Cold-start nonce: BENCH_NONCE=<x> appends `export const __nonce` to every function module (node, v8, rustv8)
// so a redeploy changes every module hash (new isolates / module loads on the next call).
const nonce = process.env.BENCH_NONCE;
for (const d of ["node", "v8", "rustv8"]) {
  const dir = path.join(dst, d);
  if (!fs.existsSync(dir)) continue;
  for (const f of fs.readdirSync(dir)) {
    if (!f.endsWith(".ts") || f.startsWith("_") || f.endsWith(".d.ts")) continue;
    const fp = path.join(dir, f);
    let t = fs.readFileSync(fp, "utf8").replace(/\nexport const __nonce = .*\n?$/s, "\n");
    if (nonce) t = t.replace(/\n*$/, "\n") + `export const __nonce = ${JSON.stringify(nonce)};\n`;
    fs.writeFileSync(fp, t);
  }
}

// Overridden cases are reported so the report can flag them.
const overriddenCases = [];
for (const [mod, list] of byModule) {
  const ovFile = path.join(here, "overrides", mod + ".ts");
  if (!fs.existsSync(ovFile)) continue;
  const ov = parseModule(ovFile);
  for (const { id, exp } of list) {
    const pure = exp.replace("_", "");
    if (ov.decls.has(pure) || ov.decls.has(exp)) if (!na[id]) overriddenCases.push(id);
  }
}
fs.writeFileSync(path.join(here, "v8-na.json"), JSON.stringify({ na, node_na: JSON.parse(fs.readFileSync(path.join(here, "node-manual-na.json"), "utf8")), overridden: overriddenCases }, null, 2) + "\n");
console.log(`v8 modules: ${generated.join(", ")}`);
console.log(`ts-v8 runnable: ${cases.length - Object.keys(na).length}/${cases.length}; n/a: ${Object.keys(na).length}; overridden: ${overriddenCases.length}`);

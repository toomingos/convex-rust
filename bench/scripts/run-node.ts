// Runs the Node side of one or more cases in-process (phase-0 style, no backend)
// and prints / writes the output.
//   node scripts/run-node.ts cpu-001 [cpu-002 ...]           print outputs
//   node scripts/run-node.ts --write cpu-001 ...              write bench/expected/<id>.json
//   node scripts/run-node.ts --write --category cpu           all phase-0 cases of a category
//   node scripts/run-node.ts --print-args cpu-001 ...         one JSON line per case: {"id","args"} (argsFor, Convex JSON)
//                                                             or {"id","error"} if args can't be built
// Each case's Node file exports the pure implementation as camelCase id without
// the dash (cpu-001 -> cpu001) with signature (ctx, args) => Promise<ConvexJSON>.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { convexToJson } from "convex/values";
import { argsFor, type Case } from "../shared/cases.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const all: Case[] = JSON.parse(readFileSync(join(root, "cases.json"), "utf8")).cases;

const argv = process.argv.slice(2);
const write = argv.includes("--write");
const printArgs = argv.includes("--print-args");
const catIdx = argv.indexOf("--category");
let ids = argv.filter((a, i) => !a.startsWith("--") && !(catIdx >= 0 && i === catIdx + 1));
if (catIdx >= 0) ids = all.filter((c) => c.category === argv[catIdx + 1] && c.phase0).map((c) => c.id);

// Minimal deterministic ctx stub so net-007 (ctx.storage.store) and net-008 (ctx.runMutation)
// can be regenerated; values mirror the committed expected/ files.
const stubCtx = {
  storage: { store: async (_blob: unknown) => "kg2stubstorageid" },
  runMutation: async (_ref: unknown, a: Record<string, unknown>) => ({ _id: "stubdocid", _creationTime: 1700000000000, ...a }),
};

for (const id of ids) {
  const c = all.find((x) => x.id === id);
  if (!c) throw new Error(`unknown case ${id}`);
  if (printArgs) {
    try {
      console.log(JSON.stringify({ id, args: convexToJson(argsFor(c) as any) }));
    } catch (e: any) {
      console.log(JSON.stringify({ id, error: String(e?.message ?? e) }));
    }
    continue;
  }
  const [file] = c.node.path.replace(/^node\//, "").split(":");
  const mod = await import(join(root, "convex", "node", `${file}.ts`));
  const fn = mod[id.replace("-", "")];
  if (typeof fn !== "function") throw new Error(`${id}: export ${id.replace("-", "")} not found in node/${file}.ts`);
  let out: unknown;
  try {
    out = { ok: convexToJson((await fn(stubCtx, argsFor(c))) ?? null) };
  } catch (e: any) {
    // Same envelope as convex_fn's Error::to_json (contracts/ABI.md "Result envelope").
    const message = String(e?.message ?? e);
    out = { error: e?.data !== undefined ? { message, data: convexToJson(e.data), kind: "convex" } : { message, kind: "user" } };
  }
  const text = JSON.stringify(out);
  if (write) {
    writeFileSync(join(root, "expected", `${id}.json`), text + "\n");
    console.log(`${id}: wrote ${text.length} bytes`);
  } else {
    console.log(`${id}: ${text.length > 2000 ? text.slice(0, 2000) + "…" : text}`);
  }
}

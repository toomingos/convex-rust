// Times the Node side of phase-0 cases in-process.
//   node scripts/time-node.ts [--iterations N] [--warmup N] cpu-001 [cpu-002 ...]
// Prints one JSON line per iteration:
//   {"case_id","iteration","ms","ok","output"}            (output = result envelope, Convex JSON)
// or, if the case's Node export / file / args are unavailable:
//   {"case_id","missing":"<reason>"}
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { convexToJson, jsonToConvex } from "convex/values";
import { argsFor, type Case } from "../shared/cases.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const all: Case[] = JSON.parse(readFileSync(join(root, "cases.json"), "utf8")).cases;

const argv = process.argv.slice(2);
const itIdx = argv.indexOf("--iterations");
const iterations = itIdx >= 0 ? Number(argv[itIdx + 1]) : 5;
const wuIdx = argv.indexOf("--warmup");
const warmup = wuIdx >= 0 ? Number(argv[wuIdx + 1]) : 2;
const ids = argv.filter((a, i) => !a.startsWith("--") && i !== itIdx + 1 && i !== wuIdx + 1);

// Result lines own stdout; user console output from cases goes to stderr.
const writeOut = (line: string) => process.stdout.write(line + "\n");
for (const k of ["log", "info", "debug", "warn"] as const) console[k] = console.error;
const emit = (o: unknown) => writeOut(JSON.stringify(o));

for (const id of ids) {
  const c = all.find((x) => x.id === id);
  if (!c) {
    emit({ case_id: id, missing: "unknown case" });
    continue;
  }
  const [file] = c.node.path.replace(/^node\//, "").split(":");
  let fn: any;
  try {
    const mod = await import(join(root, "convex", "node", `${file}.ts`));
    fn = mod[id.replace("-", "")];
  } catch (e: any) {
    emit({ case_id: id, missing: `node/${file}.ts not found` });
    continue;
  }
  if (typeof fn !== "function") {
    emit({ case_id: id, missing: `export ${id.replace("-", "")} not found in node/${file}.ts` });
    continue;
  }
  let args: Record<string, any>;
  try {
    args = argsFor(c);
  } catch (e: any) {
    emit({ case_id: id, missing: `args: ${String(e?.message ?? e)}` });
    continue;
  }
  // Boundary symmetric with the Rust host call: the args JSON string is built outside the
  // timer; inside, parse -> Convex values -> handler -> serialize the result envelope.
  const argsJson = JSON.stringify(convexToJson(args as any));
  for (let iteration = -warmup; iteration < iterations; iteration++) {
    let out: any;
    let text: string;
    const t0 = performance.now();
    try {
      const o = await fn(null, jsonToConvex(JSON.parse(argsJson)));
      text = JSON.stringify({ ok: convexToJson(o ?? null) });
    } catch (e: any) {
      const message = String(e?.message ?? e);
      text = JSON.stringify({ error: e?.data !== undefined ? { message, data: convexToJson(e.data), kind: "convex" } : { message, kind: "user" } });
    }
    const ms = performance.now() - t0;
    if (iteration < 0) continue; // warm-up: untimed, not reported
    out = JSON.parse(text);
    emit({ case_id: id, iteration, ms, ok: "ok" in out, output: out });
  }
}

// node:test reporter: one JSON line per finished leaf test, prefixed so the runner can find it.
export default async function* reporter(source) {
  for await (const ev of source) {
    if (ev.type !== "test:pass" && ev.type !== "test:fail") continue;
    const d = ev.data;
    // Suites only matter when they failed (a before/after hook threw, e.g. the native push).
    if (d.details?.type === "suite" && ev.type === "test:pass") continue;
    // ...and not when it only failed because a test inside it did.
    if (d.details?.type === "suite" && /subtests? failed/i.test(String(d.details?.error?.message ?? ""))) continue;
    const skip = d.skip ? (typeof d.skip === "string" ? d.skip : "skipped") : null;
    const err = ev.type === "test:fail" ? String(d.details?.error?.cause?.message ?? d.details?.error?.message ?? d.details?.error ?? "failed") : null;
    yield `@@RESULT ${JSON.stringify({ name: d.details?.type === "suite" ? `(suite) ${d.name}` : d.name, nesting: d.nesting, file: d.file, status: skip ? "SKIP" : ev.type === "test:pass" ? "PASS" : "FAIL", skip, error: err, ms: Math.round(d.details?.duration_ms ?? 0) })}\n`;
  }
}

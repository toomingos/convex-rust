// Optional sync ops `intl/*` (contracts/ABI.md): time zones and locale formatting backed by the
// runtime's `Intl`. A wasm32 guest has no tz database; these never fall back to UTC.

type Json = any;

const dtfCache = new Map<string, Intl.DateTimeFormat>();

function zoneFormat(tz: string): Intl.DateTimeFormat {
  let f = dtfCache.get(tz);
  if (!f) {
    // Throws RangeError for an unknown zone.
    f = new Intl.DateTimeFormat("en-US", {
      timeZone: tz,
      hourCycle: "h23",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "numeric",
      minute: "numeric",
      second: "numeric",
      era: "short",
    });
    dtfCache.set(tz, f);
  }
  return f;
}

/** Offset of `tz` from UTC at `epochMs`, in seconds (east positive). */
export function tzOffsetSeconds(tz: string, epochMs: number): number {
  if (typeof tz !== "string" || tz === "") throw new Error("intl/tzOffset: `tz` must be a non-empty string");
  if (typeof epochMs !== "number" || !Number.isFinite(epochMs)) {
    throw new Error("intl/tzOffset: `epochMs` must be a finite number");
  }
  const secs = Math.floor(epochMs / 1000);
  const parts = zoneFormat(tz).formatToParts(new Date(secs * 1000));
  const get = (t: string) => parts.find((p) => p.type === t)?.value;
  let year = Number(get("year"));
  if ((get("era") ?? "").startsWith("B")) year = 1 - year; // BC: astronomical year numbering
  // The wall clock read as if it were UTC (setUTCFullYear, since Date.UTC maps years 0..99 to 1900..1999).
  const d = new Date(0);
  d.setUTCFullYear(year, Number(get("month")) - 1, Number(get("day")));
  d.setUTCHours(Number(get("hour")), Number(get("minute")), Number(get("second")), 0);
  return Math.round((d.getTime() - secs * 1000) / 1000);
}

export function intlOp(op: string, args: Json): Json {
  try {
    if (op === "intl/tzOffset") {
      return { ok: tzOffsetSeconds(args?.tz, args?.epochMs) };
    }
    if (op === "intl/format") {
      const locale = args?.locale === undefined || args.locale === "" ? undefined : args.locale;
      const options = args?.options ?? undefined;
      if (args?.kind === "date") {
        if (typeof args.value !== "number" || !Number.isFinite(args.value)) {
          throw new Error("intl/format: date `value` must be a finite number of epoch milliseconds");
        }
        return { ok: new Intl.DateTimeFormat(locale, options).format(new Date(args.value)) };
      }
      if (args?.kind === "number") {
        if (typeof args.value !== "number") throw new Error("intl/format: number `value` must be a number");
        return { ok: new Intl.NumberFormat(locale, options).format(args.value) };
      }
      throw new Error(`intl/format: unknown kind ${JSON.stringify(args?.kind)}`);
    }
    return { error: { message: `unknown sync op ${op}` } };
  } catch (e: any) {
    const m = String(e?.message ?? e);
    return { error: { message: m.startsWith("intl/") ? m : `${op}: ${m}` } };
  }
}

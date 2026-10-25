"use node";
// Data-processing cases (data-*). Each case exports the pure implementation
// (`data001`, used by the phase-0 runner) and the Convex action (`data_001`).
// Inputs come from the shared PRNG; the draw order is mirrored exactly in
// bench/convex/rust/src/data.rs.
import { parse as csvParse } from "csv-parse/sync";
import { stringify as csvStringify } from "csv-stringify/sync";
import { marked } from "marked";
import sanitizeHtml from "sanitize-html";
import { diffLines } from "diff";
import Handlebars from "handlebars";
import { z } from "zod";
import { parse as yamlParse } from "yaml";
import { XMLParser } from "fast-xml-parser";
import slugify from "slugify";
import { seedAction } from "./_case.ts";
import { Prng } from "../../shared/prng.ts";

type Args = { seed: number };

const FNV_INIT = 0x811c9dc5;
/** FNV-1a (32-bit) over the UTF-16 code units of an ASCII string. */
function fnv(h: number, s: string): number {
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619) >>> 0;
  return h;
}

/** Counts opening tags by name (`<name` followed by a non-name char). */
function countTags(html: string, names: string[]): Record<string, number> {
  const out: Record<string, number> = Object.fromEntries(names.map((n) => [n, 0]));
  let i = html.indexOf("<");
  while (i !== -1) {
    let j = i + 1;
    while (j < html.length) {
      const c = html.charCodeAt(j);
      if ((c >= 97 && c <= 122) || (c >= 65 && c <= 90) || (c >= 48 && c <= 57)) j++;
      else break;
    }
    if (j > i + 1) {
      const name = html.slice(i + 1, j).toLowerCase();
      if (name in out) out[name]++;
    }
    i = html.indexOf("<", i + 1);
  }
  return out;
}

// --- shared CSV records (data-001, data-002) ---
const CSV_ROWS = 100_000;
function csvRecords(seed: number): string[][] {
  const rng = new Prng(seed);
  const rows: string[][] = [["id", "name", "qty", "note"]];
  for (let i = 0; i < CSV_ROWS; i++) {
    const name = rng.word(3 + rng.below(8));
    const qty = rng.below(1000);
    let note: string;
    switch (rng.below(4)) {
      case 0: {
        const a = rng.word(4);
        note = `${a}, ${rng.word(4)}`;
        break;
      }
      case 1: {
        const a = rng.word(3);
        note = `say "${a}" ${rng.word(3)}`;
        break;
      }
      case 2: {
        const a = rng.word(3);
        note = `${a}\n${rng.word(3)}`;
        break;
      }
      default:
        note = rng.word(6);
    }
    rows.push([String(i + 1), name, String(qty), note]);
  }
  return rows;
}

export async function data001(_ctx: unknown, args: Args) {
  const rows = csvRecords(args.seed);
  // Serialise by hand (the parse is what is measured).
  const q = (f: string) => (/[",\n]/.test(f) ? `"${f.replaceAll('"', '""')}"` : f);
  const text = rows.map((r) => r.map(q).join(",")).join("\n") + "\n";
  const parsed: string[][] = csvParse(text);
  let qty = 0;
  let noteChars = 0;
  for (let i = 1; i < parsed.length; i++) {
    qty += Number(parsed[i][2]);
    noteChars += parsed[i][3].length;
  }
  return { rows: parsed.length, qtySum: qty, noteChars, last: parsed[parsed.length - 1] };
}
export const data_001 = seedAction(data001);

export async function data002(_ctx: unknown, args: Args) {
  const rows = csvRecords(args.seed);
  const out: string = csvStringify(rows);
  return { length: out.length, hash: fnv(FNV_INIT, out) };
}
export const data_002 = seedAction(data002);

// --- data-003 markdown ---
function words(rng: Prng, n: number): string {
  const w: string[] = [];
  for (let i = 0; i < n; i++) w.push(rng.word(2 + rng.below(7)));
  return w.join(" ");
}

function markdownDoc(seed: number): string {
  const rng = new Prng(seed);
  const parts: string[] = [];
  let len = 0;
  while (len < 1_000_000) {
    let block: string;
    switch (rng.below(6)) {
      case 0:
        block = `# ${words(rng, 3)}`;
        break;
      case 1:
        block = `## ${words(rng, 3)}`;
        break;
      case 2:
        block = `### ${words(rng, 3)}`;
        break;
      case 3: {
        const n = 8 + rng.below(13);
        const ws: string[] = [];
        for (let i = 0; i < n; i++) {
          const w = rng.word(2 + rng.below(7));
          switch (rng.below(10)) {
            case 0:
              ws.push(`*${w}*`);
              break;
            case 1:
              ws.push(`**${w}**`);
              break;
            case 2:
              ws.push(`[${w}](http://example.com/${w})`);
              break;
            case 3:
              ws.push(`\`${w}\``);
              break;
            default:
              ws.push(w);
          }
        }
        block = ws.join(" ");
        break;
      }
      case 4: {
        const n = 2 + rng.below(4);
        const items: string[] = [];
        for (let i = 0; i < n; i++) items.push(`- ${words(rng, 3 + rng.below(4))}`);
        block = items.join("\n");
        break;
      }
      default: {
        const n = 1 + rng.below(4);
        const lines: string[] = [];
        for (let i = 0; i < n; i++) lines.push(words(rng, 4));
        block = "```\n" + lines.join("\n") + "\n```";
      }
    }
    parts.push(block);
    len += block.length + 2;
  }
  return parts.join("\n\n") + "\n";
}

export async function data003(_ctx: unknown, args: Args) {
  const md = markdownDoc(args.seed);
  const html = marked.parse(md, { async: false }) as string;
  return countTags(html, ["h1", "h2", "h3", "p", "ul", "li", "a", "em", "strong", "pre", "code"]);
}
export const data_003 = seedAction(data003);

// --- data-004 sanitize ---
function htmlDoc(seed: number): string {
  const rng = new Prng(seed);
  const parts: string[] = [];
  let len = 0;
  while (len < 1_000_000) {
    let frag: string;
    switch (rng.below(9)) {
      case 0:
        frag = `<p>${words(rng, 6)}</p>`;
        break;
      case 1:
        frag = `<b>${rng.word(5)}</b> <i>${rng.word(5)}</i>`;
        break;
      case 2: {
        const w = rng.word(6);
        frag = `<a href="http://example.com/${w}">${w}</a>`;
        break;
      }
      case 3:
        frag = `<a href="javascript:alert(1)">${rng.word(5)}</a>`;
        break;
      case 4:
        frag = `<script>alert("${rng.word(4)}")</script>`;
        break;
      case 5:
        frag = `<img src="x" onerror="alert(1)">`;
        break;
      case 6:
        frag = `<div onclick="x()">${words(rng, 3)}</div>`;
        break;
      case 7:
        frag = `<ul><li>${rng.word(5)}</li><li>${rng.word(5)}</li></ul>`;
        break;
      default:
        frag = `<p style="color:red">${words(rng, 4)}</p>`;
    }
    parts.push(frag);
    len += frag.length + 1;
  }
  return parts.join("\n");
}

function countSub(s: string, sub: string): number {
  let n = 0;
  for (let i = s.indexOf(sub); i !== -1; i = s.indexOf(sub, i + sub.length)) n++;
  return n;
}

export async function data004(_ctx: unknown, args: Args) {
  const html = htmlDoc(args.seed);
  const clean = sanitizeHtml(html, {
    allowedTags: ["p", "b", "i", "a", "ul", "li"],
    allowedAttributes: { a: ["href"] },
    allowedSchemes: ["http", "https", "mailto"],
    nonTextTags: ["script", "style"],
  });
  return {
    tags: countTags(clean, ["p", "b", "i", "a", "ul", "li", "div", "script", "img"]),
    href: countSub(clean, "href="),
    script: countSub(clean, "script"),
    alert: countSub(clean, "alert"),
    onAttr: countSub(clean, " onclick") + countSub(clean, " onerror") + countSub(clean, "style="),
  };
}
export const data_004 = seedAction(data004);

// --- data-005 date parse ---
function pad(n: number, w: number): string {
  return String(n).padStart(w, "0");
}
export async function data005(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const strs: string[] = [];
  for (let i = 0; i < 100_000; i++) {
    const y = 1970 + rng.below(131);
    const mo = 1 + rng.below(12);
    const d = 1 + rng.below(28);
    const h = rng.below(24);
    const mi = rng.below(60);
    const s = rng.below(60);
    const ms = rng.below(1000);
    strs.push(`${pad(y, 4)}-${pad(mo, 2)}-${pad(d, 2)}T${pad(h, 2)}:${pad(mi, 2)}:${pad(s, 2)}.${pad(ms, 3)}Z`);
  }
  let sum = 0;
  let first = 0;
  let last = 0;
  for (let i = 0; i < strs.length; i++) {
    const t = Date.parse(strs[i]);
    if (i === 0) first = t;
    last = t;
    sum += t % 1_000_000_007;
  }
  return { count: strs.length, sum, first, last };
}
export const data_005 = seedAction(data005);

// --- data-006 timezone convert ---
const ZONES = ["America/New_York", "Europe/Lisbon", "Asia/Tokyo", "Australia/Sydney", "Asia/Kolkata"];
export async function data006(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const fmts = ZONES.map(
    (timeZone) =>
      new Intl.DateTimeFormat("en-US", {
        timeZone,
        hourCycle: "h23",
        year: "numeric",
        month: "numeric",
        day: "numeric",
        hour: "numeric",
        minute: "numeric",
      }),
  );
  let acc = 0;
  let firstLocal = 0;
  for (let i = 0; i < 100_000; i++) {
    const secs = 946_684_800 + rng.below(946_080_000);
    const zone = rng.below(ZONES.length);
    const p: Record<string, number> = {};
    for (const part of fmts[zone].formatToParts(secs * 1000)) {
      if (part.type !== "literal") p[part.type] = Number(part.value);
    }
    const local = (((p.year * 12 + p.month) * 31 + p.day) * 24 + p.hour) * 60 + p.minute;
    if (i === 0) firstLocal = local;
    acc = (acc * 31 + local) % 1_000_000_007;
  }
  return { count: 100_000, acc, firstLocal };
}
export const data_006 = seedAction(data006);

// --- data-007 NFC ---
const NFC_PIECES = [
  "abc ",
  "é",
  "é",
  "ä",
  "ñ",
  "ộ",
  "ộ",
  "Å",
  "Å",
  "한",
  "한",
  "ḉ",
  "Ç",
  "x",
];
export async function data007(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const parts: string[] = [];
  for (let i = 0; i < 3_000_000; i++) parts.push(NFC_PIECES[rng.below(NFC_PIECES.length)]);
  const input = parts.join("");
  const out = input.normalize("NFC");
  let inCp = 0;
  for (const _ of input) inCp++;
  let outCp = 0;
  let h = FNV_INIT;
  for (const ch of out) {
    outCp++;
    h = Math.imul(h ^ ch.codePointAt(0)!, 16777619) >>> 0;
  }
  return { inCodePoints: inCp, outCodePoints: outCp, outUtf8Bytes: Buffer.byteLength(out), hash: h };
}
export const data_007 = seedAction(data007);

// --- data-008 word count ---
export async function data008(_ctx: unknown, args: Args) {
  const text = new Prng(args.seed).text(10 * 1024 * 1024);
  const counts = new Map<string, number>();
  let total = 0;
  for (const seg of new Intl.Segmenter("en", { granularity: "word" }).segment(text)) {
    if (!seg.isWordLike) continue;
    total++;
    counts.set(seg.segment, (counts.get(seg.segment) ?? 0) + 1);
  }
  const top = [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
    .slice(0, 3)
    .map(([word, count]) => ({ word, count }));
  return { words: total, unique: counts.size, top };
}
export const data_008 = seedAction(data008);

// --- data-009 line diff ---
export async function data009(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const a: string[] = [];
  const b: string[] = [];
  for (let i = 0; i < 2500; i++) {
    const ws: string[] = [];
    for (let k = 0; k < 5; k++) ws.push(rng.word(3 + rng.below(6)));
    const line = ws.join(" ") + "\n";
    a.push(line);
    switch (rng.below(10)) {
      case 0:
        break;
      case 1:
        b.push(rng.word(8) + " " + rng.word(8) + "\n");
        break;
      case 2:
        b.push(line);
        b.push(rng.word(8) + " " + rng.word(8) + "\n");
        break;
      default:
        b.push(line);
    }
  }
  let added = 0;
  let removed = 0;
  let unchanged = 0;
  for (const part of diffLines(a.join(""), b.join(""))) {
    const n = part.count ?? 0;
    if (part.added) added += n;
    else if (part.removed) removed += n;
    else unchanged += n;
  }
  return { linesA: a.length, linesB: b.length, added, removed, unchanged };
}
export const data_009 = seedAction(data009);

// --- data-010 handlebars ---
const TEMPLATE =
  '<li class="user">{{name}} ({{age}}){{#if active}} <b>active</b>{{/if}}{{#each tags}} #{{this}}{{/each}}</li>\n';
export async function data010(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const render = Handlebars.compile(TEMPLATE);
  let total = 0;
  let h = FNV_INIT;
  let first = "";
  for (let i = 0; i < 10_000; i++) {
    let name = rng.word(3 + rng.below(6));
    if (rng.below(4) === 0) name += " & <co>";
    const age = 18 + rng.below(60);
    const active = rng.below(2) === 0;
    const tags: string[] = [];
    for (let k = rng.below(4); k > 0; k--) tags.push(rng.word(4));
    const out = render({ name, age, active, tags });
    if (i === 0) first = out;
    total += out.length;
    h = fnv(h, out);
  }
  return { length: total, hash: h, first };
}
export const data_010 = seedAction(data010);

// --- data-011 zod ---
const UserSchema = z.object({
  id: z.number().int().positive(),
  email: z.string().email(),
  age: z.number().int().min(0).max(120),
  name: z.string().min(1).max(20),
  tags: z.array(z.string()).max(5),
});
export async function data011(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const bad = { id: 0, email: 0, age: 0, name: 0, tags: 0 };
  let valid = 0;
  for (let i = 0; i < 10_000; i++) {
    let id: unknown = i + 1;
    let email = `${rng.word(3 + rng.below(8))}@${rng.word(3 + rng.below(6))}.com`;
    let age = rng.below(121);
    let name = rng.word(1 + rng.below(20));
    const tags: string[] = [];
    for (let k = rng.below(6); k > 0; k--) tags.push(rng.word(3));
    switch (rng.below(10)) {
      case 0:
        id = "x";
        break;
      case 1:
        email = email.split("@")[0];
        break;
      case 2:
        age = 121 + rng.below(100);
        break;
      case 3:
        name = rng.word(21 + rng.below(5));
        break;
      case 4:
        for (let k = 6 + rng.below(3); k > 0; k--) tags.push(rng.word(3));
        break;
    }
    const r = UserSchema.safeParse({ id, email, age, name, tags });
    if (r.success) valid++;
    else for (const issue of r.error.issues) bad[issue.path[0] as keyof typeof bad]++;
  }
  return { valid, invalidId: bad.id, invalidEmail: bad.email, invalidAge: bad.age, invalidName: bad.name, invalidTags: bad.tags };
}
export const data_011 = seedAction(data011);

// --- data-012 YAML ---
function yamlDoc(seed: number): string {
  const rng = new Prng(seed);
  const parts: string[] = [];
  let len = 0;
  for (let i = 1; len < 1_000_000; i++) {
    const name = rng.word(4 + rng.below(5));
    const active = rng.below(2) === 0;
    const score = rng.below(10_000);
    const lines = [`- id: ${i}`, `  name: ${name}`, `  active: ${active}`, `  score: ${score}`, "  tags:"];
    for (let k = rng.below(4); k > 0; k--) lines.push(`    - ${rng.word(5)}`);
    lines.push("  meta:", `    owner: ${rng.word(6)}`, `    level: ${rng.below(10)}`);
    const item = lines.join("\n") + "\n";
    parts.push(item);
    len += item.length;
  }
  return parts.join("");
}
export async function data012(_ctx: unknown, args: Args) {
  const doc = yamlDoc(args.seed);
  const items = yamlParse(doc) as {
    id: number;
    active: boolean;
    score: number;
    tags: string[] | null;
    meta: { owner: string; level: number };
  }[];
  let idSum = 0, scoreSum = 0, active = 0, tags = 0, ownerChars = 0, levelSum = 0;
  for (const it of items) {
    idSum += it.id;
    scoreSum += it.score;
    if (it.active) active++;
    tags += it.tags?.length ?? 0;
    ownerChars += it.meta.owner.length;
    levelSum += it.meta.level;
  }
  return { items: items.length, idSum, scoreSum, active, tags, ownerChars, levelSum };
}
export const data_012 = seedAction(data012);

// --- data-013 XML ---
function xmlDoc(seed: number): string {
  const rng = new Prng(seed);
  const parts: string[] = ["<catalog>\n"];
  let len = 0;
  for (let i = 1; len < 5_000_000; i++) {
    const cat = rng.word(3);
    const name = rng.word(5 + rng.below(8));
    const price = rng.below(100_000);
    let tags = "";
    for (let k = 1 + rng.below(4); k > 0; k--) tags += `<tag>${rng.word(4)}</tag>`;
    const item = `<item id="${i}" cat="${cat}"><name>${name}</name><price>${price}</price><tags>${tags}</tags></item>\n`;
    parts.push(item);
    len += item.length;
  }
  parts.push("</catalog>\n");
  return parts.join("");
}
export async function data013(_ctx: unknown, args: Args) {
  const xml = xmlDoc(args.seed);
  const parser = new XMLParser({
    ignoreAttributes: false,
    attributeNamePrefix: "@_",
    isArray: (name) => name === "item" || name === "tag",
  });
  const items = parser.parse(xml).catalog.item as {
    "@_id": string;
    "@_cat": string;
    name: string;
    price: number;
    tags: { tag: string[] };
  }[];
  let idSum = 0, priceSum = 0, nameChars = 0, catChars = 0, tags = 0;
  for (const it of items) {
    idSum += Number(it["@_id"]);
    priceSum += Number(it.price);
    nameChars += String(it.name).length;
    catChars += it["@_cat"].length;
    tags += it.tags.tag.length;
  }
  return { items: items.length, idSum, priceSum, nameChars, catChars, tags };
}
export const data_013 = seedAction(data013);

// --- data-014 URL parse ---
export async function data014(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const schemes = ["http", "https", "ws"];
  const tlds = ["com", "org", "net"];
  const ports = [80, 443, 8080, 3000];
  const urls: string[] = [];
  for (let i = 0; i < 100_000; i++) {
    const scheme = schemes[rng.below(3)];
    let host = `${rng.word(3 + rng.below(8))}.${tlds[rng.below(3)]}`;
    if (rng.below(3) === 0) host = host.toUpperCase();
    const port = rng.below(4) === 0 ? `:${ports[rng.below(4)]}` : "";
    let user = "";
    if (rng.below(5) === 0) {
      const u = rng.word(3);
      user = `u${u}:p${rng.word(3)}@`;
    }
    let path = "";
    for (let k = rng.below(4); k > 0; k--) {
      path += "/" + (rng.below(5) === 0 ? `${rng.word(3)} ${rng.word(2)}` : rng.word(3 + rng.below(6)));
    }
    let query = "";
    if (rng.below(2) === 0) {
      const ps: string[] = [];
      for (let k = 1 + rng.below(3); k > 0; k--) {
        const key = rng.word(2);
        ps.push(`${key}=${rng.word(3)}`);
      }
      query = "?" + ps.join("&");
    }
    const frag = rng.below(3) === 0 ? `#${rng.word(4)}` : "";
    urls.push(`${scheme}://${user}${host}${port}${path}${query}${frag}`);
  }
  let h = FNV_INIT;
  let params = 0;
  let len = 0;
  for (const s of urls) {
    const u = new URL(s);
    params += [...u.searchParams].length;
    const sig = `${u.hostname}|${u.port}|${u.username}|${u.password}|${u.pathname}|${u.search}|${u.hash}\n`;
    len += sig.length;
    h = fnv(h, sig);
  }
  return { count: urls.length, params, len, hash: h };
}
export const data_014 = seedAction(data014);

// --- data-015 slugify ---
export async function data015(_ctx: unknown, args: Args) {
  const rng = new Prng(args.seed);
  const accents = ["é", "ü", "ñ", "å"];
  const inputs: string[] = [];
  for (let i = 0; i < 100_000; i++) {
    let s = "";
    for (let k = 2 + rng.below(5); k > 0; k--) {
      let w = rng.word(2 + rng.below(8));
      if (rng.below(4) === 0) w = w[0].toUpperCase() + w.slice(1);
      if (rng.below(8) === 0) w += accents[rng.below(4)];
      if (rng.below(5) === 0) w += rng.below(2) === 0 ? "," : ".";
      s += w + (rng.below(6) === 0 ? "  " : " ");
    }
    inputs.push(s.trimEnd());
  }
  let h = FNV_INIT;
  let len = 0;
  const first: string[] = [];
  for (const s of inputs) {
    const slug = slugify(s, { lower: true, strict: true });
    if (first.length < 3) first.push(slug);
    len += slug.length;
    h = fnv(h, slug + "\n");
  }
  return { count: inputs.length, len, hash: h, first };
}
export const data_015 = seedAction(data015);

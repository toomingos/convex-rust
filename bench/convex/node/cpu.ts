"use node";
// Pure CPU cases (cpu-*). Each case exports the pure implementation (`cpu001`,
// used by the phase-0 runner) and the Convex action (`cpu_001`).

// --- cpu-001..010 (W1-03) ---
import { createHash } from "node:crypto";
import { crc32 } from "node:zlib";
import { blake3 } from "@noble/hashes/blake3";
import { bytesToHex } from "@noble/hashes/utils";
import { seedAction, TEN_MB } from "./_case.ts";
import { ALPHABET, Prng } from "../../shared/prng.ts";

export async function cpu001(_ctx: unknown, args: { seed: number }): Promise<string> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return createHash("sha256").update(data).digest("hex");
}
export const cpu_001 = seedAction(cpu001);

export async function cpu002(_ctx: unknown, args: { seed: number }): Promise<string> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return createHash("sha512").update(data).digest("hex");
}
export const cpu_002 = seedAction(cpu002);

export async function cpu003(_ctx: unknown, args: { seed: number }): Promise<string> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return bytesToHex(blake3(data));
}
export const cpu_003 = seedAction(cpu003);

export async function cpu004(_ctx: unknown, args: { seed: number }): Promise<string> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return createHash("md5").update(data).digest("hex");
}
export const cpu_004 = seedAction(cpu004);

export async function cpu005(_ctx: unknown, args: { seed: number }): Promise<number> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return crc32(data);
}
export const cpu_005 = seedAction(cpu005);

function fib(n: number): number {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}
export async function cpu006(_ctx: unknown, _args: { seed: number }): Promise<number> {
  return fib(30);
}
export const cpu_006 = seedAction(cpu006);

export async function cpu007(
  _ctx: unknown,
  _args: { seed: number },
): Promise<{ count: number; last: number }> {
  const n = 10_000_000;
  const composite = new Uint8Array(n + 1);
  for (let i = 2; i * i <= n; i++) {
    if (composite[i] === 0) {
      for (let j = i * i; j <= n; j += i) composite[j] = 1;
    }
  }
  let count = 0;
  let last = 0;
  for (let i = 2; i <= n; i++) {
    if (composite[i] === 0) {
      count++;
      last = i;
    }
  }
  return { count, last };
}
export const cpu_007 = seedAction(cpu007);

export async function cpu008(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ min: number; median: number; max: number; sample: number[]; sum: number }> {
  const n = 1_000_000;
  const rng = new Prng(args.seed);
  const a = new Int32Array(n);
  for (let i = 0; i < n; i++) a[i] = rng.nextU32() | 0;
  a.sort(); // typed-array sort is numeric
  let sum = 0; // |sum| < 1e6 * 2^31 < 2^53, exact
  for (let i = 0; i < n; i++) sum += a[i];
  const sample = [0, 1, 1000, 250000, 500000, 750000, 999999].map((i) => a[i]);
  return { min: a[0], median: a[n >> 1], max: a[n - 1], sample, sum };
}
export const cpu_008 = seedAction(cpu008);

// ASCII lowercase words: JS default sort (UTF-16 code units) == Rust byte order.
export async function cpu009(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ first: string; median: string; last: string; unique: number; sample: string[] }> {
  const n = 100_000;
  const rng = new Prng(args.seed);
  const words: string[] = [];
  for (let i = 0; i < n; i++) words.push(rng.word(4 + rng.below(13)));
  words.sort();
  let unique = 1;
  for (let i = 1; i < n; i++) if (words[i] !== words[i - 1]) unique++;
  const sample = [1, 1000, 25000, 50000, 75000, 99998].map((i) => words[i]);
  return { first: words[0], median: words[n >> 1], last: words[n - 1], unique, sample };
}
export const cpu_009 = seedAction(cpu009);

export async function cpu010(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ cells: number[]; sum: number }> {
  const n = 256;
  const rng = new Prng(args.seed);
  const a = new Float64Array(n * n);
  const b = new Float64Array(n * n);
  for (let i = 0; i < n * n; i++) a[i] = rng.f64();
  for (let i = 0; i < n * n; i++) b[i] = rng.f64();
  const c = new Float64Array(n * n);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      let s = 0;
      for (let k = 0; k < n; k++) s += a[i * n + k] * b[k * n + j];
      c[i * n + j] = s;
    }
  }
  let sum = 0;
  for (let i = 0; i < n * n; i++) sum += c[i];
  const cells = [c[0], c[n - 1], c[(n / 2) * n + n / 2], c[(n - 1) * n], c[n * n - 1]];
  return { cells, sum };
}
export const cpu_010 = seedAction(cpu010);

// --- cpu-011..020 (W1-04) ---
import { gunzipSync, gzipSync } from "node:zlib";
import { compress as zstdCompress, decompress as zstdDecompress } from "@mongodb-js/zstd";
import { distance } from "fastest-levenshtein";

const MB10 = 10_000_000;

/** FNV-1a 32-bit over bytes. */
function fnv1a(buf: Uint8Array): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < buf.length; i++) h = Math.imul(h ^ buf[i], 0x01000193);
  return h >>> 0;
}

/** PRNG.text(n) as a Buffer of ASCII bytes (same draws, no string concat). */
function textBytes(p: Prng, n: number): Buffer {
  const out = Buffer.allocUnsafe(n);
  for (let i = 0; i < n; i++) out[i] = ALPHABET.charCodeAt(p.below(ALPHABET.length));
  return out;
}

// cpu-011: n-body, 1000 steps (5 bodies, energy before/after)
export async function cpu011(_ctx: unknown, args: { seed: number }): Promise<number[]> {
  const p = new Prng(args.seed);
  const n = 5;
  const pos: number[][] = [];
  const vel: number[][] = [];
  const mass: number[] = [];
  for (let i = 0; i < n; i++) {
    pos.push([(p.f64() - 0.5) * 20, (p.f64() - 0.5) * 20, (p.f64() - 0.5) * 20]);
    vel.push([p.f64() - 0.5, p.f64() - 0.5, p.f64() - 0.5]);
    mass.push(0.5 + p.f64());
  }
  const energy = (): number => {
    let e = 0;
    for (let i = 0; i < n; i++) {
      const [vx, vy, vz] = vel[i];
      e += 0.5 * mass[i] * (vx * vx + vy * vy + vz * vz);
      for (let j = i + 1; j < n; j++) {
        const dx = pos[i][0] - pos[j][0];
        const dy = pos[i][1] - pos[j][1];
        const dz = pos[i][2] - pos[j][2];
        e -= (mass[i] * mass[j]) / Math.sqrt(dx * dx + dy * dy + dz * dz);
      }
    }
    return e;
  };
  const e0 = energy();
  const dt = 0.001;
  for (let step = 0; step < 1000; step++) {
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const dx = pos[i][0] - pos[j][0];
        const dy = pos[i][1] - pos[j][1];
        const dz = pos[i][2] - pos[j][2];
        const d2 = dx * dx + dy * dy + dz * dz;
        const mag = dt / (d2 * Math.sqrt(d2));
        vel[i][0] -= dx * mass[j] * mag;
        vel[i][1] -= dy * mass[j] * mag;
        vel[i][2] -= dz * mass[j] * mag;
        vel[j][0] += dx * mass[i] * mag;
        vel[j][1] += dy * mass[i] * mag;
        vel[j][2] += dz * mass[i] * mag;
      }
    }
    for (let i = 0; i < n; i++) {
      pos[i][0] += dt * vel[i][0];
      pos[i][1] += dt * vel[i][1];
      pos[i][2] += dt * vel[i][2];
    }
  }
  return [e0, energy()];
}
export const cpu_011 = seedAction(cpu011);

// cpu-012: mandelbrot 1000x1000, 100 max iterations, iteration-count checksum
export async function cpu012(_ctx: unknown, _args: { seed: number }): Promise<number> {
  const size = 1000;
  const maxIter = 100;
  let sum = 0;
  for (let py = 0; py < size; py++) {
    const ci = (py / size) * 2 - 1;
    for (let px = 0; px < size; px++) {
      const cr = (px / size) * 3.5 - 2.5;
      let zr = 0;
      let zi = 0;
      let it = 0;
      while (it < maxIter && zr * zr + zi * zi <= 4) {
        const t = zr * zr - zi * zi + cr;
        zi = 2 * zr * zi + ci;
        zr = t;
        it++;
      }
      sum += it;
    }
  }
  return sum;
}
export const cpu_012 = seedAction(cpu012);

// cpu-013: regex scan over 10 MB of PRNG text
export async function cpu013(_ctx: unknown, args: { seed: number }): Promise<{ count: number; chars: number }> {
  const text = textBytes(new Prng(args.seed), MB10).toString("latin1");
  const re = /\b[a-z]{4,6}\b/g;
  let count = 0;
  let chars = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    count++;
    chars += m[0].length;
  }
  return { count, chars };
}
export const cpu_013 = seedAction(cpu013);

const RECORDS = 100_000;

// cpu-014: JSON parse ~10 MB (document built by string template from the PRNG)
export async function cpu014(_ctx: unknown, args: { seed: number }): Promise<Record<string, number>> {
  const p = new Prng(args.seed);
  const parts: string[] = [];
  for (let i = 0; i < RECORDS; i++) {
    const name = p.word(8);
    const score = p.below(1_000_000);
    const t0 = p.word(6);
    const t1 = p.word(6);
    const t2 = p.word(6);
    const active = p.below(2) === 1;
    parts.push(
      `{"id":${i},"name":"${name}","score":${score},"tags":["${t0}","${t1}","${t2}"],"active":${active}}`,
    );
  }
  const text = `[${parts.join(",")}]`;
  const docs = JSON.parse(text) as { id: number; name: string; score: number; tags: string[]; active: boolean }[];
  let scoreSum = 0;
  let nameChars = 0;
  let tagCount = 0;
  let activeCount = 0;
  for (const d of docs) {
    scoreSum += d.score;
    nameChars += d.name.length;
    tagCount += d.tags.length;
    if (d.active) activeCount++;
  }
  return { len: text.length, records: docs.length, scoreSum, nameChars, tagCount, activeCount };
}
export const cpu_014 = seedAction(cpu014);

// cpu-015: JSON stringify ~10 MB
export async function cpu015(_ctx: unknown, args: { seed: number }): Promise<{ len: number; hash: number }> {
  const p = new Prng(args.seed);
  const docs = [];
  for (let i = 0; i < RECORDS; i++) {
    docs.push({
      id: i,
      name: p.word(8),
      score: p.below(1_000_000),
      tags: [p.word(6), p.word(6), p.word(6)],
      active: p.below(2) === 1,
    });
  }
  const text = JSON.stringify(docs);
  const bytes = Buffer.from(text, "utf8");
  return { len: bytes.length, hash: fnv1a(bytes) };
}
export const cpu_015 = seedAction(cpu015);

// cpu-016: base64 encode 10 MB
export async function cpu016(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ len: number; head: string; mid: string; tail: string }> {
  const u8 = new Prng(args.seed).bytes(MB10);
  const data = Buffer.from(u8.buffer, u8.byteOffset, u8.byteLength); // zero-copy view
  const s = data.toString("base64");
  const m = s.length >> 1;
  return { len: s.length, head: s.slice(0, 32), mid: s.slice(m, m + 32), tail: s.slice(-32) };
}
export const cpu_016 = seedAction(cpu016);

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// cpu-017: base64 decode 10 MB of base64 text (generated from the PRNG)
export async function cpu017(_ctx: unknown, args: { seed: number }): Promise<{ len: number; hash: number }> {
  const p = new Prng(args.seed);
  const enc = Buffer.allocUnsafe(MB10);
  for (let i = 0; i < MB10; i++) enc[i] = B64.charCodeAt(p.below(64));
  const out = Buffer.from(enc.toString("latin1"), "base64");
  return { len: out.length, hash: fnv1a(out) };
}
export const cpu_017 = seedAction(cpu017);

// cpu-018: gzip compress 10 MB of PRNG text (level 6), roundtrip check
export async function cpu018(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ inLen: number; outLen: number; roundtrip: boolean }> {
  const data = textBytes(new Prng(args.seed), MB10);
  const z = gzipSync(data, { level: 6 });
  return { inLen: data.length, outLen: z.length, roundtrip: gunzipSync(z).equals(data) };
}
export const cpu_018 = seedAction(cpu018);

// cpu-019: zstd compress 10 MB of PRNG text (default level), roundtrip check
export async function cpu019(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ inLen: number; outLen: number; roundtrip: boolean }> {
  const data = textBytes(new Prng(args.seed), MB10);
  const z = await zstdCompress(data, 1); // level 1 = ruzstd "Fastest", the only level the Rust side has
  return { inLen: data.length, outLen: z.length, roundtrip: (await zstdDecompress(z)).equals(data) };
}
export const cpu_019 = seedAction(cpu019);

// cpu-020: Levenshtein over 1k random lowercase string pairs (50..149 chars)
export async function cpu020(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ sum: number; first: number; max: number }> {
  const p = new Prng(args.seed);
  let sum = 0;
  let first = 0;
  let max = 0;
  for (let i = 0; i < 1000; i++) {
    const a = p.word(50 + p.below(100));
    const b = p.word(50 + p.below(100));
    const d = distance(a, b);
    if (i === 0) first = d;
    sum += d;
    if (d > max) max = d;
  }
  return { sum, first, max };
}
export const cpu_020 = seedAction(cpu020);

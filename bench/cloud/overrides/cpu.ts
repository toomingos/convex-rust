// ts-v8 replacements (default runtime has no node:crypto / Buffer). Same work as the node case:
// the same PRNG input and the same operation, through web-standard APIs. Declarations here replace
// the same-named ones of ../../convex/node/cpu.ts when generating convex/v8/cpu.ts.
import { ALPHABET, Prng } from "../../shared/prng.ts";
import { seedAction, TEN_MB } from "./_case.ts";

const hex = (b: ArrayBuffer): string => Array.from(new Uint8Array(b), (x) => x.toString(16).padStart(2, "0")).join("");

export async function cpu001(_ctx: unknown, args: { seed: number }): Promise<string> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return hex(await crypto.subtle.digest("SHA-256", data));
}

export async function cpu002(_ctx: unknown, args: { seed: number }): Promise<string> {
  const data = new Prng(args.seed).bytes(TEN_MB);
  return hex(await crypto.subtle.digest("SHA-512", data));
}

/** PRNG.text(n) as ASCII bytes (same draws, no string concat). */
function textBytes(p: Prng, n: number): Uint8Array {
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = ALPHABET.charCodeAt(p.below(ALPHABET.length));
  return out;
}

/** Latin-1 decode of bytes (Buffer#toString("latin1")). */
function latin1(b: Uint8Array): string {
  const parts: string[] = [];
  for (let i = 0; i < b.length; i += 0x8000) parts.push(String.fromCharCode.apply(null, b.subarray(i, i + 0x8000) as unknown as number[]));
  return parts.join("");
}

export async function cpu013(_ctx: unknown, args: { seed: number }): Promise<{ count: number; chars: number }> {
  const text = latin1(textBytes(new Prng(args.seed), MB10));
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
  const bytes = new TextEncoder().encode(text);
  return { len: bytes.length, hash: fnv1a(bytes) };
}

export async function cpu016(
  _ctx: unknown,
  args: { seed: number },
): Promise<{ len: number; head: string; mid: string; tail: string }> {
  const data = new Prng(args.seed).bytes(MB10);
  const s = btoa(latin1(data));
  const m = s.length >> 1;
  return { len: s.length, head: s.slice(0, 32), mid: s.slice(m, m + 32), tail: s.slice(-32) };
}

export async function cpu017(_ctx: unknown, args: { seed: number }): Promise<{ len: number; hash: number }> {
  const p = new Prng(args.seed);
  const enc = new Uint8Array(MB10);
  for (let i = 0; i < MB10; i++) enc[i] = B64.charCodeAt(p.below(64));
  const bin = atob(latin1(enc));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return { len: out.length, hash: fnv1a(out) };
}

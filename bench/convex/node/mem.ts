"use node";
// Memory and concurrency cases (mem-*). Each exports the pure implementation
// (`mem001`) and the Convex action (`mem_001`). Rust twin: rust/src/mem.rs.
import { seedAction } from "./_case.ts";
import { Prng } from "../../shared/prng.ts";

type Args = { seed: number };

/** mem-001: allocate 100 MiB, write every byte, checksum every 4096th byte. */
export async function mem001(_ctx: unknown, a: Args): Promise<number> {
  const n = 100 * 1024 * 1024;
  const buf = new Uint8Array(n);
  for (let i = 0; i < n; i++) buf[i] = (Math.imul(i, 31) + a.seed) & 255;
  let sum = 0;
  for (let i = 0; i < n; i += 4096) sum += buf[i];
  return sum;
}
export const mem_001 = seedAction(mem001);

/** mem-002: 1M small heap allocations (4-element u32 buffers), all kept alive. */
export async function mem002(_ctx: unknown, a: Args): Promise<number> {
  const n = 1_000_000;
  const items: Uint32Array[] = new Array(n);
  for (let i = 0; i < n; i++) {
    const b = new Uint32Array(4);
    b[0] = i;
    b[1] = (i ^ a.seed) >>> 0;
    b[2] = Math.imul(i, 3) >>> 0;
    b[3] = (i + a.seed) >>> 0;
    items[i] = b;
  }
  let sum = 0;
  for (let i = 0; i < n; i++) sum = (sum + items[i][i & 3]) % 4294967296;
  return sum;
}
export const mem_002 = seedAction(mem002);

/** mem-003: fan out 100 tasks that each sleep 1-5 ms, join them. */
export async function mem003(_ctx: unknown, a: Args): Promise<number[]> {
  const task = async (i: number): Promise<number> => {
    await new Promise<void>((r) => setTimeout(r, 1 + (i % 5)));
    return i * i + a.seed;
  };
  return await Promise.all(Array.from({ length: 100 }, (_, i) => task(i)));
}
export const mem_003 = seedAction(mem003);

/** mem-004: insert 1M PRNG keys into a hash map, then look all of them up. */
export async function mem004(_ctx: unknown, a: Args): Promise<{ size: number; sum: number }> {
  const n = 1_000_000;
  const m = new Map<number, number>();
  const p = new Prng(a.seed);
  for (let i = 0; i < n; i++) m.set(p.nextU32(), i);
  const q = new Prng(a.seed);
  let sum = 0;
  for (let i = 0; i < n; i++) sum += m.get(q.nextU32())!;
  return { size: m.size, sum };
}
export const mem_004 = seedAction(mem004);

/** mem-005: build a 10 MiB string by repeated concatenation of 10-char pieces. */
export async function mem005(_ctx: unknown, a: Args): Promise<{ len: number; hash: number }> {
  const p = new Prng(a.seed);
  const words = Array.from({ length: 16 }, () => p.word(10));
  const n = 1024 * 1024;
  let s = "";
  for (let i = 0; i < n; i++) s += words[(i + a.seed) & 15];
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (Math.imul(h, 31) + s.charCodeAt(i)) >>> 0;
  return { len: s.length, hash: h };
}
export const mem_005 = seedAction(mem005);

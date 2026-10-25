// ts-v8 replacement: Buffer.byteLength(out) -> TextEncoder (UTF-8 length), same work otherwise.
import { Prng } from "../../shared/prng.ts";

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
  return { inCodePoints: inCp, outCodePoints: outCp, outUtf8Bytes: new TextEncoder().encode(out).length, hash: h };
}

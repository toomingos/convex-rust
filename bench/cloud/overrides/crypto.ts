// ts-v8 replacement: HMAC-SHA256 x100k chained, via crypto.subtle instead of node:crypto createHmac.
import { Prng } from "../../shared/prng.ts";

export async function crypto001(_ctx: unknown, args: { seed: number }): Promise<string> {
  const p = new Prng(args.seed);
  const key = p.bytes(32);
  let mac: Uint8Array = p.bytes(64);
  const k = await crypto.subtle.importKey("raw", key, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  for (let i = 0; i < 100_000; i++) {
    mac = new Uint8Array(await crypto.subtle.sign("HMAC", k, mac));
  }
  return Array.from(mac, (x) => x.toString(16).padStart(2, "0")).join("");
}

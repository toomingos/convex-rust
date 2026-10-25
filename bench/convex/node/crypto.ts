"use node";
// Crypto cases (crypto-*). Each case exports the pure implementation
// (`crypto001`, used by the phase-0 runner) and the Convex action (`crypto_001`).
// All keys, salts, nonces and messages come from the shared PRNG (seeded by
// args.seed) so Node and Rust produce identical bytes. The one exception is the
// RSA key in crypto-003: key generation cannot be seeded, so a fixed 2048-bit
// test key (also embedded in rust/src/crypto.rs) is used.
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  createPrivateKey,
  createPublicKey,
  sign as edSign,
  verify as edVerify,
} from "node:crypto";
import { importSPKI, jwtVerify, SignJWT } from "jose";
import argon2 from "argon2";
import bcrypt from "bcrypt";
import { v4 as uuidv4, v7 as uuidv7 } from "uuid";
import { seedAction, TEN_MB } from "./_case.ts";
import { Prng } from "../../shared/prng.ts";


/** Fixed RSA-2048 test key (crypto-003 only). Not a secret. */
const RSA_PRIVATE_PEM = `-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC/2SS1gepgjkeP
ljs1LlcrY/NIB+71q3XXWbrorXCKu1SB87MJrF9iS65AHe5B7HVoPeX7CnrIBgaM
Ueq2LsITl3WVNfFT0C1iz97aOH3b7Td3DOFRuINdoaPiNTLdrcqaYT4noYw4QWe9
UYkbuxxF0e7UfJAZB4HNlY5AnP8/znS6LKcJdf2+cRerM7m2f2me3cLY6f7lJPDR
klulZMIp19l5/CqXJML12lYO+LizQZuxu3oxkJ9QtCrf2rfDzcxncl1UYLF8b/q1
e68yoFt0YugzjsdHaxh+8Qijidnw9gUhO8asoyQ+XSTU5uFMAcQBkMi54oXFGNNi
KWMyOacBAgMBAAECggEAVzudsPiPQXNAf74gUuis0zqB6v1MfyGFFjQjRFxkwXg3
j1Vn0dyyjGf8JgRWC1zA7/LBSXp05NG0lYZD8q3lKnJikUOzZVhR4s3BN7mbJO7C
mxvcskNzXBY9BgUJiXnWF4di8zNGJmg7Cv7xmI3mZ6ry7WRsQy1CLOA1jBS3DWQA
dobIUY8I4CixywEXDIv+05wTwQqop0E8XBxB5WRxfsZ8rlJz6CiDWsBP1egR+L7x
Ei0Cj+RnYiwi+mmgdGWRg4ftyS6xgR+FO0pyWbfnQXZGLq8mSoe7mzC/y5aNublV
K/ceqRFDjlo89jXw9xySmpJW2NvYTwI1Srvds+R6nwKBgQD6CoigbWMm/HDTPopV
uzMYvk1+s3kcO2oDX5RVXdHzPXdM8XkVKf+CIU43wzmWLXf0CFncxFvstNWj0s5u
Po5uvyIJJFpTpw7CNxtwXWqZRhlgvW88VmrRBloNfiCPrOSYMCWI6cbS7GWHk2Jc
wPqS/maxbtskd16WnTKCcX5dfwKBgQDEa5UseqKnIzwdegInbJ0uhZsDrz7zM3Da
Ub3aAYEhJvC/1IsWm0uzdoL20uWnC4muYx8Fj2fp0imgZt84pM4xLZLvO4LI5JtN
0UijCK7+5I11kTcwYWgSSVDti0k1pU1Mtx0vkDgDpVwB4lQlYgYtB4DUA0/F3A34
BKRVk4M7fwKBgB2p6EGYGJ5EbqIxFjpLhilWUl1Ir4KQ7efOjAuAoOrJjQC82Q+Q
tvZaOyxUZd2SMVvrLbYKOZmgboQL+1+K3lz3IfORs4GfHRofbYrsGjxq25Hlh2/S
FMQNi9tz7ir9aPVBKQLQMT1OvFDURGQapN+RfN9p9meiHb+ikJ53FnXDAoGBAIyK
HZZRuQe14lGE9v2WbXDYD+42+eexqtHLIvhVkztRTXk0bsCiJSwnY9WPWNqJGt43
9f5YeZWH1lj6wkUZRp9RaIzcG9v6FlU7palhkTS3+69fVAm96GUNKGaO/rbvq7t0
WeGqYynYq1h9xI9q79hQQhFwvSdpKGWJjLvebEupAoGAcGmRn+r6KQfVCuJOZUaz
2T88MADo16cVPDwbBQcrp7fTHF6L+648rg1nuYvRAtiQ18pjsfsJoEaPmvXlhvGW
mqcP72lkFutdpGwWkvjvizPyTrO722s1H7wrzDeKnA5JEE7bbNDtVWSkBqIBsu18
iXwZCHU0OIc636cUrvIcUao=
-----END PRIVATE KEY-----
`;

const RSA_PUBLIC_PEM = `-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAv9kktYHqYI5Hj5Y7NS5X
K2PzSAfu9at111m66K1wirtUgfOzCaxfYkuuQB3uQex1aD3l+wp6yAYGjFHqti7C
E5d1lTXxU9AtYs/e2jh92+03dwzhUbiDXaGj4jUy3a3KmmE+J6GMOEFnvVGJG7sc
RdHu1HyQGQeBzZWOQJz/P850uiynCXX9vnEXqzO5tn9pnt3C2On+5STw0ZJbpWTC
KdfZefwqlyTC9dpWDvi4s0Gbsbt6MZCfULQq39q3w83MZ3JdVGCxfG/6tXuvMqBb
dGLoM47HR2sYfvEIo4nZ8PYFITvGrKMkPl0k1ObhTAHEAZDIueKFxRjTYiljMjmn
AQIDAQAB
-----END PUBLIC KEY-----
`;

/** crypto-001: HMAC-SHA256 ×100k, chained (each MAC is the next message). */
export async function crypto001(_ctx: unknown, args: { seed: number }): Promise<string> {
  const p = new Prng(args.seed);
  const key = p.bytes(32);
  let mac: Uint8Array = p.bytes(64);
  for (let i = 0; i < 100_000; i++) {
    mac = createHmac("sha256", key).update(mac).digest();
  }
  return Buffer.from(mac).toString("hex");
}
export const crypto_001 = seedAction(crypto001);

/** crypto-002: sign 10k HS256 JWTs; return sha256 over all tokens + last token. */
export async function crypto002(_ctx: unknown, args: { seed: number }) {
  const secret = new Prng(args.seed).bytes(32);
  const all = createHash("sha256");
  let last = "";
  for (let i = 0; i < 10_000; i++) {
    last = await new SignJWT({ sub: `user-${i}`, n: i, iat: 1700000000, exp: 4102444800 })
      .setProtectedHeader({ alg: "HS256" })
      .sign(secret);
    all.update(last);
  }
  return { digest: all.digest("hex"), last };
}
export const crypto_002 = seedAction(crypto002);

// Imported keys are cached across calls (module level), like the convex-rust wasm host caches its CryptoKeys.
let rsaPrivateKey: ReturnType<typeof createPrivateKey> | undefined;
let rsaPublicKey: ReturnType<typeof importSPKI> | undefined;

/** crypto-003: sign one RS256 token (setup), then verify it 1k times. */
export async function crypto003(_ctx: unknown, _args: { seed: number }) {
  const token = await new SignJWT({ sub: "user-1", iat: 1700000000, exp: 4102444800 })
    .setProtectedHeader({ alg: "RS256" })
    .sign((rsaPrivateKey ??= createPrivateKey(RSA_PRIVATE_PEM)));
  const key = await (rsaPublicKey ??= importSPKI(RSA_PUBLIC_PEM, "RS256"));
  let valid = 0;
  let sub = "";
  for (let i = 0; i < 1000; i++) {
    const r = await jwtVerify(token, key, { algorithms: ["RS256"] });
    valid++;
    sub = r.payload.sub as string;
  }
  return { token, valid, sub };
}
export const crypto_003 = seedAction(crypto003);

/** crypto-004: AES-256-GCM encrypt 10 MB; return tag, ciphertext head and length. */
export async function crypto004(_ctx: unknown, args: { seed: number }) {
  const p = new Prng(args.seed);
  const key = p.bytes(32);
  const iv = p.bytes(12);
  const data = p.bytes(TEN_MB);
  const c = createCipheriv("aes-256-gcm", key, iv);
  const ct = Buffer.concat([c.update(data), c.final()]);
  return {
    tag: c.getAuthTag().toString("hex"),
    head: ct.subarray(0, 32).toString("hex"),
    len: ct.length,
  };
}
export const crypto_004 = seedAction(crypto004);

/** crypto-005: AES-256-GCM encrypt (setup) then decrypt + authenticate 10 MB. */
export async function crypto005(_ctx: unknown, args: { seed: number }) {
  const p = new Prng(args.seed);
  const key = p.bytes(32);
  const iv = p.bytes(12);
  const data = p.bytes(TEN_MB);
  const c = createCipheriv("aes-256-gcm", key, iv);
  const ct = Buffer.concat([c.update(data), c.final()]);
  const tag = c.getAuthTag();
  const d = createDecipheriv("aes-256-gcm", key, iv);
  d.setAuthTag(tag);
  const pt = Buffer.concat([d.update(ct), d.final()]);
  return { match: Buffer.compare(pt, data) === 0, len: pt.length };
}
export const crypto_005 = seedAction(crypto005);

const ED25519_PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

function edKeys(seed32: Uint8Array) {
  const priv = createPrivateKey({
    key: Buffer.concat([ED25519_PKCS8_PREFIX, seed32]),
    format: "der",
    type: "pkcs8",
  });
  return { priv, pub: createPublicKey(priv) };
}

/** crypto-006: Ed25519 sign 10k 64-byte messages; return digest of all signatures. */
export async function crypto006(_ctx: unknown, args: { seed: number }) {
  const p = new Prng(args.seed);
  const { priv } = edKeys(p.bytes(32));
  const all = createHash("sha256");
  let first = "";
  let last = "";
  for (let i = 0; i < 10_000; i++) {
    const sig = edSign(null, p.bytes(64), priv);
    all.update(sig);
    if (i === 0) first = sig.toString("hex");
    last = sig.toString("hex");
  }
  return { digest: all.digest("hex"), first, last };
}
export const crypto_006 = seedAction(crypto006);

/** crypto-007: sign 64 messages (setup), then verify 10k (every 100th signature corrupted). */
export async function crypto007(_ctx: unknown, args: { seed: number }) {
  const p = new Prng(args.seed);
  const { priv, pub } = edKeys(p.bytes(32));
  const msgs: Uint8Array[] = [];
  const sigs: Buffer[] = [];
  for (let i = 0; i < 64; i++) {
    const m = p.bytes(64);
    msgs.push(m);
    sigs.push(edSign(null, m, priv));
  }
  let valid = 0;
  for (let i = 0; i < 10_000; i++) {
    let sig = sigs[i % 64];
    if (i % 100 === 0) {
      sig = Buffer.from(sig);
      sig[0] ^= 1;
    }
    if (edVerify(null, msgs[i % 64], pub, sig)) valid++;
  }
  return { valid, total: 10_000 };
}
export const crypto_007 = seedAction(crypto007);

/** crypto-008: argon2id m=19456 KiB (19 MiB), t=2, p=1, 16-byte PRNG salt, 32-byte raw hash. */
export async function crypto008(_ctx: unknown, args: { seed: number }) {
  const p = new Prng(args.seed);
  const password = p.word(16);
  const salt = Buffer.from(p.bytes(16));
  const raw = await argon2.hash(password, {
    type: argon2.argon2id,
    memoryCost: 19456,
    timeCost: 2,
    parallelism: 1,
    hashLength: 32,
    salt,
    raw: true,
  });
  return { hash: raw.toString("hex"), m: 19456, t: 2, p: 1 };
}
export const crypto_008 = seedAction(crypto008);

const BCRYPT_B64 = "./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/** bcrypt's base64 variant, 16 bytes -> 22 chars (no padding). */
function bcryptSaltB64(b: Uint8Array): string {
  let out = "";
  let i = 0;
  while (i < b.length) {
    let c1 = b[i++];
    out += BCRYPT_B64[c1 >> 2];
    c1 = (c1 & 3) << 4;
    if (i >= b.length) {
      out += BCRYPT_B64[c1];
      break;
    }
    let c2 = b[i++];
    c1 |= c2 >> 4;
    out += BCRYPT_B64[c1];
    c1 = (c2 & 0x0f) << 2;
    if (i >= b.length) {
      out += BCRYPT_B64[c1];
      break;
    }
    c2 = b[i++];
    c1 |= c2 >> 6;
    out += BCRYPT_B64[c1];
    out += BCRYPT_B64[c2 & 0x3f];
  }
  return out;
}

/** crypto-009: bcrypt cost 10 with a PRNG-derived salt. */
export async function crypto009(_ctx: unknown, args: { seed: number }) {
  const p = new Prng(args.seed);
  const password = p.word(16);
  const salt = `$2b$10$${bcryptSaltB64(p.bytes(16))}`;
  const hash = await bcrypt.hash(password, salt);
  return { hash, cost: 10 };
}
export const crypto_009 = seedAction(crypto009);

/** crypto-010: 100k UUID v4 and 100k UUID v7 built from PRNG bytes. */
export async function crypto010(_ctx: unknown, args: { seed: number }) {
  const N = 100_000;
  const rnd = new Prng(args.seed).bytes(N * 32);
  const seen4 = new Set<string>();
  const seen7 = new Set<string>();
  let last4 = "";
  let last7 = "";
  let sorted = true;
  for (let i = 0; i < N; i++) {
    last4 = uuidv4({ random: rnd.subarray(i * 32, i * 32 + 16) });
    seen4.add(last4);
    const u7 = uuidv7({
      random: rnd.subarray(i * 32 + 16, i * 32 + 32),
      msecs: 1_700_000_000_000 + i,
      seq: i,
    });
    if (u7 <= last7) sorted = false;
    last7 = u7;
    seen7.add(u7);
  }
  return { unique4: seen4.size, unique7: seen7.size, sorted7: sorted, last4, last7 };
}
export const crypto_010 = seedAction(crypto010);

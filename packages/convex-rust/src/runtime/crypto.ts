// Host crypto sync ops (contracts/ABI.md "Optional sync ops", crypto version 2): `crypto/caps`,
// `crypto/verify`, `crypto/sign`, `crypto/aead`, `crypto/digest`, `crypto/hmac`, on WebCrypto
// (`crypto.subtle`; async, so the caller's JSPI `Suspending` makes them look synchronous to the guest).
//
// Byte arguments are either a base64 string or a guest-memory reference `{ptr, len}` that is read
// straight out of the guest's linear memory (no copy through JSON/base64). `crypto/aead` transforms
// its `data` reference in place.
import { decodeBase64, encodeBase64 } from "./base64.js";

type Json = any;

export const CRYPTO_VERSION = 2;

const HASHES: Record<string, { name: string; len: number }> = {
  "256": { name: "SHA-256", len: 32 },
  "384": { name: "SHA-384", len: 48 },
  "512": { name: "SHA-512", len: 64 },
};
const DIGESTS = ["SHA-1", "SHA-256", "SHA-384", "SHA-512"];
const AEADS = ["A128GCM", "A256GCM"];
const AEAD_KEY_LEN: Record<string, number> = { A128GCM: 16, A256GCM: 32 };
const TAG_LEN = 16;
const NONCE_LEN = 12;
/** RSA modulus policy, identical on every host (the wasmtime host's aws-lc-rs limits). */
const RSA_MIN_BITS = 2048;
const RSA_MAX_BITS = 8192;

interface SigSpec {
  /** importKey algorithm */
  import: any;
  /** sign/verify algorithm */
  op: any;
  /** key format for verify keys: "jwk" (PKCS#1 -> JWK) or "raw" */
  pub: "jwk" | "raw";
  rsa: boolean;
}
const SIG_ALGS: Record<string, SigSpec> = {};
for (const [bits, { name: hash, len }] of Object.entries(HASHES)) {
  SIG_ALGS["RS" + bits] = {
    import: { name: "RSASSA-PKCS1-v1_5", hash },
    op: { name: "RSASSA-PKCS1-v1_5" },
    pub: "jwk",
    rsa: true,
  };
  SIG_ALGS["PS" + bits] = {
    import: { name: "RSA-PSS", hash },
    op: { name: "RSA-PSS", saltLength: len },
    pub: "jwk",
    rsa: true,
  };
}
SIG_ALGS.ES256 = {
  import: { name: "ECDSA", namedCurve: "P-256" },
  op: { name: "ECDSA", hash: "SHA-256" },
  pub: "raw",
  rsa: false,
};
SIG_ALGS.ES384 = {
  import: { name: "ECDSA", namedCurve: "P-384" },
  op: { name: "ECDSA", hash: "SHA-384" },
  pub: "raw",
  rsa: false,
};
SIG_ALGS.EdDSA = { import: { name: "Ed25519" }, op: { name: "Ed25519" }, pub: "raw", rsa: false };

const subtleOf = (): any => (globalThis as any).crypto?.subtle;

// ---- minimal DER ----
interface Tlv {
  tag: number;
  start: number;
  end: number;
}
function tlv(der: Uint8Array, pos: number): Tlv {
  const tag = der[pos];
  let b = der[pos + 1];
  if (tag === undefined || b === undefined) throw new Error("malformed DER");
  let p = pos + 2;
  let len = b;
  if (b >= 0x80) {
    const n = b & 0x7f;
    if (n === 0 || n > 4) throw new Error("malformed DER");
    len = 0;
    for (let i = 0; i < n; i++) len = len * 256 + (der[p++] ?? 0);
  }
  if (p + len > der.length) throw new Error("malformed DER");
  return { tag, start: p, end: p + len };
}

/** (modulus, exponent) of a PKCS#1 RSAPublicKey DER. */
function parseRsaPublicKey(der: Uint8Array): { n: Uint8Array; e: Uint8Array } {
  const seq = tlv(der, 0);
  if (seq.tag !== 0x30) throw new Error("bad RSA key");
  const n = tlv(der, seq.start);
  const e = tlv(der, n.end);
  if (n.tag !== 0x02 || e.tag !== 0x02) throw new Error("bad RSA key");
  const strip = (t: Tlv) => {
    let v = der.subarray(t.start, t.end);
    while (v.length > 1 && v[0] === 0) v = v.subarray(1);
    return v;
  };
  return { n: strip(n), e: strip(e) };
}

function bitLength(v: Uint8Array): number {
  return v.length === 0 ? 0 : (v.length - 1) * 8 + (32 - Math.clz32(v[0]));
}

/** Modulus bit length of a PKCS#8 PrivateKeyInfo wrapping an RSAPrivateKey. */
function pkcs8RsaBits(der: Uint8Array): number {
  const top = tlv(der, 0);
  const version = tlv(der, top.start);
  const algId = tlv(der, version.end);
  const octets = tlv(der, algId.end);
  const inner = tlv(der, octets.start);
  const ver = tlv(der, inner.start);
  const n = tlv(der, ver.end);
  let v = der.subarray(n.start, n.end);
  while (v.length > 1 && v[0] === 0) v = v.subarray(1);
  return bitLength(v);
}

function b64url(b: Uint8Array): string {
  return encodeBase64(b).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// ---- argument access ----
type Mem = () => WebAssembly.Memory;

function bytes(mem: Mem, v: Json, field: string): Uint8Array {
  if (typeof v === "string") return decodeBase64(v);
  if (v && typeof v === "object" && Number.isInteger(v.ptr) && Number.isInteger(v.len)) {
    const buf = mem().buffer;
    const ptr = v.ptr >>> 0;
    const len = v.len >>> 0;
    if (ptr + len > buf.byteLength) throw new Error(`${field}: out of bounds`);
    return new Uint8Array(buf, ptr, len);
  }
  throw new Error(`missing ${field}`);
}

function latin1(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 8192) s += String.fromCharCode(...b.subarray(i, i + 8192));
  return s;
}

// ---- key cache (per isolate; keys are immutable and small) ----
const keys = new Map<string, Promise<CryptoKey>>();
function cached(id: string, make: () => Promise<CryptoKey>): Promise<CryptoKey> {
  let k = keys.get(id);
  if (!k) {
    if (keys.size >= 256) keys.clear();
    k = make();
    k.catch(() => keys.delete(id)); // do not cache failures
    keys.set(id, k);
  }
  return k;
}

// ---- caps ----
let caps: Promise<Json> | undefined;
// RFC 8032 section 7.1 test 1 public key: a valid point, unlike an arbitrary 32-byte string.
const ED25519_PROBE_KEY = new Uint8Array([
  0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
  0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
]);
function probe(): Promise<Json> {
  return (caps ??= (async () => {
    const subtle = subtleOf();
    if (!subtle) return { error: { message: "crypto.subtle unavailable" } };
    const sigs = Object.keys(SIG_ALGS);
    try {
      // EdDSA is the only family not guaranteed everywhere; prove it before advertising it.
      const key = await subtle.importKey("raw", ED25519_PROBE_KEY, { name: "Ed25519" }, false, ["verify"]);
      await subtle.verify({ name: "Ed25519" }, key, new Uint8Array(64), new Uint8Array(1));
    } catch {
      sigs.splice(sigs.indexOf("EdDSA"), 1);
    }
    return {
      ok: {
        version: CRYPTO_VERSION,
        ops: {
          verify: sigs,
          sign: sigs,
          pubkey: sigs.filter((a) => !SIG_ALGS[a].rsa),
          aead: AEADS,
          digest: DIGESTS,
          hmac: DIGESTS,
        },
      },
    };
  })());
}

// ---- ops ----
function sigSpec(op: string, alg: any): SigSpec {
  const spec = typeof alg === "string" && Object.hasOwn(SIG_ALGS, alg) ? SIG_ALGS[alg] : undefined;
  if (!spec) throw new Error(`unsupported alg ${alg}`);
  return spec;
}

async function verify(args: Json, mem: Mem): Promise<Json> {
  const spec = sigSpec("verify", args.alg);
  const subtle = subtleOf();
  const keyBytes = bytes(mem, args.key, "key");
  const id = `v:${args.alg}:${latin1(keyBytes)}`;
  const key = await cached(id, () => {
    if (spec.pub === "raw") return subtle.importKey("raw", keyBytes.slice(), spec.import, false, ["verify"]);
    const { n, e } = parseRsaPublicKey(keyBytes);
    const bits = bitLength(n);
    if (bits < RSA_MIN_BITS || bits > RSA_MAX_BITS) throw new Error(`unsupported RSA key size ${bits}`);
    return subtle.importKey("jwk", { kty: "RSA", n: b64url(n), e: b64url(e), ext: true }, spec.import, false, ["verify"]);
  });
  const ok = await subtle.verify(spec.op, key, bytes(mem, args.sig, "sig"), bytes(mem, args.msg, "msg"));
  return { ok: ok === true };
}

async function sign(args: Json, mem: Mem): Promise<Json> {
  const spec = sigSpec("sign", args.alg);
  const subtle = subtleOf();
  const keyBytes = bytes(mem, args.key, "key");
  const id = `s:${args.alg}:${latin1(keyBytes)}`;
  const key = await cached(id, () => {
    if (spec.rsa) {
      const bits = pkcs8RsaBits(keyBytes);
      if (bits < RSA_MIN_BITS || bits > RSA_MAX_BITS) throw new Error(`unsupported RSA key size ${bits}`);
    }
    return subtle.importKey("pkcs8", keyBytes.slice(), spec.import, false, ["sign"]);
  });
  const sig = await subtle.sign(spec.op, key, bytes(mem, args.msg, "msg"));
  return { ok: encodeBase64(new Uint8Array(sig)) };
}

function b64urlDecode(s: string): Uint8Array {
  return decodeBase64(s.replace(/-/g, "+").replace(/_/g, "/"));
}

async function pubkey(args: Json, mem: Mem): Promise<Json> {
  const spec = sigSpec("pubkey", args.alg);
  if (spec.rsa) throw new Error(`unsupported alg ${args.alg}`);
  const subtle = subtleOf();
  const keyBytes = bytes(mem, args.key, "key");
  // The private key is imported extractable only here (never cached), to read the public half.
  const key = await subtle.importKey("pkcs8", keyBytes.slice(), spec.import, true, ["sign"]);
  const jwk = await subtle.exportKey("jwk", key);
  if (args.alg === "EdDSA") return { ok: encodeBase64(b64urlDecode(jwk.x)) };
  const x = b64urlDecode(jwk.x);
  const y = b64urlDecode(jwk.y);
  const out = new Uint8Array(1 + x.length + y.length);
  out[0] = 4;
  out.set(x, 1);
  out.set(y, 1 + x.length);
  return { ok: encodeBase64(out) };
}

async function aead(args: Json, mem: Mem): Promise<Json> {
  const alg = args.alg;
  if (!AEADS.includes(alg)) throw new Error(`unsupported alg ${alg}`);
  const dir = args.dir;
  if (dir !== "seal" && dir !== "open") throw new Error(`bad dir ${dir}`);
  const subtle = subtleOf();
  const keyBytes = bytes(mem, args.key, "key");
  if (keyBytes.length !== AEAD_KEY_LEN[alg]) throw new Error(`${alg} needs a ${AEAD_KEY_LEN[alg]}-byte key`);
  const nonce = bytes(mem, args.nonce, "nonce");
  if (nonce.length !== NONCE_LEN) throw new Error(`nonce must be ${NONCE_LEN} bytes`);
  const key = await cached(`a:${latin1(keyBytes)}`, () =>
    subtle.importKey("raw", keyBytes.slice(), { name: "AES-GCM" }, false, ["encrypt", "decrypt"]),
  );
  const params: any = { name: "AES-GCM", iv: nonce, tagLength: TAG_LEN * 8 };
  if (args.aad !== undefined && args.aad !== null) {
    const aad = bytes(mem, args.aad, "aad");
    if (aad.length > 0) params.additionalData = aad;
  }
  const data = bytes(mem, args.data, "data");
  if (dir === "seal") {
    const out = new Uint8Array(await subtle.encrypt(params, key, data));
    const n = out.length - TAG_LEN;
    bytes(mem, args.data, "data").set(out.subarray(0, n));
    return { ok: { tag: encodeBase64(out.subarray(n)) } };
  }
  const tag = bytes(mem, args.tag, "tag");
  if (tag.length !== TAG_LEN) throw new Error(`tag must be ${TAG_LEN} bytes`);
  const input = new Uint8Array(data.length + TAG_LEN);
  input.set(data);
  input.set(tag, data.length);
  let pt: Uint8Array;
  try {
    pt = new Uint8Array(await subtle.decrypt(params, key, input));
  } catch {
    return { ok: false }; // authentication failure; `data` is left untouched
  }
  bytes(mem, args.data, "data").set(pt);
  return { ok: true };
}

async function digest(args: Json, mem: Mem): Promise<Json> {
  if (!DIGESTS.includes(args.alg)) throw new Error(`unsupported alg ${args.alg}`);
  const out = await subtleOf().digest(args.alg, bytes(mem, args.data, "data"));
  return { ok: encodeBase64(new Uint8Array(out)) };
}

async function hmac(args: Json, mem: Mem): Promise<Json> {
  if (!DIGESTS.includes(args.alg)) throw new Error(`unsupported alg ${args.alg}`);
  const subtle = subtleOf();
  let keyBytes = bytes(mem, args.key, "key");
  // HMAC zero-pads the key to the block size, so an empty key equals one zero byte;
  // WebCrypto rejects zero-length HMAC keys.
  if (keyBytes.length === 0) keyBytes = new Uint8Array(1);
  const key = await cached(`h:${args.alg}:${latin1(keyBytes)}`, () =>
    subtle.importKey("raw", keyBytes.slice(), { name: "HMAC", hash: args.alg }, false, ["sign"]),
  );
  const out = await subtle.sign("HMAC", key, bytes(mem, args.data, "data"));
  return { ok: encodeBase64(new Uint8Array(out)) };
}

// ---- synchronous SHA-256 / HMAC-SHA256 for small inputs ----
// A WebCrypto call costs a promise + a JSPI suspend/resume (~15-20 us), which dominates tiny inputs
// (100k chained HMACs). Inputs up to FAST_MAX bytes are hashed here in plain JS and answered without
// suspending; larger ones go to crypto.subtle.
const FAST_MAX = 1024;
const K256 = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
  0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
  0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
  0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
  0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
  0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
const W256 = new Uint32Array(64);
function sha256(data: Uint8Array): Uint8Array {
  const h = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
  const n = data.length;
  const total = (n + 9 + 63) & ~63;
  const m = new Uint8Array(total);
  m.set(data);
  m[n] = 0x80;
  const dv = new DataView(m.buffer);
  dv.setUint32(total - 8, Math.floor((n * 8) / 4294967296));
  dv.setUint32(total - 4, (n * 8) >>> 0);
  const w = W256;
  for (let off = 0; off < total; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4);
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15], b = w[i - 2];
      const s0 = ((a >>> 7) | (a << 25)) ^ ((a >>> 18) | (a << 14)) ^ (a >>> 3);
      const s1 = ((b >>> 17) | (b << 15)) ^ ((b >>> 19) | (b << 13)) ^ (b >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const t1 = (hh + (((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7))) + ((e & f) ^ (~e & g)) + K256[i] + w[i]) | 0;
      const t2 = ((((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10))) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
      hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
  }
  const out = new Uint8Array(32);
  const odv = new DataView(out.buffer);
  for (let i = 0; i < 8; i++) odv.setUint32(i * 4, h[i]);
  return out;
}
function hmacSha256(key: Uint8Array, data: Uint8Array): Uint8Array {
  const k = new Uint8Array(64);
  k.set(key.length > 64 ? sha256(key) : key);
  const inner = new Uint8Array(64 + data.length);
  const outer = new Uint8Array(96);
  for (let i = 0; i < 64; i++) {
    inner[i] = k[i] ^ 0x36;
    outer[i] = k[i] ^ 0x5c;
  }
  inner.set(data, 64);
  outer.set(sha256(inner), 64);
  return sha256(outer);
}
/** The synchronous answer for a small SHA-256 digest / HMAC, else undefined (use WebCrypto). */
function fast(op: string, args: Json, mem: Mem): Json | undefined {
  if (args?.alg !== "SHA-256" || (op !== "crypto/digest" && op !== "crypto/hmac")) return undefined;
  try {
    const data = bytes(mem, args.data, "data");
    if (data.length > FAST_MAX) return undefined;
    if (op === "crypto/digest") return { ok: encodeBase64(sha256(data)) };
    const key = bytes(mem, args.key, "key");
    return key.length > FAST_MAX ? undefined : { ok: encodeBase64(hmacSha256(key, data)) };
  } catch (e: any) {
    return { error: { message: `${op}: ${e?.message ?? e}` } };
  }
}

const OPS: Record<string, (args: Json, mem: Mem) => Promise<Json>> = {
  "crypto/verify": verify,
  "crypto/sign": sign,
  "crypto/pubkey": pubkey,
  "crypto/aead": aead,
  "crypto/digest": digest,
  "crypto/hmac": hmac,
};

/** Serves one `crypto/*` sync op. `mem` returns the guest's linear memory (for `{ptr, len}` byte arguments). */
/** Answers synchronously (no suspend) for small SHA-256 digest / HMAC, else with a promise. */
export function cryptoOp(op: string, args: Json, mem: Mem): Json | Promise<Json> {
  const quick = fast(op, args, mem);
  return quick !== undefined ? quick : cryptoAsync(op, args, mem);
}

async function cryptoAsync(op: string, args: Json, mem: Mem): Promise<Json> {
  if (op === "crypto/caps") return probe();
  const f = Object.hasOwn(OPS, op) ? OPS[op] : undefined;
  if (!f) return { error: { message: `unknown sync op ${op}` } };
  if (!subtleOf()) return { error: { message: `${op}: crypto.subtle unavailable` } };
  try {
    return await f(args, mem);
  } catch (e: any) {
    return { error: { message: `${op}: ${e?.message ?? e}` } };
  }
}

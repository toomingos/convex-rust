// crypto/* sync ops (contracts/ABI.md "Optional sync ops", crypto version 2): the WebCrypto host in
// src/runtime/crypto.ts against node:crypto and known answers, plus the real bench guest when its wasm is built.
import { createHash, createHmac, createSign, generateKeyPairSync, sign as nodeSign } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { cryptoOp } from "../src/runtime/crypto.js";
import { runRust } from "../src/runtime/index.js";

const b = (s: string) => new Uint8Array(Buffer.from(s, "base64"));
const hex = (s: string) => new Uint8Array(Buffer.from(s, "hex"));
const utf8 = (s: string) => new TextEncoder().encode(s);
const toHex = (x: Uint8Array) => Buffer.from(x).toString("hex");

function pair(type: "rsa" | "ec" | "ed25519", opt: object = {}) {
  const { privateKey, publicKey } = generateKeyPairSync(type as any, opt as any) as { privateKey: any; publicKey: any };
  return { privateKey, publicKey, pk8: new Uint8Array(privateKey.export({ type: "pkcs8", format: "der" })) };
}
const rsa = pair("rsa", { modulusLength: 2048 });
const rsaPub = new Uint8Array(rsa.publicKey.export({ type: "pkcs1", format: "der" }));
const p256 = pair("ec", { namedCurve: "P-256" });
const p384 = pair("ec", { namedCurve: "P-384" });
const ed = pair("ed25519");
/** SEC1 uncompressed point from an EC public key. */
const point = (k: any) => {
  const j = k.export({ format: "jwk" });
  return new Uint8Array(Buffer.concat([Buffer.from([4]), Buffer.from(j.x, "base64url"), Buffer.from(j.y, "base64url")]));
};
const edPub = (k: any) => new Uint8Array(Buffer.from(k.export({ format: "jwk" }).x, "base64url"));

/** A scratch guest memory; `ref` copies bytes in and returns the {ptr, len} the guest would pass. */
function guest() {
  const memory = new WebAssembly.Memory({ initial: 4 });
  let top = 16;
  const ref = (bytes: Uint8Array) => {
    const ptr = top;
    new Uint8Array(memory.buffer).set(bytes, ptr);
    top += bytes.length;
    return { ptr, len: bytes.length };
  };
  const read = (r: { ptr: number; len: number }) => new Uint8Array(memory.buffer, r.ptr, r.len).slice();
  const call = (op: string, args: any): Promise<any> => cryptoOp(op, args, () => memory) as Promise<any>;
  return { ref, read, call };
}

describe("host crypto ops (WebCrypto)", () => {
  it("advertises version 2 ops and algorithms", async () => {
    const r = await guest().call("crypto/caps", { version: 2 });
    expect(r.ok.version).toBe(2);
    expect(Object.keys(r.ok.ops).sort()).toEqual(["aead", "digest", "hmac", "pubkey", "sign", "verify"]);
    expect(r.ok.ops.verify).toEqual(expect.arrayContaining(["RS256", "PS512", "ES384", "EdDSA"]));
    expect(r.ok.ops.aead).toEqual(["A128GCM", "A256GCM"]);
    expect(r.ok.ops.digest).toEqual(["SHA-1", "SHA-256", "SHA-384", "SHA-512"]);
  });

  it("unknown op and bad alg are errors", async () => {
    const g = guest();
    expect((await g.call("crypto/nope", {})).error.message).toContain("unknown sync op");
    expect((await g.call("crypto/verify", { alg: "XX" })).error.message).toContain("unsupported alg");
    expect((await g.call("crypto/digest", { alg: "MD5", data: g.ref(utf8("x")) })).error).toBeDefined();
  });

  it("sign then verify round-trips for every algorithm and rejects another message", async () => {
    const g = guest();
    const msg = utf8("hello host crypto");
    const cases: [string[], Uint8Array, Uint8Array][] = [
      [["RS256", "RS384", "RS512", "PS256", "PS384", "PS512"], rsa.pk8, rsaPub],
      [["ES256"], p256.pk8, point(p256.publicKey)],
      [["ES384"], p384.pk8, point(p384.publicKey)],
      [["EdDSA"], ed.pk8, edPub(ed.publicKey)],
    ];
    for (const [algs, pk8, pub] of cases) {
      for (const alg of algs) {
        const s = await g.call("crypto/sign", { alg, key: g.ref(pk8), msg: g.ref(msg) });
        expect(typeof s.ok, alg + JSON.stringify(s)).toBe("string");
        const sig = b(s.ok);
        const v = (m: Uint8Array) => g.call("crypto/verify", { alg, key: g.ref(pub), msg: g.ref(m), sig: g.ref(sig) });
        expect(await v(msg), alg).toEqual({ ok: true });
        expect(await v(utf8("other")), alg).toEqual({ ok: false });
      }
    }
  });

  it("RS256 signatures match node:crypto and node verifies PS256 / ES256 from the host", async () => {
    const g = guest();
    const msg = utf8("cross check");
    const rs = await g.call("crypto/sign", { alg: "RS256", key: g.ref(rsa.pk8), msg: g.ref(msg) });
    expect(Buffer.from(b(rs.ok)).equals(createSign("RSA-SHA256").update(msg).sign(rsa.privateKey))).toBe(true);
    const es = await g.call("crypto/sign", { alg: "ES256", key: g.ref(p256.pk8), msg: g.ref(msg) });
    const ok = (await import("node:crypto")).verify("sha256", msg, { key: p256.publicKey, dsaEncoding: "ieee-p1363" }, b(es.ok));
    expect(ok).toBe(true);
    const sigEd = nodeSign(null, msg, ed.privateKey);
    const v = await g.call("crypto/verify", { alg: "EdDSA", key: g.ref(edPub(ed.publicKey)), msg: g.ref(msg), sig: g.ref(sigEd) });
    expect(v).toEqual({ ok: true });
  });

  it("verifies RFC 8032 Ed25519 test 1 and rejects unusable keys", async () => {
    const g = guest();
    const key = g.ref(hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"));
    const sig = g.ref(
      hex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"),
    );
    expect(await g.call("crypto/verify", { alg: "EdDSA", key, msg: g.ref(new Uint8Array(0)), sig })).toEqual({ ok: true });
    const bad = await g.call("crypto/verify", { alg: "RS256", key: g.ref(point(p256.publicKey)), msg: g.ref(utf8("m")), sig: g.ref(utf8("s")) });
    expect(bad.error).toBeDefined();
  });

  it("applies the 2048..8192 RSA key policy", async () => {
    const g = guest();
    const small = pair("rsa", { modulusLength: 1024 });
    const pub = new Uint8Array(small.publicKey.export({ type: "pkcs1", format: "der" }));
    const r = await g.call("crypto/verify", { alg: "RS256", key: g.ref(pub), msg: g.ref(utf8("m")), sig: g.ref(new Uint8Array(128)) });
    expect(r.error.message).toContain("1024");
  });

  it("derives public keys", async () => {
    const g = guest();
    expect(b((await g.call("crypto/pubkey", { alg: "ES256", key: g.ref(p256.pk8) })).ok)).toEqual(point(p256.publicKey));
    expect(b((await g.call("crypto/pubkey", { alg: "ES384", key: g.ref(p384.pk8) })).ok)).toEqual(point(p384.publicKey));
    expect(b((await g.call("crypto/pubkey", { alg: "EdDSA", key: g.ref(ed.pk8) })).ok)).toEqual(edPub(ed.publicKey));
    expect((await g.call("crypto/pubkey", { alg: "RS256", key: g.ref(rsa.pk8) })).error).toBeDefined();
  });

  it("AES-128-GCM matches NIST test case 4, in place, and refuses a bad tag", async () => {
    const g = guest();
    const key = hex("feffe9928665731c6d6a8f9467308308");
    const iv = hex("cafebabefacedbaddecaf888");
    const aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
    const pt = hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39");
    const ct = hex("42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091");
    const tag = hex("5bc94fbc3221a5db94fae95ae7121a47");
    const data = g.ref(pt);
    const base = { alg: "A128GCM", key: g.ref(key), nonce: g.ref(iv), aad: g.ref(aad), data };
    const sealed = await g.call("crypto/aead", { ...base, dir: "seal" });
    expect(toHex(b(sealed.ok.tag))).toBe(toHex(tag));
    expect(g.read(data)).toEqual(ct);
    const badTag = tag.slice();
    badTag[0] ^= 1;
    expect(await g.call("crypto/aead", { ...base, dir: "open", tag: g.ref(badTag) })).toEqual({ ok: false });
    expect(g.read(data)).toEqual(ct); // untouched on failure
    expect(await g.call("crypto/aead", { ...base, dir: "open", tag: g.ref(tag) })).toEqual({ ok: true });
    expect(g.read(data)).toEqual(pt);
    // the key size must match the algorithm
    expect((await g.call("crypto/aead", { ...base, alg: "A256GCM", dir: "seal" })).error).toBeDefined();
  });

  it("digest and hmac known answers (empty HMAC key too)", async () => {
    const g = guest();
    const d = (alg: string, s: string) => g.call("crypto/digest", { alg, data: g.ref(utf8(s)) });
    expect(toHex(b((await d("SHA-256", "abc")).ok))).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    expect(toHex(b((await d("SHA-1", "abc")).ok))).toBe("a9993e364706816aba3e25717850c26c9cd0d89d");
    const h = (key: Uint8Array, s: string) => g.call("crypto/hmac", { alg: "SHA-256", key: g.ref(key), data: g.ref(utf8(s)) });
    expect(toHex(b((await h(utf8("Jefe"), "what do ya want for nothing?")).ok))).toBe(
      "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
    );
    expect(await h(new Uint8Array(0), "x")).toEqual(await h(new Uint8Array(1), "x"));
  });

  it("SHA-256 / HMAC-SHA256 agree with node:crypto across the sync fast path and WebCrypto sizes", async () => {
    const g = guest();
    for (const n of [0, 1, 55, 56, 63, 64, 65, 119, 120, 1023, 1024, 1025, 5000]) {
      const data = new Uint8Array(n).map((_, i) => i * 7 + n);
      const d = await g.call("crypto/digest", { alg: "SHA-256", data: g.ref(data) });
      expect(toHex(b(d.ok)), `digest ${n}`).toBe(createHash("sha256").update(data).digest("hex"));
      for (const kn of [0, 1, 32, 64, 65, 200, 1500]) {
        const key = new Uint8Array(kn).map((_, i) => i + 3);
        const h = await g.call("crypto/hmac", { alg: "SHA-256", key: g.ref(key), data: g.ref(data) });
        expect(toHex(b(h.ok)), `hmac ${n}/${kn}`).toBe(createHmac("sha256", key).update(data).digest("hex"));
      }
    }
  });

  it("rejects out-of-bounds memory references", async () => {
    const g = guest();
    const r = await g.call("crypto/digest", { alg: "SHA-256", data: { ptr: 4 * 65536 - 4, len: 100 } });
    expect(r.error.message).toContain("out of bounds");
  });

  it("10 MB AES-GCM seal/open and SHA-256 through memory references", async () => {
    const memory = new WebAssembly.Memory({ initial: 400 }); // 25 MiB
    const call = (op: string, args: any): Promise<any> => cryptoOp(op, args, () => memory) as Promise<any>;
    const n = 10 * 1024 * 1024;
    const view = new Uint8Array(memory.buffer);
    for (let i = 0; i < n; i++) view[1024 + i] = i * 31;
    const orig = view.slice(1024, 1024 + n);
    const small = (off: number, bytes: Uint8Array) => (view.set(bytes, off), { ptr: off, len: bytes.length });
    const data = { ptr: 1024, len: n };
    const t0 = performance.now();
    const base = {
      alg: "A256GCM",
      key: small(16, new Uint8Array(32).fill(7)),
      nonce: small(64, new Uint8Array(12).fill(3)),
      aad: small(80, new Uint8Array(0)),
      data,
    };
    const sealed = await call("crypto/aead", { ...base, dir: "seal" });
    const tag = small(96, b(sealed.ok.tag));
    expect(Buffer.from(view.subarray(1024, 1024 + n)).equals(Buffer.from(orig))).toBe(false);
    expect(await call("crypto/aead", { ...base, dir: "open", tag })).toEqual({ ok: true });
    expect(Buffer.from(view.subarray(1024, 1024 + n)).equals(Buffer.from(orig))).toBe(true);
    const dg = await call("crypto/digest", { alg: "SHA-256", data });
    expect(toHex(b(dg.ok))).toBe(createHash("sha256").update(orig).digest("hex"));
    expect(performance.now() - t0).toBeLessThan(5000);
  });
});

const candidates = [
  "../../../target-e2e/wasm32-wasip1/release/bench_rust.wasm",
  "../../../target/wasm32-wasip1/release/bench_rust.wasm",
].map((p) => new URL(p, import.meta.url));
const CRYPTO_003 = "convex_run:bench_rust::crypto:crypto_003";
// A grouped bench build (one feature group per Convex module) leaves only the last group's exports here.
const path = candidates.find(
  (p) =>
    existsSync(p) &&
    WebAssembly.Module.exports(new WebAssembly.Module(readFileSync(p))).some((e) => e.name === CRYPTO_003),
);

describe.skipIf(!path)("host crypto (bench guest)", () => {
  it("crypto-003 verifies 1000 RS256 tokens through WebCrypto", async () => {
    const module = new WebAssembly.Module(readFileSync(path!));
    const ctx: any = {};
    const t = performance.now();
    const r: any = await runRust(ctx, module, CRYPTO_003, {
      seed: 1,
    });
    const ms = performance.now() - t;
    expect(r.valid).toBe(1000);
    expect(r.sub).toBe("user-1");
    // Pure-Rust RSA on wasm takes ~1 s; the host path is far below that.
    expect(ms).toBeLessThan(900);
  }, 60_000);
});

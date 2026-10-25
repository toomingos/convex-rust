//! Host-delegated cryptography for wasm guests (`crypto/*` sync ops, see
//! contracts/ABI.md "Optional sync ops", crypto version 2).
//!
//! A wasm32 guest does not carry its own signature, AEAD, digest or HMAC code:
//! the host runs them natively (WebCrypto in the V8 host, aws-lc-rs in the
//! wasmtime host), which is far faster than bignum/SIMD-less code on wasm32.
//! There is **no pure-Rust fallback**. The guest probes `crypto/caps` once per
//! instance; an op or algorithm the host lacks is a clear error telling the
//! user to update the host, never a silent slow path.
//!
//! The ops ride on the existing `sync_syscall` import (no new wasm import, so
//! the module still instantiates on an older host). Byte arguments are passed
//! as `{"ptr", "len"}` references into the guest's linear memory: the host
//! reads them in place, and `crypto/aead` transforms `data` in place, so a
//! 10 MB payload is never copied through JSON/base64. Small results (signature,
//! digest, MAC, tag) come back base64 in the JSON reply.
//!
//! Only meaningful on wasm32 (the refs are guest addresses). Native builds use
//! aws-lc-rs and the usual crates directly.

use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};

use crate::{Error, Result};

/// Capability version this SDK speaks. Hosts answering a lower version (or the
/// v1 `algs` form) lack the ops below.
pub const CRYPTO_VERSION: u64 = 2;
/// First `convex-rust` release that serves crypto version 2 to V8 hosts.
pub const MIN_CONVEX_RUST: &str = "0.2.0";
/// AES-GCM tag length.
pub const TAG_LEN: usize = 16;

fn call(op: &str, args: Value) -> Option<Value> {
    let out = crate::sys::sync(op, args.to_string().as_bytes());
    serde_json::from_slice::<Value>(&out).ok().map(|v| {
        let mut v = v;
        match v.get_mut("ok") {
            Some(ok) => ok.take(),
            None => v,
        }
    })
}

/// A byte slice as a guest-memory reference.
fn r(b: &[u8]) -> Value {
    json!({ "ptr": b.as_ptr() as usize, "len": b.len() })
}

fn caps() -> &'static Option<Value> {
    static CAPS: OnceLock<Option<Value>> = OnceLock::new();
    CAPS.get_or_init(|| {
        let v = call("crypto/caps", json!({ "version": CRYPTO_VERSION }))?;
        (v.get("version")?.as_u64()? >= CRYPTO_VERSION).then_some(v)
    })
}

/// True if the host serves `alg` for `op` (`"verify"`, `"sign"`, `"aead"`,
/// `"digest"`, `"hmac"`). Probes `crypto/caps` once per instance.
pub fn host_supports(op: &str, alg: &str) -> bool {
    caps()
        .as_ref()
        .and_then(|c| c.get("ops")?.get(op)?.as_array())
        .is_some_and(|algs| algs.iter().any(|a| a.as_str() == Some(alg)))
}

/// `Ok` if the host serves `alg` for `op`, else the clear "update the host" error.
pub fn require(op: &str, alg: &str) -> Result<()> {
    if host_supports(op, alg) {
        Ok(())
    } else {
        Err(lacks(op, alg))
    }
}

/// The "host too old" error for `crypto/<op> <alg>`.
fn lacks(op: &str, alg: &str) -> Error {
    Error::msg(format!(
        "host lacks crypto/{op} {alg}: update convex-rust to >= {MIN_CONVEX_RUST} (V8 host) / the backend (wasmtime host)"
    ))
}

/// Checks the capability, then performs the op; the reply is the `ok` value.
fn run(op: &str, alg: &str, args: Value) -> Result<Value> {
    require(op, alg)?;
    let name = format!("crypto/{op}");
    let reply = crate::sys::sync(&name, args.to_string().as_bytes());
    let v: Value = serde_json::from_slice(&reply).map_err(|e| Error::msg(format!("{name}: bad reply: {e}")))?;
    if let Some(e) = v.get("error") {
        let m = e.get("message").and_then(Value::as_str).unwrap_or("error");
        return Err(Error::msg(m.to_string()));
    }
    v.get("ok").cloned().ok_or_else(|| Error::msg(format!("{name}: empty reply")))
}

fn b64_out(v: &Value, what: &str) -> Result<Vec<u8>> {
    let s = v.as_str().ok_or_else(|| Error::msg(format!("crypto: {what} missing in reply")))?;
    B64.decode(s).map_err(|e| Error::msg(format!("crypto: bad {what}: {e}")))
}

/// Verifies a signature on the host. `alg` is a JWS name (`RS256/384/512`,
/// `PS256/384/512`, `ES256`, `ES384`, `EdDSA`).
///
/// `key`: PKCS#1 `RSAPublicKey` DER for RS*/PS*, SEC1 uncompressed point
/// `04 || X || Y` for ES*, the 32 raw bytes for EdDSA. `sig`: raw RSA
/// signature, or fixed-size `r || s` for ECDSA. `Ok(false)` is a definitive
/// "does not verify"; an unusable key or unsupported alg is an `Err`.
pub fn verify(alg: &str, key: &[u8], msg: &[u8], sig: &[u8]) -> Result<bool> {
    let v = run("verify", alg, json!({ "alg": alg, "key": r(key), "msg": r(msg), "sig": r(sig) }))?;
    v.as_bool().ok_or_else(|| Error::msg("crypto/verify: bad reply"))
}

/// Signs `msg` on the host. `key` is a PKCS#8 DER private key (RSA, P-256,
/// P-384 or Ed25519 to match `alg`). RSA-PSS uses a salt as long as the digest;
/// ECDSA signatures are fixed-size `r || s`.
pub fn sign(alg: &str, key: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
    let v = run("sign", alg, json!({ "alg": alg, "key": r(key), "msg": r(msg) }))?;
    b64_out(&v, "signature")
}

/// Public key of a PKCS#8 private key, in the form [`verify`] takes: the SEC1
/// uncompressed point for `ES256`/`ES384`, the 32 raw bytes for `EdDSA`.
/// (Not offered for RSA.)
pub fn public_key(alg: &str, key: &[u8]) -> Result<Vec<u8>> {
    let v = run("pubkey", alg, json!({ "alg": alg, "key": r(key) }))?;
    b64_out(&v, "public key")
}

/// AES-GCM seal (`alg`: `"A128GCM"` / `"A256GCM"`; 12-byte `nonce`). `data` is
/// encrypted in place; the 16-byte tag is returned.
pub fn aead_seal(alg: &str, key: &[u8], nonce: &[u8], aad: &[u8], data: &mut [u8]) -> Result<[u8; TAG_LEN]> {
    let v = run(
        "aead",
        alg,
        json!({ "alg": alg, "dir": "seal", "key": r(key), "nonce": r(nonce), "aad": r(aad), "data": r(data) }),
    )?;
    b64_out(v.get("tag").unwrap_or(&Value::Null), "tag")?
        .try_into()
        .map_err(|_| Error::msg("crypto/aead: bad tag length"))
}

/// AES-GCM open: `data` (ciphertext) is decrypted in place when `tag`
/// authenticates and left untouched otherwise (`Ok(false)`).
pub fn aead_open(alg: &str, key: &[u8], nonce: &[u8], aad: &[u8], data: &mut [u8], tag: &[u8]) -> Result<bool> {
    let v = run(
        "aead",
        alg,
        json!({
            "alg": alg, "dir": "open", "key": r(key), "nonce": r(nonce), "aad": r(aad),
            "data": r(data), "tag": r(tag),
        }),
    )?;
    v.as_bool().ok_or_else(|| Error::msg("crypto/aead: bad reply"))
}

/// Digest + key + message bytes up to which SHA-2 / HMAC-SHA-2 run inside the guest.
/// A host call costs ~6 us (JSON, base64, alloc, JSPI crossing); larger inputs are
/// faster on the host. A deliberate size split, not a fallback: the host path still
/// requires the caps.
pub const LOCAL_MAX: usize = 2048;

fn local_digest(alg: &str, data: &[u8]) -> Option<Vec<u8>> {
    use sha2::Digest;
    Some(match alg {
        "SHA-256" => sha2::Sha256::digest(data).to_vec(),
        "SHA-384" => sha2::Sha384::digest(data).to_vec(),
        "SHA-512" => sha2::Sha512::digest(data).to_vec(),
        _ => return None,
    })
}

fn local_hmac(alg: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    use hmac::{Hmac, KeyInit, Mac};
    macro_rules! mac {
        ($h:ty) => {{
            let mut m = <Hmac<$h> as KeyInit>::new_from_slice(key).ok()?;
            m.update(data);
            m.finalize().into_bytes().to_vec()
        }};
    }
    Some(match alg {
        "SHA-256" => mac!(sha2::Sha256),
        "SHA-384" => mac!(sha2::Sha384),
        "SHA-512" => mac!(sha2::Sha512),
        _ => return None,
    })
}

/// Digest (`alg`: `"SHA-1"`, `"SHA-256"`, `"SHA-384"`, `"SHA-512"`) of `data`.
/// SHA-2 inputs up to [`LOCAL_MAX`] bytes are hashed in the guest.
pub fn digest(alg: &str, data: &[u8]) -> Result<Vec<u8>> {
    if data.len() <= LOCAL_MAX {
        if let Some(d) = local_digest(alg, data) {
            return Ok(d);
        }
    }
    let v = run("digest", alg, json!({ "alg": alg, "data": r(data) }))?;
    b64_out(&v, "digest")
}

/// HMAC (`alg` names the hash like [`digest`]) of `data` under `key` (any length).
/// SHA-2 inputs with `key + data` up to [`LOCAL_MAX`] bytes are computed in the guest.
pub fn hmac(alg: &str, key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if key.len() + data.len() <= LOCAL_MAX {
        if let Some(d) = local_hmac(alg, key, data) {
            return Ok(d);
        }
    }
    let v = run("hmac", alg, json!({ "alg": alg, "key": r(key), "data": r(data) }))?;
    b64_out(&v, "mac")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn local_vectors() {
        assert_eq!(
            hex(&digest("SHA-256", b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // RFC 4231 case 2
        assert_eq!(
            hex(&hmac("SHA-256", b"Jefe", b"what do ya want for nothing?").unwrap()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(hmac("SHA-512", b"", b"x").unwrap().len(), 64);
    }
}

//! jsonwebtoken `CryptoProvider` for wasm32 that hands every signature / MAC to
//! the host (`convex_fn::crypto`, ABI.md "Optional sync ops"). The wasm build
//! carries no jsonwebtoken crypto backend and no pure-Rust fallback: a host
//! without the ops makes `encode`/`decode` fail with the "update convex-rust"
//! error. Native builds use aws-lc-rs directly.

use jsonwebtoken::crypto::{CryptoProvider, JwkUtils, JwtSigner, JwtVerifier};
use jsonwebtoken::errors::{Error as JwtError, ErrorKind};
use jsonwebtoken::{Algorithm, DecodingKey, DecodingKeyKind, EncodingKey};
use signature::{Error, Signer, Verifier};
use std::sync::OnceLock;

const HMAC_HASH: [(&str, Algorithm, &str); 3] = [
    ("HS256", Algorithm::HS256, "SHA-256"),
    ("HS384", Algorithm::HS384, "SHA-384"),
    ("HS512", Algorithm::HS512, "SHA-512"),
];

fn jws_name(alg: &Algorithm) -> Option<&'static str> {
    Some(match alg {
        Algorithm::RS256 => "RS256",
        Algorithm::RS384 => "RS384",
        Algorithm::RS512 => "RS512",
        Algorithm::PS256 => "PS256",
        Algorithm::PS384 => "PS384",
        Algorithm::PS512 => "PS512",
        Algorithm::ES256 => "ES256",
        Algorithm::ES384 => "ES384",
        Algorithm::EdDSA => "EdDSA",
        _ => return None,
    })
}

fn host_err(e: convex_fn::Error) -> Error {
    Error::from_source(e.message().to_string())
}

fn provider_err(e: convex_fn::Error) -> JwtError {
    ErrorKind::Provider(e.message().to_string()).into()
}

fn require(op: &str, alg: &str) -> Result<(), JwtError> {
    convex_fn::crypto::require(op, alg).map_err(provider_err)
}

struct HostSigner {
    algorithm: Algorithm,
    /// JWS name for RS/PS/ES/EdDSA, hash name for HS*.
    alg: &'static str,
    hmac: bool,
    key: Vec<u8>,
}

impl Signer<Vec<u8>> for HostSigner {
    fn try_sign(&self, msg: &[u8]) -> Result<Vec<u8>, Error> {
        if self.hmac {
            convex_fn::crypto::hmac(self.alg, &self.key, msg).map_err(host_err)
        } else {
            convex_fn::crypto::sign(self.alg, &self.key, msg).map_err(host_err)
        }
    }
}

impl JwtSigner for HostSigner {
    fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

struct HostVerifier {
    algorithm: Algorithm,
    alg: &'static str,
    hmac: bool,
    key: Vec<u8>,
}

impl Verifier<Vec<u8>> for HostVerifier {
    fn verify(&self, msg: &[u8], sig: &Vec<u8>) -> Result<(), Error> {
        if self.hmac {
            let mac = convex_fn::crypto::hmac(self.alg, &self.key, msg).map_err(host_err)?;
            let diff = mac.len() ^ sig.len() | mac.iter().zip(sig).fold(0, |a, (x, y)| a | (x ^ y) as usize);
            return if diff == 0 { Ok(()) } else { Err(Error::new()) };
        }
        match convex_fn::crypto::verify(self.alg, &self.key, msg, sig) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::new()),
            Err(e) => Err(host_err(e)),
        }
    }
}

impl JwtVerifier for HostVerifier {
    fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

fn der_len(out: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let b = n.to_be_bytes();
        let b = &b[b.iter().position(|x| *x != 0).unwrap_or(7)..];
        out.push(0x80 | b.len() as u8);
        out.extend_from_slice(b);
    }
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    der_len(&mut out, content.len());
    out.extend_from_slice(content);
    out
}

fn der_uint(out: &mut Vec<u8>, v: &[u8]) {
    let v = &v[v.iter().position(|x| *x != 0).unwrap_or(v.len().saturating_sub(1))..];
    let mut body = Vec::with_capacity(v.len() + 1);
    if v.first().is_some_and(|b| b & 0x80 != 0) {
        body.push(0);
    }
    body.extend_from_slice(v);
    out.extend(der(0x02, &body));
}

/// PKCS#1 `RSAPublicKey` DER from (n, e).
fn rsa_pkcs1(n: &[u8], e: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    der_uint(&mut body, n);
    der_uint(&mut body, e);
    der(0x30, &body)
}

/// PKCS#8 `PrivateKeyInfo` wrapping a PKCS#1 `RSAPrivateKey` (jsonwebtoken keeps
/// RSA private keys as PKCS#1; the host takes PKCS#8).
fn rsa_pkcs8(pkcs1: &[u8]) -> Vec<u8> {
    // rsaEncryption AlgorithmIdentifier: OID 1.2.840.113549.1.1.1, NULL
    const ALG: [u8; 15] = [0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];
    let mut body = vec![0x02, 0x01, 0x00];
    body.extend_from_slice(&ALG);
    body.extend(der(0x04, pkcs1));
    der(0x30, &body)
}

fn signer(alg: &Algorithm, key: &EncodingKey) -> Result<Box<dyn JwtSigner>, JwtError> {
    if let Some((_, _, hash)) = HMAC_HASH.iter().find(|(_, a, _)| a == alg) {
        require("hmac", hash)?;
        return Ok(Box::new(HostSigner {
            algorithm: *alg,
            alg: hash,
            hmac: true,
            key: key.try_get_hmac_secret()?.to_vec(),
        }));
    }
    let name = jws_name(alg).ok_or_else(|| JwtError::from(ErrorKind::InvalidAlgorithm))?;
    require("sign", name)?;
    let k = if name.starts_with("RS") || name.starts_with("PS") {
        rsa_pkcs8(key.inner())
    } else {
        key.inner().to_vec()
    };
    Ok(Box::new(HostSigner { algorithm: *alg, alg: name, hmac: false, key: k }))
}

fn verifier(alg: &Algorithm, key: &DecodingKey) -> Result<Box<dyn JwtVerifier>, JwtError> {
    if let Some((_, _, hash)) = HMAC_HASH.iter().find(|(_, a, _)| a == alg) {
        require("hmac", hash)?;
        return Ok(Box::new(HostVerifier {
            algorithm: *alg,
            alg: hash,
            hmac: true,
            key: key.try_get_hmac_secret()?.to_vec(),
        }));
    }
    let name = jws_name(alg).ok_or_else(|| JwtError::from(ErrorKind::InvalidAlgorithm))?;
    require("verify", name)?;
    let k = match key.kind() {
        DecodingKeyKind::SecretOrDer(b) => b.clone(),
        DecodingKeyKind::RsaModulusExponent { n, e } => rsa_pkcs1(n, e),
    };
    Ok(Box::new(HostVerifier { algorithm: *alg, alg: name, hmac: false, key: k }))
}

static PROVIDER: OnceLock<CryptoProvider> = OnceLock::new();

/// Installs the provider (idempotent). Call before the first jsonwebtoken use.
pub fn install() {
    let p = PROVIDER.get_or_init(|| CryptoProvider {
        signer_factory: signer,
        verifier_factory: verifier,
        jwk_utils: JwkUtils::new_unimplemented(),
    });
    let _ = p.install_default();
}

//! Crypto cases (crypto-*). Mirrors bench/convex/node/crypto.ts: all keys,
//! salts, nonces and messages come from the shared PRNG.

use crate::prng::Prng;
use argon2::{Algorithm, Argon2, Params, Version};
use convex_fn::{action, Ctx, Error, Result};
use jsonwebtoken::{Algorithm as JwtAlg, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const TEN_MB: usize = 10 * 1024 * 1024;

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

fn hex(b: &[u8]) -> String {
    hex::encode(b)
}

/// SHA-256, HMAC-SHA-256, AES-256-GCM and Ed25519: the host's `crypto/*` ops on
/// wasm32 (no in-guest fallback, contracts/ABI.md), RustCrypto natively.
#[cfg(target_arch = "wasm32")]
mod prim {
    use convex_fn::{crypto, Error, Result};

    pub fn sha256(data: &[u8]) -> Result<Vec<u8>> {
        crypto::digest("SHA-256", data)
    }

    pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        crypto::hmac("SHA-256", key, data)
    }

    pub fn aes_seal(key: &[u8], iv: &[u8], data: &mut [u8]) -> Result<Vec<u8>> {
        Ok(crypto::aead_seal("A256GCM", key, iv, b"", data)?.to_vec())
    }

    pub fn aes_open(key: &[u8], iv: &[u8], data: &mut [u8], tag: &[u8]) -> Result<()> {
        if crypto::aead_open("A256GCM", key, iv, b"", data, tag)? {
            Ok(())
        } else {
            Err(Error::msg("aes-gcm decrypt failed"))
        }
    }

    /// PKCS#8 v1 DER for an Ed25519 seed.
    fn pkcs8(seed: &[u8; 32]) -> Vec<u8> {
        let mut k = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
        k.extend_from_slice(seed);
        k
    }

    pub struct Ed(Vec<u8>);

    pub fn ed_key(seed: [u8; 32]) -> Ed {
        Ed(pkcs8(&seed))
    }

    impl Ed {
        pub fn public(&self) -> Result<Vec<u8>> {
            crypto::public_key("EdDSA", &self.0)
        }

        pub fn sign(&self, msg: &[u8]) -> Result<[u8; 64]> {
            crypto::sign("EdDSA", &self.0, msg)?
                .try_into()
                .map_err(|_| Error::msg("bad signature length"))
        }
    }

    pub fn ed_verify(public: &[u8], msg: &[u8], sig: &[u8]) -> Result<bool> {
        crypto::verify("EdDSA", public, msg, sig)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod prim {
    use aes_gcm::aead::{AeadInOut, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use convex_fn::{Error, Result};
    use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    pub fn sha256(data: &[u8]) -> Result<Vec<u8>> {
        Ok(Sha256::digest(data).to_vec())
    }

    pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let mut m = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key)?;
        m.update(data);
        Ok(m.finalize().into_bytes().to_vec())
    }

    pub fn aes_seal(key: &[u8], iv: &[u8], data: &mut [u8]) -> Result<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::msg("bad key"))?;
        let nonce = Nonce::try_from(iv).map_err(|_| Error::msg("bad nonce"))?;
        let tag = cipher
            .encrypt_inout_detached(&nonce, b"", (&mut *data).into())
            .map_err(|_| Error::msg("aes-gcm encrypt failed"))?;
        Ok(tag.to_vec())
    }

    pub fn aes_open(key: &[u8], iv: &[u8], data: &mut [u8], tag: &[u8]) -> Result<()> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::msg("bad key"))?;
        let nonce = Nonce::try_from(iv).map_err(|_| Error::msg("bad nonce"))?;
        let tag = aes_gcm::Tag::try_from(tag).map_err(|_| Error::msg("bad tag"))?;
        cipher
            .decrypt_inout_detached(&nonce, b"", (&mut *data).into(), &tag)
            .map_err(|_| Error::msg("aes-gcm decrypt failed"))
    }

    pub struct Ed(SigningKey);

    pub fn ed_key(seed: [u8; 32]) -> Ed {
        Ed(SigningKey::from_bytes(&seed))
    }

    impl Ed {
        pub fn public(&self) -> Result<Vec<u8>> {
            Ok(self.0.verifying_key().to_bytes().to_vec())
        }

        pub fn sign(&self, msg: &[u8]) -> Result<[u8; 64]> {
            Ok(self.0.sign(msg).to_bytes())
        }
    }

    pub fn ed_verify(public: &[u8], msg: &[u8], sig: &[u8]) -> Result<bool> {
        let vk = VerifyingKey::from_bytes(public.try_into().map_err(|_| Error::msg("bad key"))?)
            .map_err(|e| Error::msg(e.to_string()))?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| Error::msg("bad signature"))?;
        Ok(vk.verify(msg, &Signature::from_bytes(&sig)).is_ok())
    }
}

/// Fixed RSA-2048 test key (crypto-003 only). Not a secret.
const RSA_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
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
";

const RSA_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAv9kktYHqYI5Hj5Y7NS5X
K2PzSAfu9at111m66K1wirtUgfOzCaxfYkuuQB3uQex1aD3l+wp6yAYGjFHqti7C
E5d1lTXxU9AtYs/e2jh92+03dwzhUbiDXaGj4jUy3a3KmmE+J6GMOEFnvVGJG7sc
RdHu1HyQGQeBzZWOQJz/P850uiynCXX9vnEXqzO5tn9pnt3C2On+5STw0ZJbpWTC
KdfZefwqlyTC9dpWDvi4s0Gbsbt6MZCfULQq39q3w83MZ3JdVGCxfG/6tXuvMqBb
dGLoM47HR2sYfvEIo4nZ8PYFITvGrKMkPl0k1ObhTAHEAZDIueKFxRjTYiljMjmn
AQIDAQAB
-----END PUBLIC KEY-----
";

/// crypto-001: HMAC-SHA256 x100k, chained.
#[action]
pub async fn crypto_001(_ctx: &Ctx, args: SeedArgs) -> Result<String> {
    let mut p = Prng::new(args.seed);
    let key = p.bytes(32);
    let mut mac = p.bytes(64);
    for _ in 0..100_000 {
        mac = prim::hmac_sha256(&key, &mac)?;
    }
    Ok(hex(&mac))
}

#[derive(Serialize)]
struct Claims2 {
    sub: String,
    n: u32,
    iat: u64,
    exp: u64,
}

#[derive(Serialize)]
pub struct Crypto002Out {
    digest: String,
    last: String,
}

/// crypto-002: sign 10k HS256 JWTs.
#[action]
pub async fn crypto_002(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto002Out> {
    #[cfg(target_arch = "wasm32")]
    crate::jwt_host::install();
    let secret = Prng::new(args.seed).bytes(32);
    let key = EncodingKey::from_secret(&secret);
    let mut all = Vec::new();
    let mut last = String::new();
    for i in 0..10_000u32 {
        let mut header = Header::new(JwtAlg::HS256);
        header.typ = None;
        let claims = Claims2 {
            sub: format!("user-{i}"),
            n: i,
            iat: 1_700_000_000,
            exp: 4_102_444_800,
        };
        last = jsonwebtoken::encode(&header, &claims, &key)?;
        all.extend_from_slice(last.as_bytes());
    }
    Ok(Crypto002Out {
        digest: hex(&prim::sha256(&all)?),
        last,
    })
}

#[derive(Serialize, Deserialize)]
struct Claims3 {
    sub: String,
    iat: u64,
    exp: u64,
}

#[derive(Serialize)]
pub struct Crypto003Out {
    token: String,
    valid: u32,
    sub: String,
}

/// crypto-003: sign one RS256 token (setup), verify it 1k times.
#[action]
pub async fn crypto_003(_ctx: &Ctx, _args: SeedArgs) -> Result<Crypto003Out> {
    #[cfg(target_arch = "wasm32")]
    crate::jwt_host::install();
    let mut header = Header::new(JwtAlg::RS256);
    header.typ = None;
    let claims = Claims3 {
        sub: "user-1".into(),
        iat: 1_700_000_000,
        exp: 4_102_444_800,
    };
    let token = jsonwebtoken::encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(RSA_PRIVATE_PEM.as_bytes())?,
    )?;
    let key = DecodingKey::from_rsa_pem(RSA_PUBLIC_PEM.as_bytes())?;
    let validation = Validation::new(JwtAlg::RS256);
    let mut valid = 0;
    let mut sub = String::new();
    for _ in 0..1000 {
        let d = jsonwebtoken::decode::<Claims3>(&token, &key, &validation)?;
        valid += 1;
        sub = d.claims.sub;
    }
    Ok(Crypto003Out { token, valid, sub })
}

#[derive(Serialize)]
pub struct Crypto004Out {
    tag: String,
    head: String,
    len: usize,
}

fn aes_inputs(seed: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut p = Prng::new(seed);
    let key = p.bytes(32);
    let iv = p.bytes(12);
    let data = p.bytes(TEN_MB);
    (key, iv, data)
}

/// crypto-004: AES-256-GCM encrypt 10 MB.
#[action]
pub async fn crypto_004(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto004Out> {
    let (key, iv, mut data) = aes_inputs(args.seed);
    let tag = prim::aes_seal(&key, &iv, &mut data)?;
    Ok(Crypto004Out {
        tag: hex(&tag),
        head: hex(&data[..32]),
        len: data.len(),
    })
}

#[derive(Serialize)]
pub struct Crypto005Out {
    #[serde(rename = "match")]
    matches: bool,
    len: usize,
}

/// crypto-005: AES-256-GCM encrypt (setup) then decrypt + authenticate 10 MB.
#[action]
pub async fn crypto_005(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto005Out> {
    let (key, iv, data) = aes_inputs(args.seed);
    let mut buf = data.clone();
    let tag = prim::aes_seal(&key, &iv, &mut buf)?;
    prim::aes_open(&key, &iv, &mut buf, &tag)?;
    Ok(Crypto005Out {
        matches: buf == data,
        len: buf.len(),
    })
}

fn seed32(p: &mut Prng) -> prim::Ed {
    let s: [u8; 32] = p.bytes(32).try_into().unwrap();
    prim::ed_key(s)
}

#[derive(Serialize)]
pub struct Crypto006Out {
    digest: String,
    first: String,
    last: String,
}

/// crypto-006: Ed25519 sign 10k 64-byte messages.
#[action]
pub async fn crypto_006(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto006Out> {
    let mut p = Prng::new(args.seed);
    let sk = seed32(&mut p);
    let mut all = Vec::with_capacity(10_000 * 64);
    let mut first = String::new();
    let mut last = String::new();
    for i in 0..10_000 {
        let sig = sk.sign(&p.bytes(64))?;
        all.extend_from_slice(&sig);
        if i == 0 {
            first = hex(&sig);
        }
        last = hex(&sig);
    }
    Ok(Crypto006Out {
        digest: hex(&prim::sha256(&all)?),
        first,
        last,
    })
}

#[derive(Serialize)]
pub struct Crypto007Out {
    valid: u32,
    total: u32,
}

/// crypto-007: sign 64 messages (setup), verify 10k (every 100th corrupted).
#[action]
pub async fn crypto_007(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto007Out> {
    let mut p = Prng::new(args.seed);
    let sk = seed32(&mut p);
    let vk = sk.public()?;
    let mut msgs = Vec::new();
    let mut sigs = Vec::new();
    for _ in 0..64 {
        let m = p.bytes(64);
        sigs.push(sk.sign(&m)?);
        msgs.push(m);
    }
    let mut valid = 0;
    for i in 0..10_000usize {
        let mut sig = sigs[i % 64];
        if i % 100 == 0 {
            sig[0] ^= 1;
        }
        if prim::ed_verify(&vk, &msgs[i % 64], &sig)? {
            valid += 1;
        }
    }
    Ok(Crypto007Out { valid, total: 10_000 })
}

#[derive(Serialize)]
pub struct Crypto008Out {
    hash: String,
    m: u32,
    t: u32,
    p: u32,
}

/// crypto-008: argon2id m=19456 KiB, t=2, p=1.
#[action]
pub async fn crypto_008(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto008Out> {
    let mut p = Prng::new(args.seed);
    let password = p.word(16);
    let salt = p.bytes(16);
    let params = Params::new(19456, 2, 1, Some(32)).map_err(|e| Error::msg(e.to_string()))?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    a2.hash_password_into(password.as_bytes(), &salt, &mut out)
        .map_err(|e| Error::msg(e.to_string()))?;
    Ok(Crypto008Out {
        hash: hex(&out),
        m: 19456,
        t: 2,
        p: 1,
    })
}

#[derive(Serialize)]
pub struct Crypto009Out {
    hash: String,
    cost: u32,
}

/// crypto-009: bcrypt cost 10 with a PRNG-derived salt.
#[action]
pub async fn crypto_009(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto009Out> {
    let mut p = Prng::new(args.seed);
    let password = p.word(16);
    let salt: [u8; 16] = p.bytes(16).try_into().unwrap();
    let parts = bcrypt::hash_with_salt(password, 10, salt)?;
    Ok(Crypto009Out {
        hash: parts.format_for_version(bcrypt::Version::TwoB),
        cost: 10,
    })
}

#[derive(Serialize)]
pub struct Crypto010Out {
    unique4: usize,
    unique7: usize,
    sorted7: bool,
    last4: String,
    last7: String,
}

/// crypto-010: 100k UUID v4 and v7 from PRNG bytes.
#[action]
pub async fn crypto_010(_ctx: &Ctx, args: SeedArgs) -> Result<Crypto010Out> {
    const N: usize = 100_000;
    let rnd = Prng::new(args.seed).bytes(N * 32);
    let mut seen4 = HashSet::with_capacity(N);
    let mut seen7 = HashSet::with_capacity(N);
    let mut last4 = String::new();
    let mut last7 = String::new();
    let mut sorted = true;
    for i in 0..N {
        let b = &rnd[i * 32..(i + 1) * 32];
        last4 = uuid::Builder::from_random_bytes(b[..16].try_into().unwrap())
            .into_uuid()
            .to_string();
        seen4.insert(last4.clone());
        let r10: [u8; 10] = b[16..26].try_into().unwrap();
        let u7 = uuid::Builder::from_unix_timestamp_millis(1_700_000_000_000 + i as u64, &r10)
            .into_uuid()
            .to_string();
        if u7 <= last7 {
            sorted = false;
        }
        last7 = u7.clone();
        seen7.insert(u7);
    }
    Ok(Crypto010Out {
        unique4: seen4.len(),
        unique7: seen7.len(),
        sorted7: sorted,
        last4,
        last7,
    })
}

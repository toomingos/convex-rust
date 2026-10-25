//! jsonwebtoken + reqwest (JWKS-style fetch): sign/verify HS256 locally, fetch JSON over the shim.
use convex_fn::{action, json, Ctx, Error, Result, Value};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct NoArgs {}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
}

fn e<E: std::fmt::Display>(x: E) -> Error {
    Error::msg(x.to_string())
}

#[action]
pub async fn probe(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let jwks: Value = reqwest::get("https://auth.test/.well-known/jwks.json").await?.json().await?;
    let claims = Claims { sub: "u1".into(), exp: 4_102_444_800 };
    let tok = encode(&Header::default(), &claims, &EncodingKey::from_secret(b"s")).map_err(e)?;
    let back = decode::<Claims>(&tok, &DecodingKey::from_secret(b"s"), &Validation::new(Algorithm::HS256)).map_err(e)?;
    Ok(json!({"sub": back.claims.sub, "fetchedUrl": jwks["url"]}))
}

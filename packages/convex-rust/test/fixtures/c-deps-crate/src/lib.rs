use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Args {
    pub text: String,
}

/// Round-trips the text through zstd (C `zstd-sys`).
#[cfg(feature = "zstd")]
#[action]
pub async fn zstd_roundtrip(_ctx: &Ctx, a: Args) -> Result<String> {
    let packed = zstd::encode_all(a.text.as_bytes(), 3).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    let back = zstd::decode_all(&packed[..]).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    Ok(format!("{} -> {} bytes -> {}", a.text.len(), packed.len(), String::from_utf8_lossy(&back)))
}

/// In-memory SQLite (C `libsqlite3-sys`, bundled).
#[cfg(feature = "sqlite")]
#[action]
pub async fn sqlite_version(_ctx: &Ctx, _a: Args) -> Result<String> {
    let c = rusqlite::Connection::open_in_memory().map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    c.query_row("select sqlite_version()", [], |r| r.get::<_, String>(0))
        .map_err(|e| convex_fn::Error::msg(e.to_string()))
}

/// SHA-256 with `ring` (C and assembly).
#[cfg(feature = "ring")]
#[action]
pub async fn ring_sha256(_ctx: &Ctx, a: Args) -> Result<String> {
    let d = ring::digest::digest(&ring::digest::SHA256, a.text.as_bytes());
    Ok(d.as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

#[action]
pub async fn plain(_ctx: &Ctx, a: Args) -> Result<String> {
    Ok(a.text)
}

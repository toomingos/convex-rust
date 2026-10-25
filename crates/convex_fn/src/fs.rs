//! The per-call in-memory `/tmp` (contracts/ABI.md "Optional WASI: /tmp").
//!
//! The V8 host preopens `/tmp` as an in-memory directory: `std::fs` works under it, it is empty at the start
//! of every call and discarded after it, and its size is capped (default 64 MiB, `ENOSPC` beyond). File data
//! lives in the guest's linear memory. Everything outside `/tmp` has no file system.
//!
//! `std::env::temp_dir()` **panics** on `wasm32-wasip1` ("not supported by WASI yet"), whatever `TMPDIR` says,
//! so use [`temp_dir`] here, and for crates that default to `std::env::temp_dir()` use their `*_in` forms
//! (for example `tempfile::Builder::new().tempfile_in(convex_fn::fs::temp_dir())`).

use std::path::PathBuf;

/// The directory the host preopens for temporary files.
pub const TMP_DIR: &str = "/tmp";

/// `/tmp`; the replacement for `std::env::temp_dir()`, which panics on `wasm32-wasip1`.
pub fn temp_dir() -> PathBuf {
    PathBuf::from(TMP_DIR)
}

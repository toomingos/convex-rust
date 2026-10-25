//! Rust side of the 100 benchmark pairs (bench/cases.json).
//! Orchestrator-owned: workers request `mod` lines and deps in their reports.

pub mod prng;

pub mod base;
pub mod cb;
pub mod cpu;
pub mod mem;
pub mod crypto;
#[cfg(target_arch = "wasm32")]
pub mod jwt_host;
pub mod media;
pub mod ser;
pub mod data;
pub mod net;

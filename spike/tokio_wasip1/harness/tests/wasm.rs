//! Runs the spike action as wasm32-wasip1 on wasmtime with real timers.
//! Build the guest first:
//!   CARGO_TARGET_DIR=<repo>/target \
//!   cargo build --release --target wasm32-wasip1 -p tokio_wasip1 --lib

use std::path::PathBuf;

fn wasm_path() -> PathBuf {
    if let Ok(p) = std::env::var("SPIKE_WASM") {
        return p.into();
    }
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../target").into());
    PathBuf::from(target).join("wasm32-wasip1/release/tokio_wasip1.wasm")
}

#[test]
fn probe_wasm() {
    let out = tokio_wasip1_harness::run(&wasm_path(), "convex_run:tokio_wasip1:probe", "{}").unwrap();
    println!("envelope = {}", out.envelope);
    println!("wall = {:.1} ms, stats = {:?}", out.wall_ms, out.stats);
    println!("stdio = {:?}\nlogs = {:?}", out.stdio, out.logs);
    let ok = &out.envelope["ok"];
    assert!(ok.is_object(), "envelope: {}", out.envelope);
    assert_eq!(ok["fetchStatus"], 200);
    assert_eq!(ok["fetchBody"], "hello");
    assert_eq!(ok["spawned"], 7);
    assert_eq!(ok["fastRace"], "fetch:200");
    // Real timers: the 300 ms host fetch loses to the 60 ms tokio timeout.
    assert_eq!(ok["slowRace"], "timeout");
    assert_eq!(ok["chanSum"], 6);
    // join! of host sleeps 60 + 120 ms takes ~120 ms (concurrent), not 180.
    let after = ok["afterSleepsMs"].as_u64().unwrap();
    assert!((115..170).contains(&after), "afterSleepsMs = {after}");
}

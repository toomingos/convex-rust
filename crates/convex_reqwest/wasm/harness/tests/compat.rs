//! Runs the compat probes (crates/convex_reqwest/compat/*) that build for wasm32-wasip1.
//! Build them first: crates/convex_reqwest/compat/build.sh <name>

use std::path::PathBuf;

fn run(krate: &str) -> serde_json::Value {
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../target-e2e").into());
    let wasm = PathBuf::from(target).join(format!("wasm32-wasip1/release/{krate}.wasm"));
    let out = reqwest_wasm_harness::run(&wasm, &format!("convex_run:{krate}:probe"), "{}").unwrap();
    println!("{krate}: {} ({:.0} ms)", out.envelope, out.wall_ms);
    assert!(out.envelope["ok"].is_object(), "{krate}: {}", out.envelope);
    out.envelope["ok"].clone()
}

#[test]
fn jsonwebtoken_sign_verify_and_fetch() {
    let ok = run("compat_jsonwebtoken");
    assert_eq!(ok["sub"], "u1");
    assert_eq!(ok["fetchedUrl"], "https://auth.test/.well-known/jwks.json");
}

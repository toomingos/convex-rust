//! Runs the probe actions as wasm32-wasip1 on wasmtime with real timers.
//! Build the guest first (see ../Cargo.toml):
//!   CARGO_TARGET_DIR=<repo>/target-e2e \
//!   cargo build --offline --release --target wasm32-wasip1 -p reqwest_wasm_probe

use std::path::PathBuf;

use serde_json::Value;

fn run(name: &str) -> Value {
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| "<repo>/target-e2e".into());
    let wasm = PathBuf::from(target).join("wasm32-wasip1/release/reqwest_wasm_probe.wasm");
    let out = reqwest_wasm_harness::run(&wasm, &format!("convex_run:reqwest_wasm_probe:{name}"), "{}")
        .unwrap();
    println!("{name}: {} ({:.0} ms, {:?})", out.envelope, out.wall_ms, out.stats);
    assert!(out.envelope["ok"].is_object(), "{name}: {}", out.envelope);
    out.envelope["ok"].clone()
}

fn header<'a>(echo: &'a Value, name: &str) -> Option<&'a str> {
    echo["headers"]
        .as_array()?
        .iter()
        .find(|h| h[0].as_str().is_some_and(|k| k.eq_ignore_ascii_case(name)))
        .and_then(|h| h[1].as_str())
}

#[test]
fn basic_get() {
    let ok = run("basic");
    assert_eq!(ok["status"], 200);
    assert_eq!(ok["xEcho"], "1");
    assert_eq!(ok["echo"]["method"], "GET");
    assert_eq!(ok["echo"]["url"], "https://api.test/echo?x=1");
}

#[test]
fn post_json_form_concurrent() {
    let ok = run("post");
    let j = &ok["json"];
    assert_eq!(j["method"], "POST");
    assert_eq!(j["body"], "{\"a\":1}");
    assert_eq!(header(j, "authorization"), Some("Bearer tok"));
    assert_eq!(header(j, "user-agent"), Some("probe/1"));
    assert_eq!(ok["form"]["body"], "k=v");
    assert_eq!(ok["get"]["url"], "https://api.test/g?q=1");
}

#[test]
fn timeouts_fire_with_real_timers() {
    let ok = run("timeout");
    assert_eq!(ok["clientTimeout"], true);
    assert_eq!(ok["requestTimeout"], true);
    assert_eq!(ok["fast"], 200);
}

#[test]
fn status_and_streaming_body() {
    let ok = run("body");
    assert_eq!(ok["isStatus"], true);
    assert_eq!(ok["status"], 404);
    assert_eq!(ok["parts"], serde_json::json!(["ab", "cd", "ef"]));
    assert_eq!(ok["firstChunkLen"], 2);
}

#[test]
fn vendor_oauth2_client_credentials() {
    let ok = run("oauth");
    assert_eq!(ok["accessToken"], "tok123");
    assert_eq!(ok["expiresIn"], 3600);
}

#[test]
fn vendor_reqwest_middleware_on_0_13() {
    let ok = run("middleware");
    assert_eq!(header(&ok["echo"], "x-mw"), Some("yes"));
    assert_eq!(ok["echo"]["url"], "https://api.test/mw");
}

#[test]
fn blocking_get_inside_action() {
    let ok = run("blocking_get");
    assert_eq!(ok["status"], 200);
    assert_eq!(ok["echo"]["url"], "https://api.test/blocking?x=1");
}

#[test]
fn async_and_blocking_mixed() {
    let ok = run("mixed");
    assert_eq!(ok["a"], "https://api.test/async-1");
    assert_eq!(ok["b"], "https://api.test/blocking-2");
    assert_eq!(ok["bBody"], "payload");
    assert_eq!(ok["c"], "https://api.test/async-3");
    assert_eq!(ok["spawned"], 7);
}

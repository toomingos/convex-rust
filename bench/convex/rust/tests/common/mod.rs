//! Shared test support for bench/convex/rust/tests/<category>.rs.
//! Orchestrator-owned.
#![allow(dead_code)]

use std::{path::PathBuf, sync::OnceLock};

pub mod argsgen;

use serde_json::Value;

fn bench_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// bench/cases.json, parsed once per test binary.
fn all_cases() -> &'static Value {
    static ALL: OnceLock<Value> = OnceLock::new();
    ALL.get_or_init(|| {
        serde_json::from_str(&std::fs::read_to_string(bench_dir().join("cases.json")).unwrap())
            .unwrap()
    })
}

/// Declares `#[test] fn $test` that runs `$name` on a fresh `TestHost` (or on
/// the optional `$host` expression) and checks it against bench/expected.
/// Use via `#[macro_use] mod common;`; the call site needs no imports.
#[allow(unused_macros)]
macro_rules! case {
    ($name:ident, $test:ident, $id:literal) => {
        case!($name, $test, $id, convex_fn_test::TestHost::new());
    };
    ($name:ident, $test:ident, $id:literal, $host:expr) => {
        #[test]
        fn $test() {
            common::check($id, &convex_fn_test::call!($host, $name, common::args($id)));
        }
    };
}

/// The case entry from bench/cases.json.
pub fn case(id: &str) -> Value {
    all_cases()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("unknown case {id}"))
        .clone()
}

/// The case's literal `args` (cases with `argsGen` build their own).
pub fn args(id: &str) -> Value {
    let c = case(id);
    if c.get("argsGen").is_some() {
        return argsgen::generate(&c);
    }
    c["args"].clone()
}

/// The Node output for a case (bench/expected/<id>.json, written by
/// `node scripts/run-node.ts --write <id>`).
pub fn expected(id: &str) -> Value {
    let p = bench_dir().join("expected").join(format!("{id}.json"));
    let s = std::fs::read_to_string(&p).unwrap_or_else(|_| {
        panic!(
            "missing {}; run: node scripts/run-node.ts --write {id}",
            p.display()
        )
    });
    serde_json::from_str(&s).unwrap()
}

/// Asserts the Rust envelope matches the Node output under the case's compare mode.
pub fn check(id: &str, rust: &Value) {
    let c = case(id);
    let mode = c["compare"].as_str().unwrap();
    let tol = c["tolerance"].as_f64().unwrap_or(1e-9);
    let node = expected(id);
    if let Err(path) = convex_fn_test::compare::matches(mode, tol, rust, &node) {
        let show = |v: &Value| {
            let s = v.to_string();
            if s.len() > 400 {
                format!("{}…", &s[..400])
            } else {
                s
            }
        };
        panic!(
            "{id}: rust and node differ ({mode}) at {path}\n rust: {}\n node: {}",
            show(rust),
            show(&node)
        );
    }
}

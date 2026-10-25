//! Rust mirror of `generateArgs` in bench/shared/cases.ts (argsGen cases).
//! Produces Convex-JSON args; keep in lock-step with the TS generator.

use bench_rust::prng::Prng;
use convex_fn::{json, Bytes, Float64, Int64, Value};
use serde_json::Map;

const UNICODE_POOL: &[&str] = &[
    "\u{1F600}",
    "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
    "\u{1F1F5}\u{1F1F9}",
    "日本語",
    "中文",
    "한국어",
    "שלום",
    "مرحبا بالعالم",
    "e\u{0301}",
    "\u{00C5}",
    "\u{212B}",
    "Ω",
    "ß",
    "\u{10348}",
    "\u{200F}",
    "\u{202E}",
    "\t",
    "\"",
    "\\",
    " ",
    "abc",
];

pub fn generate(case: &Value) -> Value {
    let g = &case["argsGen"];
    let seed = case["args"]["seed"].as_u64().unwrap() as u32;
    let mut p = Prng::new(seed);
    let payload: Value = match g["kind"].as_str().unwrap() {
        "object" => {
            // Convex args allow at most 1024 fields per object: beyond 1000 keys, grow the values.
            let bytes = g["bytes"].as_u64().unwrap() as usize;
            let total = bytes.div_ceil(76);
            let n = total.min(1000);
            let vlen = if total <= 1000 { 64 } else { bytes.div_ceil(1000) - 12 };
            let mut m = Map::new();
            for i in 0..n {
                m.insert(format!("k{i}"), Value::String(p.text(vlen)));
            }
            Value::Object(m)
        }
        "nested" => {
            let depth = g["depth"].as_u64().unwrap();
            let levels: Vec<(u64, String)> = (1..=depth).map(|d| (d, p.word(8))).collect();
            let mut cur = json!({ "leaf": p.text(16) });
            for (d, s) in levels.into_iter().rev() {
                cur = json!({ "d": d, "s": s, "child": cur });
            }
            cur
        }
        "array_of_objects" => {
            let count = g["count"].as_u64().unwrap();
            let mut a = Vec::new();
            for i in 0..count {
                let name = p.word(8);
                let score = p.below(1_000_000);
                let x = p.f64();
                a.push(json!({ "id": i, "name": name, "score": score, "x": x }));
            }
            Value::Array(a)
        }
        "bytes" => {
            serde_json::to_value(Bytes(p.bytes(g["bytes"].as_u64().unwrap() as usize))).unwrap()
        }
        "int64" => {
            let mut v = vec![0, 1, -1, i64::MAX, i64::MIN];
            for _ in 0..16 {
                let hi = u64::from(p.next_u32());
                let lo = u64::from(p.next_u32());
                v.push(((hi << 32) | lo) as i64);
            }
            serde_json::to_value(v.into_iter().map(Int64).collect::<Vec<_>>()).unwrap()
        }
        "float_specials" => {
            let mut v = vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0, 0.0];
            for _ in 0..16 {
                v.push(p.f64() * 1e6 - 5e5);
            }
            serde_json::to_value(v.into_iter().map(Float64).collect::<Vec<_>>()).unwrap()
        }
        "unicode" => {
            let mut a = Vec::new();
            for _ in 0..64 {
                let mut s = String::new();
                for _ in 0..8 {
                    s.push_str(UNICODE_POOL[p.below(UNICODE_POOL.len() as u32) as usize]);
                }
                a.push(Value::String(s));
            }
            Value::Array(a)
        }
        k => panic!("unknown argsGen kind {k}"),
    };
    json!({ "seed": seed, "payload": payload })
}

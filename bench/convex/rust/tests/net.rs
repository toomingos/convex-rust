//! net-* against the mock host. `on_fetch` handlers emulate
//! bench/mock-server/server.ts; expected outputs are bench/expected/net-*.json.
#[macro_use]
mod common;

use bench_rust::{net::*, prng::Prng};
use convex_fn_test::{call, FetchStubRequest, FetchStubResponse, TestHost};
use serde_json::json;
use sha2::{Digest, Sha256};

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(str::to_string))
}

fn path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let p = &rest[rest.find('/').unwrap_or(rest.len())..];
    p.split('?').next().unwrap()
}

fn json_resp(v: serde_json::Value) -> FetchStubResponse {
    FetchStubResponse::new(200, v.to_string()).header("content-type", "application/json")
}

/// Emulates the mock server. Redirects are followed by the host, so `/redirect/k`
/// resolves directly to its final response.
fn mock(req: &FetchStubRequest) -> FetchStubResponse {
    let p = path(&req.url).to_string();
    let seed: u32 = query_param(&req.url, "seed")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let parts: Vec<&str> = p.split('/').collect();
    let n: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    match parts[1] {
        "bytes" | "stream" => FetchStubResponse::new(200, Prng::new(seed).bytes(n)),
        "echo" => json_resp(json!({
            "length": req.body.len(),
            "sha256": hex::encode(Sha256::digest(&req.body)),
        })),
        "redirect" => json_resp(json!({ "done": true, "hops": n })),
        "json" => {
            let mut pr = Prng::new(seed);
            json_resp(
                (0..n)
                    .map(|i| json!({ "i": i, "name": pr.word(8), "score": pr.below(1000) }))
                    .collect(),
            )
        }
        _ => FetchStubResponse::new(404, "{}"),
    }
}

fn host() -> TestHost {
    TestHost::new().on_fetch(mock)
}

case!(net_001, net_001_matches_node, "net-001", host());
case!(net_002, net_002_matches_node, "net-002", host());
case!(net_003, net_003_matches_node, "net-003", host());
case!(net_004, net_004_matches_node, "net-004", host());
case!(net_005, net_005_matches_node, "net-005", host());
case!(net_006, net_006_matches_node, "net-006", host());
case!(net_010, net_010_matches_node, "net-010", host());

#[test]
fn net_007_matches_node() {
    let h = host();
    let out = call!(h, net_007, common::args("net-007"));
    common::check("net-007", &out);
    let id = out["ok"]["id"].as_str().unwrap();
    assert!(h.storage_read(id).is_none(), "blob is deleted after the store");
}

#[test]
fn net_008_matches_node() {
    let h = host().on_mutation("cbTargets:insertDoc", |a| {
        let mut doc = a.clone();
        doc["_id"] = json!("stubdocid");
        doc["_creationTime"] = json!(1700000000000u64);
        Ok::<_, String>(doc)
    });
    common::check("net-008", &call!(h, net_008, common::args("net-008")));
    let calls = h.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args, json!({ "key": 1, "value": "n10" }));
}

#[test]
fn net_009_matches_node() {
    // The stub answers instantly; the timer still wins because `race` polls it
    // first and the mock host delivers events in submission order.
    let h = host();
    common::check("net-009", &call!(h, net_009, common::args("net-009")));
    assert_eq!(h.slept_ms(), 200);
}

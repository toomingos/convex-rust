//! Native run of the spike action against convex_fn_test's mock host.

use convex_fn::json;
use convex_fn_test::{FetchStubResponse, TestHost};

#[test]
fn probe_native() {
    let host = TestHost::new().on_fetch(|_| FetchStubResponse::chunked(200, ["hel", "lo"]));
    let env = convex_fn_test::call!(host, tokio_wasip1::probe, json!({}));
    println!("{env}");
    let ok = &env["ok"];
    assert!(ok.is_object(), "envelope: {env}");
    assert_eq!(ok["fetchStatus"], 200);
    assert_eq!(ok["fetchBody"], "hello");
    assert_eq!(ok["spawned"], 7);
    assert_eq!(ok["fastRace"], "fetch:200");
    // The mock completes fetches instantly, so it beats the 60 ms timeout.
    assert_eq!(ok["slowRace"], "fetch:200");
    assert_eq!(ok["chanSum"], 6);
    assert_eq!(host.slept_ms(), 180);
}

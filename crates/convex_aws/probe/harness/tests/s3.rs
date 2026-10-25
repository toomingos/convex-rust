//! Runs the aws-sdk-s3 probe as wasm32-wasip1 on wasmtime against a minimal S3 mock.
//! Build the guest first (see ../Cargo.toml).

use std::path::PathBuf;

#[test]
fn s3_put_get_list_signed_with_sigv4() {
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../target-e2e").into());
    let wasm = PathBuf::from(target).join("wasm32-wasip1/release/convex_aws_probe.wasm");
    let size = std::fs::metadata(&wasm).unwrap().len();
    let out = convex_aws_probe_harness::run(
        &wasm,
        "convex_run:convex_aws_probe:s3",
        "{}",
        &[
            ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
            ("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
            ("AWS_SESSION_TOKEN", "sess-token"),
            ("AWS_REGION", "eu-west-1"),
            ("AWS_ENDPOINT_URL", "http://s3.mock:9000"),
        ],
    )
    .unwrap();
    println!("wasm size: {size} bytes; wall {:.0} ms; {:?}", out.wall_ms, out.stats);
    println!("result: {}", out.envelope);
    for r in &out.requests {
        println!("  {} {} body={}", r["method"], r["url"], r["bodyLen"]);
    }
    let ok = &out.envelope["ok"];
    assert!(ok.is_object(), "{}", out.envelope);
    assert_eq!(ok["body"], "hello s3");
    assert_eq!(ok["contentType"], "text/plain");
    assert_eq!(ok["keys"], serde_json::json!(["dir/hello.txt"]));
    assert_eq!(ok["keyCount"], 1);
    assert_eq!(ok["missingIsNoSuchKey"], true);

    assert!(out.requests.len() >= 4);
    for r in &out.requests {
        let auth = r["headers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h[0].as_str().unwrap().eq_ignore_ascii_case("authorization"))
            .map(|h| h[1].as_str().unwrap().to_string())
            .expect("authorization header");
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"), "{auth}");
        assert!(auth.contains("/eu-west-1/s3/aws4_request"), "{auth}");
        let has = |n: &str| r["headers"].as_array().unwrap().iter().any(|h| h[0] == n);
        assert!(has("x-amz-date") && has("x-amz-security-token"));
    }
    assert!(out.requests[0]["url"].as_str().unwrap().starts_with("http://s3.mock:9000/probe/dir/hello.txt"));
}

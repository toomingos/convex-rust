//! Native test of the smithy HttpClient conversion through convex_fn_test's
//! fetch stub: an aws-sdk-s3 client signs with SigV4 and sends via ctx.fetch.

use std::{cell::RefCell, rc::Rc};

use convex_fn_test::{FetchStubRequest, FetchStubResponse, TestHost};

fn header<'a>(r: &'a FetchStubRequest, name: &str) -> Option<&'a str> {
    r.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn s3_client(cfg: &aws_types::SdkConfig) -> aws_sdk_s3::Client {
    let conf = aws_sdk_s3::config::Builder::from(cfg)
        .force_path_style(true)
        .build();
    aws_sdk_s3::Client::from_conf(conf)
}

#[test]
fn put_and_get_object_via_ctx_fetch() {
    let seen: Rc<RefCell<Vec<FetchStubRequest>>> = Rc::default();
    let s = seen.clone();
    let host = TestHost::new().on_fetch(move |r| {
        s.borrow_mut().push(r.clone());
        match r.method.as_str() {
            "PUT" => FetchStubResponse::new(200, "").header("etag", "\"abc\""),
            _ => FetchStubResponse::new(404, concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>NoSuchKey</Code>",
                "<Message>nope</Message></Error>"
            ))
            .header("content-type", "application/xml"),
        }
    });
    host.run(|_| async {
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret");
        std::env::set_var("AWS_REGION", "eu-west-1");
        std::env::set_var("AWS_ENDPOINT_URL", "http://s3.mock:9000");
        let cfg = convex_aws::config_from_env().await;
        let s3 = s3_client(&cfg);
        let put = s3
            .put_object()
            .bucket("b")
            .key("k.txt")
            .body(b"hello".to_vec().into())
            .send()
            .await
            .unwrap();
        assert_eq!(put.e_tag(), Some("\"abc\""));
        let err = s3.get_object().bucket("b").key("missing").send().await;
        let err = err.unwrap_err().into_service_error();
        assert!(err.is_no_such_key(), "{err:?}");
    });
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].url, "http://s3.mock:9000/b/k.txt?x-id=PutObject");
    assert_eq!(seen[0].method, "PUT");
    let auth = header(&seen[0], "authorization").unwrap();
    assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"), "{auth}");
    assert!(auth.contains("/eu-west-1/s3/aws4_request"), "{auth}");
    assert!(header(&seen[0], "x-amz-date").is_some());
}

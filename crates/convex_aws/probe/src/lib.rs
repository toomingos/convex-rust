//! DX-06c probe: S3 put/get/list from a `"use rust"` action.

use convex_fn::{action, json, Ctx, Error, Result, Value};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

fn e<E: std::fmt::Debug>(x: E) -> Error {
    Error::msg(format!("{x:?}"))
}

#[action]
pub async fn s3(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let cfg = convex_aws::config_from_env().await;
    let conf = aws_sdk_s3::config::Builder::from(&cfg)
        .force_path_style(true)
        .build();
    let s3 = aws_sdk_s3::Client::from_conf(conf);

    let put = s3
        .put_object()
        .bucket("probe")
        .key("dir/hello.txt")
        .content_type("text/plain")
        .body(b"hello s3".to_vec().into())
        .send()
        .await
        .map_err(e)?;
    let got = s3
        .get_object()
        .bucket("probe")
        .key("dir/hello.txt")
        .send()
        .await
        .map_err(e)?;
    let ctype = got.content_type().map(str::to_string);
    let body = got.body.collect().await.map_err(e)?.into_bytes();
    let list = s3
        .list_objects_v2()
        .bucket("probe")
        .prefix("dir/")
        .send()
        .await
        .map_err(e)?;
    let keys: Vec<String> = list
        .contents()
        .iter()
        .filter_map(|o| o.key().map(str::to_string))
        .collect();
    let missing = s3.get_object().bucket("probe").key("nope").send().await;
    let missing_is_404 = missing
        .err()
        .map(|er| er.into_service_error().is_no_such_key())
        .unwrap_or(false);
    Ok(json!({
        "etag": put.e_tag(),
        "contentType": ctype,
        "body": String::from_utf8_lossy(&body),
        "keys": keys,
        "keyCount": list.key_count(),
        "missingIsNoSuchKey": missing_is_404,
    }))
}

use std::collections::HashMap;

use convex_fn::{Bytes, ConvexArgs, ConvexType, ConvexValue, FunctionRef, Int64};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize, ConvexArgs)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SendArgs {
    body_text: String,
    #[convex(id = "users")]
    author: String,
    #[convex(id = "users")]
    cc: Vec<String>,
    count: Int64,
    ratio: f64,
    small: i32,
    flag: bool,
    blob: Bytes,
    tags: Vec<String>,
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nick: Option<String>,
    #[serde(default)]
    retries: u32,
    #[serde(rename = "type")]
    kind: String,
    meta: HashMap<String, f64>,
    extra: serde_json::Value,
    nested: Inner,
    #[serde(skip)]
    ignored: u8,
}

#[derive(Serialize, Deserialize, ConvexValue)]
struct Inner {
    x: f64,
}

#[derive(Serialize, Deserialize, ConvexValue)]
enum Status {
    Ok,
    Failed { code: f64 },
    Wrapped(String),
}

#[derive(Serialize, Deserialize, ConvexValue)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Started,
    Done { took_ms: f64 },
}

fn json_of<T: ConvexType>() -> serde_json::Value {
    serde_json::from_str(&T::VALIDATOR.to_json()).unwrap()
}

#[test]
fn struct_validator() {
    let v = json_of::<SendArgs>();
    let fields = &v["value"];
    assert_eq!(v["type"], "object");
    let f = |n: &str| {
        (
            fields[n]["fieldType"].clone(),
            fields[n]["optional"].clone(),
        )
    };
    assert_eq!(f("bodyText"), (json!({"type":"string"}), json!(false)));
    assert_eq!(
        f("author"),
        (json!({"type":"id","tableName":"users"}), json!(false))
    );
    assert_eq!(
        f("cc"),
        (
            json!({"type":"array","value":{"type":"id","tableName":"users"}}),
            json!(false)
        )
    );
    assert_eq!(f("count"), (json!({"type":"bigint"}), json!(false)));
    assert_eq!(f("ratio"), (json!({"type":"number"}), json!(false)));
    assert_eq!(f("small"), (json!({"type":"number"}), json!(false)));
    assert_eq!(f("flag"), (json!({"type":"boolean"}), json!(false)));
    assert_eq!(f("blob"), (json!({"type":"bytes"}), json!(false)));
    assert_eq!(
        f("note"),
        (
            json!({"type":"union","value":[{"type":"string"},{"type":"null"}]}),
            json!(true)
        )
    );
    assert_eq!(f("nick"), (json!({"type":"string"}), json!(true)));
    assert_eq!(f("retries"), (json!({"type":"number"}), json!(true)));
    assert_eq!(f("type"), (json!({"type":"string"}), json!(false)));
    assert_eq!(
        f("meta").0,
        json!({"type":"record","keys":{"type":"string"},"values":{"fieldType":{"type":"number"},"optional":false}})
    );
    assert_eq!(f("extra"), (json!({"type":"any"}), json!(false)));
    assert_eq!(f("nested").0["type"], "object");
    assert!(fields.get("ignored").is_none());
    assert!(fields.get("body_text").is_none());
}

#[test]
fn enum_validators() {
    assert_eq!(
        json_of::<Status>(),
        json!({"type":"union","value":[
            {"type":"literal","value":"Ok"},
            {"type":"object","value":{"Failed":{"fieldType":{"type":"object","value":{"code":{"fieldType":{"type":"number"},"optional":false}}},"optional":false}}},
            {"type":"object","value":{"Wrapped":{"fieldType":{"type":"string"},"optional":false}}},
        ]})
    );
    assert_eq!(
        json_of::<Event>(),
        json!({"type":"union","value":[
            {"type":"object","value":{"type":{"fieldType":{"type":"literal","value":"started"},"optional":false}}},
            {"type":"object","value":{
                "type":{"fieldType":{"type":"literal","value":"done"},"optional":false},
                "took_ms":{"fieldType":{"type":"number"},"optional":false}}},
        ]})
    );
}

#[test]
fn serde_agrees_with_validator() {
    // The wire shapes serde produces are the ones the validator describes.
    assert_eq!(serde_json::to_value(Status::Ok).unwrap(), json!("Ok"));
    assert_eq!(
        serde_json::to_value(Status::Failed { code: 1.0 }).unwrap(),
        json!({"Failed":{"code":1.0}})
    );
    assert_eq!(
        serde_json::to_value(Event::Done { took_ms: 2.0 }).unwrap(),
        json!({"type":"done","took_ms":2.0})
    );
    let a: SendArgs = serde_json::from_value(json!({
        "bodyText":"hi","author":"u1","cc":[],"count":{"$integer":"AQAAAAAAAAA="},
        "ratio":0.5,"small":3,"flag":true,"blob":{"$bytes":"AQI="},"tags":["a"],
        "type":"t","meta":{"a":1.0},"extra":null,"nested":{"x":1.0}
    }))
    .unwrap();
    assert_eq!(a.count, Int64(1));
    assert_eq!(a.retries, 0);
}

#[test]
fn manifest_embeds_validators() {
    use convex_fn::__rt::{manifest_string, NoValidator as _, Probe};
    let m = manifest_string(
        "app::m",
        "send",
        "public",
        "",
        Probe::<SendArgs>::VALIDATOR,
        Probe::<Vec<Inner>>::VALIDATOR,
    );
    let v: serde_json::Value = serde_json::from_str(m.trim_end()).unwrap();
    assert_eq!(v["args"]["type"], "object");
    assert_eq!(v["returns"]["type"], "array");
    // A type without a validator stays unvalidated.
    struct Plain;
    let m = manifest_string("app::m", "f", "public", "", Probe::<Plain>::VALIDATOR, None);
    assert!(m.contains("\"args\":null,\"returns\":null"));
}

/// Compile-time check: string paths and typed refs both work with `run_*`.
#[allow(dead_code)]
async fn overloads(ctx: &convex_fn::Ctx) -> convex_fn::Result<()> {
    const LIST: FunctionRef<serde_json::Value, Vec<serde_json::Value>> =
        FunctionRef::new("messages:list");
    let a: Vec<serde_json::Value> = ctx.run_query("messages:list", json!({})).await?;
    let b = ctx.run_query(LIST, json!({})).await?;
    let c: Vec<serde_json::Value> = ctx.run_query(&LIST, json!({})).await?;
    let _: serde_json::Value = ctx.run_action(String::from("a:b"), json!({})).await?;
    let _ = (a, b, c);
    Ok(())
}

#[test]
fn function_ref_is_a_copy_const() {
    const LIST: FunctionRef<(), Vec<serde_json::Value>> = FunctionRef::new("messages:list");
    let a = LIST;
    let b = a;
    assert_eq!(a.path(), "messages:list");
    assert_eq!(b.path(), "messages:list");
}

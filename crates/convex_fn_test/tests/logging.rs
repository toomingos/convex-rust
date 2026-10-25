//! DX-05: `log` / `tracing` bridges and nested `block_on`, run natively.
#![allow(unused_imports, dead_code)]

use convex_fn::{json, Ctx, Result, Value};
use convex_fn_test::TestHost;

const D: i32 = convex_fn::__wire::LogLevel::Debug.as_i32();
const I: i32 = convex_fn::__wire::LogLevel::Info.as_i32();
const W: i32 = convex_fn::__wire::LogLevel::Warn.as_i32();
const E: i32 = convex_fn::__wire::LogLevel::Error.as_i32();

#[cfg(feature = "log")]
#[test]
fn log_crate_routes_to_host_with_levels() {
    let host = TestHost::new();
    host.run(|_ctx| async move {
        log::trace!("t");
        log::debug!("d {}", 1);
        log::info!("i");
        log::warn!("w");
        log::error!("e");
    });
    assert_eq!(
        host.logs(),
        vec![
            (D, "t".to_string()),
            (D, "d 1".to_string()),
            (I, "i".to_string()),
            (W, "w".to_string()),
            (E, "e".to_string()),
        ]
    );
}

#[cfg(feature = "log")]
#[convex_fn::action]
async fn logs_in_action(_ctx: &Ctx, _a: Value) -> Result<Value> {
    log::info!("from action");
    Ok(json!(1))
}

#[cfg(feature = "log")]
#[test]
fn log_installed_by_action_entry() {
    let host = TestHost::new();
    let env = host.run_envelope(&json!({}), |c, a: Value| async move {
        logs_in_action(&c, a).await
    });
    assert_eq!(env, json!({"ok": 1}));
    assert_eq!(host.logs(), vec![(I, "from action".to_string())]);
}

#[cfg(feature = "tracing")]
#[test]
fn tracing_events_with_fields_and_spans() {
    let host = TestHost::new();
    host.run(|_ctx| async move {
        tracing::info!("plain");
        tracing::warn!(user = "ann", n = 3, "careful");
        tracing::error!(code = 7);
        let span = tracing::info_span!("req", id = 9);
        let _g = span.enter();
        tracing::debug!("inside");
    });
    assert_eq!(
        host.logs(),
        vec![
            (I, "plain".to_string()),
            (W, "careful user=\"ann\" n=3".to_string()),
            (E, "code=7".to_string()),
            (D, "req{id=9}: inside".to_string()),
        ]
    );
}

#[cfg(feature = "tokio-rt")]
#[test]
fn block_on_inside_action_drives_host_tasks_directly() {
    let host = TestHost::new().on_query("q:get", |_| Ok::<_, String>(json!(5)));
    let out = host.run(|ctx| async move {
        // A nested block_on must not start a second tokio runtime.
        let v: Value = convex_fn::block_on(ctx.run_query("q:get", json!({}))).unwrap();
        let again: Value = ctx.run_query("q:get", json!({})).await.unwrap();
        (v, again)
    });
    assert_eq!(out, (json!(5), json!(5)));
}

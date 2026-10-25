//! cb-* against the mock host. No bench/expected files exist for these
//! (phase0: false); each test emulates the cbTargets.ts function in memory and
//! asserts the result plus the recorded calls. Equivalence with node/cb.ts is
//! described per case.
mod common;

use std::{cell::RefCell, rc::Rc};

use bench_rust::cb::*;
use convex_fn_test::{call, TestHost};
use serde_json::{json, Value};

fn host() -> TestHost {
    // cbTargets:square emulation.
    TestHost::new().on_query("cbTargets:square", |a| {
        let n = a["n"].as_f64().unwrap();
        Ok::<_, String>(json!(n * n))
    })
}

fn squares(n: f64, seed: f64) -> Vec<f64> {
    (0..n as u32).map(|i| (seed + f64::from(i)) * (seed + f64::from(i))).collect()
}

fn check_square_calls(h: &TestHost, n: usize, seed: f64) {
    let calls = h.calls();
    assert_eq!(calls.len(), n);
    for (i, c) in calls.iter().enumerate() {
        assert_eq!(
            (c.kind.as_str(), c.name.as_str()),
            ("query", "cbTargets:square")
        );
        assert_eq!(c.args["n"].as_f64(), Some(seed + i as f64));
    }
}

fn args(id: &str) -> Value {
    common::args(id)
}

// Node: one ctx.runQuery(api.cbTargets.square, {n: seed}).
#[test]
fn cb_001_matches_node() {
    let h = host();
    assert_eq!(call!(h, cb_001, args("cb-001")), json!({"ok": 1.0}));
    check_square_calls(&h, 1, 1.0);
}

// Node: awaited for-loop of 10 runQuery; same order, same results.
#[test]
fn cb_002_matches_node() {
    let h = host();
    assert_eq!(
        call!(h, cb_002, args("cb-002")),
        json!({"ok": squares(10.0, 1.0)})
    );
    check_square_calls(&h, 10, 1.0);
}

// Node: Promise.all of 10 runQuery <-> join_all; results keep input order,
// also when the host delivers them in reverse.
#[test]
fn cb_003_matches_node() {
    let h = host();
    assert_eq!(
        call!(h, cb_003, args("cb-003")),
        json!({"ok": squares(10.0, 1.0)})
    );
    check_square_calls(&h, 10, 1.0);
    let r = host().with_reverse_delivery();
    assert_eq!(
        call!(r, cb_003, args("cb-003")),
        json!({"ok": squares(10.0, 1.0)})
    );
}

// Node: awaited for-loop of 100 runQuery.
#[test]
fn cb_004_matches_node() {
    let h = host();
    assert_eq!(
        call!(h, cb_004, args("cb-004")),
        json!({"ok": squares(100.0, 1.0)})
    );
    check_square_calls(&h, 100, 1.0);
}

// Node: Promise.all of 100 runQuery <-> join_all.
#[test]
fn cb_005_matches_node() {
    let h = host().with_reverse_delivery();
    assert_eq!(
        call!(h, cb_005, args("cb-005")),
        json!({"ok": squares(100.0, 1.0)})
    );
    check_square_calls(&h, 100, 1.0);
}

fn with_insert(h: TestHost) -> (TestHost, Rc<RefCell<Vec<Value>>>) {
    // cbTargets:insertDoc emulation: returns the inserted doc with a fake _id.
    let docs = Rc::new(RefCell::new(Vec::new()));
    let d = docs.clone();
    let h = h.on_mutation("cbTargets:insertDoc", move |a| {
        let mut d = d.borrow_mut();
        let doc = json!({
            "_id": format!("doc{}", d.len()),
            "_creationTime": 1.0,
            "key": a["key"],
            "value": a["value"],
        });
        d.push(doc.clone());
        Ok::<_, String>(doc)
    });
    (h, docs)
}

// Node: one runMutation(insertDoc); compare=shape (ids/times differ).
#[test]
fn cb_006_matches_node() {
    let (h, docs) = with_insert(TestHost::new());
    let out = call!(h, cb_006, args("cb-006"));
    assert_eq!(out["ok"]["key"], 1);
    assert_eq!(out["ok"]["value"], "v1");
    assert_eq!(docs.borrow().len(), 1);
    let c = h.calls();
    assert_eq!(
        (c[0].kind.as_str(), c[0].name.as_str()),
        ("mutation", "cbTargets:insertDoc")
    );
    assert_eq!(c[0].args, json!({"key": 1, "value": "v1"}));
}

// Node: awaited loop of 10 runMutation, keys seed..seed+9.
#[test]
fn cb_007_matches_node() {
    let (h, docs) = with_insert(TestHost::new());
    let out = call!(h, cb_007, args("cb-007"));
    let arr = out["ok"].as_array().unwrap();
    assert_eq!(arr.len(), 10);
    for (i, d) in arr.iter().enumerate() {
        assert_eq!(d["key"], 1.0 + i as f64);
        assert_eq!(d["value"], format!("v{}", 1 + i));
    }
    assert_eq!(docs.borrow().len(), 10);
    assert!(h
        .calls()
        .iter()
        .all(|c| c.kind == "mutation" && c.name == "cbTargets:insertDoc"));
}

fn with_inc(h: TestHost) -> TestHost {
    // cbTargets:incAction emulation: n -> n + 1.
    h.on_action("cbTargets:incAction", |a| {
        Ok::<_, String>(json!(a["n"].as_f64().unwrap() + 1.0))
    })
}

// Node: single runAction(incAction) on the isolate action.
#[test]
fn cb_008_matches_node() {
    let h = with_inc(TestHost::new());
    assert_eq!(call!(h, cb_008, args("cb-008")), json!({"ok": 2.0}));
    let c = h.calls();
    assert_eq!(c.len(), 1);
    assert_eq!(
        (c[0].kind.as_str(), c[0].name.as_str()),
        ("action", "cbTargets:incAction")
    );
}

// Node: three awaited runAction calls, each fed the previous result.
#[test]
fn cb_009_matches_node() {
    let h = with_inc(TestHost::new());
    assert_eq!(call!(h, cb_009, args("cb-009")), json!({"ok": 4.0}));
    let a: Vec<Value> = h.calls().into_iter().map(|c| c.args).collect();
    assert_eq!(a, vec![json!({"n": 1.0}), json!({"n": 2.0}), json!({"n": 3.0})]);
}

// Node: scheduler.runAfter(0, internal.cbTargets.touch, {n: seed}) -> id.
#[test]
fn cb_010_matches_node() {
    let h = TestHost::new();
    let out = call!(h, cb_010, args("cb-010"));
    assert!(out["ok"].is_string());
    let s = h.scheduled();
    assert_eq!(s.len(), 1);
    assert_eq!(
        (s[0].name.as_str(), s[0].args.clone()),
        ("cbTargets:touch", json!({"n": 1}))
    );
    assert_eq!(out["ok"], s[0].id);
}

// Node: 10 awaited scheduler.runAt, 1s apart starting 1s from now.
#[test]
fn cb_011_matches_node() {
    let h = TestHost::new();
    let out = call!(h, cb_011, args("cb-011"));
    let ids = out["ok"].as_array().unwrap();
    assert_eq!(ids.len(), 10);
    let s = h.scheduled();
    assert_eq!(s.len(), 10);
    for (i, j) in s.iter().enumerate() {
        assert_eq!(j.name, "cbTargets:touch");
        assert_eq!(j.args["n"].as_f64(), Some(1.0 + i as f64));
        assert_eq!(ids[i], j.id);
        if i > 0 {
            assert!(
                (j.ts - s[i - 1].ts - 1.0).abs() < 1e-6,
                "runAt spacing is 1 s (ts in seconds)"
            );
        }
    }
}

// Node: storage.store(new Blob([1 MB])) -> {id, size}, then storage.delete; compare=shape.
#[test]
fn cb_012_matches_node() {
    let h = TestHost::new();
    let out = call!(h, cb_012, args("cb-012"));
    assert_eq!(out["ok"]["size"], 1 << 20);
    let id = out["ok"]["id"].as_str().unwrap();
    assert!(h.storage_read(id).is_none(), "blob is deleted after the store");
}

// Node: store then storage.get -> {size, sum, contentType}.
#[test]
fn cb_013_matches_node() {
    let h = TestHost::new();
    let out = call!(h, cb_013, args("cb-013"));
    let sum: u64 = payload(1).iter().map(|&b| u64::from(b)).sum();
    assert_eq!(
        out,
        json!({"ok": {"size": 1 << 20, "sum": sum, "contentType": "application/octet-stream"}})
    );
}

// Node: auth.getUserIdentity -> {subject, issuer} or null.
#[test]
fn cb_014_matches_node() {
    assert_eq!(
        call!(TestHost::new(), cb_014, args("cb-014")),
        json!({"ok": null})
    );
    let h = TestHost::new()
        .with_identity(json!({"subject": "u1", "issuer": "https://iss.test", "name": "x"}));
    assert_eq!(
        call!(h, cb_014, args("cb-014")),
        json!({"ok": {"subject": "u1", "issuer": "https://iss.test"}})
    );
}

// Node: process.env.CB_ENV_0..9 ?? null. (Only test touching the process env.)
#[test]
fn cb_015_matches_node() {
    for i in 0..10 {
        if i == 3 {
            std::env::remove_var("CB_ENV_3");
        } else {
            std::env::set_var(format!("CB_ENV_{i}"), format!("val{i}"));
        }
    }
    let mut want: Vec<Value> = (0..10).map(|i| json!(format!("val{i}"))).collect();
    want[3] = Value::Null;
    assert_eq!(
        call!(TestHost::new(), cb_015, args("cb-015")),
        json!({"ok": want})
    );
}

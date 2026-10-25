mod common;

use bench_rust::ser::*;
use convex_fn_test::{call, TestHost};

#[test]
fn ser_001_matches_node() {
    let id = "ser-001";
    common::check(id, &call!(TestHost::new(), ser_001, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_002_matches_node() {
    let id = "ser-002";
    common::check(id, &call!(TestHost::new(), ser_002, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_003_matches_node() {
    let id = "ser-003";
    common::check(id, &call!(TestHost::new(), ser_003, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_004_matches_node() {
    let id = "ser-004";
    common::check(id, &call!(TestHost::new(), ser_004, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_005_matches_node() {
    let id = "ser-005";
    common::check(id, &call!(TestHost::new(), ser_005, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_006_matches_node() {
    let id = "ser-006";
    common::check(id, &call!(TestHost::new(), ser_006, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_007_matches_node() {
    let id = "ser-007";
    common::check(id, &call!(TestHost::new(), ser_007, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_008_matches_node() {
    let id = "ser-008";
    common::check(id, &call!(TestHost::new(), ser_008, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_009_matches_node() {
    let id = "ser-009";
    common::check(id, &call!(TestHost::new(), ser_009, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

#[test]
fn ser_010_matches_node() {
    let id = "ser-010";
    common::check(id, &call!(TestHost::new(), ser_010, common::args(id)));
    // The generated args themselves must equal the Node-generated args.
    assert!(
        convex_fn_test::compare::matches(
            "exact",
            0.0,
            &common::args(id),
            &common::expected(id)["ok"]
        )
        .is_ok(),
        "{id}: generated args differ from TS"
    );
}

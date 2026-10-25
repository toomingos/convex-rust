mod common;

use bench_rust::base::*;
use convex_fn_test::{call, TestHost};

#[test]
fn base_001_matches_node() {
    common::check(
        "base-001",
        &call!(TestHost::new(), base_001, common::args("base-001")),
    );
}

#[test]
fn base_002_matches_node() {
    common::check(
        "base-002",
        &call!(TestHost::new(), base_002, common::args("base-002")),
    );
}

#[test]
fn base_003_matches_node() {
    common::check(
        "base-003",
        &call!(TestHost::new(), base_003, common::args("base-003")),
    );
}

#[test]
fn base_004_matches_node() {
    common::check(
        "base-004",
        &call!(TestHost::new(), base_004, common::args("base-004")),
    );
}

#[test]
fn base_005_runs() {
    common::check(
        "base-005",
        &call!(TestHost::new(), base_005, common::args("base-005")),
    );
}

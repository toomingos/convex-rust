mod common;

use bench_rust::mem::*;
use convex_fn_test::{call, TestHost};

#[test]
fn mem_001_runs() {
    common::check("mem-001", &call!(TestHost::new(), mem_001, common::args("mem-001")));
}

#[test]
fn mem_002_runs() {
    common::check("mem-002", &call!(TestHost::new(), mem_002, common::args("mem-002")));
}

#[test]
fn mem_003_matches_node() {
    common::check("mem-003", &call!(TestHost::new(), mem_003, common::args("mem-003")));
}

#[test]
fn mem_004_matches_node() {
    common::check("mem-004", &call!(TestHost::new(), mem_004, common::args("mem-004")));
}

#[test]
fn mem_005_matches_node() {
    common::check("mem-005", &call!(TestHost::new(), mem_005, common::args("mem-005")));
}

mod common;

use bench_rust::media::*;
use convex_fn_test::{call, TestHost};

macro_rules! media_test {
    ($name:ident, $id:literal, $f:ident) => {
        #[test]
        fn $name() {
            let out = call!(TestHost::new(), $f, common::args($id));
            if std::env::var_os("MEDIA_DUMP").is_some() {
                eprintln!("{}: {out}", $id);
            }
            common::check($id, &out);
        }
    };
}

media_test!(media_001_matches_node, "media-001", media_001);
media_test!(media_002_matches_node, "media-002", media_002);
media_test!(media_003_matches_node, "media-003", media_003);
media_test!(media_004_matches_node, "media-004", media_004);
media_test!(media_005_matches_node, "media-005", media_005);
media_test!(media_006_matches_node, "media-006", media_006);
media_test!(media_007_matches_node, "media-007", media_007);
media_test!(media_008_matches_node, "media-008", media_008);
media_test!(media_009_matches_node, "media-009", media_009);
media_test!(media_010_matches_node, "media-010", media_010);

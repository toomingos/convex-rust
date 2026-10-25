//! `bench_report_fixture <out-dir> [--seed N] [--warm-n N] [--cases path] [--scale case=factor]...`
use bench_report::fixture::{generate, FixtureConfig};
use bench_report::load::Catalog;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out) = args.next() else {
        eprintln!("usage: bench_report_fixture <out-dir> [--seed N] [--warm-n N] [--cases path] [--scale case=factor]...");
        std::process::exit(2);
    };
    let mut cfg = FixtureConfig::default();
    let mut cases = bench_report::default_cases_path();
    while let Some(a) = args.next() {
        let v = args.next().unwrap_or_default();
        match a.as_str() {
            "--seed" => cfg.seed = v.parse().expect("seed"),
            "--warm-n" => cfg.warm_n = v.parse().expect("warm-n"),
            "--cases" => cases = v.into(),
            "--scale" => {
                let (k, f) = v.split_once('=').expect("case=factor");
                cfg.rust_scale.insert(k.into(), f.parse().expect("factor"));
            }
            o => panic!("unknown flag {o}"),
        }
    }
    let cat = Catalog::load(&cases).expect("cases.json");
    generate(std::path::Path::new(&out), &cat, &cfg).expect("generate");
}

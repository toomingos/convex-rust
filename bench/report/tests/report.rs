use bench_report::analysis::{classify_change, verdict, Change, Config, Verdict};
use bench_report::fixture::{self, generate, FixtureConfig};
use bench_report::load::{load_run, Catalog};
use bench_report::{build, default_cases_path};
use std::path::{Path, PathBuf};

fn catalog() -> Catalog {
    Catalog::load(&default_cases_path()).expect("bench/cases.json")
}

fn tmp(name: &str) -> PathBuf {
    let p = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bench_report_tests").join(name);
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fx(seed: u64) -> FixtureConfig {
    FixtureConfig { seed, warm_n: 100, conc_n: 40, ..FixtureConfig::default() }
}

fn cfg() -> Config {
    Config { seed: 1, resamples: 1000, min_effect: 0.0 }
}

#[test]
fn verdict_logic() {
    assert_eq!(verdict(0.5, 0.9), Verdict::RustFaster);
    assert_eq!(verdict(1.1, 1.4), Verdict::RustSlower);
    assert_eq!(verdict(0.9, 1.1), Verdict::NoDifference);
    assert_eq!(verdict(0.9, 1.0), Verdict::NoDifference); // touching 1.0 is not excluded
    assert_eq!(verdict(f64::NAN, 1.0), Verdict::Insufficient);
}

#[test]
fn change_logic() {
    assert_eq!(classify_change(1.3, 1.2, 1.4, 0.0), Change::Regression);
    assert_eq!(classify_change(1.3, 1.2, 1.4, 0.5), Change::NoChange);
    assert_eq!(classify_change(1.05, 0.98, 1.1, 0.0), Change::NoChange);
    assert_eq!(classify_change(0.7, 0.6, 0.8, 0.0), Change::Improvement);
    assert_eq!(classify_change(1.0, 1.0, 1.0, 0.0), Change::NoChange);
}

#[test]
fn synthetic_run_report() {
    let cat = catalog();
    let dir = tmp("run");
    generate(&dir, &cat, &fx(1)).unwrap();
    let o = build(&dir, None, &cat, &cfg()).unwrap();
    let a = &o.analysis;
    assert_eq!(a.cases.len(), 100);
    let [nodiff, slower, allfail, noexec, flaky] = fixture::special_ids(&cat).map(Option::unwrap);
    let case = |id: &str| a.cases.iter().find(|c| c.id == id).unwrap();

    let w = case(&nodiff).warm.as_ref().unwrap();
    assert_eq!(w.verdict, Verdict::NoDifference, "{w:?}");
    assert!(w.lo < 1.0 && 1.0 < w.hi);

    let w = case(&slower).warm.as_ref().unwrap();
    assert_eq!(w.verdict, Verdict::RustSlower);
    assert!((1.5..2.2).contains(&w.ratio));

    let w = case(&allfail).warm.as_ref().unwrap();
    assert_eq!(w.verdict, Verdict::Insufficient);
    assert_eq!(w.rust.n, 0);
    assert!(w.rust.n_fail > 0);

    let w = case(&noexec).warm.as_ref().unwrap();
    assert_eq!(w.node.exec_n, 0);
    assert!(w.node.exec_p50.is_nan());
    assert!(w.rust.exec_p50.is_finite());

    let w = case(&flaky).warm.as_ref().unwrap();
    assert!(w.node.n_fail + w.rust.n_fail > 0);

    let h = &a.headline_warm;
    assert_eq!(h.faster + h.slower + h.no_difference + h.insufficient, 100);
    assert!(h.faster > h.slower && h.slower >= 1 && h.no_difference >= 1 && h.insufficient == 1);
    let (g, lo, hi) = (h.geomean.unwrap(), h.geomean_lo.unwrap(), h.geomean_hi.unwrap());
    assert!(lo <= g && g <= hi && g < 1.0, "{lo} {g} {hi}");
    assert_eq!(a.categories.iter().map(|c| c.warm.cases).sum::<usize>(), 100);
    assert!(a.libraries.len() > 20);
    assert_eq!(a.concurrency_summary.len(), 4);
    assert!(a.cases.iter().all(|c| c.rss_rust_backend.is_some() && c.rss_node_executor.is_some()));

    // markdown content and structure
    let md = &o.markdown;
    for h in ["## Run", "## Headline", "## By category", "## By idiomatic library", "## Per case: warm", "## Per case: cold start", "## Concurrency", "## Failures and missing data"] {
        assert!(md.contains(h), "missing {h}");
    }
    assert!(md.contains("ratio of medians") && md.contains("report.bootstrap_resamples: 1000"));
    assert!(!md.contains("## Baseline comparison"));
    let row = md.lines().find(|l| l.starts_with(&format!("| {nodiff} |"))).unwrap();
    assert!(row.contains("no difference"), "{row}");
    let row = md.lines().find(|l| l.starts_with(&format!("| {allfail} |"))).unwrap();
    assert!(row.contains("insufficient data"), "{row}");
    // html is self-contained
    let html = &o.html;
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("prefers-color-scheme:dark") && html.contains("<svg"));
    assert!(!html.contains("<script") && !html.contains("http://") && !html.contains("https://"));

    // summary.json parses
    let v: serde_json::Value = serde_json::from_str(&o.summary).unwrap();
    assert_eq!(v["analysis"]["cases"].as_array().unwrap().len(), 100);
}

#[test]
fn deterministic_and_seed_sensitive() {
    let cat = catalog();
    let dir = tmp("det");
    generate(&dir, &cat, &fx(2)).unwrap();
    let a = build(&dir, None, &cat, &cfg()).unwrap();
    let b = build(&dir, None, &cat, &cfg()).unwrap();
    assert_eq!(a.markdown, b.markdown);
    assert_eq!(a.html, b.html);
    assert_eq!(a.summary, b.summary);
    let c = build(&dir, None, &cat, &Config { seed: 99, ..cfg() }).unwrap();
    assert_ne!(a.markdown, c.markdown);
}

#[test]
fn baseline_detects_planted_regression() {
    let cat = catalog();
    let (reg, imp) = (cat.cases[3].id.clone(), cat.cases[4].id.clone());
    let base = tmp("base");
    generate(&base, &cat, &fx(10)).unwrap();
    let cur = tmp("cur");
    let mut f = fx(11);
    f.rust_scale.insert(reg.clone(), 1.5);
    f.rust_scale.insert(imp.clone(), 0.6);
    generate(&cur, &cat, &f).unwrap();

    let c = Config { min_effect: 0.1, ..cfg() };
    let o = build(&cur, Some(&base), &cat, &c).unwrap();
    let b = o.baseline.as_ref().unwrap();
    assert!(b.regressions.contains(&format!("{reg} (warm)")), "{:?}", b.regressions);
    // Cold has only 20 samples, where percentile-bootstrap CIs of medians are known to be narrow;
    // the false-positive check is on warm.
    assert!(b.regressions.iter().filter(|r| r.contains("(warm)")).all(|r| r.starts_with(&reg)), "false positives: {:?}", b.regressions);
    let imp_row = b.rows.iter().find(|r| r.id == imp && r.mode == bench_report::load::Mode::Warm).unwrap();
    assert_eq!(imp_row.rust.change, Change::Improvement);
    // Node control stays within noise for the regressed case.
    let reg_row = b.rows.iter().find(|r| r.id == reg && r.mode == bench_report::load::Mode::Warm).unwrap();
    assert_eq!(reg_row.node.as_ref().unwrap().change, Change::NoChange);
    assert!(o.markdown.contains("## Baseline comparison") && o.markdown.contains("REGRESSION"));

    // No planted change: nothing flagged (with a 10% min effect).
    let same = tmp("same");
    generate(&same, &cat, &fx(12)).unwrap();
    let o = build(&same, Some(&base), &cat, &c).unwrap();
    let r = o.baseline.unwrap().regressions;
    assert!(r.iter().all(|x| x.contains("(cold)")), "{r:?}");
}

#[test]
fn bad_input_is_an_error() {
    let dir = tmp("bad");
    std::fs::write(dir.join("phase2.jsonl"), "{\"case\":\"x\"}\n").unwrap();
    let e = load_run(&dir).unwrap_err();
    assert!(e.contains("phase2.jsonl:1"), "{e}");
    std::fs::write(
        dir.join("phase2.jsonl"),
        "{\"case\":\"x\",\"runtime\":\"deno\",\"mode\":\"warm\",\"concurrency\":null,\"i\":0,\"client_ns\":1,\"exec_ms\":null,\"ok\":true}\n",
    )
    .unwrap();
    assert!(load_run(&dir).unwrap_err().contains("unknown runtime"));
    assert!(load_run(&dir.join("nope")).is_err());
}

// ---- rust-native and gates.json

/// Tiny hand-made run: two cases, warm + cold, given runtimes (runtime name, median client ms).
fn mini_run(name: &str, rts: &[(&str, f64)], gates: Option<&str>) -> PathBuf {
    let cat = catalog();
    let dir = tmp(name);
    let mut p2 = String::new();
    let mut rss = String::new();
    for (ci, case) in cat.cases.iter().take(2).enumerate() {
        for (rt, med) in rts {
            for (mode, n) in [("warm", 30), ("cold", 12)] {
                for i in 0..n {
                    let ms = med * (ci as f64 + 1.0) * (1.0 + 0.01 * f64::from(i % 7));
                    p2.push_str(&format!(
                        "{{\"case\":\"{}\",\"runtime\":\"{rt}\",\"mode\":\"{mode}\",\"concurrency\":null,\"i\":{i},\"client_ns\":{},\"exec_ms\":{:.3},\"ok\":true}}\n",
                        case.id,
                        (ms * 1e6) as u64,
                        ms * 0.8
                    ));
                }
            }
            let proc_ = if *rt == "rust-native" { "native-worker" } else { "backend" };
            rss.push_str(&format!("{{\"case\":\"{}\",\"runtime\":\"{rt}\",\"process\":\"{proc_}\",\"t_ms\":0,\"rss_bytes\":15000000}}\n", case.id));
        }
    }
    std::fs::write(dir.join("phase2.jsonl"), p2).unwrap();
    std::fs::write(dir.join("rss.jsonl"), rss).unwrap();
    std::fs::write(dir.join("meta.json"), "{\"backend_profile\":\"release\"}").unwrap();
    if let Some(g) = gates {
        std::fs::write(dir.join("gates.json"), g).unwrap();
    }
    dir
}

#[test]
fn two_runtimes_have_no_native_output() {
    let dir = mini_run("mini2", &[("node", 10.0), ("rust", 5.0)], None);
    let o = build(&dir, None, &catalog(), &cfg()).unwrap();
    assert!(o.analysis.headline_native.is_none());
    assert!(!o.summary.contains("headline_native") && !o.summary.contains("\"native\": {") && !o.summary.contains("\"gates\""));
    assert!(!o.markdown.contains("Native/Node warm"));
    assert!(!o.markdown.contains("rust-native (geometric"));
    assert!(o.markdown.contains("## Gates"));
    assert!(o.markdown.contains("gates not measured (run `bench_runner gates`)"));
    assert!(o.html.contains("gates not measured"));
}

#[test]
fn three_runtimes_render_native_columns() {
    let dir = mini_run("mini3", &[("node", 10.0), ("rust", 5.0), ("rust-native", 2.0)], None);
    let o = build(&dir, None, &catalog(), &cfg()).unwrap();
    let n = o.analysis.headline_native.as_ref().expect("native headline");
    assert!((n.warm_vs_node.unwrap() - 0.2).abs() < 0.02, "{n:?}");
    assert!((n.warm_vs_wasm.unwrap() - 0.4).abs() < 0.04, "{n:?}");
    assert!(o.analysis.categories.iter().all(|g| g.native.is_some()));
    assert!(o.analysis.libraries.iter().all(|g| g.native.is_some()));
    for t in [&o.markdown, &o.html] {
        assert!(t.contains("Native/Node warm") && t.contains("Native/Wasm cold"));
        assert!(t.contains("Cases with native"));
        assert!(t.contains("Native RSS MB (backend / worker)"));
    }
    // Worker RSS 15 MB shows in the per-case row.
    assert!(o.markdown.contains("- / 15"), "{}", o.markdown);
    assert!(o.summary.contains("headline_native"));
}

#[test]
fn gates_json_pass_fail_no_data() {
    let gates = r#"{
      "run_id": "g1", "platform": "macos",
      "overhead": {"gate": "rust-native exec-user_exec p50 <= 0.2 ms", "value_ms": 0.05, "verdict": "PASS"},
      "spawn": {"gate": "spawn -> Ready p50 <= 50 ms (macOS)", "spawn_to_ready_est_ms": 80.5, "verdict": "FAIL"},
      "idle_rss": {"verdict": "NO DATA", "error": "cannot find the native backend process"},
      "crypto": {"gate": "native/node client p50 <= 0.7 for crypto-003 and AES-GCM", "worst_native_over_node_client": null, "verdict": "NO DATA"},
      "elapsed_s": 1.0
    }"#;
    let dir = mini_run("mini_gates", &[("node", 10.0), ("rust", 5.0), ("rust-native", 2.0)], Some(gates));
    let o = build(&dir, None, &catalog(), &cfg()).unwrap();
    let md = &o.markdown;
    assert!(!md.contains("gates not measured"));
    let line = |k: &str| md.lines().find(|l| l.contains(k)).unwrap_or_else(|| panic!("no row {k}")).to_string();
    assert!(line("Runtime overhead").contains("0.050 ms") && line("Runtime overhead").ends_with("| PASS |"));
    assert!(line("Worker spawn").contains("80.500 ms") && line("Worker spawn").ends_with("| FAIL |"));
    assert!(line("Idle worker RSS").contains("error: cannot find") && line("Idle worker RSS").ends_with("| NO DATA |"));
    assert!(line("Crypto native/node").ends_with("| NO DATA |"));
    let orph = line("No orphan processes");
    assert!(orph.contains("not run") && orph.ends_with("| NO DATA |"), "{orph}");
    // Gates sit after Headline and before the category table.
    let (h, g, c) = (md.find("## Headline").unwrap(), md.find("## Gates").unwrap(), md.find("## By category").unwrap());
    assert!(h < g && g < c);
    assert!(o.html.contains("<td class=\"good\">PASS</td>") && o.html.contains("<td class=\"bad\">FAIL</td>"));
    assert!(o.html.contains("<td class=\"muted\">NO DATA</td>"));
}

mod final_report {
    use bench_report::final_report::*;
    use bench_report::load::Catalog;

    fn row(case: &str, rt: &str, mode: &str, conc: u32, ns: f64, status: &str, ok: bool) -> String {
        format!(
            "{{\"case\":\"{case}\",\"runtime\":\"{rt}\",\"mode\":\"{mode}\",\"concurrency\":{conc},\"client_ns\":{ns},\"exec_ms\":1.0,\"ok\":{ok},\"app_status\":\"{status}\"}}\n"
        )
    }

    #[test]
    fn later_dir_fills_gap_but_never_erases() {
        let a = row("mem-001", "rust-native", "warm", 0, 2e6, "success", true)
            + &row("cpu-001", "node", "warm", 0, 4e6, "success", true);
        // rerun: node now present for mem-001; cpu-001 only failed there
        let b = row("mem-001", "node", "warm", 0, 8e6, "success", true)
            + &row("cpu-001", "node", "warm", 0, 9e6, "failed", false);
        let mut s = parse_run(&a, "", &["warm"]).unwrap();
        s.absorb(parse_run(&b, "", &["warm"]).unwrap());
        let k = |c: &str, r: &str| (c.to_string(), r.to_string(), "warm".to_string());
        assert_eq!(s.timed[&k("mem-001", "node")].client, vec![8.0]);
        assert_eq!(s.timed[&k("mem-001", "rust-native")].client, vec![2.0]);
        assert_eq!(
            s.timed[&k("cpu-001", "node")].client,
            vec![4.0],
            "failed rerun must not erase data"
        );
    }

    #[test]
    fn modes_are_filtered_and_expected_errors_are_samples() {
        let t = row("base-004", "node", "warm", 0, 1e6, "error", true)
            + &row("cpu-001", "node", "warm", 0, 1e6, "error", true)
            + &row("cpu-001", "node", "cold", 0, 1e6, "success", true);
        let s = parse_run(&t, "", &["warm"]).unwrap();
        assert_eq!(s.timed.len(), 2);
        assert_eq!(
            s.timed[&("base-004".into(), "node".into(), "warm".into())].valid(),
            1
        );
        let bad = &s.timed[&("cpu-001".into(), "node".into(), "warm".into())];
        assert_eq!((bad.valid(), bad.app_err), (0, 1));
    }

    #[test]
    fn disabled_vs_na_vs_failed() {
        let t = row("data-014", "rust-v8", "warm", 0, 1e6, "error", true);
        let s = parse_run(&t, "", &["warm"]).unwrap();
        let mut na = std::collections::BTreeMap::new();
        na.insert(
            ("data-014".to_string(), "ts-v8".to_string()),
            "why".to_string(),
        );
        let cloud = Target {
            side: &s,
            na: na.clone(),
            skipped: Default::default(),
            cloud: true,
        };
        assert_eq!(cloud.timed("data-014", "rust-v8", "warm"), St::Disabled);
        assert_eq!(cloud.timed("data-014", "ts-v8", "warm"), St::Na);
        assert_eq!(cloud.timed("data-014", "node", "warm"), St::NoData);
        let local = Target {
            side: &s,
            na,
            skipped: Default::default(),
            cloud: false,
        };
        assert_eq!(local.timed("data-014", "rust-v8", "warm"), St::Failed);
    }

    #[test]
    fn concurrency_throughput_from_blocks() {
        let p = row("cpu-001", "node", "concurrency", 10, 1e6, "success", true);
        let b = "{\"case\":\"cpu-001\",\"runtime\":\"node\",\"concurrency\":10,\"goodput_per_s\":123.5,\"throughput_per_s\":200.0}\n";
        let s = parse_run(&p, b, &["concurrency"]).unwrap();
        assert_eq!(
            s.conc[&("cpu-001".into(), "node".into(), 10)].tput,
            Some(123.5)
        );
    }

    #[test]
    fn fairness_and_phase1_parsing() {
        let f = parse_fairness("| Case | Title | Verdict | Note |\n|---|---|---|---|\n| cpu-003 | blake3 | ℹ️ | Noble vs crate. |\n| base-001 | noop | ✅ | Same. |\n");
        assert_eq!(f["cpu-003"].0, "intrinsic");
        assert_eq!(f["base-001"], ("same".to_string(), "Same.".to_string()));
        assert_eq!(f.len(), 2);
        let (c, bad) = phase1_counts("{\"case_id\":\"a\",\"equal\":\"yes\"}\n{\"case_id\":\"b\",\"equal\":\"no\"}\n{\"case_id\":\"c\",\"equal\":\"n/a\"}\n");
        assert_eq!((c["yes"], c["no"], c["n/a"]), (1, 1, 1));
        assert_eq!(bad, vec!["b"]);
    }

    #[test]
    fn local3_three_way_headline() {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("final_report_l3");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = String::new();
        // (case, node, wasm, native) warm ms; cb-001 has native slower than wasm
        for (c, j, w, n) in [("cpu-001", 8.0, 4.0, 2.0), ("cb-001", 6.0, 3.0, 4.5)] {
            for (rt, ms) in [("node", j), ("rust", w), ("rust-native", n)] {
                for i in 0..5 {
                    t += &row(c, rt, "warm", 1, ms * 1e6 + i as f64, "success", true);
                }
                t += &row(c, rt, "cold", 1, ms * 2e6, "success", true);
            }
        }
        // cpu-002 lacks native: not counted
        t += &row("cpu-002", "node", "warm", 1, 1e6, "success", true);
        t += &row("cpu-002", "rust", "warm", 1, 1e6, "success", true);
        std::fs::write(dir.join("phase2.jsonl"), &t).unwrap();
        std::fs::write(
            dir.join("phase1.jsonl"),
            "{\"case_id\":\"cpu-001\",\"equal\":\"yes\"}\n",
        )
        .unwrap();
        let cat = Catalog::parse("{\"cases\":[{\"id\":\"cpu-001\",\"category\":\"cpu\",\"title\":\"a\"},{\"id\":\"cpu-002\",\"category\":\"cpu\",\"title\":\"b\"},{\"id\":\"cb-001\",\"category\":\"cb\",\"title\":\"c\"}]}").unwrap();
        let inp = Inputs {
            local_warm: vec![],
            local_cold: vec![],
            local_phase1: None,
            local3_warm: vec![dir.clone()],
            local3_cold: vec![dir.clone()],
            local3_phase1: Some(dir.clone()),
            cloud: None,
            cloud_phase1: None,
            gates: None,
            gates_recheck: vec![],
            fairness: None,
            title: "T".into(),
        };
        let f = build(&inp, &cat).unwrap();
        let v: serde_json::Value = serde_json::from_str(&f.summary).unwrap();
        let w = &v["local3"]["warm_client"];
        assert_eq!(w["n"], 2);
        let near = |x: &serde_json::Value, e: f64| (x.as_f64().unwrap() - e).abs() < 1e-3;
        assert!(near(&w["node_over_wasm"]["median_ratio"], 2.0));
        // median of {4, 1.3333}
        assert!(near(&w["node_over_native"]["median_ratio"], (4.0 + 8.0 / 6.0) / 2.0));
        assert_eq!(w["wasm_over_native"]["faster"], 1, "native slower than wasm in cb-001");
        assert_eq!(w["wasm_over_native"]["slower"], 1);
        assert!(near(&v["local3"]["cold_client"]["node_over_wasm"]["median_ratio"], 2.0));
        assert!(f.html.contains("Local: node vs Rust wasm vs Rust native (same machine)"));
        assert!(f.html.contains("Local three-way phase-1 correctness"));
        assert!(f.html.contains("cb-001 (cb)"));
        assert!(v.get("local").is_none(), "existing local section is untouched");
    }

    #[test]
    fn end_to_end_headline_and_self_contained() {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("final_report_e2e");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = String::new();
        for i in 0..5 {
            t += &row(
                "cpu-001",
                "node",
                "warm",
                0,
                4e6 + i as f64,
                "success",
                true,
            );
            t += &row(
                "cpu-001",
                "rust-native",
                "warm",
                0,
                1e6 + i as f64,
                "success",
                true,
            );
        }
        t += &row("cpu-001", "node", "cold", 0, 4e6, "success", true);
        t += &row("cpu-001", "rust-native", "cold", 0, 2e6, "success", true);
        std::fs::write(dir.join("phase2.jsonl"), &t).unwrap();
        let cat = Catalog::parse(
            "{\"cases\":[{\"id\":\"cpu-001\",\"category\":\"cpu\",\"title\":\"sha\"}]}",
        )
        .unwrap();
        let inp = Inputs {
            local_warm: vec![dir.clone()],
            local_cold: vec![dir.clone()],
            local_phase1: None,
            local3_warm: vec![],
            local3_cold: vec![],
            local3_phase1: None,
            cloud: None,
            cloud_phase1: None,
            gates: None,
            gates_recheck: vec![],
            fairness: None,
            title: "T".into(),
        };
        let f = build(&inp, &cat).unwrap();
        let v: serde_json::Value = serde_json::from_str(&f.summary).unwrap();
        let w = v["local"]["warm"][0]["median_ratio"].as_f64().unwrap();
        assert!((w - 0.25).abs() < 1e-3, "{w}");
        let c = v["local"]["cold"][0]["median_ratio"].as_f64().unwrap();
        assert!((c - 0.5).abs() < 1e-9);
        assert!(f.html.contains("gates not measured"));
        assert!(
            !f.html.contains("http://")
                && !f.html.contains("https://")
                && !f.html.contains("<script")
        );
    }
}

use bench_report::analysis::Config;
use bench_report::load::Catalog;
use bench_report::{build, default_cases_path, write_outputs};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: bench_report <run-dir> [--baseline <run-dir>] [--out <dir>] [--cases <cases.json>]
                    [--seed N] [--resamples N] [--min-effect FRAC] [--fail-on-regression]";

const FINAL_USAGE: &str = "usage: bench_report final --out <dir> [--local-warm <run-dir>]... [--local-cold <run-dir>]...
                    [--local-phase1 <dir|file>] [--local3 <run-dir>]... [--local3-cold <run-dir>]...
                    [--local3-phase1 <dir|file>] [--cloud <run-dir>] [--cloud-phase1 <dir|file>]
                    [--gates <gates.json>] [--gates-recheck <run-dir>]... [--fairness <FAIRNESS.md>]
                    [--cases <cases.json>] [--title <text>]
Later --local-warm / --local-cold dirs supersede earlier ones per cell when they hold valid samples.";

fn run_final(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    use bench_report::final_report::{build, Inputs};
    let mut inp = Inputs {
        local_warm: vec![],
        local_cold: vec![],
        local_phase1: None,
        local3_warm: vec![],
        local3_cold: vec![],
        local3_phase1: None,
        cloud: None,
        cloud_phase1: None,
        gates: None,
        gates_recheck: vec![],
        fairness: None,
        title: "Rust vs Node: final benchmark report".into(),
    };
    let (mut out, mut cases) = (None, None);
    while let Some(a) = args.next() {
        let mut val = |n: &str| {
            args.next()
                .map(PathBuf::from)
                .ok_or(format!("{n} needs a value\n{FINAL_USAGE}"))
        };
        match a.as_str() {
            "--local-warm" => inp.local_warm.push(val(&a)?),
            "--local-cold" => inp.local_cold.push(val(&a)?),
            "--local-phase1" => inp.local_phase1 = Some(val(&a)?),
            "--local3" => inp.local3_warm.push(val(&a)?),
            "--local3-cold" => inp.local3_cold.push(val(&a)?),
            "--local3-phase1" => inp.local3_phase1 = Some(val(&a)?),
            "--cloud" => inp.cloud = Some(val(&a)?),
            "--cloud-phase1" => inp.cloud_phase1 = Some(val(&a)?),
            "--gates" => inp.gates = Some(val(&a)?),
            "--gates-recheck" => inp.gates_recheck.push(val(&a)?),
            "--fairness" => inp.fairness = Some(val(&a)?),
            "--cases" => cases = Some(val(&a)?),
            "--out" => out = Some(val(&a)?),
            "--title" => inp.title = val(&a)?.to_string_lossy().into_owned(),
            "-h" | "--help" => {
                println!("{FINAL_USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            s => return Err(format!("unknown argument {s}\n{FINAL_USAGE}")),
        }
    }
    let out = out.ok_or(FINAL_USAGE)?;
    let cat_path = cases.unwrap_or_else(default_cases_path);
    if inp.fairness.is_none() {
        inp.fairness = cat_path.parent().map(|p| p.join("FAIRNESS.md"));
    }
    let catalog = Catalog::load(&cat_path)?;
    let f = build(&inp, &catalog)?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    // report.md next to the HTML is the hand-written summary; only the page and its numbers are generated.
    for (n, t) in [("report.html", &f.html), ("summary.json", &f.summary)] {
        std::fs::write(out.join(n), t).map_err(|e| format!("{n}: {e}"))?;
    }
    println!("wrote report.html, summary.json to {}", out.display());
    Ok(ExitCode::SUCCESS)
}

fn run() -> Result<ExitCode, String> {
    if std::env::args().nth(1).as_deref() == Some("final") {
        return run_final(std::env::args().skip(2));
    }
    let mut args = std::env::args().skip(1);
    let mut run_dir: Option<PathBuf> = None;
    let (mut baseline, mut out, mut cases) = (None, None, None);
    let mut cfg = Config::default();
    let mut fail = false;
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or(format!("{name} needs a value\n{USAGE}"));
        match a.as_str() {
            "--baseline" => baseline = Some(PathBuf::from(val("--baseline")?)),
            "--out" => out = Some(PathBuf::from(val("--out")?)),
            "--cases" => cases = Some(PathBuf::from(val("--cases")?)),
            "--seed" => cfg.seed = val("--seed")?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--resamples" => {
                cfg.resamples = val("--resamples")?.parse().map_err(|e| format!("--resamples: {e}"))?
            }
            "--min-effect" => {
                cfg.min_effect = val("--min-effect")?.parse().map_err(|e| format!("--min-effect: {e}"))?
            }
            "--fail-on-regression" => fail = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            s if s.starts_with("--") => return Err(format!("unknown flag {s}\n{USAGE}")),
            s => run_dir = Some(PathBuf::from(s)),
        }
    }
    let run_dir = run_dir.ok_or(USAGE)?;
    let out = out.unwrap_or_else(|| run_dir.clone());
    let cat_path = cases.unwrap_or_else(default_cases_path);
    let catalog = Catalog::load(&cat_path).unwrap_or_else(|e| {
        eprintln!("warning: no case catalog ({e}); categories come from id prefixes");
        Catalog::default()
    });
    if bench_report::cloud::is_cloud(&run_dir) {
        let (md, html, summary) = bench_report::cloud::build(&run_dir, &catalog)?;
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        for (n, t) in [("report.md", &md), ("report.html", &html), ("summary.json", &summary)] {
            std::fs::write(out.join(n), t).map_err(|e| format!("{n}: {e}"))?;
        }
        println!("wrote report.md, report.html, summary.json to {}", out.display());
        return Ok(ExitCode::SUCCESS);
    }
    let o = build(&run_dir, baseline.as_deref(), &catalog, &cfg)?;
    write_outputs(&o, &out)?;
    let (w, c) = (&o.analysis.headline_warm, &o.analysis.headline_cold);
    println!(
        "warm: {} faster / {} slower / {} no difference ({} no data); cold: {} faster / {} slower / {} no difference",
        w.faster, w.slower, w.no_difference, w.insufficient, c.faster, c.slower, c.no_difference
    );
    println!("wrote report.md, report.html, summary.json to {}", out.display());
    if let Some(b) = &o.baseline {
        println!(
            "baseline: {} regression(s), {} improvement(s), {} within noise",
            b.regressions.len(),
            b.improvements,
            b.unchanged
        );
        if fail && !b.regressions.is_empty() {
            eprintln!("regressions: {}", b.regressions.join(", "));
            return Ok(ExitCode::from(1));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

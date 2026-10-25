//! Benchmark report generator (W4-02).
//!
//! Input: `<run-dir>/phase2.jsonl`, `rss.jsonl`, `meta.json` (contract owned by bench/runner).
//! Output: `report.md`, `report.html`, `summary.json`.

pub mod analysis;
pub mod cloud;
pub mod final_report;
pub mod fixture;
pub mod load;
pub mod render;
pub mod stats;

use analysis::{analyze, compare_baseline, Analysis, BaselineAnalysis, Config};
use load::{load_run, Catalog};
use std::path::{Path, PathBuf};

/// Default location of bench/cases.json (resolved at compile time, then relative to cwd).
pub fn default_cases_path() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cases.json");
    if p.exists() {
        p
    } else {
        PathBuf::from("bench/cases.json")
    }
}

pub struct Outcome {
    pub analysis: Analysis,
    pub baseline: Option<BaselineAnalysis>,
    pub markdown: String,
    pub html: String,
    pub summary: String,
}

/// Pure part: load, analyse, render. Nothing is written.
pub fn build(
    run: &Path,
    baseline: Option<&Path>,
    catalog: &Catalog,
    cfg: &Config,
) -> Result<Outcome, String> {
    let ds = load_run(run)?;
    let analysis = analyze(&ds, catalog, cfg);
    let baseline = match baseline {
        Some(b) => Some(compare_baseline(&ds, &load_run(b)?, cfg)),
        None => None,
    };
    let blocks = render::build_blocks(&analysis, baseline.as_ref());
    let summary = serde_json::to_string_pretty(&serde_json::json!({
        "analysis": analysis,
        "baseline": baseline,
    }))
    .map_err(|e| e.to_string())?;
    Ok(Outcome {
        markdown: render::render_md(&blocks),
        html: render::render_html(&blocks),
        summary,
        analysis,
        baseline,
    })
}

pub fn write_outputs(o: &Outcome, out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for (name, text) in [("report.md", &o.markdown), ("report.html", &o.html), ("summary.json", &o.summary)] {
        std::fs::write(out.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

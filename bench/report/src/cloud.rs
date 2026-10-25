//! Report for `--target cloud` runs (runtimes: node, ts-v8, rust-v8). Reuses the Block/Table model.
//!
//! Timing: `client` = client round trip (POST /api/action), `exec` = server-side executionTime from the
//! function-log stream. Ratios are geometric means over cases of per-case p50 ratios.

use crate::load::Catalog;
use crate::render::{fmt_ms, fmt_ratio, render_html, render_md, Block, Cell, Table, Tone};
use crate::stats::median;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

const RTS: [&str; 3] = ["node", "ts-v8", "rust-v8"];

type Key = (String, String, usize); // (case, runtime, conc) per mode map
#[derive(Default)]
struct Samples {
    client: Vec<f64>,
    exec: Vec<f64>,
    fail: usize,
}

fn geomean(xs: &[f64]) -> f64 {
    let v: Vec<f64> = xs.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
    if v.is_empty() {
        f64::NAN
    } else {
        (v.iter().map(|x| x.ln()).sum::<f64>() / v.len() as f64).exp()
    }
}

fn plain(s: impl Into<String>) -> Cell {
    Cell(s.into(), Tone::Plain)
}

fn ratio_cell(x: f64) -> Cell {
    let t = if !x.is_finite() {
        Tone::Muted
    } else if x < 0.95 {
        Tone::Good
    } else if x > 1.05 {
        Tone::Bad
    } else {
        Tone::Plain
    };
    Cell(fmt_ratio(x), t)
}

pub fn is_cloud(run: &Path) -> bool {
    std::fs::read_to_string(run.join("meta.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| v["target"] == "cloud")
        .unwrap_or(false)
}

/// Returns (markdown, html, summary json).
pub fn build(run: &Path, catalog: &Catalog) -> Result<(String, String, String), String> {
    let meta: Value = std::fs::read_to_string(run.join("meta.json"))
        .map_err(|e| format!("meta.json: {e}"))
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))?;
    let text = std::fs::read_to_string(run.join("phase2.jsonl")).map_err(|e| format!("phase2.jsonl: {e}"))?;
    let mut warm: BTreeMap<Key, Samples> = BTreeMap::new();
    let mut cold: BTreeMap<Key, Samples> = BTreeMap::new();
    let mut conc: BTreeMap<Key, Samples> = BTreeMap::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(l).map_err(|e| e.to_string())?;
        let mode = v["mode"].as_str().unwrap_or("");
        let map = match mode {
            "warm" => &mut warm,
            "cold" => &mut cold,
            "concurrency" => &mut conc,
            _ => continue,
        };
        let c = if mode == "concurrency" { v["concurrency"].as_u64().unwrap_or(1) as usize } else { 1 };
        let e = map
            .entry((v["case"].as_str().unwrap_or("").into(), v["runtime"].as_str().unwrap_or("").into(), c))
            .or_default();
        // failed calls (app error) are not timed as results
        if v["ok"] == true && v["app_status"] != "error" || v["app_status"] == "success" {
            if let Some(x) = v["client_ns"].as_f64() {
                e.client.push(x / 1e6);
            }
            if let Some(x) = v["exec_ms"].as_f64() {
                e.exec.push(x);
            }
        } else {
            e.fail += 1;
        }
    }
    let cat_of = |id: &str| -> String {
        catalog
            .cases
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.category.clone())
            .unwrap_or_else(|| id.split('-').next().unwrap_or("?").to_string())
    };
    let mut cases: Vec<String> = warm.keys().chain(cold.keys()).chain(conc.keys()).map(|k| k.0.clone()).collect();
    cases.sort();
    cases.dedup();
    let p50 = |m: &BTreeMap<Key, Samples>, c: &str, rt: &str, n: usize, exec: bool| -> f64 {
        m.get(&(c.into(), rt.into(), n))
            .map(|s| {
                let v = if exec { &s.exec } else { &s.client };
                if v.is_empty() {
                    f64::NAN
                } else {
                    median(v)
                }
            })
            .unwrap_or(f64::NAN)
    };

    let mut blocks = vec![Block::H2("Convex cloud: node vs ts-v8 vs rust-v8".into())];
    let s = |p: &str| meta[p].as_str().unwrap_or("?").to_string();
    let cl = &meta["cloud"];
    blocks.push(Block::Facts(vec![
        ("deployment".into(), cl["url"].as_str().unwrap_or("?").into()),
        ("run id".into(), s("run_id")),
        ("convex CLI".into(), cl["convex_cli"].as_str().unwrap_or("?").into()),
        ("config".into(), format!("warmup {}, iters {}, concurrency {}, cold {}", meta["options"]["warmup"], meta["options"]["iters"], meta["options"]["concurrency"], meta["options"]["cold_restarts"])),
        ("skipped".into(), "net category (calls a localhost mock server the cloud cannot reach)".into()),
        ("ts-v8 n/a".into(), format!("{} cases (Node built-ins / native addons / Buffer / 64 MB limit); see table below", cl["ts_v8_na"].as_object().map_or(0, |m| m.len()))),
        ("node n/a".into(), format!("{} cases (Node action OOM at 512 MB on the cloud)", cl["node_na"].as_object().map_or(0, |m| m.len()))),
    ]));
    blocks.push(Block::Para(
        "Timing. client = round trip of POST /api/action measured by the runner (includes network from this machine to the cloud region, so it is the same constant for all runtimes but dominates short cases). exec = server-side executionTime from GET /api/stream_function_logs (Completion entries matched by Convex-Request-Id). Cold = the first call of each function after a redeploy that changes every function module (a nonce constant), repeated for each cold round. Warm = after warmup. T = ts-v8, R = rust-v8, J = node. A ratio below 1 means the numerator runtime is faster. Cells are geometric means over cases of per-case p50 ratios; a case only counts when both runtimes returned a successful result.".into(),
    ));

    // per-category geomeans
    let mut cats: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in &cases {
        cats.entry(cat_of(c)).or_default().push(c.clone());
    }
    let specs: [(&str, &BTreeMap<Key, Samples>, usize, bool); 6] = [
        ("warm client", &warm, 1, false),
        ("warm exec", &warm, 1, true),
        ("cold client", &cold, 1, false),
        ("cold exec", &cold, 1, true),
        ("c=10 client", &conc, 10, false),
        ("c=1 client", &conc, 1, false),
    ];
    let pairs = [("ts-v8", "node", "T/J"), ("rust-v8", "node", "R/J"), ("rust-v8", "ts-v8", "R/T")];
    let mut summary = serde_json::Map::new();
    for (label, m, n, exec) in specs {
        blocks.push(Block::H3(format!("Ratios, {label} (geomean of per-case p50 ratios)")));
        let mut rows = vec![];
        let mut all: Vec<Vec<f64>> = vec![vec![]; 3];
        for (cat, cs) in &cats {
            let mut row = vec![plain(cat.clone()), plain(cs.len().to_string())];
            for (i, (a, b, _)) in pairs.iter().enumerate() {
                let rs: Vec<f64> = cs.iter().map(|c| p50(m, c, a, n, exec) / p50(m, c, b, n, exec)).filter(|x| x.is_finite()).collect();
                all[i].extend(rs.iter());
                row.push(ratio_cell(geomean(&rs)));
                row.push(plain(rs.len().to_string()));
            }
            rows.push(row);
        }
        let mut row = vec![plain("all"), plain(cases.len().to_string())];
        let mut sm = serde_json::Map::new();
        for (i, (_, _, nm)) in pairs.iter().enumerate() {
            let g = geomean(&all[i]);
            row.push(ratio_cell(g));
            row.push(plain(all[i].len().to_string()));
            sm.insert((*nm).into(), serde_json::json!(g));
        }
        rows.push(row);
        summary.insert(label.into(), Value::Object(sm));
        blocks.push(Block::Table(Table {
            headers: vec!["category", "cases", "T/J", "n", "R/J", "n", "R/T", "n"],
            rows,
        }));
    }

    // per-case table
    blocks.push(Block::H3("Per case (p50 ms)".into()));
    let mut rows = vec![];
    for c in &cases {
        let w = |rt, e| fmt_ms(p50(&warm, c, rt, 1, e));
        let cd = |rt| fmt_ms(p50(&cold, c, rt, 1, false));
        let c10 = |rt| fmt_ms(p50(&conc, c, rt, 10, false));
        let fails: usize = RTS.iter().map(|rt| warm.get(&(c.clone(), (*rt).into(), 1)).map_or(0, |s| s.fail)).sum();
        rows.push(vec![
            plain(c.clone()),
            plain(w("node", false)),
            plain(w("ts-v8", false)),
            plain(w("rust-v8", false)),
            plain(w("node", true)),
            plain(w("ts-v8", true)),
            plain(w("rust-v8", true)),
            plain(cd("node")),
            plain(cd("ts-v8")),
            plain(cd("rust-v8")),
            plain(c10("node")),
            plain(c10("ts-v8")),
            plain(c10("rust-v8")),
            plain(if fails > 0 { fails.to_string() } else { String::new() }),
        ]);
    }
    blocks.push(Block::Table(Table {
        headers: vec!["case", "J warm", "T warm", "R warm", "J exec", "T exec", "R exec", "J cold", "T cold", "R cold", "J c10", "T c10", "R c10", "warm fails"],
        rows,
    }));

    // n/a tables
    for (title, key) in [("ts-v8 not applicable", "ts_v8_na"), ("node not applicable on the cloud", "node_na")] {
        if let Some(m) = cl[key].as_object() {
            if !m.is_empty() {
                blocks.push(Block::H3(title.into()));
                blocks.push(Block::Table(Table {
                    headers: vec!["case", "reason"],
                    rows: m.iter().map(|(k, v)| vec![plain(k.clone()), plain(v.as_str().unwrap_or(""))]).collect(),
                }));
            }
        }
    }
    if let Some(a) = cl["ts_v8_overridden"].as_array() {
        if !a.is_empty() {
            blocks.push(Block::Para(format!(
                "ts-v8 uses a web-standard replacement for the Node-specific part of these cases (same timed work, e.g. crypto.subtle for createHash): {}.",
                a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }
    }
    let summary = serde_json::to_string_pretty(&serde_json::json!({"target": "cloud", "geomeans": summary, "cases": cases.len()})).map_err(|e| e.to_string())?;
    Ok((render_md(&blocks), render_html(&blocks), summary))
}

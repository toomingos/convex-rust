//! Synthetic run generator (same on-disk contract as bench/runner's phase 2 output).

use crate::load::Catalog;
use crate::stats::{derive_seed, Rng};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct FixtureConfig {
    /// Noise seed (samples).
    pub seed: u64,
    /// Seed for per-case "true" parameters. Same value + different `seed` = a re-run.
    pub param_seed: u64,
    pub warm_n: usize,
    pub cold_n: usize,
    pub conc_n: usize,
    /// case id -> multiplier applied to Rust latency (to plant regressions/improvements).
    pub rust_scale: BTreeMap<String, f64>,
}

impl Default for FixtureConfig {
    fn default() -> Self {
        FixtureConfig {
            seed: 1,
            param_seed: 7,
            warm_n: 500,
            cold_n: 20,
            conc_n: 100,
            rust_scale: BTreeMap::new(),
        }
    }
}

/// Case indices (into cases.json order) with special behaviour.
pub const IDX_NO_DIFF: usize = 7;
pub const IDX_SLOWER: usize = 11;
pub const IDX_RUST_ALL_FAIL: usize = 13;
pub const IDX_NODE_NO_EXEC: usize = 17;
pub const IDX_FLAKY: usize = 19;
pub const LEVELS: [u32; 4] = [1, 10, 50, 200];

/// Ids of the specially-behaving cases for a catalog.
pub fn special_ids(cat: &Catalog) -> [Option<String>; 5] {
    let g = |i: usize| cat.cases.get(i).map(|c| c.id.clone());
    [g(IDX_NO_DIFF), g(IDX_SLOWER), g(IDX_RUST_ALL_FAIL), g(IDX_NODE_NO_EXEC), g(IDX_FLAKY)]
}

fn lognormal(rng: &mut Rng, median: f64, sigma: f64) -> f64 {
    median * (sigma * rng.normal()).exp()
}

struct Params {
    node_ms: f64,
    ratio: f64,
}

fn params(cfg: &FixtureConfig, idx: usize, id: &str) -> Params {
    let mut rng = Rng::new(derive_seed(cfg.param_seed, &[id, "params"]));
    let node_ms = lognormal(&mut rng, 6.0, 1.0).clamp(0.3, 400.0);
    let mut ratio = (-0.35 + 0.55 * rng.normal()).exp();
    match idx {
        IDX_NO_DIFF => ratio = 1.0,
        IDX_SLOWER => ratio = 1.8,
        _ => {}
    }
    if let Some(s) = cfg.rust_scale.get(id) {
        ratio *= s;
    }
    Params { node_ms, ratio }
}

pub fn generate(dir: &Path, cat: &Catalog, cfg: &FixtureConfig) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut p2 = String::new();
    let mut rss = String::new();
    for (idx, case) in cat.cases.iter().enumerate() {
        let id = case.id.as_str();
        let p = params(cfg, idx, id);
        let mut rng = Rng::new(derive_seed(cfg.seed, &[id, "samples"]));
        let flaky = idx == IDX_FLAKY;
        let mut emit = |rt: &str, mode: &str, conc: Option<u32>, n: usize, med: f64, sigma: f64, overhead: f64| {
            for i in 0..n {
                let mut exec = lognormal(&mut rng, med, sigma);
                if rng.f64() < 0.03 {
                    exec *= 2.5; // tail spikes
                }
                let client = exec + lognormal(&mut rng, overhead, 0.2);
                let ok = if rt == "rust" && idx == IDX_RUST_ALL_FAIL {
                    false
                } else {
                    rng.f64() >= if flaky { 0.04 } else { 0.002 }
                };
                let exec_missing = (rt == "node" && idx == IDX_NODE_NO_EXEC) || rng.f64() < 0.05 || !ok;
                let _ = writeln!(
                    p2,
                    "{{\"case\":\"{id}\",\"runtime\":\"{rt}\",\"mode\":\"{mode}\",\"concurrency\":{},\"i\":{i},\"client_ns\":{},\"exec_ms\":{},\"ok\":{ok}}}",
                    conc.map_or("null".to_string(), |c| c.to_string()),
                    (client * 1e6).round() as u64,
                    if exec_missing { "null".to_string() } else { format!("{exec:.4}") },
                );
            }
        };
        let node_over = 1.2;
        let rust_over = 1.0;
        let rust_ms = p.node_ms * p.ratio;
        // Interleaved order is irrelevant to the report; node first, then rust.
        emit("node", "cold", None, cfg.cold_n, p.node_ms * 6.0, 0.3, node_over);
        emit("rust", "cold", None, cfg.cold_n, rust_ms * 2.5, 0.3, rust_over);
        emit("node", "warm", None, cfg.warm_n, p.node_ms, 0.25, node_over);
        emit("rust", "warm", None, cfg.warm_n, rust_ms, 0.25, rust_over);
        for &c in &LEVELS {
            let cf = f64::from(c);
            emit("node", "concurrency", Some(c), cfg.conc_n, p.node_ms * (1.0 + 0.02 * cf), 0.3, node_over);
            emit("rust", "concurrency", Some(c), cfg.conc_n, rust_ms * (1.0 + 0.008 * cf), 0.3, rust_over);
        }
        for t in 0..5 {
            let tms = t * 1000;
            let base = lognormal(&mut rng, 160e6, 0.15);
            let _ = writeln!(rss, "{{\"case\":\"{id}\",\"runtime\":\"node\",\"process\":\"backend\",\"t_ms\":{tms},\"rss_bytes\":{}}}", base as u64);
            let ex = lognormal(&mut rng, 95e6, 0.2) + f64::from(t) * 3e6;
            let _ = writeln!(rss, "{{\"case\":\"{id}\",\"runtime\":\"node\",\"process\":\"node_executor\",\"t_ms\":{tms},\"rss_bytes\":{}}}", ex as u64);
            let rb = base * 1.05 + lognormal(&mut rng, 20e6, 0.3);
            let _ = writeln!(rss, "{{\"case\":\"{id}\",\"runtime\":\"rust\",\"process\":\"backend\",\"t_ms\":{tms},\"rss_bytes\":{}}}", rb as u64);
        }
    }
    fs::write(dir.join("phase2.jsonl"), p2).map_err(|e| e.to_string())?;
    fs::write(dir.join("rss.jsonl"), rss).map_err(|e| e.to_string())?;
    let meta = serde_json::json!({
        "git": {"repo": "0123abc (synthetic)", "fork": "4567def (synthetic)"},
        "machine": {"cpu": "synthetic fixture", "cores": 16, "os": "linux", "governor": "performance"},
        "backend_profile": "release",
        "node_version": "v22.0.0",
        "toolchain": {"rust": "nightly-2026-06-28", "wasmtime": "50.0.0"},
        "started_at": "2026-01-01T00:00:00Z",
        "args": ["--fixture", format!("seed={}", cfg.seed)]
    });
    fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(())
}

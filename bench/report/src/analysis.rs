//! Turns a loaded run into per-case comparisons, aggregates and baseline deltas.

use crate::load::{Catalog, CaseInfo, Dataset, Mode, Runtime, Series};
use crate::stats::{bootstrap_ratio, derive_seed, geomean_ci, mean, quantile};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Fewer successful samples than this on either side => no verdict.
pub const MIN_SAMPLES: usize = 5;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub seed: u64,
    pub resamples: usize,
    /// Baseline gate: a change must also exceed this relative size to count (0 = CI only).
    pub min_effect: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config { seed: 1, resamples: 10_000, min_effect: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    RustFaster,
    RustSlower,
    NoDifference,
    Insufficient,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::RustFaster => "Rust faster",
            Verdict::RustSlower => "Rust slower",
            Verdict::NoDifference => "no difference",
            Verdict::Insufficient => "insufficient data",
        }
    }
}

/// Ratio = Rust/Node, so below 1.0 means Rust is faster. A CI crossing 1.0 is "no difference".
pub fn verdict(lo: f64, hi: f64) -> Verdict {
    if !(lo.is_finite() && hi.is_finite()) {
        Verdict::Insufficient
    } else if hi < 1.0 {
        Verdict::RustFaster
    } else if lo > 1.0 {
        Verdict::RustSlower
    } else {
        Verdict::NoDifference
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SideStats {
    pub n: usize,
    pub n_fail: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    /// Median backend-recorded execution time (ms); NaN when no sample has it.
    pub exec_p50: f64,
    pub exec_n: usize,
    pub exec_missing: usize,
}

impl SideStats {
    fn from(s: Option<&Series>) -> SideStats {
        let Some(s) = s else {
            return SideStats {
                n: 0,
                n_fail: 0,
                p50: f64::NAN,
                p95: f64::NAN,
                p99: f64::NAN,
                exec_p50: f64::NAN,
                exec_n: 0,
                exec_missing: 0,
            };
        };
        SideStats {
            n: s.client_ms.len(),
            n_fail: s.n_fail,
            p50: quantile(&s.client_ms, 0.50),
            p95: quantile(&s.client_ms, 0.95),
            p99: quantile(&s.client_ms, 0.99),
            exec_p50: quantile(&s.exec_ms, 0.50),
            exec_n: s.exec_ms.len(),
            exec_missing: s.n_missing_exec,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Cmp {
    pub node: SideStats,
    pub rust: SideStats,
    /// rust-native samples, when the run has the runtime.
    pub native: Option<SideStats>,
    /// median(native) / median(node) and median(native) / median(rust-wasm), bootstrap point estimates.
    pub native_vs_node: Option<f64>,
    pub native_vs_wasm: Option<f64>,
    /// median(rust) / median(node)
    pub ratio: f64,
    pub lo: f64,
    pub hi: f64,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub category: String,
    pub title: String,
    pub library: String,
    pub warm: Option<Cmp>,
    pub cold: Option<Cmp>,
    pub rss_node_backend: Option<u64>,
    pub rss_node_executor: Option<u64>,
    pub rss_rust_backend: Option<u64>,
    pub rss_native_backend: Option<u64>,
    pub rss_native_worker: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Agg {
    pub cases: usize,
    pub faster: usize,
    pub slower: usize,
    pub no_difference: usize,
    pub insufficient: usize,
    /// Geometric mean of Rust/Node ratios over cases with a verdict, with bootstrap CI.
    pub geomean: Option<f64>,
    pub geomean_lo: Option<f64>,
    pub geomean_hi: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Group {
    pub name: String,
    pub warm: Agg,
    pub cold: Agg,
    /// rust-native geomeans, only for runs that have the runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeAgg>,
}

/// Plain geometric means (no CI) of the per-case native ratios over the cases that have them.
#[derive(Clone, Debug, Default, Serialize)]
pub struct NativeAgg {
    pub warm_vs_node: Option<f64>,
    pub warm_vs_wasm: Option<f64>,
    pub cold_vs_node: Option<f64>,
    pub cold_vs_wasm: Option<f64>,
    /// Cases with a native warm series.
    pub cases: usize,
}

fn native_agg(members: &[&CaseResult]) -> Option<NativeAgg> {
    let gm = |f: &dyn Fn(&CaseResult) -> Option<f64>| {
        let v: Vec<f64> = members.iter().filter_map(|c| f(c)).filter(|x| x.is_finite() && *x > 0.0).collect();
        (!v.is_empty()).then(|| geomean_plain(&v))
    };
    let cases = members.iter().filter(|c| c.warm.as_ref().is_some_and(|w| w.native.is_some())).count();
    let cold_any = members.iter().any(|c| c.cold.as_ref().is_some_and(|w| w.native.is_some()));
    if cases == 0 && !cold_any {
        return None;
    }
    Some(NativeAgg {
        warm_vs_node: gm(&|c| c.warm.as_ref()?.native_vs_node),
        warm_vs_wasm: gm(&|c| c.warm.as_ref()?.native_vs_wasm),
        cold_vs_node: gm(&|c| c.cold.as_ref()?.native_vs_node),
        cold_vs_wasm: gm(&|c| c.cold.as_ref()?.native_vs_wasm),
        cases,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct ConcSide {
    pub n: usize,
    pub n_fail: usize,
    /// Little's law estimate: concurrency / mean latency.
    pub throughput_rps: f64,
    pub p95: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConcCase {
    pub id: String,
    /// level -> (node, rust)
    pub levels: BTreeMap<u32, (Option<ConcSide>, Option<ConcSide>)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConcLevelSummary {
    pub level: u32,
    pub cases: usize,
    /// geomean over cases of rust_throughput / node_throughput (higher = Rust better)
    pub throughput_ratio: f64,
    /// geomean over cases of rust_p95 / node_p95 (lower = Rust better)
    pub p95_ratio: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub seed: u64,
    pub resamples: usize,
    pub meta: serde_json::Value,
    pub notes: Vec<String>,
    pub headline_warm: Agg,
    pub headline_cold: Agg,
    pub categories: Vec<Group>,
    pub libraries: Vec<Group>,
    pub cases: Vec<CaseResult>,
    pub concurrency: Vec<ConcCase>,
    pub concurrency_summary: Vec<ConcLevelSummary>,
    pub failures: Vec<String>,
    /// rust-native vs node / wasm over all cases; None when the run has no rust-native rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline_native: Option<NativeAgg>,
    /// Contents of gates.json (written by `bench_runner gates`), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gates: Option<serde_json::Value>,
}

struct Internal {
    ln: f64,
    logs: Vec<f64>,
    verdict: Verdict,
}

fn compare(
    ds: &Dataset,
    id: &str,
    mode: Mode,
    cfg: &Config,
) -> Option<(Cmp, Option<Internal>)> {
    let n = ds.get(id, Runtime::Node, mode);
    let r = ds.get(id, Runtime::Rust, mode);
    let x = ds.get(id, Runtime::Native, mode);
    if n.is_none() && r.is_none() && x.is_none() {
        return None;
    }
    let native = x.map(|s| SideStats::from(Some(s)));
    let point = |a: Option<&Series>, b: Option<&Series>, tag: &str| -> Option<f64> {
        let (a, b) = (a?, b?);
        if a.client_ms.len() < MIN_SAMPLES || b.client_ms.len() < MIN_SAMPLES {
            return None;
        }
        let seed = derive_seed(cfg.seed, &[id, mode.name(), tag]);
        Some(bootstrap_ratio(&a.client_ms, &b.client_ms, cfg.resamples, seed).point)
    };
    let native_vs_node = point(x, n, "native-node");
    let native_vs_wasm = point(x, r, "native-wasm");
    let node = SideStats::from(n);
    let rust = SideStats::from(r);
    let enough = node.n >= MIN_SAMPLES && rust.n >= MIN_SAMPLES;
    if !enough {
        let cmp = Cmp {
            node,
            rust,
            native,
            native_vs_node,
            native_vs_wasm,
            ratio: f64::NAN,
            lo: f64::NAN,
            hi: f64::NAN,
            verdict: Verdict::Insufficient,
        };
        return Some((cmp, None));
    }
    let seed = derive_seed(cfg.seed, &[id, mode.name(), "ratio"]);
    let b = bootstrap_ratio(&r?.client_ms, &n?.client_ms, cfg.resamples, seed);
    let v = verdict(b.lo, b.hi);
    let cmp = Cmp { node, rust, native, native_vs_node, native_vs_wasm, ratio: b.point, lo: b.lo, hi: b.hi, verdict: v };
    Some((cmp, Some(Internal { ln: b.point.ln(), logs: b.logs, verdict: v })))
}

fn aggregate(items: &[&Internal], insufficient: usize) -> Agg {
    let mut a = Agg { cases: items.len() + insufficient, insufficient, ..Agg::default() };
    for i in items {
        match i.verdict {
            Verdict::RustFaster => a.faster += 1,
            Verdict::RustSlower => a.slower += 1,
            _ => a.no_difference += 1,
        }
    }
    let pairs: Vec<(f64, &[f64])> = items.iter().map(|i| (i.ln, i.logs.as_slice())).collect();
    if let Some((g, lo, hi)) = geomean_ci(&pairs) {
        a.geomean = Some(g);
        a.geomean_lo = Some(lo);
        a.geomean_hi = Some(hi);
    }
    a
}

fn peak(ds: &Dataset, id: &str, rt: Runtime, process: &str) -> Option<u64> {
    ds.rss.get(&(id.to_string(), rt, process.to_string())).copied()
}

fn conc_side(s: Option<&Series>, level: u32) -> Option<ConcSide> {
    let s = s?;
    if s.client_ms.is_empty() {
        return None;
    }
    let mean_s = mean(&s.client_ms) / 1000.0;
    Some(ConcSide {
        n: s.client_ms.len(),
        n_fail: s.n_fail,
        throughput_rps: f64::from(level) / mean_s,
        p95: quantile(&s.client_ms, 0.95),
    })
}

fn geomean_plain(v: &[f64]) -> f64 {
    (v.iter().map(|x| x.ln()).sum::<f64>() / v.len() as f64).exp()
}

pub fn analyze(ds: &Dataset, cat: &Catalog, cfg: &Config) -> Analysis {
    // Case order: catalog order first, then unknown ids found in the data.
    let mut order: Vec<CaseInfo> = cat.cases.clone();
    for id in ds.case_ids() {
        if cat.get(&id).is_none() {
            let category = id.split('-').next().unwrap_or("?").to_string();
            order.push(CaseInfo { id, category, title: String::new(), node_lib: None, rust_lib: None });
        }
    }

    let mut cases = Vec::new();
    let mut internals: BTreeMap<(String, Mode), Internal> = BTreeMap::new();
    let mut failures = Vec::new();
    for info in &order {
        let warm = compare(ds, &info.id, Mode::Warm, cfg);
        let cold = compare(ds, &info.id, Mode::Cold, cfg);
        let has_conc = ds.series.keys().any(|k| k.0 == info.id && k.2 == Mode::Concurrency);
        if warm.is_none() && cold.is_none() && !has_conc {
            continue;
        }
        let mut side = |mode: Mode, c: Option<(Cmp, Option<Internal>)>| -> Option<Cmp> {
            let (cmp, int) = c?;
            let mut sides = vec![("node", &cmp.node), ("rust", &cmp.rust)];
            if let Some(n) = &cmp.native {
                sides.push(("rust-native", n));
            }
            for (name, s) in sides {
                if s.n_fail > 0 || s.n == 0 {
                    failures.push(format!(
                        "{} {} {}: {} failed, {} ok",
                        info.id,
                        name,
                        mode.name(),
                        s.n_fail,
                        s.n
                    ));
                }
            }
            if let Some(i) = int {
                internals.insert((info.id.clone(), mode), i);
            }
            Some(cmp)
        };
        let warm = side(Mode::Warm, warm);
        let cold = side(Mode::Cold, cold);
        cases.push(CaseResult {
            id: info.id.clone(),
            category: info.category.clone(),
            title: info.title.clone(),
            library: info.lib_label(),
            warm,
            cold,
            rss_node_backend: peak(ds, &info.id, Runtime::Node, "backend"),
            rss_node_executor: peak(ds, &info.id, Runtime::Node, "node_executor"),
            rss_rust_backend: peak(ds, &info.id, Runtime::Rust, "backend"),
            rss_native_backend: peak(ds, &info.id, Runtime::Native, "backend"),
            rss_native_worker: peak(ds, &info.id, Runtime::Native, "native-worker"),
        });
    }

    let group_by = |key: &dyn Fn(&CaseResult) -> String| -> Vec<Group> {
        let mut names: Vec<String> = Vec::new();
        for c in &cases {
            let k = key(c);
            if !names.contains(&k) {
                names.push(k);
            }
        }
        names
            .into_iter()
            .map(|name| {
                let members: Vec<&CaseResult> = cases.iter().filter(|c| key(c) == name).collect();
                let agg = |mode: Mode| {
                    let mut items = Vec::new();
                    let mut insuff = 0;
                    for m in &members {
                        let has = match mode {
                            Mode::Warm => m.warm.is_some(),
                            _ => m.cold.is_some(),
                        };
                        if !has {
                            continue;
                        }
                        match internals.get(&(m.id.clone(), mode)) {
                            Some(i) => items.push(i),
                            None => insuff += 1,
                        }
                    }
                    aggregate(&items, insuff)
                };
                Group { name, warm: agg(Mode::Warm), cold: agg(Mode::Cold), native: native_agg(&members) }
            })
            .collect()
    };
    let categories = group_by(&|c| c.category.clone());
    let mut libraries = group_by(&|c| c.library.clone());
    libraries.sort_by(|a, b| a.name.cmp(&b.name));

    let headline = |mode: Mode| {
        let items: Vec<&Internal> =
            internals.iter().filter(|(k, _)| k.1 == mode).map(|(_, v)| v).collect();
        let insuff = cases
            .iter()
            .filter(|c| {
                let cmp = if mode == Mode::Warm { &c.warm } else { &c.cold };
                cmp.as_ref().is_some_and(|x| x.verdict == Verdict::Insufficient)
            })
            .count();
        aggregate(&items, insuff)
    };

    // Concurrency.
    let levels: BTreeSet<u32> =
        ds.series.keys().filter(|k| k.2 == Mode::Concurrency).map(|k| k.3).collect();
    let mut concurrency = Vec::new();
    for c in &cases {
        let mut lv = BTreeMap::new();
        for &l in &levels {
            let get = |rt| ds.series.get(&(c.id.clone(), rt, Mode::Concurrency, l));
            let (n, r) = (conc_side(get(Runtime::Node), l), conc_side(get(Runtime::Rust), l));
            if n.is_some() || r.is_some() {
                lv.insert(l, (n, r));
            }
        }
        if !lv.is_empty() {
            concurrency.push(ConcCase { id: c.id.clone(), levels: lv });
        }
    }
    let mut concurrency_summary = Vec::new();
    for &l in &levels {
        let mut tp = Vec::new();
        let mut p95 = Vec::new();
        for c in &concurrency {
            if let Some((Some(n), Some(r))) = c.levels.get(&l) {
                tp.push(r.throughput_rps / n.throughput_rps);
                p95.push(r.p95 / n.p95);
            }
        }
        if !tp.is_empty() {
            concurrency_summary.push(ConcLevelSummary {
                level: l,
                cases: tp.len(),
                throughput_ratio: geomean_plain(&tp),
                p95_ratio: geomean_plain(&p95),
            });
        }
    }

    let all: Vec<&CaseResult> = cases.iter().collect();
    let headline_native = native_agg(&all);
    Analysis {
        seed: cfg.seed,
        resamples: cfg.resamples,
        meta: ds.meta.clone(),
        notes: ds.notes.clone(),
        headline_warm: headline(Mode::Warm),
        headline_cold: headline(Mode::Cold),
        categories,
        libraries,
        cases,
        concurrency,
        concurrency_summary,
        failures,
        headline_native,
        gates: ds.gates.clone(),
    }
}

// ---------------------------------------------------------------- baseline

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Regression,
    Improvement,
    NoChange,
}

impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Change::Regression => "REGRESSION",
            Change::Improvement => "improvement",
            Change::NoChange => "within noise",
        }
    }
}

/// Classify a current/baseline latency ratio (>1 = slower than baseline).
pub fn classify_change(point: f64, lo: f64, hi: f64, min_effect: f64) -> Change {
    if lo > 1.0 && point > 1.0 + min_effect {
        Change::Regression
    } else if hi < 1.0 && point < 1.0 - min_effect {
        Change::Improvement
    } else {
        Change::NoChange
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DeltaCmp {
    /// median(current) / median(baseline); above 1.0 is slower.
    pub ratio: f64,
    pub lo: f64,
    pub hi: f64,
    pub change: Change,
}

#[derive(Clone, Debug, Serialize)]
pub struct BaselineRow {
    pub id: String,
    pub mode: Mode,
    /// Rust latency now vs baseline: the gate.
    pub rust: DeltaCmp,
    /// Node latency now vs baseline: a control that should sit at 1.0 (machine drift).
    pub node: Option<DeltaCmp>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BaselineAnalysis {
    pub min_effect: f64,
    pub rows: Vec<BaselineRow>,
    pub regressions: Vec<String>,
    pub improvements: usize,
    pub unchanged: usize,
    pub missing: Vec<String>,
}

fn delta(cur: &Series, base: &Series, id: &str, mode: Mode, rt: &str, cfg: &Config) -> Option<DeltaCmp> {
    if cur.client_ms.len() < MIN_SAMPLES || base.client_ms.len() < MIN_SAMPLES {
        return None;
    }
    let seed = derive_seed(cfg.seed, &[id, mode.name(), rt, "baseline"]);
    let b = bootstrap_ratio(&cur.client_ms, &base.client_ms, cfg.resamples, seed);
    Some(DeltaCmp {
        ratio: b.point,
        lo: b.lo,
        hi: b.hi,
        change: classify_change(b.point, b.lo, b.hi, cfg.min_effect),
    })
}

pub fn compare_baseline(cur: &Dataset, base: &Dataset, cfg: &Config) -> BaselineAnalysis {
    let mut out = BaselineAnalysis {
        min_effect: cfg.min_effect,
        rows: Vec::new(),
        regressions: Vec::new(),
        improvements: 0,
        unchanged: 0,
        missing: Vec::new(),
    };
    for id in cur.case_ids() {
        for mode in [Mode::Warm, Mode::Cold] {
            let (Some(cr), Some(br)) =
                (cur.get(&id, Runtime::Rust, mode), base.get(&id, Runtime::Rust, mode))
            else {
                if cur.get(&id, Runtime::Rust, mode).is_some() {
                    out.missing.push(format!("{id} {} (not in baseline)", mode.name()));
                }
                continue;
            };
            let Some(rust) = delta(cr, br, &id, mode, "rust", cfg) else {
                out.missing.push(format!("{id} {} (too few samples)", mode.name()));
                continue;
            };
            let node = match (cur.get(&id, Runtime::Node, mode), base.get(&id, Runtime::Node, mode)) {
                (Some(c), Some(b)) => delta(c, b, &id, mode, "node", cfg),
                _ => None,
            };
            match rust.change {
                Change::Regression => out.regressions.push(format!("{id} ({})", mode.name())),
                Change::Improvement => out.improvements += 1,
                Change::NoChange => out.unchanged += 1,
            }
            out.rows.push(BaselineRow { id: id.clone(), mode, rust, node });
        }
    }
    out
}

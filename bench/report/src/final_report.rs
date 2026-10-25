//! Merged final report (`bench_report final ...`, D-105).
//!
//! Merges several run directories into one self-contained HTML page:
//!   * local: node vs rust-native (fork backend). Warm/concurrency rows come from the `--local-warm`
//!     dirs and cold rows from the `--local-cold` dirs. Later dirs supersede earlier ones cell by cell,
//!     but only when they hold at least one valid sample (a rerun that fixed a skipped runtime fills the
//!     gap; an empty cell never erases data).
//!   * local three-way: node vs rust (wasm on V8) vs rust-native on one machine (`--local3*`); the
//!     ratios there are numerator/denominator times, so above 1 means the denominator is faster.
//!   * cloud: node vs ts-v8 vs rust-v8 from one run dir.
//!
//! Primary metric is end-to-end client time (`client_ns`); server `exec_ms` is a secondary cloud view.
//! Ratios below 1 mean the first runtime is faster. Headline cells are medians of per-case ratios.

use crate::load::Catalog;
use crate::render::{
    fmt_ms, fmt_ratio, render_html_titled, render_md_titled, Block, Cell, Table, Tone,
};
use crate::stats::{median, quantile};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Cases whose correct outcome is an application error (ConvexError thrown on purpose).
pub const EXPECTED_ERROR_CASES: &[&str] = &["base-004"];

/// Samples of one (case, runtime, mode[, concurrency]) cell.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Cellv {
    pub client: Vec<f64>,
    pub exec: Vec<f64>,
    /// Transport / unexpected-outcome failures.
    pub fail: usize,
    /// Calls that returned an application error where success was expected.
    pub app_err: usize,
}

impl Cellv {
    pub fn valid(&self) -> usize {
        self.client.len()
    }
    pub fn p50(&self) -> f64 {
        med(&self.client)
    }
    pub fn p95(&self) -> f64 {
        if self.client.is_empty() {
            f64::NAN
        } else {
            quantile(&self.client, 0.95)
        }
    }
    pub fn exec_p50(&self) -> f64 {
        med(&self.exec)
    }
}

#[derive(Default, Clone, Debug)]
pub struct ConcCell {
    pub lat: Cellv,
    /// Calls per second over the whole block (goodput; throughput for expected-error cases).
    pub tput: Option<f64>,
}

#[derive(Default, Debug)]
pub struct Side {
    /// (case, runtime, "warm" | "cold")
    pub timed: BTreeMap<(String, String, String), Cellv>,
    /// (case, runtime, concurrency level)
    pub conc: BTreeMap<(String, String, u32), ConcCell>,
}

impl Side {
    /// Merge `other` in. A cell of `other` replaces ours when it has valid samples (or we have none).
    pub fn absorb(&mut self, other: Side) {
        for (k, v) in other.timed {
            if v.valid() > 0 || !self.timed.contains_key(&k) {
                self.timed.insert(k, v);
            }
        }
        for (k, v) in other.conc {
            if v.lat.valid() > 0 || !self.conc.contains_key(&k) {
                self.conc.insert(k, v);
            }
        }
    }
}

/// Parse phase2.jsonl text (and blocks.jsonl text) keeping only rows whose mode is in `modes`.
pub fn parse_run(phase2: &str, blocks: &str, modes: &[&str]) -> Result<Side, String> {
    let mut s = Side::default();
    for (n, l) in phase2
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(l).map_err(|e| format!("phase2.jsonl:{}: {e}", n + 1))?;
        let mode = v["mode"].as_str().unwrap_or("");
        if !modes.contains(&mode) {
            continue;
        }
        let case = v["case"].as_str().unwrap_or("").to_string();
        let rt = v["runtime"].as_str().unwrap_or("").to_string();
        let expected_err = EXPECTED_ERROR_CASES.contains(&case.as_str());
        let status = v["app_status"].as_str().unwrap_or("");
        let ok = v["ok"] == true;
        let cell: &mut Cellv = if mode == "concurrency" {
            let lvl = v["concurrency"].as_u64().unwrap_or(1) as u32;
            &mut s.conc.entry((case, rt, lvl)).or_default().lat
        } else {
            s.timed.entry((case, rt, mode.to_string())).or_default()
        };
        if ok && (status == "success" || expected_err && status == "error") {
            if let Some(x) = v["client_ns"].as_f64() {
                cell.client.push((x / 1e6).max(1e-6));
                if let Some(e) = v["exec_ms"].as_f64() {
                    cell.exec.push(e);
                }
            }
        } else if ok && status == "error" {
            cell.app_err += 1;
        } else {
            cell.fail += 1;
        }
    }
    if modes.contains(&"concurrency") {
        for (n, l) in blocks
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let v: Value =
                serde_json::from_str(l).map_err(|e| format!("blocks.jsonl:{}: {e}", n + 1))?;
            let case = v["case"].as_str().unwrap_or("").to_string();
            let lvl = v["concurrency"].as_u64().unwrap_or(1) as u32;
            let key = if EXPECTED_ERROR_CASES.contains(&case.as_str()) {
                "throughput_per_s"
            } else {
                "goodput_per_s"
            };
            let rt = v["runtime"].as_str().unwrap_or("").to_string();
            if let (Some(t), Some(c)) = (v[key].as_f64(), s.conc.get_mut(&(case, rt, lvl))) {
                c.tput = Some(t);
            }
        }
    }
    Ok(s)
}

pub fn load_run_dir(dir: &Path, modes: &[&str]) -> Result<Side, String> {
    let p2 = std::fs::read_to_string(dir.join("phase2.jsonl"))
        .map_err(|e| format!("{}/phase2.jsonl: {e}", dir.display()))?;
    let bl = std::fs::read_to_string(dir.join("blocks.jsonl")).unwrap_or_default();
    parse_run(&p2, &bl, modes)
}

/// Why a cell has (or has no) number.
#[derive(Debug, PartialEq)]
pub enum St<'a> {
    Val(&'a Cellv),
    /// Not applicable by design.
    Na,
    /// Cloud deployment disabled mid-run: not measured.
    Disabled,
    Failed,
    NoData,
}

pub struct Target<'a> {
    pub side: &'a Side,
    /// (case, runtime) -> reason
    pub na: BTreeMap<(String, String), String>,
    pub skipped: BTreeSet<String>,
    pub cloud: bool,
}

impl<'a> Target<'a> {
    fn classify(&self, case: &str, rt: &str, cell: Option<&'a Cellv>) -> St<'a> {
        if let Some(c) = cell {
            if c.valid() > 0 {
                return St::Val(c);
            }
        }
        if self.na.contains_key(&(case.to_string(), rt.to_string())) || self.skipped.contains(case)
        {
            return St::Na;
        }
        match cell {
            Some(c) if c.fail + c.app_err > 0 => {
                if self.cloud {
                    St::Disabled
                } else {
                    St::Failed
                }
            }
            _ => St::NoData,
        }
    }
    pub fn timed(&self, case: &str, rt: &str, mode: &str) -> St<'a> {
        self.classify(
            case,
            rt,
            self.side
                .timed
                .get(&(case.to_string(), rt.to_string(), mode.to_string())),
        )
    }
    pub fn conc(&self, case: &str, rt: &str, lvl: u32) -> (St<'a>, Option<f64>) {
        let c = self.side.conc.get(&(case.to_string(), rt.to_string(), lvl));
        (
            self.classify(case, rt, c.map(|c| &c.lat)),
            c.and_then(|c| c.tput),
        )
    }
}

// ------------------------------------------------------------ small helpers

fn plain(s: impl Into<String>) -> Cell {
    Cell(s.into(), Tone::Plain)
}

fn med(v: &[f64]) -> f64 {
    if v.is_empty() {
        f64::NAN
    } else {
        median(v)
    }
}

fn ratio_ok(a: f64, b: f64) -> Option<f64> {
    let r = a / b;
    (r.is_finite() && r > 0.0).then_some(r)
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

fn st_cell(st: &St, f: fn(&Cellv) -> f64) -> Cell {
    match st {
        St::Val(c) => {
            let mark = if c.app_err + c.fail > 0 { "*" } else { "" };
            Cell(format!("{}{mark}", fmt_ms(f(c))), Tone::Plain)
        }
        St::Na => Cell("n/a".into(), Tone::Muted),
        St::Disabled => Cell("n/m".into(), Tone::Muted),
        St::Failed => Cell("fail".into(), Tone::Bad),
        St::NoData => Cell("-".into(), Tone::Muted),
    }
}

fn tput_cell(st: &St, t: Option<f64>) -> Cell {
    match (st, t) {
        (St::Val(c), Some(t)) => Cell(
            format!("{t:.1}{}", if c.app_err + c.fail > 0 { "*" } else { "" }),
            Tone::Plain,
        ),
        _ => st_cell(st, |_| f64::NAN),
    }
}

fn val(st: &St, f: fn(&Cellv) -> f64) -> Option<f64> {
    match st {
        St::Val(c) => Some(f(c)).filter(|x| x.is_finite()),
        _ => None,
    }
}

fn pair_cell(a: Option<f64>, b: Option<f64>) -> Cell {
    ratio_cell(match (a, b) {
        (Some(a), Some(b)) => ratio_ok(a, b).unwrap_or(f64::NAN),
        _ => f64::NAN,
    })
}

type RatioFn<'a> = Box<dyn Fn(&str) -> Option<f64> + 'a>;

/// Ratio of a per-case statistic, `num`/`den`, only when both runtimes have numbers.
fn timed_ratio<'a>(
    t: &'a Target<'a>,
    num: &'a str,
    den: &'a str,
    mode: &'static str,
    f: fn(&Cellv) -> f64,
) -> RatioFn<'a> {
    Box::new(move |c| {
        ratio_ok(
            val(&t.timed(c, num, mode), f)?,
            val(&t.timed(c, den, mode), f)?,
        )
    })
}

/// Inverse-throughput ratio: throughput(den)/throughput(num), so below 1 means `num` is faster.
fn conc_ratio<'a>(t: &'a Target<'a>, num: &'a str, den: &'a str, lvl: u32) -> RatioFn<'a> {
    Box::new(move |c| {
        let ((sn, tn), (sd, td)) = (t.conc(c, num, lvl), t.conc(c, den, lvl));
        if matches!((&sn, &sd), (St::Val(_), St::Val(_))) {
            ratio_ok(td?, tn?)
        } else {
            None
        }
    })
}

/// Ordered categories and case -> category.
struct Cats {
    order: Vec<String>,
    of: BTreeMap<String, String>,
    cases: Vec<String>,
}

impl Cats {
    fn new(cat: &Catalog, universe: &BTreeSet<String>) -> Cats {
        let mut c = Cats {
            order: vec![],
            of: BTreeMap::new(),
            cases: vec![],
        };
        let add = |c: &mut Cats, id: &str, category: &str| {
            if !c.order.iter().any(|o| o == category) {
                c.order.push(category.to_string());
            }
            c.of.insert(id.to_string(), category.to_string());
            c.cases.push(id.to_string());
        };
        for ci in cat.cases.iter().filter(|ci| universe.contains(&ci.id)) {
            add(&mut c, &ci.id, &ci.category);
        }
        for id in universe {
            if !c.of.contains_key(id) {
                let category = id.split('-').next().unwrap_or("?").to_string();
                add(&mut c, id, &category);
            }
        }
        c
    }
}

/// Headline table: one row per category plus "all"; per metric the median of per-case ratios and n.
/// Also returns the overall figures per metric.
fn headline(cats: &Cats, headers: &[&'static str], metrics: &[RatioFn]) -> (Table, Vec<Value>) {
    headline_with(cats, headers, metrics, ratio_cell)
}

/// `headline` with a caller-chosen cell style (the three-way tables colour neither direction).
fn headline_with(
    cats: &Cats,
    headers: &[&'static str],
    metrics: &[RatioFn],
    style: fn(f64) -> Cell,
) -> (Table, Vec<Value>) {
    let mut rows = vec![];
    let mut all: Vec<Vec<f64>> = vec![vec![]; metrics.len()];
    for cat in &cats.order {
        let cs: Vec<&String> = cats.cases.iter().filter(|c| cats.of[*c] == *cat).collect();
        let mut row = vec![plain(cat.clone()), plain(cs.len().to_string())];
        for (i, m) in metrics.iter().enumerate() {
            let rs: Vec<f64> = cs.iter().filter_map(|c| m(c)).collect();
            row.push(style(med(&rs)));
            row.push(plain(rs.len().to_string()));
            all[i].extend(rs);
        }
        rows.push(row);
    }
    let mut row = vec![plain("all"), plain(cats.cases.len().to_string())];
    let mut sums = vec![];
    for (i, h) in headers.iter().enumerate() {
        let m = med(&all[i]);
        row.push(style(m));
        row.push(plain(all[i].len().to_string()));
        sums.push(json!({
            "metric": h,
            "median_ratio": m,
            "n": all[i].len(),
            "faster": all[i].iter().filter(|x| **x < 0.95).count(),
            "slower": all[i].iter().filter(|x| **x > 1.05).count(),
        }));
    }
    rows.push(row);
    let mut hs = vec!["category", "cases"];
    for h in headers {
        hs.push(h);
        hs.push("n");
    }
    (Table { headers: hs, rows }, sums)
}

fn sum_line(label: &str, s: &Value) -> String {
    let n = s["n"].as_u64().unwrap_or(0);
    let (f, sl) = (
        s["faster"].as_u64().unwrap_or(0),
        s["slower"].as_u64().unwrap_or(0),
    );
    format!(
        "- {label}: median per-case ratio {} over {n} cases ({f} faster, {sl} slower, {} within 5%)",
        fmt_ratio(s["median_ratio"].as_f64().unwrap_or(f64::NAN)),
        n - f - sl
    )
}

// ------------------------------------------------------------ inputs

pub struct Inputs {
    pub local_warm: Vec<PathBuf>,
    pub local_cold: Vec<PathBuf>,
    pub local_phase1: Option<PathBuf>,
    /// Same-machine three-way runs (node, rust = wasm, rust-native); warm dirs, cold dirs, phase-1.
    pub local3_warm: Vec<PathBuf>,
    pub local3_cold: Vec<PathBuf>,
    pub local3_phase1: Option<PathBuf>,
    pub cloud: Option<PathBuf>,
    pub cloud_phase1: Option<PathBuf>,
    pub gates: Option<PathBuf>,
    pub gates_recheck: Vec<PathBuf>,
    pub fairness: Option<PathBuf>,
    pub title: String,
}

/// `| id | title | verdict | note |` rows of FAIRNESS.md -> id -> (verdict, note).
pub fn parse_fairness(text: &str) -> BTreeMap<String, (String, String)> {
    let mut m = BTreeMap::new();
    for l in text.lines() {
        let c: Vec<&str> = l
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let is_id = |s: &str| {
            s.split_once('-').is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.chars().all(|c| c.is_ascii_lowercase())
                    && b.chars().all(|c| c.is_ascii_digit())
            })
        };
        if c.len() >= 4 && is_id(c[0]) {
            let verdict = if c[2].contains('✅') {
                "same"
            } else {
                "intrinsic"
            };
            m.insert(c[0].to_string(), (verdict.to_string(), c[3..].join("|")));
        }
    }
    m
}

fn read_json(p: &Path) -> Option<Value> {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// (count per `equal` value, ids that are neither `yes` nor `n/a`)
pub fn phase1_counts(text: &str) -> (BTreeMap<String, usize>, Vec<String>) {
    let mut c: BTreeMap<String, usize> = BTreeMap::new();
    let mut bad = vec![];
    for v in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let e = v["equal"].as_str().unwrap_or("?").to_string();
        if e != "yes" && e != "n/a" {
            bad.push(v["case_id"].as_str().unwrap_or("?").to_string());
        }
        *c.entry(e).or_default() += 1;
    }
    (c, bad)
}

fn phase1_line(label: &str, p: &Option<PathBuf>) -> Option<String> {
    let p = p.as_ref()?;
    let f = if p.is_dir() {
        p.join("phase1.jsonl")
    } else {
        p.clone()
    };
    let (c, bad) = phase1_counts(&std::fs::read_to_string(f).ok()?);
    let tot: usize = c.values().sum();
    let na = c.get("n/a").copied().unwrap_or(0);
    let mut s = format!(
        "{label}: {}/{} equal",
        c.get("yes").copied().unwrap_or(0),
        tot - na
    );
    if na > 0 {
        s += &format!(" ({na} n/a on a runtime by design)");
    }
    if !bad.is_empty() {
        s += &format!("; NOT equal: {}", bad.join(", "));
    }
    Some(s)
}

fn note_cell(fair: &BTreeMap<String, (String, String)>, id: &str) -> Cell {
    match fair.get(id) {
        Some((v, n)) => Cell(format!("[{v}] {n}"), Tone::Note),
        None => Cell(String::new(), Tone::Note),
    }
}

fn gates_blocks(gates: &Option<Value>, rechecks: &[Value]) -> Vec<Block> {
    let mut b = vec![Block::H2("Gates (WN-14, macOS)".into())];
    let Some(g) = gates else {
        b.push(Block::Para("gates not measured".into()));
        return b;
    };
    let f = |v: &Value| v.as_f64().unwrap_or(f64::NAN);
    let mut rows = vec![];
    for (k, v) in g.as_object().into_iter().flatten() {
        if !v.is_object() || v["verdict"].is_null() {
            continue;
        }
        let value = match k.as_str() {
            "overhead" => format!("{:.3} ms", f(&v["value_ms"])),
            "spawn" => format!("{:.1} ms", f(&v["spawn_to_ready_est_ms"])),
            "idle_rss" => format!(
                "{:.1} MB (max of {} workers)",
                f(&v["max_mb"]),
                v["workers"]
            ),
            "crypto" => format!(
                "{:.3} (worst native/node)",
                f(&v["worst_native_over_node_client"])
            ),
            _ => "-".into(),
        };
        let verdict = v["verdict"].as_str().unwrap_or("?");
        rows.push(vec![
            plain(k.clone()),
            Cell(v["gate"].as_str().unwrap_or("").into(), Tone::Note),
            plain(value),
            Cell(
                verdict.into(),
                if verdict == "PASS" {
                    Tone::Good
                } else {
                    Tone::Bad
                },
            ),
        ]);
    }
    b.push(Block::Table(Table {
        headers: vec!["gate", "threshold", "measured", "verdict"],
        rows,
    }));
    if g["idle_rss"]["verdict"] == "FAIL" && !rechecks.is_empty() {
        let maxes: Vec<String> = rechecks
            .iter()
            .map(|r| format!("{:.1} MB", f(&r["idle_rss"]["max_mb"])))
            .collect();
        let passes = rechecks
            .iter()
            .filter(|r| r["idle_rss"]["verdict"] == "PASS")
            .count();
        b.push(Block::Para(format!(
            "idle-rss: the FAIL above was measured at the end of a long run (one of three workers at {:.1} MB). Run alone, the gate passes {passes}/{} times (max worker RSS {}). This is a test-timing artifact of the gate, being fixed in the runner; it is shown as measured, not hidden.",
            f(&g["idle_rss"]["max_mb"]),
            rechecks.len(),
            maxes.join(", ")
        )));
    }
    if g["overhead"]["verdict"] == "FAIL" {
        b.push(Block::Para(format!(
            "overhead: exec minus user_exec p50 is {:.3} ms against the 0.2 ms macOS limit, a marginal miss reported as measured (a recheck is pending).",
            f(&g["overhead"]["value_ms"])
        )));
    }
    b
}

// ------------------------------------------------------------ local three-way (node vs wasm vs native)

/// Runtime ids of the three-way runs: `rust` is the wasm build on the V8 backend.
const L3_NODE: &str = "node";
const L3_WASM: &str = "rust";
const L3_NATIVE: &str = "rust-native";

/// Ratio cells of the three-way tables are speedups (numerator time / denominator time); above 1 means
/// the denominator is faster. Neither direction is "good" for the page, so no colouring.
fn speedup_cell(x: f64) -> Cell {
    Cell(
        fmt_ratio(x),
        if x.is_finite() { Tone::Plain } else { Tone::Muted },
    )
}

/// (node, wasm, native) values of one statistic for the cases that have all three.
type Triple = (f64, f64, f64);

fn triple(t: &Target, case: &str, mode: &str, f: fn(&Cellv) -> f64) -> Option<Triple> {
    Some((
        val(&t.timed(case, L3_NODE, mode), f)?,
        val(&t.timed(case, L3_WASM, mode), f)?,
        val(&t.timed(case, L3_NATIVE, mode), f)?,
    ))
}

fn ratio_stats(rs: &[f64]) -> Value {
    json!({
        "median_ratio": med(rs),
        "n": rs.len(),
        "faster": rs.iter().filter(|x| **x < 0.95).count(),
        "slower": rs.iter().filter(|x| **x > 1.05).count(),
    })
}

/// Per-category table (median of per-case values in ms, median of per-case ratios) and the
/// overall figures. Only cases with a number for all three runtimes count.
fn three_way(cats: &Cats, t: &Target, mode: &str, f: fn(&Cellv) -> f64) -> (Table, Value) {
    let ts: Vec<(&String, Triple)> = cats
        .cases
        .iter()
        .filter_map(|c| triple(t, c, mode, f).map(|x| (c, x)))
        .collect();
    let summarize = |xs: &[Triple]| {
        let col = |g: fn(&Triple) -> f64| xs.iter().map(g).collect::<Vec<f64>>();
        let rat = |a: fn(&Triple) -> f64, b: fn(&Triple) -> f64| {
            xs.iter().filter_map(|x| ratio_ok(a(x), b(x))).collect::<Vec<f64>>()
        };
        let ms = [col(|x| x.0), col(|x| x.1), col(|x| x.2)].map(|v| med(&v));
        let r = [
            rat(|x| x.0, |x| x.1),
            rat(|x| x.0, |x| x.2),
            rat(|x| x.1, |x| x.2),
        ];
        (ms, r)
    };
    let row_of = |label: &str, xs: &[Triple]| {
        let (ms, r) = summarize(xs);
        let mut row = vec![plain(label), plain(xs.len().to_string())];
        row.extend(ms.iter().map(|m| plain(fmt_ms(*m))));
        row.extend(r.iter().map(|v| speedup_cell(med(v))));
        row.push(plain(r[2].iter().filter(|x| **x < 0.95).count().to_string()));
        row
    };
    let mut rows = vec![];
    for cat in &cats.order {
        let xs: Vec<Triple> = ts
            .iter()
            .filter(|(c, _)| cats.of[*c] == *cat)
            .map(|(_, x)| *x)
            .collect();
        rows.push(row_of(cat, &xs));
    }
    let all: Vec<Triple> = ts.iter().map(|(_, x)| *x).collect();
    rows.push(row_of("all", &all));
    let (ms, r) = summarize(&all);
    let summary = json!({
        "n": all.len(),
        "median_ms": {"node": ms[0], "wasm": ms[1], "native": ms[2]},
        "node_over_wasm": ratio_stats(&r[0]),
        "node_over_native": ratio_stats(&r[1]),
        "wasm_over_native": ratio_stats(&r[2]),
    });
    (
        Table {
            headers: vec![
                "category", "cases", "node ms", "wasm ms", "native ms", "J/W", "J/N", "W/N",
                "N slower than W",
            ],
            rows,
        },
        summary,
    )
}

/// One bullet per ratio of a `three_way` summary.
fn three_lines(label: &str, s: &Value) -> Vec<String> {
    [
        ("node/wasm", "node_over_wasm", "wasm", "node"),
        ("node/native", "node_over_native", "native", "node"),
        ("wasm/native", "wasm_over_native", "native", "wasm"),
    ]
    .iter()
    .map(|(nm, k, quick, slow)| {
        let v = &s[*k];
        let (n, f, sl) = (
            v["n"].as_u64().unwrap_or(0),
            v["faster"].as_u64().unwrap_or(0),
            v["slower"].as_u64().unwrap_or(0),
        );
        // `faster` counts cases where the numerator is faster, i.e. the opposite runtime is slower.
        format!(
            "- {label} {nm}: median per-case ratio {} over {n} cases; {quick} faster in {sl}, {slow} faster in {f}, within 5% in {}",
            fmt_ratio(v["median_ratio"].as_f64().unwrap_or(f64::NAN)),
            n - f - sl
        )
    })
    .collect()
}

struct Local3 {
    lines: Vec<String>,
    summary: Value,
    blocks: Vec<Block>,
}

fn local3(inp: &Inputs, catalog: &Catalog) -> Result<Option<Local3>, String> {
    if inp.local3_warm.is_empty() {
        return Ok(None);
    }
    let mut warm = Side::default();
    for d in &inp.local3_warm {
        warm.absorb(load_run_dir(d, &["warm"])?);
    }
    let mut cold = Side::default();
    for d in &inp.local3_cold {
        cold.absorb(load_run_dir(d, &["cold"])?);
    }
    let universe: BTreeSet<String> = warm
        .timed
        .keys()
        .map(|k| k.0.clone())
        .chain(cold.timed.keys().map(|k| k.0.clone()))
        .collect();
    let cats = Cats::new(catalog, &universe);
    let mk = |side| Target {
        side,
        na: BTreeMap::new(),
        skipped: BTreeSet::new(),
        cloud: false,
    };
    let (tw, tc) = (mk(&warm), mk(&cold));
    let meta = inp
        .local3_warm
        .first()
        .and_then(|d| read_json(&d.join("meta.json")))
        .unwrap_or(Value::Null);
    let ov = &meta["options"];

    let (warm_t, warm_s) = three_way(&cats, &tw, "warm", Cellv::p50);
    let (warm95_t, warm95_s) = three_way(&cats, &tw, "warm", Cellv::p95);
    let (warmx_t, warmx_s) = three_way(&cats, &tw, "warm", Cellv::exec_p50);
    let (cold_t, cold_s) = three_way(&cats, &tc, "cold", Cellv::p50);
    let (coldx_t, coldx_s) = three_way(&cats, &tc, "cold", Cellv::exec_p50);

    let mut lines = vec!["Local, same machine: J = node, W = Rust wasm (V8 backend), N = Rust native. Ratios are numerator time / denominator time, so J/N above 1 means native is faster. Client time, concurrency 1:".to_string()];
    lines.extend(three_lines("warm p50", &warm_s));
    lines.extend(three_lines("cold p50 (2 rounds)", &cold_s));
    lines.extend(three_lines("server exec, warm", &warmx_s));
    lines.extend(three_lines("server exec, cold", &coldx_s));

    let mut b = vec![Block::H2(
        "Local: node vs Rust wasm vs Rust native (same machine)".into(),
    )];
    b.push(Block::Facts(vec![
        ("backend".into(), "one fork convex-local-backend (release) serving node, rust (wasm on V8) and rust-native (RUST_NATIVE_MODE=worker), macOS aarch64; all three on this machine, no network".into()),
        ("warm runs".into(), format!("{} dirs merged; iters {}, warmup {}, concurrency 1", inp.local3_warm.len(), ov["iters"], ov["warmup"])),
        ("cold runs".into(), format!("{} dirs merged; 2 rounds, workers restarted per round", inp.local3_cold.len())),
        ("native build".into(), format!("sha256 {}", meta["build"]["native"]["sha256"].as_str().unwrap_or("?"))),
    ]));
    b.push(Block::Para("Each category row shows the median over its cases of the per-case p50 (ms), then medians of per-case ratios (J = node, W = wasm, N = native; numerator / denominator, above 1 = the denominator is faster). Only cases with a number for all three runtimes are counted (cases column). \"N slower than W\" counts cases where native is more than 5% slower than wasm. Primary metric is end-to-end client time; server exec (time inside the function, without the client round trip) is the secondary view. Local numbers have no network and are not comparable with cloud numbers. rust-native is self-hosted only: the stock Convex cloud runs node and wasm.".into()));
    for (title, tbl) in [
        ("Three-way warm, client p50 (per category)", warm_t),
        ("Three-way cold, client p50 (per category; median of 2 rounds)", cold_t),
        ("Three-way warm, client p95 (per category; 50 iterations, indicative)", warm95_t),
        ("Three-way warm, server exec p50 (secondary view)", warmx_t),
        ("Three-way cold, server exec p50 (secondary view)", coldx_t),
    ] {
        b.push(Block::H3(title.into()));
        b.push(Block::Table(tbl));
    }
    let mut rows = vec![];
    for c in &cats.cases {
        let w = [L3_NODE, L3_WASM, L3_NATIVE].map(|r| tw.timed(c, r, "warm"));
        let k = [L3_NODE, L3_WASM, L3_NATIVE].map(|r| tc.timed(c, r, "cold"));
        let rc = |s: &[St; 3], a: usize, bb: usize| {
            pair_cell(val(&s[a], Cellv::p50), val(&s[bb], Cellv::p50))
        };
        let mut row = vec![plain(c.clone())];
        row.extend(w.iter().map(|s| st_cell(s, Cellv::p50)));
        row.extend([rc(&w, 0, 1), rc(&w, 0, 2), rc(&w, 1, 2)]);
        row.extend(k.iter().map(|s| st_cell(s, Cellv::p50)));
        row.extend([rc(&k, 0, 1), rc(&k, 0, 2), rc(&k, 1, 2)]);
        rows.push(row);
    }
    b.push(Block::H3("Three-way per case (client ms, p50)".into()));
    b.push(Block::Table(Table {
        headers: vec![
            "case", "J warm", "W warm", "N warm", "J/W", "J/N", "W/N", "J cold", "W cold",
            "N cold", "J/W cold", "J/N cold", "W/N cold",
        ],
        rows,
    }));
    let slower: Vec<String> = cats
        .cases
        .iter()
        .filter(|c| {
            triple(&tw, c, "warm", Cellv::p50)
                .and_then(|(_, w, n)| ratio_ok(w, n))
                .is_some_and(|r| r < 0.95)
        })
        .map(|c| format!("{c} ({})", cats.of[c]))
        .collect();
    b.push(Block::Para(format!(
        "Cases where native is more than 5% slower than wasm (warm p50): {}.",
        if slower.is_empty() { "none".into() } else { slower.join(", ") }
    )));

    let summary = json!({
        "warm_client": warm_s, "warm_p95": warm95_s, "warm_exec": warmx_s,
        "cold_client": cold_s, "cold_exec": coldx_s,
    });
    Ok(Some(Local3 { lines, summary, blocks: b }))
}

// ------------------------------------------------------------ build

pub struct Final {
    pub markdown: String,
    pub html: String,
    pub summary: String,
}

pub fn build(inp: &Inputs, catalog: &Catalog) -> Result<Final, String> {
    let fair = inp
        .fairness
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_fairness(&t))
        .unwrap_or_default();

    let mut lwarm = Side::default();
    for d in &inp.local_warm {
        lwarm.absorb(load_run_dir(d, &["warm", "concurrency"])?);
    }
    let mut lcold = Side::default();
    for d in &inp.local_cold {
        lcold.absorb(load_run_dir(d, &["cold"])?);
    }
    let cloud_side = match &inp.cloud {
        Some(d) => Some(load_run_dir(d, &["warm", "cold", "concurrency"])?),
        None => None,
    };
    let cloud_meta = inp
        .cloud
        .as_ref()
        .and_then(|d| read_json(&d.join("meta.json")))
        .unwrap_or(Value::Null);
    let local_meta = inp
        .local_warm
        .first()
        .and_then(|d| read_json(&d.join("meta.json")))
        .unwrap_or(Value::Null);
    let gates = inp.gates.as_ref().and_then(|p| read_json(p));
    let rechecks: Vec<Value> = inp
        .gates_recheck
        .iter()
        .filter_map(|d| read_json(&d.join("gates.json")))
        .collect();

    let mut summary = serde_json::Map::new();
    let mut headline_lines: Vec<String> = vec![];
    let mut local_blocks: Vec<Block> = vec![];
    let mut cloud_blocks: Vec<Block> = vec![];

    // ---- local
    if !inp.local_warm.is_empty() {
        let universe: BTreeSet<String> = lwarm
            .timed
            .keys()
            .map(|k| k.0.clone())
            .chain(lcold.timed.keys().map(|k| k.0.clone()))
            .chain(lwarm.conc.keys().map(|k| k.0.clone()))
            .collect();
        let cats = Cats::new(catalog, &universe);
        let t = Target {
            side: &lwarm,
            na: BTreeMap::new(),
            skipped: BTreeSet::new(),
            cloud: false,
        };
        let tc = Target {
            side: &lcold,
            na: BTreeMap::new(),
            skipped: BTreeSet::new(),
            cloud: false,
        };
        let (n, r) = ("node", "rust-native");
        let (warm_t, warm_s) = headline(
            &cats,
            &["p50 N/J", "p95 N/J"],
            &[
                timed_ratio(&t, r, n, "warm", Cellv::p50),
                timed_ratio(&t, r, n, "warm", Cellv::p95),
            ],
        );
        let (cold_t, cold_s) = headline(
            &cats,
            &["cold p50 N/J"],
            &[timed_ratio(&tc, r, n, "cold", Cellv::p50)],
        );
        let (conc_t, conc_s) = headline(
            &cats,
            &["c=10 N/J", "c=50 N/J"],
            &[conc_ratio(&t, r, n, 10), conc_ratio(&t, r, n, 50)],
        );
        headline_lines.push("Local (node vs rust-native on the fork backend), ratio native/node, below 1 = native faster:".into());
        headline_lines.push(sum_line("warm p50", &warm_s[0]));
        headline_lines.push(sum_line("warm p95", &warm_s[1]));
        headline_lines.push(sum_line("cold p50 (2 rounds)", &cold_s[0]));
        headline_lines.push(sum_line("concurrency 10 (inverse throughput)", &conc_s[0]));
        headline_lines.push(sum_line("concurrency 50 (inverse throughput)", &conc_s[1]));
        summary.insert(
            "local".into(),
            json!({"warm": warm_s, "cold": cold_s, "concurrency": conc_s}),
        );

        local_blocks.push(Block::H2("Local: node vs rust-native".into()));
        let ov = &local_meta["options"];
        local_blocks.push(Block::Facts(vec![
            ("backend".into(), "fork convex-local-backend (release), rust-native workers (RUST_NATIVE_MODE=worker), macOS aarch64".into()),
            ("warm runs".into(), format!("{} dirs merged; iters {}, warmup {}, concurrency {}", inp.local_warm.len(), ov["iters"], ov["warmup"], ov["concurrency"])),
            ("cold runs".into(), format!("{} dirs merged; 2 rounds, workers restarted per round", inp.local_cold.len())),
            ("native build".into(), format!("sha256 {}", local_meta["build"]["native"]["sha256"].as_str().unwrap_or("?"))),
        ]));
        local_blocks.push(Block::Para("J = node, N = rust-native. Times are end-to-end client milliseconds. A ratio below 1 means native is faster. Headline cells are medians of per-case ratios (n = cases with a number on both sides). Concurrency ratios are node throughput / native throughput (calls per second), so below 1 also means native is faster. Fairness tags: [same] = same algorithm and work, [intrinsic] = an inherent language/runtime difference left in on purpose (D-105).".into()));
        local_blocks.push(Block::H3("Local warm (headline per category)".into()));
        local_blocks.push(Block::Table(warm_t));
        local_blocks.push(Block::H3("Local cold (headline per category)".into()));
        local_blocks.push(Block::Table(cold_t));
        local_blocks.push(Block::H3(
            "Local concurrency (headline per category)".into(),
        ));
        local_blocks.push(Block::Table(conc_t));

        let (mut rows, mut crow, mut ncrow) = (vec![], vec![], vec![]);
        for c in &cats.cases {
            let (jw, nw) = (t.timed(c, n, "warm"), t.timed(c, r, "warm"));
            let (jc, nc) = (tc.timed(c, n, "cold"), tc.timed(c, r, "cold"));
            rows.push(vec![
                plain(c.clone()),
                st_cell(&jw, Cellv::p50),
                st_cell(&nw, Cellv::p50),
                pair_cell(val(&nw, Cellv::p50), val(&jw, Cellv::p50)),
                st_cell(&jw, Cellv::p95),
                st_cell(&nw, Cellv::p95),
                pair_cell(val(&nw, Cellv::p95), val(&jw, Cellv::p95)),
                note_cell(&fair, c),
            ]);
            crow.push(vec![
                plain(c.clone()),
                st_cell(&jc, Cellv::p50),
                st_cell(&nc, Cellv::p50),
                pair_cell(val(&nc, Cellv::p50), val(&jc, Cellv::p50)),
            ]);
            let mut row = vec![plain(c.clone())];
            for lvl in [10u32, 50] {
                let ((sj, tj), (sn, tn)) = (t.conc(c, n, lvl), t.conc(c, r, lvl));
                let both = matches!((&sj, &sn), (St::Val(_), St::Val(_)));
                row.push(tput_cell(&sj, tj));
                row.push(tput_cell(&sn, tn));
                row.push(pair_cell(tj.filter(|_| both), tn.filter(|_| both)));
            }
            ncrow.push(row);
        }
        local_blocks.push(Block::H3("Local warm per case (client ms)".into()));
        local_blocks.push(Block::Table(Table {
            headers: vec![
                "case",
                "J p50",
                "N p50",
                "N/J",
                "J p95",
                "N p95",
                "N/J p95",
                "fairness note",
            ],
            rows,
        }));
        local_blocks.push(Block::H3(
            "Local cold per case (client ms, median of 2 rounds)".into(),
        ));
        local_blocks.push(Block::Table(Table {
            headers: vec!["case", "J cold", "N cold", "N/J"],
            rows: crow,
        }));
        local_blocks.push(Block::H3(
            "Local concurrency per case (calls per second; ratio = node/native throughput)".into(),
        ));
        local_blocks.push(Block::Table(Table {
            headers: vec!["case", "J c10", "N c10", "J/N", "J c50", "N c50", "J/N"],
            rows: ncrow,
        }));
        let failing: Vec<String> = lwarm
            .conc
            .iter()
            .filter(|(_, v)| v.lat.fail + v.lat.app_err > 0)
            .map(|(k, v)| {
                format!(
                    "{} {} c={} ({} failed, {} ok)",
                    k.0,
                    k.1,
                    k.2,
                    v.lat.fail + v.lat.app_err,
                    v.lat.valid()
                )
            })
            .collect();
        if !failing.is_empty() {
            local_blocks.push(Block::Para(format!(
                "Concurrency cells with failed calls (marked * above): {}",
                failing.join("; ")
            )));
        }
    }

    // ---- local three-way
    if let Some(l3) = local3(inp, catalog)? {
        headline_lines.extend(l3.lines);
        headline_lines.push(String::new());
        summary.insert("local3".into(), l3.summary);
        local_blocks.extend(l3.blocks);
    }

    // ---- cloud
    if let Some(side) = &cloud_side {
        let cl = &cloud_meta["cloud"];
        let mut na = BTreeMap::new();
        for (k, rt) in [("ts_v8_na", "ts-v8"), ("node_na", "node")] {
            for (c, why) in cl[k].as_object().into_iter().flatten() {
                na.insert(
                    (c.clone(), rt.to_string()),
                    why.as_str().unwrap_or("").to_string(),
                );
            }
        }
        let skipped: BTreeSet<String> = cl["skipped_cases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_string))
            .collect();
        let t = Target {
            side,
            na,
            skipped,
            cloud: true,
        };
        let universe: BTreeSet<String> = side
            .timed
            .keys()
            .map(|k| k.0.clone())
            .chain(side.conc.keys().map(|k| k.0.clone()))
            .chain(t.skipped.iter().cloned())
            .collect();
        let cats = Cats::new(catalog, &universe);
        let (j, ts, rv) = ("node", "ts-v8", "rust-v8");
        let hdr = ["T/J", "R/J", "R/T"];
        let three = |mode: &'static str, f: fn(&Cellv) -> f64| {
            [
                timed_ratio(&t, ts, j, mode, f),
                timed_ratio(&t, rv, j, mode, f),
                timed_ratio(&t, rv, ts, mode, f),
            ]
        };
        let (warm_t, warm_s) = headline(&cats, &hdr, &three("warm", Cellv::p50));
        let (warm95_t, warm95_s) = headline(&cats, &hdr, &three("warm", Cellv::p95));
        let (warmx_t, warmx_s) = headline(&cats, &hdr, &three("warm", Cellv::exec_p50));
        let (cold_t, cold_s) = headline(&cats, &hdr, &three("cold", Cellv::p50));
        let (coldx_t, coldx_s) = headline(&cats, &hdr, &three("cold", Cellv::exec_p50));
        let (conc_t, conc_s) = headline(
            &cats,
            &hdr,
            &[
                conc_ratio(&t, ts, j, 10),
                conc_ratio(&t, rv, j, 10),
                conc_ratio(&t, rv, ts, 10),
            ],
        );
        headline_lines.push("Cloud (Convex cloud), end-to-end client time; T = ts-v8, R = rust-v8, J = node, below 1 = numerator faster. Warm covers only cases measured before the deployment was disabled:".into());
        for (l, s) in [
            ("cold p50", &cold_s),
            ("warm p50", &warm_s),
            ("concurrency 10", &conc_s),
        ] {
            for (i, nm) in hdr.iter().enumerate() {
                headline_lines.push(sum_line(&format!("{l} {nm}"), &s[i]));
            }
        }
        summary.insert(
            "cloud".into(),
            json!({"warm_client": warm_s, "warm_p95": warm95_s, "warm_exec": warmx_s, "cold_client": cold_s, "cold_exec": coldx_s, "conc10": conc_s}),
        );

        cloud_blocks.push(Block::H2("Cloud: node vs ts-v8 vs rust-v8".into()));
        let ov = &cloud_meta["options"];
        cloud_blocks.push(Block::Facts(vec![
            (
                "deployment".into(),
                cl["url"].as_str().unwrap_or("?").into(),
            ),
            (
                "run id".into(),
                cloud_meta["run_id"].as_str().unwrap_or("?").into(),
            ),
            (
                "config".into(),
                format!(
                    "warmup {}, iters {}, concurrency {}, cold rounds {}",
                    ov["warmup"], ov["iters"], ov["concurrency"], ov["cold_restarts"]
                ),
            ),
            (
                "convex CLI".into(),
                cl["convex_cli"].as_str().unwrap_or("?").into(),
            ),
        ]));
        let count = |mode: &str, rt: &str, pred: &dyn Fn(&St) -> bool| {
            cats.cases
                .iter()
                .filter(|c| pred(&t.timed(c, rt, mode)))
                .count()
        };
        let mut cov = vec![];
        for rt in [j, ts, rv] {
            for mode in ["warm", "cold"] {
                cov.push(vec![
                    plain(rt),
                    plain(mode),
                    plain(count(mode, rt, &|s| matches!(s, St::Val(_))).to_string()),
                    plain(count(mode, rt, &|s| matches!(s, St::Na)).to_string()),
                    plain(count(mode, rt, &|s| matches!(s, St::Disabled)).to_string()),
                    plain(count(mode, rt, &|s| matches!(s, St::Failed | St::NoData)).to_string()),
                ]);
            }
        }
        cloud_blocks.push(Block::H3("Coverage (cases)".into()));
        cloud_blocks.push(Block::Para("n/a = not applicable by design (ts-v8 limits, node 512 MB OOM, net skipped). n/m = not measured (deployment disabled): the deployment was disabled mid-run for exceeding free-plan limits, so those warm and concurrency calls errored. They are shown as not measured, never as numbers and never as runtime failures. Cold calls ran before that point and are valid for every case. A * on a value means some calls of that cell failed.".into()));
        cloud_blocks.push(Block::Table(Table {
            headers: vec![
                "runtime",
                "mode",
                "measured",
                "n/a",
                "n/m (deployment disabled)",
                "other",
            ],
            rows: cov,
        }));
        let mut nm_rows = vec![];
        for rt in [j, ts, rv] {
            let cs: Vec<String> = cats
                .cases
                .iter()
                .filter(|c| matches!(t.timed(c, rt, "warm"), St::Disabled))
                .cloned()
                .collect();
            if !cs.is_empty() {
                nm_rows.push(vec![
                    plain(rt),
                    Cell(format!("{} cases: {}", cs.len(), cs.join(", ")), Tone::Note),
                ]);
            }
        }
        cloud_blocks.push(Block::H3(
            "Warm and concurrency not measured (deployment disabled)".into(),
        ));
        cloud_blocks.push(Block::Table(Table {
            headers: vec!["runtime", "cases"],
            rows: nm_rows,
        }));
        for (title, tbl) in [
            (
                "Cloud cold, client p50 (headline per category; 2 rounds)",
                cold_t,
            ),
            ("Cloud warm, client p50 (headline per category)", warm_t),
            (
                "Cloud warm, client p95 (headline per category; 15 iterations, indicative)",
                warm95_t,
            ),
            (
                "Cloud concurrency 10, inverse throughput (headline per category)",
                conc_t,
            ),
            ("Cloud cold, server exec p50 (secondary view)", coldx_t),
            ("Cloud warm, server exec p50 (secondary view)", warmx_t),
        ] {
            cloud_blocks.push(Block::H3(title.into()));
            cloud_blocks.push(Block::Table(tbl));
        }
        let (mut wrows, mut crows, mut xrows, mut krows) = (vec![], vec![], vec![], vec![]);
        for c in &cats.cases {
            let w = [
                t.timed(c, j, "warm"),
                t.timed(c, ts, "warm"),
                t.timed(c, rv, "warm"),
            ];
            let k = [
                t.timed(c, j, "cold"),
                t.timed(c, ts, "cold"),
                t.timed(c, rv, "cold"),
            ];
            let rc = |s: &[St; 3], a: usize, b: usize| {
                pair_cell(val(&s[a], Cellv::p50), val(&s[b], Cellv::p50))
            };
            let mut row = vec![plain(c.clone())];
            row.extend(w.iter().map(|s| st_cell(s, Cellv::p50)));
            row.extend(w.iter().map(|s| st_cell(s, Cellv::p95)));
            row.extend([rc(&w, 1, 0), rc(&w, 2, 0), rc(&w, 2, 1)]);
            row.push(note_cell(&fair, c));
            wrows.push(row);
            let mut row = vec![plain(c.clone())];
            row.extend(k.iter().map(|s| st_cell(s, Cellv::p50)));
            row.extend([rc(&k, 1, 0), rc(&k, 2, 0), rc(&k, 2, 1)]);
            crows.push(row);
            let mut row = vec![plain(c.clone())];
            row.extend(w.iter().map(|s| st_cell(s, Cellv::exec_p50)));
            row.extend(k.iter().map(|s| st_cell(s, Cellv::exec_p50)));
            xrows.push(row);
            let mut row = vec![plain(c.clone())];
            for rt in [j, ts, rv] {
                let (s, tp) = t.conc(c, rt, 10);
                row.push(tput_cell(&s, tp));
            }
            krows.push(row);
        }
        cloud_blocks.push(Block::H3(
            "Cloud cold per case (client ms, median of 2 rounds)".into(),
        ));
        cloud_blocks.push(Block::Table(Table {
            headers: vec!["case", "J cold", "T cold", "R cold", "T/J", "R/J", "R/T"],
            rows: crows,
        }));
        cloud_blocks.push(Block::H3("Cloud warm per case (client ms)".into()));
        cloud_blocks.push(Block::Table(Table {
            headers: vec![
                "case",
                "J p50",
                "T p50",
                "R p50",
                "J p95",
                "T p95",
                "R p95",
                "T/J",
                "R/J",
                "R/T",
                "fairness note",
            ],
            rows: wrows,
        }));
        cloud_blocks.push(Block::H3(
            "Cloud server exec per case (secondary; ms, p50)".into(),
        ));
        cloud_blocks.push(Block::Table(Table {
            headers: vec![
                "case", "J warm", "T warm", "R warm", "J cold", "T cold", "R cold",
            ],
            rows: xrows,
        }));
        cloud_blocks.push(Block::H3(
            "Cloud concurrency 10 per case (calls per second)".into(),
        ));
        cloud_blocks.push(Block::Table(Table {
            headers: vec!["case", "J c10", "T c10", "R c10"],
            rows: krows,
        }));
        for (title, key) in [
            ("ts-v8 not applicable", "ts_v8_na"),
            ("node not applicable on the cloud", "node_na"),
        ] {
            if let Some(m) = cl[key].as_object().filter(|m| !m.is_empty()) {
                cloud_blocks.push(Block::H3(format!("{title} ({} cases)", m.len())));
                cloud_blocks.push(Block::Table(Table {
                    headers: vec!["case", "reason"],
                    rows: m
                        .iter()
                        .map(|(k, v)| {
                            vec![
                                plain(k.clone()),
                                Cell(v.as_str().unwrap_or("").into(), Tone::Note),
                            ]
                        })
                        .collect(),
                }));
            }
        }
    }

    // ---- assemble
    let mut blocks = vec![Block::H2("Headline".into())];
    blocks.push(Block::Para("Metric: end-to-end time as the client sees it (D-105). Cells are medians of per-case ratios; per-case tables show medians (p50) and warm p95 of client round trips. The first local section compares node with rust-native; the three-way local section adds Rust wasm on the same machine.".into()));
    let mut group: Vec<String> = vec![];
    for l in headline_lines
        .into_iter()
        .chain(std::iter::once(String::new()))
    {
        if l.starts_with("- ") {
            group.push(l);
            continue;
        }
        if !group.is_empty() {
            blocks.push(Block::Para(group.join("\n")));
            group.clear();
        }
        if !l.is_empty() {
            blocks.push(Block::Para(l));
        }
    }
    let p1 = [
        phase1_line(
            "Local phase-1 correctness (node vs native)",
            &inp.local_phase1,
        ),
        phase1_line(
            "Local three-way phase-1 correctness (node vs wasm vs native)",
            &inp.local3_phase1,
        ),
        phase1_line(
            "Cloud phase-1 correctness (node vs ts-v8 vs rust-v8)",
            &inp.cloud_phase1,
        ),
    ];
    let p1: Vec<String> = p1.into_iter().flatten().map(|l| format!("- {l}")).collect();
    if !p1.is_empty() {
        blocks.push(Block::Para(p1.join("\n")));
    }
    blocks.extend(local_blocks);
    blocks.extend(cloud_blocks);
    blocks.extend(gates_blocks(&gates, &rechecks));
    blocks.push(Block::H2("Caveats".into()));
    blocks.push(Block::Para(
        [
            "- Cloud data is partial. The deployment was disabled mid-run for exceeding free-plan limits: warm and concurrency results exist only for the cases before data-014. data-014, data-015, crypto, media and mem are not measured in those modes (n/m). Cold results for all cases were taken earlier and are valid.",
            "- Cold numbers are the median of only 2 rounds per case and runtime (a restart or redeploy per round): indicative, not a distribution. Cloud warm uses 15 iterations (p95 is indicative); local warm uses 100.",
            "- net is not measured on the cloud (it needs a localhost mock server). net is measured locally.",
            "- 37 cases are n/a for ts-v8 by design (Node built-ins, native addons, Buffer, 64 MB isolate limit); node is n/a on the cloud for 4 cases (512 MB OOM). Local node runs those cases.",
            "- Cloud client time includes the network round trip from this machine to the region, a constant that dominates short cases; the server exec view removes it.",
            "- Local rows for data-008, mem-001, mem-002 and mem-004 come from a rerun (node was wrongly skipped in the chunk runs, runner fix 4cb61a5); the rerun supersedes the chunk rows.",
            "- Gates: see the Gates section. idle-rss failed in the full run but passes alone (test-timing artifact, being fixed); the overhead gate is a marginal miss, reported as measured.",
            "- Local concurrency 50 with 5 MB arguments (ser-004, node) hit client-side socket exhaustion on macOS (No buffer space available); those failed calls are excluded, so that throughput is understated.",
            "- Rust queries and mutations are not supported (actions only); the native host skips #[http_action]. sharp (libvips) is multithreaded on node while Rust is single-threaded, favouring node in media-001..005.",
        ]
        .join("\n"),
    ));
    blocks.push(Block::H2("Sources".into()));
    let names = |v: &[PathBuf]| {
        v.iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    blocks.push(Block::Facts(vec![
        ("local warm".into(), names(&inp.local_warm)),
        ("local cold".into(), names(&inp.local_cold)),
        ("local three-way warm".into(), names(&inp.local3_warm)),
        ("local three-way cold".into(), names(&inp.local3_cold)),
        ("cloud".into(), inp.cloud.iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).collect::<Vec<_>>().join(", ")),
        ("merge rule".into(), "later directories supersede earlier ones per (case, runtime, mode) cell when they hold valid samples".into()),
    ]));

    let summary =
        serde_json::to_string_pretty(&Value::Object(summary)).map_err(|e| e.to_string())?;
    Ok(Final {
        markdown: render_md_titled(&inp.title, &blocks),
        html: render_html_titled(&inp.title, &blocks),
        summary,
    })
}

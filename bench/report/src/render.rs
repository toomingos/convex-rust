//! Markdown and HTML rendering from one shared block model.

use crate::analysis::{Agg, Analysis, BaselineAnalysis, Change, Cmp, Group, NativeAgg, Verdict};
use serde_json::Value;
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Good,
    Bad,
    Muted,
    /// Free text: left aligned, wraps.
    Note,
}

pub struct Cell(pub String, pub Tone);

pub struct Table {
    pub headers: Vec<&'static str>,
    pub rows: Vec<Vec<Cell>>,
}

pub enum Block {
    H2(String),
    H3(String),
    Para(String),
    Facts(Vec<(String, String)>),
    Table(Table),
    /// Inline SVG, HTML only.
    Svg(String),
}

fn plain(s: impl Into<String>) -> Cell {
    Cell(s.into(), Tone::Plain)
}

// ------------------------------------------------------------ formatting

pub fn fmt_ms(x: f64) -> String {
    if x.is_nan() {
        "-".into()
    } else if x < 1.0 {
        format!("{x:.3}")
    } else if x < 100.0 {
        format!("{x:.2}")
    } else {
        format!("{x:.1}")
    }
}

pub fn fmt_ratio(x: f64) -> String {
    if x.is_nan() {
        "-".into()
    } else {
        format!("{x:.3}")
    }
}

fn fmt_ci(lo: f64, hi: f64) -> String {
    if lo.is_nan() || hi.is_nan() {
        "-".into()
    } else {
        format!("[{lo:.3}, {hi:.3}]")
    }
}

fn fmt_mb(b: Option<u64>) -> String {
    b.map_or("-".into(), |b| format!("{:.0}", b as f64 / 1e6))
}

fn native_trio(c: &crate::analysis::Cmp) -> String {
    c.native.as_ref().map_or("-".into(), |n| trio(n.p50, n.p95, n.p99))
}

fn opt_ratio(x: Option<f64>) -> String {
    x.map_or("-".into(), fmt_ratio)
}

fn trio(a: f64, b: f64, c: f64) -> String {
    format!("{} / {} / {}", fmt_ms(a), fmt_ms(b), fmt_ms(c))
}

fn verdict_tone(v: Verdict) -> Tone {
    match v {
        Verdict::RustFaster => Tone::Good,
        Verdict::RustSlower => Tone::Bad,
        _ => Tone::Muted,
    }
}

fn agg_cells(a: &Agg) -> Vec<Cell> {
    let gm = match (a.geomean, a.geomean_lo, a.geomean_hi) {
        (Some(g), Some(lo), Some(hi)) => {
            let tone = if hi < 1.0 {
                Tone::Good
            } else if lo > 1.0 {
                Tone::Bad
            } else {
                Tone::Muted
            };
            Cell(format!("{} {}", fmt_ratio(g), fmt_ci(lo, hi)), tone)
        }
        _ => plain("-"),
    };
    vec![
        plain(a.cases.to_string()),
        plain(a.faster.to_string()),
        plain(a.slower.to_string()),
        plain(a.no_difference.to_string()),
        plain(a.insufficient.to_string()),
        gm,
    ]
}

fn flatten(prefix: &str, v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&p, x, out);
            }
        }
        Value::String(s) => out.push((prefix.into(), s.clone())),
        Value::Null => {}
        o => out.push((prefix.into(), o.to_string())),
    }
}

const AGG_HEAD: [&str; 6] = ["Cases", "Rust faster", "Rust slower", "No difference", "No data", "Geomean ratio [95% CI]"];

fn native_cells(n: Option<&NativeAgg>) -> Vec<Cell> {
    let g = |f: fn(&NativeAgg) -> Option<f64>| plain(n.and_then(f).map_or("-".into(), fmt_ratio));
    vec![g(|n| n.warm_vs_node), g(|n| n.warm_vs_wasm), g(|n| n.cold_vs_node), g(|n| n.cold_vs_wasm)]
}

fn group_table(first: &'static str, groups: &[Group]) -> Table {
    let with_native = groups.iter().any(|g| g.native.is_some());
    let mut rows = Vec::new();
    for g in groups {
        let mut r = vec![plain(g.name.clone())];
        r.extend(agg_cells(&g.warm));
        r.push(agg_cells(&g.cold).pop().unwrap_or_else(|| plain("-")));
        if with_native {
            r.extend(native_cells(g.native.as_ref()));
        }
        rows.push(r);
    }
    let mut headers = vec![first, "Cases", "Faster", "Slower", "No diff", "No data", "Warm geomean [95% CI]", "Cold geomean [95% CI]"];
    if with_native {
        headers.extend(["Native/Node warm", "Native/Wasm warm", "Native/Node cold", "Native/Wasm cold"]);
    }
    Table { headers, rows }
}

fn gate_value(key: &str, g: &Value) -> String {
    let f = |v: Option<f64>, unit: &str| v.map_or("-".into(), |x| format!("{x:.3} {unit}"));
    match key {
        "overhead" => f(g["value_ms"].as_f64(), "ms"),
        "spawn" => f(g["spawn_to_ready_est_ms"].as_f64(), "ms"),
        "idle_rss" => f(g["max_mb"].as_f64(), "MB"),
        "crypto" => g["worst_native_over_node_client"].as_f64().map_or("-".into(), |x| format!("{x:.3} (native/node)")),
        "orphans" => match g["rounds"].as_array() {
            Some(r) => format!("{} orphan(s)", r.iter().map(|x| x["orphans"].as_array().map_or(0, Vec::len)).sum::<usize>()),
            None => "-".into(),
        },
        _ => "-".into(),
    }
}

fn gates_blocks(b: &mut Vec<Block>, gates: Option<&Value>) {
    b.push(Block::H2("Gates".into()));
    let Some(g) = gates else {
        b.push(Block::Para("gates not measured (run `bench_runner gates`)".into()));
        return;
    };
    let mut rows = Vec::new();
    for (key, name) in [
        ("overhead", "Runtime overhead (exec - user_exec)"),
        ("spawn", "Worker spawn to Ready"),
        ("idle_rss", "Idle worker RSS"),
        ("crypto", "Crypto native/node"),
        ("orphans", "No orphan processes"),
    ] {
        let x = g.get(key).unwrap_or(&Value::Null);
        let (verdict, tone) = match x["verdict"].as_str() {
            Some("PASS") => ("PASS", Tone::Good),
            Some("FAIL") => ("FAIL", Tone::Bad),
            _ => ("NO DATA", Tone::Muted),
        };
        let threshold = match (x["gate"].as_str(), x["error"].as_str()) {
            (Some(t), _) => t.to_string(),
            (None, Some(e)) => format!("error: {e}"),
            _ if x.is_null() => "not run".into(),
            _ => "-".into(),
        };
        rows.push(vec![plain(name), plain(gate_value(key, x)), plain(threshold), Cell(verdict.into(), tone)]);
    }
    let mut meta = Vec::new();
    for k in ["run_id", "platform"] {
        if let Some(s) = g[k].as_str() {
            meta.push(format!("{k} {s}"));
        }
    }
    if !meta.is_empty() {
        b.push(Block::Para(format!("gates.json: {}.", meta.join(", "))));
    }
    b.push(Block::Table(Table { headers: vec!["Gate", "Value", "Threshold", "Verdict"], rows }));
}

// ------------------------------------------------------------ blocks

pub fn build_blocks(a: &Analysis, base: Option<&BaselineAnalysis>) -> Vec<Block> {
    let mut b = Vec::new();

    // Meta.
    b.push(Block::H2("Run".into()));
    let mut facts = Vec::new();
    flatten("", &a.meta, &mut facts);
    facts.push(("report.seed".into(), a.seed.to_string()));
    facts.push(("report.bootstrap_resamples".into(), a.resamples.to_string()));
    b.push(Block::Facts(facts));
    for n in &a.notes {
        b.push(Block::Para(format!("Note: {n}")));
    }
    b.push(Block::Para(
        "Method: ratio = Rust/Node of the median client latency (below 1.0 means Rust is faster). \
         Confidence intervals are percentile bootstrap (95%, independent resampling of both sides, \
         ratio of medians) with a fixed seed, so the same input gives the same report. A ratio whose CI \
         crosses 1.0 counts as \"no difference\". Failed calls are excluded from latencies and counted. \
         Geometric means average the per-case ln-ratios and are bootstrapped from the same resamples; they capture measurement noise within cases only, \
         not variation between cases (the case set is treated as fixed)."
            .into(),
    ));

    // Headline.
    b.push(Block::H2("Headline".into()));
    let mut rows = Vec::new();
    for (name, agg) in [("warm", &a.headline_warm), ("cold", &a.headline_cold)] {
        let mut r = vec![plain(name)];
        r.extend(agg_cells(agg));
        rows.push(r);
    }
    let mut headers = vec!["Mode"];
    headers.extend(AGG_HEAD);
    b.push(Block::Table(Table { headers, rows }));
    if let Some(n) = &a.headline_native {
        b.push(Block::Para(
            "rust-native (geometric mean of per-case median ratios, no CI; below 1.0 means native is faster):".into(),
        ));
        let row = |mode: &str, node: Option<f64>, wasm: Option<f64>| {
            vec![plain(mode), plain(n.cases.to_string()), plain(opt_ratio(node)), plain(opt_ratio(wasm))]
        };
        b.push(Block::Table(Table {
            headers: vec!["Mode", "Cases with native", "Native/Node", "Native/Wasm"],
            rows: vec![row("warm", n.warm_vs_node, n.warm_vs_wasm), row("cold", n.cold_vs_node, n.cold_vs_wasm)],
        }));
    }

    gates_blocks(&mut b, a.gates.as_ref());

    b.push(Block::H2("By category".into()));
    b.push(Block::Table(group_table("Category", &a.categories)));
    b.push(Block::H2("By idiomatic library (Node library -> Rust crate)".into()));
    b.push(Block::Para(
        "Cases are grouped by the library pair recorded in cases.json, so native-addon vs pure-Rust \
         comparisons can be read separately."
            .into(),
    ));
    b.push(Block::Table(group_table("Library pair", &a.libraries)));

    if let Some(base) = base {
        baseline_blocks(&mut b, base);
    }

    // Per case, warm.
    b.push(Block::H2("Per case: warm".into()));
    b.push(Block::Para(
        "Latencies are client ms as p50 / p95 / p99. Exec is the backend-recorded median execution \
         time (client minus exec is transport and dispatch overhead). RSS is peak MB: Node as backend / \
         node executor, Rust as backend."
            .into(),
    ));
    let mut rows = Vec::new();
    for c in &a.cases {
        let Some(w) = &c.warm else { continue };
        rows.push(vec![
            plain(c.id.clone()),
            plain(trio(w.node.p50, w.node.p95, w.node.p99)),
            plain(trio(w.rust.p50, w.rust.p95, w.rust.p99)),
            Cell(fmt_ratio(w.ratio), verdict_tone(w.verdict)),
            plain(fmt_ci(w.lo, w.hi)),
            Cell(w.verdict.label().into(), verdict_tone(w.verdict)),
            plain(format!("{} / {}", fmt_ms(w.node.p50), fmt_ms(w.node.exec_p50))),
            plain(format!("{} / {}", fmt_ms(w.rust.p50), fmt_ms(w.rust.exec_p50))),
            plain(format!("{} / {}", fmt_mb(c.rss_node_backend), fmt_mb(c.rss_node_executor))),
            plain(fmt_mb(c.rss_rust_backend)),
            plain(format!("{} / {}", w.node.n_fail, w.rust.n_fail)),
            plain(native_trio(w)),
            plain(opt_ratio(w.native_vs_node)),
            plain(opt_ratio(w.native_vs_wasm)),
            plain(format!("{} / {}", fmt_mb(c.rss_native_backend), fmt_mb(c.rss_native_worker))),
            plain(w.native.as_ref().map_or("-".into(), |n| n.n_fail.to_string())),
        ]);
    }
    b.push(Block::Table(Table {
        headers: vec![
            "Case",
            "Node p50/p95/p99",
            "Rust p50/p95/p99",
            "Ratio",
            "95% CI",
            "Verdict",
            "Node client / exec p50",
            "Rust client / exec p50",
            "Node RSS MB",
            "Rust RSS MB",
            "Fail N/R",
            "Native p50/p95/p99",
            "Native/Node",
            "Native/Wasm",
            "Native RSS MB (backend / worker)",
            "Native fail",
        ],
        rows,
    }));
    b.push(Block::Para(
        "Rust above is rust-wasm. Native columns are empty for runs without rust-native; Native/Node and \
         Native/Wasm are median client-latency ratios (below 1 = native faster)."
            .into(),
    ));
    b.push(Block::H3("Warm ratio per case (log scale, 95% CI)".into()));
    b.push(Block::Svg(forest_svg(a)));

    // Per case, cold.
    b.push(Block::H2("Per case: cold start".into()));
    let mut rows = Vec::new();
    for c in &a.cases {
        let Some(w) = &c.cold else { continue };
        rows.push(vec![
            plain(c.id.clone()),
            plain(trio(w.node.p50, w.node.p95, w.node.p99)),
            plain(trio(w.rust.p50, w.rust.p95, w.rust.p99)),
            Cell(fmt_ratio(w.ratio), verdict_tone(w.verdict)),
            plain(fmt_ci(w.lo, w.hi)),
            Cell(w.verdict.label().into(), verdict_tone(w.verdict)),
            plain(format!("{} / {}", w.node.n_fail, w.rust.n_fail)),
            plain(native_trio(w)),
            plain(opt_ratio(w.native_vs_node)),
            plain(opt_ratio(w.native_vs_wasm)),
        ]);
    }
    b.push(Block::Table(Table {
        headers: vec![
            "Case",
            "Node p50/p95/p99",
            "Rust p50/p95/p99",
            "Ratio",
            "95% CI",
            "Verdict",
            "Fail N/R",
            "Native p50/p95/p99",
            "Native/Node",
            "Native/Wasm",
        ],
        rows,
    }));

    // Concurrency.
    b.push(Block::H2("Concurrency".into()));
    b.push(Block::Para(
        "Throughput is estimated by Little's law (calls in flight / mean client latency), in calls per \
         second; p95 is client ms. Summary ratios are plain geometric means over cases (no CI)."
            .into(),
    ));
    let mut rows = Vec::new();
    for s in &a.concurrency_summary {
        rows.push(vec![
            plain(s.level.to_string()),
            plain(s.cases.to_string()),
            Cell(fmt_ratio(s.throughput_ratio), if s.throughput_ratio > 1.0 { Tone::Good } else { Tone::Bad }),
            Cell(fmt_ratio(s.p95_ratio), if s.p95_ratio < 1.0 { Tone::Good } else { Tone::Bad }),
        ]);
    }
    b.push(Block::Table(Table {
        headers: vec!["In flight", "Cases", "Throughput Rust/Node (higher better)", "p95 Rust/Node (lower better)"],
        rows,
    }));
    let levels: Vec<u32> = a.concurrency_summary.iter().map(|s| s.level).collect();
    let mut rows = Vec::new();
    for c in &a.concurrency {
        let mut r = vec![plain(c.id.clone())];
        for l in &levels {
            let (n, ru) = c.levels.get(l).cloned().unwrap_or((None, None));
            let thr = |s: &Option<crate::analysis::ConcSide>| s.as_ref().map_or("-".into(), |s| format!("{:.0}", s.throughput_rps));
            let p95 = |s: &Option<crate::analysis::ConcSide>| s.as_ref().map_or("-".into(), |s| fmt_ms(s.p95));
            r.push(plain(format!("{} / {}", thr(&n), thr(&ru))));
            r.push(plain(format!("{} / {}", p95(&n), p95(&ru))));
        }
        rows.push(r);
    }
    let mut headers: Vec<&'static str> = vec!["Case"];
    for l in &levels {
        headers.push(level_head(*l, true));
        headers.push(level_head(*l, false));
    }
    b.push(Block::Table(Table { headers, rows }));

    if !a.failures.is_empty() {
        b.push(Block::H2("Failures and missing data".into()));
        b.push(Block::Para(
            "Sides with failed calls or no successful calls (failures are excluded from latencies).".into(),
        ));
        b.push(Block::Para(a.failures.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")));
    }
    b
}

fn level_head(level: u32, thr: bool) -> &'static str {
    // Static header strings for the usual levels; unusual levels fall back to a generic label.
    match (level, thr) {
        (1, true) => "c=1 rps N/R",
        (1, false) => "c=1 p95 N/R",
        (10, true) => "c=10 rps N/R",
        (10, false) => "c=10 p95 N/R",
        (50, true) => "c=50 rps N/R",
        (50, false) => "c=50 p95 N/R",
        (200, true) => "c=200 rps N/R",
        (200, false) => "c=200 p95 N/R",
        (_, true) => "other level rps N/R",
        (_, false) => "other level p95 N/R",
    }
}

fn baseline_blocks(b: &mut Vec<Block>, base: &BaselineAnalysis) {
    b.push(Block::H2("Baseline comparison".into()));
    b.push(Block::Para(format!(
        "Ratio = current / baseline median client latency (above 1.0 means slower than baseline). \
         Regression = CI entirely above 1.0 and point estimate at least {:.1}% above 1.0 (min effect). \
         The Node column is a control: it should sit near 1.0 unless the machine drifted. \
         Result: {} regression(s), {} improvement(s), {} within noise.",
        base.min_effect * 100.0,
        base.regressions.len(),
        base.improvements,
        base.unchanged
    )));
    if !base.regressions.is_empty() {
        b.push(Block::Para(format!("Regressions: {}", base.regressions.join(", "))));
    }
    if !base.missing.is_empty() {
        b.push(Block::Para(format!("Not compared: {}", base.missing.join(", "))));
    }
    let mut rows = Vec::new();
    for r in &base.rows {
        let tone = match r.rust.change {
            Change::Regression => Tone::Bad,
            Change::Improvement => Tone::Good,
            Change::NoChange => Tone::Muted,
        };
        rows.push(vec![
            plain(r.id.clone()),
            plain(r.mode.name()),
            Cell(fmt_ratio(r.rust.ratio), tone),
            plain(fmt_ci(r.rust.lo, r.rust.hi)),
            Cell(r.rust.change.label().into(), tone),
            plain(r.node.as_ref().map_or("-".into(), |n| format!("{} {}", fmt_ratio(n.ratio), fmt_ci(n.lo, n.hi)))),
        ]);
    }
    b.push(Block::Table(Table {
        headers: vec!["Case", "Mode", "Rust cur/base", "95% CI", "Verdict", "Node cur/base (control)"],
        rows,
    }));
}

// ------------------------------------------------------------ SVG

fn forest_svg(a: &Analysis) -> String {
    let rows: Vec<(&str, &Cmp)> =
        a.cases.iter().filter_map(|c| c.warm.as_ref().filter(|w| w.ratio.is_finite()).map(|w| (c.id.as_str(), w))).collect();
    if rows.is_empty() {
        return String::new();
    }
    let (xmin, xmax) = (0.1f64.ln(), 10f64.ln());
    let (left, width, rowh) = (70.0, 640.0, 7.0);
    let height = rows.len() as f64 * rowh + 40.0;
    let x = |v: f64| left + (v.clamp(0.1, 10.0).ln() - xmin) / (xmax - xmin) * width;
    let mut s = format!(
        "<svg viewBox=\"0 0 {} {height}\" role=\"img\" aria-label=\"Warm Rust/Node ratio per case with 95% CI\" class=\"chart\">",
        left + width + 20.0
    );
    for t in [0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 10.0] {
        let cls = if t == 1.0 { "axis one" } else { "axis" };
        let _ = write!(
            s,
            "<line class=\"{cls}\" x1=\"{0:.1}\" x2=\"{0:.1}\" y1=\"10\" y2=\"{1}\"/><text class=\"tick\" x=\"{0:.1}\" y=\"{2}\" text-anchor=\"middle\">{t}</text>",
            x(t),
            height - 25.0,
            height - 10.0
        );
    }
    for (i, (id, c)) in rows.iter().enumerate() {
        let y = 14.0 + i as f64 * rowh;
        let cls = match c.verdict {
            Verdict::RustFaster => "good",
            Verdict::RustSlower => "bad",
            _ => "muted",
        };
        let _ = write!(
            s,
            "<g class=\"{cls}\"><title>{id}: {:.3} [{:.3}, {:.3}] {}</title><line x1=\"{:.1}\" x2=\"{:.1}\" y1=\"{y:.1}\" y2=\"{y:.1}\"/><circle cx=\"{:.1}\" cy=\"{y:.1}\" r=\"2.2\"/></g>",
            c.ratio,
            c.lo,
            c.hi,
            c.verdict.label(),
            x(c.lo),
            x(c.hi),
            x(c.ratio)
        );
        if i % 10 == 0 {
            let _ = write!(s, "<text class=\"tick\" x=\"{}\" y=\"{:.1}\" text-anchor=\"end\">{id}</text>", left - 6.0, y + 3.0);
        }
    }
    s.push_str("</svg>");
    s
}

// ------------------------------------------------------------ writers

fn esc_md(s: &str) -> String {
    s.replace('|', "\\|")
}

pub fn render_md(blocks: &[Block]) -> String {
    render_md_titled("Rust vs Node benchmark report", blocks)
}

pub fn render_md_titled(title: &str, blocks: &[Block]) -> String {
    let mut o = format!("# {title}\n\n");
    for b in blocks {
        match b {
            Block::H2(t) => {
                let _ = write!(o, "\n## {t}\n\n");
            }
            Block::H3(_) | Block::Svg(_) => {}
            Block::Para(t) => {
                let _ = write!(o, "{t}\n\n");
            }
            Block::Facts(f) => {
                for (k, v) in f {
                    let _ = writeln!(o, "- {k}: {}", esc_md(v));
                }
                o.push('\n');
            }
            Block::Table(t) => {
                if t.rows.is_empty() {
                    o.push_str("(no data)\n\n");
                    continue;
                }
                let _ = writeln!(o, "| {} |", t.headers.join(" | "));
                let _ = writeln!(o, "|{}|", vec!["---"; t.headers.len()].join("|"));
                for r in &t.rows {
                    let cells: Vec<String> = r.iter().map(|c| esc_md(&c.0)).collect();
                    let _ = writeln!(o, "| {} |", cells.join(" | "));
                }
                o.push('\n');
            }
        }
    }
    o
}

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const CSS: &str = r#":root{color-scheme:light dark;--bg:#fff;--fg:#1b1f24;--muted:#6b7280;--line:#d8dce1;--head:#f1f3f5;--good:#0a7d3b;--bad:#b42318;--accent:#2457c5}
@media (prefers-color-scheme:dark){:root{--bg:#14171a;--fg:#e6e8ea;--muted:#9aa3ad;--line:#2d333a;--head:#1d2227;--good:#4cc38a;--bad:#ff7b72;--accent:#7aa2ff}}
body{background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,sans-serif;margin:0 auto;max-width:1200px;padding:16px}
h1{font-size:22px}h2{font-size:18px;margin-top:32px;border-bottom:1px solid var(--line);padding-bottom:4px}h3{font-size:15px}
.scroll{overflow-x:auto}table{border-collapse:collapse;font-size:12.5px;font-variant-numeric:tabular-nums}
th,td{border:1px solid var(--line);padding:3px 8px;text-align:right;white-space:nowrap}th{background:var(--head)}
th:first-child,td:first-child{text-align:left}td.good{color:var(--good);font-weight:600}td.bad{color:var(--bad);font-weight:600}td.muted{color:var(--muted)}td.note{white-space:normal;text-align:left;min-width:260px;max-width:420px}
dl{display:grid;grid-template-columns:max-content 1fr;gap:2px 16px;margin:0}dt{color:var(--muted)}dd{margin:0}
.chart{width:100%;max-width:760px;height:auto}.chart .axis{stroke:var(--line)}.chart .axis.one{stroke:var(--fg)}
.chart .tick{fill:var(--muted);font-size:9px}.chart g.good{stroke:var(--good);fill:var(--good)}.chart g.bad{stroke:var(--bad);fill:var(--bad)}
.chart g.muted{stroke:var(--muted);fill:var(--muted)}p{max-width:90ch}pre{white-space:pre-wrap}"#;

pub fn render_html(blocks: &[Block]) -> String {
    render_html_titled("Rust vs Node benchmark report", blocks)
}

/// Single self-contained page: inline CSS, no scripts, no external resources.
pub fn render_html_titled(title: &str, blocks: &[Block]) -> String {
    let mut o = String::new();
    let t = esc_html(title);
    o.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>");
    o.push_str(&t);
    o.push_str("</title><style>");
    o.push_str(CSS);
    let _ = writeln!(o, "</style></head><body><h1>{t}</h1>");
    for b in blocks {
        match b {
            Block::H2(t) => {
                let _ = writeln!(o, "<h2>{}</h2>", esc_html(t));
            }
            Block::H3(t) => {
                let _ = writeln!(o, "<h3>{}</h3>", esc_html(t));
            }
            Block::Para(t) if t.starts_with("- ") => {
                let _ = writeln!(o, "<pre>{}</pre>", esc_html(t));
            }
            Block::Para(t) => {
                let _ = writeln!(o, "<p>{}</p>", esc_html(t));
            }
            Block::Facts(f) => {
                o.push_str("<dl>");
                for (k, v) in f {
                    let _ = write!(o, "<dt>{}</dt><dd>{}</dd>", esc_html(k), esc_html(v));
                }
                o.push_str("</dl>\n");
            }
            Block::Svg(s) => {
                let _ = writeln!(o, "{s}");
            }
            Block::Table(t) => {
                if t.rows.is_empty() {
                    o.push_str("<p>(no data)</p>\n");
                    continue;
                }
                o.push_str("<div class=\"scroll\"><table><thead><tr>");
                for h in &t.headers {
                    let _ = write!(o, "<th>{}</th>", esc_html(h));
                }
                o.push_str("</tr></thead><tbody>");
                for r in &t.rows {
                    o.push_str("<tr>");
                    for c in r {
                        let cls = match c.1 {
                            Tone::Plain => "",
                            Tone::Good => " class=\"good\"",
                            Tone::Bad => " class=\"bad\"",
                            Tone::Muted => " class=\"muted\"",
                            Tone::Note => " class=\"note\"",
                        };
                        let _ = write!(o, "<td{cls}>{}</td>", esc_html(&c.0));
                    }
                    o.push_str("</tr>");
                }
                o.push_str("</tbody></table></div>\n");
            }
        }
    }
    o.push_str("</body></html>\n");
    o
}

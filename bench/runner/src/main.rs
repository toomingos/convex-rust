//! Phase-0 runner: runs each phase-0 case on Node (in-process, via
//! `bench/scripts/time-node.ts`) and on wasmtime (the compiled `bench_rust.wasm`,
//! host ABI per contracts/ABI.md), compares outputs and records quick timings.
//!
//! (phase1: see phase1.rs)
//! cargo run -p bench_runner --release -- phase0 [--cases a,b | --category cpu]
//!     [--iterations N] [--warmup N] [--wasm path] [--run-id id] [--timeout-secs N] [--show-logs]

mod api;
mod backend;
mod gates;
mod host;
mod node;
mod phase1;
mod phase2;
mod procs;

use std::{
    collections::BTreeMap,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
};

use wasmtime::{bail, error::Context, Result};
use serde_json::{json, Value};

use crate::host::Module;

#[derive(Clone)]
pub struct Case {
    pub id: String,
    pub title: String,
    pub category: String,
    pub compare: String,
    pub tolerance: f64,
    pub node_path: String,
    pub rust_path: String,
    /// ts-v8 (cloud bench): same module/export under `v8/`.
    pub v8_path: String,
    /// Why the ts-v8 variant cannot run (Node built-in / native addon), from bench/cloud/v8-na.json.
    pub v8_na: Option<String>,
    /// Why the node variant cannot run on the cloud (e.g. 512 MB OOM), from bench/cloud/v8-na.json.
    pub node_na: Option<String>,
}

pub fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn repo_root() -> PathBuf {
    bench_dir().join("..")
}

fn load_cases(phase0_only: bool) -> Result<Vec<Case>> {
    let text = fs::read_to_string(bench_dir().join("cases.json")).context("reading bench/cases.json")?;
    let v: Value = serde_json::from_str(&text)?;
    let mut out = vec![];
    let node_na: Value = fs::read_to_string(bench_dir().join("cloud/v8-na.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .map(|v: Value| v["node_na"].clone())
        .unwrap_or(Value::Null);
    let v8_na: Value = fs::read_to_string(bench_dir().join("cloud/v8-na.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .map(|v: Value| v["na"].clone())
        .unwrap_or(Value::Null);
    for c in v["cases"].as_array().context("cases[]")? {
        if phase0_only && c["phase0"] != true {
            continue;
        }
        out.push(Case {
            id: c["id"].as_str().context("id")?.to_string(),
            title: c["title"].as_str().unwrap_or("").to_string(),
            category: c["category"].as_str().unwrap_or("").to_string(),
            compare: c["compare"].as_str().unwrap_or("exact").to_string(),
            tolerance: c["tolerance"].as_f64().unwrap_or(1e-9),
            node_path: c["node"]["path"].as_str().unwrap_or("").to_string(),
            rust_path: c["rust"]["path"].as_str().unwrap_or("").to_string(),
            v8_path: c["node"]["path"].as_str().unwrap_or("").replacen("node/", "v8/", 1),
            node_na: node_na[c["id"].as_str().unwrap_or("")].as_str().map(str::to_string),
            v8_na: v8_na[c["id"].as_str().unwrap_or("")].as_str().map(str::to_string),
        });
    }
    Ok(out)
}

fn hash_value(v: &Value) -> String {
    // serde_json maps are sorted (no preserve_order), so this is canonical up to
    // number formatting.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn p50(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

struct Opts {
    cases: Option<Vec<String>>,
    category: Option<String>,
    iterations: usize,
    warmup: usize,
    wasm: PathBuf,
    run_id: String,
    timeout_secs: u64,
    show_logs: bool,
}

fn parse_opts(args: &[String]) -> Result<Opts> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("target"));
    let mut o = Opts {
        cases: None,
        category: None,
        iterations: 5,
        warmup: 2,
        wasm: target.join("wasm32-wasip1/release/bench_rust.wasm"),
        run_id: format!(
            "phase0-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()
        ),
        timeout_secs: 600,
        show_logs: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--cases" => o.cases = Some(val(a)?.split(',').map(str::to_string).collect()),
            "--category" => o.category = Some(val(a)?),
            "--iterations" => o.iterations = val(a)?.parse()?,
            "--warmup" => o.warmup = val(a)?.parse()?,
            "--wasm" => o.wasm = PathBuf::from(val(a)?),
            "--run-id" => o.run_id = val(a)?,
            "--timeout-secs" => o.timeout_secs = val(a)?.parse()?,
            "--show-logs" => o.show_logs = true,
            other => bail!("unknown option {other}"),
        }
    }
    if o.iterations == 0 {
        bail!("--iterations must be >= 1");
    }
    Ok(o)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("phase0") => phase0(parse_opts(&args[1..])?),
        Some("phase1") => phase1::run(&args[1..]),
        Some("phase2") => phase2::run(&args[1..]),
        Some("gates") => gates::run(&args[1..]),
        _ => bail!(
            "usage: bench_runner phase2 --run-id X [--cases a,b | --category c] [--cold N] [--warmup N] [--iters N] [--concurrency 1,10,50,200] [--seed S] [--quick]\n       [--profile release|debug] [--url U] [--admin-key K] [--external] [--isolate-rss] [--conc-min N] [--timeout-secs N]\n       [--runtimes node,rust,rust-native] [--native-url U] [--native-admin-key K] [--push]\n       bench_runner phase2 --target cloud --run-id X [--url https://<dep>.convex.cloud] [--runtimes node,ts-v8,rust-v8] ... (deploy key in $CONVEX_DEPLOY_KEY; see bench/cloud/README)\n       bench_runner gates --run-id X [--only overhead,spawn,idle-rss,crypto,orphans] [--calls N] [--external] [--push] [--url U] [--native-url U]\n       bench_runner phase1 [--cases a,b | --category c] [--url U] [--admin-key K] [--run-id id] [--timeout-secs N] [--write-expected] [--runtimes node,rust,rust-native] [--native-url U]\n       bench_runner phase0 [--cases a,b | --category c] [--iterations N] [--warmup N] [--wasm path] \
             [--run-id id] [--timeout-secs N] [--show-logs]"
        ),
    }
}

#[derive(Default)]
struct Row {
    node: Vec<f64>,
    rust: Vec<f64>,
    equal: String,
}

fn phase0(o: Opts) -> Result<()> {
    let mut cases = load_cases(true)?;
    if let Some(ids) = &o.cases {
        for id in ids {
            if !cases.iter().any(|c| &c.id == id) {
                bail!("{id} is not a phase-0 case in cases.json");
            }
        }
        cases.retain(|c| ids.contains(&c.id));
    }
    if let Some(cat) = &o.category {
        cases.retain(|c| &c.category == cat);
    }
    if cases.is_empty() {
        bail!("no cases selected");
    }
    let ids: Vec<String> = cases.iter().map(|c| c.id.clone()).collect();

    if !o.wasm.exists() {
        bail!(
            "{} not found; build it with: cargo build -p bench_rust --target wasm32-wasip1 --release",
            o.wasm.display()
        );
    }
    let module = Module::load(&o.wasm, o.timeout_secs)?;
    eprintln!(
        "wasm: {} ({} functions)",
        o.wasm.display(),
        module.functions.len()
    );

    eprintln!(
        "timing (each side, per call, {} untimed warm-up + {} timed; p50 over timed only):\n  \
         node: build args JSON string outside timer; timed = jsonToConvex(JSON.parse(args)) + handler + JSON.stringify({{ok: convexToJson(out)}}); same process/JIT warm across calls\n  \
         rust: Module/InstancePre compiled once; timed = fresh Instance (instantiate) + alloc/copy args JSON + guest parse + handler + envelope serialize + copy out of guest memory",
        o.warmup, o.iterations
    );

    // Node side: args first, then timings.
    let node_args = node::print_args(&ids)?;
    let node_runs = node::time(&ids, o.iterations, o.warmup)?;

    let out_dir = bench_dir().join("results").join(&o.run_id);
    fs::create_dir_all(&out_dir)?;
    let mut raw = std::io::BufWriter::new(fs::File::create(out_dir.join("raw.jsonl"))?);

    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for case in &cases {
        let row = rows.entry(case.id.clone()).or_default();
        let node = node_runs.get(&case.id);
        let mut node_out: Option<Value> = None;
        if let Some(node::NodeResult::Runs(runs)) = node {
            {
                for r in runs {
                    row.node.push(r.ms);
                    writeln!(
                        raw,
                        "{}",
                        json!({"case_id": case.id, "runtime": "node", "phase": 0, "iteration": r.iteration,
                               "client_ms": r.ms, "ok": r.ok, "output_hash": hash_value(&r.output)})
                    )?;
                }
                node_out = runs.first().map(|r| r.output.clone());
            }
        }
        let node_missing = match node {
            Some(node::NodeResult::Missing(why)) => Some(why.clone()),
            None => Some("no output from node".into()),
            _ => None,
        };

        let args = match node_args.get(&case.id) {
            Some(Ok(a)) => Some(a.clone()),
            _ => None,
        };
        let func = module.function_for(&case.rust_path);
        let mut rust_missing = None;
        let mut diff = None;
        match (func, &args) {
            (None, _) => rust_missing = Some(format!("no export for {}", case.rust_path)),
            (_, None) => {
                rust_missing = Some(match node_args.get(&case.id) {
                    Some(Err(e)) => format!("args: {e}"),
                    _ => "no args".into(),
                })
            },
            (Some(f), Some(args)) => {
                let args_text = args.to_string();
                for _ in 0..o.warmup {
                    module.call(f, &args_text)?;
                }
                for iteration in 0..o.iterations {
                    let r = module.call(f, &args_text)?;
                    if o.show_logs {
                        for (lvl, line) in &r.logs {
                            eprintln!("[{} wasm log {lvl}] {line}", case.id);
                        }
                    }
                    let ok = r.envelope.get("ok").is_some();
                    if diff.is_none() {
                        if let Some(n) = &node_out {
                            if let Err(path) = convex_fn_test::compare::matches(
                                &case.compare,
                                case.tolerance,
                                &r.envelope,
                                n,
                            ) {
                                diff = Some(path);
                            }
                        }
                    }
                    row.rust.push(r.instantiate_ms + r.call_ms);
                    writeln!(
                        raw,
                        "{}",
                        json!({"case_id": case.id, "runtime": "rust", "phase": 0, "iteration": iteration,
                               "client_ms": r.instantiate_ms + r.call_ms, "instantiate_ms": r.instantiate_ms,
                               "call_ms": r.call_ms, "ok": ok, "output_hash": hash_value(&r.envelope)})
                    )?;
                }
            },
        }
        row.equal = match (&node_missing, &rust_missing, &diff) {
            (Some(n), Some(_), _) => format!("missing (node: {}; rust)", short(n)),
            (Some(n), None, _) => format!("missing (node: {})", short(n)),
            (None, Some(r), _) => format!("missing (rust: {})", short(r)),
            (None, None, Some(p)) => format!("DIFF at {p}"),
            (None, None, None) => "yes".into(),
        };
    }
    raw.flush()?;

    println!(
        "{:<12} {:>12} {:>12} {:>8}  equal?",
        "case", "node p50 ms", "rust p50 ms", "ratio"
    );
    for (id, row) in &rows {
        let (n, r) = (p50(&row.node), p50(&row.rust));
        let f = |x: Option<f64>| x.map_or("-".to_string(), |x| format!("{x:.3}"));
        let ratio = match (n, r) {
            (Some(n), Some(r)) if n > 0.0 => format!("{:.1}x", r / n),
            _ => "-".into(),
        };
        println!("{id:<12} {:>12} {:>12} {ratio:>8}  {}", f(n), f(r), row.equal);
    }
    eprintln!("raw: {}", out_dir.canonicalize()?.join("raw.jsonl").display());
    Ok(())
}

pub fn short(s: &str) -> String {
    if s.len() > 60 {
        format!("{}…", &s[..s.char_indices().nth(60).map_or(s.len(), |(i, _)| i)])
    } else {
        s.to_string()
    }
}

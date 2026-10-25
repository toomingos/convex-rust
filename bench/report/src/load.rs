//! Loading a run directory (phase2.jsonl, rss.jsonl, meta.json) and the case catalog.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Node,
    /// rust-wasm (jsonl name "rust", kept for old runs and baselines)
    Rust,
    /// rust-native (jsonl name "rust-native")
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Cold,
    Warm,
    Concurrency,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Cold => "cold",
            Mode::Warm => "warm",
            Mode::Concurrency => "concurrency",
        }
    }
}

/// All ok samples of one (case, runtime, mode, concurrency) cell.
#[derive(Default, Clone, Debug)]
pub struct Series {
    /// Client-observed latency of successful calls, ms.
    pub client_ms: Vec<f64>,
    /// Backend-recorded execution time of successful calls that have it, ms.
    pub exec_ms: Vec<f64>,
    pub n_fail: usize,
    /// Successful calls without `exec_ms`.
    pub n_missing_exec: usize,
}

pub type SeriesKey = (String, Runtime, Mode, u32);

#[derive(Default, Debug)]
pub struct Dataset {
    pub series: BTreeMap<SeriesKey, Series>,
    /// (case, runtime, process) -> peak rss bytes
    pub rss: BTreeMap<(String, Runtime, String), u64>,
    pub meta: Value,
    /// gates.json, when `bench_runner gates` wrote one into the run dir.
    pub gates: Option<Value>,
    pub notes: Vec<String>,
}

impl Dataset {
    /// Series for a case/runtime in cold or warm mode.
    pub fn get(&self, case: &str, rt: Runtime, mode: Mode) -> Option<&Series> {
        self.series.get(&(case.to_string(), rt, mode, 0))
    }
    pub fn case_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.series.keys().map(|k| k.0.clone()).collect();
        v.dedup();
        v
    }
}

#[derive(Deserialize)]
struct Sample {
    case: String,
    runtime: String,
    mode: String,
    #[serde(default)]
    concurrency: Option<u32>,
    client_ns: f64,
    #[serde(default)]
    exec_ms: Option<f64>,
    ok: bool,
}

#[derive(Deserialize)]
struct RssSample {
    case: String,
    runtime: String,
    process: String,
    rss_bytes: u64,
}

fn parse_runtime(s: &str) -> Result<Runtime, String> {
    match s {
        "node" => Ok(Runtime::Node),
        "rust" | "rust-wasm" => Ok(Runtime::Rust),
        "rust-native" => Ok(Runtime::Native),
        o => Err(format!("unknown runtime {o:?}")),
    }
}

fn parse_mode(s: &str) -> Result<Mode, String> {
    match s {
        "cold" => Ok(Mode::Cold),
        "warm" => Ok(Mode::Warm),
        "concurrency" => Ok(Mode::Concurrency),
        o => Err(format!("unknown mode {o:?}")),
    }
}

pub fn load_run(dir: &Path) -> Result<Dataset, String> {
    let mut ds = Dataset::default();
    let p2 = dir.join("phase2.jsonl");
    let f = fs::File::open(&p2).map_err(|e| format!("{}: {e}", p2.display()))?;
    for (ln, line) in BufReader::new(f).lines().enumerate() {
        let line = line.map_err(|e| format!("{}:{}: {e}", p2.display(), ln + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let ctx = |e: String| format!("{}:{}: {e}", p2.display(), ln + 1);
        let s: Sample = serde_json::from_str(&line).map_err(|e| ctx(e.to_string()))?;
        let rt = parse_runtime(&s.runtime).map_err(ctx)?;
        let mode = parse_mode(&s.mode).map_err(ctx)?;
        let conc = if mode == Mode::Concurrency {
            s.concurrency.unwrap_or(1)
        } else {
            0
        };
        let ser = ds.series.entry((s.case, rt, mode, conc)).or_default();
        if !s.ok {
            ser.n_fail += 1;
            continue;
        }
        // Clamp so ratios and logs stay finite.
        ser.client_ms.push((s.client_ns / 1e6).max(1e-6));
        match s.exec_ms {
            Some(e) if e.is_finite() => ser.exec_ms.push(e),
            _ => ser.n_missing_exec += 1,
        }
    }

    let rp = dir.join("rss.jsonl");
    match fs::File::open(&rp) {
        Ok(f) => {
            for (ln, line) in BufReader::new(f).lines().enumerate() {
                let line = line.map_err(|e| format!("{}:{}: {e}", rp.display(), ln + 1))?;
                if line.trim().is_empty() {
                    continue;
                }
                let ctx = |e: String| format!("{}:{}: {e}", rp.display(), ln + 1);
                let s: RssSample = serde_json::from_str(&line).map_err(|e| ctx(e.to_string()))?;
                let rt = parse_runtime(&s.runtime).map_err(ctx)?;
                let e = ds.rss.entry((s.case, rt, s.process)).or_insert(0);
                *e = (*e).max(s.rss_bytes);
            }
        }
        Err(_) => ds.notes.push("rss.jsonl not found: memory columns are empty".into()),
    }

    let mp = dir.join("meta.json");
    match fs::read_to_string(&mp) {
        Ok(t) => ds.meta = serde_json::from_str(&t).map_err(|e| format!("{}: {e}", mp.display()))?,
        Err(_) => {
            ds.meta = Value::Null;
            ds.notes.push("meta.json not found".into());
        }
    }
    if let Ok(t) = fs::read_to_string(dir.join("gates.json")) {
        match serde_json::from_str(&t) {
            Ok(v) => ds.gates = Some(v),
            Err(e) => ds.notes.push(format!("gates.json is not valid JSON ({e}): gates ignored")),
        }
    }
    Ok(ds)
}

#[derive(Clone, Debug)]
pub struct CaseInfo {
    pub id: String,
    pub category: String,
    pub title: String,
    pub node_lib: Option<String>,
    pub rust_lib: Option<String>,
}

impl CaseInfo {
    /// "idiomatic library" grouping label, e.g. `sharp -> image`.
    pub fn lib_label(&self) -> String {
        let f = |o: &Option<String>| o.clone().unwrap_or_else(|| "(none)".into());
        format!("{} -> {}", f(&self.node_lib), f(&self.rust_lib))
    }
}

#[derive(Default, Clone, Debug)]
pub struct Catalog {
    pub cases: Vec<CaseInfo>,
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Catalog, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let arr = v["cases"].as_array().ok_or("cases.json: missing `cases` array")?;
        let lib = |c: &Value, side: &str| c[side]["lib"].as_str().map(str::to_string);
        let cases = arr
            .iter()
            .map(|c| CaseInfo {
                id: c["id"].as_str().unwrap_or("?").to_string(),
                category: c["category"].as_str().unwrap_or("?").to_string(),
                title: c["title"].as_str().unwrap_or("").to_string(),
                node_lib: lib(c, "node"),
                rust_lib: lib(c, "rust"),
            })
            .collect();
        Ok(Catalog { cases })
    }

    pub fn load(path: &Path) -> Result<Catalog, String> {
        let t = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Catalog::parse(&t)
    }

    pub fn get(&self, id: &str) -> Option<&CaseInfo> {
        self.cases.iter().find(|c| c.id == id)
    }
}

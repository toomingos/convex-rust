//! Node side: shells out to `bench/scripts/{run-node,time-node}.ts` (run with `node`, v24).

use std::{collections::HashMap, process::Command};

use wasmtime::{bail, error::Context, Result};
use serde_json::Value;

pub struct NodeRun {
    pub iteration: u64,
    pub ms: f64,
    pub ok: bool,
    pub output: Value,
}

pub enum NodeResult {
    Runs(Vec<NodeRun>),
    Missing(String),
}

fn node(script: &str, extra: &[String]) -> Result<Vec<Value>> {
    let dir = crate::bench_dir();
    let out = Command::new("node")
        .current_dir(&dir)
        .arg(format!("scripts/{script}"))
        .args(extra)
        .output()
        .context("spawning node")?;
    if !out.status.success() {
        bail!(
            "node scripts/{script} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("bad line from node: {l}")))
        .collect()
}

/// `argsFor(case)` as Convex JSON, per case id (`Err` if it can't be built).
pub fn print_args(ids: &[String]) -> Result<HashMap<String, Result<Value, String>>> {
    let mut a = vec!["--print-args".to_string()];
    a.extend(ids.iter().cloned());
    let mut out = HashMap::new();
    for l in node("run-node.ts", &a)? {
        let id = l["id"].as_str().context("id")?.to_string();
        out.insert(
            id,
            match l.get("args") {
                Some(args) => Ok(args.clone()),
                None => Err(l["error"].as_str().unwrap_or("unknown").to_string()),
            },
        );
    }
    Ok(out)
}

pub fn time(ids: &[String], iterations: usize, warmup: usize) -> Result<HashMap<String, NodeResult>> {
    let mut a = vec!["--iterations".to_string(), iterations.to_string(), "--warmup".to_string(), warmup.to_string()];
    a.extend(ids.iter().cloned());
    let mut out: HashMap<String, NodeResult> = HashMap::new();
    for l in node("time-node.ts", &a)? {
        let id = l["case_id"].as_str().context("case_id")?.to_string();
        if let Some(why) = l["missing"].as_str() {
            out.insert(id, NodeResult::Missing(why.to_string()));
            continue;
        }
        let run = NodeRun {
            iteration: l["iteration"].as_u64().unwrap_or(0),
            ms: l["ms"].as_f64().unwrap_or(f64::NAN),
            ok: l["ok"].as_bool().unwrap_or(false),
            output: l["output"].clone(),
        };
        match out.entry(id).or_insert_with(|| NodeResult::Runs(vec![])) {
            NodeResult::Runs(v) => v.push(run),
            NodeResult::Missing(_) => {},
        }
    }
    Ok(out)
}

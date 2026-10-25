//! Phase 1: call the `node/...` and `rust/...` action of every case on a real (forked) local
//! backend over `POST /api/action`, with the same seeded args, and compare the results per the
//! case's `compare` mode (mismatches are not benchmarked). With rust-native selected, the same
//! `rust/...` path is also called on the rust-native backend (default: `--url` port + 10) and
//! must equal node and rust-wasm; a failing native call is reported per case (FAIL / ERROR with
//! the reason) and never aborts the run.
//!
//! cargo run -p bench_runner --release -- phase1 [--cases a,b | --category c] [--url U]
//!     [--admin-key K] [--run-id id] [--timeout-secs N] [--write-expected]
//!     [--runtimes node,rust,rust-native] [--native-url U] [--native-admin-key K]
//!
//! Defaults: url = $CONVEX_SELF_HOSTED_URL or http://127.0.0.1:3220; admin key =
//! $CONVEX_SELF_HOSTED_ADMIN_KEY or bench/.backend-data/admin-key.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    time::Duration,
};

use serde_json::{json, Value};
use wasmtime::{bail, error::Context, Result};

use crate::{api, bench_dir, load_cases, node, short};

struct Opts {
    cases: Option<Vec<String>>,
    category: Option<String>,
    url: String,
    admin_key: String,
    run_id: String,
    timeout_secs: u64,
    write_expected: bool,
    native: bool,
    wasm: bool,
    native_url: String,
    native_admin_key: String,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        cases: None,
        category: None,
        url: api::default_url(),
        admin_key: api::default_admin_key(),
        run_id: format!(
            "phase1-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()
        ),
        timeout_secs: 300,
        write_expected: false,
        native: true,
        wasm: true,
        native_url: String::new(),
        native_admin_key: String::new(),
    };
    let (mut native_url, mut native_key) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--cases" => o.cases = Some(val(a)?.split(',').map(str::to_string).collect()),
            "--category" => o.category = Some(val(a)?),
            "--url" => o.url = val(a)?,
            "--admin-key" => o.admin_key = val(a)?,
            "--run-id" => o.run_id = val(a)?,
            "--timeout-secs" => o.timeout_secs = val(a)?.parse()?,
            "--write-expected" => o.write_expected = true,
            "--runtimes" => {
                let rts: Vec<String> = val(a)?.split(',').map(|r| r.trim().to_string()).collect();
                for r in &rts {
                    if !matches!(r.as_str(), "node" | "rust" | "rust-wasm" | "rust-native") {
                        bail!("unknown runtime {r:?} (node, rust-wasm, rust-native)");
                    }
                }
                o.wasm = rts.iter().any(|r| r == "rust" || r == "rust-wasm");
                o.native = rts.iter().any(|r| r == "rust-native");
            },
            "--native-url" => native_url = Some(val(a)?),
            "--native-admin-key" => native_key = Some(val(a)?),
            other => bail!("unknown option {other}"),
        }
    }
    o.native_url = native_url.unwrap_or_else(|| api::native_url_of(&o.url));
    o.native_admin_key = native_key.unwrap_or_else(|| api::native_admin_key(&o.admin_key));
    if o.admin_key.is_empty() {
        bail!("no admin key (set CONVEX_SELF_HOSTED_ADMIN_KEY or pass --admin-key)");
    }
    Ok(o)
}

/// One action call, normalised to the ABI result envelope: `{ok: value}` or
/// `{error: {kind, data?}}` (messages differ by runtime wrapping, kept separately).
struct Call {
    envelope: Value,
    message: Option<String>,
    ms: f64,
    logs: usize,
}

impl Call {
    fn is_ok(&self) -> bool {
        self.envelope.get("ok").is_some()
    }
}

type CallResult = std::result::Result<Call, String>;

fn call(conn: &api::Conn, agent: &ureq::Agent, path: &str, args: &Value) -> CallResult {
    let raw = api::post_action(&conn, agent, path, args, None);
    let ms = raw.ns as f64 / 1e6;
    let text = raw.body?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON ({e}): {}", short(&text)))?;
    let logs = v["logLines"].as_array().map_or(0, |l| l.len());
    match v["status"].as_str() {
        Some("success") => Ok(Call { envelope: json!({"ok": v["value"]}), message: None, ms, logs }),
        Some("error") => {
            let msg = v["errorMessage"].as_str().unwrap_or("").to_string();
            let envelope = match v.get("errorData") {
                Some(d) if !d.is_null() => json!({"error": {"kind": "convex", "data": d}}),
                _ => json!({"error": {"kind": "user"}}),
            };
            Ok(Call { envelope, message: Some(msg), ms, logs })
        },
        _ => Err(format!("unexpected response: {}", short(&text))),
    }
}

#[derive(Default)]
struct Tally {
    equal: usize,
    mismatch: usize,
    error: usize,
}

fn state(c: &CallResult) -> &'static str {
    match c {
        Ok(c) if c.is_ok() => "ok",
        Ok(_) => "err",
        Err(_) => "FAIL",
    }
}

fn side(c: &CallResult) -> Value {
    match c {
        Ok(c) => json!({"envelope": c.envelope, "error_message": c.message, "client_ms": c.ms, "log_lines": c.logs}),
        Err(e) => json!({"failure": e}),
    }
}

/// `(verdict, note)` for two sides: verdict is `yes`, `MISMATCH` (both ran, outputs differ) or
/// `ERROR` (a call failed, or only one side errored). `a`/`b` are the runtime labels.
fn judge2(case: &crate::Case, an: &str, a: &CallResult, bn: &str, b: &CallResult) -> (&'static str, String) {
    match (a, b) {
        (Err(e), _) => ("ERROR", format!("{an}: {e}")),
        (_, Err(e)) => ("ERROR", format!("{bn}: {e}")),
        (Ok(x), Ok(y)) if x.is_ok() != y.is_ok() => {
            let (who, msg) = if x.is_ok() { (bn, &y.message) } else { (an, &x.message) };
            ("ERROR", format!("only {who} errored: {}", short(msg.as_deref().unwrap_or(""))))
        },
        (Ok(x), Ok(y)) => {
            match convex_fn_test::compare::matches(&case.compare, case.tolerance, &y.envelope, &x.envelope) {
                Ok(()) if x.is_ok() => ("yes", String::new()),
                Ok(()) => (
                    "yes",
                    format!(
                        "both errored; {an}: {} | {bn}: {}",
                        short(x.message.as_deref().unwrap_or("")),
                        short(y.message.as_deref().unwrap_or(""))
                    ),
                ),
                Err(p) => ("MISMATCH", format!("at {p}")),
            }
        },
    }
}

/// Three-way verdict: node vs wasm first (the v1 result), then node vs native. The first
/// non-`yes` verdict wins and its note names the runtime.
fn judge(
    case: &crate::Case,
    n: &CallResult,
    w: Option<&CallResult>,
    r: Option<&CallResult>,
) -> (&'static str, String) {
    let mut note = String::new();
    if let Some(w) = w {
        let (v, nt) = judge2(case, "node", n, "rust-wasm", w);
        if v != "yes" {
            return (v, nt);
        }
        note = nt;
    }
    if let Some(r) = r {
        let (v, nt) = judge2(case, "node", n, "rust-native", r);
        if v != "yes" {
            return (v, nt);
        }
        if note.is_empty() {
            note = nt;
        }
    }
    ("yes", note)
}

/// `phase1 --target cloud`: node vs ts-v8 vs rust-v8 on the Convex cloud deployment (deploy key in
/// $CONVEX_DEPLOY_KEY). ts-v8 is skipped for cases marked n/a in bench/cloud/v8-na.json; net is skipped.
fn run_cloud(args: &[String]) -> Result<()> {
    let key = std::env::var("CONVEX_DEPLOY_KEY").unwrap_or_default();
    if key.is_empty() {
        bail!("--target cloud needs $CONVEX_DEPLOY_KEY");
    }
    let (mut only, mut cat, mut url, mut run_id, mut rts) = (None::<Vec<String>>, None::<String>, None::<String>, None::<String>, vec!["node".to_string(), "ts-v8".into(), "rust-v8".into()]);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| it.next().cloned().with_context(|| format!("{n} needs a value"));
        match a.as_str() {
            "--target" => {
                val(a)?;
            },
            "--cases" => only = Some(val(a)?.split(',').map(str::to_string).collect()),
            "--category" => cat = Some(val(a)?),
            "--url" => url = Some(val(a)?),
            "--run-id" => run_id = Some(val(a)?),
            "--runtimes" => rts = val(a)?.split(',').map(|r| r.trim().to_string()).collect(),
            other => bail!("unknown option {other}"),
        }
    }
    let url = url.unwrap_or_else(|| {
        let name = key.split('|').next().unwrap_or("").rsplit(':').next().unwrap_or("").to_string();
        format!("https://{name}.convex.cloud")
    });
    let run_id = run_id.unwrap_or_else(|| format!("phase1-cloud-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
    let mut cases = load_cases(false)?;
    cases.retain(|c| c.category != "net");
    if let Some(ids) = &only {
        cases.retain(|c| ids.contains(&c.id));
    }
    if let Some(c) = &cat {
        cases.retain(|x| &x.category == c);
    }
    let ids: Vec<String> = cases.iter().map(|c| c.id.clone()).collect();
    let all_args = node::print_args(&ids)?;
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(300)).build();
    let conn = api::Conn { url, admin_key: key };
    let out_dir = bench_dir().join("results").join(&run_id);
    fs::create_dir_all(&out_dir)?;
    let mut jl = std::io::BufWriter::new(fs::File::create(out_dir.join("phase1.jsonl"))?);
    let want = |n: &str| rts.iter().any(|r| r == n);
    println!("{:<9} {:<6} {:<6} {:<6} {:<9} note", "case", "node", "ts-v8", "rs-v8", "equal?");
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for case in &cases {
        let t = tallies.entry(case.category.clone()).or_default();
        let args = match all_args.get(&case.id) {
            Some(Ok(a)) => a.clone(),
            _ => {
                t.error += 1;
                println!("{:<9} args unavailable", case.id);
                continue;
            },
        };
        if let Some(why) = &case.node_na {
            let rs = want("rust-v8").then(|| call(&conn, &agent, &case.rust_path.replacen("rust/", "rustv8/", 1), &args));
            let st = rs.as_ref().map_or("-", state);
            println!("{:<9} {:<6} {:<6} {:<6} {:<9} node n/a ({why}); rust-v8 only", case.id, "-", "-", st, "n/a");
            writeln!(jl, "{}", json!({"case_id": case.id, "category": case.category, "phase": 1, "target": "cloud", "equal": "n/a", "note": format!("node n/a ({why})"), "rust_v8": rs.as_ref().map(side)}))?;
            tallies.entry(case.category.clone()).or_default();
            continue;
        }
        let n = call(&conn, &agent, &case.node_path, &args);
        let ts = (want("ts-v8") && case.v8_na.is_none()).then(|| call(&conn, &agent, &case.v8_path, &args));
        let rs = want("rust-v8").then(|| call(&conn, &agent, &case.rust_path.replacen("rust/", "rustv8/", 1), &args));
        let mut verdict = "yes";
        let mut note = String::new();
        for (name, r) in [("ts-v8", &ts), ("rust-v8", &rs)] {
            if let Some(r) = r {
                let (v, nt) = judge2(case, "node", &n, name, r);
                if v != "yes" && verdict == "yes" {
                    verdict = v;
                    note = format!("{name}: {nt}");
                } else if note.is_empty() {
                    note = nt;
                }
            }
        }
        if ts.is_none() {
            let why = case.v8_na.as_deref().unwrap_or("not selected");
            note = format!("ts-v8 n/a ({why}){}", if note.is_empty() { String::new() } else { format!("; {note}") });
        }
        match verdict {
            "yes" => t.equal += 1,
            "MISMATCH" => t.mismatch += 1,
            _ => t.error += 1,
        }
        let note = note.replace('\n', " ");
        let st = |c: &Option<CallResult>| c.as_ref().map_or("-", state);
        println!("{:<9} {:<6} {:<6} {:<6} {:<9} {note}", case.id, state(&n), st(&ts), st(&rs), verdict);
        writeln!(
            jl,
            "{}",
            json!({"case_id": case.id, "category": case.category, "compare": case.compare, "phase": 1, "target": "cloud",
                   "equal": verdict, "note": note, "node": side(&n), "ts_v8": ts.as_ref().map(side),
                   "ts_v8_na": case.v8_na, "rust_v8": rs.as_ref().map(side)})
        )?;
    }
    jl.flush()?;
    println!("\n{:<8} {:>6} {:>9} {:>6}", "category", "equal", "mismatch", "error");
    let mut tot = Tally::default();
    for (cat, t) in &tallies {
        println!("{cat:<8} {:>6} {:>9} {:>6}", t.equal, t.mismatch, t.error);
        tot.equal += t.equal;
        tot.mismatch += t.mismatch;
        tot.error += t.error;
    }
    println!("{:<8} {:>6} {:>9} {:>6}", "total", tot.equal, tot.mismatch, tot.error);
    eprintln!("results: {}", out_dir.canonicalize()?.join("phase1.jsonl").display());
    Ok(())
}

pub fn run(args: &[String]) -> Result<()> {
    if args.windows(2).any(|w| w[0] == "--target" && w[1] == "cloud") {
        return run_cloud(args);
    }
    let o = parse(args)?;
    let mut cases = load_cases(false)?; // all 100, not only phase0
    if let Some(ids) = &o.cases {
        for id in ids {
            if !cases.iter().any(|x| &x.id == id) {
                bail!("{id} is not in cases.json");
            }
        }
        cases.retain(|x| ids.contains(&x.id));
    }
    if let Some(cat) = &o.category {
        cases.retain(|x| &x.category == cat);
    }
    if cases.is_empty() {
        bail!("no cases selected");
    }
    let ids: Vec<String> = cases.iter().map(|c| c.id.clone()).collect();
    let all_args = node::print_args(&ids)?;

    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(o.timeout_secs)).build();
    let out_dir = bench_dir().join("results").join(&o.run_id);
    fs::create_dir_all(&out_dir)?;
    let mut jl = std::io::BufWriter::new(fs::File::create(out_dir.join("phase1.jsonl"))?);

    let conn = api::Conn { url: o.url.clone(), admin_key: o.admin_key.clone() };
    let conn_native = api::Conn { url: o.native_url.clone(), admin_key: o.native_admin_key.clone() };
    if o.native {
        match api::native_info(&conn_native, &agent) {
            Ok(Some(i)) => eprintln!("rust-native backend {}: mode {}", o.native_url, i["mode"]),
            Ok(None) => eprintln!("warning: {} reports rust-native disabled or missing; native calls will fail per case", o.native_url),
            Err(e) => eprintln!("warning: rust-native backend {} unreachable ({e}); native calls will fail per case", o.native_url),
        }
    }
    println!("{:<9} {:<5} {:<5} {:<5} {:<9} diff / note", "case", "node", "wasm", "nat", "equal?");
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for case in &cases {
        let t = tallies.entry(case.category.clone()).or_default();
        let args = match all_args.get(&case.id) {
            Some(Ok(a)) => a.clone(),
            Some(Err(e)) => {
                println!("{:<9} {:<5} {:<5} {:<5} {:<9} args: {}", case.id, "-", "-", "-", "ERROR", short(e));
                t.error += 1;
                continue;
            },
            None => bail!("no args for {}", case.id),
        };
        let n = call(&conn, &agent, &case.node_path, &args);
        let w = o.wasm.then(|| call(&conn, &agent, &case.rust_path, &args));
        let r = o.native.then(|| call(&conn_native, &agent, &case.rust_path, &args));
        let (equal, note) = judge(case, &n, w.as_ref(), r.as_ref());
        match equal {
            "yes" => t.equal += 1,
            "MISMATCH" => t.mismatch += 1,
            _ => t.error += 1,
        }
        let note = note.replace('\n', " ");
        let st = |c: &Option<CallResult>| c.as_ref().map_or("-", state);
        println!("{:<9} {:<5} {:<5} {:<5} {:<9} {note}", case.id, state(&n), st(&w), st(&r), equal);
        writeln!(
            jl,
            "{}",
            json!({"case_id": case.id, "category": case.category, "compare": case.compare, "phase": 1,
                   "equal": equal, "note": note, "node": side(&n),
                   "rust": w.as_ref().map(side), "rust_native": r.as_ref().map(side)})
        )?;
        if o.write_expected && (case.category == "cb" || case.category == "net") {
            if let Ok(n) = &n {
                fs::write(bench_dir().join("expected").join(format!("{}.json", case.id)), format!("{}\n", n.envelope))?;
            }
        }
    }
    jl.flush()?;

    println!("\n{:<8} {:>6} {:>9} {:>6}", "category", "equal", "mismatch", "error");
    let mut tot = Tally::default();
    for (cat, t) in &tallies {
        println!("{cat:<8} {:>6} {:>9} {:>6}", t.equal, t.mismatch, t.error);
        tot.equal += t.equal;
        tot.mismatch += t.mismatch;
        tot.error += t.error;
    }
    println!("{:<8} {:>6} {:>9} {:>6}", "total", tot.equal, tot.mismatch, tot.error);
    eprintln!("results: {}", out_dir.canonicalize()?.join("phase1.jsonl").display());
    Ok(())
}

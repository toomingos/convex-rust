//! Phase 2: the full benchmark through the backend HTTP API (`POST /api/action`).
//!
//! cargo run -p bench_runner --release -- phase2 --run-id X [--cases a,b | --category c]
//!     [--cold N] [--warmup N] [--iters N] [--concurrency 1,10,50,200] [--seed S] [--quick]
//!     [--profile release|debug] [--url U] [--admin-key K] [--external] [--isolate-rss]
//!     [--conc-min N] [--timeout-secs N] [--runtimes node,rust,rust-native] [--native-url U]
//!     [--native-admin-key K] [--push]
//!
//! Three runtimes: `node`, `rust` (wasm; the name stays `rust` in the jsonl so older runs and the
//! report keep working, the report labels it rust-wasm) and `rust-native`. One deployment holds
//! one Rust artifact, so rust-native runs on a SECOND backend (port + 10, data dir
//! `<data>-native`, `RUST_NATIVE_MODE=worker`); node and rust share the first. `--push` builds
//! and pushes bench/convex to the backends the runner starts (release profile; native with
//! `--rust-runtime native`) before measuring.
//!
//! Output: bench/results/<run-id>/{phase2.jsonl, rss.jsonl, rss_windows.jsonl, blocks.jsonl, meta.json, backend.log}.
//!
//! Definitions (also written to meta.json):
//! * cold: the FIRST call of a (case, runtime) after a backend restart (the restart clears
//!   the wasm module LRU keyed by sha256, the isolate/module caches and the node executor
//!   process). One restart yields one cold sample for every (case, runtime) pair, called in
//!   a seeded random order (`cold_pos` = position in that order; position 0 additionally
//!   pays process-wide one-offs such as the first wasm compile). `--cold N` = N restarts.
//! * warm: `--warmup` discarded calls per runtime, then `--iters` measured calls per runtime,
//!   all selected runtimes interleaved in a seeded random order.
//! * conc: for each level c, per runtime, `max(conc_min, 2c)` calls with c in flight
//!   (c worker threads); records per-call latency plus block wall time / throughput.
//! * exec_ms: backend-recorded `executionTime` from `/api/stream_function_logs`, matched to
//!   the call through the `Convex-Request-Id` header AND the absence of `parentExecutionId`
//!   (child UDFs of runQuery/runMutation/runAction share the request id; the root entry is the
//!   measured call); user_exec_ms: `userExecutionTime` (Rust runtimes only).
//! * ok: the call returned a well-formed action response (success or a thrown error;
//!   some cases intentionally throw) that is neither a backend rejection (e.g.
//!   TooManyConcurrentRequests) nor a different success/error outcome than the one the same
//!   (case, runtime) showed in warm-up. `app_status` says which; rejections are `ok=false`
//!   with `app_status = "rejected"`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufWriter, Write},
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};
use wasmtime::{bail, error::Context, Result};

use crate::{
    api::{self, Conn},
    backend::{self, Backend},
    bench_dir, load_cases, node,
    procs::{RssStats, Sampler},
    repo_root, short, Case,
};

struct Opts {
    cases: Option<Vec<String>>,
    category: Option<String>,
    url: String,
    admin_key: String,
    run_id: String,
    cold: usize,
    warmup: usize,
    iters: usize,
    concurrency: Vec<usize>,
    seed: u64,
    profile: String,
    external: bool,
    isolate_rss: bool,
    conc_min: usize,
    timeout_secs: u64,
    quick: bool,
    runtimes: Vec<Rt>,
    native_url: String,
    native_admin_key: String,
    push: bool,
    /// `--target cloud`: real Convex cloud deployment (no local backends).
    cloud: bool,
}

fn parse(args: &[String]) -> Result<Opts> {
    let quick = args.iter().any(|a| a == "--quick");
    let mut o = Opts {
        cases: None,
        category: None,
        url: api::default_url(),
        admin_key: api::default_admin_key(),
        run_id: format!("phase2-{}", SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()),
        cold: if quick { 2 } else { 20 },
        warmup: if quick { 5 } else { 50 },
        iters: if quick { 30 } else { 500 },
        concurrency: if quick { vec![1, 10] } else { vec![1, 10, 50, 200] },
        seed: 1,
        profile: "release".into(),
        external: false,
        isolate_rss: false,
        conc_min: 50,
        timeout_secs: 300,
        quick,
        runtimes: vec![Rt::Node, Rt::Rust, Rt::Native],
        native_url: String::new(),
        native_admin_key: String::new(),
        push: false,
        cloud: false,
    };
    let mut native_url = None;
    let mut native_key = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--quick" => {},
            "--cases" => o.cases = Some(val(a)?.split(',').map(str::to_string).collect()),
            "--category" => o.category = Some(val(a)?),
            "--url" => o.url = val(a)?,
            "--admin-key" => o.admin_key = val(a)?,
            "--run-id" => o.run_id = val(a)?,
            "--cold" => o.cold = val(a)?.parse()?,
            "--warmup" => o.warmup = val(a)?.parse()?,
            "--iters" => o.iters = val(a)?.parse()?,
            "--concurrency" => {
                o.concurrency = val(a)?.split(',').map(|x| x.trim().parse()).collect::<std::result::Result<_, _>>()?
            },
            "--seed" => o.seed = val(a)?.parse()?,
            "--profile" => o.profile = val(a)?,
            "--external" => o.external = true,
            "--isolate-rss" => o.isolate_rss = true,
            "--conc-min" => o.conc_min = val(a)?.parse()?,
            "--timeout-secs" => o.timeout_secs = val(a)?.parse()?,
            "--runtimes" => {
                o.runtimes = val(a)?.split(',').map(Rt::parse).collect::<Result<_>>()?;
            },
            "--native-url" => native_url = Some(val(a)?),
            "--native-admin-key" => native_key = Some(val(a)?),
            "--push" => o.push = true,
            "--target" => match val(a)?.as_str() {
                "cloud" => o.cloud = true,
                "local" => {},
                t => bail!("unknown --target {t:?} (local, cloud)"),
            },
            other => bail!("unknown option {other}"),
        }
    }
    if o.cloud {
        let key = std::env::var("CONVEX_DEPLOY_KEY").unwrap_or_default();
        if key.is_empty() {
            bail!("--target cloud needs the deploy key in $CONVEX_DEPLOY_KEY");
        }
        o.admin_key = key.clone();
        if !args.iter().any(|a| a == "--url") {
            // dev:<deployment>|<token> or prod:<deployment>|<token>
            let name = key.split('|').next().unwrap_or("").rsplit(':').next().unwrap_or("");
            o.url = std::env::var("CONVEX_URL").unwrap_or_else(|_| format!("https://{name}.convex.cloud"));
        }
        if !args.iter().any(|a| a == "--runtimes") {
            o.runtimes = vec![Rt::Node, Rt::TsV8, Rt::RustV8];
        }
        o.external = true;
    } else if o.runtimes.iter().any(|r| matches!(r, Rt::TsV8 | Rt::RustV8)) {
        bail!("ts-v8 / rust-v8 run on a Convex deployment: use --target cloud");
    }
    o.runtimes.dedup();
    if o.runtimes.is_empty() {
        bail!("--runtimes is empty");
    }
    o.native_url = native_url.unwrap_or_else(|| api::native_url_of(&o.url));
    o.native_admin_key = native_key.unwrap_or_else(|| api::native_admin_key(&o.admin_key));
    if o.admin_key.is_empty() {
        bail!("no admin key (set CONVEX_SELF_HOSTED_ADMIN_KEY or pass --admin-key)");
    }
    if o.iters == 0 || o.concurrency.contains(&0) {
        bail!("--iters and --concurrency values must be >= 1");
    }
    if o.external && !o.cloud && o.cold > 0 {
        eprintln!("note: --external cannot restart the backend; skipping cold runs");
        o.cold = 0;
    }
    Ok(o)
}

/// splitmix64: tiny seeded PRNG (no external crate).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, (self.next() % (i as u64 + 1)) as usize);
        }
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rt {
    Node,
    /// rust-wasm (jsonl name `rust`)
    Rust,
    /// rust-native (second backend)
    Native,
    /// cloud: same TS case code in the default Convex runtime
    TsV8,
    /// cloud: Rust wasm inside a default-runtime action (convex-rust package)
    RustV8,
}
impl Rt {
    pub fn name(self) -> &'static str {
        match self {
            Rt::Node => "node",
            Rt::Rust => "rust",
            Rt::Native => "rust-native",
            Rt::TsV8 => "ts-v8",
            Rt::RustV8 => "rust-v8",
        }
    }
    pub fn parse(s: &str) -> Result<Rt> {
        match s.trim() {
            "node" => Ok(Rt::Node),
            "rust" | "rust-wasm" | "wasm" => Ok(Rt::Rust),
            "rust-native" | "native" => Ok(Rt::Native),
            "ts-v8" => Ok(Rt::TsV8),
            "rust-v8" => Ok(Rt::RustV8),
            o => bail!("unknown runtime {o:?} (node, rust-wasm, rust-native, ts-v8, rust-v8)"),
        }
    }
    /// Same function paths for rust-wasm and rust-native.
    pub fn path(self, c: &Case) -> String {
        match self {
            Rt::Node => c.node_path.clone(),
            Rt::Rust | Rt::Native => c.rust_path.clone(),
            Rt::TsV8 => c.v8_path.clone(),
            // convex-rust output dir convex/rustv8 (clashes otherwise with the fork CLI's convex/rust)
            Rt::RustV8 => c.rust_path.replacen("rust/", "rustv8/", 1),
        }
    }
    /// Whether `c` can run on this runtime (ts-v8: no Node built-ins in the default runtime).
    pub fn runs(self, c: &Case) -> bool {
        !((self == Rt::TsV8 && c.v8_na.is_some()) || (self == Rt::Node && c.node_na.is_some()))
    }
}

/// One measured call, before its backend execution time is resolved.
struct Sample {
    case: String,
    rt: Rt,
    mode: &'static str,
    conc: usize,
    i: usize,
    client_ns: u64,
    start_ns: Option<u64>,
    cold_pos: Option<usize>,
    req_id: String,
    ok: bool,
    app_status: String,
    err: Option<String>,
}

/// Log-stream cursors, one per backend.
#[derive(Default)]
struct Cursors {
    main: f64,
    native: f64,
}

struct Ctx<'a> {
    o: &'a Opts,
    conn: Conn,
    conn_native: Conn,
    agent: ureq::Agent,
    log_agent: ureq::Agent,
    counter: AtomicU64,
    backend: Mutex<Option<Backend>>,
    backend_native: Mutex<Option<Backend>>,
    /// (case, runtime) -> success flag seen in warm-up; later calls with the other outcome are not ok.
    expected: Mutex<HashMap<(String, &'static str), bool>>,
    out: PathBuf,
}

impl Ctx<'_> {
    fn req_id(&self) -> String {
        format!("b2-{}-{}", self.o.run_id, self.counter.fetch_add(1, Ordering::Relaxed))
    }

    fn uses_native(&self) -> bool {
        self.o.runtimes.contains(&Rt::Native)
    }

    fn uses_main(&self) -> bool {
        self.o.runtimes.iter().any(|r| *r != Rt::Native)
    }

    fn conn_of(&self, rt: Rt) -> &Conn {
        if rt == Rt::Native {
            &self.conn_native
        } else {
            &self.conn
        }
    }

    fn call(&self, case: &Case, rt: Rt, args: &Value, mode: &'static str, conc: usize, i: usize) -> Sample {
        let req_id = self.req_id();
        let raw = api::post_action(self.conn_of(rt), &self.agent, &rt.path(case), args, Some(&req_id));
        let (mut ok, mut app_status, mut err) = match &raw.body {
            Ok(t) => match api::action_info(t) {
                Ok((true, _)) => (true, "success".to_string(), None),
                Ok((false, msg)) => {
                    let msg = msg.unwrap_or_default();
                    if api::is_rejection(&msg) {
                        (false, "rejected".to_string(), Some(short(&msg)))
                    } else {
                        (true, "error".to_string(), None)
                    }
                },
                Err(e) => (false, "bad_response".into(), Some(e)),
            },
            Err(e) => (false, "failed".into(), Some(e.clone())),
        };
        // Warm-up records the outcome each (case, runtime) normally has (some cases throw on purpose);
        // anything else afterwards is a failure, not a latency sample.
        let key = (case.id.clone(), rt.name());
        if ok && mode == "warmup" {
            self.expected.lock().unwrap().insert(key, app_status == "success");
        } else if ok && mode != "cold" {
            if let Some(exp) = self.expected.lock().unwrap().get(&key) {
                if *exp != (app_status == "success") {
                    ok = false;
                    err = Some(format!("outcome changed: warm-up was {}, now {app_status}", if *exp { "success" } else { "error" }));
                    app_status = "unexpected_outcome".into();
                }
            }
        }
        Sample {
            case: case.id.clone(),
            rt,
            mode,
            conc,
            i,
            client_ns: raw.ns,
            start_ns: None,
            cold_pos: None,
            req_id,
            ok,
            app_status,
            err,
        }
    }

    fn pid_a(&self) -> Option<u32> {
        if self.o.cloud {
            return None;
        }
        if let Some(b) = self.backend.lock().unwrap().as_ref() {
            return Some(b.pid);
        }
        backend::listener_pid(port_of(&self.o.url))
    }

    fn pid_b(&self) -> Option<u32> {
        if self.o.cloud {
            return None;
        }
        if let Some(b) = self.backend_native.lock().unwrap().as_ref() {
            return Some(b.pid);
        }
        backend::listener_pid(port_of(&self.o.native_url))
    }

    /// Pids of the backends that serve `rts`.
    fn pids_for(&self, rts: &[Rt]) -> Vec<u32> {
        let mut v = vec![];
        if rts.iter().any(|r| *r != Rt::Native) {
            v.extend(self.pid_a());
        }
        if rts.contains(&Rt::Native) {
            v.extend(self.pid_b());
        }
        v
    }

    /// Restarts every backend the selected runtimes need (cold restarts restart both).
    fn restart(&self) -> Result<()> {
        if self.uses_main() {
            let mut g = self.backend.lock().unwrap();
            if let Some(mut b) = g.take() {
                b.stop();
            }
            let port = port_of(&self.o.url);
            *g = Some(Backend::start(&self.o.profile, port, &self.o.url, self.out.join("backend.log"), false)?);
        }
        if self.uses_native() {
            let mut g = self.backend_native.lock().unwrap();
            if let Some(mut b) = g.take() {
                b.stop();
            }
            let port = port_of(&self.o.native_url);
            *g = Some(Backend::start(&self.o.profile, port, &self.o.native_url, self.out.join("backend-native.log"), true)?);
        }
        thread::sleep(Duration::from_millis(1500));
        Ok(())
    }

    /// Reads the log stream of `rt`'s backend until all of `want` have shown up (or progress
    /// stalls) and returns request_id -> completion of the ROOT entry (no parent execution:
    /// child UDFs share the request id and are skipped).
    fn resolve_on(&self, native: bool, cursor: &mut f64, mut want: HashSet<String>) -> HashMap<String, api::Completion> {
        let conn = if native { &self.conn_native } else { &self.conn };
        let mut got = HashMap::new();
        let mut last_progress = Instant::now();
        while !want.is_empty() {
            match api::poll_logs(conn, &self.log_agent, *cursor) {
                Ok((comps, nc)) => {
                    let progressed = !comps.is_empty();
                    *cursor = nc;
                    for c in comps {
                        if c.is_root() && want.remove(&c.request_id) {
                            got.insert(c.request_id.clone(), c);
                        }
                    }
                    if progressed {
                        last_progress = Instant::now();
                    }
                },
                Err(e) => {
                    eprintln!("warning: log stream poll failed: {e}");
                    thread::sleep(Duration::from_millis(500));
                },
            }
            if last_progress.elapsed() > Duration::from_secs(4) {
                break;
            }
        }
        if !want.is_empty() {
            eprintln!("warning: {} calls have no backend execution record ({})", want.len(), if native { "native" } else { "main" });
        }
        got
    }

    /// Log-stream cursors positioned at the current end of the stream (cloud streams keep history;
    /// local runs start from 0 on a freshly started backend).
    fn fresh_cursors(&self) -> Cursors {
        let mut c = Cursors::default();
        if self.o.cloud {
            for _ in 0..50 {
                match api::poll_logs(&self.conn, &self.log_agent, c.main) {
                    Ok((comps, nc)) => {
                        let more = !comps.is_empty() && nc > c.main;
                        c.main = nc;
                        if !more {
                            break;
                        }
                    },
                    Err(_) => break,
                }
            }
        }
        c
    }

    /// Cloud "cold": redeploy with a nonce that changes every function module (bench/cloud/redeploy.sh),
    /// so the next call of each function loads its module again.
    fn cloud_redeploy(&self, k: usize) -> Result<()> {
        let nonce = format!("{}-c{k}-{}", self.o.run_id, SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
        let log = fs::File::create(self.out.join(format!("deploy-cold{k}.log")))?;
        let st = Command::new("sh")
            .arg(bench_dir().join("cloud/redeploy.sh"))
            .arg(&nonce)
            .env("CONVEX_DEPLOY_KEY", &self.o.admin_key)
            .stdout(log.try_clone()?)
            .stderr(log)
            .status()
            .context("running cloud/redeploy.sh")?;
        if !st.success() {
            bail!("cloud redeploy failed; see deploy-cold{k}.log");
        }
        thread::sleep(Duration::from_secs(3));
        Ok(())
    }

    fn resolve(&self, cursors: &mut Cursors, ss: &[Sample]) -> HashMap<String, api::Completion> {
        let pick = |native: bool| -> HashSet<String> {
            ss.iter().filter(|s| s.ok && (s.rt == Rt::Native) == native).map(|s| s.req_id.clone()).collect()
        };
        let mut got = HashMap::new();
        let (a, b) = (pick(false), pick(true));
        if !a.is_empty() {
            got.extend(self.resolve_on(false, &mut cursors.main, a));
        }
        if !b.is_empty() {
            got.extend(self.resolve_on(true, &mut cursors.native, b));
        }
        got
    }
}

pub fn port_of(url: &str) -> u16 {
    url.rsplit(':').next().and_then(|p| p.trim_end_matches('/').parse().ok()).unwrap_or(3220)
}

struct Out {
    samples: BufWriter<fs::File>,
    rss: BufWriter<fs::File>,
    rss_windows: BufWriter<fs::File>,
    blocks: BufWriter<fs::File>,
    cloud: bool,
}

impl Out {
    fn write_samples(&mut self, ctx: &Ctx, cursors: &mut Cursors, ss: Vec<Sample>) -> Result<()> {
        let got = ctx.resolve(cursors, &ss);
        for s in ss {
            let c = got.get(&s.req_id);
            let mut line = json!({
                "case": s.case, "runtime": s.rt.name(), "mode": s.mode, "concurrency": s.conc, "i": s.i,
                "client_ns": s.client_ns,
                "exec_ms": c.map(|c| c.execution_time * 1e3),
                "user_exec_ms": c.and_then(|c| c.user_execution_time).map(|t| t * 1e3),
                "ok": s.ok, "app_status": s.app_status, "req_id": s.req_id,
            });
            let m = line.as_object_mut().unwrap();
            if let Some(c) = c {
                m.insert("exec_env".into(), json!(c.environment));
            }
            if let Some(p) = s.cold_pos {
                m.insert("cold_pos".into(), json!(p));
            }
            if let Some(t) = s.start_ns {
                m.insert("start_ns".into(), json!(t));
            }
            if let Some(e) = s.err {
                m.insert("error".into(), json!(e));
            }
            writeln!(self.samples, "{line}")?;
        }
        self.samples.flush()?;
        Ok(())
    }

    /// `rss_windows.jsonl` gets every window in full; `rss.jsonl` (the report contract:
    /// case, runtime, process, rss_bytes = peak) only the windows attributable to a single
    /// runtime (warm-up and concurrency blocks), one row per process class. rust-native adds
    /// `native-worker` (sum over the worker processes below the native backend).
    fn write_rss(&mut self, case: &str, rt: &str, mode: &str, conc: usize, st: &RssStats) -> Result<()> {
        if self.cloud {
            return Ok(()); // no local processes to sample
        }
        writeln!(
            self.rss_windows,
            "{}",
            json!({"case": case, "runtime": rt, "mode": mode, "concurrency": conc, "samples": st.samples,
                   "backend_start_kb": st.backend_start_kb, "backend_peak_kb": st.backend_peak_kb,
                   "backend_end_kb": st.backend_end_kb, "executor_start_kb": st.exec_start_kb,
                   "executor_peak_kb": st.exec_peak_kb, "executor_end_kb": st.exec_end_kb,
                   "executor_procs_max": st.exec_procs_max,
                   "worker_start_kb": st.worker_start_kb, "worker_peak_kb": st.worker_peak_kb,
                   "worker_end_kb": st.worker_end_kb, "worker_procs_max": st.worker_procs_max})
        )?;
        self.rss_windows.flush()?;
        if rt == "node" || rt == "rust" || rt == "rust-native" {
            let mut classes = vec![("backend", st.backend_peak_kb, st.backend_start_kb), ("executor", st.exec_peak_kb, st.exec_start_kb)];
            if rt == "rust-native" {
                classes.push(("native-worker", st.worker_peak_kb, st.worker_start_kb));
            }
            for (process, peak, start) in classes {
                writeln!(
                    self.rss,
                    "{}",
                    json!({"case": case, "runtime": rt, "process": process, "rss_bytes": peak * 1024,
                           "start_bytes": start * 1024, "mode": mode, "concurrency": conc})
                )?;
            }
            self.rss.flush()?;
        }
        Ok(())
    }
}

fn cmd_out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn git_sha(dir: &std::path::Path) -> Value {
    if !dir.exists() {
        return Value::Null;
    }
    let sha = cmd_out("git", &["-C", &dir.display().to_string(), "rev-parse", "HEAD"]);
    let dirty = !cmd_out("git", &["-C", &dir.display().to_string(), "status", "--porcelain"]).is_empty();
    json!({"sha": sha, "dirty": dirty})
}

fn machine() -> Value {
    if cfg!(target_os = "macos") {
        json!({"os": cmd_out("sw_vers", &["-productVersion"]), "uname": cmd_out("uname", &["-a"]),
               "cpu": cmd_out("sysctl", &["-n", "machdep.cpu.brand_string"]),
               "ncpu": cmd_out("sysctl", &["-n", "hw.ncpu"]), "mem_bytes": cmd_out("sysctl", &["-n", "hw.memsize"])})
    } else {
        let cpu = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string()))
            .unwrap_or_default();
        let mem = fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next().map(str::to_string)).unwrap_or_default();
        json!({"uname": cmd_out("uname", &["-a"]), "cpu": cpu, "ncpu": cmd_out("nproc", &[]), "meminfo": mem})
    }
}

/// Build info written by `scripts/cvx.sh dev` (`scripts/build-info.mjs`); the file's mtime is
/// included so a stale file from an older push is visible.
fn build_info(name: &str) -> Value {
    let p = bench_dir().join("convex").join(name);
    let Ok(text) = fs::read_to_string(&p) else { return Value::Null };
    let mut v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let (Some(m), Ok(md)) = (v.as_object_mut(), fs::metadata(&p)) {
        if let Ok(t) = md.modified().and_then(|t| t.duration_since(UNIX_EPOCH).map_err(std::io::Error::other)) {
            m.insert("file_mtime_unix".into(), json!(t.as_secs()));
        }
    }
    v
}

fn write_meta(ctx: &Ctx, ncases: usize, extra: Value) -> Result<()> {
    let o = ctx.o;
    let fork = std::env::var("CONVEX_BACKEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root().join("vendor/convex-backend"));
    let target = std::env::var("BACKEND_TARGET_DIR").unwrap_or_else(|_| repo_root().join("target-backend").to_string_lossy().into_owned());
    let bin = PathBuf::from(&target).join(&o.profile).join("convex-local-backend");
    let bin_meta = fs::metadata(&bin).ok();
    let native_info = if ctx.uses_native() {
        match api::native_info(&ctx.conn_native, &ctx.log_agent) {
            Ok(Some(v)) => v,
            Ok(None) => json!({"enabled": false}),
            Err(e) => json!({"error": e}),
        }
    } else {
        Value::Null
    };
    let wasm_info = build_info(".build-info.json");
    let native_build = build_info(".build-info-native.json");
    let mut m = json!({
        "run_id": o.run_id,
        "tool": "bench_runner phase2",
        "started_unix": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "started_utc": cmd_out("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]),
        "git": {"main": git_sha(&repo_root().canonicalize().unwrap_or_else(|_| repo_root())), "fork": git_sha(&fork)},
        "machine": machine(),
        "runtimes": o.runtimes.iter().map(|r| r.name()).collect::<Vec<_>>(),
        "backend": {
            "profile": o.profile, "binary": bin.display().to_string(),
            "binary_size": bin_meta.as_ref().map(|m| m.len()),
            "managed_by_runner": !o.external,
            "build": "cargo build --release -p local_backend --bin convex-local-backend (fork env: CONVEX_PREBUILT_JS=1)",
            "pid": ctx.pid_a(),
            "url": o.url,
        },
        "backend_native": {
            "used": ctx.uses_native(), "url": o.native_url, "pid": if ctx.uses_native() { ctx.pid_b() } else { None },
            "env": {"RUST_NATIVE_MODE": "worker"}, "info": native_info,
            "data_dir": api::data_dir(true).display().to_string(),
        },
        "node": {"version": cmd_out("/usr/local/bin/node", &["--version"]), "runner_node_on_path": cmd_out("node", &["--version"])},
        "toolchain": {"rustc": cmd_out("rustc", &["--version"]), "cargo": cmd_out("cargo", &["--version"])},
        // What the Rust artifacts were built with, as recorded by cvx.sh after the push (profile,
        // wasm-opt, artifact bytes). Profile "release" is required for a benchmark.
        "build": {"wasm": wasm_info, "native": native_build},
        "options": {
            "cases": ncases, "category": o.category, "selected": o.cases, "cold_restarts": o.cold, "warmup": o.warmup,
            "iters": o.iters, "concurrency": o.concurrency, "conc_min_calls": o.conc_min, "seed": o.seed,
            "quick": o.quick, "isolate_rss": o.isolate_rss, "url": o.url, "push": o.push,
        },
        "definitions": {
            "runtimes": "node = default-runtime TS action on the main backend; rust (report label rust-wasm) = wasm Rust action on the main backend; rust-native = native Rust action on a second backend (port+10, own data dir, RUST_NATIVE_MODE=worker) pushed with --rust-runtime native. rust-wasm and rust-native call the same function paths.",
            "cold": "first call of a (case, runtime) after a restart of the backends; one restart gives one sample per (case, runtime), called in a seeded random order (cold_pos). cold_pos 0 also pays process-wide first-use costs (first wasm compile / first worker spawn / node executor start).",
            "warm": "warmup discarded calls per runtime, then iters measured calls per runtime, all runtimes interleaved in seeded random order",
            "conc": "per level c and runtime: max(conc_min, 2c) calls with c in flight",
            "client_ns": "wall time of the HTTP round trip incl. reading the body (keep-alive connection)",
            "exec_ms": "backend executionTime of the ROOT entry (no parentExecutionId) from /api/stream_function_logs, matched via Convex-Request-Id; child UDFs of cb-* share the request id and are ignored",
            "user_exec_ms": "backend userExecutionTime (rust runtimes only)",
            "ok": "well-formed action response (success or thrown error) that is not a backend rejection and has the same outcome as the (case, runtime) showed in warm-up; see app_status (success, error, rejected, unexpected_outcome, failed, bad_response)",
            "rejected": "TooManyConcurrentRequests and similar overload errors: ok=false, excluded from latency and throughput, counted as failures",
            "rss": "libproc/proc sampled every ~10 ms; executor = sum over the non-worker descendants of the backend pid; native-worker = sum over descendants running the backend binary itself (rust-native backend only); peak values include earlier cases' high-water mark unless --isolate-rss",
        },
    });
    if let Some(e) = extra.as_object() {
        for (k, v) in e {
            m[k] = v.clone();
        }
    }
    fs::write(ctx.out.join("meta.json"), serde_json::to_string_pretty(&m)? + "\n")?;
    Ok(())
}

fn pctl(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn median(mut v: Vec<f64>) -> f64 {
    v.retain(|x| x.is_finite());
    v.sort_by(|a, b| a.total_cmp(b));
    pctl(&v, 0.5)
}

fn cold_phase(ctx: &Ctx, out: &mut Out, cases: &[Case], args: &HashMap<String, Value>) -> Result<()> {
    let mut rng = Rng(ctx.o.seed ^ 0xC01D);
    let pairs: Vec<(usize, Rt)> = (0..cases.len())
        .flat_map(|c| ctx.o.runtimes.iter().filter(move |r| r.runs(&cases[c])).map(move |r| (c, *r)))
        .collect();
    for k in 0..ctx.o.cold {
        let t = Instant::now();
        if ctx.o.cloud {
            ctx.cloud_redeploy(k)?;
        } else {
            ctx.restart()?;
        }
        let mut cursors = ctx.fresh_cursors();
        let mut order = pairs.clone();
        rng.shuffle(&mut order);
        let mut samples = Vec::with_capacity(order.len());
        for (pos, (ci, rt)) in order.into_iter().enumerate() {
            let case = &cases[ci];
            let mut s = ctx.call(case, rt, &args[&case.id], "cold", 1, k);
            s.cold_pos = Some(pos);
            samples.push(s);
        }
        out.write_samples(ctx, &mut cursors, samples)?;
        eprintln!("cold {}/{} done ({:.1}s)", k + 1, ctx.o.cold, t.elapsed().as_secs_f64());
    }
    Ok(())
}

fn rss_window<T>(ctx: &Ctx, rts: &[Rt], f: impl FnOnce() -> T) -> (T, RssStats) {
    if ctx.o.cloud {
        return (f(), RssStats::default());
    }
    let s = Sampler::start(ctx.pids_for(rts), Duration::from_millis(10));
    let r = f();
    (r, s.finish())
}

fn warm_case(ctx: &Ctx, out: &mut Out, case: &Case, args: &Value, cursors: &mut Cursors) -> Result<()> {
    let o = ctx.o;
    let mut rng = Rng(o.seed ^ hash_str(&case.id));
    if o.isolate_rss && !o.external {
        ctx.restart()?;
    }
    let mut pending: Vec<Sample> = vec![];

    // Warm-up, one runtime at a time (so the RSS window is attributable), random order.
    let case_rts: Vec<Rt> = o.runtimes.iter().copied().filter(|r| r.runs(case)).collect();
    let mut rts = case_rts.clone();
    rng.shuffle(&mut rts);
    for rt in rts {
        let (_, st) = rss_window(ctx, &[rt], || {
            for i in 0..o.warmup {
                ctx.call(case, rt, args, "warmup", 1, i);
            }
        });
        out.write_rss(&case.id, rt.name(), "warmup", 1, &st)?;
    }

    // Measured: interleaved in random order, single RSS window over all runtimes.
    let mut seq: Vec<Rt> = case_rts.iter().flat_map(|rt| std::iter::repeat_n(*rt, o.iters)).collect();
    rng.shuffle(&mut seq);
    let (ss, st) = rss_window(ctx, &case_rts, || {
        let mut idx: HashMap<&'static str, usize> = HashMap::new();
        seq.iter()
            .map(|rt| {
                let i = idx.entry(rt.name()).or_insert(0);
                let s = ctx.call(case, *rt, args, "warm", 1, *i);
                *i += 1;
                s
            })
            .collect::<Vec<_>>()
    });
    out.write_rss(&case.id, "both", "warm", 1, &st)?;
    pending.extend(ss);

    // Concurrency levels: per runtime, random order.
    for &c in &o.concurrency {
        let total = o.conc_min.max(2 * c);
        let mut rts = case_rts.clone();
        rng.shuffle(&mut rts);
        for rt in rts {
            let counter = AtomicUsize::new(0);
            let ((ss, wall), st) = rss_window(ctx, &[rt], || {
                let t0 = Instant::now();
                let all: Mutex<Vec<Sample>> = Mutex::new(Vec::with_capacity(total));
                thread::scope(|sc| {
                    for _ in 0..c.min(total) {
                        sc.spawn(|| {
                            let mut local = vec![];
                            loop {
                                let i = counter.fetch_add(1, Ordering::Relaxed);
                                if i >= total {
                                    break;
                                }
                                let start = t0.elapsed().as_nanos() as u64;
                                let mut s = ctx.call(case, rt, args, "concurrency", c, i);
                                s.start_ns = Some(start);
                                local.push(s);
                            }
                            all.lock().unwrap().extend(local);
                        });
                    }
                });
                (all.into_inner().unwrap(), t0.elapsed())
            });
            let ok = ss.iter().filter(|s| s.ok).count();
            let count = |st: &str| ss.iter().filter(|s| s.app_status == st).count();
            writeln!(
                out.blocks,
                "{}",
                json!({"case": case.id, "runtime": rt.name(), "concurrency": c, "calls": total, "ok": ok,
                       "rejected": count("rejected"), "app_success": count("success"), "app_error": count("error"),
                       "unexpected_outcome": count("unexpected_outcome"),
                       "failed": count("failed") + count("bad_response"),
                       "wall_ns": wall.as_nanos() as u64, "throughput_per_s": total as f64 / wall.as_secs_f64(),
                       "goodput_per_s": ok as f64 / wall.as_secs_f64()})
            )?;
            out.write_rss(&case.id, rt.name(), "concurrency", c, &st)?;
            pending.extend(ss);
        }
    }
    out.blocks.flush()?;
    out.write_samples(ctx, cursors, pending)?;
    Ok(())
}

/// Builds and pushes bench/convex through `scripts/cvx.sh dev --once` (release profile).
/// A failed push of the native app is reported and the run continues (its calls then fail
/// cleanly per case); a failed push of the main app aborts.
pub fn push_app(main_port: u16, native: bool) -> Result<()> {    eprintln!("pushing bench/convex to the {} backend ...", if native { "rust-native" } else { "main" });
    let t = Instant::now();
    let st = Command::new("sh")
        .arg(bench_dir().join("scripts/cvx.sh"))
        .args(["dev", "--once"])
        .env("BENCH_PORT", main_port.to_string())
        .env("BENCH_NATIVE", if native { "1" } else { "0" })
        .env("CONVEX_RUST_PROFILE", "release")
        .status()
        .context("running cvx.sh dev --once")?;
    eprintln!("push finished in {:.0}s ({st})", t.elapsed().as_secs_f64());
    if !st.success() {
        if native {
            eprintln!("warning: rust-native push failed; rust-native calls will fail per case");
        } else {
            bail!("push to the main backend failed");
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    let mut cases = load_cases(false)?;
    if !o.cloud {
        // node_na / v8_na record cloud limits (512 MB Node actions); the local
        // node executor runs every case.
        for c in &mut cases {
            c.node_na = None;
        }
    }
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
    let mut all_args = HashMap::new();
    for (id, a) in node::print_args(&ids)? {
        match a {
            Ok(a) => {
                all_args.insert(id, a);
            },
            Err(e) => eprintln!("skipping {id}: cannot build args: {e}"),
        }
    }
    cases.retain(|c| all_args.contains_key(&c.id));

    let mut skipped_net: Vec<String> = vec![];
    if o.cloud {
        // The net cases call a mock server on localhost, which the cloud cannot reach.
        skipped_net = cases.iter().filter(|c| c.category == "net").map(|c| c.id.clone()).collect();
        cases.retain(|c| c.category != "net");
        if cases.is_empty() {
            bail!("no cases selected (net cases are skipped on the cloud)");
        }
    }
    if !o.cloud && cases.iter().any(|c| c.category == "net") {
        let mock = std::env::var("MOCK_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
        // The mock may answer 404 for "/"; only transport failures matter.
        if let Err(ureq::Error::Transport(t)) = ureq::get(&mock).timeout(Duration::from_secs(2)).call() {
            bail!("net cases selected but the mock server at {mock} is unreachable ({t}); start bench/scripts/start-mock.sh");
        }
    }

    let out_dir = bench_dir().join("results").join(&o.run_id);
    fs::create_dir_all(&out_dir)?;
    let ctx = Ctx {
        conn: Conn { url: o.url.clone(), admin_key: o.admin_key.clone() },
        conn_native: Conn { url: o.native_url.clone(), admin_key: o.native_admin_key.clone() },
        agent: api::agent(Duration::from_secs(o.timeout_secs), o.concurrency.iter().copied().max().unwrap_or(1) + 8),
        log_agent: api::agent(Duration::from_secs(1), 2),
        counter: AtomicU64::new(0),
        backend: Mutex::new(None),
        backend_native: Mutex::new(None),
        expected: Mutex::new(HashMap::new()),
        out: out_dir.clone(),
        o: &o,
    };
    let mut out = Out {
        samples: BufWriter::new(fs::File::create(out_dir.join("phase2.jsonl"))?),
        rss: BufWriter::new(fs::File::create(out_dir.join("rss.jsonl"))?),
        rss_windows: BufWriter::new(fs::File::create(out_dir.join("rss_windows.jsonl"))?),
        blocks: BufWriter::new(fs::File::create(out_dir.join("blocks.jsonl"))?),
        cloud: o.cloud,
    };

    let t_all = Instant::now();
    let mut cold_secs = 0.0;
    // Start (or validate) the backends before writing meta. A restart also proves we can manage them.
    if !o.external {
        ctx.restart()?;
        if o.push {
            if ctx.uses_main() {
                push_app(port_of(&o.url), false)?;
            }
            if ctx.uses_native() {
                push_app(port_of(&o.url), true)?;
            }
            ctx.restart()?; // pushes may leave caches warm; start measuring from clean processes
        }
    } else if o.push {
        eprintln!("note: --push is ignored with --external");
    }
    let cloud_meta = if o.cloud {
        let na: Value = fs::read_to_string(bench_dir().join("cloud/v8-na.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        json!({
            "target": "cloud",
            "cloud": {
                "url": o.url,
                "convex_cli": cmd_out("sh", &["-c", "cd cloud && npx --no-install convex --version 2>/dev/null | tail -1"]),
                "skipped_categories": {"net": "calls a localhost mock server the cloud cannot reach"},
                "skipped_cases": skipped_net,
                "ts_v8_na": na["na"],
                "node_na": na["node_na"],
                "ts_v8_overridden": na["overridden"],
                "rust_v8_build": build_info_at(&bench_dir().join("cloud/convex/rustv8/_wasm/bench_rust.wasm")),
            },
        })
    } else {
        json!({})
    };
    write_meta(&ctx, cases.len(), cloud_meta.clone())?;
    if ctx.uses_native() {
        match api::native_info(&ctx.conn_native, &ctx.log_agent) {
            Ok(Some(_)) => {},
            Ok(None) => eprintln!(
                "warning: {} reports rust-native disabled (RUST_NATIVE_MODE off or endpoint missing); rust-native calls will fail per case",
                o.native_url
            ),
            Err(e) => eprintln!("warning: cannot reach the rust-native backend at {} ({e}); rust-native calls will fail per case", o.native_url),
        }
    }
    if o.cold > 0 {
        let t = Instant::now();
        cold_phase(&ctx, &mut out, &cases, &all_args)?;
        cold_secs = t.elapsed().as_secs_f64();
    }
    if !o.cloud && (ctx.uses_main() && ctx.pid_a().is_none()) || (ctx.uses_native() && ctx.pid_b().is_none()) {
        bail!("cannot find a backend process (main {}, native {})", o.url, o.native_url);
    }
    if !o.external && !o.isolate_rss && o.cold > 0 {
        ctx.restart()?; // clean RSS baseline for the warm phase
    }

    let t_warm = Instant::now();
    let mut cursors = ctx.fresh_cursors();
    for (n, case) in cases.iter().enumerate() {
        let t = Instant::now();
        warm_case(&ctx, &mut out, case, &all_args[&case.id], &mut cursors)?;
        eprintln!("[{}/{}] {} warm+conc {:.1}s", n + 1, cases.len(), case.id, t.elapsed().as_secs_f64());
    }
    let warm_secs = t_warm.elapsed().as_secs_f64();
    out.samples.flush()?;
    write_meta(
        &ctx,
        cases.len(),
        merge(cloud_meta, json!({"finished_utc": cmd_out("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]),
               "timing_s": {"total": t_all.elapsed().as_secs_f64(), "cold_phase": cold_secs, "warm_conc_phase": warm_secs}})),
    )?;
    for slot in [&ctx.backend, &ctx.backend_native] {
        if let Some(mut b) = slot.lock().unwrap().take() {
            b.stop();
        }
    }
    if o.cloud {
        summarize_cloud(&out_dir, &cases)?;
    } else {
        summarize(&out_dir, &cases)?;
    }
    eprintln!("results: {}", out_dir.canonicalize()?.display());
    Ok(())
}

/// Reads `phase2.jsonl` back and prints per-category medians of the per-case ratios:
/// rust-wasm/node, rust-native/node and rust-native/rust-wasm.
pub fn summarize(dir: &std::path::Path, cases: &[Case]) -> Result<()> {
    // (case, runtime, mode) -> (client_ms, exec_ms)
    type Key = (String, String, String);
    let mut m: HashMap<Key, (Vec<f64>, Vec<f64>)> = HashMap::new();
    let f = fs::File::open(dir.join("phase2.jsonl"))?;
    for l in std::io::BufReader::new(f).lines() {
        let v: Value = serde_json::from_str(&l?)?;
        if v["ok"] != true || (v["mode"] != "warm" && v["mode"] != "cold") {
            continue;
        }
        let e = m
            .entry((v["case"].as_str().unwrap_or("").into(), v["runtime"].as_str().unwrap_or("").into(), v["mode"].as_str().unwrap_or("").into()))
            .or_default();
        e.0.push(v["client_ns"].as_f64().unwrap_or(f64::NAN) / 1e6);
        if let Some(x) = v["exec_ms"].as_f64() {
            e.1.push(x);
        }
    }
    let med = |case: &str, rt: &str, mode: &str, exec: bool| -> f64 {
        m.get(&(case.into(), rt.into(), mode.into()))
            .map(|(c, e)| median(if exec { e.clone() } else { c.clone() }))
            .unwrap_or(f64::NAN)
    };
    let mut cats: BTreeMap<&str, Vec<&Case>> = BTreeMap::new();
    for c in cases {
        cats.entry(c.category.as_str()).or_default().push(c);
    }
    println!(
        "\nmedian over cases of per-case p50 ratios (W = rust-wasm, N = rust-native, J = node; '-' = no data)\n{:<6} {:>5} | {:>8} {:>8} {:>8} {:>8} | {:>8} {:>8} {:>8} | {:>9} {:>9} {:>9}",
        "cat", "cases", "W/J warm", "N/J warm", "N/W warm", "N/J exec", "W/J cold", "N/J cold", "N/W cold", "warm J ms", "warm W ms", "warm N ms"
    );
    let f = |x: f64| if x.is_finite() { format!("{x:.3}") } else { "-".into() };
    for (cat, cs) in &cats {
        let col = |mode: &str, exec: bool, rt: &str| median(cs.iter().map(|c| med(&c.id, rt, mode, exec)).collect());
        let ratio = |mode: &str, exec: bool, a: &str, b: &str| {
            median(cs.iter().map(|c| med(&c.id, a, mode, exec) / med(&c.id, b, mode, exec)).collect())
        };
        println!(
            "{:<6} {:>5} | {:>8} {:>8} {:>8} {:>8} | {:>8} {:>8} {:>8} | {:>9} {:>9} {:>9}",
            cat,
            cs.len(),
            f(ratio("warm", false, "rust", "node")),
            f(ratio("warm", false, "rust-native", "node")),
            f(ratio("warm", false, "rust-native", "rust")),
            f(ratio("warm", true, "rust-native", "node")),
            f(ratio("cold", false, "rust", "node")),
            f(ratio("cold", false, "rust-native", "node")),
            f(ratio("cold", false, "rust-native", "rust")),
            f(col("warm", false, "node")),
            f(col("warm", false, "rust")),
            f(col("warm", false, "rust-native")),
        );
    }
    Ok(())
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(x), Some(y)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in y {
            x.insert(k.clone(), v.clone());
        }
    }
    a
}

/// Size and sha256 of the rust-v8 wasm artifact (as pushed by cloud/build-rust.sh).
fn build_info_at(p: &std::path::Path) -> Value {
    match fs::metadata(p) {
        Ok(m) => json!({"bytes": m.len(), "sha256": cmd_out("shasum", &["-a", "256", &p.display().to_string()]).split_whitespace().next().unwrap_or("")}),
        Err(_) => Value::Null,
    }
}

/// Cloud summary: median over cases of per-case p50 ratios (client round trip and server exec_ms).
fn summarize_cloud(dir: &std::path::Path, cases: &[Case]) -> Result<()> {
    type Key = (String, String, String);
    let mut m: HashMap<Key, (Vec<f64>, Vec<f64>)> = HashMap::new();
    let f = fs::File::open(dir.join("phase2.jsonl"))?;
    for l in std::io::BufReader::new(f).lines() {
        let v: Value = serde_json::from_str(&l?)?;
        if v["ok"] != true || (v["mode"] != "warm" && v["mode"] != "cold") {
            continue;
        }
        let e = m.entry((v["case"].as_str().unwrap_or("").into(), v["runtime"].as_str().unwrap_or("").into(), v["mode"].as_str().unwrap_or("").into())).or_default();
        e.0.push(v["client_ns"].as_f64().unwrap_or(f64::NAN) / 1e6);
        if let Some(x) = v["exec_ms"].as_f64() {
            e.1.push(x);
        }
    }
    let med = |c: &str, rt: &str, mode: &str, exec: bool| {
        m.get(&(c.into(), rt.into(), mode.into())).map(|(a, b)| median(if exec { b.clone() } else { a.clone() })).unwrap_or(f64::NAN)
    };
    let mut cats: BTreeMap<&str, Vec<&Case>> = BTreeMap::new();
    for c in cases {
        cats.entry(c.category.as_str()).or_default().push(c);
    }
    println!("\nmedian over cases of per-case p50 ratios (T = ts-v8, R = rust-v8, J = node; '-' = no data)\n{:<6} {:>5} | {:>9} {:>9} {:>9} | {:>9} {:>9} {:>9} | {:>9} {:>9}", "cat", "cases", "T/J warm", "R/J warm", "R/T warm", "T/J exec", "R/J exec", "R/T exec", "T/J cold", "R/J cold");
    let f = |x: f64| if x.is_finite() { format!("{x:.3}") } else { "-".into() };
    for (cat, cs) in &cats {
        let r = |mode: &str, exec: bool, a: &str, b: &str| median(cs.iter().map(|c| med(&c.id, a, mode, exec) / med(&c.id, b, mode, exec)).collect());
        println!(
            "{:<6} {:>5} | {:>9} {:>9} {:>9} | {:>9} {:>9} {:>9} | {:>9} {:>9}",
            cat, cs.len(),
            f(r("warm", false, "ts-v8", "node")), f(r("warm", false, "rust-v8", "node")), f(r("warm", false, "rust-v8", "ts-v8")),
            f(r("warm", true, "ts-v8", "node")), f(r("warm", true, "rust-v8", "node")), f(r("warm", true, "rust-v8", "ts-v8")),
            f(r("cold", false, "ts-v8", "node")), f(r("cold", false, "rust-v8", "node")),
        );
    }
    Ok(())
}

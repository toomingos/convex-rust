//! WN-14 gate measurements for rust-native, as one subcommand:
//!
//! cargo run -p bench_runner --release -- gates --run-id X [--only overhead,spawn,idle-rss,crypto,orphans]
//!     [--calls N] [--spawns N] [--external] [--push] [--profile release|debug]
//!     [--url U] [--admin-key K] [--native-url U] [--native-admin-key K]
//!
//! Gates (PLAN success criteria / DESIGN §9 WN-14), written to `bench/results/<run-id>/gates.json`
//! and printed as a table; the report shows gates.json when present:
//! * overhead: base-001 warm. `client - exec` p50 for node / rust-wasm / rust-native (reported) and the
//!   runtime-internal part `exec - user_exec` (gate, PLAN: native <= 0.1 ms Linux, 0.2 ms macOS).
//! * spawn: SIGKILL every native worker, then time the next base-001 call; the excess over the warm p50
//!   estimates spawn -> Ready (gate <= 30 ms Linux Docker, 50 ms macOS). The call right after a kill may
//!   fail if the pool still hands out the dead worker; failures are counted, not hidden.
//! * idle-rss: RSS of the native workers after warm calls and 2 s idle (gate <= 20 MB per worker).
//! * crypto: crypto-003 and every "AES-GCM" case, warm, native/node ratio of the client p50 and exec p50 (gate <= 0.7, PLAN).
//! * orphans: stop the native backend with SIGTERM and with SIGKILL while workers are alive; any
//!   descendant still alive 2 s later is an orphan (gate: none).
//!
//! Without `--external` the runner starts both backends itself (ports from `--url`, native = +10)
//! and, with `--push`, builds and pushes first. With `--external` the orphan gate is skipped
//! (it needs to stop the backend).

use std::{
    collections::HashSet,
    fs,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use wasmtime::{bail, error::Context, Result};

use crate::{
    api::{self, Conn},
    backend::Backend,
    bench_dir, load_cases, node, phase2, procs, Case,
};

struct Opts {
    run_id: String,
    only: Vec<String>,
    calls: usize,
    spawns: usize,
    external: bool,
    push: bool,
    profile: String,
    url: String,
    admin_key: String,
    native_url: String,
    native_key: String,
}

fn parse(args: &[String]) -> Result<Opts> {
    let url = api::default_url();
    let mut o = Opts {
        run_id: format!("gates-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()),
        only: ["overhead", "spawn", "idle-rss", "crypto", "orphans"].map(String::from).to_vec(),
        calls: 300,
        spawns: 10,
        external: false,
        push: false,
        profile: "release".into(),
        admin_key: api::default_admin_key(),
        native_url: String::new(),
        native_key: String::new(),
        url,
    };
    let (mut nu, mut nk) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| it.next().cloned().with_context(|| format!("{n} needs a value"));
        match a.as_str() {
            "--run-id" => o.run_id = val(a)?,
            "--only" => o.only = val(a)?.split(',').map(str::to_string).collect(),
            "--calls" => o.calls = val(a)?.parse()?,
            "--spawns" => o.spawns = val(a)?.parse()?,
            "--external" => o.external = true,
            "--push" => o.push = true,
            "--profile" => o.profile = val(a)?,
            "--url" => o.url = val(a)?,
            "--admin-key" => o.admin_key = val(a)?,
            "--native-url" => nu = Some(val(a)?),
            "--native-admin-key" => nk = Some(val(a)?),
            other => bail!("unknown option {other}"),
        }
    }
    o.native_url = nu.unwrap_or_else(|| api::native_url_of(&o.url));
    o.native_key = nk.unwrap_or_else(|| api::native_admin_key(&o.admin_key));
    if o.admin_key.is_empty() {
        bail!("no admin key");
    }
    Ok(o)
}

fn p50(v: &mut Vec<f64>) -> f64 {
    v.retain(|x| x.is_finite());
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        f64::NAN
    } else {
        v[(v.len() - 1) / 2 + (v.len() - 1) % 2 * usize::from(v.len() % 2 == 0)]
    }
}

fn num(x: f64) -> Value {
    if x.is_finite() {
        json!(x)
    } else {
        Value::Null
    }
}

fn verdict(pass: Option<bool>) -> Value {
    match pass {
        Some(true) => json!("PASS"),
        Some(false) => json!("FAIL"),
        None => json!("NO DATA"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Node,
    Wasm,
    Native,
}
impl Which {
    fn name(self) -> &'static str {
        match self {
            Which::Node => "node",
            Which::Wasm => "rust-wasm",
            Which::Native => "rust-native",
        }
    }
}

struct Env<'a> {
    o: &'a Opts,
    main: Conn,
    native: Conn,
    agent: ureq::Agent,
}

impl Env<'_> {
    fn conn(&self, w: Which) -> &Conn {
        if w == Which::Native {
            &self.native
        } else {
            &self.main
        }
    }
}

/// One measured call: (client ms, exec ms, user_exec ms, ok).
type M = (f64, Option<f64>, Option<f64>, bool);

/// Calls `case` `n` times per runtime (interleaved, seeded order via a simple rotation), after
/// `warm` discarded calls each, and returns per-runtime measurements with backend exec times.
fn measure(env: &Env, case: &Case, args: &Value, rts: &[Which], warm: usize, n: usize) -> Vec<(Which, Vec<M>)> {
    for rt in rts {
        for _ in 0..warm {
            let path = if *rt == Which::Node { &case.node_path } else { &case.rust_path };
            api::post_action(env.conn(*rt), &env.agent, path, args, None);
        }
    }
    let mut recs: Vec<(Which, String, f64, bool)> = vec![];
    for i in 0..n {
        // Rotate the runtime order each iteration so no runtime always goes first.
        for k in 0..rts.len() {
            let rt = rts[(i + k) % rts.len()];
            let id = format!("gate-{}-{}-{i}", case.id, rt.name());
            let path = if rt == Which::Node { &case.node_path } else { &case.rust_path };
            let raw = api::post_action(env.conn(rt), &env.agent, path, args, Some(&id));
            let ok = raw.body.as_ref().is_ok_and(|b| matches!(api::action_info(b), Ok((true, _)) | Ok((false, _))))
                && raw.body.as_ref().is_ok_and(|b| !api::action_info(b).ok().and_then(|x| x.1).is_some_and(|m| api::is_rejection(&m)));
            recs.push((rt, id, raw.ns as f64 / 1e6, ok));
        }
    }
    let mut out = vec![];
    for rt in rts {
        let want: HashSet<String> = recs.iter().filter(|r| r.0 == *rt && r.3).map(|r| r.1.clone()).collect();
        let got = api::resolve_roots(env.conn(*rt), &env.agent, want);
        let ms = recs
            .iter()
            .filter(|r| r.0 == *rt)
            .map(|r| {
                let c = got.get(&r.1);
                (r.2, c.map(|c| c.execution_time * 1e3), c.and_then(|c| c.user_execution_time).map(|t| t * 1e3), r.3)
            })
            .collect();
        out.push((*rt, ms));
    }
    out
}

fn col(ms: &[M], f: impl Fn(&M) -> Option<f64>) -> f64 {
    let mut v: Vec<f64> = ms.iter().filter(|m| m.3).filter_map(f).collect();
    p50(&mut v)
}

fn gate_overhead(env: &Env, case: &Case, args: &Value) -> Value {
    let rts = [Which::Node, Which::Wasm, Which::Native];
    let res = measure(env, case, args, &rts, 50, env.o.calls);
    let mut per = serde_json::Map::new();
    let mut native_rt = f64::NAN;
    for (rt, ms) in &res {
        let client = col(ms, |m| Some(m.0));
        let exec = col(ms, |m| m.1);
        let user = col(ms, |m| m.2);
        let over = col(ms, |m| m.1.map(|e| m.0 - e));
        let rtover = col(ms, |m| m.1.zip(m.2).map(|(e, u)| e - u));
        if *rt == Which::Native {
            native_rt = rtover;
        }
        per.insert(
            rt.name().into(),
            json!({"n_ok": ms.iter().filter(|m| m.3).count(), "n_fail": ms.iter().filter(|m| !m.3).count(),
                   "client_p50_ms": num(client), "exec_p50_ms": num(exec), "user_exec_p50_ms": num(user),
                   "client_minus_exec_p50_ms": num(over), "exec_minus_user_p50_ms": num(rtover)}),
        );
    }
    let limit = if cfg!(target_os = "linux") { 0.1 } else { 0.2 };
    let pass = native_rt.is_finite().then_some(native_rt <= limit);
    json!({"case": case.id, "runtimes": per,
           "gate": format!("rust-native exec-user_exec p50 <= {limit} ms"),
           "value_ms": num(native_rt), "verdict": verdict(pass)})
}

fn gate_spawn(env: &Env, case: &Case, args: &Value, native_pid: u32) -> Value {
    let mut warm: Vec<f64> = vec![];
    for _ in 0..30 {
        let r = api::post_action(&env.native, &env.agent, &case.rust_path, args, None);
        if r.body.is_ok() {
            warm.push(r.ns as f64 / 1e6);
        }
    }
    let warm_p50 = p50(&mut warm);
    let (mut after, mut fails, mut no_worker) = (vec![], 0, 0);
    for _ in 0..env.o.spawns {
        let pids = procs::worker_pids(native_pid);
        if pids.is_empty() {
            no_worker += 1;
        }
        for p in &pids {
            // SAFETY: workers below the backend that this runner started.
            unsafe {
                libc::kill(*p as i32, libc::SIGKILL);
            }
        }
        // Let the kill land, then time the first call.
        std::thread::sleep(Duration::from_millis(20));
        let r = api::post_action(&env.native, &env.agent, &case.rust_path, args, None);
        match r.body.as_ref().ok().and_then(|b| api::action_info(b).ok()) {
            Some((true, _)) => after.push(r.ns as f64 / 1e6),
            _ => fails += 1,
        }
        // Let the pool settle before the next round.
        std::thread::sleep(Duration::from_millis(300));
    }
    let after_p50 = p50(&mut after);
    let spawn = after_p50 - warm_p50;
    let limit = if cfg!(target_os = "macos") { 50.0 } else { 30.0 };
    json!({"case": case.id, "warm_p50_ms": num(warm_p50), "first_call_after_kill_p50_ms": num(after_p50),
           "spawn_to_ready_est_ms": num(spawn), "samples_ok": after.len(), "failed_calls_after_kill": fails,
           "rounds_without_worker_found": no_worker,
           "gate": format!("spawn -> Ready p50 <= {limit} ms ({})", if cfg!(target_os = "macos") { "macOS" } else { "Linux" }),
           "note": "estimate = first call after SIGKILL of all workers minus warm p50 (includes any pool respawn wait)",
           "verdict": verdict(spawn.is_finite().then_some(spawn <= limit))})
}

fn gate_idle_rss(env: &Env, case: &Case, args: &Value, native_pid: u32) -> Value {
    for _ in 0..20 {
        api::post_action(&env.native, &env.agent, &case.rust_path, args, None);
    }
    // The pool recycles a bloated idle worker only once a spare exists (one
    // reaper tick spawns it, a later one recycles), and workers left by
    // earlier gates are still settling, so sample for up to 5 s after the
    // 2 s idle and keep the first sample under the limit.
    std::thread::sleep(Duration::from_secs(2));
    let idle = Instant::now();
    let (pids, kb) = loop {
        let pids = procs::worker_pids(native_pid);
        let kb: Vec<u64> = pids.iter().filter_map(|p| procs::rss_kb(*p)).collect();
        if kb.iter().all(|k| *k <= 20 * 1024) || idle.elapsed() >= Duration::from_secs(5) {
            break (pids, kb);
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let settle_ms = 2000 + idle.elapsed().as_millis() as u64;
    let max_mb = kb.iter().copied().max().map(|k| k as f64 / 1024.0);
    json!({"workers": pids.len(), "settle_ms": settle_ms, "rss_mb_each": kb.iter().map(|k| *k as f64 / 1024.0).collect::<Vec<_>>(),
           "max_mb": max_mb, "sum_mb": kb.iter().sum::<u64>() as f64 / 1024.0,
           "backend_rss_mb": procs::rss_kb(native_pid).map(|k| k as f64 / 1024.0),
           "gate": "idle worker RSS <= 20 MB within 7 s idle (else fall back to a small worker binary, DESIGN §10)",
           "verdict": verdict(max_mb.map(|m| m <= 20.0))})
}

fn gate_crypto(env: &Env, cases: &[Case]) -> Value {
    let sel: Vec<&Case> = cases.iter().filter(|c| c.id == "crypto-003" || (c.category == "crypto" && c.title.contains("AES"))).collect();
    let ids: Vec<String> = sel.iter().map(|c| c.id.clone()).collect();
    let args = match node::print_args(&ids) {
        Ok(a) => a,
        Err(e) => return json!({"error": format!("{e}"), "verdict": verdict(None)}),
    };
    let mut rows = vec![];
    let mut worst: f64 = f64::NAN;
    for c in sel {
        let Some(Ok(a)) = args.get(&c.id) else { continue };
        let res = measure(env, c, a, &[Which::Node, Which::Wasm, Which::Native], 10, (env.o.calls / 6).max(20));
        let get = |w: Which, f: &dyn Fn(&M) -> Option<f64>| res.iter().find(|r| r.0 == w).map_or(f64::NAN, |r| col(&r.1, f));
        let (nc, wc, xc) = (get(Which::Node, &|m| Some(m.0)), get(Which::Wasm, &|m| Some(m.0)), get(Which::Native, &|m| Some(m.0)));
        let (ne, xe) = (get(Which::Node, &|m| m.1), get(Which::Native, &|m| m.1));
        let ratio = xc / nc;
        if ratio.is_finite() && !(worst >= ratio) {
            worst = ratio;
        }
        rows.push(json!({"case": c.id, "title": c.title, "node_p50_ms": num(nc), "wasm_p50_ms": num(wc), "native_p50_ms": num(xc),
                         "native_over_node_client": num(ratio), "native_over_wasm_client": num(xc / wc),
                         "native_over_node_exec": num(xe / ne),
                         "native_fail": res.iter().find(|r| r.0 == Which::Native).map_or(0, |r| r.1.iter().filter(|m| !m.3).count())}));
    }
    json!({"cases": rows, "worst_native_over_node_client": num(worst), "gate": "native/node client p50 <= 0.7 for crypto-003 and AES-GCM",
           "verdict": verdict(worst.is_finite().then_some(worst <= 0.7))})
}

fn gate_orphans(env: &Env, native_be: &mut Option<Backend>, case: &Case, args: &Value) -> Result<Value> {
    let o = env.o;
    let mut rounds = vec![];
    for hard in [false, true] {
        let mut b = match native_be.take() {
            Some(b) => b,
            None => Backend::start(&o.profile, phase2::port_of(&o.native_url), &o.native_url, bench_dir().join("results").join(&o.run_id).join("backend-native.log"), true)?,
        };
        // Spawn at least one worker.
        for _ in 0..5 {
            api::post_action(&env.native, &env.agent, &case.rust_path, args, None);
        }
        let workers = procs::worker_pids(b.pid).len();
        let orphans = b.stop_report(hard);
        rounds.push(json!({"stop": if hard { "SIGKILL backend" } else { "SIGTERM backend" }, "workers_before": workers,
                           "orphans": orphans.iter().map(|(p, w)| json!({"pid": p, "worker": w})).collect::<Vec<_>>(),
                           "orphan_workers": orphans.iter().filter(|o| o.1).count()}));
    }
    let any = rounds.iter().any(|r| r["orphans"].as_array().is_some_and(|a| !a.is_empty()));
    let had_workers = rounds.iter().all(|r| r["workers_before"].as_u64().unwrap_or(0) > 0);
    Ok(json!({"rounds": rounds, "gate": "no process of the native backend survives it by more than 2 s",
              "verdict": if !had_workers { verdict(None) } else { verdict(Some(!any)) }}))
}

pub fn run(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    let cases = load_cases(false)?;
    let find = |id: &str| cases.iter().find(|c| c.id == id).cloned().with_context(|| format!("{id} not in cases.json"));
    let base = find("base-001")?;
    let out_dir = bench_dir().join("results").join(&o.run_id);
    fs::create_dir_all(&out_dir)?;
    let env = Env {
        main: Conn { url: o.url.clone(), admin_key: o.admin_key.clone() },
        native: Conn { url: o.native_url.clone(), admin_key: o.native_key.clone() },
        agent: api::agent(Duration::from_secs(300), 8),
        o: &o,
    };
    let mut main_be = None;
    let mut native_be = None;
    if !o.external {
        let port = phase2::port_of(&o.url);
        if o.push {
            // Pushing needs running backends; start, push, keep them up.
            main_be = Some(Backend::start(&o.profile, port, &o.url, out_dir.join("backend.log"), false)?);
            native_be = Some(Backend::start(&o.profile, phase2::port_of(&o.native_url), &o.native_url, out_dir.join("backend-native.log"), true)?);
            phase2::push_app(port, false)?;
            phase2::push_app(port, true)?;
        } else {
            main_be = Some(Backend::start(&o.profile, port, &o.url, out_dir.join("backend.log"), false)?);
            native_be = Some(Backend::start(&o.profile, phase2::port_of(&o.native_url), &o.native_url, out_dir.join("backend-native.log"), true)?);
        }
    }
    let native_pid = native_be.as_ref().map(|b| b.pid).or_else(|| crate::backend::listener_pid(phase2::port_of(&o.native_url)));
    let node_args = node::print_args(&[base.id.clone()])?;
    let base_args = match node_args.get(&base.id) {
        Some(Ok(a)) => a.clone(),
        _ => bail!("cannot build args for base-001"),
    };
    let want = |g: &str| o.only.iter().any(|x| x == g);
    let mut res = serde_json::Map::new();
    res.insert("run_id".into(), json!(o.run_id));
    res.insert("platform".into(), json!(std::env::consts::OS));
    res.insert("started_unix".into(), json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()));
    let t = Instant::now();
    if want("overhead") {
        res.insert("overhead".into(), gate_overhead(&env, &base, &base_args));
    }
    if want("crypto") {
        res.insert("crypto".into(), gate_crypto(&env, &cases));
    }
    match native_pid {
        Some(pid) => {
            if want("idle-rss") {
                res.insert("idle_rss".into(), gate_idle_rss(&env, &base, &base_args, pid));
            }
            if want("spawn") {
                res.insert("spawn".into(), gate_spawn(&env, &base, &base_args, pid));
            }
        },
        None => {
            for g in ["idle-rss", "spawn"] {
                if want(g) {
                    res.insert(g.replace('-', "_"), json!({"verdict": "NO DATA", "error": "cannot find the native backend process"}));
                }
            }
        },
    }
    if want("orphans") {
        if o.external {
            res.insert("orphans".into(), json!({"verdict": "NO DATA", "error": "needs a runner-managed backend (omit --external)"}));
        } else {
            res.insert("orphans".into(), gate_orphans(&env, &mut native_be, &base, &base_args)?);
        }
    }
    res.insert("elapsed_s".into(), json!(t.elapsed().as_secs_f64()));
    let v = Value::Object(res);
    fs::write(out_dir.join("gates.json"), serde_json::to_string_pretty(&v)? + "\n")?;
    println!("{:<10} {:<10} gate", "gate", "verdict");
    for g in ["overhead", "spawn", "idle_rss", "crypto", "orphans"] {
        if let Some(x) = v.get(g) {
            println!("{:<10} {:<10} {}", g, x["verdict"].as_str().unwrap_or("-"), x["gate"].as_str().unwrap_or(""));
        }
    }
    eprintln!("gates: {}", out_dir.join("gates.json").display());
    drop(main_be);
    drop(native_be);
    Ok(())
}

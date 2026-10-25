//! Minimal wasmtime host for contracts/ABI.md with REAL timers (the phase-0
//! host in bench/runner completes every task at submit time, which cannot
//! show timer/ordering behaviour). Copied-and-trimmed from bench/runner/src/host.rs.
//!
//! - `sleep {ms}` completes after `ms` of wall time.
//! - `fetch` completes after 20 ms (300 ms when the URL contains "slow") with a
//!   `bodyStreamId`; the body `hel`,`lo`,`end` chunks are delivered with the head.
//! - `task_poll(1)` really blocks (thread::sleep until the earliest due task).

use std::{collections::VecDeque, path::Path, time::{Duration, Instant}};

use base64::Engine as _;
use serde_json::{json, Value};
use wasmtime::{Caller, Config, Engine, Extern, Instance, Linker, Memory, Store};
use wasmtime_wasi::{p1::{self, WasiP1Ctx}, p2::pipe::MemoryOutputPipe, WasiCtxBuilder};

type Result<T> = wasmtime::Result<T>;

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub polls_nonblock: u64,
    pub polls_block: u64,
    pub tasks: u64,
}

pub struct Outcome {
    pub envelope: Value,
    pub wall_ms: f64,
    pub stats: Stats,
    pub logs: Vec<(i32, String)>,
    pub stdio: String,
}

struct State {
    wasi: WasiP1Ctx,
    ready: VecDeque<String>,
    timers: Vec<(Instant, Vec<String>)>,
    next_task: i64,
    next_stream: u64,
    stats: Stats,
    logs: Vec<(i32, String)>,
}

impl State {
    fn promote_due(&mut self) {
        let now = Instant::now();
        self.timers.sort_by_key(|t| t.0);
        while self.timers.first().is_some_and(|t| t.0 <= now) {
            let (_, evs) = self.timers.remove(0);
            self.ready.extend(evs);
        }
    }
}

pub fn run(wasm: &Path, export: &str, args: &str) -> Result<Outcome> {
    let mut cfg = Config::new();
    cfg.epoch_interruption(false);
    let engine = Engine::new(&cfg)?;
    let module = wasmtime::Module::from_file(&engine, wasm)?;
    let mut linker: Linker<State> = Linker::new(&engine);
    p1::add_to_linker_sync(&mut linker, |s: &mut State| &mut s.wasi)?;
    add_convex_imports(&mut linker)?;

    let stdout = MemoryOutputPipe::new(1 << 20);
    let stderr = MemoryOutputPipe::new(1 << 20);
    let wasi = WasiCtxBuilder::new().stdout(stdout.clone()).stderr(stderr.clone()).build_p1();
    let mut store = Store::new(
        &engine,
        State {
            wasi,
            ready: VecDeque::new(),
            timers: vec![],
            next_task: 0,
            next_stream: 0,
            stats: Stats::default(),
            logs: vec![],
        },
    );
    let instance: Instance = linker.instantiate(&mut store, &module)?;
    if let Some(init) = instance.get_func(&mut store, "_initialize") {
        init.typed::<(), ()>(&store)?.call(&mut store, ())?;
    }
    let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| wasmtime::format_err!("no memory"))?;
    let alloc = instance.get_typed_func::<i32, i32>(&mut store, "convex_alloc")?;
    let run = instance.get_typed_func::<(i32, i32), i64>(&mut store, export)?;
    let ptr = alloc.call(&mut store, args.len() as i32)?;
    memory.write(&mut store, ptr as u32 as usize, args.as_bytes())?;

    let t = Instant::now();
    let packed = run.call(&mut store, (ptr, args.len() as i32));
    let wall_ms = t.elapsed().as_secs_f64() * 1e3;
    let envelope = match packed {
        Ok(p) => {
            let (rp, rl) = ((p as u64 >> 32) as usize, (p as u64 & 0xffff_ffff) as usize);
            serde_json::from_slice(&memory.data(&store)[rp..rp + rl])?
        }
        Err(e) => json!({"trap": format!("{e:#}")}),
    };
    let mut stdio = String::from_utf8_lossy(&stdout.contents()).into_owned();
    stdio.push_str(&String::from_utf8_lossy(&stderr.contents()));
    let st = store.data();
    Ok(Outcome { envelope, wall_ms, stats: st.stats.clone(), logs: st.logs.clone(), stdio })
}

fn mem(c: &mut Caller<'_, State>) -> Memory {
    match c.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => panic!("no memory"),
    }
}

fn read(c: &mut Caller<'_, State>, ptr: i32, len: i32) -> Vec<u8> {
    let m = mem(c);
    m.data(&*c)[ptr as u32 as usize..][..len as u32 as usize].to_vec()
}

fn write(c: &mut Caller<'_, State>, bytes: &[u8]) -> Result<i64> {
    let alloc = match c.get_export("convex_alloc") {
        Some(Extern::Func(f)) => f.typed::<i32, i32>(&*c)?,
        _ => panic!("no convex_alloc"),
    };
    let ptr = alloc.call(&mut *c, bytes.len() as i32)?;
    let m = mem(c);
    m.write(&mut *c, ptr as u32 as usize, bytes)?;
    Ok((((ptr as u32 as u64) << 32) | bytes.len() as u64) as i64)
}

fn add_convex_imports(linker: &mut Linker<State>) -> Result<()> {
    linker.func_wrap("convex", "task_submit", |mut c: Caller<'_, State>, ptr: i32, len: i32| -> Result<i64> {
        let req: Value = match serde_json::from_slice(&read(&mut c, ptr, len)) {
            Ok(v) => v,
            Err(_) => return Ok(-1),
        };
        let s = c.data_mut();
        let id = s.next_task;
        let (delay, events): (u64, Vec<Value>) = match req["type"].as_str() {
            Some("sleep") => (req["ms"].as_u64().unwrap_or(0), vec![json!({"type":"done","taskId":id,"ok":null})]),
            Some("fetch") => {
                let url = req["url"].as_str().unwrap_or("");
                let delay = if url.contains("slow") { 300 } else { 20 };
                s.next_stream += 1;
                let sid = format!("00000000-0000-4000-8000-{:012x}", s.next_stream);
                let b = base64::engine::general_purpose::STANDARD;
                (
                    delay,
                    vec![
                        json!({"type":"done","taskId":id,"ok":{"status":200,"statusText":"OK","headers":[],"url":url,"bodyStreamId":sid}}),
                        json!({"type":"chunk","streamId":sid,"data":b.encode("hel")}),
                        json!({"type":"chunk","streamId":sid,"data":b.encode("lo")}),
                        json!({"type":"chunk","streamId":sid,"end":true}),
                    ],
                )
            }
            _ => return Ok(-1),
        };
        s.next_task += 1;
        s.stats.tasks += 1;
        s.timers.push((Instant::now() + Duration::from_millis(delay), events.iter().map(Value::to_string).collect()));
        Ok(id)
    })?;

    linker.func_wrap("convex", "task_poll", |mut c: Caller<'_, State>, block: i32| -> Result<i64> {
        let s = c.data_mut();
        if block != 0 { s.stats.polls_block += 1 } else { s.stats.polls_nonblock += 1 }
        s.promote_due();
        if s.ready.is_empty() && block != 0 {
            s.timers.sort_by_key(|t| t.0);
            match s.timers.first() {
                None => return Ok(-1),
                Some(t) => std::thread::sleep(t.0.saturating_duration_since(Instant::now())),
            }
            s.promote_due();
        }
        match s.ready.pop_front() {
            Some(ev) => write(&mut c, ev.as_bytes()),
            None => Ok(0),
        }
    })?;

    linker.func_wrap("convex", "sync_syscall", |mut c: Caller<'_, State>, op_ptr: i32, op_len: i32, a_ptr: i32, a_len: i32| -> Result<i64> {
        let op = String::from_utf8(read(&mut c, op_ptr, op_len))?;
        let _ = read(&mut c, a_ptr, a_len);
        let reply = if op == "now" {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1e3);
            json!({"ok": ms})
        } else {
            json!({"error": {"message": format!("sync op {op}: unsupported in spike host")}})
        };
        write(&mut c, reply.to_string().as_bytes())
    })?;

    linker.func_wrap("convex", "log", |mut c: Caller<'_, State>, level: i32, ptr: i32, len: i32| -> Result<()> {
        let b = read(&mut c, ptr, len);
        c.data_mut().logs.push((level, String::from_utf8_lossy(&b).into_owned()));
        Ok(())
    })?;
    Ok(())
}

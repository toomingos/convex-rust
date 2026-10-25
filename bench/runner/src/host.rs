//! Host side of the guest ABI v1 (contracts/ABI.md) on wasmtime, phase-0 semantics:
//! sleep completes immediately; syscalls / fetch / storage complete with an error
//! ("not available in phase 0"); sync op `now` returns wall-clock ms.
//! A module is compiled once; every call gets a fresh Store + instance.

use std::{collections::VecDeque, path::Path, time::Instant};

use wasmtime::{bail, error::Context, Result};
use serde_json::{json, Value};
use wasmtime::{
    Caller, Config, Engine, Extern, Instance, InstancePre, Linker, Memory, Store, StoreLimits,
    StoreLimitsBuilder,
};
use wasmtime_wasi::{
    p1::{self, WasiP1Ctx},
    p2::pipe::MemoryOutputPipe,
    WasiCtxBuilder,
};

const MEMORY_LIMIT: usize = 512 * 1024 * 1024;
const STDIO_CAPACITY: usize = 64 * 1024 * 1024;
const EPOCH_TICK_MS: u64 = 100;
const NOT_IN_PHASE0: &str = "not available in phase 0";

pub struct Function {
    /// Full wasm export name, `convex_run:<modulePath>:<name>`.
    pub export: String,
    /// Convex function path, `rust/<module>:<name>`.
    pub path: String,
}

pub struct Module {
    engine: Engine,
    pre: InstancePre<State>,
    pub functions: Vec<Function>,
    timeout_ticks: u64,
}

pub struct CallResult {
    pub envelope: Value,
    pub instantiate_ms: f64,
    pub call_ms: f64,
    pub logs: Vec<(i32, String)>,
}

struct State {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
    events: VecDeque<String>,
    next_task: i64,
    logs: Vec<(i32, String)>,
}

impl Module {
    pub fn load(path: &Path, timeout_secs: u64) -> Result<Module> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        check_manifest(&bytes)?;

        let mut cfg = Config::new();
        cfg.epoch_interruption(true);
        let engine = Engine::new(&cfg)?;
        let module = wasmtime::Module::new(&engine, &bytes)?;

        let mut functions = vec![];
        for e in module.exports() {
            if let Some(rest) = e.name().strip_prefix("convex_run:") {
                let (module_path, name) = rest
                    .rsplit_once(':')
                    .with_context(|| format!("bad export {}", e.name()))?;
                // bench_rust::cpu -> cpu ; app::a::b -> a/b
                let segs: Vec<&str> = module_path.split("::").skip(1).collect();
                functions.push(Function {
                    export: e.name().to_string(),
                    path: format!("rust/{}:{}", segs.join("/"), name),
                });
            }
        }

        let mut linker: Linker<State> = Linker::new(&engine);
        p1::add_to_linker_sync(&mut linker, |s: &mut State| &mut s.wasi)?;
        add_convex_imports(&mut linker)?;
        let pre = linker.instantiate_pre(&module)?;

        // Epoch ticker (one per Module; detached, dies with the process).
        let ticker = engine.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(EPOCH_TICK_MS));
            ticker.increment_epoch();
        });

        Ok(Module {
            engine,
            pre,
            functions,
            timeout_ticks: timeout_secs * 1000 / EPOCH_TICK_MS,
        })
    }

    pub fn function_for(&self, rust_path: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.path == rust_path)
    }

    /// Fresh store + instance, runs one export. Instantiate and call timed separately.
    pub fn call(&self, f: &Function, args_json: &str) -> Result<CallResult> {
        let stdout = MemoryOutputPipe::new(STDIO_CAPACITY);
        let stderr = MemoryOutputPipe::new(STDIO_CAPACITY);

        let t0 = Instant::now();
        let wasi = WasiCtxBuilder::new()
            .stdout(stdout.clone())
            .stderr(stderr.clone())
            .build_p1();
        let mut store = Store::new(
            &self.engine,
            State {
                wasi,
                limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build(),
                events: VecDeque::new(),
                next_task: 0,
                logs: vec![],
            },
        );
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(self.timeout_ticks);
        let instance: Instance = self.pre.instantiate(&mut store)?;
        if let Some(init) = instance.get_func(&mut store, "_initialize") {
            init.typed::<(), ()>(&store)?.call(&mut store, ())?;
        }
        let instantiate_ms = t0.elapsed().as_secs_f64() * 1e3;

        let t1 = Instant::now();
        let outcome = run_export(&mut store, &instance, &f.export, args_json);
        let call_ms = t1.elapsed().as_secs_f64() * 1e3;

        let envelope = match outcome {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("guest returned invalid JSON: {text}"))?,
            // Trap (panic=abort -> unreachable, OOM, epoch timeout, ...).
            Err(e) => json!({"error": {"message": format!("{e:#}"), "kind": "panic"}}),
        };

        let mut logs = std::mem::take(&mut store.data_mut().logs);
        for (level, pipe) in [(1, &stdout), (3, &stderr)] {
            let bytes = pipe.contents();
            for line in String::from_utf8_lossy(&bytes).split_terminator('\n') {
                logs.push((level, line.to_string()));
            }
        }
        Ok(CallResult { envelope, instantiate_ms, call_ms, logs })
    }
}

fn run_export(store: &mut Store<State>, instance: &Instance, export: &str, args: &str) -> Result<String> {
    let memory = instance.get_memory(&mut *store, "memory").context("no memory export")?;
    let alloc = instance.get_typed_func::<i32, i32>(&mut *store, "convex_alloc")?;
    let run = instance.get_typed_func::<(i32, i32), i64>(&mut *store, export)?;
    let len = i32::try_from(args.len())?;
    let ptr = alloc.call(&mut *store, len)?;
    memory.write(&mut *store, ptr as u32 as usize, args.as_bytes())?;
    let packed = run.call(&mut *store, (ptr, len))?;
    let (rptr, rlen) = unpack(packed);
    let data = memory.data(&*store);
    let bytes = data
        .get(rptr..rptr + rlen)
        .context("result envelope out of bounds")?;
    Ok(String::from_utf8(bytes.to_vec())?)
}

fn unpack(v: i64) -> (usize, usize) {
    let v = v as u64;
    ((v >> 32) as usize, (v & 0xffff_ffff) as usize)
}

fn pack(ptr: i32, len: usize) -> i64 {
    (((ptr as u32 as u64) << 32) | len as u64) as i64
}

fn guest_memory(caller: &mut Caller<'_, State>) -> Result<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Ok(m),
        _ => bail!("guest has no memory export"),
    }
}

fn read_guest(caller: &mut Caller<'_, State>, ptr: i32, len: i32) -> Result<Vec<u8>> {
    let mem = guest_memory(caller)?;
    let (p, l) = (ptr as u32 as usize, len as u32 as usize);
    mem.data(&*caller)
        .get(p..p + l)
        .map(<[u8]>::to_vec)
        .context("guest pointer out of bounds")
}

/// `convex_alloc` handshake: allocate in the guest, copy, return packed `(ptr << 32) | len`.
fn write_guest(caller: &mut Caller<'_, State>, bytes: &[u8]) -> Result<i64> {
    let alloc = match caller.get_export("convex_alloc") {
        Some(Extern::Func(f)) => f.typed::<i32, i32>(&*caller)?,
        _ => bail!("guest has no convex_alloc export"),
    };
    let ptr = alloc.call(&mut *caller, i32::try_from(bytes.len())?)?;
    let mem = guest_memory(caller)?;
    mem.write(&mut *caller, ptr as u32 as usize, bytes)?;
    Ok(pack(ptr, bytes.len()))
}

fn add_convex_imports(linker: &mut Linker<State>) -> Result<()> {
    linker.func_wrap("convex", "task_submit", |mut c: Caller<'_, State>, ptr: i32, len: i32| -> Result<i64> {
        let bytes = read_guest(&mut c, ptr, len)?;
        let req: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                c.data_mut().logs.push((3, format!("task_submit: malformed request: {e}")));
                return Ok(-1);
            },
        };
        let id = c.data().next_task;
        let event = match req["type"].as_str() {
            Some("sleep") => json!({"type": "done", "taskId": id, "ok": null}),
            Some("syscall" | "fetch" | "storageStore" | "storageGet") => {
                json!({"type": "done", "taskId": id, "error": {"message": NOT_IN_PHASE0}})
            },
            _ => {
                c.data_mut().logs.push((3, "task_submit: unknown request type".into()));
                return Ok(-1);
            },
        };
        let s = c.data_mut();
        s.next_task += 1;
        s.events.push_back(event.to_string());
        Ok(id)
    })?;

    linker.func_wrap("convex", "task_poll", |mut c: Caller<'_, State>, block: i32| -> Result<i64> {
        match c.data_mut().events.pop_front() {
            Some(ev) => write_guest(&mut c, ev.as_bytes()),
            // Every task completes at submit time, so an empty queue means nothing is outstanding.
            None if block != 0 => Ok(-1),
            None => Ok(0),
        }
    })?;

    linker.func_wrap(
        "convex",
        "sync_syscall",
        |mut c: Caller<'_, State>, op_ptr: i32, op_len: i32, args_ptr: i32, args_len: i32| -> Result<i64> {
            let op = String::from_utf8(read_guest(&mut c, op_ptr, op_len)?)?;
            let _args = read_guest(&mut c, args_ptr, args_len)?;
            let reply = match op.as_str() {
                "now" => {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0.0, |d| d.as_secs_f64() * 1e3);
                    json!({"ok": ms})
                },
                _ => json!({"error": {"message": format!("sync op {op}: {NOT_IN_PHASE0}")}}),
            };
            write_guest(&mut c, reply.to_string().as_bytes())
        },
    )?;

    linker.func_wrap("convex", "log", |mut c: Caller<'_, State>, level: i32, ptr: i32, len: i32| -> Result<()> {
        let bytes = read_guest(&mut c, ptr, len)?;
        c.data_mut().logs.push((level, String::from_utf8_lossy(&bytes).into_owned()));
        Ok(())
    })?;
    Ok(())
}

/// Parses the `convex:manifest` custom section (contracts/MANIFEST.md) and checks abiVersion.
fn check_manifest(wasm: &[u8]) -> Result<()> {
    let mut found = 0;
    let mut seen = std::collections::HashSet::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CustomSection(s) = payload? {
            if s.name() != "convex:manifest" {
                continue;
            }
            for line in std::str::from_utf8(s.data())?.split('\n').filter(|l| !l.trim().is_empty()) {
                let m: Value = serde_json::from_str(line).context("bad manifest fragment")?;
                if m["abiVersion"] != 1 {
                    bail!("unsupported abiVersion {}", m["abiVersion"]);
                }
                let key = format!("{}:{}", m["modulePath"].as_str().unwrap_or(""), m["name"].as_str().unwrap_or(""));
                if !seen.insert(key.clone()) {
                    bail!("duplicate manifest entry {key}");
                }
                found += 1;
            }
        }
    }
    if found == 0 {
        bail!("module has no convex:manifest entries");
    }
    Ok(())
}

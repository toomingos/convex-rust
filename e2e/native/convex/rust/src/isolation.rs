//! What a native action can see: env, filesystem, network, processes (suite 3).
//! Every probe reports instead of failing, so the test decides what is expected.
use convex_fn::{action, json, Ctx, Result, Value};
use serde::Deserialize;
use std::io::Write;

fn io_err(e: &std::io::Error) -> Value {
    json!({ "ok": false, "kind": format!("{:?}", e.kind()), "errno": e.raw_os_error() })
}

#[derive(Deserialize)]
pub struct NoArgs {}

#[derive(Deserialize)]
pub struct EnvArgs {
    names: Vec<String>,
}
/// Values of `names` plus the full list of variable names and a flat dump of all values.
#[action]
pub async fn env_probe(_ctx: &Ctx, a: EnvArgs) -> Result<Value> {
    let vals: Vec<Value> = a
        .names
        .iter()
        .map(|n| std::env::var(n).map(Value::String).unwrap_or(Value::Null))
        .collect();
    let all: Vec<(String, String)> = std::env::vars().collect();
    Ok(json!({
        "values": vals,
        "names": all.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        "allValues": all.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
pub struct PathArgs {
    path: String,
}
#[action]
pub async fn read_path(_ctx: &Ctx, a: PathArgs) -> Result<Value> {
    Ok(match std::fs::read(&a.path) {
        Ok(b) => json!({ "ok": true, "len": b.len() }),
        Err(e) => io_err(&e),
    })
}
#[action]
pub async fn list_dir(_ctx: &Ctx, a: PathArgs) -> Result<Value> {
    Ok(match std::fs::read_dir(&a.path) {
        Ok(d) => json!({ "ok": true, "count": d.count() }),
        Err(e) => io_err(&e),
    })
}

#[derive(Deserialize)]
pub struct AddrArgs {
    addr: String,
}
#[action]
pub async fn tcp_connect(_ctx: &Ctx, a: AddrArgs) -> Result<Value> {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = a.addr.to_socket_addrs().map_err(|e| convex_fn::Error::msg(e.to_string()))?.next();
    let Some(addr) = addr else { return Ok(json!({ "ok": false, "kind": "NoAddr" })) };
    Ok(match TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3)) {
        Ok(_) => json!({ "ok": true }),
        Err(e) => io_err(&e),
    })
}

#[derive(Deserialize)]
pub struct CmdArgs {
    cmd: String,
}
/// Tries to start a process. Under seccomp this kills the worker (SIGSYS), which is the
/// expected "stopped by the sandbox" error; if it returns, the result says why it failed.
#[action]
pub async fn spawn_process(_ctx: &Ctx, a: CmdArgs) -> Result<Value> {
    Ok(match std::process::Command::new(&a.cmd).arg("-c").arg("echo native-e2e-sh").output() {
        Ok(o) => json!({ "ok": true, "stdout": String::from_utf8_lossy(&o.stdout) }),
        Err(e) => io_err(&e),
    })
}

/// Writes, reads back and removes a file in the worker's temp dir.
#[action]
pub async fn tmp_write(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("native-e2e-{}", std::process::id()));
    let res = (|| -> std::io::Result<String> {
        let mut f = std::fs::File::create(&path)?;
        f.write_all(b"hello tmp")?;
        drop(f);
        let s = std::fs::read_to_string(&path)?;
        std::fs::remove_file(&path)?;
        Ok(s)
    })();
    Ok(match res {
        Ok(s) => json!({ "ok": true, "content": s, "dir": dir.to_string_lossy() }),
        Err(e) => io_err(&e),
    })
}

/// Reports the soft limits the worker runs with (Linux: /proc/self/limits).
#[action]
pub async fn limits(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    Ok(match std::fs::read_to_string("/proc/self/limits") {
        Ok(s) => json!({ "ok": true, "text": s }),
        Err(e) => io_err(&e),
    })
}

#[action]
pub async fn cwd(_ctx: &Ctx, _a: NoArgs) -> Result<Value> {
    Ok(json!({ "cwd": std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).ok(), "pid": std::process::id() }))
}

//! Lifecycle of the local backend under test (start / stop / restart), used by the
//! phase-2 cold-start protocol. Starting goes through `bench/scripts/start-backend.sh`
//! (which `exec`s the backend, so the child pid IS the backend pid).

use std::{
    fs,
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use wasmtime::{bail, error::Context, Result};

use crate::{bench_dir, procs};

pub struct Backend {
    child: Option<Child>,
    pub pid: u32,
    pub port: u16,
    log: std::path::PathBuf,
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(200)).is_ok()
}

fn http_ready(url: &str) -> bool {
    ureq::get(&format!("{}/version", url.trim_end_matches('/'))).timeout(Duration::from_secs(2)).call().is_ok()
}

/// Pid listening on `port` (via lsof), for externally managed backends.
pub fn listener_pid(port: u16) -> Option<u32> {
    let out = Command::new("lsof").args(["-ti", &format!("tcp:{port}"), "-sTCP:LISTEN"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
}

impl Backend {
    /// `native = true` starts the rust-native instance: `BENCH_NATIVE=1` makes the script use the
    /// `-native` data dir and `RUST_NATIVE_MODE=worker`.
    pub fn start(profile: &str, port: u16, url: &str, log: std::path::PathBuf, native: bool) -> Result<Backend> {
        if port_open(port) {
            bail!("port {port} is already in use; refusing to start a backend on it");
        }
        let script = bench_dir().join("scripts/start-backend.sh");
        let lf = fs::OpenOptions::new().create(true).append(true).open(&log)?;
        let child = Command::new("sh")
            .arg(&script)
            .arg(port.to_string())
            .arg((port + 1).to_string())
            .env("BACKEND_PROFILE", profile)
            .env("BENCH_NATIVE", if native { "1" } else { "0" })
            .stdin(Stdio::null())
            .stdout(lf.try_clone()?)
            .stderr(lf)
            .spawn()
            .context("spawning start-backend.sh")?;
        let pid = child.id();
        let mut b = Backend { child: Some(child), pid, port, log };
        let t = Instant::now();
        loop {
            if let Some(c) = b.child.as_mut() {
                if let Some(st) = c.try_wait()? {
                    bail!("backend exited during startup ({st}); see {}", b.log.display());
                }
            }
            if http_ready(url) {
                break;
            }
            if t.elapsed() > Duration::from_secs(120) {
                b.stop();
                bail!("backend not ready after 120 s; see {}", b.log.display());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(b)
    }

    /// Stops the backend and everything below it (node executors, native workers); waits for the port.
    pub fn stop(&mut self) {
        self.stop_report(false);
    }

    /// Like `stop`, and returns the descendants (pid, was a native worker) that were still alive
    /// 2 s after the backend itself had exited (orphans), which are then SIGKILLed.
    /// `hard` sends SIGKILL to the backend instead of SIGTERM (crash path: workers must still go).
    pub fn stop_report(&mut self, hard: bool) -> Vec<(u32, bool)> {
        let Some(mut child) = self.child.take() else { return vec![] };
        let be = procs::exe(self.pid);
        let kids: Vec<(u32, bool)> =
            procs::descendants(self.pid).into_iter().map(|p| (p, procs::is_worker(&be, p))).collect();
        // SAFETY: plain kill(2) on pids we spawned.
        unsafe {
            libc::kill(self.pid as i32, if hard { libc::SIGKILL } else { libc::SIGTERM });
        }
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Give children (PDEATHSIG / EOF on the control pipe) a moment to exit on their own.
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) && kids.iter().any(|(k, _)| procs::alive(*k)) {
            thread::sleep(Duration::from_millis(50));
        }
        let mut orphans = vec![];
        for (k, w) in kids {
            if procs::alive(k) {
                orphans.push((k, w));
                // SAFETY: descendants of the backend we started.
                unsafe {
                    libc::kill(k as i32, libc::SIGKILL);
                }
            }
        }
        let t = Instant::now();
        while port_open(self.port) && t.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(50));
        }
        orphans
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop();
    }
}

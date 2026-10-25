//! Process inspection (RSS, child processes) and the RSS sampler thread.
//! macOS uses libproc; Linux reads /proc. Everything else returns nothing.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
mod sys {
    pub fn rss_kb(pid: u32) -> Option<u64> {
        // SAFETY: proc_pidinfo writes at most `size` bytes into a zeroed plain-old-data struct.
        unsafe {
            let mut ti: libc::proc_taskinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
            let r = libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                &mut ti as *mut _ as *mut libc::c_void,
                size,
            );
            (r == size).then_some(ti.pti_resident_size / 1024)
        }
    }

    pub fn exe(pid: u32) -> Option<String> {
        let mut buf = vec![0u8; 4096];
        // SAFETY: buffer valid for the stated length.
        let n = unsafe { libc::proc_pidpath(pid as libc::c_int, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    pub fn children(pid: u32) -> Vec<u32> {
        let mut buf = vec![0i32; 1024];
        // SAFETY: buffer is valid for the stated byte length. The return value's unit
        // (pids vs bytes) differs across macOS versions, so read until the first 0.
        unsafe {
            libc::proc_listchildpids(
                pid as libc::c_int,
                buf.as_mut_ptr() as *mut libc::c_void,
                (buf.len() * 4) as libc::c_int,
            );
        }
        buf.into_iter().take_while(|p| *p > 0).map(|p| p as u32).collect()
    }

}

#[cfg(target_os = "linux")]
mod sys {
    pub fn rss_kb(pid: u32) -> Option<u64> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf is always safe to call.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        Some(pages * page / 1024)
    }

    pub fn exe(pid: u32) -> Option<String> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok().map(|p| p.display().to_string())
    }

    pub fn children(pid: u32) -> Vec<u32> {
        let mut out = vec![];
        if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for t in tasks.flatten() {
                if let Ok(s) = std::fs::read_to_string(t.path().join("children")) {
                    out.extend(s.split_whitespace().filter_map(|x| x.parse::<u32>().ok()));
                }
            }
        }
        out
    }

}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod sys {
    pub fn rss_kb(_: u32) -> Option<u64> {
        None
    }
    pub fn children(_: u32) -> Vec<u32> {
        vec![]
    }
    pub fn exe(_: u32) -> Option<String> {
        None
    }
}

pub use sys::{children, exe, rss_kb};

/// All transitive children of `pid` (not including `pid`).
pub fn descendants(pid: u32) -> Vec<u32> {
    let mut out = vec![];
    let mut stack = vec![pid];
    while let Some(p) = stack.pop() {
        for c in children(p) {
            out.push(c);
            stack.push(c);
        }
    }
    out
}

pub fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[derive(Clone, Debug, Default)]
pub struct RssStats {
    pub samples: u64,
    pub backend_start_kb: u64,
    pub backend_peak_kb: u64,
    pub backend_end_kb: u64,
    /// Sum of RSS over the backend's descendant processes (the node executor(s)).
    pub exec_start_kb: u64,
    pub exec_peak_kb: u64,
    pub exec_end_kb: u64,
    /// Max number of descendant processes seen at once.
    pub exec_procs_max: u64,
    /// Sum of RSS over the native workers (descendants running the backend binary itself),
    /// and their max count. Zero when the backend has no workers.
    pub worker_start_kb: u64,
    pub worker_peak_kb: u64,
    pub worker_end_kb: u64,
    pub worker_procs_max: u64,
}

/// One instantaneous reading of a backend and everything below it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Snap {
    pub backend_kb: u64,
    /// Node executors and other descendants that are not native workers.
    pub exec_kb: u64,
    pub exec_procs: u64,
    pub worker_kb: u64,
    pub worker_procs: u64,
}

/// Native workers are re-execs of the backend binary: a descendant whose executable has the
/// same file name as the backend's is a worker, anything else (node) is an executor.
pub fn is_worker(backend_exe: &Option<String>, pid: u32) -> bool {
    match (backend_exe, exe(pid)) {
        (Some(b), Some(p)) => file_name(b) == file_name(&p),
        _ => false,
    }
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Pids of the native workers below `backend`.
pub fn worker_pids(backend: u32) -> Vec<u32> {
    let be = exe(backend);
    descendants(backend).into_iter().filter(|p| is_worker(&be, *p)).collect()
}

pub fn snap(backend: u32) -> Snap {
    let be = exe(backend);
    let mut s = Snap { backend_kb: rss_kb(backend).unwrap_or(0), ..Snap::default() };
    for p in descendants(backend) {
        let kb = rss_kb(p).unwrap_or(0);
        if is_worker(&be, p) {
            s.worker_kb += kb;
            s.worker_procs += 1;
        } else {
            s.exec_kb += kb;
            s.exec_procs += 1;
        }
    }
    s
}

pub struct Sampler {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<RssStats>,
}

impl Sampler {
    /// Samples the backend process and its descendants every `period` in a side thread. Several
    /// backends (the main one and the rust-native one) are sampled together: `backends` lists
    /// `(pid)`s and the stats are summed per class.
    pub fn start(backends: Vec<u32>, period: Duration) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let handle = thread::spawn(move || {
            let read = || {
                let mut t = Snap::default();
                for b in &backends {
                    let s = snap(*b);
                    t.backend_kb += s.backend_kb;
                    t.exec_kb += s.exec_kb;
                    t.exec_procs += s.exec_procs;
                    t.worker_kb += s.worker_kb;
                    t.worker_procs += s.worker_procs;
                }
                t
            };
            let mut st = RssStats::default();
            let first = read();
            st.backend_start_kb = first.backend_kb;
            st.backend_peak_kb = first.backend_kb;
            st.exec_start_kb = first.exec_kb;
            st.exec_peak_kb = first.exec_kb;
            st.exec_procs_max = first.exec_procs;
            st.worker_start_kb = first.worker_kb;
            st.worker_peak_kb = first.worker_kb;
            st.worker_procs_max = first.worker_procs;
            let mut last = first;
            while !s2.load(Ordering::Relaxed) {
                let t = Instant::now();
                let n = read();
                st.samples += 1;
                st.backend_peak_kb = st.backend_peak_kb.max(n.backend_kb);
                st.exec_peak_kb = st.exec_peak_kb.max(n.exec_kb);
                st.exec_procs_max = st.exec_procs_max.max(n.exec_procs);
                st.worker_peak_kb = st.worker_peak_kb.max(n.worker_kb);
                st.worker_procs_max = st.worker_procs_max.max(n.worker_procs);
                last = n;
                if let Some(rest) = period.checked_sub(t.elapsed()) {
                    thread::sleep(rest);
                }
            }
            st.backend_end_kb = last.backend_kb;
            st.exec_end_kb = last.exec_kb;
            st.worker_end_kb = last.worker_kb;
            st
        });
        Sampler { stop, handle }
    }

    pub fn finish(self) -> RssStats {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap_or_default()
    }
}

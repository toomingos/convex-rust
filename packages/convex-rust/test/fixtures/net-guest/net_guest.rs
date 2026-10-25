//! A scriptable ABI v1 guest for the `net/*` host tests (host.net.test.ts).
//!
//! The call argument is a JSON string holding one command per line:
//!   fill N              BUF = N bytes, BUF[i] = i % 251
//!   submit <json>       task_submit; prints `s:<taskId>`
//!   sync <op> <json>    sync_syscall; prints `r:<reply>`
//!   poll                blocking task_poll; prints `e:<event>` (`e:none` when idle)
//! In `<json>`, `@PTR@` / `@LEN@` are BUF's address / length and `@PTR+N@` is BUF's address plus N.
//! The result is the printed lines joined by `\n`.
//!
//! Rebuild: node test/fixtures/net-guest/build.mjs  (writes ../net-guest.wasm)

#[link(wasm_import_module = "convex")]
extern "C" {
    fn task_submit(p: *const u8, l: usize) -> i64;
    fn task_poll(block: i32) -> i64;
    fn sync_syscall(op: *const u8, ol: usize, a: *const u8, al: usize) -> i64;
}

#[no_mangle]
pub extern "C" fn convex_alloc(n: usize) -> *mut u8 {
    let mut v: Vec<u8> = Vec::with_capacity(n.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

fn unpack(r: i64) -> &'static [u8] {
    let ptr = ((r >> 32) as u32) as usize;
    let len = (r as u32) as usize;
    unsafe { std::slice::from_raw_parts(ptr as *const u8, len) }
}

fn pack(v: Vec<u8>) -> i64 {
    let (p, l) = (v.as_ptr() as usize, v.len());
    std::mem::forget(v);
    ((p as i64) << 32) | l as i64
}

/// Unescapes the JSON string the host passes as the call argument.
fn unjson(raw: &str) -> String {
    let inner = raw.trim().trim_start_matches('"').trim_end_matches('"');
    let mut out = String::new();
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn subst(json: &str, buf: &[u8]) -> String {
    let mut s = json.replace("@PTR@", &(buf.as_ptr() as usize).to_string());
    s = s.replace("@LEN@", &buf.len().to_string());
    while let Some(i) = s.find("@PTR+") {
        let rest = &s[i + 5..];
        let end = rest.find('@').unwrap_or(0);
        let off: usize = rest[..end].parse().unwrap_or(0);
        let addr = buf.as_ptr() as usize + off;
        s = format!("{}{}{}", &s[..i], addr, &rest[end + 1..]);
    }
    s
}

#[export_name = "convex_run:t::script"]
pub extern "C" fn script(p: usize, l: usize) -> i64 {
    let raw = unsafe { std::slice::from_raw_parts(p as *const u8, l) };
    let script = unjson(std::str::from_utf8(raw).unwrap_or(""));
    let mut buf: Vec<u8> = vec![];
    let mut out: Vec<String> = vec![];
    for line in script.lines().filter(|l| !l.trim().is_empty()) {
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "fill" => {
                let n: usize = rest.trim().parse().unwrap_or(0);
                buf = (0..n).map(|i| (i % 251) as u8).collect();
            }
            "submit" => {
                let j = subst(rest, &buf);
                let id = unsafe { task_submit(j.as_ptr(), j.len()) };
                out.push(format!("s:{id}"));
            }
            "sync" => {
                let (op, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let a = subst(args, &buf);
                let r = unsafe { sync_syscall(op.as_ptr(), op.len(), a.as_ptr(), a.len()) };
                out.push(format!("r:{}", String::from_utf8_lossy(unpack(r))));
            }
            "poll" => {
                let r = unsafe { task_poll(1) };
                if r == -1 {
                    out.push("e:none".into());
                } else {
                    out.push(format!("e:{}", String::from_utf8_lossy(unpack(r))));
                }
            }
            other => out.push(format!("bad command {other}")),
        }
    }
    pack(format!("{{\"ok\":\"{}\"}}", esc(&out.join("\n"))).into_bytes())
}

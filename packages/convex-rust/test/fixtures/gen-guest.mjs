// Generates guest.wasm, a hand-written ABI v1 guest used by the host tests.
// Run: node test/fixtures/gen-guest.mjs   (needs binaryen's `wasm-opt` on PATH)
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = dirname(fileURLToPath(import.meta.url));
const strings = {
  okArr: '{"ok":[',
  ok: '{"ok":',
  closeArr: "]}",
  failed: '{"ok":"submit-failed"}',
  panic: "Uncaught panic: boom at src/x.rs:1:2",
  userErr: '{"error":{"message":"bad","kind":"user"}}',
  convexErr: '{"error":{"message":"cx","kind":"convex","data":{"code":42}}}',
  notJson: "not json",
  neither: '{"x":1}',
  now: "now",
  empty: "{}",
  out: "out1\npartial",
  err: "err1\n",
  yes: '{"ok":true}',
  no: '{"ok":false}',
  enosys: '{"ok":52}',
  notEnosys: '{"ok":-1}',
  big: '{"ok":"x"}',
  compOp: "componentArgument",
  compArgs: '{"name":"k"}',
};
let off = 256;
const at = {};
let data = "";
for (const [k, s] of Object.entries(strings)) {
  at[k] = { off, len: Buffer.byteLength(s) };
  data += `(data (i32.const ${off}) "${[...Buffer.from(s)].map((b) => "\\" + b.toString(16).padStart(2, "0")).join("")}")\n`;
  off += Buffer.byteLength(s) + 8;
}
const S = (k) => `(i32.const ${at[k].off}) (i32.const ${at[k].len})`;
const retStatic = (k) => `(call $pack ${S(k)})`;

const wat = `(module
 (import "convex" "task_submit" (func $submit (param i32 i32) (result i64)))
 (import "convex" "task_poll" (func $poll (param i32) (result i64)))
 (import "convex" "sync_syscall" (func $sync (param i32 i32 i32 i32) (result i64)))
 (import "convex" "log" (func $log (param i32 i32 i32)))
 (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
 (import "wasi_snapshot_preview1" "fd_close" (func $fd_close (param i32) (result i32)))
 (memory (export "memory") 32)
 (global $brk (mut i32) (i32.const 8192))
 ${data}
 (func (export "_initialize") (i32.store8 (i32.const 100) (i32.const 1)))
 (func $alloc (export "convex_alloc") (param $n i32) (result i32) (local $p i32)
  (local.set $p (global.get $brk))
  (global.set $brk (i32.add (global.get $brk) (i32.and (i32.add (local.get $n) (i32.const 7)) (i32.const -8))))
  (local.get $p))
 (func $pack (param $p i32) (param $l i32) (result i64)
  (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32)) (i64.extend_i32_u (local.get $l))))
 ;; {"ok":<body>} -> packed
 (func $wrap (param $p i32) (param $l i32) (result i64) (local $o i32)
  (local.set $o (call $alloc (i32.add (local.get $l) (i32.const 7))))
  (memory.copy (local.get $o) (i32.const ${at.ok.off}) (i32.const ${at.ok.len}))
  (memory.copy (i32.add (local.get $o) (i32.const ${at.ok.len})) (local.get $p) (local.get $l))
  (i32.store8 (i32.add (local.get $o) (i32.add (i32.const ${at.ok.len}) (local.get $l))) (i32.const 125))
  (call $pack (local.get $o) (i32.add (local.get $l) (i32.const 7))))
 (func (export "convex_run:t::echo") (param $p i32) (param $l i32) (result i64)
  (call $wrap (local.get $p) (local.get $l)))
 (func (export "convex_run:t::inited") (param $p i32) (param $l i32) (result i64)
  (if (result i64) (i32.load8_u (i32.const 100)) (then ${retStatic("yes")}) (else ${retStatic("no")})))
 ;; submits the args as a task request, then drains events until none are outstanding
 (func (export "convex_run:t::proxy") (param $p i32) (param $l i32) (result i64)
  (local $id i64) (local $out i32) (local $w i32) (local $ev i64) (local $ep i32) (local $el i32) (local $first i32)
  (local.set $first (i32.const 1))
  (local.set $out (call $alloc (i32.const 262144)))
  (local.set $w (local.get $out))
  (local.set $id (call $submit (local.get $p) (local.get $l)))
  (if (i64.lt_s (local.get $id) (i64.const 0)) (then (return ${retStatic("failed")})))
  (memory.copy (local.get $w) (i32.const ${at.okArr.off}) (i32.const ${at.okArr.len}))
  (local.set $w (i32.add (local.get $w) (i32.const ${at.okArr.len})))
  (block $done
   (loop $l
    (local.set $ev (call $poll (i32.const 1)))
    (br_if $done (i64.eq (local.get $ev) (i64.const -1)))
    (local.set $ep (i32.wrap_i64 (i64.shr_u (local.get $ev) (i64.const 32))))
    (local.set $el (i32.wrap_i64 (local.get $ev)))
    (if (local.get $first)
     (then (local.set $first (i32.const 0)))
     (else (i32.store8 (local.get $w) (i32.const 44)) (local.set $w (i32.add (local.get $w) (i32.const 1)))))
    (memory.copy (local.get $w) (local.get $ep) (local.get $el))
    (local.set $w (i32.add (local.get $w) (local.get $el)))
    (br $l)))
  (memory.copy (local.get $w) (i32.const ${at.closeArr.off}) (i32.const ${at.closeArr.len}))
  (local.set $w (i32.add (local.get $w) (i32.const ${at.closeArr.len})))
  (call $pack (local.get $out) (i32.sub (local.get $w) (local.get $out))))
 ;; poll(0) with nothing queued, then poll(1) with nothing outstanding
 (func (export "convex_run:t::idle") (param $p i32) (param $l i32) (result i64)
  (local $a i64) (local $b i64)
  (local.set $a (call $poll (i32.const 0)))
  (local.set $b (call $poll (i32.const 1)))
  (if (result i64) (i32.and (i64.eq (local.get $a) (i64.const 0)) (i64.eq (local.get $b) (i64.const -1)))
   (then ${retStatic("yes")}) (else ${retStatic("no")})))
 (func (export "convex_run:t::sync") (param $p i32) (param $l i32) (result i64) (local $r i64)
  (local.set $r (call $sync ${S("now")} ${S("empty")}))
  (call $wrap (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32))) (i32.wrap_i64 (local.get $r))))
 (func (export "convex_run:t::syncunknown") (param $p i32) (param $l i32) (result i64) (local $r i64)
  (local.set $r (call $sync ${S("empty")} ${S("empty")}))
  (call $wrap (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32))) (i32.wrap_i64 (local.get $r))))
 (func (export "convex_run:t::synccomp") (param $p i32) (param $l i32) (result i64) (local $r i64)
  (local.set $r (call $sync ${S("compOp")} ${S("compArgs")}))
  (call $wrap (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32))) (i32.wrap_i64 (local.get $r))))
 (func (export "convex_run:t::panic") (param $p i32) (param $l i32) (result i64)
  (call $log (i32.const 1) ${S("out")})
  (call $log (i32.const 3) ${S("panic")})
  unreachable)
 (func (export "convex_run:t::trap") (param $p i32) (param $l i32) (result i64) unreachable)
 (func (export "convex_run:t::err") (param $p i32) (param $l i32) (result i64) ${retStatic("userErr")})
 (func (export "convex_run:t::cerr") (param $p i32) (param $l i32) (result i64) ${retStatic("convexErr")})
 (func (export "convex_run:t::notjson") (param $p i32) (param $l i32) (result i64) ${retStatic("notJson")})
 (func (export "convex_run:t::neither") (param $p i32) (param $l i32) (result i64) ${retStatic("neither")})
 (func (export "convex_run:t::print") (param $p i32) (param $l i32) (result i64)
  (call $log (i32.const 0) ${S("now")})
  (call $log (i32.const 1) ${S("now")})
  (call $log (i32.const 2) ${S("now")})
  (i32.store (i32.const 3000) (i32.const ${at.out.off}))
  (i32.store (i32.const 3004) (i32.const ${at.out.len}))
  (drop (call $fd_write (i32.const 1) (i32.const 3000) (i32.const 1) (i32.const 3100)))
  (i32.store (i32.const 3008) (i32.const ${at.err.off}))
  (i32.store (i32.const 3012) (i32.const ${at.err.len}))
  (drop (call $fd_write (i32.const 2) (i32.const 3008) (i32.const 1) (i32.const 3100)))
  ${retStatic("yes")})
 (func (export "convex_run:t::enosys") (param $p i32) (param $l i32) (result i64)
  (if (result i64) (i32.eq (call $fd_close (i32.const 3)) (i32.const 52)) (then ${retStatic("enosys")}) (else ${retStatic("notEnosys")})))
)
`;
writeFileSync(join(dir, "guest.wat"), wat);
execFileSync("wasm-opt", ["--enable-bulk-memory", "--enable-multivalue", join(dir, "guest.wat"), "-o", join(dir, "guest.wasm")], { stdio: "inherit" });
console.log("wrote guest.wasm");

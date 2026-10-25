import test, { after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  cleanup, makeApp, push, callRun, write, read, constAction, cli, baseEnv, exec, CLI, TOOLCHAIN,
} from "../lib/harness.mjs";

after(cleanup);

const DUMP = !!process.env.DEPLOY_DUMP;
const dump = (label, r) => DUMP && console.log(`----- ${label} (exit ${r.code})\n${r.out}\n-----`);

test("6. #[action] at the crate root (lib.rs): push-time user error with the documented message", async () => {
  const app = await makeApp();
  write(
    app,
    "convex/rust/src/lib.rs",
    `pub mod example;

use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[action]
pub async fn rooted(_ctx: &Ctx, _args: NoArgs) -> Result<String> {
    Ok("root".to_string())
}
`,
  );
  const p = await push(app);
  dump("6", p);
  assert.notEqual(p.code, 0, "push must be rejected");
  assert.match(p.out, /Rust function in the crate root \(module path "convex_rust"\) is not supported; move it into a module\./);
  assert.match(p.out, /Rust build failed/);
});

test("7a. hand-written extern \"C\" host import: push rejected by the import allow-list", async () => {
  const app = await makeApp();
  write(
    app,
    "convex/rust/src/example.rs",
    `use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[link(wasm_import_module = "my_host")]
extern "C" {
    fn steal_secrets(x: i32) -> i32;
}

#[action]
pub async fn greet(_ctx: &Ctx, _args: NoArgs) -> Result<String> {
    Ok(format!("{}", unsafe { steal_secrets(1) }))
}
`,
  );
  const p = await push(app);
  dump("7a", p);
  assert.notEqual(p.code, 0, "push must be rejected");
  assert.match(p.out, /unsupported import `my_host::steal_secrets`/);
  assert.match(p.out, /may only import/);
});

test("7b. WASI socket import (sock_accept): module-level allow-list admits it; the call fails at runtime (documented)", async () => {
  // The allow-list is by import MODULE (`convex`, `wasi_snapshot_preview1`), not per WASI function,
  // so a socket-using dependency is not rejected at push time. The host returns an errno instead.
  const app = await makeApp();
  write(
    app,
    "convex/rust/src/example.rs",
    `use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

// what a socket-using dependency compiles down to on wasm32-wasip1
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn sock_accept(fd: i32, flags: i32, out: *mut i32) -> i32;
}

#[action]
pub async fn greet(_ctx: &Ctx, _args: NoArgs) -> Result<String> {
    let mut fd = 0i32;
    let e = unsafe { sock_accept(3, 0, &mut fd) };
    Ok(format!("errno={e}"))
}
`,
  );
  const p = await push(app);
  dump("7b", p);
  assert.equal(p.code, 0, p.out);
  const r = await callRun(app, "rust/example:greet", {});
  dump("7b run", r);
  assert.equal(r.code, 0, r.out);
  const m = r.stdout.match(/errno=(\d+)/);
  assert.ok(m && m[1] !== "0", `sockets must not work: ${r.out}`);
});

test("7c. std::net::TcpListener / std::thread in a crate: documented outcome", async () => {
  // std's wasip1 TcpListener/thread::spawn compile (they return Unsupported at runtime, no import
  // beyond the allowed WASI set), so the push is accepted and the call fails at runtime.
  const app = await makeApp();
  write(
    app,
    "convex/rust/src/example.rs",
    `use convex_fn::{action, Ctx, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NoArgs {}

#[action]
pub async fn greet(_ctx: &Ctx, _args: NoArgs) -> Result<String> {
    let t = std::thread::Builder::new().spawn(|| 1).map(|_| "spawned".to_string());
    let l = std::net::TcpListener::bind("127.0.0.1:0").map(|_| "bound".to_string());
    Ok(format!("thread={:?} net={:?}", t.map_err(|e| e.to_string()), l.map_err(|e| e.to_string())))
}
`,
  );
  const p = await push(app);
  dump("7c push", p);
  if (p.code !== 0) {
    assert.match(p.out, /unsupported import/, p.out); // rejected by allow-list: acceptable and clear
    return;
  }
  const r = await callRun(app, "rust/example:greet", {});
  dump("7c run", r);
  assert.equal(r.code, 0, r.out);
  assert.match(r.stdout, /Err\(/, "threads/sockets must not silently work");
});

test("8a. wasm32-wasip1 target missing for the pinned toolchain: actionable error, nothing installed", async (t) => {
  // `stable` is installed on this machine WITHOUT the wasm target (checked below). We only point
  // rust-toolchain.toml at it; we never run rustup target add / toolchain install.
  let installed = "";
  try {
    installed = execFileSync("rustup", ["target", "list", "--installed", "--toolchain", "stable"], { encoding: "utf8" });
  } catch {
    return t.skip("no `stable` toolchain installed to simulate a missing target");
  }
  if (installed.includes("wasm32-wasip1")) return t.skip("stable already has wasm32-wasip1; cannot simulate safely");
  const app = await makeApp();
  write(app, "convex/rust/rust-toolchain.toml", `[toolchain]\nchannel = "stable"\n`);
  const p = await push(app, { env: { RUSTUP_TOOLCHAIN: null } });
  dump("8a", p);
  assert.notEqual(p.code, 0);
  assert.match(p.out, /wasm32-wasip1/);
  assert.match(p.out, /rustup target add wasm32-wasip1 --toolchain stable/);
  assert.equal(
    execFileSync("rustup", ["target", "list", "--installed", "--toolchain", "stable"], { encoding: "utf8" }).includes("wasm32-wasip1"),
    false,
    "the CLI must not have installed the target non-interactively",
  );
});

test("8b. toolchain not installed: actionable error with the install command", async () => {
  const app = await makeApp();
  write(app, "convex/rust/rust-toolchain.toml", `[toolchain]\nchannel = "nightly-2020-01-01"\n`);
  const p = await push(app, { env: { RUSTUP_TOOLCHAIN: null, RUSTUP_AUTO_INSTALL: "0" } });
  dump("8b", p);
  assert.notEqual(p.code, 0);
  assert.match(p.out, /nightly-2020-01-01/);
  assert.match(p.out, /rustup toolchain install nightly-2020-01-01 --profile minimal --target wasm32-wasip1/);
});

test("8c. cargo/rustup not on PATH: actionable error pointing at rustup.rs", async () => {
  const app = await makeApp();
  const p = await exec(process.execPath, [CLI, "dev", "--once"], {
    cwd: app.dir,
    env: { PATH: "/usr/bin:/bin" },
  });
  dump("8c", p);
  assert.notEqual(p.code, 0);
  assert.match(p.out, /rustup\.rs/);
  assert.match(p.out, /cargo/);
});

test("9. TS file importing from convex/rust: behaviour", async () => {
  const app = await makeApp();
  write(app, "convex/rust/src/example.rs", constAction("greet", "hi"));
  write(
    app,
    "convex/bad.ts",
    `import { action } from "./_generated/server";
// @ts-ignore: there is no TS module here, only a Rust crate
import { greet } from "./rust/example";

export const viaImport = action({ args: {}, handler: async () => String(greet) });
`,
  );
  const p = await push(app);
  dump("9", p);
  assert.notEqual(p.code, 0, "importing a Rust module from TS must fail at push time");
  assert.match(p.out, /Could not resolve "\.\/rust\/example"/);
  assert.match(p.out, /bad\.ts/);

  // removing the bad file makes the same app deploy again
  write(app, "convex/bad.ts", `export {};\n`);
  const ok = await push(app);
  assert.equal(ok.code, 0, ok.out);
  assert.match((await callRun(app, "rust/example:greet", {})).stdout, /hi/);
  void read; void cli; void baseEnv; void TOOLCHAIN;
});

test("1b. rust-toolchain.toml in convex/rust is honoured when the CLI runs from the project root", async () => {
  process.env.DEPLOY_NO_TOOLCHAIN_WORKAROUND = "1";
  try {
    const app = await makeApp();
    const p = await push(app, { env: { RUSTUP_TOOLCHAIN: null } });
    dump("1b", p);
    assert.equal(p.code, 0, p.out);
  } finally {
    delete process.env.DEPLOY_NO_TOOLCHAIN_WORKAROUND;
  }
});

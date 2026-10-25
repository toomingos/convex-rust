// Real Rust (std::fs, convex_fn::intl, par_map) built for wasm32-wasip1 and run under the JS host
// (Node + JSPI). Skipped when the wasm32-wasip1 toolchain is missing.
import { describe, expect, it, beforeAll, vi } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { runRust } from "../src/runtime/index.js";
import { TmpFs } from "../src/runtime/tmpfs.js";
import { tzOffsetSeconds, intlOp } from "../src/runtime/intl.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");

const haveToolchain = spawnSync("rustup", ["target", "list", "--installed"], {
  encoding: "utf8",
  cwd: path.join(repo, "e2e/spec/convex/rust"),
}).stdout?.includes("wasm32-wasip1");

let wasm: Buffer;
let mod: WebAssembly.Module;
const call = (ctx: any, name: string, args: any = {}, opts?: any) =>
  runRust(ctx, mod, `convex_run:host_fixture:${name}`, args, opts);
const ctx0 = () => ({}) as any;

beforeAll(() => {
  if (!haveToolchain) return;
  const proj = fs.mkdtempSync(path.join(os.tmpdir(), "host-fixture-"));
  fs.cpSync(path.join(here, "fixtures/host-crate"), proj, { recursive: true });
  const toml = path.join(proj, "Cargo.toml");
  fs.writeFileSync(toml, fs.readFileSync(toml, "utf8").replace("CONVEX_FN_PATH", path.join(repo, "crates/convex_fn")));
  fs.copyFileSync(path.join(repo, "e2e/spec/rust-toolchain.toml"), path.join(proj, "rust-toolchain.toml"));
  fs.copyFileSync(path.join(repo, "e2e/spec/convex/rust/Cargo.lock"), path.join(proj, "Cargo.lock"));
  const target = process.env.CARGO_TARGET_DIR ?? path.join(repo, "target-e2e");
  const r = spawnSync("cargo", ["build", "--release", "--target", "wasm32-wasip1"], {
    cwd: proj,
    env: { ...process.env, CARGO_TARGET_DIR: target },
    encoding: "utf8",
  });
  if (r.status !== 0) throw new Error(`cargo build failed:\n${r.stderr}`);
  wasm = fs.readFileSync(path.join(target, "wasm32-wasip1/release/host_fixture.wasm"));
  fs.rmSync(proj, { recursive: true, force: true });
  mod = new WebAssembly.Module(new Uint8Array(wasm));
}, 600_000);

describe.skipIf(!haveToolchain)("in-memory /tmp under the host (std::fs)", () => {
  it("write, read, append, seek, list, rename, remove, errors", async () => {
    const r = await call(ctx0(), "fs_roundtrip");
    expect(r).toMatchObject({
      tmp: "/tmp",
      read: "hello world",
      tail: "world!!",
      pos: 13,
      all: [1, 2, 3, 4, 5, 0, 0, 0],
      len: 13,
      isFile: true,
      names: ["a.txt", "rw.bin", "sub"],
      moved: "hello world!!",
      oldGone: true,
      notEmpty: true,
      missing: true,
      exists: true,
      after: 0,
    });
  });

  it("is empty at the start of every call and discarded after it (instance reuse included)", async () => {
    for (let i = 0; i < 3; i++) expect((await call(ctx0(), "fs_leave")).before).toEqual([]);
  });

  it("round-trips 8 MiB (segments are allocated through the guest's convex_alloc)", async () => {
    const r = await call(ctx0(), "fs_big", { mb: 8 });
    expect(r).toEqual({ len: 8 << 20, sum: 7 * (8 << 20) });
  });

  it("enforces the size cap with ENOSPC and the cap is configurable", async () => {
    const r = await call(ctx0(), "fs_big", { mb: 6 }, { tmpBytes: 4 * 1024 * 1024 });
    expect(r.failedAtMb).toBeGreaterThanOrEqual(2);
    expect(r.failedAtMb).toBeLessThan(6);
    expect(r.raw).toBe(51);
    // space is returned after the call
    expect(await call(ctx0(), "fs_big", { mb: 3 }, { tmpBytes: 4 * 1024 * 1024 })).toMatchObject({ len: 3 << 20 });
  });

  it("tmpBytes: 0 removes /tmp", async () => {
    await expect(call(ctx0(), "fs_leave", {}, { tmpBytes: 0 })).rejects.toThrow();
  });
});

describe("TmpFs without a guest allocator (JS-backed segments)", () => {
  const mk = (maxBytes = 1 << 20) => new TmpFs({ maxBytes, memory: () => null as any });
  const RW = 1n << 6n;
  const text = (s: string) => new TextEncoder().encode(s);
  it("files, directories, unlinked-but-open files, quota", () => {
    const f = mk(16 * 1024);
    const fd = f.open(3, "a", 1, RW, 0); // O_CREAT
    f.write(fd, [text("hello")]);
    f.seek(fd, 0n, 0);
    const buf = new Uint8Array(10);
    expect(f.read(fd, [buf])).toBe(5);
    expect(new TextDecoder().decode(buf.subarray(0, 5))).toBe("hello");
    const used = f.used;
    f.unlink(3, "a"); // still open: data stays
    expect(f.used).toBe(used);
    f.seek(fd, 0n, 0);
    expect(f.read(fd, [buf])).toBe(5);
    f.close(fd);
    expect(f.used).toBe(0);
    f.createDir(3, "d");
    expect(() => f.createDir(3, "d")).toThrow();
    expect(() => f.stat(3, "../x")).toThrow(); // cannot leave the preopen
    const big = f.open(3, "d/big", 1, RW, 0);
    expect(() => f.write(big, [new Uint8Array(64 * 1024)])).toThrow();
  });
});

describe("intl ops", () => {
  it("tzOffset follows DST and rejects unknown zones", () => {
    expect(tzOffsetSeconds("Europe/Lisbon", Date.UTC(2024, 0, 15))).toBe(0);
    expect(tzOffsetSeconds("Europe/Lisbon", Date.UTC(2024, 6, 15))).toBe(3600);
    expect(tzOffsetSeconds("America/New_York", Date.UTC(2024, 0, 15))).toBe(-5 * 3600);
    expect(tzOffsetSeconds("America/New_York", Date.UTC(2024, 6, 15))).toBe(-4 * 3600);
    expect(tzOffsetSeconds("Asia/Kolkata", Date.UTC(2024, 0, 15))).toBe(19800);
    expect(() => tzOffsetSeconds("Mars/Olympus", 0)).toThrow();
    expect(intlOp("intl/tzOffset", { tz: "Nope/Zone", epochMs: 0 })).toMatchObject({ error: { message: expect.any(String) } });
  });
  it("format", () => {
    expect(intlOp("intl/format", { kind: "number", value: 1234.5, locale: "de-DE", options: {} })).toEqual({ ok: "1.234,5" });
    const d = intlOp("intl/format", { kind: "date", value: Date.UTC(2024, 6, 15, 12), locale: "en-US", options: { timeZone: "Asia/Tokyo", hour: "numeric", hourCycle: "h23" } });
    expect(d).toEqual({ ok: "21" });
  });
});

describe.skipIf(!haveToolchain)("convex_fn::intl from Rust", () => {
  it("offsets, formats, local->epoch and a loud error for an unknown zone", async () => {
    const ms = Date.UTC(2024, 6, 15, 12);
    const r = await call(ctx0(), "tz", { ms, tz: "Europe/Lisbon", local: Date.UTC(2024, 6, 15, 13) });
    expect(r.off).toBe(3600);
    expect(r.local).toBe(ms);
    expect(r.fmt).toMatch(/Monday.*15 July 2024.*13:00:00/);
    expect(r.num).toMatch(/1\.234\.567,89\s€/);
    const bad = await call(ctx0(), "tz", { ms, tz: "Not/AZone" });
    expect(bad.off).toBeNull();
    expect(bad.err).toMatch(/intl\/tzOffset/);
  });
});

describe.skipIf(!haveToolchain)("fan-out (par_map, run_action_each)", () => {
  it("keeps the order and never exceeds the concurrency limit", async () => {
    let live = 0;
    let peak = 0;
    const ctx = {
      runAction: vi.fn(async (_ref: any, args: any) => {
        live++;
        peak = Math.max(peak, live);
        await new Promise((r) => setTimeout(r, 15));
        live--;
        return args.x * 2;
      }),
    };
    const r = await call(ctx, "fanout", { n: 7, limit: 3 });
    expect(r.vals).toEqual([0, 10, 20, 30, 40, 50, 60]);
    expect(r.via).toEqual([0, 2, 4, 6, 8, 10, 12]);
    expect(ctx.runAction).toHaveBeenCalledTimes(7);
    expect(peak).toBe(3);
  });
});

describe.skipIf(!haveToolchain)("instance reuse leaves no guest state behind", () => {
  const stream = (chunks: string[], close: boolean) =>
    new ReadableStream<Uint8Array>({
      start(c) {
        for (const s of chunks) c.enqueue(new TextEncoder().encode(s));
        if (close) c.close();
      },
    });

  it("a body stream dropped mid-way does not swallow the next call's stream (same ids)", async () => {
    let n = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        n++;
        // The first body never ends; the second is complete.
        return new Response(n === 1 ? stream(["aaaa"], false) : stream(["BBBB", "CCCC"], true));
      }),
    );
    try {
      for (let i = 0; i < 2; i++) {
        n = 0;
        expect(await call(ctx0(), "drop_stream", { url: "http://x.test/a" })).toEqual({ first: 4 });
        expect(await call(ctx0(), "read_all", { url: "http://x.test/b" })).toEqual({ body: "BBBBCCCC" });
      }
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("a dropped (timed out) fetch task does not swallow the next call's results (same task ids)", async () => {
    let n = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_u: string, init: any) => {
        n++;
        if (n % 2 === 1) {
          return new Promise<Response>((_, rej) =>
            init.signal.addEventListener("abort", () => rej(new Error("aborted"))),
          );
        }
        return new Response("ok");
      }),
    );
    try {
      for (let i = 0; i < 2; i++) {
        expect(await call(ctx0(), "fetch_or_timeout", { url: "http://x.test/hang", ms: 20 })).toEqual({ timedOut: true });
        expect(await call(ctx0(), "fetch_or_timeout", { url: "http://x.test/ok", ms: 2000 })).toEqual({ status: 200 });
      }
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

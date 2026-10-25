// Minimal WASI preview1 for a `wasm32-wasip1` reactor (contracts/V8HOST.md).
// Everything not implemented here answers ENOSYS.
import { tmpFsImports, type TmpFs } from "./tmpfs.js";

export const ESUCCESS = 0;
export const EBADF = 8;
export const EINVAL = 28;
export const ENOSYS = 52;

export interface WasiEnv {
  memory(): WebAssembly.Memory;
  /** Evaluated on each `environ_sizes_get` / `environ_get`, so a reused instance sees the current values. */
  environ: string[] | (() => string[]);
  /** One finished stdout (fd 1) / stderr (fd 2) line. */
  line(fd: 1 | 2, text: string): void;
  /** In-memory `/tmp` (preopened as fd 3). Without it the guest sees no preopens. */
  tmp?: TmpFs;
}

export class ProcExit extends Error {
  constructor(public code: number) {
    super(`proc_exit(${code})`);
  }
}

const enc = new TextEncoder();
const dec = new TextDecoder();

/** Pending bytes without a newline are flushed as a line at this size, so unbounded output stays bounded. */
const MAX_PENDING = 32 * 1024;

/** Collects bytes written to fds 1/2 and hands out complete lines. */
export class LineBuffer {
  private chunks: Uint8Array[] = [];
  private size = 0;
  constructor(private emit: (line: string) => void) {}

  write(chunk: Uint8Array) {
    const last = chunk.lastIndexOf(10);
    if (last < 0) {
      this.chunks.push(chunk);
      this.size += chunk.length;
      if (this.size >= MAX_PENDING) this.finish();
      return;
    }
    const head = this.join(chunk.subarray(0, last));
    const tail = chunk.subarray(last + 1);
    for (const l of dec.decode(head).split("\n")) this.emit(l);
    this.chunks = tail.length > 0 ? [tail] : [];
    this.size = tail.length;
    if (this.size >= MAX_PENDING) this.finish();
  }

  private join(extra: Uint8Array): Uint8Array {
    if (this.chunks.length === 0) return extra;
    const out = new Uint8Array(this.size + extra.length);
    let o = 0;
    for (const c of this.chunks) {
      out.set(c, o);
      o += c.length;
    }
    out.set(extra, o);
    return out;
  }

  /** Emits a trailing partial line. */
  finish() {
    if (this.size > 0) {
      const text = dec.decode(this.join(new Uint8Array(0)));
      this.chunks = [];
      this.size = 0;
      this.emit(text);
    }
  }
}

export function makeWasi(env: WasiEnv, buffers: { 1: LineBuffer; 2: LineBuffer }) {
  const view = () => new DataView(env.memory().buffer);
  const bytes = () => new Uint8Array(env.memory().buffer);
  let environBytes: Uint8Array[] = [];
  let environSize = 0;
  const loadEnviron = () => {
    const list = typeof env.environ === "function" ? env.environ() : env.environ;
    environBytes = list.map((e) => enc.encode(e + "\0"));
    environSize = environBytes.reduce((n, b) => n + b.length, 0);
  };
  const tmp = env.tmp ? tmpFsImports(env.tmp, env.memory) : undefined;
  const fn: Record<string, (...a: any[]) => any> = {
    environ_sizes_get(countPtr: number, sizePtr: number) {
      countPtr >>>= 0;
      sizePtr >>>= 0;
      loadEnviron();
      const v = view();
      v.setUint32(countPtr, environBytes.length, true);
      v.setUint32(sizePtr, environSize, true);
      return ESUCCESS;
    },
    environ_get(environPtr: number, bufPtr: number) {
      environPtr >>>= 0;
      bufPtr >>>= 0;
      const v = view();
      const m = bytes();
      let p = bufPtr;
      environBytes.forEach((b, i) => {
        v.setUint32(environPtr + i * 4, p, true);
        m.set(b, p);
        p += b.length;
      });
      return ESUCCESS;
    },
    args_sizes_get(countPtr: number, sizePtr: number) {
      const v = view();
      v.setUint32(countPtr, 0, true);
      v.setUint32(sizePtr, 0, true);
      return ESUCCESS;
    },
    args_get() {
      return ESUCCESS;
    },
    clock_time_get(id: number, _precision: bigint, outPtr: number) {
      outPtr >>>= 0;
      let ns: bigint;
      if (id === 0) {
        ns = BigInt(Math.round(Date.now())) * 1_000_000n;
      } else if (id === 1) {
        const perf = (globalThis as any).performance;
        const ms: number = perf && typeof perf.now === "function" ? perf.now() : Date.now();
        ns = BigInt(Math.round(ms * 1000)) * 1000n;
      } else {
        return EINVAL;
      }
      view().setBigUint64(outPtr, ns, true);
      return ESUCCESS;
    },
    random_get(ptr: number, len: number) {
      ptr >>>= 0;
      len >>>= 0;
      const m = bytes();
      if (ptr + len > m.length) return EINVAL;
      for (let off = 0; off < len; off += 65536) {
        globalThis.crypto.getRandomValues(
          m.subarray(ptr + off, ptr + Math.min(off + 65536, len)),
        );
      }
      return ESUCCESS;
    },
    fd_write(fd: number, iovs: number, iovsLen: number, nwrittenPtr: number) {
      if (fd !== 1 && fd !== 2) return tmp ? tmp.write(fd, iovs, iovsLen, nwrittenPtr) : EBADF;
      iovs >>>= 0;
      iovsLen >>>= 0;
      const v = view();
      const m = bytes();
      if (iovs + iovsLen * 8 > m.length) return EINVAL;
      let total = 0;
      for (let i = 0; i < iovsLen; i++) {
        const base = v.getUint32(iovs + i * 8, true);
        const len = v.getUint32(iovs + i * 8 + 4, true);
        if (base + len > m.length) return EINVAL;
        buffers[fd].write(m.slice(base, base + len));
        total += len;
      }
      v.setUint32(nwrittenPtr, total, true);
      return ESUCCESS;
    },
    /**
     * Clock subscriptions only (what tokio's idle park and `thread::sleep` use). Async: the host
     * wraps it in `WebAssembly.Suspending`, so a sleep awaits `setTimeout` instead of blocking V8.
     */
    async poll_oneoff(inPtr: number, outPtr: number, nsubs: number, neventsPtr: number) {
      inPtr >>>= 0;
      outPtr >>>= 0;
      neventsPtr >>>= 0;
      nsubs >>>= 0;
      if (nsubs < 1) return EINVAL;
      const v = view();
      const perf = (globalThis as any).performance;
      const nowMono = () => BigInt(Math.round((perf && perf.now ? perf.now() : Date.now()) * 1000)) * 1000n;
      const nowReal = () => BigInt(Math.round(Date.now())) * 1_000_000n;
      let best = -1;
      let bestNs = 0n;
      for (let i = 0; i < nsubs; i++) {
        const base = inPtr + i * 48;
        if (v.getUint8(base + 8) !== 0) return EINVAL; // only clock subscriptions
        const id = v.getUint32(base + 16, true);
        const timeout = v.getBigUint64(base + 24, true);
        const abs = (v.getUint16(base + 40, true) & 1) !== 0;
        let rel = timeout;
        if (abs) {
          const now = id === 0 ? nowReal() : nowMono();
          rel = timeout > now ? timeout - now : 0n;
        }
        if (best < 0 || rel < bestNs) {
          best = i;
          bestNs = rel;
        }
      }
      const ms = Number(bestNs / 1_000_000n) + (bestNs % 1_000_000n > 0n ? 1 : 0);
      if (ms > 0) await new Promise((r) => setTimeout(r, Math.min(ms, 2 ** 31 - 1)));
      const userdata = v.getBigUint64(inPtr + best * 48, true);
      v.setBigUint64(outPtr, userdata, true);
      v.setUint16(outPtr + 8, 0, true);
      v.setUint8(outPtr + 10, 0);
      v.setUint32(neventsPtr, 1, true);
      return ESUCCESS;
    },
    proc_exit(code: number) {
      throw new ProcExit(code);
    },
  };
  if (tmp) {
    const { write: _write, ...rest } = tmp;
    Object.assign(fn, rest);
  } else {
    // No preopens: tell libc's preopen scan that there are none.
    fn.fd_prestat_get = () => EBADF;
  }
  return fn;
}

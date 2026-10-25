// JS host for the guest ABI v1 (contracts/ABI.md), running inside a normal
// Convex action (default runtime, JSPI). See contracts/V8HOST.md.
import { ConvexError, convexToJson } from "convex/values";
import { makeFunctionReference } from "convex/server";
import { decodeBase64, encodeBase64 } from "./base64.js";
import { cryptoOp } from "./crypto.js";
import { parseConvexValue, stringifyConvex } from "./fastjson.js";
import { LineBuffer, makeWasi, ENOSYS, ProcExit } from "./wasi.js";
import { intlOp } from "./intl.js";
import { TmpFs, DEFAULT_TMP_BYTES } from "./tmpfs.js";
import { HttpSink, bufferedResponse, requestHead } from "./http.js";

/** The slice of the action `ctx` the host uses. */
export interface HostCtx {
  runQuery(ref: any, args: any): Promise<any>;
  runMutation(ref: any, args: any): Promise<any>;
  runAction(ref: any, args: any): Promise<any>;
  scheduler?: {
    runAt(ts: number, ref: any, args: any): Promise<any>;
    cancel(id: any): Promise<any>;
  };
  auth?: { getUserIdentity(): Promise<any> };
  vectorSearch?(table: string, index: string, query: any): Promise<any[]>;
  storage?: {
    generateUploadUrl?(): Promise<string>;
    store(blob: Blob): Promise<string>;
    get(id: any): Promise<Blob | null>;
    getUrl(id: any): Promise<string | null>;
    delete(id: any): Promise<void>;
    getMetadata?(id: any): Promise<any>;
  };
}

export const PANIC_LOG_PREFIX = "Uncaught panic: ";
const MAX_OUTSTANDING_TASKS = 1000;
const MAX_LOG_MESSAGE_BYTES = 32 * 1024;
const MAX_RESULT_BYTES = 16 * 1024 * 1024 + 64 * 1024;
const STREAM_COALESCE_BYTES = 1024 * 1024;
const STREAM_PASSTHROUGH_BYTES = 8 * 1024;
const MAX_TASK_REQUEST_BYTES = 64 * 1024 * 1024;
/** Largest single `{ptr,len}` body (fetch / storageStore): one JS ArrayBuffer; bigger bodies stream. */
const MAX_BODY_REF_BYTES = 64 * 1024 * 1024;
/** Bytes a streamed request body buffers before `net/body_write` suspends the guest (backpressure). */
const REQUEST_BODY_HWM = 1024 * 1024;

type Json = any;

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();
const SCRATCH_MAX_BYTES = 1024 * 1024;
let scratch = new Uint8Array(4096);

function pack(ptr: number, len: number): bigint {
  return (BigInt(ptr >>> 0) << 32n) | BigInt(len >>> 0);
}
function unpack(v: bigint): [number, number] {
  const u = BigInt.asUintN(64, v);
  return [Number(u >> 32n), Number(u & 0xffffffffn)];
}

function truncateUtf8(s: string, max: number): string {
  if (s.length <= max / 4) return s;
  const b = textEncoder.encode(s);
  if (b.length <= max) return s;
  let cut = max;
  while (cut > 0 && (b[cut] & 0xc0) === 0x80) cut--;
  return textDecoder.decode(b.subarray(0, cut));
}

function errorJson(e: unknown): Json {
  // `instanceof` fails when two copies of `convex` are bundled; accept the duck type too.
  if (e instanceof ConvexError || ((e as any)?.name === "ConvexError" && "data" in (e as any))) {
    const data = (e as any).data;
    return {
      message: typeof (e as any).message === "string" ? (e as any).message : String(data),
      data: convexToJson(data === undefined ? null : data),
    };
  }
  const message =
    e && typeof (e as any).message === "string" ? (e as any).message : String(e);
  return { message };
}

function isBodyRef(b: any): b is { ptr: number; len: number } {
  return !!b && typeof b === "object" && Number.isInteger(b.ptr) && Number.isInteger(b.len);
}

function toConvexJson(v: unknown): Json {
  return convexToJson((v === undefined ? null : v) as any);
}

const TO_REFERENCE_PATH = Symbol.for("toReferencePath");

/**
 * The function argument for ctx.run* / scheduler from a syscall's args, as `getFunctionAddress`
 * in `convex/server` reads it: `{functionHandle}` (a `function://` string), `{reference}` (a
 * component function, carried under `Symbol.for("toReferencePath")`) or `{name}` (a path).
 */
export function functionRef(syscall: string, args: Json): any {
  if (typeof args?.functionHandle === "string") return args.functionHandle;
  if (typeof args?.reference === "string") return { [TO_REFERENCE_PATH]: args.reference };
  if (typeof args?.name !== "string") {
    throw new Error(`${syscall}: missing \`name\`, \`functionHandle\` or \`reference\``);
  }
  return makeFunctionReference(args.name) as any;
}

/** Rebuilds a vector filter from its wire expressions (`$eq` / `$or`) with the public builder. */
export function vectorFilter(q: any, e: Json): any {
  if (e && Array.isArray(e.$or)) return q.or(...e.$or.map((x: Json) => vectorFilter(q, x)));
  const eq = e?.$eq;
  if (Array.isArray(eq) && typeof eq[0]?.$field === "string") {
    return q.eq(eq[0].$field, parseConvexValue(eq[1]));
  }
  throw new Error(`vectorSearch: unsupported filter expression ${JSON.stringify(e)}`);
}

/** Sync syscall `1.0/componentArgument`: `{value}` (Convex JSON) or `{}` when unset. */
function componentArgument(args: Json): Json {
  if (typeof args?.name !== "string") {
    return { error: { message: "componentArgument: missing `name`" } };
  }
  const convex = (globalThis as any).Convex;
  if (typeof convex?.syscall !== "function") {
    return { error: { message: "componentArgument: not available in this runtime" } };
  }
  try {
    const out = convex.syscall("1.0/componentArgument", JSON.stringify({ name: args.name }));
    const r = typeof out === "string" ? JSON.parse(out) : out;
    return { ok: r?.value ?? null };
  } catch (e) {
    return { error: errorJson(e) };
  }
}

function webAssembly(): any {
  return (globalThis as any).WebAssembly;
}

interface ModuleInfo {
  wasiFns: string[];
}
const moduleInfo = new WeakMap<WebAssembly.Module, ModuleInfo>();

function inspectModule(module: WebAssembly.Module): ModuleInfo {
  let info = moduleInfo.get(module);
  if (info) return info;
  const wasiFns: string[] = [];
  const unsupported: string[] = [];
  for (const imp of webAssembly().Module.imports(module)) {
    if (imp.module === "wasi_snapshot_preview1" && imp.kind === "function") {
      wasiFns.push(imp.name);
    } else if (imp.module !== "convex") {
      unsupported.push(`${imp.module}.${imp.name}`);
    }
  }
  if (unsupported.length > 0) {
    throw new Error(
      `The "use rust" module has unsupported imports: ${unsupported.join(", ")}`,
    );
  }
  info = { wasiFns };
  moduleInfo.set(module, info);
  return info;
}

function parseExportName(exportName: string): { module: string; name: string } {
  // convex_run:<modulePath>:<name>, modulePath = crate::a::b
  const rest = exportName.startsWith("convex_run:")
    ? exportName.slice("convex_run:".length)
    : exportName;
  const i = rest.lastIndexOf(":");
  const modulePath = i < 0 ? rest : rest.slice(0, i);
  const name = i < 0 ? rest : rest.slice(i + 1);
  const segs = modulePath.split("::").slice(1);
  return { module: `rust/${segs.join("/")}.js`, name };
}

/** Reuse is dropped when the guest's linear memory grows past this (wasm linear memory never shrinks). */
const MAX_REUSE_MEMORY_BYTES = 128 * 1024 * 1024;
const MAX_REUSE_CALLS = 10_000;

/** A guest-fed request body: `net/body_write` enqueues, backpressure suspends the writer. */
class RequestBody {
  readonly stream: ReadableStream<Uint8Array>;
  private controller!: ReadableStreamDefaultController<Uint8Array>;
  private closed = false;
  private waiters: (() => void)[] = [];
  constructor() {
    this.stream = new ReadableStream<Uint8Array>(
      {
        start: (c) => {
          this.controller = c;
        },
        pull: () => this.release(),
        cancel: () => {
          this.closed = true;
          this.release();
        },
      },
      { highWaterMark: REQUEST_BODY_HWM, size: (c?: Uint8Array) => c?.byteLength ?? 0 },
    );
  }
  private release() {
    const w = this.waiters;
    this.waiters = [];
    for (const f of w) f();
  }
  /** Resolves once the stream can take more data. */
  write(data: Uint8Array): Promise<void> | void {
    if (this.closed) throw new Error("request body is closed (the request finished or was cancelled)");
    this.controller.enqueue(data);
    if ((this.controller.desiredSize ?? 1) > 0) return;
    return new Promise<void>((resolve) => this.waiters.push(resolve));
  }
  end() {
    if (this.closed) return;
    this.closed = true;
    this.controller.close();
    this.release();
  }
  abort(message: string) {
    if (this.closed) return;
    this.closed = true;
    this.controller.error(new Error(message));
    this.release();
  }
}

/** Per fetch task: the abort handle and (for streamed uploads) the request body. */
interface TaskCtl {
  abort: AbortController;
  body?: RequestBody;
  cancelled: boolean;
}

interface BinChunk {
  sid: string;
  parts: Uint8Array[];
  size: number;
}

/** Phase timings (ms) of one call, printed when `CONVEX_RUST_TRACE` is set. */
type Phases = Record<string, number>;
function traceEnabled(): boolean {
  try {
    const v = (globalThis as any).process?.env?.CONVEX_RUST_TRACE;
    return typeof v === "string" && v !== "" && v !== "0" && v !== "false";
  } catch {
    return false;
  }
}
const now = (): number => {
  const p = (globalThis as any).performance;
  return p && typeof p.now === "function" ? p.now() : Date.now();
};

/**
 * One wasm instance plus the per-call state around it. A Host that finished a call cleanly may serve
 * the next call of the same isolate; after a trap / panic / leftover work it is dropped.
 */
class Host {
  private instance!: WebAssembly.Instance;
  private memory!: WebAssembly.Memory;
  private nextTaskId = 0;
  private nextStreamId = 0;
  /** Queued events: JSON text, or a stream chunk kept as parts until `task_poll` frames it (binary events). */
  private events: (string | BinChunk)[] = [];
  private binary: boolean | undefined;
  /** Pending sleeps ordered by (deadline, creation); one timer serves the earliest. */
  private sleeps: { at: number; seq: number; resolve: () => void }[] = [];
  private sleepSeq = 0;
  private sleepTimer: any = null;
  private sleepTimerAt = 0;
  private waiter: (() => void) | null = null;
  private pendingTasks = 0;
  private openStreams = 0;
  /** Cancellable fetch tasks by task id (`net/cancel`, `net/body_*`). */
  private tasks = new Map<number, TaskCtl>();
  /** Cancel handles of open body streams by stream id (`net/cancel` {streamId}). */
  private streamCancels = new Map<string, () => void>();
  private panic: string | null = null;
  /** The last `Uncaught panic: ` line, kept until a guest that confirms panics (level 4) confirms it. */
  private panicCandidate: string | null = null;
  /** The guest exports `convex_panic_marker`: only a level-4 confirmation makes a panic line count. */
  private panicMarker = false;
  private done = false;
  private stdout = new LineBuffer((l) => console.log(l));
  private stderr = new LineBuffer((l) => console.error(l));
  private ctx!: HostCtx;
  /** The HTTP action this call serves (`runRustHttp`), if any. */
  private http: HttpSink | null = null;
  /** WASI environ, fixed at instantiation: libc caches it on first read, so a change retires the instance. */
  environ: string[] = [];
  envKey = "";
  private instantiated = false;
  /** A call is in flight (JSPI may have it suspended): never hand this Host to another call. */
  busy = false;
  /** False once the instance trapped / panicked / has leftover work. */
  reusable = true;
  calls = 0;
  /** Cap on the in-memory `/tmp` in bytes for the current call (0 = no `/tmp` preopen). */
  tmpBytes = DEFAULT_TMP_BYTES;
  /** Whether this instance was linked with a `/tmp` preopen. */
  tmpOn = false;
  private tmp: TmpFs | undefined;

  // ---- memory helpers ----
  private bytes(ptr: number, len: number): Uint8Array {
    return new Uint8Array(this.memory.buffer, ptr >>> 0, len >>> 0);
  }
  private readString(ptr: number, len: number): string {
    return textDecoder.decode(this.bytes(ptr, len));
  }
  /**
   * UTF-8 encodes `text` into a reused scratch buffer and copies it into guest memory: no per-event
   * `Uint8Array`. The guest allocation must be exactly the byte length (`convex_free` / `Vec` capacity), so
   * the text cannot be encoded straight into an over-sized guest block.
   */
  private writeGuestString(text: string): bigint {
    const max = text.length * 3;
    if (max > SCRATCH_MAX_BYTES) return this.writeGuest(textEncoder.encode(text));
    if (scratch.length < max) scratch = new Uint8Array(Math.max(max, 4096));
    const { written } = textEncoder.encodeInto(text, scratch);
    const alloc = this.instance.exports.convex_alloc as (n: number) => number;
    const ptr = alloc(written) >>> 0;
    this.bytes(ptr, written).set(scratch.subarray(0, written));
    return pack(ptr, written);
  }
  private writeGuest(data: Uint8Array): bigint {
    const alloc = this.instance.exports.convex_alloc as (n: number) => number;
    const ptr = alloc(data.length) >>> 0;
    this.bytes(ptr, data.length).set(data);
    return pack(ptr, data.length);
  }

  // ---- logging ----
  private flushStdio() {
    this.stdout.finish();
    this.stderr.finish();
  }
  private log(level: number, message: string) {
    if (level === 0) console.debug(message);
    else if (level === 2) console.warn(message);
    else if (level === 3) console.error(message);
    else console.log(message);
  }

  // ---- event queue ----
  private push(ev: Json) {
    this.events.push(JSON.stringify(ev));
    this.wake();
  }
  private wake() {
    const w = this.waiter;
    this.waiter = null;
    if (w) w();
  }
  /** Queues a stream data chunk: a binary event for guests that export `convex_binary_events`, else JSON+base64. */
  private pushChunk(sid: string, parts: Uint8Array[], size: number) {
    if (this.binary === undefined) {
      this.binary = typeof (this.instance.exports as any).convex_binary_events === "function";
    }
    if (!this.binary || sid.length * 3 > 255) {
      let data = parts[0];
      if (parts.length !== 1) {
        data = new Uint8Array(size);
        let o = 0;
        for (const p of parts) {
          data.set(p, o);
          o += p.length;
        }
      }
      this.push({ type: "chunk", streamId: sid, data: encodeBase64(data) });
      return;
    }
    this.events.push({ sid, parts, size });
    this.wake();
  }

  // ---- sleeps: one timer for the earliest deadline, every due sleep delivered in one wake ----
  private sleep(ms: number, signal?: AbortSignal): Promise<null> {
    return new Promise<null>((resolve) => {
      const at = now() + ms;
      const seq = this.sleepSeq++;
      const list = this.sleeps;
      let i = list.length;
      while (i > 0 && list[i - 1].at > at) i--;
      const entry = { at, seq, resolve: () => resolve(null) };
      list.splice(i, 0, entry);
      if (this.sleepTimer === null || at < this.sleepTimerAt) this.armSleep(list[0].at);
      // `net/cancel` on a sleep task: drop its deadline now so it neither keeps the call
      // outstanding nor leaves a timer behind (the guest abandoned it).
      signal?.addEventListener(
        "abort",
        () => {
          const j = this.sleeps.indexOf(entry);
          if (j >= 0) this.sleeps.splice(j, 1);
          if (this.sleeps.length === 0 && this.sleepTimer !== null) {
            clearTimeout(this.sleepTimer);
            this.sleepTimer = null;
          }
          resolve(null);
        },
        { once: true },
      );
    });
  }
  private armSleep(at: number) {
    if (this.sleepTimer !== null) clearTimeout(this.sleepTimer);
    this.sleepTimerAt = at;
    this.sleepTimer = setTimeout(() => this.fireSleeps(), Math.max(0, Math.ceil(at - now())));
  }
  private fireSleeps() {
    this.sleepTimer = null;
    const list = this.sleeps;
    const t = now();
    // The timer fired for the earliest sleep; also take everything else already due.
    let n = 1;
    while (n < list.length && list[n].at <= t) n++;
    for (const s of list.splice(0, n)) s.resolve();
    if (list.length > 0) this.armSleep(list[0].at);
  }
  private outstanding(): boolean {
    return this.pendingTasks > 0 || this.openStreams > 0;
  }

  private async poll(block: number): Promise<bigint> {
    for (;;) {
      const ev = this.events.shift();
      if (ev !== undefined) {
        if (typeof ev === "string") return this.writeGuestString(ev);
        // Binary chunk frame (contracts/ABI.md): 0x01 | u8 id_len | id | bytes, straight into guest memory.
        const id = textEncoder.encode(ev.sid);
        const n = 2 + id.length + ev.size;
        const alloc = this.instance.exports.convex_alloc as (n: number) => number;
        const ptr = alloc(n) >>> 0;
        const mem = this.bytes(ptr, n);
        mem[0] = 1;
        mem[1] = id.length;
        mem.set(id, 2);
        let o = 2 + id.length;
        for (const p of ev.parts) {
          mem.set(p, o);
          o += p.length;
        }
        return pack(ptr, n);
      }
      if (!block) return 0n;
      if (!this.outstanding()) return -1n;
      await new Promise<void>((resolve) => {
        this.waiter = resolve;
      });
    }
  }

  // ---- task_submit ----
  private submit(ptr: number, len: number): bigint {
    if (len >>> 0 > MAX_TASK_REQUEST_BYTES) {
      this.log(3, `task_submit: request of ${len >>> 0} bytes exceeds ${MAX_TASK_REQUEST_BYTES}`);
      return -1n;
    }
    if (this.pendingTasks >= MAX_OUTSTANDING_TASKS) {
      this.log(3, `task_submit: more than ${MAX_OUTSTANDING_TASKS} outstanding tasks`);
      return -1n;
    }
    let req: Json;
    try {
      req = JSON.parse(this.readString(ptr, len));
    } catch (e: any) {
      this.log(3, `task_submit: malformed request: ${e?.message ?? e}`);
      return -1n;
    }
    // A rejected request still consumes an id (as in the backend host).
    const id = this.nextTaskId++;
    let start: () => Promise<Json>;
    try {
      start = this.parseRequest(req, id);
    } catch (e: any) {
      this.log(3, `task_submit: ${e?.message ?? e}`);
      return -1n;
    }
    this.pendingTasks++;
    // Never let a task failure escape as an unhandled rejection.
    void (async () => {
      let ev: Json;
      try {
        ev = { type: "done", taskId: id, ok: await start() };
      } catch (e) {
        ev = { type: "done", taskId: id, error: errorJson(e) };
      }
      this.pendingTasks--;
      const ctl = this.tasks.get(id);
      this.tasks.delete(id);
      // A cancelled task's guest future is gone: skip the event.
      if (!this.done && !ctl?.cancelled) this.push(ev);
      else this.wake(); // a blocked poll must notice there is nothing left to wait for
    })();
    return BigInt(id);
  }

  /** Validates the request now (so a bad one returns -1) and returns the work. */
  private parseRequest(req: Json, taskId: number): () => Promise<Json> {
    const ty = req?.type;
    if (typeof ty !== "string") throw new Error("missing request type");
    const str = (k: string): string => {
      if (typeof req[k] !== "string") throw new Error(`${ty}: missing string field \`${k}\``);
      return req[k];
    };
    const body = (): Uint8Array | null => {
      const b = req.body;
      if (b === undefined || b === null) return null;
      if (isBodyRef(b)) {
        // The guest may reuse its buffer once `task_submit` returns: copy now.
        return this.refBytes(b, ty).slice();
      }
      if (typeof b !== "string") {
        throw new Error(`${ty}: \`body\` must be a base64 string, a {ptr,len} reference or null`);
      }
      try {
        return decodeBase64(b);
      } catch (e: any) {
        throw new Error(`${ty}: invalid base64 body: ${e?.message ?? e}`);
      }
    };
    switch (ty) {
      case "syscall": {
        const name = str("name");
        const args = req.args ?? {};
        return () => this.syscall(name, args);
      }
      case "fetch": {
        const url = str("url");
        try {
          new URL(url);
        } catch (e: any) {
          throw new Error(`fetch: ${e?.message ?? e}`);
        }
        const method = typeof req.method === "string" ? req.method : "GET";
        const headers = new Headers();
        if (Array.isArray(req.headers)) {
          for (const pair of req.headers) {
            if (!Array.isArray(pair) || typeof pair[0] !== "string" || typeof pair[1] !== "string") {
              throw new Error("fetch: headers must be [name, value] pairs");
            }
            headers.append(pair[0], pair[1]);
          }
        }
        const ctl: TaskCtl = { abort: new AbortController(), cancelled: false };
        let b: Uint8Array | null = null;
        if (req.body && typeof req.body === "object" && req.body.stream === true) {
          ctl.body = new RequestBody();
        } else {
          b = body();
        }
        this.tasks.set(taskId, ctl);
        return () => this.fetch(url, method, headers, b, ctl);
      }
      case "httpStart": {
        const status = req.status;
        const headers = req.headers;
        return async () => {
          this.requireHttp().start(status, headers);
          return null;
        };
      }
      case "httpWrite": {
        const b = body() ?? new Uint8Array(0);
        return async () => {
          await this.requireHttp().write(b);
          return null;
        };
      }
      case "httpEnd":
        return async () => {
          this.requireHttp().end();
          return null;
        };
      case "sleep": {
        const ms = req.ms;
        if (typeof ms !== "number" || !Number.isInteger(ms) || ms < 0) {
          throw new Error("sleep: missing integer field `ms`");
        }
        const ctl: TaskCtl = { abort: new AbortController(), cancelled: false };
        this.tasks.set(taskId, ctl);
        return () => this.sleep(ms, ctl.abort.signal);
      }
      case "storageStore": {
        const contentType = typeof req.contentType === "string" ? req.contentType : undefined;
        const opts = contentType ? { type: contentType } : {};
        // A {ptr,len} body goes from guest memory straight into the Blob (the constructor snapshots
        // it synchronously): one copy, no intermediate buffer, no base64.
        const blob = isBodyRef(req.body)
          ? new Blob([this.refBytes(req.body, ty) as BlobPart], opts)
          : new Blob([(body() ?? new Uint8Array(0)) as BlobPart], opts);
        return async () => await this.requireStorage().store(blob);
      }
      case "storageGet": {
        const id = str("storageId");
        return async () => {
          const blob = await this.requireStorage().get(id);
          if (blob === null || blob === undefined) return null;
          const bodyStreamId = this.openStream(blob.stream ? blob.stream() : blob);
          return {
            bodyStreamId,
            contentLength: blob.size,
            contentType: blob.type ? blob.type : null,
          };
        };
      }
      default:
        throw new Error(`unknown request type ${JSON.stringify(ty)}`);
    }
  }

  private requireHttp(): HttpSink {
    if (!this.http) throw new Error("httpStart/httpWrite/httpEnd are only available in an HTTP action");
    return this.http;
  }

  private requireStorage() {
    if (!this.ctx.storage) throw new Error("ctx.storage is not available in this context");
    return this.ctx.storage;
  }

  // ---- async syscalls ----
  private async syscall(name: string, args: Json): Promise<Json> {
    const ctx = this.ctx;
    const fnRef = () => functionRef(name, args);
    const fnArgs = () => parseConvexValue(args?.args ?? {});
    switch (name) {
      case "1.0/actions/query":
        return toConvexJson(await ctx.runQuery(fnRef(), fnArgs()));
      case "1.0/actions/mutation":
        return toConvexJson(await ctx.runMutation(fnRef(), fnArgs()));
      case "1.0/actions/action":
        return toConvexJson(await ctx.runAction(fnRef(), fnArgs()));
      case "1.0/actions/schedule": {
        if (!ctx.scheduler) throw new Error("ctx.scheduler is not available in this context");
        const ts = args?.ts;
        if (typeof ts !== "number") throw new Error(`${name}: missing numeric \`ts\``);
        // The wire `ts` is in seconds; ctx.scheduler.runAt takes milliseconds.
        return toConvexJson(await ctx.scheduler.runAt(ts * 1000, fnRef(), fnArgs()));
      }
      case "1.0/actions/cancel_job": {
        if (!ctx.scheduler) throw new Error("ctx.scheduler is not available in this context");
        await ctx.scheduler.cancel(args?.id);
        return null;
      }
      case "1.0/getUserIdentity": {
        if (!ctx.auth) throw new Error("ctx.auth is not available in this context");
        return toConvexJson(await ctx.auth.getUserIdentity());
      }
      case "1.0/storageGetUrl":
        return toConvexJson(await this.requireStorage().getUrl(args?.storageId));
      case "1.0/storageGetMetadata": {
        const s = this.requireStorage();
        if (!s.getMetadata) throw new Error("ctx.storage.getMetadata is not available");
        return toConvexJson(await s.getMetadata(args?.storageId));
      }
      case "1.0/storageDelete":
        await this.requireStorage().delete(args?.storageId);
        return null;
      case "1.0/storageGenerateUploadUrl": {
        const s = this.requireStorage();
        if (!s.generateUploadUrl) throw new Error("ctx.storage.generateUploadUrl is not available");
        return toConvexJson(await s.generateUploadUrl());
      }
      case "1.0/actions/vectorSearch": {
        if (!ctx.vectorSearch) throw new Error("ctx.vectorSearch is not available in this context");
        const q = args?.query;
        const dot = typeof q?.indexName === "string" ? q.indexName.indexOf(".") : -1;
        if (dot < 0) throw new Error(`${name}: \`query.indexName\` must be "<table>.<index>"`);
        const query: any = { vector: q.vector };
        if (q.limit !== undefined && q.limit !== null) query.limit = q.limit;
        if (q.expressions) query.filter = (f: any) => vectorFilter(f, q.expressions);
        const results = await ctx.vectorSearch(
          q.indexName.slice(0, dot),
          q.indexName.slice(dot + 1),
          query,
        );
        return { results: toConvexJson(results) };
      }
      default: {
        // Any other name goes to the runtime's own async syscall, as the native host does.
        const convex = (globalThis as any).Convex;
        if (typeof convex?.asyncSyscall !== "function") {
          throw new Error(`Unsupported syscall in the V8 host: ${name}`);
        }
        const out = await convex.asyncSyscall(name, JSON.stringify(args ?? {}));
        return typeof out === "string" ? JSON.parse(out) : (out ?? null);
      }
    }
  }

  // ---- fetch and body streams ----
  private async fetch(
    url: string,
    method: string,
    headers: Headers,
    body: Uint8Array | null,
    ctl: TaskCtl,
  ): Promise<Json> {
    const init: any = { method, headers, redirect: "manual", signal: ctl.abort.signal };
    if (ctl.body) {
      init.body = ctl.body.stream;
      init.duplex = "half";
    } else if (body !== null && method !== "GET" && method !== "HEAD") init.body = body;
    let res: Response;
    try {
      res = await fetch(url, init);
    } catch (e) {
      // Release a guest blocked in `net/body_write` backpressure.
      ctl.body?.abort("request failed");
      throw e;
    }
    const pairs: [string, string][] = [];
    res.headers.forEach((value, key) => pairs.push([key, value]));
    const bodyStreamId = res.body ? this.openStream(res.body, () => ctl.abort.abort()) : null;
    return {
      status: res.status,
      statusText: res.statusText ?? "",
      headers: pairs,
      url: res.url || url,
      bodyStreamId,
    };
  }

  /** Registers a body stream; chunk events follow the `done` of its task. */
  private openStream(stream: ReadableStream<Uint8Array> | Blob, cancel?: () => void): string {
    const streamId = `s${this.nextStreamId++}`;
    this.openStreams++;
    let cancelled = false;
    const reader0 =
      typeof (stream as any).getReader === "function"
        ? (stream as ReadableStream<Uint8Array>).getReader()
        : null;
    this.streamCancels.set(streamId, () => {
      cancelled = true;
      if (cancel) cancel();
      else if (reader0) void reader0.cancel().catch(() => {});
    });
    void (async () => {
      let end: Json = { type: "chunk", streamId, end: true };
      try {
        if (reader0) {
          const reader = reader0;
          // Coalesce small chunks (< 8 KiB) that arrive back to back into one guest event of up to
          // ~1 MiB (one base64 pass and one wake-up instead of many); larger chunks pass through as is.
          let parts: Uint8Array[] = [];
          let size = 0;
          const flush = () => {
            if (size === 0) return;
            const ps = parts;
            const sz = size;
            parts = [];
            size = 0;
            if (!this.done && !cancelled) this.pushChunk(streamId, ps, sz);
          };
          const TICK = Symbol("tick");
          let pending: Promise<ReadableStreamReadResult<Uint8Array>> | null = null;
          for (;;) {
            pending ??= reader.read();
            let r: ReadableStreamReadResult<Uint8Array> | typeof TICK;
            if (size === 0) r = await pending;
            else {
              let timer: any;
              r = await Promise.race([
                pending,
                new Promise<typeof TICK>((res) => {
                  timer = setTimeout(() => res(TICK), 0);
                }),
              ]);
              clearTimeout(timer);
            }
            if (r === TICK) {
              flush();
              continue;
            }
            pending = null;
            if (r.done) break;
            if (r.value && r.value.length >= STREAM_PASSTHROUGH_BYTES) {
              // Already a decent size: keep the network's chunking, just keep the order.
              flush();
              parts.push(r.value);
              size += r.value.length;
              flush();
            } else if (r.value && r.value.length > 0) {
              parts.push(r.value);
              size += r.value.length;
              if (size >= STREAM_COALESCE_BYTES) flush();
            }
          }
          flush();
        } else {
          const buf = new Uint8Array(await (stream as Blob).arrayBuffer());
          if (buf.length > 0) this.pushChunk(streamId, [buf], buf.length);
        }
      } catch (e) {
        end = { type: "chunk", streamId, error: errorJson(e).message };
      }
      this.openStreams--;
      this.streamCancels.delete(streamId);
      if (!this.done && !cancelled) this.push(end);
      else this.wake();
    })();
    return streamId;
  }

  // ---- net/* sync ops: cancel, guest-fed request bodies (contracts/ABI.md) ----
  private refBytes(ref: { ptr: number; len: number }, what: string): Uint8Array {
    const ptr = ref.ptr >>> 0;
    const len = ref.len >>> 0;
    if (len > MAX_BODY_REF_BYTES) {
      throw new Error(`${what}: body of ${len} bytes exceeds ${MAX_BODY_REF_BYTES}; stream it instead`);
    }
    if (ptr + len > this.memory.buffer.byteLength) throw new Error(`${what}: body reference out of bounds`);
    return this.bytes(ptr, len);
  }

  private netOp(op: string, args: Json): bigint | Promise<bigint> {
    const reply = (x: Json) => this.writeGuest(textEncoder.encode(JSON.stringify(x)));
    const fail = (e: unknown) => reply({ error: { message: `${op}: ${errorJson(e).message}` } });
    try {
      const taskCtl = (): TaskCtl => {
        const ctl = this.tasks.get(args?.taskId);
        if (!ctl?.body) throw new Error(`no streamed request body for task ${args?.taskId}`);
        return ctl;
      };
      switch (op) {
        case "net/caps":
          return reply({
            ok: { version: 1, ops: ["cancel", "bodyRef", "requestStream"], maxBodyRef: MAX_BODY_REF_BYTES },
          });
        case "net/cancel": {
          let found = false;
          if (typeof args?.taskId === "number") {
            const ctl = this.tasks.get(args.taskId);
            if (ctl) {
              found = true;
              ctl.cancelled = true;
              ctl.body?.abort("cancelled");
              ctl.abort.abort();
            }
          } else if (typeof args?.streamId === "string") {
            const c = this.streamCancels.get(args.streamId);
            if (c) {
              found = true;
              c();
            }
          }
          return reply({ ok: found });
        }
        case "net/body_write": {
          const ctl = taskCtl();
          // Copy: the guest reuses the buffer as soon as the call returns.
          const data = this.refBytes(args.data, op).slice();
          const w = ctl.body!.write(data);
          return w ? w.then(() => reply({ ok: true }), fail) : reply({ ok: true });
        }
        case "net/body_end":
          taskCtl().body!.end();
          return reply({ ok: true });
        case "net/body_abort":
          taskCtl().body!.abort(typeof args?.message === "string" ? args.message : "aborted");
          return reply({ ok: true });
        default:
          return reply({ error: { message: `unknown sync op ${op}` } });
      }
    } catch (e) {
      return fail(e);
    }
  }

  // ---- sync syscalls ----
  private syncSyscall(
    opPtr: number,
    opLen: number,
    argsPtr: number,
    argsLen: number,
  ): bigint | Promise<bigint> {
    let reply: Json;
    if ((opLen >>> 0) > 4096 || (argsLen >>> 0) > MAX_TASK_REQUEST_BYTES) {
      reply = { error: { message: "sync_syscall: request too large" } };
    } else {
      const op = this.readString(opPtr, opLen);
      let args: Json;
      let bad: string | null = null;
      try {
        args = JSON.parse(this.readString(argsPtr, argsLen));
      } catch (e: any) {
        bad = `sync op ${op}: invalid args: ${e?.message ?? e}`;
      }
      if (bad !== null) reply = { error: { message: bad } };
      else if (op === "now") reply = { ok: Date.now() };
      else if (op.startsWith("intl/")) reply = intlOp(op, args);
      else if (op.startsWith("net/")) return this.netOp(op, args);
      else if (op.startsWith("crypto/")) {
        const r = cryptoOp(op, args, () => this.memory);
        const write = (x: Json) => this.writeGuest(textEncoder.encode(JSON.stringify(x)));
        return r instanceof Promise ? r.then(write) : write(r);
      } else if (op === "componentArgument") {
        reply = componentArgument(args);
      } else reply = { error: { message: `unknown sync op ${op}` } };
    }
    return this.writeGuest(textEncoder.encode(JSON.stringify(reply)));
  }

  // ---- imports ----
  private imports(module: WebAssembly.Module): WebAssembly.Imports {
    const WA = webAssembly();
    if (typeof WA.Suspending !== "function" || typeof WA.promising !== "function") {
      throw new Error(
        'WebAssembly JSPI is not available in this runtime. "use rust" actions need the default Convex runtime (not "use node").',
      );
    }
    const info = inspectModule(module);
    const tmpBytes = this.tmpBytes;
    this.tmpOn = tmpBytes > 0;
    const exportNames = new Set(WA.Module.exports(module).map((x: { name: string }) => x.name));
    // File data goes into the guest's own linear memory when it exports its allocator (see tmpfs.ts).
    const ex = () => this.instance.exports as Record<string, any>;
    this.tmp =
      tmpBytes > 0
        ? new TmpFs({
            maxBytes: tmpBytes,
            memory: () => this.memory,
            alloc: exportNames.has("convex_alloc") ? (n) => ex().convex_alloc(n) : undefined,
            free: exportNames.has("convex_free") ? (p, n) => ex().convex_free(p, n) : undefined,
          })
        : undefined;
    const wasi = makeWasi(
      {
        memory: () => this.memory,
        environ: () => this.environ,
        line: () => {},
        tmp: this.tmp,
      },
      { 1: this.stdout, 2: this.stderr },
    );
    const wasiImports: Record<string, Function> = {};
    for (const name of info.wasiFns) {
      wasiImports[name] =
        name === "poll_oneoff" && wasi[name]
          ? new WA.Suspending(wasi[name])
          : (wasi[name] ?? (() => ENOSYS));
    }
    const convex = {
      task_submit: (ptr: number, len: number) => this.submit(ptr, len),
      task_poll: new WA.Suspending((block: number) => this.poll(block)),
      sync_syscall: new WA.Suspending((a: number, b: number, c: number, d: number) =>
        this.syncSyscall(a, b, c, d),
      ),
      log: (level: number, ptr: number, len: number) => {
        const max = MAX_LOG_MESSAGE_BYTES;
        let message = this.readString(ptr, Math.min(len >>> 0, max));
        message = truncateUtf8(message, max);
        // Keep stdout / stderr lines written so far ahead of this one.
        this.stdout.finish();
        this.stderr.finish();
        if (level === 4 && this.panicMarker) {
          // The guest's panic hook confirms the line it just logged; user log lines cannot send level 4.
          this.panic = this.panicCandidate;
          return;
        }
        if (level === 3 && message.startsWith(PANIC_LOG_PREFIX)) {
          const text = message.slice(PANIC_LOG_PREFIX.length);
          if (this.panicMarker) this.panicCandidate = text;
          else this.panic = text; // older guests: the prefixed line alone is the signal
        }
        this.log(level, message);
      },
    };
    return { convex, wasi_snapshot_preview1: wasiImports } as any;
  }

  /** Instantiates the module and runs `_initialize` (once per Host). */
  private instantiate(module: WebAssembly.Module, ph: Phases) {
    const WA = webAssembly();
    let t = now();
    const imports = this.imports(module);
    ph.imports = now() - t;
    t = now();
    this.instance = new WA.Instance(module, imports);
    ph.instantiate = now() - t;
    const exports = this.instance.exports as Record<string, any>;
    this.memory = exports.memory;
    this.panicMarker = typeof (exports as any).convex_panic_marker === "function";
    t = now();
    const init = exports._initialize;
    if (typeof init === "function") init();
    ph.init = now() - t;
    this.instantiated = true;
  }

  async run(
    ctx: HostCtx,
    environ: string[],
    module: WebAssembly.Module,
    exportName: string,
    args: unknown,
    ph: Phases,
    http: HttpSink | null = null,
  ): Promise<unknown> {
    const WA = webAssembly();
    this.ctx = ctx;
    this.http = http;
    if (!this.instantiated) {
      this.environ = environ;
      this.envKey = environ.join("\0");
    }
    if (this.tmp) this.tmp.maxBytes = Math.max(1, this.tmpBytes);
    this.nextTaskId = 0;
    this.nextStreamId = 0;
    this.events = [];
    this.waiter = null;
    this.sleeps = [];
    this.sleepTimer = null;
    this.tasks.clear();
    this.streamCancels.clear();
    this.panic = null;
    this.panicCandidate = null;
    this.done = false;
    this.calls++;
    // Anything but a clean finish (trap, panic, error envelope, leftover work) retires this instance.
    let clean = false;
    try {
      let t = now();
      // An HTTP action's args are the request head; its body is a host stream the guest reads.
      const argsJson = stringifyConvex(
        http
          ? { ...http.head, bodyStreamId: http.requestBody ? this.openStream(http.requestBody) : null }
          : args,
      );
      ph.encodeArgs = now() - t;
      if (!this.instantiated) this.instantiate(module, ph);
      const exports = this.instance.exports as Record<string, any>;
      const run = exports[exportName];
      if (typeof run !== "function") {
        const { module: m, name } = parseExportName(exportName);
        throw new Error(`Could not find function ${name} in module ${m}`);
      }
      if (typeof exports.convex_alloc !== "function" || !this.memory) {
        throw new Error(
          'The "use rust" module broke the guest ABI: module has no `convex_alloc` or `memory` export',
        );
      }
      t = now();
      const argBytes = textEncoder.encode(argsJson);
      const argPtr = (exports.convex_alloc as (n: number) => number)(argBytes.length) >>> 0;
      this.bytes(argPtr, argBytes.length).set(argBytes);
      let packed: bigint;
      ph.prepare = now() - t;
      t = now();
      try {
        packed = await WA.promising(run)(argPtr, argBytes.length);
      } catch (e: any) {
        this.flushStdio();
        if (e instanceof ProcExit) {
          throw new Error(`WebAssembly trap: ${e.message}`);
        }
        if (e instanceof WA.RuntimeError) {
          if (this.panic !== null) throw new Error(this.panic);
          throw new Error(`WebAssembly trap: ${e.message}`);
        }
        throw e;
      }
      this.flushStdio();
      ph.call = now() - t;
      t = now();
      // Reuse only when the guest finished with nothing left running and its memory stayed modest.
      clean =
        !this.outstanding() &&
        this.panic === null &&
        this.calls < MAX_REUSE_CALLS &&
        this.memory.buffer.byteLength <= MAX_REUSE_MEMORY_BYTES;
      const [rptr, rlen] = unpack(packed);
      if (rlen > MAX_RESULT_BYTES) {
        clean = false; // the oversized buffer is never read or freed: do not reuse this instance
        throw new Error(
          `Function result is too large (${rlen} bytes, the limit is ${MAX_RESULT_BYTES} bytes)`,
        );
      }
      let envelope: Json;
      try {
        envelope = JSON.parse(this.readString(rptr, rlen));
      } catch (e: any) {
        clean = false;
        throw new Error(`The "use rust" module broke the guest ABI: result envelope: ${e?.message ?? e}`);
      } finally {
        // Optional export (contracts/V8HOST.md): hand the result buffer back to the guest allocator.
        const free = exports.convex_free;
        if (typeof free === "function" && rlen > 0) free(rptr, rlen);
      }
      if (envelope && "ok" in envelope) {
        // An HTTP action's value is the plain response description, not a Convex value.
        const v = http ? envelope.ok : parseConvexValue(envelope.ok);
        ph.decode = now() - t;
        return v;
      }
      if (envelope && envelope.error) {
        const err = envelope.error;
        const message = typeof err.message === "string" ? err.message : "unknown error";
        if (err.kind === "convex" && "data" in err) {
          throw new ConvexError(parseConvexValue(err.data));
        }
        throw new Error(message);
      }
      throw new Error(
        'The "use rust" module broke the guest ABI: result envelope has neither `ok` nor `error`',
      );
    } finally {
      this.done = true;
      this.flushStdio();
      if (!clean) this.reusable = false;
      // `/tmp` lives for one call: hand its guest-memory segments back (a retired instance is just dropped).
      try {
        this.tmp?.reset(clean);
      } catch {
        this.reusable = false;
      }
    }
  }
}

function tmpBytesFromEnv(): number {
  try {
    const v = (globalThis as any).process?.env?.CONVEX_RUST_TMP_BYTES;
    if (v !== undefined && v !== "" && Number.isFinite(Number(v)) && Number(v) >= 0) return Math.floor(Number(v));
  } catch {
    // process.env may be unavailable
  }
  return DEFAULT_TMP_BYTES;
}

/** Reusable hosts per compiled module (one per isolate, since module scope is per isolate). */
const hosts = new WeakMap<object, Host>();
/** Modules compiled from bytes by {@link runRust}, so the compile happens once per isolate. */
const compiled = new WeakMap<object, WebAssembly.Module>();

function toModule(m: WebAssembly.Module | BufferSource): WebAssembly.Module {
  const WA = webAssembly();
  if (m instanceof WA.Module) return m;
  const hit = compiled.get(m as object);
  if (hit) return hit;
  const c: WebAssembly.Module = new WA.Module(m as BufferSource);
  compiled.set(m as object, c);
  return c;
}

/**
 * Runs one `convex_run:*` export of a `"use rust"` module (guest ABI v1). `module` is the
 * `WebAssembly.Module` the bundler produced for `import wasm from "./x.wasm"` (compiled once per
 * isolate); raw wasm bytes are also accepted and compiled once.
 *
 * The instance is reused by later calls in the same isolate after a call that completed cleanly
 * (like JS module globals, Rust statics persist). After a trap / panic / OOM, or if another call is
 * still using it, a fresh instance is used. Set the deployment variable `CONVEX_RUST_TRACE=1` to log
 * per-call phase timings.
 */
export async function runRust(
  ctx: HostCtx,
  module: WebAssembly.Module | BufferSource,
  exportName: string,
  args: unknown,
  options?: RunRustOptions,
): Promise<any> {
  return execute(ctx, module, exportName, args, options, null);
}

async function execute(
  ctx: HostCtx,
  module: WebAssembly.Module | BufferSource,
  exportName: string,
  args: unknown,
  options: RunRustOptions | undefined,
  http: HttpSink | null,
): Promise<any> {
  const trace = traceEnabled();
  const t0 = now();
  const ph: Phases = {};
  const mod = toModule(module);
  let host = hosts.get(mod);
  const environ = collectEnviron(options?.env ?? []);
  const tmpCap = options?.tmpBytes ?? tmpBytesFromEnv();
  // Whether the guest sees a `/tmp` preopen is fixed at instantiation; its cap is applied per call.
  const reused =
    host !== undefined &&
    host.reusable &&
    !host.busy &&
    host.envKey === environ.join("\0") &&
    host.tmpOn === (tmpCap > 0);
  if (!reused) {
    const fresh = new Host();
    // Do not displace a busy cached host (its call is suspended); the fresh one is single-use.
    if (host === undefined || !host.busy) hosts.set(mod, fresh);
    host = fresh;
  }
  const h = host!;
  h.tmpBytes = tmpCap;
  h.busy = true;
  let ok = false;
  try {
    const r = await h.run(ctx, environ, mod, exportName, args, ph, http);
    ok = true;
    return r;
  } finally {
    h.busy = false;
    if (!h.reusable && hosts.get(mod) === h) hosts.delete(mod);
    if (trace) {
      ph.total = now() - t0;
      const f = Object.entries(ph).map(([k, v]) => `${k}=${v.toFixed(2)}`).join(" ");
      console.log(`[convex-rust] ${exportName} reused=${reused} ok=${ok} ${f}`);
    }
  }
}

/**
 * Runs a `#[http_action]` export of a `"use rust"` module as the handler of a Convex `httpAction`
 * (contracts/V8HOST.md "HTTP actions"). The request head is passed as the call args and its body
 * as a host stream the guest reads incrementally; the guest answers with a buffered response or
 * streams it (`httpStart` / `httpWrite` / `httpEnd` tasks), in which case the returned Response
 * is available as soon as the guest started it and the body follows while the guest runs.
 *
 * Both bodies are limited to 20 MiB, like any Convex HTTP action; a buffered body also has to fit
 * the 16 MiB result limit (stream bigger ones).
 */
export async function runRustHttp(
  ctx: HostCtx,
  module: WebAssembly.Module | BufferSource,
  exportName: string,
  request: Request,
  options?: RunRustOptions,
): Promise<Response> {
  const sink = new HttpSink(requestHead(request), request.body ?? null);
  const call = execute(ctx, module, exportName, null, options, sink).then(
    (ok) => ({ ok }) as const,
    (error) => ({ error }) as const,
  );
  const first = await Promise.race([call, sink.started.then(() => "started" as const)]);
  if (first === "started") {
    // The guest keeps running while the client reads; its end (or failure) closes (or errors) the body.
    void call.then((r) => ("error" in r ? sink.fail(r.error) : sink.end()));
    return sink.response!;
  }
  if ("error" in first) throw first.error;
  // A guest that started a stream returns `{streamed: true}` only after `httpEnd`.
  if (sink.response) return sink.response;
  return bufferedResponse(first.ok);
}

export interface RunRustOptions {
  /**
   * Names of environment variables to expose to the guest (WASI environ). The default runtime's
   * `process.env` can only be read by name, so it cannot be enumerated; `convex-rust build --env A,B`
   * emits this list, and the deployment variable `CONVEX_RUST_ENV=C,D` adds more at call time.
   */
  env?: string[];
  /**
   * Cap, in bytes, on the in-memory `/tmp` the guest sees (per call, discarded after it). Default 64 MiB,
   * or the deployment variable `CONVEX_RUST_TMP_BYTES`; `0` removes `/tmp`. Writes beyond it fail with ENOSPC.
   */
  tmpBytes?: number;
}

/**
 * Environment for the guest: only the named variables (`names`, the build-time `--env` list) plus those
 * listed in the deployment variable `CONVEX_RUST_ENV`. The rest of `process.env` is never enumerated.
 */
export function collectEnviron(names: string[]): string[] {
  const env = ((globalThis as any).process?.env ?? {}) as Record<string, string | undefined>;
  const out = new Map<string, string>();
  let extra: string[] = [];
  try {
    extra = String(env.CONVEX_RUST_ENV ?? "").split(",");
  } catch {
    // ignore
  }
  for (const raw of [...names, ...extra]) {
    const name = raw.trim();
    if (name === "" || name.includes("=")) continue;
    try {
      const v = env[name];
      if (typeof v === "string") out.set(name, v);
    } catch {
      // ignore
    }
  }
  return [...out].map(([k, v]) => `${k}=${v}`);
}

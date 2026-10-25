// HTTP actions written in Rust (contracts/V8HOST.md "HTTP actions"): the Request -> guest head
// conversion, and the sink that turns a guest's `httpStart` / `httpWrite` / `httpEnd` tasks into a
// streamed Response. The wasm instance itself is driven by host.ts.
import { decodeBase64 } from "./base64.js";

/** Convex limits an HTTP action's request and response body to 20 MiB. */
export const MAX_HTTP_BODY_BYTES = 20 * 1024 * 1024;
/** Bytes buffered for a streamed response before `httpWrite` waits for the client to catch up. */
const STREAM_HIGH_WATER_BYTES = 1024 * 1024;
/** Statuses a `Response` cannot carry a body with. */
const NULL_BODY_STATUS = new Set([101, 204, 205, 304]);

export interface HttpHead {
  method: string;
  url: string;
  headers: [string, string][];
}

/** The head the guest receives as its call args (the body follows as a host stream). */
export function requestHead(request: Request): HttpHead {
  const headers: [string, string][] = [];
  request.headers.forEach((value, key) => headers.push([key, value]));
  return { method: request.method, url: request.url, headers };
}

function toHeaders(pairs: unknown, what: string): Headers {
  const headers = new Headers();
  if (pairs === undefined || pairs === null) return headers;
  if (!Array.isArray(pairs)) throw new Error(`${what}: headers must be a list of [name, value] pairs`);
  for (const pair of pairs) {
    if (!Array.isArray(pair) || typeof pair[0] !== "string" || typeof pair[1] !== "string") {
      throw new Error(`${what}: headers must be [name, value] pairs`);
    }
    headers.append(pair[0], pair[1]);
  }
  return headers;
}

function checkStatus(status: unknown, what: string): number {
  if (typeof status !== "number" || !Number.isInteger(status) || status < 200 || status > 599) {
    throw new Error(`${what}: invalid HTTP status ${String(status)} (expected 200..599)`);
  }
  return status;
}

/** The Response for a buffered guest answer `{status, headers, body}` (`body` base64 or null). */
export function bufferedResponse(ok: any): Response {
  if (ok === null || typeof ok !== "object") {
    throw new Error('The "use rust" HTTP action returned something other than an HttpResponse');
  }
  const status = checkStatus(ok.status, "HTTP response");
  const headers = toHeaders(ok.headers, "HTTP response");
  let body: Uint8Array | null = null;
  if (typeof ok.body === "string") body = decodeBase64(ok.body);
  else if (ok.body !== null && ok.body !== undefined) {
    throw new Error("HTTP response: `body` must be a base64 string or null");
  }
  if (body !== null && body.length > MAX_HTTP_BODY_BYTES) {
    throw new Error(`HTTP response of ${body.length} bytes exceeds the ${MAX_HTTP_BODY_BYTES} byte limit`);
  }
  if (NULL_BODY_STATUS.has(status)) body = null;
  return new Response(body as BodyInit | null, { status, headers });
}

/**
 * Receives the guest's response writes. `start` builds the Response (its body is a ReadableStream
 * fed by `write`); `write` waits while the client lags more than ~1 MiB behind, and fails once the
 * client cancelled the response, so the guest stops producing.
 */
export class HttpSink {
  /** Resolves when the guest sent `httpStart`. */
  readonly started: Promise<void>;
  /** Set by `start`. */
  response: Response | null = null;
  cancelled = false;
  private resolveStarted!: () => void;
  private controller!: ReadableStreamDefaultController<Uint8Array>;
  private waiting: (() => void)[] = [];
  private written = 0;
  private ended = false;

  constructor(
    readonly head: HttpHead,
    readonly requestBody: ReadableStream<Uint8Array> | null,
  ) {
    this.started = new Promise<void>((resolve) => {
      this.resolveStarted = resolve;
    });
  }

  private release() {
    const w = this.waiting;
    this.waiting = [];
    for (const f of w) f();
  }

  /** `httpStart`: validates the head and creates the streaming Response. */
  start(status: unknown, headers: unknown): void {
    if (this.response) throw new Error("httpStart: the response was already started");
    const code = checkStatus(status, "httpStart");
    if (NULL_BODY_STATUS.has(code)) {
      throw new Error(`httpStart: status ${code} cannot have a streamed body`);
    }
    const h = toHeaders(headers, "httpStart");
    const stream = new ReadableStream<Uint8Array>(
      {
        start: (c) => {
          this.controller = c;
        },
        pull: () => this.release(),
        cancel: () => {
          this.cancelled = true;
          this.release();
        },
      },
      { highWaterMark: STREAM_HIGH_WATER_BYTES, size: (chunk) => chunk?.byteLength ?? 0 },
    );
    this.response = new Response(stream, { status: code, headers: h });
    this.resolveStarted();
  }

  /** `httpWrite`: queues bytes for the client; resolves once the queue has room again. */
  write(bytes: Uint8Array): Promise<void> {
    if (!this.response) throw new Error("httpWrite before httpStart");
    if (this.ended) throw new Error("httpWrite after the response ended");
    if (this.cancelled) throw new Error("the client closed the response");
    this.written += bytes.length;
    if (this.written > MAX_HTTP_BODY_BYTES) {
      const e = new Error(`HTTP response exceeds the ${MAX_HTTP_BODY_BYTES} byte limit`);
      this.fail(e);
      throw e;
    }
    this.controller.enqueue(bytes);
    const room = this.controller.desiredSize;
    if (room !== null && room <= 0) {
      return new Promise<void>((resolve) => this.waiting.push(resolve));
    }
    return Promise.resolve();
  }

  /** `httpEnd` (and the guest returning): closes the body. */
  end(): void {
    if (!this.response || this.ended) return;
    this.ended = true;
    try {
      this.controller.close();
    } catch {
      // already cancelled / errored
    }
    this.release();
  }

  /** The guest failed after `httpStart`: error the body so the client sees a truncated response. */
  fail(e: unknown): void {
    if (!this.response || this.ended) return;
    this.ended = true;
    try {
      this.controller.error(e);
    } catch {
      // already cancelled / errored
    }
    this.release();
  }
}

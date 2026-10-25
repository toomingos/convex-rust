// Deterministic mock HTTP server for the net-* cases. Dependency-free.
import { createServer } from "node:http";
import type { ServerResponse } from "node:http";
import { createHash } from "node:crypto";
import { Prng } from "../shared/prng.ts";

const port = Number(process.env.MOCK_PORT ?? 8787);

function json(res: ServerResponse, status: number, body: unknown): void {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json", "content-length": Buffer.byteLength(text) });
  res.end(text);
}

const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://mock");
  const [, kind, arg] = url.pathname.split("/");
  const n = Number(arg);
  const seed = Number(url.searchParams.get("seed") ?? 1);

  if (kind === "bytes" && Number.isInteger(n) && n >= 0) {
    const body = Buffer.from(new Prng(seed).bytes(n));
    res.writeHead(200, { "content-type": "application/octet-stream", "content-length": n });
    res.end(body);
  } else if (kind === "stream" && Number.isInteger(n) && n >= 0) {
    // Chunked transfer encoding (no content-length), 16 KiB pieces.
    const body = Buffer.from(new Prng(seed).bytes(n));
    res.writeHead(200, { "content-type": "application/octet-stream", "transfer-encoding": "chunked" });
    for (let i = 0; i < n; i += 16384) res.write(body.subarray(i, Math.min(n, i + 16384)));
    res.end();
  } else if (kind === "echo" && req.method === "POST") {
    const hash = createHash("sha256");
    let length = 0;
    req.on("data", (c: Buffer) => {
      hash.update(c);
      length += c.length;
    });
    req.on("end", () => json(res, 200, { length, sha256: hash.digest("hex") }));
  } else if (kind === "sleep" && Number.isInteger(n) && n >= 0) {
    const t = setTimeout(() => json(res, 200, { slept: n }), n);
    res.on("close", () => clearTimeout(t));
  } else if (kind === "redirect" && Number.isInteger(n) && n >= 0) {
    const total = Number(url.searchParams.get("t") ?? n);
    if (n === 0) {
      json(res, 200, { done: true, hops: total });
    } else {
      res.writeHead(302, { location: `/redirect/${n - 1}?t=${total}`, "content-length": 0 });
      res.end();
    }
  } else if (kind === "json" && Number.isInteger(n) && n >= 0) {
    const p = new Prng(seed);
    json(res, 200, Array.from({ length: n }, (_, i) => ({ i, name: p.word(8), score: p.below(1000) })));
  } else {
    json(res, 404, { error: "not found", path: url.pathname });
  }
});

server.listen(port, "127.0.0.1", () => console.log(`mock-server listening on http://127.0.0.1:${port}`));

# The `Ctx` API

`ctx: &Ctx` is the Rust counterpart of `ctx` in a TypeScript action. Everything async returns
`convex_fn::Result`. Names mirror the JS ones (`runQuery` is `run_query`, `ctx.storage` is `ctx.storage()`).

## Running other functions

```rust
use convex_fn::json;
use crate::api;

// typed: generated FunctionRef + Args struct, return type from the TS `returns` validator
let size: f64 = ctx.run_query(api::settings::thumbSize, api::settings::ThumbSizeArgs {}).await?;

// untyped: any "module:function" path, any serde types
let docs: Vec<serde_json::Value> = ctx.run_query("messages:list", json!({})).await?;
ctx.run_mutation("messages:add", json!({ "body": "hi" })).await?;     // -> R: DeserializeOwned
let r: String = ctx.run_action("rust/images:other", json!({})).await?; // any runtime (TS, node, rust)
```

`run_query`, `run_mutation`, `run_action` take `(target, args)`. `target` is a `&str`/`String` path or an
`api::` `FunctionRef`; `args` is anything `Serialize`; the result is any `DeserializeOwned` (it is inferred
from a typed ref). A function that fails returns `Err`; a `ConvexError` from the callee keeps its `data`.
Rust function paths are `rust/<module>:<fn>`.

`target` can also be a `FunctionHandle` (a `function://...` string from `createFunctionHandle`; a plain `&str`
starting with `function://` works too) or a `ComponentFunctionRef` (the serialized reference path of a
component function, as JS keeps under `Symbol.for("toReferencePath")` on `components.<name>.<fn>`):

```rust
use convex_fn::{ComponentFunctionRef, FunctionHandle};
ctx.run_mutation(FunctionHandle::new(handle), json!({})).await?;
ctx.run_query(ComponentFunctionRef::new(reference_path), json!({})).await?;
```

Run several at once with `convex_fn::join_all` (the `Promise.all` of this SDK):

```rust
let futs = ids.iter().map(|id| ctx.run_query(api::items::get, api::items::GetArgs { id: id.clone() }));
let items: Vec<_> = convex_fn::join_all(futs).await.into_iter().collect::<Result<_>>()?;
```

## Scheduler

```rust
let id = ctx.scheduler().run_after(5_000.0, "rust/images:cleanup", json!({})).await?; // ms from now
ctx.scheduler().run_at(1_900_000_000_000.0, "jobs:daily", json!({})).await?;        // unix ms
ctx.scheduler().cancel(&id).await?;
```

The function argument takes the same targets as `run_query` (path, typed ref, `FunctionHandle`, `ComponentFunctionRef`).

## Storage

```rust
let id = ctx.storage().store(&bytes, Some("image/png")).await?;       // -> storage id
let file = ctx.storage().get(&id).await?;                              // Option<StorageFile { content_type, data }>
let url  = ctx.storage().get_url(&id).await?;                          // Option<String>
let meta = ctx.storage().get_metadata(&id).await?;                     // Option<StorageMetadata { storage_id, sha256, size, content_type }>
ctx.storage().delete(&id).await?;
let upload_url = ctx.storage().generate_upload_url().await?;           // ctx.storage.generateUploadUrl()
```

Whole files are buffered in memory (see [limits.md](limits.md)).

## Auth

```rust
let identity: Option<serde_json::Value> = ctx.user_identity().await?;  // ctx.auth.getUserIdentity()
```

`None` for unauthenticated calls; otherwise the same JSON fields as in TS (`subject`, `issuer`, `email`...).

## fetch

```rust
use convex_fn::{FetchRequest, Redirect};

let res = ctx.fetch(FetchRequest::get("https://api.example.com/x").header("authorization", "Bearer ...")).await?;
let res = ctx.fetch(FetchRequest::post(url, body_bytes).header("content-type", "application/json")).await?;
res.status; res.status_text; res.headers; res.url; res.body;     // Vec<(String,String)> headers, Vec<u8> body
let v: MyType = res.json()?;  let s = res.text()?;
```

- Redirects are followed (up to 20, like Node `fetch`); `.redirect(Redirect::Manual)` returns the 3xx.
  `res.url` is the final URL.
- It goes through the same host `fetch` as TS actions: same egress rules, no extra TLS in your wasm.
- A non-2xx status is **not** an `Err`; check `res.status`.
- `FetchRequest` fields are public if you need to build one by hand (`method`, `headers`, `body`).

### Streaming a response

`fetch` reads the whole body. For large bodies read chunks as they arrive:

```rust
let mut s = ctx.fetch_stream(FetchRequest::get(url)).await?;   // resolves when the headers arrive
println!("{} {}", s.status, s.url);
while let Some(chunk) = s.chunk().await? {                      // Vec<u8>, None at the end
    hasher.update(&chunk);
}
// or: let all = s.bytes().await?;
```

Dropping the stream early discards the rest (and, on a host with `net/cancel`, aborts the download).

### Cancelling, streaming a request body, large bodies

- Dropping a `fetch` future aborts the host request: `tokio::time::timeout(..)`, `race`, `select` and the
  reqwest shim's `.timeout()` really stop the transfer.
- Request bodies of 8 KiB or more reach the host through guest memory (no base64); bigger than 64 MiB they
  are sent in slices. `ctx.storage_store` does the same.
- Stream an upload from any source of chunks (the body never has to fit in memory; the guest is paced by
  the network):

```rust
use convex_fn::Chunks;
let parts = vec![b"first ".to_vec(), b"second".to_vec()];
let res = ctx.fetch_upload(FetchRequest::get(url).method("PUT"), Chunks(parts.into_iter())).await?;
// A FetchStream is a source too: ctx.fetch_stream_upload(req, other_stream) proxies a download.
```

  `reqwest::Body::wrap_stream` takes the same path. On a host without streamed bodies the chunks are
  buffered into one body. A streamed body cannot follow a redirect that resends it (307/308).

## DNS

No sockets, so DNS goes over HTTPS (Cloudflare by default):

```rust
use convex_fn::dns::{self, RecordType, Record};
for a in dns::resolve("example.com", RecordType::Mx).await? {   // A, AAAA, MX, TXT, SRV, NS, CNAME
    if let Record::Mx { preference, exchange } = a.record { /* ... */ }
}
let r = dns::Resolver::new(dns::GOOGLE);                         // or any JSON DoH endpoint
```

A missing name gives an empty list; a server failure is an `Err`.

## Environment variables

`std::env::var("NAME")` returns the deployment's environment variables, set with
`npx convex env set NAME value` or the dashboard. Nothing to configure.

## Logging

- `println!` / `print!` is a log line at info level, `eprintln!` at error level. No setup.
- `convex_fn::log::{debug, info, warn, error}(&str)` writes a line at that level.
- Cargo feature `log`: standard `log::info!` etc. work (a logger is installed for you unless you installed
  one). Feature `tracing`: `tracing::info!` events and span fields are written as
  `span{fields}: message key=value`.

```toml
convex_fn = { path = "...", features = ["log"] }   # or "tracing", or both
log = "0.4"
```

Lines show up in `npx convex run`/`dev`/`logs` output and the dashboard. A panic is an error line with
`file:line:col` ([writing-actions.md](writing-actions.md#errors-and-panics)).

## Time, randomness, sleeping

- `std::time::SystemTime::now()` and `Instant` work; `ctx.now_ms()` is the host clock in ms.
- `rand` and anything built on `getrandom` (0.2 and 0.3) work; no feature flags needed.
- `ctx.sleep(ms).await?` waits without burning CPU (tokio users can use `tokio::time::sleep`, below).
  Time spent waiting counts against the action timeout.

## Async: concurrency without threads

There are no threads. Concurrency is cooperative: futures make progress while others wait on the host.

```rust
use convex_fn::{join_all, race, select, Either};

let both = join_all([ctx.run_query(a, x), ctx.run_query(b, y)]).await;   // all, in order

match select(ctx.run_query(a, x), ctx.sleep(3_000)).await {              // first of two (a wins ties)
    Either::Left(res) => res?,
    Either::Right(_) => return Err(convex_fn::Error::msg("timed out")),
};
let (winner_index, out) = race(vec![fut0, fut1, fut2]).await;            // first of N; others dropped
```

A losing future is dropped and its in-flight host call is abandoned (its late result is ignored). `race`
panics on an empty list.

### `tokio-rt` feature

```toml
convex_fn = { path = "...", features = ["tokio-rt"] }
```

Runs the action on a single-thread tokio runtime, so `tokio::spawn`, `spawn_local`, `tokio::time::sleep`,
`tokio::select!`, `join!` and tokio channels work. It is enabled automatically when the built-in `reqwest`
(or `convex_aws`) is in your dependency graph. Limits:

- Single thread only: `tokio::spawn` is cooperative; `std::thread::spawn` and rayon cannot start threads.
- No `tokio::net` (no sockets).
- **Blocking calls**: `reqwest::blocking` and `block_on` inside an action work, but while a blocking call
  runs, tasks you spawned and timers do not progress; they resume when it returns. A blocking call must only
  depend on host work (fetch, `run_*`, sleep), never on a task you spawned.
- Tokio idle-polls the host (about 1 to 8 ms), a small latency cost vs. the plain executor.

Without the feature, `join_all`, `select` and `race` run on a minimal built-in executor (no tokio needed).

## Vector search and component arguments

```rust
use convex_fn::{VectorFilter, VectorSearchQuery};
let hits = ctx.vector_search("docs", "by_embedding", VectorSearchQuery {
    vector: embedding, limit: Some(5),
    filter: Some(VectorFilter::or([VectorFilter::eq("lang", "en"), VectorFilter::eq("lang", "pt")])),
}).await?;                                            // Vec<VectorHit { id, score }>
let v: Option<serde_json::Value> = ctx.component_argument("name")?; // inside a component; Convex JSON
```

## HTTP actions

`#[http_action] async fn h(ctx: &Ctx, req: HttpRequest) -> Result<HttpResponse>` has this same `Ctx` (streaming on the V8 host; buffered on the fork runtimes). `HttpRequest`: `method`,
`url`, `headers`, `header(name)`, `path()`, `query()`, `chunk()` (streamed body), `bytes()`, `text()`, `json::<T>()`. `HttpResponse`:
`new(status)`, `ok()`, `text(..)`, `json(&v)`, `.header(k, v)`, `.body(bytes)`, `.streaming(|w| async move { w.write(..).await })`
(`BodyWriter::write` / `write_str`). See [cloud.md](cloud.md#http-actions). Unit-test with `TestHost::run_http` / `call_http!` from `convex_fn_test`.

## Escape hatch

`ctx.syscall("1.0/...", json!({...})).await` runs any async host syscall with the JSON the JS runtime sends,
and `ctx.sync_syscall("now", json!({}))` any sync one. Use only for features that have no wrapper yet. On the cloud (V8) host, names without a built-in mapping go to the
runtime's own `Convex.asyncSyscall`, so only syscalls that need no per-request id work (see `contracts/ABI.md`).

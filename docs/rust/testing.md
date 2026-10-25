# Testing actions

`convex_fn_test` runs your actions natively under `cargo test` against an in-process mock host (the idea of
`convex-test` for TS). On non-wasm targets `#[action]` leaves your function a plain `async fn`, so tests call
it directly. No backend, no wasm, instant.

## Setup

```sh
cd convex/rust
cargo add --dev convex_fn_test --path /path/to/convex-rs/crates/convex_fn_test   # not published yet
```

Run them with plain `cargo test` (it builds for your host; only `npx convex dev` builds wasm):

```sh
cargo test
```

Because the crate is a `cdylib`, put tests in `#[cfg(test)] mod tests` inside `src/` (files in `tests/`
cannot link a cdylib).

## Example

Tests for the `thumbnail` action from [getting-started.md](getting-started.md):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use convex_fn::json;
    use convex_fn_test::{call, FetchStubResponse, TestHost};

    fn test_png() -> Vec<u8> {
        let img = image::RgbImage::new(200, 100);
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn thumbnail_resizes_and_stores() {
        let host = TestHost::new()
            .on_query("settings:thumbSize", |_args| Ok::<_, String>(json!(32)))
            .on_fetch(|_req| FetchStubResponse::new(200, test_png()));

        let out = call!(host, thumbnail, json!({ "url": "https://example.test/a.png" }));

        assert_eq!(out["ok"]["width"].as_f64(), Some(32.0));
        let id = out["ok"]["storageId"].as_str().unwrap();
        assert!(host.storage_read(id).is_some());
        assert!(host.logs().iter().any(|(_, m)| m.starts_with("thumbnail 32x16")));
    }

    #[test]
    fn thumbnail_reports_http_errors() {
        let host = TestHost::new()
            .on_query("settings:thumbSize", |_| Ok::<_, String>(json!(32)))
            .on_fetch(|_req| FetchStubResponse::new(404, "nope"));
        let out = call!(host, thumbnail, json!({ "url": "https://example.test/x.png" }));
        assert!(out["error"]["message"].as_str().unwrap().contains("404"));
    }
}
```

`call!(host, action_fn, args_json)` returns the same envelope the wasm export produces: `{"ok": value}` or
`{"error": {"message", "kind", "data"?}}`. Args go through serde exactly as in production.

Use `host.run(|ctx| async move { ... })` to call `Ctx` methods directly in a test instead.

## The mock host

| Builder | Mocks |
|---|---|
| `.on_query(name, \|args\| Ok(json))` / `.on_mutation` / `.on_action` | `ctx.run_query` etc. by path (`"settings:thumbSize"`); return `Err("msg")` or `Err(HostError::convex(data))` for failures. A call to an unregistered function fails. |
| `.on_fetch(\|req\| FetchStubResponse)` | `ctx.fetch` / `fetch_stream`: `FetchStubResponse::new(status, body)`, `.chunked(status, chunks)`, `.header(k, v)`, `.fail_after(msg)`; `req` has `url`, `method`, `headers`, `body`. Without a stub fetches fail. |
| `.with_identity(json)` | `ctx.user_identity()` |
| `.with_now_ms(ms)` | the clock; `ctx.sleep` advances it virtually (no real waiting) |
| `.on_syscall(name, \|args\| ...)` | any raw syscall |
| `.with_reverse_delivery()` | delivers results newest-first to shake out ordering assumptions |

Storage and the scheduler are built in: `host.storage_put(bytes, content_type)`, `host.storage_read(id)`,
`host.scheduled()`, and `host.calls()` (every `run_*` call: kind, name, args), `host.logs()` (level, message),
`host.slept_ms()`.

Not covered by the mock: real database semantics (mock your queries/mutations), real network, wasm-specific
behaviour (memory limits, panics-as-traps). For those use `npx convex dev` against a local backend.

Test your TS functions with `convex-test` as usual; the two sides meet at the function path and JSON shape.

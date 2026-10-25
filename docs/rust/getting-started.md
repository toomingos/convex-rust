# Getting started

Goal: a `thumbnail` action that downloads an image, resizes it with the `image` crate, stores it in file
storage, and is callable from React as `api.rust.images.thumbnail`. About 10 minutes, most of it the first
compile.

## Prerequisites

1. A Convex project on the fork backend, with the fork CLI as your `convex` npm dependency
   ([self-hosted-fork.md](self-hosted-fork.md)). In a brand-new folder that means `npm init -y` and the
   fork `npm install` first. Every `npx convex ...` below is that CLI.
   For a self-hosted backend export:
   ```sh
   export CONVEX_SELF_HOSTED_URL=http://127.0.0.1:3210
   export CONVEX_SELF_HOSTED_ADMIN_KEY=<admin key>
   ```
2. **Rust via rustup**: <https://rustup.rs>. Nothing else to install by hand. The CLI reads the
   `rust-toolchain.toml` that `init` writes (a pinned nightly) and checks that the toolchain and the
   `wasm32-wasip1` target are installed. If something is missing, in a terminal it prints the exact command
   and asks `Run ... now? (Y/n)`:
   ```
   rustup toolchain install nightly-2026-06-28 --profile minimal --target wasm32-wasip1
   rustup target add wasm32-wasip1 --toolchain nightly-2026-06-28
   ```
   Without a TTY (CI) it fails with that command instead; it never installs anything silently.
3. Optional: `wasm-opt` ([binaryen](https://github.com/WebAssembly/binaryen)) shrinks release builds, see
   [deploy-ci.md](deploy-ci.md).

## 1. Scaffold

From the project root (the folder that contains `convex/`):

```sh
CONVEX_FN_PATH=/path/to/convex-rs/crates/convex_fn npx convex rust init
```

`convex_fn` is not published to crates.io yet, so `CONVEX_FN_PATH` points the generated `Cargo.toml` at a
checkout of this repo. Without it `Cargo.toml` contains `convex_fn = "0.1"` plus a TODO and the first build
fails. Once `convex_fn` is published you run just `npx convex rust init`.

It creates `convex/rust/` and never overwrites existing files:

```
convex/rust/
  Cargo.toml              crate "convex_rust", cdylib, depends on convex_fn
  rust-toolchain.toml     pinned toolchain + wasm32-wasip1 target
  .cargo/config.toml      CLI-managed [patch] block (empty until a dependency uses reqwest; do not edit)
  .gitignore              target/
  src/lib.rs              declares your modules and the generated `api` module
  src/example.rs          an example #[action]
  src/_generated/api.rs   typed references to your TS functions (rewritten by `convex dev`)
```

Commit all of it, including `Cargo.lock` once it exists and `src/_generated/api.rs`.

## 2. Add a crate

```sh
cd convex/rust
cargo add image
```

Plain `cargo add` is all it takes; `npx convex dev` builds for wasm. See [crates.md](crates.md).

## 3. Write the TypeScript side

The action will call a TS query through a typed reference (`api::settings::thumbSize`). `npx convex dev`
generates `api.rs` from your TS functions before it builds Rust, so new TS and Rust code can go in the same
push. Give the query `returns` so Rust gets a real type instead of `serde_json::Value`.

```ts
// convex/settings.ts
import { query } from "./_generated/server";
import { v } from "convex/values";

export const thumbSize = query({
  args: {},
  returns: v.number(),
  handler: async () => 64,
});
```


## 4. Write the action

```rust
// convex/rust/src/images.rs
use convex_fn::{action, log, ConvexArgs, ConvexValue, Ctx, Error, FetchRequest, Result};
use image::imageops::FilterType;
use serde::{Deserialize, Serialize};

use crate::api;

#[derive(Deserialize, ConvexArgs)]
pub struct ThumbnailArgs {
    pub url: String,
}

#[derive(Serialize, ConvexValue)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub storage_id: String,
    pub width: f64,
    pub height: f64,
}

#[action]
pub async fn thumbnail(ctx: &Ctx, args: ThumbnailArgs) -> Result<Thumbnail> {
    // 1. A TypeScript query, through the generated typed reference.
    let size: f64 = ctx
        .run_query(api::settings::thumbSize, api::settings::ThumbSizeArgs {})
        .await?;

    // 2. An env var (set with: npx convex env set THUMB_FILTER nearest).
    let filter = match std::env::var("THUMB_FILTER").as_deref() {
        Ok("nearest") => FilterType::Nearest,
        _ => FilterType::Lanczos3,
    };

    // 3. Download, resize, re-encode.
    let res = ctx.fetch(FetchRequest::get(&args.url)).await?;
    if res.status != 200 {
        return Err(Error::msg(format!("GET {} returned {}", args.url, res.status)));
    }
    let img = image::load_from_memory(&res.body)?;
    let thumb = img.resize(size as u32, size as u32, filter);
    let mut png = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut png, image::ImageFormat::Png)?;

    // 4. Store it in file storage.
    let storage_id = ctx.storage().store(png.get_ref(), Some("image/png")).await?;
    log::info(&format!("thumbnail {}x{} -> {storage_id}", thumb.width(), thumb.height()));
    Ok(Thumbnail { storage_id, width: thumb.width() as f64, height: thumb.height() as f64 })
}
```

Register the module in `convex/rust/src/lib.rs` (add one line; every `pub mod` is a Convex module):

```rust
pub mod example;
pub mod images;
```

## 5. Push and run

```sh
npx convex env set THUMB_FILTER nearest
npx convex dev            # builds the wasm, pushes, regenerates types; keeps watching
# or once:  npx convex dev --once
npx convex run rust/images:thumbnail '{"url":"https://www.python.org/static/img/python-logo.png"}'
```

Any reachable PNG/JPEG URL works; the backend needs network access to fetch it.

Output:

```
[CONVEX ?(rust/images:thumbnail)] [LOG] thumbnail 64x48 -> kg2...
{ "height": 48, "storageId": "kg2...", "width": 64 }
```

`npx convex dev` uses a fast dev profile (about 1 to 5 seconds per rebuild after the first compile, which
builds all dependencies). `npx convex deploy` uses a full release build ([deploy-ci.md](deploy-ci.md)).
`println!`, `eprintln!`, [`log::info`](ctx-api.md#logging) lines appear in the terminal, in
`npx convex logs` and in the dashboard.

## 6. Call it from React

`npx convex dev` regenerates `convex/_generated/api.d.ts`; the Rust function is typed from the
`ConvexArgs` / `ConvexValue` derives:

```tsx
import { useAction } from "convex/react";
import { api } from "../convex/_generated/api";

function Uploader() {
  const thumbnail = useAction(api.rust.images.thumbnail);
  return (
    <button onClick={async () => {
      const t = await thumbnail({ url: "https://example.com/photo.png" });
      console.log(t.storageId, t.width, t.height); // string, number, number
    }}>Make thumbnail</button>
  );
}
```

Passing `{ url: 1 }`, or reading a field the Rust type does not have, is a TypeScript error. To check
without a bundler: `npm i -D typescript`, then `npx tsc --noEmit -p convex` if you have a `convex/tsconfig.json`
(self-hosted dev may not write one), otherwise use your app's own `tsconfig`.
Actions marked `#[action(internal)]` are under `internal.rust...` instead
([writing-actions.md](writing-actions.md#internal-actions)).

To show the image, have a TS query return `await ctx.storage.getUrl(storageId)`, or return the URL from the
action with `ctx.storage().get_url(&id)`.

## 7. See an error with file:line

Add a bug:

```rust
#[action]
pub async fn broken(_ctx: &Ctx, _args: ThumbnailArgs) -> Result<f64> {
    let n: u32 = "not a number".parse().unwrap();
    Ok(n as f64)
}
```

```
$ npx convex run rust/images:broken '{"url":"x"}'
[CONVEX ?(rust/images:broken)] [ERROR] Uncaught panic: called `Result::unwrap()` on an `Err` value: ParseIntError { kind: InvalidDigit } at src/images.rs:52:41
Error: [Request ID: ...] Server Error
Uncaught Error: called `Result::unwrap()` on an `Err` value: ParseIntError { kind: InvalidDigit } at src/images.rs:52:41
```

The same message and `src/images.rs:52:41` (path relative to `convex/rust`) appear in the dashboard logs
and in the error. Compile errors point at `src/...:line:col` too, with the usual rustc output. More in
[writing-actions.md](writing-actions.md#errors-and-panics).

## What next

[writing-actions.md](writing-actions.md), [ctx-api.md](ctx-api.md), [testing.md](testing.md),
[deploy-ci.md](deploy-ci.md).

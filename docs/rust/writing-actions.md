# Writing actions

```rust
use convex_fn::{action, ConvexArgs, ConvexValue, Ctx, Result};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, ConvexArgs)]
pub struct GreetArgs { pub name: String }

#[derive(Serialize, ConvexValue)]
pub struct Greeting { pub text: String }

#[action]
pub async fn greet(_ctx: &Ctx, args: GreetArgs) -> Result<Greeting> {
    Ok(Greeting { text: format!("hello {}", args.name) })
}
```

## The signature

`#[action]` takes exactly `async fn(ctx: &Ctx, args: A) -> convex_fn::Result<R>`:

- `ctx` is a shared reference (`&Ctx`), see [ctx-api.md](ctx-api.md).
- `A: serde::DeserializeOwned`, `R: serde::Serialize`.
- No `self`, no generics, must be `async`, must return `Result<R>`. The macro tells you which one you broke.
- The function must be `pub` in a module (below). Use `()` args by taking `_args: NoArgs` with an empty struct
  (`#[derive(Deserialize, ConvexArgs)] pub struct NoArgs {}`); the client then calls it with `{}`.

## Arguments and return values: ConvexArgs / ConvexValue

serde does the JSON conversion. The derives additionally describe the type to Convex as a validator, which
gives you:

- argument validation by the backend (a wrong shape fails before your code runs, with the usual
  `ArgumentValidationError` message),
- the argument form in the dashboard,
- precise TypeScript types for `api.rust.<module>.<fn>` (args and return).

Use `#[derive(ConvexArgs)]` on the args struct and `#[derive(ConvexValue)]` on the return type (and nested
types; it also supports enums). Without the derive the function still works, the type is just
`any` on the TypeScript side and unvalidated.

| Rust | Convex validator | TS |
|---|---|---|
| `String` | `v.string()` | `string` |
| `f64`, `f32`, all integer types except `Int64` | `v.float64()` | `number` |
| `convex_fn::Int64` | `v.int64()` | `bigint` |
| `bool` | `v.boolean()` | `boolean` |
| `convex_fn::Bytes` | `v.bytes()` | `ArrayBuffer` |
| `Vec<T>` | `v.array(..)` | `T[]` |
| `Option<T>` field | `v.optional(..)` | `field?: T` |
| `HashMap<String, T>` | `v.record(v.string(), ..)` | `Record<string, T>` |
| `serde_json::Value` | `v.any()` | `any` |
| struct / enum deriving `ConvexValue` | `v.object` / `v.union` | object / union |

Field attributes:

```rust
#[derive(Deserialize, ConvexArgs)]
#[serde(rename_all = "camelCase")]            // serde rename / rename_all / default / skip* are followed
pub struct Args {
    pub about_message: Option<String>,        // -> aboutMessage?: string
    #[convex(id = "messages")]
    pub message: String,                      // -> v.id("messages") (a String field)
    #[convex(any)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub tags: Vec<String>,
}
```

Numbers: the Convex number type is a float64. All Rust numeric types except `Int64` are described as
float64; use `Int64` only when you need a real bigint.

## Return values

Return `Ok(value)`; it is serialized to JSON and must fit the return size limit ([limits.md](limits.md)).
Returning `()` gives `null`.

## Errors and panics

| In Rust | What the caller sees |
|---|---|
| `Err(Error::msg("..."))`, or any `std::error::Error` via `?` | `Server Error` + `Uncaught Error: <message>` (the message is in the function log too) |
| `Err(ConvexError::new(json!({"code": 7})))` | a `ConvexError` whose `.data` is that value, exactly like `throw new ConvexError(data)` in TS |
| a panic (`unwrap`, `expect`, index out of bounds, `panic!`) | `Server Error` + `Uncaught Error: <panic message> at src/images.rs:52:41` |
| argument does not match the validator | `ArgumentValidationError` (before the action runs) |
| a failing `ctx.run_*` call | the same error as in TS: `?` propagates it; a `ConvexError` thrown by the callee keeps its `data` |

`?` works on anything that implements `std::error::Error` (`image`, `serde_json`, `std::io`, ...), like
`anyhow`. For your own application errors use `ConvexError` so the client can match on `data`:

```rust
use convex_fn::{json, ConvexError};
return Err(ConvexError::new(json!({ "code": "too_big", "max": 5 })));
```
```ts
try { await thumbnail({ url }); }
catch (e) { if (e instanceof ConvexError) console.log(e.data.code); }  // "too_big"
```

A panic ends the action; the panic message and `file:line:col` are written to the log as an error line
(`Uncaught panic: ... at src/images.rs:52:41`) and become the error message. Paths are relative to
`convex/rust`.

## Internal actions

```rust
#[action(internal)]
pub async fn cleanup(ctx: &Ctx, args: Args) -> Result<()> { ... }
```

Internal actions are not callable from clients. In TS they are `internal.rust.<module>.<fn>`
(`import { internal } from "./_generated/api"`), in Rust `api::internal::rust::<module>::<fn>`, and by
string path `"rust/<module>:<fn>"` for `ctx.run_action` and the scheduler.

## Modules and names

The Convex module is the Rust module path below the crate root, joined by `/` and prefixed with `rust/`:

| Rust location | Convex function path | TS client |
|---|---|---|
| `src/images.rs`, `fn thumbnail` | `rust/images:thumbnail` | `api.rust.images.thumbnail` |
| `src/media/video.rs`, `fn probe` | `rust/media/video:probe` | `api.rust.media.video.probe` |
| `mod admin { ... }` inside `src/images.rs` | `rust/images/admin:fn` | `api.rust.images.admin.fn` |

Every module needs a `pub mod name;` line in `lib.rs` (or its parent module). The function name is used as
written (`plain_err` stays `plain_err`).

**Crate-root restriction:** an `#[action]` directly in `src/lib.rs` is rejected at push time with
`Rust function in the crate root (module path "convex_rust") is not supported; move it into a module.`
Put it in a module file.

TS code cannot import Rust modules (`import ... from "./rust/images"` fails at push time with
`Could not resolve`); cross the boundary with the generated `api` / `ctx.run_*`, never with imports.

## Adding and removing

`npx convex dev` rebuilds on every change under `convex/rust` (`src/`, `Cargo.toml`, `Cargo.lock`) and syncs
the set of functions: adding, renaming or deleting an action or a module is picked up on the next push, and
`api.rs` / `api.d.ts` are regenerated.

## Calling TypeScript from Rust (typed)

`src/_generated/api.rs` is regenerated from the pushed deployment on every `convex dev` / `convex deploy`.
For each function it has an `Args` struct and a `FunctionRef`:

```rust
let docs = ctx.run_query(api::messages::list, api::messages::ListArgs {}).await?;
let n: f64 = ctx.run_query(api::settings::thumbSize, api::settings::ThumbSizeArgs {}).await?; // TS `returns: v.number()`
```

The return type comes from the TS `returns` validator; without one it is `serde_json::Value`. Args come from
the `args` validator (`v.id("t")` fields are `String`). Rust actions are in there too
(`api::rust::images::thumbnail`), so Rust can call Rust. Untyped string paths always work:
`ctx.run_query("messages:list", json!({}))`.

# Manifest v1 (frozen at W0-13)

A `"use rust"` module describes its functions in a wasm **custom section**
named `convex:manifest`, containing UTF-8 JSON. The SDK's `#[action]` macro
emits it at compile time (one fragment per function, merged by the linker as
concatenated custom sections). Read by the CLI with
`WebAssembly.Module.customSections(module, "convex:manifest")` and by the
backend with `wasmparser`. No code is executed to analyze a module.

## Format

The section content is one or more JSON objects separated by `\n` (one per
function, because the linker concatenates same-named custom sections):

```json
{"abiVersion":1,"modulePath":"bench_rust::cpu","name":"cpu_001","visibility":"public","args":{"type":"object","value":{"seed":{"fieldType":{"type":"number"},"optional":false}}},"returns":null}
```

| Field | Type | Meaning |
|---|---|---|
| `abiVersion` | `1` | ABI version (contracts/ABI.md) |
| `modulePath` | string | Rust `module_path!()` of the function. Convex module = the segments after the crate name joined by `/` (`bench_rust::cpu` → `cpu`, `app::a::b` → `a/b`). Functions at the crate root are rejected by the backend-hosted runtime; the JS host (`convex-rust`) maps them to the module `lib` and rejects a build that also has a real `lib` module. |
| `name` | string | function name |
| `visibility` | `"public"` \| `"internal"` | as `action` / `internalAction` |
| `kind` | `"http"` (optional) | Additive. Present (`"http"`) for `#[http_action]` functions, absent for actions. An http entry has `args: null`, `returns: null`; the V8 codegen exports it as an `httpAction` (route it in `convex/http.ts`) and leaves it out of `api`. Only the V8 host runs http entries; the native host and backend-hosted runtime do not support them. |
| `args` | ValidatorJson \| null | Convex validator JSON (`common/src/schemas/json.rs` `ValidatorJson`), always an `object` validator when present. `null` = unvalidated (`v.any()` per field) |
| `returns` | ValidatorJson \| null | return validator, `null` = none |

Populated by the SDK (DX-07): `args` comes from `#[derive(ConvexArgs)]` on the
action's args type, `returns` from `#[derive(ConvexValue)]` (or a built-in
`ConvexType`) on the `Ok` type; otherwise `null`. Mapping: `String` → string,
`f32/f64` and plain integers → number (Float64), `convex_fn::values::Int64`/`i64`
wrapper → bigint (int64), `bool` → boolean, bytes → bytes, `Vec<T>` → array,
structs → object, `Option<T>` field → optional field of `union(T, null)`.

Export: `convex_run:<modulePath>:<name>`. Function path in Convex: `rust/<module>:<name>`; client API:
`api.rust.<module>.<name>`. All functions are actions (`udfType: "Action"`).

Consumers must ignore unknown fields and reject a module whose fragments have
different `abiVersion` values, or duplicate `modulePath:name` pairs.

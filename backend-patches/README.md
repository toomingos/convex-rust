# Backend patches (convex/convex-backend PR #620)

The three commits of [convex/convex-backend#620](https://github.com/convex/convex-backend/pull/620), as filed,
in `git format-patch` form. They are unmodified. Base: upstream `convex-backend` `main` at `bb31399`.

| Patch | Contents |
|---|---|
| `0001-...wasm-runtime-for-use-rust...` | Wasm runtime for `"use rust"` actions (isolate, application, CLI) |
| `0002-...opt-in-native...` | Opt-in native worker runtime (isolate, application, local_backend, CLI) |
| `0003-docs-design-...` | Design document: request flow and safety assessment (also at `docs/rust-native-runtime-design.md`) |

Apply in a `convex-backend` checkout:

```sh
git checkout bb31399
git am /path/to/convex-rust/backend-patches/*.patch
```

The same changes are on the fork branch
[toomingos/convex-backend `rust-native-runtime`](https://github.com/toomingos/convex-backend/tree/rust-native-runtime).

The native runtime is opt-in and experimental. It runs user code as an OS process on the backend host and is
meant for trusted, single-tenant, self-hosted backends only. The Linux sandbox (Landlock, seccomp, cgroups) has
not yet been verified on a real Linux kernel; see section 13 (Safety assessment) of the design document and
[docs/rust/native.md](../docs/rust/native.md).

The patches contain test fixtures whose logs include a username; these are already public in the PR.

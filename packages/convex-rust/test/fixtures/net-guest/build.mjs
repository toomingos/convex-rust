// Rebuilds ../net-guest.wasm from net_guest.rs (needs a rust toolchain with wasm32-wasip1).
// Run: node test/fixtures/net-guest/build.mjs
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = dirname(fileURLToPath(import.meta.url));
execFileSync(
  "rustc",
  [
    "--target", "wasm32-wasip1", "--edition", "2021", "-O", "--crate-type", "cdylib",
    "-C", "strip=symbols", "-o", join(dir, "../net-guest.wasm"), join(dir, "net_guest.rs"),
  ],
  { stdio: "inherit" },
);

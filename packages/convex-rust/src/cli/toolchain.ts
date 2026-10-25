import path from "path";

import nodeFs from "fs";
import { spawnSync } from "child_process";
export const RUST_TARGET = "wasm32-wasip1";

export interface ExecResult {
  status: number | null;
  stdout: string;
  stderr: string;
  error?: { code?: string } | Error | undefined;
}
export type Exec = (
  cmd: string,
  args: string[],
  opts: { cwd: string; inherit?: boolean },
) => ExecResult;

export const defaultExec: Exec = (cmd, args, opts) => {
  const r = spawnSync(cmd, args, {
    cwd: opts.cwd,
    encoding: "utf8",
    stdio: opts.inherit ? "inherit" : ["ignore", "pipe", "pipe"],
  });
  return {
    status: r.status,
    stdout: r.stdout ?? "",
    stderr: r.stderr ?? "",
    error: r.error,
  };
};

export type ToolchainStatus =
  | { ok: true }
  | {
      ok: false;
      kind: "no-cargo" | "no-rustup" | "no-toolchain" | "no-target";
      message: string;
      /** Command that fixes it (for no-target / no-toolchain). */
      fix?: string[];
    };

/** `channel` from `<rustDir>/rust-toolchain.toml`, if any. */
export function readToolchainChannel(rustDir: string): string | null {
  for (const f of ["rust-toolchain.toml", "rust-toolchain"]) {
    try {
      const text = nodeFs.readFileSync(path.join(rustDir, f), "utf8");
      const m = text.match(/^\s*channel\s*=\s*"([^"]+)"/m);
      if (m) return m[1];
      const bare = text.trim();
      if (f === "rust-toolchain" && bare && !bare.includes("\n")) return bare;
    } catch {
      // not present
    }
  }
  return null;
}

const RUSTUP_URL = "https://rustup.rs";

export function checkRustToolchain(
  rustDir: string,
  exec: Exec = defaultExec,
  target: string = RUST_TARGET,
): ToolchainStatus {
  const notFound = (r: ExecResult) => (r.error as any)?.code === "ENOENT";
  const cargo = exec("cargo", ["--version"], { cwd: rustDir });
  const rustup = exec("rustup", ["--version"], { cwd: rustDir });
  if (notFound(rustup)) {
    if (notFound(cargo)) {
      return {
        ok: false,
        kind: "no-cargo",
        message: `Neither \`cargo\` nor \`rustup\` was found. Install Rust with rustup: ${RUSTUP_URL}\nThen re-run this command (the ${target} target will be set up for you).`,
      };
    }
    return {
      ok: false,
      kind: "no-rustup",
      message: `\`rustup\` was not found, so the ${target} target can't be verified. Install rustup: ${RUSTUP_URL}\n(If you manage Rust another way, make sure the ${target} target is installed.)`,
    };
  }
  if (notFound(cargo)) {
    return {
      ok: false,
      kind: "no-cargo",
      message: `\`cargo\` was not found. Install Rust with rustup: ${RUSTUP_URL}`,
    };
  }
  const channel = readToolchainChannel(rustDir);
  const toolchainArgs = channel ? ["--toolchain", channel] : [];
  const list = exec(
    "rustup",
    ["target", "list", "--installed", ...toolchainArgs],
    { cwd: rustDir },
  );
  if (list.status !== 0) {
    const fix = [
      "rustup",
      "toolchain",
      "install",
      channel ?? "stable",
      "--profile",
      "minimal",
      "--target",
      target,
    ];
    if (/not installed|no such toolchain|is not installed/i.test(list.stderr)) {
      return {
        ok: false,
        kind: "no-toolchain",
        message: `The Rust toolchain \`${channel ?? "stable"}\` is not installed. Run:\n  ${fix.join(" ")}`,
        fix,
      };
    }
    // Unknown rustup failure: don't block the build, cargo will report it.
    return { ok: true };
  }
  const installed = list.stdout.split("\n").map((l) => l.trim());
  if (installed.includes(target)) return { ok: true };
  const fix = ["rustup", "target", "add", target, ...toolchainArgs];
  return {
    ok: false,
    kind: "no-target",
    message: `The ${target} Rust target is not installed${channel ? ` for toolchain ${channel}` : ""}. Run:\n  ${fix.join(" ")}`,
    fix,
  };
}

/** Install hint for binaryen's `wasm-opt` for the current platform. */
export function wasmOptInstallHint(platform: string = process.platform): string {
  if (platform === "darwin") return "brew install binaryen";
  if (platform === "win32") return "npm install --save-dev binaryen (or scoop install binaryen)";
  return "apt install binaryen / dnf install binaryen / npm install --save-dev binaryen";
}

/**
 * When the toolchain check failed with a fixable cause (`status.fix`) and a person is at the terminal,
 * offers to run the fix (`Run ... now? (Y/n)`), then re-checks. Never installs anything without an
 * answer: without a TTY (CI) or when declined the original failing status is returned.
 */
export async function offerToolchainFix(
  rustDir: string,
  status: ToolchainStatus,
  o: {
    interactive?: boolean;
    ask?: (question: string) => Promise<string>;
    exec?: Exec;
    log?: (m: string) => void;
  } = {},
): Promise<ToolchainStatus> {
  if (status.ok || !status.fix) return status;
  const exec = o.exec ?? defaultExec;
  const interactive = o.interactive ?? Boolean(process.stdin.isTTY && process.stdout.isTTY);
  if (!interactive) return status;
  const ask =
    o.ask ??
    (async (q: string) => {
      const rl = (await import("node:readline/promises")).createInterface({ input: process.stdin, output: process.stderr });
      try {
        return await rl.question(q);
      } finally {
        rl.close();
      }
    });
  const cmd = status.fix.join(" ");
  const answer = (await ask(`${status.message.split("\n")[0]}\nRun \`${cmd}\` now? (Y/n) `)).trim().toLowerCase();
  if (answer !== "" && answer !== "y" && answer !== "yes") return status;
  const r = exec(status.fix[0], status.fix.slice(1), { cwd: rustDir, inherit: true });
  if (r.status !== 0) return status;
  return checkRustToolchain(rustDir, exec);
}

export interface DoctorLine {
  ok: boolean;
  /** Missing but not fatal (wasm-opt). */
  optional?: boolean;
  text: string;
}

/** Environment report for `convex-rust doctor`. `ok` is false when a required piece is missing. */
export function doctorReport(
  rustDir: string | null,
  exec: Exec = defaultExec,
  extra: { convexFnPath?: string | undefined; functionsDirExists?: boolean } = {},
): { ok: boolean; lines: DoctorLine[] } {
  const lines: DoctorLine[] = [];
  const ver = (cmd: string, args = ["--version"]) => {
    const r = exec(cmd, args, { cwd: rustDir ?? process.cwd() });
    return r.status === 0 ? r.stdout.trim().split("\n")[0] : null;
  };
  const cargo = ver("cargo");
  lines.push({ ok: cargo !== null, text: cargo ? `cargo: ${cargo}` : "cargo not found. Install Rust with rustup: https://rustup.rs" });
  const rustup = ver("rustup");
  lines.push({ ok: rustup !== null, text: rustup ? `rustup: ${rustup.split(" (")[0]}` : "rustup not found. Install it from https://rustup.rs" });
  if (rustDir && cargo && rustup) {
    const st = checkRustToolchain(rustDir, exec);
    const channel = readToolchainChannel(rustDir);
    lines.push({
      ok: st.ok,
      text: st.ok ? `toolchain ${channel ?? "default"} with ${RUST_TARGET}` : st.message,
    });
  }
  if (extra.convexFnPath !== undefined) {
    const has = nodeFs.existsSync(path.join(extra.convexFnPath, "Cargo.toml"));
    lines.push({ ok: has, text: has ? `convex_fn: ${extra.convexFnPath}` : `convex_fn not found at ${extra.convexFnPath}. Reinstall convex-rust (npm install convex-rust).` });
  }
  const wo = ver("wasm-opt");
  lines.push({
    ok: wo !== null,
    optional: true,
    text: wo ? `wasm-opt: ${wo}` : `wasm-opt not found (optional: smaller wasm). Install: ${wasmOptInstallHint()}`,
  });
  return { ok: lines.every((l) => l.ok || l.optional), lines };
}

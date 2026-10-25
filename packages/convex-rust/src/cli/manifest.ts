// Manifest v1 (contracts/MANIFEST.md): read from the `convex:manifest` custom section.

export const MANIFEST_SECTION = "convex:manifest";

export interface RustManifestEntry {
  abiVersion: number;
  /** Rust `module_path!()`, e.g. `bench_rust::cpu`. */
  modulePath: string;
  name: string;
  visibility: "public" | "internal";
  /** `"http"` for `#[http_action]` (an `httpAction`); absent or `"action"` for `#[action]`. */
  kind?: "action" | "http";
  args: unknown | null;
  returns: unknown | null;
}

/**
 * Convex module name for a Rust `module_path!()`: the segments after the crate
 * name joined by `/` (`bench_rust::cpu` -> `cpu`, `app::a::b` -> `a/b`). The
 * crate root maps to `lib` (V8HOST.md).
 */
export function convexModuleName(modulePath: string): string {
  const segments = modulePath.split("::");
  if (segments.length < 2) return "lib";
  return segments.slice(1).join("/");
}

/** Parse manifest text (one JSON entry per line) and validate it. */
export function parseManifestText(text: string): RustManifestEntry[] {
  const entries: RustManifestEntry[] = [];
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") continue;
    entries.push(JSON.parse(trimmed) as RustManifestEntry);
  }
  const seen = new Set<string>();
  for (const e of entries) {
    if (e.abiVersion !== 1) {
      throw new Error(`Rust manifest has unsupported abiVersion ${e.abiVersion} (this convex-rust supports 1).`);
    }
    if (e.abiVersion !== entries[0].abiVersion) {
      throw new Error(
        `Rust manifest has mixed abiVersion values (${entries[0].abiVersion} and ${e.abiVersion}).`,
      );
    }
    const key = `${e.modulePath}:${e.name}`;
    if (seen.has(key)) throw new Error(`Rust manifest has duplicate function ${key}.`);
    seen.add(key);
  }
  // Crate-root functions are served as module `lib` on this path (V8HOST.md); a real `crate::lib`
  // module would be the same Convex module, so the two cannot coexist.
  const root = entries.find((e) => !e.modulePath.includes("::"));
  const lib = entries.find((e) => e.modulePath.split("::").length === 2 && e.modulePath.split("::")[1] === "lib");
  if (root && lib) {
    throw new Error(
      `Rust functions in the crate root (${root.modulePath}::${root.name}) and in the module \`lib\` (${lib.modulePath}::${lib.name}) both map to the Convex module \`lib\`; move one of them.`,
    );
  }
  return entries;
}

/** Parse the `convex:manifest` custom section(s) out of wasm bytes. */
export function parseManifest(wasm: Uint8Array): RustManifestEntry[] {
  let module: WebAssembly.Module;
  try {
    module = new WebAssembly.Module(wasm as BufferSource);
  } catch (e: any) {
    throw new Error(`Invalid WebAssembly module: ${e?.message ?? e}`);
  }
  const decoder = new TextDecoder("utf-8");
  const texts: string[] = [];
  for (const section of WebAssembly.Module.customSections(module, MANIFEST_SECTION)) {
    texts.push(decoder.decode(section));
  }
  const entries = parseManifestText(texts.join("\n"));
  if (entries.length === 0) {
    throw new Error(
      "The wasm has no `convex:manifest` entries: does the crate define any #[action] functions?",
    );
  }
  return entries;
}

/** Group manifest entries by Convex module name (e.g. `cpu`, `a/b`). */
export function groupByModule(entries: RustManifestEntry[]): Map<string, RustManifestEntry[]> {
  const grouped = new Map<string, RustManifestEntry[]>();
  for (const e of entries) {
    const name = convexModuleName(e.modulePath);
    const list = grouped.get(name) ?? [];
    list.push(e);
    grouped.set(name, list);
  }
  return grouped;
}

/**
 * Rust symbol demangler: v0 (`_R...`, spec: rust-lang.github.io/rfcs/2603-rust-symbol-name-mangling-v0.html)
 * and legacy (`_ZN...E`). Best effort for display only: generics are shown, lifetimes and const
 * details are simplified, and anything unparsable is returned unchanged.
 */

const BASIC: Record<string, string> = {
  a: "i8", b: "bool", c: "char", d: "f64", e: "str", f: "f32", h: "u8", i: "isize", j: "usize", l: "i32", m: "u32",
  n: "i128", o: "u128", s: "i16", t: "u16", u: "()", v: "...", x: "i64", y: "u64", z: "!", p: "_",
};
const B62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

class Fail extends Error {}

class V0 {
  i = 0;
  depth = 0;
  constructor(readonly s: string) {}
  peek(): string {
    return this.s[this.i] ?? "";
  }
  next(): string {
    if (this.i >= this.s.length) throw new Fail();
    return this.s[this.i++];
  }
  /** `<base-62-number> = {digit | lower | upper} "_"`: "_" is 0, otherwise the value plus one. */
  base62(): number {
    if (this.peek() === "_") {
      this.i++;
      return 0;
    }
    let n = 0;
    for (;;) {
      const c = this.next();
      if (c === "_") return n + 1;
      const d = B62.indexOf(c);
      if (d < 0) throw new Fail();
      n = n * 62 + d;
    }
  }
  /** Optional `s<base62>` disambiguator. */
  disambiguator(): void {
    if (this.peek() === "s") {
      this.i++;
      this.base62();
    }
  }
  ident(): string {
    this.disambiguator();
    if (this.peek() === "u") this.i++; // punycode: shown undecoded
    let len = 0;
    let digits = 0;
    while (/[0-9]/.test(this.peek())) {
      len = len * 10 + Number(this.next());
      digits++;
      if (len === 0) break; // "0" is the empty name (closures)
    }
    if (digits === 0) throw new Fail();
    if (this.peek() === "_") this.i++;
    if (this.i + len > this.s.length) throw new Fail();
    const out = this.s.slice(this.i, this.i + len);
    this.i += len;
    return out;
  }
  backref<T>(f: () => T): T {
    const off = this.base62();
    if (off >= this.i) throw new Fail();
    const saved = this.i;
    this.i = off;
    try {
      return f();
    } finally {
      this.i = saved;
    }
  }
  enter<T>(f: () => T): T {
    if (++this.depth > 120) throw new Fail();
    try {
      return f();
    } finally {
      this.depth--;
    }
  }
  path(): string {
    return this.enter(() => {
      const c = this.next();
      switch (c) {
        case "C":
          return this.ident();
        case "M": {
          this.disambiguator();
          this.path();
          return `<${this.type()}>`;
        }
        case "X": {
          this.disambiguator();
          this.path();
          const ty = this.type();
          return `<${ty} as ${this.path()}>`;
        }
        case "Y": {
          const ty = this.type();
          return `<${ty} as ${this.path()}>`;
        }
        case "N": {
          const ns = this.next();
          const parent = this.path();
          const name = this.ident();
          if (ns === "C") return `${parent}::{closure${name ? `:${name}` : ""}}`;
          if (ns === "S") return `${parent}::{shim${name ? `:${name}` : ""}}`;
          return `${parent}::${name}`;
        }
        case "I": {
          const p = this.path();
          const args: string[] = [];
          while (this.peek() !== "E") {
            const a = this.genericArg();
            if (a !== null) args.push(a);
          }
          this.i++;
          return args.length ? `${p}::<${args.join(", ")}>` : p;
        }
        case "B":
          return this.backref(() => this.path());
        default:
          throw new Fail();
      }
    });
  }
  genericArg(): string | null {
    const c = this.peek();
    if (c === "L") {
      this.i++;
      this.base62();
      return null;
    }
    if (c === "K") {
      this.i++;
      return this.konst();
    }
    return this.type();
  }
  konst(): string {
    const c = this.peek();
    if (c === "p") {
      this.i++;
      return "_";
    }
    if (c === "B") {
      this.i++;
      return this.backref(() => this.konst());
    }
    const ty = this.type();
    let neg = false;
    if (this.peek() === "n") {
      this.i++;
      neg = true;
    }
    let hex = "";
    while (this.peek() !== "_") hex += this.next();
    this.i++;
    if (ty === "bool") return hex === "1" ? "true" : "false";
    return (neg ? "-" : "") + (hex === "" ? "0" : BigInt("0x" + hex).toString());
  }
  lifetime(): void {
    if (this.peek() === "L") {
      this.i++;
      this.base62();
    }
  }
  type(): string {
    return this.enter(() => {
      const c = this.peek();
      if (c >= "a" && c <= "z" && c in BASIC) {
        this.i++;
        return BASIC[c];
      }
      switch (c) {
        case "A": {
          this.i++;
          const t = this.type();
          return `[${t}; ${this.konst()}]`;
        }
        case "S":
          this.i++;
          return `[${this.type()}]`;
        case "T": {
          this.i++;
          const ts: string[] = [];
          while (this.peek() !== "E") ts.push(this.type());
          this.i++;
          return `(${ts.join(", ")}${ts.length === 1 ? "," : ""})`;
        }
        case "R":
        case "Q": {
          this.i++;
          this.lifetime();
          return `&${c === "Q" ? "mut " : ""}${this.type()}`;
        }
        case "P":
        case "O":
          this.i++;
          return `*${c === "O" ? "mut" : "const"} ${this.type()}`;
        case "F": {
          this.i++;
          if (this.peek() === "G") {
            this.i++;
            this.base62();
          }
          if (this.peek() === "U") this.i++;
          if (this.peek() === "K") {
            this.i++;
            if (this.peek() === "C") this.i++;
            else this.ident();
          }
          const ps: string[] = [];
          while (this.peek() !== "E") ps.push(this.type());
          this.i++;
          const ret = this.type();
          return `fn(${ps.join(", ")})${ret === "()" ? "" : ` -> ${ret}`}`;
        }
        case "D": {
          this.i++;
          if (this.peek() === "G") {
            this.i++;
            this.base62();
          }
          const bounds: string[] = [];
          while (this.peek() !== "E") {
            let b = this.path();
            const assoc: string[] = [];
            while (this.peek() === "p") {
              this.i++;
              const n = this.ident();
              assoc.push(`${n} = ${this.type()}`);
            }
            if (assoc.length) b += `<${assoc.join(", ")}>`;
            bounds.push(b);
          }
          this.i++;
          this.lifetime();
          return `dyn ${bounds.join(" + ")}`;
        }
        case "B":
          this.i++;
          return this.backref(() => this.type());
        default:
          return this.path();
      }
    });
  }
}

function demangleV0(name: string): string {
  const m = name.match(/^_R(\d*)/);
  if (!m) return name;
  // Backref offsets count from the start of the symbol after `_R` (and the optional version digits).
  const body = name.slice(m[0].length).replace(/\.[A-Za-z].*$/, "");
  const p = new V0(body);
  try {
    return p.path();
  } catch {
    return name;
  }
}

const LEGACY_ESCAPES: Record<string, string> = {
  SP: "@", BP: "*", RF: "&", LT: "<", GT: ">", LP: "(", RP: ")", C: ",", u20: " ", u27: "'", u5b: "[", u5d: "]", u7b: "{", u7d: "}", u3b: ";", u2b: "+", u22: '"',
};

function demangleLegacy(name: string): string {
  const s = name.replace(/\.(?:llvm|cold|part|isra|constprop)[\w.]*$/, "");
  if (!s.startsWith("_ZN") || !s.endsWith("E")) return name;
  const parts: string[] = [];
  let i = 3;
  const end = s.length - 1;
  while (i < end) {
    let j = i;
    while (j < end && s.charCodeAt(j) >= 48 && s.charCodeAt(j) <= 57) j++;
    if (j === i) return name;
    const len = Number(s.slice(i, j));
    if (j + len > end) return name;
    parts.push(s.slice(j, j + len));
    i = j + len;
  }
  if (parts.length > 1 && /^h[0-9a-f]{16}$/.test(parts[parts.length - 1])) parts.pop();
  return parts
    .map((p) =>
      (p.startsWith("_$") ? p.slice(1) : p)
        .replace(/\.\./g, "::")
        .replace(/\$([A-Za-z0-9]+)\$/g, (m, k: string) => LEGACY_ESCAPES[k] ?? m),
    )
    .join("::");
}

/** Demangled name, or the input when it is not a Rust symbol (or cannot be parsed). */
export function demangle(name: string): string {
  if (name.startsWith("_R")) return demangleV0(name);
  if (name.startsWith("_ZN")) return demangleLegacy(name);
  return name;
}

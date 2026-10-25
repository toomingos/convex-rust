// Dependency-free base64 (the default Convex runtime has no Buffer).
const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const LOOKUP = new Int16Array(128).fill(-1);
for (let i = 0; i < ALPHABET.length; i++) LOOKUP[ALPHABET.charCodeAt(i)] = i;

// V8's built-in Uint8Array base64 (ES2026) when present; the JS code below is the fallback.
const U8: any = typeof Uint8Array !== "undefined" ? Uint8Array : undefined;
const HAS_TO_BASE64 = typeof U8?.prototype?.toBase64 === "function";
const HAS_FROM_BASE64 = typeof U8?.fromBase64 === "function";

export function encodeBase64(bytes: Uint8Array): string {
  if (HAS_TO_BASE64) return (bytes as any).toBase64();
  return encodeBase64Js(bytes);
}

export function encodeBase64Js(bytes: Uint8Array): string {
  const parts: string[] = [];
  const CHUNK = 3 * 8192;
  for (let start = 0; start < bytes.length; start += CHUNK) {
    const end = Math.min(start + CHUNK, bytes.length);
    let out = "";
    let i = start;
    for (; i + 2 < end; i += 3) {
      const n = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2];
      out +=
        ALPHABET[(n >> 18) & 63] +
        ALPHABET[(n >> 12) & 63] +
        ALPHABET[(n >> 6) & 63] +
        ALPHABET[n & 63];
    }
    if (end - i === 1) {
      const n = bytes[i] << 16;
      out += ALPHABET[(n >> 18) & 63] + ALPHABET[(n >> 12) & 63] + "==";
    } else if (end - i === 2) {
      const n = (bytes[i] << 16) | (bytes[i + 1] << 8);
      out +=
        ALPHABET[(n >> 18) & 63] +
        ALPHABET[(n >> 12) & 63] +
        ALPHABET[(n >> 6) & 63] +
        "=";
    }
    parts.push(out);
  }
  return parts.join("");
}

export function decodeBase64(text: string): Uint8Array {
  if (HAS_FROM_BASE64) {
    try {
      // Same leniency as the fallback: ASCII whitespace and trailing padding are ignored.
      return U8.fromBase64(text.replace(/[\s=]+$/g, ""), { lastChunkHandling: "loose" });
    } catch {
      // fall through: the JS path reports the offending character
    }
  }
  return decodeBase64Js(text);
}

export function decodeBase64Js(text: string): Uint8Array {
  const s = text.replace(/[\s=]+$/g, "");
  const out = new Uint8Array(Math.floor((s.length * 3) / 4));
  let acc = 0;
  let bits = 0;
  let o = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    const v = c < 128 ? LOOKUP[c] : -1;
    if (v < 0) throw new Error(`invalid base64 character at ${i}`);
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[o++] = (acc >> bits) & 0xff;
    }
  }
  return out.subarray(0, o);
}

// Shared xorshift32 PRNG — contracts/PRNG.md. Must stay byte-identical with
// bench/convex/rust/src/prng.rs.

export const ALPHABET = "abcdefghijklmnopqrstuvwxyz     .,\n";
const LETTERS = "abcdefghijklmnopqrstuvwxyz";

export class Prng {
  private state: number;

  constructor(seed: number) {
    let s = ((seed >>> 0) ^ 0x9e3779b9) >>> 0;
    if (s === 0) s = 1;
    this.state = s;
  }

  nextU32(): number {
    let x = this.state;
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    this.state = x;
    return x;
  }

  bytes(n: number): Uint8Array {
    const out = new Uint8Array(n);
    let i = 0;
    while (i < n) {
      const v = this.nextU32();
      for (let k = 0; k < 4 && i < n; k++, i++) out[i] = (v >>> (8 * k)) & 0xff;
    }
    return out;
  }

  f64(): number {
    return this.nextU32() / 4294967296;
  }

  below(n: number): number {
    return this.nextU32() % n;
  }

  text(n: number): string {
    let s = "";
    for (let i = 0; i < n; i++) s += ALPHABET[this.below(ALPHABET.length)];
    return s;
  }

  word(len: number): string {
    let s = "";
    for (let i = 0; i < len; i++) s += LETTERS[this.below(26)];
    return s;
  }
}

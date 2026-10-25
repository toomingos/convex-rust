# Shared PRNG (frozen at W0-13)

Both sides of every case generate inputs from `args.seed` with this exact
xorshift32 generator, so inputs are byte-identical. Implementations:
`bench/shared/prng.ts` and `bench/convex/rust/src/prng.rs`. Don't reimplement
it in category files; import it.

```
state  = (seed ^ 0x9E3779B9) as u32;  if state == 0 { state = 1 }
next_u32():  x = state; x ^= x << 13; x ^= x >> 17; x ^= x << 5; state = x; return x   (all u32, wrapping)
bytes(n):    next_u32() per 4 bytes, little-endian, last word truncated
f64():       next_u32() / 2^32                     in [0, 1)
below(n):    next_u32() % n                        (n <= 2^32)
text(n):     n chars from ALPHABET[below(ALPHABET.len)], ALPHABET = "abcdefghijklmnopqrstuvwxyz     .,\n" (34 chars, ASCII)
word(len):   len chars from "abcdefghijklmnopqrstuvwxyz"[below(26)]
```

Test vector (seed 1): first five `next_u32()` = see `bench/shared/prng.test.ts`
(both implementations assert the same constants).

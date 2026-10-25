//! Shared xorshift32 PRNG — contracts/PRNG.md. Must stay byte-identical with
//! bench/shared/prng.ts.

pub const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz     .,\n";
const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

pub struct Prng {
    state: u32,
}

impl Prng {
    pub fn new(seed: u32) -> Self {
        let s = seed ^ 0x9E37_79B9;
        Self {
            state: if s == 0 { 1 } else { s },
        }
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 3);
        while out.len() < n {
            out.extend_from_slice(&self.next_u32().to_le_bytes());
        }
        out.truncate(n);
        out
    }

    pub fn f64(&mut self) -> f64 {
        f64::from(self.next_u32()) / 4_294_967_296.0
    }

    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    pub fn text(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| ALPHABET[self.below(ALPHABET.len() as u32) as usize] as char)
            .collect()
    }

    pub fn word(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| LETTERS[self.below(26) as usize] as char)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same vector as bench/shared/prng.test.ts.
    pub const SEED1_FIRST5: [u32; 5] = VECTOR;
    include!("prng_vector.rs");

    #[test]
    fn matches_shared_vector() {
        let mut p = Prng::new(1);
        let got: Vec<u32> = (0..5).map(|_| p.next_u32()).collect();
        assert_eq!(got, SEED1_FIRST5);
    }
}

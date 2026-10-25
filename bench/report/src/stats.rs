//! Quantiles, seeded RNG and the percentile bootstrap.

/// splitmix64: tiny, fast, deterministic, good enough for resampling.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }
    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = 1.0 - self.f64();
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// FNV-1a over the parts, mixed with `seed`. Order-independent seeding per (case, mode...).
pub fn derive_seed(seed: u64, parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0xff)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Linear-interpolation quantile (R type 7) of an ascending slice. NaN when empty.
pub fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    match sorted.len() {
        0 => f64::NAN,
        1 => sorted[0],
        n => {
            let pos = q.clamp(0.0, 1.0) * (n - 1) as f64;
            let lo = pos.floor() as usize;
            let hi = pos.ceil() as usize;
            sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
        }
    }
}

/// Quantile of unsorted data.
pub fn quantile(data: &[f64], q: f64) -> f64 {
    let mut v = data.to_vec();
    v.sort_by(f64::total_cmp);
    quantile_sorted(&v, q)
}

/// Median, reordering `buf` (O(n)).
pub fn median_in_place(buf: &mut [f64]) -> f64 {
    let n = buf.len();
    if n == 0 {
        return f64::NAN;
    }
    let mid = n / 2;
    let (left, hi, _) = buf.select_nth_unstable_by(mid, f64::total_cmp);
    let hi = *hi;
    if n % 2 == 1 {
        hi
    } else {
        let lo = left.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lo + hi) / 2.0
    }
}

pub fn mean(data: &[f64]) -> f64 {
    data.iter().sum::<f64>() / data.len() as f64
}

pub fn median(data: &[f64]) -> f64 {
    median_in_place(&mut data.to_vec())
}

pub struct Boot {
    /// median(num) / median(den)
    pub point: f64,
    pub lo: f64,
    pub hi: f64,
    /// ln(ratio) of every resample, in resample order.
    pub logs: Vec<f64>,
}

/// Percentile bootstrap (95%) of the ratio of medians `median(num)/median(den)`.
/// Samples must be positive. `num` and `den` are resampled independently.
pub fn bootstrap_ratio(num: &[f64], den: &[f64], resamples: usize, seed: u64) -> Boot {
    let point = median(num) / median(den);
    let mut rng = Rng::new(seed);
    let mut bn = vec![0.0; num.len()];
    let mut bd = vec![0.0; den.len()];
    let mut logs = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        for x in &mut bn {
            *x = num[rng.below(num.len())];
        }
        for x in &mut bd {
            *x = den[rng.below(den.len())];
        }
        logs.push((median_in_place(&mut bn) / median_in_place(&mut bd)).ln());
    }
    let mut s = logs.clone();
    s.sort_by(f64::total_cmp);
    Boot {
        point,
        lo: quantile_sorted(&s, 0.025).exp(),
        hi: quantile_sorted(&s, 0.975).exp(),
        logs,
    }
}

/// Geometric mean of per-item ratios with a bootstrap CI. `items` are
/// `(ln point ratio, per-resample ln ratios)`; all `logs` must have equal length.
pub fn geomean_ci(items: &[(f64, &[f64])]) -> Option<(f64, f64, f64)> {
    let first = items.first()?;
    let b = first.1.len();
    let n = items.len() as f64;
    let point = (items.iter().map(|i| i.0).sum::<f64>() / n).exp();
    let mut means: Vec<f64> = (0..b)
        .map(|k| items.iter().map(|i| i.1[k]).sum::<f64>() / n)
        .collect();
    means.sort_by(f64::total_cmp);
    Some((
        point,
        quantile_sorted(&means, 0.025).exp(),
        quantile_sorted(&means, 0.975).exp(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles() {
        let d = [4.0, 1.0, 3.0, 2.0, 5.0];
        assert_eq!(quantile(&d, 0.0), 1.0);
        assert_eq!(quantile(&d, 0.5), 3.0);
        assert_eq!(quantile(&d, 1.0), 5.0);
        assert!((quantile(&d, 0.25) - 2.0).abs() < 1e-12);
        let e = [1.0, 2.0, 3.0, 4.0];
        assert!((quantile(&e, 0.5) - 2.5).abs() < 1e-12);
        assert!((quantile(&e, 0.95) - 3.85).abs() < 1e-12);
        assert!(quantile(&[], 0.5).is_nan());
        assert_eq!(quantile(&[7.0], 0.99), 7.0);
    }

    #[test]
    fn medians_match_quantile() {
        let mut rng = Rng::new(3);
        for n in 1..40 {
            let v: Vec<f64> = (0..n).map(|_| rng.f64()).collect();
            assert!((median(&v) - quantile(&v, 0.5)).abs() < 1e-12, "n={n}");
        }
    }

    #[test]
    fn rng_deterministic_and_uniformish() {
        let mut a = Rng::new(9);
        let mut b = Rng::new(9);
        assert_eq!(a.next_u64(), b.next_u64());
        let mut counts = [0usize; 10];
        for _ in 0..100_000 {
            counts[a.below(10)] += 1;
        }
        assert!(counts.iter().all(|&c| (9_000..11_000).contains(&c)));
    }

    #[test]
    fn bootstrap_deterministic() {
        let a: Vec<f64> = (1..=50).map(f64::from).collect();
        let b: Vec<f64> = (1..=50).map(|x| f64::from(x) * 2.0).collect();
        let x = bootstrap_ratio(&a, &b, 500, 5);
        let y = bootstrap_ratio(&a, &b, 500, 5);
        assert_eq!((x.lo, x.hi, x.point), (y.lo, y.hi, y.point));
        assert!(x.lo < 0.5 && 0.5 < x.hi, "{} {}", x.lo, x.hi);
    }

    /// Sanity coverage: two lognormal samples with a true median ratio of 1.5; the 95% CI
    /// should contain 1.5 in roughly 88-99% of repeated experiments.
    #[test]
    fn bootstrap_coverage() {
        let trials = 200;
        let mut covered = 0;
        for t in 0..trials {
            let mut rng = Rng::new(1000 + t);
            let num: Vec<f64> = (0..60).map(|_| (1.5f64.ln() + 0.4 * rng.normal()).exp()).collect();
            let den: Vec<f64> = (0..60).map(|_| (0.4 * rng.normal()).exp()).collect();
            let b = bootstrap_ratio(&num, &den, 600, 77 + t);
            if b.lo <= 1.5 && 1.5 <= b.hi {
                covered += 1;
            }
        }
        let cov = f64::from(covered) / trials as f64;
        assert!((0.88..=0.995).contains(&cov), "coverage {cov}");
    }

    #[test]
    fn identical_distributions_cross_one_and_shift_does_not() {
        let mut rng = Rng::new(42);
        let a: Vec<f64> = (0..200).map(|_| (0.3 * rng.normal()).exp()).collect();
        let b: Vec<f64> = (0..200).map(|_| (0.3 * rng.normal()).exp()).collect();
        let same = bootstrap_ratio(&a, &b, 1000, 1);
        assert!(same.lo < 1.0 && same.hi > 1.0);
        let shifted: Vec<f64> = b.iter().map(|x| x * 1.5).collect();
        let diff = bootstrap_ratio(&shifted, &a, 1000, 1);
        assert!(diff.lo > 1.0);
    }

    #[test]
    fn geomean() {
        let l1 = vec![2f64.ln(); 100];
        let l2 = vec![0.5f64.ln(); 100];
        let (g, lo, hi) = geomean_ci(&[(2f64.ln(), &l1), (0.5f64.ln(), &l2)]).unwrap();
        assert!((g - 1.0).abs() < 1e-12 && (lo - 1.0).abs() < 1e-12 && (hi - 1.0).abs() < 1e-12);
        assert!(geomean_ci(&[]).is_none());
    }
}

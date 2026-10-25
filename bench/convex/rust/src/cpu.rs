//! Pure CPU cases (cpu-*).

// --- cpu-001..010 (W1-03) ---
use crate::prng::Prng;
use convex_fn::{action, Ctx, Result};
use md5::Md5;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

const TEN_MB: usize = 10 * 1024 * 1024;

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

/// SHA-2 of `data`: the host's `crypto/digest` on wasm32 (no in-guest fallback), `sha2` natively.
#[cfg(target_arch = "wasm32")]
fn sha2_hex(alg: &str, data: &[u8]) -> Result<String> {
    Ok(hex(&convex_fn::crypto::digest(alg, data)?))
}

#[cfg(not(target_arch = "wasm32"))]
fn sha2_hex(alg: &str, data: &[u8]) -> Result<String> {
    use sha2::Digest;
    Ok(match alg {
        "SHA-256" => hex(&sha2::Sha256::digest(data)),
        _ => hex(&sha2::Sha512::digest(data)),
    })
}

/// cpu-001: sha256 of 10 MB.
#[action]
pub async fn cpu_001(_ctx: &Ctx, args: SeedArgs) -> Result<String> {
    let data = Prng::new(args.seed).bytes(TEN_MB);
    sha2_hex("SHA-256", &data)
}

/// cpu-002: sha512 of 10 MB.
#[action]
pub async fn cpu_002(_ctx: &Ctx, args: SeedArgs) -> Result<String> {
    let data = Prng::new(args.seed).bytes(TEN_MB);
    sha2_hex("SHA-512", &data)
}

/// cpu-003: blake3 of 10 MB.
#[action]
pub async fn cpu_003(_ctx: &Ctx, args: SeedArgs) -> Result<String> {
    let data = Prng::new(args.seed).bytes(TEN_MB);
    Ok(blake3::hash(&data).to_hex().to_string())
}

/// cpu-004: md5 of 10 MB.
#[action]
pub async fn cpu_004(_ctx: &Ctx, args: SeedArgs) -> Result<String> {
    let data = Prng::new(args.seed).bytes(TEN_MB);
    {
        use md5::Digest;
        Ok(hex(&Md5::digest(&data)))
    }
}

/// cpu-005: crc32 of 10 MB.
#[action]
pub async fn cpu_005(_ctx: &Ctx, args: SeedArgs) -> Result<u32> {
    let data = Prng::new(args.seed).bytes(TEN_MB);
    Ok(crc32fast::hash(&data))
}

fn fib(n: u32) -> u64 {
    if n < 2 {
        u64::from(n)
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

/// cpu-006: recursive fib(30).
#[action]
pub async fn cpu_006(_ctx: &Ctx, _args: SeedArgs) -> Result<u64> {
    Ok(fib(std::hint::black_box(30)))
}

#[derive(Serialize)]
pub struct Sieve {
    pub count: u32,
    pub last: u32,
}

/// cpu-007: prime sieve to 10M.
#[action]
pub async fn cpu_007(_ctx: &Ctx, _args: SeedArgs) -> Result<Sieve> {
    let n = 10_000_000usize;
    let mut composite = vec![false; n + 1];
    let mut i = 2;
    while i * i <= n {
        if !composite[i] {
            let mut j = i * i;
            while j <= n {
                composite[j] = true;
                j += i;
            }
        }
        i += 1;
    }
    let (mut count, mut last) = (0u32, 0u32);
    for (i, &c) in composite.iter().enumerate().skip(2) {
        if !c {
            count += 1;
            last = i as u32;
        }
    }
    Ok(Sieve { count, last })
}

#[derive(Serialize)]
pub struct IntSort {
    pub min: i32,
    pub median: i32,
    pub max: i32,
    pub sample: Vec<i32>,
    pub sum: i64,
}

/// cpu-008: sort 1M ints.
#[action]
pub async fn cpu_008(_ctx: &Ctx, args: SeedArgs) -> Result<IntSort> {
    let n = 1_000_000usize;
    let mut rng = Prng::new(args.seed);
    let mut a: Vec<i32> = (0..n).map(|_| rng.next_u32() as i32).collect();
    a.sort_unstable();
    let sum: i64 = a.iter().map(|&x| i64::from(x)).sum();
    let sample = [0, 1, 1000, 250_000, 500_000, 750_000, 999_999].iter().map(|&i| a[i]).collect();
    Ok(IntSort { min: a[0], median: a[n >> 1], max: a[n - 1], sample, sum })
}

#[derive(Serialize)]
pub struct StrSort {
    pub first: String,
    pub median: String,
    pub last: String,
    pub unique: u32,
    pub sample: Vec<String>,
}

/// cpu-009: sort 100k strings (ASCII lowercase: byte order == UTF-16 order).
#[action]
pub async fn cpu_009(_ctx: &Ctx, args: SeedArgs) -> Result<StrSort> {
    let n = 100_000usize;
    let mut rng = Prng::new(args.seed);
    let mut words: Vec<String> = Vec::with_capacity(n);
    for _ in 0..n {
        let len = 4 + rng.below(13) as usize;
        words.push(rng.word(len));
    }
    words.sort();
    let unique = 1 + words.windows(2).filter(|w| w[0] != w[1]).count() as u32;
    let sample = [1, 1000, 25_000, 50_000, 75_000, 99_998].iter().map(|&i| words[i].clone()).collect();
    Ok(StrSort {
        first: words[0].clone(),
        median: words[n >> 1].clone(),
        last: words[n - 1].clone(),
        unique,
        sample,
    })
}

#[derive(Serialize)]
pub struct MatMul {
    pub cells: Vec<f64>,
    pub sum: f64,
}

/// cpu-010: 256x256 f64 matrix multiply (naive ijk, same order as Node).
#[action]
pub async fn cpu_010(_ctx: &Ctx, args: SeedArgs) -> Result<MatMul> {
    let n = 256usize;
    let mut rng = Prng::new(args.seed);
    let a: Vec<f64> = (0..n * n).map(|_| rng.f64()).collect();
    let b: Vec<f64> = (0..n * n).map(|_| rng.f64()).collect();
    let mut c = vec![0f64; n * n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0f64;
            for k in 0..n {
                s += a[i * n + k] * b[k * n + j];
            }
            c[i * n + j] = s;
        }
    }
    let sum: f64 = c.iter().sum();
    let cells = vec![c[0], c[n - 1], c[(n / 2) * n + n / 2], c[(n - 1) * n], c[n * n - 1]];
    Ok(MatMul { cells, sum })
}

// --- cpu-011..020 (W1-04) ---
use std::io::{Read, Write};

use base64::Engine as _;
use serde_json::Value;

use crate::prng::ALPHABET;

#[derive(Deserialize)]
pub struct SeedArgsB {
    pub seed: u32,
}

const MB10: usize = 10_000_000;
const RECORDS: usize = 100_000;

/// FNV-1a 32-bit over bytes.
fn fnv1a(buf: &[u8]) -> u32 {
    buf.iter().fold(0x811c_9dc5_u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// `Prng::text(n)` as ASCII bytes.
fn text_bytes(p: &mut Prng, n: usize) -> Vec<u8> {
    (0..n).map(|_| ALPHABET[p.below(ALPHABET.len() as u32) as usize]).collect()
}

fn nbody_energy(pos: &[[f64; 3]], vel: &[[f64; 3]], mass: &[f64]) -> f64 {
    let n = mass.len();
    let mut e = 0.0;
    for i in 0..n {
        let [vx, vy, vz] = vel[i];
        e += 0.5 * mass[i] * (vx * vx + vy * vy + vz * vz);
        for j in (i + 1)..n {
            let dx = pos[i][0] - pos[j][0];
            let dy = pos[i][1] - pos[j][1];
            let dz = pos[i][2] - pos[j][2];
            e -= (mass[i] * mass[j]) / (dx * dx + dy * dy + dz * dz).sqrt();
        }
    }
    e
}

/// cpu-011: n-body, 1000 steps (5 bodies, energy before/after).
#[action]
pub async fn cpu_011(_ctx: &Ctx, args: SeedArgsB) -> Result<Vec<f64>> {
    let mut p = Prng::new(args.seed);
    let n = 5;
    let mut pos = Vec::with_capacity(n);
    let mut vel = Vec::with_capacity(n);
    let mut mass = Vec::with_capacity(n);
    for _ in 0..n {
        pos.push([(p.f64() - 0.5) * 20.0, (p.f64() - 0.5) * 20.0, (p.f64() - 0.5) * 20.0]);
        vel.push([p.f64() - 0.5, p.f64() - 0.5, p.f64() - 0.5]);
        mass.push(0.5 + p.f64());
    }
    let e0 = nbody_energy(&pos, &vel, &mass);
    let dt = 0.001_f64;
    for _ in 0..1000 {
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[i][0] - pos[j][0];
                let dy = pos[i][1] - pos[j][1];
                let dz = pos[i][2] - pos[j][2];
                let d2 = dx * dx + dy * dy + dz * dz;
                let mag = dt / (d2 * d2.sqrt());
                vel[i][0] -= dx * mass[j] * mag;
                vel[i][1] -= dy * mass[j] * mag;
                vel[i][2] -= dz * mass[j] * mag;
                vel[j][0] += dx * mass[i] * mag;
                vel[j][1] += dy * mass[i] * mag;
                vel[j][2] += dz * mass[i] * mag;
            }
        }
        for i in 0..n {
            for k in 0..3 {
                pos[i][k] += dt * vel[i][k];
            }
        }
    }
    Ok(vec![e0, nbody_energy(&pos, &vel, &mass)])
}

/// cpu-012: mandelbrot 1000x1000, 100 max iterations, iteration-count checksum.
#[action]
pub async fn cpu_012(_ctx: &Ctx, _args: SeedArgsB) -> Result<u64> {
    // black_box: keep the parameters opaque so LLVM cannot precompute the loop.
    let size = std::hint::black_box(1000_u32);
    let max_iter = std::hint::black_box(100_u32);
    let s = f64::from(size);
    let mut sum = 0_u64;
    for py in 0..size {
        let ci = (f64::from(py) / s) * 2.0 - 1.0;
        for px in 0..size {
            let cr = (f64::from(px) / s) * 3.5 - 2.5;
            let (mut zr, mut zi, mut it) = (0.0_f64, 0.0_f64, 0_u32);
            while it < max_iter && zr * zr + zi * zi <= 4.0 {
                let t = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
                it += 1;
            }
            sum += u64::from(it);
        }
    }
    Ok(sum)
}

#[derive(Serialize)]
pub struct RegexOut {
    pub count: u64,
    pub chars: u64,
}

/// cpu-013: regex scan over 10 MB of PRNG text.
#[action]
pub async fn cpu_013(_ctx: &Ctx, args: SeedArgsB) -> Result<RegexOut> {
    let text = String::from_utf8(text_bytes(&mut Prng::new(args.seed), MB10)).expect("ascii");
    let re = regex::Regex::new(r"\b[a-z]{4,6}\b").expect("valid regex");
    let (mut count, mut chars) = (0_u64, 0_u64);
    for m in re.find_iter(&text) {
        count += 1;
        chars += m.len() as u64;
    }
    Ok(RegexOut { count, chars })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseOut {
    pub len: u64,
    pub records: u64,
    pub score_sum: u64,
    pub name_chars: u64,
    pub tag_count: u64,
    pub active_count: u64,
}

/// cpu-014: JSON parse ~10 MB (document built by string template from the PRNG).
#[action]
pub async fn cpu_014(_ctx: &Ctx, args: SeedArgsB) -> Result<ParseOut> {
    use std::fmt::Write as _;
    let mut p = Prng::new(args.seed);
    let mut text = String::with_capacity(MB10);
    text.push('[');
    for i in 0..RECORDS {
        if i > 0 {
            text.push(',');
        }
        let name = p.word(8);
        let score = p.below(1_000_000);
        let (t0, t1, t2) = (p.word(6), p.word(6), p.word(6));
        let active = p.below(2) == 1;
        write!(
            text,
            r#"{{"id":{i},"name":"{name}","score":{score},"tags":["{t0}","{t1}","{t2}"],"active":{active}}}"#
        )
        .expect("string write");
    }
    text.push(']');
    let docs: Vec<Value> = serde_json::from_str(&text)?;
    let mut out = ParseOut {
        len: text.len() as u64,
        records: docs.len() as u64,
        score_sum: 0,
        name_chars: 0,
        tag_count: 0,
        active_count: 0,
    };
    for d in &docs {
        out.score_sum += d["score"].as_u64().unwrap_or(0);
        out.name_chars += d["name"].as_str().map_or(0, str::len) as u64;
        out.tag_count += d["tags"].as_array().map_or(0, Vec::len) as u64;
        if d["active"].as_bool() == Some(true) {
            out.active_count += 1;
        }
    }
    Ok(out)
}

#[derive(Serialize)]
struct Doc {
    id: usize,
    name: String,
    score: u32,
    tags: [String; 3],
    active: bool,
}

#[derive(Serialize)]
pub struct HashOut {
    pub len: u64,
    pub hash: u32,
}

/// cpu-015: JSON stringify ~10 MB.
#[action]
pub async fn cpu_015(_ctx: &Ctx, args: SeedArgsB) -> Result<HashOut> {
    let mut p = Prng::new(args.seed);
    let mut docs = Vec::with_capacity(RECORDS);
    for id in 0..RECORDS {
        docs.push(Doc {
            id,
            name: p.word(8),
            score: p.below(1_000_000),
            tags: [p.word(6), p.word(6), p.word(6)],
            active: p.below(2) == 1,
        });
    }
    let bytes = serde_json::to_vec(&docs)?;
    Ok(HashOut { len: bytes.len() as u64, hash: fnv1a(&bytes) })
}

#[derive(Serialize)]
pub struct B64Out {
    pub len: u64,
    pub head: String,
    pub mid: String,
    pub tail: String,
}

/// cpu-016: base64 encode 10 MB.
#[action]
pub async fn cpu_016(_ctx: &Ctx, args: SeedArgsB) -> Result<B64Out> {
    let data = Prng::new(args.seed).bytes(MB10);
    let s = base64::engine::general_purpose::STANDARD.encode(&data);
    let m = s.len() >> 1;
    Ok(B64Out {
        len: s.len() as u64,
        head: s[..32].to_string(),
        mid: s[m..m + 32].to_string(),
        tail: s[s.len() - 32..].to_string(),
    })
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// cpu-017: base64 decode 10 MB of base64 text (generated from the PRNG).
#[action]
pub async fn cpu_017(_ctx: &Ctx, args: SeedArgsB) -> Result<HashOut> {
    let mut p = Prng::new(args.seed);
    let enc: Vec<u8> = (0..MB10).map(|_| B64[p.below(64) as usize]).collect();
    let out = base64::engine::general_purpose::STANDARD.decode(&enc)?;
    Ok(HashOut { len: out.len() as u64, hash: fnv1a(&out) })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressOut {
    pub in_len: u64,
    pub out_len: u64,
    pub roundtrip: bool,
}

/// cpu-018: gzip compress 10 MB of PRNG text (level 6), roundtrip check.
#[action]
pub async fn cpu_018(_ctx: &Ctx, args: SeedArgsB) -> Result<CompressOut> {
    let data = text_bytes(&mut Prng::new(args.seed), MB10);
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(&data)?;
    let z = enc.finish()?;
    let mut back = Vec::with_capacity(data.len());
    flate2::read::GzDecoder::new(&z[..]).read_to_end(&mut back)?;
    Ok(CompressOut { in_len: data.len() as u64, out_len: z.len() as u64, roundtrip: back == data })
}

/// cpu-019: zstd compress 10 MB of PRNG text, roundtrip check. ruzstd only implements Fastest (~level 1).
/// Uses `ruzstd` (pure Rust): the C `zstd` crate does not build for wasm32-wasip1 here.
#[action]
pub async fn cpu_019(_ctx: &Ctx, args: SeedArgsB) -> Result<CompressOut> {
    let data = text_bytes(&mut Prng::new(args.seed), MB10);
    let z = ruzstd::encoding::compress_to_vec(&data[..], ruzstd::encoding::CompressionLevel::Fastest);
    let mut back = Vec::with_capacity(data.len());
    ruzstd::decoding::StreamingDecoder::new(&z[..])
        .map_err(|e| convex_fn::Error::msg(e.to_string()))?
        .read_to_end(&mut back)?;
    Ok(CompressOut { in_len: data.len() as u64, out_len: z.len() as u64, roundtrip: back == data })
}

#[derive(Serialize)]
pub struct LevOut {
    pub sum: u64,
    pub first: u64,
    pub max: u64,
}

/// cpu-020: Levenshtein over 1k random lowercase string pairs (50..149 chars).
#[action]
pub async fn cpu_020(_ctx: &Ctx, args: SeedArgsB) -> Result<LevOut> {
    let mut p = Prng::new(args.seed);
    let mut out = LevOut { sum: 0, first: 0, max: 0 };
    for i in 0..1000 {
        let la = 50 + p.below(100) as usize;
        let a = p.word(la);
        let lb = 50 + p.below(100) as usize;
        let b = p.word(lb);
        let d = strsim::levenshtein(&a, &b) as u64;
        if i == 0 {
            out.first = d;
        }
        out.sum += d;
        out.max = out.max.max(d);
    }
    Ok(out)
}

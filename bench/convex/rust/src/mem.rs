//! Memory and concurrency cases (mem-*).

use std::collections::HashMap;

use convex_fn::{action, join_all, Ctx, Result};
use serde::{Deserialize, Serialize};

use crate::prng::Prng;

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

/// mem-001: allocate 100 MiB, write every byte, checksum every 4096th byte.
#[action]
pub async fn mem_001(_ctx: &Ctx, args: SeedArgs) -> Result<u64> {
    let n = 100 * 1024 * 1024usize;
    let mut buf = vec![0u8; n];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = ((i as u32).wrapping_mul(31).wrapping_add(args.seed) & 255) as u8;
    }
    let sum: u64 = buf.iter().step_by(4096).map(|&b| u64::from(b)).sum();
    Ok(std::hint::black_box(sum))
}

/// mem-002: 1M small heap allocations (4-element u32 boxes), all kept alive.
#[action]
pub async fn mem_002(_ctx: &Ctx, args: SeedArgs) -> Result<u64> {
    let n = 1_000_000u32;
    let mut items: Vec<Box<[u32; 4]>> = Vec::with_capacity(n as usize);
    for i in 0..n {
        items.push(Box::new([
            i,
            i ^ args.seed,
            i.wrapping_mul(3),
            i.wrapping_add(args.seed),
        ]));
    }
    let mut sum = 0u64;
    for (i, it) in items.iter().enumerate() {
        sum = (sum + u64::from(it[i & 3])) % 4_294_967_296;
    }
    Ok(sum)
}

/// mem-003: fan out 100 tasks that each sleep 1-5 ms, join them.
#[action]
pub async fn mem_003(ctx: &Ctx, args: SeedArgs) -> Result<Vec<u64>> {
    let seed = u64::from(args.seed);
    let tasks = (0..100u64).map(|i| async move {
        ctx.sleep(1 + (i % 5)).await?;
        Ok::<u64, convex_fn::Error>(i * i + seed)
    });
    join_all(tasks).await.into_iter().collect()
}

#[derive(Serialize)]
pub struct MapOut {
    pub size: usize,
    pub sum: u64,
}

/// mem-004: insert 1M PRNG keys into a hash map, then look all of them up.
#[action]
pub async fn mem_004(_ctx: &Ctx, args: SeedArgs) -> Result<MapOut> {
    let n = 1_000_000u64;
    let mut m: HashMap<u32, u64> = HashMap::new();
    let mut p = Prng::new(args.seed);
    for i in 0..n {
        m.insert(p.next_u32(), i);
    }
    let mut q = Prng::new(args.seed);
    let mut sum = 0u64;
    for _ in 0..n {
        sum += m[&q.next_u32()];
    }
    Ok(MapOut { size: m.len(), sum })
}

#[derive(Serialize)]
pub struct StrOut {
    pub len: usize,
    pub hash: u32,
}

/// mem-005: build a 10 MiB string by repeated concatenation of 10-char pieces.
#[action]
pub async fn mem_005(_ctx: &Ctx, args: SeedArgs) -> Result<StrOut> {
    let mut p = Prng::new(args.seed);
    let words: Vec<String> = (0..16).map(|_| p.word(10)).collect();
    let mut s = String::new();
    for i in 0..1024 * 1024u32 {
        s.push_str(&words[(i.wrapping_add(args.seed) & 15) as usize]);
    }
    let mut h = 0u32;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(b));
    }
    Ok(StrOut { len: s.len(), hash: h })
}

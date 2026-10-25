// Fixture for packages/convex-rust/test/host_rust.test.ts: std::fs on the in-memory /tmp, Intl helpers, fan-out.
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};

use convex_fn::{action, intl, json, par_map, Ctx, Result, Value};

fn e(err: impl std::fmt::Display) -> convex_fn::Error {
    convex_fn::Error::msg(err.to_string())
}

#[action]
pub async fn fs_roundtrip(_ctx: &Ctx, _a: Value) -> Result<Value> {
    let tmp = convex_fn::fs::temp_dir();
    let dir = tmp.join("work");
    fs::create_dir_all(dir.join("sub/deep")).map_err(e)?;
    let file = dir.join("a.txt");
    fs::write(&file, b"hello world").map_err(e)?;
    let read = fs::read_to_string(&file).map_err(e)?;

    // append + seek + tell
    let mut f = fs::OpenOptions::new().append(true).open(&file).map_err(e)?;
    f.write_all(b"!!").map_err(e)?;
    drop(f);
    let mut f = fs::File::open(&file).map_err(e)?;
    f.seek(SeekFrom::Start(6)).map_err(e)?;
    let mut tail = String::new();
    f.read_to_string(&mut tail).map_err(e)?;
    let pos = f.stream_position().map_err(e)?;
    drop(f);

    // read/write file, set_len
    let mut rw = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(dir.join("rw.bin"))
        .map_err(e)?;
    rw.write_all(&[1, 2, 3, 4, 5]).map_err(e)?;
    rw.set_len(8).map_err(e)?;
    rw.seek(SeekFrom::Start(0)).map_err(e)?;
    let mut all = Vec::new();
    rw.read_to_end(&mut all).map_err(e)?;
    drop(rw);

    let meta = fs::metadata(&file).map_err(e)?;
    fs::write(dir.join("sub/b.txt"), "b").map_err(e)?;
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map_err(e)?
        .map(|x| x.map(|x| x.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()
        .map_err(e)?;
    names.sort();

    fs::rename(&file, dir.join("sub/moved.txt")).map_err(e)?;
    let moved = fs::read_to_string(dir.join("sub/moved.txt")).map_err(e)?;
    let old_gone = fs::metadata(&file).is_err();
    let not_empty = fs::remove_dir(dir.join("sub")).is_err();
    fs::remove_file(dir.join("sub/moved.txt")).map_err(e)?;
    let missing = fs::read(dir.join("nope")).unwrap_err().kind() == std::io::ErrorKind::NotFound;
    let exists = fs::create_dir(&dir).unwrap_err().kind() == std::io::ErrorKind::AlreadyExists;
    fs::remove_dir_all(&dir).map_err(e)?;
    let after = fs::read_dir(&tmp).map_err(e)?.count();
    Ok(json!({
        "tmp": tmp.to_string_lossy(), "read": read, "tail": tail, "pos": pos, "all": all,
        "len": meta.len(), "isFile": meta.is_file(), "names": names, "moved": moved, "oldGone": old_gone,
        "notEmpty": not_empty, "missing": missing, "exists": exists, "after": after,
    }))
}

/// Writes `mb` MiB in 1 MiB pieces and reads back a checksum; reports the error kind on failure.
#[action]
pub async fn fs_big(_ctx: &Ctx, a: Value) -> Result<Value> {
    let mb = a["mb"].as_u64().unwrap_or(1) as usize;
    let mut f = fs::File::create("/tmp/big.bin").map_err(e)?;
    let chunk = vec![7u8; 1 << 20];
    for i in 0..mb {
        if let Err(err) = f.write_all(&chunk) {
            return Ok(json!({ "failedAtMb": i, "kind": format!("{:?}", err.kind()), "raw": err.raw_os_error() }));
        }
    }
    drop(f);
    let data = fs::read("/tmp/big.bin").map_err(e)?;
    let sum: u64 = data.iter().map(|b| *b as u64).sum();
    fs::remove_file("/tmp/big.bin").map_err(e)?;
    Ok(json!({ "len": data.len(), "sum": sum }))
}

/// Lists /tmp (must start empty on every call) and leaves a file behind.
#[action]
pub async fn fs_leave(_ctx: &Ctx, _a: Value) -> Result<Value> {
    let before: Vec<String> = fs::read_dir("/tmp")
        .map_err(e)?
        .map(|x| x.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    fs::write("/tmp/left.txt", vec![9u8; 100_000]).map_err(e)?;
    Ok(json!({ "before": before }))
}

#[action]
pub async fn tz(_ctx: &Ctx, a: Value) -> Result<Value> {
    let ms = a["ms"].as_f64().unwrap_or(0.0);
    let zone = a["tz"].as_str().unwrap_or("UTC");
    let off = intl::tz_offset_secs(zone, ms);
    Ok(json!({
        "off": off.as_ref().ok(),
        "err": off.as_ref().err().map(|x| x.message().to_string()),
        "fmt": intl::format_date(ms, "en-GB", json!({ "timeZone": zone, "dateStyle": "full", "timeStyle": "long" })).ok(),
        "num": intl::format_number(1234567.891, "de-DE", json!({ "style": "currency", "currency": "EUR" })).ok(),
        "local": intl::local_to_epoch_ms(zone, a["local"].as_f64().unwrap_or(0.0)).ok(),
    }))
}

#[action]
pub async fn fanout(ctx: &Ctx, a: Value) -> Result<Value> {
    let n = a["n"].as_u64().unwrap_or(6);
    let limit = a["limit"].as_u64().unwrap_or(2) as usize;
    let out = par_map(0..n, limit, |i| async move {
        ctx.sleep(20).await?;
        Ok::<u64, convex_fn::Error>(i * 10)
    })
    .await;
    let vals: Vec<u64> = out.into_iter().collect::<Result<_>>()?;
    let args: Vec<Value> = (0..n).map(|i| json!({ "x": i })).collect();
    let via: Vec<f64> = ctx.run_action_each("w:double", args, limit).await?;
    Ok(json!({ "vals": vals, "via": via }))
}

fn get(url: &str) -> convex_fn::FetchRequest {
    convex_fn::FetchRequest {
        url: url.to_string(),
        method: "GET".into(),
        headers: vec![],
        body: None,
        redirect: Default::default(),
    }
}

/// Reads one chunk of `a.url` and abandons the rest of the body (the stream is dropped mid-way).
#[action]
pub async fn drop_stream(ctx: &Ctx, a: Value) -> Result<Value> {
    let mut s = ctx.fetch_stream(get(a["url"].as_str().unwrap_or_default())).await?;
    let first = s.chunk().await?.map(|c| c.len());
    Ok(json!({ "first": first }))
}

/// Reads the whole body of `a.url`.
#[action]
pub async fn read_all(ctx: &Ctx, a: Value) -> Result<Value> {
    let r = ctx.fetch(get(a["url"].as_str().unwrap_or_default())).await?;
    Ok(json!({ "body": String::from_utf8_lossy(&r.body) }))
}

/// Races a fetch against a host sleep; the loser (a hung fetch) is dropped.
#[action]
pub async fn fetch_or_timeout(ctx: &Ctx, a: Value) -> Result<Value> {
    let url = a["url"].as_str().unwrap_or_default();
    let ms = a["ms"].as_u64().unwrap_or(20);
    match convex_fn::select(ctx.fetch(get(url)), ctx.sleep(ms)).await {
        convex_fn::Either::Left(r) => Ok(json!({ "status": r?.status })),
        convex_fn::Either::Right(_) => Ok(json!({ "timedOut": true })),
    }
}

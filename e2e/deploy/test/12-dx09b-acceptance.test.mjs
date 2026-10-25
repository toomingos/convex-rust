// DX-09b / DX-10 flow on a fresh app: rust init -> cargo add image -> TS query + Rust action that calls it
// through `api::`, reads an env var, logs and stores to file storage -> ONE `dev --once` -> plain
// `cargo test` (no --target) -> run -> panic with file:line. No hand-editing of config.
import test, { after } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { cleanup, makeApp, push, callRun, write, read, exec, cli, CONVEX_FN_PATH } from "../lib/harness.mjs";

after(() => { if (!process.env.KEEP) cleanup(); });

const IMAGES_RS = `use convex_fn::{action, log, ConvexArgs, ConvexValue, Ctx, Error, FetchRequest, Result};
use image::imageops::FilterType;
use serde::{Deserialize, Serialize};

use crate::api;

#[derive(Deserialize, ConvexArgs)]
pub struct ThumbnailArgs {
    pub url: String,
}

#[derive(Serialize, ConvexValue)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub storage_id: String,
    pub width: f64,
    pub height: f64,
}

#[action]
pub async fn thumbnail(ctx: &Ctx, args: ThumbnailArgs) -> Result<Thumbnail> {
    let size: f64 = ctx
        .run_query(api::settings::get, api::settings::GetArgs {})
        .await?;
    let filter = match std::env::var("THUMB_FILTER").as_deref() {
        Ok("nearest") => FilterType::Nearest,
        _ => FilterType::Lanczos3,
    };
    let res = ctx.fetch(FetchRequest::get(&args.url)).await?;
    if res.status != 200 {
        return Err(Error::msg(format!("GET {} returned {}", args.url, res.status)));
    }
    let img = image::load_from_memory(&res.body)?;
    let thumb = img.resize(size as u32, size as u32, filter);
    let mut png = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut png, image::ImageFormat::Png)?;
    let storage_id = ctx.storage().store(png.get_ref(), Some("image/png")).await?;
    log::info(&format!("thumbnail {}x{} -> {storage_id}", thumb.width(), thumb.height()));
    Ok(Thumbnail { storage_id, width: thumb.width() as f64, height: thumb.height() as f64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use convex_fn::json;
    use convex_fn_test::{call, FetchStubResponse, TestHost};

    #[test]
    fn thumbnail_resizes_and_stores() {
        let img = image::RgbImage::new(200, 100);
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let png = out.into_inner();
        let host = TestHost::new()
            .on_query("settings:get", |_args| Ok::<_, String>(json!(32)))
            .on_fetch(move |_req| FetchStubResponse::new(200, png.clone()));
        let out = call!(host, thumbnail, json!({ "url": "https://example.test/a.png" }));
        assert_eq!(out["ok"]["width"].as_f64(), Some(32.0));
    }
}
`;

test("12. fresh-app acceptance: single dev --once, plain cargo test, typed api, panic with file:line", async () => {
  const app = await makeApp(); // rust init
  const cfg = read(app, "convex/rust/.cargo/config.toml");
  assert.doesNotMatch(cfg, /\[build\]|target =/, "no build.target in the managed config");
  assert.doesNotMatch(cfg, /\[patch/, "no patch while reqwest is not in the graph");

  // cargo add (plain: no flags, no config editing)
  for (const args of [
    ["add", "image"],
    ["add", "--dev", "convex_fn_test", "--path", `${CONVEX_FN_PATH}_test`],
  ]) {
    const r = await exec("cargo", args, { cwd: app.rust });
    assert.equal(r.code, 0, r.out);
    assert.doesNotMatch(r.out, /was not used/, "no unused-patch warning from manual cargo");
  }

  write(
    app,
    "convex/settings.ts",
    `import { query } from "./_generated/server";
import { v } from "convex/values";
export const get = query({ args: {}, returns: v.number(), handler: async () => 32 });
`,
  );
  write(app, "convex/rust/src/images.rs", IMAGES_RS);
  write(app, "convex/rust/src/lib.rs", read(app, "convex/rust/src/lib.rs").replace("pub mod example;", "pub mod example;\npub mod images;"));

  // ONE push builds Rust against TS functions that have never been pushed
  const p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.doesNotMatch(p.out, /was not used/);
  assert.match(read(app, "convex/rust/src/_generated/api.rs"), /settings:get/);
  assert.match(read(app, "convex/_generated/api.d.ts"), /thumbnail/, p.out);

  // plain cargo test, host target
  const t = await exec("cargo", ["test"], { cwd: app.rust });
  assert.equal(t.code, 0, t.out);
  assert.match(t.out, /thumbnail_resizes_and_stores \.\.\. ok/);

  // run it against a local PNG
  const img = PNG_1X1;
  const server = http.createServer((_q, s) => { s.writeHead(200, { "content-type": "image/png" }); s.end(img); });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  try {
    await cli(app, ["env", "set", "THUMB_FILTER", "nearest"]);
    const url = `http://127.0.0.1:${server.address().port}/a.png`;
    const r = await callRun(app, "rust/images:thumbnail", { url });
    assert.equal(r.code, 0, r.out);
    assert.match(r.out, /thumbnail \d+x\d+ -> /);
    assert.match(r.stdout, /"storageId"/);
  } finally {
    server.close();
  }

  // panic location
  write(
    app,
    "convex/rust/src/images.rs",
    IMAGES_RS.replace(
      "#[cfg(test)]",
      `#[action]
pub async fn broken(_ctx: &Ctx, _args: ThumbnailArgs) -> Result<f64> {
    let n: u32 = "not a number".parse().unwrap();
    Ok(n as f64)
}

#[cfg(test)]`,
    ),
  );
  assert.equal((await push(app)).code, 0);
  const b = await callRun(app, "rust/images:broken", { url: "x" });
  assert.notEqual(b.code, 0);
  assert.match(b.out, /src\/images\.rs:\d+:\d+/);
});

// 1x1 PNG
const PNG_1X1 = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

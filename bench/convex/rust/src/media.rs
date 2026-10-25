//! Media cases (media-*). Inputs are synthesized from `args.seed` with the
//! shared PRNG, identically to bench/convex/node/media.ts. Encoders and
//! resamplers differ between libraries, so outputs carry dimensions /
//! lossless-pixel checksums / QR matrix / word counts, never encoder bytes.

use crate::prng::Prng;
use convex_fn::{action, Ctx, Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

// ---- shared synthesis helpers (mirrored in media.ts) -----------------------

/// Smooth gradient plus 3-bit PRNG noise, RGB8, w*h*3 bytes.
fn synth_rgb(seed: u32, w: usize, h: usize) -> Vec<u8> {
    let mut p = Prng::new(seed);
    let mut out = Vec::with_capacity(w * h * 3);
    let col_r: Vec<u8> = (0..w).map(|x| (x * 255 / w) as u8).collect();
    let diag: Vec<u8> = (0..w + h).map(|i| (i * 255 / (w + h)) as u8).collect();
    for y in 0..h {
        let g = (y * 255 / h) as u8;
        let noise = p.bytes(w * 3);
        for x in 0..w {
            let k = x * 3;
            out.push(col_r[x].saturating_add(noise[k] & 7));
            out.push(g.saturating_add(noise[k + 1] & 7));
            out.push(diag[x + y].saturating_add(noise[k + 2] & 7));
        }
    }
    out
}

fn adler32(buf: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in buf.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
}

/// RGB8 PNG whose IDAT is zlib "stored" blocks around `scanlines`
/// (each row = filter byte + w*3 bytes).
fn png_stored(w: u32, h: u32, scanlines: &[u8]) -> Vec<u8> {
    let nblocks = scanlines.len().div_ceil(65535).max(1);
    let mut z = Vec::with_capacity(2 + scanlines.len() + nblocks * 5 + 4);
    z.extend_from_slice(&[0x78, 0x01]);
    for i in 0..nblocks {
        let start = i * 65535;
        let end = (start + 65535).min(scanlines.len());
        let len = (end - start) as u16;
        z.push(u8::from(i == nblocks - 1));
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(&scanlines[start..end]);
    }
    z.extend_from_slice(&adler32(scanlines).to_be_bytes());
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

// ---- media-001: decode 4K PNG ---------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decoded {
    width: u32,
    height: u32,
    channels: u32,
    crc32: u32,
}

/// Input: a 3840x2160 RGB PNG built by hand (zlib stored blocks, PRNG
/// scanlines, filter type = row % 5). Only decoding is library work.
#[action]
pub async fn media_001(_ctx: &Ctx, args: SeedArgs) -> Result<Decoded> {
    let (w, h) = (3840usize, 2160usize);
    let mut p = Prng::new(args.seed);
    let stride = 1 + w * 3;
    let mut raw = Vec::with_capacity(h * stride);
    for y in 0..h {
        raw.push((y % 5) as u8);
        raw.extend_from_slice(&p.bytes(w * 3));
    }
    let png = png_stored(w as u32, h as u32, &raw);
    drop(raw);
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.into_rgb8();
    Ok(Decoded {
        width: img.width(),
        height: img.height(),
        channels: 3,
        crc32: crc32fast::hash(img.as_raw()),
    })
}

// ---- media-002: resize 4K JPEG -> 1080p ------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resized {
    src_width: u32,
    src_height: u32,
    width: u32,
    height: u32,
    channels: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizedMean {
    src_width: u32,
    src_height: u32,
    width: u32,
    height: u32,
    channels: u32,
    mean_px: u32,
}

/// Synthetic 3840x2160 RGB, JPEG-encoded (q80) by this side's encoder as part
/// of the work, then decoded and resized to 1920x1080 (Lanczos3).
#[action]
pub async fn media_002(_ctx: &Ctx, args: SeedArgs) -> Result<ResizedMean> {
    use fast_image_resize::images::Image;
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
    let (w, h) = (3840u32, 2160u32);
    let px = synth_rgb(args.seed, w as usize, h as usize);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80).encode(
        &px,
        w,
        h,
        image::ExtendedColorType::Rgb8,
    )?;
    drop(px);
    let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)?.into_rgb8();
    let (sw, sh) = img.dimensions();
    let src = Image::from_vec_u8(sw, sh, img.into_raw(), PixelType::U8x3)?;
    let mut dst = Image::new(1920, 1080, PixelType::U8x3);
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
    )?;
    Ok(ResizedMean {
        src_width: w,
        src_height: h,
        width: dst.width(),
        height: dst.height(),
        channels: 3,
        mean_px: {
            let b = dst.buffer();
            (b.iter().map(|&v| v as u64).sum::<u64>() as f64 / b.len() as f64).round() as u32
        },
    })
}

// ---- media-003: thumbnail 256 px ------------------------------------------

/// 2048x1536 raw RGB -> fit inside 256x256 (256x192).
#[action]
pub async fn media_003(_ctx: &Ctx, args: SeedArgs) -> Result<Resized> {
    let (w, h) = (2048u32, 1536u32);
    use fast_image_resize::images::Image;
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
    // Fit inside 256x256 keeping the aspect ratio (what sharp's `fit: "inside"` computes).
    let scale = (256.0 / f64::from(w)).min(256.0 / f64::from(h));
    let (dw, dh) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    let src = Image::from_vec_u8(w, h, synth_rgb(args.seed, w as usize, h as usize), PixelType::U8x3)?;
    let mut dst = Image::new(dw, dh, PixelType::U8x3);
    // Same Lanczos3 convolution as sharp's default kernel.
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
    )?;
    Ok(Resized {
        src_width: w,
        src_height: h,
        width: dst.width(),
        height: dst.height(),
        channels: 3,
    })
}

// ---- media-004: PNG -> WebP -----------------------------------------------

#[derive(Serialize)]
pub struct Webp {
    riff: bool,
    width: u32,
    height: u32,
    bytes: usize,
}

fn le24(b: &[u8]) -> u32 {
    u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16
}

/// Decode a 1280x720 PNG (stored, filter 0) and re-encode it as lossless WebP.
#[action]
pub async fn media_004(_ctx: &Ctx, args: SeedArgs) -> Result<Webp> {
    let (w, h) = (1280usize, 720usize);
    let px = synth_rgb(args.seed, w, h);
    let stride = 1 + w * 3;
    let mut raw = vec![0u8; h * stride];
    for y in 0..h {
        raw[y * stride + 1..(y + 1) * stride].copy_from_slice(&px[y * w * 3..(y + 1) * w * 3]);
    }
    let png = png_stored(w as u32, h as u32, &raw);
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.into_rgb8();
    let mut webp = Vec::new();
    image_webp::WebPEncoder::new(&mut webp).encode(
        img.as_raw(),
        img.width(),
        img.height(),
        image_webp::ColorType::Rgb8,
    )?;
    let riff = &webp[0..4] == b"RIFF" && &webp[8..12] == b"WEBP";
    let (width, height) = match &webp[12..16] {
        b"VP8L" => {
            let bits = u32::from_le_bytes(webp[21..25].try_into().unwrap());
            ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => (le24(&webp[24..27]) + 1, le24(&webp[27..30]) + 1),
        _ => (0, 0),
    };
    Ok(Webp {
        riff,
        width,
        height,
        bytes: webp.len(),
    })
}

// ---- media-005: Gaussian blur 1080p ---------------------------------------

#[derive(Serialize)]
pub struct Blurred {
    width: u32,
    height: u32,
    channels: u32,
}

/// 1920x1080 raw RGB, Gaussian blur sigma 5.
#[action]
pub async fn media_005(_ctx: &Ctx, args: SeedArgs) -> Result<Blurred> {
    let (w, h) = (1920u32, 1080u32);
    use libblur::{
        BlurImage, BlurImageMut, ConvolutionMode, EdgeMode, EdgeMode2D, FastBlurChannels,
        GaussianBlurParams, ThreadingPolicy,
    };
    let px = synth_rgb(args.seed, w as usize, h as usize);
    let src = BlurImage::borrow(&px, w, h, FastBlurChannels::Channels3);
    let mut dst = BlurImageMut::alloc(w, h, FastBlurChannels::Channels3);
    // True (separable, exact f32) Gaussian: sigma 5, kernel 2*ceil(3*sigma)+1 = 31, one thread
    // (wasm has no threads; native stays on one core).
    libblur::gaussian_blur(
        &src,
        &mut dst,
        GaussianBlurParams::new(31, 5.0),
        EdgeMode2D::new(EdgeMode::Clamp),
        ThreadingPolicy::Single,
        ConvolutionMode::Exact,
    )
    .map_err(|e| Error::msg(format!("blur: {e:?}")))?;
    Ok(Blurred {
        width: dst.width,
        height: dst.height,
        channels: 3,
    })
}

// ---- media-006: QR code -> PNG --------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Qr {
    version: u32,
    modules: u32,
    matrix_crc32: u32,
    dark: u32,
    png_width: u32,
    png_height: u32,
}

/// QR for 300 PRNG chars, ECC M, scale 8, 4-module quiet zone.
#[action]
pub async fn media_006(_ctx: &Ctx, args: SeedArgs) -> Result<Qr> {
    use qrcode::bits::Bits;
    use qrcode::{Color, EcLevel, QrCode, Version};
    let text = Prng::new(args.seed).text(300);
    // Single byte-mode segment at the smallest version that fits, as the Node
    // `qrcode` package does (the crate's default optimizer mixes segment modes).
    let qr = (1..=40)
        .find_map(|v| {
            let mut bits = Bits::new(Version::Normal(v));
            bits.push_byte_data(text.as_bytes()).ok()?;
            bits.push_terminator(EcLevel::M).ok()?;
            QrCode::with_bits(bits, EcLevel::M).ok()
        })
        .ok_or_else(|| Error::msg("qr: data too long"))?;
    let size = qr.width();
    let bits: Vec<u8> = qr
        .to_colors()
        .iter()
        .map(|c| u8::from(*c == Color::Dark))
        .collect();
    let dark = bits.iter().map(|&b| u32::from(b)).sum();
    let version = match qr.version() {
        Version::Normal(n) | Version::Micro(n) => n as u32,
    };
    let (scale, quiet) = (8usize, 4usize);
    let side = (size + 2 * quiet) * scale;
    let mut gray = vec![255u8; side * side];
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = (x / scale, y / scale);
            let inside = mx >= quiet && my >= quiet && mx < size + quiet && my < size + quiet;
            if inside && bits[(my - quiet) * size + (mx - quiet)] == 1 {
                gray[y * side + x] = 0;
            }
        }
    }
    // Grayscale 8-bit, zlib level 6, Sub filter (the Node side uses pngjs with the same settings).
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Level(6),
            image::codecs::png::FilterType::Sub,
        ),
        &gray,
        side as u32,
        side as u32,
        image::ExtendedColorType::L8,
    )?;
    Ok(Qr {
        version,
        modules: size as u32,
        matrix_crc32: crc32fast::hash(&bits),
        dark,
        png_width: u32::from_be_bytes(png[16..20].try_into().unwrap()),
        png_height: u32::from_be_bytes(png[20..24].try_into().unwrap()),
    })
}

// ---- media-007: SVG -> PNG --------------------------------------------------

fn synth_svg(seed: u32) -> String {
    use std::fmt::Write as _;
    let mut p = Prng::new(seed);
    let mut s = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1024\" height=\"1024\" viewBox=\"0 0 1024 1024\"><rect width=\"1024\" height=\"1024\" fill=\"#ffffff\"/>",
    );
    for _ in 0..300 {
        let kind = p.below(3);
        let (r, g, b) = (p.below(256), p.below(256), p.below(256));
        let fill = format!("#{r:02x}{g:02x}{b:02x}");
        let op = p.below(90) + 10;
        let a = p.below(1024);
        let b = p.below(1024);
        let c = p.below(190) + 10;
        let d = p.below(190) + 10;
        match kind {
            0 => write!(s, "<rect x=\"{a}\" y=\"{b}\" width=\"{c}\" height=\"{d}\" fill=\"{fill}\" fill-opacity=\"0.{op}\"/>"),
            1 => write!(s, "<circle cx=\"{a}\" cy=\"{b}\" r=\"{c}\" fill=\"{fill}\" fill-opacity=\"0.{op}\"/>"),
            _ => write!(s, "<ellipse cx=\"{a}\" cy=\"{b}\" rx=\"{c}\" ry=\"{d}\" fill=\"{fill}\" fill-opacity=\"0.{op}\"/>"),
        }
        .unwrap();
    }
    s.push_str("</svg>");
    s
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Svg {
    width: u32,
    height: u32,
    png_ok: bool,
    png_bytes: usize,
}

/// 300 random shapes on a 1024x1024 canvas rendered to PNG.
#[action]
pub async fn media_007(_ctx: &Ctx, args: SeedArgs) -> Result<Svg> {
    use resvg::{tiny_skia, usvg};
    let svg = synth_svg(args.seed);
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())?;
    let size = tree.size().to_int_size();
    let mut pm = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| Error::msg("bad pixmap"))?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pm.as_mut());
    let png = pm.encode_png()?;
    Ok(Svg {
        width: pm.width(),
        height: pm.height(),
        png_ok: &png[1..4] == b"PNG",
        png_bytes: png.len(),
    })
}

// ---- media-008: PDF text extraction ---------------------------------------

/// 25 pages x 40 lines x 8 words, Helvetica, hand-built with a correct xref table.
fn synth_pdf(seed: u32) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut p = Prng::new(seed);
    let pages = 25usize;
    let mut objs: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {pages} >>",
        kids.join(" ")
    ));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..pages {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        let mut c = String::from("BT\n/F1 10 Tf\n14 TL\n72 760 Td\n");
        for _ in 0..40 {
            let words: Vec<String> = (0..8)
                .map(|_| {
                    let len = 3 + p.below(6) as usize;
                    p.word(len)
                })
                .collect();
            write!(c, "({}) Tj\nT*\n", words.join(" ")).unwrap();
        }
        c.push_str("ET");
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n{body}\nendobj\n", i + 1).unwrap();
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).unwrap();
    for o in offsets {
        writeln!(out, "{o:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    )
    .unwrap();
    out.into_bytes()
}

#[derive(Serialize)]
pub struct Pdf {
    pages: usize,
    words: usize,
}

#[action]
pub async fn media_008(_ctx: &Ctx, args: SeedArgs) -> Result<Pdf> {
    let doc = lopdf::Document::load_mem(&synth_pdf(args.seed))?;
    let nums: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc.extract_text(&nums)?;
    Ok(Pdf {
        pages: nums.len(),
        words: text.split_whitespace().count(),
    })
}

// ---- media-009: read EXIF -------------------------------------------------

struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    data: Vec<u8>,
}

impl Entry {
    fn new(tag: u16, kind: u16, count: u32, data: Vec<u8>) -> Self {
        Self {
            tag,
            kind,
            count,
            data,
        }
    }
}

fn ascii(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn rational(num: u32, den: u32) -> Vec<u8> {
    let mut v = num.to_le_bytes().to_vec();
    v.extend_from_slice(&den.to_le_bytes());
    v
}

/// Little-endian TIFF: IFD0 + Exif sub-IFD, big values in a trailing data area.
fn build_tiff(mut ifd0: Vec<Entry>, exif: Vec<Entry>) -> Vec<u8> {
    let exif_off = 8 + 2 + 12 * (ifd0.len() + 1) + 4;
    let data_start = exif_off + 2 + 12 * exif.len() + 4;
    ifd0.push(Entry::new(
        0x8769,
        4,
        1,
        (exif_off as u32).to_le_bytes().to_vec(),
    ));
    ifd0.sort_by_key(|e| e.tag);
    let mut area: Vec<u8> = Vec::new();
    let mut write_ifd = |entries: &[Entry]| -> Vec<u8> {
        let mut b = vec![0u8; 2 + 12 * entries.len() + 4];
        b[0..2].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        for (i, e) in entries.iter().enumerate() {
            let o = 2 + 12 * i;
            b[o..o + 2].copy_from_slice(&e.tag.to_le_bytes());
            b[o + 2..o + 4].copy_from_slice(&e.kind.to_le_bytes());
            b[o + 4..o + 8].copy_from_slice(&e.count.to_le_bytes());
            if e.data.len() <= 4 {
                b[o + 8..o + 8 + e.data.len()].copy_from_slice(&e.data);
            } else {
                let off = (data_start + area.len()) as u32;
                b[o + 8..o + 12].copy_from_slice(&off.to_le_bytes());
                area.extend_from_slice(&e.data);
                if e.data.len() % 2 == 1 {
                    area.push(0);
                }
            }
        }
        b
    };
    let a = write_ifd(&ifd0);
    let b = write_ifd(&exif);
    let mut out = vec![0x49, 0x49, 42, 0, 8, 0, 0, 0];
    out.extend_from_slice(&a);
    out.extend_from_slice(&b);
    out.extend_from_slice(&area);
    out
}

fn stamp(p: &mut Prng) -> String {
    let yy = p.below(25);
    let mo = 1 + p.below(12);
    let dd = 1 + p.below(28);
    let hh = p.below(24);
    let mi = p.below(60);
    let ss = p.below(60);
    format!("20{yy:02}:{mo:02}:{dd:02} {hh:02}:{mi:02}:{ss:02}")
}

fn synth_exif_jpeg(seed: u32) -> Vec<u8> {
    let mut p = Prng::new(seed);
    let make = p.word(8);
    let model = p.word(10);
    let orientation = 1 + p.below(8);
    let dt = stamp(&mut p);
    let exposure_den = 1 + p.below(4000);
    let fnum = 10 + p.below(200);
    let iso = 100 * (1 + p.below(32));
    let dto = stamp(&mut p);
    let focal = 10 + p.below(200);
    let ifd0 = vec![
        Entry::new(0x010f, 2, make.len() as u32 + 1, ascii(&make)),
        Entry::new(0x0110, 2, model.len() as u32 + 1, ascii(&model)),
        Entry::new(0x0112, 3, 1, (orientation as u16).to_le_bytes().to_vec()),
        Entry::new(0x0132, 2, 20, ascii(&dt)),
    ];
    let exif = vec![
        Entry::new(0x829a, 5, 1, rational(1, exposure_den)),
        Entry::new(0x829d, 5, 1, rational(fnum, 10)),
        Entry::new(0x8827, 3, 1, (iso as u16).to_le_bytes().to_vec()),
        Entry::new(0x9003, 2, 20, ascii(&dto)),
        Entry::new(0x920a, 5, 1, rational(focal, 1)),
    ];
    let tiff = build_tiff(ifd0, exif);
    let mut body = b"Exif\0\0".to_vec();
    body.extend_from_slice(&tiff);
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exif {
    make: String,
    model: String,
    orientation: u32,
    date_time: String,
    exposure_time: f64,
    f_number: f64,
    iso: u32,
    date_time_original: String,
    focal_length: f64,
}

#[action]
pub async fn media_009(_ctx: &Ctx, args: SeedArgs) -> Result<Exif> {
    use exif::{In, Tag, Value};
    let jpeg = synth_exif_jpeg(args.seed);
    let e = exif::Reader::new().read_from_container(&mut Cursor::new(&jpeg))?;
    let field = |t: Tag| {
        e.get_field(t, In::PRIMARY)
            .ok_or_else(|| Error::msg(format!("missing tag {t}")))
    };
    let string = |t: Tag| -> Result<String> {
        match &field(t)?.value {
            Value::Ascii(v) if !v.is_empty() => Ok(String::from_utf8_lossy(&v[0]).into_owned()),
            _ => Err(Error::msg("not ascii")),
        }
    };
    let float = |t: Tag| -> Result<f64> {
        match &field(t)?.value {
            Value::Rational(v) if !v.is_empty() => Ok(v[0].to_f64()),
            _ => Err(Error::msg("not rational")),
        }
    };
    let uint = |t: Tag| -> Result<u32> {
        field(t)?
            .value
            .get_uint(0)
            .ok_or_else(|| Error::msg("not uint"))
    };
    Ok(Exif {
        make: string(Tag::Make)?,
        model: string(Tag::Model)?,
        orientation: uint(Tag::Orientation)?,
        date_time: string(Tag::DateTime)?,
        exposure_time: float(Tag::ExposureTime)?,
        f_number: float(Tag::FNumber)?,
        iso: uint(Tag::PhotographicSensitivity)?,
        date_time_original: string(Tag::DateTimeOriginal)?,
        focal_length: float(Tag::FocalLength)?,
    })
}

// ---- media-010: WAV resample 48k -> 16k ------------------------------------

fn synth_wav(seed: u32) -> Vec<u8> {
    let mut p = Prng::new(seed);
    let rate = 48000u32;
    let n = (rate * 10) as usize;
    let mut wav = Vec::with_capacity(44 + n * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + n as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(n as u32 * 2).to_le_bytes());
    let tau = std::f64::consts::PI * 2.0;
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let s = 0.5 * (tau * 440.0 * t).sin()
            + 0.3 * (tau * 1000.0 * t).sin()
            + 0.1 * (tau * 3000.0 * t).sin()
            + 0.02 * (p.f64() - 0.5);
        wav.extend_from_slice(&((s * 32767.0).round() as i16).to_le_bytes());
    }
    wav
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resampled {
    in_rate: u32,
    out_rate: u32,
    in_samples: usize,
    out_samples: usize,
    rms_preserved: bool,
}

/// wave-resampler's ButterworthLPF: `order` cascaded biquads, run forward then in reverse.
fn butterworth_lpf(x: &mut [f64], order: usize, fs: f64, fc: f64) {
    struct Stage {
        b0: f64,
        b1: f64,
        b2: f64,
        a1: f64,
        a2: f64,
        z: [f64; 2],
    }
    let mut stages: Vec<Stage> = (0..order)
        .map(|i| {
            let q = 0.5 / ((std::f64::consts::PI / (order as f64 * 2.0)) * (i as f64 + 0.5)).sin();
            let w = 2.0 * std::f64::consts::PI * fc / fs;
            let alpha = w.sin() / (2.0 * q);
            let cw = w.cos();
            let a0 = 1.0 + alpha;
            let b0 = (1.0 - cw) / (2.0 * a0);
            Stage { b0, b1: 2.0 * b0, b2: b0, a1: (-2.0 * cw) / a0, a2: (1.0 - alpha) / a0, z: [0.0; 2] }
        })
        .collect();
    let run = |stages: &mut Vec<Stage>, v: f64| {
        let mut out = v;
        for s in stages.iter_mut() {
            let t = out - s.a1 * s.z[0] - s.a2 * s.z[1];
            out = s.b0 * t + s.b1 * s.z[0] + s.b2 * s.z[1];
            s.z[1] = s.z[0];
            s.z[0] = t;
        }
        out
    };
    for v in x.iter_mut() {
        *v = run(&mut stages, *v);
    }
    for s in stages.iter_mut() {
        s.z = [0.0; 2];
    }
    for v in x.iter_mut().rev() {
        *v = run(&mut stages, *v);
    }
}

/// Parse a 16-bit mono WAV, resample 48 kHz -> 16 kHz. Resampler outputs differ
/// numerically by design, so the output holds exact integers plus a boolean
/// check that RMS energy is preserved (<2%).
#[action]
pub async fn media_010(_ctx: &Ctx, args: SeedArgs) -> Result<Resampled> {
    use audioadapter_buffers::owned::InterleavedOwned;
    use rubato::{
        Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };
    let wav = synth_wav(args.seed);
    let mut rate = 0u32;
    let mut input: Vec<f64> = Vec::new();
    let mut o = 12usize;
    while o + 8 <= wav.len() {
        let id = &wav[o..o + 4];
        let size = u32::from_le_bytes(wav[o + 4..o + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            rate = u32::from_le_bytes(wav[o + 12..o + 16].try_into().unwrap());
        } else if id == b"data" {
            input = wav[o + 8..o + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f64::from(i16::from_le_bytes(*b)) / 32768.0)
                .collect();
        }
        o += 8 + size + (size % 2);
    }
    let out_rate = 16000u32;
    let in_len = input.len();
    let rms = |x: &[f64]| (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt();
    // wave-resampler's `sinc` method first low-passes the input in place (16th-order Butterworth,
    // cutoff = out_rate/2, one forward and one reverse pass), then does the windowed-sinc resample.
    butterworth_lpf(&mut input, 16, rate as f64, f64::from(out_rate) / 2.0);
    let in_rms = rms(&input);
    // Windowed-sinc resampler (the Node library uses 12 taps, i.e. sincFilterSize 6; rubato rounds
    // the length up to a multiple of 8).
    let params = SincInterpolationParameters {
        sinc_len: 12,
        f_cutoff: None,
        oversampling_factor: 256,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut rs = Async::<f64>::new_sinc(
        f64::from(out_rate) / f64::from(rate),
        1.0,
        &params,
        1024,
        1,
        FixedAsync::Input,
    )?;
    let buf =
        InterleavedOwned::new_from(input, 1, in_len).map_err(|e| Error::msg(format!("{e:?}")))?;
    let out = rs.process_all(&buf, in_len, None)?.take_data();
    let ratio = rms(&out) / in_rms;
    Ok(Resampled {
        in_rate: rate,
        out_rate,
        in_samples: in_len,
        out_samples: out.len(),
        rms_preserved: (ratio - 1.0).abs() < 0.02,
    })
}

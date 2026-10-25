"use node";
// Media cases (media-*). Each case exports the pure implementation (`media001`,
// used by the phase-0 runner) and the Convex action (`media_001`).
//
// Inputs are synthesized from args.seed with the shared PRNG, identically to
// bench/convex/rust/src/media.rs. Encoders/resamplers legitimately differ
// between libraries, so outputs carry dimensions / lossless-pixel checksums /
// QR matrix / word counts, never encoder bytes.
import { crc32 } from "node:zlib";
import sharp from "sharp";
import QRCode from "qrcode";
// @ts-expect-error pngjs ships no types
import { PNG as PNGRaw } from "pngjs";
import { Resvg } from "@resvg/resvg-js";
import exifr from "exifr";
// @ts-expect-error pdf-parse ships no types for the lib/ entry point
import pdfParseRaw from "pdf-parse/lib/pdf-parse.js";
import waveResampler from "wave-resampler";
import { seedAction } from "./_case.ts";
import { Prng } from "../../shared/prng.ts";

// The package index runs a debug self-test when imported without a parent module, so import lib/ directly.
const pdfParse = pdfParseRaw as (b: Buffer) => Promise<{ numpages: number; text: string }>;
const PNG = PNGRaw as { sync: { write(png: unknown, opts: Record<string, unknown>): Buffer } };
const { resample } = waveResampler as typeof import("wave-resampler");

type SeedArgs = { seed: number };

// ---- shared synthesis helpers (mirrored in media.rs) -----------------------

/** Smooth gradient plus 3-bit PRNG noise, RGB8, w*h*3 bytes. */
function synthRgb(seed: number, w: number, h: number): Buffer {
  const p = new Prng(seed);
  const out = Buffer.alloc(w * h * 3);
  const colR = new Uint8Array(w);
  for (let x = 0; x < w; x++) colR[x] = Math.floor((x * 255) / w);
  const diag = new Uint8Array(w + h);
  for (let i = 0; i < w + h; i++) diag[i] = Math.floor((i * 255) / (w + h));
  let o = 0;
  for (let y = 0; y < h; y++) {
    const g = Math.floor((y * 255) / h);
    const noise = p.bytes(w * 3);
    for (let x = 0; x < w; x++) {
      const k = x * 3;
      out[o++] = Math.min(255, colR[x] + (noise[k] & 7));
      out[o++] = Math.min(255, g + (noise[k + 1] & 7));
      out[o++] = Math.min(255, diag[x + y] + (noise[k + 2] & 7));
    }
  }
  return out;
}

function adler32(buf: Uint8Array): number {
  let a = 1;
  let b = 0;
  for (let i = 0; i < buf.length; ) {
    const end = Math.min(i + 5552, buf.length);
    for (; i < end; i++) {
      a += buf[i];
      b += a;
    }
    a %= 65521;
    b %= 65521;
  }
  return ((b << 16) | a) >>> 0;
}

function u32be(n: number): Buffer {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n >>> 0);
  return b;
}

function pngChunk(type: string, data: Buffer): Buffer {
  const td = Buffer.concat([Buffer.from(type, "latin1"), data]);
  return Buffer.concat([u32be(data.length), td, u32be(crc32(td))]);
}

/** RGB8 PNG whose IDAT is zlib "stored" blocks around `scanlines` (each row = filter byte + w*3). */
function pngStored(w: number, h: number, scanlines: Buffer): Buffer {
  const nblocks = Math.max(1, Math.ceil(scanlines.length / 65535));
  const z = Buffer.alloc(2 + scanlines.length + nblocks * 5 + 4);
  z[0] = 0x78;
  z[1] = 0x01;
  let o = 2;
  for (let i = 0; i < nblocks; i++) {
    const start = i * 65535;
    const len = Math.min(65535, scanlines.length - start);
    z[o++] = i === nblocks - 1 ? 1 : 0;
    z.writeUInt16LE(len, o);
    z.writeUInt16LE(~len & 0xffff, o + 2);
    o += 4;
    scanlines.copy(z, o, start, start + len);
    o += len;
  }
  z.writeUInt32BE(adler32(scanlines), o);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", z),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---- media-001: decode 4K PNG ---------------------------------------------

/**
 * Input: a 3840x2160 RGB PNG built by hand (zlib stored blocks, PRNG scanlines,
 * filter type = row % 5 so every PNG filter is exercised). Only decoding is
 * library work; the container is byte-identical on both sides.
 * Output: dimensions + crc32 of the decoded pixels (lossless, so exact).
 */
export async function media001(_ctx: unknown, args: SeedArgs) {
  const w = 3840;
  const h = 2160;
  const p = new Prng(args.seed);
  const stride = 1 + w * 3;
  const raw = Buffer.alloc(h * stride);
  for (let y = 0; y < h; y++) {
    raw[y * stride] = y % 5;
    raw.set(p.bytes(w * 3), y * stride + 1);
  }
  const png = pngStored(w, h, raw);
  const { data, info } = await sharp(png).raw().toBuffer({ resolveWithObject: true });
  return { width: info.width, height: info.height, channels: info.channels, crc32: crc32(data) };
}

export const media_001 = seedAction(media001);

// ---- media-002: resize 4K JPEG -> 1080p ------------------------------------

/**
 * Input: synthetic 3840x2160 RGB, JPEG-encoded (q80) by each side's own encoder
 * as part of the work, then decoded and resized to 1920x1080 (Lanczos3).
 */
export async function media002(_ctx: unknown, args: SeedArgs) {
  const w = 3840;
  const h = 2160;
  const jpeg = await sharp(synthRgb(args.seed, w, h), { raw: { width: w, height: h, channels: 3 } })
    .jpeg({ quality: 80, chromaSubsampling: "4:4:4", optimiseCoding: false }) // image crate: 4:4:4, fixed (standard) Huffman tables
    .toBuffer();
  // Decode the full-size JPEG to raw first (no shrink-on-load, which would decode at 1/2 size), then
  // resize the raw buffer. The re-wrap is zero-copy; it is needed to keep a full-size decode.
  const dec = await sharp(jpeg).raw().toBuffer({ resolveWithObject: true });
  const { data, info } = await sharp(dec.data, {
    raw: { width: dec.info.width, height: dec.info.height, channels: dec.info.channels },
  })
    .resize(1920, 1080, { kernel: "lanczos3" })
    .raw()
    .toBuffer({ resolveWithObject: true });
  let sum = 0;
  for (let i = 0; i < data.length; i++) sum += data[i];
  const meanPx = Math.round(sum / data.length);
  return { srcWidth: w, srcHeight: h, width: info.width, height: info.height, channels: info.channels, meanPx };
}

export const media_002 = seedAction(media002);

// ---- media-003: thumbnail 256 px ------------------------------------------

/** 2048x1536 raw RGB -> fit inside 256x256 (256x192). */
export async function media003(_ctx: unknown, args: SeedArgs) {
  const w = 2048;
  const h = 1536;
  const { info } = await sharp(synthRgb(args.seed, w, h), { raw: { width: w, height: h, channels: 3 } })
    .resize(256, 256, { fit: "inside" })
    .raw()
    .toBuffer({ resolveWithObject: true });
  return { srcWidth: w, srcHeight: h, width: info.width, height: info.height, channels: info.channels };
}

export const media_003 = seedAction(media003);

// ---- media-004: PNG -> WebP -----------------------------------------------

/** Dimensions parsed from the WebP container (VP8L or VP8X), plus RIFF/WEBP check. */
function webpInfo(b: Buffer): { riff: boolean; width: number; height: number } {
  const riff = b.toString("latin1", 0, 4) === "RIFF" && b.toString("latin1", 8, 12) === "WEBP";
  const fourcc = b.toString("latin1", 12, 16);
  let width = 0;
  let height = 0;
  if (fourcc === "VP8L") {
    const bits = b.readUInt32LE(21);
    width = (bits & 0x3fff) + 1;
    height = ((bits >>> 14) & 0x3fff) + 1;
  } else if (fourcc === "VP8X") {
    width = b.readUIntLE(24, 3) + 1;
    height = b.readUIntLE(27, 3) + 1;
  }
  return { riff, width, height };
}

/** Decode a 1280x720 PNG (stored, filter 0) and re-encode it as lossless WebP. */
export async function media004(_ctx: unknown, args: SeedArgs) {
  const w = 1280;
  const h = 720;
  const px = synthRgb(args.seed, w, h);
  const stride = 1 + w * 3;
  const raw = Buffer.alloc(h * stride);
  for (let y = 0; y < h; y++) px.copy(raw, y * stride + 1, y * w * 3, (y + 1) * w * 3);
  const png = pngStored(w, h, raw);
  const webp = await sharp(png).webp({ lossless: true, effort: 0 }).toBuffer();
  const info = webpInfo(webp);
  return { riff: info.riff, width: info.width, height: info.height, bytes: webp.length };
}

export const media_004 = seedAction(media004);

// ---- media-005: Gaussian blur 1080p ---------------------------------------

/** 1920x1080 raw RGB, Gaussian blur sigma 5. */
export async function media005(_ctx: unknown, args: SeedArgs) {
  const w = 1920;
  const h = 1080;
  const { info } = await sharp(synthRgb(args.seed, w, h), { raw: { width: w, height: h, channels: 3 } })
    .blur(5)
    .raw()
    .toBuffer({ resolveWithObject: true });
  return { width: info.width, height: info.height, channels: info.channels };
}

export const media_005 = seedAction(media005);

// ---- media-006: QR code -> PNG --------------------------------------------

/**
 * QR for 300 PRNG chars, ECC M, rendered at scale 8 with a 4-module quiet zone.
 * Output: module count, crc32 of the module matrix (one byte per module), dark
 * module count, and the PNG dimensions read back from its IHDR.
 */
export async function media006(_ctx: unknown, args: SeedArgs) {
  const text = new Prng(args.seed).text(300);
  const qr = QRCode.create(text, { errorCorrectionLevel: "M" });
  const size = qr.modules.size;
  const bits = Uint8Array.from(qr.modules.data as unknown as ArrayLike<number>, (x) => (x ? 1 : 0));
  let dark = 0;
  for (const b of bits) dark += b;
  // Render the module matrix (scale 8, quiet zone 4) to an 8-bit grayscale PNG, zlib level 6, Sub filter
  // (the Rust side uses the same pixel format and settings).
  const scale = 8;
  const quiet = 4;
  const side = (size + 2 * quiet) * scale;
  const gray = Buffer.alloc(side * side, 255);
  for (let y = 0; y < side; y++) {
    const my = Math.floor(y / scale);
    for (let x = 0; x < side; x++) {
      const mx = Math.floor(x / scale);
      if (mx >= quiet && my >= quiet && mx < size + quiet && my < size + quiet && bits[(my - quiet) * size + (mx - quiet)] === 1) {
        gray[y * side + x] = 0;
      }
    }
  }
  const png = PNG.sync.write({ width: side, height: side, data: gray }, {
    colorType: 0,
    inputColorType: 0,
    bitDepth: 8,
    deflateLevel: 6,
    filterType: 1,
  });
  return {
    version: qr.version,
    modules: size,
    matrixCrc32: crc32(bits),
    dark,
    pngWidth: png.readUInt32BE(16),
    pngHeight: png.readUInt32BE(20),
  };
}

export const media_006 = seedAction(media006);

// ---- media-007: SVG -> PNG --------------------------------------------------

function synthSvg(seed: number): string {
  const p = new Prng(seed);
  const hex = (n: number) => n.toString(16).padStart(2, "0");
  let s =
    '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">' +
    '<rect width="1024" height="1024" fill="#ffffff"/>';
  for (let i = 0; i < 300; i++) {
    const kind = p.below(3);
    const fill = `#${hex(p.below(256))}${hex(p.below(256))}${hex(p.below(256))}`;
    const op = p.below(90) + 10;
    const a = p.below(1024);
    const b = p.below(1024);
    const c = p.below(190) + 10;
    const d = p.below(190) + 10;
    if (kind === 0) s += `<rect x="${a}" y="${b}" width="${c}" height="${d}" fill="${fill}" fill-opacity="0.${op}"/>`;
    else if (kind === 1) s += `<circle cx="${a}" cy="${b}" r="${c}" fill="${fill}" fill-opacity="0.${op}"/>`;
    else s += `<ellipse cx="${a}" cy="${b}" rx="${c}" ry="${d}" fill="${fill}" fill-opacity="0.${op}"/>`;
  }
  return s + "</svg>";
}

/** 300 random shapes on a 1024x1024 canvas rendered to PNG. */
export async function media007(_ctx: unknown, args: SeedArgs) {
  const svg = synthSvg(args.seed);
  const img = new Resvg(svg, { fitTo: { mode: "width", value: 1024 }, font: { loadSystemFonts: false } }).render();
  const png = img.asPng();
  return {
    width: img.width,
    height: img.height,
    pngOk: png.subarray(1, 4).toString("latin1") === "PNG",
    pngBytes: png.length,
  };
}

export const media_007 = seedAction(media007);

// ---- media-008: PDF text extraction ---------------------------------------

/** 25 pages x 40 lines x 8 words, Helvetica, hand-built with a correct xref table. */
function synthPdf(seed: number): string {
  const p = new Prng(seed);
  const pages = 25;
  const objs: string[] = [];
  const kids = Array.from({ length: pages }, (_, i) => `${4 + 2 * i} 0 R`).join(" ");
  objs.push("<< /Type /Catalog /Pages 2 0 R >>");
  objs.push(`<< /Type /Pages /Kids [${kids}] /Count ${pages} >>`);
  objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  for (let i = 0; i < pages; i++) {
    objs.push(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + 2 * i} 0 R >>`,
    );
    let c = "BT\n/F1 10 Tf\n14 TL\n72 760 Td\n";
    for (let l = 0; l < 40; l++) {
      const words: string[] = [];
      for (let k = 0; k < 8; k++) words.push(p.word(3 + p.below(6)));
      c += `(${words.join(" ")}) Tj\nT*\n`;
    }
    c += "ET";
    objs.push(`<< /Length ${c.length} >>\nstream\n${c}\nendstream`);
  }
  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  objs.forEach((body, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n`;
  for (const o of offsets) out += `${String(o).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return out;
}

export async function media008(_ctx: unknown, args: SeedArgs) {
  const data = await pdfParse(Buffer.from(synthPdf(args.seed), "latin1"));
  const words = data.text.split(/\s+/).filter((t) => t.length > 0).length;
  return { pages: data.numpages, words };
}

export const media_008 = seedAction(media008);

// ---- media-009: read EXIF -------------------------------------------------

type Entry = { tag: number; type: number; count: number; data: Buffer };

function ascii(s: string): Entry["data"] {
  return Buffer.from(s + "\0", "latin1");
}
function short(n: number): Buffer {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n);
  return b;
}
function rational(num: number, den: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeUInt32LE(num, 0);
  b.writeUInt32LE(den, 4);
  return b;
}

/** Little-endian TIFF: IFD0 + Exif sub-IFD, big values in a trailing data area. */
function buildTiff(ifd0: Entry[], exif: Entry[]): Buffer {
  const ifd0Off = 8;
  const exifOff = ifd0Off + 2 + 12 * (ifd0.length + 1) + 4;
  const dataStart = exifOff + 2 + 12 * exif.length + 4;
  ifd0 = [...ifd0, { tag: 0x8769, type: 4, count: 1, data: Buffer.from(new Uint32Array([exifOff]).buffer) }];
  ifd0.sort((a, b) => a.tag - b.tag);
  const area: Buffer[] = [];
  let cursor = dataStart;
  const writeIfd = (entries: Entry[]): Buffer => {
    const b = Buffer.alloc(2 + 12 * entries.length + 4);
    b.writeUInt16LE(entries.length, 0);
    entries.forEach((e, i) => {
      const o = 2 + 12 * i;
      b.writeUInt16LE(e.tag, o);
      b.writeUInt16LE(e.type, o + 2);
      b.writeUInt32LE(e.count, o + 4);
      if (e.data.length <= 4) e.data.copy(b, o + 8);
      else {
        b.writeUInt32LE(cursor, o + 8);
        const padded = e.data.length % 2 ? Buffer.concat([e.data, Buffer.alloc(1)]) : e.data;
        area.push(padded);
        cursor += padded.length;
      }
    });
    return b;
  };
  const head = Buffer.from([0x49, 0x49, 42, 0, 8, 0, 0, 0]);
  const a = writeIfd(ifd0);
  const b = writeIfd(exif);
  return Buffer.concat([head, a, b, ...area]);
}

function synthExifJpeg(seed: number): Buffer {
  const p = new Prng(seed);
  const pad2 = (n: number) => String(n).padStart(2, "0");
  const stamp = () =>
    `20${pad2(p.below(25))}:${pad2(1 + p.below(12))}:${pad2(1 + p.below(28))} ${pad2(p.below(24))}:${pad2(p.below(60))}:${pad2(p.below(60))}`;
  const make = p.word(8);
  const model = p.word(10);
  const orientation = 1 + p.below(8);
  const dt = stamp();
  const exposureDen = 1 + p.below(4000);
  const fnum = 10 + p.below(200);
  const iso = 100 * (1 + p.below(32));
  const dto = stamp();
  const focal = 10 + p.below(200);
  const ifd0: Entry[] = [
    { tag: 0x010f, type: 2, count: make.length + 1, data: ascii(make) },
    { tag: 0x0110, type: 2, count: model.length + 1, data: ascii(model) },
    { tag: 0x0112, type: 3, count: 1, data: short(orientation) },
    { tag: 0x0132, type: 2, count: 20, data: ascii(dt) },
  ];
  const exif: Entry[] = [
    { tag: 0x829a, type: 5, count: 1, data: rational(1, exposureDen) },
    { tag: 0x829d, type: 5, count: 1, data: rational(fnum, 10) },
    { tag: 0x8827, type: 3, count: 1, data: short(iso) },
    { tag: 0x9003, type: 2, count: 20, data: ascii(dto) },
    { tag: 0x920a, type: 5, count: 1, data: rational(focal, 1) },
  ];
  const tiff = buildTiff(ifd0, exif);
  const body = Buffer.concat([Buffer.from("Exif\0\0", "latin1"), tiff]);
  const len = Buffer.alloc(2);
  len.writeUInt16BE(body.length + 2);
  return Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xe1]), len, body, Buffer.from([0xff, 0xd9])]);
}

export async function media009(_ctx: unknown, args: SeedArgs) {
  const jpeg = synthExifJpeg(args.seed);
  const t = await exifr.parse(jpeg, {
    tiff: true,
    exif: true,
    mergeOutput: true,
    translateKeys: true,
    translateValues: false,
    reviveValues: false,
  });
  return {
    make: t.Make as string,
    model: t.Model as string,
    orientation: t.Orientation as number,
    dateTime: t.ModifyDate as string,
    exposureTime: t.ExposureTime as number,
    fNumber: t.FNumber as number,
    iso: t.ISO as number,
    dateTimeOriginal: t.DateTimeOriginal as string,
    focalLength: t.FocalLength as number,
  };
}

export const media_009 = seedAction(media009);

// ---- media-010: WAV resample 48k -> 16k ------------------------------------

function synthWav(seed: number): Buffer {
  const p = new Prng(seed);
  const rate = 48000;
  const n = rate * 10;
  const wav = Buffer.alloc(44 + n * 2);
  wav.write("RIFF", 0, "latin1");
  wav.writeUInt32LE(36 + n * 2, 4);
  wav.write("WAVEfmt ", 8, "latin1");
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(rate, 24);
  wav.writeUInt32LE(rate * 2, 28);
  wav.writeUInt16LE(2, 32);
  wav.writeUInt16LE(16, 34);
  wav.write("data", 36, "latin1");
  wav.writeUInt32LE(n * 2, 40);
  const tau = Math.PI * 2;
  for (let i = 0; i < n; i++) {
    const t = i / rate;
    const s =
      0.5 * Math.sin(tau * 440 * t) +
      0.3 * Math.sin(tau * 1000 * t) +
      0.1 * Math.sin(tau * 3000 * t) +
      0.02 * (p.f64() - 0.5);
    wav.writeInt16LE(Math.round(s * 32767), 44 + i * 2);
  }
  return wav;
}

/**
 * Parse a 16-bit mono WAV, resample 48 kHz -> 16 kHz. Resampler outputs differ
 * numerically by design, so the output holds the exact integers (rates, sample
 * counts) and a boolean sanity check that the RMS energy is preserved (<2%).
 */
export async function media010(_ctx: unknown, args: SeedArgs) {
  const wav = synthWav(args.seed);
  let rate = 0;
  let pcm = new Int16Array(0);
  for (let o = 12; o + 8 <= wav.length; ) {
    const id = wav.toString("latin1", o, o + 4);
    const size = wav.readUInt32LE(o + 4);
    if (id === "fmt ") rate = wav.readUInt32LE(o + 12);
    else if (id === "data") {
      pcm = new Int16Array(size / 2);
      for (let i = 0; i < pcm.length; i++) pcm[i] = wav.readInt16LE(o + 8 + i * 2);
    }
    o += 8 + size + (size % 2);
  }
  const input = new Float64Array(pcm.length);
  for (let i = 0; i < pcm.length; i++) input[i] = pcm[i] / 32768;
  const outRate = 16000;
  const out = resample(input, rate, outRate, { method: "sinc" });
  const rms = (x: ArrayLike<number>) => {
    let s = 0;
    for (let i = 0; i < x.length; i++) s += x[i] * x[i];
    return Math.sqrt(s / x.length);
  };
  const ratio = rms(out) / rms(input);
  return {
    inRate: rate,
    outRate,
    inSamples: input.length,
    outSamples: out.length,
    rmsPreserved: Math.abs(ratio - 1) < 0.02,
  };
}

export const media_010 = seedAction(media010);

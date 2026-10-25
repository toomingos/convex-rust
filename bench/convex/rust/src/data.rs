//! Data-processing cases (data-*). Input generation mirrors
//! bench/convex/node/data.ts draw for draw.

use crate::prng::Prng;
use convex_fn::{action, Ctx, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Deserialize)]
pub struct SeedArgs {
    pub seed: u32,
}

const FNV_INIT: u32 = 0x811c_9dc5;

/// FNV-1a (32-bit) over bytes.
fn fnv(mut h: u32, s: &[u8]) -> u32 {
    for &b in s {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    h
}

/// Counts opening tags by name (`<name` followed by a non-name char).
fn count_tags(html: &str, names: &[&str]) -> BTreeMap<String, u32> {
    let mut out: BTreeMap<String, u32> = names.iter().map(|n| ((*n).to_string(), 0)).collect();
    let b = html.as_bytes();
    let mut i = 0;
    while let Some(p) = html[i..].find('<') {
        let start = i + p + 1;
        let mut j = start;
        while j < b.len() && b[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j > start {
            if let Some(c) = out.get_mut(&html[start..j].to_ascii_lowercase()) {
                *c += 1;
            }
        }
        i = start;
    }
    out
}

fn count_sub(s: &str, sub: &str) -> u32 {
    s.matches(sub).count() as u32
}

/// A word of `base + below(span)` letters.
fn wv(rng: &mut Prng, base: usize, span: u32) -> String {
    let n = base + rng.below(span) as usize;
    rng.word(n)
}

fn words(rng: &mut Prng, n: usize) -> String {
    let w: Vec<String> = (0..n).map(|_| wv(rng, 2, 7)).collect();
    w.join(" ")
}

// --- CSV ---
fn csv_records(seed: u32) -> Vec<Vec<String>> {
    let mut rng = Prng::new(seed);
    let mut rows = vec![vec!["id".to_string(), "name".into(), "qty".into(), "note".into()]];
    for i in 0..100_000 {
        let name = wv(&mut rng, 3, 8);
        let qty = rng.below(1000);
        let note = match rng.below(4) {
            0 => {
                let a = rng.word(4);
                format!("{a}, {}", rng.word(4))
            }
            1 => {
                let a = rng.word(3);
                format!("say \"{a}\" {}", rng.word(3))
            }
            2 => {
                let a = rng.word(3);
                format!("{a}\n{}", rng.word(3))
            }
            _ => rng.word(6),
        };
        rows.push(vec![(i + 1).to_string(), name, qty.to_string(), note]);
    }
    rows
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvParsed {
    rows: usize,
    qty_sum: u64,
    note_chars: usize,
    last: Vec<String>,
}

/// data-001: CSV parse 100k rows.
#[action]
pub async fn data_001(_ctx: &Ctx, args: SeedArgs) -> Result<CsvParsed> {
    let rows = csv_records(args.seed);
    let quote = |f: &str| {
        if f.contains([',', '"', '\n']) {
            format!("\"{}\"", f.replace('"', "\"\""))
        } else {
            f.to_string()
        }
    };
    let lines: Vec<String> = rows
        .iter()
        .map(|r| r.iter().map(|f| quote(f)).collect::<Vec<_>>().join(","))
        .collect();
    let text = lines.join("\n") + "\n";
    let mut rdr = csv::ReaderBuilder::new().has_headers(false).from_reader(text.as_bytes());
    let mut parsed: Vec<csv::StringRecord> = Vec::new();
    for rec in rdr.records() {
        parsed.push(rec.map_err(|e| convex_fn::Error::msg(e.to_string()))?);
    }
    let mut qty_sum = 0u64;
    let mut note_chars = 0usize;
    for r in parsed.iter().skip(1) {
        qty_sum += r[2].parse::<u64>().unwrap_or(0);
        note_chars += r[3].encode_utf16().count();
    }
    let last = parsed.last().map(|r| r.iter().map(str::to_string).collect()).unwrap_or_default();
    Ok(CsvParsed { rows: parsed.len(), qty_sum, note_chars, last })
}

#[derive(Serialize)]
pub struct Hashed {
    length: usize,
    hash: u32,
}

/// data-002: CSV write 100k rows.
#[action]
pub async fn data_002(_ctx: &Ctx, args: SeedArgs) -> Result<Hashed> {
    let rows = csv_records(args.seed);
    let mut w = csv::Writer::from_writer(Vec::new());
    for r in &rows {
        w.write_record(r).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    }
    let out = w.into_inner().map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    Ok(Hashed { length: out.len(), hash: fnv(FNV_INIT, &out) })
}

/// data-003: Markdown to HTML (~1 MB); returns tag counts.
#[action]
pub async fn data_003(_ctx: &Ctx, args: SeedArgs) -> Result<BTreeMap<String, u32>> {
    let mut rng = Prng::new(args.seed);
    let mut parts: Vec<String> = Vec::new();
    let mut len = 0usize;
    while len < 1_000_000 {
        let block = match rng.below(6) {
            0 => format!("# {}", words(&mut rng, 3)),
            1 => format!("## {}", words(&mut rng, 3)),
            2 => format!("### {}", words(&mut rng, 3)),
            3 => {
                let n = 8 + rng.below(13);
                let mut ws: Vec<String> = Vec::new();
                for _ in 0..n {
                    let w = wv(&mut rng, 2, 7);
                    ws.push(match rng.below(10) {
                        0 => format!("*{w}*"),
                        1 => format!("**{w}**"),
                        2 => format!("[{w}](http://example.com/{w})"),
                        3 => format!("`{w}`"),
                        _ => w,
                    });
                }
                ws.join(" ")
            }
            4 => {
                let n = 2 + rng.below(4);
                let items: Vec<String> = (0..n)
                    .map(|_| {
                        let k = 3 + rng.below(4) as usize;
                        format!("- {}", words(&mut rng, k))
                    })
                    .collect();
                items.join("\n")
            }
            _ => {
                let n = 1 + rng.below(4);
                let lines: Vec<String> = (0..n).map(|_| words(&mut rng, 4)).collect();
                format!("```\n{}\n```", lines.join("\n"))
            }
        };
        len += block.len() + 2;
        parts.push(block);
    }
    let md = parts.join("\n\n") + "\n";
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(&md));
    Ok(count_tags(&html, &["h1", "h2", "h3", "p", "ul", "li", "a", "em", "strong", "pre", "code"]))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sanitized {
    tags: BTreeMap<String, u32>,
    href: u32,
    script: u32,
    alert: u32,
    on_attr: u32,
}

/// data-004: HTML sanitize (~1 MB).
#[action]
pub async fn data_004(_ctx: &Ctx, args: SeedArgs) -> Result<Sanitized> {
    let mut rng = Prng::new(args.seed);
    let mut parts: Vec<String> = Vec::new();
    let mut len = 0usize;
    while len < 1_000_000 {
        let frag = match rng.below(9) {
            0 => format!("<p>{}</p>", words(&mut rng, 6)),
            1 => {
                let a = rng.word(5);
                format!("<b>{a}</b> <i>{}</i>", rng.word(5))
            }
            2 => {
                let w = rng.word(6);
                format!("<a href=\"http://example.com/{w}\">{w}</a>")
            }
            3 => format!("<a href=\"javascript:alert(1)\">{}</a>", rng.word(5)),
            4 => format!("<script>alert(\"{}\")</script>", rng.word(4)),
            5 => "<img src=\"x\" onerror=\"alert(1)\">".to_string(),
            6 => format!("<div onclick=\"x()\">{}</div>", words(&mut rng, 3)),
            7 => {
                let a = rng.word(5);
                format!("<ul><li>{a}</li><li>{}</li></ul>", rng.word(5))
            }
            _ => format!("<p style=\"color:red\">{}</p>", words(&mut rng, 4)),
        };
        len += frag.len() + 1;
        parts.push(frag);
    }
    let html = parts.join("\n");
    let clean = ammonia::Builder::empty()
        .tags(HashSet::from(["p", "b", "i", "a", "ul", "li"]))
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"]))]))
        .url_schemes(HashSet::from(["http", "https", "mailto"]))
        .clean_content_tags(HashSet::from(["script", "style"]))
        .link_rel(None)
        .clean(&html)
        .to_string();
    Ok(Sanitized {
        tags: count_tags(&clean, &["p", "b", "i", "a", "ul", "li", "div", "script", "img"]),
        href: count_sub(&clean, "href="),
        script: count_sub(&clean, "script"),
        alert: count_sub(&clean, "alert"),
        on_attr: count_sub(&clean, " onclick") + count_sub(&clean, " onerror") + count_sub(&clean, "style="),
    })
}

#[derive(Serialize)]
pub struct Dates {
    count: usize,
    sum: i64,
    first: i64,
    last: i64,
}

/// data-005: parse 100k ISO-8601 strings.
#[action]
pub async fn data_005(_ctx: &Ctx, args: SeedArgs) -> Result<Dates> {
    let mut rng = Prng::new(args.seed);
    let mut strs: Vec<String> = Vec::with_capacity(100_000);
    for _ in 0..100_000 {
        let y = 1970 + rng.below(131);
        let mo = 1 + rng.below(12);
        let d = 1 + rng.below(28);
        let h = rng.below(24);
        let mi = rng.below(60);
        let s = rng.below(60);
        let ms = rng.below(1000);
        strs.push(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z"));
    }
    let (mut sum, mut first, mut last) = (0i64, 0i64, 0i64);
    for (i, s) in strs.iter().enumerate() {
        let t = chrono::DateTime::parse_from_rfc3339(s)
            .map_err(|e| convex_fn::Error::msg(e.to_string()))?
            .timestamp_millis();
        if i == 0 {
            first = t;
        }
        last = t;
        sum += t % 1_000_000_007;
    }
    Ok(Dates { count: strs.len(), sum, first, last })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Zones {
    count: usize,
    acc: u64,
    first_local: u64,
}

/// data-006: convert 100k instants to local civil time in 5 zones.
#[action]
pub async fn data_006(_ctx: &Ctx, args: SeedArgs) -> Result<Zones> {
    use chrono::{Datelike, TimeZone, Timelike};
    let mut rng = Prng::new(args.seed);
    let names = ["America/New_York", "Europe/Lisbon", "Asia/Tokyo", "Australia/Sydney", "Asia/Kolkata"];
    let zones: Vec<chrono_tz::Tz> = names.iter().map(|n| n.parse().unwrap()).collect();
    let (mut acc, mut first_local) = (0u64, 0u64);
    for i in 0..100_000 {
        let secs = 946_684_800 + i64::from(rng.below(946_080_000));
        let zone = zones[rng.below(zones.len() as u32) as usize];
        let t = chrono::Utc.timestamp_opt(secs, 0).unwrap().with_timezone(&zone);
        let local = (((t.year() as u64 * 12 + u64::from(t.month())) * 31 + u64::from(t.day())) * 24
            + u64::from(t.hour()))
            * 60
            + u64::from(t.minute());
        if i == 0 {
            first_local = local;
        }
        acc = (acc * 31 + local) % 1_000_000_007;
    }
    Ok(Zones { count: 100_000, acc, first_local })
}

const NFC_PIECES: [&str; 14] = [
    "abc ",
    "e\u{301}",
    "\u{e9}",
    "a\u{308}",
    "n\u{303}",
    "o\u{302}\u{323}",
    "o\u{323}\u{302}",
    "\u{212b}",
    "A\u{30a}",
    "\u{1112}\u{1161}\u{11ab}",
    "\u{d55c}",
    "c\u{327}\u{301}",
    "\u{c7}",
    "x",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Nfc {
    in_code_points: usize,
    out_code_points: usize,
    out_utf8_bytes: usize,
    hash: u32,
}

/// data-007: NFC-normalize ~10 MB of mixed composed/decomposed text.
#[action]
pub async fn data_007(_ctx: &Ctx, args: SeedArgs) -> Result<Nfc> {
    use unicode_normalization::UnicodeNormalization;
    let mut rng = Prng::new(args.seed);
    let mut input = String::new();
    for _ in 0..3_000_000 {
        input.push_str(NFC_PIECES[rng.below(NFC_PIECES.len() as u32) as usize]);
    }
    let out: String = input.nfc().collect();
    let mut h = FNV_INIT;
    let mut out_cp = 0;
    for ch in out.chars() {
        out_cp += 1;
        h = (h ^ ch as u32).wrapping_mul(16_777_619);
    }
    Ok(Nfc {
        in_code_points: input.chars().count(),
        out_code_points: out_cp,
        out_utf8_bytes: out.len(),
        hash: h,
    })
}

#[derive(Serialize)]
pub struct TopWord {
    word: String,
    count: u32,
}

#[derive(Serialize)]
pub struct WordCount {
    words: u32,
    unique: usize,
    top: Vec<TopWord>,
}

/// data-008: word-tokenize and count 10 MB.
#[action]
pub async fn data_008(_ctx: &Ctx, args: SeedArgs) -> Result<WordCount> {
    use unicode_segmentation::UnicodeSegmentation;
    let text = Prng::new(args.seed).text(10 * 1024 * 1024);
    let mut counts: HashMap<&str, u32> = HashMap::new();
    let mut total = 0;
    for w in text.unicode_words() {
        total += 1;
        *counts.entry(w).or_insert(0) += 1;
    }
    let mut all: Vec<(&str, u32)> = counts.iter().map(|(w, c)| (*w, *c)).collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let top = all.iter().take(3).map(|(w, c)| TopWord { word: (*w).to_string(), count: *c }).collect();
    Ok(WordCount { words: total, unique: counts.len(), top })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffStats {
    lines_a: usize,
    lines_b: usize,
    added: u32,
    removed: u32,
    unchanged: u32,
}

/// data-009: line diff of two ~100 KB texts.
#[action]
pub async fn data_009(_ctx: &Ctx, args: SeedArgs) -> Result<DiffStats> {
    let mut rng = Prng::new(args.seed);
    let (mut a, mut b): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for _ in 0..2500 {
        let ws: Vec<String> = (0..5).map(|_| wv(&mut rng, 3, 6)).collect();
        let line = ws.join(" ") + "\n";
        a.push(line.clone());
        match rng.below(10) {
            0 => {}
            1 => {
                let x = rng.word(8);
                b.push(format!("{x} {}\n", rng.word(8)));
            }
            2 => {
                b.push(line);
                let x = rng.word(8);
                b.push(format!("{x} {}\n", rng.word(8)));
            }
            _ => b.push(line),
        }
    }
    let (ta, tb) = (a.concat(), b.concat());
    let diff = similar::TextDiff::from_lines(&ta, &tb);
    let (mut added, mut removed, mut unchanged) = (0, 0, 0);
    for c in diff.iter_all_changes() {
        match c.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => unchanged += 1,
        }
    }
    Ok(DiffStats { lines_a: a.len(), lines_b: b.len(), added, removed, unchanged })
}

const TEMPLATE: &str = "<li class=\"user\">{{name}} ({{age}}){{#if active}} <b>active</b>{{/if}}{{#each tags}} #{{this}}{{/each}}</li>\n";

/// data-010: render a Handlebars template 10k times.
#[action]
pub async fn data_010(_ctx: &Ctx, args: SeedArgs) -> Result<serde_json::Value> {
    let mut rng = Prng::new(args.seed);
    let mut reg = handlebars::Handlebars::new();
    reg.register_template_string("t", TEMPLATE).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    let (mut total, mut h, mut first) = (0usize, FNV_INIT, String::new());
    for i in 0..10_000 {
        let mut name = wv(&mut rng, 3, 6);
        if rng.below(4) == 0 {
            name.push_str(" & <co>");
        }
        let age = 18 + rng.below(60);
        let active = rng.below(2) == 0;
        let n = rng.below(4);
        let tags: Vec<String> = (0..n).map(|_| rng.word(4)).collect();
        let out = reg
            .render("t", &json!({ "name": name, "age": age, "active": active, "tags": tags }))
            .map_err(|e| convex_fn::Error::msg(e.to_string()))?;
        if i == 0 {
            first = out.clone();
        }
        total += out.encode_utf16().count();
        h = fnv(h, out.as_bytes());
    }
    Ok(json!({ "length": total, "hash": h, "first": first }))
}

fn valid_id(v: &serde_json::Value) -> std::result::Result<(), validator::ValidationError> {
    match v.as_u64() {
        Some(n) if n >= 1 => Ok(()),
        _ => Err(validator::ValidationError::new("id")),
    }
}

#[derive(Deserialize, validator::Validate)]
struct User {
    // Kept as a JSON value so a wrongly-typed id is reported by validation (not by an early
    // deserialize error) and every other field is still validated, like zod's issue list.
    #[validate(custom(function = "valid_id"))]
    #[allow(dead_code)]
    id: serde_json::Value,
    #[validate(email)]
    #[allow(dead_code)]
    email: String,
    #[validate(range(min = 0, max = 120))]
    #[allow(dead_code)]
    age: i32,
    #[validate(length(min = 1, max = 20))]
    #[allow(dead_code)]
    name: String,
    #[validate(length(max = 5))]
    #[allow(dead_code)]
    tags: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    valid: u32,
    invalid_id: u32,
    invalid_email: u32,
    invalid_age: u32,
    invalid_name: u32,
    invalid_tags: u32,
}

/// data-011: validate 10k objects with serde + validator.
#[action]
pub async fn data_011(_ctx: &Ctx, args: SeedArgs) -> Result<Validation> {
    use validator::Validate;
    let mut rng = Prng::new(args.seed);
    let mut bad: HashMap<String, u32> = HashMap::new();
    let mut valid = 0;
    for i in 0..10_000u32 {
        let mut id = json!(i + 1);
        let local = wv(&mut rng, 3, 8);
        let dom = wv(&mut rng, 3, 6);
        let mut email = format!("{local}@{dom}.com");
        let mut age = rng.below(121);
        let mut name = wv(&mut rng, 1, 20);
        let n = rng.below(6);
        let mut tags: Vec<String> = (0..n).map(|_| rng.word(3)).collect();
        match rng.below(10) {
            0 => id = json!("x"),
            1 => email = local,
            2 => age = 121 + rng.below(100),
            3 => name = wv(&mut rng, 21, 5),
            4 => {
                for _ in 0..6 + rng.below(3) {
                    tags.push(rng.word(3));
                }
            }
            _ => {}
        }
        let obj = json!({ "id": id, "email": email, "age": age, "name": name, "tags": tags });
        match serde_json::from_value::<User>(obj) {
            Err(_) => *bad.entry("id".to_string()).or_insert(0) += 1,
            Ok(u) => match u.validate() {
                Ok(()) => valid += 1,
                // All field errors are collected (validate() does not stop at the first).
                Err(e) => {
                    for f in e.field_errors().keys() {
                        *bad.entry(f.to_string()).or_insert(0) += 1;
                    }
                }
            },
        }
    }
    let g = |k: &str| bad.get(k).copied().unwrap_or(0);
    Ok(Validation {
        valid,
        invalid_id: g("id"),
        invalid_email: g("email"),
        invalid_age: g("age"),
        invalid_name: g("name"),
        invalid_tags: g("tags"),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YamlStats {
    items: usize,
    id_sum: i64,
    score_sum: i64,
    active: u32,
    tags: usize,
    owner_chars: usize,
    level_sum: i64,
}

/// data-012: parse ~1 MB of YAML.
#[action]
pub async fn data_012(_ctx: &Ctx, args: SeedArgs) -> Result<YamlStats> {
    let mut rng = Prng::new(args.seed);
    let mut doc = String::new();
    let mut i = 1;
    while doc.len() < 1_000_000 {
        let name = wv(&mut rng, 4, 5);
        let active = rng.below(2) == 0;
        let score = rng.below(10_000);
        doc.push_str(&format!("- id: {i}\n  name: {name}\n  active: {active}\n  score: {score}\n  tags:\n"));
        for _ in 0..rng.below(4) {
            doc.push_str(&format!("    - {}\n", rng.word(5)));
        }
        let owner = rng.word(6);
        doc.push_str(&format!("  meta:\n    owner: {owner}\n    level: {}\n", rng.below(10)));
        i += 1;
    }
    let v: serde_yaml::Value = serde_yaml::from_str(&doc).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    let seq = v.as_sequence().ok_or_else(|| convex_fn::Error::msg("not a sequence"))?;
    let mut s = YamlStats { items: seq.len(), id_sum: 0, score_sum: 0, active: 0, tags: 0, owner_chars: 0, level_sum: 0 };
    for it in seq {
        s.id_sum += it["id"].as_i64().unwrap_or(0);
        s.score_sum += it["score"].as_i64().unwrap_or(0);
        if it["active"].as_bool() == Some(true) {
            s.active += 1;
        }
        s.tags += it["tags"].as_sequence().map_or(0, Vec::len);
        s.owner_chars += it["meta"]["owner"].as_str().map_or(0, str::len);
        s.level_sum += it["meta"]["level"].as_i64().unwrap_or(0);
    }
    Ok(s)
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct XmlStats {
    items: u32,
    id_sum: u64,
    price_sum: u64,
    name_chars: usize,
    cat_chars: usize,
    tags: u32,
}

/// data-013: parse ~5 MB of XML.
#[action]
pub async fn data_013(_ctx: &Ctx, args: SeedArgs) -> Result<XmlStats> {
    let mut rng = Prng::new(args.seed);
    let mut xml = String::from("<catalog>\n");
    let mut i = 1;
    while xml.len() < 5_000_000 {
        let cat = rng.word(3);
        let name = wv(&mut rng, 5, 8);
        let price = rng.below(100_000);
        let mut tags = String::new();
        for _ in 0..1 + rng.below(4) {
            tags.push_str(&format!("<tag>{}</tag>", rng.word(4)));
        }
        xml.push_str(&format!(
            "<item id=\"{i}\" cat=\"{cat}\"><name>{name}</name><price>{price}</price><tags>{tags}</tags></item>\n"
        ));
        i += 1;
    }
    xml.push_str("</catalog>\n");

    // Build the full document tree (like Node's fast-xml-parser object tree), then reduce over it.
    let doc = roxmltree::Document::parse(&xml).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
    let mut s = XmlStats::default();
    for it in doc.root_element().children().filter(|n| n.has_tag_name("item")) {
        s.items += 1;
        s.id_sum += it.attribute("id").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        s.cat_chars += it.attribute("cat").map_or(0, str::len);
        for c in it.children().filter(|n| n.is_element()) {
            match c.tag_name().name() {
                "name" => s.name_chars += c.text().map_or(0, str::len),
                "price" => s.price_sum += c.text().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0),
                "tags" => s.tags += c.children().filter(|n| n.has_tag_name("tag")).count() as u32,
                _ => {}
            }
        }
    }
    Ok(s)
}

#[derive(Serialize)]
pub struct UrlStats {
    count: usize,
    params: usize,
    len: usize,
    hash: u32,
}

/// data-014: parse 100k URLs.
#[action]
pub async fn data_014(_ctx: &Ctx, args: SeedArgs) -> Result<UrlStats> {
    let mut rng = Prng::new(args.seed);
    let schemes = ["http", "https", "ws"];
    let tlds = ["com", "org", "net"];
    let ports = [80, 443, 8080, 3000];
    let mut urls: Vec<String> = Vec::with_capacity(100_000);
    for _ in 0..100_000 {
        let scheme = schemes[rng.below(3) as usize];
        let mut host = format!("{}.{}", wv(&mut rng, 3, 8), tlds[rng.below(3) as usize]);
        if rng.below(3) == 0 {
            host = host.to_uppercase();
        }
        let port = if rng.below(4) == 0 { format!(":{}", ports[rng.below(4) as usize]) } else { String::new() };
        let mut user = String::new();
        if rng.below(5) == 0 {
            let u = rng.word(3);
            user = format!("u{u}:p{}@", rng.word(3));
        }
        let mut path = String::new();
        for _ in 0..rng.below(4) {
            path.push('/');
            if rng.below(5) == 0 {
                let a = rng.word(3);
                path.push_str(&format!("{a} {}", rng.word(2)));
            } else {
                path.push_str(&wv(&mut rng, 3, 6));
            }
        }
        let mut query = String::new();
        if rng.below(2) == 0 {
            let mut ps: Vec<String> = Vec::new();
            for _ in 0..1 + rng.below(3) {
                let key = rng.word(2);
                ps.push(format!("{key}={}", rng.word(3)));
            }
            query = format!("?{}", ps.join("&"));
        }
        let frag = if rng.below(3) == 0 { format!("#{}", rng.word(4)) } else { String::new() };
        urls.push(format!("{scheme}://{user}{host}{port}{path}{query}{frag}"));
    }
    let (mut h, mut params, mut len) = (FNV_INIT, 0, 0);
    for s in &urls {
        let u = url::Url::parse(s).map_err(|e| convex_fn::Error::msg(e.to_string()))?;
        params += u.query_pairs().count();
        let sig = format!(
            "{}|{}|{}|{}|{}|{}|{}\n",
            u.host_str().unwrap_or(""),
            u.port().map(|p| p.to_string()).unwrap_or_default(),
            u.username(),
            u.password().unwrap_or(""),
            u.path(),
            u.query().map(|q| format!("?{q}")).unwrap_or_default(),
            u.fragment().map(|f| format!("#{f}")).unwrap_or_default(),
        );
        len += sig.len();
        h = fnv(h, sig.as_bytes());
    }
    Ok(UrlStats { count: urls.len(), params, len, hash: h })
}

#[derive(Serialize)]
pub struct SlugStats {
    count: usize,
    len: usize,
    hash: u32,
    first: Vec<String>,
}

/// data-015: slugify 100k strings.
#[action]
pub async fn data_015(_ctx: &Ctx, args: SeedArgs) -> Result<SlugStats> {
    let mut rng = Prng::new(args.seed);
    let accents = ["é", "ü", "ñ", "å"];
    let mut inputs: Vec<String> = Vec::with_capacity(100_000);
    for _ in 0..100_000 {
        let mut s = String::new();
        for _ in 0..2 + rng.below(5) {
            let mut w = wv(&mut rng, 2, 8);
            if rng.below(4) == 0 {
                w[..1].make_ascii_uppercase();
            }
            if rng.below(8) == 0 {
                w.push_str(accents[rng.below(4) as usize]);
            }
            if rng.below(5) == 0 {
                w.push(if rng.below(2) == 0 { ',' } else { '.' });
            }
            s.push_str(&w);
            s.push_str(if rng.below(6) == 0 { "  " } else { " " });
        }
        inputs.push(s.trim_end().to_string());
    }
    let (mut h, mut len, mut first) = (FNV_INIT, 0, Vec::new());
    for s in &inputs {
        let slug = slug::slugify(s);
        if first.len() < 3 {
            first.push(slug.clone());
        }
        len += slug.len();
        h = fnv(h, slug.as_bytes());
        h = fnv(h, b"\n");
    }
    Ok(SlugStats { count: inputs.len(), len, hash: h, first })
}

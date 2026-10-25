# Benchmark fairness

**Rule.** Allowed: intrinsic language/runtime/compiler/standard-ecosystem advantages, each side written as a normal user of
that language would (zero-copy borrows, lazy iterators, typed serde decode, platform APIs). Not allowed: a different
algorithm, parameters or output quality, skipped work the result needs, bench-code waste on one side, or hand-tuning only one side.

Per-case audit of whether the Node and Rust sides do the same work with an equivalent algorithm.
`✅` = same work. `ℹ️` = a difference that is inherent or deliberately left, labeled in reports.

Fixes made in the fairness pass: data-006/011/013 (Rust now does the full work Node does), media-002/003/004/005/006/007/010,
cpu-016, crypto-003; cpu-012 inputs hidden from constant folding; cpu-020 uses the standard strsim crate instead of hand-tuned code.
Allowed as intrinsic (labeled): cpu-017, data-008, data-014, net-006, cpu-013/019, mem-002/003, media-008, net-009, sharp multithreading,
crypto-008, cpu-010 (same loop order).

sharp (libvips) runs on its default thread pool in media-001..005; Rust is single-threaded (wasm has no threads), which favors Node.

| Case | Title | Verdict | Note |
|---|---|---|---|
| base-001 | noop | ✅ | Same empty action on both sides. |
| base-002 | echo small args (1 field) | ✅ | Same echo of one small field. |
| base-003 | return 1 MB value | ✅ | Same 1 MB value built and returned. |
| base-004 | throw ConvexError with data | ✅ | Same ConvexError with identical data payload. |
| base-005 | 100 × console.log / log! | ✅ | Same 100 log lines per call. |
| cb-001 | runQuery ×1 | ✅ | Same single runQuery against the same target. |
| cb-002 | runQuery ×10 sequential | ✅ | Same 10 sequential runQuery calls. |
| cb-003 | runQuery ×10 parallel | ✅ | Same 10 parallel runQuery calls. |
| cb-004 | runQuery ×100 sequential | ✅ | Same 100 sequential runQuery calls. |
| cb-005 | runQuery ×100 parallel | ✅ | Same 100 parallel runQuery calls. |
| cb-006 | runMutation ×1 | ✅ | Same single runMutation. |
| cb-007 | runMutation ×10 sequential | ✅ | Same 10 sequential runMutation calls. |
| cb-008 | runAction cross-runtime (Node↔isolate, Rust↔isolate) | ✅ | Same cross-runtime runAction call. |
| cb-009 | runAction chain of 3 | ✅ | Same chain of three runAction calls. |
| cb-010 | scheduler.runAfter(0) ×1 | ✅ | Same scheduler.runAfter(0) call. |
| cb-011 | scheduler.runAt ×10 | ✅ | Same 10 scheduler.runAt calls. |
| cb-012 | storage.store 1 MB blob | ✅ | Same 1 MB blob stored. |
| cb-013 | storage.get 1 MB blob | ✅ | Same 1 MB blob read back. |
| cb-014 | auth.getUserIdentity | ✅ | Same auth.getUserIdentity call. |
| cb-015 | read 10 env vars | ✅ | Same ten environment variable reads. |
| ser-001 | args 1 KB object | ✅ | Same 1 KB argument decode and return. |
| ser-002 | args 100 KB object | ✅ | Same 100 KB argument decode and return. |
| ser-003 | args 1 MB object | ✅ | Same 1 MB argument decode and return. |
| ser-004 | args 5 MB object | ✅ | Same 5 MB argument decode and return. |
| ser-005 | 14-level nested object | ✅ | Same 14-level nested value. |
| ser-006 | array of 8k objects | ℹ️ | Rust decodes into typed structs (idiomatic); Node uses plain objects. |
| ser-007 | bytes 1 MB | ✅ | Same 1 MB byte payload. |
| ser-008 | int64 / bigint values | ℹ️ | Rust decodes into typed structs (idiomatic); Node uses plain objects. |
| ser-009 | float specials (NaN, ±Inf, -0) | ℹ️ | Rust decodes into typed structs (idiomatic); Node uses plain objects. |
| ser-010 | Unicode-heavy strings (emoji, CJK, RTL) | ℹ️ | Rust decodes into typed structs (idiomatic); Node uses plain objects. |
| cpu-001 | sha256 10 MB | ✅ | Same PRNG input, one SHA-256 over 10 MB; both native code. |
| cpu-002 | sha512 10 MB | ✅ | Same PRNG input, one SHA-512 over 10 MB; both native code. |
| cpu-003 | blake3 10 MB | ℹ️ | Noble (JS) vs blake3 crate; its SIMD only on native x86, not wasm. |
| cpu-004 | md5 10 MB | ✅ | Same MD5 over 10 MB. |
| cpu-005 | crc32 10 MB | ✅ | Same CRC-32 over 10 MB. |
| cpu-006 | recursive fib(30) | ℹ️ | LLVM may turn the recursion into a loop; V8 does not. |
| cpu-007 | prime sieve to 10M | ✅ | Same sieve algorithm, same bound. |
| cpu-008 | sort 1M ints | ✅ | Same 1M-integer sort from the same PRNG draws. |
| cpu-009 | sort 100k strings | ✅ | Same 100k-string sort, same comparison order. |
| cpu-010 | matrix multiply 256×256 f64 | ✅ | Same ijk loop order on both sides. |
| cpu-011 | n-body, 1000 steps | ✅ | Same n-body integrator, same steps. |
| cpu-012 | mandelbrot 1000×1000 | ✅ | Rust parameters pass through black_box; same loop and arithmetic as Node. |
| cpu-013 | regex scan over 10 MB | ℹ️ | Backtracking (V8 Irregexp) vs automaton (Rust regex) engines. |
| cpu-014 | JSON parse 10 MB | ✅ | Same 10 MB JSON parsed into a generic value tree. |
| cpu-015 | JSON stringify 10 MB | ✅ | Same document serialized to a 10 MB JSON string. |
| cpu-016 | base64 encode 10 MB | ✅ | Same PRNG bytes; Node encodes a zero-copy Buffer view. |
| cpu-017 | base64 decode 10 MB | ℹ️ | Rust decodes bytes directly; Node API needs a string (intrinsic). |
| cpu-018 | gzip compress 10 MB | ✅ | Same gzip level 6 and round-trip check. |
| cpu-019 | zstd compress 10 MB | ℹ️ | C zstd vs pure-Rust ruzstd; slows Rust, not flattering. |
| cpu-020 | Levenshtein, 1k string pairs | ℹ️ | Rust uses standard strsim crate; Node uses fastest-levenshtein; same distances. |
| data-001 | CSV parse 100k rows | ✅ | Same CSV, same rows, parsed into records. |
| data-002 | CSV write 100k rows | ✅ | Same 100k rows written to CSV text. |
| data-003 | Markdown → HTML 1 MB | ✅ | Same Markdown input rendered to HTML. |
| data-004 | HTML sanitize 1 MB | ✅ | Same HTML input sanitized with a comparable allowlist. |
| data-005 | date parse 100k ISO strings | ✅ | Same 100k ISO strings parsed to epoch millis. |
| data-006 | timezone convert 100k | ✅ | Both convert every instant to its zone via platform API, read fields. |
| data-007 | Unicode NFC normalize 10 MB | ✅ | Same NFC normalization of the same text. |
| data-008 | word tokenize and count 10 MB | ℹ️ | Rust borrows slices (intrinsic zero-copy); Node allocates segment objects. |
| data-009 | line diff of 100 KB texts | ✅ | Same line diff of the same texts. |
| data-010 | template render ×10k | ✅ | Same template compiled once, rendered 10k times. |
| data-011 | schema validate 10k objects | ✅ | Both validate every field of every object and collect all errors. |
| data-012 | YAML parse 1 MB | ✅ | Same 1 MB YAML parsed into a full value tree. |
| data-013 | XML parse 5 MB | ✅ | Both build the full document tree (roxmltree vs fast-xml-parser), then reduce. |
| data-014 | URL parse 100k | ℹ️ | Rust iterates lazily and borrows; Node allocates getter strings (intrinsic). |
| data-015 | slugify 100k strings | ✅ | Same slugify rules on the same 100k strings. |
| crypto-001 | HMAC-SHA256 ×100k | ℹ️ | Wasm uses host WebCrypto; Node uses native crypto too. |
| crypto-002 | JWT sign HS256 ×10k | ℹ️ | Wasm uses host WebCrypto; Node uses native crypto too. |
| crypto-003 | JWT verify RS256 ×1k | ℹ️ | Wasm uses host WebCrypto; both cache the imported key across calls. |
| crypto-004 | AES-256-GCM encrypt 10 MB | ℹ️ | Wasm uses host WebCrypto; Node uses native crypto too. |
| crypto-005 | AES-256-GCM decrypt 10 MB | ℹ️ | Wasm uses host WebCrypto; Node uses native crypto too. |
| crypto-006 | Ed25519 sign ×10k | ℹ️ | Wasm uses host WebCrypto; Node uses native crypto too. |
| crypto-007 | Ed25519 verify ×10k | ℹ️ | Wasm uses host WebCrypto; Node uses native crypto too. |
| crypto-008 | argon2id hash (m=19MiB, t=2) | ℹ️ | argon2 C binding vs pure Rust, same parameters. |
| crypto-009 | bcrypt hash cost 10 | ℹ️ | bcrypt cost 10, same salt on both sides. |
| crypto-010 | UUID v4 and v7 ×100k | ℹ️ | UUIDv7 layouts differ; compared by shape only. |
| media-001 | decode 4K PNG | ℹ️ | sharp uses its default thread pool; Rust is single-threaded (favors Node). |
| media-002 | resize 4K JPEG → 1080p | ℹ️ | Both JPEG q80 4:4:4 with default Huffman, full decode, Lanczos3; sharp is multithreaded. |
| media-003 | thumbnail 256 px | ℹ️ | Both Lanczos3 to 256x192 (Rust via fast_image_resize); sharp multithreaded. |
| media-004 | PNG → WebP | ℹ️ | Both lossless WebP, fastest effort; encoders differ, output bytes reported. |
| media-005 | Gaussian blur 1080p | ℹ️ | Both true Gaussian, sigma 5, 31-tap; sharp multithreaded, Rust single. |
| media-006 | QR code → PNG | ✅ | Both build one QR, render 8-bit gray PNG, zlib 6, Sub filter. |
| media-007 | SVG → PNG | ✅ | Same SVG; neither side loads system fonts. |
| media-008 | PDF text extraction | ℹ️ | pdf.js layout analysis vs lopdf raw text extraction. |
| media-009 | read EXIF | ✅ | Same EXIF fields read from the same JPEG. |
| media-010 | WAV resample 48k → 16k | ℹ️ | Both Butterworth 16 + windowed sinc; kernel length and window approximate. |
| net-001 | GET 1 KB | ✅ | Same 1 KB GET against the same mock server. |
| net-002 | GET 10 MB | ℹ️ | Wasm copies the body host to guest (inherent). |
| net-003 | POST JSON 1 MB | ✅ | Same 1 MB JSON POST. |
| net-004 | 20 GETs in parallel | ✅ | Same 20 parallel GETs. |
| net-005 | 20 GETs sequential | ✅ | Same 20 sequential GETs. |
| net-006 | streamed body, hash while reading | ✅ | Both stream the body and hash incrementally; same digest. |
| net-007 | fetch then storage.store | ✅ | Same 64 KiB fetch then storage.store. |
| net-008 | fetch then runMutation | ✅ | Same fetch then one mutation. |
| net-009 | timeout handling (server sleeps) | ℹ️ | Rust cannot cancel the losing fetch; phase-0 sleep is instant (phase-2 real). |
| net-010 | follow 5 redirects | ✅ | Same five redirects followed automatically. |
| mem-001 | allocate and touch 100 MB | ✅ | Same 100 MB allocated and touched. |
| mem-002 | 1M small allocations | ℹ️ | Uint32Array vs Box<[u32;4]> small allocations. |
| mem-003 | fan-out/join 100 async tasks | ℹ️ | Promise.all vs join_all; phase-0 sleep is instant (phase-2 real). |
| mem-004 | hash map insert 1M | ✅ | Same 1M insertions into the language hash map. |
| mem-005 | build a 10 MB string by concatenation | ✅ | Same 10 MB string built by repeated concatenation. |

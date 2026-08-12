# oximedia-compat-ffmpeg

FFmpeg CLI argument compatibility layer for [OxiMedia](https://github.com/cool-japan/oximedia).

Version: 0.2.1 — 2026-08-12 — extensively tested

## Overview

This crate parses FFmpeg-style command-line arguments and translates them into native OxiMedia
transcode operations. It provides a familiar interface for users migrating from FFmpeg, while
automatically mapping patent-encumbered codecs (e.g., `libx264`, `aac`) to OxiMedia's patent-free
alternatives (AV1, Opus).

### Key Components

- **Argument Parser** — Parses FFmpeg-style flags (`-i`, `-c:v`, `-c:a`, `-crf`, `-vf`, `-af`, `-map`, etc.)
- **Codec Map** — 80+ codec name mappings with semantic categories (direct match, patent-substituted, copy)
- **Filter Complex Parser** — Recursive-descent parser for `-filter_complex` graphs (`filter_complex.rs`)
- **Filter Lexer** — Parses simpler FFmpeg filter graph syntax (`-vf`, `-af`)
- **Stream Specifier** — Handles FFmpeg stream selection notation (`0:v`, `0:a:1`, `[label]`, etc.)
- **Seek / Duration** — Parses `-ss`, `-to`, `-t` duration strings in all FFmpeg formats
- **ffprobe Output** — Formats probe results as JSON, XML, CSV, or flat key=value output
- **Translator** — Converts parsed arguments into `TranscodeJob` structs for OxiMedia's transcoding engine
- **Diagnostics** — Structured warnings and errors formatted in FFmpeg style

## Usage

```rust
use oximedia_compat_ffmpeg::parse_and_translate;

let args: Vec<String> = vec![
    "-i".into(), "input.mkv".into(),
    "-c:v".into(), "libaom-av1".into(),
    "-crf".into(), "28".into(),
    "-c:a".into(), "libopus".into(),
    "output.webm".into(),
];

let result = parse_and_translate(&args);
for diag in &result.diagnostics {
    eprintln!("{}", diag.format_ffmpeg_style("oximedia-ff"));
}
```

## Feature Details

### `-filter_complex` Parsing — `FilterGraph::parse()`

Recursive-descent parser for the full FFmpeg `-filter_complex` syntax. Produces an AST of
filter chains with input/output pad labels and per-filter key=value options.

```rust
use oximedia_compat_ffmpeg::filter_complex::FilterGraph;

// Single-chain scale filter
let g = FilterGraph::parse("[in]scale=1280:720[out]").unwrap();
assert_eq!(g.chains.len(), 1);

// Multi-chain graph with overlay
let g = FilterGraph::parse(
    "[0:v]scale=1920:1080[bg];[bg][1:v]overlay=x=10:y=10[out]"
).unwrap();
assert_eq!(g.chains.len(), 2);
```

Grammar (BNF):
```text
filter_graph  := filter_chain (';' filter_chain)*
filter_chain  := input_pad* filter_node+ output_pad*
filter_node   := name ('=' option_list)?
option_list   := option (',' option)* | option (':' option)*
option        := key '=' value | value
input_pad     := '[' label ']'
output_pad    := '[' label ']'
```

### `-map` Stream Specifiers — `StreamSelector::parse()`

Parses the full range of FFmpeg stream specifiers used with the `-map` flag:

```rust
use oximedia_compat_ffmpeg::stream_spec::StreamSelector;

// First video stream of input file 0
let sel = StreamSelector::parse("0:v:0").expect("ok");

// Exclude a stream (negative map)
let excl = StreamSelector::parse("-0:a:1").expect("ok");

// Filter-complex output pad label
let lbl = StreamSelector::parse("[out_v]").expect("ok");

// Metadata-keyed selector
let meta = StreamSelector::parse("0:m:language:eng").expect("ok");
```

Supported forms: `file:type:index`, `file:type:#pid`, `file:m:key:value`, `[label]`,
and negated variants (`-…`).

### `-ss` / `-to` / `-t` Duration Parsing — `parse_duration()`

Converts FFmpeg duration strings into `std::time::Duration`:

```rust
use oximedia_compat_ffmpeg::seek::parse_duration;
use std::time::Duration;

assert_eq!(parse_duration("01:23:45.500").unwrap(), Duration::from_millis(5_025_500));
assert_eq!(parse_duration("2h").unwrap(),           Duration::from_secs(7_200));
assert_eq!(parse_duration("90m").unwrap(),          Duration::from_secs(5_400));
assert_eq!(parse_duration("300s").unwrap(),         Duration::from_secs(300));
assert_eq!(parse_duration("45.678").unwrap(),       Duration::from_millis(45_678));
```

`check_seek_args()` validates that `-to` and `-t` are not both specified (which FFmpeg
also rejects as a conflict).

### `ffprobe -of json/xml/csv/default` — `format_probe_result()`

Renders a `ProbeOutput` struct in any of the four standard ffprobe output formats:

```rust
use oximedia_compat_ffmpeg::ffprobe::{ProbeFormat, ProbeStream, ProbeOutput};
use oximedia_compat_ffmpeg::ffprobe_output::{FfprobeOutputFormat, format_probe_result};

let stream = ProbeStream::new_video("av1", 1920, 1080, "16:9", 30.0);
let format = ProbeFormat::new("clip.mkv", "matroska,webm", 100_000_000, 60.0);
let output = ProbeOutput { format: Some(format), streams: vec![stream] };

// JSON (-print_format json)
let json = format_probe_result(&output, FfprobeOutputFormat::Json).unwrap();
assert!(json.contains("\"codec_name\""));

// XML (-print_format xml)
let xml = format_probe_result(&output, FfprobeOutputFormat::Xml).unwrap();
assert!(xml.contains("<ffprobe "));  // tag has xmlns attributes

// CSV (-print_format csv)
let csv = format_probe_result(&output, FfprobeOutputFormat::Csv).unwrap();

// Flat key=value (-print_format flat / default)
let flat = format_probe_result(&output, FfprobeOutputFormat::Default).unwrap();
```

### APV and MJPEG Codec Aliases

The codec map includes direct-match entries for:

- `"apv"` / `"apv1"` → OxiMedia `apv` (ISO/IEC 23009-13 royalty-free intra-frame codec)
- `"mjpeg"` / `"mjpegb"` → OxiMedia `mjpeg`

```rust
use oximedia_compat_ffmpeg::codec_map::CodecMap;

let map = CodecMap::new();
let apv = map.lookup("apv").expect("APV is supported");
assert_eq!(apv.oxi_name, "apv");

let mjpeg = map.lookup("mjpeg").expect("MJPEG is supported");
assert_eq!(mjpeg.oxi_name, "mjpeg");
```

## Wave 4 Additions (0.1.4)

### Codec-Map `OnceLock` Caching

The internal codec lookup table (80+ entries) is initialised once with
`std::sync::OnceLock` and reused for the lifetime of the process.  The first
call to `CodecMap::new()` builds and caches the `HashMap`; every subsequent call
returns a view into the same allocation — no locking, no allocation.

```rust
use oximedia_compat_ffmpeg::codec_map::CodecMap;

let map = CodecMap::new();  // First call: build + cache
let _   = CodecMap::new();  // Subsequent calls: zero-cost reuse
let entry = map.lookup("opus").expect("Opus is supported");
assert_eq!(entry.oxi_name, "opus");
```

### `-preset` / `-tune` / `-profile:v` — `EncoderQualityOptions`

The argument parser extracts these three standard encoder-quality flags into
`EncoderQualityOptions`.  All fields are `Option<_>`; a missing flag becomes `None`.

| FFmpeg flag | Rust type | Representative values |
|-------------|-----------|----------------------|
| `-preset` | `EncoderQualityPreset` | `ultrafast`, `fast`, `medium`, `slow`, `veryslow`, `placebo` |
| `-tune` | `EncoderTune` | `film`, `animation`, `grain`, `zerolatency`, `psnr`, `ssim` |
| `-profile:v` | `EncoderProfile` | `baseline`, `main`, `high`, `high10`, `high422`, `high444` |

All three types implement `std::str::FromStr` (case-insensitive).

```rust
use oximedia_compat_ffmpeg::encoder_options::{
    EncoderQualityOptions, EncoderQualityPreset, EncoderTune, EncoderProfile,
};

let preset: EncoderQualityPreset = "slow".parse().unwrap();
let tune:   EncoderTune          = "film".parse().unwrap();
let profile: EncoderProfile      = "high".parse().unwrap();

let opts = EncoderQualityOptions { preset: Some(preset), tune: Some(tune), profile: Some(profile) };
assert_eq!(opts.preset, Some(EncoderQualityPreset::Slow));
```

### `-vf` / `-af` Filter Shorthand — `parse_vf` / `parse_af`

`parse_vf(s)` and `parse_af(s)` parse the compact comma-separated syntax used with
`-vf` and `-af` into a single-chain `FilterGraph`.  Each filter entry is either
`name` (no options) or `name=key:val:…`.

```rust
use oximedia_compat_ffmpeg::filter_shorthand::{parse_vf, parse_af};

// Two-filter video chain
let vf = parse_vf("scale=1920:1080,format=yuv420p").unwrap();
assert_eq!(vf.chains.len(), 1);
assert_eq!(vf.chains[0].filters.len(), 2);

// Audio resample
let af = parse_af("aresample=48000").unwrap();
assert_eq!(af.chains[0].filters[0].name, "aresample");
```

### `-pass 1` / `-pass 2` Two-Pass Encoding — `PassPhase`

`parse_pass(&args)` extracts `-pass` (1 or 2) and the optional `-passlogfile` path,
returning a `PassPhase` enum.

| Variant | Description |
|---------|-------------|
| `First { stats_path }` | Analysis pass — encode and write statistics |
| `Second { stats_path }` | Quality pass — read statistics for rate control |

Default `stats_path` (when `-passlogfile` is absent): `ffmpeg2pass-0.log`.

```rust
use oximedia_compat_ffmpeg::pass::{parse_pass, PassPhase};

let args: Vec<String> = vec!["-pass".into(), "1".into()];
let phase = parse_pass(&args).unwrap().unwrap();
assert!(matches!(phase, PassPhase::First { .. }));

let args2: Vec<String> = vec!["-pass".into(), "2".into(), "-passlogfile".into(), "stats".into()];
let phase2 = parse_pass(&args2).unwrap().unwrap();
assert!(matches!(phase2, PassPhase::Second { .. }));
```

## License

Apache-2.0

Copyright COOLJAPAN OU (Team Kitasan)

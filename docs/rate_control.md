# Rate Control Guide — OxiMedia 0.1.5

Rate-control surface across `oximedia-transcode` and
`oximedia-compat-ffmpeg`: modes, parameters, how FFmpeg CLI flags map to
typed Rust values, and where playback-side ABR is wired.

Mode names follow
[`crates/oximedia-transcode/src/bitrate_control.rs`](../crates/oximedia-transcode/src/bitrate_control.rs).
Decoder-side status: [`docs/codec_status.md`](codec_status.md).

## Rate-control modes

`RateControlMode` in `bitrate_control.rs` defines four first-class modes:

| Variant | Behaviour | Use case |
|---|---|---|
| `ConstantBitrate` (CBR)    | Fixed output bitrate over time.         | Live, broadcast. |
| `VariableBitrate` (VBR)    | Average bitrate with peak/floor bounds. | VOD, archival. |
| `ConstantRateFactor` (CRF) | Quality-first; size floats.             | Mastering. |
| `ConstantQuality` (CQ)     | Perceptual-quality target.              | JPEG-style stills. |

`RateControlMode::is_quality_based()` returns `true` for CRF/CQ and
`false` for CBR/VBR. Two-pass VBR is `VariableBitrate` plus a stats file —
see Two-pass below.

## Encoder coverage matrix

Encoder-side support; decoder coverage is in `codec_status.md`.

| Encoder | CBR | VBR | CRF | Two-pass | Capped-CRF + VBV |
|---|---|---|---|---|---|
| AV1    | yes | yes | 0–63      | yes | yes |
| VP9    | yes | yes | 0–63      | yes | yes |
| VP8    | yes | yes | 4–63      | yes | —   |
| FFV1   | —   | —   | —         | —   | —   (lossless) |
| MJPEG  | —   | —   | Q-scale   | —   | —   |
| Opus   | yes | yes | —         | —   | —   (CELT only) |
| Vorbis | —   | —   | `-q` only | —   | —   |
| FLAC / PCM | — | — | —      | —   | —   (lossless) |

> **Honesty note (0.2.1, 2026-08-12).** This table describes which
> rate-control *knobs* each encoder exposes. It is **not** a statement that
> the resulting bitstream is decodable. Measured during the 0.2.1 audit: the
> **AV1, VP9, VP8 and Opus encoders do not emit valid bitstreams** — a
> reference decoder either rejects their output or decodes it to something
> different from the input, and the Opus encoder additionally writes a TOC
> byte that misdescribes its own payload. Consumers that need a trustworthy
> compressed encoder are expected to refuse rather than call these; several
> in this workspace now do (`oximedia-videoip`, `oximedia-server`'s
> transcode engine, `oximedia-cli`'s `switcher record`). FFV1, MJPEG and
> FLAC encode are real. See `codec_status.md` for decoder status and
> `CHANGELOG.md`'s `[0.2.1]` section for the measurements.

## Target bitrate spec

CBR/VBR targets live in `TargetBitrate`
(`bitrate_control.rs`):

```rust
use oximedia_transcode::bitrate_control::TargetBitrate;
// peak 8 Mbps, avg 5 Mbps; min derives to avg/2.
let t = TargetBitrate::with_peak(8_000, 5_000);
assert_eq!(t.min_kbps, 2_500);
```

CRF ranges are in `crf_optimizer.rs`: `CrfRange::av1_range()` (0–63),
`CrfRange::h264_range()` (17–51), `CrfRange::midpoint()`.

## VBV buffer semantics

VBV (video buffering verifier) parameters constrain decoder buffer
occupancy and enable capped-CRF delivery.

Per-rung ABR config (`crates/oximedia-transcode/src/abr_ladder.rs`):

- `AbrRungConfig::video_bitrate_bps` — analogue of `-b:v`.
- `AbrRungConfig::bufsize_bits` — analogue of `-bufsize`, set via
  `AbrRungConfig::with_bufsize(bufsize_bits: u64)`.
- `AbrRungConfig::crf` — analogue of `-crf`. Combined with a bufsize, gives
  a capped-CRF rung.

FFmpeg-compat per-stream options
(`crates/oximedia-compat-ffmpeg/src/arg_parser.rs::StreamOptions`):
`crf: Option<f64>` for `-crf`, `bitrate: Option<String>` for `-b:v`/`-b:a`,
`quality: Option<f64>` for `-q:v`/`-q:a`.

`-maxrate` / `-bufsize` are recognised by the top-level parser but reach
consumers today via the per-output `extra_args` fallback; they are queued
for promotion to first-class `StreamOptions` fields.

Encoder tuning knobs live on `EncoderQualityOptions`
(`crates/oximedia-compat-ffmpeg/src/encoder_options.rs`):
`preset` (`Ultrafast`..`Placebo`, 10 variants), `tune` (8), `profile` (6).

## Two-pass encoding

Pass 1 analyses per-frame complexity and writes a stats file; pass 2 reads
it and allocates bits proportionally to complexity, hitting a target
average bitrate.

The FFmpeg-compat layer recognises both phases
(`crates/oximedia-compat-ffmpeg/src/pass.rs`):

```rust
use oximedia_compat_ffmpeg::pass::{parse_pass, PassPhase};

let args = vec![
    "-pass".to_string(), "1".to_string(),
    "-passlogfile".to_string(), "encode".to_string(),
];
match parse_pass(&args).expect("parse") {
    Some(PassPhase::First { stats_path })  => { let _ = stats_path; }
    Some(PassPhase::Second { stats_path }) => { let _ = stats_path; }
    None => { /* single-pass */ }
}
```

If `-passlogfile` is omitted, the default is `ffmpeg2pass-0.log`.

The pass-1 analysis model is in
`crates/oximedia-transcode/src/two_pass.rs`: `TwoPassConfig::new`,
`TwoPassConfig::total_bits`, `PassOneResult::allocate_bits`.

## CRF optimiser

`crates/oximedia-transcode/src/crf_optimizer.rs` runs a bisection CRF
search. Given `QualityTarget::new(min_psnr_db, min_ssim, max_bitrate_kbps)`
and a `CrfRange`, it returns the most-compressed CRF that still meets the
quality floor.

## Per-title / per-scene encoding

`per_scene_encode.rs` + `abr_ladder.rs` realise per-title encoding: each
scene or ABR rung picks its own CRF/bitrate/bufsize from a
characterisation pass. `encode_ladder_validator.rs` checks monotonicity in
bitrate and resolution.

## Playback-side ABR (BBA-1)

Client-side, the BBA-1 strategy at
[`crates/oximedia-net/src/abr/bba1.rs`](../crates/oximedia-net/src/abr/bba1.rs)
selects the next variant from buffer level alone — no throughput estimate.
The buffer splits into three zones:

```text
┌──────────────┬──────────────────────────────────┬───────────────────────┐
│  Reservoir   │           Cushion                │   Above-cushion       │
│  [0, r]      │  (r, r+c]                        │   (r+c, ∞)            │
│  → lowest    │  → linear interp of variant idx  │  → highest            │
└──────────────┴──────────────────────────────────┴───────────────────────┘
0s             r             r+c             buffer_capacity
```

Defaults (`B = 30 s`, `r = 10 s`, `c = 20 s`) match Huang et al. (SIGCOMM
2014); `BbaParams::low_latency()` (`10 / 2 / 8`) is preferred for live.
Selection is a stateless pure function: `select_variant(buffer_level,
&params, &variants)`.

## See also

- [`docs/codec_status.md`](codec_status.md) — decoder / encoder taxonomy.
- [`docs/simd_dispatch.md`](simd_dispatch.md) — how encoder hot loops pick
  a SIMD backend at runtime.
- [`docs/wave5_deltas.md`](wave5_deltas.md) — what shipped in 0.1.5.

# Codec Status — OxiMedia 0.1.7

This document is the single source of truth for the honest status of every
codec decoder in the `oximedia-codec` and `oximedia-audio` crates. It was
produced by a static-analysis audit of the 0.1.5 source tree and is
referenced from the top-level `README.md`, from `crates/oximedia-codec/README.md`,
and from `TODO.md`.

The goal is that downstream users, packagers, and integrators can tell at a
glance whether a given codec can be used for playback, or only for
container/bitstream work, without having to read the source.

## Taxonomy

OxiMedia classifies each decoder with one of four honesty labels.

| Label | Meaning |
|-------|---------|
| **Verified** | End-to-end decode matches a reference implementation on external fixtures. |
| **Functional** | Real reconstruction path present and self-consistent on round-trip tests. No third-party conformance proof yet. |
| **Bitstream-parsing** | Headers / syntax are parsed; pixel or sample production is stubbed, partial, or returns empty/constant data. Useful for format inspection, not for playback. |
| **Experimental** | API sketch; not intended to decode. |

### Effort buckets (roadmap)

| Bucket | Approximate cost |
|--------|------------------|
| **small** | A focused bug-fix or a single missing stage; a few days of work by one engineer. |
| **medium** | Multiple decoder stages; several weeks of work, typically one engineer. |
| **large** | Complete reconstruction pipeline for a modern codec; months of work, possibly more than one engineer. |
| **specialist** | Requires a codec specialist, a reference generator, and conformance-suite validation. |

## Video decoders

### AV1 — Bitstream-parsing

- **Module:** `crates/oximedia-codec/src/av1/`
- **Current state:** OBU parsing, sequence/frame headers, loop-filter / CDEF /
  quantization parameters, symbol decoder, and an output-queue scaffold are
  all present. The decoder allocates a `VideoFrame` and populates metadata
  (width, height, format, pts) but the reconstruction stages inside
  `crates/oximedia-codec/src/reconstruct/pipeline.rs` (`stage_parse`,
  `stage_entropy`, `stage_predict`, `stage_transform`) are no-ops. The
  `TileGroup` branch of `decode_temporal_unit` explicitly returns without
  touching tile data (`"Tile group data would be processed here"`).
- **What is missing:** entropy decode of coefficients, intra / inter
  prediction, inverse transform, loop-filter / CDEF / film-grain application
  back into the output buffer, reference-frame management wired to the
  pipeline.
- **Effort:** specialist.
- **Target:** 0.2.0+. Tracked by GitHub issue #9.

### VP9 — Bitstream-parsing

- **Module:** `crates/oximedia-codec/src/vp9/`
- **Current state:** Frame parsing, tile infrastructure, and block/superblock
  decode methods exist. Decode routes through the same
  `DecoderPipeline::process_frame` shell as AV1, whose stages are no-ops.
  Output `VideoFrame` gets an allocated but unpopulated work buffer.
- **What is missing:** wiring the existing superblock/block/intra decode
  routines to actually write into the returned `VideoFrame`; filling in the
  pipeline stages; reference-frame management.
- **Effort:** large.
- **Target:** 0.2.0+.

### VP8 — Bitstream-parsing

- **Module:** `crates/oximedia-codec/src/vp8/`
- **Current state:** Bitstream parsing exists; the Y plane is filled with a
  constant `128` and no chroma decode is performed. Comment in
  `src/vp8/decoder.rs` states `"In a full implementation, we would decode the
  actual pixel data here"`.
- **What is missing:** intra/inter decode, DCT/WHT inverse transform, loop
  filter, actual Y/U/V output.
- **Effort:** large.
- **Target:** 0.2.0+.

### Theora — Functional

- **Module:** `crates/oximedia-codec/src/theora/`
- **Current state:** Real DCT/IDCT, quantization/dequantization, intra
  prediction, and direct sign-magnitude DCT coefficient encoding are all
  implemented and self-consistent. Three round-trip integration tests in
  `crates/oximedia-codec/tests/theora_roundtrip.rs` pass at quality 48, quality
  32, and for a flat-grey (DC-only) frame. Luma round-trip error is ≤ 8 LSB
  at Q48, ≤ 16 LSB at Q32; DC-only frames are pixel-near-exact (≤ 2 LSB).
- **What was fixed (Wave 4 Slice 1, 0.1.7):**
  1. `to_video_frame` wrote decoded pixels into a temporary `Vec` clone
     (`data.to_vec()`) which was immediately dropped — fixed by writing
     directly into `frame.planes[i].data`.
  2. `encode_dct_coefficients` passed raw signed coefficient values as
     Huffman symbol indices (negative i16 values wrap-to-large usize,
     out-of-range error) — replaced with a self-consistent 11-bit DC
     (sign + 10-bit magnitude) and 16-bit AC run-length (6-bit run + 10-bit
     sign-magnitude) direct encoding that matches the new decoder.
  3. The Theora `DC_HUFF_LENGTHS`/`AC_HUFF_TABLES` violate the Kraft
     inequality — the Huffman layer in the encoder/decoder was bypassed in
     favour of the self-consistent direct encoding; existing huffman.rs
     infrastructure is preserved for future table-correct re-implementation.
- **Effort to promote to Verified:** medium (conformance fixtures against
  libtheora reference decoder required; P-frame and inter-coding paths not
  yet exercised by round-trip tests).
- **Target:** 0.1.7 (promoted from Bitstream-parsing to Functional).

### AVIF — Bitstream-parsing

- **Module:** `crates/oximedia-codec/src/avif/`
- **Current state:** `decode()` returns the raw AV1 bitstream in
  `y_plane`. Comment: `"full decode of AV1 frames is out of scope for this
  implementation"`. Transitively depends on the AV1 decoder gap.
- **What is missing:** real AV1 pixel output + image-item demux.
- **Effort:** specialist (bounded by AV1).
- **Target:** follows AV1 (0.2.0+).

### WebP — Functional (VP8L lossless only)

- **Module:** `crates/oximedia-codec/src/webp/`
- **Current state:** VP8L (lossless) decoder is real and self-consistent.
  VP8 lossy WebP decoder is not present — only a lossy encoder module exists.
- **What is missing:** a lossy VP8 WebP decoder. Blocked on VP8 decoder.
- **Effort:** large (follows VP8).
- **Target:** 0.2.0+.

### MJPEG — Functional

- **Module:** `crates/oximedia-codec/src/mjpeg/`
- **Delegates to `oximedia-image::jpeg::JpegDecoder`; real RGB→YUV
  conversion. Round-trip tested, ≥28 dB PSNR at Q85.**
- **Effort to promote to Verified:** medium (conformance fixtures).

### FFV1 — Functional

- **Module:** `crates/oximedia-codec/src/ffv1/`
- **Current state (0.1.7 Wave 5+6):** Real range decoder, median prediction,
  CRC-32 verification. Supports full 8/10/12/16-bit depth matrix via 2-byte LE
  sample paths (`Yuv420p`, `Yuv420p10le`, `Yuv420p12le`, `Yuv420p16le`,
  `Yuv422p`, `Yuv422p10le`, `Yuv422p12le`, `Yuv422p16le`, `Yuv444p`,
  `Yuv444p10le`, `Yuv444p12le`, `Yuv444p16le`). Multi-slice decode uses
  `rayon::par_iter` with per-slice RFC 9043 §3.8.2.2.1-compliant context resets.
  Encoder is bit-depth-aware. Wave 5 `ffv1_higher_bit_depth.rs` verifies
  10/12-bit; Wave 6 extends to 16-bit.
- **Effort to promote to Verified:** medium (RFC 9043 conformance fixtures).

### JPEG-XL — Functional

- **Module:** `crates/oximedia-codec/src/jpegxl/`
- **Current state:** Real codestream extraction, modular decoder,
  `channels_to_interleaved`.
- **Effort to promote to Verified:** specialist (JPEG-XL conformance suite
  is non-trivial).

### H.263 — Functional

- **Module:** `crates/oximedia-codec/src/h263/`
- **Current state:** Real `decode_picture` with macroblock decode, motion
  compensation, loop filter.
- **Effort to promote to Verified:** medium.

### H.264 / AVC — Functional

- **Module:** `crates/oximedia-codec/src/h264/`
- **Current state:** Full Annex-B byte stream → `Frame` pipeline behind
  the `Decoder` driver in `pipeline.rs`. SPS / PPS / slice-header parsing,
  CAVLC and CABAC entropy decoding, intra prediction (4×4 / 16×16 /
  chroma 8×8), 4×4 integer inverse transform with Hadamard luma + chroma
  DC dequant, sub-pel motion compensation (6-tap luma, bilinear chroma),
  median MV prediction, P (all partition shapes including P_8x8 sub-mb)
  and B (16×16 / 16×8 / 8×16 / B_8x8 / B_Direct spatial) macroblock
  orchestrators, multi-reference and bi-predicted reconstruction, DPB
  with POC types 0/1/2 and RefPicList0 + RefPicList1 construction,
  in-loop deblocking for luma and chroma 4:2:0.
- **Known approximations:** B-slice CAVLC walks but emits placeholders
  (proper bin tree expansion is the headline remaining gap);
  `ref_pic_list_modification` ops are parsed but not yet applied;
  weighted prediction parsed but not yet applied;
  `directZeroPredictionFlag` for B_Direct (temporal MV cache) not
  tracked.
- **Out of scope (deliberate):** High profile (8×8 transform, custom
  scaling lists), 4:2:2 / 4:4:4 chroma, MBAFF / field coding, SP/SI
  slices.
- **Tests:** 279 unit tests plus a synthetic Annex-B → pixel-exact
  round-trip suite (`bitstream_roundtrip.rs`). The round-trip proves
  the encoder + decoder agree; it does not prove conformance against
  JVT-AVC reference vectors.
- **Patent status:** The MPEG-LA AVC patent pool wound down its
  licensing program in December 2024. The bulk of AVC-essential
  patents originated from late-1990s / early-2000s filings and have
  reached or are within a year of their 20-year terms. Individual
  patents in certain jurisdictions may still exist; commercial users
  should consult counsel. Open-source distribution in the United
  States is generally considered safe as of 2026.
- **Effort to promote to Verified:** specialist (JVT-AVC conformance
  suite integration + bit-exact agreement against a reference decoder
  on the full test-vector set).

### APV — Functional

- **Module:** `crates/oximedia-codec/src/apv/`
- **Current state:** Real DCT, dequantization, entropy decode.
- **Effort to promote to Verified:** medium.

### PNG / APNG — Functional

- **Modules:** `crates/oximedia-codec/src/png/`, `src/apng/`.
- **Current state:** Real sequential and interlaced decode, unfilter, RGBA
  conversion. APNG reuses the PNG pipeline per frame.
- **Effort to promote to Verified:** small (PngSuite).

### GIF — Functional

- **Module:** `crates/oximedia-codec/src/gif/`
- **Current state:** Real LZW decode loop plus color-table application.
- **Effort to promote to Verified:** small.

### JPEG 2000 — Functional (encoder + decoder, lossless 5-3 + lossy 9-7)

- **Module:** `crates/oximedia-codec/src/jpeg2000/` (feature-gated: `jpeg2000`)
- **Current state (0.1.7 Wave 4+5+6):** Full ISO/IEC 15444-1 decode pipeline for
  both lossless (5-3 reversible, LeGall lifting) and lossy (9-7 irreversible,
  CDF lifting) profiles. JP2 ISOBMFF box parser (`ftyp`/`jp2h`/`ihdr`/`colr`/
  `jp2c`), J2K marker parser (SOC/SIZ/COD/QCD/SOT/SOD/EOC), MQ arithmetic
  coder (47-state ISO 15444-1 Annex C), EBCOT Tier-1 (three coding passes:
  SPP/MRP/CUP), Tier-2 packet headers with TagTree. 9-7 path uses CDF 9/7
  lifting with `QcdMarker::step_size_for_subband()` epsilon/mu decomposition.
  Wave 6 adds full multi-tile support: `SizMarker::num_tiles_x/y()`,
  `tile_rect(idx)` and `collect_tile_map()` decode each tile independently then
  assemble into a full-frame buffer. Single-layer, single-resolution-level
  constraints remain; multi-layer/resolution deferred.
- **Current state (0.1.7 Wave 9):** Encoder added, completing the lossless 5-3
  encoder ↔ decoder pair. New `jpeg2000/mq_encoder.rs` (MQ arithmetic *encoder*
  per ISO 15444-1 Annex C — mirrors the existing 47-state decoder, shares the
  `MQ_TABLE` Qe/NMPS/NLPS/SWITCH constants, full carry propagation with 0xFF
  stuffing, `flush()`), `jpeg2000/tier1_encode.rs` (forward EBCOT — per-codeblock
  bit-plane scan with significance / magnitude-refinement / cleanup passes
  feeding the MQ encoder; context labels identical to the decode side),
  `jpeg2000/tier2_encode.rs` (`J2kBitWriter` + forward tag-tree coding +
  packet-header emission, fixed `lblock=3` block-length signalling),
  `jpeg2000/marker_write.rs` (SOC/SIZ/COD/QCD/SOT/SOD/EOC writers mirroring
  `markers.rs`), `jpeg2000/encoder.rs` (`Jpeg2000Encoder` +
  `Jpeg2000EncoderConfig { levels, tile_size, lossless }`; forward pipeline:
  per-component DC level-shift → forward 5-3 LeGall DWT (`forward_wavelet_2d`
  + `decompose_levels` added to `wavelet.rs`) → per-subband codeblock partition
  → Tier-1 encode → Tier-2 packets → markers; raw `.j2k` codestream). The
  slice also fixed a non-standard INITDEC/BYTEIN/DECODE/RENORMD path in the
  existing `mq_coder.rs` decoder that prevented decoding a 0 for the first
  decision of a fresh context, raised `MQ_TABLE` to `pub(crate)`, removed the
  `num_levels==0` rejection from `decoder.rs`, and taught `markers.rs` to
  honour `Psot>0` as a tile-part length delimiter — all necessary for the
  encoder ↔ decoder round-trip. Encode → decode is byte-exact on the lossless
  subset (single-layer LRCP, even dimensions; odd dimensions limited to 0–1
  decomposition levels by the existing decoder); multi-component encode is
  constrained by the decoder's single-tile-body assumption. Multi-layer /
  progression encode and JP2 box wrapping deferred to follow-ups.
- **Current state (0.1.7 Wave 10):** Lossy 9-7 *encoder* added, completing
  the lossy codec pair (decoder shipped Wave 5). Forward CDF 9/7 DWT (f64)
  promoted to public API: `wavelet.rs::forward_wavelet_1d_97` /
  `forward_wavelet_2d_97` / `decompose_levels_97` mirror the existing
  reversible 5-3 forward path but use the same α/β/γ/δ/K/K_INV CDF 9/7
  lifting constants as the inverse path. New `jpeg2000/quantize_fwd.rs`
  provides `quantize_subband_97(coeffs, step_size, num_bit_planes)` — exact
  inverse of `tier1.rs::dequantize`, mid-tread quantiser with sign-magnitude
  i32 output. `marker_write.rs` gains `write_qcd_lossy` (Sqcd style 2,
  expounded; per-subband 16-bit ε/μ pairs) and `write_cod_lossy` (kernel
  byte = 0 for 9-7) alongside the existing lossless variants. The encoder
  `Jpeg2000EncoderConfig.lossless: bool` flag now dispatches: `true` →
  existing 5-3 path; `false` → 9-7 path (`decompose_levels_97` →
  `quantize_subband_97` per subband → existing Tier-1 EBCOT encoder → existing
  Tier-2 packets → lossy COD + QCD writers). The lossy path emits `mct = 0`
  (no color transform) matching the lossless path; per ISO 15444-1 §E.1 a
  single global ε = 8, μ = 0 produces uniform `Δ_b = 2^(R_b − 8)` step
  sizes. Encode → decode is within ±2 LSB at 1 decomposition level on flat
  16×16 frames and PSNR ≥ 35 dB on 32×32 gradients at 3 levels. Multi-
  component lossy (ICT), multi-layer / progression, and JP2 box wrapping
  remain deferred follow-ups.
- **Effort to promote to Verified:** medium (OpenJPEG conformance fixture suite;
  multi-layer and progressive-quality extensions needed for full compliance).

### JPEG XS — Functional (encoder + decoder)

- **Module:** `crates/oximedia-codec/src/jpegxs/` (feature-gated: `jpegxs`)
- **Current state (0.1.7 Wave 5+7+8):** Full encoder ↔ decoder pair for ISO/IEC
  21122-1:2019 (SMPTE ST 2110-22). The decoder parses all header markers
  (SOC/PIH/CDT/WGT/NLT/CWD/SLH/EOC), uses an MSB-first VLC bitreader, a
  LeGall 5/3 inverse wavelet (self-contained, independent of the jpeg2000
  module), and VLC entropy tables. Wave 7 added full NLT quadratic reverse
  transform (ISO 21122-1 §A.2.2): integer-only ceiling-sqrt inverse with
  three-region dispatch (low=identity, mid=T1+⌈√((s'−T1)·(T2−T1))⌉,
  high=MaxVal−⌈√((MaxVal−s')·(MaxVal+1−T2))⌉). `JxsHeaders::nlt_payload` captures
  the raw 5-byte NLT marker payload; the decoder parses and applies NLT after
  wavelet reconstruction. **Wave 8 adds the encoder:** new `bitwriter.rs`
  (MSB-first, no byte-stuffing), `marker_write.rs` (SOC/PIH/CDT/WGT/CWD/SLH/EOC
  writers mirroring `markers.rs`), `vlc_encode.rs` (forward VLC, exact inverse
  of the decoder's entropy stage), `encoder.rs` (`JpegXsEncoder`,
  `JpegXsEncoderConfig`; forward 5/3 DWT added to `wavelet.rs`; per-band
  quantize → VLC encode → slice assembly → marker emission). Encode→decode
  round-trip is byte-exact with unit weights (lossless 5/3) and within ±2 LSB
  for quantized streams. `NltType::Extended` remains deferred.
- **Effort to promote to Verified:** medium (requires FFmpeg or reference JPEG XS
  bitstream corpus; NLT Extended transform needed for full ISO compliance).

### JPEG-LS — Functional (encoder + decoder, regular + RUN modes)

- **Module:** `crates/oximedia-codec/src/jpegls/` (feature-gated: `jpegls`)
- **Current state (0.1.7 Wave 6+7+8):** Full ISO 14495-1 encode/decode pipeline
  (LOCO-I algorithm). The decoder implements SOI/SOF55/LSE/SOS marker parsing,
  the LOCO-I edge-detecting predictor (Clarkson–Orchard–Barford), 365-context
  gradient quantisation with sign normalisation, adaptive Golomb-Rice entropy
  decode (LIMIT/qbpp overflow encoding per ISO 14495-1 §A.3), and bias-correction
  / adaptive k-update (§6.4). Supports 8–16-bit greyscale and multi-component.
  Wave 7 added near-lossless mode (NEAR > 0): error quantisation step
  `q = 2·NEAR+1`, error mapped via `unmap_error_near` (§A.4), reconstructed as
  `corrected_px + err_q·q_step·sign`; plus interleaved multi-component (ILV=0
  non-interleaved, ILV=1 line-interleaved, ILV=2 sample-interleaved), each
  component using its own independent 365-entry context array. **Wave 8 adds the
  encoder:** new `golomb_write.rs` (MSB-first `BitWriter` with JPEG byte-stuffing
  + `encode_golomb_unsigned_limited` exact inverse of the decoder side),
  `marker_write.rs` (SOI/SOF55/LSE/SOS/EOI writers mirroring `markers.rs`), and
  `encoder.rs` (`JpegLsEncoder`, `JpegLsEncoderConfig { near, interleave,
  components, bit_depth, width, height }`; full forward LOCO-I — predict via
  shared `predict()`, quantize via shared `quantize_gradient`, share context
  state with the decoder, map errors, Golomb-encode; ILV 0/1/2 dispatch).
  Encode→decode round-trip is byte-exact for lossless (NEAR=0) and within
  ±NEAR for near-lossless. Container aliases: jls/dicom/dcm. HP patents
  expired 2017–2019.
- **Current state (0.1.7 Wave 10):** ISO 14495-1 §A.7 **RUN mode** added on
  both sides, promoting the codec from "regular only" (§A.6) to "regular +
  RUN" (§A.6 + §A.7). Flat regions now compress exponentially better via
  §A.7 length tokens instead of long unary residual chains. New
  `jpegls/run_mode.rs` (~335 LoC) holds Table A.5 `J[0..=30]`, the
  `RUN_THRESHOLD[r] = 1 << J[r]` lookup, the `RunState { run_index,
  run_value }` accumulator, `enter_run_lossless` / `enter_run_near` raw-
  gradient entry tests, `bump_run_index` (capped at 30), and
  `run_termination_ctx(ra, rb)` returning context 365 (Ra==Rb) or 366 (Ra!=
  Rb). `context.rs` extends the per-component `ContextState` array to 367
  entries (365 regular + 2 RUN termination). `decoder.rs` and `encoder.rs`
  each dispatch RUN mode at the top of their per-pixel inner loop: when the
  three raw gradients all stay within NEAR, count consecutive matching
  samples, Golomb-encode the run length with `k = J[run_index]`, increment
  `run_index` per full token, terminate with the residual length plus the
  breaking sample under the 365/366 context. Per-line `run_index` reset
  matches the spec. ILV=0 (non-interleaved) and ILV=1 (line-interleaved)
  exercise RUN mode; ILV=2 (sample-interleaved) intentionally suspends RUN
  per the CharLS reference convention. The pre-existing flat-region tests
  `round_trip_constant_grey_8x8` and `roundtrip_lossless_16x16_constant`
  remain byte-exact (decoded pixels equal input — only the encoded byte
  stream becomes shorter). 8 new integration tests in
  `tests/jpegls_runmode_roundtrip.rs` (constant 32×32, stripes, two-color
  columns, near-lossless flat 24×24, ILV=1 RGB constant, zero-length runs
  at line start, long-run 64×64, gradient-then-flat) all pass.
- **Effort to promote to Verified:** small (HP/charLS conformance fixtures
  against reference encoder).

### ProRes 422 — Functional

- **Module:** `crates/oximedia-codec/src/prores/` (feature-gated: `prores`)
- **Current state (0.1.7 Wave 3+7):** ProRes 422 family encoder (Wave 3) and decoder
  (Wave 7). Encoder supports Proxy/LT/Standard/HQ profiles via `ProResEncoderConfig`.
  Decoder (`ProResDecoder`) parses the `icpf` atom, reads the frame header and
  picture/slice structure, iterates all slices, delegates each to the per-slice
  IDCT+dequant pipeline, and assembles the full-frame YUV 4:2:2 planar output.
  10-bit native samples are downscaled to 8-bit output (right-shift 2) for the
  public `ProResFrame`. Also implements the `VideoDecoder` trait for push-pull
  use. Round-trip tests in `tests/prores_roundtrip.rs` verify constant-grey
  frames decode within ±4 LSB of expected values across all four 422 profiles
  (Proxy/LT/Standard/HQ).
  Apple ProRes is patent-encumbered but the codec is widely licensed for use in
  production tools; this implementation is for educational/format-compatibility use.
- **Effort to promote to Verified:** medium (reference bitstream comparison against
  Apple or FFmpeg ProRes output for pixel-level conformance; interlaced field
  deinterleaving and 4444/4444-XQ profile decode also needed).

### VC-3 / DNxHD — Functional

- **Module:** `crates/oximedia-codec/src/dnxhd/` (feature-gated: `dnxhd`)
- **Current state (0.1.7 Wave 4):** Decode SMPTE ST 2019-1 VC-3/DNxHD to YUV
  4:2:2 planar (8-bit and 10-bit). Profiles: DNxHD 145 / 220 / 220x / 145x /
  100 / 60 (CIDs 1235–1243). DC Huffman + MPEG-2 AC VLC tables, Q15 8×8 IDCT,
  DC DPCM, progressive zigzag. Encoder deferred to v0.1.8+ (Avid licence).
- **Effort to promote to Verified:** medium (conformance fixtures from Avid or
  FFmpeg-generated streams).

### MPEG-2 — Functional (I-frame encoder + decoder, 4:2:0 + 4:2:2 + 4:4:4)

- **Module:** `crates/oximedia-codec/src/mpeg2/` (feature-gated: `mpeg2`,
  opt-in — **not** in default features).
- **Current state (0.1.7 Wave 8):** I-frame decoder for ISO/IEC 13818-2 /
  ITU-T H.262. MPEG-2 video patents expired February 2023, so it is now
  fully patent-free and admissible to the workspace Green-List. New module
  (~3,400 LoC across 9 files, self-contained — does **not** depend on the
  `dnxhd` feature): `bitreader.rs` (MSB-first + start-code scanner),
  `headers.rs` (sequence header, sequence extension, GOP, picture header,
  picture coding extension, slice header — chroma_format, intra_dc_precision,
  picture_structure, q_scale_type, alternate_scan, intra_vlc_format),
  `vlc_tables.rs` (Tables B-12 / B-13 / B-14 / B-15 written directly from the
  standard rather than reusing DNxHD's reordered table),
  `idct.rs` (IEEE-1180-tolerant Q15 8×8 inverse DCT),
  `zigzag.rs` (progressive Figure 7-2 + alternate Figure 7-3 scans),
  `dequant.rs` (intra inverse-quant per §7.4: intra DC via `intra_dc_mult`,
  intra AC via `(2·QF)·W·qscale/32` with saturation + sum-oddification mismatch
  control on F[63] per §7.4.4),
  `entropy.rs` (intra macroblock decode: per-component DC DPCM predictor reset
  to `2^(7+intra_dc_precision)` at slice start, AC run/level VLC with escape
  6-bit run + 12-bit signed level, Table B-1 macroblock_address_increment,
  Table B-2 I-picture macroblock_type),
  `decode.rs` (full pipeline → YUV 4:2:0 planar). P/B frames (motion
  compensation) and field pictures are rejected with `Err` and documented as
  follow-ups. `CodecId::Mpeg2` (aliases mpeg2/mpeg-2/m2v/h262) added to
  `oximedia-core`; codec_matrix arms for ts/ps/mpeg/mp4/mkv containers.
- **Current state (0.1.7 Wave 9):** I-frame encoder added, completing the pair.
  New `mpeg2/bitwriter.rs` (MSB-first writer + start-code emit — MPEG-2 video
  elementary streams do **not** use byte-stuffing), `mpeg2/fdct.rs` (forward
  8×8 DCT matched to the Q15 IDCT — IEEE-1180-tolerant FDCT↔IDCT recovers DC
  exactly), `mpeg2/quantize_fwd.rs` (forward §7.4 intra quant: `QF[0] =
  round(F[0] / intra_dc_mult)`, `QF[u,v] = round(16·F[u,v] / (W[u,v]·q_scale))`,
  clamped to ±2047, default intra matrix), `mpeg2/vlc_encode.rs` (forward VLC:
  DC size+diff via B-12 / B-13, AC run/level via B-14 / alternate B-15 — the
  rare codeword pairs that collide on inverse-lookup are routed through the
  6-bit-run / 12-bit-signed-level escape so the existing Wave 8 decoder accepts
  every encoded entry; verified by a `match_vlc` round-trip unit test over the
  full tables), `mpeg2/marker_write.rs` (sequence_header with default
  quantiser matrices, sequence_extension at chroma_format = 4:2:0,
  picture_header, picture_coding_extension with intra_dc_precision +
  q_scale_type + progressive_frame + f_codes = 0xF, slice header),
  `mpeg2/encoder.rs` (`Mpeg2Encoder` + `Mpeg2EncoderConfig { width, height,
  q_scale, intra_dc_precision, frame_rate, aspect_ratio, chroma_format =
  4:2:0 }`; full forward pipeline per macroblock: split planes → 4 luma 8×8 +
  2 chroma 8×8 → FDCT → forward quant → progressive zigzag → DC DPCM + AC
  run/level → VLC encode → slice / marker emission; DC predictor resets to
  `2^(7 + intra_dc_precision)` at every slice; implements the `VideoEncoder`
  trait). `Mpeg2Error::{InvalidConfig, Encode}` added to the module error
  enum. Encode → decode round-trip is verified against the Wave 8 decoder
  (flat / DC / gradient frames within bounded LSB tolerance — DCT + quant are
  lossy, but the bitstream parses cleanly through `Mpeg2Decoder`). 9
  integration tests pass under the `mpeg2` feature. P/B frames + field
  pictures (both decoder and encoder) still deferred to v0.1.8+.
- **Current state (0.1.7 Wave 10):** Adds 4:2:2 (chroma_format = 2) and 4:4:4
  (chroma_format = 3) chroma formats on BOTH decode and encode sides per ISO
  13818-2 §6.1.1.4 Table 6-10. `headers.rs` lifts the chroma_format != 1
  rejection guard to accept `1..=3`. `decode.rs` dispatches the per-MB
  block-list on chroma_format (6 / 8 / 12 blocks: 4 luma + 1/2/4 Cb +
  1/2/4 Cr), computes per-component block origins for 4:2:2 (chroma 8×16,
  blocks stacked vertically — Cb_top, Cb_bot, Cr_top, Cr_bot) and 4:4:4
  (chroma 16×16, 2×2 tiles in raster order), and parameterises
  `output_format()` + `VideoFrame::new` on the stored chroma_format
  (`Yuv420p` / `Yuv422p` / `Yuv444p`). `encoder.rs` gains
  `Mpeg2EncoderConfig.chroma_format: u8` (default 1) with `yuv420p()`/
  `yuv422p()`/`yuv444p()` factory shortcuts; the input `frame.format` accept
  is relaxed to all three YUV planar formats; `encode_macroblock` mirrors
  the decoder's block-list and origin dispatch. `marker_write.rs` adds
  `CHROMA_FORMAT_422 = 2` / `CHROMA_FORMAT_444 = 3` constants and
  `write_sequence_extension()` now writes the configured chroma_format. All
  Wave 8 / Wave 9 4:2:0 tests continue to pass; new tests round-trip flat
  and gradient frames in Yuv422p and Yuv444p (within ±2 LSB at high
  q_scale, PSNR ≥ 40 dB at low q_scale) and verify `Mpeg2Decoder` accepts
  the new 4:2:2 / 4:4:4 sequence headers. P/B frames + field pictures
  remain deferred.
- **Effort to promote to Verified:** medium (real MPEG-2 elementary-stream
  conformance corpus; field pictures and P/B inter frames needed for full
  ISO compliance).

## Audio decoders

### Vorbis — Bitstream-parsing

- **Module:** `crates/oximedia-codec/src/vorbis/`
- **Current state:** Headers parse. `decode_audio_packet` returns
  `Ok(Vec::new())` with a comment noting that a full decode would need the
  stateful MDCT/OLA context. Floor reconstruction is a "simplified linear
  interpolation" placeholder. A real MDCT exists but is only wired to a
  custom test format.
- **What is missing:** full Vorbis reverse codebook decode, residue, floor
  curve, MDCT/IMDCT, overlap-add, window switching, channel coupling.
- **Effort:** specialist.
- **Target:** 0.2.0+.

### Opus — Functional (CELT only)

- **Module:** `crates/oximedia-codec/src/opus/`
- **Current state:** CELT decoder (MDCT, pitch, bands) is real. SILK and
  hybrid modes are explicit stubs (`src/silk.rs:873` emits a comfort-noise
  indicator byte). CELT-only streams produce real PCM.
- **What is missing:** real SILK LP analysis/synthesis (LTP, LSF, LPC),
  hybrid-mode band splitting.
- **Effort:** specialist.
- **Target:** 0.1.6 / 0.2.0+.

### FLAC — Functional / Verified

- **Module:** `crates/oximedia-codec/src/flac/`
- **Current state:** Real `decode_frame` with subframe / LPC decode, CRC-16
  verification, round-trip tests. Treated as Functional for public-codec
  parity and "Verified" on internal round-trip fixtures.
- **Effort to promote to externally-Verified:** small (reference encoder
  fixture suite).

### ALAC — Functional (encoder + decoder, patent-free Apple Lossless)

- **Module:** `crates/oximedia-codec/src/alac/` (feature-gated: `alac`, opt-in
  — **not** in default features).
- **Current state (0.1.7 Wave 9):** Full encoder ↔ decoder pair for Apple
  Lossless (ALAC). Apple released the reference under the Apache License 2.0
  in October 2011, so the codec is royalty-free and admissible to the
  workspace Green-List. New greenfield module (~2,200 LoC across 8 files):
  `mod.rs` (`AlacError` / `AlacResult` + re-exports), `config.rs`
  (`AlacSpecificConfig` — the 24-byte big-endian "magic cookie":
  `frameLength`, `compatibleVersion`, `bitDepth`, `pb`/`mb`/`kb` Rice tuning,
  `numChannels`, `maxRun`, `maxFrameBytes`, `avgBitRate`, `sampleRate`),
  `bitstream.rs` (MSB-first `BitReader` + `BitWriter`; ALAC packs MSB-first
  with no stuffing), `rice.rs` (adaptive modified-Rice / Golomb encode +
  decode with `k`-history update and escape-to-fixed-bits path for outliers),
  `lpc.rs` (adaptive FIR predictor — sign-LMS coefficient adaptation with
  `lpc_quant` step, predictor mode 0; rare extended modes rejected with
  `AlacError::Unsupported`), `mix.rs` (inter-channel decorrelation via
  `interlacing_shift` + `interlacing_leftweight`, exact integer mid/side ↔
  left/right inverse), `decoder.rs` (`AlacDecoder`: per-frame element decode
  with compressed / uncompressed-escape / constant paths, 16/20/24-bit
  interleaved i32 PCM output, mono + 2-channel decorrelation), `encoder.rs`
  (`AlacEncoder` + `AlacEncoderConfig`; forward path mirroring the decoder —
  chooses predictor + Rice params, emits frame elements, picks uncompressed
  when smaller). `CodecId::Alac` (lossless audio; aliases `alac` /
  `m4a-alac`) added to `oximedia-core`; `codec_matrix` arms for
  mp4 / m4a / mov / caf / mkv containers. Encode → decode round-trip is
  byte-exact for 16-bit / 20-bit / 24-bit mono and stereo (with and without
  decorrelation); 11 integration tests pass. The encoder uses a fixed-`k`
  remainder path (Apple's variable-remainder path is decoder-side optional);
  32-bit and the rare extended predictor modes are explicit `Unsupported`
  and tracked as follow-ups.
- **Effort to promote to Verified:** medium (Apple reference / `caf` corpus
  comparison; extended predictor modes and 32-bit needed for full parity).

### PCM — Verified

- **Module:** `crates/oximedia-codec/src/pcm/` (formats) +
  `crates/oximedia-audio/src/pcm/` (audio-layer glue).
- **Current state:** Trivial round-trip, passes trivially.

### MP3 — Functional (oximedia-audio)

- **Module:** `crates/oximedia-audio/src/mp3/` (note: lives in
  `oximedia-audio`, not `oximedia-codec`).
- **Current state:** Full Huffman / IMDCT / synthesis filterbank / stereo
  processing / VBR / ID3. Decoder-only; MP3 encoding is still on the
  red-list (Fraunhofer). MP3 decoding patents expired in 2017.
- **Effort to promote to Verified:** medium.

## Historical note

Prior to 0.1.5, the README advertised AV1 / VP9 / VP8 / Theora / Vorbis /
AVIF decoders as "Stable" / "Complete". That was inaccurate: the bitstreams
parsed, but the reconstruction path was stubbed. 0.1.5 introduces this
four-tier taxonomy, demotes the affected codecs to `Bitstream-parsing`,
keeps the accurately-working codecs at `Functional`, and opens a tracked
roadmap for closing the gap. No source behaviour changed in this pass;
only documentation, the `decode_video` example, and an `#[ignore]`'d
integration test.

The AV1 gap specifically is tracked by GitHub issue #9; the Theora copy bug
is tracked separately as a small bug-fix ticket.

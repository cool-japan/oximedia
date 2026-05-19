# Codec Status — OxiMedia 0.1.5

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

### Theora — Bitstream-parsing (with a known copy bug)

- **Module:** `crates/oximedia-codec/src/theora/`
- **Current state:** Real DCT, IDCT, and motion compensation are implemented.
  The returned `VideoFrame` never receives the decoded pixels because
  `src/theora/mod.rs:162-183` copies into a local `Vec` produced by
  `frame.planes[0].data.to_vec()` — the local `Vec` is mutated and dropped,
  leaving `frame.planes[0].data` untouched.
- **What is missing:** replace the `to_vec()` mis-copy with a direct write
  into `frame.planes[<i>].data`. Small bug-fix.
- **Effort:** small.
- **Target:** 0.1.5 point release or 0.1.6.

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
- **Current state:** Real range decoder, median prediction, CRC-32
  verification, real pixel output via planes.
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

# Codec Status — OxiMedia 0.2.1

This document is the single source of truth for the honest status of every
codec decoder in the `oximedia-codec` and `oximedia-audio` crates, and (as of
0.1.9) for the cryptographic honesty of the networking (`oximedia-net`) and
DRM (`oximedia-drm`) crates. It was originally produced by a static-analysis
audit of the 0.1.5 source tree (0.1.7 stamp), was re-audited directly against
the 0.1.9 source tree on 2026-07-08 (see "0.1.9 re-audit summary" below), was
**re-audited a third time, for AV1/VP9/VP8 specifically, against the 0.2.0
source tree on 2026-07-15** — every codec entry below was re-verified by
reading the current module rather than trusting the prior text (see "0.2.0
re-audit summary" below for what changed) — and has now been **re-audited a
fourth time, for VP8/VP9/FLAC/AVIF/Opus, against the 0.2.1 source tree on
2026-08-12** (see "0.2.1 re-audit summary" below). Sections carrying an
earlier date describe the tree as it stood on that date and are superseded,
not rewritten, by the later summaries and by the per-codec entries. It is
referenced from the top-level `README.md`, from
`crates/oximedia-codec/README.md`, from `SECURITY.md` (crypto status table),
and from `TODO.md`.

The goal is that downstream users, packagers, and integrators can tell at a
glance whether a given codec can be used for playback, or only for
container/bitstream work, and whether a given transport or DRM path provides
real cryptographic confidentiality, without having to read the source.

## 0.1.9 re-audit summary

- **AV1 / VP9 / VP8 pixel-reconstruction decode — re-confirmed unchanged.**
  `crates/oximedia-codec/src/reconstruct/pipeline.rs`'s `stage_parse` /
  `stage_entropy` / `stage_predict` / `stage_transform` are still literal
  no-ops (each pushes a `StageResult` and returns `Ok(())` without touching
  pixel data); `av1/decoder.rs` still contains the dead
  `"Tile group data would be processed here"` branch; `vp9/decoder.rs`
  defines a real `decode_superblock` method but it is never called from the
  `decode_frame` path actually used for output (which instead routes through
  the same no-op `DecoderPipeline::process_frame`, then zero-fills the output
  via `buffer_pool.acquire()`); `vp8/decoder.rs` still fills Y/U/V planes
  with the constant `128` next to the comment `"In a full implementation, we
  would decode the actual pixel data here."`. `git log -- crates/oximedia-codec/src`
  shows no commits between the 0.1.8 release and this audit other than two
  small "CUDA"-labelled commits (an AV1 parallel-tile SIMD tweak, and the
  SILK addition described next) — so this three-codec gap is confirmed still
  open and unchanged, per `TODO.md` and issue #9.
- **Opus / SILK — status corrected upward; the 0.1.7 text was stale.** The
  previous entry read "SILK and hybrid modes are explicit stubs
  (`src/silk.rs:873` emits a comfort-noise indicator byte)". That line number
  no longer corresponds to any stub in the tree. Since the 0.1.7 audit, a
  normative SILK decoder was added
  (`crates/oximedia-codec/src/opus/silk_decoder.rs`, RFC 6716 §4.2: LSF
  stage-1/2 decode, NLSF stabilisation + NLSF→LPC conversion, LCG-seeded
  shell-pulse excitation decode, LTP + LPC synthesis) and is genuinely wired
  into the production path: `opus/silk.rs::SilkDecoder::decode_into` calls
  `decode_silk_frame` directly, and `OpusDecoder::decode_mode` dispatches
  `OpusMode::Silk` packets to it. Hybrid mode (`opus/hybrid.rs`) is likewise
  real — it decodes SILK (low band) and CELT (high band) from the *same*
  range-coded bitstream per RFC 6716 §4.5, replacing an earlier "split the
  packet at its midpoint" heuristic that does not exist in the spec. This was
  only caught by reading `opus/silk_decoder.rs` and `opus/silk.rs` directly;
  the document being audited had never been updated to reflect it. See the
  revised Opus entry below.
- **All other codec entries** (Theora, WebP, MJPEG, FFV1, JPEG-XL, H.263,
  APV, PNG/APNG, GIF, JPEG 2000, JPEG XS, JPEG-LS, ProRes, VC-3/DNxHD,
  MPEG-2, Vorbis, FLAC, ALAC, PCM, MP3) were re-checked against their current
  module directories and source comments; their status labels below are
  unchanged from 0.1.7 (source in these areas has not moved since the
  respective wave that produced each entry).
- **New:** a "Network & DRM crypto status" section has been added, extending
  this document's honesty taxonomy to WebRTC/SRT transport security and
  FairPlay/Widevine/PlayReady/Clear Key DRM.
- **New:** a "Browser (`oximedia-wasm` npm) surface note" has been added
  below, documenting that the WASM/npm decoder surface (as opposed to the
  native crates this document otherwise describes) shrank in 0.1.9.

### Browser (`oximedia-wasm` npm) surface note — 0.1.9

The status table below describes the **native** `oximedia-codec` /
`oximedia-audio` crates only. As of 0.1.9, the separate `oximedia-wasm`
crate (published to npm as `@cooljapan/oximedia`; see
[`oximedia-wasm/README.md`](../oximedia-wasm/README.md)) exports a
**narrower** decoder surface than the native crates support:

- The standalone `WasmVp8Decoder`, `WasmAv1Decoder`, and `WasmVorbisDecoder`
  classes were **removed** from the WASM bindings — each wrapped a decoder
  that returned an error, unpopulated buffers, or output that only
  round-tripped a synthetic test format, respectively (never real pixel/PCM
  output), so shipping them was strictly worse than not shipping them.
- AV1 decode remains reachable in the browser only via `WasmMediaPlayer`
  (no standalone `WasmAv1Decoder` class). VP8 and Vorbis decode are not
  exposed anywhere in `oximedia-wasm` today.
- This is a **WASM-surface-only** change. The native VP8/AV1/Vorbis status
  in this document (Bitstream-parsing for AV1/VP8/VP9, Bitstream-parsing for
  Vorbis — see below) is unchanged and unaffected; `oximedia-wasm` simply no
  longer advertises decode capability that the native crates never actually
  had either (VP8/AV1 pixel reconstruction and Vorbis sample production are
  still stubbed — see their entries below).
- The separate, independently-versioned `@cooljapan/oximedia-web` package
  (`web/`, unpublished as of 0.1.9 — see [`web/README.md`](../web/README.md))
  does not touch codec decode at all; it consumes already-decoded
  `VideoFrame`s from the browser's own `VideoDecoder`.

**Note (0.2.0):** the native AV1/VP8/VP9 status referenced in the paragraph
above (all "Bitstream-parsing") is now stale — see "0.2.0 re-audit summary"
immediately below and the revised entries under "Video decoders". This WASM
surface note itself describes the `oximedia-wasm` npm package as of 0.1.9
and has not been independently re-checked against 0.2.0 `oximedia-wasm`
source in this pass.

## 0.2.0 re-audit summary

- **AV1 pixel-reconstruction decode — promoted from Bitstream-parsing to
  Functional (keyframe/intra only).** The 0.1.9 summary above ("AV1 / VP9 /
  VP8 pixel-reconstruction decode — re-confirmed unchanged") no longer
  applies to AV1. A new `crates/oximedia-codec/src/av1/kf/` module — an
  exact port of the intra decode path of the AV1 specification — is now
  called directly from `av1/decoder.rs::decode_temporal_unit`; the old
  no-op `"Tile group data would be processed here"` branch is confirmed
  gone from the source. Verified bit-exact (0 differing Y/U/V pixels)
  against both `dav1d` 1.5.1 and `aomdec`/libaom v3.12.1 on 13 keyframe
  test vectors (`av1/kf/mod.rs`, tests `stage1_lossless_gray64_bit_exact`
  through `stage4_switchable_wiener_320x192_bit_exact`), covering lossless
  coding, 128×128 superblocks, 2 tile columns, an odd 76×42 crop, and
  320×192 loop-restoration cases, with the full post-filter chain
  (deblocking, CDEF, and loop restoration — Wiener and self-guided/SGRPROJ)
  applied. See the revised AV1 entry below for the current scope and the
  specific file:line markers for what is still missing.
- **VP9 / VP8 pixel-reconstruction decode — also promoted, in an earlier
  0.2.0-session pass that predates this particular audit.**
  `crates/oximedia-codec/src/vp9/kf/` and
  `crates/oximedia-codec/src/vp8/keyframe/` shipped real keyframe/intra
  decode, verified bit-exact against libvpx/ffmpeg and libwebp reference
  decodes respectively (see `CHANGELOG.md`'s `[0.2.0]` Added section).
  This document was not updated when that code shipped — flagged as
  cross-file drift in `TODO.md` — and is corrected below in the same pass
  as the AV1 update above.
  **Note (0.2.1):** both module paths named in this bullet have since been
  renamed — `vp9/kf/` → `vp9/dec/` and `vp8/keyframe/` → `vp8/dec/` — when
  inter-frame decode landed in each. The 0.2.0-era sentence is kept as the
  historical record; the current paths are in the per-codec entries below.
- **The orphaned `av1/avif.rs`** (never declared as a module in
  `av1/mod.rs`, and a duplicate of the live `avif/mod.rs` AVIF
  implementation) **was deleted.** `avif/mod.rs::decode()` itself has
  **not** been wired to the new AV1 keyframe decoder and still returns an
  honest `Err` — see the AVIF entry below, unchanged by this audit.
  **Note (0.2.1):** the second sentence no longer holds. `decode()` was
  wired to the AV1 keyframe decoder in 0.2.1 and now produces real pixels
  for 8-bit 4:2:0 colour items; see the revised AVIF entry below for the
  two cases that still refuse honestly.
- **All other codec entries below are unchanged from the 0.1.9 re-audit
  above** and were not re-verified in this pass.

## 0.2.1 re-audit summary (VP8, VP9, FLAC, AVIF, Opus)

- **VP8 inter-frame decode — promoted from Functional (keyframe/intra only)
  to Verified.** The "What is missing (an honest
  `CodecError::UnsupportedFeature`)" line of the previous VP8 entry no
  longer applies: that error path is gone from `vp8/decoder.rs`, and inter
  frames are decoded by `vp8/dec/inter.rs`'s `Vp8SequenceDecoder` (motion
  vectors, per-macroblock prediction records, sub-pixel motion
  compensation, and last/golden/altref management). Evidence is external
  and multi-frame: five libvpx-encoded streams — 39 coded frames, of which
  38 are shown (one is a hidden altref) — decode bit-exactly against libvpx's own
  reconstruction, including a stream with a hidden (`show_frame == 0`)
  altref frame and one with a non-macroblock-aligned width. The key-frame
  path was re-verified unchanged (still bit-exact vs libwebp on its three
  still-image vectors) after being rerouted through the same sequence
  decoder. See the VP8 entry below.
- **VP9 inter-frame and intra-only decode — promoted from Functional
  (keyframe/intra only) to Verified.** Both honest-`Err` dispatch arms
  named by the previous entry (`vp9/decoder.rs:91` intra-only,
  `vp9/decoder.rs:102` inter) are gone from the source. The keyframe
  module was renamed `vp9/kf/` → `vp9/dec/` and grew the full inter path
  (cross-frame reference/probability state, MV reference scan, inter mode
  and motion-vector entropy decode, eight-tap motion compensation,
  compound prediction, backward probability adaptation). Evidence is
  external and multi-frame: **10 of 10** non-deferred libvpx-encoded
  streams decode bit-exactly through the public
  `VideoDecoder::send_packet`/`receive_frame` API, with no `#[ignore]`, no
  PSNR threshold and no skip. Two features remain deferred and refuse at a
  pinned packet index rather than silently misdecoding: inter-frame
  segmentation and reference scaling. See the VP9 entry below.
- **FLAC in `oximedia-codec` — promoted to Verified, after the previous
  label proved to be an overclaim.** The prior entry read "Functional /
  Verified … Verified on internal round-trip fixtures". That was true only
  of this crate's own output: fed a stock `ffmpeg`-produced FLAC file, the
  decoder **panicked** (`attempt to shift left with overflow`), and the
  encoder emitted a bitstream no other decoder could read — the two sides
  were a self-consistent pair that could not interoperate with any real
  FLAC. Both were rebuilt against RFC 9639 and are now verified in **both**
  directions against `libFLAC`/`ffmpeg`. See the FLAC entry below.
- **AVIF — promoted from Bitstream-parsing to Functional.**
  `avif/mod.rs::decode()` is wired to the AV1 keyframe decoder and produces
  real pixels; a real `ffmpeg`+libaom fixture decodes byte-identically to
  ffmpeg's own dav1d decode. The AVIF `iloc` parser was also fixed — it
  accepted only version 1 (which is what *this crate's* encoder writes),
  so it would have rejected essentially every real-world AVIF file before
  reaching AV1 decode at all. Alpha and >8-bit items still refuse
  honestly. See the AVIF entry below.
- **Opus in `oximedia-codec` — demoted from Functional (CELT + SILK +
  Hybrid) to Bitstream-parsing, on empirical evidence.** The 0.1.9 re-audit
  promoted this entry upward by *reading* `opus/silk_decoder.rs` and
  confirming it was wired; no real libopus-encoded packet was ever fed
  through it in that audit. Doing so in 0.2.1 gives: 26 real libopus CELT
  packets decode to `Ok` with **0 non-zero samples out of 49,920** (i.e.
  silence), and real SILK packets produce out-of-range output (max abs
  4.0) at 960 samples/frame regardless of the configured rate. The encoder
  is worse than untested — it emits a TOC byte that misdescribes its own
  payload (0x1c, claiming SILK/NB/60 ms for a CELT/FB/20 ms frame). Under
  this document's own taxonomy, "returns empty/constant data" is
  Bitstream-parsing. Every wired consumer of Opus in the workspace now
  refuses with an honest `Err` rather than passing the silence on. This
  concerns `crates/oximedia-codec/src/opus/` only; `oximedia-audio` has a
  separate implementation that was not part of this audit. See the Opus
  entry below.

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

### AV1 — Functional (keyframe/intra only)

- **Module:** `crates/oximedia-codec/src/av1/` (keyframe pipeline:
  `av1/kf/` — `bits`, `msac`, `hdr`, `cdfs`, `coef`, `pred`, `itx`,
  `recon`, `lf`, `cdef`, `lr`, plus `tables_*`/`consts` machine-extracted
  from the AV1 specification sources and cross-checked against libaom
  v3.12.1).
- **Current state (0.2.0):** keyframe/intra ✓ — 8-bit 4:2:0 profile 0,
  bit-exact vs dav1d/aomdec incl. deblock, CDEF and loop restoration;
  superres, film grain, palette, intrabc, qmatrix, 10/12-bit and inter are
  honest `Err` (mirrors the `lib.rs` Codec Feature Matrix row). The `kf`
  module is an exact port of the intra decode path of the AV1
  specification (symbol/range decoder, sequence/frame header parsing,
  transform-coefficient decode, intra prediction including CFL, exact
  inverse transforms, tile/partition/block decode driver), and
  `av1/decoder.rs::decode_temporal_unit` calls into it directly — the old
  no-op `"Tile group data would be processed here"` branch is gone.
  Verified bit-exact (0 differing pixels, Y+U+V) against **both** dav1d
  1.5.1 and aomdec (libaom) on 13 keyframe vectors (aomenc + SVT-AV1
  encodes incl. lossless, 128×128 superblocks, 2 tile columns, an odd
  76×42 crop, and 320×192 loop-restoration cases), with the full
  post-filter chain applied: deblocking loop filter, CDEF, and loop
  restoration (Wiener + self-guided/SGRPROJ). Tests:
  `stage1_lossless_gray64_bit_exact` through
  `stage4_switchable_wiener_320x192_bit_exact` in `av1/kf/mod.rs`.
- **What is missing (each an honest `CodecError::UnsupportedFeature` —
  never a fabricated frame):** inter-frame decode (`av1/kf/hdr.rs:440`),
  intra block copy (`av1/kf/recon.rs:665`), palette mode
  (`av1/kf/recon.rs:739`), 10/12-bit / monochrome / 4:2:2 / 4:4:4
  (`av1/kf/recon.rs:1475`), horizontal super-resolution upscaling
  (`av1/kf/recon.rs:1482`), quantizer matrices (`av1/kf/recon.rs:1488`),
  and film-grain synthesis on output (`av1/kf/recon.rs:1494`).
- **Effort to close the inter gap:** specialist.
- **Target:** inter and the remaining surfaces 0.2.x. Tracked by GitHub
  issue #9. *(Audited against 0.2.0 source on 2026-07-15 — see "0.2.0
  re-audit summary" above.)*

### VP9 — Verified (key frames, intra-only **and** inter frames, bit-exact vs libvpx)

- **Module:** `crates/oximedia-codec/src/vp9/dec/` (renamed from `vp9/kf/`
  when inter decode landed): `booldec`, `hdr`, `itx`, `lf`, `pred`,
  `recon`, `scan`, `tables` (the intra pipeline) plus `state`, `refs`,
  `counts`, `adapt`, `predctx`, `mvref`, `modeinfo`, `mc`, `interpred`,
  `tables_inter` (the inter pipeline).
- **Current state (0.2.1):** all three frame types decode to pixels —
  key frames, intra-only frames, and inter frames. The intra pipeline is
  unchanged from 0.2.0 (boolean/range decoder, inverse DCT/ADST, the
  lossless 4×4 Walsh-Hadamard transform, loop filter, tile/partition/block
  driver). The inter path adds: persistent cross-frame state (four frame
  probability contexts, an 8-slot MI-aligned reference DPB carrying per-MI
  motion-vector grids, loop-filter-delta and segmentation inheritance),
  the `find_mv_refs` candidate scan with sign-bias flipping and temporal
  candidates, inter mode / reference-frame / motion-vector entropy decode
  (including sub-8×8 and the `use_mv_hp` gate), the `vpx_convolve8` eight-tap
  motion-compensation family across the four interpolation filter sets,
  compound prediction, whole-superblock prediction before the residual
  loop, backward probability adaptation, `show_existing_frame` redisplay,
  and superframe (hidden-frame) unpacking. Every function is a port of
  libvpx **v1.15.2** with per-function `file:line` citations.
- **Verification tier — verified, against libvpx, multi-frame, through the
  public API:** **10 of 10** non-deferred conformance streams decode
  bit-exactly via `VideoDecoder::send_packet` / `receive_frame`, with
  **0 `#[ignore]`, 0 PSNR thresholds and 0 skips**:

  | stream | dims | coded/shown | what it pins |
  |---|---|---|---|
  | `p9still` | 76×42 | 8/8 | static content; backward adaptation across 8 frames |
  | `p9basic` | 76×42 | 8/8 | ordinary LAST-referencing P frames |
  | `p9er` | 76×42 | 8/8 | `error_resilient` (adaptation structurally off) |
  | `p9hp` | 76×42 | 8/8 | high-precision (⅛-pel) motion vectors |
  | `p9tc` | 512×64 | 5/5 | two tile columns |
  | `p9alt` | 128×128 | 8/8 | a hidden ALTREF **sub-frame** inside the packet-1 superframe (decodes, refreshes slot 2, emits nothing); the only native fixture with `sign_bias` flips, and 33 of its blocks measurably decode as two-reference compound |
  | `compound` | 96×64 | 13 coded / 12 shown | a second, independent hidden-ARF stream whose hidden sub-frame sits on a *different* `frame_context_idx` than the shown chain, so it pins per-index context save/load too (52 measured compound blocks) |
  | `switch` | 100×68 | 10/10 | more than one interpolation filter, and the only fixture with a partial last superblock row **and** column (`mi_rows = (68+7)>>3 = 9`) |
  | `p9sef` | 76×42 | 2/2 | `show_existing_frame` redisplay |
  | `p9io` | 352×288 | intra-only + SEF | intra-only frame decode |

  Suite: `src/vp9/dec/inter_fixture_tests.rs` for the nine inter streams and
  `src/vp9/decoder.rs`'s test module for `p9io`; fixture provenance and the
  exact `ffmpeg`/libvpx commands: `src/vp9/dec/testdata/RECIPE.md` and
  `RECIPE-p9io.md`. The
  gate carries its own anti-vacuity test
  (`the_gate_is_not_vacuous_a_planted_mismatch_is_reported`) and a coverage
  assertion (`the_corpus_genuinely_exercises_the_inter_machinery`), which
  harvests the decoder's own symbol counters across the corpus and enforces
  floors on each: compound (two-reference) blocks **and** the `comp_ref`
  reads that select their references, all four inter modes
  (NEARESTMV/NEARMV/ZEROMV/NEWMV), both intra and inter blocks *inside*
  inter frames, genuine per-block filter choice on SWITCHABLE frames (not
  always landing on EIGHTTAP), non-zero motion vectors in each component
  separately and together, and the ⅛-pel high-precision bit actually being
  read. Its point is that a stream may *permit* a feature in its headers
  without any block exercising it — the floors are on decoded symbols, not
  on header flags.
- **What is still missing (each an honest
  `CodecError::UnsupportedFeature`, refused at a *pinned packet index* so
  that a break earlier in the stream cannot masquerade as the expected
  deferral):**
  - **Inter-frame segmentation** — `read_inter_segment_id` and the
    predicted segment map. Fixture `p9seg` is committed: its key frame
    (which uses intra segmentation, and works) decodes bit-exactly, then
    packet 1 refuses by name.
  - **Reference scaling** — the scale factors and `vpx_scaled_2d` path
    used when a reference frame's dimensions differ from the current
    frame's. Fixture `scaled` is committed: packets 0–5 (96×64, five real
    inter frames) decode bit-exactly, then packet 6 refuses naming
    `96x64` vs `64x48`.
  - **Profiles 1–3** — 4:2:2 / 4:4:4 subsampling and 10/12-bit depths
    (`vp9/dec/mod.rs:102`).
- **Effort to close the two deferrals:** small–medium each (both are
  scoped, both already have a committed real-bitstream gate fixture
  waiting).
- **Target:** 0.2.x. *(Inter and intra-only decode landed and audited
  against 0.2.1 source on 2026-08-12.)*

### VP8 — Verified (key frames **and** inter frames, bit-exact vs libvpx)

- **Module:** `crates/oximedia-codec/src/vp8/dec/` (`bool_decoder`,
  `header`, `state`, `mode`, `mv`, `mc`, `refs`, `inter`, `residual`,
  `recon`, `predict`, `loopfilter`, `filter`, `tables`, `tables_inter`,
  `transform`).
- **Current state (0.2.1):** both frame types decode to pixels. Key frames
  run the RFC 6386 §11–§15 intra pipeline (macroblock mode parsing, DCT
  token decode, dequantise/inverse-transform, intra prediction, in-loop
  deblocking), ported from the production-verified `oximedia-image`
  WebP/VP8 still-image decoder. Inter frames add the §16 per-macroblock
  prediction records (including the §16.3 near-MV survey), §17 motion-vector
  entropy decode, §18 sub-pixel motion compensation (six-tap and bilinear,
  with the version-3 full-pel-chroma rule), §9.7–§9.8 last/golden/altref
  management with the reference decoder's sequential (non-swap) update
  order, and the §9.9 `refresh_entropy_probs` snapshot/restore — which also
  closed a latent key-frame gap, since that bit is coded for both frame
  types. `vp8/decoder.rs` routes every frame through
  `dec::Vp8SequenceDecoder`.
- **Verification tier — verified, against libvpx, multi-frame:** five
  libvpx-encoded streams decode **bit-exactly**, every shown frame, all
  three planes, against libvpx's own reconstruction of the same bytes:
  `p5basic` (5 frames, LAST-only P frames, inherited loop-filter deltas),
  `refswap` (19 coded / 18 shown, a genuine hidden altref frame, altref
  sign bias, and a `refresh_golden` + `copy_buffer_to_alternate=2` frame),
  `refswap_er` (5, `refresh_entropy_probs = 0` on every frame),
  `splitmv` (5, 100x64 — a non-macroblock-aligned width — with a strong
  diagonal pan), `segdelta` (5, segmentation with per-segment quantiser
  deltas). Suite: `src/vp8/dec/inter_fixture_tests.rs`; fixture provenance
  and the exact ffmpeg/libvpx commands: `src/vp8/dec/testdata/README.md`.
  Key frames additionally stay bit-exact against libwebp (`dwebp -yuv`) on
  three still-image vectors (`tests/vp8_real_bitstream.rs`).
- **Hidden frames:** a frame with `show_frame == 0` is decoded and updates
  the reference buffers, but is never emitted — `send_packet` returns
  `Ok(())` and queues nothing, so a packet-in/frame-out loop legitimately
  sees fewer frames than packets. No placeholder frame is fabricated.
- **Known bounded gap (documented, not silently clamped):** motion
  compensation validates every read against the reference surface's 32-pixel
  replicated border. Non-`SPLITMV` vectors are clamped during decode to at
  most 16 pixels beyond the frame edge, so with the six-tap reach they stay
  within 19 of 32; `SPLITMV` sub-vectors are deliberately **not** re-clamped
  (RFC 6386 §18.1), so a pathological stream can point further out than the
  border covers. That is reported as `CodecError::InvalidBitstream` rather
  than silently clamped or read out of bounds; accepting it would mean a
  wider margin or `build_mc_border`-style edge emulation. No stream in the
  conformance set triggers it.
- **What is still missing:** nothing in the RFC 6386 decode path that the
  five conformance streams exercise. Not covered by a fixture (so: parsed
  and implemented, but unproven): bitstream versions 1–3 (bilinear /
  full-pel profiles — libvpx emits version 0 for every stream here), and
  multi-token-partition **inter** frames (the 4-partition key frame in
  `tests/vp8_fixtures/` covers the partition setup itself).
- **Target:** 0.2.x. *(Inter decode landed and audited against 0.2.1 source
  on 2026-08-11.)*

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

### AVIF — Functional (8-bit 4:2:0 colour items, via the AV1 keyframe decoder)

- **Module:** `crates/oximedia-codec/src/avif/` — `mod.rs` (public API +
  `probe`), `container.rs` (ISOBMFF/`meta`/`iloc` parsing), `decode.rs`
  (AV1-backed pixel decode, compiled only under the `av1` feature).
- **Current state (0.2.1):** `decode()` produces real pixels. It validates
  the ISOBMFF container, extracts the AV1 OBU payload(s), and feeds them
  through `crate::av1::Av1Decoder` — the same keyframe/intra decoder that
  is bit-exact against dav1d/aomdec (see the AV1 entry above). Scope
  therefore matches that decoder exactly: **8-bit 4:2:0**. The
  `#[cfg(not(feature = "av1"))]` build keeps an honest
  `CodecError::UnsupportedFeature` rather than a second, weaker code path.
  The earlier revision that returned the raw AV1 bitstream in the
  `y_plane` field as if it were decoded pixels remains gone.
- **Verification:** `testdata/color_only.avif` (a real `ffmpeg` + libaom
  encode) decodes **byte-identically** to ffmpeg's own dav1d decode of the
  same file, Y/U/V planes compared in full against the committed
  `color_only_ref.yuv`. Regeneration commands are in `testdata/README.md`.
- **Fixed in the same pass — a parser bug that would have rejected almost
  every real file:** the `iloc` reader accepted only box **version 1**,
  which is what *this crate's own* AVIF encoder writes. Real-world
  encoders (`ffmpeg`, libaom) write **version 0**, which has no
  `construction_method` field. Every such file was refused before AV1
  decode was ever reached. Both versions are now parsed, through a
  bounds-checked reader that returns `Err` rather than panicking on
  truncated or fuzzed input (two truncation-fuzz tests pin this).
- **What still refuses honestly** (each an
  `CodecError::UnsupportedFeature`, each with a committed fixture proving
  it is a refusal and not a crash or a fabrication):
  - **Alpha** — `testdata/with_alpha.avif`. Real-world AVIF encodes the
    alpha auxiliary item as **monochrome** AV1 (`mono_chrome = 1`), which
    the AV1 keyframe decoder does not implement. The colour and alpha
    items are treated as inseparable: an image whose alpha cannot be
    decoded is refused by name, never returned as if it were opaque.
  - **10/12-bit** — `testdata/color_10bit.avif`, refused through the same
    AV1 gate, which also proves the *colour* path propagates the AV1
    decoder's honest errors rather than only the alpha path.
  - `iloc` version ≥ 2, non-file `construction_method`, multi-extent
    items, and non-zero `base_offset_size`.
- **Effort to promote to Verified / close the gaps:** the remaining AVIF
  work is not AVIF work — alpha needs AV1 monochrome and the 10-bit
  fixture needs the AV1 high-bit-depth pipeline, both tracked under AV1.
- **Target:** 0.2.x for alpha/high-bit-depth, following AV1.

### WebP — Functional (VP8L lossless + VP8 lossy)

- **Module:** `crates/oximedia-codec/src/webp/`
- **Current state:** VP8L (lossless) decoder is real and self-consistent.
  VP8 lossy decode is now wired too (0.2.1,
  `crates/oximedia-codec/src/webp/vp8_decoder.rs`, `vp8` feature): a lossy
  WebP's `VP8 ` chunk is a VP8 key frame (RFC 6386), so this module parses
  the RIFF container with the existing `WebPContainer::parse` and calls the
  already-shipped, bit-exact `vp8::decode_keyframe` — no new VP8 decoding
  was written. Verified bit-exact against the `dwebp -yuv` reference on the
  3 real-bitstream vectors in `tests/vp8_fixtures/` (grad32 32x32, tex48x40
  48x40, and a 4-partition libvpx 48x48 key frame), each wrapped in a
  hand-built RIFF container in `tests/webp_vp8_lossy.rs`; that claim is
  bounded by those vectors, the same way the underlying VP8 decoder's
  bit-exactness claim is. The no-alpha path returns native YUV 4:2:0 (not
  converted to RGB) so it stays pixel-identical to that reference; an
  extended-format (`VP8X`+`ALPH`+`VP8 `) file promotes the output to
  RGBA32 by decoding the alpha chunk and converting YUV->RGB.
  `ImageDecoder::decode_webp` (`crates/oximedia-codec/src/image.rs`, which
  already dispatched lossy WebP to a `Vp8Decoder` call since 0.1.2 but had
  no test coverage) now delegates to this module instead of duplicating the
  call, so there is one lossy-decode implementation, not two.
- **What is missing:** VP8L-compressed `ALPH` alpha
  (`AlphaCompression::WebPLossless` in `webp/alpha.rs`) still returns an
  honest `CodecError::UnsupportedFeature` — that boundary predates this
  change and is unrelated to the VP8 decode wiring; only the uncompressed
  alpha mode (all four spatial filters) is decoded.
- **Effort:** small (done, 0.2.1) for the VP8 wiring; small for the
  remaining VP8L-compressed-alpha gap, whenever it is picked up.
- **Target:** 0.2.1 (VP8 wiring shipped); VP8L-compressed alpha 0.2.x+.

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
- **Current state (re-checked 2026-07-15):** Headers parse (and are
  validated). `decode_audio_packet` returns an honest
  `CodecError::UnsupportedFeature` — "Vorbis full decode not implemented:
  stateless audio-packet decode is unsupported" — rather than fabricated
  empty samples (`vorbis/decoder.rs:275`; this entry previously described
  an `Ok(Vec::new())` return, which is gone from the source). Floor
  reconstruction is a "simplified linear interpolation" placeholder
  (`vorbis/decoder.rs:567`). A real MDCT exists but is only wired to
  `decode_packet`'s custom OxiMedia simplified packet format, not to real
  Vorbis streams.
- **What is missing:** full Vorbis reverse codebook decode, residue, floor
  curve, MDCT/IMDCT, overlap-add, window switching, channel coupling.
- **Effort:** specialist.
- **Target:** 0.2.0+.

### Opus — Bitstream-parsing (decode returns silence / out-of-range output on real packets)

- **Module:** `crates/oximedia-codec/src/opus/` (this entry does **not**
  cover `oximedia-audio`, which carries a separate implementation that has
  not been re-audited).
- **Headline (0.2.1, empirical):** this codec is **not usable in either
  direction**, and the previous "Functional" label was reached by reading
  the source rather than by decoding a real file.
  - **Decode, fed 26 real libopus-encoded CELT packets:** returns `Ok` with
    **0 non-zero samples out of 49,920** — i.e. digital silence, which this
    document's taxonomy classifies as Bitstream-parsing ("returns
    empty/constant data"), not Functional.
  - **Decode, fed real libopus SILK packets:** produces out-of-range values
    (max abs 4.0, outside the ±1.0 a normalised sample may take) and emits
    960 samples per frame regardless of the configured 16 kHz rate.
  - **Encode:** emits a TOC byte that misdescribes its own payload — `0x1c`
    claims SILK/NB/60 ms for what is a CELT/FB/20 ms frame — so a
    conformant decoder reads 2880 samples where 960 were encoded. Where
    sample counts do line up, decoded energy and correlation with the input
    are both 0.0.
  - Consequently **every wired consumer refuses rather than passing the
    output on**: `oximedia-videoip` returns an honest `Err` for Opus in
    both directions; `oximedia-normalize`'s batch path sniffs `OpusHead`
    and errors with text naming the silence problem;
    `oximedia-transcode`'s frame-level path keeps Opus disabled
    (`frame_level.rs:128`, `audio_adapters.rs:369`).
- **Why the 0.1.9 entry said otherwise:** that re-audit corrected the entry
  *upward* from "CELT only" after confirming that a normative SILK decoder
  existed and was genuinely dispatched to. That much is still true — the
  code below is real, wired code, not a stub — but "wired" was mistaken for
  "correct". No libopus-encoded fixture was fed through it in that pass.
  The structural description that follows is retained because it remains an
  accurate account of *what is implemented*; it is the output that is
  wrong.
- **What is implemented (retained from the 0.1.9 audit, structure only):**
  all three Opus layers have real, wired decode paths.
  - **CELT** (`opus/celt.rs`): real MDCT / pitch / band decode, unchanged
    since 0.1.7.
  - **SILK** (`opus/silk_decoder.rs` + `opus/silk.rs` + `opus/silk_ltp.rs` +
    `opus/silk_lpc.rs` + `opus/silk_excitation.rs`): a normative RFC 6716
    §4.2 decoder — LSF stage-1/2 decode, NLSF stabilisation and NLSF→LPC
    conversion, LCG-seeded shell-pulse excitation decode (LSB + sign rounds),
    LTP + LPC synthesis, resampled to the caller's output rate.
    `opus/silk.rs::SilkDecoder::decode_into` — the public path `OpusDecoder`
    actually calls — invokes `decode_silk_frame` directly; this is not a
    test-only helper. Heavily annotated against the libopus reference
    (`silk_NLSF2A`, `silk_NLSF_unpack`, `silk_LPC_inverse_pred_gain_c`,
    `A_LIMIT = 0.99975`, etc.) and clamps output to finite/bounded
    (`clamp(-4.0, 4.0)`) values. ~82 unit/integration tests across the SILK
    modules (70 in `silk.rs`, 12 in `silk_decoder.rs`, plus
    `tests/silk_ltp_roundtrip.rs`), including a same-input
    high-level-API-vs-direct-call regression test
    (`test_silk_decode_high_level_vs_direct`).
  - **Hybrid** (`opus/hybrid.rs`): decodes SILK (low band, ≤ 8 kHz) and CELT
    (high band) from the *same* range-coded bitstream per RFC 6716 §4.5 (SILK
    decodes first; CELT continues from the bytes SILK did not consume) and
    sums the two bands through per-channel crossover biquad filters. The
    previous "split the packet at its midpoint" heuristic — which RFC 6716
    does not define — has been removed.
  - The prior stub citation (`src/silk.rs:873` "emits a comfort-noise
    indicator byte") no longer corresponds to any code in the tree; DTX /
    comfort-noise suppression on the *encoder* side (`opus/encoder.rs`,
    `dtx_silence_frames`) is real and separately wired, and is not part of
    decode.
- **Why the existing tests did not catch this:** coverage is internal
  round-trip and self-consistency only (~82 unit/integration tests across
  the SILK modules). A self-consistent encoder/decoder pair agreeing with
  each other proves nothing about either one's conformance — the same class
  of blind spot that the FLAC codec turned out to have, and that its
  rebuild (below) closed by testing against `libFLAC`/`ffmpeg` instead.
- **⚠️ Provenance of the numbers above, so this entry is not promoted back
  by another source-reading pass:** they were **measured during the 0.2.1
  audit with an ad-hoc harness, and no committed fixture reproduces them.**
  No `.opus` fixture exists in the tree. What *is* committed is the
  consequence, not the measurement: tests such as
  `oximedia-videoip`'s `opus_fails_honestly_in_both_directions` assert that
  the wired surfaces **refuse**, and that the refusal message names the TOC
  defect — they do not decode a real packet and check for silence. A future
  auditor must therefore re-measure against real libopus output before
  changing this label in either direction, exactly as the 0.1.9 pass should
  have done and did not. Committing a small libopus fixture that pins the
  silence would close this hole and is the cheapest guard available.
- **Effort to fix:** large / specialist. This is not a wiring gap; it needs
  a decoder debugged against libopus output packet-by-packet, plus an
  encoder rebuilt against RFC 6716 §3.1 (TOC) and validated by libopus
  decoding it. A libopus / opus-tools reference fixture corpus and the
  official RFC 6716 test vectors are prerequisites, not follow-ups.
- **Target:** 0.2.x+ (unscheduled — see `TODO.md`'s "Deferred (0.2.x)").

### FLAC — Verified (RFC 9639, both directions, vs libFLAC/ffmpeg)

- **Module:** `crates/oximedia-codec/src/flac/` — `bitio.rs`, `frame.rs`,
  `subframe.rs`, `residual.rs`, `lpc.rs`, `rice.rs`, `encoder.rs`,
  `decoder.rs`. (`crates/oximedia-audio/src/flac/` is a **separate**
  implementation and was not touched or audited in this pass.)
- **Current state (0.2.1):** encoder and decoder were both **rebuilt** to
  RFC 9639 and are verified in both directions against external tools:
  this crate decodes stock `ffmpeg`/`libFLAC` output sample-exactly, and
  `flac -t`/`ffmpeg` validate this crate's output sample-exactly.
  `tests/flac_external.rs` **degrades loudly, not silently**: when a binary
  is missing it prints an explicit `PARTIAL: … not on PATH` line for the
  check it could not run, rather than passing quietly as if it had. In the
  0.2.1 verification environment `flac`, `ffmpeg` and `metaflac` were all
  present and all checks genuinely ran (0.224 s / 0.296 s / 0.732 s), so
  this promotion rests on external comparisons that actually executed — but
  a CI host without those binaries will report PARTIAL, and that output must
  be read rather than assumed green. Constant, fixed, LPC and verbatim
  subframes, all four stereo decorrelation modes, both residual coding
  methods with escaped partitions, and wasted-bits handling are covered.
- **What the previous "Functional / Verified" label was hiding:** the two
  sides were a self-consistent toy pair. Fed a stock `ffmpeg`-produced
  FLAC file the decoder **panicked** (`attempt to shift left with
  overflow`), and essentially every field was mis-laid-out relative to the
  spec — subframe header written as a whole byte rather than 1+6+1 bits,
  LPC coded as the reserved `01xxxxxx` type, Rice unary inverted
  (ones-then-zero; the spec is zeros-then-one), block size stored without
  its `-1`, sample rate hard-coded, bit-depth nibble landing on a reserved
  code for 16-bit. The LPC round-trip break was structural rather than a
  bug: the encoder computed residuals from **float** coefficients while the
  decoder dequantised and rounded, which can never be lossless. Both sides
  now use identical `i64` accumulation with the quantised coefficients that
  are actually written.
- **Effort:** none outstanding for the covered scope.

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

### AAC — decode: not implemented (returns error); planned via external extension

- **Module:** `crates/oximedia-audio/src/aac.rs` (note: lives in
  `oximedia-audio`, not `oximedia-codec`).
- **Current state:** **Bitstream-parsing only.** `AdtsHeader::parse` is a real
  ADTS transport-header parser (sync word, MPEG version, profile, sampling
  frequency index, channel configuration, frame length, CRC flag), and
  `AacObjectType` carries the ISO 14496-3 object-type ids. There is **no
  decoder**: no spectral (Huffman) decode, no inverse quantisation, no
  filterbank, no SBR/PS. `AacDecoder::send_packet` and
  `AacDecoder::receive_frame` fail closed with
  `AudioError::UnsupportedFormat("AAC decoding is not implemented …")`;
  `send_packet` first records the stream's sample rate / channel layout from
  the ADTS header so inspection still works.
- **Honesty fix (0.2.1):** the previous revision returned `Ok(AudioFrame)`
  whose samples came from reinterpreting raw payload bit-patterns as 4-bit
  quantised coefficients through a stub IMDCT — audio unrelated to the encoded
  content — and reported `CodecId::Mp3` as its codec. Both are removed.
  `oximedia_core::CodecId::Aac` now exists as an *identification-only* variant,
  so `AacDecoder` implements `AudioDecoder` again with `codec() ->
  CodecId::Aac`; every decode entry point still fails closed.
  `CodecId::Aac` is deliberately unreachable from `FromStr` (`"aac"`,
  `"mp4a"`, `"aac-lc"`, `"he-aac"` all return `Err`) so no name-driven code
  path can select it, and the MP4/Matroska demuxer whitelists are unchanged —
  they still reject `mp4a` tracks with `PatentViolation`.
- **What is missing:** everything below the transport layer — AAC-LC spectral
  decode (scalefactors, Huffman codebooks, TNS, M/S and intensity stereo,
  PNS), the IMDCT filterbank with window-shape/sequence switching, and
  optionally SBR (HE-AAC v1) and PS (HE-AAC v2).
- **Plan:** real AAC decode is expected to arrive through a separate platform
  **extension crate** (for example AudioToolbox on macOS via `objc2`), not as
  an in-tree pure-Rust decoder. Patents are not the blocker — the
  Fraunhofer/Via Licensing AAC patents expired in April 2023 — engineering
  effort is.
- **Effort for an in-tree pure-Rust decoder:** specialist.

## Network & DRM crypto status

New in the 0.1.9 re-audit: the same honesty discipline applied to codecs
above is extended here to the cryptographic paths in `oximedia-net` (WebRTC,
SRT) and `oximedia-drm` (segment packaging + license systems). This section
was produced by reading the modules listed below directly on 2026-07-08.
`SECURITY.md` should reference this table rather than re-describing crypto
status inline, so the two documents cannot drift out of sync.

### Crypto honesty taxonomy

| Label | Meaning |
|-------|---------|
| **Real** | An actual cryptographic primitive from a vetted implementation (RustCrypto `aes`/`ctr`, the `aes-gcm` crate, `rustls` certificates) operating on real key material; interoperates with the standard it claims to implement. |
| **Non-interoperable placeholder** | The message formats / API surface for a licensed or proprietary DRM system are present, but the real license-server protocol, key derivation, or hardware trust anchor is not implemented — cannot interoperate with actual Apple / Google / Microsoft infrastructure. In-source comments self-describe these as "partial implementation for educational purposes." |
| **Experimental (do not use for confidential media)** | Explicitly documented in-source as insecure, simulated, or unimplemented. Where possible the code fails closed (returns an error) rather than fabricating protection. |

### Status table

| Component | Module | Status | Notes |
|-----------|--------|--------|-------|
| SRT payload encryption | `oximedia-net/src/srt/crypto.rs` | **Real** | AES-128/192/256-**CTR** (NIST SP 800-38A) via RustCrypto `aes`+`ctr`, variant selected by key length; a NIST known-answer-vector test pins the CTR keystream output. |
| SRT authenticated encryption (extension) | `oximedia-net/src/srt_aes256gcm.rs` | **Real** | AES-256-**GCM** AEAD via the `aes-gcm` crate; PBKDF2-SHA256 key derivation from a passphrase + salt, 12-byte nonce (4-byte stream-ID⊕sequence prefix ‖ 8-byte random base), configurable key-rotation policy. |
| SRT key-exchange (KEK) wrap | `oximedia-net/src/srt/key_exchange.rs` | **Experimental** | `wrap`/`unwrap` are explicitly documented as an "XOR-based key wrapping (simulation only; production uses RFC 3394 AES-WRAP)" — do not rely on this path for real key wrapping. |
| WebRTC DTLS handshake | `oximedia-net/src/webrtc/dtls.rs` | **Experimental — NOT implemented** | Module doc: "⚠️ Experimental — DTLS-SRTP handshake is NOT implemented." Generates a real self-signed Ed25519 certificate + SDP fingerprint for signaling, but implements no DTLS 1.2 record layer / handshake / RFC 5764 `use_srtp` key export. `DtlsEndpoint::handshake` fails closed (returns an error) instead of fabricating all-zero SRTP keys — but WebRTC media transport must not be used for confidential media until this lands. |
| WebRTC SRTP/SRTCP packet protection | `oximedia-net/src/webrtc/srtp.rs` | **Experimental — simulated** | Module doc: "a simulated SRTP/SRTCP implementation for WebRTC... we simulate with an XOR-based scheme and a checksum auth tag." No real AES-CM / AES-GCM is applied to RTP payloads on this path. |
| DRM CENC/CMAF segment encryption | `oximedia-drm/src/cmaf_encrypt.rs`, `aes_ctr.rs`, `aes_cbc.rs` | **Real** | AES-128-CTR (`cenc`), AES-128-CBC + PKCS#7 (`cbc1`), and partial-block CBC (`cbcs`) per ISO/IEC 23001-7, delegating to real block-cipher modules. |
| DRM W3C Clear Key | `oximedia-drm/src/clearkey.rs` | **Real (by design; dev/test-grade)** | Implements the W3C Clear Key spec, which is itself an unencrypted key-exchange scheme intended for testing / open platforms — not suitable for production content protection, but that is what the spec defines, not a shortfall in this implementation. |
| DRM FairPlay Streaming | `oximedia-drm/src/fairplay.rs`, `fairplay_rpc.rs` | **Non-interoperable placeholder** | Module doc: "partial implementation for educational purposes. Full FairPlay integration requires Apple developer credentials and FPS SDK." SPC/CKC message shapes exist; there is no real Apple FPS key exchange. |
| DRM Widevine | `oximedia-drm/src/widevine.rs`, `widevine_rpc.rs` | **Non-interoperable placeholder** | Module doc: "partial implementation for educational purposes. Full Widevine integration requires licensed CDM libraries." |
| DRM PlayReady | `oximedia-drm/src/playready.rs`, `playready_rpc.rs` | **Non-interoperable placeholder** | Module doc: "partial implementation for educational purposes." |

**Headline takeaway:** real, interoperable content-key cryptography exists
for **SRT transport** (AES-CTR and AES-256-GCM) and for **CENC/CMAF segment
packaging + Clear Key licensing** — these are safe to build on today.
**WebRTC media transport security is not implemented** (no DTLS handshake,
simulated SRTP) and must not carry confidential media yet. **FairPlay /
Widevine / PlayReady** provide message-format and workflow scaffolding only —
they do not interoperate with real Apple / Google / Microsoft license
infrastructure and must not be represented as "supports FairPlay / Widevine /
PlayReady DRM" in customer-facing material without this caveat.

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

**0.1.9 addendum:** this document itself had gone stale in the opposite
direction for one codec — the 0.1.7 text under-claimed Opus, still labelling
SILK/hybrid as stubs after a real normative SILK decoder had shipped (see
"0.1.9 re-audit summary" and the revised Opus entry above). The lesson
generalises: a status document is only honest for as long as it is
re-verified against source, in both directions — demotions and promotions
alike — every time it is touched, not merely re-stamped with a new version
number.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-08-12

Theme: **the audio loudness stack is now standards-accurate and honest, and
OxiMedia gains Pure-Rust live camera/device capture.** The default K-weighting
filters were measurably wrong (up to ±35 dB), the crate-root `EbuR128Meter`
pointed at the approximate meter, the loudness normalizer never touched the
audio it claimed to normalise, the MIR log-mel front-end returned placeholder
data, and the AAC decoder fabricated PCM. All fixed. Separately, a new
`oximedia-capture` crate adds AVFoundation/V4L2/Media Foundation camera
capture with an honest, per-platform verification story (macOS
hardware-verified during package A5; Linux/Windows implemented and
host-tested, device-runs still open) — see its Added entries below and
`crates/oximedia-capture/README.md`.

Second theme: **VP8 and VP9 now decode inter frames bit-exactly, FLAC is
conformant in both directions, and a workspace-wide sweep replaced
fabricated success with either real work or an honest `Err`.** The sweep is
the larger half. Nine areas were shipping code that reported success
without doing the work — `oximedia-videoip` exposed hardcoded 1920×1080
"decoders" and encoders that emitted bytes no decoder could read;
`oximedia-mam`'s S3/Azure/GCS backends logged and returned `Ok`, with
`exists()` unconditionally `true`; `oximedia-server` uploaded nothing to
any CDN and fabricated "presigned" URLs that carried no signature;
`oximedia-workflow`'s Transcode/QC/Analysis/HTTP tasks returned `Ok(())`
without executing; `oximedia-farm` slept instead of transcoding;
`oximedia-repair` returned empty vectors; `oximedia-review` returned empty
lists in place of persistence; and `oximedia-denoise` wrote every filtered
pixel into a clone it immediately discarded, so most of the denoiser
returned its input unchanged. Each is now either real or refuses by name.
The count of `TODO(0.2.x)` markers in Rust source fell from **123 to 22**
(the same command counted 117 at the previous, 2026-07-15 harvest; the tree
gained six between then and the start of this session). The 22 survivors are
enumerated in `TODO.md`'s "Deferred (0.2.x)" section with a `file:line`
each, alongside a separate list of real deferrals that carry no marker at
all and so would not survive a marker grep. Two codec labels moved in opposite directions:
FLAC and VP9 up to Verified on external fixtures, and the in-tree Opus
codec **down** to Bitstream-parsing after being fed real libopus packets
for the first time (see Known issues).

### Fixed
- **The lossy WebP encoder produced VP8 frames no decoder could read**
  (`oximedia-codec/src/webp/encoder.rs`). `WebPLossyEncoder::encode_rgb`'s
  output had never been decoded by anything in the tree, and had drifted into
  a bitstream that this crate's own bit-exact VP8 decoder rejected outright
  with `InvalidBitstream("VP8: token partition exceeds payload")` — at *every*
  dimension tried (5x3, 8x8, 16x16, 32x32, 64x48). A boolean arithmetic coder
  has no resynchronisation point, so each of the faults below independently
  corrupts everything downstream of it:
  - **The boolean encoder was not RFC 6386 §7.3.** It batched the
    renormalisation shifts and emitted a byte afterwards, which advances the
    value register eight bits too far after the first byte — the emitted
    stream was the correct one *with a byte deleted*. It also never
    propagated the carry out of `bottom` into already-written bytes
    (`add_one_to_output`), and its flush omitted the specified
    `v <<= bit_count & 7` alignment. Now a verbatim transcription of §7.3,
    matching the RFC-verbatim encoder already used in the decoder's tests.
  - **`refresh_entropy_probs` (§9.8) was never written.** RFC 6386 §19.2
    codes it on key frames too, immediately after `quant_indices()`.
  - **The 1056 coefficient-probability-update gates (§9.9/§13.4) were written
    at probability 1/2** instead of each against its own `coeff_update_probs`
    entry, which the decoder uses to read them.
  - **The luma mode was coded as `B_PRED`, not `DC_PRED`.** The encoder
    emitted a single `false` at probability 145, but §11.2's `kf_ymode_tree`
    reaches `DC_PRED` three levels down; the bit written selects `B_PRED`,
    which then obliges 16 additional sub-block modes that were never sent.
  - **The DCT token coding (§13) was not the decoder's dual.** Tree node 0 is
    end-of-block, not "is zero", so every token was coded one node off; the
    initial token context ignored the above/left non-zero neighbours and was
    hard-coded to 0; the "skip the EOB branch after a literal zero" rule was
    absent; and the Y2 block was coded with block type 3 instead of 1.
  - **414 of the 1056 entries in the encoder's copy of `default_coeff_probs`
    were wrong** — a corrupted transcription, now regenerated from RFC 6386
    §13.5 and verified byte-for-byte against the decoder's table.
  - **The forward transforms were not invertible by §14.3/§14.4.** The
    forward DCT had the 2217/5352 rotation constants exchanged, and the
    forward Walsh-Hadamard pass omitted the factor-of-two normalisation the
    specified inverse expects, so luma DC came out doubled. Encoder-side
    reconstruction also used an approximate inverse DCT and a mis-scaled
    inverse WHT; it now uses exact §14.3/§14.4 transcriptions, so intra
    prediction is fed from what a decoder will actually reconstruct.
  - **Dequantisation factors disagreed with the decoder** at low quality:
    §14.1 applies the floor of 8 to the Y2 AC *product* (not to the table
    entry) and caps chroma DC at 132.

  `assemble_bitstream` was *not* at fault: the frame tag, the 19-bit
  `first_partition_size` (§9.1) and the token-partition offset were correct
  all along, and are unchanged. New round-trip tests in
  `tests/webp_vp8_lossy.rs` encode and decode at 5x3, 13x7, 16x16, 32x32 and
  64x48 — the last spanning several macroblock rows, so a reconstruction that
  disagreed with the decoder would compound visibly — and assert mean
  absolute luma error under a generous bound (measured 0.7-1.6 at quality
  90), plus a two-tone check and a truncation guard. External-decoder
  (libwebp/`dwebp`) validation of the *encoder* remains absent; the decoder
  side is already pinned bit-exact against `dwebp` fixtures.
- **Stereo and multichannel loudness read 3.01 LU too low**
  (`oximedia-metering/src/ebu_r128_impl.rs`): `complete_hop` divided the
  K-weighted hop energy by `n_samples × Σ G_ch`, averaging the weighted channel
  powers instead of summing them. ITU-R BS.1770-4 §2 defines loudness as
  `−0.691 + 10·log₁₀(Σ_ch G_ch · z_ch)` — a **sum**, with no division by channel
  count or weight sum — so every dual-mono stereo programme measured exactly
  10·log₁₀(2) = 3.01 LU quiet, and a normaliser targeting −14 LUFS would have
  over-amplified it by 3 dB. The single division by the hop length remains (that
  is the per-channel mean square); the channel term is now summed in all paths,
  which corrects momentary, short-term, integrated, gating and LRA together
  since all four derive from the same hop power. Mono is bit-identical to before.
  New conformance tests assert a dual-mono stereo sine reads +3.010 LU (±0.05)
  above the identical mono sine for **both** momentary and integrated, pin the
  absolute readings (−20 dBFS 997 Hz sine → −23.01 LUFS mono, −20.00 LUFS
  dual-mono stereo), and re-verify that the −10 LU relative gate still excludes
  a −60 dBFS section. `oximedia-audio`'s `R128Meter` already summed correctly
  (`r128_calibration_stereo_channel_sum_plus_3db`); the two meters now agree.
- **K-weighting filters were the wrong biquads** (`oximedia-metering/src/filters.rs`,
  and its clone in `oximedia-audio/src/loudness/filter.rs`). Stage 1 was
  implemented as a *high-pass* whose coefficients were scaled by the ITU shelf
  gain `3.9998` — a value in **decibels** — used as a linear multiplier, and
  Stage 2 was a +1 dB *high-shelf* instead of the RLB high-pass. Measured against
  ITU-R BS.1770-4 Table 1 at 48 kHz the chain was ≈ **−35 dB at 100 Hz** and
  ≈ **+9 dB at 4–10 kHz**. Both modules now use the exact Table 1 coefficients at
  48 kHz and the correct bilinear-transform designs (high-shelf + high-pass) at
  every other rate — the same designs `ebu_r128_impl::KWeightingFilter` already
  had. New regression tests in **both** crates pin the chain magnitude to
  −1.1335 dB @ 100 Hz, +0.6910 dB @ 997 Hz and +3.9680 dB @ 4 kHz within 0.1 dB,
  at 48 kHz and 44.1 kHz. Everything built on the shared filter bank — 
  `LoudnessMeter`, `LkfsCalculator`, `GatingProcessor`, `R128Meter`,
  `LoudnessNormalizer` — is corrected as a result.
- **Test expectations that encoded the filter bug** were corrected rather than
  deleted: `oximedia-metering`'s `test_ebu_r128_reference_signal` (997 Hz
  compensation `3.41 dB` → `0.691 dB`) and `oximedia-audio`'s four
  `r128_calibration_*` conformance goldens (which had been written against the
  broken chain's +3.4554 dB at 1 kHz; now +0.6977 dB, giving −3.003 / −9.024 /
  −23.003 LUFS mono and −19.993 LUFS stereo).
- A **third copy** of the broken Stage 1 design in
  `oximedia-audio/src/meters/itu.rs` was corrected the same way. Note that this
  file is an orphan: it is not declared in `meters/mod.rs`, is therefore not
  compiled, and does not currently build (two `E0502` borrow errors in
  `KWeightingFilter::process`). The correction is documented as unverified in the
  module header so that reviving the file does not resurrect the bug. It stays
  unwired deliberately — see the "Files in this directory that are deliberately
  not modules" section of `meters/mod.rs` for the full assessment of `itu.rs`
  and `dolby.rs`.
- **Crate root re-exported the wrong `EbuR128Meter`**
  (`oximedia-metering/src/lib.rs`): `oximedia_metering::EbuR128Meter` resolved to
  the approximate `ebu::EbuR128Meter` while the standards-accurate
  `ebu_r128_impl::EbuR128Meter` (exact Table 1 K-weighting, ITU-R BS.1771
  two-stage gating, EBU Tech 3342 LRA) was only reachable by module path. The
  root name now points at the accurate implementation; the program-type-aware
  wrapper remains available as `ebu::EbuR128Meter` with a doc note about its
  accuracy.
- **True peak was measured on channel 0 only**
  (`oximedia-metering/src/ebu_r128_impl.rs`): a loud right channel next to a
  quiet left channel was reported at the left channel's level. The 4×-oversampled
  detector now runs on **every** channel and `true_peak_dbtp()` returns the
  maximum across them, per ITU-R BS.1770-4.
- **`LoudnessNormalizer::normalize()` was a silent no-op**
  (`oximedia-audio/src/loudness/normalize.rs`): it measured the input, computed a
  correct gain, decoded the samples, scaled them in a temporary `Vec`, dropped
  it, and returned correct-looking parameters with the audio byte-identical
  ("Would need to convert back to bytes", "Would need to write samples back to
  frame"). Gain and true-peak limiting are now written back into the frame's own
  sample format.
- **Sample extraction assumed f32 and mis-interleaved planar buffers**
  (`oximedia-audio/src/loudness/{normalize,mod}.rs`): planar frames repeatedly
  pushed the *first* sample of each plane instead of walking them. Both paths now
  decode according to the frame's `SampleFormat` and interleave correctly.
- **`oximedia_mir::audio_features::compute_log_mel_spectrogram` returned fake
  data**: every mel bin of a frame held the same frame RMS, and the function
  name-shadowed the real implementation in `oximedia-audio`. It now delegates to
  `oximedia_audio::spectrum::compute_log_mel_spectrogram` (real windowed FFT via
  `oxifft` through a triangular HTK mel filterbank), with the conventions
  documented. New numeric tests assert a 1 kHz sine peaks in the correct mel band
  and sits ≥ 40 dB above distant bands, and that the peak band tracks frequency.
- **AAC "decoder" fabricated PCM** (`oximedia-audio/src/aac.rs`): it returned
  `Ok(AudioFrame)` whose samples came from reinterpreting raw payload
  bit-patterns as 4-bit coefficients through a stub IMDCT, and reported
  `CodecId::Mp3` as its codec. All decode paths now fail closed with
  `AudioError::UnsupportedFormat("AAC decoding is not implemented …")`, the
  codec identity reported is the honest `CodecId::Aac`, and the real ADTS header
  parser is retained for stream inspection. Documented in
  `docs/codec_status.md`.
- **Fragmented MP4 files demuxed to zero packets**
  (`oximedia-container/src/demux/mp4/mod.rs`): `moof` fell into the "skip
  unknown top-level box" arm, so a valid fMP4/CMAF file yielded no packets and
  no error. A new `demux/mp4/fragments.rs` parses
  `moof`/`mfhd`/`traf`/`tfhd`/`tfdt`/`trun` — including `base-data-offset` /
  `default-base-is-moof` addressing, `first-sample-flags`, every per-sample
  duration/size/flags/composition-offset variant, and `trex` defaults from
  `mvex` — and splices the resulting samples onto the track tables. Fragmented
  and progressive files written from the same packets now demux packet-for-packet
  identically (new `tests/mp4_fmp4_demux.rs`). `trun` sample counts are bounded
  before allocation.
- **The fMP4 muxer wrote an unparseable `trun`** (`oximedia-container/src/mux/cmaf.rs`):
  the per-sample loop emitted four 32-bit words (duration, size, flags,
  composition offset) but the flags word was `0x0B01`, which omits
  `sample-flags-present` (`0x000400`). Any spec-conformant reader consumed three
  words per sample and walked off the end of the run, mis-decoding sizes and
  offsets for the whole fragment. Now `0x0F01`. This affected `Mp4Muxer`'s
  fragmented mode and `CmafMuxer`.
- **`Mp4Demuxer` had no working seek**
  (`oximedia-container/src/demux/mp4/mod.rs`): it inherited the trait default
  ("Seeking not supported"), and `seek_sample_accurate` only *planned* a cursor
  without repositioning anything. `Demuxer::seek` now resolves the target sample
  via the sample table, rewinds to the preceding `stss` sync sample, realigns
  every other track through its own timescale, and `is_seekable()` reports
  the truth.
- **`tkhd` display matrix was skipped**
  (`oximedia-container/src/demux/mp4/boxes.rs`): portrait phone/camera captures —
  landscape pixels plus a 90/270-degree rotation — were indistinguishable from
  real landscape footage, so any 9:16 reframing cropped the wrong axis. The 3x3
  matrix (16.16 / 2.30 fixed-point), `layer`, `alternate_group`, `volume`,
  `version` and `flags` are all parsed now, and the previously dead
  `track_header::TransformMatrix` is wired in.
- **MP4 packet emission was O(n²)**
  (`oximedia-container/src/demux/mp4/mod.rs`): `next_track_index` and
  `read_packet` re-summed every preceding sample duration per packet, and
  `build_sample_table` re-summed intra-chunk byte offsets per sample. All three
  now use precomputed prefix sums / running cursors (`SampleInfo::dts`), making
  packet emission O(1) amortised.
- **MP4 track interleaving compared raw ticks across timescales**
  (`oximedia-container/src/demux/mp4/mod.rs`): 90 kHz video next to 48 kHz audio
  made the demuxer run one track far ahead of the other. DTS values are now
  compared as exact rationals via `u128` cross-multiplication.
- **Matroska `BlockDuration` never reached `Packet`**
  (`oximedia-container/src/demux/matroska/mod.rs`): `block_to_packet` parsed the
  element and then dropped it. It is now forwarded verbatim (TimestampScale
  units), with the track's `DefaultDuration` (nanoseconds, converted) as the
  fallback for blocks that carry none.
- **`SimpleMp4Muxer` wrote a non-interleaved `mdat`**
  (`oximedia-container/src/mux/mp4/simple.rs`): all of track 0 then all of
  track 1, one chunk per sample. Samples are now merged by decode timestamp
  (compared as exact rationals across differing timescales), consecutive
  same-track samples are grouped into chunks, and `stsc` run-length encodes the
  resulting per-chunk counts.
- **`SimpleMp4Muxer` truncated the `mdat` size to `u32`**
  (`oximedia-container/src/mux/mp4/simple.rs`): a saturating cast produced a file
  whose `mdat` claimed the wrong length and whose sample offsets all pointed past
  the end. Boxes now switch to the 64-bit `largesize` form, and chunk offsets to
  `co64`, when the payload exceeds 32 bits — resolved *before* the moov is
  measured so the two-pass offset fixup stays consistent.
- **`SimpleMp4Muxer` fragments had an unpatched `trun.data_offset`**
  (`oximedia-container/src/mux/mp4/simple.rs`): the `0` placeholder was never
  replaced, pointing every sample read into the middle of the `moof` header. It
  is now computed with a two-pass build, and the fragmented `moov` emits the
  empty sample tables ISO/IEC 14496-12 §8.8.1 requires instead of duplicating
  every sample.
- **`VideoFrame::allocate()` under-sized packed-4:2:2 and NV12/NV21 chroma
  planes** (`oximedia-codec/src/frame.rs`): the stride computation was
  `stride = width` for every plane regardless of format, which is correct for
  planar 8-bit formats but wrong for two families. Packed 4:2:2
  (`PixelFormat::Yuyv422`/`Uyvy422`, 2 bytes/pixel in a single plane)
  allocated half the bytes actually needed — a 64×48 `Yuyv422` frame got a
  3072-byte plane instead of the 6144 its own `frame_buffer_size()` reports.
  The interleaved-UV chroma plane of `Nv12`/`Nv21` (full luma *byte* width at
  half height, not half the chroma *pixel* width) was undersized by the same
  2x. `allocate()` now consults `PixelFormat::stride_for_width()` for exactly
  those two cases (packed formats on their one plane; semi-planar formats on
  plane index 1) and keeps the historical `stride = width` path for
  everything else, pinned unchanged by a new
  `test_frame_allocate_yuv420p_64x48_unchanged` regression. New
  `test_frame_allocate_yuyv422_packed_stride`,
  `test_frame_allocate_uyvy422_packed_stride` and
  `test_frame_allocate_semi_planar_chroma_full_byte_width` verify the
  corrected sizes against `PixelFormat::frame_buffer_size()`. **`P010`/`P016`
  (16-bit semi-planar) are knowingly left on the old, equally under-sized
  path** — no in-tree caller allocates them today, and widening the fix to
  cover them risked silently changing behaviour for code that had already
  compensated for the historical sizing; this is documented in `allocate()`'s
  own doc comment as a deliberate deferral, not an oversight. The bug
  surfaced from `oximedia-capture`'s mock backend, the first in-tree caller
  to allocate semi-planar (`Nv12`) frames through this path.
- **`convert_yuv422_row_to_rgb`'s length assertion undercounted odd-width
  rows** (`oximedia-simd/src/yuv_ops.rs`): 4:2:2 packed rows are stored in
  4-byte macropixels, and an odd-width row still occupies its final
  macropixel in full (the odd-tail branch reads a V byte at `si + 3`), so the
  correct minimum is `width.div_ceil(2) * 4` bytes, not `width * 2`. For an
  odd width the old bound under-counted by 2 bytes, so a tightly-sized buffer
  that satisfied the assertion could still let the tail path read past the
  end of `src` — a raw out-of-bounds panic instead of the intended "src too
  short" assertion message. Note this makes the assertion **stricter, not
  purely corrective**: an odd-width buffer sized to exactly `width * 2` bytes
  that happened to work before (because it was never actually read that far)
  is no longer accepted. The new sibling function `convert_uyvy422_row_to_rgb`
  (see Added) carries the corrected bound from the start and is cross-checked
  against `convert_yuv422_row_to_rgb` byte-for-byte on both even and odd
  widths (`test_convert_uyvy422_row_matches_yuyv422`,
  `test_convert_uyvy422_row_matches_yuyv422_odd_width`).
- **`pf_is_yuv422` didn't recognize the packed 4:2:2 formats**
  (`oximedia-core/src/codec_negotiation.rs`): the helper matched only
  `PixelFormat::Yuv422p`, so negotiating between a packed capture format
  (`Yuyv422`/`Uyvy422`) and planar `Yuv422p` scored as a cross-family
  conversion instead of a same-family one. Both packed variants are now
  included. The 10/12/16-bit planar 4:2:2 variants
  (`Yuv422p10le`/`Yuv422p12le`/`Yuv422p16le`) predate this helper and are
  **deliberately left unlisted**, so their existing negotiation cost is
  unchanged by this fix.
- **A family of 4:2:0 chroma converters floor-divided odd width/height
  instead of using the VP8/WebP `div_ceil(2)` convention**
  (`oximedia-codec/src/image.rs`, `oximedia-core/src/convert/pixel.rs`):
  `webp/vp8_decoder.rs`'s local `yuv420p_to_rgb24` already sized chroma as
  `div_ceil(width, 2) x div_ceil(height, 2)` (`DecodedImage::chroma_width`/
  `chroma_height`), but `image::convert_yuv420p_to_rgb` still read chroma
  with a floor stride -- a real, not just latent, bug: `ImageDecoder::
  decode_webp`'s lossy-WebP path (`decode_vp8_to_frame`) feeds it planes
  straight from `webp::vp8_decoder::decode_vp8_keyframe`, which already
  emits correctly ceil-sized chroma, so **any odd-width lossy WebP decoded
  through the public `ImageDecoder` API silently sheared chroma**.
  `image::convert_rgb_to_yuv420p` and `oximedia_core::convert::pixel`'s
  `yuv420p_to_rgb24`, `rgb24_to_yuv420p`, and `yuv420p_to_yuv444p` all
  floor-allocated their own chroma planes and then wrote or read the odd
  "extra" ceil column/row out of bounds -- **unconditional panics for any
  odd width or height**, not latent ones (e.g. a 3x3 RGB frame:
  `u_plane[(y/2)*uv_width + x/2]` indexes 1 into a 1-byte plane). All five
  functions now size and index chroma with `div_ceil(2)`; `image.rs`'s
  pair additionally validate plane lengths up front and return typed
  `CodecError::InvalidData` instead of panicking on a short or mismatched
  plane, while the `oximedia-core` trio keep their documented
  `assert_eq!`/panic precondition contract (an established public API)
  with ceil-aware size math and more descriptive panic messages.
  Even-width/height behavior is unchanged (`div_ceil(2)` is identical to
  floor division when the dividend is even) -- new odd-dimension (3x3) and
  even-dimension regression-pin tests were added for every fixed function
  in both files' `#[cfg(test)]` modules. `webp/vp8_decoder.rs`'s own doc
  comment (the converter that already had this right) was corrected to
  stop claiming the other two YUV->RGB helpers panic on odd dimensions,
  now that `image::convert_yuv420p_to_rgb` returns a typed error instead
  and `pixel::yuv420p_to_rgb24` keeps its (now ceil-aware) assert
  contract. `oximedia_core::convert::pixel::yuv420p_to_gray8` was reviewed
  and deliberately left alone (luma-only, no chroma). Investigating an
  odd-width `WebPLossyEncoder` -> `ImageDecoder` integration test
  (`tests/webp_vp8_lossy.rs`) surfaced a separate, pre-existing,
  dimension-independent `WebPLossyEncoder` / `vp8::decode_keyframe`
  bitstream incompatibility (reproduces at 4x4/8x8/16x16/32x32 too, not
  just odd dimensions) -- out of scope for this fix and documented, typed
  and non-panicking, in that test rather than worked around.
- **A follow-up pass over the same file found three more functions in the
  same bug class** (`oximedia-core/src/convert/pixel.rs`): the pass above
  covered five functions; these three round out every remaining
  4:2:0-chroma helper in the file (`yuv420p_to_gray8` stays luma-only and
  was already reviewed and left alone above). `yuv444p_to_yuv420p`'s 2x2
  downsampling loop read `(y + dy) * width + (x + dx)` for `dy, dx` in
  `0..2` unconditionally, overrunning the source planes for odd
  height/width (a 3x3 input: at `y=2`, the last valid row, `dy=1` computes
  source row 3, one past the end) and, separately, floor-allocated its own
  output chroma as `(width/2) * (height/2)` -- a 3x3 output needs 2x2 = 4
  bytes, not 1x1 = 1. It is fixed the same way `rgb24_to_yuv420p` already
  handles its own partial 2x2 blocks: `div_ceil(2)` output allocation, and
  a block at the right/bottom edge now averages only the source pixels
  that actually exist (a dynamic divisor, not a hardcoded `/ 4`);
  even-dimension output is bit-identical, since every block there always
  has all four neighbors in bounds, so the sum and divisor are unchanged.
  `yuv420_to_rgb` (the standalone BT.601 function, distinct from
  `yuv420p_to_rgb24`) had the identical floor-divided chroma stride bug as
  the first pass's `yuv420p_to_rgb24` -- `(row/2) * (width/2) + col/2`
  instead of `(row/2) * chroma_width + col/2` -- and is corrected the same
  way, including its assert-style panic contract and doc example. Like the
  first pass's trio, this was an **unconditional panic for any odd width
  or height**, not a latent one: a plane sized to the old floor contract
  passed the (unlabeled) length assert and then the read went out of
  bounds (a 3x3 frame: at `row=0, col=2` the index is `0*1 + 1 = 1` into a
  1-byte plane), while a correctly ceil-sized plane was rejected by the
  assert outright. No previously-working call now fails; a mis-sized plane
  now fails at a descriptive assert instead of an out-of-bounds index
  panic, with a message naming the expected `div_ceil(width, 2) *
  div_ceil(height, 2)` size. `gray8_to_yuv420p` allocated its constant-128
  U/V planes as `(width/2) * (height/2)`; there is no indexed write into
  them, so it never panicked, but the returned planes were inconsistent
  with the `div_ceil(2)` convention every sibling function in this file
  now follows -- e.g. a 3x3 gray image produced a 1-byte chroma plane that
  the now ceil-aware `yuv420p_to_rgb24` rejects (it requires 2x2 = 4
  bytes); its allocation is now `div_ceil(2)` too. None of the three had
  any caller elsewhere in the workspace (`yuv444p_to_yuv420p` and
  `gray8_to_yuv420p` are re-exported from `convert::mod` but were unused
  outside this file and its own tests; `yuv420_to_rgb` is not re-exported
  at all), so this is not a breaking change for any in-tree consumer. New
  odd-dimension (3x3) and even-dimension (6x4) regression-pin tests were
  added for all three in the same `#[cfg(test)]` module, following the
  first pass's naming and structure; the odd-dimension tests value-assert
  every output chroma byte against hand-identified source indices (not
  just plane lengths), and the even-dimension `yuv444p_to_yuv420p` pin
  test uses non-uniform per-pixel data rather than a constant fill, since
  a constant fill cannot distinguish a correct 4-neighbor average from a
  regression that sums the wrong pixels or divides by the wrong count.
- **The FLAC encoder and decoder in `oximedia-codec` could not interoperate
  with any real FLAC, in either direction** — they were a self-consistent
  toy pair, and the "Verified on internal round-trip fixtures" label was
  measuring only that self-consistency. Fed a stock `ffmpeg`-produced FLAC
  file the decoder **panicked** (`attempt to shift left with overflow`).
  Both sides were rebuilt against RFC 9639. Essentially every field had
  been mis-laid-out: the subframe header was written as a whole byte
  instead of 1+6+1 bits, with LPC coded as the reserved `01xxxxxx` type;
  warmup and verbatim samples were byte-aligned `i16` instead of bit-packed
  at the declared `bps`; LPC precision and shift were bytes instead of 4
  and 5 bits; the residual header was 2 bytes instead of 2+4 bits; the Rice
  unary convention was **inverted** (ones-then-zero, where the spec is
  zeros-then-one); block size was stored without its `-1`; the sample rate
  was hard-coded to `0x09`; the bit-depth nibble was computed as
  `bps/4 - 1`, which is the *reserved* code 3 for 16-bit, and sat one bit
  off position. Constant, fixed and verbatim subframes, stereo
  decorrelation, escaped partitions and wasted bits did not exist at all.
  Two further defects were specific and load-bearing: `decode_lpc_subframe`
  took `slice_from_current()` and **never advanced the reader**, so channel
  1 of every stereo frame parsed from garbage offsets (this, not the
  zero-padding the in-tree comment blamed, was the primary cause of the
  8243-vs-42104 length disagreement); and the LPC path was structurally
  unable to be lossless, because the encoder computed residuals from
  **float** coefficients while the decoder dequantised and rounded. Both
  sides now use identical `i64` accumulation with an arithmetic right shift
  over the quantised coefficients actually written. New modules
  `flac/{bitio,frame,subframe,residual}.rs`; `flac/rice.rs`'s zigzag is now
  `i32::MIN`-safe. The test that had been written to pass around the bug
  (`consumed <= len`) is gone — `flac/encoder.rs` now asserts
  `consumed == frame.data.len()`. Verified in both directions against
  `libFLAC` and `ffmpeg`. `crates/oximedia-audio/src/flac/` is a separate
  implementation and was not touched.
- **Every MPEG-TS PMT this workspace has ever emitted was non-conformant**
  (`oximedia-container`, `mux/mpegts/mod.rs:370`). `write_pmt` captured
  `section_length_pos` *after* pushing the table id, so it already pointed
  at the length field — but the fix-up wrote to `+1`/`+2`. The result:
  `section_length` read back as `0xB0` (176 instead of 21), and the low
  byte clobbered the high byte of `program_number`. `write_header`/
  `write_pmt` had zero test coverage — none of the module's five tests ever
  wrote a header. Found while wiring `oximedia-server`'s HLS segmenter to
  this muxer, which is exactly the failure this codebase's honesty policy
  targets: output that looks like a `.ts` file and that no demuxer can
  parse. Regression test:
  `oximedia-server`'s `hls::segment::tests::segment_reparses_with_correct_stream_type`.
- **The CMAF muxer nested each sample entry inside a box named after
  itself** instead of the codec's configuration box — an `av01` box inside
  the `av01` sample entry, rather than `av1C` (`oximedia-container`,
  `mux/cmaf.rs`). New `config_box_fourcc()` maps `av01→av1C`,
  `vp09`/`vp08`→`vpcC`, `Opus→dOps`, `fLaC→dfLa`, with a documented,
  unit-tested `conf` fallback mirroring `mux/mp4/writer/boxes.rs`. Pinned
  by a real box-tree walker that descends
  `moov > trak > mdia > minf > stbl > stsd`, skips the exact SampleEntry
  preamble (78 bytes video / 28 audio) and asserts the config box starts
  immediately after it **and** consumes every remaining byte.
- **The AVIF `iloc` parser accepted only box version 1 — which is what this
  crate's own encoder writes — and so would have rejected essentially every
  real-world AVIF file** before AV1 decode was ever reached
  (`oximedia-codec`, new `avif/container.rs`). `ffmpeg` and libaom write
  version 0, which has no `construction_method` field; confirmed by
  hex-dumping real ffmpeg output. Both versions now parse, through a
  bounds-checked reader that returns `Err` rather than panicking on
  truncated or fuzzed input.
- **A reachable panic-as-denial-of-service in the Matroska/WebM demuxer**
  (`oximedia-container`, `demux/matroska/mod.rs`). The `TIMESTAMP`,
  `SIMPLE_BLOCK` and `BLOCK_GROUP` branches of `read_next_block` sliced
  `self.buffer[header_size..total_size]` with no bounds check, and
  `ensure_buffer` does not error on EOF — it fills whatever the source has
  and returns `Ok`. A Cluster element declaring a size larger than the
  remaining file therefore panicked
  (`range end index 5009 out of range for slice of length 13`), on
  untrusted input. All three branches now return
  `OxiError::UnexpectedEof`, matching the `read_bytes` idiom already used
  elsewhere in the file. The regression test fails before the fix and
  passes after.
- **Most of `oximedia-denoise` returned its input unchanged.** The
  bilateral filter wrote its output into `&mut plane.data.clone()` — a
  temporary discarded at the end of the statement. The same defect was
  present at **12 sites across 8 files**: `spatial/bilateral.rs` (both
  `bilateral_filter` and `fast_bilateral_filter`), `temporal/kalman.rs`,
  `temporal/median.rs` (×2), `temporal/average.rs` (×2),
  `spatial/wiener.rs` (×2), `spatial/wavelet.rs`,
  `hybrid/spatiotemporal.rs` and `hybrid/adaptive.rs`. Because
  `Denoiser::process` routes `Fast`, `Balanced`, `GrainAware`, `Bm3d` and
  most `Custom` configurations through those functions, this repaired
  essentially the whole denoiser rather than one filter; only `Quality` and
  `NlMeans` were already correct. Every site now writes through
  `plane.data.as_mut()`. New tests assert output ≠ input and a measured
  reduction in local variance; their validity was checked by reintroducing
  the original bug and confirming they fail.
- **VP9 `parse_frame_size_with_refs` resolved every size-from-reference
  frame as 0×0**, and the bug was live, not theoretical — the committed
  `seq76x42.frame1.bin` fixture sets `found_ref` and was being parsed as a
  0×0 frame. A pure parse-then-fix-up was impossible here: `parse_tile_info`
  derives *how many bits it reads* from `header.width`, so a placeholder
  desynchronises the bitstream from tile_info onward for any frame wider
  than about 449 px. Fixed by threading the reference dimensions into the
  parse (`UncompressedHeader::parse_with_ref_sizes`, with `parse()` kept as
  a wrapper). The same pass fixed loop-filter-delta and segmentation
  feature data being parsed and then discarded rather than inherited across
  frames.
- **`Superframe::parse` could return silently truncated frames**
  (`oximedia-codec`, `vp9/superframe.rs`). The `Ok(Some(index))` branch
  never checked that the claimed frame sizes sum to the payload length
  minus the index, so a payload with a coincidental marker byte could pass
  disambiguation and come back `Ok`. It now validates the sum and returns
  an honest `Err`; `Vp9Decoder::send_packet` inherits the protection.
- **The VP9 loop filter never skipped anything, and collapsed its level
  table.** `lf.rs` hard-coded `skip_this = false` where libvpx uses
  `mi.skip && is_inter_block(mi)`, and stored levels as `lvl_intra[seg]`
  where the reference keeps `lvl[seg][ref_frame][mode_bucket]`. Both were
  invisible on key frames (where `is_inter` is always false and only the
  intra reference exists) and would have been wrong on every inter frame.
- **`oximedia-transcode` rejected the most ordinary output path there is.**
  `validation.rs:162` tested `Path::new("out.flac").parent()`, which is
  `Some("")`, and `Path::new("").exists()` is always false — so
  `oximedia transcode -i in.wav -o out.flac` failed with "Parent directory
  does not exist" while `./out.flac` and absolute paths worked. An empty
  parent now resolves to `.` before the existence and writability probes.
  Note the consequence: a genuinely missing directory is now caught one
  layer downstream, by `create_dir_all`, with a different message — still
  an honest failure with no output written.
- **`oximedia-qc`'s probe fallback would fabricate an AV1 verdict for
  content it had never read.** Neutralised for the `oximedia-farm` caller
  by checking the file's real magic bytes against the exact set
  `probe_file` genuinely parses (ISO-BMFF/Matroska/AVI/WAV/FLAC/Ogg/MXF)
  and refusing anything else by name, rather than letting the fallback
  synthesize a stream and report "passed".
- `oximedia-renderfarm`: two latent task-state bugs — `execute_post_render`
  propagated a new `Err` with a bare `?`, leaving the pipeline task stuck
  at `Running` instead of `Failed` (the same latent defect existed for
  `calculate_quality_metrics`, previously unreachable because
  `assemble_output` always failed first); and `execute_pre_render` called
  `verify_assets` *before* `resolve_dependencies`, which made the new
  `add_asset_source` API silently inert through the real public entry
  point.
- `oximedia-container`: `demux/mp4/mod.rs:559` re-called
  `build_stream_info(...).unwrap_err()` to recover an error it already
  held — the crate's only production `unwrap`, now bound by pattern.
- `oximedia-codec`: ProRes `parse_slice_header` now rejects `quant_scale`
  outside `1..=224` per RDD 36 §6.5.3 (new `FrameError::BadQuantScale`),
  and an adjacent truncation guard in the same function was fixed.
- **`oximedia-capture`'s Linux backend could advertise a capture resolution
  its own driver doesn't support, which then failed to configure once
  streaming actually started.** `stepwise_size_corners`
  (`platform/linux/mod.rs`) samples a V4L2 `STEPWISE` frame-size range at its
  minimum, a step-snapped midpoint, and the driver-reported maximum — but the
  kernel defines a stepwise range's valid sizes as `min`, `min + step`,
  `min + 2·step`, … up to *and including* `max` only when the span is a whole
  multiple of the step, so e.g. 32 to 1080 in steps of 16 stops at 1072 and
  never reaches 1080. The old code offered the unchecked maximum, putting a
  size on the device's advertised list — what `enumerate()`/`negotiate()`
  report as supported — that `VIDIOC_S_FMT` would actually echo back
  differently; `configure_format`'s echo check then turns that mismatch into
  `CaptureError::FormatRejected`, on the capture thread rather than from
  `open()` itself. The maximum corner is now snapped onto the grid before
  being offered — a no-op for the conforming ranges real drivers report —
  with the midpoint re-derived from the corrected maximum.
- `cargo deny check licenses` passes again (30 rejected crates → 0).
  `deny.toml` gained `Apache-2.0 WITH LLVM-exception` (proven **not**
  satisfied by bare `Apache-2.0` — `target-lexicon` was rejected despite
  the latter already being allowed), `Unicode-3.0`, `Zlib`, `BSL-1.0` and
  `CDLA-Permissive-2.0`; the stale unmatched `Unicode-DFS-2016` and
  `OFL-1.1` entries were dropped; `colored`, `option-ext` and `serialport`
  are scoped MPL-2.0 exceptions; and `fuchsia-cprng` (no SPDX `license`
  field) is clarified. `async-graphql` moved to `default-features = false`
  with its default set minus `email-validator`, which removes
  `fast_chemail`/`ascii_utils` from the graph entirely — verified
  behaviour-neutral, since `oximedia-mam`'s GraphQL surface uses no email
  validator.

### Added
- **`oximedia-capture`** — a new workspace crate: Pure-Rust live camera/device
  capture, the `libavdevice` equivalent. Enumerate devices, negotiate a format
  against what one advertises, open a session and receive frames on a bounded
  queue with `DropOldest`/`DropNewest`/`Block` policies. Three platform
  backends behind the same `enumerate()`/`open()` entry points: AVFoundation
  on macOS (via `objc2`), Video4Linux2 on Linux (via `rustix` syscalls), and
  Media Foundation on Windows (via `windows-rs` COM calls) — no C/C++/Fortran
  compiled on any of them. Verification is deliberately not uniform across
  platforms, and the crate says so rather than claiming otherwise: macOS was
  hardware-verified during package A5 (`enumerate()` exercised by a
  non-`#[ignore]`d unit test that calls real AVFoundation on every run;
  streaming gated behind the TCC permission prompt); Linux's V4L2 struct
  transcription is pinned by a compile-time golden-layout table, const-evaluated against
  kernel 6.19 UAPI headers on both `aarch64-unknown-linux-gnu` and
  `i686-unknown-linux-gnu`, but has not yet opened a real `/dev/video*` node;
  Windows' Media Foundation logic (`mf_logic.rs`) is host-tested against
  synthetically constructed inputs but has not yet driven a real
  `IMFSourceReader`. A target with none of the three reports
  `CaptureError::UnsupportedPlatform` rather than an empty device list, so
  "not implemented" and "no camera attached" are never the same answer.
  Delivers raw pixel formats or an untouched MJPEG bitstream only — never a
  decode of an encumbered codec (see the root README's Red List section). 246
  host unit tests (behind the `mock` feature and `cfg(test)`); opt-in
  `#[ignore]`d live-device tests behind `OXIMEDIA_CAPTURE_DEVICE` for all
  three platforms. Wired into the `oximedia` facade behind a new `capture`
  feature. See `crates/oximedia-capture/README.md` for the full platform
  matrix, purity statement and usage example.
- **`oximedia_core::PixelFormat::{Yuyv422, Uyvy422}`** — packed YUV 4:2:2
  pixel formats (`YUYV`/`YUY2` and `UYVY`/`2vuy` FourCCs), added to back
  `oximedia-capture`'s two most common UVC/AVFoundation raw formats. Single
  interleaved plane, 2 bytes/pixel, `(2, 1)` chroma subsampling; participate
  in `bits_per_pixel`, `plane_count`, `is_planar`/`is_semi_planar`,
  `bits_per_component`, `is_yuv`, `chroma_subsampling`, `frame_buffer_size`,
  `stride_for_width`, and `FromStr`/`Display`
  (`"yuyv422"`/`"yuyv"`/`"yuy2"` and `"uyvy422"`/`"uyvy"`/`"2vuy"`) exactly
  like every other `PixelFormat` variant. New unit tests pin every property
  plus the parse round-trip; `pixel_format_exhaustive.rs`'s exhaustive table
  gained both variants, and its "every YUV format has ≥2 planes" invariant
  was narrowed to planar/semi-planar formats only — packed formats interleave
  luma and chroma into a single plane by definition (see Fixed for the
  matching `pf_is_yuv422` negotiation update).
- **`oximedia_simd::yuv_ops::convert_uyvy422_row_to_rgb`** — UYVY-order
  sibling of the existing `convert_yuv422_row_to_rgb` (BT.601, one row at a
  time). Six new tests: agreement with `convert_yuv422_row_to_rgb` on the same
  underlying samples at even and odd widths, near-black and mid-gray sanity
  checks, and a zero-width no-op.
- **`oximedia_core::CodecId::Aac`** — an *identification-only* green-list
  variant (the Fraunhofer/Via Licensing AAC-LC patent pool expired in April
  2023). It carries the same trimmings as its siblings: `media_type() ==
  MediaType::Audio`, `is_audio()`, `name()`/`Display` → `"aac"`,
  `canonical_name()`, `is_lossless() == false`, and `CodecMatrix` container
  compatibility (`mp4`/`isobmff`/`m4a`/`mov`/`mkv`/`ts`/`aac`/`adts`).
  Deliberate exception: **`FromStr` refuses it** — `"aac"`, `"mp4a"`,
  `"aac-lc"` and `"he-aac"` still return `Err`, so no name-driven code path
  (CLI flag, container hint, manifest) can select AAC; the variant is reachable
  only by naming it in Rust, which is what a stream inspector does when
  reporting what it found. It is therefore the one variant whose `name()` does
  not round-trip through `FromStr`, which `types::codec_id`'s docs,
  `test_aac_is_never_parsed_from_a_name` and `tests/patent_detection.rs` all
  pin. The MP4/Matroska demuxer patent whitelists are untouched — they still
  reject `mp4a` tracks with `PatentViolation`.
- **`oximedia_audio::meters::batch`** — `BatchMeterProcessor`,
  `BatchMeterConfig` and `BatchMeterReading`, mixing-console style multi-lane
  metering (peak / RMS / true-peak / peak-hold / overload for dozens of channels
  in one allocation-free pass, interleaved f32/f64 or planar per-channel). The
  file existed but was never declared in `meters/mod.rs`, so none of it — nor
  its twelve tests — was compiled. Now wired in and re-exported from
  `oximedia_audio::meters`.
- **TikTok delivery target**: `oximedia_metering::Standard::TikTok` and
  `oximedia_normalize::TargetPreset::TikTok` — −14.0 LUFS integrated, −1.0 dBTP
  true-peak ceiling, ±1.0 LU tolerance. Selectable from the CLI as
  `--standard tiktok`.
- `oximedia_metering::ebu_r128_impl::EbuR128Meter::channel_true_peaks_dbtp()` —
  per-channel true peak readings in channel order.
- `oximedia_audio::loudness::sample_bytes` — PCM byte ⇄ normalised `f64`
  conversion covering every `SampleFormat` (`U8`, `S16`/`S16p`, `S24`/`S24p`,
  `S32`/`S32p`, `F32`/`F32p`, `F64`/`F64p`), returning `None` rather than
  fabricating audio for formats it does not know.
- `oximedia_audio::loudness::NormalizationParams::frames_modified` — how many
  frames `normalize()` actually rewrote, so a caller can detect a no-op.
- `oximedia_mir::audio_features::compute_log_mel_spectrogram_with_fft` — log-mel
  with an explicit FFT window size (the 4-argument entry point defaults to
  `4 × hop_length`, i.e. 75 % overlap).
- **MP4 seek API**: `Mp4Demuxer::seek_position(SeekTarget)` and
  `Mp4Demuxer::seek_to_stream_pts(stream, pts)` return an `Mp4SeekPosition`
  (`stream_index`, `sample_index`, `dts`, `pts`, `byte_offset`, `is_sync`)
  describing exactly where the demuxer landed. `SeekFlags::ANY` opts out of the
  keyframe rewind; `SeekFlags::BYTE` resolves through the chunk offsets.
- **Rotation metadata on streams**: `StreamInfo::rotation` (0/90/180/270 degrees,
  snapped from the container's display matrix), `StreamInfo::display_matrix`
  (`TransformMatrix`), plus `StreamInfo::is_quarter_turned()` and
  `StreamInfo::display_dimensions()`. Populated by `Mp4Demuxer` from `tkhd`.
- **`esds` builders** (`oximedia_container::mux::mp4::esds`, also re-exported
  from `mux::mp4` and `mux::mp4::simple`): `build_esds_payload`,
  `build_esds_box` and `EsdsParams` emit a complete AAC `ES_Descriptor` →
  `DecoderConfigDescriptor` → `DecoderSpecificInfo` + `SLConfigDescriptor` tree
  with correct expandable descriptor-length encoding, and
  `build_audio_specific_config` builds the `AudioSpecificConfig` they wrap
  (including the `0x0F` escape for non-table sample rates). Nothing in the
  workspace could construct an `esds` before, so `SimpleMp4Muxer` could only emit
  undecodable `mp4a` tracks.
- MP4 fragment parsing types: `oximedia_container::demux::mp4::{parse_moof,
  parse_mvex, FragmentSample, MoofInfo, TrexBox}`, plus
  `Mp4Demuxer::is_fragmented()`, `Mp4Demuxer::fragment_count()`,
  `Mp4Demuxer::tracks()` and `TrackState::{dts_at, pts_at}`.
- `TkhdBox` now exposes `version`, `flags`, `layer`, `alternate_group`,
  `volume`, the raw `matrix`, `transform_matrix()`, `rotation_degrees()`,
  `has_unity_matrix()` and `UNITY_MATRIX`; `MoovBox` exposes `trex` and
  `is_fragmented()`.
- **Lossy WebP (VP8) decode**: `crates/oximedia-codec/src/webp/vp8_decoder.rs`
  (new, `vp8` feature) wires a lossy WebP's `VP8 ` RIFF chunk to the
  already-shipped, bit-exact VP8 key-frame decoder
  (`vp8::decode_keyframe`) — a lossy WebP image *is* a VP8 key frame per
  RFC 6386, so this is container plumbing, not a new decoder. Two entry
  points: `decode_vp8_keyframe` (raw `VP8 ` chunk payload -> YUV 4:2:0
  `VideoFrame`, the plane hand-off mirrors `vp8::Vp8Decoder`) and
  `decode_vp8` (a whole RIFF/WebP file -> `VideoFrame`, reusing the
  existing `WebPContainer::parse` rather than a second RIFF parser).
  Extended-format files (`VP8X`+`ALPH`+`VP8 `) get their uncompressed
  `ALPH` alpha merged into an RGBA32 output; VP8L-compressed alpha keeps
  `webp/alpha.rs`'s pre-existing honest `CodecError::UnsupportedFeature`.
  `ImageDecoder::decode_webp` (`crates/oximedia-codec/src/image.rs`) now
  delegates to `decode_vp8_keyframe` instead of its own inline
  `Vp8Decoder` call, so there is one lossy-decode implementation. Verified
  bit-exact against the `dwebp -yuv` reference on all 3 real-bitstream
  vectors in `tests/vp8_fixtures/` (grad32 32x32, tex48x40 48x40, a
  4-partition libvpx 48x48 key frame), each wrapped in a hand-built RIFF
  container by the new `tests/webp_vp8_lossy.rs`, which also pins
  container-vs-raw-payload plane equivalence and typed errors (never a
  panic or a fabricated frame) for a corrupt RIFF magic, an
  impossible chunk size, and a `VP8L` chunk fed to the lossy entry point.
  `TODO.md`'s "WebP VP8 lossy decode" row previously read "Missing /
  large / follows VP8"; corrected to reflect that the VP8 decoder it was
  waiting on already shipped in 0.2.0, making this the small, independent
  wiring task it actually was.
- **VP8 inter-frame decode** (`crates/oximedia-codec/src/vp8/dec/`, renamed
  from `vp8/keyframe/` now that it holds both frame types) — the second half
  of the RFC 6386 pipeline, built on the existing bit-exact key-frame path:
  per-macroblock prediction-mode records including the §16.3 near-MV survey
  (NEAREST/NEAR/ZERO/NEWMV, `SPLITMV` sub-partitioning), §17 motion-vector
  entropy decode, §18 sub-pixel motion compensation (six-tap and bilinear,
  with the version-3 full-pel-chroma rule), and §9.7-§9.8 last/golden/altref
  reference-frame management with the reference decoder's sequential
  (non-swap) update order and hidden (`show_frame == 0`) frame handling. The
  §9.9 `refresh_entropy_probs` snapshot/restore landed alongside it; per
  `docs/codec_status.md`'s 0.2.1 VP8 audit, this also closes a latent
  key-frame gap, since that bit is coded for both frame types.
  `vp8::Vp8Decoder::send_packet` now routes every frame — key or
  inter — through the new `dec::Vp8SequenceDecoder` instead of returning
  `CodecError::UnsupportedFeature` for inter frames (see Changed for the
  resulting decoder-behaviour changes). Verified **bit-exact against
  libvpx**, every shown frame, all three planes, on five libvpx-encoded
  multi-frame streams (39 coded / 38 shown frames total): `p5basic`
  (LAST-only P frames, inherited loop-filter deltas), `refswap` (a genuine
  hidden altref frame, altref sign bias, and a `refresh_golden_frame` +
  `copy_buffer_to_alternate = 2` frame in the same packet), `refswap_er`
  (`refresh_entropy_probs = 0` on every frame, i.e. error-resilient),
  `splitmv` (100x64 — a non-macroblock-aligned width — with a strong
  diagonal pan, exercising `SPLITMV`), `segdelta` (segmentation with
  per-segment quantiser deltas). Suite: `vp8/dec/inter_fixture_tests.rs`;
  fixture provenance and the exact `ffmpeg`/`libvpx` commands:
  `vp8/dec/testdata/README.md`. Key frames were re-verified unchanged (still
  bit-exact vs libwebp on their three still-image vectors) after being
  rerouted through the same sequence decoder.

  The loop filter's `hev_threshold` (RFC 6386 §15.2) was a real bug the
  conformance run exists to catch: the key-frame-only seed computed it with
  a fixed formula that never took frame type into account — there was no
  such thing as an inter frame yet when it was written — but RFC 6386 (and
  dixie.c's `calculate_filter_parameters`) gives inter frames one additional
  `hev_threshold` increment at filter level >= 20 that key frames do not
  get. `vp8/dec/loopfilter.rs::compute_filter_params` now takes the frame
  type and applies it; `test_hev_threshold_frame_type_table` pins the full
  key-vs-inter table the RFC and dixie.c specify.
- **VP9 inter-frame and intra-only decode — the decoder is now complete for
  8-bit 4:2:0 apart from two named, fixture-gated deferrals.** Both honest
  `Err` arms of `vp9/decoder.rs` are gone. `vp9/kf/` was renamed `vp9/dec/`
  and grew: persistent cross-frame state (four frame probability contexts,
  an eight-slot MI-aligned reference DPB carrying per-MI motion-vector
  grids, loop-filter-delta and segmentation inheritance) in `state.rs` and
  `refs.rs`; the `vpx_convolve8` eight-tap motion-compensation family in
  `mc.rs`; `find_mv_refs` with sign-bias flipping, temporal candidates and
  sub-8×8 handling in `mvref.rs`; the prediction-context functions in
  `predctx.rs`; inter mode, reference-frame and motion-vector entropy
  decode (including the `use_mv_hp` gate) in `modeinfo.rs`; whole-superblock
  inter prediction with compound `_avg` in `interpred.rs`; backward
  probability adaptation in `counts.rs`/`adapt.rs`; and the inter sections
  of the compressed header in `hdr.rs`. Every function is a port of libvpx
  **v1.15.2** carrying its own `file:line` citation, and the source file
  each was transcribed from was `diff`-verified byte-identical to a fresh
  fetch of the tagged upstream before use.

  **Verification: 10 of 10 non-deferred conformance streams decode
  bit-exactly through the public `VideoDecoder::send_packet` /
  `receive_frame` API — 0 `#[ignore]`, 0 PSNR thresholds, 0 skips.**
  `p9still`, `p9basic`, `p9er`, `p9hp` (76×42, 8 frames each — the last
  pinning ⅛-pel motion vectors), `p9tc` (512×64, two tile columns),
  `p9alt` (128×128, a hidden ALTREF **sub-frame** inside the packet-1
  superframe, and the only `sign_bias` flips in the native set),
  `compound` (96×64, 13 coded / 12 shown — a second, independent hidden-ARF
  stream whose hidden sub-frame sits on a *different* `frame_context_idx`
  than the shown chain, so it pins per-index context save/load too),
  `switch` (100×68, more than one interpolation filter, and the only
  fixture with a partial last superblock row *and* column), `p9sef`
  (`show_existing_frame` redisplay) and `p9io` (352×288 intra-only, tested
  from `vp9/decoder.rs`). The gate carries its own anti-vacuity test — a planted
  mismatch must be reported — and a coverage assertion that harvests the
  decoder's own symbol counters and puts a floor on each, so that a stream
  which merely *permits* a feature in its headers cannot be mistaken for one
  that exercises it: compound blocks and the `comp_ref` reads that select
  their references, all four inter modes, both intra and inter blocks inside
  inter frames, real per-block filter choice on SWITCHABLE frames rather
  than always landing on EIGHTTAP, non-zero motion vectors in each component
  separately and together, and the ⅛-pel high-precision bit actually being
  read. Fixture provenance and exact encoder commands:
  `vp9/dec/testdata/RECIPE.md`.

  Two features are **deliberately deferred and refuse at a pinned packet
  index**, so that a break earlier in the stream cannot masquerade as the
  expected deferral: inter-frame segmentation (fixture `p9seg` — its key
  frame decodes bit-exactly, then packet 1 refuses naming
  `read_inter_segment_id`) and reference scaling (fixture `scaled` —
  packets 0–5 of the 96×64 prefix decode bit-exactly, then packet 6
  refuses naming `96x64` vs `64x48`). Both fixtures are committed, so
  closing either deferral has its gate waiting.

  Three findings from the port that are easy to get wrong and are now
  documented in the code: the decoder does **not** apply the MC-stage
  motion-vector clamp on the unscaled path (libvpx calls
  `clamp_mv_to_umv_border_sb` only under `if (is_scaled)`); the VP9 decoder
  never extends reference-frame borders, so a reference's only valid pixels
  are inside its *display* rectangle and everything else is edge-replicated
  on the fly (which means the DPB must **not** grow padding); and
  `uncompressed.rs` stores the raw 2-bit `interp_filter` literal without
  applying `LITERAL_TO_FILTER`, so the mapping must happen exactly once at
  the recon call site — wiring the raw value silently swaps EIGHTTAP and
  EIGHTTAP_SMOOTH on every block of every non-switchable frame.
- **AVIF decodes to real pixels** (`oximedia-codec`, new `avif/decode.rs`
  and `avif/container.rs`). `AvifDecoder::decode()` extracts the AV1 OBU
  payload and feeds it through `crate::av1::Av1Decoder`, so its scope is
  exactly that decoder's: 8-bit 4:2:0. A real `ffmpeg` + libaom fixture
  decodes byte-identically to ffmpeg's own dav1d decode, plane for plane.
  Alpha and 10/12-bit items refuse by name — real-world AVIF encodes alpha
  as monochrome AV1, which the AV1 keyframe decoder does not implement, and
  colour and alpha are treated as inseparable so an undecodable alpha
  channel is never silently returned as opaque. Committed fixtures pin all
  three outcomes. The `#[cfg(not(feature = "av1"))]` build keeps a single
  honest `UnsupportedFeature` rather than a second, weaker path.
- **`oximedia-server` can emit playable HLS and DASH segments**, over a
  patent-free Enhanced-RTMP ingest path. New shared
  `ingest_depacketizer.rs` handles E-RTMP AV1/VP9/Opus/FLAC via
  `EnhancedVideoTag`/`EnhancedAudioTag`; legacy FLV AVC/AAC is a Red-List
  refusal by design, not an omission. `hls/segment.rs` muxes real MPEG-TS
  through `MpegTsMuxer`, `dash/segment.rs` real fragmented MP4 through
  `CmafMuxer`, and both packagers now retry initialisation until the
  depacketizer reports `ready()` instead of latching a failed first
  attempt. Config-record conversions are real, not passthrough:
  `opus_head_to_dops` (drop magic, version 1→0, LE→BE on
  PreSkip/InputSampleRate/OutputGain) and `flac_config_to_dfla` (prepends
  the FullBox version+flags the CMAF muxer does not write, accepts three
  input shapes, preserves all metadata blocks); `vpcC` is treated as the
  FullBox v1 it is, with bare-vs-prefixed resolved from
  `codecInitializationDataSize` rather than guessed. Integration tests
  re-parse the emitted segments with `MpegTsDemuxer` / `FragmentedMp4Ingest`
  rather than checking they are non-empty, and the no-fabrication negative
  tests were re-pointed at H.264 input, where they still assert nothing is
  written. `transcode/engine.rs` stays an honest per-codec `Err` — no
  ingest codec is fully decodable yet.
- **RTMP sequence headers are cached and replayed on subscribe**, so a
  client joining mid-stream receives the configuration records it needs.
  `SeqHeaderCache` in `oximedia-net`'s `registry.rs`, captured at
  `connection.rs`, replayed at both sites that need it —
  `connection.rs::handle_play` (after subscribe and `onStatus`) and
  `oximedia-server`'s `rtmp/server.rs::run_bridge`. No `MediaPacket` field
  and no `register_stream` return type changed.
- **`oximedia-wasm` demuxes real containers in memory** — Matroska/WebM,
  Ogg, FLAC, WAV and MP4, via `oximedia-container` over a `MemorySource`.
  The 14 "not yet available in the WASM build" markers are gone. New
  `block_on.rs` drives the async demuxer with a single-poll, no-op-`Waker`
  driver that returns an honest `Err` on `Pending` — it never spins;
  `container_bridge.rs` converts real container types into the crate's
  pre-existing local mirror types. `streaming_demuxer.rs` rebuilds a fresh
  demuxer over the whole accumulated buffer per read rather than fighting
  per-format latched-EOF state, with a peek-ahead guard against reporting a
  packet that may still grow. The truncated-fixture tests now assert a real
  parse error rather than "unavailable", and MP4's H.264 refusal surfaces
  as a named per-codec JS error.
- **Nine `oximedia-cli` commands do real frame work**, through a new shared
  `frame_harness/` module (`mod`/`adapt`/`ops`/`font`/`scale`/`text`) lifted
  out of `restore_cmd.rs`. `process_clip` and `process_frames` (the latter
  genuinely streaming) give a Y4M-in/Y4M-out contract; `ClipStats.bytes_changed`
  is an anti-fabrication counter — a command that changed nothing refuses to
  claim success and removes its output. Consumers: `scaling` (scale, compare,
  batch — real `Resampler` per plane, real PSNR/SSIM), `denoise` (real
  `Denoiser::process`, with a test asserting measured variance reduction),
  `stabilize` (honouring `--mode`/`--quality`/`--smoothing`/`--zoom`, the
  last driving a real `ZoomOptimizer`), `multicam color-match` (real
  per-angle BT.709 statistics feeding `ColorMatcher`, replacing
  `ColorStats::new`'s fabricated 0.5/6500 K defaults), `timecode burn`,
  `subtitle burn` and `captions burn`. A new `--font <PATH>` flag is
  required for every text-burning command: no font ships in the tree, and
  the error names the flag rather than pretending. Non-Y4M input keeps a
  shared "convert first" message naming `oximedia transcode`.
- **`oximedia-metadata`: all six embed arms are real container splices**
  (`src/embed.rs` → `src/embed/`, public API unchanged). Matroska `Tags`
  via a self-contained EBML walker that splices before the first Cluster
  and re-emits `SeekHead`/`Cues`/`Cluster` positions as fixed-width 8-byte
  uints so the final layout is known in one pass, with no fixpoint
  iteration; JPEG APP13 IPTC as a Photoshop 8BIM `0x0404` resource that
  preserves other resources and concatenates multi-segment APP13 streams
  first; Ogg Vorbis/Opus by de-lacing the header run, substituting only the
  comment packet, re-lacing and recomputing CRC-32 — which keeps the setup
  and codebook header that the container's own writer loses on the standard
  libvorbis layout; FLAC by walking the `METADATA_BLOCK` chain with PADDING
  absorbing the delta so audio frames never move; MP4/QuickTime via
  `moov/udta/meta/ilst` with `stco`/`co64` patched by the moov delta; and
  oversized JPEG APP1 via a real `ExtendedXMP` split whose
  `xmpNote:HasExtendedXMP` GUID is a genuine MD5 of the extended
  serialization. Honest `Err` only where the format cannot represent the
  request (Exif > 64 KB, fragmented MP4, `mdat` on both sides of `moov`,
  `stco` overflow).
- **`oximedia-transcode`: FFV1 frame-level decode is reachable, ALAC gained
  its compressed element form, and Matroska/Ogg audio decodes in-container.**
  The FFV1 decode adapter existed but the engine had no path to it —
  `probe_format` has no magic entry for this crate's `OXIFFV1\0` framing, so
  `detect_format` failed before dispatch ever ran, and only a test driving
  the decoder directly exercised it. A magic sniff plus a shared
  `open_video_source()` now put raw-FFV1 and Y4M input on the identical
  downstream dispatch. ALAC's adaptive-Rice + LPC compressed form is
  implemented (previously uncompressed-only). ProRes stays an honest `Err`
  for the 10-bit 4:2:2 path, citing all three real reasons — the decoder is
  hard-capped to 8-bit output, it fails on realistic high-contrast content,
  and fabricating 10-bit by shifting 8-bit is refused.
- **`oximedia-packager`: real input probing and NAL-aware `cbcs` subsample
  mapping.** New `source_probe.rs` runs the real
  `oximedia_container::MultiFormatProber` over the first 64 KiB of the
  input and derives width/height/codec from the first usable video stream;
  an explicit `config.source_media` still wins, and an unreadable or
  unrecognised input falls through to the pre-existing honest error rather
  than erroring hard. `encryption.rs` gained `nal_subsamples`, which parses
  length-prefixed NAL units per ISO/IEC 14496-15, classifies VCL vs
  non-VCL, and emits per-NAL clear/protected splits with a 32-byte VCL
  clear lead — replacing a whole-buffer pattern that is correct only for
  already-elementary payloads.
- **`oximedia-review` persists.** A new `store/` module backed by
  `oxisql-sqlite-compat` (seven tables) replaces the `Ok(Vec::new())` stubs
  across comments/replies/threads, tasks, versions/comparisons/timeline,
  change requests and notifications. `ReviewStore::in_memory()` and
  `::open(path)` both run their migrations; a process-wide default backs
  the crate's free functions, whose signatures are unchanged, so existing
  callers compile untouched. One deliberate flip in the other direction:
  `find_frame_differences` now returns `Err` instead of a fabricated empty
  list — it needs decoded pixels this crate has no codec dependency to
  produce, and "no differences found" would misrepresent that.
- **`oximedia-py`: 24 module families bound.** Eight neural families
  (attention, recurrent, layers, graph, object/face detection, optical
  flow, model zoo, ONNX) plus per-channel quantisation; ten analytics
  families (A/B testing, bandit, cohort, funnel/churn/loyalty, retention,
  geo/device, quantile/TDigest + realtime, replay/anomaly/attribution,
  social-signal engagement); and six cache families (tiered, bloom,
  distributed, warming, eviction, content-aware/write-behind). Every
  string→enum boundary validates against an explicit allowlist and raises
  `ValueError` on an unknown value, rather than inheriting the Rust side's
  silent fallback — a typo like `"convrsion"` can never be misread as a CTR
  result. `distributed_cache` is documented as performing no network I/O:
  its quorum functions are decision functions, never claims of having
  contacted a peer.
- **`oximedia-vfx` renders real text.** `text/render.rs` was five honest
  `Err` markers; it now rasterises through `fontdue` with real per-glyph
  metrics, greedy word wrap with mid-word fallback, `\n`/`\r\n` hard
  breaks, alignment and block anchoring, outline via circular dilation of
  the coverage mask, drop shadow, and alpha compositing where `color.a`
  multiplies glyph coverage — so fade animations are genuinely visual. The
  unused `ab_glyph` dependency was dropped. Honest `Err` for a missing or
  unparsable font, naming `with_font_bytes`/`set_font_bytes`; no gradient
  was invented, because the API declares no gradient field.
- **`oximedia-automation`: real EAS SAME/AFSK audio.** A continuous-phase
  AFSK modulator at the SAME baud rate (derived as `SAME_SPACE_HZ / 3.0` so
  the mark tone is an exact 4× multiple rather than two independently
  rounded literals), a full `ZCZC-ORG-EEE-PSSCCC...` header builder per
  47 CFR 11.31, and round-trip tests that demodulate the output with a real
  Goertzel detector rather than re-reading the modulator's own state. TTS
  remains an honest deferral.
- **`oximedia-capture`: an optional Tokio-native receive path.** The new
  `tokio` feature adds `CaptureStream::into_async()` /
  `into_async_with_capacity()`, a bounded bridge that preserves the ring
  buffer's drop policy as backpressure rather than converting it into
  unbounded channel latency. It touches no platform code: every `!Send`
  handle already lives and dies on the dedicated capture thread, and the
  bridge consumes only the already-public `recv_timeout`/`is_ended`. The
  `recv()` tri-state (`Ok(Some)` / `Ok(None)` = ended / `Err`) survives the
  wrapper unchanged. Off by default, so a caller with no async runtime
  never pays for tokio.
- **`oximedia-conform`: real Premiere and DaVinci Resolve timeline import.**
  A streaming `xmeml` (FCP7 XML Interchange) parser with a tag-path stack
  for context-sensitive scoping. Researched rather than assumed: Resolve's
  "Timeline > Export > Timeline > Final Cut Pro 7 XML" emits the *same*
  DTD Premiere does, so both entry points call the same parser and differ
  only in the `metadata["format"]` tag — documented in both modules so
  nobody reads "real Resolve importer" as a separately derived parser.
  Honest `Err`, never a silent drop, for compound/nested-sequence clips and
  for xmeml's `-1` "determined by adjacent transition" sentinel, whose
  resolution has no verifiable cross-exporter formula.
- `oximedia-captions`: real frame-difference shot-change detection.
- **`oximedia-renderfarm`: all four pipeline gaps closed.** Real dependency
  resolution via a new `add_asset_source()` (local dir, `file://`,
  `http(s)://`; unsupported schemes named in the error), SHA-256 per-frame
  checksums recomputed from disk on verification, real output assembly
  (JPEG frames muxed byte-for-byte as MJPEG-in-AVI; other decodable formats
  normalised to YUV420p and muxed as Y4M; mixed formats or dimensions are
  an honest `Err` naming the frame), and real PSNR/SSIM against a
  configured reference, falling back to blockiness/blur — no hardcoded
  score anywhere.
- `oximedia-gaming`: `GameCapture::auto_detect` scans the live OS process
  table via `sysinfo` and matches against a `KNOWN_GAMES` table by exact
  name. `window_handle`/`resolution` stay `0`/`(0,0)` and
  `has_hardware_accel` stays `false` — the crate's NVENC/QSV/VCE probes are
  themselves simulated, so reporting `true` would launder a fake signal.
  `InputCapture::poll_events` is an honest `UnsupportedPlatform` error: the
  crate has no OS input-hook machinery at all.
- `oximedia-normalize`: real end-to-end FLAC in the batch path, decoding and
  encoding directly through `oximedia_codec::flac` rather than through
  `oximedia-container`'s `FlacDemuxer` (whose own doc admits it estimates
  frame boundaries from `STREAMINFO.max_frame_size` instead of resolving
  them via CRC-16). `BatchConfig::output_format` — previously dead — now
  genuinely selects the output container, and `write_metadata` measures the
  **output** samples and writes a real BWF `bext` chunk (WAV) or
  VORBIS_COMMENT block (FLAC). Opus and MP3 are honest `Err`s whose text
  states the actual reason.
- `oximedia-container`: `MpegTsMuxer::streams()` returns the streams that
  were added instead of `&[]`; `streaming/demux.rs` has a real packet-level
  jitter buffer (with a `source_exhausted` latch so `read_packet` is never
  polled again after one `Eof`) in place of a no-op; and the Ogg writer
  extracts the real three-header Vorbis split.
- `oximedia-access`: real picture-in-picture compositing for the
  sign-language overlay.
- `oximedia-bench`: the six placeholder sequence generators now synthesise
  real test content.

### Changed
- `oximedia_audio::aac::AacDecoder` implements
  `oximedia_audio::traits::AudioDecoder` again, now that `CodecId::Aac` exists:
  `codec()` returns `CodecId::Aac` instead of the fabricated `CodecId::Mp3`, and
  every decode entry point on the trait fails closed exactly like the inherent
  methods it delegates to. `AacDecoder::codec_name()` is unchanged (`"aac"`).
- `oximedia_core::CodecId` gained a variant. The enum is `#[non_exhaustive]`, so
  downstream `match`es already need a wildcard arm; in-crate matches
  (`media_type`, `name`, `CodecMatrix::is_compatible`) were updated exhaustively.
  `serde` round-trips are unaffected (unit variants serialise by name, not
  ordinal).
- `oximedia_container::StreamInfo` gained the `rotation` and `display_matrix`
  fields. Code that constructs it with a struct literal must add them (use
  `StreamInfo::new` to avoid this); every reader is unaffected.
- `oximedia_container::demux::mp4::SampleInfo` gained a precomputed `dts` field.
- `oximedia_container::demux::mp4::TkhdBox` gained `version`, `flags`, `layer`,
  `alternate_group`, `volume` and `matrix`, and now implements `Default`
  (identity matrix) so existing struct literals can use `..Default::default()`.
- `Mp4Demuxer` emits interleaved packets in true wall-clock DTS order rather than
  raw-tick order, which changes the relative ordering of packets from tracks with
  different timescales. Per-stream ordering is unchanged.
- The workspace-level `quick-xml` dependency now resolves to
  `oxixml-quickxml-compat` 0.1.1, a source-compatible drop-in shim (COOLJAPAN
  Pure Rust dependency migration). All 14 depending crates (`oximedia-archive-pro`,
  `oximedia-automation`, `oximedia-captions`, `oximedia-conform`, `oximedia-drm`,
  `oximedia-edit`, `oximedia-imf`, `oximedia-lut`, `oximedia-metadata`,
  `oximedia-packager`, `oximedia-qc`, `oximedia-stream`, `oximedia-subtitle`,
  `oximedia-timeline`) are source-unchanged; `deny.toml` now bans the upstream
  `quick-xml` crate directly.
- **`vp8::Vp8Decoder` behaviour, following from inter-frame decode landing**
  (see Added): `send_packet` decodes inter frames instead of returning
  `CodecError::UnsupportedFeature`. A frame with `show_frame == 0` —
  including an inter (altref) frame now, not only a key frame as before —
  is decoded and updates the reference buffers, but `send_packet` returns
  `Ok(())` and queues nothing, so a packet-in/frame-out loop may
  legitimately see fewer frames than packets it sent.
  `Vp8Decoder::reset()` now also drops the last/golden/altref reference
  surfaces, entropy context, segmentation map and loop-filter deltas
  (previously a no-op beyond clearing the output queue and flush flag,
  since there was no cross-frame state to drop before this) — after a seek
  the next packet must be a key frame, and an inter frame is rejected with
  `CodecError::InvalidBitstream` until one arrives. Width/height are
  deliberately kept across `reset()` so stream properties stay available
  to a caller still probing. Separately, the criterion for "does an inter
  frame have anything to predict from" is now the sequence decoder's actual
  reference state rather than merely "has a key frame's tag ever parsed":
  sending an inter frame after a key frame's *tag* parsed but whose full
  reconstruction failed previously returned the misleading
  `CodecError::UnsupportedFeature` ("not implemented"); it now correctly
  returns `CodecError::InvalidBitstream`
  (`test_inter_frame_after_a_failed_keyframe_is_rejected_honestly`).
- **BREAKING (`oximedia-videoip`) — `VideoIpReceiver::new` and `connect` now
  take `&VideoFormat, &AudioFormat`.** A receiver cannot construct a real
  decoder without knowing what it is decoding; the previous signatures made
  that impossible, which is part of why the crate shipped fakes. In the same
  crate: the `VideoDecoder::decode` trait method gained a
  `packet_is_keyframe` argument and `AudioDecoder::decode` gained `pts` —
  the old signatures forced `pts: 0` on every frame. Seven examples updated.
  `VideoConfig::new` still defaults to VP9 and `AudioConfig::new` to Opus,
  neither of which this crate can *send*; the defaults are deliberately
  unchanged and the doc comments now name `with_codec` and say so.
- **BREAKING (`oximedia-server`) — `CdnUploader::new()` is gone, replaced by
  `CdnUploader::with_config(...)`.** The old constructor took no
  configuration, so it produced an uploader with an empty bucket and no
  backend whose worker logged and dropped every packet — rebinding dispatch
  alone would have preserved the fake. `with_config` fails fast on an empty
  bucket or a missing feature, so a server configured for CDN now refuses to
  start rather than silently discarding packets or emitting one error per
  packet forever. `RtmpIngestConfig` and `StreamingServerConfig` gained a
  `cdn: Option<CdnConfig>` field.
- **BREAKING-adjacent (`oximedia-gaming`) — new `GamingError::UnsupportedPlatform`
  variant on a non-`#[non_exhaustive]` enum.** A downstream exhaustive
  `match` without a wildcard will break. No in-crate consumer matched
  exhaustively.
- **Behaviour change (`oximedia-mam`) — `statistics()` is now O(bucket) over
  the network** when a cloud feature is enabled, because it genuinely
  enumerates and sums object sizes instead of returning fabricated zeros.
  `StorageManager::check_health()` calls it per registered backend and
  treats `Err` as unhealthy, so with a cloud feature **on** every health
  check now walks the whole bucket, and with the feature **off** a
  registered cloud backend reports unhealthy every time (previously: an
  instant fabricated `Ok`). `available_space`/`used_space` are reported as
  `None` — genuinely unknown through this API — not as `0`.
- **The in-tree Opus codec is now refused by every wired consumer rather
  than having its output passed on.** `oximedia-videoip` returns an honest
  `Err` for Opus in both directions; `oximedia-normalize`'s batch path
  sniffs `OpusHead` and errors with text naming the silence problem;
  `oximedia-transcode` keeps Opus disabled in the frame-level path. See
  Known issues for the measurements behind this.
- `docs/codec_status.md` and `README.md` were re-synchronised with the tree:
  VP9 and FLAC promoted to **Verified**, AVIF and WebP to **Functional**,
  and Opus **demoted** to Bitstream-parsing. `docs/codec_status.md` gained a
  "0.2.1 re-audit summary" covering all five; superseded sentences in the
  0.1.9 and 0.2.0 summaries carry dated `Note (0.2.1)` pointers rather than
  being rewritten, so the audit history stays readable. **`crates/oximedia-codec/src/lib.rs`'s
  own Codec Feature Matrix was not part of this pass and is now the one
  remaining stale copy** — see Known issues.
- `oximedia-workflow`'s built-in patterns used preset names (`1080p-web`,
  `thumbnail-grid`) and QC rule names (`video_quality`, `audio_levels`) that
  exist nowhere in the workspace; they now reference real presets, and the
  module doc states exactly what `DefaultTaskExecutor` can and cannot run.
- `oximedia-server`'s S3 multipart path now streams the real per-part chunks
  and retries the upload with backoff. It previously copied every part into
  a `Vec` it then discarded (doubling peak memory) and "retried" that copy
  while uploading the payload as a single blob — the module doc describing
  parallel per-part retry was fabricated, and has been corrected.
- `crates/oximedia-bench/src/sequences.rs` was split (2,414 → 949 lines,
  plus `sequences/{analysis,database,generator,validate,y4m}.rs`) to satisfy
  the 2,000-line file policy. The public API is reproduced exactly by
  `pub use`; the crate has no in-workspace dependents.

### Removed

- **Five hollow VP9 modules, ~4,800 lines** (`oximedia-codec`), each
  verified to have zero real consumers workspace-wide before deletion —
  including through the crate root's `pub use`, the `oximedia` facade,
  `oximedia-py` and `oximedia-wasm`:
  - `vp9/compressed.rs` (812 lines) — `CompressedHeaderParser` with eleven
    hollow methods and a no-op `SegmentFeatureParser`. The real equivalents
    already exist in `vp9::dec::hdr`.
  - `vp9/prediction.rs` (1,116 lines) — its `SUBPEL_SHIFTS = 8` was simply
    wrong (libvpx uses 16, so it carried only 8 of the 16 subpel rows), and
    it is superseded by `vp9::dec::mc` / `vp9::dec::interpred`.
  - `vp9/mvref.rs` (963 lines) — AV1-shaped `ref_mv_stack` scaffolding,
    superseded by the bit-exact `vp9::dec::mvref`.
  - `vp9/inter.rs` (1,079 lines) — `COMPOUND_MODES = 8` is AV1's count, and
    its `REF_FRAMES = 4` collided with two other constants of the same name
    and different meanings (libvpx's is 8).
  - `vp9/reference.rs` (807 lines) — including `MAX_FRAME_WIDTH`/`HEIGHT`
    caps of 4096×2304 that would have rejected legal streams.

  `vp9::dec::decode_keyframe` was also removed once its last caller went
  away; `Vp9Decoder` dispatches every frame type through one path.
- **`oximedia-videoip`'s `DummyVideoEncoder` / `DummyVideoDecoder` /
  `DummyAudioEncoder` / `DummyAudioDecoder`**, which sat on the public API
  with no feature gate: the encoder returned the raw frame as "compressed"
  and the decoder wrapped arbitrary bytes in a hardcoded 1920×1080 frame.
  No `Mock*` replacement was retained — the new tests use real fixtures.
- **`oximedia-mam`'s fake `S3Storage`/`AzureStorage`/`GCSStorage`
  `StorageBackend` impls**, whose `upload()` logged and reported `size: 0`,
  whose `download()`/`delete()` were no-ops, and whose `exists()` returned
  unconditional `Ok(true)` — a data-loss hazard for any caller using it to
  decide whether to re-upload. Replaced by real delegation to
  `oximedia-storage` behind the pre-existing `s3`/`gcs`/`azure` features,
  structured so a feature-off build **cannot** produce a filled backend
  cache in production.
- **`oximedia-server`'s legacy `S3Uploader`/`GcsUploader`/`AzureUploader`**,
  all three of whose `upload()` bodies were `info!(...); Ok(())` with no
  feature gate. Their `presigned_url`/`signed_url`/`sas_url` were fabricating
  even in the feature-**on** path — all three returned `self.url(key)`, an
  unsigned URL labelled "presigned"; those now delegate to
  `CloudStorage::generate_presigned_url`, which really signs. A dead
  duplicate `CdnUploader` in `streaming_server.rs`, never fed by any
  producer, was removed too.
- `oximedia-vfx`'s unused `ab_glyph` dependency (zero references), replaced
  by `fontdue`.

### Performance
- **`oximedia_graph::filters::video::ScaleFilter` is realtime for vertical-video
  reframing** — roughly **6x faster** on a 9:16 conversion, with **not one output
  byte changed**. On an Apple M3 (8-core, release, `benches/scale_bench.rs`,
  best-observed per-frame cost — the machine was under heavy concurrent load, so
  these are upper bounds on the true improvement):

  | Lanczos3 conversion | before | after | speedup |
  |---|---|---|---|
  | 608x1080 → 1080x1920 (cropped 9:16 reframe) | 33.9 ms | **5.8 ms** | 5.8x |
  | 1920x1080 → 1080x1920 (squeeze to 9:16) | 25.3 ms | **5.6 ms** | 4.5x |

  Against the 33.3 ms budget of a 30 fps frame this moves the filter from *at
  best* break-even to about **5.7x realtime**. Pinned to a single thread
  (`RAYON_NUM_THREADS=1`) it still lands at ~9.5 ms, so the target is met without
  relying on spare cores; the rayon pool is headroom, not the mechanism. Four
  independent changes, in descending order of effect:
  - **The vertical pass no longer walks a column per output pixel.** It
    accumulated `dst_height x dst_width` output samples by striding
    `dst_width x 8` bytes through the f64 intermediate buffer once per tap — for
    a 1080-wide plane every single tap was a cache miss. It now accumulates whole
    intermediate *rows* into a row accumulator. Each output pixel still sums its
    taps in ascending order into an accumulator seeded at `0.0`, so the
    floating-point result is unchanged; only the access pattern is.
  - **Both passes are row-parallel over rayon.** Rows never read each other's
    output, so this is a pure scheduling change. Passes producing fewer than
    64 Ki samples stay on the calling thread, keeping thumbnail work free of
    fork/join overhead. `PassParallelism` makes the choice explicit so tests can
    force either policy and assert the outputs match bit for bit.
  - **Coefficients are cached per plane geometry instead of rebuilt per frame.**
    The old code deep-cloned both full luma coefficient tables (~3000 individual
    `Vec<f64>` allocations), swapped in freshly computed chroma tables, scaled the
    plane, then restored the clones — for *every chroma plane of every frame*,
    roughly 12000 allocate/free pairs per frame. There are now two cached entries,
    one for the luma geometry and one for the shared chroma geometry, keyed on
    source size, target size, algorithm and antialias.
  - **The coefficient tables are flat.** `Vec<Vec<f64>>` became a single
    contiguous `Vec<f64>` plus a `(src_start, weight_offset, len)` span table, so
    the innermost loop is a straight zip over two slices instead of a pointer
    chase with a `.get().copied().unwrap_or(0)` bounds check per tap.

  Byte-exactness is enforced, not assumed: `tests/scale_golden.rs` pins BLAKE3
  digests of the full output — every plane, every byte — for 42 combinations of
  six geometries (antialiased downscale, non-antialiased downscale, pure upscale,
  odd dimensions with `div_ceil` chroma, mixed-direction axes, and one large
  enough that the luma planes take the rayon path while chroma stays serial) and
  all seven `ScaleAlgorithm` variants. **Every digest was generated from the
  pre-optimisation implementation** and is matched exactly by the new one.
  `benches/scale_bench.rs` additionally carries a verbatim copy of the old
  resampler, asserts at setup that the filter still agrees with it byte for byte,
  and benchmarks both in the same process so the comparison stays meaningful on a
  loaded machine.
- **The AV1 inverse transform runs on `i32` instead of `i64`**
  (`av1/kf/itx.rs`, all 46 occurrences; `av1/kf/recon.rs`'s coefficient
  buffers follow). This is a memory-traffic and arithmetic win with **no
  behavioural change**, established by exact equivalence against the
  pre-conversion kernel over 19 transform sizes × 16 transform types ×
  dense and sparse inputs × 3 seeds at full ±32767 coefficient magnitude —
  0 mismatches, in a debug build with overflow checks on — plus all 13
  AV1 bit-exact fixture tests. Safety was proved rather than assumed, by
  propagating exact magnitude bounds through the real stage-by-stage
  network: the widest intermediate is 2^28.5 at 8-bit, i.e. 0.18× of
  `i32::MAX`. The same table shows **12-bit would overflow at 2.83×**, which
  is why this is sound only for an 8-bit decoder: `recon.rs` rejects any
  other bit depth, and closing that gap needs dav1d's restructured
  multiplies (`dav1d/src/itx_1d.c` documents exactly this case), not a
  wider integer. No clamp was added to the spec's `B()` — 7.13.2.1 makes
  the range a *bitstream-conformance requirement*, not a decoder clamp — so
  `butterfly` stays byte-for-byte the spec's function; a single load clamp
  keeps the `pub fn` total for arbitrary callers.
- **The AV1 loop filter is SIMD**, via a new `Simd4` 4×i32 backend in
  `oximedia-simd` (portable scalar fallback always compiled, plus an
  `aarch64` NEON backend) with `av1_loopfilter.rs` on top. One algorithm
  over N backends, so a backend cannot drift from the reference. Four
  sample lines map to four lanes — legal because a vertical edge's four
  lines are distinct rows and a horizontal edge's four are distinct
  columns — and the wide filter is specialised from the spec's O(n²) tap
  loop into an incremental sliding window, which is an identity rather than
  an approximation. The kernels live in `oximedia-simd` because
  `oximedia-codec` has `unsafe_code = "deny"`, making `std::arch`
  impossible there; the dependency is unconditional and dependency-free, so
  the Pure-Rust default build is preserved and `cargo deny check bans`
  stays green. Equivalence is exhaustive over `lvl 0..=63 × sharpness 0..=7`
  through the real strength derivation × 3 sizes × 2 planes (18,432 cases),
  plus 4,000 arbitrary-parameter cases, extremes, and a forced-scalar pass.
  `force-avx2`/`avx512`/`neon` are deliberately **not** honoured, since
  those features are visible to feature unification and would `SIGILL` a
  dependent.

  **Honest deviation:** the vectorised intra-prediction kernels were built,
  measured, and **deleted**. They lost to LLVM's auto-vectorisation of the
  plain spec loops (at width 64: `store_u8` 0.22×, `smooth_h` 0.46×,
  `zone1` 0.72×, `paeth` 1.00×). Shipping them would have been a measured
  regression. What `av1/kf/pred.rs` ships instead is structural:
  `V_PRED`→`copy_from_slice`, `H_PRED`→`fill`, a zone-1 count split that
  removes the per-pixel `base < maxBaseX` branch, and an AboveRow/LeftCol
  split into a contiguous copy plus edge fill that removes the per-sample
  clamp.
- **AV1 CDEF and loop restoration run in parallel over output row bands**
  (`av1/kf/{recon,cdef,lr}.rs`). Reconstruction itself is provably
  sequential — intra prediction reads reconstructed above/left samples —
  and that is documented in the code rather than worked around; the two
  post-reconstruction filter stages are not, because each reads an
  immutable input frame and writes a separate output where every sample is
  produced by exactly one source block. A new `OutBand` type carries no
  caller-supplied stride, which makes a stride mismatch unrepresentable;
  `split_out_bands` uses a safe `split_at_mut` chain (no `unsafe`). Band
  count falls back to **1**, with rayon never entered, when the pool has
  one thread or the frame is small. Bit-exactness is checked against the
  dav1d/aomdec reference YUVs — not self-compared — for 6 fixtures × 8 band
  counts, including an odd 76×42 size, SGRPROJ, switchable Wiener and a
  2-tile stream.

### Known issues
- `ScaleAlgorithm::Nearest` can emit black pixels. Its support is exactly `0.5`,
  so an output centre landing on a half-integer produces two taps at distance
  exactly `0.5`, and the kernel condition (`x < 0.5`) evaluates to zero for both.
  Weight normalisation skips a zero sum, leaving an all-zero window, and that
  output sample resolves to `0` rather than to the nearest source pixel. A
  608→1080 resize hits this on 8 of 1080 columns. The defect predates the
  performance work and is preserved verbatim by it (the goldens pin `Nearest`
  output); `test_coefficient_table_is_normalized_and_in_bounds` documents and
  bounds it, asserting that no *other* kernel produces a degenerate window.
  Fixing it is a deliberate numeric change and needs its own golden refresh.
- **The Opus codec in `oximedia-codec` is not usable in either direction.**
  Fed 26 real libopus-encoded CELT packets, decode returns `Ok` with **0
  non-zero samples out of 49,920** — silence. Fed real SILK packets, it
  produces out-of-range values (max abs 4.0) and 960 samples per frame
  regardless of the configured 16 kHz. The encoder emits a TOC byte that
  misdescribes its own payload (`0x1c`, claiming SILK/NB/60 ms for a
  CELT/FB/20 ms frame), so a conformant decoder reads 2880 samples where
  960 were encoded; where the counts do line up, decoded energy and
  correlation with the input are both 0.0. The code is real and wired — the
  0.1.9 audit that labelled it Functional confirmed that much by reading it
  — but no libopus-encoded fixture had ever been fed through it, and its
  ~82 tests are all self-consistency round-trips, which prove nothing about
  conformance. Every wired consumer now refuses. Fixing this is
  large/specialist work, not a wiring gap. **Provenance of these numbers:**
  they were measured during the 0.2.1 audit with an ad-hoc harness and **no
  committed fixture reproduces them** — the tree has no `.opus` fixture, and
  the committed tests (e.g. `opus_fails_honestly_in_both_directions`) assert
  the *refusal* and its TOC-naming message, not the silence. Re-measure
  against real libopus output before moving this label in either direction;
  assuming a test guards it is precisely how the 0.1.9 audit promoted this
  entry upward in error. **`oximedia-audio` carries a
  separate Opus implementation** (`crates/oximedia-audio/src/opus/`) which
  was **not** audited in this pass and should not be assumed sound on the
  strength of this entry either; it is what `oximedia-wasm`'s
  `WasmOpusDecoder` uses.
- **The `oximedia` facade's `video` feature reaches zero codecs.** The root
  `Cargo.toml` pins `oximedia-codec` with `default-features = false`, and
  `oximedia/Cargo.toml` declares `video = ["dep:oximedia-codec"]` with no
  sub-feature forwarding — so `--features video` compiles cleanly and
  exposes no AV1, VP9 or VP8 decoder at all, contradicting
  `oximedia/src/lib.rs`'s own feature table. Every other consumer in the
  workspace forwards explicitly (`oximedia-cli`, `oximedia-py`,
  `oximedia-videoip`, `oximedia-server`, `benches`, `fuzz`); the facade is
  the only one that does not. Under `--features full` the codecs *are*
  present, but only by unification from sibling crates, never because
  `video` asked for them. `minimal = ["audio", "video", "metadata-ext"]`
  inherits the same gap. This is a capability gap, not a broken build
  (`cargo check -p oximedia --features video --lib` exits 0). The fix
  follows the same file's existing `mjpeg`/`apv` precedent:
  `video = ["dep:oximedia-codec", "oximedia-codec/av1", "oximedia-codec/vp9", "oximedia-codec/vp8"]`.
  Left unapplied pending a maintainer decision on the default feature
  surface.
- **`oximedia-capture`'s new `tokio` feature is unreachable through the
  facade** at any feature combination, for the same reason:
  `capture = ["dep:oximedia-capture"]` forwards nothing, and the facade is
  that crate's only in-workspace consumer. `CaptureStream::into_async` is
  therefore reachable only by depending on `oximedia-capture` directly.
- `oximedia-container`'s `FlacDemuxer::read_packet` **estimates** frame
  boundaries from `STREAMINFO.max_frame_size` rather than resolving them
  via CRC-16, as its own doc comment admits. `oximedia-normalize`'s new
  FLAC path deliberately bypasses it and drives `oximedia_codec::flac`
  directly; any other caller of that demuxer inherits the estimate.
- `oximedia-container`'s CMAF muxer now writes correctly *named* config
  boxes, but forwards `track.extradata` as their payload verbatim. A
  `dOps`/`dfLa` box is therefore only spec-conformant if the caller
  supplied a real `OpusSpecificBox`/`FLACSpecificBox` record;
  `fragment/mp4.rs` passes `codec_params.extradata` through unchanged, so
  an Ogg-style `OpusHead` blob would sit inside a correctly-named but
  wrongly-encoded box. `oximedia-server`'s ingest path does the conversion
  properly (see Added); other callers must too. `av01`/`vp09` are
  unaffected, since their extradata is already the right record.
- `oximedia-container`'s `MultiFormatProber` never parses `tkhd` or the
  visual `stsd` sample entry for MP4, so `DetailedStreamInfo::width` and
  `height` are always `None` for MP4 input. `oximedia-packager`'s new
  source probe therefore falls through to configured `source_media` for
  real MP4 files. Relatedly, no prober path ever populates `fps`, so every
  probe-derived ladder uses a documented `DEFAULT_FRAMERATE = 30.0` — which
  is logged explicitly, but does feed `calculate_bitrate`'s `fps_factor`
  and so shifts every rung's bitrate.
- `VideoFrame::allocate()` (`oximedia-codec/src/frame.rs`) still sizes
  `P010`/`P016` planes as one byte per sample, so both are under-allocated;
  the format layer knows the real layout but `allocate` only consults the
  stride table for `Yuyv422`/`Uyvy422` and NV12/NV21 chroma. No in-tree
  caller allocates either format today. Documented in `allocate`'s doc
  comment as a deliberate deferral.
- `oximedia-capture`'s Linux and Windows backends still have no
  live-device run — ABI layouts are const-verified and the logic is
  host-tested against synthetic input, but no `/dev/video*` node or Media
  Foundation device has been opened by this crate in this workspace's own
  verification.
- **`crates/oximedia-codec/src/lib.rs`'s Codec Feature Matrix is stale** and
  is the last copy of the codec status that disagrees with the tree. It is a
  `//!` doc comment, so it ships in this crate's published rustdoc. Four
  rows are wrong as of 2026-08-12: `lib.rs:46` says VP9 "inter not yet
  (honest `Err`)"; `lib.rs:55` says AVIF "not supported (honest `Err`)";
  `lib.rs:58` says WebP "lossless VP8L only; no lossy VP8 WebP decode";
  and `lib.rs:59` says Opus decode is "✓ (CELT + SILK + Hybrid)", which is
  the overclaim this release demotes. The corrected text for each is in
  `docs/codec_status.md` and the README Codec Matrix. Left unedited because
  this documentation pass was scoped to `TODO.md` / `CHANGELOG.md` /
  `README.md` / `docs/` / `oximedia-cli/TODO.md`; it is a small,
  well-specified follow-up.

## [0.2.0] - 2026-07-15

Development release on branch `0.2.0` (workspace version bumped from
`0.1.9`). Theme: **a real frame-level transcode engine, real AV1/VP9/VP8
key-frame video decoding, and a broad "real or honest error" sweep** that
replaces silent placeholder/fabricated-success behaviour across the
packager, network, workflow, Python bindings, and CLI layers with genuine
implementations or explicit, testable errors. AV1 key-frame/intra decode
lands bit-exact against dav1d and aomdec in this release; inter-frame
decode for AV1/VP9/VP8 remains open for 0.2.x.

### Added
- **Real frame-level transcode engine** (`crates/oximedia-transcode/src/{frame_level,frame_adapters,audio_adapters,raw_sinks,flac_bitstream,flac_decode,alac_bitstream}.rs`):
  a genuine decode → filter → encode pipeline behind `TranscodePipeline`'s
  `requires_frame_level()` gate (`pipeline.rs`), replacing the prior
  stream-copy-only path for any job that actually needs re-encoding.
  WAV/FLAC audio input re-encodes through OxiMedia's own FLAC codec, with
  an encoder→decoder round-trip test (`test_own_encoder_round_trip_bit_exact`)
  asserting the result is bit-exact. Y4M video decode is wired in, with
  MPEG-2/FFV1/ProRes/raw-video encode targets. `-r` frame-rate conversion
  is a real drop/duplicate resampler (`FpsResamplingDecoder`). New file
  muxers back real outputs: `RawEsFileMuxer`, `FlacFileMuxer`,
  `CafAlacFileMuxer` (standards-compliant CAF/ALAC), and `Y4mFileMuxer`.
- **AV1 key-frame/intra-frame video decoder** (`crates/oximedia-codec/src/av1/kf/`:
  `bits`, `msac`, `hdr`, `cdfs`, `coef`, `pred`, `itx`, `recon`, `lf`,
  `cdef`, `lr`, plus mechanically-extracted `tables_*`/`consts`) — an exact
  port of the intra decode path of the AV1 specification
  (AOMediaCodec/av1-spec): symbol/range decoder, sequence/frame header
  parsing, transform-coefficient decode, intra prediction (including CFL),
  and exact inverse transforms, driven by a tile/partition/block decode
  driver, with the full post-filter chain applied to the output —
  deblocking loop filter, CDEF, and loop restoration (both Wiener and
  self-guided/SGRPROJ). `av1/decoder.rs`'s `decode_temporal_unit` now calls
  into this module directly, replacing the old no-op tile-group branch.
  Verified bit-exact (0 differing Y/U/V pixels) against both `dav1d` 1.5.1
  and `aomdec`/libaom v3.12.1 on 13 keyframe test vectors encoded with
  `aomenc` and SVT-AV1 (`stage1_lossless_gray64_bit_exact` through
  `stage4_switchable_wiener_320x192_bit_exact` in `av1/kf/mod.rs`),
  covering lossless coding, 128×128 superblocks, 2 tile columns, an odd
  76×42 crop, and 320×192 loop-restoration cases. Scope is 8-bit 4:2:0
  profile 0, keyframe/intra only: inter-frame decode, intra block copy,
  palette mode, super-resolution, quantizer matrices, film-grain
  synthesis, 10/12-bit, monochrome, and 4:2:2/4:4:4 all return an honest
  `CodecError::UnsupportedFeature` instead of a fabricated frame. The
  orphaned `av1/avif.rs` — never declared as a module, and duplicating the
  live `avif/mod.rs` AVIF implementation — was deleted as dead code found
  during this work.
- **VP9 key-frame/intra-frame video decoder** (`crates/oximedia-codec/src/vp9/kf/`:
  `booldec`, `hdr`, `itx`, `lf`, `pred`, `recon`, `scan`, `tables`) — an
  exact port of libvpx's intra decode path (boolean/range decoder, inverse
  DCT/ADST transforms, the lossless 4×4 Walsh-Hadamard transform, loop
  filter, and the tile/partition/block decode driver), verified bit-exact
  against `ffmpeg`/libvpx reference decodes of real encoder output.
  Non-8-bit profiles, 4:2:2/4:4:4 subsampling, intra-only frames, and
  inter-frame decode are all out of scope for this pass and return an
  honest `CodecError::UnsupportedFeature`.
- **VP8 key-frame video decoder** (`crates/oximedia-codec/src/vp8/keyframe/`) —
  the full RFC 6386 §11–§15 intra pipeline (macroblock mode parsing, DCT
  coefficient token decode, dequantise/inverse-transform, intra
  prediction, in-loop deblocking filter), ported from and cross-checked
  against the pre-existing, production-verified `oximedia-image` WebP/VP8
  still-image decoder (a lossy WebP image *is* a single VP8 key frame);
  both decoders are independently verified bit-exact against libwebp
  reference output (`test_decode_cwebp_textured_48x40_bit_exact_vs_libwebp_reference`,
  `test_decode_libvpx_multi_partition_48x48_bit_exact_vs_libwebp_reference`
  in `oximedia-image/src/webp/vp8/decode.rs`). VP8 inter-frame decode
  returns an honest `CodecError::UnsupportedFeature`.
- **Real CENC/`cbcs` sample encryption in the streaming packager**
  (`crates/oximedia-packager/src/encryption.rs`): `encrypt_cenc`/`decrypt_cenc`
  now perform genuine full-sample AES-128-CTR (128-bit big-endian counter,
  incremented per 16-byte block), and `encrypt_sample_aes`/`decrypt_sample_aes`
  implement the real ISO/IEC 23001-7 §9.6 `cbcs` pattern (1 block encrypted
  AES-128-CBC / 9 blocks left clear, CBC chain reset per sample) — the
  format FairPlay/Shaka/hls.js/dash.js actually expect. Both previously
  just called the plain full-buffer AES-128-CBC helper under a CENC/
  SAMPLE-AES label, producing ciphertext no real CENC or SAMPLE-AES client
  could decrypt.
- **oximedia-cli**: verified real (not stub) behaviour for `--map` stream
  mapping, `-ss`/`-t` seek/duration trim, `-vf` scale, `-af` volume, `-r`
  frame-rate conversion, `--crf`, `--normalize-audio`, `probe --hash`
  (real SHA-256), `probe --quality-snapshot` (decodes frame 0), `validate
  --loudness-check` (real EBU R128 over a decoded WAV), `mam
  --extract-metadata` and its date-range filters, `batch-engine
  --priority`/`--config`/`--state` (SQLite-persisted), `workflow
  --source`/`--destination`, `edl parse --format`, `recommend
  --bitrate`/`--resolution`, and a global `--quiet` flag (logging plus
  several commands' status banners — a documented partial rollout, with
  the remaining ~50 handlers tracked as `TODO(0.2.x)` in `progress.rs`).
  Subtitle/caption extraction now demuxes real Matroska subtitle tracks;
  multicam export renders from real timeline JSON; the virtual-production
  session registry persists to real JSON on disk (not in-memory only);
  `archivepro` performs real FLAC/WAV preservation encodes; `dolbyvision`
  parses real RPU bitstreams (with a NAL-wrapper fallback); and the
  `quality`/`dedup`/`mir` commands decode real media, failing honestly
  when a file can't be decoded instead of scoring synthetic data.
- Matroska muxer `SeekHead` writing (`crates/oximedia-container/src/mux/matroska/seek_head.rs`)
  and matching `SeekHead`-aware seeking on the demuxer side.
- **`oximedia-server` RTMP ingest now actually accepts connections**
  (`crates/oximedia-server/src/rtmp/server.rs`): `RtmpIngestServer::start`
  previously spawned a `run_server` loop that only slept one second at a
  time and iterated an always-empty stream map — the real
  `oximedia_net::rtmp::RtmpServer` accept loop was built but never run, so
  the ingest server never bound a socket or accepted a single publish.
  `start` now spawns the real `RtmpServer::run` accept loop directly, plus
  a new bridge task: `StreamKeyValidator` gained a `publish_notifier`
  channel that fires a `PublishEvent` the moment a publish is authorized,
  and `run_bridge` waits (bounded ~2 s poll) for the corresponding stream
  to appear in `oximedia_net`'s `StreamRegistry` before creating the
  `IngestStream` that feeds transcoding, recording, and CDN upload.
  Verified against a real loopback TCP socket
  (`oximedia-server/tests/rtmp_ingest.rs`).
- **`oximedia-switcher` downstream-keyer (DSK) real auto-transition**
  (`keyer.rs`): `DownstreamKeyer`'s auto-transition now drives a genuine
  linear ramp of the key mix level from its current value to the target
  over `duration_frames` (new `DskTransition` state, advanced by
  `advance_transition`), matching real DSK hardware where the tally state
  reflects the commanded on/off target immediately while the mix level
  ramps over time; previously the mix level had no time dimension at all.
- **`oximedia-vfx` planar tracking: real homography solve**
  (`tracking/planar.rs`): `PlanarData::calculate_homography` previously
  returned a hardcoded identity matrix ("simplified... as placeholder");
  it now calls a new `solve_homography_dlt`, a real 4-point Direct Linear
  Transform (Hartley & Zisserman §4.1) computing the actual 3×3 homography
  mapping the reference corner quad onto the tracked corner quad, used by
  `PlanarTracker::warp_to_reference` for real perspective-warped
  planar-surface tracking (e.g. screen replacement).

### Changed
- **Fabricated-success elimination ("real or honest error") across several
  layers**, each backed by a new regression test proving the old
  behaviour is gone:
  - **`oximedia-py` PyO3 bindings**: `proxy_py.rs` no longer writes a
    placeholder/marker file at the target path for an unsupported output
    container or after a real pipeline failure (`test_*_must_return_err_not_fabricate_a_proxy`);
    `cloud_py.rs`'s `upload()` no longer returns a fabricated
    `provider://bucket/key` URI when no bytes were actually transferred
    (`test_upload_existing_file_returns_err_not_fabricated_uri`);
    `video.rs` no longer turns a decode `Err` into a blank placeholder
    `VideoFrame`; `workflow_py.rs` no longer reports a fabricated
    "completed" status for a task type or failure it cannot honestly
    execute (`test_run_workflow_transcode_failure_reported_honestly`).
  - **`oximedia-net` RTMP relay** (`rtmp/server/relay.rs`): forwarding to
    an unreachable target, or a full outbound queue, is now honest
    back-pressure — packets are counted as dropped rather than silently
    accepted (`test_relay_manager_forward_to_unreachable_is_honest`).
  - **Codec honesty**: Opus hybrid-mode encode now returns an honest
    `UnsupportedFeature` error instead of emitting a packet that wasn't a
    real hybrid encode (`test_hybrid_mode_encode_returns_honest_unsupported_error`);
    the JPEG XS decoder propagates real entropy-decode errors instead of
    zero-filling the output (`decode_propagates_entropy_error_instead_of_zero_filling`);
    VP8/VP9 inter-frame decode return honest errors (see Added).
  - **`oximedia-cli`**: `loudness analyze`/`check` now decode and meter
    real WAV samples instead of a block of synthetic silence, which
    previously reported fabricated metrics/compliance for every input;
    subtitle and timecode burn-in now validate their real inputs and then
    return an explicit "burn-in not implemented yet, no output file was
    written" error rather than a silent no-op success.
  - **`oximedia-effects`**: `FilterBand::low_shelf`/`high_shelf` now
    compute real RBJ Audio-EQ-Cookbook shelf-filter coefficients; the
    prior code built a `LowShelf`/`HighShelf` band but its coefficients
    were a flat pass-through ("simplified"), so the requested EQ shape
    was silently never applied.
  - **Eight more "honest `Err` instead of fabricated `Ok`" fixes** found
    during the same sweep, each backed by a `*_is_honest_err_*` regression
    test: `oximedia-renderfarm`'s render pipeline (`pipeline.rs`) —
    `resolve_dependencies` no longer infers "satisfied" from an empty list,
    `verify_all_frames` no longer hardcodes `true`, and `assemble_output`/
    `calculate_quality_metrics` now return honest `Err` instead of a
    fabricated completed-render result (real dependency download, frame
    verification, sequence assembly, and quality metrics remain
    `TODO(0.2.x)`); `oximedia-stabilize`'s `ThreeDStabilizer::stabilize_3d`
    no longer silently passes the input transforms through as if they were
    a real structure-from-motion 3D solve; `oximedia-vfx`'s text-rendering
    `VideoEffect::apply` (`text/render.rs`) no longer reports `Ok(())` for
    non-empty text with no glyph rasterizer wired up; `oximedia-access`'s
    sign-language overlay `apply()` (`sign/overlay.rs`) no longer returns
    an empty-but-`Ok` composited frame when it has no pixel compositor;
    `oximedia-captions`'s `detect_shot_changes` (`shotchange.rs`) no longer
    returns a fabricated empty `Ok` result; `oximedia-automation`'s EAS
    message composer (`eas/audio.rs`) no longer fabricates silence when
    `load_tts_audio` can't produce real TTS samples; `oximedia-conform`'s
    Premiere Pro / DaVinci Resolve XML importers (`importers/xml.rs`) no
    longer return a fabricated empty `Ok(vec![])` clip list; and
    `oximedia-accel`'s Vulkan compute backend (`compute_backend.rs`) no
    longer returns a result buffer that was never actually dispatched to a
    GPU.
- `oximedia-container`'s metadata writer (`metadata/writer.rs`, over the
  2000-line policy limit) split via `splitrs` into `metadata/writer/mod.rs`
  and `metadata/writer/flac.rs`.

### Removed
- **`oximedia-cli transcode --resume`**: the flag was already dead code
  (`TranscodeOptions::resume` was `#[allow(dead_code)]` and never consulted
  by any transcode code path — no resume-from-partial-encode capability was
  ever implemented behind it). It is now removed from the CLI entirely:
  clap rejects `--resume` as an unknown argument, and it no longer appears
  in `transcode --help` (`resume_flag_is_rejected_by_clap`,
  `resume_flag_absent_from_help` in `oximedia-cli/tests/transcode_trim_map.rs`).
  Any script currently passing `--resume` will need to drop it; it was
  already a silent no-op before this change.

### Fixed
- **RTMP client handshake ordering** (`crates/oximedia-net/src/rtmp/client.rs`):
  `perform_handshake` called `parse_s2` (validate S2, which transitions
  the handshake state machine to `Done`) before `generate_c2` (send C2,
  which transitions it to `AckSent`) — the reverse of RFC-specified
  order. `generate_c2` now runs first.
- **`oximedia-workflow` executor dropped non-root tasks** (`executor.rs`):
  `execute()` scanned the task order with a single-pass iterator, so a
  task whose dependencies weren't yet satisfied was skipped past and,
  because the shared iterator never revisited it, every non-root task in
  a dependency chain was silently dropped while the workflow still
  reported `Completed`. Replaced with a repeated-rescan (fixpoint)
  scheduler that runs all tasks in dependency order; a real non-root
  failure now correctly yields `Failed`
  (`test_non_root_failure_is_not_silently_dropped`,
  `test_dependency_chain_runs_all_steps_in_order`).
- **FLAC encoder frame-header CRC-8** (`crates/oximedia-codec/src/flac/encoder.rs`):
  was hardcoded to `0`; now computes the real CRC-8 (polynomial `0x07`,
  matching the standard catalogue check value `0xF4` for `"123456789"`).
- **JPEG-LS RUN-mode decoder out-of-bounds write** (`crates/oximedia-codec/src/jpegls/decoder.rs`):
  a run interruption near the end of a row could write past the row
  boundary; fixed with an explicit bounds check before the token is
  written, pinned by a regression test at `run_index = 7`.
- **DNG writer IFD offset computation** (`crates/oximedia-image/src/dng/writer.rs`):
  tag/value offsets into the out-of-line deferred-data area are now
  computed and written in a single, final pass over the
  tag-ascending-sorted entry list.
- **wasm32 `1usize << 32` constant-eval/overflow**: allocation-limit
  constants in `oximedia-codec` (`util/limits.rs::MAX_ALLOC_BYTES`),
  `oximedia-image` (`limits.rs::MAX_ALLOC_BYTES`), and `oximedia-pipeline`
  (`memory_pool.rs::MAX_POOL_BYTES`) were expressed as a `usize` left
  shift, which overflows on the 32-bit `usize` of the
  `wasm32-unknown-unknown` target; all three are now explicit `u64`.
- **SRT key exchange used a fake RFC 3394 key wrap** (`crates/oximedia-net/src/srt/crypto.rs`):
  `aes_key_wrap`/unwrap only masqueraded as RFC 3394 AES Key Wrap and
  produced non-interoperable output. Rewritten as the real algorithm (six
  rounds over all key blocks, the `0xA6A6A6A6A6A6A6A6` integrity IV per
  RFC 3394 §2.2.1) and made fallible, so a corrupt or forged wrapped key
  is rejected instead of silently accepted; verified against the RFC 3394
  §4.1 128-bit test vector.

### Security
- **Parser bounds/allocation-cap hardening** against maliciously-crafted
  input, added across several parsers: MP4 box nesting now rejected
  beyond `MAX_BOX_DEPTH` (32 levels, preventing a stack-overflow via deep
  recursion) with `checked_add` on box-offset arithmetic
  (`oximedia-container/src/demux/mp4/{boxes,mod}.rs`); DVB subtitle
  region-composition dimensions capped at `MAX_REGION_DIMENSION_PX`
  (4096 px) before use (`oximedia-subtitle/src/parser/dvb.rs`); RTSP
  request/response bodies capped at `MAX_RTSP_BODY_LEN` (16 MiB) with a
  `checked_add` guard on the body-end offset so a huge `Content-Length`
  can't overflow it (`oximedia-net/src/rtsp/message.rs`); WebRTC SCTP
  message reassembly capped at 4 MiB per stream / 16 MiB total via
  `saturating_add` accounting (`oximedia-net/src/webrtc/sctp.rs`); RTMP
  chunk size clamped to `MAX_CHUNK_SIZE` (64 KiB) and message-length
  preallocation capped at `MAX_MESSAGE_PREALLOC` (64 KiB) regardless of
  the attacker-declared message length (`oximedia-net/src/rtmp/chunk.rs`);
  AAF `LazyEssence` now validates a declared `(offset, length)` essence
  range against the real file/stream size (with `checked_add`) before
  allocating the read buffer, closing an over-declared-length
  memory-exhaustion path (`oximedia-aaf/src/lazy_essence.rs`); and a
  division-by-zero guard (`checked_div`) was added to container bitrate
  estimation (`oximedia-container/src/container_probe/multi_format.rs`).
- **SRT RFC 3394 AES key wrap** — see Fixed.
- **Real CENC/`cbcs` AES-CTR packager encryption** — see Added; the
  previous mislabeled full-buffer CBC path was not a real confidentiality
  guarantee against a client expecting CENC/SAMPLE-AES semantics.

## [0.1.9] - 2026-07-14

This is a production-readiness release: the default build is now **100% Pure
Rust** end to end (verified — `aws-lc-sys`, `libsqlite3-sys`, `mlua-sys`,
`shaderc-sys`, `zstd-sys`, `openssl-sys`, `rustfft`, and `realfft` are all
absent from the default dependency graph), four real security vulnerabilities
(cryptographic, denial-of-service, and SQL injection) inherited from earlier
"demonstration" code were fixed, the algorithmic hardening from development
Waves 21–30 lands for general use, and
a new **`oximedia-web`** npm package brings four browser WebAssembly modules
(scopes, colour, scale, quality) downstream of WebCodecs for the first time.

### Security
- **SRT payload encryption** (`oximedia-net::srt::crypto`): replaced a
  homebrew XOR/byte-mixing "cipher" that provided no real confidentiality
  with genuine **AES-128/192/256-CTR** (NIST SP 800-38A) via the vetted
  RustCrypto `aes` + `ctr` crates. Key derivation replaced a toy
  hash-and-extend construction with real **PBKDF2-HMAC-SHA256** (RFC 8018),
  and salts now come from the process CSPRNG (`rand`) instead of a
  timestamp-seeded LCG.
- **HLS/DASH packager encryption key generation** (`oximedia-packager::encryption::KeyGenerator`):
  `generate_aes128_key()`/`generate_iv()` now draw from the OS CSPRNG
  (`rand::rngs::SysRng`) instead of deriving "random" bytes from
  `SystemTime::now()`; `from_passphrase()` now uses real
  PBKDF2-HMAC-SHA256 (100,000 rounds, salted) instead of a bare
  `DefaultHasher` (SipHash, not designed for password hashing). Both
  key-generation methods are now fallible (`PackagerResult<Vec<u8>>`) so a
  CSPRNG failure is surfaced instead of silently producing predictable keys.
- **MP4 sample-table parsing** (`oximedia-container::demux::mp4::boxes`):
  `stts`/`stsc`/`stsz`/`stco`/`co64`/`stss`/`ctts` box parsers now validate
  that the attacker-controlled 32-bit `entry_count`/`sample_count` field can
  actually fit within the bytes remaining in the box *before* calling
  `Vec::with_capacity`, closing a memory-exhaustion DoS where a ~20-byte
  crafted file could trigger a multi-gigabyte allocation attempt.
- **WebRTC DTLS-SRTP honestly demoted to Experimental** (`oximedia-net::webrtc::dtls`):
  the module previously returned a "successful" handshake with an all-zero
  SRTP master key/salt, silently transmitting media in plaintext under a
  DTLS-protected label. `DtlsEndpoint::handshake()` and
  `DtlsConnection::send`/`recv` now refuse (return an error) instead of
  fabricating a connection, since no real DTLS 1.2 handshake or RFC 5764
  SRTP key export is implemented yet. SDP fingerprint generation and
  signaling are unaffected. **Do not use WebRTC media transport for
  confidential media until this lands.**
- **SQL injection in `oximedia-mam` smart-collection queries**
  (`oximedia-mam::collection::CollectionManager::build_condition_sql`/
  `execute_smart_query`): user-authored smart-collection filter conditions
  (`QueryCondition::field`/`::value`, stored as JSON and replayed every time
  the collection is viewed) were spliced directly into SQL text — `field`
  was interpolated into the query with no validation at all (letting a
  stored collection inject an arbitrary column or expression), and `value`
  was embedded inside hand-rolled `'...'` string literals for the
  `Equals`/`NotEquals`/`Contains`/`StartsWith`/`EndsWith` operators with no
  escaping (a `'` in the value could break out of the literal). Fixed with a
  new `ALLOWED_ASSET_COLUMNS` allowlist validating every user-supplied
  `field`/`sort_by` identifier before interpolation (identifiers can never be
  SQL bind parameters) and a `BindValue` enum that routes every condition
  value through `sqlx`'s real parameter binding instead of string
  formatting; surfaced while auditing call sites for sqlx 0.9's
  `SqlSafeStr`/`AssertSqlSafe` gate.
- `cargo-audit` advisories re-triaged for the new dependency set:
  `RUSTSEC-2026-0049` (rustls-rustcrypto → rustls-webpki 0.102.8, reachable
  only via oximedia-drm's non-default widevine/playready/fairplay features),
  `RUSTSEC-2026-0174` (azure_core → http-types notice, no user-controlled
  input reaches the affected constructors), and `RUSTSEC-2026-0192`
  (unmaintained `ttf-parser` via fontdue/usvg font-rasterization chains,
  application-supplied font assets only) documented in `audit.toml` with
  unreachability rationale; the stale `RUSTSEC-2026-0002` (tantivy/ratatui →
  `lru` unsound `IterMut`) ignore entry was removed now that it no longer
  applies.
- `cargo-audit`: `RUSTSEC-2026-0206` (`rustybuzz` unmaintained-crate notice,
  not a scored vulnerability) added to `.cargo/audit.toml`'s ignore list with
  documented unreachability rationale — reachable only via font-shaping/
  rendering paths operating on application-supplied font assets (subtitle
  rendering, SVG overlays), never attacker-controlled network input.

### Changed
- **Default build is now 100% Pure Rust.** All C/C++/Fortran dependencies
  that used to be compiled unconditionally are now behind non-default,
  opt-in Cargo features:
  - `oximedia-server`/`oximedia-rights`: SQLite storage migrated from `sqlx`
    (libsqlite3-sys, C) to **oxisql-sqlite-compat** (Pure Rust), via a
    small `sqlx`-API-shaped compat shim so existing call sites needed
    only an import-path change.
  - `oximedia-accel`: real Vulkan compute (`vulkano`/`vulkano-shaders`, which
    pull the shaderc/glslang C++ toolchain) gated behind the new
    `vulkan-backend` feature (`vulkan-detect` now implies it); Pure-Rust CPU
    fallback (and optional `webgpu`) is used by default.
  - `oximedia-automation`: Lua scripting (`mlua`, which vendors the Lua 5.4
    C interpreter) gated behind the new `lua-scripting` feature.
  - `oximedia-cloud`: the official AWS SDK (`aws-sdk-*`, whose smithy TLS
    stack only ships C-based crypto providers — `ring`/`aws-lc`/`s2n`) gated
    behind the new `aws-sdk` feature; S3-compatible endpoints remain
    available by default via `GenericStorage` (reqwest + rustls-rustcrypto).
  - `oximedia-videoip`: QUIC transport (`quinn`, which requires `ring` or
    `aws-lc-rs` since `rustls-rustcrypto` doesn't implement QUIC cipher
    suites) gated behind the new `quic-quinn` feature.
  - `tantivy` (oximedia-mam/search): default features trimmed to drop
    `zstd-sys`-backed columnar compression, keeping Pure-Rust
    `lz4-compression` only.
  - `actix-web` (oximedia-server): default features trimmed to drop
    `zstd`/`gzip`/`brotli` response compression (all C-backed).
  - `azure_core`/`azure_storage`/`azure_storage_blobs`: migrated to the
    unified `azure_storage_blob` 1.0 track with `default-features = false`
    and explicit `reqwest` + `hmac_rust` (Pure-Rust HMAC via sha2/hmac,
    replacing the openssl-backed default), closing the quick-xml
    RUSTSEC-2026-0194/0195 chain pinned by the deprecated 0.21 track.
  - `oximedia-audio`: the `rubato` resampler (which pulls `rustfft`/`realfft`)
    replaced with a hand-written 100% Pure-Rust band-limited
    windowed-sinc polyphase resampler (Blackman-Harris window, exact
    rational-position accumulator for drift-free long streams, chunk-size
    invariant output, explicit `flush()` for stream tails) — no FFT
    dependency of any kind.
- Added `[profile.release]` to the root `Cargo.toml`: `opt-level = 3`,
  `lto = "thin"`, `codegen-units = 1`, `strip = true` for smaller, faster
  release binaries across all ~109 crates.
- `sqlx` workspace dependency trimmed from `["runtime-tokio", "sqlite"]` to
  `["runtime-tokio", "postgres"]` (Postgres support is Pure Rust; SQLite use
  sites migrate to `oxisql-sqlite-compat` instead).
- `README.md`: added a "Live demos" section linking the OxiScope colour
  pipeline demo and the new peer-to-peer **OxiLink** video-call project —
  both running the same `oximedia-web` WebAssembly modules in production.
- Root `Cargo.toml`: fifteen dependencies that were hardcoded identically
  (or near-identically) across two or more member crates' `Cargo.toml`
  files — `approx`, `csv`, `dirs`, `encoding_rs`, `flume`, `futures-util`,
  `jsonwebtoken` (`rust_crypto` feature), `mockito`, `num-traits`,
  `pin-project`, `protox`, `reed-solomon-erasure`, `tonic-build`,
  `tonic-prost-build`, `unicode-segmentation` — centralized into
  `[workspace.dependencies]` for consistent version resolution across the
  workspace.

### Added
- **`oximedia-web`** (`web/`): a new nested Cargo workspace + npm package
  (`@cooljapan/oximedia-web`, unpublished — packaging is prepared, publish
  is pending explicit instruction) providing four independent,
  tree-shakeable WebAssembly modules downstream of the browser's own
  WebCodecs decoder — `scopes` (waveform/vectorscope/histogram/
  false-colour), `color` (exposure/contrast/saturation, tone-mapping,
  gamut mapping, 3D LUT), `scale` (Lanczos3/Catmull-Rom/Mitchell/bilinear
  resampling), and `quality` (PSNR/SSIM). Ported, dependency-free
  `f32`/`u8` kernels (no `rayon`/`scirs2`/`f64` data planes), each crate
  `#![forbid(unsafe_code)]`, no COOP/COEP requirement, all four modules
  comfortably under their gzip size budgets (150,072 B measured combined
  vs. a 512,000 B soft budget, 29% — re-measured after the kernel perf
  retune below; see `web/README.md`'s size table for the per-module
  breakdown). Ships with the OxiScope colorist demo
  (`web/demo/`, grading + four live scopes fed from the graded output) and
  a reproducible benchmark harness (`web/bench/`, headless-Chrome driven,
  zero committed/hard-coded numbers) plus four local CI-gate shell scripts
  (`build.sh`, `size-gate.sh`, `dep-gate.sh`, `serve.sh`) — no GitHub
  Actions workflow. See [`web/README.md`](web/README.md) and
  [`web/TODO.md`](web/TODO.md).
- `oximedia-web`'s public JS/TS surface (`web/js/`): hand-written ES-module
  wrappers (`_frame.js`, `color.js`, `quality.js`, `scale.js`, `scopes.js`,
  ~1,900 lines combined) plus matching hand-written `.d.ts` type
  declarations wrap each crate's raw `wasm-bindgen` glue in an idiomatic,
  tree-shakeable API — this, not the raw wasm-bindgen output, is what
  `@cooljapan/oximedia-web`'s four subpath exports (`./scopes`, `./color`,
  `./scale`, `./quality` in `web/package.json`) actually resolve to.
  `web/deny.toml` (a `cargo-deny` config scoped to the `web/` nested
  workspace — license allowlist plus the `wasm32-unknown-unknown`/
  `x86_64-unknown-linux-gnu`/`aarch64-apple-darwin` target graph) and
  `web/allowed-deps.txt` (the exact-name crate allowlist `dep-gate.sh`
  diffs against) enforce the "no `rayon`/`scirs2`/heavyweight dependency"
  constraint at CI-gate time, not just by convention.
- `rust-toolchain.toml` pinning `channel = "stable"` with `rustfmt`/`clippy`
  components, for reproducible CI and local builds.
- `CONTRIBUTING.md`, `SECURITY.md`, and `CODE_OF_CONDUCT.md` at the repo root.
- Per-crate opt-in Cargo features documented above (`vulkan-backend`,
  `lua-scripting`, `aws-sdk`, `quic-quinn`) so downstream users can restore
  the C-backed fast paths deliberately, without them leaking into the
  default build.
- Ten new `cargo-fuzz` targets in `fuzz/fuzz_targets/` — `aaf_parser`,
  `ass_parser`, `exr_parser`, `ffv1_decoder`, `jpegxl_decoder`, `srt_parser`,
  `tiff_parser`, `ttml_parser`, `webvtt_parser`, `y4m_parser` — extending
  malformed-input fuzz coverage to the AAF/ASS/OpenEXR/FFV1/JPEG XL/SubRip/
  TIFF/TTML/WebVTT/Y4M parsers, alongside the pre-existing DASH/FLAC/Opus/
  RTMP/Vorbis targets.
- `oximedia` facade crate: `oximedia/src/lib.rs` gained three real,
  `cargo test --doc`-executed doctests (probing + `dedup`, `transcode`
  config + `quality` PSNR assessment, and a `prelude` quick-start), a
  per-example "Cookbook" table cross-referencing each `examples/*.rs` file
  to the Cargo feature(s) it needs, and a doc-link to the underlying
  `oximedia_*` crate on every feature-gated re-export module;
  `oximedia/examples/ffmpeg_translate_demo.rs` demonstrates translating an
  FFmpeg command line into an OxiMedia transcode job via the
  `compat-ffmpeg` feature; `oximedia/tests/feature_matrix.rs` is a
  compile-only harness proving every Cargo feature flag builds
  independently (`cargo check --no-default-features --features <flag>` per
  flag, parsed straight out of the crate's own `Cargo.toml`);
  `oximedia/tests/prelude_smoke.rs` verifies `prelude::*` is usable with
  zero optional features enabled; and `oximedia/tests/integration.rs` adds
  cross-feature subsystem tests (always-on `OxiError`/`OxiResult`/
  `probe_format` coverage for Matroska and MP4 headers, plus feature-gated
  `quality`/`timecode`/`metering`/`archive` and combined `search`+`quality`
  suites).
- `oximedia-cli/tests/cli.rs`: a 267-line core end-to-end CLI smoke suite —
  `--version`/`version`/`version --json` reporting, top-level `--help`,
  invalid-subcommand error handling, and a real `probe` run against a
  synthetic WAV file written to `std::env::temp_dir()` — alongside the
  pre-existing, more granular `cli_help.rs`/`cli_help_per_command.rs`/
  `probe_json_snapshot.rs`/`exit_code_smoke.rs` suites.
- `oximedia` facade crate: `[package.metadata.docs.rs]` added to
  `oximedia/Cargo.toml` (`all-features = true`, `rustdoc-args = ["--cfg",
  "docsrs"]`) so the published docs.rs build renders every optional
  feature, with `doc_cfg` feature-availability badges, instead of only the
  defaults.

### Fixed
- Repo hygiene: removed tracked build-artifact droppings that should never
  have been committed (`crates/oximedia-core/src/hdr/mod.rs.orig`/`.rej`
  patch-reject files, `crates/oximedia-proxy/Cargo.toml.bak`,
  `crates/oximedia-simd/Cargo.toml.bak`, `crates/oximedia-playout/IMPLEMENTATION_SUMMARY.md`,
  a generated `doc/` rustdoc output tree, and an orphaned
  `linker-scripts/glibc_compat.lds` — a `PROVIDE()`-based shim aliasing ISO
  C23 `strtol`/`strtoll`/`strtoull` symbols for a pre-compiled ONNX Runtime
  object on glibc <2.38, unreferenced by any current build script).
- **Root workspace tokio feature-unification** (`Cargo.toml`): pinning
  `tokio = { features = ["full"] }` at the workspace root was silently
  unioning `"full"` (via Cargo's workspace feature-unification) into every
  member that declares a `tokio` dependency, including `oximedia-graph` —
  the sole `tokio` consumer reachable from the `oximedia-wasm` wasm32
  build graph — pulling in `mio` (`"full"` → `"net"` → `mio`) and breaking
  `cargo check -p oximedia-wasm --target wasm32-unknown-unknown`. Root pin
  lowered to `default-features = false` with an explicit feature list on
  every tokio-declaring member (`oximedia-graph` gets the minimal
  `net`-free set; every other member keeps an exact superset of its prior
  union-derived features, zero behavioural change); the wasm32 `mio`
  blocker is resolved.
- **`oximedia-wasm` data-plane and decoder honesty** (`oximedia-wasm/`):
  `Float64Array`-crossing `#[wasm_bindgen]` APIs (colour-management
  buffers, HDR EOTF/OETF buffers, `audiopost_wasm::wasm_mix_audio`)
  converted to `f32`/`u8`; `webcodecs_bridge.rs`'s JSON-string hot-path
  methods (`get_video_decoder_config`, `oximedia_packet_to_encoded_chunk`)
  replaced with typed getter classes; the standalone `WasmVp8Decoder`,
  `WasmAv1Decoder`, and `WasmVorbisDecoder` classes removed — each wrapped
  a decoder that produced no real output (error, unpopulated buffers, or a
  synthetic-format-only round-trip), so shipping them was dishonest;
  `oximedia-stabilize`/`oximedia-imf`/`oximedia-aaf`/`oximedia-analytics`
  (unused) dependencies and a dead `/tmp`-path line pruned; the orphaned
  `hdr_wasm`/`lut_wasm`/`spatial_wasm` modules wired into `lib.rs` (with
  their own f64→f32 conversion); `wasm-opt` re-enabled (`-Oz`); npm
  packaging (`build.sh`/`build-dev.sh`/`npm-publish.yml`) and `README.md`
  corrected so only `pkg-bundler`'s `@cooljapan/oximedia` is presented as
  published/installable — `pkg-web`/`pkg-node` are documented as
  unpublished local build artifacts. Native check/clippy(`-D
  warnings`)/test are clean, and the wasm32 target now compiles: the two
  `#[cfg(target_arch = "wasm32")]` `RequestAdapterOptions` initializers in
  `crates/oximedia-gpu/src/device.rs` were missing the `apply_limit_buckets`
  field required by `wgpu` 30.0 (the native path already set it), so
  `cargo check -p oximedia-wasm --target wasm32-unknown-unknown` failed;
  both browser paths now set `apply_limit_buckets: true` (limit-bucketing is
  a browser GPU-fingerprinting mitigation), restoring the `oximedia-gpu`
  wasm32 build reached transitively via `oximedia-colormgmt`'s default
  `gpu-accel` feature.
- **wasm32 warnings-zero sweep**: `cargo check -p oximedia-wasm --target
  wasm32-unknown-unknown` and `cargo clippy --target wasm32-unknown-unknown
  -- -D warnings` across `oximedia-gpu`, `oximedia-convert`,
  `oximedia-dolbyvision`, `oximedia-container`, `oximedia-monitor`, and
  `oximedia-batch` are now clean (0 warnings, native and wasm32 alike):
  unreachable-expression fixes in `oximedia-convert`, cfg-gated unused
  constants in `oximedia-dolbyvision`, a genuine `Shared<T>` type-alias
  split (`Rc` on `wasm32`, `Arc` elsewhere) resolving `arc_with_non_send_sync`
  in `oximedia-gpu` plus matching `apply_limit_buckets: false` wasm32
  semantics, and matching consumer-tied cfg-gates in `oximedia-container` /
  `oximedia-monitor` / `oximedia-batch`. `docs/simd_dispatch.md`'s
  "SSE4.2 fallback" section corrected: it no longer claims WASM SIMD128
  "falls to SSE4.2 paths in `x86.rs`" (that module is
  `core::arch::x86_64`-gated and never compiles on `wasm32`); it now
  forward-references the doc's own WASM SIMD128 section instead.
- `oximedia-server`: 13 production `.expect()`/`.unwrap()` call sites removed
  across `admin.rs`, `rtmp/server.rs`, `webhooks.rs`, and `db.rs` — replaced
  with proper error propagation or infallible-by-construction rewrites (e.g.
  indexing the element just pushed instead of `.last().expect(...)`,
  constructing default socket addresses instead of `"...".parse().expect(...)`).
- **`oximedia-mam` list-filter bind-parameter mismatch**
  (`AssetManager::list` in `asset.rs`, `AuditLogger::query_logs` in
  `audit.rs`): every optional filter (`mime_type`/`min_duration`/
  `max_duration`/`status`/`created_by` for assets; all 7 `AuditLogFilter`
  fields for audit logs) appended its `$N` placeholder to the query text
  but was never actually passed to `.bind()` — `audit.rs`'s `bindings` Vec
  only compiled because `user_id`/`resource_id` happen to share a type, and
  even that Vec was never bound. Any call supplying so much as one filter
  would have failed at execution time with a bind-parameter count
  mismatch. Replaced with a `bind_filters!` macro binding each present
  field in the exact order its placeholder was appended; a pre-existing bug
  unrelated to any dependency version, surfaced while auditing these call
  sites for sqlx 0.9's `SqlSafeStr` gate (`sqlx::AssertSqlSafe` now wraps
  both dynamically-built query strings, whose only dynamic content is the
  bound `$N` placeholders).
- Version metadata (`workspace.package.version`, all 108 crate path-dependency
  version pins) bumped `0.1.8` → `0.1.9`.
- **`oxiarc-*` dependency family realigned at `0.3.6`** (`Cargo.toml`):
  `oxiarc-archive`, `oxiarc-deflate`, `oxiarc-lz4`, and `oxiarc-zstd` bumped
  `0.3.5` → `0.3.6`. `oxiarc-archive` 0.3.6 calls APIs added in its own
  transitive siblings (`oxiarc-brotli`/`oxiarc-bzip2`/`oxiarc-lzma`/
  `oxiarc-snappy`/`oxiarc-core`/`oxiarc-lzhuf`) at the same `0.3.6` release;
  crates.io confirms all of them published `0.3.6` within the same few
  minutes on 2026-07-13 (siblings first, `oxiarc-archive` last, respecting
  publish dependency order), and `Cargo.lock` now resolves the whole family
  at a uniform `0.3.6`, unblocking `cargo build` for `oximedia-archive-pro`,
  `oximedia-batch`, `oximedia-convert`, `oximedia-cli`, `oximedia-py`, and
  `oximedia-wasm`.
- `oximedia-transcode`: the `TranscodeCache` module-doc example passed a
  `PathBuf` (`std::env::temp_dir().join(...)`) directly to
  `TranscodeCache::insert`, which takes `output_path: String` — the doctest
  did not compile. Fixed with `.to_string_lossy().into_owned()`.

### Improved
- **`oximedia-web` WASM kernel perf retune**: `scopes`/`color`/`scale`
  per-frame kernels retuned — `scale`'s Lanczos3 h-pass monomorphised over
  a const tap-count span with a 2-bank FMA accumulator (the prior
  runtime-tap-count loop broke the FMA latency chain) plus a 4-tap-fused
  v-pass and a bit-exact opaque-frame premultiply skip; `color`'s 3D LUT
  path repacked to u64 lattice points (1 load/corner), a branchless
  Sakamoto tetrahedron select, and a bit-identical last-pixel memo;
  `scopes` killed per-pixel function-pointer YCbCr dispatch, added
  vectorised row-buffer conversion and run-collapsed scatter accumulation
  for the vectorscope/histogram, and gained a `Scopes.load_frame` +
  `*_current()` API so a caller pays one frame-boundary copy per frame
  instead of four. wasm SIMD128 confirmed real via `wasm-dis` v128-op
  counts (hundreds per module, not just the codegen flag). Measured via
  `web/bench/run.sh` (headless Chrome 150, median of 60, macOS, this
  machine): `scopes` all-four-combined ~13.0–13.25 ms (≤16 ms budget
  target: **met**; ≤8 ms stretch goal: not met), `color`
  exposure+ACES+LUT33 ~24.9–25.1 ms (≤12 ms target: **not met**, ~3.7x
  faster than the pre-retune baseline), `scale` Lanczos3 4K→1080p
  ~51.8–52.25 ms (≤40 ms target: **not met**, ~5.4x faster). Total gzip
  across all four modules + glue held at 150,072 B / 512,000 B soft budget
  (29%) despite the kernel work. See `web/README.md`'s "Measured
  performance" section for the full table and `web/TODO.md` for the
  itemized remaining gaps (`color`/`scale` targets, both attributed to
  `VideoFrame` copy/acquisition cost rather than the wasm kernel itself).

### Development waves 21–30 (algorithmic hardening)
- **oximedia-calibrate**: flagship fix — the ICC/display color-matrix solver
  now performs a real least-squares 3×3 color matrix fit (`B·A⁻¹` via a 3×3
  adjugate inverse with a condition-number/rank-deficiency guard),
  replacing an identity-matrix stub; verified against known-answer fixtures
  at ΔE2000 < 2.0.
- Six pre-existing bugs fixed across the audio/video pipeline: `oximedia-restore`
  Wiener-filter gain (~14 dB error) and wow/flutter destructive processing,
  `oximedia-audio-analysis` Hann-window and 2048× synthesis attenuation,
  `oximedia-graph` node-collision on merge, and related SILK NSQ 440 Hz SNR
  threshold correction (now matches the actual round-trip floor, ~5.2 dB).
- `oximedia-proxy`: frequency/recency-aware cache warming (`ProxyCacheWarmer`);
  `oximedia-align`: bit-exact cross-frame descriptor cache for feature
  matching; `oximedia-search`: reusable P@k/R@k/AP/MAP evaluation harness.
  `oximedia-forensics`: perceptual-hash nearest-neighbor search wired in.
- Extensive test hardening across `gpu`, `neural`, `stream`, `video`,
  `multicam`, `normalize`, `container`, `captions`, and other crates —
  combined test gate green with 0 clippy warnings at each wave checkpoint.

## [0.1.8] - 2026-06-02

### Added
- `oximedia-repair`: mmap-backed `deep_scan` (memmap2, ≥4 MiB threshold with streaming fallback for smaller files), mtime-aware `detection_cache` (parking_lot `RwLock` short-circuit), and full `fix_issue` dispatcher wired to `conceal`, `partial`, `container_migrate`, and `codec_probe` submodules.
- `oximedia-neural`: `onnx` Cargo feature gate; new `OnnxBackend` struct (`load`, `run` with `HashMap<String, Tensor>` API) backed by `oxionnx`.
- `oximedia-audio`: `compute_log_mel_spectrogram` (STFT → Hann window → MelScale filterbank → log) added to the `spectrum` module.
- `oximedia-ml`: `AutoCaptionPipeline` — Whisper-compatible encoder+decoder ONNX inference pipeline with greedy decode; `AutoCaptionConfig`, `encode_audio`, `step_decode`, and `caption` entry points; gated behind the `auto-caption` Cargo feature.
- `oxionnx` (companion crate): `SessionBuilder::with_provider_kinds()` for typed runtime EP selection; `ProviderKind::DirectMl` variant (behind `directml` feature); EP dispatch chain consults the provider priority list at runtime.
- `oximedia-hdr`: process-wide `GamutConversionMatrix` cache (`OnceLock<RwLock<HashMap<(ColorGamut, ColorGamut), [[f32;3];3]>>>`) eliminates redundant Bradford CAT + matrix-inverse computation per call pair.
- `oximedia-stream`: six `SpliceInfoSection` encode→parse→re-encode roundtrip tests; `CmafChunk.data` migrated from `Vec<u8>` to `bytes::Bytes`; `write_cmaf_segment` returns `Vec<Bytes>` for zero-copy scatter-gather segment output.
- `oximedia-colormgmt`: `ToneCurve` enum with `ReinhardSimple`, `ReinhardExtended { l_white }`, `FilmicHable` (Hable/Uncharted2), and `AcesFitted` (Narkowicz rational) operators.
- `oximedia-dedup`: `MergeExecutor`, `AppliedAction`, and `MergeReport` — real filesystem duplicate resolution with symlink, hardlink, delete, and dry-run modes, including safety precondition checks.

### Changed
- `oxionnx`: version bumped `0.1.2 → 0.1.3` to reflect the new typed EP selection API.

### Fixed
- `oxionnx`: 18 clippy warnings in CoreML example files (`coreml_arcface_smoke.rs`, `coreml_scrfd_smoke.rs`, `coreml_inswapper_smoke.rs`) resolved; examples now compile cleanly under `-D warnings`.
- `oximedia-repair`: orphaned stub `repair_engine.rs` (all branches logged no-ops with no real implementation) deleted.

## [0.1.7] - 2026-05-21

### Fixed
- **Issue #9** — Theora decoded frame plane data now correctly written into `VideoFrame` planes via `copy_from_slice` instead of writing to a dropped temporary clone.
- **Issue #13** — `oximedia-timesync` IPC socket module now gated on `#[cfg(all(unix, not(target_arch = "wasm32")))]` to prevent Windows/WASM compilation failures.
- **Issue #14** — `TempFileManager` filenames are now globally unique across concurrent instances using a static `AtomicU64` manager-ID counter combined with process ID and creation nanoseconds.
- **Issue #15** — AVC SPS constraint byte doctest corrected to use 8-bit read (6 flags + 2 reserved) per H.264 §7.3.2.1.1.
- **Issue #16** — Scope command temp input files now use unique per-call names (PID + thread ID + AtomicU64 + nanos) to prevent parallel invocation collisions; frame-extract hardcoded `/tmp/out.y4m` replaced with `std::env::temp_dir()`.

### Added
- Regression tests for all fixed issues (#9, #13, #14, #15, #16).
- Build prerequisites documentation for `protoc` (tonic-build), `cmake`, and `shaderc` toolchain in root `README.md` and `crates/oximedia-accel/README.md`.

## [0.1.6] - 2026-04-25

### Added
- **Stub implementations across 10+ crates** — accel color-space conversion helpers (RGB↔YCbCr, HSV, linear↔sRGB), Vorbis codebook VQ decode scaffolding, ACES Output Device Transform (ODT) variants (P3-D65, Rec.709, Rec.2020, D60-sim, sRGB), DASH segment HTTP fetch skeleton, and system font directory scanning (`/System/Library/Fonts`, `~/.local/share/fonts`, Windows `C:\Windows\Fonts`). All stubs compile cleanly, are documented with `#[allow(dead_code)]` guards, and carry TODO markers pinned to specific crate milestones.
- **Wave 3 stub resolution** — 13 previously-placeholder functions across `oximedia-codec`, `oximedia-audio`, `oximedia-image`, `oximedia-lut`, and `oximedia-caption-gen` replaced with functional implementations; total test count rose to **81,582** (up from ~80,900 at Wave 2 baseline).
- **`oxifft` upgraded to 0.3.0** — workspace dependency bumped from 0.2.0 to 0.3.0; all 13 dependent crates (`oximedia-audio`, `oximedia-audio-analysis`, `oximedia-audiopost`, `oximedia-mir`, `oximedia-effects`, `oximedia-dedup`, `oximedia-watermark`, `oximedia-multicam`, `oximedia-cv`, `oximedia-metering`, `oximedia-restore`, `oximedia-analysis`, `oximedia-watermark`) pass `cargo check` cleanly. OxiFFT 0.3.0 delivers Makhoul-reduction DCT-II (~4× faster vs 0.2.0), plan caching for R2r/R2c solvers, and hand-optimized AVX-512 codelets for sizes 16/32/64; the `fft`/`ifft`/`Complex` surface used by OxiMedia is API-stable.

### Changed
- **`exr.rs` refactored into 9 modules** via `splitrs` — the monolithic `oximedia-image/src/exr.rs` (previously over 2000 lines) was split into: `exr/core.rs`, `exr/compression.rs`, `exr/channels.rs`, `exr/metadata.rs`, `exr/scan_lines.rs`, `exr/tiles.rs`, `exr/deep.rs`, `exr/multipart.rs`, and `exr/mod.rs`. All files are under 2000 lines; public API is unchanged.
- **AWS SDK sub-crate version constraints updated** to match Cargo.lock actuals: `aws-sdk-s3 1.131`, `aws-sdk-mediaconvert 1.126`, `aws-sdk-medialive 1.134`, `aws-sdk-mediapackage 1.98`, `aws-sdk-cloudwatch 1.110`, `aws-sdk-sts 1.103`, `aws-sdk-kms 1.105` (cosmetic alignment; Cargo.lock was already current).
- Workspace version bumped to **0.1.6** (was 0.1.5).

### Security
- **RUSTSEC-2026-0104 documented and ignored** (`audit.toml` + `.cargo/audit.toml`) — reachable panic in `rustls-webpki 0.101.7` CRL parsing, transitive via `aws-sdk-* → aws-smithy-runtime/tls-rustls → legacy-rustls-ring → rustls 0.21.12`. OxiMedia S3/cloud calls never perform CRL checks (standard DNS hostnames, no revocation list usage); the affected code path is unreachable at runtime. Upgrading to the patched `rustls-webpki 0.103.13` path requires `aws-lc-sys` (C dependency excluded by COOLJAPAN Pure Rust Policy). Entry mirrors the existing rationale for RUSTSEC-2026-0098 and RUSTSEC-2026-0099; `cargo audit` exits 0. Will resolve when AWS SDK migrates `aws-smithy-runtime` to rustls 0.23+.

### Validated
- `cargo check` clean for all 13 `oxifft`-dependent crates after upgrade to 0.3.0.
- `cargo audit --no-fetch` exits 0 (no unignored vulnerabilities).
- `splitrs`-generated `exr/` modules all under 2000 lines; no public API regressions.

## [0.1.5] - 2026-04-21

### Added
- **Pure-Rust ONNX inference via OxiONNX** — new `oximedia-ml` crate wrapping `oxionnx` 0.1.2, `oxionnx-core`, `oxionnx-gpu`, and `oxionnx-directml` as optional deps. Typed pipelines with zero-cost defaults: no ONNX symbols are linked unless the `onnx` feature is explicitly enabled.
- **`oximedia-ml` core types** — `OnnxModel` (Session wrapper), `ModelCache` (concurrent `Arc<Mutex<_>>` map with optional LRU capacity), `TypedPipeline` trait (`Input`/`Output` associated types + `process()`), `DeviceType` with `DeviceType::auto()` runtime probe (`Cpu`/`Cuda`/`WebGpu`/`DirectMl`/`CoreMl`), `ImagePreprocessor` (ImageNet mean/std normalization, NCHW/NHWC, letterbox/resize-to-fit), postprocess helpers (`softmax`, `sigmoid`, `argmax`, `top_k`), and a `ModelZoo` registry scaffold.
- **`SceneClassifier` pipeline** — Places365/ImageNet-style typed pipeline on OxiONNX, configurable `top_k`, ImageNet-normalized 224×224 NCHW preprocessing, softmax → top-K postprocess. Constructors: `from_model`, `from_path`, `with_top_k`.
- **`ShotBoundaryDetector` pipeline** — TransNetV2-compatible I/O (48×27 NCHW rolling window of frames, many-hot output for hard/soft cuts) with configurable window length and threshold; returns `Vec<ShotBoundary { frame_index, confidence, kind: Hard | SoftCut }>`.
- **Facade integration** — new `oximedia::ml` module re-exporting `oximedia-ml` behind `features = ["ml"]`; sub-features `ml-scene-classifier`, `ml-shot-boundary`, and `ml-onnx` for selective inclusion. `full` feature now picks up `ml`, `ml-scene-classifier`, `ml-shot-boundary`.
- **Workspace deps** — added `oxionnx-ops`, `oxionnx-gpu`, `oxionnx-directml`, and `oxionnx-proto` at 0.1.2 to root `[workspace.dependencies]` so sub-crates can opt in via `workspace = true`.
- **Example** — `examples/ml_scene_classify.rs` demonstrates end-to-end scene classification via the typed pipeline (gated by `ml` + `ml-scene-classifier`).
- **Feature gates on `oximedia-ml`** — `onnx`, `cuda`, `webgpu`, `directml`, `scene-classifier`, `shot-boundary`, `all-pipelines` (default build remains symbol-free).
- **Tests** — 55+ tests across `oximedia-ml` covering model-cache concurrency, LRU eviction, preprocessing (ImageNet normalize, letterbox, layout), pipeline contracts, and synthetic tensor fixtures.
- **Comprehensive ML guide** (`docs/ml_guide.md`) + README `Sovereign ML Pipelines` section covering typed pipelines, feature matrix (crate + facade + downstream), device selection with GPU backend table, CLI reference, WASM support matrix, and roadmap — Wave 6 Slice C.
- **Python `oximedia.ml` submodule** (Wave 5 Slice B, 2026-04-21) — new PyO3 bindings for the typed ML pipeline stack, gated on the `oximedia-py/ml` feature. Exposes `MlDeviceType` (with `auto`/`cpu`/`cuda`/`webgpu`/`directml`/`coreml` constructors, `from_name`, `list_available`, `capabilities`), `MlDeviceCapabilities` (rich probe record), `OnnxModel` (`load`/`load_from_bytes` accepting bytes, per-model `info()`/`device()`), `MlModelInfo`/`MlTensorSpec`/`MlTensorDType`, `MlModelZoo` + `MlModelEntry` mirroring the zoo registry, and the full pipeline set: `SceneClassifier`, `ShotBoundaryDetector` (+ always-available `heuristic()` fallback), `AestheticScorer`, `ObjectDetector`, `FaceEmbedder`. Numpy `(H, W, 3) uint8` arrays for image pipelines and `(N, H, W, 3) uint8` for the shot-boundary sliding window. Result wrappers (`SceneClassification`, `ShotBoundary`, `AestheticScore`, `Detection`, `FaceEmbedding`) are Python-native dataclass-like objects; `FaceEmbedding` supports `cosine_similarity`, `to_list()`, and `to_numpy()`. 11 integration smoke tests in `crates/oximedia-py/tests/ml_smoke.rs` drive the submodule via an embedded Python interpreter. Depends on `oximedia-ml/all-pipelines`; not pulled in by default, so the default `pip install oximedia` build stays lean.

### Changed
- Workspace version bumped to **0.1.5** (was 0.1.4).
- `oximedia` facade gains the `ml` feature (off by default); the `full` feature now pulls in `ml` plus the `ml-scene-classifier` and `ml-shot-boundary` sub-features.
- **Codec decoder honesty pass (documentation-only)**: introduced a four-tier decoder taxonomy (`Verified` / `Functional` / `Bitstream-parsing` / `Experimental`) in the top-level README and in `oximedia-codec/README.md`. Decoders that previously carried a "Stable" / "Complete" label but do not yet reconstruct pixel or sample data end-to-end (AV1, VP9, VP8, Theora, Vorbis, AVIF) are now accurately labelled `Bitstream-parsing`. No source behaviour changes — the decoders still parse the bitstream as before. See `docs/codec_status.md` for the full per-decoder status, what each stub is missing, and the effort estimate.
- `examples/decode_video.rs` rewritten to reflect the real decoder-status matrix instead of printing fake code samples that pretended to drive a full AV1/VP9 decode.

### Added
- **`docs/codec_status.md`** — per-decoder state, missing pieces, effort bucket (small / medium / large / specialist), and 0.1.5-vs-0.2.0+ target. Referenced from the top-level README, `oximedia-codec/README.md`, and `TODO.md`.
- **`crates/oximedia-codec/tests/av1_real_bitstream.rs`** — `#[ignore]`'d integration test harness for GitHub issue #9. Reads a real AV1 bitstream path from the `OXIMEDIA_AV1_FIXTURE` env var (skips cleanly when unset, so no binary fixture ships in the repo) and asserts that the Y plane of at least one decoded frame has non-zero variance. Will pass automatically once AV1 pixel reconstruction lands.
- **`TODO.md`** gains a "Codec Implementation Roadmap" section mirroring `docs/codec_status.md` effort buckets.
- Documentation round 3: `docs/rate_control.md`, `docs/simd_dispatch.md`, `docs/wave5_deltas.md`.

### Notes
- `oximedia-neural` continues to ship its pre-existing homegrown ONNX-style runtime alongside the new `oximedia-ml` OxiONNX-backed pipelines; consolidation onto a single ML stack is planned for a future milestone.
- CPU inference is fully pure-Rust via `oxionnx`. GPU backends (`cuda`, `webgpu`, `directml`) are additive feature gates wired in `oximedia-ml`; broader crate-by-crate integration (Waves 3–6 on the 0.1.5 TODO list) will land in subsequent cycles.

### Validated
- **Wave 6 Slice D — Full CI gate** (2026-04-21): `cargo check --workspace --all-features` clean; `cargo clippy --workspace --features onnx --all-targets -- -D warnings` clean (zero warnings); `cargo doc --workspace --features onnx --no-deps` clean after fixing 3 pre-existing unresolved intra-doc links to `MlError` in `oximedia-scene::ml` (fully-qualified to `oximedia_ml::MlError`); ML stack end-to-end tests all green — `oximedia-ml` 124 + 22 doctests, `oximedia-scene` 790, `oximedia-shots` 906, `oximedia-recommend` 991, `oximedia-mir` 800, `oximedia-caption-gen` 491 (4,124 tests); WASM gate clean for `oximedia-ml` (default, `onnx`, `onnx+webgpu`) and facade `oximedia --features ml` on `wasm32-unknown-unknown`; facade feature matrix validated (`no-default`, `ml`, `ml-onnx`, `full`); all `oximedia-ml` source files well under 2000-line refactor threshold (largest: `model.rs` at 500 lines).
- **Pre-existing non-ML surface noise surfaced (not blocking)**: `oximedia-container` emits an `unused import: TagMap` warning on `cargo check -p oximedia --target wasm32-unknown-unknown --features ml` (`crates/oximedia-container/src/metadata/editor.rs:8`) — exit code 0, unrelated to Wave 1-6 ML work, tracked separately for a future sweep.

## [0.1.4] - 2026-04-20

### Added
- **MJPEG codec end-to-end wiring**: encoder, decoder, MP4/MOV sample entry (`jpeg` fourcc), Matroska `V_MJPEG` codec ID, proxy codec integration in `oximedia-multicam`, transcode dispatch in `oximedia-transcode`
- **APV codec end-to-end wiring**: encoder, decoder, MP4 sample entry (`apv1` fourcc), Matroska `V_MS/VFW/FOURCC` with BITMAPINFOHEADER CodecPrivate, compat-ffmpeg pass-through, transcode dispatch
- **AVI container (Wave 3)**: AVI v3 OpenDML support for files >1 GB; PCM audio muxing; H264/RGB24 codec arms in RIFF-AVI muxer (`mux/avi/writer.rs`) and demuxer (`demux/avi/reader.rs`); hdrl + movi + idx1 index
- **AJXL ISOBMFF animated container**: `AnimatedJxlEncoder::finish_isobmff()` emits spec-conformant `ftyp` + `jxll` + `jxlp*` box chain (ISO/IEC 18181-2); shared ISOBMFF helper module (`make_box`, `make_full_box`, `BoxIter<R: Read>`)
- **AJXL streaming decoder**: `JxlStreamingDecoder<R: Read>: Iterator<Item = CodecResult<JxlFrame>>` with auto-detection of ISOBMFF vs OxiMedia native format; lazy `jxlp` box parsing; memory-bounded (one frame in-flight)
- **`CodecId::FromStr` + `FourCc`**: 24-alias `FromStr` implementation and `canonical_name()` for all codec IDs; `FourCc` struct with 31 predefined constants in `oximedia-core` (`types/fourcc.rs`)
- **CLI MJPEG/APV support**: `VideoCodec::{Mjpeg, Apv}` variants with `is_intra_only()`, `default_crf()`, `validate_crf()`; intra-codec fast path in `TranscodePipeline`
- **WASM32 platform gating**: `oximedia-batch` and `oximedia-convert` `mio` dependency cfg-gated for WASM; `oximedia-gpu` and `oximedia-graphics` `GpuAccelerator` Send+Sync WASM cfg-gate; `oximedia-colormgmt`, `oximedia-workflow`, `oximedia-farm` also pass `cargo check --target wasm32-unknown-unknown` cleanly; tokio/tonic/rusqlite deps target-gated in `oximedia-farm`
- **MP4 muxer fragment modes (Wave 3)**: `Mp4FragmentMode` enum (Progressive/Fragmented); AV1 `av1C` config box emission; MJPEG/APV codec arms in MP4 sample entry
- **Matroska enhancements (Wave 3 + Wave 4)**: `seek_sample_accurate()` in Matroska demuxer; `preroll_samples`/`padding_samples` fields in MP4 elst box; `BlockAdditionMapping` support in MKV muxer
- **DASH/CMAF streaming (Wave 3 + Wave 4)**: DASH MPD manifest emitter (`dash/manifest.rs`); CMAF-LL chunked DASH MPD emitter for low-latency delivery; cross-format `seek_sample_accurate()` trait
- **FFmpeg compat extensions (Wave 3)**: `filter_complex.rs` — FilterGraph parser for `-filter_complex` arguments; `stream_spec.rs` — `StreamSelector` for FFmpeg stream specifiers; `seek.rs` — `parse_duration` for FFmpeg duration strings; `ffprobe_output.rs` — `FfprobeOutputFormat` output struct
- **FFmpeg compat quality flags (Wave 4)**: `OnceLock`-cached codec-map for zero-cost repeated lookups; `-crf`/`-b:v`/`-maxrate`/`-bufsize` arguments translated to `EncoderQuality`; `-vf`/`-af` filter chain parsing; two-pass encoding support (`-pass 1`/`-pass 2`)
- **APV codec aliases (Wave 3)**: APV codec aliases added to `codec_map.rs` and `codec_mapping.rs` in `oximedia-compat-ffmpeg`; 4 pre-existing failing tests fixed
- **Dolby Atmos channel layouts (Wave 4)**: `oximedia-core` gains 7.1.2, 5.1.4, 7.1.4, 9.1.6, and binaural Dolby Atmos channel layout variants
- **Color metadata types (Wave 4)**: `ColorPrimaries`, `TransferCharacteristics`, and `MatrixCoefficients` enums plus `ColorMetadata` struct added to `oximedia-core`
- **Timestamp arithmetic (Wave 4)**: arithmetic operator impls (`Add`, `Sub`, `Mul`, `Div`) on `Timestamp` in `oximedia-core`

### Fixed
- **JPEG encoder spec-compliance**: DQT table now serialized in zigzag order per JPEG spec; EOB marker emitted only when trailing-zero AC run exists; dequantization ordering corrected. MJPEG round-trip PSNR at Q85: 6.16 dB → 32.53 dB
- **Matroska MJPEG/APV codec IDs**: `codec_id_string` now returns `V_MJPEG` / `V_MS/VFW/FOURCC` instead of falling through to `V_UNCOMPRESSED`
- **MP4 muxer APV/MJPEG validation**: `validate_codec` now accepts royalty-free codecs APV and MJPEG; `codec_to_fourcc` maps them to `apv1`/`jpeg`

### Improved
- **87,387 tests passing** (up from 80,901 in Wave 3; 80,901 up from ~80,500 pre-Wave 3); zero clippy warnings
- **Docs sweep (Wave 3 + Wave 4)**: rustdoc updated for 10 crates (gpu, storage, routing, collab, presets, switcher, automation, core, codec, compat-ffmpeg) plus codec, io, and bitstream crates; 20 TODO markers resolved

## [0.1.3] - 2026-04-15

### Added
- `JobProgress` tracking in `oximedia-farm` job queue
- `bit_depth()` method on `SampleFormat` in `oximedia-core`
- `output_validator`, `worker_health`, `auto_scaler`, `cloud_storage` modules now public in `oximedia-farm`

### Fixed
- VU meter ballistics -Inf poisoning when processing zero-amplitude samples (`oximedia-audio`)
- Subtitle chain comma replacement corrupting subtitle text (`oximedia-convert`)
- ABR rate control overflow in lookahead multiplier calculation (`oximedia-codec`)
- Scene cut detection depth limit missing spikes beyond index 4 (`oximedia-codec`)
- EWA resampling weight table returning non-empty on zero source dimensions (`oximedia-scaling`)
- Audio codec validation rejecting patent-free codecs only (`oximedia-cli`)
- Module conflict between `processor.rs` and `processor/mod.rs` (`oximedia-image-transform`)
- Broken intra-doc links in `oximedia-routing`, `oximedia-server`, `oximedia-container`, `oximedia-neural`, `oximedia-review`, `oximedia-effects`
- Multiple clippy warnings across workspace

### Changed
- Replaced banned `lz4` dependency with `lz4_flex` in `oximedia-collab` and `oximedia-renderfarm`
- Replaced `zstd` with `lz4_flex` compression in `oximedia-renderfarm` storage
- Updated workspace metadata: authors, homepage fields standardized across all crates

### Improved
- 80,393 tests passing (up from 70,800+ in v0.1.2)
- Zero clippy warnings with `-D warnings`
- Clean rustdoc build with strict flags
- 2.65M SLOC across 106 crates

## [0.1.2] - 2026-03-16

### Added

#### New Crates (11)
- **oximedia-hdr** — HDR processing with PQ/HLG transfer functions, tone mapping, gamut mapping, HDR10+ SEI metadata, HLG advanced modes, color volume analysis, and Dolby Vision profile support.
- **oximedia-spatial** — Spatial audio engine with Higher-Order Ambisonics (HOA), HRTF binaural rendering, room simulation, VBAP panning, head tracking, Wave Field Synthesis, and object-based audio.
- **oximedia-cache** — Intelligent media caching with LRU eviction, tiered storage, predictive warming, Bloom filter membership, consistent hashing, ARC adaptive replacement, and content-aware policies.
- **oximedia-stream** — Adaptive streaming with BOLA ABR algorithm, segment lifecycle management, SCTE-35 ad signaling, multi-CDN failover, manifest builder, and stream packager.
- **oximedia-video** — Video processing toolkit with motion estimation, deinterlacing, frame interpolation, scene detection, pulldown removal, video fingerprinting, and temporal denoising.
- **oximedia-cdn** — Content delivery network management with edge node orchestration, cache invalidation, origin failover, geographic routing, and CDN performance metrics.
- **oximedia-neural** — Neural network inference for media with tensor operations, Conv2D layers, batch normalization, activation functions, and media-specific models (scene classifier).
- **oximedia-360** — 360-degree video processing with equirectangular-to-cubemap projection, fisheye correction, stereo 3D layout, and Google Spatial Media XMP metadata.
- **oximedia-analytics** — Media analytics with session tracking, retention curve analysis, A/B testing framework, and engagement scoring models.
- **oximedia-caption-gen** — Automatic caption generation with speech-to-text alignment, Knuth-Plass line breaking, WCAG 2.1 accessibility compliance, and speaker diarization.
- **oximedia-pipeline** — Declarative media processing DSL with typed filter graph construction, execution planning, and optimization passes.

#### Plugin System
- **oximedia-plugin** — SemVer dependency resolver, u32 bitmask capability sandbox, FNV-1a hash-based hot-reload for dynamic codec plugins at runtime.

#### Broadcast and Routing
- **NMOS IS-04/05/07/08/09/11 REST APIs** in `oximedia-routing` with full device discovery, connection management, event and tally, audio channel mapping, stream compatibility, and system API support (656 tests).
- **NMOS mDNS/DNS-SD discovery** for automatic service registration and browsing (605 tests).

#### CLI Extensions
- Loudness analysis and normalization commands.
- Quality assessment (VMAF/SSIM/PSNR) commands.
- Deduplication detection commands.
- Timecode conversion and arithmetic commands.
- Batch engine commands for job scheduling.
- Scopes rendering (waveform/vectorscope/histogram) commands.
- Workflow template execution commands.
- Version info command (333 tests across CLI).

#### Benchmarks and Testing
- 4 criterion benchmark suites in `benches/` crate for codec, filter, I/O, and pipeline performance regression testing.
- 9 new examples demonstrating common workflows.
- 51 integration tests in `oximedia/tests/integration.rs`.
- 70,800+ tests passing across the entire workspace.

#### WASM and Python
- WASM target `wasm32-unknown-unknown` now builds cleanly with all feature gates (505 tests pass).
- PyPI publish workflow fixed (maturin 1.8.4, corrected protoc URL, macOS Intel runner).

### Changed

#### Major Crate Enhancements (40+)

- **oximedia-normalize** — DisneyPlus, PrimeVideo, Apple Spatial Audio, and Dolby Atmos loudness standards; adaptive scene-based normalization; multiband IIR filtering.
- **oximedia-server** — Admin API endpoints, Prometheus `/metrics` endpoint with AtomicU64 counters, HMAC webhook signing, batch delete and batch transcode operations.
- **oximedia-playout** — Transitions (dissolve, wipe, dip-to-color), CEA-608/708 subtitle insertion into playout streams, pre-flight validation checks, MultiChannelScheduler for parallel channel playout.
- **oximedia-net** — Low-Latency HLS (RFC 8216bis) with partial segments and preload hints, XOR FEC (RFC 5109) for packet recovery, QUIC transport abstraction layer.
- **oximedia-mam** — Pub/sub EventBus for asset lifecycle events, rule-based AI auto-tagger, BM25+Jaccard smart search with relevance ranking.
- **oximedia-batch** — Priority-heap job queue, conditional DAG execution (OnSuccess/OnFailure/Threshold branches), timeout enforcer with graceful cancellation.
- **oximedia-graphics** — HDR compositor with 16 blend modes, 1D/3D LUT application with Adobe .cube parser, ASC CDL color grading with slope/offset/power/saturation.
- **oximedia-workflow** — 8 pipeline templates with DOT graph export, StepCondition evaluator for conditional branching, p95 latency metrics tracking.
- **oximedia-monitor** — Alerting rules engine (Threshold, RateOfChange, Absence detection), LTTB downsampling with EWMA time-series smoothing, health registry with dependency checks.
- **oximedia-archive** — LZ77+LZ4 streaming compressor, pure-Rust SHA-256 digest verification, split/reassemble OARC format for large media archives.
- **oximedia-farm** — 6 load-balancing strategies (round-robin, least-connections, weighted, random, hash, power-of-two), locality-aware job distribution, heartbeat-based worker pool management.
- **oximedia-scopes** — False color overlay (7 exposure zones), 3D RGB histogram visualization, 5-mode exposure metering (spot, center-weighted, matrix, highlight, shadow).
- **oximedia-subtitle** — SRT/VTT/ASS/TTML parsers and serializers, 8x12 bitmap burn-in renderer, timing adjuster with offset and stretch.
- **oximedia-effects** — Freeverb and convolution reverb, multi-voice chorus and flanger, 7 distortion algorithms (overdrive, fuzz, bitcrush, wavefold, tube, tape, digital clip).
- **oximedia-mixer** — Topology-sorted mixing bus graph, 8-band parametric EQ with biquad filters, DAW-style automation lanes with interpolation.
- **oximedia-drm** — AES-128/256 implementation from scratch (NIST FIPS 197 verified), content key lifecycle management, license server with region-based gating.
- **oximedia-gpu** — RGBA-to-YUV420 and YUV420-to-RGBA conversion kernels, Gaussian/Sobel/Otsu image processing, buffer pool allocator, pipeline stage chaining.
- **oximedia-rights** — Royalty calculation engine (6 revenue bases), clearance workflow with counter-offer/region/time constraints, ISRC/ISWC/ISAN identifier validation.
- **oximedia-virtual** — LED volume stage simulation with moire pattern checker, FreeD D1 camera tracking protocol, frustum culling with 6-plane extraction.
- **oximedia-io** — 42-variant magic-byte content detector, Boyer-Moore-Horspool optimized reader, MP4/FLAC/WAV/MKV probe implementations.
- **oximedia-mir** — Beat tracking with dynamic programming, mood detection on Russell circumplex model, Camelot harmonic mixing codes (607+ tests).
- **oximedia-colormgmt** — Rec.709/Rec.2020/DCI-P3 gamut mapping, Bradford chromatic adaptation, CIECAM02 full forward/inverse transform, CIEDE2000 with RT rotation term, median-cut/k-means/octree palette quantization.
- **oximedia-cv** — SORT multi-object tracker, pyramidal Lucas-Kanade optical flow (831+302 tests).
- **oximedia-shots** — Audio scene boundary detection via spectral flux analysis, flash detection and Harding PSE compliance checker.
- **oximedia-recommend** — ALS and SVD++ collaborative filtering for encoding parameter recommendation.
- **oximedia-quality** — Temporal quality analyzer for frame-over-frame drift, pipeline quality gate with broadcast/streaming/preview threshold presets.
- **oximedia-codec** — VBV-aware rate control, AV1 level constraint table, PacketReorderer for B-frame output ordering.
- **oximedia-audio** — YIN pitch detection (4 algorithm variants), Kaiser-windowed sinc resampler, EBU R128 K-weighted loudness gating.
- **oximedia-image** — 2D DFT with Butterworth frequency-domain filters, 7 morphological operations with union-find connected components, Non-Local Means denoising.
- **oximedia-simd** — AVX-512 SIMD kernels with runtime CPU feature detection (`CpuFeatures` dispatcher).
- **oximedia-transcode** — 9 platform presets (YouTube, Netflix, Twitch, Vimeo, Instagram, TikTok, Broadcast, Archive, Web), VP9 CRF encoding, FFV1 lossless archive mode, TranscodeEstimator for time/size prediction, per-scene CRF adaptation, 6-rung quality ladder, HW acceleration config, Prometheus metrics export.
- **oximedia-dedup** — Perceptual hash (pHash), SSIM structural similarity, histogram comparison, feature-based matching, audio fingerprint dedup, metadata-based dedup (404 tests).
- **oximedia-search** — Real facet aggregation across 7 dimensions (codec, resolution, duration, format, date, tags, status) with 444 tests.
- **oximedia-core** — RationalTime with GCD/LCM arithmetic, PtsMediaTime 128-bit rebase for sub-sample precision, RingBuffer and MediaFrameQueue lock-free structures.
- **oximedia-lut** — Hald CLUT (identity generation + trilinear interpolation), 12 photographic presets (portra, velvia, tri-x, etc.), LutChainOps bake-to-33-cubed optimization.
- **oximedia-compat-ffmpeg** — 19-node FilterGraph parser, 75 codec and 30 format mappings, FfmpegArgumentBuilder for programmatic CLI construction.
- **oximedia-scaling** — EWA Lanczos elliptical weighted average resampling, FidelityFX CAS sharpening, half-pixel correction for chroma, per-title encoding ladder generator.
- **oximedia-auto** — Narrative arc detection (3-Act, Hero's Journey, Kishotenketsu), beat-synced automatic cuts, saliency-based reframing for aspect ratio adaptation.
- **oximedia-dolbyvision** — IPT-PQ color space transforms, CM v4.0 trim metadata with sloped curves, quickselect-based shot statistics, Dolby Vision XML import/export.
- **oximedia-collab** — Three-way merge for concurrent edits, Operational Transform primitives, presence and cursor tracking, snapshot-based branching.
- **oximedia-plugin** — SemVer dependency resolver, u32 bitmask capability sandbox, FNV-1a hash-based hot-reload detection.

### Fixed

- Facade crate (`oximedia`) now correctly re-exports all 108 crates with proper feature gating.
- WASM build target resolves all feature-gate incompatibilities for browser environments.
- PyPI publish workflow corrected for maturin 1.8.4, protoc binary URL, and macOS Intel runner matrix.

## [0.1.1] - 2026-03-10

### Added

- **FFmpeg CLI compatibility layer** — `oximedia-compat-ffmpeg` crate and `oximedia-ff` binary providing drop-in argument compatibility with FFmpeg CLI for common transcoding, streaming, and filter workflows.
- **OpenCV Python API compatibility** — `oximedia.cv2` submodule in `oximedia-py` exposing 18 modules aligned to the OpenCV Python API surface (imread, imwrite, resize, cvtColor, VideoCapture, VideoWriter, etc.).
- **MP4 demuxer complete implementation** — `probe` and `read_packet` fully implemented in `oximedia-container`, enabling reliable MP4/MOV source reading in transcode pipelines.
- **Transcode pipeline implementation** — end-to-end demux→filter→encode→mux pipeline in `oximedia-transcode`, connecting all processing stages with backpressure and async task scheduling.
- **Archive checksum real hash verification** — `oximedia-archive` now performs actual MD5, SHA-1, SHA-256, and xxHash digest verification (replacing placeholder stubs).
- **QR code watermarking** — ISO 18004 compliant QR code generation and embedding in `oximedia-watermark`, supporting data capacity modes 1–4 with Reed-Solomon error correction.
- **DCT-domain forensic watermarking** — Quantization Index Modulation (QIM) embedding and blind detection in `oximedia-watermark`, providing robust invisible watermarks surviving re-encoding.
- **Video deinterlacing** — Edge-Directed Interpolation (EDI) deinterlacer added to `oximedia-cv`, including bob, weave, and blend fallback modes.
- **Smart crop** — content-aware crop detection using saliency maps and face-priority weighting in `oximedia-cv`.
- **Super-resolution (EDI)** — single-frame and multi-frame SR upscaling in `oximedia-cv` via learned edge-directed interpolation.
- **GCS storage enhancements** — ACL management, signed URL generation (V4), CMEK encryption key association, and storage class transitions in `oximedia-cloud`.
- **NMF source separation** — Non-negative Matrix Factorisation based audio source separation in `oximedia-audio-analysis`.
- **CEA-608 subtitle parser** — Line 21 closed caption byte-pair decoding in `oximedia-subtitle`.
- **DVB subtitle parser** — ETSI EN 300 743 PES/segment parsing in `oximedia-subtitle`.
- **Plugin system** — `oximedia-plugin` crate providing `CodecPlugin` trait, `PluginRegistry`, `StaticPlugin` builder, `declare_plugin!` macro, JSON manifests, and `dynamic-loading` feature gate for shared library support.
- **FFV1 codec** — Lossless video codec (decoder + encoder) in `oximedia-codec` with range coder, Golomb-Rice coding, and multi-plane support.
- **Y4M container** — Raw YUV sequence format (demuxer + muxer) in `oximedia-container` for uncompressed video interchange.
- **JPEG-XL codec** — Next-generation image codec (decoder + encoder) in `oximedia-codec` with modular transform, entropy coding, and progressive decoding.
- **DNG image format** — Digital Negative RAW image support (reader + writer) in `oximedia-image` with TIFF/IFD parsing, CFA demosaicing, and color calibration.

### Changed

- Refactored 6 over-limit source files (super_resolution, denoise, grading, lut, delogo, ivtc) — each split below the 2000-line policy boundary using splitrs.
- Promoted 22 Alpha crates and 10 Partial crates to fuller implementation status.

## [0.1.0] - 2026-03-07

### Added

- Initial release of the oximedia workspace — a comprehensive professional media processing platform in pure Rust.

#### Core Infrastructure
- `oximedia-core` — foundational types, error handling, and shared abstractions for the entire workspace
- `oximedia-io` — unified I/O layer with async file and stream support
- `oximedia-codec` — audio/video codec abstractions and implementations
- `oximedia-container` — media container format support (MXF, MP4, MOV, MPEG-TS, MKV, etc.)
- `oximedia-simd` — SIMD-accelerated media processing primitives
- `oximedia-accel` — hardware acceleration abstractions (GPU, FPGA, DSP)
- `oximedia-gpu` — GPU compute pipelines for media processing

#### Audio Processing
- `oximedia-audio` — core audio processing primitives and pipelines
- `oximedia-audio-analysis` — audio analysis including rhythm, tempo, and spectral features
- `oximedia-audiopost` — post-production audio tools (mixing, mastering, restoration)
- `oximedia-effects` — audio effects processing (chorus, reverb, EQ, dynamics)
- `oximedia-metering` — broadcast-grade audio metering (LUFS, LRA, peak, PPM)
- `oximedia-mixer` — multi-channel audio mixing and routing
- `oximedia-normalize` — audio normalization to broadcast standards
- `oximedia-mir` — music information retrieval and audio fingerprinting (AcoustID)

#### Video Processing
- `oximedia-cv` — computer vision and image analysis with super-resolution support
- `oximedia-vfx` — visual effects compositing and processing
- `oximedia-image` — image processing and format conversion
- `oximedia-lut` — LUT (Look-Up Table) processing for color grading
- `oximedia-colormgmt` — ICC color management and color space conversion
- `oximedia-dolbyvision` — Dolby Vision HDR metadata processing
- `oximedia-scopes` — broadcast video scopes (waveform, vectorscope, histogram)
- `oximedia-denoise` — video and audio denoising algorithms
- `oximedia-stabilize` — video stabilization
- `oximedia-scaling` — high-quality video scaling and resizing
- `oximedia-watermark` — digital watermarking

#### Graph and Pipeline
- `oximedia-graph` — media processing graph/pipeline engine
- `oximedia-edit` — non-linear editing operations
- `oximedia-timeline` — timeline management and sequencing
- `oximedia-timecode` — SMPTE timecode parsing, generation, and arithmetic
- `oximedia-timesync` — clock synchronization and PTP/NTP support
- `oximedia-clips` — clip management and media bin
- `oximedia-shots` — shot detection and scene segmentation
- `oximedia-scene` — scene analysis and classification

#### Transcoding and Conversion
- `oximedia-transcode` — multi-format transcoding pipeline
- `oximedia-convert` — universal media format conversion
- `oximedia-packager` — DASH/HLS adaptive streaming packaging
- `oximedia-proxy` — proxy media generation and management
- `oximedia-optimize` — media optimization for delivery targets
- `oximedia-batch` — batch processing job management
- `oximedia-renderfarm` — distributed render farm coordination

#### Distributed and Cloud
- `oximedia-distributed` — distributed encoding coordinator with consensus, leader election, and work stealing
- `oximedia-farm` — production-grade encoding farm job management and worker coordination
- `oximedia-jobs` — job scheduling and queue management
- `oximedia-cloud` — cloud storage and processing integration
- `oximedia-storage` — cloud storage abstraction (S3, Azure Blob, Google Cloud Storage)
- `oximedia-workflow` — media workflow automation and orchestration
- `oximedia-automation` — event-driven automation and rules engine

#### Networking
- `oximedia-net` — network transport protocols for media (RTP, RTMP, SRT, RIST)
- `oximedia-ndi` — NDI (Network Device Interface) protocol support
- `oximedia-server` — media server with WebSocket and HTTP APIs
- `oximedia-videoip` — video-over-IP transport (ST 2110, ST 2022)
- `oximedia-routing` — software-defined media routing and signal routing
- `oximedia-switcher` — live production switcher functionality
- `oximedia-playout` — broadcast playout automation

#### Quality and Analysis
- `oximedia-qc` — automated quality control and validation
- `oximedia-quality` — perceptual quality metrics (VMAF, SSIM, PSNR)
- `oximedia-analysis` — comprehensive media analysis and reporting
- `oximedia-monitor` — real-time media monitoring and alerting
- `oximedia-forensics` — media forensics and chain-of-custody tools
- `oximedia-dedup` — media deduplication and similarity detection
- `oximedia-profiler` — GPU and CPU profiling for media workloads

#### Metadata and Rights
- `oximedia-metadata` — media metadata extraction, editing, and standards (XMP, ID3, etc.)
- `oximedia-rights` — digital rights management metadata
- `oximedia-drm` — DRM encryption and key management
- `oximedia-access` — accessibility features (audio description generation)
- `oximedia-captions` — caption and subtitle processing
- `oximedia-subtitle` — subtitle format parsing and conversion

#### Format-Specific
- `oximedia-aaf` — AAF (Advanced Authoring Format) support
- `oximedia-edl` — EDL (Edit Decision List) parsing and generation
- `oximedia-imf` — IMF (Interoperable Master Format) support
- `oximedia-lut` — LUT format support (cube, 3dl, etc.)

#### Advanced Features
- `oximedia-align` — audio/video alignment and synchronization
- `oximedia-calibrate` — camera and display calibration tools
- `oximedia-collab` — collaborative editing and review workflows
- `oximedia-conform` — media conform and EDL-to-media matching
- `oximedia-gaming` — game capture and streaming integration
- `oximedia-graphics` — graphics overlay and titling
- `oximedia-mam` — Media Asset Management integration
- `oximedia-multicam` — multi-camera editing and synchronization
- `oximedia-playlist` — playlist management and scheduling
- `oximedia-presets` — encoding and processing preset management
- `oximedia-recommend` — AI-powered encoding parameter recommendation
- `oximedia-repair` — media repair and error concealment
- `oximedia-restore` — media restoration and archival tools
- `oximedia-review` — collaborative review and approval workflows
- `oximedia-search` — full-text and semantic media search
- `oximedia-virtual` — virtual production tools
- `oximedia-archive` — media archiving and long-term preservation
- `oximedia-archive-pro` — advanced archival formats and migration

#### Tooling
- `oximedia-bench` — benchmarking harnesses for media processing
- `oximedia-py` — Python bindings via PyO3
- `oximedia-wasm` — WebAssembly bindings
- `oximedia-cli` — command-line interface

[0.2.1]: https://github.com/cool-japan/oximedia/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/cool-japan/oximedia/compare/v0.1.9...v0.2.0
[0.1.9]: https://github.com/cool-japan/oximedia/compare/v0.1.8...v0.1.9
[0.1.2]: https://github.com/cool-japan/oximedia/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/cool-japan/oximedia/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/cool-japan/oximedia/releases/tag/v0.1.0

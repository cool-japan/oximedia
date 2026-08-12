# oximedia-audio TODO

## Current Status
- 100+ source files across codecs (Opus, Vorbis, FLAC, MP3, PCM), DSP (biquad, compressor, delay, EQ, limiter, reverb), effects (chorus, flanger, phaser, LFO), spectrum (FFT, analyzer, spectrogram, waveform, features), fingerprint (constellation, hash, matching, database), loudness (EBU R128, ATSC A/85, K-weighting, gating, true peak), meters (VU, PPM, peak, RMS, correlation, goniometer, Dolby, ITU), spatial (ambisonics, binaural, panning, reverb), description (ducking, mixing, synthesis, timing, metadata)
- `AudioDecoder`/`AudioEncoder` traits, `AudioFrame`, `ChannelLayout`, `Resampler`
- Feature-gated codecs: opus, vorbis, flac, mp3, pcm
- Dependencies: oximedia-core, oxifft, bytes (Pure Rust; rubato/audioadapter removed in favor of the in-crate windowed-sinc polyphase resampler)

## Enhancements
- [x] Add gapless playback support with proper encoder delay/padding handling in codec traits
- [x] Implement true peak limiter in `loudness/peak` with 4x oversampled detection
- [x] Add multi-band compressor in `compressor` (crossover network + per-band compression)
- [x] Implement look-ahead delay in `compressor` and `gate` for attack anticipation
- [x] Add wet/dry mix parameter to all `effects` (chorus, flanger, phaser)
- [x] Implement sidechain input for `compressor` and `gate` (external key signal)
- [x] Add auto-gain in `loudness/normalize` to maintain consistent output level after processing (verified 2026-05-16; src/auto_gain.rs:560 lines AutoGain)
- [x] Implement `Resampler` quality presets (draft/good/best) mapping to windowed-sinc filter configurations (Pure-Rust engine, 2026-07-08)
- [x] Add `AudioFrame` format conversion utilities (interleaved <-> planar, bit depth conversion)
- [x] Implement FLAC encoder compression level parameter (0-8) in `flac/encoder` (verified 2026-05-16; src/flac/encoder.rs:24 CompressionLevel(u8) 0-8, with_compression_level:150)

## New Features
- [x] Add AAC decoder (patent-free since 2023) as feature-gated module (verified 2026-05-16; src/aac.rs:734 lines AacDecoder)
- [x] Implement ALAC (Apple Lossless) decoder for Apple ecosystem compatibility (verified 2026-05-16; src/alac.rs:612 lines AlacDecoder)
- [x] Add WAV file reader/writer with full RIFF chunk handling (verified 2026-05-16; src/wav.rs:790 lines WavReader/WavWriter)
- [x] Implement audio watermarking module (embed/detect inaudible watermarks) (AudioWatermarker + AudioDetector done)
- [x] Add noise reduction module (spectral subtraction, Wiener filter) (verified 2026-05-16; src/noise_reduce.rs:638 lines NoiseReducer)
- [x] Implement click/pop removal for vinyl restoration workflows (verified 2026-05-16; src/click_remove.rs:506 lines ClickRemover)
- [x] Add convolution reverb using impulse response loading (verified 2026-05-16; src/convolution_reverb.rs:567 lines ConvolutionReverb)
- [x] Implement graphic equalizer (31-band ISO standard) using `biquad` banks (verified 2026-05-16; src/graphic_eq.rs:581 lines GraphicEq)
- [x] Add audio ducking module (auto-duck music under voiceover) (verified 2026-05-16; src/ducking.rs:557 lines AudioDucker)
- [x] Implement Dolby Atmos object metadata parsing for spatial audio rendering (verified 2026-05-16; src/dolby_atmos.rs:962 lines DolbyAtmosParser)

## Performance
- [x] Replace `rustfft` with OxiFFT per COOLJAPAN Policy
- [x] Add SIMD-optimized sample format conversion in `format_convert` (verified 2026-05-16; src/format_convert.rs:168 SIMD-optimised batch conversion, auto-vectorised chunks of 8)
- [x] Implement lock-free ring buffer for real-time audio threading in `stream_buffer` (verified 2026-05-16; src/stream_buffer.rs:136 struct StreamBuffer)
- [x] Optimize `biquad` filter with direct form II transposed for better numerical behavior (implemented 2026-05-15; src/dsp/biquad.rs BiquadDf2t struct, 2 delay elements, IR matches DF1 to 1e-12)
- [x] Add batch processing mode to `meters` (process multiple channels simultaneously) (src/meters/batch.rs:57 BatchMeterConfig, BatchMeterProcessor; the 2026-05-16 "verified" note was wrong — the file was never declared in `meters/mod.rs`, so neither it nor its 12 tests were compiled. Wired in and re-exported 2026-07-29; `process_channel` now returns `AudioResult<()>` instead of `Result<(), ()>`.)
- [x] Implement FFT plan caching in `spectrum/fft` to avoid repeated planner allocation (verified 2026-05-16; src/spectrum/fft_cache.rs:52 struct FftPlanCache, hit/miss counters)
- [x] Optimize Vorbis MDCT with split-radix algorithm in `vorbis/mdct` (implemented 2026-05-15; src/vorbis/mdct.rs MdctFast struct, FFT-based O(N log N) forward+inverse via oxifft)

## Orphan meter modules (assessed 2026-07-29, deliberately not wired)

Both files exist under `src/meters/` but are **not** declared in
`meters/mod.rs`, so they are not compiled. Nothing is deleted; wiring either one
requires the work below plus conformance tests, otherwise it would ship a
loudness reading that is quietly wrong.

- [ ] `src/meters/itu.rs` (1107 lines) — revive or retire. Blockers, in order:
  (1) two `E0502` borrow errors in `KWeightingFilter::process` (`&self.pre_b`
  immutable + `&mut self.pre_state[ch]` mutable in one call) — fixable by making
  `process_biquad` an associated fn; (2) `Bs1770Meter::channel_weights` is
  computed in `new()` and **never applied** — `SlidingWindow::calculate_loudness`
  sums unweighted channel mean squares, so 5.1 meters LFE at weight 1.0 instead
  of 0.0 and the surrounds at 1.0 instead of 1.41; (3) `ItuMeter::true_peak` is a
  plain sample-peak detector with no oversampling, yet is reported as
  `true_peak_dbtp` and feeds `check_streaming_compliance`; (4)
  `LoudnessRangeCalculator` applies no EBU Tech 3342 −20 LU relative gate, so it
  over-reports LRA on material with quiet tails; (5) dead fields
  (`SlidingWindow::overlap`, `BlockMeasurement::timestamp`,
  `ItuMeter::{sample_rate, channels}`) hidden only by the crate-wide
  `#![allow(dead_code)]`; (6) zero tests. Also note it would be the **third**
  BS.1770-4 meter in the workspace, after `crate::loudness` and
  `oximedia_metering::ebu_r128_impl` — pick one to keep. The genuinely
  non-duplicated parts are the pure data tables `Bs1864Practice` (target level /
  tolerance per programme type) and `Bs2217Streaming` (per-platform targets);
  extracting just those into a small `meters/targets.rs` is the cheap win.
- [ ] `src/meters/dolby.rs` (944 lines) — sketch, not an implementation. Blockers:
  (1) `AWeightingFilter::new` ignores its `sample_rate` argument and hard-codes
  one unstated sample rate's coefficients ("simplified for this implementation …
  in practice, would use proper IIR filter design"), so `LeqAMeter` is wrong at
  every rate but one; (2) `MWeightingFilter` is two hand-tuned biquads with
  round-number coefficients labelled "Simplified M-weighting … in practice,
  M-weighting is more complex"; (3) `SpeechDetector::geometric_mean` takes the
  `product()` of every sample in a 25 ms frame (1200 values < 1.0) before the
  `powf(1/n)`, which underflows to 0.0 — the spectral-flatness term is therefore
  constant and `detect_speech` reduces to energy + ZCR thresholds; (4)
  `calculate_spectral_flatness` is a time-domain stand-in ("in a full
  implementation, would use FFT"); (5) zero tests. `dialnorm` derived from this
  detector would be unusable. Rewrite against real A-weighting (IEC 61672-1
  bilinear design at the actual sample rate) and a real spectral front-end, or
  drop the file.

## Testing
- [x] Add FLAC round-trip test: encode -> decode -> bit-exact comparison (8 tests in tests/conformance_tests.rs)
- [ ] Test Opus encoder/decoder with ITU-T P.862 PESQ-like quality metric (requires external PESQ library; deferred)
- [x] Add `loudness` EBU R128 conformance test with EBU test signals (8 tests in tests/conformance_tests.rs)
- [x] Fix `loudness/gate.rs` power double-weighting bug + golden calibration (2026-06-05; corrected golden K-weighting term 2026-06-06; `calculate_block_power`/`calculate_block_power_planar` divided by `weight_sum*frames`, double-counting the channel weights already folded into `power_sum` — understated loudness by ~10·log10(frames). Divisor corrected to `frames` only — crates/oximedia-audio/src/loudness/gate.rs:115 + :159. Golden 1 kHz tests in tests/conformance_tests.rs run the FULL K-weighted `R128Meter` path; the BS.1770-4 K-weighting magnitude at 1 kHz/48 kHz is **+3.4554 dB** (|H_K|²=2.21586, curve crosses 0 dB near ~2 kHz, NOT 1 kHz), so the golden absolutes are unweighted+3.4554: mono amp 1.0→−0.25, 0.5→−6.27, 0.1→−20.25 LUFS; stereo channel-sum Δ=+3.0103 LU (K-weighting cancels in the diff) → −17.24 LUFS abs; silence/near-silent→−∞; direct `GatingProcessor::calculate_block_power` divisor lock bypasses K-weighting (mono a², stereo 2·a²). Verified: all 7 R128 golden + 7 transcode ebu_r128_conformance tests pass.)
- [x] Test `meters/vu` ballistics against IEC 60268-10 specified rise/fall times (8 tests in tests/conformance_tests.rs)
- [x] Test `spatial/ambisonics` encoding/decoding round-trip for 1st order (4 tests in tests/conformance_tests.rs)
- [x] Add `fingerprint` matching accuracy test with time-stretched and pitch-shifted audio (4 tests in tests/conformance_tests.rs)
- [x] Test `effects/chorus` with known LFO parameters and verify modulation depth (4 tests in tests/conformance_tests.rs)

## Documentation
- [x] Document codec feature gates and their compile-time implications (implemented 2026-05-15; lib.rs feature gate table)
- [x] Add DSP signal flow diagrams for compressor, reverb, and EQ chains (implemented 2026-05-15; dsp/compressor.rs, dsp/reverb.rs, dsp/eq.rs signal flow ASCII art)
- [x] Document `AudioFrame` memory layout and channel ordering conventions (implemented 2026-05-15; frame.rs module + struct doc)

## 0.1.8 Wave 4 follow-up (added 2026-05-29 by /ultra)

- [x] Implement FLAC decoder full orchestration — wire `FlacDecoder::receive_frame/send_packet` through existing flac/ primitives (planned 2026-05-29)
  - **Goal:** `FlacDecoder::receive_frame()` currently returns `Ok(None)` stub. All bitstream primitives exist (`frame.rs:339 FrameHeader::parse`, `subframe.rs: decode_fixed/lpc/verbatim/constant`, `rice.rs: RiceDecoder`, `crc.rs: crc8/crc16`). Wire them into a streaming decoder that emits `AudioFrame` per FLAC frame. Lossless round-trip through `FlacEncoder`.
  - **Design:** Add `FlacStream { stream_info: Option<StreamInfo>, buffer: Vec<u8>, pending_frames: VecDeque<AudioFrame>, last_pts: i64 }`. `send_packet` appends bytes, parses STREAMINFO metadata block on first call (detect "fLaC" marker, BLOCK_TYPE==0). `receive_frame` pumps `try_decode_one_frame`: locate 14-bit sync code `11111111 111110xx`, parse FrameHeader, decode per-channel subframes (constant/verbatim/fixed/LPC), apply channel decorrelation (Independent/LeftSide/RightSide/MidSide), verify CRC16, convert int samples to AudioFrame PlanarF32. PTS from frame_or_sample_number × sample_rate. Optional MD5 stream verification (build `flac/md5.rs` ~80 LoC if not present).
  - **Files:** `crates/oximedia-audio/src/flac/mod.rs` (replace stub lines 178–180), `crates/oximedia-audio/src/flac/decoder.rs` (new, ~500–800 LoC orchestration), `crates/oximedia-audio/src/flac/md5.rs` (new if needed, ~80 LoC)
  - **Tests:** `test_flac_decode_synth_16bit_stereo` (round-trip via FlacEncoder), `test_flac_decode_synth_24bit_mono`, `test_flac_decode_left_side_decorrelation`, `test_flac_decode_mid_side_decorrelation`, `test_flac_decode_lpc_subframe`, `test_flac_decode_rice_partitions`, `test_flac_decode_crc_mismatch_rejects`, `test_flac_decode_md5_match`, `test_flac_decode_send_partial_packets`, `test_flac_decoder_audio_decoder_trait`
  - **Risk:** Variable-blocksize block-size codes; wasted-bits unary off-by-one (cross-check with FlacEncoder mirror). MD5 if not present: standalone Pure-Rust impl ~80 LoC.

- [x] Implement Vorbis decoder full orchestration — three-header state machine + per-packet floor1/residue/IMDCT/overlap-add pipeline (planned 2026-05-29)
  - **Goal:** `VorbisDecoder::receive_frame()` returns `Ok(None)` stub. All primitives present (`codebook.rs: decode/decode_vq`, `floor.rs: Floor::synthesize`, `mdct.rs: Mdct::inverse`, `bitpack.rs`, `header.rs: parse`). Wire them into a Vorbis I compliant decoder. ≤ -50 dBFS RMS error round-trip through VorbisEncoder.
  - **Design:** State machine `WaitingForIdentification → Comment → Setup → Ready`. Identification: stash sample_rate/channels/blocksize_0/1. Comment: skip. Setup: parse codebooks/floors/residues/mappings/modes. Audio packet: read mode number → window_size; per submap: decode floor1 curve, decode residue (types 0/1/2), apply residue to spectrum; undo channel coupling (mag/ang → L/R per Vorbis I §6.3.2 sign rules); apply floor multiplicatively; IMDCT via Mdct::inverse; window + overlap-add with self.overlap[ch]; emit AudioFrame. Add `decode_residue1/2` + `apply_residue` to `vorbis/residue.rs`. Overlap stored as `Vec<Vec<f32>>` per channel.
  - **Files:** `crates/oximedia-audio/src/vorbis/mod.rs` (replace stub lines 127–129), `crates/oximedia-audio/src/vorbis/decoder.rs` (new, ~600–800 LoC state machine + orchestration), `crates/oximedia-audio/src/vorbis/residue.rs` (add decode_residue1/2 + apply_residue)
  - **Tests:** `test_vorbis_decode_synth_440hz_stereo`, `test_vorbis_decode_mono_short_block`, `test_vorbis_decode_mono_long_block`, `test_vorbis_decode_long_then_short` (overlap-add seam), `test_vorbis_decode_residue_type1`, `test_vorbis_decode_residue_type2_coupled_stereo`, `test_vorbis_decode_state_machine_rejects_audio_before_setup`, `test_vorbis_decoder_audio_decoder_trait`
  - **Risk:** Residue type 2 + channel coupling is densest path — silent corruption risk; ground-truth from VorbisEncoder round-trip. IMDCT window alignment must match encoder's Vorbis I §1.3.2 "vorbis_window".

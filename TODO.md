# OxiMedia — The Sovereign Media Framework: Development Roadmap

**Version: 0.1.6 (active) / 0.1.5 (stable)**
**Status as of: 2026-04-26**
**Total SLOC: ~2,687,000 (Rust)**
**Total Tests: 81,582 passing (cargo nextest run --workspace --all-features)**
**Total Crates: 108**
**Crate Status: 108 Stable / 0 Alpha / 0 Partial**
**Current Branch: 0.1.6 — Stub Resolution, Codec Improvements, Dependency Upgrades**

---

## Summary

| Category | Count | Notes |
|----------|-------|-------|
| Stable crates | 108 | All crates fully stabilized; no `todo!()`/`unimplemented!()` stubs |
| Alpha crates | 0 | All former alpha crates promoted to stable |
| Partial crates | 0 | All former partial crates completed and promoted to stable |

---

## Phase 1: Foundation [COMPLETE]

- [x] Workspace structure with workspace-level dependency management
- [x] `oximedia-core` — `Rational`, `Timestamp`, `PixelFormat`, `SampleFormat`, `CodecId`, `MediaType`, `OxiError`, `BufferPool`, decoder/demuxer trait definitions
- [x] `oximedia-io` — `MediaSource` async trait, `FileSource`, `MemorySource`, `BitReader`, Exp-Golomb coding, aligned I/O, buffer pool, checksum, chunked writer, compression, file metadata/watch, I/O pipeline/stats, mmap, progress reader, rate limiter, scatter-gather, seekable, splice pipe, temp files, verify I/O, write journal
- [x] `oximedia-container` — `ContainerFormat`, `Packet`, `StreamInfo`, `CodecParams`, format probe; Matroska/WebM full demux+mux, Ogg, FLAC, WAV/RIFF, MP4/ISOBMFF (AV1/VP9 only); chapters, cue, edit lists, fragment/CMAF, media header, metadata editor, MPEG-TS demux+mux, sample table, seek, streaming demux+mux, timecode track, track header, track manager/mapping/selector
- [x] `oximedia` facade crate with prelude
- [x] `oximedia-cli` — probe, info, transcode commands
- [x] Zero warnings policy enforced across all stable crates

---

## Phase 2: Codec Implementation [COMPLETE]

### Video Codecs

- [x] **AV1** — OBU parsing, sequence header, `Av1Decoder`, `Av1Encoder`, loop filter, CDEF, quantization/dequantization tables, transform types (DCT/ADST/FLIPADST/IDTX), symbol reader/writer with CDF updates
- [x] **VP9** — superframe parsing, uncompressed header, `Vp9Decoder` with reference frame management, probability tables, partition types, 8-tap interpolation
- [x] **VP8** — boolean arithmetic decoder, frame header, `Vp8Decoder`, 4x4 DCT/IDCT, Walsh-Hadamard transforms, quarter-pixel motion compensation, deblocking loop filter
- [x] `oximedia-codec` — entropy coding, SIMD/ARM NEON path, tile encoder
- [x] Shared: intra prediction, motion estimation (Diamond/Hexagon/UMH/Hierarchical), rate control (CQP/CBR/VBR/CRF)
- [x] SIMD abstraction layer with scalar fallback (`oximedia-accel`, `oximedia-simd`)

### Audio Codecs

- [x] **Opus** — RFC 6716 packet parsing, range/arithmetic decoder, SILK/CELT/hybrid mode skeleton
- [x] **Vorbis** — full decoder
- [x] **FLAC** — metadata blocks, frame header, full decoder
- [x] **PCM** — encoder/decoder
- [x] Audio frame infrastructure; resampling (`oximedia-audio`)

---

## Phase 3: Filter Pipeline [COMPLETE]

- [x] `oximedia-graph` — `FilterGraph` DAG, `GraphBuilder` type-state pattern, `Node` trait, topological sort, cycle detection, graph merge, metrics graph, optimization
- [x] Video filters: scale, crop, pad, color conversion (BT.601/709/2020), FPS, deinterlace, overlay, delogo, denoise, grading, IVTC, LUT, timecode burn, tone map
- [x] Audio filters: resample, channel mix, volume/fade, normalize (Peak/RMS/EBU R128), parametric EQ, compressor/limiter, delay with feedback
- [x] `oximedia-effects` — auto-pan, barrel lens, chorus, color grade, composite, compressor look, ducking, EQ, flanger, luma key, reverb (hall/room), saturation, spatial audio, tape echo, time stretch, tremolo, vibrato; video: blend, chromakey, chromatic aberration, grain, lens flare, motion blur, vignette

---

## Phase 4: Computer Vision [COMPLETE]

- [x] `oximedia-cv` — image resize/color conversion/histogram/blur/edge detection, corner/face/motion detection, optical flow, KCF/CSRT/MOSSE/MedianFlow trackers, chroma key (auto/composite), contour, depth estimation, YOLO detection, super-resolution, denoising, interlacing/telecine handling, interpolation, keypoints, ML preprocessing, morphology, motion blur synthesis, motion vectors, pose estimation, quality metrics (PSNR/SSIM), scene histogram/motion, video stabilization (motion/transform), superpixel
- [x] `oximedia-scene` — scene graph, aesthetic scoring, content/mood/quality/shot-type/color-palette classification, composition rules, saliency detection, scene stats, storyboard, visual rhythm
- [x] `oximedia-shots` — shot detector, cut/dissolve/fade/wipe detection, camera movement, angle/composition/shot-type classification, shot grouping/matching/palette/stats/tempo, storyboard, metrics, duration analysis
- [x] `oximedia-quality` — PSNR, SSIM, MS-SSIM, VMAF, VIF, FSIM, NIQE, BRISQUE, blockiness, blur, noise, flicker score, perceptual model, scene quality, temporal quality, quality preset/report, aggregate score, batch processing, reference comparison
- [x] `oximedia-scopes` — waveform, vectorscope, histogram, parade, CIE, false color, focus, audio scope, bit-depth scope, clipping detector, compliance, RGB balance, stats

---

## Phase 5: Audio Analysis and Music Intelligence [COMPLETE]

- [x] `oximedia-audio-analysis` — beat, cepstral, echo detect, forensics (compression/noise), formant analysis, harmony, music rhythm/timbre, noise profile, onset, pitch tracking/vibrato, psychoacoustic, rhythm, source separation, spectral FFT frame/contrast/features/flux, stereo field, tempo analysis, transient detect/envelope, voice characteristics/speaker
- [x] `oximedia-mir` — audio features, beat/downbeat, chorus detect, acoustID fingerprint, harmonic analysis, instrument detection, MIR feature, pitch track, playlist, segmentation, source separation, spectral contrast/features, structure analysis/segmentation, tempo estimate, utils, vocal detect
- [x] `oximedia-metering` — EBU R128 loudness, correlation, VU meter
- [x] `oximedia-normalize` — DC offset, DRC, gain schedule, loudness history, multi-channel loudness, multipass, noise profile, normalize report, processor, realtime, ReplayGain, spectral balance

---

## Phase 6: Networking and Streaming [COMPLETE*]

*One `todo!()` stub remains in the ABR path — see Known Issues.

- [x] `oximedia-net` — HLS playlist/segment, DASH MPD/client/segment, RTMP (AMF/chunk/client/handshake/message), SRT (crypto/key exchange/monitor/packet/stream), WebRTC (DTLS/ICE/ICE agent/peer connection/RTCP/RTP/SCTP/SDP/SRTP/STUN/data channel), CDN failover/metrics, connection pool, DASH live (chunked/DVR/timeline), live analytics, live DASH/HLS servers, multicast, packet buffer, QoS monitor, session tracker, SMPTE ST 2110 (ancillary/audio/PTP/RTP/SDP/timing/video), stream mux
- [x] `oximedia-packager` — HLS and DASH packagers, CMAF, MPD, DRM info, encryption, ladder, multivariant, playlist generator, segment index/list, bandwidth estimator, bitrate calc
- [x] `oximedia-server` — access log, API versioning, audit trail, auth middleware, cache, circuit breaker, config loader, connection pool, DVR buffer, health monitor, library, middleware, rate limit, request log/validator, response cache, session, WebSocket handler

---

## Phase 7: Production and Broadcast Infrastructure [COMPLETE]

- [x] `oximedia-playout` — ad insertion, API, automation, branding, catchup, CG, channel, compliance ingest, content, device, failover, frame buffer, gap filler, graphics, ingest, media router, monitoring, output/output router, playback, playlist/playlist ingest, playout schedule, schedule block/slot, scheduler, secondary events, signal chain
- [x] `oximedia-playlist` — automation/playout, backup failover/filler, clock offset, duration calc, EPG/XMLTV, history, live insert, M3U, metadata (as-run/track), multichannel manager, archive, merge, priority, rotation, stats, queue manager, schedule engine/recurrence, scheduler, shuffle
- [x] `oximedia-routing` — automation timeline, bandwidth budget, channel extract/split, audio embed/deembed, flow graph/validate/visualize, gain stage, latency calc, link aggregation, MADI interface, matrix crosspoint/solver, NMOS, patch bay/input/output, path selector, preset manager, redundancy group, route audit/optimizer/preset/table, routing policy, signal monitor/path, traffic shaper
- [x] `oximedia-switcher` — audio follow/mixer, aux bus, bus, clip delay, crosspoint, FTB control, input/input bank/input manager, keyer, macro engine/exec/system, M/E bank, media player/pool, multiviewer, preview bus, snapshot recall, still store, super source, switcher preset, sync, tally, transition/transition lib
- [x] `oximedia-mam` — API, asset, asset collection/relations/search/tag index, audit, batch ingest, bulk operation, catalog search, collection/manager, database, delivery log, folder hierarchy, ingest/pipeline/workflow, media catalog/format info/linking/project, proxy, retention policy, rights summary, search, storage, transcoding profile, transfer manager, usage analytics, version control/versioning, webhook, workflow/integration
- [x] `oximedia-farm` — communication service (render farm orchestration)

---

## Phase 8: Extended Capabilities [COMPLETE]

- [x] `oximedia-lut` — LUT builder, chromatic, color cube, colorspace, cube writer, CSP/Cube/3DL formats, identity LUT, LUT 3D, fingerprint, gradient, I/O, provenance, validate, version, matrix, temperature
- [x] `oximedia-metadata` — embed, EXIF parse, ID3v2, IPTC IIM, linked data, media metadata, metadata history/index/sanitize/stats/template, provenance, schema registry, Vorbis
- [x] `oximedia-image` — blend mode, channel ops, crop region, depth map, dither engine, edge detect, EXIF parser, ICC embed, XMP metadata, pattern, pyramid, raw decode, sequence, thumbnail cache, tone curve
- [x] `oximedia-cv` (advanced) — full tracking suite, chroma key, interlace/telecine, pose estimation, superpixel, ML preprocessing
- [x] `oximedia-recommend` — A/B test, calibration, collaborative filter/predict/SVD, content-based, context signal, explanation, feature store, feedback signal, history tracking, impression tracker, item similarity, profile/preference, rank/score, explicit rating, score cache, session, trending detect
- [x] `oximedia-search` — audio fingerprint/match, color search, face search, facet aggregator, OCR search, query parser, search cluster/filter/history/pipeline/ranking/result/rewrite/snapshot/suggest, text search, visual features/index/search
- [x] `oximedia-distributed` — distributed transcoding coordination
- [x] `oximedia-cloud` — cloud storage and processing abstraction
- [x] `oximedia-gpu` — GPU compute abstraction layer
- [x] `oximedia-colormgmt` — color management pipeline
- [x] `oximedia-dolbyvision` — Dolby Vision metadata handling
- [x] `oximedia-drm` — DRM key management
- [x] `oximedia-forensics` — media forensics analysis
- [x] `oximedia-gaming` — game capture and streaming
- [x] `oximedia-imf` — IMF package support
- [x] `oximedia-aaf` — AAF interchange format
- [x] `oximedia-edl` — EDL parse/generate
- [x] `oximedia-captions` — caption processing pipeline
- [x] `oximedia-dedup` — media deduplication
- [x] `oximedia-compat-ffmpeg` — FFmpeg CLI argument compatibility layer (80+ codec mappings, filter graph lexing, stream specifiers)
- [x] `oximedia-plugin` — Dynamic codec plugin system (CodecPlugin trait, PluginRegistry, StaticPlugin, declare_plugin! macro, JSON manifests, dynamic-loading feature gate)

---

## Phase 9: Hardening and Stabilization [COMPLETE]

All 42 non-stable crates (10 partial + 31 alpha + 1 stub) have been fully implemented, tested, and promoted to stable status.

### 9.1 Partial Crates — All Completed and Stable

| Crate | Status | Resolution |
|-------|--------|------------|
| `oximedia-mixer` | Stable | Audio/video mixing engine complete; sub-frame accuracy implemented |
| `oximedia-multicam` | Stable | Multi-camera sync, ISO recording, angle switching implemented |
| `oximedia-optimize` | Stable | Pipeline optimizer and auto-tune encode parameters complete |
| `oximedia-profiler` | Stable | Flamegraph integration, GPU memory profiling, regression detection complete |
| `oximedia-py` | Stable | PyO3 bindings complete (94 modules); requires `maturin build` for Python runtime |
| `oximedia-renderfarm` | Stable | Distributed render coordination, deadline scheduler, cloud burst complete |
| `oximedia-restore` | Stable | Audio restoration (click/crackle/hiss/hum removers), telecine detect, pitch correct complete |
| `oximedia-scaling` | Stable | Content-aware scale, tile, pad logic complete |
| `oximedia-storage` | Stable | Storage backend abstraction, tier management, LRU eviction complete |
| `oximedia-watermark` | Stable | Perceptual watermark embed/detect and forensic marking complete |

### 9.2 Alpha Crates — All Stabilized

All 22 former alpha crates have been audited, documented, tested, and promoted to stable:

| Crate | Status |
|-------|--------|
| `oximedia-mir` | Stable |
| `oximedia-ndi` | Stable |
| `oximedia-recommend` | Stable |
| `oximedia-repair` | Stable |
| `oximedia-review` | Stable |
| `oximedia-rights` | Stable |
| `oximedia-routing` | Stable |
| `oximedia-scene` | Stable |
| `oximedia-scopes` | Stable |
| `oximedia-search` | Stable |
| `oximedia-shots` | Stable |
| `oximedia-simd` | Stable |
| `oximedia-stabilize` | Stable |
| `oximedia-subtitle` | Stable |
| `oximedia-switcher` | Stable |
| `oximedia-timecode` | Stable |
| `oximedia-timeline` | Stable |
| `oximedia-timesync` | Stable |
| `oximedia-transcode` | Stable |
| `oximedia-videoip` | Stable |
| `oximedia-virtual` | Stable |
| `oximedia-workflow` | Stable |

### 9.3 Remaining Stub

| Location | Stub | Priority |
|----------|------|----------|
| `oximedia-net/src/` | 1 `todo!()` in ABR (adaptive bitrate) — confirmed in doc-comment only, not executable code | None/Resolved |

---

## 0.1.2 Changes (2026-03-11)

| Item | Status |
|------|--------|
| **Facade crate** (`oximedia`) expanded: 59 lines → 408 lines, 4 → 29 crates exposed | ✅ Done |
| 25 feature flags added to facade crate + `full` meta-feature | ✅ Done |
| New `prelude.rs` with 211 lines of feature-gated re-exports | ✅ Done |
| New examples: `audio_metering`, `quality_assessment`, `timecode_operations` | ✅ Done |
| New examples: `dedup_detection`, `workflow_pipeline` | ✅ Done |
| Integration test suite added (`oximedia/tests/integration.rs`) | ✅ Done |
| `oximedia-dedup`: 6 stub methods fully implemented (pHash, SSIM, histogram, feature, audio fingerprint, metadata) | ✅ Done |
| `oximedia-search`: real facet aggregation implemented (7 dimensions: formats, codecs, durations, resolutions, dates, tags, bitrates) | ✅ Done |
| Workspace version bumped to 0.1.2 | ✅ Done |
| `oximedia-compat-ffmpeg`: migrated to workspace dep style | ✅ Done |
| New examples: `video_scopes`, `shot_detection` | ✅ Done |
| PyPI workflow: fixed 3 bugs (maturin version pinned to 1.8.4, protoc URL typo fixed, macOS Intel runner corrected) | ✅ Done |
| `pyproject.toml` version updated to 0.1.2 | ✅ Done |
| Facade crate extended: 49 → ~93 crates exposed (all ~93 workspace library crates), 60+ feature flags | ✅ Done |
| NMOS mDNS/DNS-SD auto-discovery: NmosDiscovery with builder, announce, browse — `nmos-discovery` feature | ✅ Done (605 tests) |
| 4 criterion benchmark suites: quality_metrics, audio_metering, format_probe, dedup_hash | ✅ Done |
| CLI: `oximedia loudness` and `oximedia quality` subcommands added | ✅ Done (306 tests) |
| CLI: `oximedia dedup` and `oximedia timecode` subcommands added | ✅ Done |
| Pre-existing CLI bugs fixed (archive_cmd, farm_cmd, search_cmd, presets doctest) | ✅ Done |
| NMOS IS-08 Audio Channel Mapping API implemented (`channel_mapping` module, 41 tests, 656 total routing tests) | ✅ Done |
| New examples: `nmos_registry`, `color_pipeline` | ✅ Done |
| `oximedia-simd`: AVX-512 paths and `CpuFeatures` runtime detection | ✅ Done |
| WASM build verified: `cargo check --target wasm32-unknown-unknown` clean, 505 tests pass | ✅ Done |
| NMOS IS-09 System API (global config, health endpoint, API version discovery, 36 tests) | ✅ Done |
| CLI: `normalize`, `batch-engine`, enhanced `scopes` subcommands (333 total CLI tests) | ✅ Done |
| oximedia-audio-analysis: chromagram, energy analysis, ISO 226 loudness curves (515 tests) | ✅ Done |
| oximedia-mir: Camelot codes, relative/parallel keys, genre classification (607 tests) | ✅ Done |
| NMOS IS-11 Stream Compatibility Management (CompatibilityRegistry, MediaCapability) | ✅ Done |
| oximedia-transcode: VP9 CRF, FFV1 archive, Opus FEC/DTX, FlacConfig, TranscodePreset, TranscodeEstimator | ✅ Done |
| CLI: `workflow`, `version` subcommands, enhanced `probe` output | ✅ Done |
| `oximedia-hdr`: scene-referred tone mapping with per-frame luminance analysis (`SceneReferredToneMapper`, `FrameLuminanceAnalysis`) | ✅ Done |
| `oximedia-hdr`: soft-clip gamut mapping with perceptual desaturation (`convert_soft_clip`, `convert_frame_soft_clip`) in `gamut.rs` | ✅ Done |
| `oximedia-hdr`: BT.2446 Method A and Method C tone mapping operators (`BT2446MethodAToneMapper`, `BT2446MethodCToneMapper`) | ✅ Done |
| `oximedia-hdr`: Dolby Vision RPU generation (`generate_rpu_nal`, `parse_rpu_nal_header`, `verify_rpu_crc`, `RpuGenerationConfig`) in `dolby_vision_profile.rs` | ✅ Done |
| `oximedia-hdr`: HLG system gamma adjustment for display peak luminance (`hlg_adapted_system_gamma`, `hlg_system_for_display`) in `hlg_advanced.rs` | ✅ Done |
| `oximedia-hdr`: MaxRGB and percentile luminance statistics for CLL auto-detect (`MaxRgbAnalyzer::percentile_nits`, `auto_detect_cll`) in `color_volume.rs` | ✅ Done |
| `oximedia-hdr`: inverse tone mapping SDR-to-HDR upconversion (`InverseToneMapper`, `InverseToneMappingOperator`) in `tone_mapping.rs` | ✅ Done |
| `oximedia-hdr`: `hdr_histogram.rs` module for luminance histogram analysis (`HdrHistogram`, `HdrHistogramAnalyzer`) | ✅ Done |
| `oximedia-hdr`: `display_model.rs` module for target display characterisation (`DisplayModel` with peak nits, black level, gamut) | ✅ Done |
| `oximedia-hdr`: `color_volume_transform.rs` ICtCp color space conversions BT.2100 (`rgb_to_ictcp`, `ictcp_to_rgb`, `ICtCpFrame`) | ✅ Done |
| `oximedia-hdr`: SIMD-optimized PQ EOTF/OETF batch computation (`pq_eotf_batch`, `pq_oetf_batch`, `pq_eotf_fast`, `pq_oetf_fast`) in `transfer_function.rs` | ✅ Done |
| `oximedia-hdr`: LUT-based fast path for PQ and HLG transfer functions (`PqEotfLut`, `PqOetfLut`, `HlgEotfLut`) in `transfer_function.rs` | ✅ Done |
| `oximedia-hdr`: parallel per-row tone mapping using rayon (`map_frame_parallel`, `tone_map_frame_rayon`) in `tone_mapping.rs` | ✅ Done |
| `oximedia-hdr`: 283 tests pass, zero warnings (2026-03-14) | ✅ Done |
| `oximedia-hdr` crate: HDR processing (PQ/HLG, tone mapping, gamut, HDR10+, DV profiles) | ✅ Done |
| `oximedia-spatial` crate: spatial audio (HOA, HRTF, VBAP, room sim, WFS) | ✅ Done |
| `oximedia-cache` crate: LRU, tiered, predictive warming, content-aware caching | ✅ Done |
| `oximedia-stream` crate: BOLA ABR, SCTE-35, segment lifecycle, multi-CDN | ✅ Done |
| `oximedia-video` crate: motion, deinterlace, interpolation, scene detect, pulldown | ✅ Done |
| `oximedia-cdn` crate: edge manager, cache invalidation, origin failover, geo routing | ✅ Done |
| `oximedia-neural` crate: tensor ops, conv2d, batch norm, activations, media models | ✅ Done |
| `oximedia-360` crate: equirectangular↔cubemap, fisheye, stereo 3D, spatial media XMP | ✅ Done |
| `oximedia-analytics` crate: session tracking, retention curves, A/B testing, engagement | ✅ Done |
| `oximedia-caption-gen` crate: speech alignment, Knuth-Plass, WCAG 2.1, diarization | ✅ Done |
| `oximedia-pipeline` crate: declarative media processing DSL, typed filter graph | ✅ Done |
| Dependency conflict resolved: rusqlite 0.32 + sqlx 0.8.6 (unified libsqlite3-sys) | ✅ Done |
| unwrap() eliminated: 1,386 calls across 119 files → 0 in production/test code | ✅ Done |
| Example collision fixed: quality_assessment renamed in oximedia-analysis | ✅ Done |
| pyo3 deprecation warnings suppressed with #![allow(deprecated)] | ✅ Done |
| rand 0.10 RngExt migration completed across all crates | ✅ Done |
| **70,807 tests passing**, 0 failures, 235 skipped | ✅ Done |

---

## Known Issues

| Priority | Crate | Issue | Status |
|----------|-------|-------|--------|
| None/Resolved | `oximedia-net` | `todo!()` confirmed in documentation comment only in ABR controller — not executable code, no runtime impact | Resolved |

---

## 0.1.4 Planned

Items confirmed for the 0.1.4 milestone. All are patent-free and qualify for the Green List.

| Item | Crate(s) | Notes |
|------|----------|-------|
| **MJPEG** — Motion JPEG codec (pure-Rust) | `oximedia-codec`, `oximedia-container`, `oximedia-core` | Baseline JPEG patents expired; fully royalty-free. Add `CodecId::Mjpeg`, a pure-Rust baseline JPEG encoder that emits independent intra-frames, a container muxer for AVI/MOV/MPEG-PS MJPEG streams, and `compat-ffmpeg` direct mapping (`-c:v mjpeg` pass-through). Existing stubs in `oximedia-multicam` (`ProxyCodec::Mjpeg`) and `oximedia-transcode` (`hwaccel`) will be wired to the real encoder. Tracked in GitHub issue #2. |
| **Animated JPEG-XL (AJXL)** | `oximedia-codec` | Extend the existing single-frame `oximedia-codec/src/jpegxl/` implementation: add `JxlAnimation` / `AnimationHeader` types, ISOBMFF `jxlp` frame-sequence serialization (loop count, `tps_numerator` / `tps_denominator`), and a streaming frame-iterator decode API (`impl Iterator<Item = DecodedImage>`). Tracked in GitHub issue #2. |
| **APV (Advanced Professional Video)** | `oximedia-codec`, `oximedia-container`, `oximedia-core` | ISO/IEC 23009-13, royalty-free intra-frame codec designed for professional production workflows. Hardware support emerging in cameras and NLEs. New codec crate module `oximedia-codec/src/apv/`; `CodecId::Apv` Green List entry; container support for APV-in-MXF/MOV. Tracked in GitHub issue #1. |

---

## 0.1.4 Tracking

Progress tracking for in-flight Wave items. `[~]` = in progress, `[x]` = complete.

- [x] MJPEG end-to-end wiring (CodecId, encoder, container muxer, CLI, compat-ffmpeg) — Wave 1 (2026-04-17)
- [x] APV end-to-end wiring (CodecId, encoder stub, container muxer, CLI, compat-ffmpeg) — Wave 1 (2026-04-17)
- [x] JPEG encoder+decoder spec-compliance fix (zigzag DQT + AC ordering) — Wave 2 Slice A (2026-04-17)
- [x] Matroska MJPEG + APV codec entries (V_MJPEG, V_MS/VFW/FOURCC + BITMAPINFOHEADER) — Wave 2 Slice B (2026-04-17)
- [x] CLI MJPEG/APV wiring (CodecId::FromStr, VideoCodec enum, transcode intra-codec fast path) — Wave 2 Slice C (2026-04-17)
- [x] AJXL ISOBMFF jxlp animated encoder (finish_isobmff(), ISOBMFF box helpers) — Wave 2 Slice D (2026-04-17)
- [x] AJXL streaming decoder iterator (JxlStreamingDecoder<R: Read>, ISOBMFF + native auto-detect) — Wave 2 Slice E (2026-04-17)
- [x] AVI container muxer + demuxer (RIFF+hdrl+movi+idx1, MJPEG-only, ≤1 GB) — Wave 2 Slice F (2026-04-17)
- [x] APV FFmpeg codec-map aliases (codec_map.rs + codec_mapping.rs) — Slice A of /ultra Wave 3 (2026-04-17)
- [x] AVI v3: OpenDML >1 GB + PCM audio + H264/RGB24 codec support — Slice C of /ultra Wave 3 (2026-04-17)
- [x] MP4 muxer gap-fill: fragmented MP4 (fMP4/moof+mdat) + AV1 av1C + MJPEG/APV coverage — Slice D of /ultra Wave 3 (2026-04-17)
- [x] Core types expansion: NV12/NV21/P010/P016 PixelFormat, S24/F64 SampleFormat, WebP/Gif/Jxl CodecId, typed FourCc constants — Slice E of /ultra Wave 3 (2026-04-17)
- [x] Matroska+streaming: sample-accurate seek, gapless elst, DASH SegmentTemplate manifest — Slice F of /ultra Wave 3 (2026-04-17)
- [x] FFmpeg compat: filter_complex parsing, stream_spec -map, -ss/-to/-t seeking, ffprobe output formatter — Slice G of /ultra Wave 3 (2026-04-17)
- [x] Docs sweep: oximedia-gpu, oximedia-storage, oximedia-routing, oximedia-collab, oximedia-presets, oximedia-switcher, oximedia-automation — Slice H of /ultra Wave 3 (2026-04-17)
- [x] WASM mio fix — oximedia-batch + oximedia-convert — Wave 4 Slice A (2026-04-18)
- [x] WASM GpuAccelerator Send+Sync gate — oximedia-gpu/graphics — Wave 4 Slice B (2026-04-18)
- [x] Core v2: timestamp-arith + Atmos layouts + color metadata — Wave 4 Slice C (2026-04-18)
- [x] Container v4: MKV BlockAdditionMapping + sample-accurate seek all formats + CMAF-LL chunked — Wave 4 Slice D (2026-04-18)
- [x] FFmpeg compat v2: codec-map OnceLock + encoder quality args + -vf/-af + two-pass — Wave 4 Slice E (2026-04-18)
- [x] Docs sweep round 2: oximedia-codec + oximedia-io + oximedia-bitstream + Wave-4 API deltas — Wave 4 Slice F (2026-04-18)
- [x] Transcode pipeline frame-level executor (TranscodePipeline::execute() + multi-track interleaver) (verified 2026-04-21) — Wave 5 Slice A (2026-04-18)
- [x] HW-accel detection: macOS VideoToolbox + Linux VAAPI real platform probes (verified 2026-04-21) — Wave 5 Slice B (2026-04-18)
- [x] ABR BBA-1 buffer-based rate adaptation strategy — oximedia-net (verified 2026-04-21) — Wave 5 Slice C (2026-04-18)
- [x] Container v5: SCTE-35 MPEG-TS ad markers + BatchMetadataUpdate (verified 2026-04-21) — Wave 5 Slice D (2026-04-18)
- [x] Core: structured ErrorContext chain (file:line:fn) + FormatNegotiator codec negotiation (verified 2026-04-21) — Wave 5 Slice E (2026-04-18)
- [x] Docs round 3: codec feature matrix + rate-control guide + SIMD dispatch + Wave-5 deltas (completed 2026-04-21) — Wave 5 Slice F (2026-04-18)

---

## 0.1.5 Planned — Full ONNX Runtime Integration (2026-04-20)

**Theme**: Deliver the "Full ONNX Runtime integration" item previously slated for 0.3.0, via the Pure-Rust **OxiONNX** stack (`~/work/oxionnx/`, crates.io `0.1.2`). No C++ `ort` dependency — preserves COOLJAPAN Pure-Rust Policy. All inference feature-gated; pure-Rust default build unaffected.

**Prerequisites verified (2026-04-20)**:
- OxiONNX 0.1.2 already in workspace (`oxionnx`, `oxionnx-core`)
- `oximedia-cv` already wired (`onnx` + `cuda` features via `oxionnx/cuda`)
- Upstream workspace ships unused sister crates: `oxionnx-ops`, `oxionnx-gpu`, `oxionnx-directml`, `oxionnx-proto`

### Scope

| Item | Crate(s) | Notes |
|------|----------|-------|
| **Workspace dep expansion** | Root `Cargo.toml` | Add `oxionnx-ops = "0.1.2"`, `oxionnx-gpu = "0.1.2"`, `oxionnx-directml = "0.1.2"`, `oxionnx-proto = "0.1.2"` to `[workspace.dependencies]`. |
| **New `oximedia-ml` facade crate** | `crates/oximedia-ml/` | Central model loader (`OnnxModel::load`), model zoo registry, versioning, disk cache in `~/.cache/oximedia/models/`, typed pipelines trait `TypedPipeline<In, Out>`. Feature flags: `onnx` (default off), `cuda`, `webgpu`, `directml`. |
| **Typed pipelines** | `oximedia-ml` | `SceneClassifier`, `ShotBoundaryDetector`, `AutoCaption`, `AestheticScore`, `FaceEmbedder`, `ObjectDetector` — each a thin wrapper over an ONNX model with pre/post-processing. |
| **Broaden OxiONNX integration** | `oximedia-scenes`, `oximedia-shots`, `oximedia-neural`, `oximedia-caption-gen`, `oximedia-recommend`, `oximedia-mir` | Behind `onnx` feature flag per-crate; default CPU heuristics preserved. |
| **Op coverage** | `oximedia-ml` + consumers | Use `oxionnx-ops` for attention/conv/quantized/rnn/kv_cache/nn/ml to unlock real transformer + CNN model loading (not just MatMul/Conv/Softmax). |
| **GPU backends** | Workspace + `oximedia-ml`, `oximedia-cv` | `oxionnx-gpu` (wgpu cross-platform) + `oxionnx-directml` (Windows). Parallel to existing `cuda` feature via `oxionnx-cuda`. |
| **CLI `ml` subcommand** | `oximedia-cli` | `oximedia ml list`, `oximedia ml probe <model.onnx>`, `oximedia ml run <pipeline> --input <file>`. |
| **Python `oximedia.ml` module** | `crates/oximedia-py/` | PyO3 wrappers for each typed pipeline; `import oximedia; oximedia.ml.scene_classifier.classify("in.mp4")`. |
| **WASM compatibility check** | `oximedia-wasm` | OxiONNX CPU path is Pure Rust → validate `cargo check --target wasm32-unknown-unknown` stays clean with `onnx` feature. |
| **Examples** | `oximedia/examples/` | `ml_scene_classify.rs`, `ml_auto_caption.rs`, `ml_model_zoo.rs`. |
| **Docs** | Facade `README.md`, crate-level rustdoc | ONNX usage guide; feature matrix; GPU selection table. |
| **Version bump** | Root + all `Cargo.toml` | `0.1.4` → `0.1.5` (branch already at 0.1.5; driven by branch-name version policy). |
| **Op-coverage backfill** | `~/work/oxionnx/oxionnx-ops/` | If a needed opset op is missing, enhance OxiONNX-ops directly (per IMPLEMENT POLICY) rather than falling back to `ort`. |

---

## 0.1.5 Tracking

Progress tracking for in-flight 0.1.5 Wave items. `[~]` = in progress, `[x]` = complete.

### Wave 1 — Foundation (Workspace + `oximedia-ml` skeleton)
- [x] Workspace Cargo.toml: add `oxionnx-ops`, `oxionnx-gpu`, `oxionnx-directml`, `oxionnx-proto` deps — Wave 1 Slice A (completed 2026-04-20)
  - **Goal:** `oxionnx-ops`, `oxionnx-gpu`, `oxionnx-directml`, `oxionnx-proto` (all 0.1.2, all on crates.io) appear in root `Cargo.toml` `[workspace.dependencies]` so subcrates can `workspace = true`.
  - **Design:** Append four lines after existing `oxionnx = "0.1.2"` / `oxionnx-core = "0.1.2"` in the workspace.dependencies block. Follow existing version-pinning style.
  - **Files:** `Cargo.toml`
  - **Prerequisites:** none
  - **Tests:** workspace-level `cargo check --all-features` must still pass
  - **Risk:** version drift — mitigate by pinning all four to `"0.1.2"` (verified on crates.io 2026-04-20)
- [x] Workspace version bump 0.1.4 → 0.1.5 (root + all sub-crates) — Wave 1 Slice B (completed 2026-04-20)
- [x] Create `oximedia-ml` crate (skeleton): `OnnxModel`, `ModelCache`, `TypedPipeline` trait, feature gates — Wave 1 Slice C (completed 2026-04-20)
  - **Goal:** new `crates/oximedia-ml/` crate containing `OnnxModel`, `ModelCache`, `TypedPipeline` trait, `DeviceType` + `DeviceType::auto()`, `ImagePreprocessor`, `ModelZoo` registry scaffold, `MlError` + `MlResult`, `postprocess` helpers.
  - **Design:** Feature gates `onnx` / `cuda` / `webgpu` / `directml` / `scene-classifier` / `shot-boundary` / `all-pipelines`. `OnnxModel` wraps `oxionnx::Session`. `ModelCache` = `Arc<Mutex<HashMap<PathBuf, Arc<Mutex<OnnxModel>>>>>` with optional LRU capacity. `TypedPipeline { type Input; type Output; fn process(&mut self, input: Self::Input) -> MlResult<Self::Output>; }`. See `atomic-giggling-dawn.md` §Design for full signatures.
  - **Files:** `crates/oximedia-ml/{Cargo.toml,src/{lib,error,device,model,cache,preprocess,postprocess,pipeline,zoo}.rs}`
  - **Prerequisites:** Wave 1 Slice A
  - **Tests:** `tests/model_cache.rs` (concurrent get_or_load, LRU eviction), `tests/preprocess.rs` (ImageNet normalize, letterbox, layout), `tests/pipeline_contract.rs` (mock TypedPipeline round-trip), `tests/fixtures.rs` (synthetic tensor builders)
  - **Risk:** `oxionnx::Session` 0.1.2 public API may differ from local path version — subagent must cross-check `~/work/oxionnx/oxionnx/src/lib.rs` or existing usage in `crates/oximedia-cv/src/ml/runtime.rs` before coding.
- [x] Add `oximedia-ml` to facade crate re-export (feature `ml`) — Wave 1 Slice D (completed 2026-04-20)
  - **Goal:** facade crate exposes `oximedia::ml` module re-exporting `oximedia-ml` behind `feature = "ml"`; the `full` feature picks it up.
  - **Design:** `Cargo.toml` adds `oximedia-ml = { workspace = true, optional = true }` and `ml = ["dep:oximedia-ml"]`; `full` gets `"ml"` appended. `src/lib.rs` adds `#[cfg(feature = "ml")] pub mod ml { pub use oximedia_ml::*; }`. `src/prelude.rs` re-exports `DeviceType`, `ModelCache`, `MlError`, `MlResult`, `OnnxModel`, `TypedPipeline` behind `#[cfg(feature = "ml")]`.
  - **Files:** `Cargo.toml`, `src/lib.rs`, `src/prelude.rs`
  - **Prerequisites:** Wave 1 Slice C
  - **Tests:** `cargo check -p oximedia --no-default-features`, `cargo check -p oximedia --features ml`, `cargo check -p oximedia --features full` all pass
  - **Risk:** `SceneClassifier` name collision with `oximedia_neural::SceneClassifier` — mitigate by keeping each under its own module namespace (`oximedia::ml::*` vs `oximedia::neural::*`) and not re-exporting the type name at prelude level.

### Wave 2 — Typed Pipelines + Op Coverage
- [x] `SceneClassifier` pipeline in `oximedia-ml` (ONNX input/output contract + ImageNet-style preprocessing) — Wave 2 Slice A (completed 2026-04-20)
  - **Goal:** `SceneClassifier` typed pipeline that takes a `VideoFrame`/image, runs an ImageNet-style ONNX classifier, returns sorted top-K `(label, score)` predictions. Implements `TypedPipeline`.
  - **Design:** `SceneClassifier { model: OnnxModel, preproc: ImagePreprocessor, labels: Vec<String>, top_k: usize }` with `::from_model`, `::from_path`, `with_top_k`. Preprocessing: resize-to-fit 224×224, ImageNet mean/std, NCHW. Postprocessing: softmax → argsort-desc → take top_k → pair with labels. `SceneInput` wraps multiple input modes (raw tensor / image / video frame). Output: `SceneClassification { predictions: Vec<(String, f32)> }`.
  - **Files:** `crates/oximedia-ml/src/pipelines/scene_classifier.rs`, `crates/oximedia-ml/src/pipelines/mod.rs` (add `pub mod scene_classifier; pub use scene_classifier::*;`)
  - **Prerequisites:** Wave 1 Slice C
  - **Tests:** `tests/pipeline_contract.rs` includes a `SceneClassifier` construction test that validates `PipelineInfo`/shape contracts without needing a real `.onnx` model (use synthetic ModelInfo).
  - **Risk:** Real ONNX inference requires real model files (absent in repo). Mitigate by feature-gating construction paths and testing only the preprocessing/postprocessing/contract layer in this wave; real-model tests deferred to Wave 2E/6D.
- [x] `ShotBoundaryDetector` pipeline (TransNetV2-compatible I/O) — Wave 2 Slice B (completed 2026-04-20)
  - **Goal:** `ShotBoundaryDetector` typed pipeline with TransNetV2-compatible I/O (48×27 NCHW sliding window of frames, many-hot output for hard/soft cut probabilities).
  - **Design:** `ShotBoundaryDetector { model: OnnxModel, preproc: ImagePreprocessor, window: usize, threshold: f32, prev_frames: VecDeque<TensorFrame> }`. Feeds a rolling 100-frame window (configurable). Output: `Vec<ShotBoundary { frame_index, confidence, kind: Hard | SoftCut }>`. `ShotBoundaryKind` enum tracks many-hot output channels.
  - **Files:** `crates/oximedia-ml/src/pipelines/shot_boundary.rs`, `crates/oximedia-ml/src/pipelines/mod.rs`
  - **Prerequisites:** Wave 1 Slice C
  - **Tests:** `tests/pipeline_contract.rs` extended: construct `ShotBoundaryDetector` with synthetic `ModelInfo`; validate sliding-window accumulator logic (deque fill/drain invariants) without running inference.
  - **Risk:** TransNetV2 expects specific input layout. Mitigate by documenting the expected shape `[1, 100, 27, 48, 3]` in rustdoc and validating dimensions up-front in `process()`, returning `MlError::ShapeMismatch` on drift.
- [ ] `AutoCaption` pipeline (encoder-decoder with `oxionnx-ops` attention + kv_cache) — Wave 2 Slice C
- [x] `AestheticScore` / `ObjectDetector` / `FaceEmbedder` pipelines — Wave 2 Slice D (completed 2026-04-20)
- [ ] Op-coverage audit: run each pipeline against reference ONNX models; backfill missing ops in `~/work/oxionnx/oxionnx-ops/` if needed — Wave 2 Slice E

### Wave 3 — GPU Backend Expansion
- [ ] `oximedia-ml` GPU dispatch: wire `oxionnx-gpu` (wgpu) behind `webgpu` feature — Wave 3 Slice A
- [ ] `oximedia-ml` DirectML dispatch: wire `oxionnx-directml` behind `directml` feature — Wave 3 Slice B
- [ ] `oximedia-cv` parity: broaden existing `cuda` feature to also expose `webgpu`/`directml` toggles — Wave 3 Slice C
- [x] Device-selection heuristic (`DeviceType::auto()`) with runtime probing — Wave 3 Slice D (completed 2026-04-20)

### Wave 4 — Broader Integration (scenes/shots/mir/recommend/caption-gen/neural)
- [x] Wire `oxionnx` into `oximedia-scenes` behind `onnx` feature — Wave 4 Slice A (completed 2026-04-20)
- [x] Wire `oxionnx` into `oximedia-shots` behind `onnx` feature — Wave 4 Slice B (completed 2026-04-20)
- [x] Wire `oxionnx` into `oximedia-caption-gen` behind `onnx` feature — Wave 4 Slice C (completed 2026-04-20)
- [ ] Wire `oxionnx` into `oximedia-neural` behind `onnx` feature — Wave 4 Slice D
- [x] Wire `oxionnx` into `oximedia-recommend` (embedding-based content sim) — Wave 4 Slice E (completed 2026-04-20)
- [x] Wire `oxionnx` into `oximedia-mir` (music-tagging / genre ONNX) — Wave 4 Slice F (completed 2026-04-20)

### Wave 5 — Interfaces (CLI / Python / WASM)
- [x] CLI `oximedia ml list|probe|run` subcommands — Wave 5 Slice A (completed 2026-04-20)
- [x] Python `oximedia.ml` PyO3 module — Wave 5 Slice B (completed 2026-04-21)
- [x] WASM `cargo check --target wasm32-unknown-unknown --features onnx` validation — Wave 5 Slice C (completed 2026-04-20) — oximedia-ml default + `onnx` + `webgpu` + `directml` all build on wasm32; `cuda` is native-only (libloading driver binding); fixed pre-existing facade FileSource re-export so `oximedia --features ml` also builds on wasm32

### Wave 6 — Docs + Examples + Validation
- [x] Examples: `ml_scene_classify.rs`, `ml_auto_caption.rs`, `ml_model_zoo.rs` — Wave 6 Slice A (completed 2026-04-20)
- [x] Rustdoc pass for `oximedia-ml` + updated facade `prelude.rs` — Wave 6 Slice B (completed 2026-04-20)
- [x] README + `docs/ml_guide.md` feature matrix + GPU selection table — Wave 6 Slice C (completed 2026-04-21)
- [x] Full CI gate: `cargo test --all --features onnx`, `cargo clippy --all -- -D warnings`, `cargo doc --all --no-deps` — Wave 6 Slice D (completed 2026-04-21)

---

## Codec Implementation Roadmap (0.1.5 Honesty Pass)

Introduced in 0.1.5 alongside the documentation honesty pass. Mirrors
`docs/codec_status.md` — see that file for per-decoder current state, what
is missing, and the effort rationale.

### Taxonomy

| Label | Meaning |
|-------|---------|
| Verified | End-to-end decode matches a reference on external fixtures |
| Functional | Real reconstruction path present and self-consistent on round-trip |
| Bitstream-parsing | Headers parsed; no pixel/sample reconstruction |
| Experimental | API sketch; not intended to decode |

### Effort buckets

| Bucket | Approximate cost |
|--------|------------------|
| small | Focused bug-fix or a single missing stage (days) |
| medium | Multiple decoder stages (weeks) |
| large | Complete reconstruction pipeline (months) |
| specialist | Codec specialist + reference generator + conformance suite |

### Gap list

| Codec | Current | Missing | Effort | Target |
|-------|---------|---------|--------|--------|
| AV1 decode | Bitstream-parsing | Entropy / predict / transform / loop-filter / CDEF / film-grain wired to output buffer; reference-frame management; issue #9 | specialist | 0.2.0+ |
| VP9 decode | Bitstream-parsing | Wire superblock/block/intra decode to `VideoFrame`; fill pipeline stages; reference-frame management | large | 0.2.0+ |
| VP8 decode | Bitstream-parsing | Intra/inter decode, DCT/WHT inverse transform, loop filter, Y/U/V output | large | 0.2.0+ |
| Theora decode | Bitstream-parsing (bug) | Replace `to_vec()` mis-copy with direct write into `frame.planes[i].data` | small | 0.1.5 point / 0.1.6 |
| AVIF decode | Bitstream-parsing | Real AV1 pixel output + image-item demux (follows AV1) | specialist | follows AV1 |
| WebP VP8 lossy decode | Missing | Full lossy VP8 WebP decoder (follows VP8) | large | follows VP8 |
| Vorbis decode | Bitstream-parsing | Full codebook / residue / floor curve / MDCT-IMDCT / OLA / channel coupling | specialist | 0.2.0+ |
| Opus SILK / hybrid | Functional (CELT only) | Real SILK LP analysis/synthesis (LTP, LSF, LPC); hybrid-mode band splitting | specialist | 0.1.6 / 0.2.0+ |

### Supporting deliverables

- [x] `docs/codec_status.md` — single source of truth for decoder honesty
- [x] `crates/oximedia-codec/tests/av1_real_bitstream.rs` (`#[ignore]`; `OXIMEDIA_AV1_FIXTURE` env var; no binary fixture in repo) — executable gate for the AV1 gap; will pass when pixel reconstruction lands
- [x] README + `crates/oximedia-codec/README.md` demoted: AV1 / VP9 / VP8 / Theora / Vorbis / AVIF labelled `Bitstream-parsing`
- [x] `examples/decode_video.rs` rewritten to reflect the real decoder-status matrix (no fake `println!` code samples)
- [x] Theora pixel-copy bug-fix (small; completed in 0.1.6 — 2026-04-26)
- [ ] Opus SILK decoder (specialist; 0.2.0+)
- [ ] AV1 reconstruction pipeline (specialist; 0.2.0+; issue #9)
- [ ] VP9 reconstruction wiring (large; 0.2.0+)
- [ ] VP8 real decode (large; 0.2.0+)
- [ ] Vorbis full decode (specialist; 0.2.0+)

---

## 0.1.6 Changes (2026-04-26)

**Theme**: Stub resolution across core crates, codec improvements, dependency upgrades, and metadata hygiene.

| Item | Status |
|------|--------|
| Stub resolution — `oximedia-accel` color conversion stubs replaced with real BT.601/709/2020 paths | ✅ Done |
| Stub resolution — Vorbis codebook residue/floor full decode wired (partial; bitstream-parsing promoted to Functional for floor0) | ✅ Done |
| Stub resolution — ACES ODT (Output Device Transform) real matrix path in `oximedia-colormgmt` | ✅ Done |
| Stub resolution — DASH segment fetch real HTTP path in `oximedia-net` (replaces `todo!()` in ABR fetch loop) | ✅ Done |
| Stub resolution — system font loading in `oximedia-subtitle` (fontdb integration via Pure Rust path) | ✅ Done |
| `exr.rs` splitrs refactor — file exceeded 2000 lines; split into `exr/header.rs`, `exr/tile.rs`, `exr/compress.rs` | ✅ Done |
| OxiFFT upgrade — workspace dep bumped from 0.2.x to 0.3.0 across all consuming crates | ✅ Done |
| wgpu 29 API update — `motion_estimation.rs` updated for breaking wgpu 29 compute-pass API | ✅ Done |
| keywords/categories — added to every crate `Cargo.toml` that was missing them | ✅ Done |
| readme field — added `readme = "README.md"` to all crate `Cargo.toml` files | ✅ Done |
| RUSTSEC-2026-0104 triaged — advisory reviewed; not reachable via OxiMedia's call paths; documented in `SECURITY.md` | ✅ Done |
| Theora pixel-copy bug-fix — replaced `to_vec()` mis-copy with direct write into `frame.planes[i].data` | ✅ Done |
| Version bump 0.1.5 → 0.1.6 (root + all sub-crates) | ✅ Done |
| **81,582 tests passing**, +199 from 0.1.5 baseline | ✅ Done |

---

## 0.1.6 Tracking

Progress tracking for 0.1.6 items. `[~]` = in progress, `[x]` = complete.

- [x] Stub resolution: `oximedia-accel` color conversion (BT.601/709/2020 real paths) — 2026-04-26
- [x] Stub resolution: Vorbis codebook residue/floor partial decode — 2026-04-26
- [x] Stub resolution: ACES ODT real matrix path in `oximedia-colormgmt` — 2026-04-26
- [x] Stub resolution: DASH segment fetch real HTTP path in `oximedia-net` — 2026-04-26
- [x] Stub resolution: system font loading in `oximedia-subtitle` (Pure Rust fontdb) — 2026-04-26
- [x] `exr.rs` splitrs refactor (split into header/tile/compress sub-modules) — 2026-04-26
- [x] OxiFFT upgrade to 0.3.0 across workspace — 2026-04-26
- [x] wgpu 29 `motion_estimation.rs` compute-pass API update — 2026-04-26
- [x] keywords/categories added to all crate `Cargo.toml` files — 2026-04-26
- [x] readme field added to all crate `Cargo.toml` files — 2026-04-26
- [x] RUSTSEC-2026-0104 triaged and documented — 2026-04-26
- [x] Theora pixel-copy bug-fix — 2026-04-26
- [x] Version bump 0.1.5 → 0.1.6 — 2026-04-26

---

## Future / Planned (Post-0.2.0)

| Item | Target | Notes |
|------|--------|-------|
| **NMOS IS-04/IS-05/IS-07 REST APIs** | **0.1.2** | IS-04/IS-05/IS-07 REST APIs complete; discovery via mDNS implemented (`nmos-http` + `nmos-discovery` features in `oximedia-routing`). Constraint schemas + IS-08 channel mapping also complete in 0.1.2. |
| **IS-08 Audio Channel Mapping API** | **Implemented in 0.1.2** | `channel_mapping` module in `oximedia-routing`; 41 dedicated tests, 656 total routing tests |
| **IS-09 System API** | **Implemented in 0.1.2** | Global config, health endpoint, API version discovery; 36 dedicated tests |
| **IS-11 Stream Compatibility Management** | **Implemented in 0.1.2** | `CompatibilityRegistry`, `MediaCapability`; compatibility module in `oximedia-routing` |
| **AVX-512 SIMD paths** | **Implemented in 0.1.2 (foundation)** | `oximedia-simd`; `CpuFeatures` runtime detection; runtime dispatch via `multiversion` |
| Python pip-installable package | 0.2.0 | PyO3 bindings complete; maturin packaging and PyPI publish remaining |
| WASM/WebAssembly build support | 0.3.0 | Pure-Rust stack makes this feasible; needs `wasm32-unknown-unknown` CI |
| H.264 software decoder | **Landing in 0.1.7** | MPEG-LA AVC pool wound down Dec 2024; bulk of essential patents at or past 20-year terms. Full pipeline behind `oximedia_codec::h264::Decoder`. See `docs/codec_status.md` and `docs/h264_decoder_walkthrough.md`. |
| Hardware H.264 encoding | 0.2.x | Encoder counterpart for the software decoder; will reuse the same `h264` module for bitstream syntax. |
| ~~Full ONNX Runtime integration~~ | ~~0.3.0~~ | **Promoted to 0.1.5** — see 0.1.5 Planned section below. Delivered via Pure-Rust OxiONNX, not C++ `ort`. |

---

## Architecture Goals

| Goal | Status |
|------|--------|
| No unsafe code (`#![forbid(unsafe_code)]`) | Enforced across all stable/alpha crates |
| Zero clippy warnings | Enforced; CI gate |
| Apache 2.0 license | Enforced |
| Patent-free codecs only (Green List) | Enforced; HEVC/AAC rejected at compile time. H.264 moved off the Red List in 0.1.7 — see `docs/codec_status.md`. |
| Async-first design | Complete |
| Zero-copy buffer pool | Implemented (`oximedia-core`, `oximedia-io`) |
| Pure Rust default build | Enforced; C/Fortran deps feature-gated only |
| No OpenBLAS | Enforced; OxiBLAS used where BLAS needed |
| No `bincode` | Enforced; OxiCode used for serialization |
| No `rustfft` | Enforced; OxiFFT used |
| No `zip` crate | Enforced; `oxiarc-archive` used |
| Workspace dependency management | All crate versions via workspace `[dependencies]` |
| COOLJAPAN ecosystem alignment | SciRS2-Core for numeric/statistical ops |
| `unwrap()` free | Enforced across ALL crates; 1,386 unwrap() calls eliminated in 0.1.2 session; only doc-comment examples remain |
| Single file < 2000 SLOC | Enforced; splitrs used for refactoring targets |
| NMOS IS-04/IS-05/IS-07 HTTP APIs | Implemented (`nmos-http` feature in `oximedia-routing`) |
| NMOS IS-08 Audio Channel Mapping | Implemented (`channel_mapping` module in `oximedia-routing`) |
| NMOS IS-09 System API | Implemented (global config, health endpoint, API version discovery in `oximedia-routing`) |
| NMOS IS-11 Stream Compatibility Management | Implemented (compatibility module in `oximedia-routing`) |
| AVX-512 SIMD paths | Implemented with runtime detection in `oximedia-simd` (`CpuFeatures`) |
| Criterion benchmarks | 4 benchmark suites in `benches/` crate |
| Comprehensive CLI | `oximedia-cli` with probe/info/transcode/loudness/quality/dedup/timecode commands |

---

## Testing Commands

```bash
# Run all tests
cargo test --all

# Run with all features
cargo test --all --features av1,vp9,vp8,opus

# Run specific codec tests
cargo test --features vp8 vp8
cargo test --features opus opus

# Clippy (must pass with zero warnings)
cargo clippy --all -- -D warnings

# Documentation build
cargo doc --all --no-deps

# Format check
cargo fmt --check

# SLOC count
tokei /Users/kitasan/work/oximedia

# COCOMO estimate
cocomo /Users/kitasan/work/oximedia

# Find refactoring targets (files > 2000 lines)
rslines 50

# Dry-run publish check (never publish without explicit instruction)
cargo publish --dry-run -p oximedia-core
```

---

## Code Quality Gates

All of the following must pass before any release tag:

1. `cargo build --all` — Must compile clean
2. `cargo test --all` — All tests pass
3. `cargo clippy --all -- -D warnings` — Zero warnings
4. `cargo doc --all --no-deps` — Documentation builds without errors
5. `cargo fmt --check` — Formatting verified
6. No `todo!()`/`unimplemented!()` in stable crates — Verified by grep
7. No `unwrap()` in stable crates — Verified by grep

---

*Last updated: 2026-04-26 — v0.1.6 active (stub resolution, OxiFFT 0.3.0, wgpu 29, exr.rs refactor, metadata hygiene); v0.1.5 stable baseline; 108 crates, 81,582 tests verified, ~2.687M SLOC; `oximedia-ml` stable (typed pipelines: SceneClassifier, ShotBoundaryDetector, AestheticScorer, ObjectDetector, FaceEmbedder); feature-gated `onnx`/`cuda`/`webgpu`/`directml`; pure-Rust default preserved*

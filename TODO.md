# OxiMedia — The Sovereign Media Framework: Development Roadmap

**Version: 0.2.1 (active, dev branch `0.2.1`; gains a new `oximedia-capture` crate — see "0.2.1 Capture" near the end of this file) / 0.2.0 (stable, `master` — released 2026-07-15)**
**Status as of: 2026-08-12**
**Total SLOC: 3,057,331 lines of Rust code (measured via `tokei .` on 2026-08-12; 3,697,874 total Rust lines / 9,013 files / 163,846 comments. Whole tree, all languages: 3,754,585 lines / 3,074,095 code / 9,563 files)**
**Total Tests: 104,693 passing with `--all-features` / 103,322 with default features (0 failed, 155 / 144 skipped, 576 test binaries — `cargo nextest run --workspace`, genuine full runs, verified 2026-08-12)**
**Total Crates: 115 (measured via `cargo metadata --no-deps` on 2026-08-12; root workspace only — `web/` is a separate, excluded nested workspace, see below)**
**Crate Status: 111 Stable library crates under `crates/` + facade `oximedia` + `oximedia-cli` + `oximedia-wasm` + internal bench harness = 115 workspace members; 0 Alpha / 0 Partial**
**Quality gates (2026-08-12, all green): `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` 0 lints across 261 crates; `cargo deny check bans` + `check licenses` both ok; `wasm32-unknown-unknown` builds for `oximedia-wasm` and `oximedia`; `cargo check -p oximedia-py` clean; no source file over 2,000 lines.**
**Current Branch: 0.2.1 — production-readiness release landed 2026-07-08 (100% Pure Rust default build); Waves 21–30 + `oximedia-web` (browser modules) shipped in 0.1.9. 0.2.0 shipped 2026-07-15 (tagged, pushed, live on crates.io): a real frame-level transcode engine (`oximedia-transcode`), real AV1, VP9 and VP8 key-frame/intra video decoders (bit-exact vs dav1d/aomdec/libvpx/libwebp), ~40 `oximedia-cli` flags made real or honest, real CENC/`cbcs` packager encryption, a real RFC 3394 SRT key wrap, a broad fabricated-success-elimination sweep, and parser bounds/DoS hardening across MP4/DVB/RTSP/RTMP/WebRTC/AAF — see `CHANGELOG.md`'s `[0.2.0]` section. **On this branch since:** VP8 inter-frame decode (bit-exact vs libvpx on 5 multi-frame streams, 2026-08-11); a new `oximedia-capture` crate; lossy WebP decode; a corrected EBU R128 / K-weighting loudness stack; and (2026-08-12) VP9 inter-frame and intra-only decode (bit-exact vs libvpx on 10 conformance streams), a full RFC 9639 FLAC rebuild verified against libFLAC/ffmpeg in both directions, real AVIF decode via AV1, and a workspace-wide fabricated-success elimination sweep across nine crates that took the `TODO(0.2.x)` marker count from 123 to 22. See `CHANGELOG.md`'s `[0.2.1]` section for full detail and the rewritten "Deferred (0.2.x)" section at the end of this file for exactly what is left.**

---

## Summary

| Category | Count | Notes |
|----------|-------|-------|
| Stable crates | 111 | Library crates under `crates/`; no `todo!()`/`unimplemented!()` stubs. (+ facade `oximedia`, `oximedia-cli`, `oximedia-wasm`, internal bench harness = 115 workspace members total) |
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
| None/Resolved | `oxiarc-archive` (dep) | Bumped to 0.3.6 (2026-07-13, "bump oxiarc" commit) while sibling crates `oxiarc-brotli`/`oxiarc-bzip2`/`oxiarc-lzma`/`oxiarc-snappy` stayed at 0.3.5; `oxiarc-archive` 0.3.6's source calls `decompress_with_limit`/`decompress_frame_with_limit`/`DICT_SIZE_ALLOC_CAP`/`with_max_output`/`with_max_output_size` APIs that don't exist in the 0.3.5 siblings — 11 compile errors. Breaks `oxiarc-archive` itself and transitively `oximedia-archive-pro`, `oximedia-batch`, `oximedia-convert`, `oximedia-cli`, `oximedia-py`, `oximedia-wasm` (all in workspace `default-members` except `oximedia-py`) — a plain `cargo build` currently fails. `oximedia` facade unaffected under default (no-feature) build (these three deps are optional there). Pure-Rust API mismatch only, not a C/C++/Fortran regression. | Resolved (2026-07-14 — cargo check --workspace --all-features passes clean; oxiarc-brotli/bzip2/lzma/snappy republished at 0.3.6 matching oxiarc-archive) |
| None/Resolved | `oximedia-net` | `todo!()` confirmed in documentation comment only in ABR controller — not executable code, no runtime impact | Resolved |
| Low | `oximedia-wasm` (dep chain, pre-existing) | `oximedia-watermark` → `reed-solomon-erasure` v6.0.0 → `parking_lot` v0.11.2 → `instant` v0.1.13, confirmed via `cargo tree -p oximedia-watermark -i instant` (2026-08-11). Unrelated to and untouched by package A8 (`oximedia-capture`); recorded here per that package's verification pass, not investigated further. | Open |
| Low | `oximedia-mam` (dep chain, pre-existing) | `cargo deny check licenses` rejects MPL-2.0 reached via `oximedia-mam` → `async-graphql` → `fast_chemail`/`ascii_utils`, and separately via `oximedia-cloud`'s dev-dependency `mockito` → `colored`; confirmed 2026-08-11. `deny.toml`'s `[licenses] allow` list has no MPL-2.0 entry. Unrelated to and untouched by package A8; recorded here per that package's verification pass, not investigated further. | Open |

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
- [x] `AutoCaption` pipeline (encoder-decoder with `oxionnx-ops` attention + kv_cache) — Wave 2 Slice C (implemented; `crates/oximedia-ml/src/pipelines/auto_caption.rs` 435 lines, Whisper-compatible encoder-decoder, greedy decode loop, tests verified 2026-05-19)
- [x] `AestheticScore` / `ObjectDetector` / `FaceEmbedder` pipelines — Wave 2 Slice D (completed 2026-04-20)
- [x] Op-coverage audit: run each pipeline against reference ONNX models; backfill missing ops in `~/work/oxionnx/oxionnx-ops/` if needed — Wave 2 Slice E (audit 2026-05-19: zero gaps; oxionnx-ops covers all six pipeline op-graphs, 112 ops implemented; `pipelines/*` use `OnnxModel` indirection — no direct op refs; static coverage complete)

### Wave 3 — GPU Backend Expansion
- [x] `oximedia-ml` GPU dispatch: wire `oxionnx-gpu` (wgpu) behind `webgpu` feature — Wave 3 Slice A (implemented 2026-05-11; see Slice OXIONNX-EP — `device_to_providers(WebGpu)` → `[Gpu, Cpu]` via `with_provider_kinds`)
- [x] `oximedia-ml` DirectML dispatch: wire `oxionnx-directml` behind `directml` feature — Wave 3 Slice B (implemented 2026-05-11; see Slice OXIONNX-EP — `ProviderKind::DirectMl` added; `device_to_providers(DirectMl)` → `[DirectMl, Cpu]` via `with_provider_kinds`)
- [x] `oximedia-cv` parity: broaden existing `cuda` feature to also expose `webgpu`/`directml` toggles — Wave 3 Slice C (verified 2026-05-11; features declared in crates/oximedia-cv/Cargo.toml)
- [x] Device-selection heuristic (`DeviceType::auto()`) with runtime probing — Wave 3 Slice D (completed 2026-04-20)

### Wave 4 — Broader Integration (scenes/shots/mir/recommend/caption-gen/neural)
- [x] Wire `oxionnx` into `oximedia-scenes` behind `onnx` feature — Wave 4 Slice A (completed 2026-04-20)
- [x] Wire `oxionnx` into `oximedia-shots` behind `onnx` feature — Wave 4 Slice B (completed 2026-04-20)
- [x] Wire `oxionnx` into `oximedia-caption-gen` behind `onnx` feature — Wave 4 Slice C (completed 2026-04-20)
- [x] Wire `oxionnx` into `oximedia-neural` behind `onnx` feature — Wave 4 Slice D (implemented; `oxionnx` optional dep in Cargo.toml `onnx` feature; `onnx_backend.rs` wraps `oxionnx::Session`; verified 2026-05-19)
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
| AV1 decode | **Functional** (keyframe/intra only, 0.2.0 — `crates/oximedia-codec/src/av1/kf/`, bit-exact vs dav1d 1.5.1/aomdec-libaom v3.12.1 on 13 vectors, full deblock/CDEF/loop-restoration chain) | Inter-frame decode (motion vectors, reference-frame management, compound prediction); intra block copy, palette mode, super-resolution, quantizer matrices, film-grain synthesis, 10/12-bit, monochrome, 4:2:2/4:4:4; issue #9 | specialist | 0.2.0+ |
| VP9 decode | **Verified** (key + intra-only + inter frames, 0.2.1 — `crates/oximedia-codec/src/vp9/dec/`, renamed from `vp9/kf/`; cross-frame reference/probability state, `find_mv_refs` candidate scan, inter mode + MV entropy decode, `vpx_convolve8` eight-tap MC across all four filter sets, compound prediction, backward probability adaptation, `show_existing_frame` and superframes; bit-exact vs libvpx on 10 conformance streams through the public API, 0 ignores / 0 thresholds / 0 skips) | Inter-frame segmentation (`read_inter_segment_id` + predicted seg map; `p9seg` fixture committed, refusal asserted at packet 1) and reference scaling (scale factors + `vpx_scaled_2d`; `scaled` fixture committed, refusal asserted at packet 6); profiles 1-3 / non-4:2:0 / non-8-bit (`vp9/dec/mod.rs:102`) | small-medium each | 0.2.x |
| VP8 decode | **Verified** (key + inter frames, 0.2.1 — `crates/oximedia-codec/src/vp8/dec/`, full RFC 6386 pipeline incl. inter-frame decode: motion-vector entropy decode, per-macroblock prediction records, six-tap/bilinear sub-pixel MC, last/golden/altref reference management incl. hidden `show_frame == 0` frames; bit-exact vs libvpx on 5 multi-frame conformance streams covering a hidden altref, SPLITMV at a non-macroblock-aligned width, error-resilient entropy reset, and segmentation) | Not covered by a fixture (parsed and implemented, but unproven): bitstream versions 1-3 (bilinear/full-pel profiles) and multi-token-partition inter frames; see `docs/codec_status.md` | small | 0.2.x |
| Theora decode | Bitstream-parsing (decode hand-off fixed in 0.1.7) | Encoder↔decoder bitstream alignment so a self-consistent encode→decode round-trip succeeds; promote to Functional once that lands. | medium | 0.2.0+ |
| AVIF decode | **Functional** (0.2.1 — `crates/oximedia-codec/src/avif/{container,decode}.rs`; `decode()` feeds the extracted AV1 OBU through `Av1Decoder`, so scope is that decoder's: 8-bit 4:2:0. Byte-identical to ffmpeg/dav1d on a real libaom fixture. Also fixed: the `iloc` parser accepted only box version 1 — what this crate's own encoder writes — and would have rejected essentially every real-world AVIF, which writes version 0) | Alpha (real-world AVIF encodes it as **monochrome** AV1) and 10/12-bit items — both blocked on AV1 `recon.rs:1606`, not on anything in `avif/`; committed fixtures pin both refusals | follows AV1 | follows AV1 |
| WebP VP8 lossy decode | **Functional** (0.2.1 — `crates/oximedia-codec/src/webp/vp8_decoder.rs`; wires the RIFF `VP8 ` chunk to the existing bit-exact VP8 key-frame decoder (`vp8::decode_keyframe`) — a lossy WebP image *is* a VP8 key frame, so this is plumbing, not a new decoder; bit-exact vs `dwebp -yuv` on all 3 `vp8_fixtures` key-frame vectors, wrapped in a hand-built RIFF container; uncompressed `ALPH` alpha merges into RGBA32) | VP8L-compressed `ALPH` alpha (`webp/alpha.rs`'s pre-existing honest `UnsupportedFeature`, not a new gap); n/a for inter frames (a lossy WebP is always exactly one key frame, never inter) | small (done) | 0.2.1 |
| Vorbis decode | Bitstream-parsing | Full codebook / residue / floor curve / MDCT-IMDCT / OLA / channel coupling | specialist | 0.2.0+ |
| Opus (`oximedia-codec`) | **Bitstream-parsing** — DEMOTED 0.2.1 on empirical evidence. CELT/SILK/Hybrid decode paths exist and are dispatched, but on **real libopus packets** decode returns silence (0 non-zero of 49,920 samples) or out-of-range output, and the encoder emits a TOC byte that misdescribes its own payload. The ~82 existing tests are self-consistency round-trips, which is why a prior source-reading audit promoted this entry upward | A decoder debugged packet-by-packet against libopus output **and** an encoder rebuilt against RFC 6716 §3.1; official RFC 6716 test vectors are a prerequisite, not a follow-up. Every wired consumer now refuses rather than passing the output on | specialist | 0.2.x+ (unscheduled) |

### Supporting deliverables

- [x] `docs/codec_status.md` — single source of truth for decoder honesty
- [x] `crates/oximedia-codec/tests/av1_real_bitstream.rs` (`#[ignore]`; `OXIMEDIA_AV1_FIXTURE` env var; no binary fixture in repo) — executable gate for the AV1 gap; will pass when pixel reconstruction lands
- [x] README + `crates/oximedia-codec/README.md` demoted: AV1 / VP9 / VP8 / Theora / Vorbis / AVIF labelled `Bitstream-parsing`
- [x] `examples/decode_video.rs` rewritten to reflect the real decoder-status matrix (no fake `println!` code samples)
- [x] Theora decoder hand-off bug-fix — `to_vec()` mis-copy replaced with direct write into `frame.planes[i].data`; pinned by `theora::tests::test_issue_9_to_video_frame_writes_planes_into_videoframe` (small; completed in 0.1.7 — 2026-05-03)
- [ ] Opus (`oximedia-codec`) real decoder **and** encoder — the SILK/CELT/Hybrid
  paths shipped and are dispatched, but were demoted to Bitstream-parsing in
  0.2.1 after being fed real libopus packets for the first time (silence /
  out-of-range output; encoder emits an invalid TOC). Specialist; 0.2.x+,
  unscheduled — see the Deferred section
- [x] AV1 keyframe/intra reconstruction wiring — bit-exact vs dav1d 1.5.1 and aomdec/libaom v3.12.1 on 13 keyframe test vectors, full deblock/CDEF/loop-restoration (Wiener + SGRPROJ) chain (`crates/oximedia-codec/src/av1/kf/`); inter-frame decode remains open (specialist; 0.2.x+; see Deferred section)
- [x] VP9 keyframe/intra reconstruction wiring — bit-exact vs libvpx/ffmpeg reference decodes (`crates/oximedia-codec/src/vp9/kf/`, renamed to `vp9/dec/` in 0.2.1 when inter decode landed)
- [x] VP9 inter-frame **and** intra-only decode — cross-frame reference/probability
  state (four frame contexts, an eight-slot MI-aligned DPB with per-MI MV grids,
  loop-filter-delta and segmentation inheritance), the `find_mv_refs` candidate
  scan with sign-bias flipping and temporal candidates, inter mode / reference /
  motion-vector entropy decode incl. sub-8x8 and the `use_mv_hp` gate, the
  `vpx_convolve8` eight-tap MC family across all four interpolation filter sets,
  compound prediction, backward probability adaptation, `show_existing_frame`
  redisplay and superframe unpacking (`crates/oximedia-codec/src/vp9/dec/`,
  ported from libvpx v1.15.2 with per-function `file:line` citations against a
  `diff`-verified upstream fetch). **10 of 10** non-deferred conformance streams
  decode bit-exactly through the public `VideoDecoder::send_packet` /
  `receive_frame` API — `p9still`/`p9basic`/`p9er`/`p9hp` (76x42, 8 frames each),
  `p9tc` (512x64, 2 tile columns), `p9alt` (128x128, hidden ALTREF in a
  superframe), `compound` (13 coded / 12 shown, compound prediction + a second
  hidden ARF), `switch` (100x68, >1 interp filter at a non-8-aligned width),
  `p9sef` (SEF redisplay) and `p9io` (352x288 intra-only) — with 0 `#[ignore]`,
  0 PSNR thresholds and 0 skips, an anti-vacuity test that a planted mismatch is
  reported, and a coverage assertion that puts a floor on the decoder's own
  symbol counters (compound blocks + `comp_ref` reads, all four inter modes,
  intra-and-inter blocks inside inter frames, real per-block filter choice on
  SWITCHABLE frames, non-zero MVs per component, and the 1/8-pel high-precision
  bit) so a header-permitted-but-unexercised feature cannot pass. Inter segmentation and
  reference scaling refuse at a **pinned packet index** with committed gate
  fixtures (`p9seg`, `scaled`). Suite: `vp9/dec/inter_fixture_tests.rs`;
  provenance: `vp9/dec/testdata/RECIPE.md`. 0.2.1, 2026-08-12; see
  `CHANGELOG.md`'s Unreleased section
- [x] Five hollow VP9 modules deleted (~4,800 lines) — `vp9/compressed.rs`,
  `prediction.rs`, `mvref.rs`, `inter.rs`, `reference.rs`, each verified to have
  zero real consumers workspace-wide first; `vp9::dec::decode_keyframe` removed
  once its last caller went away
- [x] AVIF decode wired to the AV1 keyframe decoder — `avif/decode.rs` +
  `avif/container.rs`; byte-identical to ffmpeg/dav1d on a real libaom fixture.
  Fixed in the same pass: the `iloc` parser accepted only box version 1 and
  would have rejected essentially every real-world AVIF. Alpha (monochrome AV1)
  and 10/12-bit refuse honestly, with committed fixtures pinning both
- [x] FLAC (`oximedia-codec`) encoder **and** decoder rebuilt to RFC 9639 —
  the pair was previously self-consistent but could not interoperate with any
  real FLAC in either direction (the decoder *panicked* on stock ffmpeg output).
  Now verified in both directions against libFLAC/ffmpeg. New
  `flac/{bitio,frame,subframe,residual}.rs`; `crates/oximedia-audio/src/flac/`
  is a separate implementation and was not touched
- [x] VP8 keyframe/intra decode — full RFC 6386 pipeline, bit-exact vs libwebp (`crates/oximedia-codec/src/vp8/dec/`, renamed from `vp8/keyframe/` when inter-frame decode landed alongside it — see immediately below)
- [x] VP8 inter-frame decode — RFC 6386 §16 per-macroblock prediction records (incl. the §16.3 near-MV survey), §17 motion-vector entropy decode, §18 six-tap/bilinear sub-pixel motion compensation, and §9.7-§9.9 last/golden/altref reference management (incl. hidden `show_frame == 0` frames and the `refresh_entropy_probs` snapshot/restore) (`crates/oximedia-codec/src/vp8/dec/`); `Vp8Decoder` now routes every frame through `dec::Vp8SequenceDecoder` instead of returning `CodecError::UnsupportedFeature` for inter frames. Verified bit-exact against libvpx on 5 multi-frame conformance streams — 39 coded / 38 shown frames spanning a genuine hidden altref frame, `SPLITMV` at a non-macroblock-aligned width (100x64), error-resilient entropy reset, and segmentation with per-segment quantiser deltas (`vp8/dec/inter_fixture_tests.rs`; fixtures and exact ffmpeg/libvpx provenance in `vp8/dec/testdata/README.md`). 0.2.1, 2026-08-11; see `CHANGELOG.md`'s Unreleased section.
- [x] WebP lossy (VP8) decode wired to the VP8 key-frame decoder —
  `crates/oximedia-codec/src/webp/vp8_decoder.rs` (`vp8` feature): RIFF `VP8 `
  chunk -> `vp8::decode_keyframe` -> YUV 4:2:0 `VideoFrame`, reusing the
  existing `WebPContainer::parse` RIFF parser rather than a second one;
  uncompressed `ALPH` alpha merges into RGBA32. Bit-exact vs `dwebp -yuv` on
  all 3 `vp8_fixtures` vectors wrapped in-test in a RIFF container
  (`crates/oximedia-codec/tests/webp_vp8_lossy.rs`); `ImageDecoder::decode_webp`
  now delegates to it instead of its own now-removed ad hoc `Vp8Decoder`
  call. VP8L-compressed `ALPH` alpha stays an honest `UnsupportedFeature`
  (`webp/alpha.rs`'s pre-existing boundary). 2026-08-11.
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
| Theora pixel-copy bug-fix — listed in 0.1.6 plan but the decoder source still routed the per-plane copy through a temporary `to_vec()` clone in 0.1.6; the actual fix landed in 0.1.7 (see issue #9) | ⚠️ Carried into 0.1.7 |
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
- [x] Theora pixel-copy bug-fix — listed for 0.1.6 but source still buggy at 0.1.6 ship; actually fixed in 0.1.7 (see issue #9 — 2026-05-03) (stale in-progress marker; fix confirmed shipped 0.1.7 issue #9; canonical entry ~line 455)
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
| Hardware H.264 encoding | 2027+ | Blocked on patent expiry (est. September 2027); feature-gated, separate repo `oximedia-avc` |
| ~~Full ONNX Runtime integration~~ | ~~0.3.0~~ | **Promoted to 0.1.5** — see 0.1.5 Planned section below. Delivered via Pure-Rust OxiONNX, not C++ `ort`. |

---

## Architecture Goals

| Goal | Status |
|------|--------|
| No unsafe code (`#![forbid(unsafe_code)]`) | Enforced across all stable/alpha crates |
| Zero clippy warnings | Enforced; CI gate |
| Apache 2.0 license | Enforced |
| Patent-free codecs only (Green List) | Enforced; H.264/HEVC/AAC rejected at compile time |
| Async-first design | Complete |
| Zero-copy buffer pool | Implemented (`oximedia-core`, `oximedia-io`) |
| Pure Rust default build | Enforced; C/Fortran deps feature-gated only |
| No OpenBLAS | Enforced; OxiBLAS used where BLAS needed |
| No `bincode` | Enforced; OxiCode used for serialization |
| No `rustfft` | Enforced; OxiFFT used |
| No `zip` crate | Enforced; `oxiarc-archive` used |
| No `lz4_flex` crate | Enforced; `oxiarc-lz4` used (last direct holdout removed 2026-06-05; only a transitive `tantivy` pull remains) |
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

## Pure Rust Migration (COOLJAPAN Policy)

Tracking removal of non-OxiARC compression/foreign dependencies. `[x]` = complete.

- [x] `lz4_flex` → `oxiarc-lz4` in `oximedia-renderfarm` storage (`compress_data`/`decompress_data` in `crates/oximedia-renderfarm/src/storage.rs` now use the `oxiarc_lz4::compress` / `oxiarc_lz4::decompress` LZ4 frame format; added `test_compress_decompress_roundtrip`). Removed dead `lz4_flex` declaration from `oximedia-collab/Cargo.toml` (zero call sites; it uses `oxiarc-deflate`) and dropped `lz4_flex` from the workspace `[dependencies]`. This removes the last direct non-OxiARC compression holdout in OxiMedia. Verified: `oximedia-renderfarm` 1031 tests pass, clippy clean (`-D warnings`), no `lz4_flex` in the renderfarm/collab dependency trees (only a transitive `tantivy` pull remains). — 2026-06-05

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
tokei .

# COCOMO estimate
cocomo .

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

## 0.1.7 Changes (2026-05-04)

- [x] **`oximedia-convert::perform_conversion`** — Replaced stub with real `TranscodePipeline` integration; forwards `video_codec`, `audio_codec`, `video_bitrate` to builder; `resolution`/`frame_rate` logged as best-effort; 2 new tests
- [x] **`oximedia-cli captions generate`** — Full ASR pipeline: WAV parse → 80-bin log-mel spectrogram → `CaptionEncoder` (ONNX) → greedy decode → `build_caption_blocks` → `optimal_break` → export; gated behind `--features caption-gen`; added `--model` + `--vocab` CLI args; 2 new tests
- [x] **`oximedia-cli proxy generate`** — Replaced `TranscodePipeline` direct call with `oximedia-proxy::ProxyGenerator`; `--resolution` + `--codec` args now honored via `ProxyPreset` / `ProxyGenerationSettings`; removed placeholder-text fallback; 3 new tests
- [x] **`oximedia-cli extract`** — Replaced synthetic colored-gradient generator with real container demux + codec decode + YUV→RGB conversion; PNG and JPEG encoders wired; real source resolution used; 3+ new tests

- [x] **PR #23 (Windows build fixes)** — 8 MiB stack reserve (`/STACK:8388608` in `.cargo/config.toml`), `default-members` excluding `oximedia-py` to avoid PDB collision, `#[cfg(unix)]` gates on `ipc/socket.rs` with Windows stubs (2026-05-19)
- [x] **PR #18 (RTSP 1.0 client)** — `crates/oximedia-net/src/rtsp/` (auth, client, message, rtp, sdp, transport, url) + smoke tests; pure Rust, no new deps, IETF RFC-based (2026-05-19)
- [x] **PR #22 (FLV demuxer)** — `crates/oximedia-container/src/demux/flv/` Adobe Flash Video container demuxer; 15 tests; additive (2026-05-19)
- [x] **PR #20/#21 (ProRes 422 parser + decoder)** — `crates/oximedia-codec/src/prores/` (bitreader, frame, picture, quant, entropy, zigzag, dequant, idct, decode); `prores` Cargo feature; SMPTE RDD 36-2015; 66 tests; `docs/prores_decoder.md` (2026-05-19)
- [ ] **PR #19 (HW accel -sys crates)** — deferred to 2027+ (patent-encumbered codec targets: H.264/HEVC/AAC)

*Last updated: 2026-06-02 — v0.1.8 active (Waves 1–20 complete, 100,278 tests, 0 failures, 0 warnings); v0.1.7 stable baseline; 109 crates; `oximedia-ml` stable; feature-gated `onnx`/`cuda`/`webgpu`/`directml`; pure-Rust default preserved; Wave 3: ProRes 422 encoder, FLV muxer, RTSP 1.0 server; Wave 4: Theora promoted to Functional, JPEG 2000 lossless decoder (`jpeg2000`), VC-3/DNxHD decoder (`dnxhd`); Wave 5: FFV1 8/10/12-bit + rayon parallel slices, JPEG 2000 9-7 lossy wavelet (completing ISO 15444-1), JPEG XS decoder (`jpegxs`, ISO 21122-1, `CodecId::JpegXs`); Wave 6: FFV1 16-bit depth (`Yuv*16le` formats), JPEG 2000 multi-tile decode, JPEG-LS lossless decoder (`jpegls`, ISO 14495-1, `CodecId::JpegLs`); Wave 7–20: deep algorithmic additions across 60+ crates — see Wave status in MEMORY.md*

---

## 0.1.7 /ultra Wave 2 slices (planned 2026-05-04)

- [x] **Slice 1**: `oximedia-cv` — add `webgpu` and `directml` features mirroring `oximedia-ml`
- [x] **Slice 2**: `oximedia-cli timeline_cmd` — replace `generate_otio_placeholder` with real OTIO 0.17-compatible JSON serializer
- [x] **Slice 3** (deviated — TCP coordinator server started; full gRPC codegen deferred): `oximedia-distributed` — start gRPC coordinator server in background; workers can connect; unified job store
- [x] **Slices 4+5**: `oximedia-cli restore_cmd` — format-aware audio decode (WAV/FLAC/MP3) + frame-level video restore (deinterlace/upscale/color-correct via FramePipelineConfig)

## 0.1.7 /ultra Wave 3 slices (2026-05-19)

- [x] **Slice 1**: `oximedia-codec` ProRes 422 encoder — SMPTE RDD 36-2015 forward path; `bitwriter.rs`, `fdct.rs`, `quantize.rs`, `entropy_encode.rs`, `encode.rs`, `frame_write.rs`, `encoder.rs`; `PixelFormat::Yuv422p10le` + `CodecId::ProRes` added to `oximedia-core`; `ProResEncoder` implements `VideoEncoder`; 97 tests pass; full encode→decode round-trip verified
- [x] **Slice 2**: `oximedia-container` FLV muxer — `mux/flv/writer.rs` + `amf0.rs`; `ContainerFormat::Flv` + probe magic; supports Mp3/H263/PCM (patent-free only); 7 round-trip tests pass with `FlvDemuxer`
- [x] **Slice 3**: `oximedia-net` RTSP 1.0 server — `rtsp/server/` (state, registry, connection, rtsp_server, auth_server); `try_parse_request` + `Response::encode` added to `message.rs`; `SessionDescription::serialize` added to `sdp.rs`; `RtpPacketBuilder` added to `rtp.rs`; server-side Digest auth in `auth_server.rs`; 20 tests including end-to-end OPTIONS→DESCRIBE→SETUP→PLAY→TEARDOWN integration test

## 0.1.7 /ultra Wave 4 slices (2026-05-19)

- [x] **Slice 1**: `oximedia-codec` Theora encode↔decode bitstream alignment — self-consistent sign-magnitude DC/AC encoding replaces broken Huffman tree; `theora/mod.rs` decode_dct_coefficients + encode_dct_coefficients rewritten to agree on 11-bit DC (sign+magnitude) + 6-bit-run / 10-bit-value / 63-EOB AC scheme; new `tests/theora_roundtrip.rs` (3 tests pass); Theora promoted from "Bitstream-parsing" to "Functional" in `docs/codec_status.md`
- [x] **Slice 2**: `oximedia-codec` JPEG 2000 lossless decoder (5-3 reversible wavelet) — new `jpeg2000/` module (9 files, ~3,500 LoC): `bitreader.rs` (byte-stuffing), `mq_coder.rs` (47-state MQ arithmetic decoder), `markers.rs` (SOC/SIZ/COD/QCD/SOT/SOD/EOC), `box_parser.rs` (JP2 ISOBMFF), `wavelet.rs` (5-3 lifting), `tier1.rs` (EBCOT Tier-1 SPP/MRP/CUP), `tier2.rs` (packet headers), `decoder.rs`; `CodecId::Jpeg2000` added to `oximedia-core`; 47 unit + integration tests pass
- [x] **Slice 3**: `oximedia-codec` VC-3 / DNxHD decoder (SMPTE ST 2019-1) — new `dnxhd/` module (7 files, ~2,100 LoC): `frame_header.rs` (CIDs 1235/1237/1238/1241/1242/1243), `vlc_tables.rs` (DC + MPEG-2 AC tables), `bitreader.rs`, `idct.rs` (Q15 8×8 IDCT), `zigzag.rs`, `entropy.rs` (DC DPCM + AC run/level), `decode.rs` (full pipeline → YUV 4:2:2 planes); `CodecId::Dnxhd` added to `oximedia-core`; 4 integration tests pass; encoder stub pending v0.1.8+

## 0.1.7 /ultra Wave 5 slices (2026-05-19)

- [x] **Slice 1**: `oximedia-codec` FFV1 8 → 8/10/12-bit depth + rayon parallel multi-slice decode — `oximedia-core` gains `Yuv422p12le`/`Yuv444p10le`/`Yuv444p12le`; `ffv1/decoder.rs` pixel-format dispatch extended to 9 arms; 2-byte LE sample read/write paths; `par_iter` multi-slice with per-slice RFC 9043 §3.8.2.2.1-compliant context resets; encoder bit-depth aware; new `tests/ffv1_higher_bit_depth.rs` (4 round-trip tests at 10/12-bit); 16-bit deferred (no `Yuv*p16le` formats yet)
- [x] **Slice 2**: `oximedia-codec` JPEG 2000 lossy 9-7 irreversible wavelet — `jpeg2000/wavelet.rs` gains `WaveletKind` enum, `inverse_wavelet_1d_97`/`inverse_wavelet_2d_97`/`reconstruct_levels_97` with CDF 9/7 lifting (α/β/γ/δ/K constants); `markers.rs` adds `QcdMarker::step_size_for_subband(idx, bit_depth)` decomposing epsilon/mu; `tier1.rs` adds `CodeBlock::dequantize(step_size, decoded_planes)`; `decoder.rs` removes `is_lossless_wavelet()` gate, dispatches 5-3 or 9-7 with `find_qcd`; JPEG 2000 decodes both lossless (5-3) and lossy (9-7) profiles
- [x] **Slice 3**: `oximedia-codec` JPEG XS decoder (ISO/IEC 21122-1) — new `jpegxs/` module (8 files, ~2,394 LoC): `bitreader.rs`, `markers.rs` (SOC/PIH/CDT/WGT/NLT/CWD/SLH/EOC), `vlc.rs`, `wavelet.rs` (self-contained LeGall 5/3), `nlt.rs` (quadratic deferred), `entropy.rs`, `decoder.rs`; `CodecId::JpegXs` (jpegxs/jpeg-xs/jxs); feature `jpegxs = []`; 24 integration + unit tests; encoder deferred v0.1.8+

## 0.1.7 /ultra Wave 6 slices (2026-05-19)

- [x] **Slice 1**: `oximedia-core` adds `Yuv420p16le`/`Yuv422p16le`/`Yuv444p16le` (16-bit LE planar YUV formats, 24/32/48 bpp); `oximedia-codec` FFV1 `pixel_format_for_config()` gains 3 new 16-bit arms completing the 8/10/12/16-bit archival matrix; `tests/ffv1_higher_bit_depth.rs` gains 3 new round-trip tests at 16-bit (all pass, lossless)
- [x] **Slice 2**: `oximedia-codec` JPEG 2000 multi-tile support — `SizMarker` gains `num_tiles_x()`, `num_tiles_y()`, `tile_rect(idx)` helpers; `collect_tile_data()` replaced by `collect_tile_map()` returning `HashMap<u16, Vec<u8>>`; `decode_codestream()` allocates full-frame output buffers and iterates all tiles, each decoded independently then assembled by `tile_rect` copy; `decode_component_53/97()` parameters renamed to `tile_w/tile_h`; `MultiTileOrLayer` error message updated; 5 new tests (single-tile regression, 2-tile horizontal, 4×4 grid, tile geometry helpers, partial-tile geometry)
- [x] **Slice 3**: `oximedia-codec` JPEG-LS decoder (ISO 14495-1, LOCO-I algorithm) — new `jpegls/` module (6 files, ~963 LoC): `mod.rs` (`JlsError`, `JlsResult`), `markers.rs` (SOI/SOF55/LSE/SOS/EOI parser with loop-break-value pattern), `predictor.rs` (LOCO-I edge-detecting predictor + gradient quantizer), `context.rs` (365 regular contexts, sign-normalised index, adaptive bias/k update), `golomb.rs` (`BitReader` with JPEG byte-stuffing, `decode_golomb_unsigned`, `map/unmap_error_lossless`), `decoder.rs` (LOCO-I scan decode pipeline); `CodecId::JpegLs` (lossless, jpegls/jpeg-ls/jls aliases); `jpegls = []` feature; `tests/jpegls_decode.rs` with inline encoder for round-trip tests; patents expired 2017–2019

## 0.1.7 /ultra Wave 7 slices (2026-05-19)

- [x] **Slice 1**: `oximedia-codec` JPEG-LS near-lossless (NEAR > 0) + interleaved multi-component (ILV=1/2) — `jpegls/golomb.rs` adds `map_error_near`/`unmap_error_near` (ISO 14495-1 §A.4); `jpegls/decoder.rs` removes both `Unsupported` guards, extracts `decode_pixel()` helper supporting both lossless and near-lossless paths, adds ILV=0/1/2 dispatch; new `encode_near_lossless_greyscale` + `encode_lossless_multicomponent` test helpers; 4 new tests: `round_trip_near_lossless_1/2`, `round_trip_ilv1_rgb_4x4`, `round_trip_ilv2_rgb_2x2`
- [x] **Slice 2**: `oximedia-codec` JPEG XS NLT quadratic reverse transform (ISO 21122-1 §A.2.2) — `jpegxs/nlt.rs` gains `isqrt64_floor`/`isqrt64_ceil` (Newton-refined integer sqrt), `nlt_quadratic_inverse` (ceiling-sqrt inverse with three-region dispatch); `NltType::Quadratic` replaced from `Err(Unsupported)` to full implementation; `jpegxs/markers.rs` captures raw 5-byte NLT payload in `JxsHeaders::nlt_payload: Option<Vec<u8>>`; `jpegxs/decoder.rs` wires `parse_nlt_payload` + `apply_nlt_reverse` — streams with NLT markers now decode correctly; 20+ new unit tests, 4 integration tests
- [x] **Slice 3**: `oximedia-codec` ProRes 422 decoder API — new `prores/decoder.rs` (~360 LoC): `ProResDecoderConfig` (optional profile validation), `ProResFrame` (8-bit YUV 4:2:2 output with interlaced field assembly), `ProResDecoder` (parses `icpf` frame → picture → slice loop via `decode_slice_to_yuv422`; implements `VideoDecoder` trait send_packet/receive_frame); exported from `prores/mod.rs` + `lib.rs`; `tests/prores_roundtrip.rs` (11 integration tests); `docs/codec_status.md` gains ProRes 422 — Functional section

## 0.1.7 /ultra Wave 8 slices (2026-05-20)

- [x] **Slice 1**: `oximedia-codec` JPEG-LS encoder (ISO 14495-1 LOCO-I forward path) — new `jpegls/golomb_write.rs` (243 LoC: MSB-first `BitWriter` with JPEG byte-stuffing + `encode_golomb_unsigned_limited` exact inverse of decoder side including LIMIT overflow escape), `jpegls/marker_write.rs` (193 LoC: SOI/SOF55/LSE/SOS/EOI writers mirroring `markers.rs`), `jpegls/encoder.rs` (505 LoC: `JpegLsEncoder` + `JpegLsEncoderConfig`, full forward LOCO-I — predict via shared `predict()`, quantize via shared `quantize_gradient`, share 365-context state with the decoder, map errors via shared `map_error_*`, Golomb-encode; ILV 0/1/2 dispatch; public API `encode_planes` / `encode_greyscale` / `encode_planes_u8`); `jpegls/mod.rs` re-exports `JpegLsEncoder`/`JpegLsEncoderConfig`; new `tests/jpegls_encode_roundtrip.rs` with 9 round-trips (lossless gradient/constant, NEAR=2, multicomponent ILV=0/1/2, 12-bit, 1×1, u8 helper) — all byte-exact for lossless; pure regular-mode coding (decoder side does not implement ISO §A.7 run-mode — encoder mirrors that, documented in module header)
- [x] **Slice 2**: `oximedia-codec` MPEG-2 video I-frame decoder (ISO/IEC 13818-2 / H.262, patents expired Feb 2023) — new `mpeg2/` module (~3,400 LoC across 9 files, **self-contained**, does NOT depend on `dnxhd` feature): `bitreader.rs` (MSB-first + start-code scanner), `headers.rs` (sequence header / extension, GOP, picture header / coding-extension, slice header — chroma_format, intra_dc_precision, picture_structure, q_scale_type, alternate_scan, intra_vlc_format), `vlc_tables.rs` (Tables B-12 DC-luma / B-13 DC-chroma / B-14 AC / B-15 alternate-AC written canonically from the standard), `idct.rs` (IEEE-1180-tolerant Q15 8×8 inverse DCT), `zigzag.rs` (progressive Figure 7-2 + alternate Figure 7-3 scans), `dequant.rs` (intra inverse-quant §7.4 with mismatch control / sum-oddification on F[63] §7.4.4), `entropy.rs` (intra macroblock decode: DC DPCM predictor reset to `2^(7+intra_dc_precision)` at slice start, AC run/level VLC + 6+12-bit escape, Table B-1 macroblock_address_increment, Table B-2 I-picture macroblock_type), `decode.rs` (top-level → YUV 4:2:0 planar; `VideoDecoder` impl); `CodecId::Mpeg2` (aliases mpeg2/mpeg-2/m2v/h262) in `oximedia-core`, codec_matrix arms for ts/ps/mpeg/mp4/mkv; `mpeg2 = []` feature (opt-in, not in default); P/B frames + 4:2:2/4:4:4 + field pictures + encoder rejected with `Err` and deferred to v0.1.8+
- [x] **Slice 3**: `oximedia-codec` JPEG XS encoder (ISO/IEC 21122-1) — completes the JPEG XS codec (encoder + decoder pair). New `jpegxs/bitwriter.rs` (MSB-first, no byte-stuffing per XS spec), `jpegxs/marker_write.rs` (SOC/PIH/CDT/WGT/CWD/SLH/EOC writers mirroring `markers.rs`), `jpegxs/vlc_encode.rs` (forward VLC, exact inverse of decoder entropy), `jpegxs/encoder.rs` (`JpegXsEncoder` + `JpegXsEncoderConfig`; forward pipeline: per-component forward 5/3 DWT → weight-quantize → VLC-encode → slice assembly → marker emission); forward 5/3 DWT (`forward_wavelet_1d`/`forward_wavelet_2d`, LeGall lifting) added to `jpegxs/wavelet.rs`; `jpegxs/mod.rs` re-exports `JpegXsEncoder`/`JpegXsEncoderConfig`; new `tests/jpegxs_encode_roundtrip.rs` — markers PIH write→parse, wavelet 5/3 forward+inverse identity, VLC encode↔decode, gradient 32×16 unit-weight lossless round-trip byte-exact, random 64×32 lossless round-trip, constant grey 16×16 ±2 LSB, odd dimensions

## 0.1.7 /ultra Wave 9 slices (2026-05-21)

- [x] **Slice 1**: `oximedia-codec` MPEG-2 I-frame encoder (ISO/IEC 13818-2 / H.262 forward path) — completes the MPEG-2 I-frame codec pair started in Wave 8. New `mpeg2/bitwriter.rs` (~211 LoC: MSB-first writer + start-code emit, no byte-stuffing per MPEG-2 video ES), `mpeg2/fdct.rs` (~199 LoC: forward 8×8 DCT matched to the Q15 IDCT, IEEE-1180-tolerant FDCT↔IDCT identity recovers DC exactly), `mpeg2/quantize_fwd.rs` (~200 LoC: forward §7.4 intra quant — `QF[0]=round(F[0]/intra_dc_mult)`, `QF[u,v]=round(16·F/(W·q_scale))`, clamp to ±2047, default intra matrix), `mpeg2/vlc_encode.rs` (~354 LoC: forward VLC — B-12/B-13 DC size+diff, B-14/B-15 AC run/level; duplicate inverse-table codeword pairs route through the 6-bit-run / 12-bit-signed-level escape so the Wave 8 decoder accepts every encoded entry), `mpeg2/marker_write.rs` (~292 LoC: sequence_header / sequence_extension at chroma_format=4:2:0 / picture_header / picture_coding_extension at intra_dc_precision + progressive + f_codes=0xF / slice_header), `mpeg2/encoder.rs` (~670 LoC: `Mpeg2Encoder` + `Mpeg2EncoderConfig`; full forward pipeline per macroblock — split planes → 4 luma 8×8 + 2 chroma 8×8 → FDCT → forward quant → progressive zigzag → DC DPCM + AC run/level VLC → slice / marker emission; DC predictor resets to `2^(7+intra_dc_precision)` at every slice; implements `VideoEncoder` trait); `Mpeg2Error::{InvalidConfig, Encode}` added; 9 integration round-trip tests in `tests/mpeg2_encode_roundtrip.rs` pass under the `mpeg2` feature (encoder→Wave 8 decoder round-trip verified for flat / DC / gradient frames within bounded LSB tolerance, since FDCT+quant are lossy)
- [x] **Slice 2**: `oximedia-codec` ALAC — Apple Lossless audio (encoder + decoder, Apache-2.0 patent-free) — new greenfield module (~2,200 LoC across 8 files in `crates/oximedia-codec/src/alac/`): `mod.rs` (`AlacError` / `AlacResult` + re-exports), `config.rs` (`AlacSpecificConfig` — the 24-byte big-endian "magic cookie": `frameLength`/`compatibleVersion`/`bitDepth`/`pb`/`mb`/`kb` Rice tuning / `numChannels`/`maxRun`/`maxFrameBytes`/`avgBitRate`/`sampleRate`), `bitstream.rs` (MSB-first `BitReader` + `BitWriter`, no stuffing), `rice.rs` (adaptive modified-Rice / Golomb encode + decode with `k`-history update and escape-to-fixed-bits path), `lpc.rs` (adaptive sign-LMS FIR predictor, mode 0 — rare extended modes rejected with `AlacError::Unsupported`), `mix.rs` (inter-channel decorrelation via `interlacing_shift` + `interlacing_leftweight`, exact integer mid/side ↔ left/right inverse), `decoder.rs` (`AlacDecoder` — per-frame element decode with compressed / uncompressed-escape / constant paths, 16/20/24-bit interleaved i32 PCM output, mono + 2-channel decorrelation), `encoder.rs` (`AlacEncoder` + `AlacEncoderConfig` mirroring the decoder, picks uncompressed when smaller); `CodecId::Alac` (lossless audio; aliases `alac` / `m4a-alac`) + `codec_matrix` arms for mp4 / m4a / mov / caf / mkv added to `oximedia-core`; `alac = []` feature (opt-in, not in default); `tests/alac_roundtrip.rs` with 11 integration tests (config cookie round-trip, mono 16-bit sine, stereo 16-bit decorrelated, 24-bit stereo, constant block, random-noise escape, Rice-`k` sweep, truncated-frame rejection) all byte-exact for 16/20/24-bit; 32-bit and the rare extended predictor modes are explicit `Unsupported` and tracked as follow-ups; the encoder uses a fixed-`k` remainder path (Apple's variable-remainder path is decoder-side optional)
- [x] **Slice 3**: `oximedia-codec` JPEG 2000 lossless (5-3) encoder (ISO/IEC 15444-1 forward path) — completes the JPEG 2000 lossless codec pair started in Wave 4. New `jpeg2000/mq_encoder.rs` (~346 LoC: MQ arithmetic ENCODER per ISO 15444-1 Annex C — full carry propagation, 0xFF stuffing, shares the `MQ_TABLE` Qe / NMPS / NLPS / SWITCH constants with the existing 47-state decoder, `flush()`), `jpeg2000/tier1_encode.rs` (~563 LoC: forward EBCOT — per-codeblock bit-plane scan + significance / magnitude-refinement / cleanup passes feeding the MQ encoder; context labels identical to the decode side), `jpeg2000/tier2_encode.rs` (~245 LoC: `J2kBitWriter` + forward tag-tree coding + packet-header emission, fixed `lblock=3` block-length signalling, single-layer LRCP), `jpeg2000/marker_write.rs` (~227 LoC: SOC / SIZ / COD / QCD / SOT / SOD / EOC writers mirroring `markers.rs`; raw `.j2k` codestream — JP2 box wrapping deferred), `jpeg2000/encoder.rs` (~393 LoC: `Jpeg2000Encoder` + `Jpeg2000EncoderConfig { levels, tile_size, lossless }`; forward pipeline — per-component DC level-shift → forward 5-3 LeGall DWT → per-subband codeblock partition → Tier-1 encode → Tier-2 packets → markers); forward 5-3 DWT (`forward_wavelet_2d`, `decompose_levels`) added to `wavelet.rs`. In-scope existing-file fixes (all inside `jpeg2000/`): a non-standard INITDEC/BYTEIN/DECODE/RENORMD path in `mq_coder.rs` that prevented decoding a 0 for the first decision of a fresh context, `MQ_TABLE` raised to `pub(crate)`, `decoder.rs` removed the `num_levels==0` rejection, `markers.rs` now honours `Psot>0` as a tile-part length delimiter — all necessary for the encoder ↔ decoder round-trip. Encode → decode is byte-exact on the lossless subset (single-layer LRCP, even dimensions; odd dimensions limited to 0–1 decomposition levels by the existing decoder); multi-component encode is constrained by the decoder's single-tile-body assumption. 9 integration round-trip tests in `tests/jpeg2000_encode_roundtrip.rs` pass under the `jpeg2000` feature. Lossy 9-7 encoder, multi-layer / progression encode, and JP2 box wrapping deferred to follow-ups.

## 0.1.7 /ultra Wave 10 slices (2026-05-21)

- [x] **Slice 1**: `oximedia-codec` MPEG-2 4:2:2 + 4:4:4 chroma formats (decoder + encoder, ISO/IEC 13818-2 §6.1.1.4 Table 6-10) — promotes MPEG-2 from "4:2:0 only" to "4:2:0 + 4:2:2 + 4:4:4" on both decode and encode. Files modified (all inside `crates/oximedia-codec/src/mpeg2/`): `headers.rs` lifts the `chroma_format != 1` rejection guard to accept `1..=3`; `decode.rs` dispatches the per-MB block-list on chroma_format (6 / 8 / 12 blocks = 4 luma + 1/2/4 Cb + 1/2/4 Cr), computes per-component block origins for 4:2:2 (chroma 8×16, two vertically-stacked 8×8: Cb_top, Cb_bot, Cr_top, Cr_bot) and 4:4:4 (chroma 16×16, 2×2 tiles in raster order), and parameterises `output_format()` + `VideoFrame::new` on the stored chroma_format (`Yuv420p` / `Yuv422p` / `Yuv444p`); `encoder.rs` gains `Mpeg2EncoderConfig.chroma_format: u8` (default 1) with `yuv420p()`/`yuv422p()`/`yuv444p()` factory shortcuts, relaxes the input `frame.format` check to all three YUV planar formats, and mirrors the decoder's block-list + origin dispatch in `encode_macroblock`; `marker_write.rs` adds `CHROMA_FORMAT_422 = 2` / `CHROMA_FORMAT_444 = 3` constants and `write_sequence_extension()` writes the configured value. All Wave 8 + Wave 9 4:2:0 tests continue to pass; 6 new integration tests + 4 new unit tests cover flat / gradient round-trips for Yuv422p and Yuv444p, sequence_extension write/parse for all three chroma formats, and decoder header acceptance. Patent-free (MPEG-2 patents expired Feb 2023). P/B frames + field pictures remain deferred to v0.1.8+
- [x] **Slice 2**: `oximedia-codec` JPEG 2000 lossy (9-7) encoder (ISO/IEC 15444-1 forward path) — completes the JPEG 2000 lossy codec pair (decoder shipped Wave 5; lossless encoder shipped Wave 9). New `jpeg2000/quantize_fwd.rs` (~155 LoC: `quantize_subband_97(coeffs, step_size, num_bit_planes)` — exact inverse of `tier1.rs::dequantize`, mid-tread sign-magnitude i32 quantiser); `wavelet.rs` promotes the private `forward_97` to public `forward_wavelet_1d_97` and adds `forward_wavelet_2d_97` + `decompose_levels_97` mirroring the existing 5-3 forward shape but using the same α/β/γ/δ/K/K_INV CDF 9/7 lifting constants as the inverse path; `marker_write.rs` adds `write_qcd_lossy` (Sqcd style 2 expounded, per-subband 16-bit ε/μ pairs) and `write_cod_lossy` (kernel byte 0 = 9-7) alongside the existing lossless writers (sharing a private `write_cod_with_filter` helper); `encoder.rs` dispatches on `Jpeg2000EncoderConfig.lossless`: lossless → existing 5-3 path, lossy → 9-7 (`decompose_levels_97` → `quantize_subband_97` per subband → existing Tier-1 EBCOT → existing Tier-2 → lossy COD + QCD writers); the lossy path picks a single global ε = 8, μ = 0 → uniform `Δ_b = 2^(R_b − 8)` step sizes per ISO 15444-1 §E.1; `mct = 0` (no color transform) matches the lossless path. Encode → decode within ±2 LSB at 1 decomposition level on flat 16×16 and PSNR ≥ 35 dB on 32×32 gradient at 3 levels; 6 new integration tests + 13 new unit tests across `wavelet`, `quantize_fwd`, `marker_write`. Multi-component lossy (ICT), multi-layer / progression, JP2 box wrapping remain deferred follow-ups
- [x] **Slice 3**: `oximedia-codec` JPEG-LS RUN mode (ISO/IEC 14495-1 §A.7) on both encoder + decoder — promotes JPEG-LS from "regular only" (§A.6) to "regular + RUN" (§A.6 + §A.7), matching the full ISO 14495-1 entropy model. Flat regions now compress exponentially better via §A.7 length tokens. New `jpegls/run_mode.rs` (~335 LoC: ISO Table A.5 `J: [i32; 31]`, `RUN_THRESHOLD[r] = 1 << J[r]`, `RunState { run_index, run_value }`, `enter_run_lossless(d1,d2,d3)` / `enter_run_near(d1,d2,d3,near)` raw-gradient entry tests, `bump_run_index` capped at 30, `run_termination_ctx(ra, rb)` → ctx 365 if Ra==Rb else 366, 12 unit tests); `context.rs` extends the per-component `ContextState` array to 367 entries (365 regular + 2 RUN termination); `decoder.rs` and `encoder.rs` each dispatch RUN mode at the top of their per-pixel inner loop — when the three raw gradients all stay within NEAR: count consecutive matching samples, Golomb-encode the run length with `k = J[run_index]`, increment `run_index` per full token, terminate with the residual length plus the breaking sample under context 365/366; per-line `run_index` reset matches the spec. ILV=0 (non-interleaved) and ILV=1 (line-interleaved) exercise RUN mode; ILV=2 (sample-interleaved) intentionally suspends RUN per the CharLS reference convention. The pre-existing flat-region tests `round_trip_constant_grey_8x8` and `roundtrip_lossless_16x16_constant` remain byte-exact (decoded pixels = input; only the encoded stream is shorter). New `tests/jpegls_runmode_roundtrip.rs` with 8 integration tests (constant 32×32 lossless smaller than baseline, stripes 32×32, two-color columns exercising ctx 366, near-lossless flat 24×24 NEAR=2, ILV=1 RGB constant, zero-length runs at line start, long-run 64×64, gradient-then-flat); the 3 inline test encoders in `tests/jpegls_decode.rs` were rewritten to delegate to the production `JpegLsEncoder` rather than reimplementing §A.7 inside the test fixture. Patent-free (HP patents expired 2017–2019)

## Refinement proposals (added 2026-05-04 by /ultra)

### Refinement 1 — 7 pre-existing UU files (HIGH priority)

The git index has unresolved (UU) stage entries for these files. Working-tree blobs are conflict-marker-free and compile cleanly — they are "resolved-but-unstaged". The merge resolution exists in the working tree but was never `git add`-ed.

Files affected:
- `crates/oximedia-accel/src/cpu_fallback.rs` (1350 lines)
- `crates/oximedia-farm/src/worker_health_check.rs` (887 lines)
- `crates/oximedia-graphics/src/text.rs` (736 lines)
- `crates/oximedia-lut/src/aces.rs` (888 lines)
- `crates/oximedia-net/src/dash/client.rs` (1096 lines)
- `crates/oximedia-py/src/context_manager.rs` (627 lines)
- `oximedia-cli/src/image_cmd.rs` (1782 lines)

Options:
1. **Stage as-is** — `git add <file>` for each (the working-tree resolution is what we want to keep).
2. **Discard the resolution** — `git restore --staged --source <ref> <file>` to roll back.
3. **Manual review** — open `git mergetool` for each file and verify the resolution.

This run did NOT touch any of these 7 files. The UU staging decision belongs to the user.

### Refinement 2 — Greenfield crates (5 candidates)

**Stale claim** (2026-05-19 audit): All five crates are 23–27 k SLOC each with hundreds of in-source tests and zero stub markers. The original "0/N items done" tally referred to a different enhancement checklist that was never reconciled. No further action required — all five crates are substantively implemented.

- `oximedia-repair` (27k SLOC, 770 in-source tests, 4 integration tests — **truly stable**)
- `oximedia-review` (27k SLOC, 776 in-source tests — **truly stable**)
- `oximedia-playlist` (24k SLOC, 670 in-source tests — **truly stable**)
- `oximedia-profiler` (23k SLOC, 594 in-source tests — **truly stable**)
- `oximedia-proxy` (23k SLOC, 754 in-source tests — **truly stable**)

### Refinement 3 — oximedia-cli polish (deferred until UU staged)

34 pending items including conform_cmd, cloud_cmd, scopes_cmd, normalize_cmd, JSON output ergonomics, man pages, completions, cargo-dist. Deferred because `oximedia-cli/src/image_cmd.rs` is UU.

### Refinement 4 — oximedia-py ergonomics (deferred until UU staged)

30 pending items including buffer-protocol numpy zero-copy, type-stub .pyi generation, context-manager protocol. Deferred because `oximedia-py/src/context_manager.rs` is UU.

### Refinement 5 — Documentation singletons (defer to /readme)

Singleton fixes (`oximedia-monitor/cardinality.rs`, `oximedia-bitstream/integer.rs`) are too small to justify a standalone slice. Bundle with the next `/readme` pass.

## 0.1.8 Wave 15 Slice E — `oximedia-dolbyvision` SIMD IPT↔PQ (2026-06-01)

- [x] `oximedia-dolbyvision` SIMD IPT↔PQ batch conversion — new `ipt_pq_simd.rs` (1011 L) with runtime-dispatched AVX2+FMA / SSE4.1 / NEON / scalar paths; matrix multiplies fully vectorised with `_mm256_fmadd_ps`; PQ OETF/EOTF per-lane scalar within SIMD functions; `ipt_pq_batch_simd(input, output, direction)` public API. Files: `crates/oximedia-dolbyvision/src/ipt_pq_simd.rs`, `src/lib.rs`
- [x] Cache parsed RPU structures — `RPU_CACHE: OnceLock<Mutex<HashMap<…>>>` process-global cache; `parse_nal_unit_cached` and `parse_rpu_bitstream_cached` FNV-1a hash key + LRU-style 256-entry eviction. File: `crates/oximedia-dolbyvision/src/parser.rs:43-131`
- [x] 4 new tests: `test_ipt_pq_simd_matches_scalar` (256 LCG pixels, ≤1e-4 tol), `test_ipt_pq_roundtrip` (10 representative pixels, ≤1e-3), `test_l1_through_l11_metadata_roundtrip` (write→parse structural equality for L1/L2/L5/L6/L8/L9/L11), `test_parser_robustness_random_bytes` (50 random slices 0–1000 B, no panic). All 966 tests pass, 0 clippy warnings.

## 0.1.8 Wave 15 Slice B — `oximedia-calibrate` algorithmic depth (2026-06-01)

- [x] `oximedia-calibrate` ICC tile parallelism — `apply_to_image` converted from sequential `chunks_exact` to `par_chunks_exact_mut` via rayon; per-pixel transform is stateless and embarrassingly parallel; output is bit-exact to scalar path. File: `crates/oximedia-calibrate/src/icc/apply.rs`
- [x] `oximedia-calibrate` chromatic adaptation matrix cache — thread-safe `OnceLock<Mutex<HashMap<(Illuminant, Illuminant, ChromaticAdaptationMethod), Matrix3x3>>>` module-level cache; `ChromaticAdaptation::new` checks cache first, computes on miss; `Illuminant` and `ChromaticAdaptationMethod` both derive `Hash`. Files: `crates/oximedia-calibrate/src/chromatic/adapt.rs`, `src/lib.rs`
- [x] 6 Wave 15 integration tests in `crates/oximedia-calibrate/tests/wave15_tests.rs` — all 659 tests pass, 0 clippy warnings

## 0.1.8 Wave 15 Slice H — `oximedia-profiler` thread-local sampling + atomic allocation tracker (2026-06-01)

- [x] `oximedia-profiler` `SamplingProfiler` — replaced direct `samples`/`hit_counts` mutation with `thread_local!` TLS staging buffers (`TL_SAMPLES: RefCell<Vec<SampleEvent>>`, `TL_HIT_COUNTS: RefCell<HashMap<String,u64>>`); `record()` writes to TLS; `merge_thread_local()` drains current thread's TLS into global aggregate; `stop()` calls `merge_thread_local()` before clearing `running`. Removed `#![allow(dead_code)]`. File: `crates/oximedia-profiler/src/sampling_profiler.rs`
- [x] `oximedia-profiler` `AllocationTracker` — atomicised three scalar counters (`seq_counter: AtomicU64`, `current_bytes: AtomicU64`, `peak_bytes: AtomicU64`); `record()` and `free()` now take `&self` (lock-free on hot counters); `peak_bytes` updated via CAS-max loop; `records: Vec<AllocRecord>` stays behind `Mutex`; manual `Default` impl; removed `#![allow(dead_code)]`. File: `crates/oximedia-profiler/src/allocation_tracker.rs`
- [x] 5 new tests: `test_merge_thread_local_drains_to_aggregate`, `test_explicit_merge_then_stop_no_double_count`, `test_thread_local_sampling_concurrent` (4 threads × 100 events = 400 total), `test_atomic_counter_concurrent` (8 threads × 1000 records × 100 B = 800_000 B), `test_atomic_peak_bytes_correctness` (100B + 200B → peak=300). 604 tests pass, 0 clippy warnings.

## 0.1.8 Wave 15 Slice F — `oximedia-analysis` ring-buffer + parallel dispatch (2026-06-01)

- [x] Ring-buffer bounded temporal analysis — `TemporalWindow.means`, `TemporalAnalysis.luma_means`, and `TemporalAnalyzer.brightness_history` converted from unbounded `Vec<f64>` to fixed-capacity `VecDeque<f64>` (`MAX_WINDOW=300`, `BRIGHTNESS_HISTORY_CAP=900`); `pop_front` + `push_back` on overflow; all downstream methods (`detect_cuts`, `compute_flicker_score`, `compute_motion_score`) rewritten to iterate over VecDeque via `.iter().zip()` pairs instead of `.windows(2)`. Files: `crates/oximedia-analysis/src/temporal_analysis.rs`, `crates/oximedia-analysis/src/temporal.rs`
- [x] Parallel sub-analyzer dispatch — `Analyzer::process_video_frame` converted from sequential to concurrent using `rayon::scope`; each of the 8 independent sub-analyzers (scene, black, quality, classifier, thumbnail, motion, color, temporal) dispatched as a separate rayon task borrowing its own struct field; errors collected post-scope and first error propagated. File: `crates/oximedia-analysis/src/lib.rs`
- [x] Use integer histograms instead of float arrays — `type Histogram = [usize; 256]` already in `scene.rs:225`; k-means cluster assignment (not float histogram bins) already in `color.rs:201`; verified integer histogram path is the sole code path for scene change detection. Files: `crates/oximedia-analysis/src/scene.rs`, `crates/oximedia-analysis/src/color.rs`
- [x] 3 regression tests added: `test_ring_buffer_bounds_memory` (1000-frame push, assert ≤ MAX_WINDOW), `test_ring_buffer_stats_match_unbounded_over_window` (50-frame push, mean check ≤ 1e-9), `test_parallel_sub_analyzers_match_sequential` (100×100 RGBA, 5-frame run, structural equality); 770 tests pass, 0 warnings, 0 clippy findings.

## 0.1.8 Wave 20 (completed 2026-06-02)

- [x] `oximedia-access` — speech-clarity biquad DRC + SIMD contrast: 4th-order Butterworth band-pass (300–3400 Hz) + downward DRC for `enhance_speech`; AVX2/NEON/scalar 256-entry gamma LUT for contrast `enhance`; helpers (`calculate_snr`, `speech_clarity_index`, `estimate_sti`) wired
- [x] `oximedia-scaling` — `scale_tiled` + `scale_reference`: cache-blocked tiled downscale with rayon `par_iter`; bit-exact vs reference; 7 tests
- [x] `oximedia-archive-pro` — DataCite 4.x metadata (`datacite.rs`), PBCore 2.1 crosswalk (`metadata_crosswalk.rs`), `MigrationTriggerPolicy` (`risk/migration_trigger.rs`); 18 tests
- [x] `oximedia-proxy` — `batch_conform` + `BatchConformResult` (conform/engine.rs), `ProxyDbExport` + `import_with_rebase` (proxy_sync.rs); 9 tests
- [x] `oximedia-convert` — `SegmentPlan` + `encode_segments_parallel` (segment_encoder.rs); rayon parallel per-segment encoding; codec-agnostic concat; 5 tests
- [x] `oximedia-align` — `phase_correlate_1d` switched to `oxifft::rfft`/`irfft` (N/2+1 bins); `phase_correlate_1d_full_complex` kept as regression reference; 4 tests
- [x] `oximedia-analysis` — `AnalysisScale { Full, Half, Quarter }` enum; `downsample_box_luma` + `downsample_box_channels`; `AnalysisConfig.analysis_scale`; 6 tests
- [x] `oximedia-codec` (NSQ fix) — SILK LTP coarse-to-fine decimated pitch search, per-subframe contour RD, fractional-lag parabolic interpolation, encode→decode round-trip harness (`tests/silk_ltp_roundtrip.rs`)
- [x] **100,278 tests passing**, 0 failures, 0 clippy warnings

## 0.1.8 Wave 4 deferrals (carried forward)
> Added 2026-05-29 by /ultra Wave 5. These items were scoped in Wave 4 but deferred to research / 0.2.0+.

- [x] SILK encoder 1 kHz LTP architecture redesign — Wave 4 delivered 440 Hz=6.91 dB / 1 kHz=3.09 dB structural floor; true 1 kHz fix requires LTP min-lag re-derivation (research-grade). **Spec-floor clarification (2026-06-01):** 1 kHz synthetic tone is above SILK's max trackable pitch (internal_rate÷min_lag ≈ 500 Hz at WB) — the 3.09 dB floor is a spec constraint, not a bug, and cannot be fixed without violating RFC 6716. Real LTP quality improvements (coarse-to-fine pitch search, per-subframe contour RD, fractional-lag refinement, round-trip harness) are tracked in crates/oximedia-codec/TODO.md Wave 19 section. File: crates/oximedia-codec/src/opus/silk_ltp.rs (live path; src/audio/silk/ path in this entry is stale).
- [ ] Vorbis I spec-compliant encoder rewrite — current Vorbis encoder is an OxiMedia-internal format wrapper; full spec compliance requires psychoacoustic flooring + residue codebook design. Gated to 0.2.0+. File: crates/oximedia-codec/src/audio/vorbis/
- [ ] LARSCH pure O(n) line-breaking proof — existing LARSCH is O(n log n) two-pass; strict O(n) requires KP cost function is strictly totally-Monge (formal proof needed). Research-grade. File: crates/oximedia-caption-gen/src/line_breaking/larsch.rs
- [x] Motion blur Wiener deconvolution ≥3 dB — spatial-domain RL diverged in Wave 4; FFT-Wiener via oxifft::rfft2d/irfft2d implemented in Wave 5 β₁ (8/15 corpus ≥3 dB). File: crates/oximedia-cv/src/motion_blur/deconvolve.rs (verified 2026-05-29)

## 0.1.9 Wave 27 (in progress)

- [x] `oximedia-audio-analysis` emotion synthetic-signal pins — added in-file tests `test_emotion_scores_high_arousal_vs_calm_ordinal` (ordinal: aroused>calm for angry/happy, calm>aroused for sad/neutral; dominant-emotion direction) and `test_emotion_scores_monotone_in_f0` (angry non-decreasing, sad non-increasing as F0 sweeps 100→260 Hz); directions verified against the `detect_emotion_scores` scoring math. File: crates/oximedia-audio-analysis/src/voice/emotion.rs
- [x] `oximedia-audio-analysis` forensics splice end-to-end — new `tests/forensics_splice.rs`: `spliced_audio_detected_near_boundary` (300→3000 Hz splice at 0.5 s detected within ±0.05 s), `continuous_sine_no_splice` (clean 440 Hz → 0 edits), `authenticity_verifier_scores_spliced_lower_than_clean`. **Surfaced + fixed a REAL bug:** `lib.rs::hann_window` used `cos(PI·i/(N−1))` instead of `cos(2·PI·i/(N−1))`, degenerating into a half-cosine ramp. The broken window leaked a pure tone across the whole spectrum (440 Hz tone reported a ~2237 Hz centroid swinging 607→3068 Hz), producing **57 false-positive splices on a clean sine**. Fixed Hann → centroid 444.8 Hz (stable 440–449), false positives → 0. File: crates/oximedia-audio-analysis/src/lib.rs (also tightened `test_window_generation` to assert symmetric taper)
- [x] `oximedia-audio-analysis` vocal separation direction + energy — new `tests/vocal_separation.rs`: `harmonic_output_correlates_more_with_vocal_than_instrumental` (r_voc>r_ins, r_voc>0.3 on steady middle segment) and `separation_preserves_energy_order` (rms(harmonic) > 0.01·rms(mix)). **Surfaced + fixed a REAL bug:** `sources.rs::synthesize` divided IFFT output by `window_size` even though `oxifft::ifft` already normalizes by 1/N, attenuating separated sources by ~2048×. Removed the redundant `/window_size`; harmonic energy restored. File: crates/oximedia-audio-analysis/src/separate/sources.rs
- [x] Per-crate gate: 677/678 tests pass + 0 clippy warnings. The one remaining failure (`formant::analyze::tests::test_lpc_all_pole_synthetic`) is **pre-existing and out of this slice's scope** — a sign-convention mismatch in another slice's test assertion (production `compute_lpc` stores prediction-polynomial-sign coefficients, i.e. `a[1]≈−a1_true`; the test asserts `≈+a1_true`). The production LPC is internally consistent. File (for the owning slice): crates/oximedia-audio-analysis/src/formant/analyze.rs:643

## Stubs to implement (added 2026-06-12 by /cooljapan-stub-check) — ALL RESOLVED 2026-06-22 (/ultra Wave 28)

All 10 stub/placeholder items resolved. Three were stale-label false-positives (already implemented); the rest received real, honest implementations. Each crate verified green (`cargo test -p <crate>` + `cargo clippy ... -D warnings`). Two pre-existing broken doctests (never run because `nextest` skips doctests) were fixed as a bonus.

- [x] `oximedia-graphics`: `svg_overlay.rs` — FALSE-POSITIVE: module was already registered in `lib.rs` (line 125) and `resvg = "0.47"` already a dependency; only a stale "orphan/requires deps not yet listed" header comment remained. Removed the misleading header; 18 doctests + crate tests green. (2026-06-22)
- [x] `oximedia-net`: `rtsp/server/connection.rs` — Real fix: RTP-over-TCP-interleaved transport split via `TcpStream::into_split()` with `Arc<Mutex<OwnedWriteHalf>>`; dedicated `rtp_writer_loop` task (AtomicBool stop + `Notify`, biased `select!`, hard-abort on drop) now delivers RTP concurrently after PLAY without waiting for the next request. Removed stale `let _ = rtp_rx;` workaround + 2 dead helpers. +1 test (`test_rtp_delivered_after_play_without_further_requests`); 71 rtsp + 21 smoke tests pass. (2026-06-22)
- [x] `oximedia-dedup`: `dedup_policy.rs` — Real fix: new `quality.rs` with `quality_score(path)` from real file-header reads (image dims for PNG/JPEG/GIF/BMP/WebP; ISO-BMFF `tkhd` resolution + `mvhd` duration → effective bitrate; bit depth). Log2-weighted (resolution primary, bitrate secondary, depth tie-break) so a tiny 4K beats a huge SD; honest size-only fallback when unparseable (documented). +15 tests; 930 lib tests pass. (2026-06-22)
- [x] `oximedia-analytics`: `engagement.rs` — Real fix: killed hardcoded `0.5`. New `SocialSignals { views, likes, shares, comments }` with saturating `engagement_score()` (zero views → 0.0); `compute_engagement_with_social()` entry point; `EngagementWeights::redistribute_social()` so absent social data redistributes its weight (no fabricated midpoint) and is distinguished from measured-zero. +14 tests; 376 lib tests pass. (2026-06-22)
- [x] `oximedia-audio`: `click_remove.rs` — Real fix: `magnitude` (was `1.0`) now `region_magnitude()` = region peak-abs ÷ RMS of adjacent clean context (peak fallback when no context). 1430+ tests pass. (2026-06-22)
- [x] `oximedia-container`: `mux/mp4/facade.rs` — FALSE-POSITIVE: the `:8` reference is a doc-comment cross-link, not a stub; the facade muxer is fully implemented (ftyp+moov+mdat, all boxes) with 8 passing tests. Verified green; crate TODO already marks mp4-muxer `[x]`. (2026-06-22)
- [x] `oximedia-convert`: `pipeline/job.rs` — Real fix: added `total_frames: Option<u64>` field (`#[serde(default)]` for forward-compat with persisted queues), `set_total_frames`/`with_total_frames`; checkpoint now records the real probed total (0 = honestly unknown, not fake `1_000`) and derives `frames_processed` from it. Restored in `resume_from_checkpoint`. +2 tests; 941 lib tests pass. (2026-06-22)
- [x] `oximedia-image`: `dng/writer.rs` — FALSE-POSITIVE: the deferred camera-model/software tag offsets ARE back-patched by the deferred-offset loop (`writer.rs:232-235`); the `// placeholder` comments describe construction-time zeros filled later. Verified green (1439 tests). (2026-06-22)
- [x] `oximedia-codec`: `opus/silk_nsq.rs` — Real fix: added test-only decoder hook `reconstruct_subframe_from_excitation` and `test_nsq_decoder_bitexact_consistency` proving excitation **bit-exact**, LPC residual **bit-exact**, and LPC-synthesis output **<1 ULP** (sub-ULP gap honestly documented as IEEE-754 reassociation). Note: `silk_nsq` is behind the non-default `opus` feature → verify with `--features opus`. 13 + 99 silk tests pass, clippy clean. (2026-06-22)
- [x] `oximedia-playout`: `catchup.rs` — Real fix: `StartoverSession::new` now sets `started_at = SystemTime::now()`; added `new_at(...)` ctor to anchor to an explicit EPG/schedule start time. 929 tests pass. (2026-06-22)

### Bonus — pre-existing broken doctests fixed (latent: `nextest` skips doctests; `cargo test` runs them)
- [x] `oximedia-graphics`: `bitmap_font.rs` doctest — `let font` → `let mut font` (`render_text` takes `&mut self`). (2026-06-22)
- [x] `oximedia-dedup`: `lib.rs` crate-level doctest — used `sqlite`-gated `DuplicateDetector` under default features; marked the example ```ignore``` with a feature note. (2026-06-22)

## Stubs to implement (added 2026-06-22 by /cooljapan-stub-check)

> Merge note (2026-06-23): the dedup + codec items below duplicate stubs already resolved in /ultra Wave 28 (see the RESOLVED section above) — marked done here for cross-reference. The `oximedia-bitstream` item is genuinely new and remains open.

- [x] `oximedia-dedup`: `crates/oximedia-dedup/src/dedup_policy.rs:441` — codec-aware quality scoring. RESOLVED in Wave 28 (new `quality.rs::quality_score` ranks by real header-derived resolution/bitrate/depth; size fallback). (2026-06-22)
- [x] `oximedia-codec`: `crates/oximedia-codec/src/opus/silk_nsq.rs:1120` — bit-exact SilkDecoder consistency test. RESOLVED in Wave 28 (`#[cfg(test)] pub(crate)` `reconstruct_subframe_from_excitation` hook + `test_nsq_decoder_bitexact_consistency`). (2026-06-22)
- [ ] **oximedia** `oximedia-bitstream`: `crates/oximedia-bitstream/src/integer.rs:510` — `TODO`: `enable these in the future` (commented-out `shl_default`/`shr_default` unbounded-shift methods)
  - **Priority:** P2  **Scope:** trivial  **Cross-project:** none
  - **Approach:** Un-comment the `shl_default`/`shr_default` arms (backed by `unbounded_shl`/`unbounded_shr`) once the MSRV provides the stabilized API, and add round-trip cases exercising shift ≥ BITS.
  - **Risk:** `unbounded_shl`/`unbounded_shr` MSRV gating — guard or bump MSRV before enabling to avoid breaking the build.

## 0.1.9 oximedia-web phase (2026-07-12)

New nested workspace `web/` (excluded from the root workspace via root
`Cargo.toml`'s `exclude = ["fuzz", "web"]`), plus the `oximedia-wasm`
defect-fix and root-tokio-fix work that unblocked it. Full detail in
[`web/TODO.md`](web/TODO.md); summary here for cross-reference.

- [x] **M0** — gates + skeleton: 5-crate nested Cargo workspace
  (`oximedia-web-core` + `scopes`/`color`/`scale`/`quality`), 4 bash-3.2
  gate scripts (`build.sh`/`size-gate.sh`/`dep-gate.sh`/`serve.sh`),
  `allowed-deps.txt` (23 crates, generated from a real `cargo tree`),
  `deny.toml` licenses check green, `package.json`. All 10 requested
  verification steps passed.
- [x] **M1** — `oximedia-web-scopes`: waveform/vectorscope/histogram/
  false-colour ported from `oximedia-scopes`, allocation-free after
  warm-up, three known upstream bugs fixed in the port. 39 tests pass;
  wasm 21,669 B gzip (14% of soft budget).
- [x] **M2** — OxiScope demo (`web/demo/`): grading + all four scopes fed
  from graded output, `.cube` export, verified end-to-end in headless
  Chrome (tone-map roll-off provable on the waveform).
- [x] **M3** — `oximedia-web-color`: exposure/contrast/saturation, 6
  tone-map operators (incl. Narkowicz `aces` and RRT/ODT-shaped
  `aces-odt`), gamut mapping, `.cube` load/export. 106 tests pass; wasm
  59,389 B gzip (29% of soft budget). Known shortfall: 6 ms/1080p wasm
  perf target not met (~44 ms measured; documented safe-scalar-wasm
  limitation).
- [x] **M4** — ship-prep: bench harness (`web/bench/`, headless-Chrome
  driven, zero committed numbers), patent-paragraph README, npm packaging
  prepared — **publish deliberately withheld**, pending explicit user
  instruction.
- [x] **M5** — `oximedia-web-scale` (Lanczos3/Catmull-Rom/Mitchell/
  bilinear, corrected an upstream Catmull-Rom/Bicubic naming bug) +
  `oximedia-web-quality` (windowed single-scale SSIM, PSNR, VMAF
  explicitly deferred). 33 + 32 tests pass; wasm 17,439 B / 15,485 B gzip.
- [~] **X1** — `oximedia-wasm` defect fixes: all 7 documented defects
  fixed (f64→f32/u8 data plane, JSON hot path→typed getters, dishonest
  VP8/AV1/Vorbis decoder classes removed, dead deps pruned, orphaned
  hdr/lut/spatial modules wired in, `wasm-opt` re-enabled, npm/README
  honesty corrected) plus one extra `&[f64]` violation found and fixed
  (`audiopost_wasm::wasm_mix_audio`). Marked partial only because
  `--target wasm32-unknown-unknown` remains blocked by a pre-existing,
  out-of-scope `wgpu` API mismatch in `crates/oximedia-gpu` (via
  `oximedia-colormgmt`'s default `gpu-accel` feature) — flagged to the
  owning team, not fixed here.
- [x] **X2** — root workspace tokio feature-unification fix: root
  `Cargo.toml` pin lowered to `default-features = false` with an explicit
  per-member feature list, eliminating the `mio` (via `tokio "full"` →
  `"net"`) wasm32 build blocker in `oximedia-graph` with zero behavioural
  change to other members.
- [x] **X3** — docs honesty pass: `docs/simd_dispatch.md`'s WASM SIMD128
  section (falsely claimed a working `oximedia-simd` WASM tier reusing
  SSE4.2 paths "exercised by an `oximedia-codec` WASM test matrix" — untrue
  on both counts) rewritten to document the two WASM SIMD paths that
  actually exist (`oximedia-codec`'s own `core::arch::wasm32` module; the
  new `web/` crates' autovectorization-over-`chunks_exact` approach).
  `docs/codec_status.md` gained a browser-surface note. `oximedia-wasm/README.md`
  reviewed, already honest.

---

## 0.1.9 Doctest & Rustdoc Hardening Pass (2026-07-13)

- [x] 7 genuine doctest bugs fixed workspace-wide (real doc-comment/API
  drift, distinct from the two bonus doctest fixes already recorded under
  2026-06-22 above).
- [x] All 68 crates that previously failed strict rustdoc
  (`-D warnings -D rustdoc::broken_intra_doc_links
  -D rustdoc::missing_crate_level_docs -D rustdoc::private_intra_doc_links`)
  now pass it — **except** the crates that briefly could not compile at all
  during the `oxiarc-archive` regression (see immediately below, resolved
  2026-07-14), which could not run rustdoc until they built again.
- [x] Zero clippy warnings workspace-wide (`--all-features --all-targets
  -D warnings`); zero `cargo fmt --check` diffs.
- [x] `cargo nextest run --workspace` (default features): 100,160 tests,
  100,160 passed, 0 failed, 128 skipped. Same with `--all-features`:
  101,814 tests, 101,814 passed, 0 failed, 138 skipped. Both genuine full
  runs, fully clean, verified 2026-07-13.
- [x] **Regression confirmed open during this pass, since resolved
  (2026-07-14):** the subsequent "bump oxiarc" commit (root `Cargo.toml`
  `oxiarc-archive` 0.3.6, sibling `oxiarc-brotli`/`oxiarc-bzip2`/`oxiarc-lzma`/
  `oxiarc-snappy` left at 0.3.5) landed *after* the clean test run above
  and broke compilation for six crates. This pass re-ran the affected
  build (`cargo check -p oximedia-archive-pro -p oximedia-batch
  -p oximedia-convert`, 2026-07-13) and confirmed it was still broken, and
  found the true blast radius was wider than first scoped: `oximedia-cli`
  and `oximedia-wasm` (both unconditional dependents, both in workspace
  `default-members`) and `oximedia-py` also failed to compile, not just
  the three archive/batch/convert library crates. See Known Issues above
  for full detail. Not fixed in that pass (out of scope — that was a
  documentation-refresh task, not a dependency fix). **Resolved
  2026-07-14:** sibling `oxiarc-brotli`/`oxiarc-bzip2`/`oxiarc-lzma`/
  `oxiarc-snappy` published matching 0.3.6 releases; `cargo check
  --workspace --all-features` now passes clean.

---

## Deferred (0.2.x)

**Re-harvested from scratch on 2026-08-12** with
`rg -n "TODO\(0\.2" --type rust` across the whole workspace: **22 markers**,
down from **123** at the start of this session. (The previous revision of
this section recorded **117** at its 2026-07-15 harvest, by the same command;
the tree gained six markers between that harvest and the start of this
session, which is the whole of the 117 → 123 difference.) Every `file:line` below was
read out of the current tree, not carried forward from the previous
revision of this section (whose line numbers had drifted badly — the AV1
entries alone were off by 100+ lines). See `CHANGELOG.md`'s `[0.2.1]`
section for what closed.

Deferrals that have **no code marker** — recorded in prose, or found by
verification rather than by a comment — are listed separately at the end
under "Deferrals without a code marker". They are real backlog and are not
implied by the 22.

A broader sweep (`rg "In a real implementation|would be implemented"`,
`--type rust`) returns 153 hits. Those were spot-checked and are dominated
by doc comments describing what a *caller* would do and by
deliberately-scoped, documented simplifications; they are not a second
backlog of the same kind and are not enumerated here.

### The 22 markers

| Crate | `file:line` | Deferred work |
|---|---|---|
| `oximedia-codec` | `av1/kf/hdr.rs:440` | AV1 inter-frame decode |
| `oximedia-codec` | `av1/kf/recon.rs:779` | AV1 intra block copy |
| `oximedia-codec` | `av1/kf/recon.rs:853` | AV1 palette mode |
| `oximedia-codec` | `av1/kf/recon.rs:1606` | AV1 10/12-bit, monochrome, 4:2:2/4:4:4 |
| `oximedia-codec` | `av1/kf/recon.rs:1613` | AV1 horizontal super-resolution (spec 7.16) |
| `oximedia-codec` | `av1/kf/recon.rs:1619` | AV1 quantizer matrices (spec 7.12.2) |
| `oximedia-codec` | `av1/kf/recon.rs:1625` | AV1 film-grain synthesis (spec 7.18.3) |
| `oximedia-codec` | `vp9/dec/mod.rs:102` | VP9 profiles 1–3 (4:2:2 / 4:4:4, 10/12-bit) |
| `oximedia-transcode` | `frame_level.rs:128` | Re-enable Opus once decode **and** encode are trustworthy |
| `oximedia-transcode` | `frame_level.rs:946` | ProRes 10-bit 4:2:2 frame path (three blockers, below) |
| `oximedia-transcode` | `audio_adapters.rs:369` | Matching Opus encoder/decoder adapters |
| `oximedia-packager` | `encryption.rs:211` | NAL-aware mapping on the legacy `encrypt_sample_aes` path |
| `oximedia-server` | `transcode/engine.rs:195` | Real per-stream transcode pipeline |
| `oximedia-py` | `cache_py.rs:231` | 14 further `oximedia_cache` modules |
| `oximedia-accel` | `compute_backend.rs:598` | Vulkan dispatch (call site) |
| `oximedia-accel` | `compute_backend.rs:644` | Vulkan compute dispatch via `vulkano` |
| `oximedia-automation` | `eas/audio.rs:505` | Real TTS for EAS voice announcements |
| `oximedia-stabilize` | `three_d/stabilize.rs:45` | Real 3D camera-motion solve (SfM) |
| `oximedia-cli` | `archivepro_cmd.rs:514` | `VideoFfv1Mkv` migration target |
| `oximedia-cli` | `captions_cmd.rs:1061` | MP4 (tx3g) and ASS/SSA-in-Matroska caption extraction |
| `oximedia-cli` | `distributed_cmd.rs:228` | State/persistence-directory config field |
| `oximedia-cli` | `edl_cmd.rs:281` | Per-dialect EDL writers in `oximedia-edl` |

### Codec — AV1 (7 markers)

AV1 keyframe/intra decode shipped in 0.2.0 and is bit-exact vs dav1d 1.5.1
and aomdec/libaom v3.12.1 on 13 vectors with the full deblock/CDEF/loop-restoration
chain. Everything below returns an honest `CodecError::UnsupportedFeature`.

- **Inter-frame decode** (`av1/kf/hdr.rs:440`) — motion vectors, reference
  frames, compound prediction. Effort: **specialist**. VP9's inter path
  (closed this session) is the closest in-tree precedent for the shape of
  the work, but AV1's is substantially larger.
- **Intra block copy** (`recon.rs:779`) and **palette mode**
  (`recon.rs:853`).
- **10/12-bit, monochrome, 4:2:2 and 4:4:4** (`recon.rs:1606`). **This one
  gained a hard constraint this session and is no longer a
  straight-widening job:** slice 4A converted the inverse transform from
  `i64` to `i32` after proving the widest 8-bit intermediate is 2^28.5
  (0.18× of `i32::MAX`). The same range analysis shows **12-bit reaches
  2.83× of `i32::MAX` and would overflow**. Restoring 12-bit therefore
  requires dav1d's restructured multiplies (see the header comment of
  `dav1d/src/itx_1d.c`, which documents this exact 19-bit-plus-sign case),
  not a revert to `i64`. 10-bit fits at 0.71× but leaves little headroom.
  Monochrome is separately load-bearing: real-world AVIF encodes its alpha
  auxiliary item as monochrome AV1, so AVIF alpha is blocked on this
  marker, not on anything in `avif/`.
- **Horizontal super-resolution** (`recon.rs:1613`), **quantizer matrices**
  (`recon.rs:1619`), **film-grain synthesis** (`recon.rs:1625`).

### Codec — VP9 (1 marker + 2 fixture-gated deferrals)

VP9 key-frame, intra-only **and** inter decode all shipped this session,
bit-exact vs libvpx on 10 conformance streams through the public API
(see `docs/codec_status.md`'s VP9 entry). Three gaps remain, each an honest
`CodecError::UnsupportedFeature`:

- **Inter-frame segmentation (no marker — gated by a committed fixture).**
  Needs `read_inter_segment_id` and the predicted segment map. The
  `p9seg` fixture is committed and its key frame decodes bit-exactly; the
  refusal is asserted **positionally** at packet 1, so a regression earlier
  in the stream cannot masquerade as the expected deferral. Effort: small.
- **Reference scaling (no marker — gated by a committed fixture).** Needs
  the scale factors and the `vpx_scaled_2d` path for a reference whose
  dimensions differ from the current frame's. The `scaled` fixture is
  committed (96×64 → 64×48 at packet 6); packets 0–5 decode bit-exactly and
  the refusal is asserted at packet 6. Effort: small–medium. Note that
  `clamp_mv_to_umv_border_sb` is already implemented and tested even though
  the unscaled path deliberately does not call it — libvpx applies it only
  under `if (is_scaled)`, and this package is where it becomes live.
- **Profiles 1–3** (`vp9/dec/mod.rs:102`) — 4:2:2 / 4:4:4 subsampling and
  10/12-bit depths. Effort: medium.

### Codec — Opus (large / specialist; the evidence is empirical)

`crates/oximedia-codec/src/opus/` was **demoted to Bitstream-parsing** this
session. This is not a wiring gap and not a missing feature — the code is
real and dispatched — the output is wrong:

- 26 real libopus-encoded **CELT** packets decode to `Ok` with **0 non-zero
  samples out of 49,920** (silence).
- Real **SILK** packets produce out-of-range values (max abs 4.0) and 960
  samples per frame regardless of the configured 16 kHz.
- The **encoder** emits a TOC byte that misdescribes its own payload
  (`0x1c`, claiming SILK/NB/60 ms for a CELT/FB/20 ms frame). Where sample
  counts do line up, decoded energy and correlation with the input are both
  0.0.

The ~82 existing tests are self-consistency round-trips, which is precisely
why this survived a prior audit that promoted the entry upward by reading
the source. Closing it needs a decoder debugged packet-by-packet against
libopus output plus an encoder rebuilt against RFC 6716 §3.1, with the
official RFC 6716 test vectors as a **prerequisite**, not a follow-up.
Effort: **large / specialist**. Tracked markers:
`oximedia-transcode/src/frame_level.rs:128` and `audio_adapters.rs:369`.
`crates/oximedia-audio/src/opus/` is a **separate** implementation that was
not audited; it is what `oximedia-wasm`'s `WasmOpusDecoder` uses, and it
should not be assumed sound on the strength of the `oximedia-codec`
measurements either way.

### oximedia-transcode (3 markers)

- `frame_level.rs:128` / `audio_adapters.rs:369` — Opus, per the entry
  above.
- `frame_level.rs:946` — **ProRes 10-bit 4:2:2**, deferred for three
  independent reasons documented at the marker, none of which is a
  container-extradata mapping: (1) the frame-level pipeline is flat 8-bit
  4:2:0 and the only demuxer wired to it (Y4M) has no 10-bit chroma tag, so
  there is no genuine 10-bit source — widening 8-bit by left-shifting is
  refused as fabricating precision; (2) `ProResDecoder` has no 10-bit
  output API at all, right-shifting its internal reconstruction by 2 on
  both the inherent and trait paths; (3) the decoder independently fails on
  realistic high-contrast content ("malformed codeword" on a 500→900 step),
  pinned by `test_prores_decoder_rejects_high_contrast_content`, which
  deliberately asserts today's broken behaviour with a "revisit if this
  starts passing" note.

### oximedia-packager (1 marker)

- `encryption.rs:211` — NAL-unit-aware subsample mapping on the **legacy**
  `encrypt_sample_aes` path, which still applies the crypt/skip pattern
  over the whole sample buffer (correct only for already-elementary
  payloads). The real NAL-aware implementation shipped this session and is
  reachable through `EncryptionHandler::encrypt_for_codec` /
  `nal_subsamples` / `codec_structure_for`; this marker is the older entry
  point that has not been migrated onto it.

### oximedia-server (1 marker)

- `transcode/engine.rs:195` — the real per-stream transcode pipeline. This
  is **blocked, not merely unscheduled**: it needs both inter-frame decode
  and a trustworthy encoder for one patent-free codec, and no ingest codec
  currently satisfies both (AV1 is keyframe-only decode and its encoder
  does not emit a valid bitstream; FLAC decodes but an ABR ladder for
  lossless audio gains nothing). The engine returns a per-codec honest
  `Err` naming the specific reason rather than a generic refusal. Segment
  *packaging* is unaffected and shipped this session.

### oximedia-py (1 marker)

- `cache_py.rs:231` — 14 further `oximedia_cache` modules remain unbound:
  `two_queue`, `sharded_lru`, `cache_partitioning`, `cache_serialization`,
  `slab_allocator`, `prefetch`, `cache_metrics`, `admission_filter`,
  `adaptive`, `negative`, `segment_cache`, `weighted_cache`,
  `write_through`, `tier_compressor`. The 24 neural/analytics/cache
  families named in the previous revision of this section all shipped.

### Other crates (5 markers)

- `oximedia-accel/src/compute_backend.rs:598,644` — real Vulkan compute
  dispatch via `vulkano`, behind the non-default `vulkan-backend` feature.
  Needs a dependency decision first (Pure-Rust policy: `vulkano` and the
  shader-compilation toolchain must be assessed against the C/C++-free
  default-build rule before this is scheduled).
- `oximedia-automation/src/eas/audio.rs:505` — real TTS for EAS voice
  announcements. The SAME/AFSK **data** path is no longer deferred: a real
  continuous-phase modulator shipped this session with round-trip
  verification through a Goertzel demodulator.
- `oximedia-stabilize/src/three_d/stabilize.rs:45` — real 3D camera-motion
  solve (structure-from-motion). Effort: **specialist**; needs its own
  wave, not a slice.

### oximedia-cli (4 markers; full detail in `oximedia-cli/TODO.md`)

The frame-level-pipeline root cause named in the previous revision of this
section is **closed** — a shared `frame_harness/` module now backs nine
commands (scaling ×3, denoise, stabilize, multicam colour-match, timecode
burn, subtitle burn, captions burn). Four markers remain, each verified
still present in the tree on 2026-08-12:

- `archivepro_cmd.rs:514` — `VideoFfv1Mkv` migration target. Honest refusal
  today: `oximedia-transcode`'s frame level writes FFV1 only in its own
  private raw framing, and labelling that output `.mkv` would be exactly
  the mislabelling this command refuses elsewhere.
- `captions_cmd.rs:1061` — MP4 (tx3g) and ASS/SSA-in-Matroska extraction.
- `distributed_cmd.rs:228` — a state/persistence-directory config field.
- `edl_cmd.rs:281` — real per-dialect writers in `oximedia-edl`.

**`--resume` remains USER-UNDECIDED** and its disposition is unchanged by
this session: removed from the CLI surface entirely (not hidden), because
its only real backing, `TranscodeJob::resume()`, is job-queue-level resume
from a different subsystem and is not adaptable to per-file transcode
resume without new persisted fields. See the "`--resume` disposition
record" in `oximedia-cli/TODO.md`. **Do not implement without an explicit
user decision.**

### Deferrals without a code marker

Real backlog that carries no `TODO(0.2.x)` comment, so a marker harvest
alone will not surface it:

- **VP9 inter segmentation** and **VP9 reference scaling** — see the VP9
  section above. Both have a committed gate fixture.
- **`oximedia` facade: the `video` feature reaches zero codecs.** The root
  `Cargo.toml` pins `oximedia-codec` with `default-features = false`, and
  `oximedia/Cargo.toml` declares `video = ["dep:oximedia-codec"]` with no
  sub-feature forwarding, so `--features video` compiles and exposes no
  AV1/VP9/VP8 decoder — contradicting `oximedia/src/lib.rs`'s own feature
  table. Every other consumer in the workspace forwards explicitly. Under
  `--features full` the codecs are present only by unification from sibling
  crates. `minimal = ["audio", "video", "metadata-ext"]` inherits the gap.
  Ready-to-apply, following the same file's `mjpeg`/`apv` precedent:
  `video = ["dep:oximedia-codec", "oximedia-codec/av1", "oximedia-codec/vp9", "oximedia-codec/vp8"]`.
  Left unapplied pending a maintainer decision on the default feature
  surface.
- **`oximedia` facade: `capture` does not forward `oximedia-capture/tokio`**,
  so `CaptureStream::into_async` is unreachable through the facade at any
  feature combination.
- **`oximedia-normalize`'s batch path refuses MP3 even though a real MP3
  decoder exists in-tree.** `batch/codecs.rs:104-106` sniffs ID3v2 / MPEG
  frame sync and returns an honest `NormalizeError::UnsupportedFormat`
  reading "MP3 decoding is not wired into oximedia-normalize's batch …".
  That wording is deliberate and correct — the decoder at
  `crates/oximedia-audio/src/mp3/` is real and complete (MP3 decoding
  patents expired 2017; see the MP3 entry in `docs/codec_status.md`), so
  this is **unwired, not absent**, and the entry must not be restated as
  "no decoder in tree" or "patent-encumbered". Small, well-scoped follow-up:
  add the dependency and route the sniff to it.
- **`oximedia-container`'s `FlacDemuxer` estimates frame boundaries** from
  `STREAMINFO.max_frame_size` instead of resolving them via CRC-16 — its
  own doc comment says so. `oximedia-normalize`'s new FLAC path bypasses it
  deliberately; every other caller inherits the estimate.
- **CMAF/`fragment/mp4.rs` config-box payload conformance is the caller's
  duty.** The muxer now writes correctly *named* config boxes but forwards
  `track.extradata` verbatim, so a `dOps`/`dfLa` box is only conformant if
  the caller supplied a real `OpusSpecificBox`/`FLACSpecificBox` record.
  `oximedia-server`'s ingest path converts properly; `fragment/mp4.rs`
  passes `codec_params.extradata` through unchanged.
- **`MultiFormatProber` never parses MP4 `tkhd` or the visual `stsd`
  entry**, so `DetailedStreamInfo::width`/`height` are always `None` for
  MP4. No prober path populates `fps` either, so probe-derived ABR ladders
  use a documented `DEFAULT_FRAMERATE = 30.0` that feeds `fps_factor` and
  shifts every rung's bitrate.
- **`VideoFrame::allocate()` under-allocates `P010`/`P016`** — it sizes
  planes at one byte per sample, consulting the stride table only for
  `Yuyv422`/`Uyvy422` and NV12/NV21 chroma. No in-tree caller allocates
  either format; documented in `allocate`'s doc comment as a deliberate
  deferral, and the consumer that would need it is `oximedia-capture`'s
  legacy path.
- **`oximedia-gaming` has no OS input hook.** `InputCapture::poll_events`
  is an honest `UnsupportedPlatform` error; the crate's `capture::hooks` is
  screen-capture region/FPS hooks, an unrelated concept despite the name.
  Its `KNOWN_GAMES` table is also an unverified best-effort sample — a
  wrong entry means "never matches", never a false positive, because
  matching is exact-name and the tests lock that in. Do not "fix" it into
  substring matching.
- **WebRTC DTLS is still non-functional** (`oximedia-net`), unchanged this
  session. It remains labelled and unexported; see the "Network & DRM
  crypto status" section of `docs/codec_status.md`.
- **`oximedia-capture` Linux and Windows live-device runs.** ABI layouts
  are const-verified and the logic is host-tested against synthetic input,
  but no `/dev/video*` node or Media Foundation device has been opened by
  this crate in this workspace's own verification. Needs hardware; see
  "0.2.1 Capture — Known follow-ups" below for the exact commands.
- **WebP VP8L-compressed `ALPH` alpha** — a pre-existing honest
  `UnsupportedFeature` in `webp/alpha.rs`, not a new gap.
- **`crates/oximedia-codec/src/lib.rs`'s Codec Feature Matrix is the last
  stale copy of the codec status.** It is a `//!` doc comment, so it ships
  in that crate's published rustdoc. Four rows disagree with the tree as of
  2026-08-12: `lib.rs:46` VP9 "inter not yet (honest `Err`)"; `lib.rs:55`
  AVIF "not supported (honest `Err`)"; `lib.rs:58` WebP "lossless VP8L
  only; no lossy VP8 WebP decode"; `lib.rs:59` Opus decode "✓ (CELT + SILK
  + Hybrid)" — the exact overclaim this release demotes. Corrected text for
  all four is in `docs/codec_status.md` and the README Codec Matrix. Not
  edited because the 2026-08-12 documentation pass was scoped to `TODO.md`
  / `CHANGELOG.md` / `README.md` / `docs/` / `oximedia-cli/TODO.md`. Small,
  fully specified follow-up.
- **`oximedia-wasm`'s `instant` dependency chain**
  (`oximedia-watermark` → `reed-solomon-erasure` → `parking_lot` 0.11 →
  `instant`) is unfixable upstream — `reed-solomon-erasure` 6.0.0 is the
  latest release. The RUSTSEC advisory is already ignored with a recorded
  rationale.

### Corrections to the previous revision of this section

- **`oximedia-bitstream/src/integer.rs:510` is CLOSED.** The
  `shl_default`/`shr_default` unbounded-shift methods are implemented at
  `integer.rs:516-522` via `unbounded_shl`/`unbounded_shr`, which have been
  stable since Rust 1.87. The item said "pending MSRV support"; the MSRV
  moved and the item was never re-checked.
- **"Container — (nothing open)" was wrong when written, and is now
  *mostly* right for a different reason.** Three gaps existed at the time
  (`MpegTsMuxer::streams()` returning `&[]`, a decorative
  `streaming/demux.rs` buffering path, and a simplified Ogg Vorbis setup
  extraction) and all three closed this session, along with a CMAF
  config-box bug and a reachable Matroska panic-DoS. What remains open in
  `oximedia-container` is listed under "Deferrals without a code marker"
  above: the `FlacDemuxer` frame-boundary estimate, the MP4 prober's
  missing `tkhd`/`stsd` parse, and the CMAF extradata-passthrough caveat.
- **The FLAC entry is moot.** It described the `oximedia-codec` FLAC
  encoder and decoder as "not round-trip self-consistent (the LPC path)".
  That understated it — the decoder *panicked* on stock `ffmpeg` output —
  and both sides have since been rebuilt to RFC 9639 and verified against
  libFLAC/ffmpeg in both directions.
- **The AVIF entry is closed.** `decode()` is wired to the AV1 keyframe
  decoder and byte-identical to ffmpeg/dav1d on a real libaom fixture.
  Alpha and 10/12-bit refuse honestly and are tracked under AV1 above, not
  under AVIF.
- **The `oximedia-wasm`, `oximedia-server`, `oximedia-py`,
  `oximedia-metadata`, `oximedia-renderfarm`, `oximedia-conform`,
  `oximedia-captions`, `oximedia-vfx`, `oximedia-access` and `oximedia-net`
  entries are closed** — see the `[0.2.1]` Added section of
  `CHANGELOG.md`. `oximedia-transcode`'s FFV1 and ALAC items are closed;
  only Opus and ProRes 10-bit remain.
- **`oximedia-normalize` is only *partly* closed.** Of its old entry —
  "additional codec support (MP3/FLAC/Opus) in batch normalize;
  `write_metadata` config flag not yet honored" — FLAC and `write_metadata`
  shipped, and Opus is correctly rolled into the Opus deferral above. **MP3
  did not close**, and it is a different kind of gap from the other two; it
  is listed under "Deferrals without a code marker".
- **All AV1 marker line numbers in the previous revision were stale** (it
  cited `recon.rs:665,739,1475,1482,1488,1494`; the real lines are
  `779,853,1606,1613,1619,1625`). The table above is a fresh read.
- **`prores/picture.rs:139` is closed** — `quant_scale` is now enforced in
  `1..=224` per RDD 36 §6.5.3, with a new `FrameError::BadQuantScale`
  variant and four boundary tests.
- **The "Cross-file drift flagged, not fixed" subsection was removed**, not
  lost: every item in it was already marked resolved, and the drift it
  tracked (`docs/codec_status.md` / `README.md` / `lib.rs` disagreeing about
  AV1/VP9/VP8/AVIF/Vorbis) was re-checked and re-synchronised across all
  three files on 2026-08-12. The one genuinely stale row it had missed —
  `README.md`'s "WebP (VP8L) … Lossless only — no VP8 lossy WebP decoder",
  which had contradicted the shipped `webp/vp8_decoder.rs` — is now
  corrected too.

---

## 0.2.1 Capture — packages A4–A8 (completed 2026-08-11)

Pure-Rust live camera/device capture, landed as a new `oximedia-capture`
crate plus the `oximedia-core`/`oximedia-codec`/`oximedia-simd` pixel-format
work it needed. Packages A4 (skeleton: format model, negotiation, delivery
queue, mock backend), A5 (macOS AVFoundation), A6 (Linux V4L2) and A7
(Windows Media Foundation) implemented the crate itself; this entry is where
all of A4–A7 first get recorded in `TODO.md`/`CHANGELOG.md`/`README.md`. A8
(this pass) is documentation + umbrella wiring only — no new capture logic.

- [x] `oximedia-capture` — new workspace crate (`crates/oximedia-capture/`).
  `enumerate()`/`open()`/`backend_kind()` entry points; `CaptureConfig`,
  `CaptureDevice`, `CaptureFormat`, `CaptureEncoding` (`Raw(PixelFormat)` /
  `Mjpeg`), `DeviceSelector`, `DropPolicy`, `CaptureSession`/`CaptureStream`/
  `CaptureStats`, a deterministic `negotiate()`. Three backends behind one
  API: AVFoundation (macOS, `objc2`), V4L2 (Linux, `rustix` syscalls), Media
  Foundation (Windows, `windows-rs` COM) — zero C/C++/Fortran on any of them.
  `mock` feature (also compiled automatically under `cfg(test)`) drives the
  whole pipeline without hardware, and honestly refuses to fake MJPEG (see
  follow-ups). 246 host unit tests pass (`cargo test -p oximedia-capture
  --features mock`, verified 2026-08-11); opt-in `#[ignore]`d live-device
  tests behind `OXIMEDIA_CAPTURE_DEVICE` for all three platforms
  (`tests/live_capture.rs`). Already registered in workspace `Cargo.toml`
  `members`/`default-members` by package A4; this pass (A8) additionally
  wires it into the `oximedia` facade behind a new `capture` feature
  (optional dependency, feature declaration, `full` list, `pub mod capture`
  re-export, curated `prelude.rs` re-export, feature-flag doc table) and
  into the root `README.md` crate table. See
  `crates/oximedia-capture/README.md` for the full per-platform verification
  breakdown.
- [x] `oximedia_core::PixelFormat::{Yuyv422, Uyvy422}` — packed YUV 4:2:2
  variants backing the two most common UVC/AVFoundation raw formats
  (`crates/oximedia-core/src/types/pixel_format.rs`). Full property support
  (bpp/plane count/planar-ness/chroma subsampling/`frame_buffer_size`/
  `stride_for_width`/`FromStr`/`Display`); new unit tests plus
  `pixel_format_exhaustive.rs` coverage.
- [x] `VideoFrame::allocate()` packed-4:2:2 + NV12/NV21 chroma stride fix
  (`crates/oximedia-codec/src/frame.rs`) — both families were under-allocated
  (packed 4:2:2 by 2x; the NV12/NV21 chroma plane by 2x). Fixed via
  `PixelFormat::stride_for_width()`; every pre-existing planar format
  verified byte-identical via a new regression test. `P010`/`P016` left on
  the legacy, still-undersized path on purpose — see the follow-ups below.
- [x] `oximedia_simd::yuv_ops::convert_uyvy422_row_to_rgb`
  (`crates/oximedia-simd/src/yuv_ops.rs`) — UYVY-order sibling of
  `convert_yuv422_row_to_rgb`; cross-verified byte-for-byte against it on
  even and odd widths. The sibling function's own length assertion was
  simultaneously tightened (`width.div_ceil(2) * 4`, not `width * 2` — the
  old bound under-counted odd-width rows).
- [x] `pf_is_yuv422` widened to the packed variants
  (`crates/oximedia-core/src/codec_negotiation.rs`) — `Yuyv422`/`Uyvy422`
  now score as same-family against `Yuv422p` during negotiation. 10/12/16-bit
  planar 4:2:2 variants deliberately left out of this helper (pre-existing
  scope, unchanged).

### Known follow-ups

- **Linux backend has no real-device run yet.** ABI layouts are
  const-verified (`crates/oximedia-capture/README.md`'s Linux specifics; the
  golden-layout table passes `cargo check` on both
  `aarch64-unknown-linux-gnu` and `i686-unknown-linux-gnu`), but no
  `/dev/video*` node has been opened by this crate in this workspace's own
  verification. Needs a container with a `/dev/video*` node, or a real Linux
  host, running
  `OXIMEDIA_CAPTURE_DEVICE=/dev/videoN cargo test -p oximedia-capture --test live_capture -- --ignored`.
- **Windows backend likewise has no real-device run yet** — `mf_logic.rs` is
  host-tested against synthetic inputs only; needs a Windows host with a
  camera and the equivalent `OXIMEDIA_CAPTURE_DEVICE` live test.
- **`P010`/`P016` `allocate()` sizing left legacy.** Documented in
  `oximedia-codec/src/frame.rs`'s `allocate()` doc comment as a deliberate
  deferral (no in-tree caller allocates them today); fixing it is a small,
  well-scoped follow-up whenever a caller needs it.
- **`mock` cannot emit MJPEG, by design.** Asking the mock backend for
  `CaptureEncoding::Mjpeg` returns `CaptureError::FormatRejected` rather than
  bytes that claim to be a JPEG and are not — the same honesty policy the
  crate applies to real backends, not a gap to close.
- ~~**Async `recv()` deferred.**~~ **CLOSED 2026-08-12.** The new opt-in
  `tokio` feature adds `CaptureStream::into_async()` /
  `into_async_with_capacity()` (`src/asyncio.rs`), a **bounded** bridge that
  preserves the ring buffer's drop policy as backpressure rather than
  converting it into unbounded channel latency. It touches no platform code:
  every `!Send` handle already lives and dies on the dedicated capture
  thread, and the bridge consumes only the already-public
  `recv_timeout`/`is_ended`. The `recv()` tri-state
  (`Ok(Some)` / `Ok(None)` = ended / `Err`) survives the wrapper unchanged.
  The synchronous API is untouched, and the feature is off by default so a
  caller with no async runtime never pays for tokio. **Remaining caveat:** the
  `oximedia` facade's `capture` feature does not forward
  `oximedia-capture/tokio`, so the async path is reachable only by depending
  on `oximedia-capture` directly — see "Deferrals without a code marker".
- **Stepwise frame-size negotiation is an approximation, on purpose.** A
  `STEPWISE`/`CONTINUOUS` V4L2 range is sampled at its two corners and a
  step-snapped midpoint rather than enumerated in full (a real range can be
  500,000+ distinct modes); documented in
  `platform/linux/mod.rs::stepwise_size_corners`.
- **`oximedia-wasm` `instant` breakage — pre-existing, unrelated.** See
  Known Issues above (`oximedia-watermark` → `reed-solomon-erasure` →
  `parking_lot` 0.11 → `instant`); not touched by package A8.
- ~~**`oximedia-mam` MPL license failures — pre-existing.**~~ **CLOSED
  2026-08-12.** `cargo deny check licenses` and `check bans` are both green
  (30 rejected crates → 0). `async-graphql` moved to
  `default-features = false` with its default set minus `email-validator`,
  which removes `fast_chemail`/`ascii_utils` from the graph entirely
  (behaviour-neutral — `oximedia-mam`'s GraphQL surface uses no email
  validator); `colored`, `option-ext` and `serialport` are scoped MPL-2.0
  exceptions; `fuchsia-cprng` is clarified. Note the diagnosis in the
  original entry was partly wrong: `colored` is a **direct, non-optional
  `oximedia-cli` dependency**, not a `mockito` dev-dep ingress.

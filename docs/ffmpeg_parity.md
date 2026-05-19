# FFmpeg Parity — Strategy and Status

This document captures the OxiMedia → FFmpeg feature-parity analysis: where
the gaps are, what strategic approaches close them, what's already landed,
and what the next concrete targets are. It exists so the answer to "is this
already in scope?" and "what should we work on next?" doesn't have to be
re-derived every time.

The matrix here is *coarse* — finer-grained per-codec / per-container status
lives in [`codec_status.md`](codec_status.md). This doc is about strategic
direction.

## TL;DR

OxiMedia covers a wide modern surface (royalty-free codecs, ACES color, ST
2110, NDI, CEA-708, IMF/AAF, ML, forensics, MAM) that FFmpeg doesn't reach
out of the box. FFmpeg's remaining advantage sits in three categories:

1. **Patent-encumbered codecs that everyone has to ingest** — H.264, HEVC,
   AAC. Pure-Rust implementations of these are multi-quarter projects per
   codec and have the same patent exposure regardless of language.
2. **Vendor hardware encoders/decoders** — NVENC, QSV/oneVPL, VAAPI, AMF,
   VideoToolbox. These are vendor C SDKs by definition; the *only* way to
   match FFmpeg here is FFI bindings or driving the equivalent
   Khronos-standard `VK_KHR_video_*` Vulkan extensions.
3. **Legacy container / protocol coverage** — AVI, FLV, MOV variants,
   RTSP, RTMP-pull, MMS, etc. Mostly small protocol-and-parsing PRs.

The roadmap below pursues all three on parallel tracks.

## Feature parity matrix

✓ = supported; ✗ = absent; partial = subset or behind a feature flag.
References point to the crate that owns the implementation.

### Containers

|Format|OxiMedia|FFmpeg|
|---|---|---|
|MP4 / ISOBMFF|✓ (AV1/VP9 video tracks)|✓ (all)|
|Matroska / WebM|✓|✓|
|MPEG-TS|✓ (AV1/VP9/VP8/Opus/FLAC)|✓ (all)|
|OGG|✓|✓|
|WAV / Y4M|✓|✓|
|AVI / FLV / MOV-legacy|✗|✓|
|IMF (SMPTE ST 2067), AAF|✓ ([oximedia-imf](../crates/oximedia-imf/), [oximedia-aaf](../crates/oximedia-aaf/))|partial|

### Video codecs

|Codec|OxiMedia|FFmpeg|
|---|---|---|
|AV1, VP9, VP8 (encode + decode)|✓|✓|
|Theora, MJPEG, H.263, FFV1|✓|✓|
|APV, JPEG-XL, AVIF (encode + decode)|✓|partial|
|**H.264 / H.265 (HEVC)**|**✗** software; ✓ hardware via [oximedia-vtb](../crates/oximedia-vtb/) (decode, Apple platforms)|✓|
|ProRes / DNxHD-HR|✗|✓|

### Audio codecs

|Codec|OxiMedia|FFmpeg|
|---|---|---|
|Opus, Vorbis, FLAC, PCM|✓|✓|
|MP3|decode only|✓|
|**AAC, AC-3 / E-AC-3**|**✗**|✓|

### Image codecs

|Format|OxiMedia|FFmpeg|
|---|---|---|
|PNG/APNG, JPEG, JPEG-XL, WebP, GIF, AVIF|✓|✓|
|DPX, OpenEXR, TIFF / BigTIFF|✓|partial|

Stronger pro/VFX surface than FFmpeg out of the box.

### Hardware acceleration

|Path|OxiMedia|FFmpeg|
|---|---|---|
|Vulkan compute, wgpu (Metal/DX12/WebGPU)|✓ ([oximedia-accel](../crates/oximedia-accel/), [oximedia-gpu](../crates/oximedia-gpu/))|partial|
|NVIDIA NVENC / NVDEC|scaffolded ([oximedia-nvenc-sys](../crates/oximedia-nvenc-sys/))|✓|
|Intel QSV / oneVPL|scaffolded ([oximedia-vpl-sys](../crates/oximedia-vpl-sys/))|✓|
|Linux VAAPI|scaffolded ([oximedia-vaapi-sys](../crates/oximedia-vaapi-sys/))|✓|
|AMD AMF|scaffolded ([oximedia-amf-sys](../crates/oximedia-amf-sys/))|✓|
|Apple VideoToolbox|scaffolded ([oximedia-vtb-sys](../crates/oximedia-vtb-sys/)) + safe wrapper for H.264 decode ([oximedia-vtb](../crates/oximedia-vtb/))|✓|

"Scaffolded" means the `-sys` crate exists and bindings build when the SDK
is present, but the safe wrapper that drives the SDK isn't written yet.

### Networking / streaming

|Protocol|OxiMedia|FFmpeg|
|---|---|---|
|HLS, DASH, RTMP, SRT, WebRTC|✓ ([oximedia-net](../crates/oximedia-net/))|✓|
|SMPTE ST 2110, NDI|✓ ([oximedia-ndi](../crates/oximedia-ndi/))|partial / external|
|RTSP (client, TCP-interleaved)|✓ ([oximedia-net::rtsp](../crates/oximedia-net/src/rtsp/))|✓|
|CDN (Cloudflare/Fastly/Akamai/CloudFront)|✓ ([oximedia-cdn](../crates/oximedia-cdn/))|—|
|RTSP server / RECORD, RIST|✗|✓|

### HDR / color management

ACES (IDT/RRT/ODT/LMT), ICC v2/v4, Rec.709/2020, P3, PQ/HLG, HDR10/10+,
Dolby Vision RPU — [oximedia-colormgmt](../crates/oximedia-colormgmt/),
[oximedia-hdr](../crates/oximedia-hdr/),
[oximedia-dolbyvision](../crates/oximedia-dolbyvision/). Substantially
deeper than FFmpeg's built-in color path.

### Subtitles / captions

SRT, WebVTT, SSA/ASS, TTML/DFXP, CEA-608/708, SCC, EBU STL, iTT,
Teletext/ARIB, DVB, PGS, VobSub — [oximedia-captions](../crates/oximedia-captions/).
Broader and more unified than FFmpeg.

## Strategic approaches to closing remaining gaps

For each major class of gap there's more than one way to close it. The
trade-offs are different enough that the choice matters before any code is
written. None of these is "the right answer" globally — they're a menu.

### (A) Match FFmpeg literally — vendor `-sys` crates

Per-vendor `-sys` crate with bindgen against the vendor's C/Obj-C SDK.
This is mechanically what `libavcodec/{nvenc,qsvenc,vaapi_encode,amfenc,videotoolboxenc}.c`
does. Same blast radius FFmpeg accepts: each developer must install the
relevant SDK; CI runs only the ones the host supports.

- **Pros:** widest hardware coverage, matches FFmpeg surface, can ship
  whichever path the user's machine has, vendor-specific tuning available.
- **Cons:** ties OxiMedia to closed vendor binaries, contradicts the
  workspace's "pure Rust" positioning, builds become platform-conditional,
  every SDK is its own maintenance burden, the patent-encumbered codecs
  themselves are still patent-encumbered regardless of language.
- **Status:** five `-sys` crates landed
  ([oximedia-vtb-sys](../crates/oximedia-vtb-sys/),
  [oximedia-nvenc-sys](../crates/oximedia-nvenc-sys/),
  [oximedia-vpl-sys](../crates/oximedia-vpl-sys/),
  [oximedia-vaapi-sys](../crates/oximedia-vaapi-sys/),
  [oximedia-amf-sys](../crates/oximedia-amf-sys/)). One safe wrapper landed:
  [oximedia-vtb](../crates/oximedia-vtb/) — H.264 decode.

### (B) Stay pure-Rust — `VK_KHR_video_*` Vulkan extensions

Khronos-standard, vendor-neutral APIs for the *same dedicated encode/decode
silicon* that NVENC/QSV/VAAPI/AMF/VTB expose. Drivable from Rust via `ash`
or the existing `vulkano` dependency. Covers H.264 / H.265 / AV1.

- **Pros:** one API for all GPUs, no vendor SDKs in the dependency graph,
  consistent with the rest of the workspace's GPU layer
  ([oximedia-accel](../crates/oximedia-accel/) / [oximedia-gpu](../crates/oximedia-gpu/)),
  no Linux/Windows-only restrictions on the build.
- **Cons:** narrower hardware coverage (newer NVIDIA / AMD / Intel only),
  no Apple Silicon path (MoltenVK doesn't expose video extensions yet),
  still needs H.264/HEVC bitstream parsing in Rust (~5K lines of header
  syntax — patent-free, but real work). Driver maturity varies.
- **Status:** not started. The natural home is a new `oximedia-vkvideo`
  crate sitting alongside [oximedia-accel](../crates/oximedia-accel/).

### (C) Both, layered

`oximedia-vkvideo` is the default; the vendor `-sys` crates are opt-in
features for absolute fastest path or platforms Vulkan video doesn't reach
(Apple Silicon → VTB feature).

- **Pros:** best of both worlds, no platform left behind.
- **Cons:** dispatch crate has to maintain awareness of both backends,
  more code to test, more places things can be wrong.
- **Status:** the `-sys` half exists; `oximedia-hwaccel` dispatcher and
  the Vulkan video crate are both follow-ups.

### Pure-Rust H.264/HEVC/AAC decoders from scratch

This is FFmpeg's *other* approach (`libavcodec/h264*.c`, `hevcdec.c`,
`aacdec.c`). The bitstream specs are public; the patents on the algorithms
apply regardless of who writes them in which language.

- **Pros:** runs everywhere, no vendor lock-in, no FFI surface, pure
  workspace style.
- **Cons:** **H.264 ≈ 50–80K lines, HEVC ≈ 80–120K lines, AAC-LC ≈ 5–10K
  lines** of careful Rust per codec. No production-grade Rust
  implementation of any of them exists today. Multi-quarter projects per
  codec. Patent exposure is identical to (A) — the issue is not source
  language but practicing the patent.
- **Status:** not started; deliberately deprioritized vs (A)/(B).

### Pure-Rust niche codecs (ProRes, DNxHR, etc.)

Professional intermediate codecs are well-documented, royalty-free in
practice, and tractable in Rust (~5–10K lines per codec).

- **Pros:** real FFmpeg gap closed without patent concerns, fits the
  workspace style perfectly.
- **Cons:** narrower audience than H.264 — pro post-production only.
- **Status:** not started; good follow-up candidate.

## What landed in the recent session

|PR / branch|Crate(s) added|Status|
|---|---|---|
|`feat/rtsp-client`|[oximedia-net::rtsp](../crates/oximedia-net/src/rtsp/)|RTSP 1.0 client, TCP-interleaved transport, Basic + Digest auth, SDP parser, RTP packet parser. 54 unit + 39 doc + 8 integration tests, zero warnings.|
|`feat/hwaccel-sys-crates`|5× `-sys` crates|Bindings to all five vendor SDKs (VTB, NVENC, oneVPL, VAAPI, AMF), gated cleanly per platform, empty-stub fallback when SDK absent.|
|`feat/vtb-h264-decoder`|[oximedia-vtb](../crates/oximedia-vtb/)|Safe Rust wrappers over VideoToolbox: H.264 *decode* lifecycle complete (CFType RAII, NAL Annex-B↔AVCC, format-description, decompression session, decoder facade). 24 tests.|

## Recommended next targets

Ordered by yield-per-engineer-week, biased toward closing user-visible
gaps over architectural cleanup.

1. **RTP depacketizers** — RFC 6184 (H.264) and RFC 7798 (HEVC) NAL-unit
   reassembly. ~500–1000 LOC each, pure Rust, no patents. **Bridges the
   gap between [oximedia-net::rtsp](../crates/oximedia-net/src/rtsp/)
   (producer of RTP payloads) and [oximedia-vtb](../crates/oximedia-vtb/)
   (consumer of Annex-B NAL units).** The first PR that yields an
   IP-camera → decoded-frames pipeline end-to-end.

2. **VTB encoder + HEVC** — extend
   [oximedia-vtb](../crates/oximedia-vtb/) with `VTCompressionSession`
   (encode side, same module shape) and HEVC support (same APIs, different
   `kCMVideoCodecType_HEVC` constant). ~600–800 LOC. Closes the macOS
   side of the H.264/HEVC encode gap completely.

3. **AAC via AudioToolbox** — [oximedia-vtb](../crates/oximedia-vtb/)'s
   sibling for audio. `AudioConverter` API, separate from VT but in the
   same `-sys` crate. Closes the AAC gap on Apple platforms. ~800 LOC.

4. **Pure-Rust ProRes decoder** — ~5–10K LOC, tractable in pure Rust, no
   patent exposure, real pro-video gap closed. Format documented; FFmpeg's
   `proresdec.c` is a reference. Lands in `oximedia-codec` as a peer to
   the existing AV1/VP9 decoders.

5. **AVI / FLV demuxers** — small parsing PRs (~500–1000 LOC each) in
   [oximedia-container](../crates/oximedia-container/). Low marginal value
   on their own (most content inside is H.264, which still needs item 6
   to actually decode), but cheap.

6. **Pure-Rust H.264 software decoder** — multi-quarter, but it's the
   single biggest *durable* parity item. Eliminates the macOS-only
   restriction of VTB and the patent-language ambiguity of vendor SDKs.

7. **`oximedia-vkvideo` (Strategy B)** — Vulkan video extension wrapper.
   Real pure-Rust hardware path. Best long-term answer for non-Apple
   hardware. Tractable to skeleton in one PR (capability probe + H.264
   decode session lifecycle); production-quality is more.

8. **`oximedia-hwaccel` dispatcher** — runtime probing of `HAS_BINDINGS`
   across all the backends, plus a trait that picks the best one. Only
   useful once at least two safe wrappers exist; would currently dispatch
   to a single backend.

9. **RTSP follow-ups** — UDP transport, `rtsps://` (TLS over the existing
   client), server-side `RECORD` path. Each is a small focused PR.

## Trade-offs and constraints to keep in mind

- **Workspace tagline.** The current `Cargo.toml` description says
  "patent-free, memory-safe multimedia processing in pure Rust." Strategy
  (A) above (vendor `-sys` crates) and any pure-Rust H.264/HEVC/AAC
  decoder both step outside "patent-free" in practice; (A) also steps
  outside "pure Rust." This is a positioning decision, not a technical
  one; the choice belongs with the project's maintainers.
- **Build hosts.** Every vendor SDK adds an environment variable
  (`NV_CODEC_SDK`, `VPL_ROOT`, `AMF_ROOT`) or a pkg-config requirement
  (`libva`, `libvpl`) that CI needs to provide on the relevant platform.
  The empty-stub fallback keeps `cargo build` green everywhere, but
  testing the real path requires the SDK.
- **Patent licensing.** None of the strategies for H.264/HEVC/AAC changes
  the patent landscape. The risk model is: vendor SDKs and platform
  frameworks (Apple/MS/Google) ship with patent licenses included;
  custom software decoders, including OxiMedia's pure-Rust ones, do not.
  Downstream commercial users handle their own licensing in all cases —
  same model FFmpeg uses, documented on FFmpeg's
  [legal page](https://ffmpeg.org/legal.html).
- **Maintenance.** Every `-sys` crate is its own ongoing burden as
  vendor SDKs evolve. Wrapping them is a one-time cost; keeping the
  bindgen allowlists and Rust facades in step with new SDK versions is
  recurring.

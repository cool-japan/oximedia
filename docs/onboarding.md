# OxiMedia Contributor Onboarding

This is the entry point for someone joining the project who wants to land
real code — fix a decoder stage, add a container parser, wire up a new
hardware-accel backend. It points at the right docs to read first and the
right code to read after.

## Map of the technical docs

```text
                ┌─────────────────────────────────┐
                │  onboarding.md  (you are here)  │
                └─────────────────────────────────┘
                         │
        ┌────────────────┼────────────────────────────┐
        │                │                            │
        ▼                ▼                            ▼
 ┌──────────────┐  ┌────────────────┐    ┌──────────────────┐
 │ codec_       │  │ wire_formats.md│    │ ffmpeg_parity.md │
 │ internals.md │  │ — protocols /  │    │ — strategy and   │
 │ — how video  │  │ packetization /│    │ what's done /    │
 │ and audio    │  │ FFI conventions│    │ what's next      │
 │ compression  │  │                │    │                  │
 │ actually     │  └────────────────┘    └──────────────────┘
 │ works        │
 └──────────────┘
        │
        ▼
 ┌──────────────────┐    ┌──────────────────┐
 │ codec_status.md  │    │ rate_control.md  │
 │ — per-codec      │    │ — encoder-side   │
 │ honest status    │    │ rate control     │
 │ ("what's a stub")│    │ math deeper      │
 └──────────────────┘    └──────────────────┘
```

Specialised companions: [`ml_guide.md`](ml_guide.md) for the ML pipeline,
[`simd_dispatch.md`](simd_dispatch.md) for the SIMD path,
[`wave5_deltas.md`](wave5_deltas.md) for Wave5-specific notes.

## Reading order by goal

### "I want to land code in a video decoder"

1. [`codec_internals.md`](codec_internals.md) — read end to end. ~30 min.
2. [`codec_status.md`](codec_status.md) — find a codec marked
   *bitstream-parsing* with effort *small* or *medium*. That's where new
   contributors land first wins.
3. Open the codec's directory under
   [`crates/oximedia-codec/src/`](../crates/oximedia-codec/src/) (e.g.
   `av1/`, `vp9/`, `mjpeg/`). Read `mod.rs` first, then the stages
   referenced from
   [`reconstruct/pipeline.rs`](../crates/oximedia-codec/src/reconstruct/pipeline.rs).
4. Pick one stage that's stubbed (`stage_entropy`, `stage_predict`,
   `stage_transform`, ...) and implement it against the spec linked in
   `codec_status.md`.

### "I want to land code in a hardware-accel backend"

1. [`codec_internals.md`](codec_internals.md) — at minimum §2 (pipeline)
   and §13 (module layout). The HW path still needs to produce a
   `VideoFrame` in the right pixel format.
2. [`wire_formats.md`](wire_formats.md) — §H.264 NAL, §CoreFoundation
   refcounting, §VT callback model.
3. [`ffmpeg_parity.md`](ffmpeg_parity.md) — §Strategic approaches, to
   understand where this work fits in the bigger plan.
4. Existing safe wrapper:
   [`crates/oximedia-vtb/`](../crates/oximedia-vtb/) — read it end to
   end. It's the template every other vendor-SDK wrapper should follow.
5. Pick a `*-sys` crate to wrap. Candidates: NVENC, oneVPL, VAAPI, AMF.
   The structure is identical to `oximedia-vtb`; copy that and adapt.

### "I want to land code in the network layer"

1. [`wire_formats.md`](wire_formats.md) — §RTP, §TCP-interleaved,
   §SDP, §HTTP Digest.
2. Look at [`crates/oximedia-net/src/rtsp/`](../crates/oximedia-net/src/rtsp/)
   for the current pattern (request/response framing, async client
   state machine, integration tests under `tests/`).
3. Pick from the network follow-ups in
   [`ffmpeg_parity.md`](ffmpeg_parity.md): UDP transport for RTSP,
   `rtsps://` (TLS), RTSP server, RTP depacketizers (the next must-have
   for anyone touching live video).

### "I want to add or fix a container format"

1. [`codec_internals.md`](codec_internals.md) §12 (Container basics) —
   vocabulary and timestamp model.
2. Existing parsers in
   [`crates/oximedia-container/`](../crates/oximedia-container/). The
   MP4 path is the most complete; Matroska/WebM and MPEG-TS are next.
3. AVI and FLV demuxers are open follow-ups in
   [`ffmpeg_parity.md`](ffmpeg_parity.md). Both are small (~500–1000
   LOC); good first PRs.

### "I want to understand the project before committing to anything"

1. [`ffmpeg_parity.md`](ffmpeg_parity.md) — TL;DR plus the feature
   matrix. ~10 min.
2. [`codec_status.md`](codec_status.md) — honest decoder-by-decoder
   status. ~15 min.
3. The top-level [`README.md`](../README.md) and
   [`TODO.md`](../TODO.md) for what's actually called out as
   ship-blocking.

## Project conventions worth knowing before you write code

- **No-warnings policy.** `cargo build` and `cargo clippy
  --all-targets` must produce zero warnings on every PR.
  Per-crate lint overrides go in the crate's `Cargo.toml`, not as
  `#[allow]` attributes on individual items.
- **Tests live with the code.** Unit tests go in `#[cfg(test)] mod
  tests` at the bottom of the same file. Integration tests go under
  `crates/<crate>/tests/`. Every public method should have a runnable
  `# Example` doctest unless it requires a network resource (then mark
  `no_run` or `ignore`).
- **Tokio for async I/O.** Never `std::thread::spawn` for things that
  could be tasks. Never block-on inside an async function.
- **Prefer the COOLJAPAN ecosystem.** `OxiFFT`, `OxiBLAS`, `OxiONNX`,
  `oxiarc-*` over external C dependencies. The workspace tagline says
  "pure Rust" — this is the practical meaning.
- **No emojis in commits, code, or docs** unless the user explicitly
  asks for them.
- **Commit messages document the change, not the developer.** Don't
  include "verified on my machine" sections, don't editorialize about
  downstream choices, don't add Co-Authored-By trailers unless asked.
  The commit body is read by people in five years; the PR description
  is read by reviewers in five days. Put run-output and review-context
  in the PR, not the commit.
- **Branches are small and stack cleanly.** One concern per branch.
  Build infrastructure changes (cargo config, lint overrides) go in
  their own commits separate from feature work.

## Project layout at a glance

|Where|What|
|---|---|
|[`crates/oximedia-core/`](../crates/oximedia-core/)|Type system: `CodecId`, `PixelFormat`, `Rational`, `Timestamp`, frame metadata. Every other crate depends on this.|
|[`crates/oximedia-codec/`](../crates/oximedia-codec/)|Codec implementations: entropy decode, prediction, transform, reconstruction. Per-codec dirs (`av1/`, `vp9/`, …).|
|[`crates/oximedia-container/`](../crates/oximedia-container/)|Mux/demux: MP4, Matroska, MPEG-TS, OGG, WAV.|
|[`crates/oximedia-audio/`](../crates/oximedia-audio/)|Audio codec implementations and audio-specific filters.|
|[`crates/oximedia-net/`](../crates/oximedia-net/)|Network protocols: HLS, DASH, RTMP, SRT, WebRTC, RTSP, ST 2110, CDN.|
|[`crates/oximedia-gpu/`](../crates/oximedia-gpu/)|wgpu-based GPU compute path (Metal/DX12/WebGPU).|
|[`crates/oximedia-accel/`](../crates/oximedia-accel/)|Vulkan compute path.|
|[`crates/oximedia-vtb-sys/`](../crates/oximedia-vtb-sys/), [`oximedia-nvenc-sys/`](../crates/oximedia-nvenc-sys/), [`oximedia-vpl-sys/`](../crates/oximedia-vpl-sys/), [`oximedia-vaapi-sys/`](../crates/oximedia-vaapi-sys/), [`oximedia-amf-sys/`](../crates/oximedia-amf-sys/)|FFI bindings to vendor hardware-accel SDKs.|
|[`crates/oximedia-vtb/`](../crates/oximedia-vtb/)|Safe Rust wrapper over `oximedia-vtb-sys` (currently H.264 decode). Template for future vendor wrappers.|
|[`crates/oximedia-cv/`](../crates/oximedia-cv/), [`oximedia-ml/`](../crates/oximedia-ml/)|Computer vision + ML pipelines.|
|[`crates/oximedia-colormgmt/`](../crates/oximedia-colormgmt/), [`oximedia-hdr/`](../crates/oximedia-hdr/)|Colour management, ACES, HDR transfer functions.|
|[`crates/oximedia-captions/`](../crates/oximedia-captions/), [`oximedia-subtitle/`](../crates/oximedia-subtitle/)|Caption and subtitle parsing/rendering.|

There are ~110 crates total; the ones above are where most contributor
PRs end up landing.

## Getting your first PR landed

1. Pick a target from the reading-order section above.
2. Read the relevant doc, then the relevant code.
3. Open an issue *before* you write code if the change is non-trivial
   (more than ~200 lines or touching public API). Cheaper to align on
   approach in a comment than to redo a PR.
4. Build green, clippy clean, tests pass. CI enforces all three.
5. PR description: motivation, what changed, test evidence. The
   maintainer should be able to merge from reading the PR alone —
   they shouldn't need to spelunk through commits to figure out what
   you did.
6. Expect to revise. The bar is high. The aim is to make the codebase
   stronger, not to merge fast.

Welcome.

# oximedia-capture

**Status: macOS shipping, hardware-verified during package A5 · Linux and
Windows shipping, device-unverified** | Version: 0.2.1

Pure-Rust live video capture for OxiMedia: the replacement for `libavdevice`'s
capture side. Open a camera, negotiate a format with it, receive frames on a
bounded queue.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a
comprehensive pure-Rust media processing framework.

## Purity

No C, C++ or Fortran is compiled or linked, at any point, on any target this
crate builds for. Each platform backend talks to its OS through a Rust-native
binding to the platform's own ABI — the same class of dependency
`oximedia-ndi` uses for its own OS integration — never a vendored or
`cc`-compiled C translation unit:

- **macOS** — the `objc2` family emits Objective-C message sends and
  `extern "C"` declarations directly from Rust.
- **Linux** — `rustix` issues `ioctl`/`mmap`/`poll` syscalls directly (its
  `linux_raw` backend on `linux-gnu`/`linux-musl`), with no libc shim in the
  chain.
- **Windows** — `windows-rs` generates `extern "system"` declarations and COM
  vtable calls from metadata, linking OS DLLs (`mf`, `mfplat`, `mfreadwrite`,
  `ole32`) rather than compiling any vendored source.

Portable dependencies are `oximedia-core`, `oximedia-codec` (frame types only,
default features off), `thiserror`, `tracing`, `crossbeam-channel` and
`parking_lot`. Each platform's OS-binding dependency lives in its own
`[target.'cfg(target_os = "...")'.dependencies]` table in `Cargo.toml`, so no
other target ever resolves it: a Linux build never sees `objc2` or `windows`,
a macOS build never sees `rustix` or `windows`, `wasm32-unknown-unknown` sees
none of the three, and so on.

The workspace denies `unsafe_code`. Eight files opt out — three per
hardware-facing backend, except Windows' two — and every `unsafe` block in
them carries a `// SAFETY:` comment naming its invariant:

| Platform | Files | Responsibility |
|---|---|---|
| macOS | `platform/macos/{avf,delegate,pixel}.rs` | discovery + session lifecycle; the sample-buffer delegate class; the `CVPixelBuffer` copy |
| Linux | `platform/linux/{uapi,ioctl,stream}.rs` | `#[repr(C)]` UAPI transcription; the ioctl wrappers; `mmap`/`poll`/`DQBUF`/copy |
| Windows | `platform/windows/{com,reader}.rs` | COM lifetime/apartment management; `IMFSourceReader` calls |

Every other file — each platform's own `mod.rs`, and everything outside
`platform/` entirely — contains no `unsafe` at all.

## Platform support

| Target | API | Package | Status |
|---|---|---|---|
| macOS | AVFoundation | A5 | shipping, hardware-verified |
| Linux | Video4Linux2 | A6 | shipping, device-unverified |
| Windows | Media Foundation | A7 | shipping, device-unverified |
| anything else | — | — | `CaptureError::UnsupportedPlatform` |

"Shipping" means implemented, compiling, and green on its own unit tests —
on its own, that says nothing about a real device. What backs each status:

- **macOS: hardware-verified during package A5.** `enumerate()`'s macOS path
  is exercised by a non-`#[ignore]`d unit test
  (`discovery_describes_real_devices_without_prompting`) that calls real
  AVFoundation on every run and accepts either a populated device list or
  `CaptureError::PermissionDenied` as correct — a materially stronger bar
  than Linux/Windows below, whose real-hardware paths are entirely opt-in and
  `#[ignore]`d. Package A5 validated this against a physical camera.
  Re-running it during this A8 documentation pass, in a sandboxed environment
  with no camera permission granted, correctly returned
  `CaptureError::PermissionDenied` rather than a device list — the test's
  other honest outcome, not a failure, and exactly what an unprivileged
  sandbox should produce. Streaming through `open()` is additionally gated
  behind the system TCC permission prompt (see below).
- **Linux: device-unverified.** The V4L2 struct layouts in `uapi.rs` are
  hand-transcribed from `linux/videodev2.h` and pinned by a compile-time
  golden-layout table of sizes, alignments and field offsets that `cargo
  check` itself const-evaluates, failing the build if any transcription
  drifted. That check has been run and passes on both a 64-bit target
  (`aarch64-unknown-linux-gnu`) and a 32-bit target
  (`i686-unknown-linux-gnu`), against the kernel 6.19 UAPI header set. No
  `/dev/video*` node has been opened by this backend yet in this workspace's
  own verification — see "Live-device tests" below for how to close that gap.
- **Windows: device-unverified.** The Media Foundation logic (`mf_logic.rs`)
  is exercised host-side against synthetically constructed `IMFMediaType`-shaped
  inputs and is green on every host this crate has been built on. No real
  `IMFSourceReader` has been driven against a physical camera in this
  workspace's own verification.

A target with no backend does not return an empty device list. An empty list is
what a working backend reports when no camera is plugged in; conflating that
with "not implemented" turns a missing feature into a hardware bug hunt.
`backend_kind()` likewise reports `BackendKind::Unsupported` rather than the API
the platform *would* use.

iOS is deliberately not on the AVFoundation arm: the framework is there, but the
discovery types, session-preset behaviour and permission flow differ enough that
claiming support without testing it would be the fabrication the `Unsupported`
variant exists to prevent.

### macOS specifics

- **Permissions.** `enumerate()` never prompts — an undetermined TCC status
  lists devices and modes without raising the dialog — but a previously
  denied or restricted status makes it return
  `CaptureError::PermissionDenied` instead of a device list. `open()` goes
  further: a process that has never been asked triggers the system dialog and
  waits, up to 60 s, for the answer; a refusal is
  `CaptureError::PermissionDenied`, never a session that quietly delivers
  black frames.
- **Formats.** Device modes are read from `AVCaptureDevice.formats`. Media
  sub-types `420v` and `420f` map to `Nv12`, `2vuy` to `Uyvy422`, `yuvs` to
  `Yuyv422`, and `dmb1`/`jpeg` to `Mjpeg`. Anything else is skipped with a
  `debug!` line rather than guessed at. Frame rates come from each range's
  `CMTime` frame *duration*, so `30000/1001` stays exact instead of becoming
  `29.97`.
- **MJPEG is enumerated but cannot be opened.** `AVCaptureVideoDataOutput` is
  configured through `kCVPixelBufferPixelFormatTypeKey` and delivers
  `CVPixelBuffer`s; there is no compressed delivery path. Selecting an MJPEG
  mode reports `CaptureError::FormatRejected` rather than handing back decoded
  samples labelled as a JPEG bitstream. In practice AVFoundation offers an
  uncompressed mode for every camera, and `Mjpeg` ranks last by default.
- **`buffer_count` is ignored.** AVFoundation owns its capture buffer ring and
  exposes no knob for its depth. `queue_depth` and `DropPolicy` still apply.

### Linux specifics

- **Permissions.** Access is plain file permissions on the `/dev/video*` node
  — on most distributions, membership of the `video` group. A node this
  process may not open is skipped during `enumerate()`, because a machine can
  have one camera the user may use and one they may not; but if *every* node
  was refused, so the device list would otherwise be empty, that is reported
  as `CaptureError::PermissionDenied` rather than "no cameras attached" — the
  same list-vs-refusal distinction this crate makes everywhere else, so the
  answer points at `usermod -aG video` instead of at a loose USB cable.
- **No file descriptor is held between `enumerate()` and `open()`.** `open()`
  records the device-node path, the negotiated format and the buffer count as
  plain data and opens nothing; the `/dev/video*` node itself is opened on the
  capture thread, inside `run()`. This mirrors the AVFoundation backend's rule
  (there, Objective-C objects are `!Send` and cannot cross a thread boundary
  at all) for a different reason: an `OwnedFd` *could* cross a thread, but one
  opened by the caller and closed on the capture thread would have a lifetime
  spanning a `spawn`/`join`, and every error path in between would have to
  decide who closes it. A plain local descriptor whose `Drop` always runs on
  the thread that opened it avoids that question entirely.
- **Formats.** Modes are read from `VIDIOC_ENUM_FMT` × `VIDIOC_ENUM_FRAMESIZES`
  × `VIDIOC_ENUM_FRAMEINTERVALS`. Four-character codes map through an explicit
  table (e.g. `NV12` → `Nv12`, `YUYV` → `Yuyv422`, `UYVY` → `Uyvy422`, `MJPG` →
  `Mjpeg`); anything else is skipped with a `debug!` line rather than guessed
  at. A `STEPWISE`/`CONTINUOUS` frame-size range is not enumerated in full —
  doing so for, say, a 32×32-to-1920×1080-step-2 range would be 508,320
  distinct modes — it is sampled at its two corners and a step-snapped
  midpoint instead, so a caller asking for a size inside the range that is not
  one of those three gets the nearest of the three, never a fabricated exact
  match.
- **Negotiation is echo-based, which is a real difference from AVFoundation.**
  `VIDIOC_S_FMT`/`VIDIOC_S_PARM` do not fail when the driver cannot deliver
  exactly what was requested — they succeed and overwrite the request with
  what the driver *will* do. Geometry and pixel format must still echo back
  exactly or the mode is `CaptureError::FormatRejected`; the frame rate is
  accepted as echoed instead, with an `info!` line when it differs from the
  request, because rounding an interval (a camera whose descriptor lists
  30 fps happily accepting 29.97 and echoing back 30) is specified V4L2
  driver behaviour, not a fault. `CaptureFrame::format` always carries what
  the driver actually granted, which is the authority when it and
  `CaptureSession::negotiated_format()` disagree.
- **`buffer_count` is honoured.** Unlike the macOS and Windows backends, V4L2
  hands buffer-ring depth to the caller: `open()`'s `buffer_count` (clamped to
  at least one) becomes the `VIDIOC_REQBUFS` request, `mmap`ed once and
  requeued after every copy. `queue_depth` and `DropPolicy` still govern the
  crate's own delivery queue on top of that ring.
- **ABI honesty.** Every V4L2 struct this backend uses is a hand transcription
  of the sanitized kernel UAPI headers (`linux/videodev2.h`,
  `asm-generic/ioctl.h`) — no `bindgen`, no C compiled at build time. The
  transcription is pinned, not merely asserted: a golden layout table of
  `(size, align)` per struct is derived mechanically via `zig translate-c`
  plus comptime `@sizeOf`/`@alignOf`/`@offsetOf` against the UAPI header set
  bundled with Zig 0.16.0 (`linux/version.h` reports kernel 6.19), run across
  four triples (`x86_64-linux-gnu`, `aarch64-linux-gnu`,
  `arm-linux-gnueabihf`, `x86-linux-gnu`) that collapse onto the two arms
  Rust's `target_pointer_width` actually distinguishes. Those numbers back
  compile-time assertions, not merely test assertions: a plain `cargo check`
  fails immediately if a transcribed struct's shape drifts from the header.
  This crate's own check has been run and passes on
  `aarch64-unknown-linux-gnu` (64-bit) and `i686-unknown-linux-gnu` (32-bit).
  What this buys is confidence in the *layout*; it says nothing about the
  *driver conversation* — see "Live-device tests" below for the gap that
  closes.

### Windows specifics

- **Permissions.** No explicit permission grant is required to list devices,
  but listing is not side-effect-free: Media Foundation has no way to read a
  device's supported modes without briefly instantiating its media source, so
  `enumerate()` activates each camera just long enough to read its native
  media types, then releases it. That is visible — on hardware with a privacy
  LED, plain enumeration can flicker it, not just `open()`. Activating a
  device answers `E_ACCESSDENIED` when the camera privacy setting is off,
  which is reported as `CaptureError::PermissionDenied`; during `enumerate()`
  the same refusal — or a device held by another process — leaves that device
  listed with an empty format list rather than failing the whole call: it
  exists, and this process could not ask it anything.
- **Device ids** are the device's symbolic link, e.g.
  `\\?\usb#vid_046d&pid_0825&mi_00#...`. A source with no symbolic link is
  skipped: without a stable id it could be listed but never reopened.
- **Formats.** Modes are read from `IMFSourceReader::GetNativeMediaType`.
  Subtypes `NV12` → `Nv12`, `YUY2` → `Yuyv422`, `UYVY` → `Uyvy422` and `MJPG` →
  `Mjpeg`. Anything else is skipped with a `debug!` line rather than guessed at.
  Frame rates come from `MF_MT_FRAME_RATE` as an exact rational reduced to
  lowest terms, so `30000/1001` stays exact.
- **`MFVideoFormat_RGB24` is skipped on purpose.** Windows' uncompressed RGB is
  **B, G, R** in memory and usually bottom-up. `PixelFormat` has no `Bgr24`, so
  accepting the subtype would mean either delivering frames with red and blue
  swapped or permuting channels in the capture path — turning this backend into
  a converter. Skipping it keeps both promises.
- **Native formats only.** The source reader is created *without*
  `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING`, so Media Foundation never inserts
  a converter and the negotiated format describes the camera rather than a
  conversion. After `SetCurrentMediaType` the current type is read back and
  compared; a reader that settled on a different mode is
  `CaptureError::FormatRejected`.
- **MJPEG is delivered as a bitstream**, unlike the macOS backend: an `MJPG`
  mode yields `FramePayload::Compressed` with the JPEG untouched.
- **Bottom-up buffers** (`MF_MT_DEFAULT_STRIDE` negative) are re-ordered during
  the copy, so a delivered frame is always top-down.
- **`buffer_count` is ignored.** Media Foundation owns its capture buffer pool.
  `queue_depth` and `DropPolicy` still apply.

Everything that does not depend on a platform API is portable and tested
everywhere:

- **Format model** — `CaptureEncoding` separates raw pixel layouts from MJPEG,
  which is a bitstream and not a pixel layout. Where a backend can deliver
  MJPEG at all (Windows and Linux; see the platform sections above — the
  macOS backend cannot, `CaptureError::FormatRejected`), it is always a
  *passthrough*: the JPEG bytes the driver produced, handed back verbatim as
  `FramePayload::Compressed`, never decoded or re-encoded by this crate. The
  `mock` backend keeps that same honesty rather than faking it: it
  synthesizes only pixel data it can actually fill, and asking it for
  `CaptureEncoding::Mjpeg` returns `CaptureError::FormatRejected` rather than
  bytes that claim to be a JPEG and are not.
- **Negotiation** — `negotiate()` is a pure, total, deterministic function of
  the device's advertised modes and the request. Exact size beats everything;
  otherwise it downscales rather than upscales.
- **Delivery** — a bounded queue with `DropOldest` (default), `DropNewest` and
  `Block` policies, plus per-session statistics that keep this crate's own
  drops separate from the driver's.
- **Clocks** — device timestamps are normalized to a non-decreasing
  since-first-frame timeline; backwards jumps are clamped and counted, never
  silently passed through or replaced with a plausible-looking guess.

## Usage

```toml
[dependencies]
oximedia-capture = "0.2.1"
```

```rust,no_run
use oximedia_capture::{CaptureConfig, CaptureEncoding, DeviceSelector};
use oximedia_core::PixelFormat;

fn main() -> Result<(), oximedia_capture::CaptureError> {
    // 1. Enumerate. `Ok(vec![])` means "no camera plugged in"; `Err` means
    //    either no backend for this target or no permission to ask the OS.
    let devices = oximedia_capture::enumerate()?;
    let Some(device) = devices.first() else {
        eprintln!("no capture device found");
        return Ok(());
    };

    // 2. Open. `negotiate()` reconciles this request against what `device`
    //    actually advertises — size/fps/encoding below are preferences, not
    //    guarantees, and `DeviceSelector::Id` survives re-enumeration in a
    //    way a positional `Index` does not.
    let config = CaptureConfig::default()
        .with_device(DeviceSelector::Id(device.id.clone()))
        .with_size(1280, 720)
        .with_fps(30.0)
        .with_preferred([CaptureEncoding::Raw(PixelFormat::Nv12)]);
    let mut session = oximedia_capture::open(config)?;
    println!("negotiated {}", session.negotiated_format());

    // 3. Receive. `recv()` returns `Ok(None)` when the session has ended
    //    cleanly and `Err` on a transient device error — both distinct from
    //    a frame, and neither silently dropped.
    if let Some(mut stream) = session.take_stream() {
        while let Ok(Some(frame)) = stream.recv() {
            println!("frame {} at {:?}", frame.sequence, frame.timestamp);
        }
    }
    Ok(())
}
```

The format actually used is `session.negotiated_format()`, not the request.
`no_run` above because opening a real device: on macOS it raises the TCC
permission dialog, and everywhere it requires a camera actually be present —
neither is available where this README is rendered or doctested.

## Features

| Feature | Default | Effect |
|---|---|---|
| `mock` | off | A deterministic synthetic backend with a scripted frame count, cadence, failure point, clock and sequence counter. Also compiled automatically under `cfg(test)`. |

```rust,ignore
use std::time::Duration;
use oximedia_capture::{mock, CaptureConfig};

let script = mock::MockScript::default()
    .with_frames(120)
    .with_size(640, 480)
    .with_cadence(Duration::from_millis(16));
let mut session = mock::open(CaptureConfig::default(), script)?;
```

## Live-device tests

Tests that touch real hardware are opt-in through `OXIMEDIA_CAPTURE_DEVICE`,
which names the device id to open:

```bash
# macOS — the id is the AVCaptureDevice uniqueID reported by `enumerate()`
OXIMEDIA_CAPTURE_DEVICE=0x8020000005ac8514 \
    cargo test -p oximedia-capture --test live_capture -- --ignored

# Linux — the id is a /dev/video* node path
OXIMEDIA_CAPTURE_DEVICE=/dev/video0 \
    cargo test -p oximedia-capture --test live_capture -- --ignored

# Windows (PowerShell) — the id is the device's symbolic link, as `enumerate()` reports it
$env:OXIMEDIA_CAPTURE_DEVICE = "\\?\usb#vid_046d&pid_0825&mi_00#..."
cargo test -p oximedia-capture --test live_capture -- --ignored
```

Every one of them is also `#[ignore]`d, and they *fail* rather than pass when the
variable is unset: a live test that quietly passes on a machine with no camera
turns a green CI run into evidence of something that was never checked.

`tests/live_capture.rs` asserts that `enumerate()` finds the configured id, that
`open()` negotiates a mode the device advertised, that ten frames arrive within
five seconds, that device, host and sequence timestamps never go backwards, that
each payload matches the negotiated format byte for byte (plane count, stride
and total size against `PixelFormat::frame_buffer_size`), that a healthy session
reports no transient errors, and that `stop()` returns within 500 ms.

Running them on macOS raises the camera-permission dialog and lights the camera
indicator, which is the other reason they are off by default.

## License

Apache-2.0

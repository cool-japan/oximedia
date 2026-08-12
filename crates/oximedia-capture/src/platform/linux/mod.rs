//! Video4Linux2 capture backend for Linux.
//!
//! This module is the *safe* half of the backend: device-node discovery, the
//! `VIDIOC_ENUM_*` walks that turn a driver's advertised modes into
//! [`CaptureFormat`]s, and the pure conversions those walks depend on. Not one
//! line of it needs `unsafe`; every raw structure and every syscall lives in the
//! three sibling modules, which are the only files here that opt out of the
//! workspace's `unsafe_code = "deny"`:
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`uapi`] | `#[repr(C)]` transcription of `linux/videodev2.h`, with a golden layout table |
//! | [`ioctl`] | The fourteen opcodes, and one wrapper per ioctl |
//! | [`stream`] | `mmap`, `poll`, `DQBUF`, the per-frame copy, teardown |
//!
//! # Threading invariant
//!
//! **No file descriptor is opened by [`CaptureBackend::open`].**
//!
//! This mirrors the AVFoundation backend's rule, for a different reason. There,
//! Objective-C objects are `!Send` and *cannot* cross a thread boundary; here an
//! `OwnedFd` could, but a descriptor opened on the caller's thread and closed on
//! the capture thread has a lifetime that spans a `spawn` and a `join`, and any
//! error path between the two would have to decide who closes it. Opening inside
//! [`CaptureRunner::run`] makes the descriptor an ordinary local whose `Drop`
//! runs on the thread that created it, on every path, including the early
//! returns. `open()` therefore records plain data — a node path, the negotiated
//! [`CaptureFormat`], the requested buffer count — and nothing else.
//!
//! # What "supported" means here
//!
//! A `/dev/video*` node is reported only when `VIDIOC_QUERYCAP` says it can do
//! `V4L2_CAP_VIDEO_CAPTURE` *and* `V4L2_CAP_STREAMING`. That is not a
//! formality: a modern UVC camera registers several nodes — a metadata node
//! (`V4L2_CAP_META_CAPTURE`), sometimes a separate output or radio node — and
//! all of them answer `QUERYCAP`. Reporting them as cameras would make
//! [`crate::negotiate`] pick a device that can never deliver a frame.
//!
//! Multi-planar capture (`V4L2_CAP_VIDEO_CAPTURE_MPLANE`) is **not** implemented
//! by this package and is not silently treated as single-planar; a node that
//! offers only that is skipped with a `debug!` line naming why.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use crate::backend::{CaptureBackend, CaptureRunner, FrameSink, StopSignal};
use crate::config::CaptureConfig;
use crate::device::{BackendKind, CaptureDevice, CaptureFormat};
use crate::error::CaptureError;
use crate::platform::common::gcd;

mod ioctl;
mod stream;
mod uapi;

use uapi::{
    encoding_for_pixelformat, fourcc_name, pixelformat_for_encoding, v4l2_frmival_stepwise,
    v4l2_frmsize_stepwise, V4L2_CAP_STREAMING, V4L2_CAP_VIDEO_CAPTURE,
    V4L2_CAP_VIDEO_CAPTURE_MPLANE, V4L2_FMT_FLAG_EMULATED,
};

// ── Tunables ─────────────────────────────────────────────────────────────────

/// Directory the device nodes live in.
const DEVICE_DIRECTORY: &str = "/dev";

/// How long [`stream`] waits in `poll` before re-checking the stop flag.
///
/// This is pure shutdown latency: [`crate::CaptureSession::stop`] joins the
/// capture thread, and the thread cannot notice the flag faster than this. 200 ms
/// is well inside the crate's shutdown budget while still being long enough that
/// a 1 fps source does not spin.
pub(crate) const POLL_TIMEOUT: Duration = Duration::from_millis(200);

/// Hard ceiling on any `VIDIOC_ENUM_*` walk.
///
/// The walks are supposed to end when the driver answers `EINVAL`. A driver that
/// never does would spin here forever, and "the process hung while listing
/// cameras" is a far worse failure than "one implausible device advertised a
/// truncated mode list", so the loops are bounded and say so. No real device
/// comes close: a UVC camera offers a handful of formats, tens of sizes and a
/// handful of intervals.
const MAX_ENUMERATION_STEPS: u32 = 1024;

// ── Device-node names ────────────────────────────────────────────────────────

/// The numeric suffix of a `videoN` device-node name.
///
/// `None` for anything that is not exactly `video` followed by at least one
/// ASCII digit — which keeps `v4l-subdev0`, `video` and `videoX` out of the
/// scan, and keeps the sort below numeric rather than lexicographic (`video10`
/// must not come between `video1` and `video2`).
fn video_node_index(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("video")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// Decode a fixed-width, NUL-padded C string from a V4L2 struct.
///
/// `v4l2_capability::card` is 32 bytes and the kernel documents it as
/// NUL-terminated, but a firmware that fills all 32 bytes leaves no terminator,
/// so the whole array is the fallback. The bytes have no declared encoding —
/// they are whatever the device's descriptor held — so they are decoded lossily
/// rather than rejected: a camera whose name has a mangled byte in it is still a
/// camera, and refusing to list it would be a worse answer than one `U+FFFD`.
fn c_string_lossy(raw: &[u8]) -> String {
    let end = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

// ── Frame-rate arithmetic ────────────────────────────────────────────────────

/// Convert a V4L2 frame **interval** into a frame **rate**.
///
/// This inversion is the easiest thing in V4L2 to get backwards.
/// `VIDIOC_ENUM_FRAMEINTERVALS` and `v4l2_captureparm::timeperframe` report
/// *seconds per frame* as a `struct v4l2_fract` — 30 fps is `1/30`, and NTSC is
/// `1001/30000`. A frame rate is the reciprocal, so the numerator and
/// denominator simply swap: `1001/30000` s/frame is `30000/1001` frames/s.
/// Reading the fraction as a rate would report NTSC as 0.03337 fps and plain
/// 30 fps as 0.0333 fps, and both would survive every type check.
///
/// The result is reduced to lowest terms, because [`CaptureFormat`] derives
/// `PartialEq` field by field: without reduction a driver that reports `2/60`
/// and another that reports `1/30` would produce two "different" 30 fps modes,
/// defeating the duplicate filter in [`describe_formats`].
///
/// `None` when either term is zero — an interval of zero seconds is not a rate,
/// and a zero denominator is not a fraction.
fn fps_from_interval(numerator: u32, denominator: u32) -> Option<(u32, u32)> {
    if numerator == 0 || denominator == 0 {
        return None;
    }
    // Seconds per frame is `numerator / denominator`, so frames per second is
    // `denominator / numerator`.
    let divisor = gcd(denominator, numerator).max(1);
    Some((denominator / divisor, numerator / divisor))
}

/// Snap `value` onto the grid `min + k * step`, without leaving `[min, max]`.
fn snap_to_step(value: u32, min: u32, max: u32, step: u32) -> u32 {
    let step = step.max(1);
    let clamped = value.clamp(min, max);
    let snapped = min.saturating_add((clamped - min) / step * step);
    snapped.min(max)
}

/// Three representative sizes from a `STEPWISE` or `CONTINUOUS` frame-size
/// range.
///
/// **This is an approximation, and it is one on purpose.** A stepwise range such
/// as 32×32 to 1920×1080 in steps of 2 describes 508 320 distinct modes;
/// enumerating them would produce a device listing nothing can display, a
/// [`crate::negotiate`] call that ranks half a million candidates, and a
/// `CaptureDevice` that costs megabytes to clone. So the range is represented by
/// its two corners and a step-snapped midpoint.
///
/// The consequence is honest but worth stating: a caller who asks for a size
/// inside the range that is not one of these three gets the nearest of the three
/// rather than an exact match. Nothing is fabricated — every size returned is
/// one the driver said it supports, on the grid it gave — but the list is a
/// sample of the range, not the range.
///
/// Keeping that promise means the **maximum is snapped onto the grid too**, not
/// just the midpoint. The kernel documents a stepwise range as the sizes
/// `min`, `min + step`, `min + 2·step`, … up to and including `max`, so a range
/// whose span is not a whole multiple of its step — 32 to 1080 in steps of 16,
/// where the last size on the grid is 1072 — does not actually contain its own
/// stated maximum. Offering 1080 there would invent a mode the driver never
/// claimed, and `VIDIOC_S_FMT` would quietly hand back something else. For the
/// conforming ranges real drivers report, the snap is a no-op and the maximum
/// is offered exactly as given. The minimum needs no such treatment: it is the
/// grid's own origin.
///
/// Ranges whose bounds are nonsensical (zero, or `min > max`) yield nothing.
fn stepwise_size_corners(range: v4l2_frmsize_stepwise) -> Vec<(u32, u32)> {
    if range.min_width == 0
        || range.min_height == 0
        || range.min_width > range.max_width
        || range.min_height > range.max_height
    {
        return Vec::new();
    }
    // The largest size on the grid at or below the driver's stated maximum.
    let largest_width = snap_to_step(
        range.max_width,
        range.min_width,
        range.max_width,
        range.step_width,
    );
    let largest_height = snap_to_step(
        range.max_height,
        range.min_height,
        range.max_height,
        range.step_height,
    );
    let middle_width = snap_to_step(
        range.min_width + (largest_width - range.min_width) / 2,
        range.min_width,
        largest_width,
        range.step_width,
    );
    let middle_height = snap_to_step(
        range.min_height + (largest_height - range.min_height) / 2,
        range.min_height,
        largest_height,
        range.step_height,
    );

    let mut sizes = Vec::with_capacity(3);
    for candidate in [
        (range.min_width, range.min_height),
        (middle_width, middle_height),
        (largest_width, largest_height),
    ] {
        if !sizes.contains(&candidate) {
            sizes.push(candidate);
        }
    }
    sizes
}

/// The two rate endpoints of a `STEPWISE` or `CONTINUOUS` frame-interval range.
///
/// The shortest interval is the *highest* frame rate and the longest interval is
/// the *lowest*, so this returns the fastest and slowest rates the driver
/// admits. As with [`stepwise_size_corners`] the intermediate grid points are
/// not enumerated; unlike sizes, the endpoints are what callers actually ask for
/// ("give me the fastest this can do"), so no midpoint is synthesized.
fn stepwise_interval_endpoints(range: v4l2_frmival_stepwise) -> Vec<(u32, u32)> {
    let mut rates = Vec::with_capacity(2);
    for interval in [range.min, range.max] {
        if let Some(rate) = fps_from_interval(interval.numerator, interval.denominator) {
            if !rates.contains(&rate) {
                rates.push(rate);
            }
        }
    }
    rates
}

// ── Enumeration ──────────────────────────────────────────────────────────────

/// Open a device node for querying or streaming.
///
/// `O_NONBLOCK` is set so that no ioctl and no `read` can park the thread
/// indefinitely: the capture loop polls before every `VIDIOC_DQBUF` and treats
/// `EAGAIN` as "not yet", which keeps [`crate::CaptureSession::stop`]'s join
/// bounded by [`POLL_TIMEOUT`] no matter how the driver behaves.
fn open_node(path: &Path) -> Result<OwnedFd, Errno> {
    rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
}

/// Every `/dev/videoN` node, in numeric order.
fn video_nodes() -> Result<Vec<PathBuf>, CaptureError> {
    let entries = std::fs::read_dir(DEVICE_DIRECTORY)
        .map_err(|error| CaptureError::io(DEVICE_DIRECTORY, error))?;

    let mut nodes: Vec<(u32, PathBuf)> = Vec::new();
    for entry in entries {
        // A single unreadable directory entry must not abort the scan: `/dev` is
        // volatile, and a node that vanished between `readdir` and `stat` is a
        // re-plug, not a broken backend.
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(index) = video_node_index(name) {
            nodes.push((index, entry.path()));
        }
    }
    // Numeric, not lexicographic: `video2` must come before `video10`, because
    // enumeration order is what `DeviceSelector::Index` and `negotiate()`'s
    // final tie-break are defined against.
    nodes.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(nodes.into_iter().map(|(_, path)| path).collect())
}

/// List every `/dev/video*` node that is a streaming capture device.
///
/// # Permission
///
/// A node this process may not open is skipped with a `debug!` line, because a
/// machine can perfectly well have one camera the user may use and one they may
/// not. But if *every* node was refused and the list would therefore be empty,
/// an `Ok(vec![])` would say "this backend works and no camera is attached" —
/// which is exactly the ambiguity this crate refuses elsewhere, and it would
/// send the reader looking for a loose USB cable instead of at their group
/// membership. In that one case the refusal is reported as
/// [`CaptureError::PermissionDenied`], whose documentation already names Linux
/// group membership on the `/dev/video*` node as the usual cause.
fn enumerate_devices() -> Result<Vec<CaptureDevice>, CaptureError> {
    let nodes = video_nodes()?;
    let mut devices = Vec::new();
    let mut refused: Option<String> = None;

    for path in nodes {
        let node = path.to_string_lossy().into_owned();
        let fd = match open_node(&path) {
            Ok(fd) => fd,
            Err(errno) if errno == Errno::ACCESS || errno == Errno::PERM => {
                tracing::debug!(device = %node, "no permission to open device node; skipping");
                refused.get_or_insert(node);
                continue;
            }
            Err(errno) => {
                tracing::debug!(
                    device = %node,
                    error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    "device node could not be opened; skipping"
                );
                continue;
            }
        };
        if let Some(device) = describe_device(fd.as_fd(), &node) {
            devices.push(device);
        }
    }

    if devices.is_empty() {
        if let Some(device) = refused {
            return Err(CaptureError::PermissionDenied { device });
        }
    }
    Ok(devices)
}

/// Describe one already-opened node, or explain why it is not a camera.
fn describe_device(fd: BorrowedFd<'_>, node: &str) -> Option<CaptureDevice> {
    let capability = match ioctl::querycap(fd) {
        Ok(capability) => capability,
        Err(errno) => {
            tracing::debug!(
                device = %node,
                error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                "VIDIOC_QUERYCAP failed; not a V4L2 device"
            );
            return None;
        }
    };

    let caps = capability.effective_caps();
    if caps & V4L2_CAP_VIDEO_CAPTURE == 0 {
        if caps & V4L2_CAP_VIDEO_CAPTURE_MPLANE != 0 {
            tracing::debug!(
                device = %node,
                "node offers only multi-planar capture, which this backend does not implement"
            );
        } else {
            tracing::debug!(device = %node, caps = format!("{caps:#010x}"), "not a video capture node");
        }
        return None;
    }
    if caps & V4L2_CAP_STREAMING == 0 {
        tracing::debug!(
            device = %node,
            "node has no streaming I/O; this backend has no read() fallback"
        );
        return None;
    }

    let name = {
        let card = c_string_lossy(&capability.card);
        if card.is_empty() {
            // A driver that left `card` blank still has a device; naming it
            // after its node is the honest fallback, and it is what `v4l2-ctl`
            // shows in the same situation.
            node.to_owned()
        } else {
            card
        }
    };

    let formats = describe_formats(fd, node);
    if formats.is_empty() {
        tracing::debug!(device = %node, "device advertises no format this backend understands");
    }
    Some(CaptureDevice {
        id: node.to_owned(),
        name,
        formats,
        backend: BackendKind::V4l2,
    })
}

/// Walk `VIDIOC_ENUM_FMT` × `VIDIOC_ENUM_FRAMESIZES` × `VIDIOC_ENUM_FRAMEINTERVALS`.
///
/// Formats whose four-character code is not in
/// [`encoding_for_pixelformat`]'s table are skipped with a `debug!` line, never
/// guessed at — see that function for why a wrong guess is more expensive than a
/// missing mode.
fn describe_formats(fd: BorrowedFd<'_>, node: &str) -> Vec<CaptureFormat> {
    let mut formats: Vec<CaptureFormat> = Vec::new();

    for index in 0..MAX_ENUMERATION_STEPS {
        let descriptor = match ioctl::enum_fmt(fd, index) {
            Ok(descriptor) => descriptor,
            // The documented end of the walk.
            Err(Errno::INVAL) => break,
            Err(errno) => {
                tracing::debug!(
                    device = %node,
                    index,
                    error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    "VIDIOC_ENUM_FMT failed; ending the format walk"
                );
                break;
            }
        };

        let code = descriptor.pixelformat;
        let Some(encoding) = encoding_for_pixelformat(code) else {
            tracing::debug!(
                device = %node,
                fourcc = %fourcc_name(code),
                description = %c_string_lossy(&descriptor.description),
                "skipping format with an unsupported pixel format"
            );
            continue;
        };
        if descriptor.flags & V4L2_FMT_FLAG_EMULATED != 0 {
            // Not a reason to skip — an emulated format is still delivered — but
            // it means the kernel is converting, so the frame rate the device
            // advertises may not survive contact with the CPU.
            tracing::debug!(
                device = %node,
                fourcc = %fourcc_name(code),
                "format is emulated in software by the kernel"
            );
        }

        for (width, height) in frame_sizes(fd, code, node) {
            if width == 0 || height == 0 {
                continue;
            }
            for (fps_num, fps_den) in frame_rates(fd, code, width, height, node) {
                let candidate = CaptureFormat::new(encoding, width, height, fps_num, fps_den);
                if !formats.contains(&candidate) {
                    formats.push(candidate);
                }
            }
        }
    }
    formats
}

/// Every frame size the device offers for `code`.
///
/// Discrete ranges are expanded in full; stepwise and continuous ones are
/// sampled by [`stepwise_size_corners`]. A driver that does not implement
/// `VIDIOC_ENUM_FRAMESIZES` at all — which older and simpler drivers do not —
/// falls back to the one size it is configured for right now, read with
/// `VIDIOC_G_FMT`. That is a real mode the device really supports, not a guess.
fn frame_sizes(fd: BorrowedFd<'_>, code: u32, node: &str) -> Vec<(u32, u32)> {
    let mut sizes: Vec<(u32, u32)> = Vec::new();

    for index in 0..MAX_ENUMERATION_STEPS {
        let entry = match ioctl::enum_framesizes(fd, index, code) {
            Ok(entry) => entry,
            Err(Errno::INVAL) => break,
            Err(errno) => {
                tracing::debug!(
                    device = %node,
                    fourcc = %fourcc_name(code),
                    index,
                    error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    "VIDIOC_ENUM_FRAMESIZES failed; ending the frame-size walk"
                );
                break;
            }
        };

        if let Some(discrete) = entry.discrete() {
            let candidate = (discrete.width, discrete.height);
            if !sizes.contains(&candidate) {
                sizes.push(candidate);
            }
            continue;
        }
        if let Some(range) = entry.stepwise() {
            tracing::debug!(
                device = %node,
                fourcc = %fourcc_name(code),
                min = format!("{}x{}", range.min_width, range.min_height),
                max = format!("{}x{}", range.max_width, range.max_height),
                step = format!("{}x{}", range.step_width, range.step_height),
                "sampling a stepwise frame-size range at its corners and midpoint"
            );
            for candidate in stepwise_size_corners(range) {
                if !sizes.contains(&candidate) {
                    sizes.push(candidate);
                }
            }
            // A stepwise or continuous range is always the whole answer: the
            // kernel documents index 0 as the only valid one for these types.
            break;
        }
        tracing::debug!(
            device = %node,
            fourcc = %fourcc_name(code),
            frmsize_type = entry.type_,
            "unknown frame-size type; ending the frame-size walk"
        );
        break;
    }

    if sizes.is_empty() {
        if let Some(current) = current_frame_size(fd, code, node) {
            sizes.push(current);
        }
    }
    sizes
}

/// The size the device is configured for right now, if it matches `code`.
///
/// Used only when `VIDIOC_ENUM_FRAMESIZES` produced nothing.
fn current_frame_size(fd: BorrowedFd<'_>, code: u32, node: &str) -> Option<(u32, u32)> {
    let format = ioctl::g_fmt(fd)
        .inspect_err(|errno| {
            tracing::debug!(
                device = %node,
                error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                "VIDIOC_G_FMT failed; the device reports no frame size at all"
            );
        })
        .ok()?;
    let pix = format.pix();
    if pix.pixelformat != code || pix.width == 0 || pix.height == 0 {
        return None;
    }
    tracing::debug!(
        device = %node,
        fourcc = %fourcc_name(code),
        width = pix.width,
        height = pix.height,
        "device does not enumerate frame sizes; reporting its current one"
    );
    Some((pix.width, pix.height))
}

/// Every frame rate the device offers for `code` at `width` × `height`.
///
/// The values V4L2 reports are *intervals*; [`fps_from_interval`] does the
/// inversion. A device that enumerates no interval falls back to the one it is
/// configured for (`VIDIOC_G_PARM`), and a device that reports none at all
/// yields `(0, 0)` — which [`CaptureFormat`] documents as "unknown / variable"
/// and which is the honest record. Inventing 30 fps here would be a guess that
/// every downstream timestamp calculation would then trust.
fn frame_rates(
    fd: BorrowedFd<'_>,
    code: u32,
    width: u32,
    height: u32,
    node: &str,
) -> Vec<(u32, u32)> {
    let mut rates: Vec<(u32, u32)> = Vec::new();

    for index in 0..MAX_ENUMERATION_STEPS {
        let entry = match ioctl::enum_frameintervals(fd, index, code, width, height) {
            Ok(entry) => entry,
            Err(Errno::INVAL) => break,
            Err(errno) => {
                tracing::debug!(
                    device = %node,
                    fourcc = %fourcc_name(code),
                    index,
                    error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    "VIDIOC_ENUM_FRAMEINTERVALS failed; ending the interval walk"
                );
                break;
            }
        };

        if let Some(interval) = entry.discrete() {
            if let Some(rate) = fps_from_interval(interval.numerator, interval.denominator) {
                if !rates.contains(&rate) {
                    rates.push(rate);
                }
            }
            continue;
        }
        if let Some(range) = entry.stepwise() {
            for rate in stepwise_interval_endpoints(range) {
                if !rates.contains(&rate) {
                    rates.push(rate);
                }
            }
            break;
        }
        break;
    }

    if rates.is_empty() {
        if let Some(rate) = current_frame_rate(fd) {
            rates.push(rate);
        } else {
            tracing::debug!(
                device = %node,
                fourcc = %fourcc_name(code),
                width,
                height,
                "device reports no usable frame rate for this mode"
            );
            rates.push((0, 0));
        }
    }
    rates
}

/// The frame rate the device is configured for right now.
fn current_frame_rate(fd: BorrowedFd<'_>) -> Option<(u32, u32)> {
    let parm = ioctl::g_parm(fd).ok()?;
    let interval = parm.capture().timeperframe;
    fps_from_interval(interval.numerator, interval.denominator)
}

// ── Backend ──────────────────────────────────────────────────────────────────

/// The Video4Linux2 capture backend.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct V4l2Backend;

impl V4l2Backend {
    /// A backend handle. Holds no state and opens nothing until used.
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl CaptureBackend for V4l2Backend {
    fn kind(&self) -> BackendKind {
        BackendKind::V4l2
    }

    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        enumerate_devices()
    }

    fn open(
        &self,
        device: &CaptureDevice,
        format: CaptureFormat,
        config: &CaptureConfig,
    ) -> Result<Box<dyn CaptureRunner>, CaptureError> {
        // Deliberately opens nothing; see the module docs. The one check made
        // here is the one that needs no device: an encoding this backend cannot
        // ask a driver for would fail on the capture thread, where the error
        // reaches the caller as a dead session rather than as a failed `open()`.
        if pixelformat_for_encoding(format.encoding).is_none() {
            return Err(CaptureError::format_rejected(
                device.id.clone(),
                format,
                "no V4L2 pixel format corresponds to this encoding",
            ));
        }
        Ok(Box::new(V4l2Runner {
            node: device.id.clone(),
            name: device.name.clone(),
            format,
            buffer_count: config.buffer_count.max(1),
        }))
    }
}

// ── Runner ───────────────────────────────────────────────────────────────────

/// The capture loop for one opened V4L2 device.
///
/// Plain data only — no descriptor, no mapping. See the module docs.
#[derive(Debug, Clone)]
pub(crate) struct V4l2Runner {
    /// Device-node path, e.g. `/dev/video0`.
    node: String,
    /// `v4l2_capability::card`, carried for logs and errors.
    name: String,
    /// The mode [`crate::negotiate()`] settled on.
    format: CaptureFormat,
    /// How many driver buffers to request. Clamped to at least one.
    buffer_count: u32,
}

impl V4l2Runner {
    /// The device node this runner will open.
    pub(crate) fn node(&self) -> &str {
        &self.node
    }

    /// Human-readable device name, for logs and errors.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The negotiated mode.
    pub(crate) const fn format(&self) -> CaptureFormat {
        self.format
    }

    /// How many driver buffers to request.
    pub(crate) const fn buffer_count(&self) -> u32 {
        self.buffer_count
    }
}

impl CaptureRunner for V4l2Runner {
    fn run(&mut self, sink: &FrameSink, stop: &StopSignal) -> Result<(), CaptureError> {
        stream::run_capture(self, sink, stop)
    }
}

#[cfg(test)]
mod tests {
    use oximedia_core::PixelFormat;

    use super::*;
    use crate::device::CaptureEncoding;
    use uapi::{v4l2_fract, V4L2_PIX_FMT_MJPEG, V4L2_PIX_FMT_NV12};

    // ── Device-node names ───────────────────────────────────────────────────

    #[test]
    fn only_video_nodes_with_a_numeric_suffix_are_recognised() {
        assert_eq!(video_node_index("video0"), Some(0));
        assert_eq!(video_node_index("video7"), Some(7));
        assert_eq!(video_node_index("video10"), Some(10));
        assert_eq!(video_node_index("video255"), Some(255));
    }

    #[test]
    fn nodes_that_are_not_capture_devices_are_not_recognised() {
        for name in [
            "video",       // no index
            "videoX",      // not a digit
            "video0a",     // trailing junk
            "v4l-subdev0", // a sub-device, not a capture node
            "vbi0",        // VBI
            "media0",      // media controller
            "radio0",
            "",
            "Video0", // case matters; udev only makes lower-case names
        ] {
            assert_eq!(video_node_index(name), None, "{name} must not be scanned");
        }
    }

    #[test]
    fn an_index_too_large_for_u32_is_rejected_rather_than_wrapped() {
        assert_eq!(video_node_index("video99999999999999999999"), None);
    }

    /// `video10` sorting between `video1` and `video2` would silently renumber
    /// every `DeviceSelector::Index`, so the ordering is asserted directly.
    #[test]
    fn node_indices_order_numerically_not_lexicographically() {
        let mut names = ["video10", "video2", "video1", "video0"];
        names.sort_by_key(|name| video_node_index(name).unwrap_or(u32::MAX));
        assert_eq!(names, ["video0", "video1", "video2", "video10"]);
    }

    // ── Card names ──────────────────────────────────────────────────────────

    #[test]
    fn a_terminated_card_name_stops_at_the_nul() {
        let mut raw = [0_u8; 32];
        raw[..9].copy_from_slice(b"HD Webcam");
        assert_eq!(c_string_lossy(&raw), "HD Webcam");
    }

    #[test]
    fn an_unterminated_card_name_uses_the_whole_field() {
        // A firmware that fills all 32 bytes leaves no terminator.
        let raw = *b"0123456789012345678901234567890A";
        assert_eq!(c_string_lossy(&raw), "0123456789012345678901234567890A");
    }

    #[test]
    fn an_empty_card_name_is_empty_not_a_placeholder() {
        assert_eq!(c_string_lossy(&[0_u8; 32]), "");
        assert_eq!(c_string_lossy(&[]), "");
    }

    #[test]
    fn a_non_utf8_card_name_is_decoded_lossily_rather_than_dropped() {
        let raw = [b'C', b'a', b'm', 0xFF, 0xFE, b'!', 0];
        let name = c_string_lossy(&raw);
        assert!(name.starts_with("Cam"), "{name}");
        assert!(name.ends_with('!'), "{name}");
        assert!(name.contains('\u{FFFD}'), "{name}");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let mut raw = [0_u8; 32];
        raw[..13].copy_from_slice(b"  Some Cam   ");
        assert_eq!(c_string_lossy(&raw), "Some Cam");
    }

    // ── Frame-rate arithmetic ───────────────────────────────────────────────

    /// The single most important conversion in this file: V4L2 reports seconds
    /// per frame, and this crate models frames per second.
    #[test]
    fn a_frame_interval_inverts_into_a_frame_rate() {
        // NTSC: 1001/30000 s per frame is 30000/1001 fps ~ 29.97.
        assert_eq!(fps_from_interval(1001, 30_000), Some((30_000, 1001)));
        let ntsc = CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 30_000, 1001);
        assert!((ntsc.fps() - 29.970_029_97).abs() < 1e-6);

        // The plain cases every UVC camera reports.
        assert_eq!(fps_from_interval(1, 30), Some((30, 1)));
        assert_eq!(fps_from_interval(1, 60), Some((60, 1)));
        assert_eq!(fps_from_interval(1, 5), Some((5, 1)));
    }

    #[test]
    fn the_inversion_is_not_the_identity() {
        // If numerator and denominator were passed through unswapped, 1/30 s
        // per frame would be reported as 1/30 fps. This is the regression test
        // for that.
        let (fps_num, fps_den) = fps_from_interval(1, 30).expect("a real interval");
        assert_eq!((fps_num, fps_den), (30, 1));
        assert!(
            fps_num > fps_den,
            "30 fps must be greater than one, not 1/30"
        );
    }

    #[test]
    fn rates_are_reduced_so_equal_rates_compare_equal() {
        // A driver reporting 2/60 and one reporting 1/30 describe the same mode.
        assert_eq!(fps_from_interval(2, 60), fps_from_interval(1, 30));
        assert_eq!(fps_from_interval(2002, 60_000), Some((30_000, 1001)));
        assert_eq!(fps_from_interval(1000, 30_000), Some((30, 1)));
    }

    #[test]
    fn reduction_never_changes_the_rate_it_describes() {
        for (numerator, denominator) in [(1001_u32, 30_000_u32), (1, 30), (2, 60), (7, 13)] {
            let (fps_num, fps_den) =
                fps_from_interval(numerator, denominator).expect("a real interval");
            // fps_num / fps_den == denominator / numerator
            assert_eq!(
                u64::from(fps_num) * u64::from(numerator),
                u64::from(fps_den) * u64::from(denominator),
                "{fps_num}/{fps_den} is not {denominator}/{numerator}"
            );
            assert_eq!(gcd(fps_num, fps_den), 1, "{fps_num}/{fps_den} not reduced");
        }
    }

    #[test]
    fn a_degenerate_interval_is_not_a_rate() {
        assert_eq!(fps_from_interval(0, 30), None);
        assert_eq!(fps_from_interval(1, 0), None);
        assert_eq!(fps_from_interval(0, 0), None);
    }

    // ── Stepwise sampling ───────────────────────────────────────────────────

    fn stepwise(min: (u32, u32), max: (u32, u32), step: (u32, u32)) -> v4l2_frmsize_stepwise {
        v4l2_frmsize_stepwise {
            min_width: min.0,
            max_width: max.0,
            step_width: step.0,
            min_height: min.1,
            max_height: max.1,
            step_height: step.1,
        }
    }

    #[test]
    fn a_stepwise_range_is_sampled_at_both_corners_and_a_midpoint() {
        // A conforming range: both spans are whole multiples of their step, so
        // the driver's own maximum is on the grid and is offered verbatim.
        let sizes = stepwise_size_corners(stepwise((32, 32), (1920, 1080), (16, 8)));
        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0], (32, 32), "the minimum must be offered");
        assert_eq!(sizes[2], (1920, 1080), "the maximum must be offered");
        let (middle_width, middle_height) = sizes[1];
        assert!(middle_width > 32 && middle_width < 1920, "{middle_width}");
        assert!(
            middle_height > 32 && middle_height < 1080,
            "{middle_height}"
        );
    }

    /// A range whose span is not a whole multiple of its step does not contain
    /// its own stated maximum: from 32 in steps of 16 the sizes are 32, 48, …,
    /// 1072, and then 1088 — 1080 is never one of them. Offering it would be a
    /// mode this device cannot be set to.
    #[test]
    fn a_maximum_off_the_grid_is_snapped_down_rather_than_offered_as_is() {
        let sizes = stepwise_size_corners(stepwise((32, 32), (1920, 1080), (16, 16)));
        assert_eq!(
            sizes[sizes.len() - 1],
            (1920, 1072),
            "1920 is on the width grid and stays; 1080 is not on the height grid"
        );
    }

    /// Every sampled size must be a size the driver actually said it supports:
    /// on the grid, and inside the range. A midpoint off the grid would be a
    /// fabricated mode.
    #[test]
    fn every_sampled_size_is_on_the_grid_the_driver_gave() {
        for range in [
            stepwise((32, 32), (1920, 1080), (16, 16)),
            stepwise((48, 32), (640, 480), (2, 2)),
            stepwise((176, 144), (1280, 720), (1, 1)),
            stepwise((320, 240), (321, 241), (1, 1)),
        ] {
            for (width, height) in stepwise_size_corners(range) {
                assert!(
                    width >= range.min_width && width <= range.max_width,
                    "{width}"
                );
                assert!(
                    height >= range.min_height && height <= range.max_height,
                    "{height}"
                );
                assert_eq!(
                    (width - range.min_width) % range.step_width.max(1),
                    0,
                    "{width} is off the width grid"
                );
                assert_eq!(
                    (height - range.min_height) % range.step_height.max(1),
                    0,
                    "{height} is off the height grid"
                );
            }
        }
    }

    #[test]
    fn a_degenerate_range_yields_a_single_size_rather_than_duplicates() {
        let sizes = stepwise_size_corners(stepwise((640, 480), (640, 480), (1, 1)));
        assert_eq!(sizes, vec![(640, 480)]);
    }

    #[test]
    fn a_zero_step_does_not_divide_by_zero() {
        // Some drivers report step 0 for a continuous range; the kernel means 1.
        let sizes = stepwise_size_corners(stepwise((16, 16), (64, 64), (0, 0)));
        assert!(!sizes.is_empty());
        assert_eq!(sizes[0], (16, 16));
    }

    #[test]
    fn a_nonsensical_range_yields_nothing_rather_than_a_made_up_mode() {
        assert!(stepwise_size_corners(stepwise((0, 32), (1920, 1080), (2, 2))).is_empty());
        assert!(stepwise_size_corners(stepwise((32, 0), (1920, 1080), (2, 2))).is_empty());
        assert!(stepwise_size_corners(stepwise((1920, 32), (640, 480), (2, 2))).is_empty());
        assert!(stepwise_size_corners(stepwise((32, 1080), (640, 480), (2, 2))).is_empty());
    }

    #[test]
    fn snapping_stays_inside_the_range() {
        assert_eq!(snap_to_step(100, 32, 1920, 16), 96);
        assert_eq!(
            snap_to_step(5, 32, 1920, 16),
            32,
            "below the minimum clamps up"
        );
        assert_eq!(
            snap_to_step(9999, 32, 1920, 16),
            1920,
            "above the maximum clamps down"
        );
        assert_eq!(
            snap_to_step(33, 32, 64, 0),
            33,
            "a zero step behaves as one"
        );
    }

    // ── Stepwise intervals ──────────────────────────────────────────────────

    #[test]
    fn a_stepwise_interval_range_yields_its_fastest_and_slowest_rates() {
        let range = v4l2_frmival_stepwise {
            // Shortest interval = highest rate.
            min: v4l2_fract {
                numerator: 1,
                denominator: 60,
            },
            // Longest interval = lowest rate.
            max: v4l2_fract {
                numerator: 1,
                denominator: 5,
            },
            step: v4l2_fract {
                numerator: 1,
                denominator: 1000,
            },
        };
        let rates = stepwise_interval_endpoints(range);
        assert_eq!(rates, vec![(60, 1), (5, 1)]);
    }

    #[test]
    fn an_interval_range_with_no_usable_endpoint_yields_nothing() {
        let range = v4l2_frmival_stepwise::default();
        assert!(stepwise_interval_endpoints(range).is_empty());
    }

    // ── Backend surface ─────────────────────────────────────────────────────

    fn device(id: &str, format: CaptureFormat) -> CaptureDevice {
        CaptureDevice {
            id: id.to_owned(),
            name: "Test Camera".into(),
            formats: vec![format],
            backend: BackendKind::V4l2,
        }
    }

    #[test]
    fn the_backend_names_itself_v4l2() {
        let backend = V4l2Backend::new();
        assert_eq!(backend.kind(), BackendKind::V4l2);
        assert!(backend.kind().is_available());
    }

    #[test]
    fn open_records_the_request_without_touching_the_device() {
        // No node is opened and no ioctl is issued: `open` is pure bookkeeping,
        // which is the threading invariant this backend depends on.
        let format = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 1280, 720, 30, 1);
        let runner = V4l2Backend::new()
            .open(
                &device("/dev/video0", format),
                format,
                &CaptureConfig::default().with_buffer_count(6),
            )
            .expect("open records the request");
        drop(runner);
    }

    #[test]
    fn open_refuses_an_encoding_no_v4l2_code_describes() {
        // `Gray16` is a perfectly good `PixelFormat` with no entry in this
        // backend's table; asking a driver for it is impossible, and saying so
        // now is better than a session that dies on its capture thread.
        let format = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Gray16), 640, 480, 30, 1);
        match V4l2Backend::new().open(
            &device("/dev/video0", format),
            format,
            &CaptureConfig::default(),
        ) {
            Err(error @ CaptureError::FormatRejected { .. }) => {
                assert!(error.to_string().contains("Gray16"), "{error}");
            }
            Err(other) => panic!("expected FormatRejected, got {other:?}"),
            Ok(_) => panic!("Gray16 has no V4L2 code and must not open"),
        }
    }

    #[test]
    fn runner_accessors_report_what_open_recorded() {
        let format = CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 30, 1);
        let runner = V4l2Runner {
            node: "/dev/video2".into(),
            name: "A Camera".into(),
            format,
            buffer_count: 5,
        };
        assert_eq!(runner.node(), "/dev/video2");
        assert_eq!(runner.name(), "A Camera");
        assert_eq!(runner.format(), format);
        assert_eq!(runner.buffer_count(), 5);
    }

    #[test]
    fn a_zero_buffer_count_is_clamped_before_it_reaches_reqbufs() {
        // `REQBUFS` with count 0 means "free the ring", which would start a
        // capture with nothing to capture into.
        let format = CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 1);
        let mut config = CaptureConfig::default();
        config.buffer_count = 0;
        let runner = V4l2Backend::new()
            .open(&device("/dev/video0", format), format, &config)
            .expect("open records the request");
        drop(runner);
    }

    #[test]
    fn the_runner_is_send_because_it_holds_no_descriptor() {
        const fn assert_send<T: Send>() {}
        assert_send::<V4l2Runner>();
        assert_send::<V4l2Backend>();
    }

    // ── Enumeration against whatever this machine has ───────────────────────

    /// Exercises the whole ioctl half of enumeration — `QUERYCAP`, the
    /// capability filter, the card-name decode and all three `ENUM_*` walks —
    /// against the real `/dev`. It passes on a machine with no camera (an empty
    /// list is the right answer there) and on one with several.
    #[test]
    fn enumeration_describes_real_devices_or_says_why_it_cannot() {
        let devices = match enumerate_devices() {
            Ok(devices) => devices,
            // The one other honest outcome: every node exists and none may be
            // opened by this user.
            Err(CaptureError::PermissionDenied { device }) => {
                assert!(device.starts_with("/dev/video"), "{device}");
                return;
            }
            Err(CaptureError::Io { device, .. }) => {
                // A container without `/dev` mounted. Still not "no cameras".
                assert_eq!(device, DEVICE_DIRECTORY);
                return;
            }
            Err(other) => panic!("unexpected enumeration failure: {other:?}"),
        };

        for device in &devices {
            assert!(device.id.starts_with("/dev/video"), "{device}");
            assert!(!device.name.is_empty(), "every device needs a name");
            assert_eq!(device.backend, BackendKind::V4l2);
            for format in &device.formats {
                assert!(format.width > 0, "{format}");
                assert!(format.height > 0, "{format}");
                // A rate is either fully known or fully unknown; a half-filled
                // pair would mean something invented one of the two.
                assert!(
                    (format.fps_num == 0) == (format.fps_den == 0),
                    "half-known frame rate {}/{}",
                    format.fps_num,
                    format.fps_den
                );
                assert!(
                    pixelformat_for_encoding(format.encoding).is_some(),
                    "enumerated a format this backend cannot request: {format}"
                );
            }
            // Enumeration order is `negotiate()`'s final tie-break, so a
            // duplicate entry would make selection ambiguous.
            for (index, format) in device.formats.iter().enumerate() {
                assert!(
                    !device.formats[index + 1..].contains(format),
                    "duplicate format {format} on {}",
                    device.id
                );
            }
        }
    }

    #[test]
    fn scanning_dev_never_returns_a_node_that_is_not_a_video_node() {
        let Ok(nodes) = video_nodes() else {
            // No `/dev` to scan; nothing to assert.
            return;
        };
        let mut previous = None;
        for node in &nodes {
            let name = node
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let index = video_node_index(name).expect("only video nodes may be scanned");
            if let Some(previous) = previous {
                assert!(index >= previous, "nodes must come back in numeric order");
            }
            previous = Some(index);
        }
    }

    #[test]
    fn every_requestable_encoding_round_trips_through_the_format_table() {
        // The guard `open()` applies must agree with what enumeration can emit,
        // or a device could advertise a mode that `open()` then refuses.
        for code in [V4L2_PIX_FMT_NV12, V4L2_PIX_FMT_MJPEG] {
            let encoding = encoding_for_pixelformat(code).expect("a mapped code");
            assert_eq!(pixelformat_for_encoding(encoding), Some(code));
        }
    }
}

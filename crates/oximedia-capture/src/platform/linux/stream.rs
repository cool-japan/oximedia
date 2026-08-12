//! The V4L2 streaming loop: `mmap`, `poll`, `DQBUF`, copy, `QBUF`.
//!
//! Everything that touches driver memory or a syscall that can block lives
//! here. The pure decisions it depends on — which pixel formats exist, how a
//! frame interval inverts into a frame rate, where each plane starts inside a
//! driver buffer — are either in [`super`] or in [`plane_copies`] below, and are
//! unit-tested without a camera.
//!
//! # Negotiation is echo-based, and that is a real difference from AVFoundation
//!
//! `VIDIOC_S_FMT` and `VIDIOC_S_PARM` do not fail when the driver cannot give
//! you what you asked for. They succeed, and overwrite your request with what
//! the driver *will* do. So this module compares, and it treats the two halves
//! differently on purpose:
//!
//! * **Geometry and pixel format must match exactly.** A driver that echoes back
//!   a different size or four-character code is reported as
//!   [`CaptureError::FormatRejected`], never accepted. Delivering 640×480 frames
//!   labelled 1280×720 would make every consumer that trusts
//!   [`CaptureFrame::width`](crate::CaptureFrame::width) read past the end of
//!   the samples.
//! * **The frame rate is accepted as echoed**, and the delivered
//!   [`CaptureFrame::format`] carries the rate the driver granted, with an
//!   `info!` line when it differs from the request.
//!
//! The asymmetry is deliberate and is a documented divergence from the macOS
//! backend, which rejects a frame-rate mismatch. There,
//! `-[AVCaptureDevice setActiveVideoMinFrameDuration:]` has an enumerated set of
//! supported durations and asking for one outside it is a programming error. In
//! V4L2, rounding the interval is the *specified* behaviour of `VIDIOC_S_PARM` —
//! a UVC camera whose descriptor lists 30 fps will happily accept 29.97 and echo
//! back 30 — so refusing every rounded rate would refuse most cameras for doing
//! exactly what the standard says. Recording the granted rate on each frame
//! keeps the result honest without keeping it useless.
//!
//! Note the one consequence: when the driver rounds,
//! [`CaptureSession::negotiated_format`](crate::CaptureSession::negotiated_format)
//! reports what [`crate::negotiate()`] picked from the device's advertised list,
//! while [`CaptureFrame::format`] reports what the driver granted. The frame is
//! the authority, and the `info!` line names both.
//!
//! # Why frames are not staged through [`BufferPool`](crate::pool::BufferPool)
//!
//! Same argument as the macOS backend. A pooled buffer returns to its free-list
//! when its guard drops, and `PooledBuffer::into_inner` removes it from the pool
//! *permanently* — after `capacity` frames the pool is empty. A [`VideoFrame`]
//! plane owns its `Vec<u8>` and is handed to the consumer, so it can never be
//! given back. Staging through the pool and then copying into the plane would
//! double the per-frame copy — around 500 MB/s of extra traffic at 1080p60 NV12
//! — to save an allocation `Vec::with_capacity` already makes cheap. One copy,
//! straight from the `mmap`ed driver buffer into the plane, is the right shape.

#![allow(unsafe_code)]

use core::ffi::c_void;
use std::path::Path;
use std::time::{Duration, Instant};

use oximedia_codec::{Plane, VideoFrame};
use oximedia_core::PixelFormat;
use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fd::{AsFd, BorrowedFd};
use rustix::io::Errno;
use rustix::mm::{MapFlags, ProtFlags};

use crate::backend::{FrameSink, StopSignal};
use crate::device::{CaptureEncoding, CaptureFormat};
use crate::error::CaptureError;
use crate::frame::{CaptureFrame, FramePayload, TimestampSource};

use super::ioctl;
use super::uapi::{
    fourcc_name, pixelformat_for_encoding, v4l2_buffer, v4l2_captureparm, v4l2_format, v4l2_fract,
    v4l2_pix_format, V4L2_CAP_TIMEPERFRAME, V4L2_FIELD_NONE,
};
use super::{open_node, V4l2Runner, POLL_TIMEOUT};

// ── Plane geometry ───────────────────────────────────────────────────────────

/// How one plane is laid out inside a driver buffer, and what it becomes.
///
/// `source_stride` is the driver's row pitch and `stride` is this crate's; the
/// two are equal only when the driver chose no padding. Copying `stride` bytes
/// per row out of a `source_stride`-pitched buffer is the whole job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlaneCopy {
    /// Byte offset of this plane's first row within the driver buffer.
    offset: usize,
    /// Distance in bytes between two consecutive source rows.
    source_stride: usize,
    /// Meaningful bytes in one row — also the destination row pitch.
    stride: usize,
    /// Number of rows.
    rows: usize,
    /// Plane width in pixels, as [`Plane::width`] records it.
    width: u32,
    /// Plane height in pixels, as [`Plane::height`] records it.
    height: u32,
}

impl PlaneCopy {
    /// Bytes that must be readable from the buffer for this plane to be copied.
    ///
    /// The *last* row only needs `stride` bytes, not `source_stride`, because
    /// the trailing padding is never read. Computing the bound that precisely is
    /// what lets a driver whose `sizeimage` omits the final row's padding still
    /// deliver frames.
    ///
    /// `None` on arithmetic overflow, which is a geometry that cannot exist.
    const fn required_bytes(&self) -> Option<usize> {
        if self.rows == 0 {
            return Some(self.offset);
        }
        let Some(last_row) = (self.rows - 1).checked_mul(self.source_stride) else {
            return None;
        };
        let Some(end) = last_row.checked_add(self.stride) else {
            return None;
        };
        self.offset.checked_add(end)
    }
}

/// Chroma-plane row pitch, as a fraction of `bytesperline`.
///
/// V4L2 defines `bytesperline` as the row pitch of the **largest** plane — the
/// luma one for every planar format here — and leaves the chroma pitch implied
/// by the subsampling:
///
/// * semi-planar 4:2:0 (`NV12`, `NV21`) interleaves Cb and Cr, so one chroma row
///   covers two pixels with two bytes: the *same* pitch as luma;
/// * planar 4:2:0 (`YU12`) has one sample per two pixels in each of two separate
///   planes: *half* the luma pitch.
///
/// Returning the divisor rather than the pitch keeps the caller's rounding in
/// one place.
const fn chroma_stride_divisor(pixel: PixelFormat) -> Option<u32> {
    match pixel {
        PixelFormat::Nv12 | PixelFormat::Nv21 => Some(1),
        PixelFormat::Yuv420p => Some(2),
        _ => None,
    }
}

/// Where every plane of one frame sits inside a driver buffer.
///
/// `bytesperline` is the driver's echoed luma pitch. `None` for a multi-plane
/// layout whose chroma pitch this backend has no rule for — see
/// [`chroma_stride_divisor`] — for a zero-sized frame, or for a driver pitch
/// narrower than the frame itself, which would mean the driver and this code
/// disagree about what it just promised to deliver.
///
/// This is geometry, and only geometry: it answers "given that pitch, where
/// does each plane sit", for any layout whose row pitches are known. It is
/// deliberately not a second copy of the list of formats this backend speaks.
/// A `PixelFormat` V4L2 has no code for — [`PixelFormat::Gray16`], say — is
/// refused by [`pixelformat_for_encoding`] before a device is ever configured,
/// which names the format in the error; reaching here and reporting it as a row
/// pitch that "cannot hold the frame" would describe the wrong problem.
fn plane_copies(
    pixel: PixelFormat,
    width: u32,
    height: u32,
    bytesperline: u32,
) -> Option<Vec<PlaneCopy>> {
    if width == 0 || height == 0 {
        return None;
    }
    // `plane_dimensions` needs a frame to read the chroma ratios from; `new`
    // allocates nothing.
    let geometry = VideoFrame::new(pixel, width, height);
    let luma_stride = pixel.stride_for_width(width, 0)?;
    let luma_source = bytesperline as usize;
    if luma_source < luma_stride {
        return None;
    }

    let plane_count = pixel.plane_count();
    let mut copies: Vec<PlaneCopy> = Vec::with_capacity(plane_count as usize);
    let mut offset = 0_usize;
    for plane in 0..plane_count {
        let stride = pixel.stride_for_width(width, plane)?;
        let (plane_width, plane_height) = geometry.plane_dimensions(plane as usize);
        let source_stride = if plane == 0 {
            luma_source
        } else {
            let divisor = chroma_stride_divisor(pixel)? as usize;
            let chroma_source = luma_source / divisor;
            if chroma_source < stride {
                return None;
            }
            chroma_source
        };
        let rows = plane_height as usize;
        copies.push(PlaneCopy {
            offset,
            source_stride,
            stride,
            rows,
            width: plane_width,
            height: plane_height,
        });
        // The next plane starts after this one's *padded* rows: the driver lays
        // planes out back to back at its own pitch, not at ours.
        offset = offset.checked_add(source_stride.checked_mul(rows)?)?;
    }
    Some(copies)
}

/// Bytes that must be readable for a whole frame in this layout.
fn required_bytes(copies: &[PlaneCopy]) -> Option<usize> {
    let mut required = 0_usize;
    for copy in copies {
        required = required.max(copy.required_bytes()?);
    }
    Some(required)
}

// ── Driver buffer mappings ───────────────────────────────────────────────────

/// One `mmap`ed driver buffer.
#[derive(Debug, Clone, Copy)]
struct Mapping {
    /// Base address returned by `mmap`.
    address: *mut c_void,
    /// Mapped length, from `v4l2_buffer::length`.
    length: usize,
}

/// Every mapping of one capture session, unmapped together on the way out.
///
/// This is a guard, not a collection: the only reason it exists is that
/// [`Drop`] must run on every path out of [`run_capture`], including the early
/// `?` returns between `mmap` and `STREAMON`, and including a panic. A leaked
/// mapping keeps the driver's DMA buffers pinned for the life of the process.
struct Mappings {
    /// One entry per driver buffer, indexed by `v4l2_buffer::index`.
    mappings: Vec<Mapping>,
    /// Device node, for the log line if `munmap` ever fails.
    node: String,
}

impl Mappings {
    /// `QUERYBUF` + `mmap` every buffer the driver granted.
    fn acquire(fd: BorrowedFd<'_>, node: &str, count: u32) -> Result<Self, CaptureError> {
        let mut mappings = Self {
            mappings: Vec::with_capacity(count as usize),
            node: node.to_owned(),
        };
        for index in 0..count {
            let buffer = ioctl::querybuf(fd, index)
                .map_err(|errno| ioctl::ioctl_error(node, "VIDIOC_QUERYBUF", errno))?;
            let length = buffer.length as usize;
            if length == 0 {
                return Err(CaptureError::platform(format!(
                    "driver reported a zero-length buffer {index} for {node:?}"
                )));
            }
            // SAFETY: `mmap` with a null hint asks the kernel to choose the
            // address, which has no aliasing precondition of its own. The
            // descriptor is a live V4L2 capture node, and `offset`/`length` are
            // the pair the kernel just reported for buffer `index` through
            // `VIDIOC_QUERYBUF` — the only values it accepts here. The mapping
            // is owned by `self` from this point and is unmapped exactly once,
            // in `Drop`.
            let address = unsafe {
                rustix::mm::mmap(
                    core::ptr::null_mut(),
                    length,
                    ProtFlags::READ | ProtFlags::WRITE,
                    MapFlags::SHARED,
                    fd,
                    u64::from(buffer.mmap_offset()),
                )
            }
            .map_err(|errno| {
                CaptureError::platform(format!(
                    "mmap of buffer {index} for {node:?} failed: {}",
                    std::io::Error::from_raw_os_error(errno.raw_os_error())
                ))
            })?;
            mappings.mappings.push(Mapping { address, length });
        }
        Ok(mappings)
    }

    /// The shortest mapping, which is the bound every frame must fit inside.
    fn smallest_length(&self) -> usize {
        self.mappings
            .iter()
            .map(|mapping| mapping.length)
            .min()
            .unwrap_or(0)
    }

    /// How many buffers are mapped.
    fn len(&self) -> usize {
        self.mappings.len()
    }

    /// Borrow the bytes of one dequeued buffer.
    ///
    /// # Safety
    ///
    /// The buffer must be *dequeued* — owned by userspace between `VIDIOC_DQBUF`
    /// and the matching `VIDIOC_QBUF`. Inside that window the kernel is
    /// documented not to touch it, which is what makes a shared reference to the
    /// mapping sound; outside it, the driver may be writing and the reference
    /// would alias a concurrent write.
    unsafe fn dequeued(&self, index: u32) -> Option<&[u8]> {
        let mapping = self.mappings.get(index as usize)?;
        // SAFETY: `address`/`length` come from a successful `mmap` that `Drop`
        // has not run yet, so the whole range is mapped, readable and
        // initialised (the kernel zeroes fresh buffer memory). The caller
        // guarantees the buffer is dequeued, so nothing else writes to it while
        // the returned slice lives, and the slice cannot outlive `self`.
        Some(unsafe { core::slice::from_raw_parts(mapping.address.cast::<u8>(), mapping.length) })
    }
}

impl Drop for Mappings {
    fn drop(&mut self) {
        for mapping in &self.mappings {
            // SAFETY: each `address`/`length` pair is exactly what `mmap`
            // returned and was passed, this is the only `munmap` of it — `Drop`
            // runs once — and no slice handed out by `dequeued` can still be
            // alive, because every one of them borrows `self`.
            if let Err(errno) = unsafe { rustix::mm::munmap(mapping.address, mapping.length) } {
                tracing::warn!(
                    device = %self.node,
                    error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    "munmap of a capture buffer failed"
                );
            }
        }
    }
}

// ── Streaming guard ──────────────────────────────────────────────────────────

/// Holds `VIDIOC_STREAMOFF` so it runs before the mappings are torn down.
///
/// The order matters: `STREAMOFF` is what stops the driver writing into the
/// mapped buffers *and* un-queues every one of them. Unmapping first would leave
/// a live DMA target pointing at memory this process no longer owns.
///
/// Rust drops locals in reverse declaration order, so [`run_capture`] declares
/// the descriptor first, the mappings second and this guard last, and the
/// resulting teardown is `STREAMOFF` → `munmap` → `close` on every path,
/// including the `?` returns and a panic.
struct StreamGuard<'fd> {
    /// The capture node.
    fd: BorrowedFd<'fd>,
    /// Device node, for the log line if `STREAMOFF` fails.
    node: &'fd str,
    /// Whether `STREAMON` succeeded and therefore needs undoing.
    streaming: bool,
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        if !self.streaming {
            return;
        }
        if let Err(errno) = ioctl::streamoff(self.fd) {
            tracing::warn!(
                device = %self.node,
                error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                "VIDIOC_STREAMOFF failed"
            );
        }
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

/// What the driver actually agreed to.
#[derive(Debug, Clone, Copy)]
struct Configured {
    /// The mode frames will be labelled with. Identical to the negotiated one
    /// except, possibly, for the frame rate the driver rounded to.
    format: CaptureFormat,
    /// The driver's echoed luma row pitch.
    bytesperline: u32,
    /// The driver's echoed image size, used only for the sanity check below.
    sizeimage: u32,
}

/// `VIDIOC_S_FMT` the device into the negotiated mode, and check its echo.
fn configure_format(
    fd: BorrowedFd<'_>,
    node: &str,
    wanted: CaptureFormat,
) -> Result<Configured, CaptureError> {
    let Some(code) = pixelformat_for_encoding(wanted.encoding) else {
        return Err(CaptureError::format_rejected(
            node,
            wanted,
            "no V4L2 pixel format corresponds to this encoding",
        ));
    };

    let request = v4l2_pix_format {
        width: wanted.width,
        height: wanted.height,
        pixelformat: code,
        // Progressive frames. `V4L2_FIELD_ANY` would let an interlaced device
        // hand back fields, which this crate has no model for.
        field: V4L2_FIELD_NONE,
        ..v4l2_pix_format::default()
    };
    let echoed = ioctl::s_fmt(fd, v4l2_format::capture(request))
        .map_err(|errno| ioctl::ioctl_error(node, "VIDIOC_S_FMT", errno))?
        .pix();

    if echoed.pixelformat != code {
        return Err(CaptureError::format_rejected(
            node,
            wanted,
            format!(
                "driver substituted {} for {}",
                fourcc_name(echoed.pixelformat),
                fourcc_name(code)
            ),
        ));
    }
    if echoed.width != wanted.width || echoed.height != wanted.height {
        return Err(CaptureError::format_rejected(
            node,
            wanted,
            format!(
                "driver substituted {}x{} for {}x{}",
                echoed.width, echoed.height, wanted.width, wanted.height
            ),
        ));
    }
    if echoed.sizeimage == 0 {
        return Err(CaptureError::format_rejected(
            node,
            wanted,
            "driver reported a zero image size",
        ));
    }

    Ok(Configured {
        format: wanted,
        bytesperline: echoed.bytesperline,
        sizeimage: echoed.sizeimage,
    })
}

/// `VIDIOC_S_PARM` the device to the negotiated frame rate, and take its echo.
///
/// Returns the rate the driver granted. A device that does not support
/// `V4L2_CAP_TIMEPERFRAME`, or a negotiated format whose rate is the documented
/// "unknown / variable" `fps_den == 0`, is left alone — there is nothing to ask
/// for and nothing to check.
fn configure_rate(fd: BorrowedFd<'_>, node: &str, wanted: CaptureFormat) -> (u32, u32) {
    if wanted.fps_num == 0 || wanted.fps_den == 0 {
        tracing::debug!(
            device = %node,
            "negotiated mode has no frame rate; leaving the device's default in place"
        );
        return (wanted.fps_num, wanted.fps_den);
    }

    match ioctl::g_parm(fd) {
        Ok(current) if current.capture().capability & V4L2_CAP_TIMEPERFRAME == 0 => {
            tracing::debug!(
                device = %node,
                "device does not support setting the frame interval; leaving its default"
            );
            return (wanted.fps_num, wanted.fps_den);
        }
        Ok(_) => {}
        Err(errno) => {
            tracing::debug!(
                device = %node,
                error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                "VIDIOC_G_PARM failed; requesting the frame interval anyway"
            );
        }
    }

    // A frame *rate* of `fps_num / fps_den` is an *interval* of
    // `fps_den / fps_num` seconds; see `super::fps_from_interval`.
    let mut request = super::uapi::v4l2_streamparm::query();
    request.set_capture(v4l2_captureparm {
        timeperframe: v4l2_fract {
            numerator: wanted.fps_den,
            denominator: wanted.fps_num,
        },
        ..v4l2_captureparm::default()
    });

    let echoed = match ioctl::s_parm(fd, request) {
        Ok(echoed) => echoed.capture().timeperframe,
        Err(errno) => {
            tracing::debug!(
                device = %node,
                error = %std::io::Error::from_raw_os_error(errno.raw_os_error()),
                "VIDIOC_S_PARM failed; the device keeps its own frame interval"
            );
            return (wanted.fps_num, wanted.fps_den);
        }
    };

    let Some(granted) = super::fps_from_interval(echoed.numerator, echoed.denominator) else {
        tracing::debug!(
            device = %node,
            numerator = echoed.numerator,
            denominator = echoed.denominator,
            "device echoed an unusable frame interval; recording the rate as unknown"
        );
        return (0, 0);
    };
    if granted != (wanted.fps_num, wanted.fps_den) {
        // Not an error: rounding is `VIDIOC_S_PARM`'s specified behaviour. See
        // the module docs for why this diverges from the macOS backend.
        tracing::info!(
            device = %node,
            requested = format!("{}/{}", wanted.fps_num, wanted.fps_den),
            granted = format!("{}/{}", granted.0, granted.1),
            "device rounded the frame rate; delivered frames carry the granted rate"
        );
    }
    granted
}

// ── Readiness ────────────────────────────────────────────────────────────────

/// Outcome of one bounded wait for a filled buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readiness {
    /// A buffer is ready to dequeue.
    Ready,
    /// Nothing happened before the timeout; the caller re-checks the stop flag.
    Waiting,
    /// The device went away, or reported an error condition on the descriptor.
    Lost(&'static str),
}

/// Classify the `revents` word `poll` filled in.
///
/// `POLLERR` is how a V4L2 driver reports a streaming error, and `POLLHUP` /
/// `POLLNVAL` are how a node that was unplugged reports itself. Looping on
/// `POLLIN` while one of those is set would spin at full speed forever, so each
/// ends the session with an error — the honest outcome, since the device really
/// did stop.
const fn classify(revents: PollFlags) -> Readiness {
    if revents.contains(PollFlags::NVAL) {
        Readiness::Lost("the device node was closed underneath the capture thread")
    } else if revents.contains(PollFlags::ERR) {
        Readiness::Lost("the driver reported a streaming error")
    } else if revents.contains(PollFlags::HUP) {
        Readiness::Lost("the device was disconnected")
    } else if revents.contains(PollFlags::IN) {
        Readiness::Ready
    } else {
        Readiness::Waiting
    }
}

/// Wait up to [`POLL_TIMEOUT`] for a filled buffer.
fn wait_for_frame(fd: BorrowedFd<'_>, node: &str) -> Result<Readiness, CaptureError> {
    let timeout = Timespec {
        tv_sec: POLL_TIMEOUT.as_secs() as _,
        tv_nsec: POLL_TIMEOUT.subsec_nanos() as _,
    };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        // A signal is not a failure; the caller re-checks the stop flag and
        // waits again.
        Err(Errno::INTR) => Ok(Readiness::Waiting),
        Err(errno) => Err(ioctl::ioctl_error(node, "poll", errno)),
        Ok(0) => Ok(Readiness::Waiting),
        Ok(_) => Ok(classify(fds[0].revents())),
    }
}

// ── Copying ──────────────────────────────────────────────────────────────────

/// Copy one dequeued buffer into owned planes, honouring both row pitches.
///
/// `bytes` is the whole mapping; `filled` is `v4l2_buffer::bytesused`, the part
/// the driver says it wrote.
fn copy_planes(
    bytes: &[u8],
    filled: usize,
    pixel: PixelFormat,
    copies: &[PlaneCopy],
) -> Result<VideoFrame, String> {
    let Some(required) = required_bytes(copies) else {
        return Err("frame geometry does not fit in memory".to_owned());
    };
    if filled < required {
        return Err(format!(
            "driver filled {filled} byte(s), a complete frame needs {required}"
        ));
    }
    // The mapping length was checked once at setup, but a driver is free to
    // report a `bytesused` larger than the buffer it gave us, and that value is
    // what the bound above was compared against. Re-check against the memory
    // that actually exists before any of it is read.
    if bytes.len() < required {
        return Err(format!(
            "buffer is {} byte(s), a complete frame needs {required}",
            bytes.len()
        ));
    }

    let mut planes = Vec::with_capacity(copies.len());
    for copy in copies {
        let mut data = Vec::with_capacity(copy.stride.saturating_mul(copy.rows));
        for row in 0..copy.rows {
            // Every index here is inside `required`, which `bytes.len()` was
            // just checked against, so the slice cannot be out of range.
            let start = copy.offset + row * copy.source_stride;
            data.extend_from_slice(&bytes[start..start + copy.stride]);
        }
        planes.push(Plane::with_dimensions(
            data,
            copy.stride,
            copy.width,
            copy.height,
        ));
    }

    let mut frame = VideoFrame::new(pixel, copies[0].width, copies[0].height);
    frame.planes = planes;
    Ok(frame)
}

/// The capture instant a dequeued buffer carries, if it carries one.
///
/// Only `V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC` is a device clock. `UNKNOWN` and
/// `COPY` are not: the first says the driver did not sample one and the second
/// says the value was copied from an output buffer, so neither is an exposure
/// time. Returning `None` for them is what makes the caller fall back to
/// [`TimestampSource::HostArrival`] instead of inventing a device timeline.
///
/// A negative `tv_sec` — which the kernel never produces on `CLOCK_MONOTONIC` —
/// also yields `None` rather than a saturated zero, because a zero would look
/// like a real instant at the start of the timeline.
fn device_timestamp(buffer: &v4l2_buffer) -> Option<Duration> {
    if !buffer.has_monotonic_timestamp() {
        return None;
    }
    let seconds = u64::try_from(buffer.timestamp.tv_sec).ok()?;
    let micros = u64::try_from(buffer.timestamp.tv_usec).ok()?;
    Some(Duration::from_secs(seconds) + Duration::from_micros(micros))
}

// ── Capture ──────────────────────────────────────────────────────────────────

/// Open the node, stream from it, and stop when told to.
///
/// The descriptor, the mappings and the streaming state are all created here and
/// torn down before this function returns — see the [module docs](super) on why
/// none of it can happen in `open()`.
pub(crate) fn run_capture(
    runner: &V4l2Runner,
    sink: &FrameSink,
    stop: &StopSignal,
) -> Result<(), CaptureError> {
    let node = runner.node().to_owned();
    let wanted = runner.format();

    // Three bindings below carry the teardown, and Rust drops locals in reverse
    // declaration order, so the order they are *declared* in is the order the
    // teardown runs in, reversed:
    //
    //   1. `fd`       — declared first,  dropped last  → close
    //   2. `mappings` — declared second, dropped second → munmap
    //   3. `guard`    — declared last,   dropped first  → VIDIOC_STREAMOFF
    //
    // which is the only correct sequence: stopping the stream is what makes the
    // driver let go of the mapped memory, and closing the descriptor is what
    // frees the ring. Moving any of the three past another silently breaks it,
    // on the error paths first.
    let fd = open_node(Path::new(&node)).map_err(|errno| {
        if errno == Errno::ACCESS || errno == Errno::PERM {
            CaptureError::PermissionDenied {
                device: node.clone(),
            }
        } else {
            CaptureError::io(
                node.clone(),
                std::io::Error::from_raw_os_error(errno.raw_os_error()),
            )
        }
    })?;

    let configured = configure_format(fd.as_fd(), &node, wanted)?;
    let (fps_num, fps_den) = configure_rate(fd.as_fd(), &node, wanted);
    let delivered_format = CaptureFormat {
        fps_num,
        fps_den,
        ..configured.format
    };

    // Plane geometry, computed once from the driver's own echoed pitch.
    let copies = match wanted.encoding {
        CaptureEncoding::Mjpeg => None,
        CaptureEncoding::Raw(pixel) => Some(
            plane_copies(pixel, wanted.width, wanted.height, configured.bytesperline).ok_or_else(
                || {
                    CaptureError::format_rejected(
                        &node,
                        wanted,
                        format!(
                            "driver's row pitch of {} byte(s) cannot hold a {}x{} {pixel:?} frame",
                            configured.bytesperline, wanted.width, wanted.height
                        ),
                    )
                },
            )?,
        ),
    };

    let granted = ioctl::reqbufs(fd.as_fd(), runner.buffer_count())
        .map_err(|errno| ioctl::ioctl_error(&node, "VIDIOC_REQBUFS", errno))?;
    if granted.count == 0 {
        return Err(CaptureError::format_rejected(
            &node,
            wanted,
            "driver granted no capture buffers",
        ));
    }
    if granted.count != runner.buffer_count() {
        tracing::debug!(
            device = %node,
            requested = runner.buffer_count(),
            granted = granted.count,
            "driver granted a different number of capture buffers"
        );
    }

    let mappings = Mappings::acquire(fd.as_fd(), &node, granted.count)?;

    // A frame must fit in the smallest buffer the driver handed over. Checking
    // once here, against real mapped memory, is what makes the per-frame copy's
    // bounds a formality rather than the only thing standing between a lying
    // driver and a wild read.
    if let Some(copies) = copies.as_deref() {
        let required = required_bytes(copies).unwrap_or(usize::MAX);
        if required > mappings.smallest_length() {
            return Err(CaptureError::format_rejected(
                &node,
                wanted,
                format!(
                    "a complete frame needs {required} byte(s) but the driver's buffers are {}",
                    mappings.smallest_length()
                ),
            ));
        }
        if (configured.sizeimage as usize) < required {
            tracing::debug!(
                device = %node,
                sizeimage = configured.sizeimage,
                required,
                "driver's reported image size is smaller than its own geometry implies"
            );
        }
    }

    for index in 0..granted.count {
        ioctl::qbuf(fd.as_fd(), index)
            .map_err(|errno| ioctl::ioctl_error(&node, "VIDIOC_QBUF", errno))?;
    }

    let mut guard = StreamGuard {
        fd: fd.as_fd(),
        node: &node,
        streaming: false,
    };
    ioctl::streamon(fd.as_fd())
        .map_err(|errno| ioctl::ioctl_error(&node, "VIDIOC_STREAMON", errno))?;
    guard.streaming = true;

    tracing::info!(
        device = %node,
        name = %runner.name(),
        negotiated = %delivered_format,
        buffers = mappings.len(),
        bytesperline = configured.bytesperline,
        "V4L2 capture running"
    );

    pump(&Session {
        fd: fd.as_fd(),
        node: &node,
        mappings: &mappings,
        copies: copies.as_deref(),
        format: delivered_format,
        sink,
        stop,
    })
}

/// Everything the frame loop needs, gathered so the loop takes one argument.
struct Session<'a> {
    /// The capture node.
    fd: BorrowedFd<'a>,
    /// Device node path, for logs and errors.
    node: &'a str,
    /// The mapped driver buffers.
    mappings: &'a Mappings,
    /// Plane geometry, or `None` for a compressed stream.
    copies: Option<&'a [PlaneCopy]>,
    /// The mode delivered frames are labelled with.
    format: CaptureFormat,
    /// Where frames go.
    sink: &'a FrameSink,
    /// Shutdown flag.
    stop: &'a StopSignal,
}

/// Dequeue, copy, publish and re-queue until the stop flag is raised.
fn pump(session: &Session<'_>) -> Result<(), CaptureError> {
    let epoch = Instant::now();
    let mut warned_about_the_clock = false;

    while !session.stop.is_stopped() {
        match wait_for_frame(session.fd, session.node)? {
            Readiness::Waiting => continue,
            Readiness::Lost(reason) => {
                return Err(CaptureError::platform(format!(
                    "capture on {:?} ended: {reason}",
                    session.node
                )));
            }
            Readiness::Ready => {}
        }

        let buffer = match ioctl::dqbuf(session.fd) {
            Ok(buffer) => buffer,
            // `poll` said ready and the buffer was taken by something else, or a
            // signal arrived: wait again rather than end the session.
            Err(errno) if errno == Errno::AGAIN || errno == Errno::INTR => continue,
            Err(errno) => {
                return Err(ioctl::ioctl_error(session.node, "VIDIOC_DQBUF", errno));
            }
        };

        // The buffer is now owned by userspace. Everything below must reach the
        // re-queue, so the outcome is computed first and re-queuing happens
        // unconditionally afterwards.
        let host_arrival = epoch.elapsed();
        let outcome = build_frame(session, &buffer, host_arrival, &mut warned_about_the_clock);
        ioctl::qbuf(session.fd, buffer.index)
            .map_err(|errno| ioctl::ioctl_error(session.node, "VIDIOC_QBUF", errno))?;

        match outcome {
            Ok(Some(frame)) => {
                session.sink.deliver(frame);
            }
            // A frame the driver marked bad, or one whose bytes did not add up.
            // The buffer is already back in the ring and the gap it leaves shows
            // up in the device's own sequence counter, which `FrameSink` counts.
            Ok(None) => {}
            Err(error) => session.sink.report_transient(&error),
        }
    }
    Ok(())
}

/// Turn one dequeued buffer into a deliverable frame.
///
/// `Ok(None)` means the buffer carried no usable frame and was skipped;
/// `Err` means the same but with something worth counting as a transient error.
fn build_frame(
    session: &Session<'_>,
    buffer: &v4l2_buffer,
    host_arrival: Duration,
    warned_about_the_clock: &mut bool,
) -> Result<Option<CaptureFrame>, CaptureError> {
    if buffer.is_error() {
        // `V4L2_BUF_FLAG_ERROR` means the driver filled this buffer with a
        // damaged frame. It must still be re-queued — the caller does that —
        // but publishing the contents would be publishing garbage.
        tracing::debug!(
            device = %session.node,
            sequence = buffer.sequence,
            "driver flagged a damaged frame; skipping it"
        );
        return Ok(None);
    }

    // SAFETY: `buffer` came out of `VIDIOC_DQBUF` and the caller re-queues it
    // only after this function returns, so for the whole of this borrow the
    // buffer is owned by userspace and the kernel does not write to it.
    let Some(bytes) = (unsafe { session.mappings.dequeued(buffer.index) }) else {
        return Err(CaptureError::platform(format!(
            "driver dequeued buffer {} on {:?}, which was never mapped",
            buffer.index, session.node
        )));
    };

    let filled = buffer.bytesused as usize;
    let payload = match session.copies {
        // Compressed: the bitstream is exactly the bytes the driver filled.
        None => {
            if filled == 0 || filled > bytes.len() {
                return Err(CaptureError::format_rejected(
                    session.node,
                    session.format,
                    format!(
                        "driver reported {filled} byte(s) in a {}-byte buffer",
                        bytes.len()
                    ),
                ));
            }
            FramePayload::Compressed(bytes[..filled].to_vec())
        }
        Some(copies) => {
            let Some(pixel) = session.format.pixel() else {
                return Err(CaptureError::format_rejected(
                    session.node,
                    session.format,
                    "raw plane geometry for a compressed encoding",
                ));
            };
            match copy_planes(bytes, filled, pixel, copies) {
                Ok(video) => FramePayload::Raw(video),
                Err(reason) => {
                    return Err(CaptureError::format_rejected(
                        session.node,
                        session.format,
                        reason,
                    ))
                }
            }
        }
    };

    let (timestamp, timestamp_source) = match device_timestamp(buffer) {
        Some(device) => (device, TimestampSource::DeviceMonotonic),
        None => {
            if !*warned_about_the_clock {
                *warned_about_the_clock = true;
                tracing::warn!(
                    device = %session.node,
                    flags = format!("{:#010x}", buffer.flags),
                    "driver reports no monotonic capture clock; timestamps are host arrival times"
                );
            }
            (host_arrival, TimestampSource::HostArrival)
        }
    };

    Ok(Some(CaptureFrame {
        format: session.format,
        payload,
        timestamp,
        host_timestamp: host_arrival,
        timestamp_source,
        // The driver's own counter, not one of ours: a gap in it means frames
        // were lost upstream of this crate, which is exactly what
        // `FrameSink::deliver` attributes to `CaptureStats::device_dropped`.
        sequence: u64::from(buffer.sequence),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::CaptureEncoding;
    use crate::platform::linux::uapi::{
        V4L2_BUF_FLAG_ERROR, V4L2_BUF_FLAG_TIMESTAMP_COPY, V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC,
        V4L2_BUF_FLAG_TIMESTAMP_UNKNOWN,
    };

    /// Marker written into every padding byte of a synthetic driver buffer, so
    /// a copy that ignored the row pitch is detectable by inspection.
    const PADDING: u8 = 0xEE;

    /// Byte a test expects at `(plane, row, column)` of the payload.
    fn sample(plane: usize, row: usize, column: usize) -> u8 {
        u8::try_from((plane * 97 + row * 7 + column * 3) % 233).unwrap_or(0)
    }

    /// Build a driver-shaped buffer: padded rows, payload written at the source
    /// pitch, everything else [`PADDING`].
    fn driver_buffer(copies: &[PlaneCopy], slack: usize) -> Vec<u8> {
        let end = required_bytes(copies).expect("a real geometry");
        let mut bytes = vec![PADDING; end + slack];
        for (index, copy) in copies.iter().enumerate() {
            for row in 0..copy.rows {
                for column in 0..copy.stride {
                    bytes[copy.offset + row * copy.source_stride + column] =
                        sample(index, row, column);
                }
            }
        }
        bytes
    }

    // ── Plane geometry ──────────────────────────────────────────────────────

    #[test]
    fn packed_formats_are_one_plane_of_two_or_three_bytes_per_pixel() {
        for (pixel, bytes_per_pixel) in [
            (PixelFormat::Yuyv422, 2),
            (PixelFormat::Uyvy422, 2),
            (PixelFormat::Rgb24, 3),
        ] {
            let copies = plane_copies(pixel, 640, 480, 640 * bytes_per_pixel)
                .unwrap_or_else(|| panic!("{pixel:?} is supported"));
            assert_eq!(copies.len(), 1, "{pixel:?}");
            assert_eq!(
                copies[0].stride,
                640 * bytes_per_pixel as usize,
                "{pixel:?}"
            );
            assert_eq!(copies[0].rows, 480, "{pixel:?}");
            assert_eq!(copies[0].offset, 0, "{pixel:?}");
            assert_eq!(
                copies[0].stride * copies[0].rows,
                pixel.frame_buffer_size(640, 480),
                "{pixel:?} total must match the workspace convention"
            );
        }
    }

    #[test]
    fn semi_planar_chroma_shares_the_luma_row_pitch() {
        for pixel in [PixelFormat::Nv12, PixelFormat::Nv21] {
            // A driver that pads 1920 luma bytes out to 2048.
            let copies = plane_copies(pixel, 1920, 1080, 2048)
                .unwrap_or_else(|| panic!("{pixel:?} is supported"));
            assert_eq!(copies.len(), 2, "{pixel:?}");
            assert_eq!(copies[0].source_stride, 2048, "{pixel:?} luma pitch");
            assert_eq!(copies[0].stride, 1920, "{pixel:?} luma stride");
            assert_eq!(copies[0].rows, 1080, "{pixel:?}");
            // Interleaved Cb/Cr: 960 pairs of two bytes is 1920 bytes per row,
            // at the same driver pitch as luma.
            assert_eq!(copies[1].source_stride, 2048, "{pixel:?} chroma pitch");
            assert_eq!(copies[1].stride, 1920, "{pixel:?} chroma stride");
            assert_eq!(copies[1].rows, 540, "{pixel:?}");
            assert_eq!(copies[1].offset, 2048 * 1080, "{pixel:?} chroma offset");
            let total: usize = copies.iter().map(|copy| copy.stride * copy.rows).sum();
            assert_eq!(total, pixel.frame_buffer_size(1920, 1080), "{pixel:?}");
        }
    }

    #[test]
    fn planar_chroma_takes_half_the_luma_row_pitch() {
        let copies = plane_copies(PixelFormat::Yuv420p, 640, 480, 704).expect("yuv420p");
        assert_eq!(copies.len(), 3);
        assert_eq!((copies[0].source_stride, copies[0].stride), (704, 640));
        assert_eq!((copies[1].source_stride, copies[1].stride), (352, 320));
        assert_eq!((copies[2].source_stride, copies[2].stride), (352, 320));
        assert_eq!(copies[1].rows, 240);
        assert_eq!(copies[0].offset, 0);
        assert_eq!(copies[1].offset, 704 * 480);
        assert_eq!(copies[2].offset, 704 * 480 + 352 * 240);
        let total: usize = copies.iter().map(|copy| copy.stride * copy.rows).sum();
        assert_eq!(total, PixelFormat::Yuv420p.frame_buffer_size(640, 480));
    }

    #[test]
    fn an_unpadded_buffer_has_equal_source_and_destination_pitches() {
        let copies = plane_copies(PixelFormat::Nv12, 1280, 720, 1280).expect("nv12");
        for copy in &copies {
            assert_eq!(copy.source_stride, copy.stride);
        }
        assert_eq!(
            required_bytes(&copies),
            Some(PixelFormat::Nv12.frame_buffer_size(1280, 720))
        );
    }

    #[test]
    fn a_row_pitch_narrower_than_the_frame_is_refused_not_clamped() {
        // A driver claiming 1280-wide YUYV in 640 bytes per row is describing
        // something impossible; reading it as 2560 would walk off the buffer.
        assert!(plane_copies(PixelFormat::Yuyv422, 1280, 720, 640).is_none());
        assert!(plane_copies(PixelFormat::Nv12, 1280, 720, 1279).is_none());
    }

    #[test]
    fn a_layout_this_backend_has_no_copy_path_for_has_no_geometry() {
        // Multi-plane layouts whose chroma pitch `chroma_stride_divisor` has no
        // rule for. Guessing one would place the chroma planes at the wrong
        // offsets and deliver a frame that is the right size and wrong.
        for pixel in [
            PixelFormat::P010,
            PixelFormat::P016,
            PixelFormat::Yuv422p,
            PixelFormat::Yuv444p,
            PixelFormat::Yuv420p10le,
        ] {
            assert!(
                plane_copies(pixel, 640, 480, 1280).is_none(),
                "{pixel:?} has no chroma pitch rule and must not get a geometry"
            );
        }
    }

    /// A format V4L2 has no code for is refused where that fact lives — in the
    /// fourcc table, before a device is configured — not by [`plane_copies`],
    /// whose arithmetic is perfectly well defined for a single grey plane. The
    /// two rejections are different answers to different questions, and keeping
    /// them apart is what makes the error name the format instead of blaming
    /// the driver's row pitch.
    #[test]
    fn a_format_v4l2_has_no_code_for_is_refused_before_any_geometry_is_needed() {
        for pixel in [PixelFormat::Gray8, PixelFormat::Gray16] {
            assert!(
                pixelformat_for_encoding(CaptureEncoding::Raw(pixel)).is_none(),
                "{pixel:?} is not a V4L2 code this backend asks for"
            );
            let copies = plane_copies(pixel, 640, 480, 640 * 2).expect("single-plane geometry");
            assert_eq!(copies.len(), 1, "{pixel:?}");
            assert_eq!(
                copies[0].stride,
                pixel.stride_for_width(640, 0).expect("a known row pitch"),
                "{pixel:?}"
            );
            assert_eq!(copies[0].rows, 480, "{pixel:?}");
        }
    }

    #[test]
    fn a_zero_sized_frame_has_no_geometry() {
        assert!(plane_copies(PixelFormat::Nv12, 0, 480, 0).is_none());
        assert!(plane_copies(PixelFormat::Nv12, 640, 0, 640).is_none());
    }

    #[test]
    fn odd_heights_round_the_chroma_plane_up() {
        let copies = plane_copies(PixelFormat::Nv12, 640, 481, 640).expect("nv12");
        assert_eq!(copies[1].rows, 241, "a half row still has to be carried");
    }

    #[test]
    fn the_last_row_needs_only_its_meaningful_bytes() {
        // 4 rows of 64 bytes at a pitch of 96: the padding after the *last* row
        // is never read, so requiring it would reject a driver whose sizeimage
        // stops at the last pixel.
        let copies = plane_copies(PixelFormat::Gray8, 64, 4, 96).expect("one grey plane");
        assert_eq!(
            copies,
            vec![PlaneCopy {
                offset: 0,
                source_stride: 96,
                stride: 64,
                rows: 4,
                width: 64,
                height: 4,
            }]
        );
        assert_eq!(copies[0].required_bytes(), Some(96 * 3 + 64));
    }

    #[test]
    fn an_impossible_geometry_reports_no_requirement_rather_than_overflowing() {
        let copy = PlaneCopy {
            offset: 0,
            source_stride: usize::MAX,
            stride: 8,
            rows: 4,
            width: 8,
            height: 4,
        };
        assert_eq!(copy.required_bytes(), None);
        assert_eq!(required_bytes(&[copy]), None);
    }

    // ── Copying ─────────────────────────────────────────────────────────────

    /// Copy a synthetic driver buffer and check it byte for byte.
    fn round_trip(pixel: PixelFormat, width: u32, height: u32, bytesperline: u32) {
        let copies = plane_copies(pixel, width, height, bytesperline)
            .unwrap_or_else(|| panic!("{pixel:?} {width}x{height} is supported"));
        let bytes = driver_buffer(&copies, 64);
        let filled = required_bytes(&copies).expect("a real geometry");

        let video = copy_planes(&bytes, filled, pixel, &copies)
            .unwrap_or_else(|error| panic!("copy failed for {pixel:?}: {error}"));

        assert_eq!(video.format, pixel);
        assert_eq!(video.width, width);
        assert_eq!(video.height, height);
        assert_eq!(video.planes.len(), copies.len(), "{pixel:?} plane count");
        assert_eq!(
            video.size_bytes(),
            pixel.frame_buffer_size(width, height),
            "{pixel:?} total size must match the workspace convention"
        );

        for (index, (plane, copy)) in video.planes.iter().zip(&copies).enumerate() {
            assert_eq!(plane.stride, copy.stride, "plane {index} stride");
            assert_eq!(plane.width, copy.width, "plane {index} width");
            assert_eq!(plane.height, copy.height, "plane {index} height");
            assert_eq!(
                plane.data.len(),
                copy.stride * copy.rows,
                "plane {index} size"
            );
            assert!(
                !plane.data.contains(&PADDING),
                "plane {index} leaked the driver's row padding"
            );
            for row in 0..copy.rows {
                for column in 0..copy.stride {
                    assert_eq!(
                        plane.data[row * copy.stride + column],
                        sample(index, row, column),
                        "plane {index} row {row} column {column}"
                    );
                }
            }
        }
    }

    #[test]
    fn padded_buffers_copy_without_their_padding() {
        round_trip(PixelFormat::Yuyv422, 640, 480, 1408);
        round_trip(PixelFormat::Nv12, 1920, 1080, 2048);
        round_trip(PixelFormat::Nv21, 320, 240, 384);
        round_trip(PixelFormat::Yuv420p, 640, 480, 704);
        round_trip(PixelFormat::Rgb24, 176, 144, 544);
    }

    #[test]
    fn unpadded_buffers_copy_verbatim() {
        round_trip(PixelFormat::Yuyv422, 1280, 720, 2560);
        round_trip(PixelFormat::Nv12, 1280, 720, 1280);
        round_trip(PixelFormat::Yuv420p, 352, 288, 352);
        round_trip(PixelFormat::Uyvy422, 64, 32, 128);
    }

    #[test]
    fn a_short_frame_is_rejected_rather_than_padded_with_whatever_was_there() {
        let copies = plane_copies(PixelFormat::Nv12, 640, 480, 640).expect("nv12");
        let bytes = driver_buffer(&copies, 0);
        let required = required_bytes(&copies).expect("a real geometry");
        let error = copy_planes(&bytes, required - 1, PixelFormat::Nv12, &copies)
            .expect_err("a partial frame is not a frame");
        assert!(error.contains("needs"), "{error}");
    }

    #[test]
    fn a_buffer_shorter_than_the_geometry_is_rejected_rather_than_read_past() {
        // The driver claims a full frame but handed over a smaller mapping. The
        // `bytesused` check would pass; only the length check stops the read.
        let copies = plane_copies(PixelFormat::Nv12, 640, 480, 640).expect("nv12");
        let required = required_bytes(&copies).expect("a real geometry");
        let bytes = vec![0_u8; required - 1];
        let error = copy_planes(&bytes, required, PixelFormat::Nv12, &copies)
            .expect_err("the mapping is too small");
        assert!(error.contains("buffer is"), "{error}");
    }

    #[test]
    fn a_frame_exactly_as_long_as_it_needs_to_be_copies() {
        let copies = plane_copies(PixelFormat::Yuyv422, 64, 32, 64 * 2).expect("yuyv");
        let required = required_bytes(&copies).expect("a real geometry");
        let bytes = driver_buffer(&copies, 0);
        assert_eq!(bytes.len(), required);
        assert!(copy_planes(&bytes, required, PixelFormat::Yuyv422, &copies).is_ok());
    }

    // ── Timestamps ──────────────────────────────────────────────────────────

    fn buffer_with(flags: u32, seconds: i64, micros: i64) -> v4l2_buffer {
        let mut buffer = v4l2_buffer::mmap(0);
        buffer.flags = flags;
        buffer.timestamp.tv_sec = seconds as _;
        buffer.timestamp.tv_usec = micros as _;
        buffer
    }

    #[test]
    fn a_monotonic_timestamp_becomes_a_duration() {
        let buffer = buffer_with(V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC, 12_345, 678_901);
        assert_eq!(
            device_timestamp(&buffer),
            Some(Duration::from_secs(12_345) + Duration::from_micros(678_901))
        );
    }

    #[test]
    fn only_the_monotonic_clock_is_a_device_clock() {
        for flags in [
            V4L2_BUF_FLAG_TIMESTAMP_UNKNOWN,
            V4L2_BUF_FLAG_TIMESTAMP_COPY,
        ] {
            let buffer = buffer_with(flags, 12_345, 0);
            assert_eq!(
                device_timestamp(&buffer),
                None,
                "flags {flags:#x} must fall back to host arrival"
            );
        }
    }

    #[test]
    fn a_damaged_frame_flag_does_not_disturb_the_clock_check() {
        let buffer = buffer_with(
            V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC | V4L2_BUF_FLAG_ERROR,
            1,
            0,
        );
        assert_eq!(device_timestamp(&buffer), Some(Duration::from_secs(1)));
        assert!(buffer.is_error());
    }

    #[test]
    fn a_negative_instant_is_no_instant_rather_than_a_fabricated_zero() {
        let buffer = buffer_with(V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC, -1, 0);
        assert_eq!(device_timestamp(&buffer), None);
        let buffer = buffer_with(V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC, 1, -1);
        assert_eq!(device_timestamp(&buffer), None);
    }

    #[test]
    fn zero_is_a_representable_instant() {
        let buffer = buffer_with(V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC, 0, 0);
        assert_eq!(device_timestamp(&buffer), Some(Duration::ZERO));
    }

    /// A device clock that steps backwards is the `MonotonicClock`'s problem,
    /// not this module's: raw instants are handed through unchanged.
    #[test]
    fn backwards_device_timestamps_are_passed_through_for_the_clock_to_clamp() {
        let mut clock = crate::frame::MonotonicClock::new();
        let raw: Vec<Duration> = [(3_i64, 0_i64), (4, 0), (3, 500_000)]
            .into_iter()
            .map(|(seconds, micros)| {
                device_timestamp(&buffer_with(
                    V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC,
                    seconds,
                    micros,
                ))
                .expect("valid")
            })
            .collect();
        let mapped: Vec<Duration> = raw.iter().map(|&raw| clock.map(raw)).collect();
        assert_eq!(
            mapped,
            vec![
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(1)
            ]
        );
        assert_eq!(clock.regressions(), 1);
    }

    // ── Readiness ───────────────────────────────────────────────────────────

    #[test]
    fn a_readable_descriptor_is_ready() {
        assert_eq!(classify(PollFlags::IN), Readiness::Ready);
    }

    #[test]
    fn an_empty_revents_word_is_not_a_frame() {
        assert_eq!(classify(PollFlags::empty()), Readiness::Waiting);
        assert_eq!(classify(PollFlags::OUT), Readiness::Waiting);
    }

    /// A driver error or an unplugged device must end the session, not spin: a
    /// descriptor with `POLLERR` raised stays raised, so treating it as "no
    /// frame yet" would burn a core forever.
    #[test]
    fn an_error_condition_ends_the_session_rather_than_spinning() {
        for flags in [PollFlags::ERR, PollFlags::HUP, PollFlags::NVAL] {
            assert!(
                matches!(classify(flags), Readiness::Lost(_)),
                "{flags:?} must end the session"
            );
            // Even when the driver also says a buffer is ready.
            assert!(
                matches!(classify(flags | PollFlags::IN), Readiness::Lost(_)),
                "{flags:?} alongside POLLIN must still end the session"
            );
        }
    }

    #[test]
    fn the_poll_timeout_keeps_shutdown_well_inside_the_promised_budget() {
        assert!(POLL_TIMEOUT < Duration::from_millis(500));
    }

    // ── Configuration arithmetic ────────────────────────────────────────────

    /// The frame rate is inverted on the way *into* `VIDIOC_S_PARM` as well as
    /// on the way out; asking for `fps_num/fps_den` seconds per frame would set
    /// a camera to 0.033 fps.
    #[test]
    fn a_frame_rate_request_is_sent_as_an_interval() {
        for (fps_num, fps_den) in [(30_u32, 1_u32), (60, 1), (30_000, 1001)] {
            let interval = v4l2_fract {
                numerator: fps_den,
                denominator: fps_num,
            };
            assert_eq!(
                super::super::fps_from_interval(interval.numerator, interval.denominator),
                Some((fps_num, fps_den)),
                "{fps_num}/{fps_den} did not round-trip through an interval"
            );
        }
    }

    #[test]
    fn a_compressed_encoding_has_no_plane_geometry() {
        assert_eq!(CaptureEncoding::Mjpeg.pixel(), None);
    }
}

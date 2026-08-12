//! Video4Linux2 UAPI: the structs, constants and four-character codes that the
//! `/dev/video*` ioctl protocol is defined in terms of.
//!
//! # Where these definitions came from
//!
//! Everything in this module is a hand transcription of the sanitized kernel
//! UAPI headers — `linux/videodev2.h` for the types and constants,
//! `asm-generic/ioctl.h` for the `_IOC` encoding used by [`super::ioctl`]. No
//! header is parsed at build time and no C is compiled: a `bindgen` step would
//! make this crate depend on a C toolchain, which the workspace forbids.
//!
//! Hand transcription only works if it is *checked*, so every layout claim in
//! this file is pinned by the [`golden layout table`](#golden-layout-table) at
//! the bottom, and those numbers are not remembered — they were derived
//! mechanically:
//!
//! ```text
//! zig translate-c -target <triple> -lc <(echo '#include <linux/videodev2.h>')
//! # then, at Zig comptime, @sizeOf / @alignOf / @offsetOf over the result
//! ```
//!
//! against the UAPI header set bundled with Zig 0.16.0
//! (`lib/zig/libc/include/any-linux-any/`), whose `linux/version.h` reports
//! `LINUX_VERSION_MAJOR 6`, `LINUX_VERSION_PATCHLEVEL 19`. The derivation was
//! run for four triples — `x86_64-linux-gnu`, `aarch64-linux-gnu`,
//! `arm-linux-gnueabihf` and `x86-linux-gnu` — which is what the
//! `target_pointer_width` split below records: the two 64-bit triples agree
//! with each other and the two 32-bit triples agree with each other, and
//! nothing else varies.
//!
//! These structures are ABI-frozen. `struct v4l2_buffer` and
//! `struct v4l2_format` have not changed shape since V4L2 was merged, and even
//! the 5.9 rework that added `capabilities`/`flags` to
//! `struct v4l2_requestbuffers` did so inside its existing `reserved` space, so
//! its size — and therefore `VIDIOC_REQBUFS`'s opcode — is unchanged. Reading a
//! 6.19 header to talk to a 4.x kernel is safe for exactly that reason.
//!
//! # Single-planar only
//!
//! This package implements `V4L2_BUF_TYPE_VIDEO_CAPTURE` (single-planar
//! streaming I/O with `mmap` buffers). [`v4l2_plane`] is transcribed for
//! completeness and pinned by the layout table, but nothing here issues a
//! multi-planar ioctl: the `_MPLANE` buffer types need a different `v4l2_format`
//! member, a different `v4l2_buffer` payload and per-plane `mmap` calls, and
//! claiming support without that code would be the fabrication this crate
//! exists to avoid. A device that offers *only* `_MPLANE` capture is reported
//! by [`super::V4l2Backend::enumerate`] as a device with no usable formats.

#![allow(unsafe_code)]
// The types below deliberately carry their C names. A `v4l2_buffer` in a Rust
// backtrace, a log line or a review diff is greppable against
// `linux/videodev2.h`; a `V4l2Buffer` is one rename away from being audited
// against the wrong struct. This is the same convention `libc` and
// `linux-raw-sys` use, and it is confined to this module — every type that
// leaves it is a normal Rust type in `super`.
#![allow(non_camel_case_types)]
// A UAPI transcription is a *record of an ABI*, and its completeness is the
// point: `v4l2_plane`, the reserved fields and the timestamp-clock constants
// that only the tests name today are what make the next reader able to check
// this file against the header instead of re-deriving it. Pruning it down to
// whatever the current capture path happens to read would turn every future
// addition into a fresh guess. Nothing here is dead in the sense the lint
// means — every struct's size, alignment and hot-field offsets are asserted by
// the golden table below, which is what keeps the record honest.
#![allow(dead_code)]

use core::mem::{align_of, size_of};

use oximedia_core::PixelFormat;

use crate::device::CaptureEncoding;

// ── Capability flags (`V4L2_CAP_*`) ──────────────────────────────────────────

/// `V4L2_CAP_VIDEO_CAPTURE` — the device can capture single-planar video.
pub(crate) const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
/// `V4L2_CAP_VIDEO_CAPTURE_MPLANE` — multi-planar capture, which this package
/// does not implement. Recognised only so the skip can name it.
pub(crate) const V4L2_CAP_VIDEO_CAPTURE_MPLANE: u32 = 0x0000_1000;
/// `V4L2_CAP_STREAMING` — the device supports the `mmap`/`QBUF`/`DQBUF`
/// streaming I/O this backend uses. A device without it would need `read()` I/O.
pub(crate) const V4L2_CAP_STREAMING: u32 = 0x0400_0000;
/// `V4L2_CAP_DEVICE_CAPS` — `v4l2_capability::device_caps` is filled in.
///
/// When it is clear, `device_caps` is a zero from an older kernel rather than a
/// device that can do nothing, and [`v4l2_capability::effective_caps`] falls
/// back to the driver-wide `capabilities` word.
pub(crate) const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

// ── Enumerations ─────────────────────────────────────────────────────────────

/// `V4L2_BUF_TYPE_VIDEO_CAPTURE` (`enum v4l2_buf_type`).
pub(crate) const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
/// `V4L2_MEMORY_MMAP` (`enum v4l2_memory`).
pub(crate) const V4L2_MEMORY_MMAP: u32 = 1;
/// `V4L2_FIELD_ANY` (`enum v4l2_field`) — "the driver may choose".
pub(crate) const V4L2_FIELD_ANY: u32 = 0;
/// `V4L2_FIELD_NONE` (`enum v4l2_field`) — progressive frames.
pub(crate) const V4L2_FIELD_NONE: u32 = 1;

/// `V4L2_FRMSIZE_TYPE_DISCRETE` — the device lists exact sizes.
pub(crate) const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;
/// `V4L2_FRMSIZE_TYPE_CONTINUOUS` — any size in a range, step 1.
pub(crate) const V4L2_FRMSIZE_TYPE_CONTINUOUS: u32 = 2;
/// `V4L2_FRMSIZE_TYPE_STEPWISE` — any size in a range, on a step grid.
pub(crate) const V4L2_FRMSIZE_TYPE_STEPWISE: u32 = 3;

/// `V4L2_FRMIVAL_TYPE_DISCRETE` — the device lists exact frame intervals.
pub(crate) const V4L2_FRMIVAL_TYPE_DISCRETE: u32 = 1;
/// `V4L2_FRMIVAL_TYPE_CONTINUOUS` — any interval in a range.
pub(crate) const V4L2_FRMIVAL_TYPE_CONTINUOUS: u32 = 2;
/// `V4L2_FRMIVAL_TYPE_STEPWISE` — any interval in a range, on a step grid.
pub(crate) const V4L2_FRMIVAL_TYPE_STEPWISE: u32 = 3;

// ── Buffer flags (`V4L2_BUF_FLAG_*`) ─────────────────────────────────────────

/// `V4L2_BUF_FLAG_ERROR` — the driver filled this buffer with a damaged frame.
///
/// The buffer must still be re-queued; the *frame* is what is unusable.
pub(crate) const V4L2_BUF_FLAG_ERROR: u32 = 0x0000_0040;
/// `V4L2_BUF_FLAG_TIMESTAMP_MASK` — the two bits that name the timestamp clock.
pub(crate) const V4L2_BUF_FLAG_TIMESTAMP_MASK: u32 = 0x0000_E000;
/// `V4L2_BUF_FLAG_TIMESTAMP_UNKNOWN` — the driver did not say.
pub(crate) const V4L2_BUF_FLAG_TIMESTAMP_UNKNOWN: u32 = 0x0000_0000;
/// `V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC` — `CLOCK_MONOTONIC`, sampled by the
/// driver when the frame was captured.
///
/// This is the only value that lets this backend report
/// [`TimestampSource::DeviceMonotonic`](crate::TimestampSource::DeviceMonotonic).
pub(crate) const V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC: u32 = 0x0000_2000;
/// `V4L2_BUF_FLAG_TIMESTAMP_COPY` — the timestamp was copied from an output
/// buffer, which for a capture device means it carries no capture instant.
pub(crate) const V4L2_BUF_FLAG_TIMESTAMP_COPY: u32 = 0x0000_4000;

// ── Format-description flags (`V4L2_FMT_FLAG_*`) ─────────────────────────────

/// `V4L2_FMT_FLAG_COMPRESSED` — the format is a bitstream, not a pixel layout.
pub(crate) const V4L2_FMT_FLAG_COMPRESSED: u32 = 0x0001;
/// `V4L2_FMT_FLAG_EMULATED` — the format is produced by an in-kernel software
/// converter (`libv4l`-style), not by the hardware.
pub(crate) const V4L2_FMT_FLAG_EMULATED: u32 = 0x0002;

// ── Stream-parameter flags ───────────────────────────────────────────────────

/// `V4L2_CAP_TIMEPERFRAME` — `VIDIOC_S_PARM` can set the frame interval.
///
/// Note that this shares its numeric value with
/// [`V4L2_CAP_VIDEO_CAPTURE_MPLANE`]; the two live in different words
/// (`v4l2_captureparm::capability` and `v4l2_capability::capabilities`) and are
/// never compared against the same value.
pub(crate) const V4L2_CAP_TIMEPERFRAME: u32 = 0x1000;

// ── Four-character codes ─────────────────────────────────────────────────────

/// Pack a four-character code the way V4L2 stores one.
///
/// `v4l2_fourcc(a, b, c, d)` is `a | b << 8 | c << 16 | d << 24` — **little**
/// endian, the opposite of Core Media's `FourCharCode`. `'Y','U','Y','V'` is
/// therefore `0x5659_5559`, not `0x5955_5956`.
pub(crate) const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | ((code[1] as u32) << 8) | ((code[2] as u32) << 16) | ((code[3] as u32) << 24)
}

/// `V4L2_PIX_FMT_YUYV` — packed 4:2:2, Y first.
pub(crate) const V4L2_PIX_FMT_YUYV: u32 = fourcc(b"YUYV");
/// `V4L2_PIX_FMT_UYVY` — packed 4:2:2, U first.
pub(crate) const V4L2_PIX_FMT_UYVY: u32 = fourcc(b"UYVY");
/// `V4L2_PIX_FMT_NV12` — Y plane + interleaved Cb/Cr plane.
pub(crate) const V4L2_PIX_FMT_NV12: u32 = fourcc(b"NV12");
/// `V4L2_PIX_FMT_NV21` — Y plane + interleaved Cr/Cb plane.
pub(crate) const V4L2_PIX_FMT_NV21: u32 = fourcc(b"NV21");
/// `V4L2_PIX_FMT_YUV420` (`YU12`) — planar 4:2:0, Cb plane before Cr.
pub(crate) const V4L2_PIX_FMT_YUV420: u32 = fourcc(b"YU12");
/// `V4L2_PIX_FMT_YVU420` (`YV12`) — planar 4:2:0, Cr plane before Cb.
pub(crate) const V4L2_PIX_FMT_YVU420: u32 = fourcc(b"YV12");
/// `V4L2_PIX_FMT_RGB24` (`RGB3`) — packed 8:8:8, red byte first.
pub(crate) const V4L2_PIX_FMT_RGB24: u32 = fourcc(b"RGB3");
/// `V4L2_PIX_FMT_BGR24` (`BGR3`) — packed 8:8:8, blue byte first.
pub(crate) const V4L2_PIX_FMT_BGR24: u32 = fourcc(b"BGR3");
/// `V4L2_PIX_FMT_MJPEG` — Motion JPEG, one JPEG image per buffer.
pub(crate) const V4L2_PIX_FMT_MJPEG: u32 = fourcc(b"MJPG");
/// `V4L2_PIX_FMT_JPEG` — baseline JPEG, which UVC cameras use interchangeably
/// with `MJPG`.
pub(crate) const V4L2_PIX_FMT_JPEG: u32 = fourcc(b"JPEG");

/// Map a V4L2 pixel format onto this crate's encoding model.
///
/// `None` means "this backend has no copy path for that code", and the format
/// is *skipped* by [`super::describe_formats`] with a `debug!` line rather than
/// guessed at. The distinction matters: a wrong guess is reported to the caller
/// as a real capture mode and then produces mis-decoded samples, which costs
/// far more to diagnose than a camera that advertises one mode fewer.
///
/// Three codes a UVC camera can plausibly offer are skipped on purpose, because
/// [`PixelFormat`] has no variant that describes them and there is nowhere
/// honest to put them:
///
/// * `YV12` (`V4L2_PIX_FMT_YVU420`) — planar 4:2:0 with the Cr plane before the
///   Cb plane. [`PixelFormat::Yuv420p`] means Cb-then-Cr; delivering YV12 under
///   that name would swap the chroma channels of every frame, which looks like
///   a working capture with the colours wrong.
/// * `BGR3` (`V4L2_PIX_FMT_BGR24`) — packed 8:8:8 with the channels reversed.
///   There is no `PixelFormat::Bgr24`, and [`PixelFormat::Rgb24`] is not it.
/// * everything else, including every 10- and 12-bit and Bayer code.
///
/// `NV21` *is* mapped, because [`PixelFormat::Nv21`] exists and means exactly
/// what V4L2 means by it.
pub(crate) const fn encoding_for_pixelformat(code: u32) -> Option<CaptureEncoding> {
    match code {
        V4L2_PIX_FMT_YUYV => Some(CaptureEncoding::Raw(PixelFormat::Yuyv422)),
        V4L2_PIX_FMT_UYVY => Some(CaptureEncoding::Raw(PixelFormat::Uyvy422)),
        V4L2_PIX_FMT_NV12 => Some(CaptureEncoding::Raw(PixelFormat::Nv12)),
        V4L2_PIX_FMT_NV21 => Some(CaptureEncoding::Raw(PixelFormat::Nv21)),
        V4L2_PIX_FMT_YUV420 => Some(CaptureEncoding::Raw(PixelFormat::Yuv420p)),
        V4L2_PIX_FMT_RGB24 => Some(CaptureEncoding::Raw(PixelFormat::Rgb24)),
        V4L2_PIX_FMT_MJPEG | V4L2_PIX_FMT_JPEG => Some(CaptureEncoding::Mjpeg),
        _ => None,
    }
}

/// The V4L2 code this backend asks a device for, given a negotiated encoding.
///
/// The inverse of [`encoding_for_pixelformat`], except that
/// [`CaptureEncoding::Mjpeg`] has two possible codes and this returns `MJPG` —
/// the one every UVC camera advertises. A device that only offered `JPEG` will
/// have had that code enumerated, and [`super::stream`] re-checks the driver's
/// echo, so a mismatch is reported rather than silently accepted.
pub(crate) const fn pixelformat_for_encoding(encoding: CaptureEncoding) -> Option<u32> {
    match encoding {
        CaptureEncoding::Raw(PixelFormat::Yuyv422) => Some(V4L2_PIX_FMT_YUYV),
        CaptureEncoding::Raw(PixelFormat::Uyvy422) => Some(V4L2_PIX_FMT_UYVY),
        CaptureEncoding::Raw(PixelFormat::Nv12) => Some(V4L2_PIX_FMT_NV12),
        CaptureEncoding::Raw(PixelFormat::Nv21) => Some(V4L2_PIX_FMT_NV21),
        CaptureEncoding::Raw(PixelFormat::Yuv420p) => Some(V4L2_PIX_FMT_YUV420),
        CaptureEncoding::Raw(PixelFormat::Rgb24) => Some(V4L2_PIX_FMT_RGB24),
        CaptureEncoding::Mjpeg => Some(V4L2_PIX_FMT_MJPEG),
        CaptureEncoding::Raw(_) => None,
    }
}

/// Render a four-character code the way `v4l2-ctl` spells it.
///
/// Non-printable bytes become `.`, so a corrupt or unexpected code still
/// produces a readable log line instead of mangling the output stream.
pub(crate) fn fourcc_name(code: u32) -> String {
    code.to_le_bytes()
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                '.'
            }
        })
        .collect()
}

// ── struct v4l2_capability ───────────────────────────────────────────────────

/// `struct v4l2_capability` — the answer to `VIDIOC_QUERYCAP`.
///
/// Transcribed from `linux/videodev2.h`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_capability {
    /// Kernel module name, e.g. `uvcvideo`.
    pub(crate) driver: [u8; 16],
    /// Product name as the device reports it; the source of
    /// [`CaptureDevice::name`](crate::CaptureDevice::name).
    pub(crate) card: [u8; 32],
    /// Bus location, e.g. `usb-0000:00:14.0-4`.
    pub(crate) bus_info: [u8; 32],
    /// Kernel version, `KERNEL_VERSION(a, b, c)`.
    pub(crate) version: u32,
    /// Capabilities of the *driver* — the union over every node it owns.
    pub(crate) capabilities: u32,
    /// Capabilities of *this* node. Meaningful only when
    /// [`V4L2_CAP_DEVICE_CAPS`] is set in [`Self::capabilities`].
    pub(crate) device_caps: u32,
    /// Reserved; must be read as zero.
    pub(crate) reserved: [u32; 3],
}

impl Default for v4l2_capability {
    fn default() -> Self {
        Self {
            driver: [0; 16],
            card: [0; 32],
            bus_info: [0; 32],
            version: 0,
            capabilities: 0,
            device_caps: 0,
            reserved: [0; 3],
        }
    }
}

impl v4l2_capability {
    /// The capability word that describes the node that was queried.
    ///
    /// `device_caps` is per-node and is what distinguishes a capture node from
    /// the metadata, radio or VBI nodes a driver registers alongside it. It only
    /// exists from Linux 3.3 onwards, and the kernel says so by setting
    /// [`V4L2_CAP_DEVICE_CAPS`]; without that bit the field is a zero this
    /// backend must not read as "this node can do nothing", so the driver-wide
    /// word is used instead.
    pub(crate) const fn effective_caps(&self) -> u32 {
        if self.capabilities & V4L2_CAP_DEVICE_CAPS == 0 {
            self.capabilities
        } else {
            self.device_caps
        }
    }
}

// ── struct v4l2_fmtdesc ──────────────────────────────────────────────────────

/// `struct v4l2_fmtdesc` — one entry of the `VIDIOC_ENUM_FMT` walk.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_fmtdesc {
    /// Zero-based index; the caller increments it until `EINVAL`.
    pub(crate) index: u32,
    /// `enum v4l2_buf_type`; set by the caller.
    pub(crate) type_: u32,
    /// `V4L2_FMT_FLAG_*`.
    pub(crate) flags: u32,
    /// NUL-terminated human-readable name, e.g. `"Motion-JPEG"`.
    pub(crate) description: [u8; 32],
    /// The four-character code this entry describes.
    pub(crate) pixelformat: u32,
    /// Media-bus code, for sub-device pipelines. Unused here.
    pub(crate) mbus_code: u32,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved: [u32; 3],
}

impl Default for v4l2_fmtdesc {
    fn default() -> Self {
        Self {
            index: 0,
            type_: 0,
            flags: 0,
            description: [0; 32],
            pixelformat: 0,
            mbus_code: 0,
            reserved: [0; 3],
        }
    }
}

// ── struct v4l2_pix_format ───────────────────────────────────────────────────

/// `struct v4l2_pix_format` — the single-planar member of [`v4l2_format`].
///
/// Twelve `__u32`s; the eleventh is an anonymous union of `ycbcr_enc` and
/// `hsv_enc`, which are the same width and are therefore transcribed as one
/// field under the name the capture path would use.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_pix_format {
    /// Frame width in pixels.
    pub(crate) width: u32,
    /// Frame height in pixels.
    pub(crate) height: u32,
    /// Four-character code.
    pub(crate) pixelformat: u32,
    /// `enum v4l2_field`.
    pub(crate) field: u32,
    /// Distance in bytes between the start of two consecutive rows of the
    /// **largest** plane. Zero for compressed formats.
    ///
    /// This is the driver's stride, and it is routinely larger than
    /// `width × bytes-per-pixel`. Copying without honouring it is the classic
    /// V4L2 bug: the image shears progressively down the frame.
    pub(crate) bytesperline: u32,
    /// Size in bytes of one complete image, including padding.
    pub(crate) sizeimage: u32,
    /// `enum v4l2_colorspace`.
    pub(crate) colorspace: u32,
    /// Format-dependent private data.
    pub(crate) priv_: u32,
    /// `V4L2_PIX_FMT_FLAG_*`.
    pub(crate) flags: u32,
    /// Anonymous union of `ycbcr_enc` (`enum v4l2_ycbcr_encoding`) and
    /// `hsv_enc` (`enum v4l2_hsv_encoding`), both `__u32`.
    pub(crate) ycbcr_enc: u32,
    /// `enum v4l2_quantization`.
    pub(crate) quantization: u32,
    /// `enum v4l2_xfer_func`.
    pub(crate) xfer_func: u32,
}

// ── struct v4l2_format ───────────────────────────────────────────────────────

/// Size of `union v4l2_format::fmt`, which `raw_data[200]` pins.
///
/// Pinned by the golden table below rather than assumed: the union's size is
/// what `VIDIOC_S_FMT`'s opcode encodes, and an opcode that is one byte off is
/// an `ENOTTY` from every driver.
pub(crate) const V4L2_FORMAT_UNION_BYTES: usize = 200;

/// `union v4l2_format::fmt`.
///
/// The C union's widest member is `__u8 raw_data[200]`, but its *alignment*
/// comes from `struct v4l2_window`, which contains `struct v4l2_clip *clips` —
/// a pointer. So the union is 200 bytes with pointer alignment: 8 on a 64-bit
/// ABI, 4 on a 32-bit one. That is why [`v4l2_format`] is 208 bytes on 64-bit
/// and 204 on 32-bit, and it is the single most expensive detail in this file to
/// get wrong.
///
/// `_align` carries that alignment. It is never read; `usize` is `unsigned long`
/// on every Linux ABI this crate targets, which is exactly the width of the
/// pointer the C union really contains.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_format_fmt {
    /// `__u8 raw_data[200]`.
    pub(crate) raw_data: [u8; V4L2_FORMAT_UNION_BYTES],
    /// Alignment carrier; see the type docs.
    _align: usize,
}

impl Default for v4l2_format_fmt {
    fn default() -> Self {
        Self {
            raw_data: [0; V4L2_FORMAT_UNION_BYTES],
        }
    }
}

/// `struct v4l2_format` — the argument to `VIDIOC_G_FMT` / `VIDIOC_S_FMT`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct v4l2_format {
    /// `enum v4l2_buf_type`; selects which union member is live.
    pub(crate) type_: u32,
    /// The format itself, as raw bytes. Use [`Self::pix`] / [`Self::set_pix`].
    pub(crate) fmt: v4l2_format_fmt,
}

impl v4l2_format {
    /// A `VIDIOC_S_FMT` argument for single-planar video capture.
    pub(crate) fn capture(pix: v4l2_pix_format) -> Self {
        let mut format = Self {
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
            fmt: v4l2_format_fmt::default(),
        };
        format.set_pix(pix);
        format
    }

    /// Read the union as a [`v4l2_pix_format`].
    ///
    /// Reading a `#[repr(C)]` union member of a plain-old-data type is always
    /// defined here: the whole 200-byte payload is initialised (by
    /// [`v4l2_format_fmt::default`] before the ioctl, by the kernel after it),
    /// `v4l2_pix_format` is 48 bytes of `u32` with no padding and no niche, and
    /// [`core::ptr::read_unaligned`] imposes no alignment requirement of its own.
    ///
    /// The caller is responsible for only trusting the result when
    /// [`Self::type_`] says the `pix` member is the live one — which for this
    /// backend it always does, because [`Self::capture`] is the only constructor
    /// and the driver echoes the type back.
    pub(crate) fn pix(&self) -> v4l2_pix_format {
        // SAFETY: `self.fmt` is a fully initialised 200-byte payload and
        // `v4l2_pix_format` is 48 bytes of `u32`, so the read is in bounds and
        // every bit pattern it can see is a valid value of the type.
        // `read_unaligned` needs no alignment guarantee.
        unsafe {
            core::ptr::from_ref(&self.fmt)
                .cast::<v4l2_pix_format>()
                .read_unaligned()
        }
    }

    /// Write the union from a [`v4l2_pix_format`], leaving the tail zeroed.
    pub(crate) fn set_pix(&mut self, pix: v4l2_pix_format) {
        self.fmt = v4l2_format_fmt::default();
        // SAFETY: the destination is a 200-byte payload and the value written is
        // 48 bytes, so the write is in bounds; `write_unaligned` needs no
        // alignment guarantee, and the union has no member whose invariants
        // could be violated by overwriting its bytes.
        unsafe {
            core::ptr::from_mut(&mut self.fmt)
                .cast::<v4l2_pix_format>()
                .write_unaligned(pix);
        }
    }
}

// ── struct v4l2_requestbuffers ───────────────────────────────────────────────

/// `struct v4l2_requestbuffers` — the argument to `VIDIOC_REQBUFS`.
///
/// The `capabilities` and `flags` members arrived in Linux 5.9, carved out of
/// the original `reserved[2]`; the struct size is unchanged, which is why the
/// opcode is unchanged and why a 5.9-derived transcription talks to a 4.x kernel
/// correctly. Older kernels leave both zero.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct v4l2_requestbuffers {
    /// Requested buffer count on the way in; **granted** count on the way out.
    pub(crate) count: u32,
    /// `enum v4l2_buf_type`.
    pub(crate) type_: u32,
    /// `enum v4l2_memory`.
    pub(crate) memory: u32,
    /// `V4L2_BUF_CAP_*`, Linux 5.4+. Zero on older kernels.
    pub(crate) capabilities: u32,
    /// `V4L2_MEMORY_FLAG_*`, Linux 5.9+.
    pub(crate) flags: u8,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved: [u8; 3],
}

// ── struct timeval / struct v4l2_timecode ────────────────────────────────────

/// `struct timeval`, as `struct v4l2_buffer::timestamp` uses it.
///
/// `linux/videodev2.h` includes `<sys/time.h>` for userspace, so this is the
/// C library's `struct timeval` — two `long`s, and therefore
/// `target_pointer_width`-sensitive: 16 bytes on a 64-bit ABI, 8 on a 32-bit
/// one, which is what moves every field after it in [`v4l2_buffer`] and changes
/// that struct's size from 88 to 68.
///
/// The 32-bit arm below models the **kernel's** legacy 32-bit `timeval`, which
/// is the layout `VIDIOC_DQBUF`'s opcode is computed from and the one the
/// kernel's 32-bit ABI accepts. A 32-bit userspace built against a
/// `_TIME_BITS=64` C library has a 16-byte `struct timeval` and would compute a
/// *different*, 84-byte `v4l2_buffer`; the kernel handles that case through its
/// separate `VIDIOC_DQBUF_TIME64` opcode, which this package does not implement.
/// 32-bit builds are therefore correct against the legacy ABI and are not
/// silently wrong: the golden table records exactly which one is transcribed.
#[cfg(target_pointer_width = "64")]
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_timeval {
    /// Whole seconds.
    pub(crate) tv_sec: i64,
    /// Microseconds within the second.
    pub(crate) tv_usec: i64,
}

/// `struct timeval` on a 32-bit ABI. See the 64-bit arm for the full argument.
#[cfg(not(target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_timeval {
    /// Whole seconds.
    pub(crate) tv_sec: i32,
    /// Microseconds within the second.
    pub(crate) tv_usec: i32,
}

/// `struct v4l2_timecode` — SMPTE timecode; carried but never interpreted here.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct v4l2_timecode {
    /// `V4L2_TC_TYPE_*`.
    pub(crate) type_: u32,
    /// `V4L2_TC_FLAG_*`.
    pub(crate) flags: u32,
    /// Frames within the second.
    pub(crate) frames: u8,
    /// Seconds.
    pub(crate) seconds: u8,
    /// Minutes.
    pub(crate) minutes: u8,
    /// Hours.
    pub(crate) hours: u8,
    /// Four user bytes.
    pub(crate) userbits: [u8; 4],
}

// ── struct v4l2_plane ────────────────────────────────────────────────────────

/// `union v4l2_plane::m`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_plane_m {
    /// `V4L2_MEMORY_MMAP` offset, to be passed to `mmap`.
    pub(crate) mem_offset: u32,
    /// `V4L2_MEMORY_USERPTR` address. `unsigned long`, hence `usize`.
    pub(crate) userptr: usize,
    /// `V4L2_MEMORY_DMABUF` file descriptor.
    pub(crate) fd: i32,
}

impl Default for v4l2_plane_m {
    fn default() -> Self {
        Self { userptr: 0 }
    }
}

/// `struct v4l2_plane` — one plane of a multi-planar buffer.
///
/// **Transcribed for completeness only.** This package implements single-planar
/// capture; no ioctl here ever points at a `v4l2_plane`. It is pinned by the
/// golden table so that a future multi-planar package starts from a layout that
/// is already checked rather than from a fresh guess.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct v4l2_plane {
    /// Bytes the driver filled.
    pub(crate) bytesused: u32,
    /// Plane size in bytes.
    pub(crate) length: u32,
    /// Where the plane lives, per `v4l2_buffer::memory`.
    pub(crate) m: v4l2_plane_m,
    /// Offset of the data within the plane.
    pub(crate) data_offset: u32,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved: [u32; 11],
}

// ── struct v4l2_buffer ───────────────────────────────────────────────────────

/// `union v4l2_buffer::m`.
///
/// `planes` is `struct v4l2_plane *` in C and is transcribed as a `usize`
/// rather than a raw pointer on purpose: this backend only ever uses the
/// `V4L2_MEMORY_MMAP` `offset` member, a `usize` is exactly as wide and as
/// aligned as the pointer it replaces (`unsigned long` and `void *` agree on
/// every Linux ABI), and keeping a raw pointer out of the union keeps
/// [`v4l2_buffer`] `Send` — which matters because the capture runner is moved to
/// its own thread.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_buffer_m {
    /// `V4L2_MEMORY_MMAP` offset, to be passed to `mmap`.
    pub(crate) offset: u32,
    /// `V4L2_MEMORY_USERPTR` address. `unsigned long`.
    pub(crate) userptr: usize,
    /// `struct v4l2_plane *` for multi-planar buffers; see the type docs.
    pub(crate) planes: usize,
    /// `V4L2_MEMORY_DMABUF` file descriptor.
    pub(crate) fd: i32,
}

impl Default for v4l2_buffer_m {
    fn default() -> Self {
        Self { userptr: 0 }
    }
}

/// `struct v4l2_buffer` — the argument to `VIDIOC_QUERYBUF`, `VIDIOC_QBUF` and
/// `VIDIOC_DQBUF`.
///
/// The trailing anonymous `union { __s32 request_fd; __u32 reserved; }` is
/// transcribed as a single `i32` under the name of its first member: both
/// members are four bytes with four-byte alignment, so the layout is identical,
/// and neither is used by this backend.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct v4l2_buffer {
    /// Buffer index within the driver's ring.
    pub(crate) index: u32,
    /// `enum v4l2_buf_type`.
    pub(crate) type_: u32,
    /// Bytes the driver actually filled. Less than `length` for MJPEG.
    pub(crate) bytesused: u32,
    /// `V4L2_BUF_FLAG_*`.
    pub(crate) flags: u32,
    /// `enum v4l2_field`.
    pub(crate) field: u32,
    /// Capture instant, on the clock named by
    /// `flags & V4L2_BUF_FLAG_TIMESTAMP_MASK`.
    pub(crate) timestamp: v4l2_timeval,
    /// SMPTE timecode; only valid with `V4L2_BUF_FLAG_TIMECODE`.
    pub(crate) timecode: v4l2_timecode,
    /// The driver's own frame counter. A gap means the *driver* lost frames.
    pub(crate) sequence: u32,
    /// `enum v4l2_memory`.
    pub(crate) memory: u32,
    /// Where the buffer lives, per [`Self::memory`].
    pub(crate) m: v4l2_buffer_m,
    /// Buffer size in bytes.
    pub(crate) length: u32,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved2: u32,
    /// Anonymous union of `__s32 request_fd` and `__u32 reserved`.
    pub(crate) request_fd: i32,
}

impl v4l2_buffer {
    /// A `VIDIOC_QUERYBUF` / `VIDIOC_QBUF` argument for one `mmap` buffer.
    pub(crate) fn mmap(index: u32) -> Self {
        Self {
            index,
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
            memory: V4L2_MEMORY_MMAP,
            ..Self::default()
        }
    }

    /// The `mmap` offset the driver assigned to this buffer.
    pub(crate) const fn mmap_offset(&self) -> u32 {
        // SAFETY: the caller filled `memory` with `V4L2_MEMORY_MMAP` before the
        // `VIDIOC_QUERYBUF` that produced this value, and for that memory type
        // the kernel documents `m.offset` as the live member. Every member of
        // the union is a plain integer with no invalid bit pattern, so even a
        // driver that filled a different member yields a number, not undefined
        // behaviour.
        unsafe { self.m.offset }
    }

    /// Whether the driver reported this frame as damaged.
    pub(crate) const fn is_error(&self) -> bool {
        self.flags & V4L2_BUF_FLAG_ERROR != 0
    }

    /// Whether [`Self::timestamp`] is a `CLOCK_MONOTONIC` capture instant.
    pub(crate) const fn has_monotonic_timestamp(&self) -> bool {
        self.flags & V4L2_BUF_FLAG_TIMESTAMP_MASK == V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC
    }
}

// ── Frame-size enumeration ───────────────────────────────────────────────────

/// `struct v4l2_frmsize_discrete`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_frmsize_discrete {
    /// Frame width in pixels.
    pub(crate) width: u32,
    /// Frame height in pixels.
    pub(crate) height: u32,
}

/// `struct v4l2_frmsize_stepwise`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_frmsize_stepwise {
    /// Smallest supported width.
    pub(crate) min_width: u32,
    /// Largest supported width.
    pub(crate) max_width: u32,
    /// Width granularity. `1` for `V4L2_FRMSIZE_TYPE_CONTINUOUS`.
    pub(crate) step_width: u32,
    /// Smallest supported height.
    pub(crate) min_height: u32,
    /// Largest supported height.
    pub(crate) max_height: u32,
    /// Height granularity. `1` for `V4L2_FRMSIZE_TYPE_CONTINUOUS`.
    pub(crate) step_height: u32,
}

/// The anonymous union inside `struct v4l2_frmsizeenum`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_frmsizeenum_size {
    /// Live when `type_ == V4L2_FRMSIZE_TYPE_DISCRETE`.
    pub(crate) discrete: v4l2_frmsize_discrete,
    /// Live when `type_` is `CONTINUOUS` or `STEPWISE`.
    pub(crate) stepwise: v4l2_frmsize_stepwise,
}

impl Default for v4l2_frmsizeenum_size {
    fn default() -> Self {
        Self {
            stepwise: v4l2_frmsize_stepwise::default(),
        }
    }
}

/// `struct v4l2_frmsizeenum` — one entry of the `VIDIOC_ENUM_FRAMESIZES` walk.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct v4l2_frmsizeenum {
    /// Zero-based index; the caller increments it until `EINVAL`.
    pub(crate) index: u32,
    /// The four-character code being enumerated; set by the caller.
    pub(crate) pixel_format: u32,
    /// `V4L2_FRMSIZE_TYPE_*`.
    pub(crate) type_: u32,
    /// Discrete size or stepwise range, per [`Self::type_`].
    pub(crate) size: v4l2_frmsizeenum_size,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved: [u32; 2],
}

impl v4l2_frmsizeenum {
    /// A `VIDIOC_ENUM_FRAMESIZES` argument.
    pub(crate) fn query(index: u32, pixel_format: u32) -> Self {
        Self {
            index,
            pixel_format,
            ..Self::default()
        }
    }

    /// The discrete size, when [`Self::type_`] says there is one.
    pub(crate) const fn discrete(&self) -> Option<v4l2_frmsize_discrete> {
        if self.type_ == V4L2_FRMSIZE_TYPE_DISCRETE {
            // SAFETY: the kernel sets `type_` and the union together, and this
            // is the member it documents for `V4L2_FRMSIZE_TYPE_DISCRETE`. Both
            // members are plain `u32`s, so no bit pattern is invalid either way.
            Some(unsafe { self.size.discrete })
        } else {
            None
        }
    }

    /// The stepwise range, when [`Self::type_`] says there is one.
    ///
    /// `V4L2_FRMSIZE_TYPE_CONTINUOUS` uses the same member with both step fields
    /// set to `1`, which is why both types map here.
    pub(crate) const fn stepwise(&self) -> Option<v4l2_frmsize_stepwise> {
        if self.type_ == V4L2_FRMSIZE_TYPE_CONTINUOUS || self.type_ == V4L2_FRMSIZE_TYPE_STEPWISE {
            // SAFETY: as `discrete`, for the member the kernel documents for
            // these two types.
            Some(unsafe { self.size.stepwise })
        } else {
            None
        }
    }
}

// ── Frame-interval enumeration ───────────────────────────────────────────────

/// `struct v4l2_fract` — an exact rational.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_fract {
    /// Numerator.
    pub(crate) numerator: u32,
    /// Denominator.
    pub(crate) denominator: u32,
}

/// `struct v4l2_frmival_stepwise`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_frmival_stepwise {
    /// Shortest interval, i.e. the *highest* frame rate.
    pub(crate) min: v4l2_fract,
    /// Longest interval, i.e. the *lowest* frame rate.
    pub(crate) max: v4l2_fract,
    /// Interval granularity.
    pub(crate) step: v4l2_fract,
}

/// The anonymous union inside `struct v4l2_frmivalenum`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_frmivalenum_interval {
    /// Live when `type_ == V4L2_FRMIVAL_TYPE_DISCRETE`.
    pub(crate) discrete: v4l2_fract,
    /// Live when `type_` is `CONTINUOUS` or `STEPWISE`.
    pub(crate) stepwise: v4l2_frmival_stepwise,
}

impl Default for v4l2_frmivalenum_interval {
    fn default() -> Self {
        Self {
            stepwise: v4l2_frmival_stepwise::default(),
        }
    }
}

/// `struct v4l2_frmivalenum` — one entry of the `VIDIOC_ENUM_FRAMEINTERVALS`
/// walk.
///
/// The values here are frame **intervals** — seconds per frame — not rates. See
/// [`super::fps_from_interval`] for the inversion, which is the other easy V4L2
/// mistake to make.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct v4l2_frmivalenum {
    /// Zero-based index; the caller increments it until `EINVAL`.
    pub(crate) index: u32,
    /// The four-character code being enumerated; set by the caller.
    pub(crate) pixel_format: u32,
    /// Frame width being enumerated; set by the caller.
    pub(crate) width: u32,
    /// Frame height being enumerated; set by the caller.
    pub(crate) height: u32,
    /// `V4L2_FRMIVAL_TYPE_*`.
    pub(crate) type_: u32,
    /// Discrete interval or stepwise range, per [`Self::type_`].
    pub(crate) interval: v4l2_frmivalenum_interval,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved: [u32; 2],
}

impl v4l2_frmivalenum {
    /// A `VIDIOC_ENUM_FRAMEINTERVALS` argument.
    pub(crate) fn query(index: u32, pixel_format: u32, width: u32, height: u32) -> Self {
        Self {
            index,
            pixel_format,
            width,
            height,
            ..Self::default()
        }
    }

    /// The discrete interval, when [`Self::type_`] says there is one.
    pub(crate) const fn discrete(&self) -> Option<v4l2_fract> {
        if self.type_ == V4L2_FRMIVAL_TYPE_DISCRETE {
            // SAFETY: the kernel sets `type_` and the union together, and this
            // is the member it documents for `V4L2_FRMIVAL_TYPE_DISCRETE`. Every
            // member is made of `u32`s, so no bit pattern is invalid.
            Some(unsafe { self.interval.discrete })
        } else {
            None
        }
    }

    /// The stepwise range, when [`Self::type_`] says there is one.
    pub(crate) const fn stepwise(&self) -> Option<v4l2_frmival_stepwise> {
        if self.type_ == V4L2_FRMIVAL_TYPE_CONTINUOUS || self.type_ == V4L2_FRMIVAL_TYPE_STEPWISE {
            // SAFETY: as `discrete`, for the member the kernel documents for
            // these two types.
            Some(unsafe { self.interval.stepwise })
        } else {
            None
        }
    }
}

// ── Stream parameters ────────────────────────────────────────────────────────

/// `struct v4l2_captureparm` — the capture member of [`v4l2_streamparm`].
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct v4l2_captureparm {
    /// `V4L2_CAP_*` for stream parameters; see [`V4L2_CAP_TIMEPERFRAME`].
    pub(crate) capability: u32,
    /// `V4L2_MODE_*`.
    pub(crate) capturemode: u32,
    /// Seconds per frame — an interval, not a rate.
    pub(crate) timeperframe: v4l2_fract,
    /// Driver-specific extensions.
    pub(crate) extendedmode: u32,
    /// Buffers used by `read()` I/O; irrelevant to streaming I/O.
    pub(crate) readbuffers: u32,
    /// Reserved; must be zeroed by the caller.
    pub(crate) reserved: [u32; 4],
}

/// Size of `union v4l2_streamparm::parm`, which `raw_data[200]` pins.
pub(crate) const V4L2_STREAMPARM_UNION_BYTES: usize = 200;

/// `union v4l2_streamparm::parm`.
///
/// Unlike [`v4l2_format_fmt`] this union has **no** pointer member — its widest
/// members are `struct v4l2_captureparm`, `struct v4l2_outputparm` and
/// `__u8 raw_data[200]`, all of them `__u32`-aligned — so it is 200 bytes with
/// four-byte alignment on every ABI, and [`v4l2_streamparm`] is 204 bytes on
/// 64-bit *and* 32-bit. Two unions that look identical in the header do not have
/// the same layout; the golden table records both.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_streamparm_parm {
    /// `__u8 raw_data[200]`.
    pub(crate) raw_data: [u8; V4L2_STREAMPARM_UNION_BYTES],
    /// Alignment carrier: `__u32`, the widest member of `struct
    /// v4l2_captureparm`. Never read.
    _align: u32,
}

impl Default for v4l2_streamparm_parm {
    fn default() -> Self {
        Self {
            raw_data: [0; V4L2_STREAMPARM_UNION_BYTES],
        }
    }
}

/// `struct v4l2_streamparm` — the argument to `VIDIOC_G_PARM` / `VIDIOC_S_PARM`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct v4l2_streamparm {
    /// `enum v4l2_buf_type`; selects which union member is live.
    pub(crate) type_: u32,
    /// The parameters themselves. Use [`Self::capture`] / [`Self::set_capture`].
    pub(crate) parm: v4l2_streamparm_parm,
}

impl v4l2_streamparm {
    /// A `VIDIOC_G_PARM` argument for single-planar video capture.
    pub(crate) fn query() -> Self {
        Self {
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
            parm: v4l2_streamparm_parm::default(),
        }
    }

    /// Read the union as a [`v4l2_captureparm`].
    ///
    /// Sound for the same reasons as [`v4l2_format::pix`]: the payload is fully
    /// initialised, the value read is 40 bytes of `u32` out of 200, and
    /// `read_unaligned` imposes no alignment requirement.
    pub(crate) fn capture(&self) -> v4l2_captureparm {
        // SAFETY: see the doc comment; the read is in bounds and every bit
        // pattern of a `v4l2_captureparm` is a valid value.
        unsafe {
            core::ptr::from_ref(&self.parm)
                .cast::<v4l2_captureparm>()
                .read_unaligned()
        }
    }

    /// Write the union from a [`v4l2_captureparm`], leaving the tail zeroed.
    pub(crate) fn set_capture(&mut self, capture: v4l2_captureparm) {
        self.parm = v4l2_streamparm_parm::default();
        // SAFETY: the destination is a 200-byte payload and the value written is
        // 40 bytes, so the write is in bounds, and `write_unaligned` needs no
        // alignment guarantee.
        unsafe {
            core::ptr::from_mut(&mut self.parm)
                .cast::<v4l2_captureparm>()
                .write_unaligned(capture);
        }
    }
}

// ── Golden layout table ──────────────────────────────────────────────────────
//
// DERIVED, NOT REMEMBERED. Every number below was produced by the Zig
// `translate-c` + comptime `@sizeOf`/`@alignOf`/`@offsetOf` procedure described
// in the module docs, run over the real `linux/videodev2.h`. The two arms differ
// only where `struct timeval` and the pointer inside `union v4l2_format::fmt` do.
//
// The block is at module scope, not inside `mod tests`, on purpose: a plain
// `cargo check --target aarch64-unknown-linux-gnu` from a non-Linux host
// evaluates these consts and fails the build if any transcription drifted. The
// `#[test]` mirrors below repeat the assertions so a real Linux test run reports
// them by name.

/// Expected sizes, alignments and offsets for a 64-bit Linux ABI.
#[cfg(target_pointer_width = "64")]
mod expected {
    /// `(size, align)` of each transcribed struct.
    pub(super) const SIZES: [(&str, usize, usize); 17] = [
        ("v4l2_capability", 104, 4),
        ("v4l2_fmtdesc", 64, 4),
        ("v4l2_pix_format", 48, 4),
        ("v4l2_format", 208, 8),
        ("v4l2_requestbuffers", 20, 4),
        ("v4l2_buffer", 88, 8),
        ("v4l2_plane", 64, 8),
        ("v4l2_timecode", 16, 4),
        ("v4l2_timeval", 16, 8),
        ("v4l2_frmsizeenum", 44, 4),
        ("v4l2_frmsize_discrete", 8, 4),
        ("v4l2_frmsize_stepwise", 24, 4),
        ("v4l2_frmivalenum", 52, 4),
        ("v4l2_frmival_stepwise", 24, 4),
        ("v4l2_fract", 8, 4),
        ("v4l2_streamparm", 204, 4),
        ("v4l2_captureparm", 40, 4),
    ];

    /// Byte offsets of the `v4l2_buffer` fields the capture loop reads.
    pub(super) const BUFFER_OFFSETS: [(&str, usize); 12] = [
        ("index", 0),
        ("type_", 4),
        ("bytesused", 8),
        ("flags", 12),
        ("field", 16),
        ("timestamp", 24),
        ("timecode", 40),
        ("sequence", 56),
        ("memory", 60),
        ("m", 64),
        ("length", 72),
        ("reserved2", 76),
    ];

    /// Offset of `v4l2_format::fmt` — 8, because the union is pointer-aligned.
    pub(super) const FORMAT_FMT_OFFSET: usize = 8;
    /// Offset of `v4l2_streamparm::parm` — 4, because that union is not.
    pub(super) const STREAMPARM_PARM_OFFSET: usize = 4;
}

/// Expected sizes, alignments and offsets for a 32-bit Linux ABI.
///
/// This is the kernel's legacy 32-bit ABI, with an 8-byte `struct timeval`; see
/// [`super::v4l2_timeval`] for why a `_TIME_BITS=64` userspace is a different,
/// unimplemented case rather than a silent mismatch.
#[cfg(not(target_pointer_width = "64"))]
mod expected {
    /// `(size, align)` of each transcribed struct.
    pub(super) const SIZES: [(&str, usize, usize); 17] = [
        ("v4l2_capability", 104, 4),
        ("v4l2_fmtdesc", 64, 4),
        ("v4l2_pix_format", 48, 4),
        ("v4l2_format", 204, 4),
        ("v4l2_requestbuffers", 20, 4),
        ("v4l2_buffer", 68, 4),
        ("v4l2_plane", 60, 4),
        ("v4l2_timecode", 16, 4),
        ("v4l2_timeval", 8, 4),
        ("v4l2_frmsizeenum", 44, 4),
        ("v4l2_frmsize_discrete", 8, 4),
        ("v4l2_frmsize_stepwise", 24, 4),
        ("v4l2_frmivalenum", 52, 4),
        ("v4l2_frmival_stepwise", 24, 4),
        ("v4l2_fract", 8, 4),
        ("v4l2_streamparm", 204, 4),
        ("v4l2_captureparm", 40, 4),
    ];

    /// Byte offsets of the `v4l2_buffer` fields the capture loop reads.
    pub(super) const BUFFER_OFFSETS: [(&str, usize); 12] = [
        ("index", 0),
        ("type_", 4),
        ("bytesused", 8),
        ("flags", 12),
        ("field", 16),
        ("timestamp", 20),
        ("timecode", 28),
        ("sequence", 44),
        ("memory", 48),
        ("m", 52),
        ("length", 56),
        ("reserved2", 60),
    ];

    /// Offset of `v4l2_format::fmt`.
    pub(super) const FORMAT_FMT_OFFSET: usize = 4;
    /// Offset of `v4l2_streamparm::parm`.
    pub(super) const STREAMPARM_PARM_OFFSET: usize = 4;
}

/// `(size, align)` of every transcribed struct, in [`expected::SIZES`] order.
///
/// Kept as a table rather than as loose assertions so that the `const` check and
/// the `#[test]` mirror walk exactly the same list and cannot drift apart.
const ACTUAL_SIZES: [(&str, usize, usize); 17] = [
    (
        "v4l2_capability",
        size_of::<v4l2_capability>(),
        align_of::<v4l2_capability>(),
    ),
    (
        "v4l2_fmtdesc",
        size_of::<v4l2_fmtdesc>(),
        align_of::<v4l2_fmtdesc>(),
    ),
    (
        "v4l2_pix_format",
        size_of::<v4l2_pix_format>(),
        align_of::<v4l2_pix_format>(),
    ),
    (
        "v4l2_format",
        size_of::<v4l2_format>(),
        align_of::<v4l2_format>(),
    ),
    (
        "v4l2_requestbuffers",
        size_of::<v4l2_requestbuffers>(),
        align_of::<v4l2_requestbuffers>(),
    ),
    (
        "v4l2_buffer",
        size_of::<v4l2_buffer>(),
        align_of::<v4l2_buffer>(),
    ),
    (
        "v4l2_plane",
        size_of::<v4l2_plane>(),
        align_of::<v4l2_plane>(),
    ),
    (
        "v4l2_timecode",
        size_of::<v4l2_timecode>(),
        align_of::<v4l2_timecode>(),
    ),
    (
        "v4l2_timeval",
        size_of::<v4l2_timeval>(),
        align_of::<v4l2_timeval>(),
    ),
    (
        "v4l2_frmsizeenum",
        size_of::<v4l2_frmsizeenum>(),
        align_of::<v4l2_frmsizeenum>(),
    ),
    (
        "v4l2_frmsize_discrete",
        size_of::<v4l2_frmsize_discrete>(),
        align_of::<v4l2_frmsize_discrete>(),
    ),
    (
        "v4l2_frmsize_stepwise",
        size_of::<v4l2_frmsize_stepwise>(),
        align_of::<v4l2_frmsize_stepwise>(),
    ),
    (
        "v4l2_frmivalenum",
        size_of::<v4l2_frmivalenum>(),
        align_of::<v4l2_frmivalenum>(),
    ),
    (
        "v4l2_frmival_stepwise",
        size_of::<v4l2_frmival_stepwise>(),
        align_of::<v4l2_frmival_stepwise>(),
    ),
    (
        "v4l2_fract",
        size_of::<v4l2_fract>(),
        align_of::<v4l2_fract>(),
    ),
    (
        "v4l2_streamparm",
        size_of::<v4l2_streamparm>(),
        align_of::<v4l2_streamparm>(),
    ),
    (
        "v4l2_captureparm",
        size_of::<v4l2_captureparm>(),
        align_of::<v4l2_captureparm>(),
    ),
];

/// Byte offsets of the `v4l2_buffer` fields the capture loop reads, in
/// [`expected::BUFFER_OFFSETS`] order.
const ACTUAL_BUFFER_OFFSETS: [(&str, usize); 12] = [
    ("index", core::mem::offset_of!(v4l2_buffer, index)),
    ("type_", core::mem::offset_of!(v4l2_buffer, type_)),
    ("bytesused", core::mem::offset_of!(v4l2_buffer, bytesused)),
    ("flags", core::mem::offset_of!(v4l2_buffer, flags)),
    ("field", core::mem::offset_of!(v4l2_buffer, field)),
    ("timestamp", core::mem::offset_of!(v4l2_buffer, timestamp)),
    ("timecode", core::mem::offset_of!(v4l2_buffer, timecode)),
    ("sequence", core::mem::offset_of!(v4l2_buffer, sequence)),
    ("memory", core::mem::offset_of!(v4l2_buffer, memory)),
    ("m", core::mem::offset_of!(v4l2_buffer, m)),
    ("length", core::mem::offset_of!(v4l2_buffer, length)),
    ("reserved2", core::mem::offset_of!(v4l2_buffer, reserved2)),
];

const _: () = {
    let mut index = 0;
    while index < ACTUAL_SIZES.len() {
        assert!(
            ACTUAL_SIZES[index].1 == expected::SIZES[index].1,
            "a transcribed V4L2 struct has the wrong size; see the golden table"
        );
        assert!(
            ACTUAL_SIZES[index].2 == expected::SIZES[index].2,
            "a transcribed V4L2 struct has the wrong alignment; see the golden table"
        );
        index += 1;
    }

    let mut index = 0;
    while index < ACTUAL_BUFFER_OFFSETS.len() {
        assert!(
            ACTUAL_BUFFER_OFFSETS[index].1 == expected::BUFFER_OFFSETS[index].1,
            "a v4l2_buffer field moved; see the golden table"
        );
        index += 1;
    }

    assert!(core::mem::offset_of!(v4l2_format, fmt) == expected::FORMAT_FMT_OFFSET);
    assert!(core::mem::offset_of!(v4l2_streamparm, parm) == expected::STREAMPARM_PARM_OFFSET);
    assert!(size_of::<v4l2_format_fmt>() == V4L2_FORMAT_UNION_BYTES);
    assert!(size_of::<v4l2_streamparm_parm>() == V4L2_STREAMPARM_UNION_BYTES);
    // The two unions are the same size and — on a 64-bit ABI — different
    // alignments, because only one of them contains a pointer in C. That
    // asymmetry is what the two `_align` carriers exist for, so it is asserted
    // directly rather than left to fall out of the struct sizes.
    assert!(align_of::<v4l2_format_fmt>() == align_of::<usize>());
    assert!(align_of::<v4l2_streamparm_parm>() == 4);
};

#[cfg(test)]
mod tests {
    use super::*;

    // ── Golden layout ───────────────────────────────────────────────────────

    #[test]
    fn every_struct_has_the_size_and_alignment_the_kernel_abi_requires() {
        for (actual, wanted) in ACTUAL_SIZES.iter().zip(expected::SIZES.iter()) {
            assert_eq!(
                actual.0, wanted.0,
                "the two tables must list the same types"
            );
            assert_eq!(actual.1, wanted.1, "{} size", actual.0);
            assert_eq!(actual.2, wanted.2, "{} align", actual.0);
        }
    }

    #[test]
    fn the_v4l2_buffer_fields_the_capture_loop_reads_are_where_the_kernel_puts_them() {
        for (actual, wanted) in ACTUAL_BUFFER_OFFSETS
            .iter()
            .zip(expected::BUFFER_OFFSETS.iter())
        {
            assert_eq!(
                actual.0, wanted.0,
                "the two tables must list the same fields"
            );
            assert_eq!(actual.1, wanted.1, "v4l2_buffer.{}", actual.0);
        }
    }

    /// The trap this whole file is arranged around: two "u32 plus a 200-byte
    /// union" structs whose layouts differ because only one of the unions
    /// contains a pointer.
    #[test]
    fn only_the_format_union_is_pointer_aligned() {
        assert_eq!(
            size_of::<v4l2_format_fmt>(),
            size_of::<v4l2_streamparm_parm>()
        );
        assert_eq!(align_of::<v4l2_format_fmt>(), align_of::<usize>());
        assert_eq!(align_of::<v4l2_streamparm_parm>(), 4);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(size_of::<v4l2_format>(), 208);
            assert_eq!(size_of::<v4l2_streamparm>(), 204);
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            assert_eq!(size_of::<v4l2_format>(), 204);
            assert_eq!(size_of::<v4l2_streamparm>(), 204);
        }
    }

    #[test]
    fn the_timeval_width_follows_the_pointer_width() {
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<v4l2_timeval>(), 16);
        #[cfg(not(target_pointer_width = "64"))]
        assert_eq!(size_of::<v4l2_timeval>(), 8);
    }

    // ── Four-character codes ────────────────────────────────────────────────

    #[test]
    fn fourcc_packs_little_endian_ascii() {
        // Derived from `linux/videodev2.h`'s `v4l2_fourcc` macro; the byte order
        // is the opposite of Core Media's, and getting it backwards would make
        // every format comparison fail silently.
        assert_eq!(V4L2_PIX_FMT_YUYV, 0x5659_5559);
        assert_eq!(V4L2_PIX_FMT_UYVY, 0x5956_5955);
        assert_eq!(V4L2_PIX_FMT_NV12, 0x3231_564E);
        assert_eq!(V4L2_PIX_FMT_NV21, 0x3132_564E);
        assert_eq!(V4L2_PIX_FMT_YUV420, 0x3231_5559);
        assert_eq!(V4L2_PIX_FMT_YVU420, 0x3231_5659);
        assert_eq!(V4L2_PIX_FMT_RGB24, 0x3342_4752);
        assert_eq!(V4L2_PIX_FMT_BGR24, 0x3352_4742);
        assert_eq!(V4L2_PIX_FMT_MJPEG, 0x4750_4A4D);
        assert_eq!(V4L2_PIX_FMT_JPEG, 0x4745_504A);
    }

    #[test]
    fn known_codes_map_to_the_documented_encodings() {
        let table = [
            (
                V4L2_PIX_FMT_YUYV,
                CaptureEncoding::Raw(PixelFormat::Yuyv422),
            ),
            (
                V4L2_PIX_FMT_UYVY,
                CaptureEncoding::Raw(PixelFormat::Uyvy422),
            ),
            (V4L2_PIX_FMT_NV12, CaptureEncoding::Raw(PixelFormat::Nv12)),
            (V4L2_PIX_FMT_NV21, CaptureEncoding::Raw(PixelFormat::Nv21)),
            (
                V4L2_PIX_FMT_YUV420,
                CaptureEncoding::Raw(PixelFormat::Yuv420p),
            ),
            (V4L2_PIX_FMT_RGB24, CaptureEncoding::Raw(PixelFormat::Rgb24)),
            (V4L2_PIX_FMT_MJPEG, CaptureEncoding::Mjpeg),
            (V4L2_PIX_FMT_JPEG, CaptureEncoding::Mjpeg),
        ];
        for (code, expected) in table {
            assert_eq!(
                encoding_for_pixelformat(code),
                Some(expected),
                "{} mapped wrong",
                fourcc_name(code)
            );
        }
    }

    #[test]
    fn chroma_swapped_and_channel_swapped_codes_are_skipped_not_guessed() {
        // YV12 is Yuv420p with Cb and Cr exchanged and BGR3 is Rgb24 with the
        // channels reversed. Mapping either onto the format it resembles would
        // produce a capture that works and is wrong, which is worse than a
        // device that advertises one mode fewer.
        assert_eq!(encoding_for_pixelformat(V4L2_PIX_FMT_YVU420), None);
        assert_eq!(encoding_for_pixelformat(V4L2_PIX_FMT_BGR24), None);
    }

    #[test]
    fn unknown_codes_are_skipped_never_guessed() {
        for code in [
            fourcc(b"BA81"), // 8-bit Bayer
            fourcc(b"Y16 "), // 16-bit greyscale
            fourcc(b"H264"), // an encoded stream this backend cannot carry
            fourcc(b"NM12"), // NV12 multi-planar
            0,
            u32::MAX,
        ] {
            assert_eq!(
                encoding_for_pixelformat(code),
                None,
                "{} must not be guessed at",
                fourcc_name(code)
            );
        }
    }

    #[test]
    fn every_mapped_encoding_round_trips_back_to_a_code() {
        for code in [
            V4L2_PIX_FMT_YUYV,
            V4L2_PIX_FMT_UYVY,
            V4L2_PIX_FMT_NV12,
            V4L2_PIX_FMT_NV21,
            V4L2_PIX_FMT_YUV420,
            V4L2_PIX_FMT_RGB24,
            V4L2_PIX_FMT_MJPEG,
        ] {
            let encoding = encoding_for_pixelformat(code).expect("a mapped code");
            assert_eq!(
                pixelformat_for_encoding(encoding),
                Some(code),
                "{} did not round-trip",
                fourcc_name(code)
            );
        }
        // `JPEG` is the one asymmetry, and it is deliberate: both JPEG codes
        // mean MJPEG, and `MJPG` is the one to ask a device for.
        assert_eq!(
            pixelformat_for_encoding(CaptureEncoding::Mjpeg),
            Some(V4L2_PIX_FMT_MJPEG)
        );
    }

    #[test]
    fn encodings_with_no_v4l2_code_have_no_request() {
        assert_eq!(
            pixelformat_for_encoding(CaptureEncoding::Raw(PixelFormat::Gray8)),
            None
        );
        assert_eq!(
            pixelformat_for_encoding(CaptureEncoding::Raw(PixelFormat::P010)),
            None
        );
    }

    #[test]
    fn fourcc_names_are_readable_even_when_the_code_is_not() {
        assert_eq!(fourcc_name(V4L2_PIX_FMT_YUYV), "YUYV");
        assert_eq!(fourcc_name(V4L2_PIX_FMT_MJPEG), "MJPG");
        assert_eq!(fourcc_name(0), "....");
        assert_eq!(fourcc_name(fourcc(b"Y16 ")), "Y16 ");
    }

    // ── Accessors ───────────────────────────────────────────────────────────

    #[test]
    fn the_format_union_round_trips_a_pix_format() {
        let pix = v4l2_pix_format {
            width: 1280,
            height: 720,
            pixelformat: V4L2_PIX_FMT_NV12,
            field: V4L2_FIELD_NONE,
            bytesperline: 1408,
            sizeimage: 1_520_640,
            ..v4l2_pix_format::default()
        };
        let format = v4l2_format::capture(pix);
        assert_eq!(format.type_, V4L2_BUF_TYPE_VIDEO_CAPTURE);
        assert_eq!(format.pix(), pix);
    }

    #[test]
    fn writing_the_format_union_clears_whatever_was_there() {
        let mut format = v4l2_format::capture(v4l2_pix_format {
            width: 1920,
            height: 1080,
            ..v4l2_pix_format::default()
        });
        format.set_pix(v4l2_pix_format {
            width: 640,
            ..v4l2_pix_format::default()
        });
        assert_eq!(format.pix().width, 640);
        assert_eq!(
            format.pix().height,
            0,
            "the previous value must not survive"
        );
        // SAFETY: the payload is fully initialised and `raw_data` is a byte
        // array, which can read any initialised bytes.
        let tail = unsafe { format.fmt.raw_data }[size_of::<v4l2_pix_format>()..].to_vec();
        assert!(
            tail.iter().all(|&byte| byte == 0),
            "the union tail must be zeroed"
        );
    }

    #[test]
    fn the_streamparm_union_round_trips_a_captureparm() {
        let capture = v4l2_captureparm {
            capability: V4L2_CAP_TIMEPERFRAME,
            timeperframe: v4l2_fract {
                numerator: 1001,
                denominator: 30_000,
            },
            ..v4l2_captureparm::default()
        };
        let mut parm = v4l2_streamparm::query();
        parm.set_capture(capture);
        assert_eq!(parm.type_, V4L2_BUF_TYPE_VIDEO_CAPTURE);
        assert_eq!(parm.capture(), capture);
    }

    #[test]
    fn effective_caps_falls_back_when_the_kernel_did_not_fill_device_caps() {
        let old = v4l2_capability {
            capabilities: V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_STREAMING,
            device_caps: 0,
            ..v4l2_capability::default()
        };
        assert_eq!(
            old.effective_caps(),
            V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_STREAMING,
            "a pre-3.3 kernel's zero must not be read as a node that can do nothing"
        );

        // A metadata node on a modern kernel: the driver can capture, this node
        // cannot, and only `device_caps` says so.
        let metadata = v4l2_capability {
            capabilities: V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_STREAMING | V4L2_CAP_DEVICE_CAPS,
            device_caps: V4L2_CAP_STREAMING,
            ..v4l2_capability::default()
        };
        assert_eq!(metadata.effective_caps(), V4L2_CAP_STREAMING);
        assert_eq!(metadata.effective_caps() & V4L2_CAP_VIDEO_CAPTURE, 0);
    }

    #[test]
    fn an_mmap_buffer_is_built_with_the_type_and_memory_the_driver_expects() {
        let buffer = v4l2_buffer::mmap(3);
        assert_eq!(buffer.index, 3);
        assert_eq!(buffer.type_, V4L2_BUF_TYPE_VIDEO_CAPTURE);
        assert_eq!(buffer.memory, V4L2_MEMORY_MMAP);
        assert_eq!(buffer.mmap_offset(), 0);
        assert!(!buffer.is_error());
    }

    #[test]
    fn only_the_monotonic_timestamp_flag_counts_as_a_device_clock() {
        let monotonic = v4l2_buffer {
            flags: V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC,
            ..v4l2_buffer::default()
        };
        assert!(monotonic.has_monotonic_timestamp());

        for flags in [
            V4L2_BUF_FLAG_TIMESTAMP_UNKNOWN,
            V4L2_BUF_FLAG_TIMESTAMP_COPY,
            V4L2_BUF_FLAG_TIMESTAMP_MASK,
        ] {
            let buffer = v4l2_buffer {
                flags,
                ..v4l2_buffer::default()
            };
            assert!(
                !buffer.has_monotonic_timestamp(),
                "flags {flags:#x} must not be read as a device clock"
            );
        }

        // Unrelated flags in the same word must not disturb the test.
        let with_error = v4l2_buffer {
            flags: V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC | V4L2_BUF_FLAG_ERROR,
            ..v4l2_buffer::default()
        };
        assert!(with_error.has_monotonic_timestamp());
        assert!(with_error.is_error());
    }

    #[test]
    fn frame_size_accessors_answer_only_for_the_live_union_member() {
        let discrete = v4l2_frmsizeenum {
            type_: V4L2_FRMSIZE_TYPE_DISCRETE,
            size: v4l2_frmsizeenum_size {
                discrete: v4l2_frmsize_discrete {
                    width: 1280,
                    height: 720,
                },
            },
            ..v4l2_frmsizeenum::query(0, V4L2_PIX_FMT_YUYV)
        };
        assert_eq!(
            discrete.discrete(),
            Some(v4l2_frmsize_discrete {
                width: 1280,
                height: 720
            })
        );
        assert_eq!(discrete.stepwise(), None);

        let stepwise = v4l2_frmsizeenum {
            type_: V4L2_FRMSIZE_TYPE_STEPWISE,
            size: v4l2_frmsizeenum_size {
                stepwise: v4l2_frmsize_stepwise {
                    min_width: 32,
                    max_width: 1920,
                    step_width: 16,
                    min_height: 32,
                    max_height: 1080,
                    step_height: 16,
                },
            },
            ..v4l2_frmsizeenum::query(1, V4L2_PIX_FMT_YUYV)
        };
        assert_eq!(stepwise.discrete(), None);
        assert_eq!(stepwise.stepwise().map(|s| s.max_width), Some(1920));

        // `CONTINUOUS` uses the same member with unit steps.
        let continuous = v4l2_frmsizeenum {
            type_: V4L2_FRMSIZE_TYPE_CONTINUOUS,
            ..stepwise
        };
        assert!(continuous.stepwise().is_some());
        assert_eq!(continuous.discrete(), None);

        // An unknown type is neither.
        let unknown = v4l2_frmsizeenum {
            type_: 99,
            ..stepwise
        };
        assert_eq!(unknown.discrete(), None);
        assert_eq!(unknown.stepwise(), None);
    }

    #[test]
    fn frame_interval_accessors_answer_only_for_the_live_union_member() {
        let discrete = v4l2_frmivalenum {
            type_: V4L2_FRMIVAL_TYPE_DISCRETE,
            interval: v4l2_frmivalenum_interval {
                discrete: v4l2_fract {
                    numerator: 1001,
                    denominator: 30_000,
                },
            },
            ..v4l2_frmivalenum::query(0, V4L2_PIX_FMT_YUYV, 1280, 720)
        };
        assert_eq!(
            discrete.discrete(),
            Some(v4l2_fract {
                numerator: 1001,
                denominator: 30_000
            })
        );
        assert_eq!(discrete.stepwise(), None);

        let stepwise = v4l2_frmivalenum {
            type_: V4L2_FRMIVAL_TYPE_STEPWISE,
            interval: v4l2_frmivalenum_interval {
                stepwise: v4l2_frmival_stepwise {
                    min: v4l2_fract {
                        numerator: 1,
                        denominator: 60,
                    },
                    max: v4l2_fract {
                        numerator: 1,
                        denominator: 5,
                    },
                    step: v4l2_fract {
                        numerator: 1,
                        denominator: 1000,
                    },
                },
            },
            ..v4l2_frmivalenum::query(1, V4L2_PIX_FMT_YUYV, 1280, 720)
        };
        assert_eq!(stepwise.discrete(), None);
        assert_eq!(
            stepwise.stepwise().map(|s| s.min),
            Some(v4l2_fract {
                numerator: 1,
                denominator: 60
            })
        );
        assert_eq!(
            v4l2_frmivalenum {
                type_: 0,
                ..stepwise
            }
            .stepwise(),
            None
        );
    }

    #[test]
    fn enumeration_queries_carry_the_index_and_code_the_kernel_needs() {
        let sizes = v4l2_frmsizeenum::query(2, V4L2_PIX_FMT_MJPEG);
        assert_eq!((sizes.index, sizes.pixel_format), (2, V4L2_PIX_FMT_MJPEG));
        assert_eq!(sizes.reserved, [0; 2]);

        let intervals = v4l2_frmivalenum::query(1, V4L2_PIX_FMT_MJPEG, 640, 480);
        assert_eq!(
            (
                intervals.index,
                intervals.pixel_format,
                intervals.width,
                intervals.height
            ),
            (1, V4L2_PIX_FMT_MJPEG, 640, 480)
        );
        assert_eq!(intervals.reserved, [0; 2]);
    }

    #[test]
    fn a_v4l2_buffer_is_send_because_the_union_holds_no_pointer() {
        // The runner is built on the caller's thread and moved to the capture
        // thread; a raw pointer in `union m` would make that impossible.
        const fn assert_send<T: Send>() {}
        assert_send::<v4l2_buffer>();
        assert_send::<v4l2_format>();
        assert_send::<v4l2_streamparm>();
    }
}

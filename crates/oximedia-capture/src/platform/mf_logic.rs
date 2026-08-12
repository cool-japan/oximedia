//! The pure half of the Media Foundation capture backend.
//!
//! Everything in this module is arithmetic over plain integers and byte
//! slices: how Media Foundation packs a frame size and a frame rate into a
//! `u64` attribute, how `MF_MT_DEFAULT_STRIDE` describes the row layout of a
//! locked buffer, how a contiguous buffer becomes a [`VideoFrame`], and which
//! `HRESULT`s end a capture session rather than being retried. It names no
//! Windows type and calls no Windows function.
//!
//! # Why it is not inside `platform::windows`
//!
//! `platform::windows` is `cfg(target_os = "windows")`, so on any other host it
//! is not compiled and its tests cannot run. The decisions in this module are
//! exactly the ones worth testing — a stride misread by one row, a bottom-up
//! image delivered upside down, an unknown `HRESULT` retried forever — and they
//! are all expressible without a camera or a Windows box. Hoisting them here,
//! under
//!
//! ```text
//! #[cfg(any(test, target_os = "windows"))]
//! ```
//!
//! makes them part of the host test suite on every platform while keeping them
//! out of a non-Windows *library* build, where they would be dead code.
//!
//! # Destination convention
//!
//! Delivered planes follow the same convention as the AVFoundation backend and
//! the rest of the workspace:
//! [`PixelFormat::stride_for_width`](oximedia_core::PixelFormat::stride_for_width)
//! for the row length and
//! [`VideoFrame::plane_dimensions`](oximedia_codec::VideoFrame::plane_dimensions)
//! for the row count, so a delivered frame's total size is
//! [`PixelFormat::frame_buffer_size`](oximedia_core::PixelFormat::frame_buffer_size).
//! No [`BufferPool`](crate::pool::BufferPool) staging: a plane owns its `Vec`
//! and is handed to the consumer, so it can never be returned to a pool, and
//! copying through one would double the per-frame traffic to save an allocation
//! `Vec::with_capacity` already makes cheap. One copy, straight out of the
//! locked buffer.

use oximedia_codec::{Plane, VideoFrame};
use oximedia_core::PixelFormat;

use super::common::gcd;
use crate::device::CaptureFormat;
use crate::error::CaptureError;

// ── Packed 64-bit attributes ─────────────────────────────────────────────────

/// Split a Media Foundation `u64` attribute into its two 32-bit halves.
///
/// `MF_MT_FRAME_SIZE` and `MF_MT_FRAME_RATE` are both stored as
/// `(high << 32) | low` — width/height for the first, numerator/denominator for
/// the second.
const fn unpack_pair(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)
}

/// Join two 32-bit halves the way Media Foundation stores them.
const fn pack_pair(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | (low as u64)
}

/// Frame size from an `MF_MT_FRAME_SIZE` attribute.
///
/// `None` when either edge is zero, which is not a mode any device can be
/// driven in and must therefore not be reported as one.
pub(crate) const fn unpack_frame_size(packed: u64) -> Option<(u32, u32)> {
    let (width, height) = unpack_pair(packed);
    if width == 0 || height == 0 {
        None
    } else {
        Some((width, height))
    }
}

/// Build an `MF_MT_FRAME_SIZE` attribute value.
pub(crate) const fn pack_frame_size(width: u32, height: u32) -> u64 {
    pack_pair(width, height)
}

/// Exact rational frame rate from an `MF_MT_FRAME_RATE` attribute.
///
/// The result is reduced to lowest terms. Media Foundation reports NTSC as
/// `30000/1001` already, but a device is free to say `60000/2002`, and
/// [`CaptureFormat`] derives `PartialEq` field by field — so an unreduced pair
/// would make two descriptions of one mode compare unequal, defeating the
/// duplicate filter in enumeration and putting `60000/2002` in front of users.
/// Dividing both terms by their GCD cannot change the value the pair denotes.
///
/// `None` when either term is zero. [`CaptureFormat`] documents `fps_den == 0`
/// as "unknown / variable", and that is what the caller records instead of a
/// guess.
pub(crate) fn unpack_frame_rate(packed: u64) -> Option<(u32, u32)> {
    let (numerator, denominator) = unpack_pair(packed);
    if numerator == 0 || denominator == 0 {
        return None;
    }
    let divisor = gcd(numerator, denominator).max(1);
    Some((numerator / divisor, denominator / divisor))
}

/// Build an `MF_MT_FRAME_RATE` attribute value.
pub(crate) const fn pack_frame_rate(fps_num: u32, fps_den: u32) -> u64 {
    pack_pair(fps_num, fps_den)
}

// ── Echo check ───────────────────────────────────────────────────────────────

/// Does the mode a reader settled on satisfy the mode that was asked for?
///
/// Encoding, width and height must match exactly: a reader that quietly chose a
/// different size or subtype would make every delivered frame's own description
/// of itself false.
///
/// The frame rate must match too — *unless* the request carried
/// [`CaptureFormat`]'s documented "unknown / variable" pair, `fps_den == 0`,
/// which is what enumeration records for a device that reported no usable rate.
/// Nothing was asked for in that case, so nothing can disagree, and whatever
/// the reader reports becomes the answer rather than a rejection. Requiring
/// equality there would make such a mode permanently unopenable.
pub(crate) fn satisfies(requested: CaptureFormat, settled: CaptureFormat) -> bool {
    if settled.encoding != requested.encoding
        || settled.width != requested.width
        || settled.height != requested.height
    {
        return false;
    }
    requested.fps_den == 0
        || (settled.fps_num == requested.fps_num && settled.fps_den == requested.fps_den)
}

// ── Row layout ───────────────────────────────────────────────────────────────

/// How the rows of one image sit inside a locked Media Foundation buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowOrder {
    /// Distance in bytes between the starts of two consecutive rows in memory.
    ///
    /// Always positive; the sign of `MF_MT_DEFAULT_STRIDE` is carried by
    /// [`Self::bottom_up`] instead.
    pub(crate) stride: usize,
    /// `true` when the first row in memory is the *bottom* row of the picture.
    ///
    /// This is what a negative `MF_MT_DEFAULT_STRIDE` means. [`Plane`] has no
    /// way to express a bottom-up image, so the copy re-orders the rows and
    /// what the consumer receives is always top-down.
    pub(crate) bottom_up: bool,
}

/// Interpret an `MF_MT_DEFAULT_STRIDE` attribute.
///
/// The attribute is a `UINT32` that holds a *signed* value:
///
/// * absent (`None`) — the media type does not carry one, which Media
///   Foundation documents as a tightly packed buffer, so the row length is
///   `packed_row_bytes`;
/// * positive — a top-down image with that many bytes per row, padding
///   included;
/// * negative — a bottom-up image whose rows are `-stride` bytes apart. This is
///   the normal shape of uncompressed RGB on Windows, inherited from DIBs.
///
/// A stride of exactly zero is `None`: no image has rows of no length, and
/// treating it as "packed" would hide a driver that reported nonsense.
pub(crate) const fn row_order(
    default_stride: Option<i32>,
    packed_row_bytes: usize,
) -> Option<RowOrder> {
    match default_stride {
        None => Some(RowOrder {
            stride: packed_row_bytes,
            bottom_up: false,
        }),
        Some(0) => None,
        Some(value) => Some(RowOrder {
            // `unsigned_abs` widens to `u32`, which is lossless into `usize` on
            // every target this crate builds for.
            stride: value.unsigned_abs() as usize,
            bottom_up: value < 0,
        }),
    }
}

/// Bytes in one *meaningful* row of `format`, ignoring any padding.
///
/// This is the row length a packed buffer has, and the destination row length
/// of the first plane. `None` for a compressed encoding, which has no rows, and
/// for a pixel layout this backend has no copy path for.
pub(crate) fn packed_row_bytes(format: CaptureFormat) -> Option<usize> {
    format.pixel()?.stride_for_width(format.width, 0)
}

// ── Copying ──────────────────────────────────────────────────────────────────

/// Copy `rows` rows of `row_bytes` meaningful bytes each out of `source`.
///
/// `offset` is where the plane starts in the buffer, `order` says how far apart
/// its rows are and in which direction they run. Padding between rows is never
/// copied: it is whatever the previous tenant of that memory left behind, not
/// image data.
///
/// Returns `None` rather than reading out of bounds when the row length exceeds
/// the stride, when the buffer is too short for the geometry, or when the
/// arithmetic overflows. Only the last row has to be complete — a tightly sized
/// buffer may end immediately after it, without its trailing padding.
fn copy_rows(
    source: &[u8],
    offset: usize,
    order: RowOrder,
    row_bytes: usize,
    rows: usize,
) -> Option<Vec<u8>> {
    if row_bytes > order.stride {
        return None;
    }
    let capacity = row_bytes.checked_mul(rows)?;
    if rows > 0 {
        let last_row = order.stride.checked_mul(rows - 1)?;
        let needed = offset.checked_add(last_row)?.checked_add(row_bytes)?;
        if needed > source.len() {
            return None;
        }
    }

    let mut data = Vec::with_capacity(capacity);
    for row in 0..rows {
        // A bottom-up buffer holds the picture's last row first, so walking the
        // destination top to bottom means walking the source bottom to top.
        let index = if order.bottom_up { rows - 1 - row } else { row };
        let start = offset + index * order.stride;
        data.extend_from_slice(source.get(start..start + row_bytes)?);
    }
    Some(data)
}

// ── Plane geometry ───────────────────────────────────────────────────────────

/// The destination shape of one plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlaneShape {
    /// Meaningful bytes in one row.
    stride: usize,
    /// Number of rows.
    rows: usize,
    /// Plane width in pixels, as [`Plane::width`] records it.
    width: u32,
    /// Plane height in pixels, as [`Plane::height`] records it.
    height: u32,
}

/// Destination geometry for `pixel` at `width` × `height`.
///
/// `None` for a layout with no copy path here, which is every layout
/// [`super::windows::encoding_for_subtype`] does not produce.
fn plane_shapes(pixel: PixelFormat, width: u32, height: u32) -> Option<Vec<PlaneShape>> {
    if width == 0 || height == 0 {
        return None;
    }
    // `plane_dimensions` reads the frame's chroma ratios; `new` allocates
    // nothing.
    let geometry = VideoFrame::new(pixel, width, height);
    let mut shapes = Vec::with_capacity(pixel.plane_count() as usize);
    for plane in 0..pixel.plane_count() {
        let stride = pixel.stride_for_width(width, plane)?;
        let (plane_width, plane_height) = geometry.plane_dimensions(plane as usize);
        shapes.push(PlaneShape {
            stride,
            rows: plane_height as usize,
            width: plane_width,
            height: plane_height,
        });
    }
    Some(shapes)
}

// ── Frames ───────────────────────────────────────────────────────────────────

/// Build a [`VideoFrame`] from one contiguous Media Foundation buffer.
///
/// `IMFSample::ConvertToContiguousBuffer` hands back a single buffer holding
/// every plane back to back: for NV12 that is the luma plane, `stride × height`
/// bytes of it, immediately followed by the interleaved chroma plane at the
/// same stride. Both planes are copied row by row so that neither the source
/// padding nor a bottom-up row order reaches the consumer.
///
/// # Errors
///
/// [`CaptureError::FormatRejected`] when the buffer cannot hold the negotiated
/// geometry, when the encoding has no copy path here, or when a multi-plane
/// format arrives bottom-up — Media Foundation only ever inverts packed RGB,
/// and guessing at where the planes of an inverted semi-planar buffer start
/// would deliver mis-ordered chroma rather than an error.
pub(crate) fn video_frame_from_contiguous(
    source: &[u8],
    format: CaptureFormat,
    order: RowOrder,
    device: &str,
) -> Result<VideoFrame, CaptureError> {
    let Some(pixel) = format.pixel() else {
        return Err(CaptureError::format_rejected(
            device,
            format,
            "compressed encodings are delivered as a bitstream, not as planes",
        ));
    };
    let Some(shapes) = plane_shapes(pixel, format.width, format.height) else {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!("no plane geometry for {pixel:?}"),
        ));
    };
    if order.bottom_up && shapes.len() > 1 {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!(
                "a bottom-up buffer cannot describe {}-plane {pixel:?}",
                shapes.len()
            ),
        ));
    }

    let mut planes = Vec::with_capacity(shapes.len());
    let mut offset = 0_usize;
    for (index, shape) in shapes.iter().enumerate() {
        let Some(data) = copy_rows(source, offset, order, shape.stride, shape.rows) else {
            return Err(CaptureError::format_rejected(
                device,
                format,
                format!(
                    "buffer of {} byte(s) cannot hold plane {index}: {} row(s) of {} byte(s) \
                     at stride {} from offset {offset}",
                    source.len(),
                    shape.rows,
                    shape.stride,
                    order.stride
                ),
            ));
        };
        planes.push(Plane::with_dimensions(
            data,
            shape.stride,
            shape.width,
            shape.height,
        ));
        // The next plane begins where this one's *source* rows end, which is
        // the padded stride times its row count, not the destination size.
        let Some(next) = order
            .stride
            .checked_mul(shape.rows)
            .and_then(|span| offset.checked_add(span))
        else {
            return Err(CaptureError::format_rejected(
                device,
                format,
                format!("plane {index} geometry does not fit in memory"),
            ));
        };
        offset = next;
    }

    let mut frame = VideoFrame::new(pixel, format.width, format.height);
    frame.planes = planes;
    Ok(frame)
}

// ── HRESULT classification ───────────────────────────────────────────────────

/// `MF_E_INVALIDREQUEST`.
pub(crate) const MF_E_INVALIDREQUEST: i32 = 0xC00D_36B2_u32 as i32;
/// `MF_E_HW_MFT_FAILED_START_STREAMING`.
pub(crate) const MF_E_HW_MFT_FAILED_START_STREAMING: i32 = 0xC00D_3704_u32 as i32;
/// `MF_E_HW_STREAM_NOT_CONNECTED`.
pub(crate) const MF_E_HW_STREAM_NOT_CONNECTED: i32 = 0xC00D_A7FD_u32 as i32;
/// `MF_E_SHUTDOWN`.
pub(crate) const MF_E_SHUTDOWN: i32 = 0xC00D_3E85_u32 as i32;
/// `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED`.
pub(crate) const MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED: i32 = 0xC00D_3EA2_u32 as i32;
/// `MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED`.
pub(crate) const MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED: i32 = 0xC00D_3EA3_u32 as i32;

/// How many consecutive reads may produce no frame before the loop parks.
///
/// See [`should_park`].
pub(crate) const FRAMELESS_READS_BEFORE_PARK: u32 = 2;

/// Should a read loop that has produced no frame `n` times in a row park before
/// trying again?
///
/// `ReadSample` normally blocks until the next frame, so the loop runs at the
/// device's pace and never reaches this. But several outcomes return
/// *immediately* with no frame — a stream tick, a success with no sample, a
/// retryable error, a buffer that could not be copied — and a source that has
/// stopped producing without saying so can hand back one of those forever. Left
/// alone that is a hot loop: a pinned core, and in the stream-tick case
/// [`CaptureStats::device_dropped`](crate::CaptureStats::device_dropped)
/// inflated by thousands per second, which would read as a hardware fault
/// rather than as the stall it is.
///
/// A small run is tolerated without parking, because a device legitimately
/// interleaves the occasional tick with real frames and delaying those would
/// add latency to a healthy stream.
pub(crate) const fn should_park(consecutive_frameless_reads: u32) -> bool {
    consecutive_frameless_reads > FRAMELESS_READS_BEFORE_PARK
}

/// Whether a failed `ReadSample` ends the session or is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fatality {
    /// The loop reports the error and reads again.
    Transient,
    /// The loop stops and the session ends with this error.
    Fatal,
}

/// Classify an `HRESULT` returned by `IMFSourceReader::ReadSample`.
///
/// The transient set is an explicit allow-list, and **the default is
/// [`Fatality::Fatal`]**. That asymmetry is deliberate: an unrecognized failure
/// treated as transient becomes a capture thread spinning on a device that will
/// never produce another frame, reporting the same error thousands of times a
/// second while [`crate::CaptureSession`] still looks healthy. Ending the
/// session is recoverable — the caller can reopen — and it is the honest report
/// that something the backend does not understand went wrong.
///
/// The fatal set names the four ways a camera goes away underneath a running
/// reader, plus the shutdown case:
///
/// | `HRESULT` | Meaning |
/// |---|---|
/// | `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED` | the device was unplugged, disabled, or its driver restarted |
/// | `MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED` | another process took exclusive control |
/// | `MF_E_HW_MFT_FAILED_START_STREAMING` | the capture pipeline could not stream and will not recover by retrying |
/// | `MF_E_HW_STREAM_NOT_CONNECTED` | the stream this reader was reading is no longer connected |
/// | `MF_E_SHUTDOWN` | the media source has been shut down |
///
/// `MF_E_INVALIDREQUEST` is the one documented recoverable case: it reports a
/// request the reader could not service *at that moment*, and the next read is
/// free to succeed.
pub(crate) const fn classify_read_error(code: i32) -> Fatality {
    match code {
        MF_E_INVALIDREQUEST => Fatality::Transient,
        _ => Fatality::Fatal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::CaptureEncoding;

    /// Marker written into padding, so a copy that ignored the stride shows up.
    const PADDING: u8 = 0xEE;

    fn format(pixel: PixelFormat, width: u32, height: u32) -> CaptureFormat {
        CaptureFormat::new(CaptureEncoding::Raw(pixel), width, height, 30, 1)
    }

    /// Byte a test expects at `(plane, row, column)`.
    fn sample(plane: usize, row: usize, column: usize) -> u8 {
        u8::try_from((plane * 97 + row * 7 + column * 3) % 233).unwrap_or(0)
    }

    // ── Packed attributes ───────────────────────────────────────────────────

    #[test]
    fn frame_size_lives_in_the_two_halves_of_the_attribute() {
        assert_eq!(
            unpack_frame_size(pack_frame_size(1920, 1080)),
            Some((1920, 1080))
        );
        assert_eq!(unpack_frame_size((1280 << 32) | 720), Some((1280, 720)));
        assert_eq!(
            unpack_frame_size(pack_frame_size(u32::MAX, u32::MAX)),
            Some((u32::MAX, u32::MAX)),
            "no bits are lost in either direction"
        );
    }

    #[test]
    fn a_zero_edge_is_not_a_frame_size() {
        assert_eq!(unpack_frame_size(pack_frame_size(0, 1080)), None);
        assert_eq!(unpack_frame_size(pack_frame_size(1920, 0)), None);
        assert_eq!(unpack_frame_size(0), None);
    }

    #[test]
    fn frame_rates_round_trip_and_reduce() {
        assert_eq!(unpack_frame_rate(pack_frame_rate(30, 1)), Some((30, 1)));
        assert_eq!(
            unpack_frame_rate(pack_frame_rate(30_000, 1001)),
            Some((30_000, 1001))
        );
        // The same rate written twice as large.
        assert_eq!(
            unpack_frame_rate(pack_frame_rate(60_000, 2002)),
            Some((30_000, 1001))
        );
        assert_eq!(
            unpack_frame_rate(pack_frame_rate(30_000_000, 1_000_000)),
            Some((30, 1))
        );
    }

    #[test]
    fn reduction_never_changes_the_rate_it_denotes() {
        for (numerator, denominator) in [
            (30_u32, 1_u32),
            (60_000, 2002),
            (30_000, 1001),
            (15_000_000, 500_000),
            (7, 13),
        ] {
            let (fps_num, fps_den) =
                unpack_frame_rate(pack_frame_rate(numerator, denominator)).expect("a real rate");
            assert_eq!(
                u64::from(fps_num) * u64::from(denominator),
                u64::from(fps_den) * u64::from(numerator),
                "{fps_num}/{fps_den} is not {numerator}/{denominator}"
            );
            assert_eq!(gcd(fps_num, fps_den), 1, "{fps_num}/{fps_den} not reduced");
        }
    }

    #[test]
    fn a_zero_term_is_an_unknown_rate_not_a_rate_of_zero() {
        assert_eq!(unpack_frame_rate(pack_frame_rate(0, 1)), None);
        assert_eq!(unpack_frame_rate(pack_frame_rate(30, 0)), None);
        assert_eq!(unpack_frame_rate(0), None);
    }

    // ── Echo check ──────────────────────────────────────────────────────────

    #[test]
    fn an_exact_echo_satisfies_the_request() {
        let requested = format(PixelFormat::Nv12, 1280, 720);
        assert!(satisfies(requested, requested));
    }

    #[test]
    fn a_different_size_subtype_or_rate_does_not_satisfy() {
        let requested = format(PixelFormat::Nv12, 1280, 720);
        for settled in [
            format(PixelFormat::Nv12, 640, 720),
            format(PixelFormat::Nv12, 1280, 480),
            format(PixelFormat::Yuyv422, 1280, 720),
            CaptureFormat::new(CaptureEncoding::Mjpeg, 1280, 720, 30, 1),
            CaptureFormat::new(
                CaptureEncoding::Raw(PixelFormat::Nv12),
                1280,
                720,
                30_000,
                1001,
            ),
        ] {
            assert!(
                !satisfies(requested, settled),
                "{settled} must not satisfy {requested}"
            );
        }
    }

    /// The case that would otherwise make a mode with no advertised frame rate
    /// permanently unopenable: nothing was asked for, so nothing disagrees.
    #[test]
    fn an_unknown_requested_rate_accepts_whatever_the_reader_reports() {
        let requested = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 640, 480, 0, 0);
        for settled in [
            CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 640, 480, 30, 1),
            CaptureFormat::new(
                CaptureEncoding::Raw(PixelFormat::Nv12),
                640,
                480,
                30_000,
                1001,
            ),
            requested,
        ] {
            assert!(satisfies(requested, settled), "{settled}");
        }
        // The size still has to match, even with the rate left open.
        assert!(!satisfies(
            requested,
            CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 320, 240, 30, 1)
        ));
    }

    #[test]
    fn a_requested_rate_is_not_satisfied_by_an_unknown_one() {
        let requested = format(PixelFormat::Nv12, 640, 480);
        let settled = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 640, 480, 0, 0);
        assert!(
            !satisfies(requested, settled),
            "asking for 30 fps and being told nothing is a mismatch"
        );
    }

    // ── Row order ───────────────────────────────────────────────────────────

    #[test]
    fn an_absent_stride_means_a_packed_buffer() {
        assert_eq!(
            row_order(None, 1280),
            Some(RowOrder {
                stride: 1280,
                bottom_up: false
            })
        );
    }

    #[test]
    fn a_positive_stride_is_top_down_padding_included() {
        assert_eq!(
            row_order(Some(2048), 1920),
            Some(RowOrder {
                stride: 2048,
                bottom_up: false
            })
        );
    }

    #[test]
    fn a_negative_stride_is_bottom_up() {
        assert_eq!(
            row_order(Some(-1920), 1920),
            Some(RowOrder {
                stride: 1920,
                bottom_up: true
            })
        );
        // i32::MIN has no positive counterpart; `unsigned_abs` still answers.
        assert_eq!(
            row_order(Some(i32::MIN), 4),
            Some(RowOrder {
                stride: 2_147_483_648,
                bottom_up: true
            })
        );
    }

    #[test]
    fn a_zero_stride_is_rejected_rather_than_read_as_packed() {
        assert_eq!(row_order(Some(0), 1280), None);
    }

    #[test]
    fn packed_row_bytes_follow_the_workspace_convention() {
        assert_eq!(
            packed_row_bytes(format(PixelFormat::Nv12, 1280, 720)),
            Some(1280)
        );
        assert_eq!(
            packed_row_bytes(format(PixelFormat::Yuyv422, 640, 480)),
            Some(1280)
        );
        assert_eq!(
            packed_row_bytes(format(PixelFormat::Uyvy422, 640, 480)),
            Some(1280)
        );
        assert_eq!(
            packed_row_bytes(CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 1)),
            None,
            "a bitstream has no rows"
        );
    }

    // ── Row copying ─────────────────────────────────────────────────────────

    /// A plane whose padding differs from its payload.
    fn padded_plane(stride: usize, rows: usize, payload: usize, plane: usize) -> Vec<u8> {
        let mut buffer = vec![PADDING; stride * rows];
        for row in 0..rows {
            for column in 0..payload {
                buffer[row * stride + column] = sample(plane, row, column);
            }
        }
        buffer
    }

    #[test]
    fn padded_rows_are_copied_without_their_padding() {
        let source = padded_plane(96, 4, 64, 0);
        let order = RowOrder {
            stride: 96,
            bottom_up: false,
        };
        let data = copy_rows(&source, 0, order, 64, 4).expect("a padded plane copies");
        assert_eq!(data.len(), 64 * 4);
        assert!(!data.contains(&PADDING), "padding leaked into the frame");
        for row in 0..4_usize {
            for column in 0..64_usize {
                assert_eq!(
                    data[row * 64 + column],
                    sample(0, row, column),
                    "{row},{column}"
                );
            }
        }
    }

    #[test]
    fn an_unpadded_plane_copies_verbatim() {
        let source = padded_plane(32, 8, 32, 0);
        let order = RowOrder {
            stride: 32,
            bottom_up: false,
        };
        assert_eq!(copy_rows(&source, 0, order, 32, 8), Some(source.clone()));
    }

    #[test]
    fn a_bottom_up_buffer_is_turned_the_right_way_up() {
        // Four rows, each filled with its own index: reversing them is visible.
        let mut source = vec![0_u8; 4 * 8];
        for row in 0..4_usize {
            for column in 0..8_usize {
                source[row * 8 + column] = row as u8;
            }
        }
        let order = RowOrder {
            stride: 8,
            bottom_up: true,
        };
        let data = copy_rows(&source, 0, order, 8, 4).expect("a bottom-up plane copies");
        // The buffer's first row is the picture's last, so the output starts
        // with row 3.
        assert_eq!(&data[0..8], &[3_u8; 8]);
        assert_eq!(&data[8..16], &[2_u8; 8]);
        assert_eq!(&data[16..24], &[1_u8; 8]);
        assert_eq!(&data[24..32], &[0_u8; 8]);
    }

    #[test]
    fn a_bottom_up_copy_is_its_own_inverse() {
        let source = padded_plane(40, 6, 32, 1);
        let bottom_up = RowOrder {
            stride: 40,
            bottom_up: true,
        };
        let top_down = RowOrder {
            stride: 32,
            bottom_up: true,
        };
        let once = copy_rows(&source, 0, bottom_up, 32, 6).expect("first flip");
        let twice = copy_rows(&once, 0, top_down, 32, 6).expect("second flip");
        let straight = copy_rows(
            &source,
            0,
            RowOrder {
                stride: 40,
                bottom_up: false,
            },
            32,
            6,
        )
        .expect("no flip");
        assert_eq!(twice, straight, "flipping twice must be the identity");
    }

    #[test]
    fn a_plane_at_an_offset_starts_where_it_is_told() {
        let mut source = vec![PADDING; 16 * 4];
        for column in 0..8_usize {
            source[16 * 2 + column] = 0xAB;
        }
        let order = RowOrder {
            stride: 16,
            bottom_up: false,
        };
        let data = copy_rows(&source, 16 * 2, order, 8, 1).expect("an offset plane copies");
        assert_eq!(data, vec![0xAB_u8; 8]);
    }

    #[test]
    fn a_row_longer_than_the_stride_is_refused() {
        let source = vec![0_u8; 64];
        let order = RowOrder {
            stride: 8,
            bottom_up: false,
        };
        assert_eq!(copy_rows(&source, 0, order, 16, 4), None);
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_read_past() {
        let source = vec![0_u8; 8 * 3];
        let order = RowOrder {
            stride: 8,
            bottom_up: false,
        };
        assert_eq!(
            copy_rows(&source, 0, order, 8, 4),
            None,
            "three rows, four wanted"
        );
        assert_eq!(copy_rows(&source, 0, order, 8, 3), Some(source.clone()));
    }

    #[test]
    fn only_the_last_row_has_to_be_complete() {
        // A tightly sized buffer ends right after the final row's payload, with
        // no trailing padding. That is a valid buffer and must copy.
        let source = vec![7_u8; 12 * 2 + 8];
        let order = RowOrder {
            stride: 12,
            bottom_up: false,
        };
        assert_eq!(
            copy_rows(&source, 0, order, 8, 3).map(|d| d.len()),
            Some(24)
        );
    }

    #[test]
    fn an_impossible_geometry_is_none_not_a_capacity_panic() {
        let source = vec![0_u8; 8];
        let order = RowOrder {
            stride: usize::MAX,
            bottom_up: false,
        };
        assert_eq!(copy_rows(&source, 0, order, usize::MAX, 4), None);
    }

    #[test]
    fn zero_rows_copy_to_an_empty_buffer() {
        let source = vec![0_u8; 8];
        let order = RowOrder {
            stride: 8,
            bottom_up: false,
        };
        assert_eq!(copy_rows(&source, 0, order, 8, 0), Some(Vec::new()));
    }

    // ── Plane geometry ──────────────────────────────────────────────────────

    #[test]
    fn nv12_is_two_planes_with_a_full_width_chroma_stride() {
        let shapes = plane_shapes(PixelFormat::Nv12, 1920, 1080).expect("nv12");
        assert_eq!(shapes.len(), 2);
        assert_eq!((shapes[0].stride, shapes[0].rows), (1920, 1080));
        // Interleaved Cb/Cr: 960 pairs of 2 bytes = 1920 bytes per row.
        assert_eq!((shapes[1].stride, shapes[1].rows), (1920, 540));
    }

    #[test]
    fn plane_geometry_sums_to_the_workspace_frame_size() {
        for (pixel, width, height) in [
            (PixelFormat::Nv12, 1280_u32, 720_u32),
            (PixelFormat::Yuyv422, 640, 480),
            (PixelFormat::Uyvy422, 320, 240),
        ] {
            let shapes = plane_shapes(pixel, width, height).expect("a supported layout");
            let total: usize = shapes.iter().map(|shape| shape.stride * shape.rows).sum();
            assert_eq!(total, pixel.frame_buffer_size(width, height), "{pixel:?}");
        }
    }

    #[test]
    fn zero_dimensions_have_no_geometry() {
        assert!(plane_shapes(PixelFormat::Nv12, 0, 480).is_none());
        assert!(plane_shapes(PixelFormat::Nv12, 640, 0).is_none());
    }

    // ── Whole frames ────────────────────────────────────────────────────────

    /// Build a contiguous NV12 buffer the way Media Foundation lays one out.
    fn nv12_buffer(stride: usize, width: usize, height: usize) -> Vec<u8> {
        let chroma_rows = height.div_ceil(2);
        let mut buffer = vec![PADDING; stride * (height + chroma_rows)];
        for row in 0..height {
            for column in 0..width {
                buffer[row * stride + column] = sample(0, row, column);
            }
        }
        for row in 0..chroma_rows {
            for column in 0..width {
                buffer[stride * height + row * stride + column] = sample(1, row, column);
            }
        }
        buffer
    }

    #[test]
    fn nv12_splits_into_luma_and_interleaved_chroma() {
        // A padded stride, so a copy that assumed `width` would be visible.
        let source = nv12_buffer(80, 64, 32);
        let order = RowOrder {
            stride: 80,
            bottom_up: false,
        };
        let frame =
            video_frame_from_contiguous(&source, format(PixelFormat::Nv12, 64, 32), order, "test")
                .expect("nv12 copies");

        assert_eq!(frame.format, PixelFormat::Nv12);
        assert_eq!((frame.width, frame.height), (64, 32));
        assert_eq!(frame.planes.len(), 2);
        assert_eq!(
            frame.size_bytes(),
            PixelFormat::Nv12.frame_buffer_size(64, 32)
        );
        for (index, plane) in frame.planes.iter().enumerate() {
            assert!(
                !plane.data.contains(&PADDING),
                "plane {index} leaked padding"
            );
        }
        assert_eq!(frame.planes[0].data.len(), 64 * 32);
        assert_eq!(frame.planes[1].data.len(), 64 * 16);
        for row in 0..32_usize {
            for column in 0..64_usize {
                assert_eq!(
                    frame.planes[0].data[row * 64 + column],
                    sample(0, row, column)
                );
            }
        }
        for row in 0..16_usize {
            for column in 0..64_usize {
                assert_eq!(
                    frame.planes[1].data[row * 64 + column],
                    sample(1, row, column)
                );
            }
        }
    }

    #[test]
    fn an_unpadded_nv12_buffer_copies_too() {
        let source = nv12_buffer(64, 64, 32);
        let order = RowOrder {
            stride: 64,
            bottom_up: false,
        };
        let frame =
            video_frame_from_contiguous(&source, format(PixelFormat::Nv12, 64, 32), order, "test")
                .expect("nv12 copies");
        assert_eq!(
            frame.size_bytes(),
            PixelFormat::Nv12.frame_buffer_size(64, 32)
        );
    }

    #[test]
    fn packed_422_is_one_plane_of_two_bytes_per_pixel() {
        for pixel in [PixelFormat::Yuyv422, PixelFormat::Uyvy422] {
            let source = padded_plane(160, 24, 128, 0);
            let order = RowOrder {
                stride: 160,
                bottom_up: false,
            };
            let frame = video_frame_from_contiguous(&source, format(pixel, 64, 24), order, "test")
                .unwrap_or_else(|error| panic!("{pixel:?}: {error}"));
            assert_eq!(frame.planes.len(), 1, "{pixel:?}");
            assert_eq!(frame.planes[0].stride, 128, "{pixel:?}");
            assert_eq!(
                frame.size_bytes(),
                pixel.frame_buffer_size(64, 24),
                "{pixel:?}"
            );
            assert!(!frame.planes[0].data.contains(&PADDING), "{pixel:?}");
        }
    }

    #[test]
    fn a_bottom_up_packed_frame_arrives_top_down() {
        let mut source = vec![0_u8; 4 * 8];
        for row in 0..4_usize {
            for column in 0..8_usize {
                source[row * 8 + column] = row as u8;
            }
        }
        let order = RowOrder {
            stride: 8,
            bottom_up: true,
        };
        let frame =
            video_frame_from_contiguous(&source, format(PixelFormat::Yuyv422, 4, 4), order, "test")
                .expect("a bottom-up packed frame copies");
        assert_eq!(&frame.planes[0].data[0..8], &[3_u8; 8], "top row first");
    }

    #[test]
    fn a_bottom_up_two_plane_buffer_is_refused_rather_than_guessed_at() {
        let source = nv12_buffer(64, 64, 32);
        let order = RowOrder {
            stride: 64,
            bottom_up: true,
        };
        let error =
            video_frame_from_contiguous(&source, format(PixelFormat::Nv12, 64, 32), order, "cam")
                .expect_err("MF never inverts semi-planar buffers");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("bottom-up"), "{error}");
    }

    #[test]
    fn a_buffer_too_small_for_the_negotiated_size_is_rejected() {
        let source = nv12_buffer(64, 64, 32);
        let error = video_frame_from_contiguous(
            &source,
            format(PixelFormat::Nv12, 1280, 720),
            RowOrder {
                stride: 1280,
                bottom_up: false,
            },
            "cam",
        )
        .expect_err("a 64x32 buffer is not 1280x720");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_buffer_holding_only_the_luma_plane_is_rejected() {
        // Exactly the trap a plane-count-blind copy falls into: enough bytes
        // for Y, nothing for UV.
        let source = vec![0_u8; 64 * 32];
        let error = video_frame_from_contiguous(
            &source,
            format(PixelFormat::Nv12, 64, 32),
            RowOrder {
                stride: 64,
                bottom_up: false,
            },
            "cam",
        )
        .expect_err("no chroma plane");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("plane 1"), "{error}");
    }

    #[test]
    fn a_compressed_format_has_no_plane_path() {
        let source = vec![0_u8; 1024];
        let error = video_frame_from_contiguous(
            &source,
            CaptureFormat::new(CaptureEncoding::Mjpeg, 64, 32, 30, 1),
            RowOrder {
                stride: 64,
                bottom_up: false,
            },
            "cam",
        )
        .expect_err("MJPEG is a bitstream");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_format_with_no_plane_geometry_is_rejected_not_approximated() {
        // A zero edge has no geometry at all. `plane_shapes` answers `None`,
        // and the frame must be refused rather than delivered empty.
        let source = vec![0_u8; 1 << 10];
        for (width, height) in [(0_u32, 32_u32), (64, 0)] {
            let error = video_frame_from_contiguous(
                &source,
                format(PixelFormat::Nv12, width, height),
                RowOrder {
                    stride: 64,
                    bottom_up: false,
                },
                "cam",
            )
            .expect_err("a zero-sized mode is not a mode");
            assert!(
                matches!(error, CaptureError::FormatRejected { .. }),
                "{error:?}"
            );
            assert!(error.to_string().contains("no plane geometry"), "{error}");
        }
    }

    // ── Idle parking ────────────────────────────────────────────────────────

    #[test]
    fn a_healthy_stream_never_parks() {
        // A frame resets the counter, so a device that interleaves the odd
        // stream tick with real frames never reaches the threshold.
        for frameless in 0..=FRAMELESS_READS_BEFORE_PARK {
            assert!(!should_park(frameless), "{frameless} frameless read(s)");
        }
    }

    #[test]
    fn a_stalled_stream_parks() {
        assert!(should_park(FRAMELESS_READS_BEFORE_PARK + 1));
        assert!(should_park(1_000));
        assert!(should_park(u32::MAX));
    }

    #[test]
    fn the_threshold_is_small_enough_to_bound_a_spin() {
        // The whole point is that a stalled source cannot burn a core for long.
        assert!(FRAMELESS_READS_BEFORE_PARK < 16);
    }

    // ── HRESULT classification ──────────────────────────────────────────────

    #[test]
    fn the_device_loss_family_ends_the_session() {
        for code in [
            MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED,
            MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED,
            MF_E_HW_MFT_FAILED_START_STREAMING,
            MF_E_HW_STREAM_NOT_CONNECTED,
            MF_E_SHUTDOWN,
        ] {
            assert_eq!(classify_read_error(code), Fatality::Fatal, "{code:#010x}");
        }
    }

    #[test]
    fn an_invalid_request_is_retried() {
        assert_eq!(
            classify_read_error(MF_E_INVALIDREQUEST),
            Fatality::Transient
        );
    }

    /// The load-bearing half of the classifier: anything it has never heard of
    /// must end the session, not spin.
    #[test]
    fn an_unknown_hresult_is_fatal_by_default() {
        for code in [
            0_i32,                  // S_OK, which ReadSample never fails with
            0x8000_4005_u32 as i32, // E_FAIL
            0x8007_0005_u32 as i32, // E_ACCESSDENIED
            0x8007_001F_u32 as i32, // ERROR_GEN_FAILURE
            0xC00D_36B4_u32 as i32, // MF_E_INVALIDMEDIATYPE
            i32::MIN,
            i32::MAX,
        ] {
            assert_eq!(classify_read_error(code), Fatality::Fatal, "{code:#010x}");
        }
    }

    #[test]
    fn the_hresult_constants_have_the_documented_values() {
        // Guards against a typo in a hand-written constant; `reader.rs` adds a
        // compile-time check against the `windows` crate's own definitions.
        assert_eq!(MF_E_INVALIDREQUEST as u32, 0xC00D_36B2);
        assert_eq!(MF_E_HW_MFT_FAILED_START_STREAMING as u32, 0xC00D_3704);
        assert_eq!(MF_E_HW_STREAM_NOT_CONNECTED as u32, 0xC00D_A7FD);
        assert_eq!(MF_E_SHUTDOWN as u32, 0xC00D_3E85);
        assert_eq!(MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED as u32, 0xC00D_3EA2);
        assert_eq!(MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED as u32, 0xC00D_3EA3);
    }
}

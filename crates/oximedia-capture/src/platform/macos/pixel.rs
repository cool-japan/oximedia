//! Copying samples out of a `CVPixelBuffer`.
//!
//! AVFoundation hands the delegate a `CMSampleBuffer` that references memory
//! owned by the capture system's own pool. That memory must be read while it is
//! locked and released promptly — holding sample buffers starves the pool and
//! the device starts dropping frames — so this module makes exactly one copy,
//! row by row, into freshly owned [`VideoFrame`] planes.
//!
//! # Strides are not widths
//!
//! Core Video pads rows. A 1280-wide NV12 buffer routinely reports 1280 bytes
//! per row, but the same camera at 1920 wide may report 1920, 2048 or something
//! else again depending on the alignment the pool chose, and the padding is
//! *not* zero — it is whatever the previous tenant of that memory left behind.
//! Every copy here therefore walks rows individually using
//! `CVPixelBufferGetBytesPerRow(OfPlane)` for the source and this crate's own
//! convention for the destination, and copies only the meaningful prefix of
//! each row.
//!
//! # Destination convention
//!
//! Destination strides come from
//! [`PixelFormat::stride_for_width`](oximedia_core::PixelFormat::stride_for_width),
//! which is the convention the rest of the workspace is written against:
//!
//! | Format | Planes | Stride(s) | Rows |
//! |---|---|---|---|
//! | `Nv12` | 2 | `width`, `width` (interleaved Cb/Cr) | `height`, `⌈height/2⌉` |
//! | `Uyvy422`, `Yuyv422` | 1 | `width * 2` | `height` |
//!
//! The resulting frame's total size equals
//! [`PixelFormat::frame_buffer_size`](oximedia_core::PixelFormat::frame_buffer_size),
//! which the live tests assert.
//!
//! Note that [`VideoFrame::allocate`] is deliberately *not* used: it sizes the
//! NV12 chroma plane as `⌈width/2⌉ × ⌈height/2⌉`, half of the interleaved
//! plane's real size, and its documentation states that this historical
//! behaviour is preserved on purpose. Building the planes here from
//! `stride_for_width` keeps this backend consistent with `frame_buffer_size`
//! and with every converter in `oximedia-simd`.
//!
//! # Why no [`BufferPool`](crate::pool::BufferPool)
//!
//! A pooled buffer is returned to its free-list when the guard drops, and
//! `PooledBuffer::into_inner` removes it from the pool permanently — after
//! `capacity` frames the pool would be empty. A [`VideoFrame`] plane owns its
//! `Vec<u8>` and is handed to the consumer, so it can never be given back.
//! Staging through the pool and then copying into the plane would double the
//! per-frame copy — ~500 MB/s of extra traffic at 1080p60 NV12 — to save an
//! allocation that `Vec::with_capacity` already makes cheap. One copy, straight
//! from the locked buffer into the plane, is the right shape here.

#![allow(unsafe_code)]

use objc2_core_video::{
    kCVReturnSuccess, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use oximedia_codec::{Plane, VideoFrame};
use oximedia_core::PixelFormat;

use crate::device::CaptureFormat;
use crate::error::CaptureError;

// ── Lock guard ───────────────────────────────────────────────────────────────

/// A read-only lock on a `CVPixelBuffer`'s base addresses.
///
/// `CVPixelBufferLockBaseAddress` and `CVPixelBufferUnlockBaseAddress` must be
/// called in matched pairs *with matching flags*; Core Video's own header calls
/// asymmetric use undefined behaviour. Every failure path in
/// [`copy_video_frame`] is an early `return`, so pairing them by hand would be
/// one forgotten `?` away from leaking the lock and wedging the capture pool.
struct ReadLock<'buffer> {
    buffer: &'buffer CVPixelBuffer,
}

impl<'buffer> ReadLock<'buffer> {
    /// Take the read-only lock, or report why it could not be taken.
    fn acquire(buffer: &'buffer CVPixelBuffer) -> Result<Self, CaptureError> {
        // SAFETY: `buffer` is a live `CVPixelBuffer` borrowed from the sample
        // buffer that AVFoundation passed to the delegate, and the lock flags
        // used here are the ones `Drop` will unlock with.
        let status =
            unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status == kCVReturnSuccess {
            Ok(Self { buffer })
        } else {
            Err(CaptureError::platform(format!(
                "CVPixelBufferLockBaseAddress failed with CVReturn {status}"
            )))
        }
    }

    /// The locked buffer.
    const fn buffer(&self) -> &'buffer CVPixelBuffer {
        self.buffer
    }
}

impl Drop for ReadLock<'_> {
    fn drop(&mut self) {
        // SAFETY: the buffer is still borrowed for `'buffer`, and these are the
        // same flags `acquire` locked with, which is what Core Video requires.
        let status = unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly)
        };
        if status != kCVReturnSuccess {
            tracing::warn!(status, "CVPixelBufferUnlockBaseAddress failed");
        }
    }
}

// ── Plane geometry ───────────────────────────────────────────────────────────

/// The destination shape of one plane: bytes per row and number of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaneShape {
    /// Meaningful bytes in one row.
    pub(crate) stride: usize,
    /// Number of rows.
    pub(crate) rows: usize,
    /// Plane width in pixels, as [`Plane::width`] records it.
    pub(crate) width: u32,
    /// Plane height in pixels, as [`Plane::height`] records it.
    pub(crate) height: u32,
}

/// Destination geometry for `format` at `width` × `height`.
///
/// Returns `None` for a pixel layout this backend has no copy path for, which
/// is every layout [`super::encoding_for_fourcc`] does not produce.
pub(crate) fn plane_shapes(pixel: PixelFormat, width: u32, height: u32) -> Option<Vec<PlaneShape>> {
    if width == 0 || height == 0 {
        return None;
    }
    // `plane_dimensions` needs a frame to read `width`/`height` and the chroma
    // ratios from; no buffer is allocated by `new`.
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

// ── Copy ─────────────────────────────────────────────────────────────────────

/// Copy one `CVPixelBuffer` into a [`VideoFrame`] in the negotiated format.
///
/// # Errors
///
/// * [`CaptureError::FormatRejected`] when the buffer's dimensions, plane count
///   or row stride contradict the negotiated format. The frame is dropped
///   rather than delivered at the wrong size: a caller that trusted
///   [`CaptureFrame::width`](crate::CaptureFrame::width) would read past the
///   end of the samples.
/// * [`CaptureError::Platform`] when the base address cannot be locked or a
///   plane's base address comes back null.
pub(crate) fn copy_video_frame(
    buffer: &CVPixelBuffer,
    format: CaptureFormat,
    device: &str,
) -> Result<VideoFrame, CaptureError> {
    let Some(pixel) = format.pixel() else {
        return Err(CaptureError::format_rejected(
            device,
            format,
            "compressed encodings do not arrive as a CVPixelBuffer",
        ));
    };

    let buffer_width = CVPixelBufferGetWidth(buffer);
    let buffer_height = CVPixelBufferGetHeight(buffer);
    if buffer_width != format.width as usize || buffer_height != format.height as usize {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!("device delivered {buffer_width}x{buffer_height}"),
        ));
    }

    let Some(shapes) = plane_shapes(pixel, format.width, format.height) else {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!("no plane geometry for {pixel:?}"),
        ));
    };

    let lock = ReadLock::acquire(buffer)?;
    let source_planes = CVPixelBufferGetPlaneCount(lock.buffer());

    // A non-planar buffer reports zero planes and is addressed with the
    // un-suffixed accessors; a planar one must agree with the layout the
    // negotiated format implies.
    if source_planes == 0 {
        if shapes.len() != 1 {
            return Err(CaptureError::format_rejected(
                device,
                format,
                format!(
                    "device delivered a packed buffer for a {}-plane format",
                    shapes.len()
                ),
            ));
        }
    } else if source_planes != shapes.len() {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!(
                "device delivered {source_planes} plane(s), expected {}",
                shapes.len()
            ),
        ));
    }

    let mut planes = Vec::with_capacity(shapes.len());
    for (index, shape) in shapes.iter().enumerate() {
        let (base, source_stride, source_rows) = if source_planes == 0 {
            (
                CVPixelBufferGetBaseAddress(lock.buffer()),
                CVPixelBufferGetBytesPerRow(lock.buffer()),
                buffer_height,
            )
        } else {
            (
                CVPixelBufferGetBaseAddressOfPlane(lock.buffer(), index),
                CVPixelBufferGetBytesPerRowOfPlane(lock.buffer(), index),
                CVPixelBufferGetHeightOfPlane(lock.buffer(), index),
            )
        };
        planes.push(copy_plane(
            base,
            source_stride,
            source_rows,
            *shape,
            format,
            device,
            index,
        )?);
    }
    drop(lock);

    let mut frame = VideoFrame::new(pixel, format.width, format.height);
    frame.planes = planes;
    Ok(frame)
}

/// Copy one plane, honouring both strides.
///
/// `base` is a raw pointer into memory the caller keeps locked for the whole
/// call; the pointer is not stored anywhere.
fn copy_plane(
    base: *const core::ffi::c_void,
    source_stride: usize,
    source_rows: usize,
    shape: PlaneShape,
    format: CaptureFormat,
    device: &str,
    index: usize,
) -> Result<Plane, CaptureError> {
    if base.is_null() {
        return Err(CaptureError::platform(format!(
            "CVPixelBuffer plane {index} has a null base address"
        )));
    }
    if source_stride < shape.stride {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!(
                "plane {index} row is {source_stride} bytes, needs at least {}",
                shape.stride
            ),
        ));
    }
    if source_rows < shape.rows {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!(
                "plane {index} has {source_rows} row(s), needs {}",
                shape.rows
            ),
        ));
    }

    // `Vec::with_capacity` panics on a capacity that cannot exist, and this
    // code runs inside an Objective-C callback frame where unwinding is
    // undefined behaviour. Compute the size fallibly instead.
    let Some(capacity) = shape.stride.checked_mul(shape.rows) else {
        return Err(CaptureError::format_rejected(
            device,
            format,
            format!(
                "plane {index} geometry {}x{} does not fit in memory",
                shape.stride, shape.rows
            ),
        ));
    };

    let base = base.cast::<u8>();
    let mut data = Vec::with_capacity(capacity);
    for row in 0..shape.rows {
        // SAFETY: `base` is the locked base address of a plane with at least
        // `source_rows` rows of `source_stride` bytes, and `row < shape.rows <=
        // source_rows`, so `row * source_stride` is within the plane and
        // `shape.stride <= source_stride` bytes are readable from there. The
        // slice does not outlive this iteration, and the read-only lock is held
        // by the caller for the whole loop.
        let source =
            unsafe { std::slice::from_raw_parts(base.add(row * source_stride), shape.stride) };
        data.extend_from_slice(source);
    }

    Ok(Plane::with_dimensions(
        data,
        shape.stride,
        shape.width,
        shape.height,
    ))
}

#[cfg(test)]
mod tests {
    use core::ptr::NonNull;

    use objc2_core_foundation::CFRetained;
    use objc2_core_video::CVPixelBufferCreate;

    use super::*;
    use crate::device::CaptureEncoding;
    use crate::platform::macos::{FOURCC_2VUY, FOURCC_420V, FOURCC_YUVS};

    /// Marker written into every byte of a source buffer before its payload,
    /// so that a copy which ignored the stride is detectable by inspection.
    const PADDING: u8 = 0xEE;

    fn format(pixel: PixelFormat, width: u32, height: u32) -> CaptureFormat {
        CaptureFormat::new(CaptureEncoding::Raw(pixel), width, height, 30, 1)
    }

    // ── Synthetic Core Video buffers ────────────────────────────────────────

    /// Allocate a real `CVPixelBuffer`, exactly as the capture pool would.
    ///
    /// Core Video chooses the row alignment, so this exercises the padded and
    /// unpadded cases the same way a camera does — without a camera, without
    /// permission, and without a prompt.
    fn create_pixel_buffer(fourcc: u32, width: u32, height: u32) -> CFRetained<CVPixelBuffer> {
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: `pixel_buffer_out` points at a live local, and no attributes
        // dictionary is passed, so there are no generics to get wrong.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                width as usize,
                height as usize,
                fourcc,
                None,
                NonNull::from(&mut out),
            )
        };
        assert_eq!(status, kCVReturnSuccess, "CVPixelBufferCreate failed");
        let ptr = NonNull::new(out).expect("a successful create yields a buffer");
        // SAFETY: `CVPixelBufferCreate` returns a +1 reference, which is what
        // `from_raw` takes ownership of.
        unsafe { CFRetained::from_raw(ptr) }
    }

    /// Byte a test expects at `(plane, row, column)` of the payload.
    fn sample(plane: usize, row: usize, column: usize) -> u8 {
        u8::try_from((plane * 97 + row * 7 + column * 3) % 233).unwrap_or(0)
    }

    /// Fill every plane's padding with [`PADDING`] and its payload with
    /// [`sample`], using the *destination* stride as the payload width.
    fn fill(buffer: &CVPixelBuffer, shapes: &[PlaneShape]) {
        // SAFETY: a read/write lock on a buffer this test owns; unlocked below
        // with the same (empty) flags.
        let status =
            unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::empty()) };
        assert_eq!(status, kCVReturnSuccess, "lock for write");

        let planar = CVPixelBufferGetPlaneCount(buffer) > 0;
        for (index, shape) in shapes.iter().enumerate() {
            let (base, stride, rows) = if planar {
                (
                    CVPixelBufferGetBaseAddressOfPlane(buffer, index),
                    CVPixelBufferGetBytesPerRowOfPlane(buffer, index),
                    CVPixelBufferGetHeightOfPlane(buffer, index),
                )
            } else {
                (
                    CVPixelBufferGetBaseAddress(buffer),
                    CVPixelBufferGetBytesPerRow(buffer),
                    CVPixelBufferGetHeight(buffer),
                )
            };
            assert!(!base.is_null(), "plane {index} base address");
            assert!(
                stride >= shape.stride,
                "plane {index} is narrower than needed"
            );
            assert!(rows >= shape.rows, "plane {index} is shorter than needed");

            // SAFETY: `base` is the locked base address of a plane of at least
            // `rows * stride` bytes, which is exactly the region written here.
            let plane = unsafe { std::slice::from_raw_parts_mut(base.cast::<u8>(), rows * stride) };
            plane.fill(PADDING);
            for row in 0..shape.rows {
                for column in 0..shape.stride {
                    plane[row * stride + column] = sample(index, row, column);
                }
            }
        }

        // SAFETY: matching unlock with the same flags as the lock above.
        let status =
            unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::empty()) };
        assert_eq!(status, kCVReturnSuccess, "unlock after write");
    }

    /// Was this buffer actually padded? Reported so a passing test says which
    /// case it proved.
    fn is_padded(buffer: &CVPixelBuffer, shapes: &[PlaneShape]) -> bool {
        if CVPixelBufferGetPlaneCount(buffer) > 0 {
            (0..shapes.len()).any(|index| {
                CVPixelBufferGetBytesPerRowOfPlane(buffer, index) > shapes[index].stride
            })
        } else {
            CVPixelBufferGetBytesPerRow(buffer) > shapes[0].stride
        }
    }

    /// Copy a freshly filled buffer and check it byte for byte.
    fn round_trip(fourcc: u32, pixel: PixelFormat, width: u32, height: u32) {
        let shapes = plane_shapes(pixel, width, height).expect("a supported layout");
        let buffer = create_pixel_buffer(fourcc, width, height);
        fill(&buffer, &shapes);
        let padded = is_padded(&buffer, &shapes);

        let video = copy_video_frame(&buffer, format(pixel, width, height), "synthetic")
            .unwrap_or_else(|error| panic!("copy failed for {pixel:?} {width}x{height}: {error}"));

        assert_eq!(video.format, pixel);
        assert_eq!(video.width, width);
        assert_eq!(video.height, height);
        assert_eq!(video.planes.len(), shapes.len(), "{pixel:?} plane count");
        assert_eq!(
            video.size_bytes(),
            pixel.frame_buffer_size(width, height),
            "{pixel:?} total size must match the workspace convention"
        );

        for (index, (plane, shape)) in video.planes.iter().zip(&shapes).enumerate() {
            assert_eq!(plane.stride, shape.stride, "plane {index} stride");
            assert_eq!(plane.width, shape.width, "plane {index} width");
            assert_eq!(plane.height, shape.height, "plane {index} height");
            assert_eq!(
                plane.data.len(),
                shape.stride * shape.rows,
                "plane {index} size"
            );
            assert!(
                !plane.data.contains(&PADDING),
                "plane {index} of a {}padded buffer leaked row padding",
                if padded { "" } else { "un" }
            );
            for row in 0..shape.rows {
                for column in 0..shape.stride {
                    assert_eq!(
                        plane.data[row * shape.stride + column],
                        sample(index, row, column),
                        "plane {index} row {row} column {column}"
                    );
                }
            }
        }
    }

    // ── Whole-buffer copies ─────────────────────────────────────────────────

    #[test]
    fn nv12_buffers_copy_plane_by_plane() {
        // 66 is even (4:2:0 needs that) but not a multiple of any plausible row
        // alignment, so Core Video pads these; 64 is the aligned case.
        round_trip(FOURCC_420V, PixelFormat::Nv12, 66, 34);
        round_trip(FOURCC_420V, PixelFormat::Nv12, 64, 32);
    }

    #[test]
    fn packed_422_buffers_copy_through_the_non_planar_path() {
        let buffer = create_pixel_buffer(FOURCC_2VUY, 66, 34);
        assert_eq!(
            CVPixelBufferGetPlaneCount(&buffer),
            0,
            "a packed buffer must take the un-suffixed accessors"
        );
        drop(buffer);
        round_trip(FOURCC_2VUY, PixelFormat::Uyvy422, 66, 34);
        round_trip(FOURCC_YUVS, PixelFormat::Yuyv422, 64, 32);
    }

    #[test]
    fn a_buffer_of_the_wrong_size_is_rejected_not_resized() {
        let buffer = create_pixel_buffer(FOURCC_420V, 64, 32);
        let error = copy_video_frame(&buffer, format(PixelFormat::Nv12, 1280, 720), "synthetic")
            .expect_err("64x32 is not 1280x720");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("64x32"), "{error}");
    }

    #[test]
    fn a_packed_buffer_cannot_satisfy_a_two_plane_format() {
        // The buffer is packed 4:2:2 but the negotiated format says NV12, so
        // the plane counts disagree and no frame may be produced.
        let buffer = create_pixel_buffer(FOURCC_2VUY, 64, 32);
        let error = copy_video_frame(&buffer, format(PixelFormat::Nv12, 64, 32), "synthetic")
            .expect_err("a packed buffer has one plane, NV12 needs two");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("packed buffer"), "{error}");
    }

    #[test]
    fn a_planar_buffer_cannot_satisfy_a_packed_format() {
        let buffer = create_pixel_buffer(FOURCC_420V, 64, 32);
        let error = copy_video_frame(&buffer, format(PixelFormat::Uyvy422, 64, 32), "synthetic")
            .expect_err("NV12 has two planes, packed 4:2:2 needs one");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("plane(s)"), "{error}");
    }

    #[test]
    fn a_compressed_format_has_no_pixel_buffer_path() {
        let buffer = create_pixel_buffer(FOURCC_420V, 64, 32);
        let error = copy_video_frame(
            &buffer,
            CaptureFormat::new(CaptureEncoding::Mjpeg, 64, 32, 30, 1),
            "synthetic",
        )
        .expect_err("MJPEG does not arrive as a CVPixelBuffer");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn the_read_lock_is_released_on_every_path() {
        // A leaked read lock is invisible until the next writer blocks, so the
        // proof is that a second copy of the same buffer still succeeds after a
        // failing one.
        let buffer = create_pixel_buffer(FOURCC_420V, 64, 32);
        let shapes = plane_shapes(PixelFormat::Nv12, 64, 32).expect("nv12");
        fill(&buffer, &shapes);

        assert!(copy_video_frame(&buffer, format(PixelFormat::Uyvy422, 64, 32), "x").is_err());
        assert!(copy_video_frame(&buffer, format(PixelFormat::Nv12, 1280, 720), "x").is_err());
        // Writing needs the buffer unlocked by every previous reader.
        fill(&buffer, &shapes);
        assert!(copy_video_frame(&buffer, format(PixelFormat::Nv12, 64, 32), "x").is_ok());
    }

    // ── Destination geometry ────────────────────────────────────────────────

    #[test]
    fn nv12_has_two_planes_with_a_full_width_chroma_stride() {
        let shapes = plane_shapes(PixelFormat::Nv12, 1920, 1080).expect("nv12 is supported");
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].stride, 1920);
        assert_eq!(shapes[0].rows, 1080);
        // Interleaved Cb/Cr: 960 sample pairs of 2 bytes = 1920 bytes per row.
        assert_eq!(shapes[1].stride, 1920);
        assert_eq!(shapes[1].rows, 540);
    }

    #[test]
    fn nv12_geometry_matches_the_workspace_frame_size() {
        let shapes = plane_shapes(PixelFormat::Nv12, 1280, 720).expect("nv12 is supported");
        let total: usize = shapes.iter().map(|shape| shape.stride * shape.rows).sum();
        assert_eq!(total, PixelFormat::Nv12.frame_buffer_size(1280, 720));
    }

    #[test]
    fn packed_422_is_one_plane_of_two_bytes_per_pixel() {
        for pixel in [PixelFormat::Uyvy422, PixelFormat::Yuyv422] {
            let shapes = plane_shapes(pixel, 640, 480).expect("packed 4:2:2 is supported");
            assert_eq!(shapes.len(), 1, "{pixel:?}");
            assert_eq!(shapes[0].stride, 1280, "{pixel:?}");
            assert_eq!(shapes[0].rows, 480, "{pixel:?}");
            let total = shapes[0].stride * shapes[0].rows;
            assert_eq!(total, pixel.frame_buffer_size(640, 480), "{pixel:?}");
        }
    }

    #[test]
    fn odd_heights_round_the_chroma_plane_up() {
        let shapes = plane_shapes(PixelFormat::Nv12, 640, 481).expect("nv12 is supported");
        assert_eq!(shapes[1].rows, 241, "a half row still has to be carried");
    }

    #[test]
    fn zero_dimensions_have_no_geometry() {
        assert!(plane_shapes(PixelFormat::Nv12, 0, 480).is_none());
        assert!(plane_shapes(PixelFormat::Nv12, 640, 0).is_none());
    }

    // ── Row copying ─────────────────────────────────────────────────────────

    /// Build a source plane whose padding differs from its payload, so a copy
    /// that ignored the stride would be detectable.
    fn padded_plane(stride: usize, rows: usize, payload: usize) -> Vec<u8> {
        let mut buffer = vec![0xEE_u8; stride * rows];
        for row in 0..rows {
            for column in 0..payload {
                buffer[row * stride + column] = u8::try_from((row * 7 + column) % 251).unwrap_or(0);
            }
        }
        buffer
    }

    #[test]
    fn padded_rows_are_copied_without_their_padding() {
        let shape = PlaneShape {
            stride: 64,
            rows: 4,
            width: 64,
            height: 4,
        };
        let source = padded_plane(96, 4, 64);
        let plane = copy_plane(
            source.as_ptr().cast(),
            96,
            4,
            shape,
            format(PixelFormat::Gray8, 64, 4),
            "test",
            0,
        )
        .expect("a padded plane copies");
        assert_eq!(plane.data.len(), 64 * 4);
        assert_eq!(plane.stride, 64);
        assert!(
            !plane.data.contains(&0xEE),
            "padding bytes leaked into the frame"
        );
        for row in 0..4_usize {
            for column in 0..64_usize {
                assert_eq!(
                    plane.data[row * 64 + column],
                    u8::try_from((row * 7 + column) % 251).unwrap_or(0),
                    "row {row} column {column}"
                );
            }
        }
    }

    #[test]
    fn an_unpadded_plane_copies_verbatim() {
        let shape = PlaneShape {
            stride: 32,
            rows: 8,
            width: 32,
            height: 8,
        };
        let source = padded_plane(32, 8, 32);
        let plane = copy_plane(
            source.as_ptr().cast(),
            32,
            8,
            shape,
            format(PixelFormat::Gray8, 32, 8),
            "test",
            0,
        )
        .expect("an unpadded plane copies");
        assert_eq!(plane.data, source);
    }

    #[test]
    fn a_short_source_row_is_rejected_rather_than_read_past() {
        let shape = PlaneShape {
            stride: 64,
            rows: 2,
            width: 64,
            height: 2,
        };
        let source = vec![0_u8; 32 * 2];
        let error = copy_plane(
            source.as_ptr().cast(),
            32,
            2,
            shape,
            format(PixelFormat::Gray8, 64, 2),
            "/dev/camera",
            0,
        )
        .expect_err("32 < 64 bytes per row");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("32 bytes"), "{error}");
    }

    #[test]
    fn a_short_source_plane_is_rejected() {
        let shape = PlaneShape {
            stride: 16,
            rows: 8,
            width: 16,
            height: 8,
        };
        let source = vec![0_u8; 16 * 4];
        let error = copy_plane(
            source.as_ptr().cast(),
            16,
            4,
            shape,
            format(PixelFormat::Gray8, 16, 8),
            "/dev/camera",
            1,
        )
        .expect_err("4 < 8 rows");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("4 row(s)"), "{error}");
    }

    #[test]
    fn a_null_base_address_is_a_platform_error_not_a_frame() {
        let shape = PlaneShape {
            stride: 8,
            rows: 1,
            width: 8,
            height: 1,
        };
        let error = copy_plane(
            std::ptr::null(),
            8,
            1,
            shape,
            format(PixelFormat::Gray8, 8, 1),
            "/dev/camera",
            0,
        )
        .expect_err("a null plane cannot be copied");
        assert!(matches!(error, CaptureError::Platform(_)), "{error:?}");
    }

    #[test]
    fn an_impossible_geometry_is_an_error_not_a_capacity_panic() {
        let shape = PlaneShape {
            stride: usize::MAX,
            rows: 2,
            width: 8,
            height: 2,
        };
        let source = vec![0_u8; 8];
        let error = copy_plane(
            source.as_ptr().cast(),
            usize::MAX,
            2,
            shape,
            format(PixelFormat::Gray8, 8, 2),
            "/dev/camera",
            0,
        )
        .expect_err("stride * rows overflows usize");
        assert!(
            matches!(error, CaptureError::FormatRejected { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_zero_row_plane_copies_to_an_empty_buffer() {
        let shape = PlaneShape {
            stride: 8,
            rows: 0,
            width: 8,
            height: 0,
        };
        let source = vec![0_u8; 8];
        let plane = copy_plane(
            source.as_ptr().cast(),
            8,
            0,
            shape,
            format(PixelFormat::Gray8, 8, 0),
            "test",
            0,
        )
        .expect("nothing to copy is not a failure");
        assert!(plane.data.is_empty());
    }
}

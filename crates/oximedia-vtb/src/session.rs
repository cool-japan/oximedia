//! Safe wrapper around `VTDecompressionSession`.
//!
//! Builds a session against an [`H264FormatDescription`], owns a Rust-side
//! refcon that the C decode callback pushes decoded frames into, and
//! exposes a synchronous `decode_packet` method that returns when the
//! callback has finished.
//!
//! Threading: VideoToolbox can invoke the output callback on its own
//! internal serial queue. We force synchronous decode (no
//! `kVTDecodeFrame_EnableAsynchronousDecompression`) so the callback
//! has fired by the time `decode_packet` returns. The refcon is a
//! [`Box`] that is leaked at session-create time and reclaimed in
//! [`Drop`], by which point [`VTDecompressionSessionInvalidate`] has
//! ensured no further callbacks can run.

use std::ffi::c_void;
use std::sync::Mutex;

use oximedia_codec::frame::{Plane, VideoFrame};
use oximedia_core::{PixelFormat, Rational, Timestamp};
use oximedia_vtb_sys::{
    CMBlockBufferCreateWithMemoryBlock, CMBlockBufferRef, CMBlockBufferReplaceDataBytes,
    CMSampleBufferCreateReady, CMSampleBufferRef, CMSampleTimingInfo, CMTime,
    CVImageBufferRef, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetPlaneCount, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferUnlockBaseAddress, VTDecodeInfoFlags,
    VTDecompressionOutputCallbackRecord, VTDecompressionSessionCreate,
    VTDecompressionSessionDecodeFrame, VTDecompressionSessionInvalidate,
    VTDecompressionSessionRef,
};

use crate::cf::CfOwned;
use crate::error::{StatusContext, VtbError};
use crate::format::H264FormatDescription;
use crate::nal::annex_b_to_avcc;

/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` — '420v' four-char-code.
/// This is the default VideoToolbox H.264 decode output on macOS/iOS: NV12
/// in video range (Y in 16–235, UV in 16–240).
const PIXEL_FORMAT_NV12_VIDEO_RANGE: u32 = 0x3432_3076;
/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` — '420f'.
const PIXEL_FORMAT_NV12_FULL_RANGE: u32 = 0x3432_3066;
/// `kCVPixelBufferLock_ReadOnly` — non-mutating lock flag.
const PIXEL_BUFFER_LOCK_READ_ONLY: u32 = 1;
/// `kCMTimeFlags_Valid`.
const CMTIME_FLAGS_VALID: u32 = 1;
/// Default H.264 timebase (PTS counts at 90 kHz).
const H264_TIMESCALE: i32 = 90_000;

/// One frame queued from the decompression callback.
struct QueuedFrame {
    frame: VideoFrame,
}

/// Refcon storage handed to the decode callback. Lives in a `Box` and is
/// pointed at by the session for its entire lifetime.
struct CallbackContext {
    /// Frames pushed by the callback, popped by `pull_frame`.
    queue: Mutex<Vec<QueuedFrame>>,
}

impl CallbackContext {
    fn new() -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
        }
    }
}

/// Safe handle to a `VTDecompressionSession` configured for H.264 AVCC input.
pub struct DecompressionSession {
    session: CfOwned<c_void>,
    #[allow(dead_code)] // held to keep format alive while session is in use
    format: H264FormatDescription,
    /// Refcon owned by the session — must outlive `session`.
    ctx: *mut CallbackContext,
}

// VTDecompressionSession is documented as safe to use from any single
// thread at a time. We do not allow Sync (would permit concurrent
// `decode_packet` calls) but Send is fine.
unsafe impl Send for DecompressionSession {}

impl DecompressionSession {
    /// Create a new decompression session for the given H.264 format.
    pub fn new(format: H264FormatDescription) -> Result<Self, VtbError> {
        let ctx = Box::new(CallbackContext::new());
        let ctx_ptr = Box::into_raw(ctx);

        let callback_record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(output_callback_trampoline),
            decompressionOutputRefCon: ctx_ptr.cast::<c_void>(),
        };

        let mut session_out: VTDecompressionSessionRef = std::ptr::null_mut();
        // SAFETY: format.as_ptr() is valid (held by `format`); callback_record
        // is on-stack and read synchronously by Create; ctx_ptr will be
        // retained by the session for its lifetime.
        let status = unsafe {
            VTDecompressionSessionCreate(
                std::ptr::null_mut(),
                format.as_ptr(),
                std::ptr::null_mut(), // videoDecoderSpecification: NULL = system default
                std::ptr::null_mut(), // destinationImageBufferAttributes: NULL = NV12 video range
                &callback_record,
                &mut session_out,
            )
        };
        if let Err(e) = VtbError::check_status(status, StatusContext::SessionCreate) {
            // Reclaim the box so we don't leak when create fails.
            // SAFETY: ctx_ptr was just leaked; nothing else holds it.
            unsafe {
                let _ = Box::from_raw(ctx_ptr);
            }
            return Err(e);
        }

        // SAFETY: VTDecompressionSessionCreate returns +1 retain on success.
        let session = unsafe { CfOwned::from_create(session_out.cast::<c_void>()) }
            .ok_or(VtbError::SessionCreate(0))?;
        Ok(Self {
            session,
            format,
            ctx: ctx_ptr,
        })
    }

    /// Submit one Annex-B encoded H.264 access unit. SPS/PPS units inside
    /// are stripped (they're already in the format description); the rest
    /// is wrapped in AVCC framing and submitted to the session.
    ///
    /// On success, any frames the callback produced are now queued and
    /// can be drained with [`Self::pull_frame`].
    pub fn decode_packet(&mut self, annex_b: &[u8], pts: i64) -> Result<(), VtbError> {
        let avcc = annex_b_to_avcc(annex_b);
        if avcc.is_empty() {
            // Pure SPS/PPS access unit — nothing to decode but not an error.
            return Ok(());
        }
        let block = create_block_buffer(&avcc)?;
        let sample = create_sample_buffer(block.as_ptr(), self.format.as_ptr(), avcc.len(), pts)?;

        // SAFETY: session and sample buffer are both live; passing
        // `pts` through `sourceFrameRefCon` so the callback can recover it
        // without needing to read CMTime back from the timing info.
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                self.session.as_ptr().cast::<oximedia_vtb_sys::OpaqueVTDecompressionSession>(),
                sample.as_ptr().cast::<oximedia_vtb_sys::opaqueCMSampleBuffer>(),
                0, // decodeFlags: synchronous
                pts as *mut c_void,
                std::ptr::null_mut(),
            )
        };
        VtbError::check_status(status, StatusContext::DecodeFrame)
    }

    /// Pop a decoded frame off the queue, if one is ready.
    pub fn pull_frame(&mut self) -> Option<VideoFrame> {
        // SAFETY: ctx is owned by self and lives until Drop.
        let ctx = unsafe { &*self.ctx };
        let mut q = ctx.queue.lock().ok()?;
        q.pop().map(|qf| qf.frame)
    }
}

impl Drop for DecompressionSession {
    fn drop(&mut self) {
        // SAFETY: session is live; Invalidate stops any further callbacks
        // and is safe to call exactly once.
        unsafe {
            VTDecompressionSessionInvalidate(
                self.session.as_ptr().cast::<oximedia_vtb_sys::OpaqueVTDecompressionSession>(),
            );
        }
        // Releasing the CfOwned for the session happens via its own Drop
        // after this function returns. By then Invalidate has guaranteed
        // no callback can fire, so we can reclaim the refcon box.
        // SAFETY: ctx was created by Box::into_raw in `new()` and the
        // session is now invalidated.
        unsafe {
            let _ = Box::from_raw(self.ctx);
        }
    }
}

/// Wrap raw AVCC bytes in a `CMBlockBuffer`. CM allocates a fresh backing
/// buffer (so the original Rust slice can drop immediately after this
/// call) and the bytes are memcpy'd in via `CMBlockBufferReplaceDataBytes`.
fn create_block_buffer(avcc: &[u8]) -> Result<CfOwned<c_void>, VtbError> {
    let mut block: CMBlockBufferRef = std::ptr::null_mut();
    // SAFETY: memoryBlock=NULL + non-zero blockLength means CM allocates.
    let status = unsafe {
        CMBlockBufferCreateWithMemoryBlock(
            std::ptr::null_mut(), // structureAllocator
            std::ptr::null_mut(), // memoryBlock — NULL means CM allocates
            avcc.len(),
            std::ptr::null_mut(), // blockAllocator
            std::ptr::null_mut(), // customBlockSource
            0,                    // offsetToData
            avcc.len(),
            0, // flags
            &mut block,
        )
    };
    VtbError::check_status(status, StatusContext::SampleBuffer)?;

    // SAFETY: CMBlockBuffer freshly allocated; copying our bytes into it.
    let status = unsafe {
        CMBlockBufferReplaceDataBytes(avcc.as_ptr().cast::<c_void>(), block, 0, avcc.len())
    };
    VtbError::check_status(status, StatusContext::SampleBuffer)?;

    // SAFETY: block has +1 retain on success.
    unsafe { CfOwned::from_create(block.cast::<c_void>()) }.ok_or(VtbError::SampleBuffer(0))
}

/// Wrap a `CMBlockBuffer` of AVCC bytes in a `CMSampleBuffer` tied to the
/// given format description, with a single sample carrying `pts` at the
/// 90 kHz H.264 timescale.
fn create_sample_buffer(
    block: *mut c_void,
    format: oximedia_vtb_sys::CMFormatDescriptionRef,
    sample_size: usize,
    pts: i64,
) -> Result<CfOwned<c_void>, VtbError> {
    let timing = CMSampleTimingInfo {
        duration: invalid_cm_time(),
        presentationTimeStamp: CMTime {
            value: pts,
            timescale: H264_TIMESCALE,
            flags: CMTIME_FLAGS_VALID,
            epoch: 0,
        },
        decodeTimeStamp: invalid_cm_time(),
    };
    let sizes = [sample_size];

    let mut sample: CMSampleBufferRef = std::ptr::null_mut();
    // SAFETY: arguments live for the duration of the call.
    let status = unsafe {
        CMSampleBufferCreateReady(
            std::ptr::null_mut(),
            block.cast::<oximedia_vtb_sys::OpaqueCMBlockBuffer>(),
            format,
            1, // numSamples
            1, // numSampleTimingEntries
            &timing,
            1, // numSampleSizeEntries
            sizes.as_ptr(),
            &mut sample,
        )
    };
    VtbError::check_status(status, StatusContext::SampleBuffer)?;

    // SAFETY: sample has +1 retain on success.
    unsafe { CfOwned::from_create(sample.cast::<c_void>()) }.ok_or(VtbError::SampleBuffer(0))
}

fn invalid_cm_time() -> CMTime {
    CMTime {
        value: 0,
        timescale: 0,
        flags: 0,
        epoch: 0,
    }
}

/// C trampoline for the VTDecompressionOutputCallback.
///
/// Casts `decomp_ref_con` back to our `CallbackContext`, copies the
/// `CVImageBuffer` into a `VideoFrame`, and pushes it onto the queue.
/// Errors during extraction are silently dropped — VideoToolbox already
/// signaled the error status, and we don't want to panic across an FFI
/// boundary.
unsafe extern "C" fn output_callback_trampoline(
    decomp_ref_con: *mut c_void,
    source_frame_ref_con: *mut c_void,
    status: oximedia_vtb_sys::OSStatus,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: CVImageBufferRef,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 || image_buffer.is_null() || decomp_ref_con.is_null() {
        return;
    }
    // SAFETY: decomp_ref_con is the Box<CallbackContext> we leaked in `new`,
    // and the session is still live (Invalidate hasn't returned yet).
    let ctx = unsafe { &*(decomp_ref_con.cast::<CallbackContext>()) };
    let pts = source_frame_ref_con as i64;

    let Some(frame) = (unsafe { extract_video_frame(image_buffer, pts) }) else {
        return;
    };
    if let Ok(mut q) = ctx.queue.lock() {
        q.push(QueuedFrame { frame });
    }
}

/// Copy pixel data out of a CVImageBuffer into a `VideoFrame`.
///
/// # Safety
///
/// `image_buffer` must be a valid `CVPixelBufferRef` with a refcount the
/// caller is keeping alive for the duration of this call.
unsafe fn extract_video_frame(image_buffer: CVImageBufferRef, pts: i64) -> Option<VideoFrame> {
    let pixel_format = unsafe { CVPixelBufferGetPixelFormatType(image_buffer) };
    if !matches!(
        pixel_format,
        PIXEL_FORMAT_NV12_VIDEO_RANGE | PIXEL_FORMAT_NV12_FULL_RANGE
    ) {
        return None;
    }

    // Lock for read-only access.
    let lock_status = unsafe {
        CVPixelBufferLockBaseAddress(image_buffer, PIXEL_BUFFER_LOCK_READ_ONLY as u64)
    };
    if lock_status != 0 {
        return None;
    }

    let plane_count = unsafe { CVPixelBufferGetPlaneCount(image_buffer) };
    if plane_count == 0 {
        unsafe {
            CVPixelBufferUnlockBaseAddress(image_buffer, PIXEL_BUFFER_LOCK_READ_ONLY as u64);
        }
        return None;
    }

    let width0 = unsafe { CVPixelBufferGetWidthOfPlane(image_buffer, 0) } as u32;
    let height0 = unsafe { CVPixelBufferGetHeightOfPlane(image_buffer, 0) } as u32;

    let mut planes: Vec<Plane> = Vec::with_capacity(plane_count);
    for i in 0..plane_count {
        let src = unsafe { CVPixelBufferGetBaseAddressOfPlane(image_buffer, i) };
        if src.is_null() {
            unsafe {
                CVPixelBufferUnlockBaseAddress(image_buffer, PIXEL_BUFFER_LOCK_READ_ONLY as u64);
            }
            return None;
        }
        let bytes_per_row = unsafe { CVPixelBufferGetBytesPerRowOfPlane(image_buffer, i) };
        let plane_w = unsafe { CVPixelBufferGetWidthOfPlane(image_buffer, i) };
        let plane_h = unsafe { CVPixelBufferGetHeightOfPlane(image_buffer, i) };
        // NV12 chroma plane: width is in 2-byte chroma samples (CbCr pairs)
        // for the byte-wide accounting in VideoFrame::Plane we want the
        // *byte* width, not the pair count, so use bytes_per_row directly.
        let row_bytes = if i == 0 { plane_w } else { plane_w * 2 };
        let mut data = vec![0u8; row_bytes * plane_h];
        for row in 0..plane_h {
            // SAFETY: source row is in CV-allocated memory currently locked;
            // destination is our owned Vec. `bytes_per_row >= row_bytes` is
            // guaranteed by CV (it may add padding).
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (src as *const u8).add(row * bytes_per_row),
                    data.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                );
            }
        }
        planes.push(Plane::with_dimensions(
            data,
            row_bytes,
            plane_w as u32,
            plane_h as u32,
        ));
    }

    unsafe {
        CVPixelBufferUnlockBaseAddress(image_buffer, PIXEL_BUFFER_LOCK_READ_ONLY as u64);
    }

    let mut frame = VideoFrame::new(PixelFormat::Nv12, width0, height0);
    frame.planes = planes;
    frame.timestamp = Timestamp::new(pts, Rational::new(1, H264_TIMESCALE as i64));
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same Baseline-profile SPS/PPS as in `format::tests`.
    const SAMPLE_SPS: &[u8] = &[
        0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01, 0x40, 0x16, 0xE8, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40,
        0x00, 0x00, 0x0C, 0x03, 0xC5, 0x0A, 0x44,
    ];
    const SAMPLE_PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    #[test]
    fn create_and_drop_session_without_decoding() {
        let fmt = H264FormatDescription::from_parameter_sets(SAMPLE_SPS, SAMPLE_PPS)
            .expect("format description created");
        // If the FFI plumbing is right, this constructs a real VT session
        // against the system H.264 decoder and immediately tears it down.
        // The test exercises the new + drop path including refcon cleanup.
        let session = DecompressionSession::new(fmt).expect("session created");
        drop(session);
    }
}

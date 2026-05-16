//! Error types for VideoToolbox operations.
//!
//! Every VideoToolbox/CoreMedia/CoreVideo API returns an `OSStatus` —
//! a 32-bit signed integer where 0 means success. We map non-zero
//! values into structured [`VtbError`] variants for the most common
//! cases and fall back to [`VtbError::OsStatus`] for anything else.

use thiserror::Error;

/// Result alias used throughout this crate.
pub type Result<T> = std::result::Result<T, VtbError>;

/// All errors returned by the safe wrappers.
#[derive(Debug, Error)]
pub enum VtbError {
    /// `VTDecompressionSessionCreate` / `VTCompressionSessionCreate` failed.
    ///
    /// The inner `OSStatus` is the Apple error code; common values:
    ///   * `kVTVideoDecoderUnsupportedDataFormatErr` (-12986) — no decoder
    ///     installed for the format description (e.g. an HEVC stream on a
    ///     machine without HEVC hardware decode).
    ///   * `kVTVideoDecoderBadDataErr` (-12909) — the parameter sets are
    ///     malformed.
    #[error("session creation failed: OSStatus {0}")]
    SessionCreate(i32),

    /// A `VTDecompressionSessionDecodeFrame` call returned non-zero.
    #[error("decode call failed: OSStatus {0}")]
    DecodeFrame(i32),

    /// `CMVideoFormatDescriptionCreateFromH264ParameterSets` failed.
    ///
    /// Usually means the SPS or PPS bytes aren't a valid H.264 parameter
    /// set NAL unit (wrong NAL type, length-prefix mismatch, etc.).
    #[error("format description creation failed: OSStatus {0}")]
    FormatDescription(i32),

    /// `CMSampleBuffer` / `CMBlockBuffer` creation failed.
    #[error("sample buffer creation failed: OSStatus {0}")]
    SampleBuffer(i32),

    /// `CVPixelBufferLockBaseAddress` failed.
    #[error("pixel buffer lock failed: OSStatus {0}")]
    PixelBufferLock(i32),

    /// The decoder produced an output frame in a pixel format we don't
    /// know how to translate into [`oximedia_codec::VideoFrame`].
    #[error("unsupported VideoToolbox output pixel format: 0x{0:08X}")]
    UnsupportedPixelFormat(u32),

    /// SPS/PPS could not be located in the supplied parameter byte stream.
    ///
    /// Either the caller didn't pass extradata, or the start-code scan
    /// found no NAL units of type 7 (SPS) / 8 (PPS).
    #[error("missing parameter sets: {0}")]
    MissingParameterSets(&'static str),

    /// A bytestream we expected to be in Annex-B format is malformed —
    /// e.g. a NAL unit declared via 4-byte length prefix exceeds the
    /// input buffer.
    #[error("malformed bitstream: {0}")]
    Malformed(&'static str),

    /// Catch-all for OSStatus values that don't map to a specific
    /// VtbError variant. Reported verbatim so callers can match on
    /// the numeric code if they need to.
    #[error("VideoToolbox OSStatus {0}")]
    OsStatus(i32),
}

impl VtbError {
    /// Convert a raw `OSStatus` plus a context tag into a `Result<()>`.
    ///
    /// Zero is treated as success. The `context` parameter chooses which
    /// `VtbError` variant a non-zero status maps to — there is no
    /// authoritative way to tell `kVTVideoDecoderUnsupportedDataFormatErr`
    /// (returned by session create) from a generic decode-frame error at
    /// the FFI boundary, so the caller passes its own intent.
    pub fn check_status(status: i32, context: StatusContext) -> Result<()> {
        if status == 0 {
            return Ok(());
        }
        Err(match context {
            StatusContext::SessionCreate => Self::SessionCreate(status),
            StatusContext::DecodeFrame => Self::DecodeFrame(status),
            StatusContext::FormatDescription => Self::FormatDescription(status),
            StatusContext::SampleBuffer => Self::SampleBuffer(status),
            StatusContext::PixelBufferLock => Self::PixelBufferLock(status),
            StatusContext::Other => Self::OsStatus(status),
        })
    }
}

/// Identifies which VideoToolbox call produced an `OSStatus`, so
/// non-zero values can be mapped to a meaningful [`VtbError`] variant.
#[derive(Debug, Clone, Copy)]
pub enum StatusContext {
    /// `VT*SessionCreate`.
    SessionCreate,
    /// `VTDecompressionSessionDecodeFrame`.
    DecodeFrame,
    /// `CMVideoFormatDescriptionCreate*`.
    FormatDescription,
    /// `CMSampleBufferCreate*` / `CMBlockBufferCreate*`.
    SampleBuffer,
    /// `CVPixelBufferLockBaseAddress` / `CVPixelBufferUnlockBaseAddress`.
    PixelBufferLock,
    /// Anything else.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_status_is_ok() {
        assert!(VtbError::check_status(0, StatusContext::SessionCreate).is_ok());
        assert!(VtbError::check_status(0, StatusContext::DecodeFrame).is_ok());
    }

    #[test]
    fn nonzero_status_maps_by_context() {
        match VtbError::check_status(-12986, StatusContext::SessionCreate).unwrap_err() {
            VtbError::SessionCreate(s) => assert_eq!(s, -12986),
            other => panic!("expected SessionCreate, got {other:?}"),
        }
        match VtbError::check_status(-99, StatusContext::DecodeFrame).unwrap_err() {
            VtbError::DecodeFrame(s) => assert_eq!(s, -99),
            other => panic!("expected DecodeFrame, got {other:?}"),
        }
        match VtbError::check_status(-1, StatusContext::Other).unwrap_err() {
            VtbError::OsStatus(-1) => {}
            other => panic!("expected OsStatus(-1), got {other:?}"),
        }
    }

    #[test]
    fn error_messages_include_status_code() {
        let e = VtbError::SessionCreate(-12986);
        let s = format!("{e}");
        assert!(s.contains("-12986"));
    }
}

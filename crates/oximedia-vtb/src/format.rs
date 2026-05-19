//! `CMVideoFormatDescription` construction from H.264 parameter sets.
//!
//! VideoToolbox identifies the format of a compressed video stream via a
//! `CMVideoFormatDescription`. For H.264 this is built by passing the
//! SPS and PPS byte payloads to
//! `CMVideoFormatDescriptionCreateFromH264ParameterSets`, plus the
//! length of the AVCC length prefix (always 4 bytes in our pipeline).

use oximedia_vtb_sys::{
    CMFormatDescriptionRef, CMVideoFormatDescriptionCreateFromH264ParameterSets,
};

use crate::cf::CfOwned;
use crate::error::{StatusContext, VtbError};

/// Safe wrapper around a `CMVideoFormatDescription` retained by the caller.
#[derive(Clone, Debug)]
pub struct H264FormatDescription {
    inner: CfOwned<std::ffi::c_void>,
}

impl H264FormatDescription {
    /// Build a `CMVideoFormatDescription` from raw SPS and PPS NAL unit
    /// payloads (no start codes, no length prefix — the bytes you get
    /// from [`crate::nal::extract_sps_pps`]).
    pub fn from_parameter_sets(sps: &[u8], pps: &[u8]) -> Result<Self, VtbError> {
        // Array of pointers + array of sizes, both length-2.
        let pointers: [*const u8; 2] = [sps.as_ptr(), pps.as_ptr()];
        let sizes: [usize; 2] = [sps.len(), pps.len()];

        let mut fmt: CMFormatDescriptionRef = std::ptr::null_mut();
        // SAFETY: pointers/sizes arrays are length 2 and valid for the
        // duration of the call; `&mut fmt` is a valid out-pointer.
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                std::ptr::null_mut(),
                2,
                pointers.as_ptr(),
                sizes.as_ptr(),
                4, // 4-byte AVCC length prefix
                &mut fmt,
            )
        };
        VtbError::check_status(status, StatusContext::FormatDescription)?;

        // SAFETY: fmt was returned with +1 retain by Create on success.
        // `CMFormatDescriptionRef` is `*const opaqueCMFormatDescription`;
        // we cast to `*mut c_void` for storage in `CfOwned`. The cast is
        // sound because CFRetain/CFRelease operate on the type-erased
        // CFTypeRef and don't care about mutability.
        let owned = unsafe { CfOwned::from_create(fmt as *mut std::ffi::c_void) }
            .ok_or(VtbError::FormatDescription(0))?;
        Ok(Self { inner: owned })
    }

    /// Pointer for handing to other VideoToolbox APIs. The return type
    /// matches what `CM*` functions expect (a `*const` ref).
    pub fn as_ptr(&self) -> CMFormatDescriptionRef {
        self.inner.as_ptr() as CMFormatDescriptionRef
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid Baseline-profile SPS captured from a 320x240 stream.
    /// Hand-crafted as parameter sets large enough that
    /// `CMVideoFormatDescriptionCreateFromH264ParameterSets` will accept
    /// them; we don't decode against this, just construct the format
    /// description to exercise the FFI path.
    const SAMPLE_SPS: &[u8] = &[
        0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01, 0x40, 0x16, 0xE8, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40,
        0x00, 0x00, 0x0C, 0x03, 0xC5, 0x0A, 0x44,
    ];
    const SAMPLE_PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    #[test]
    fn build_format_description_from_known_parameter_sets() {
        let fmt = H264FormatDescription::from_parameter_sets(SAMPLE_SPS, SAMPLE_PPS)
            .expect("format description created");
        assert!(!fmt.as_ptr().is_null());
    }

    #[test]
    fn empty_sps_or_pps_fails_gracefully() {
        let err = H264FormatDescription::from_parameter_sets(&[], SAMPLE_PPS).unwrap_err();
        assert!(matches!(err, VtbError::FormatDescription(_)));
    }
}

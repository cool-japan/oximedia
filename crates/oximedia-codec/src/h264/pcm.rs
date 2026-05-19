//! H.264 `I_PCM` macroblock payload reader.
//!
//! On `I_PCM` the bitstream aligns to a byte boundary and emits
//! raw samples instead of running the residual / transform
//! pipeline.  Per ITU-T Rec. H.264 / ISO/IEC 14496-10 § 7.3.5.1:
//!
//! 1. `pcm_alignment_zero_bit` repeated until byte-aligned (CABAC
//!    decoders need to consume any leftover bits in the current
//!    arithmetic-coder register before reading raw bytes).
//! 2. `pcm_sample_luma[i]` for i = 0..256 (8-bit each on baseline /
//!    main / high 8-bit profiles).
//! 3. `pcm_sample_chroma[i]` for i = 0..(2 × MbWidthC × MbHeightC)
//!    — 128 bytes total on 4:2:0, 256 on 4:2:2, 512 on 4:4:4.
//!
//! After all samples are consumed the encoder reinitialises CABAC
//! from the next byte.  Callers therefore need to:
//!
//! - Run [`read_pcm_macroblock_420`] (or the wider-chroma variant
//!   when 4:2:2 / 4:4:4 lands) on the byte slice starting at the
//!   post-`I_PCM`-mb_type position.
//! - Write the returned samples into the picture buffer at the
//!   macroblock's coordinates with no prediction or transform.
//! - Construct a fresh [`crate::h264::cabac::CabacContext`] from
//!   the leftover bytes so the next macroblock decodes against a
//!   clean arithmetic-coder state.

use crate::CodecError;

/// Per-`I_PCM`-macroblock raw sample bundle (4:2:0).
#[derive(Debug, Clone)]
pub struct PcmSamples420 {
    /// 256 raw luma samples in raster order (16 × 16).
    pub luma: [u8; 256],
    /// 64 raw Cb samples in raster order (8 × 8).
    pub cb: [u8; 64],
    /// 64 raw Cr samples in raster order (8 × 8).
    pub cr: [u8; 64],
    /// Number of bytes consumed from the input slice.  Always 384
    /// for 4:2:0 + 8-bit samples.  Callers use this to advance the
    /// bitstream cursor and re-init CABAC.
    pub bytes_consumed: usize,
}

/// Reads one `I_PCM` macroblock payload for a 4:2:0 8-bit slice.
///
/// `buf` must point at the first PCM byte (the caller has already
/// consumed `mb_type == I_PCM` from the bitstream and, for CABAC,
/// has drained any leftover bits in the arithmetic-coder register
/// via the spec's `pcm_alignment_zero_bit` rule).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when `buf` is shorter than
/// 384 bytes — the minimum needed for one 4:2:0 `I_PCM` macroblock.
pub fn read_pcm_macroblock_420(buf: &[u8]) -> Result<PcmSamples420, CodecError> {
    const LUMA_BYTES: usize = 256;
    const CHROMA_BYTES: usize = 64;
    const TOTAL_BYTES: usize = LUMA_BYTES + 2 * CHROMA_BYTES;
    if buf.len() < TOTAL_BYTES {
        return Err(CodecError::InvalidData(format!(
            "h264 pcm: I_PCM macroblock needs {} bytes, only {} available",
            TOTAL_BYTES,
            buf.len()
        )));
    }
    let mut luma = [0u8; LUMA_BYTES];
    luma.copy_from_slice(&buf[0..LUMA_BYTES]);
    let mut cb = [0u8; CHROMA_BYTES];
    cb.copy_from_slice(&buf[LUMA_BYTES..LUMA_BYTES + CHROMA_BYTES]);
    let mut cr = [0u8; CHROMA_BYTES];
    cr.copy_from_slice(&buf[LUMA_BYTES + CHROMA_BYTES..TOTAL_BYTES]);
    Ok(PcmSamples420 {
        luma,
        cb,
        cr,
        bytes_consumed: TOTAL_BYTES,
    })
}

/// Writes one decoded `I_PCM` macroblock into a [`crate::h264::frame::Frame`].
///
/// Bypasses prediction / transform / deblocking — the samples land
/// in the frame exactly as transmitted.  Per spec § 8.5.2, the
/// post-deblocking filter is also disabled for `I_PCM`
/// macroblocks; callers driving frame-level deblocking should
/// mark the corresponding [`crate::h264::deblock_frame::DeblockMbState::skip_external_filter`]
/// accordingly.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the macroblock extends
/// past the frame.
pub fn write_pcm_macroblock_420(
    frame: &mut crate::h264::frame::Frame,
    mb_x: usize,
    mb_y: usize,
    samples: &PcmSamples420,
) -> Result<(), CodecError> {
    let px = mb_x.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 pcm: mb_x overflow".into())
    })?;
    let py = mb_y.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 pcm: mb_y overflow".into())
    })?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 pcm: I_PCM mb at ({mb_x}, {mb_y}) extends past frame"
        )));
    }
    for j in 0..16 {
        for i in 0..16 {
            frame.set_luma(px + i, py + j, samples.luma[j * 16 + i]);
        }
    }
    let cx = mb_x * 8;
    let cy = mb_y * 8;
    let cw = frame.chroma_width();
    let ch = frame.chroma_height();
    if cx + 8 > cw || cy + 8 > ch {
        return Err(CodecError::InvalidData(format!(
            "h264 pcm: chroma extends past plane ({cw}x{ch})"
        )));
    }
    for j in 0..8 {
        for i in 0..8 {
            frame.set_cb(cx + i, cy + j, samples.cb[j * 8 + i]);
            frame.set_cr(cx + i, cy + j, samples.cr[j * 8 + i]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::frame::Frame;

    #[test]
    fn read_short_buffer_errors() {
        let buf = vec![0u8; 100];
        assert!(read_pcm_macroblock_420(&buf).is_err());
    }

    #[test]
    fn read_one_macroblock_returns_384_bytes() {
        let buf = vec![42u8; 384];
        let s = read_pcm_macroblock_420(&buf).unwrap();
        assert_eq!(s.bytes_consumed, 384);
        assert!(s.luma.iter().all(|&v| v == 42));
        assert!(s.cb.iter().all(|&v| v == 42));
        assert!(s.cr.iter().all(|&v| v == 42));
    }

    #[test]
    fn write_macroblock_places_samples_at_correct_offsets() {
        let mut buf = vec![0u8; 384];
        // Fill luma with row*16 + col so we can verify position.
        for j in 0..16 {
            for i in 0..16 {
                buf[j * 16 + i] = (j * 16 + i) as u8;
            }
        }
        // Cb / Cr just identical patterns.
        for k in 0..64 {
            buf[256 + k] = k as u8;
            buf[256 + 64 + k] = (255 - k) as u8;
        }
        let samples = read_pcm_macroblock_420(&buf).unwrap();
        let mut frame = Frame::new(32, 16);
        write_pcm_macroblock_420(&mut frame, 1, 0, &samples).unwrap();
        // Macroblock at column 1 means luma px = 16.
        assert_eq!(frame.get_luma(16, 0), Some(0));
        assert_eq!(frame.get_luma(31, 15), Some(255));
        assert_eq!(frame.get_cb(8, 0), Some(0));
        assert_eq!(frame.get_cr(15, 7), Some(255 - 63));
    }
}

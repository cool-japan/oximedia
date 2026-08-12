//! VP9 Superframe handling.
//!
//! A VP9 superframe contains multiple frames packed together with an index.

use crate::error::{CodecError, CodecResult};

/// VP9 Superframe container.
#[derive(Clone, Debug)]
pub struct Superframe {
    /// Individual frames within the superframe.
    pub frames: Vec<Vec<u8>>,
}

/// Superframe index information.
#[derive(Clone, Debug)]
pub struct SuperframeIndex {
    /// Number of frames in the superframe.
    pub frame_count: usize,
    /// Size of each frame in bytes.
    pub frame_sizes: Vec<usize>,
    /// Number of bytes used to encode each frame size.
    pub bytes_per_size: usize,
    /// Total size of the index in bytes.
    pub index_size: usize,
}

impl Superframe {
    /// Parses a superframe from data.
    ///
    /// # Errors
    ///
    /// Returns error if the superframe index is invalid, or if the claimed
    /// per-frame sizes do not account for exactly the payload bytes that
    /// precede the index (see the sum check below).
    pub fn parse(data: &[u8]) -> CodecResult<Self> {
        if data.is_empty() {
            return Err(CodecError::InvalidBitstream("Empty data".into()));
        }

        if let Some(index) = SuperframeIndex::parse(data)? {
            let mut frames = Vec::with_capacity(index.frame_count);
            let mut offset = 0;

            for &size in &index.frame_sizes {
                if offset + size > data.len() - index.index_size {
                    return Err(CodecError::InvalidBitstream(
                        "Frame size exceeds data".into(),
                    ));
                }
                frames.push(data[offset..offset + size].to_vec());
                offset += size;
            }

            // The per-frame loop above only guarantees each individual
            // claimed size fits inside the payload; it does not guarantee
            // the sizes *sum* to exactly the payload length minus the
            // index. Without this check, a payload whose last byte matches
            // the marker shape (`0b110xxxxx`) by pure coincidence -- not an
            // actual superframe -- could pass the length/marker-repeat
            // disambiguation in `SuperframeIndex::parse` and still come
            // back `Ok` here with `frames` that silently omit some of the
            // payload's real tail bytes (found and documented in
            // `dec/testdata/RECIPE.md`'s "Known sharp edge" section; fixed
            // here rather than only worked around by callers).
            let consumed = data.len() - index.index_size;
            if offset != consumed {
                return Err(CodecError::InvalidBitstream(format!(
                    "Superframe frame sizes sum to {offset} bytes but the \
                     payload holds {consumed} bytes before the \
                     {}-byte index",
                    index.index_size
                )));
            }

            Ok(Self { frames })
        } else {
            Ok(Self {
                frames: vec![data.to_vec()],
            })
        }
    }

    /// Returns true if this contains multiple frames.
    #[must_use]
    pub fn is_superframe(&self) -> bool {
        self.frames.len() > 1
    }

    /// Returns the number of frames in this superframe.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Returns an iterator over the frames.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.frames.iter().map(Vec::as_slice)
    }
}

/// Marker-byte bit layout, verified against two independent authoritative
/// sources (both agree, and both disagree with an earlier draft of this
/// package's own task brief that placed the two fields the other way
/// around):
///
/// - **VP9 spec Annex B.2.2**, `superframe_header()`: `f(3)
///   superframe_marker`, `f(2) bytes_per_framesize_minus_1`, `f(3)
///   frames_in_superframe_minus_1`, in that order — spec `f(n)` reads bits
///   MSB-to-LSB (spec 4.9.1), so within the byte this is marker in bits 7-5,
///   `bytes_per_framesize_minus_1` in bits 4-3, `frames_in_superframe_minus_1`
///   in bits 2-0.
/// - **libvpx v1.15.2**, `vp9/decoder/vp9_decoder.c`,
///   `vp9_parse_superframe_index()`:
///   ```c
///   if ((marker & 0xe0) == 0xc0) {
///     const uint32_t frames = (marker & 0x7) + 1;        /* low 3 bits */
///     const uint32_t mag = ((marker >> 3) & 0x3) + 1;     /* next 2 bits up */
///   ```
///
/// Both sources agree: bits 7-5 = marker (`0b110`), bits 4-3 =
/// `bytes_per_size - 1`, bits 2-0 = `frame_count - 1` — exactly what
/// [`SuperframeIndex::parse`] below derives (`bytes_per_size` from `(marker
/// >> 3) & 0x3`, `frame_count` from `marker & 0x7`). Confirmed empirically
/// too: this layout parses `p9alt.frame1.bin` (a real libvpx-encoded
/// superframe) into two frames sized 1853 and 411 bytes, which sum exactly
/// to its payload length minus the 6-byte index (see
/// `dec/testdata/RECIPE.md`) — a wrong bit assignment would produce
/// nonsense sizes that fail that sum check immediately.
impl SuperframeIndex {
    const MARKER: u8 = 0b110;
    const MAX_FRAMES: usize = 8;

    /// Parses a superframe index from data.
    ///
    /// # Errors
    ///
    /// Returns error if the index is malformed.
    pub fn parse(data: &[u8]) -> CodecResult<Option<Self>> {
        if data.is_empty() {
            return Ok(None);
        }

        let marker = data[data.len() - 1];
        if (marker >> 5) != Self::MARKER {
            return Ok(None);
        }

        let bytes_per_size = ((marker >> 3) & 0x3) as usize + 1;
        let frame_count = (marker & 0x7) as usize + 1;

        if frame_count > Self::MAX_FRAMES {
            return Err(CodecError::InvalidBitstream(
                "Too many frames in superframe".into(),
            ));
        }

        let index_size = 1 + frame_count * bytes_per_size + 1;

        if data.len() < index_size {
            return Err(CodecError::InvalidBitstream(
                "Data too short for superframe index".into(),
            ));
        }

        let index_start = data.len() - index_size;
        if data[index_start] != marker {
            return Err(CodecError::InvalidBitstream(
                "Superframe index marker mismatch".into(),
            ));
        }

        let mut frame_sizes = Vec::with_capacity(frame_count);
        let mut offset = index_start + 1;

        for _ in 0..frame_count {
            let mut size: usize = 0;
            for i in 0..bytes_per_size {
                size |= (data[offset + i] as usize) << (i * 8);
            }
            frame_sizes.push(size);
            offset += bytes_per_size;
        }

        Ok(Some(Self {
            frame_count,
            frame_sizes,
            bytes_per_size,
            index_size,
        }))
    }

    /// Returns the total size of all frames.
    #[must_use]
    pub fn total_frame_size(&self) -> usize {
        self.frame_sizes.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_superframe() {
        let data = [0x00, 0x01, 0x02, 0x03];
        let sf = Superframe::parse(&data).expect("should succeed");
        assert!(!sf.is_superframe());
        assert_eq!(sf.frame_count(), 1);
    }

    #[test]
    fn test_empty_data() {
        let data: [u8; 0] = [];
        assert!(Superframe::parse(&data).is_err());
    }

    #[test]
    fn test_superframe_marker() {
        let data = [0x00, 0x01, 0x00, 0x01, 0x02, 0xC1, 0x02, 0x03, 0xC1];
        let index = SuperframeIndex::parse(&data)
            .expect("should succeed")
            .expect("should succeed");
        assert_eq!(index.frame_count, 2);
        assert_eq!(index.frame_sizes, vec![2, 3]);
    }

    #[test]
    fn test_superframe_parse() {
        let data = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xC1, 0x02, 0x03, 0xC1];
        let sf = Superframe::parse(&data).expect("should succeed");
        assert!(sf.is_superframe());
        assert_eq!(sf.frame_count(), 2);
    }

    /// Regression test for the "quiet" failure mode documented in
    /// `dec/testdata/RECIPE.md`'s "Known sharp edge" section: a payload
    /// whose marker byte and index-repeat both check out, but whose
    /// claimed sizes sum to *less* than the payload bytes actually
    /// available before the index. Before this package's fix,
    /// `Superframe::parse` accepted this and silently dropped the
    /// unaccounted byte (index 5 below) instead of decoding or rejecting
    /// it; it must now be an honest `Err`.
    ///
    /// Same trailing shape as `test_superframe_parse` above (marker 0xC1 =>
    /// `bytes_per_size=1`, `frame_count=2`, `index_size=4`, claimed sizes
    /// `[2, 3]` summing to 5) but with one extra byte (`0xFF` at index 5)
    /// spliced in before the index, so the payload holds 6 bytes before the
    /// index while the claimed sizes only account for 5 of them.
    #[test]
    fn test_superframe_size_sum_mismatch_is_rejected() {
        let data = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, // 6 real frame-data bytes
            0xC1, 0x02, 0x03, 0xC1, // index claiming sizes [2, 3] (sum 5)
        ];
        let err = Superframe::parse(&data)
            .expect_err("sizes summing to less than the available payload must be rejected");
        match err {
            CodecError::InvalidBitstream(msg) => {
                assert!(msg.contains('5') && msg.contains('6'), "{msg}");
            }
            other => panic!("expected InvalidBitstream, got {other:?}"),
        }
    }

    /// The same shape, but the claimed sizes overshoot instead of falling
    /// short (sum to more than the available payload). The pre-existing
    /// per-frame `offset + size > data.len() - index.index_size` check
    /// already caught this before this package's fix (this is not a
    /// regression test for the fix itself), kept here so both directions of
    /// "sizes don't add up" are pinned in one place.
    #[test]
    fn test_superframe_size_sum_overshoot_is_rejected() {
        let data = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, // 5 real frame-data bytes
            0xC1, 0x05, 0x03, 0xC1, // index claiming sizes [5, 3] (sum 8)
        ];
        assert!(Superframe::parse(&data).is_err());
    }
}

//! H.264 NAL unit handling: Annex-B ↔ AVCC conversion and parameter-set
//! extraction.
//!
//! VideoToolbox expects video samples in **AVCC** format: each NAL unit is
//! prefixed with a 32-bit big-endian length field and no start codes
//! between units. Network and RTP delivery overwhelmingly uses **Annex-B**:
//! each NAL unit is preceded by the byte sequence `0x000001` or
//! `0x00000001` ("start codes"), and NAL units run back-to-back.
//!
//! This module converts between the two and pulls SPS / PPS out of an
//! Annex-B stream so we can hand them to
//! `CMVideoFormatDescriptionCreateFromH264ParameterSets`.

use crate::error::VtbError;

/// H.264 NAL unit type 7 (Sequence Parameter Set).
pub const NAL_TYPE_SPS: u8 = 7;
/// H.264 NAL unit type 8 (Picture Parameter Set).
pub const NAL_TYPE_PPS: u8 = 8;

/// NAL unit types that don't carry video data and shouldn't be wrapped in
/// AVCC samples (they're configured into the format description instead).
fn is_parameter_set(nal_type: u8) -> bool {
    matches!(nal_type, NAL_TYPE_SPS | NAL_TYPE_PPS)
}

/// Iterator over NAL units in an Annex-B byte stream.
///
/// Yields the *payload* of each NAL unit (i.e. start codes are stripped).
/// Both 3-byte (`00 00 01`) and 4-byte (`00 00 00 01`) start codes are
/// recognized.
pub struct AnnexBIter<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> AnnexBIter<'a> {
    /// Create an iterator over the Annex-B stream `bytes`.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    /// Find the next start code at or after `start`, returning the byte
    /// offset of the first byte *after* the start code (i.e. the NAL unit
    /// payload start). `None` if no start code remains.
    fn find_next_start_code(&self, start: usize) -> Option<usize> {
        let mut i = start;
        while i + 3 <= self.bytes.len() {
            if self.bytes[i] == 0 && self.bytes[i + 1] == 0 {
                // 3-byte: 00 00 01
                if self.bytes[i + 2] == 0x01 {
                    return Some(i + 3);
                }
                // 4-byte: 00 00 00 01
                if i + 4 <= self.bytes.len()
                    && self.bytes[i + 2] == 0x00
                    && self.bytes[i + 3] == 0x01
                {
                    return Some(i + 4);
                }
            }
            i += 1;
        }
        None
    }
}

impl<'a> Iterator for AnnexBIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let payload_start = self.find_next_start_code(self.cursor)?;
        // The NAL ends where the next start code begins, or at EOF.
        let payload_end = match self.find_next_start_code(payload_start) {
            Some(next_payload_start) => {
                // Back up over the start code (3 or 4 bytes).
                let scan_back = next_payload_start - 1;
                if scan_back >= 3
                    && self.bytes.get(scan_back - 3) == Some(&0)
                    && self.bytes.get(scan_back - 2) == Some(&0)
                    && self.bytes.get(scan_back - 1) == Some(&0)
                {
                    scan_back - 3
                } else {
                    scan_back - 2
                }
            }
            None => self.bytes.len(),
        };
        self.cursor = payload_end;
        Some(&self.bytes[payload_start..payload_end])
    }
}

/// Read the 5-bit `nal_unit_type` from the first byte of a NAL unit payload.
///
/// Returns `None` for an empty slice.
#[must_use]
pub fn nal_unit_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| b & 0x1F)
}

/// Locate and extract the first SPS and PPS NAL units from an Annex-B
/// byte stream.
///
/// Returns `Ok((sps, pps))` on success or [`VtbError::MissingParameterSets`]
/// if either parameter set is absent.
pub fn extract_sps_pps(annex_b: &[u8]) -> Result<(Vec<u8>, Vec<u8>), VtbError> {
    let mut sps: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;

    for nal in AnnexBIter::new(annex_b) {
        match nal_unit_type(nal) {
            Some(NAL_TYPE_SPS) if sps.is_none() => sps = Some(nal.to_vec()),
            Some(NAL_TYPE_PPS) if pps.is_none() => pps = Some(nal.to_vec()),
            _ => {}
        }
        if sps.is_some() && pps.is_some() {
            break;
        }
    }

    match (sps, pps) {
        (Some(s), Some(p)) => Ok((s, p)),
        (None, _) => Err(VtbError::MissingParameterSets("SPS not found in stream")),
        (_, None) => Err(VtbError::MissingParameterSets("PPS not found in stream")),
    }
}

/// Convert an Annex-B byte stream into AVCC framing.
///
/// Each non-parameter-set NAL unit is emitted as `<u32 length BE><payload>`
/// in the output buffer. Parameter sets (SPS/PPS) are *skipped*: they're
/// configured into the `CMVideoFormatDescription` once during session
/// setup, not sent inline with frames.
#[must_use]
pub fn annex_b_to_avcc(annex_b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(annex_b.len());
    for nal in AnnexBIter::new(annex_b) {
        let Some(nal_type) = nal_unit_type(nal) else {
            continue;
        };
        if is_parameter_set(nal_type) {
            continue;
        }
        let len = nal.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Reverse direction: AVCC → Annex-B with 4-byte start codes.
///
/// Used when handing decoded data back to consumers that expect Annex-B
/// (most software decoders, FFmpeg interop, etc.). Each `<u32 len><payload>`
/// frame becomes `00 00 00 01 <payload>`.
pub fn avcc_to_annex_b(avcc: &[u8]) -> Result<Vec<u8>, VtbError> {
    let mut out = Vec::with_capacity(avcc.len());
    let mut i = 0;
    while i < avcc.len() {
        if i + 4 > avcc.len() {
            return Err(VtbError::Malformed("AVCC length prefix truncated"));
        }
        let len =
            u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        i += 4;
        if i + len > avcc.len() {
            return Err(VtbError::Malformed("AVCC NAL payload extends past buffer"));
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&avcc[i..i + len]);
        i += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build an Annex-B stream from a list of (nal_type, payload).
    fn build_annex_b(units: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, (nal_type, payload)) in units.iter().enumerate() {
            // Alternate 3- and 4-byte start codes so the parser sees both.
            if i % 2 == 0 {
                out.extend_from_slice(&[0, 0, 0, 1]);
            } else {
                out.extend_from_slice(&[0, 0, 1]);
            }
            // First byte: forbidden_zero_bit | nal_ref_idc | nal_unit_type
            // We set nal_ref_idc=3 (highest) for SPS/PPS, 0 otherwise.
            let ref_idc = if matches!(*nal_type, NAL_TYPE_SPS | NAL_TYPE_PPS) {
                3
            } else {
                0
            };
            let header = (ref_idc << 5) | (*nal_type & 0x1F);
            out.push(header);
            out.extend_from_slice(payload);
        }
        out
    }

    #[test]
    fn annex_b_iter_handles_3_and_4_byte_start_codes() {
        let stream = build_annex_b(&[(7, b"SPS"), (8, b"PPS"), (5, b"IDR")]);
        let nals: Vec<&[u8]> = AnnexBIter::new(&stream).collect();
        assert_eq!(nals.len(), 3);
        assert_eq!(nal_unit_type(nals[0]), Some(NAL_TYPE_SPS));
        assert_eq!(nal_unit_type(nals[1]), Some(NAL_TYPE_PPS));
        assert_eq!(nal_unit_type(nals[2]), Some(5)); // IDR
        // Payload should follow the header byte.
        assert_eq!(&nals[0][1..], b"SPS");
        assert_eq!(&nals[2][1..], b"IDR");
    }

    #[test]
    fn empty_stream_yields_nothing() {
        assert!(AnnexBIter::new(&[]).next().is_none());
        assert!(AnnexBIter::new(&[0u8; 16]).next().is_none());
    }

    #[test]
    fn extract_sps_pps_finds_both() {
        let stream = build_annex_b(&[(5, b"IDR1"), (7, b"SPS-DATA"), (8, b"PPS-DATA")]);
        let (sps, pps) = extract_sps_pps(&stream).unwrap();
        assert_eq!(nal_unit_type(&sps), Some(NAL_TYPE_SPS));
        assert_eq!(nal_unit_type(&pps), Some(NAL_TYPE_PPS));
        assert!(sps.ends_with(b"SPS-DATA"));
        assert!(pps.ends_with(b"PPS-DATA"));
    }

    #[test]
    fn extract_sps_pps_takes_first_of_each() {
        let stream = build_annex_b(&[
            (7, b"FIRST-SPS"),
            (8, b"FIRST-PPS"),
            (7, b"SECOND-SPS"),
            (8, b"SECOND-PPS"),
        ]);
        let (sps, pps) = extract_sps_pps(&stream).unwrap();
        assert!(sps.ends_with(b"FIRST-SPS"));
        assert!(pps.ends_with(b"FIRST-PPS"));
    }

    #[test]
    fn extract_sps_pps_missing_sps_errors() {
        let stream = build_annex_b(&[(8, b"PPS-only")]);
        let err = extract_sps_pps(&stream).unwrap_err();
        assert!(matches!(err, VtbError::MissingParameterSets(_)));
    }

    #[test]
    fn extract_sps_pps_missing_pps_errors() {
        let stream = build_annex_b(&[(7, b"SPS-only")]);
        let err = extract_sps_pps(&stream).unwrap_err();
        assert!(matches!(err, VtbError::MissingParameterSets(_)));
    }

    #[test]
    fn annex_b_to_avcc_strips_parameter_sets_and_length_prefixes() {
        // SPS + PPS + IDR + non-IDR. Output should contain only IDR + non-IDR
        // with 4-byte big-endian length prefixes.
        let stream = build_annex_b(&[
            (7, b"sps"),
            (8, b"pps"),
            (5, b"AAAA"),
            (1, b"BB"),
        ]);
        let avcc = annex_b_to_avcc(&stream);
        // IDR: header byte (nal_ref_idc=0, type=5) + "AAAA" = 5 bytes →
        //      length prefix 0x00000005
        // non-IDR: header byte (nal_ref_idc=0, type=1) + "BB" = 3 bytes →
        //      length prefix 0x00000003
        assert_eq!(&avcc[..4], &[0, 0, 0, 5]);
        assert_eq!(&avcc[4..9], &[5, b'A', b'A', b'A', b'A']);
        assert_eq!(&avcc[9..13], &[0, 0, 0, 3]);
        assert_eq!(&avcc[13..16], &[1, b'B', b'B']);
        assert_eq!(avcc.len(), 16);
    }

    #[test]
    fn avcc_to_annex_b_round_trip_preserves_video_nals() {
        let stream = build_annex_b(&[(5, b"AAAA"), (1, b"BB")]);
        let avcc = annex_b_to_avcc(&stream);
        let back = avcc_to_annex_b(&avcc).unwrap();
        // Round-trip uses 4-byte start codes only.
        assert_eq!(&back[..4], &[0, 0, 0, 1]);
        assert!(back.windows(4).any(|w| w == [0, 0, 0, 1]));
    }

    #[test]
    fn avcc_to_annex_b_rejects_truncated_length() {
        let bad = vec![0, 0, 0]; // length prefix incomplete
        assert!(matches!(
            avcc_to_annex_b(&bad).unwrap_err(),
            VtbError::Malformed(_)
        ));
    }

    #[test]
    fn avcc_to_annex_b_rejects_payload_overrun() {
        let bad = vec![0, 0, 0, 100, 0xAB]; // declares 100 bytes, has 1
        assert!(matches!(
            avcc_to_annex_b(&bad).unwrap_err(),
            VtbError::Malformed(_)
        ));
    }

    #[test]
    fn nal_unit_type_handles_empty() {
        assert!(nal_unit_type(&[]).is_none());
    }
}

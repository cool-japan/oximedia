//! H.264 RTP depacketization (RFC 6184).
//!
//! RTP carries H.264 in three common packetization shapes:
//!
//! 1. **Single NAL unit packet** — one RTP packet carries one whole NAL
//!    unit. The first byte of the payload *is* the NAL unit header.
//!    RTP NAL types 1–23.
//! 2. **STAP-A** (Single-Time Aggregation Packet, RTP NAL type 24) — one
//!    packet carries N NAL units that share a timestamp. Each NAL is
//!    prefixed with a 2-byte big-endian length. Used for sending
//!    `SPS + PPS` together (and often + an IDR slice) in one frame.
//! 3. **FU-A** (Fragmentation Unit, RTP NAL type 28) — one NAL unit
//!    *split across* multiple RTP packets. Mandatory for any frame larger
//!    than the MTU (~1400 bytes), i.e. nearly every IDR.
//!
//! The other shapes (STAP-B, MTAP16, MTAP24, FU-B — types 25–27 and 29)
//! exist in the spec but are essentially never seen in the wild; this
//! depacketizer rejects them with a `Protocol` error.
//!
//! # Output
//!
//! The depacketizer emits **access units** in Annex-B framing — exactly
//! the format an H.264 software decoder, or
//! [`oximedia_vtb::H264Decoder::send_packet`] (via its internal
//! Annex-B→AVCC conversion), wants. Each access unit is delimited by the
//! RTP marker bit (`marker=true` means "last packet of this frame").
//!
//! # Pipeline
//!
//! ```text
//!   RTSP client → InterleavedPacket.data → RtpPacket::parse → payload, marker
//!                                                            │
//!                                                            ▼
//!                                              H264Depacketizer::process
//!                                                            │
//!                                                            ▼
//!                                              Option<AccessUnit { annex_b, .. }>
//!                                                            │
//!                                                            ▼
//!                                              VTB / software H.264 decoder
//! ```

use crate::error::NetError;

/// RTP NAL "type" for STAP-A aggregation packets.
const RTP_TYPE_STAP_A: u8 = 24;
/// RTP NAL "type" for FU-A fragmentation units.
const RTP_TYPE_FU_A: u8 = 28;
/// H.264 NAL unit type 5 = coded slice of an IDR picture (keyframe).
const NAL_TYPE_IDR: u8 = 5;
/// Annex-B 4-byte start code prefixed in front of every emitted NAL.
const START_CODE: &[u8] = &[0x00, 0x00, 0x00, 0x01];

/// One complete H.264 access unit — i.e. one frame's worth of NAL units.
#[derive(Debug, Clone)]
pub struct AccessUnit {
    /// Annex-B framed bytestream (each NAL prefixed by `00 00 00 01`).
    pub annex_b: Vec<u8>,
    /// True if this access unit contains an IDR slice (RTP marker bit at
    /// the end of an AU that contained a NAL of type 5).
    pub keyframe: bool,
}

/// Stateful RFC 6184 depacketizer.
///
/// Feed each RTP packet's payload and marker bit to [`Self::process`];
/// receive `Some(AccessUnit)` whenever the marker bit signals end of
/// frame. The depacketizer holds internal buffers for FU-A reassembly
/// and the in-flight access unit; it is not [`Sync`] and is intended
/// for use from a single demuxer thread.
#[derive(Debug, Default)]
pub struct H264Depacketizer {
    /// Bytes accumulated for an FU-A in progress (with the synthesized
    /// NAL header byte at index 0).
    fu_buffer: Vec<u8>,
    /// True while we're between an FU-A "start" packet and its "end".
    fu_in_progress: bool,
    /// Annex-B bytes for the access unit currently being assembled.
    current_au: Vec<u8>,
    /// Set true the moment any NAL of type 5 (IDR) is appended to the
    /// current access unit.
    current_au_has_keyframe: bool,
}

impl H264Depacketizer {
    /// Construct an empty depacketizer.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::depacketize::H264Depacketizer;
    /// let dep = H264Depacketizer::new();
    /// assert!(!dep.has_pending_fragment());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True while we're mid-FU-A and waiting for further fragments.
    ///
    /// Useful for upstream sequence-loss detection: if the caller's
    /// [`SequenceTracker`](crate::rtsp::SequenceTracker) reports a gap
    /// while this returns `true`, the in-progress NAL is unrecoverable
    /// and should be dropped via [`Self::reset`].
    #[must_use]
    pub fn has_pending_fragment(&self) -> bool {
        self.fu_in_progress
    }

    /// Drop any in-progress fragment and unfinished access unit.
    ///
    /// Call after detecting RTP packet loss in the middle of an FU-A
    /// sequence, or when re-syncing after PAUSE/PLAY.
    pub fn reset(&mut self) {
        self.fu_buffer.clear();
        self.fu_in_progress = false;
        self.current_au.clear();
        self.current_au_has_keyframe = false;
    }

    /// Feed one RTP packet's H.264 payload.
    ///
    /// `payload` is the bytes *after* the RTP header (the slice returned
    /// by [`RtpPacket::parse`](crate::rtsp::RtpPacket::parse) as
    /// `pkt.payload`). `marker` is the RTP marker bit; per RFC 6184
    /// §5.1 this is set on the last packet of an access unit.
    ///
    /// Returns `Ok(Some(AccessUnit))` when `marker` is true and at least
    /// one NAL has accumulated since the last AU. Returns `Ok(None)`
    /// when more packets are needed.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Protocol`] for empty payloads, malformed
    /// STAP-A length prefixes, truncated FU-A headers, or RTP NAL types
    /// the depacketizer doesn't support (STAP-B, MTAP, FU-B).
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_net::depacketize::H264Depacketizer;
    ///
    /// // A single NAL unit packet carrying one byte of payload, with
    /// // marker=true to close the access unit.
    /// let mut dep = H264Depacketizer::new();
    /// let nal_header_type1 = 0x41; // F=0, NRI=2, type=1 (non-IDR slice)
    /// let payload = &[nal_header_type1, 0xAA];
    /// let au = dep.process(payload, true).unwrap().expect("AU emitted on marker");
    /// // Annex-B = start code + payload.
    /// assert_eq!(&au.annex_b[..4], &[0, 0, 0, 1]);
    /// assert_eq!(&au.annex_b[4..], payload);
    /// assert!(!au.keyframe); // type 1 is not IDR
    /// ```
    pub fn process(
        &mut self,
        payload: &[u8],
        marker: bool,
    ) -> Result<Option<AccessUnit>, NetError> {
        if payload.is_empty() {
            return Err(NetError::Protocol("empty H.264 RTP payload".into()));
        }
        let header = payload[0];
        let rtp_type = header & 0x1F;

        match rtp_type {
            1..=23 => self.append_nal(payload),
            RTP_TYPE_STAP_A => self.handle_stap_a(payload)?,
            RTP_TYPE_FU_A => self.handle_fu_a(payload)?,
            25 | 26 | 27 | 29 => {
                return Err(NetError::Protocol(format!(
                    "unsupported RTP H.264 packetization mode: NAL type {rtp_type}"
                )));
            }
            _ => {
                return Err(NetError::Protocol(format!(
                    "invalid H.264 RTP NAL type: {rtp_type}"
                )));
            }
        }

        if marker {
            Ok(self.take_access_unit())
        } else {
            Ok(None)
        }
    }

    /// Internal: append a single NAL unit to the in-flight access unit,
    /// prepending the Annex-B start code and tracking IDR presence.
    fn append_nal(&mut self, nal: &[u8]) {
        if nal.is_empty() {
            return;
        }
        let nal_type = nal[0] & 0x1F;
        if nal_type == NAL_TYPE_IDR {
            self.current_au_has_keyframe = true;
        }
        self.current_au.extend_from_slice(START_CODE);
        self.current_au.extend_from_slice(nal);
    }

    /// Internal: pull the assembled AU out, leaving the buffers empty.
    fn take_access_unit(&mut self) -> Option<AccessUnit> {
        if self.current_au.is_empty() {
            return None;
        }
        let annex_b = std::mem::take(&mut self.current_au);
        let keyframe = self.current_au_has_keyframe;
        self.current_au_has_keyframe = false;
        Some(AccessUnit { annex_b, keyframe })
    }

    /// STAP-A: payload = [STAP-A header byte][ (2-byte len)(NAL bytes) ]*
    fn handle_stap_a(&mut self, payload: &[u8]) -> Result<(), NetError> {
        let mut i = 1; // skip STAP-A header byte
        while i < payload.len() {
            if i + 2 > payload.len() {
                return Err(NetError::Protocol(
                    "STAP-A length field truncated".into(),
                ));
            }
            let len = u16::from_be_bytes([payload[i], payload[i + 1]]) as usize;
            i += 2;
            if len == 0 {
                return Err(NetError::Protocol("STAP-A zero-length NAL".into()));
            }
            if i + len > payload.len() {
                return Err(NetError::Protocol(
                    "STAP-A NAL extends past payload".into(),
                ));
            }
            self.append_nal(&payload[i..i + len]);
            i += len;
        }
        Ok(())
    }

    /// FU-A: payload = [FU indicator][FU header][fragment payload]
    ///
    /// FU indicator byte: F | NRI | type=28
    /// FU header byte: S | E | R | type
    ///   S = start, E = end, R = reserved (must be 0)
    ///
    /// The synthesized NAL header is `(indicator & 0xE0) | (header & 0x1F)`
    /// — i.e. F + NRI from the indicator, NAL type from the FU header.
    fn handle_fu_a(&mut self, payload: &[u8]) -> Result<(), NetError> {
        if payload.len() < 2 {
            return Err(NetError::Protocol("FU-A header truncated".into()));
        }
        let fu_indicator = payload[0];
        let fu_header = payload[1];
        let start = fu_header & 0x80 != 0;
        let end = fu_header & 0x40 != 0;
        let nal_type_inner = fu_header & 0x1F;

        if start {
            // Start of a new fragmented NAL. If we were already mid-FU
            // (a packet loss scenario), the partial buffer is unrecoverable
            // — drop it and start fresh.
            self.fu_buffer.clear();
            let nal_header = (fu_indicator & 0xE0) | nal_type_inner;
            self.fu_buffer.push(nal_header);
            self.fu_in_progress = true;
        }

        if !self.fu_in_progress {
            // Middle/end fragment without a corresponding start — packet
            // loss recovery. Skip silently; the upstream sequence tracker
            // is the right place to surface the loss.
            return Ok(());
        }

        // Append the fragment payload (everything after the 2-byte FU header).
        if payload.len() > 2 {
            self.fu_buffer.extend_from_slice(&payload[2..]);
        }

        if end {
            let nal = std::mem::take(&mut self.fu_buffer);
            self.fu_in_progress = false;
            self.append_nal(&nal);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a single-NAL RTP payload of the given NAL type with
    /// `body` as the payload bytes (after the NAL header byte).
    fn single_nal(nal_type: u8, body: &[u8]) -> Vec<u8> {
        // F=0, NRI=3 (highest priority), type=nal_type
        let header = (3u8 << 5) | (nal_type & 0x1F);
        let mut out = vec![header];
        out.extend_from_slice(body);
        out
    }

    /// Helper: build a STAP-A payload aggregating the given NAL units.
    fn stap_a(nals: &[&[u8]]) -> Vec<u8> {
        // F=0, NRI=3, type=24
        let header = (3u8 << 5) | RTP_TYPE_STAP_A;
        let mut out = vec![header];
        for nal in nals {
            let len = nal.len() as u16;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(nal);
        }
        out
    }

    /// Helper: build an FU-A packet for a NAL of `nal_type`, carrying
    /// `fragment` as the payload chunk, with start/end flags as requested.
    fn fu_a(nal_type: u8, start: bool, end: bool, fragment: &[u8]) -> Vec<u8> {
        // FU indicator: F=0, NRI=3, type=28
        let indicator = (3u8 << 5) | RTP_TYPE_FU_A;
        let mut fu_header = nal_type & 0x1F;
        if start {
            fu_header |= 0x80;
        }
        if end {
            fu_header |= 0x40;
        }
        let mut out = vec![indicator, fu_header];
        out.extend_from_slice(fragment);
        out
    }

    #[test]
    fn single_nal_packet_emits_au_on_marker() {
        let mut dep = H264Depacketizer::new();
        let payload = single_nal(1, b"slice-bytes");
        let au = dep
            .process(&payload, true)
            .expect("ok")
            .expect("AU emitted");
        assert_eq!(&au.annex_b[..4], &[0, 0, 0, 1]);
        assert_eq!(&au.annex_b[4..], payload.as_slice());
        assert!(!au.keyframe);
    }

    #[test]
    fn idr_single_nal_sets_keyframe_flag() {
        let mut dep = H264Depacketizer::new();
        let payload = single_nal(NAL_TYPE_IDR, b"idr-slice");
        let au = dep.process(&payload, true).unwrap().expect("AU emitted");
        assert!(au.keyframe);
    }

    #[test]
    fn no_marker_returns_none_and_buffers() {
        let mut dep = H264Depacketizer::new();
        let nal1 = single_nal(1, b"first");
        let nal2 = single_nal(1, b"second");
        assert!(dep.process(&nal1, false).unwrap().is_none());
        let au = dep.process(&nal2, true).unwrap().expect("AU");
        // Annex-B contains both NALs in order.
        assert!(au.annex_b.windows(5).any(|w| w == b"first"));
        assert!(au.annex_b.windows(6).any(|w| w == b"second"));
        // Two start codes.
        assert_eq!(
            au.annex_b
                .windows(4)
                .filter(|w| *w == [0, 0, 0, 1])
                .count(),
            2
        );
    }

    #[test]
    fn stap_a_emits_all_aggregated_nals() {
        // Typical real-world STAP-A: SPS (type 7) + PPS (type 8) bundled.
        let sps = single_nal(7, b"sps-body");
        let pps = single_nal(8, b"pps-body");
        let agg = stap_a(&[&sps, &pps]);

        let mut dep = H264Depacketizer::new();
        let au = dep.process(&agg, true).unwrap().expect("AU");
        // Both NALs should be present, each with its own start code.
        assert_eq!(
            au.annex_b
                .windows(4)
                .filter(|w| *w == [0, 0, 0, 1])
                .count(),
            2
        );
        assert!(au.annex_b.windows(sps.len()).any(|w| w == sps));
        assert!(au.annex_b.windows(pps.len()).any(|w| w == pps));
    }

    #[test]
    fn stap_a_with_idr_sets_keyframe() {
        let sps = single_nal(7, b"sps");
        let idr = single_nal(NAL_TYPE_IDR, b"idr");
        let agg = stap_a(&[&sps, &idr]);

        let mut dep = H264Depacketizer::new();
        let au = dep.process(&agg, true).unwrap().expect("AU");
        assert!(au.keyframe);
    }

    #[test]
    fn stap_a_truncated_length_field_errors() {
        // Header + only 1 byte of length field.
        let bad = vec![(3u8 << 5) | RTP_TYPE_STAP_A, 0x00];
        let mut dep = H264Depacketizer::new();
        let err = dep.process(&bad, true).unwrap_err();
        assert!(matches!(err, NetError::Protocol(_)));
    }

    #[test]
    fn stap_a_payload_overrun_errors() {
        // Header + length=100 + 5 bytes (claims 100, has 5).
        let mut bad = vec![(3u8 << 5) | RTP_TYPE_STAP_A];
        bad.extend_from_slice(&100u16.to_be_bytes());
        bad.extend_from_slice(b"short");
        let mut dep = H264Depacketizer::new();
        assert!(dep.process(&bad, true).is_err());
    }

    #[test]
    fn fu_a_reassembles_across_three_packets() {
        // A type-5 IDR split into 3 fragments.
        let part1 = fu_a(NAL_TYPE_IDR, true, false, b"PART-ONE-");
        let part2 = fu_a(NAL_TYPE_IDR, false, false, b"PART-TWO-");
        let part3 = fu_a(NAL_TYPE_IDR, false, true, b"PART-THREE");

        let mut dep = H264Depacketizer::new();
        assert!(dep.process(&part1, false).unwrap().is_none());
        assert!(dep.has_pending_fragment());
        assert!(dep.process(&part2, false).unwrap().is_none());
        assert!(dep.has_pending_fragment());
        // Final fragment + marker → AU emitted.
        let au = dep
            .process(&part3, true)
            .unwrap()
            .expect("AU after final fragment");

        assert!(!dep.has_pending_fragment());
        assert!(au.keyframe, "FU-A reassembled IDR should be flagged keyframe");
        // Reassembled NAL: synthesized header byte + "PART-ONE-PART-TWO-PART-THREE"
        let nal_header_expected = (3u8 << 5) | NAL_TYPE_IDR;
        assert_eq!(au.annex_b[4], nal_header_expected);
        assert_eq!(&au.annex_b[5..], b"PART-ONE-PART-TWO-PART-THREE");
    }

    #[test]
    fn fu_a_middle_fragment_without_start_is_dropped() {
        // Simulate packet loss: we never see the start, only middle + end.
        let middle = fu_a(NAL_TYPE_IDR, false, false, b"middle");
        let end = fu_a(NAL_TYPE_IDR, false, true, b"end");

        let mut dep = H264Depacketizer::new();
        assert!(dep.process(&middle, false).unwrap().is_none());
        assert!(dep.process(&end, true).unwrap().is_none());
        // No AU emitted because no start was seen.
    }

    #[test]
    fn fu_a_truncated_header_errors() {
        let bad = vec![(3u8 << 5) | RTP_TYPE_FU_A];
        let mut dep = H264Depacketizer::new();
        assert!(dep.process(&bad, true).is_err());
    }

    #[test]
    fn empty_payload_errors() {
        let mut dep = H264Depacketizer::new();
        assert!(dep.process(&[], true).is_err());
    }

    #[test]
    fn unsupported_rtp_types_error() {
        for unsup_type in [25u8, 26, 27, 29] {
            let header = (3u8 << 5) | unsup_type;
            let mut dep = H264Depacketizer::new();
            let err = dep.process(&[header, 0xAA], true).unwrap_err();
            assert!(
                matches!(err, NetError::Protocol(_)),
                "expected Protocol error for type {unsup_type}, got {err:?}"
            );
        }
    }

    #[test]
    fn invalid_zero_type_errors() {
        // Type 0 is reserved per H.264 spec.
        let bad = vec![3u8 << 5, 0xAA]; // F=0, NRI=3, type=0
        let mut dep = H264Depacketizer::new();
        assert!(dep.process(&bad, true).is_err());
    }

    #[test]
    fn reset_clears_in_progress_state() {
        let mut dep = H264Depacketizer::new();
        // Start an FU but never finish it.
        let part1 = fu_a(NAL_TYPE_IDR, true, false, b"orphan");
        dep.process(&part1, false).unwrap();
        assert!(dep.has_pending_fragment());

        dep.reset();
        assert!(!dep.has_pending_fragment());

        // After reset, the next single NAL should produce a clean AU.
        let payload = single_nal(1, b"clean");
        let au = dep.process(&payload, true).unwrap().expect("AU");
        // The orphaned fragment must not appear in the output.
        assert!(!au.annex_b.windows(6).any(|w| w == b"orphan"));
        assert!(au.annex_b.windows(5).any(|w| w == b"clean"));
    }

    #[test]
    fn two_consecutive_access_units() {
        // Frame 1: single NAL + marker.
        let mut dep = H264Depacketizer::new();
        let p1 = single_nal(NAL_TYPE_IDR, b"frame1");
        let au1 = dep.process(&p1, true).unwrap().expect("AU 1");
        assert!(au1.keyframe);
        assert!(au1.annex_b.windows(6).any(|w| w == b"frame1"));

        // Frame 2: another single NAL + marker, no carryover from frame 1.
        let p2 = single_nal(1, b"frame2");
        let au2 = dep.process(&p2, true).unwrap().expect("AU 2");
        assert!(!au2.keyframe);
        assert!(au2.annex_b.windows(6).any(|w| w == b"frame2"));
        assert!(!au2.annex_b.windows(6).any(|w| w == b"frame1"));
    }
}

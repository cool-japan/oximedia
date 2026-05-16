//! Public H.264 decoder facade.
//!
//! Bundles parameter-set extraction, format-description construction,
//! and the underlying `VTDecompressionSession` into a single
//! `H264Decoder` type with a `send_packet` / `receive_frame` API
//! shape that matches `oximedia_codec::VideoDecoder`.

use oximedia_codec::frame::VideoFrame;

use crate::error::VtbError;
use crate::format::H264FormatDescription;
use crate::nal::extract_sps_pps;
use crate::session::DecompressionSession;

/// Hardware-accelerated H.264 decoder backed by Apple VideoToolbox.
///
/// Construction takes the first SPS + PPS from an Annex-B bytestream
/// (the typical packetization for RTSP / RTP / network sources).
/// Subsequent packets are submitted with [`Self::send_packet`].
/// Decoded frames in NV12 (planar Y + interleaved CbCr) are pulled
/// out with [`Self::receive_frame`].
pub struct H264Decoder {
    session: DecompressionSession,
}

impl H264Decoder {
    /// Construct from a bytestream that contains at least the first SPS
    /// and PPS (with any number of additional NAL units that will be
    /// ignored — they aren't decoded here).
    pub fn from_extradata(annex_b: &[u8]) -> Result<Self, VtbError> {
        let (sps, pps) = extract_sps_pps(annex_b)?;
        let format = H264FormatDescription::from_parameter_sets(&sps, &pps)?;
        let session = DecompressionSession::new(format)?;
        Ok(Self { session })
    }

    /// Construct directly from explicit SPS / PPS payloads (without
    /// start codes). Useful when extradata is already separated by the
    /// container demuxer (e.g. MP4 `avcC` box parsing).
    pub fn from_parameter_sets(sps: &[u8], pps: &[u8]) -> Result<Self, VtbError> {
        let format = H264FormatDescription::from_parameter_sets(sps, pps)?;
        let session = DecompressionSession::new(format)?;
        Ok(Self { session })
    }

    /// Submit one Annex-B access unit to the decoder.
    ///
    /// PTS is in the 90 kHz H.264 timebase (i.e. ticks where one second
    /// is 90 000 ticks) — the conventional default.
    pub fn send_packet(&mut self, annex_b: &[u8], pts: i64) -> Result<(), VtbError> {
        self.session.decode_packet(annex_b, pts)
    }

    /// Pull a decoded frame if one is ready. Returns `None` if the
    /// decoder hasn't produced a frame yet (which can happen for the
    /// first few non-IDR packets while it builds its reference list).
    pub fn receive_frame(&mut self) -> Option<VideoFrame> {
        self.session.pull_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same SPS/PPS used elsewhere in this crate — Baseline 320x240.
    const SAMPLE_SPS: &[u8] = &[
        0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01, 0x40, 0x16, 0xE8, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40,
        0x00, 0x00, 0x0C, 0x03, 0xC5, 0x0A, 0x44,
    ];
    const SAMPLE_PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    /// Build an Annex-B extradata buffer with start codes.
    fn extradata() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(SAMPLE_SPS);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(SAMPLE_PPS);
        out
    }

    #[test]
    fn construct_from_extradata_and_drop() {
        let dec = H264Decoder::from_extradata(&extradata()).expect("decoder created");
        drop(dec);
    }

    #[test]
    fn construct_from_explicit_parameter_sets() {
        let dec = H264Decoder::from_parameter_sets(SAMPLE_SPS, SAMPLE_PPS)
            .expect("decoder created from explicit SPS/PPS");
        drop(dec);
    }

    #[test]
    fn missing_sps_or_pps_in_extradata_errors() {
        // Stream with only PPS, no SPS.
        let mut bad = Vec::new();
        bad.extend_from_slice(&[0, 0, 0, 1]);
        bad.extend_from_slice(SAMPLE_PPS);
        match H264Decoder::from_extradata(&bad) {
            Err(VtbError::MissingParameterSets(_)) => {}
            Err(other) => panic!("expected MissingParameterSets, got {other:?}"),
            Ok(_) => panic!("expected error for stream missing SPS"),
        }
    }

    #[test]
    fn receive_frame_returns_none_before_any_packet() {
        let mut dec = H264Decoder::from_parameter_sets(SAMPLE_SPS, SAMPLE_PPS).unwrap();
        assert!(dec.receive_frame().is_none());
    }
}

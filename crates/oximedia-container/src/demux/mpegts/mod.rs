//! MPEG Transport Stream (MPEG-TS) demuxer.
//!
//! This module implements a demuxer for MPEG-TS container format,
//! supporting patent-free codecs only (AV1, VP9, VP8, Opus, FLAC).
//!
//! # Features
//!
//! - PAT (Program Association Table) parsing
//! - PMT (Program Map Table) parsing
//! - PES (Packetized Elementary Stream) extraction
//! - PCR (Program Clock Reference) tracking
//! - Continuity counter validation
//! - Automatic patent-encumbered codec rejection
//!
//! # Example
//!
//! ```ignore
//! use oximedia_container::demux::{MpegTsDemuxer, Demuxer};
//! use oximedia_io::FileSource;
//!
//! let source = FileSource::open("video.ts").await?;
//! let mut demuxer = MpegTsDemuxer::new(source);
//! demuxer.probe().await?;
//!
//! for stream in demuxer.streams() {
//!     println!("Stream {}: {:?}", stream.index, stream.codec);
//! }
//!
//! while let Ok(packet) = demuxer.read_packet().await {
//!     // Process packet
//! }
//! ```

mod packet;
mod psi;
pub mod scte35;

use async_trait::async_trait;
use bytes::Bytes;
use oximedia_core::{OxiError, OxiResult, Rational, Timestamp};
use oximedia_io::MediaSource;
use std::collections::HashMap;

use crate::{
    CodecParams, ContainerFormat, Demuxer, Metadata, Packet, PacketFlags, ProbeResult, StreamInfo,
};
use packet::{ContinuityTracker, TsPacket, PAT_PID, TS_PACKET_SIZE};
use psi::{ElementaryStreamInfo, ProgramAssociationTable, ProgramMapTable, SectionAssembler};

/// Default timebase for MPEG-TS (90 kHz clock).
const MPEG_TS_TIMEBASE: Rational = Rational { num: 1, den: 90000 };

/// PES packet start code prefix.
const PES_START_CODE_PREFIX: u32 = 0x0000_0001;

/// Maximum PES packet size (16 MB - reasonable limit).
const MAX_PES_SIZE: usize = 16 * 1024 * 1024;

/// PES stream information for assembling packets.
#[derive(Debug, Clone)]
struct PesStreamState {
    /// Stream index in the demuxer.
    stream_index: usize,
    /// Elementary stream info.
    #[allow(dead_code)]
    es_info: ElementaryStreamInfo,
    /// Accumulated PES packet data.
    buffer: Vec<u8>,
    /// Expected PES packet length (0 = unbounded).
    expected_length: usize,
    /// Presentation timestamp.
    pts: Option<i64>,
    /// Decode timestamp.
    dts: Option<i64>,
    /// Whether this is a keyframe.
    is_keyframe: bool,
}

impl PesStreamState {
    /// Creates a new PES stream state.
    const fn new(stream_index: usize, es_info: ElementaryStreamInfo) -> Self {
        Self {
            stream_index,
            es_info,
            buffer: Vec::new(),
            expected_length: 0,
            pts: None,
            dts: None,
            is_keyframe: false,
        }
    }

    /// Resets the PES buffer.
    fn reset(&mut self) {
        self.buffer.clear();
        self.expected_length = 0;
        self.pts = None;
        self.dts = None;
        self.is_keyframe = false;
    }
}

/// MPEG-TS demuxer.
pub struct MpegTsDemuxer<S: MediaSource> {
    /// Media source.
    source: S,
    /// Stream information.
    streams: Vec<StreamInfo>,
    /// Program Association Table.
    pat: Option<ProgramAssociationTable>,
    /// Program Map Tables (one per program).
    pmts: HashMap<u16, ProgramMapTable>,
    /// Section assemblers for PSI tables.
    section_assemblers: HashMap<u16, SectionAssembler>,
    /// PES stream states.
    pes_streams: HashMap<u16, PesStreamState>,
    /// Continuity counter tracker.
    continuity_tracker: ContinuityTracker,
    /// Program Clock Reference (PCR) base value.
    pcr_base: Option<u64>,
    /// Whether the stream has been probed.
    probed: bool,
}

impl<S: MediaSource> MpegTsDemuxer<S> {
    /// Creates a new MPEG-TS demuxer.
    #[must_use]
    pub fn new(source: S) -> Self {
        Self {
            source,
            streams: Vec::new(),
            pat: None,
            pmts: HashMap::new(),
            section_assemblers: HashMap::new(),
            pes_streams: HashMap::new(),
            continuity_tracker: ContinuityTracker::new(),
            pcr_base: None,
            probed: false,
        }
    }

    /// Reads the next TS packet from the source.
    async fn read_ts_packet(&mut self) -> OxiResult<TsPacket> {
        let mut buffer = [0u8; TS_PACKET_SIZE];
        let mut bytes_read = 0;

        while bytes_read < TS_PACKET_SIZE {
            let n = self.source.read(&mut buffer[bytes_read..]).await?;
            if n == 0 {
                return Err(OxiError::Eof);
            }
            bytes_read += n;
        }

        TsPacket::parse(&buffer)
    }

    /// Processes a PAT packet.
    fn process_pat(&mut self, packet: &TsPacket) -> OxiResult<()> {
        let assembler = self.section_assemblers.entry(PAT_PID).or_default();

        if let Some(section_data) = assembler.push(&packet.payload, packet.payload_unit_start) {
            let pat = ProgramAssociationTable::parse(&section_data)?;

            // Create section assemblers for each PMT
            for &pmt_pid in pat.programs.values() {
                self.section_assemblers.entry(pmt_pid).or_default();
            }

            self.pat = Some(pat);
        }

        Ok(())
    }

    /// Processes a PMT packet.
    fn process_pmt(&mut self, pid: u16, packet: &TsPacket) -> OxiResult<()> {
        let assembler = self.section_assemblers.entry(pid).or_default();

        if let Some(section_data) = assembler.push(&packet.payload, packet.payload_unit_start) {
            let pmt = ProgramMapTable::parse(&section_data)?;

            // Register elementary streams
            for es_info in &pmt.streams {
                if let Some(codec_id) = es_info.codec_id {
                    if !self.pes_streams.contains_key(&es_info.pid) {
                        let stream_index = self.streams.len();

                        // Create stream info
                        let stream_info = StreamInfo {
                            index: stream_index,
                            codec: codec_id,
                            media_type: codec_id.media_type(),
                            timebase: MPEG_TS_TIMEBASE,
                            duration: None,
                            codec_params: CodecParams::default(),
                            metadata: Metadata::default(),
                            rotation: None,
                            display_matrix: None,
                        };

                        self.streams.push(stream_info);

                        // Create PES stream state
                        let pes_state = PesStreamState::new(stream_index, es_info.clone());
                        self.pes_streams.insert(es_info.pid, pes_state);
                    }
                }
            }

            self.pmts.insert(pmt.program_number, pmt);
        }

        Ok(())
    }

    /// Processes a PES packet.
    fn process_pes(&mut self, pid: u16, packet: &TsPacket) -> OxiResult<Option<Packet>> {
        let pes_state = self
            .pes_streams
            .get_mut(&pid)
            .ok_or_else(|| OxiError::InvalidData(format!("Unknown PES PID: 0x{pid:04X}")))?;

        // Check for PES start
        if packet.payload_unit_start {
            // If we have accumulated data, create a packet
            let has_data = !pes_state.buffer.is_empty();
            let _ = pes_state; // Release mutable borrow

            let result = if has_data {
                self.finalize_pes_packet(pid)
            } else {
                Ok(None)
            };

            // Reacquire mutable reference for reset
            let pes_state = self
                .pes_streams
                .get_mut(&pid)
                .ok_or_else(|| OxiError::InvalidData(format!("Unknown PES PID: 0x{pid:04X}")))?;
            pes_state.reset();

            // Parse PES header
            if packet.payload.len() >= 6 {
                let start_code = (u32::from(packet.payload[0]) << 16)
                    | (u32::from(packet.payload[1]) << 8)
                    | u32::from(packet.payload[2]);

                if start_code == PES_START_CODE_PREFIX {
                    let pes_packet_length =
                        (u16::from(packet.payload[4]) << 8) | u16::from(packet.payload[5]);

                    pes_state.expected_length = if pes_packet_length == 0 {
                        0 // Unbounded
                    } else {
                        pes_packet_length as usize + 6
                    };

                    // Parse PTS/DTS if present
                    if packet.payload.len() >= 9 {
                        let pts_dts_flags = (packet.payload[7] >> 6) & 0x03;
                        let header_data_length = packet.payload[8] as usize;

                        let mut offset = 9;

                        if pts_dts_flags >= 0x02 && packet.payload.len() >= offset + 5 {
                            // PTS present
                            pes_state.pts = Some(Self::parse_timestamp(&packet.payload[offset..]));
                            offset += 5;
                        }

                        if pts_dts_flags == 0x03 && packet.payload.len() >= offset + 5 {
                            // DTS present
                            pes_state.dts = Some(Self::parse_timestamp(&packet.payload[offset..]));
                        }

                        // Skip to payload
                        offset = 9 + header_data_length;
                        if offset < packet.payload.len() {
                            pes_state
                                .buffer
                                .extend_from_slice(&packet.payload[offset..]);
                        }
                    }

                    // Check for random access indicator
                    if packet.is_random_access() {
                        pes_state.is_keyframe = true;
                    }
                }
            }

            return result;
        }

        // Continue existing PES packet
        pes_state.buffer.extend_from_slice(&packet.payload);

        // Check size limit
        if pes_state.buffer.len() > MAX_PES_SIZE {
            return Err(OxiError::InvalidData(format!(
                "PES packet too large: {} bytes",
                pes_state.buffer.len()
            )));
        }

        // Check if PES packet is complete
        if pes_state.expected_length > 0 && pes_state.buffer.len() >= pes_state.expected_length - 6
        {
            return self.finalize_pes_packet(pid);
        }

        Ok(None)
    }

    /// Finalizes a PES packet and creates a media packet.
    fn finalize_pes_packet(&mut self, pid: u16) -> OxiResult<Option<Packet>> {
        let pes_state = self
            .pes_streams
            .get_mut(&pid)
            .ok_or_else(|| OxiError::InvalidData(format!("Unknown PES PID: 0x{pid:04X}")))?;

        if pes_state.buffer.is_empty() {
            return Ok(None);
        }

        let data = Bytes::copy_from_slice(&pes_state.buffer);
        let stream_index = pes_state.stream_index;

        let mut timestamp = Timestamp::new(
            pes_state.pts.unwrap_or(0),
            self.streams[stream_index].timebase,
        );
        timestamp.dts = pes_state.dts;

        let mut flags = PacketFlags::empty();
        if pes_state.is_keyframe {
            flags |= PacketFlags::KEYFRAME;
        }

        Ok(Some(Packet::new(stream_index, data, timestamp, flags)))
    }

    /// Parses a 33-bit PTS/DTS timestamp from PES header.
    fn parse_timestamp(data: &[u8]) -> i64 {
        ((i64::from(data[0]) & 0x0E) << 29)
            | (i64::from(data[1]) << 22)
            | ((i64::from(data[2]) & 0xFE) << 14)
            | (i64::from(data[3]) << 7)
            | (i64::from(data[4]) >> 1)
    }

    /// Checks if a PID is a PMT PID.
    fn is_pmt_pid(&self, pid: u16) -> bool {
        if let Some(ref pat) = self.pat {
            pat.programs.values().any(|&pmt_pid| pmt_pid == pid)
        } else {
            false
        }
    }
}

#[async_trait]
impl<S: MediaSource> Demuxer for MpegTsDemuxer<S> {
    async fn probe(&mut self) -> OxiResult<ProbeResult> {
        const MAX_PROBE_PACKETS: usize = 1000;

        if self.probed {
            return Ok(ProbeResult::new(ContainerFormat::MpegTs, 0.95));
        }

        // Read packets until we have PAT and at least one PMT
        let mut packets_read = 0;

        while packets_read < MAX_PROBE_PACKETS {
            let ts_packet = self.read_ts_packet().await?;

            // Track PCR
            if let Some(pcr) = ts_packet.pcr() {
                if self.pcr_base.is_none() {
                    self.pcr_base = Some(pcr);
                }
            }

            // Process packet based on PID
            if ts_packet.is_pat() {
                self.process_pat(&ts_packet)?;
            } else if self.is_pmt_pid(ts_packet.pid) {
                self.process_pmt(ts_packet.pid, &ts_packet)?;
            }

            // Check if we have found streams
            if !self.streams.is_empty() {
                break;
            }

            packets_read += 1;
        }

        if self.streams.is_empty() {
            return Err(OxiError::InvalidData(
                "No valid streams found in MPEG-TS".to_string(),
            ));
        }

        self.probed = true;

        Ok(ProbeResult::new(ContainerFormat::MpegTs, 0.95))
    }

    async fn read_packet(&mut self) -> OxiResult<Packet> {
        if !self.probed {
            return Err(OxiError::InvalidData(
                "Must call probe() before reading packets".to_string(),
            ));
        }

        loop {
            let ts_packet = self.read_ts_packet().await?;

            // Check continuity
            let has_payload = ts_packet.adaptation_field_control.has_payload();
            if !self.continuity_tracker.check(
                ts_packet.pid,
                ts_packet.continuity_counter,
                has_payload,
            ) {
                // Discontinuity detected - log but continue
                tracing::warn!(
                    "MPEG-TS continuity error on PID 0x{:04X}, CC={}",
                    ts_packet.pid,
                    ts_packet.continuity_counter
                );
            }

            // Update PCR
            if let Some(pcr) = ts_packet.pcr() {
                if self.pcr_base.is_none() {
                    self.pcr_base = Some(pcr);
                }
            }

            // Skip null packets
            if ts_packet.is_null() {
                continue;
            }

            // Process PAT/PMT
            if ts_packet.is_pat() {
                self.process_pat(&ts_packet)?;
                continue;
            }

            if self.is_pmt_pid(ts_packet.pid) {
                self.process_pmt(ts_packet.pid, &ts_packet)?;
                continue;
            }

            // Process PES
            if self.pes_streams.contains_key(&ts_packet.pid) {
                if let Some(packet) = self.process_pes(ts_packet.pid, &ts_packet)? {
                    return Ok(packet);
                }
            }
        }
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn is_seekable(&self) -> bool {
        self.source.is_seekable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::CodecId;
    use oximedia_io::MemorySource;

    #[tokio::test]
    async fn test_parse_timestamp() {
        // PTS = 0x000000000
        let data = [0x21, 0x00, 0x01, 0x00, 0x01];
        let pts = MpegTsDemuxer::<MemorySource>::parse_timestamp(&data);
        assert_eq!(pts, 0);

        // Test another value
        let data = [0x31, 0xFF, 0xFF, 0xFF, 0xFE];
        let pts = MpegTsDemuxer::<MemorySource>::parse_timestamp(&data);
        assert!(pts > 0);
    }

    #[test]
    fn test_pes_stream_state() {
        let es_info = ElementaryStreamInfo {
            stream_type: psi::StreamType::Av1,
            pid: 0x100,
            codec_id: Some(CodecId::Av1),
            descriptors: Vec::new(),
        };

        let mut state = PesStreamState::new(0, es_info);
        assert_eq!(state.stream_index, 0);
        assert_eq!(state.buffer.len(), 0);

        state.buffer.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(state.buffer.len(), 4);

        state.reset();
        assert_eq!(state.buffer.len(), 0);
        assert!(state.pts.is_none());
    }
}

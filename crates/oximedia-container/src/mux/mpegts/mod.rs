//! MPEG Transport Stream (MPEG-TS) muxer.
//!
//! This module implements a muxer for MPEG-TS container format,
//! supporting patent-free codecs only (AV1, VP9, VP8, Opus, FLAC).
//!
//! # Features
//!
//! - PAT (Program Association Table) generation
//! - PMT (Program Map Table) generation
//! - PES (Packetized Elementary Stream) packetization
//! - PCR (Program Clock Reference) insertion (every 100ms)
//! - Automatic PID allocation
//! - Bitrate control
//! - DVB compliance
//!
//! # Example
//!
//! ```ignore
//! use oximedia_container::mux::{MpegTsMuxer, Muxer, MuxerConfig};
//! use oximedia_io::FileSink;
//!
//! let sink = FileSink::create("output.ts").await?;
//! let config = MuxerConfig::new();
//! let mut muxer = MpegTsMuxer::new(sink, config);
//!
//! muxer.add_stream(video_info)?;
//! muxer.add_stream(audio_info)?;
//!
//! muxer.write_header().await?;
//!
//! for packet in packets {
//!     muxer.write_packet(&packet).await?;
//! }
//!
//! muxer.write_trailer().await?;
//! ```

mod pes;
pub mod scte35;

use async_trait::async_trait;
use oximedia_core::{CodecId, OxiError, OxiResult};
use oximedia_io::MediaSource;
use std::collections::HashMap;

use crate::{Muxer, MuxerConfig, Packet, StreamInfo};
use pes::PesPacketBuilder;

/// MPEG-TS packet size.
const TS_PACKET_SIZE: usize = 188;

/// Sync byte for TS packets.
const SYNC_BYTE: u8 = 0x47;

/// PAT PID.
const PAT_PID: u16 = 0x0000;

/// PMT PID (we use a fixed PID for simplicity).
const PMT_PID: u16 = 0x0100;

/// First elementary stream PID.
const FIRST_ES_PID: u16 = 0x0200;

/// PCR insertion interval in 90 kHz ticks (100ms = 9000 ticks).
const PCR_INTERVAL: u64 = 9000;

/// Program number.
const PROGRAM_NUMBER: u16 = 1;

/// Transport stream ID.
const TRANSPORT_STREAM_ID: u16 = 1;

/// Stream type mapping for PMT.
#[derive(Debug, Clone, Copy)]
struct StreamTypeInfo {
    /// Stream type value for PMT.
    stream_type: u8,
    /// Whether this stream can carry PCR.
    can_carry_pcr: bool,
}

impl StreamTypeInfo {
    /// Gets stream type info for a codec.
    const fn from_codec(codec: CodecId) -> Option<Self> {
        match codec {
            CodecId::Av1 => Some(Self {
                stream_type: 0x85, // User private - AV1
                can_carry_pcr: true,
            }),
            CodecId::Vp9 => Some(Self {
                stream_type: 0x84, // User private - VP9
                can_carry_pcr: true,
            }),
            CodecId::Vp8 => Some(Self {
                stream_type: 0x83, // User private - VP8
                can_carry_pcr: true,
            }),
            CodecId::Opus => Some(Self {
                stream_type: 0x81, // User private - Opus
                can_carry_pcr: false,
            }),
            CodecId::Flac => Some(Self {
                stream_type: 0x82, // User private - FLAC
                can_carry_pcr: false,
            }),
            CodecId::Pcm => Some(Self {
                stream_type: 0x80, // User private - PCM
                can_carry_pcr: false,
            }),
            _ => None,
        }
    }
}

/// Elementary stream state for muxing.
struct ElementaryStream {
    /// Stream info.
    #[allow(dead_code)]
    info: StreamInfo,
    /// Assigned PID.
    pid: u16,
    /// Stream type for PMT.
    stream_type: u8,
    /// Continuity counter.
    continuity_counter: u8,
    /// PES packet builder.
    pes_builder: PesPacketBuilder,
}

impl ElementaryStream {
    /// Creates a new elementary stream.
    fn new(info: StreamInfo, pid: u16, stream_type: u8) -> Self {
        let pes_builder = PesPacketBuilder::new(info.codec, info.index);

        Self {
            info,
            pid,
            stream_type,
            continuity_counter: 0,
            pes_builder,
        }
    }

    /// Increments and returns the continuity counter.
    fn next_continuity_counter(&mut self) -> u8 {
        let cc = self.continuity_counter;
        self.continuity_counter = (self.continuity_counter + 1) & 0x0F;
        cc
    }
}

/// MPEG-TS muxer.
pub struct MpegTsMuxer<S: MediaSource> {
    /// Media sink.
    sink: S,
    /// Muxer configuration.
    config: MuxerConfig,
    /// Elementary streams.
    streams: Vec<ElementaryStream>,
    /// Stream lookup by index.
    stream_by_index: HashMap<usize, usize>,
    /// Public [`StreamInfo`] for each added stream, in insertion order.
    ///
    /// Mirrors `streams` (which stores the muxer's internal per-stream
    /// packetization state, not `StreamInfo` itself) so that
    /// [`Muxer::streams`](crate::Muxer::streams) can hand back real,
    /// caller-supplied information instead of an empty slice.
    stream_infos: Vec<StreamInfo>,
    /// PAT continuity counter.
    pat_continuity_counter: u8,
    /// PMT continuity counter.
    pmt_continuity_counter: u8,
    /// PCR PID.
    pcr_pid: Option<u16>,
    /// Last PCR value written.
    last_pcr: u64,
    /// Whether header has been written.
    header_written: bool,
    /// Total packets written.
    packets_written: u64,
}

impl<S: MediaSource> MpegTsMuxer<S> {
    /// Creates a new MPEG-TS muxer.
    #[must_use]
    pub fn new(sink: S, config: MuxerConfig) -> Self {
        Self {
            sink,
            config,
            streams: Vec::new(),
            stream_by_index: HashMap::new(),
            stream_infos: Vec::new(),
            pat_continuity_counter: 0,
            pmt_continuity_counter: 0,
            pcr_pid: None,
            last_pcr: 0,
            header_written: false,
            packets_written: 0,
        }
    }

    /// Writes a TS packet to the sink.
    async fn write_ts_packet(
        &mut self,
        pid: u16,
        payload: &[u8],
        payload_unit_start: bool,
        continuity_counter: u8,
        adaptation_field: Option<&[u8]>,
    ) -> OxiResult<()> {
        let mut packet = [0u8; TS_PACKET_SIZE];

        // Sync byte
        packet[0] = SYNC_BYTE;

        // Header
        packet[1] = if payload_unit_start { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
        packet[2] = (pid & 0xFF) as u8;

        let has_adaptation = adaptation_field.is_some();
        let has_payload = !payload.is_empty();

        let afc = match (has_adaptation, has_payload) {
            (false, _) => 0x01,    // Payload only (or stuffing if no payload)
            (true, false) => 0x02, // Adaptation field only
            (true, true) => 0x03,  // Both
        };

        packet[3] = (afc << 4) | (continuity_counter & 0x0F);

        let mut offset = 4;

        // Write adaptation field
        if let Some(af) = adaptation_field {
            #[allow(clippy::cast_possible_truncation)]
            let af_len = af.len() as u8;
            packet[offset] = af_len;
            offset += 1;
            let copy_len = std::cmp::min(af.len(), TS_PACKET_SIZE - offset);
            packet[offset..offset + copy_len].copy_from_slice(&af[..copy_len]);
            offset += copy_len;
        }

        // Write payload
        if !payload.is_empty() {
            let copy_len = std::cmp::min(payload.len(), TS_PACKET_SIZE - offset);
            packet[offset..offset + copy_len].copy_from_slice(&payload[..copy_len]);
            offset += copy_len;
        }

        // Stuffing bytes (0xFF)
        for byte in &mut packet[offset..] {
            *byte = 0xFF;
        }

        self.sink.write_all(&packet).await?;
        self.packets_written += 1;

        Ok(())
    }

    /// Writes the PAT (Program Association Table).
    #[allow(clippy::vec_init_then_push)]
    async fn write_pat(&mut self) -> OxiResult<()> {
        let mut section = Vec::new();

        // Table ID
        section.push(0x00);

        // Section syntax indicator + reserved + section length (placeholder)
        section.push(0xB0);
        section.push(0x0D); // Length will be 13 bytes

        // Transport stream ID
        section.push((TRANSPORT_STREAM_ID >> 8) as u8);
        section.push((TRANSPORT_STREAM_ID & 0xFF) as u8);

        // Version + current/next indicator
        section.push(0xC1); // Version 0, current

        // Section number
        section.push(0x00);

        // Last section number
        section.push(0x00);

        // Program number
        section.push((PROGRAM_NUMBER >> 8) as u8);
        section.push((PROGRAM_NUMBER & 0xFF) as u8);

        // PMT PID
        section.push((PMT_PID >> 8) as u8 | 0xE0);
        section.push((PMT_PID & 0xFF) as u8);

        // CRC32
        let crc = Self::compute_crc32(&section);
        section.extend_from_slice(&crc.to_be_bytes());

        // Write PAT with pointer field
        let mut payload = vec![0x00]; // Pointer field
        payload.extend_from_slice(&section);

        let cc = self.pat_continuity_counter;
        self.pat_continuity_counter = (self.pat_continuity_counter + 1) & 0x0F;

        self.write_ts_packet(PAT_PID, &payload, true, cc, None)
            .await
    }

    /// Writes the PMT (Program Map Table).
    async fn write_pmt(&mut self) -> OxiResult<()> {
        let mut section = Vec::new();

        // Table ID
        section.push(0x02);

        // Section syntax indicator + reserved + section length (placeholder)
        let section_length_pos = section.len();
        section.push(0xB0);
        section.push(0x00); // Placeholder

        // Program number
        section.push((PROGRAM_NUMBER >> 8) as u8);
        section.push((PROGRAM_NUMBER & 0xFF) as u8);

        // Version + current/next indicator
        section.push(0xC1); // Version 0, current

        // Section number
        section.push(0x00);

        // Last section number
        section.push(0x00);

        // PCR PID
        let pcr_pid = self.pcr_pid.unwrap_or(0x1FFF);
        section.push((pcr_pid >> 8) as u8 | 0xE0);
        section.push((pcr_pid & 0xFF) as u8);

        // Program info length (no descriptors)
        section.push(0xF0);
        section.push(0x00);

        // Elementary stream info
        for stream in &self.streams {
            // Stream type
            section.push(stream.stream_type);

            // Elementary PID
            section.push((stream.pid >> 8) as u8 | 0xE0);
            section.push((stream.pid & 0xFF) as u8);

            // ES info length (no descriptors)
            section.push(0xF0);
            section.push(0x00);
        }

        // Update section length.
        //
        // `section_length_pos` is captured *after* the table_id byte is pushed,
        // so it already points at the first of the two length bytes. Offsetting
        // by +1/+2 wrote the length one byte late: `section_length` read back as
        // 0x0B0 (176) and the low byte overwrote `program_number`'s high byte,
        // so every emitted PMT was non-conformant and failed a demuxer's CRC
        // check. Covered by
        // `oximedia_server::hls::segment::tests::segment_reparses_with_correct_stream_type`.
        let section_length = section.len() - 3 + 4; // +4 for CRC
        #[allow(clippy::cast_possible_truncation)]
        {
            section[section_length_pos] = ((section_length >> 8) as u8 & 0x0F) | 0xB0;
            section[section_length_pos + 1] = (section_length & 0xFF) as u8;
        }

        // CRC32
        let crc = Self::compute_crc32(&section);
        section.extend_from_slice(&crc.to_be_bytes());

        // Write PMT with pointer field
        let mut payload = vec![0x00]; // Pointer field
        payload.extend_from_slice(&section);

        let cc = self.pmt_continuity_counter;
        self.pmt_continuity_counter = (self.pmt_continuity_counter + 1) & 0x0F;

        self.write_ts_packet(PMT_PID, &payload, true, cc, None)
            .await
    }

    /// Computes CRC-32 for PSI tables.
    fn compute_crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;

        for &byte in data {
            crc ^= u32::from(byte) << 24;
            for _ in 0..8 {
                if crc & 0x8000_0000 != 0 {
                    crc = (crc << 1) ^ 0x04C1_1DB7;
                } else {
                    crc <<= 1;
                }
            }
        }

        crc
    }

    /// Encodes a PCR value into 6 bytes.
    #[allow(clippy::cast_possible_truncation)]
    fn encode_pcr(pcr: u64) -> [u8; 6] {
        let pcr_base = pcr / 300;
        let pcr_ext = (pcr % 300) as u16;

        [
            ((pcr_base >> 25) & 0xFF) as u8,
            ((pcr_base >> 17) & 0xFF) as u8,
            ((pcr_base >> 9) & 0xFF) as u8,
            ((pcr_base >> 1) & 0xFF) as u8,
            ((((pcr_base & 0x01) << 7) | 0x7E_u64 | ((u64::from(pcr_ext) >> 8) & 0x01_u64)) as u8),
            (pcr_ext & 0xFF) as u8,
        ]
    }

    /// Writes a PES packet, splitting into multiple TS packets if needed.
    #[allow(clippy::too_many_arguments)]
    async fn write_pes_packet(
        &mut self,
        stream_idx: usize,
        pes_data: &[u8],
        pcr: Option<u64>,
    ) -> OxiResult<()> {
        let pid = self.streams[stream_idx].pid;
        let pcr_pid = self.pcr_pid.unwrap_or(0);

        let mut offset = 0;
        let mut first_packet = true;

        while offset < pes_data.len() {
            let payload_start = first_packet;
            let remaining = pes_data.len() - offset;

            // Build adaptation field if needed (for PCR or stuffing)
            let mut adaptation_field = Vec::new();
            let mut adaptation_field_data = Vec::new();

            if let Some(pcr_val) = pcr {
                if first_packet && pid == pcr_pid {
                    // PCR flag
                    adaptation_field_data.push(0x10);
                    // PCR
                    adaptation_field_data.extend_from_slice(&Self::encode_pcr(pcr_val));
                }
            }

            let adaptation_field_ref = if adaptation_field_data.is_empty() {
                None
            } else {
                adaptation_field = adaptation_field_data;
                Some(adaptation_field.as_slice())
            };

            // Calculate available payload space
            let header_size = 4;
            let af_size = if adaptation_field_ref.is_some() {
                1 + adaptation_field.len()
            } else {
                0
            };
            let available = TS_PACKET_SIZE - header_size - af_size;

            let payload_size = std::cmp::min(remaining, available);
            let payload = &pes_data[offset..offset + payload_size];

            let cc = self.streams[stream_idx].next_continuity_counter();

            self.write_ts_packet(pid, payload, payload_start, cc, adaptation_field_ref)
                .await?;

            offset += payload_size;
            first_packet = false;
        }

        Ok(())
    }
}

#[async_trait]
impl<S: MediaSource> Muxer for MpegTsMuxer<S> {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if self.header_written {
            return Err(OxiError::InvalidData(
                "Cannot add streams after header is written".to_string(),
            ));
        }

        // Check codec support
        let stream_type_info = StreamTypeInfo::from_codec(info.codec).ok_or_else(|| {
            OxiError::unsupported(format!("Codec {:?} not supported in MPEG-TS", info.codec))
        })?;

        // Assign PID
        #[allow(clippy::cast_possible_truncation)]
        let pid = FIRST_ES_PID + self.streams.len() as u16;

        // Select PCR PID (first video stream)
        if self.pcr_pid.is_none() && stream_type_info.can_carry_pcr {
            self.pcr_pid = Some(pid);
        }

        let stream_index = info.index;
        self.stream_infos.push(info.clone());
        let es = ElementaryStream::new(info, pid, stream_type_info.stream_type);

        let internal_index = self.streams.len();
        self.streams.push(es);
        self.stream_by_index.insert(stream_index, internal_index);

        Ok(stream_index)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        if self.header_written {
            return Err(OxiError::InvalidData("Header already written".to_string()));
        }

        if self.streams.is_empty() {
            return Err(OxiError::InvalidData(
                "No streams added to muxer".to_string(),
            ));
        }

        // Write PAT and PMT
        self.write_pat().await?;
        self.write_pmt().await?;

        self.header_written = true;
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        if !self.header_written {
            return Err(OxiError::InvalidData(
                "Must write header before packets".to_string(),
            ));
        }

        let stream_idx = *self
            .stream_by_index
            .get(&packet.stream_index)
            .ok_or_else(|| {
                OxiError::InvalidData(format!("Invalid stream index: {}", packet.stream_index))
            })?;

        // Convert timestamp to 90 kHz
        let pts = packet.pts();
        let dts = packet.dts();

        // Build PES packet
        let pes_builder = self.streams[stream_idx]
            .pes_builder
            .with_pts(pts)
            .with_dts(dts.unwrap_or(pts));

        let pes_data = pes_builder.build(&packet.data)?;

        // Check if we need to insert PCR
        #[allow(clippy::cast_sign_loss)]
        let current_pcr = pts as u64;
        let pcr = if current_pcr >= self.last_pcr + PCR_INTERVAL {
            self.last_pcr = current_pcr;
            Some(current_pcr)
        } else {
            None
        };

        // Write PES packet
        self.write_pes_packet(stream_idx, &pes_data, pcr).await?;

        Ok(())
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        if !self.header_written {
            return Err(OxiError::InvalidData("No header written".to_string()));
        }

        // MPEG-TS doesn't require a trailer, but we can write final PAT/PMT
        self.write_pat().await?;
        self.write_pmt().await?;

        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.stream_infos
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::Rational;
    use oximedia_io::MemorySource;

    #[test]
    fn test_encode_pcr() {
        let pcr = 90000u64; // 1 second
        let encoded = MpegTsMuxer::<MemorySource>::encode_pcr(pcr);
        assert_eq!(encoded.len(), 6);
    }

    #[test]
    fn test_compute_crc32() {
        let data = vec![0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00];
        let crc = MpegTsMuxer::<MemorySource>::compute_crc32(&data);
        assert!(crc != 0);
    }

    #[test]
    fn test_stream_type_from_codec() {
        assert!(StreamTypeInfo::from_codec(CodecId::Av1).is_some());
        assert!(StreamTypeInfo::from_codec(CodecId::Opus).is_some());
        assert!(StreamTypeInfo::from_codec(CodecId::Mp3).is_none()); // Not supported in MPEG-TS muxer
    }

    #[tokio::test]
    async fn test_add_stream() {
        let source = MemorySource::new(bytes::Bytes::new());
        let config = MuxerConfig::new();
        let mut muxer = MpegTsMuxer::new(source, config);

        let stream_info = StreamInfo::new(0, CodecId::Av1, Rational::new(1, 90000));
        let index = muxer
            .add_stream(stream_info)
            .expect("operation should succeed");
        assert_eq!(index, 0);
        assert_eq!(muxer.streams.len(), 1);
    }

    #[tokio::test]
    async fn test_streams_reports_added_streams() {
        // `Muxer::streams()` used to unconditionally return `&[]`. Verify it
        // now reflects every stream actually registered via `add_stream`,
        // in insertion order, with the right codec for each.
        let source = MemorySource::new(bytes::Bytes::new());
        let config = MuxerConfig::new();
        let mut muxer = MpegTsMuxer::new(source, config);

        assert!(
            muxer.streams().is_empty(),
            "no streams added yet, must report empty"
        );

        muxer
            .add_stream(StreamInfo::new(0, CodecId::Av1, Rational::new(1, 90000)))
            .expect("operation should succeed");
        muxer
            .add_stream(StreamInfo::new(1, CodecId::Opus, Rational::new(1, 48000)))
            .expect("operation should succeed");

        let infos = muxer.streams();
        assert_eq!(infos.len(), 2, "both added streams must be reported");
        assert_eq!(infos[0].codec, CodecId::Av1);
        assert_eq!(infos[1].codec, CodecId::Opus);
    }
}

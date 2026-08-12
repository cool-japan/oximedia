//! Ogg muxer implementation.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

use async_trait::async_trait;
use oximedia_core::{CodecId, OxiError, OxiResult};
use oximedia_io::MediaSource;

use crate::mux::traits::{Muxer, MuxerConfig};
use crate::{Packet, StreamInfo};

use super::stream::OggStreamWriter;

// ============================================================================
// Constants
// ============================================================================

/// Ogg page magic bytes.
const OGG_MAGIC: &[u8; 4] = b"OggS";

/// Maximum page size (header + 255 segments * 255 bytes each).
const MAX_PAGE_PAYLOAD: usize = 255 * 255;

// ============================================================================
// Ogg Muxer
// ============================================================================

/// Ogg container muxer.
///
/// Creates Ogg container files from compressed audio packets.
pub struct OggMuxer<W> {
    /// The underlying writer.
    sink: W,

    /// Muxer configuration.
    config: MuxerConfig,

    /// Registered streams.
    streams: Vec<StreamInfo>,

    /// Ogg stream writers (one per stream).
    stream_writers: Vec<OggStreamWriter>,

    /// Whether header has been written.
    header_written: bool,

    /// Current write position.
    position: u64,
}

impl<W> OggMuxer<W> {
    /// Creates a new Ogg muxer.
    ///
    /// # Arguments
    ///
    /// * `sink` - The writer to output to
    /// * `config` - Muxer configuration
    #[must_use]
    pub fn new(sink: W, config: MuxerConfig) -> Self {
        Self {
            sink,
            config,
            streams: Vec::new(),
            stream_writers: Vec::new(),
            header_written: false,
            position: 0,
        }
    }

    /// Returns a reference to the underlying sink.
    #[must_use]
    pub const fn sink(&self) -> &W {
        &self.sink
    }

    /// Returns a mutable reference to the underlying sink.
    pub fn sink_mut(&mut self) -> &mut W {
        &mut self.sink
    }

    /// Consumes the muxer and returns the underlying sink.
    #[must_use]
    #[allow(dead_code)]
    pub fn into_sink(self) -> W {
        self.sink
    }

    /// Generates a serial number for a stream.
    fn generate_serial(stream_index: usize) -> u32 {
        // Simple serial generation based on index
        // In production, this should be random
        0x1234_5678_u32.wrapping_add(stream_index as u32)
    }
}

impl<W: MediaSource> OggMuxer<W> {
    /// Writes bytes to the sink.
    async fn write_bytes(&mut self, data: &[u8]) -> OxiResult<()> {
        self.sink.write_all(data).await?;
        self.position += data.len() as u64;
        Ok(())
    }

    /// Writes an Ogg page.
    async fn write_page(&mut self, page: &OggPage) -> OxiResult<()> {
        let data = page.to_bytes();
        self.write_bytes(&data).await
    }

    /// Writes BOS (Beginning of Stream) pages for all streams.
    async fn write_bos_pages(&mut self) -> OxiResult<()> {
        // Collect all pages first to avoid borrow issues
        let mut pages_to_write = Vec::new();

        for i in 0..self.streams.len() {
            let stream = &self.streams[i];

            // Build identification header based on codec
            let id_header = match stream.codec {
                CodecId::Opus => build_opus_head(stream),
                CodecId::Vorbis => build_vorbis_id_header(stream),
                CodecId::Flac => build_flac_header(),
                _ => {
                    return Err(OxiError::unsupported(format!(
                        "Codec {:?} not supported in Ogg",
                        stream.codec
                    )))
                }
            };

            // Build BOS page
            let page = self.stream_writers[i].build_page(&id_header, true, false, true);
            pages_to_write.push(page);
        }

        // Write all pages
        for page in &pages_to_write {
            self.write_page(page).await?;
        }

        Ok(())
    }

    /// Writes secondary header pages (comments, etc.).
    async fn write_header_pages(&mut self) -> OxiResult<()> {
        // Collect all pages first to avoid borrow issues
        let mut pages_to_write = Vec::new();

        // Clone config to avoid borrow issues
        let config = self.config.clone();

        for i in 0..self.streams.len() {
            let stream = &self.streams[i];

            // Build comment header based on codec
            let comment_header = match stream.codec {
                CodecId::Opus => build_opus_tags(&config),
                CodecId::Vorbis => build_vorbis_comment(&config),
                CodecId::Flac => build_flac_comment(&config),
                _ => Vec::new(),
            };

            if !comment_header.is_empty() {
                let page = self.stream_writers[i].build_page(&comment_header, false, false, true);
                pages_to_write.push(page);
            }

            // Vorbis needs a third header (codebook)
            if stream.codec == CodecId::Vorbis {
                if let Some(ref extradata) = stream.codec_params.extradata {
                    // Extract setup header from extradata
                    if let Some(setup_header) = extract_vorbis_setup(extradata) {
                        let page =
                            self.stream_writers[i].build_page(&setup_header, false, false, true);
                        pages_to_write.push(page);
                    }
                }
            }
        }

        // Write all pages
        for page in &pages_to_write {
            self.write_page(page).await?;
        }

        Ok(())
    }

    /// Converts a timestamp to granule position.
    #[allow(clippy::cast_precision_loss)]
    fn to_granule(stream: &StreamInfo, pts: i64) -> u64 {
        let sample_rate = stream.codec_params.sample_rate.unwrap_or(48000);

        if stream.timebase.den == 0 {
            return pts as u64;
        }

        // Convert from timebase to samples
        let samples = (pts as f64 * stream.timebase.num as f64 / stream.timebase.den as f64)
            * f64::from(sample_rate);
        samples.max(0.0) as u64
    }
}

#[async_trait]
impl<W: MediaSource> Muxer for OggMuxer<W> {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if self.header_written {
            return Err(OxiError::InvalidData(
                "Cannot add stream after header is written".into(),
            ));
        }

        // Validate codec is supported
        match info.codec {
            CodecId::Opus | CodecId::Vorbis | CodecId::Flac => {}
            _ => {
                return Err(OxiError::unsupported(format!(
                    "Codec {:?} not supported in Ogg",
                    info.codec
                )))
            }
        }

        let index = self.streams.len();
        let serial = Self::generate_serial(index);
        let writer = OggStreamWriter::new(serial);

        self.streams.push(info);
        self.stream_writers.push(writer);
        Ok(index)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        if self.header_written {
            return Err(OxiError::InvalidData("Header already written".into()));
        }

        if self.streams.is_empty() {
            return Err(OxiError::InvalidData("No streams configured".into()));
        }

        // Write BOS pages for all streams
        self.write_bos_pages().await?;

        // Write secondary header pages
        self.write_header_pages().await?;

        self.header_written = true;
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        if !self.header_written {
            return Err(OxiError::InvalidData("Header not written".into()));
        }

        if packet.stream_index >= self.streams.len() {
            return Err(OxiError::InvalidData(format!(
                "Invalid stream index: {}",
                packet.stream_index
            )));
        }

        let stream = &self.streams[packet.stream_index];
        let granule = Self::to_granule(stream, packet.pts());

        // Handle large packets that need to span multiple pages
        let data = packet.data.to_vec();
        if data.len() <= MAX_PAGE_PAYLOAD {
            // Single page
            let page = self.stream_writers[packet.stream_index]
                .build_page_with_granule(&data, false, false, false, granule);
            self.write_page(&page).await?;
        } else {
            // Multiple pages
            let mut offset = 0;
            while offset < data.len() {
                let remaining = data.len() - offset;
                let chunk_size = remaining.min(MAX_PAGE_PAYLOAD);
                let chunk = &data[offset..offset + chunk_size];
                let is_continuation = offset > 0;
                let is_complete = offset + chunk_size >= data.len();

                let page_granule = if is_complete { granule } else { u64::MAX };
                let page = self.stream_writers[packet.stream_index].build_page_with_granule(
                    chunk,
                    is_continuation,
                    false,
                    false,
                    page_granule,
                );
                self.write_page(&page).await?;
                offset += chunk_size;
            }
        }

        Ok(())
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        if !self.header_written {
            return Err(OxiError::InvalidData("Header not written".into()));
        }

        // Write EOS pages for all streams
        for writer in &mut self.stream_writers {
            let granule = writer.last_granule();
            let page = writer.build_page_with_granule(&[], false, true, true, granule);
            let data = page.to_bytes();
            self.sink.write_all(&data).await?;
            self.position += data.len() as u64;
        }

        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

// ============================================================================
// Ogg Page Structure
// ============================================================================

/// An Ogg page for writing.
#[derive(Debug)]
pub struct OggPage {
    /// Stream structure version (always 0).
    pub version: u8,

    /// Page flags.
    pub flags: u8,

    /// Absolute granule position.
    pub granule_position: u64,

    /// Stream serial number.
    pub serial_number: u32,

    /// Page sequence number.
    pub page_sequence: u32,

    /// Segment table.
    pub segments: Vec<u8>,

    /// Page data.
    pub data: Vec<u8>,
}

impl OggPage {
    /// Creates a new Ogg page.
    #[must_use]
    pub fn new(serial: u32, sequence: u32) -> Self {
        Self {
            version: 0,
            flags: 0,
            granule_position: 0,
            serial_number: serial,
            page_sequence: sequence,
            segments: Vec::new(),
            data: Vec::new(),
        }
    }

    /// Converts the page to bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(27 + self.segments.len() + self.data.len());

        // Magic
        bytes.extend_from_slice(OGG_MAGIC);

        // Version
        bytes.push(self.version);

        // Flags
        bytes.push(self.flags);

        // Granule position (64-bit LE)
        bytes.extend_from_slice(&self.granule_position.to_le_bytes());

        // Serial number (32-bit LE)
        bytes.extend_from_slice(&self.serial_number.to_le_bytes());

        // Page sequence (32-bit LE)
        bytes.extend_from_slice(&self.page_sequence.to_le_bytes());

        // CRC placeholder (will be filled in)
        let crc_offset = bytes.len();
        bytes.extend_from_slice(&[0, 0, 0, 0]);

        // Segment count
        bytes.push(self.segments.len() as u8);

        // Segment table
        bytes.extend_from_slice(&self.segments);

        // Data
        bytes.extend_from_slice(&self.data);

        // Calculate and set CRC
        let crc = crc32_ogg(&bytes);
        bytes[crc_offset..crc_offset + 4].copy_from_slice(&crc.to_le_bytes());

        bytes
    }
}

// ============================================================================
// CRC32 Calculation
// ============================================================================

/// Calculates CRC32 for Ogg page verification.
#[must_use]
fn crc32_ogg(data: &[u8]) -> u32 {
    const CRC_TABLE: [u32; 256] = generate_crc_table();
    let mut crc = 0u32;

    for (i, &byte) in data.iter().enumerate() {
        // Skip the CRC field itself (bytes 22-25)
        let input_byte = if (22..26).contains(&i) { 0 } else { byte };
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) as u8 ^ input_byte) as usize];
    }

    crc
}

/// Generates the CRC32 lookup table for Ogg.
const fn generate_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

// ============================================================================
// Codec Header Builders
// ============================================================================

/// Builds an Opus identification header.
fn build_opus_head(stream: &StreamInfo) -> Vec<u8> {
    let mut header = Vec::with_capacity(19);

    // Magic signature
    header.extend_from_slice(b"OpusHead");

    // Version (always 1)
    header.push(1);

    // Channel count
    let channels = stream.codec_params.channels.unwrap_or(2);
    header.push(channels);

    // Pre-skip (samples to skip at start)
    header.extend_from_slice(&312u16.to_le_bytes());

    // Input sample rate (for playback gain calculation)
    let sample_rate = stream.codec_params.sample_rate.unwrap_or(48000);
    header.extend_from_slice(&sample_rate.to_le_bytes());

    // Output gain (dB * 256, signed)
    header.extend_from_slice(&0i16.to_le_bytes());

    // Channel mapping family
    header.push(u8::from(channels > 2));

    // If mapping family > 0, add mapping table
    if channels > 2 {
        // Stream count
        header.push(1);
        // Coupled stream count
        header.push(0);
        // Channel mapping
        for i in 0..channels {
            header.push(i);
        }
    }

    header
}

/// Builds an Opus comment header.
fn build_opus_tags(config: &MuxerConfig) -> Vec<u8> {
    let mut header = Vec::new();

    // Magic signature
    header.extend_from_slice(b"OpusTags");

    // Vendor string
    let vendor = config.muxing_app.as_deref().unwrap_or("OxiMedia");
    header.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    header.extend_from_slice(vendor.as_bytes());

    // User comment list length
    let mut comments = Vec::new();
    if let Some(ref title) = config.title {
        comments.push(format!("TITLE={title}"));
    }

    header.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for comment in comments {
        header.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        header.extend_from_slice(comment.as_bytes());
    }

    header
}

/// Builds a Vorbis identification header.
fn build_vorbis_id_header(stream: &StreamInfo) -> Vec<u8> {
    let mut header = Vec::new();

    // Packet type (1 = identification)
    header.push(1);

    // Vorbis magic
    header.extend_from_slice(b"vorbis");

    // Vorbis version (always 0)
    header.extend_from_slice(&0u32.to_le_bytes());

    // Channel count
    header.push(stream.codec_params.channels.unwrap_or(2));

    // Sample rate
    let sample_rate = stream.codec_params.sample_rate.unwrap_or(44100);
    header.extend_from_slice(&sample_rate.to_le_bytes());

    // Bitrate maximum
    header.extend_from_slice(&0i32.to_le_bytes());

    // Bitrate nominal
    header.extend_from_slice(&128_000_i32.to_le_bytes());

    // Bitrate minimum
    header.extend_from_slice(&0i32.to_le_bytes());

    // Block sizes (4-bit each)
    header.push(0xB8); // Block size 0: 256, Block size 1: 2048

    // Framing flag
    header.push(1);

    header
}

/// Builds a Vorbis comment header.
fn build_vorbis_comment(config: &MuxerConfig) -> Vec<u8> {
    let mut header = Vec::new();

    // Packet type (3 = comment)
    header.push(3);

    // Vorbis magic
    header.extend_from_slice(b"vorbis");

    // Vendor string
    let vendor = config.muxing_app.as_deref().unwrap_or("OxiMedia");
    header.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    header.extend_from_slice(vendor.as_bytes());

    // User comment list
    let mut comments = Vec::new();
    if let Some(ref title) = config.title {
        comments.push(format!("TITLE={title}"));
    }

    header.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for comment in comments {
        header.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        header.extend_from_slice(comment.as_bytes());
    }

    // Framing flag
    header.push(1);

    header
}

/// Splits packed Vorbis header extradata into its three constituent header
/// packets: identification, comment, and setup.
///
/// Vorbis always has exactly three header packets (Vorbis I spec §4.2.1,
/// "Header decode and decode setup"), each beginning with a 1-byte packet
/// type (`1`/`3`/`5`) followed by the 6-byte `"vorbis"` magic. Ogg itself
/// frames each as its own page/packet, but outside of Ogg page framing
/// (e.g. as a single `StreamInfo::codec_params.extradata` blob handed to
/// this muxer) the three packets need their own container-level framing to
/// know where one ends and the next begins. Two such framings are actually
/// produced elsewhere in this crate and are both handled here, tried in
/// order:
///
/// 1. **Xiph-laced framing** — populated by
///    `crate::demux::matroska::mod` from the raw Matroska `CodecPrivate`
///    element, which is the format the Matroska/WebM spec mandates for
///    `A_VORBIS` tracks (and the same convention FFmpeg's own
///    `AVCodecContext.extradata` uses for Vorbis): a leading
///    `packet_count - 1` byte (always `2` for Vorbis's fixed three
///    packets), followed by 2 Xiph-laced lengths — each a run of `0xFF`
///    continuation bytes plus a final byte `< 0xFF` — for the first two
///    packets, followed by all three raw packets concatenated back to
///    back (the third packet's length is implied by whatever remains).
///    This is the same lacing scheme
///    `crate::demux::matroska::parser::parse_xiph_lacing` reads for
///    Matroska Block lacing.
/// 2. **Length-prefixed framing** — populated by
///    `crate::demux::ogg::mod::OggDemuxer` for its own round-trip use: no
///    leading count byte, just `[u16 LE len][packet]` repeated for each
///    header (`[u16 LE len0][packet0][u16 LE len1][packet1][u16 LE
///    len2][packet2]` for Vorbis's three packets).
///
/// A candidate split — from either framing — is accepted only once all
/// three resulting packets have been confirmed to start with their
/// expected packet-type byte and the `"vorbis"` magic; a split that landed
/// on the right byte ranges by pure coincidence without the right framing
/// is never honored. Returns `None` if neither framing produces a
/// validated split.
fn split_vorbis_headers(extradata: &[u8]) -> Option<[Vec<u8>; 3]> {
    split_vorbis_headers_xiph_laced(extradata)
        .or_else(|| split_vorbis_headers_length_prefixed(extradata))
}

/// Parses the Matroska/WebM/FFmpeg-style Xiph-laced framing. See
/// [`split_vorbis_headers`] for the exact byte layout.
fn split_vorbis_headers_xiph_laced(extradata: &[u8]) -> Option<[Vec<u8>; 3]> {
    let (&packet_count_minus_one, rest) = extradata.split_first()?;
    if packet_count_minus_one != 2 {
        return None;
    }

    let mut offset = 0usize;
    let mut lengths = [0usize; 2];
    for length in &mut lengths {
        let mut total = 0usize;
        loop {
            let byte = *rest.get(offset)?;
            offset += 1;
            total = total.checked_add(usize::from(byte))?;
            if byte != 0xFF {
                break;
            }
        }
        *length = total;
    }

    let payload = rest.get(offset..)?;
    let comment_end = lengths[0].checked_add(lengths[1])?;
    let id_header = payload.get(..lengths[0])?;
    let comment_header = payload.get(lengths[0]..comment_end)?;
    let setup_header = payload.get(comment_end..)?;

    validate_vorbis_headers(id_header, comment_header, setup_header)
}

/// Parses this crate's own `OggDemuxer` length-prefixed framing. See
/// [`split_vorbis_headers`] for the exact byte layout.
fn split_vorbis_headers_length_prefixed(extradata: &[u8]) -> Option<[Vec<u8>; 3]> {
    let mut offset = 0usize;
    let mut headers: [&[u8]; 3] = [&[], &[], &[]];
    for header in &mut headers {
        let len_bytes = extradata.get(offset..offset + 2)?;
        let len = usize::from(u16::from_le_bytes([len_bytes[0], len_bytes[1]]));
        offset += 2;
        *header = extradata.get(offset..offset + len)?;
        offset += len;
    }
    // Vorbis extradata carries exactly 3 packets; leftover trailing bytes
    // mean this wasn't actually 3-packet length-prefixed Vorbis framing, so
    // the parse is rejected rather than silently ignoring the remainder.
    if offset != extradata.len() {
        return None;
    }

    validate_vorbis_headers(headers[0], headers[1], headers[2])
}

/// Confirms `id`/`comment`/`setup` each begin with their expected Vorbis
/// packet-type byte (`1`/`3`/`5`) and the `"vorbis"` magic, per the Vorbis I
/// spec's common header packet framing (§4.2.1). Returns the three packets
/// (owned) only once all three check out.
fn validate_vorbis_headers(id: &[u8], comment: &[u8], setup: &[u8]) -> Option<[Vec<u8>; 3]> {
    for (header, expected_type) in [(id, 1u8), (comment, 3u8), (setup, 5u8)] {
        if header.len() < 7 || header[0] != expected_type || &header[1..7] != b"vorbis" {
            return None;
        }
    }
    Some([id.to_vec(), comment.to_vec(), setup.to_vec()])
}

/// Extracts the Vorbis setup header (packet type 5) from packed extradata.
///
/// See [`split_vorbis_headers`] for the exact three-header framing this
/// recognizes. Returns `None` if `extradata` is not validly-framed packed
/// Vorbis header data, rather than guessing at a byte range.
fn extract_vorbis_setup(extradata: &[u8]) -> Option<Vec<u8>> {
    split_vorbis_headers(extradata).map(|[_id, _comment, setup]| setup)
}

/// Builds a FLAC header marker.
fn build_flac_header() -> Vec<u8> {
    let mut header = Vec::new();

    // FLAC marker in Ogg
    header.push(0x7F);
    header.extend_from_slice(b"FLAC");

    // Version
    header.push(1); // Major
    header.push(0); // Minor

    // Number of non-audio packets
    header.extend_from_slice(&0u16.to_be_bytes());

    // fLaC signature
    header.extend_from_slice(b"fLaC");

    // Minimal STREAMINFO block
    header.push(0x80); // Last metadata block, type = STREAMINFO
    header.extend_from_slice(&34u32.to_be_bytes()[1..]); // Size: 34 bytes

    // STREAMINFO data (34 bytes of defaults)
    header.extend_from_slice(&[0u8; 34]);

    header
}

/// Builds a FLAC Vorbis comment.
fn build_flac_comment(config: &MuxerConfig) -> Vec<u8> {
    let mut header = Vec::new();

    // FLAC comment block type
    header.push(0x84); // Type 4 (VORBIS_COMMENT) with last block flag

    // Build comment content
    let mut content = Vec::new();

    // Vendor string
    let vendor = config.muxing_app.as_deref().unwrap_or("OxiMedia");
    content.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    content.extend_from_slice(vendor.as_bytes());

    // User comments
    let mut comments = Vec::new();
    if let Some(ref title) = config.title {
        comments.push(format!("TITLE={title}"));
    }

    content.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for comment in comments {
        content.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        content.extend_from_slice(comment.as_bytes());
    }

    // Size (24-bit big-endian)
    let size_bytes = (content.len() as u32).to_be_bytes();
    header.extend_from_slice(&size_bytes[1..]);

    header.extend(content);
    header
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use oximedia_core::{Rational, Timestamp};
    use oximedia_io::MemorySource;

    fn create_opus_stream() -> StreamInfo {
        let mut stream = StreamInfo::new(0, CodecId::Opus, Rational::new(1, 48000));
        stream.codec_params.sample_rate = Some(48000);
        stream.codec_params.channels = Some(2);
        stream
    }

    fn create_vorbis_stream(extradata: Vec<u8>) -> StreamInfo {
        let mut stream = StreamInfo::new(0, CodecId::Vorbis, Rational::new(1, 44100));
        stream.codec_params.sample_rate = Some(44100);
        stream.codec_params.channels = Some(2);
        stream.codec_params.extradata = Some(Bytes::from(extradata));
        stream
    }

    // ─── Vorbis header packing test helpers ────────────────────────────────

    /// Encodes `len` as a Xiph-laced length: a run of `0xFF` continuation
    /// bytes plus a final byte `< 0xFF` — the inverse of the decode loop in
    /// `split_vorbis_headers_xiph_laced`.
    fn xiph_lace_len(mut len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        while len >= 0xFF {
            out.push(0xFF);
            len -= 0xFF;
        }
        out.push(len as u8);
        out
    }

    /// Packs three header packets using the Matroska/WebM/FFmpeg-style
    /// Xiph-laced framing `split_vorbis_headers_xiph_laced` parses.
    fn pack_vorbis_headers_xiph(id: &[u8], comment: &[u8], setup: &[u8]) -> Vec<u8> {
        let mut out = vec![2u8]; // packet_count - 1 == 2 (three packets)
        out.extend(xiph_lace_len(id.len()));
        out.extend(xiph_lace_len(comment.len()));
        out.extend_from_slice(id);
        out.extend_from_slice(comment);
        out.extend_from_slice(setup);
        out
    }

    /// Packs three header packets using this crate's own `OggDemuxer`-style
    /// length-prefixed framing `split_vorbis_headers_length_prefixed` parses.
    fn pack_vorbis_headers_length_prefixed(id: &[u8], comment: &[u8], setup: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for header in [id, comment, setup] {
            out.extend_from_slice(&(header.len() as u16).to_le_bytes());
            out.extend_from_slice(header);
        }
        out
    }

    /// A minimal but spec-shaped synthetic Vorbis identification packet:
    /// packet type 1 + `"vorbis"` magic + placeholder id-header fields.
    fn synthetic_id_header() -> Vec<u8> {
        let mut h = vec![1u8];
        h.extend_from_slice(b"vorbis");
        h.extend_from_slice(&[0u8; 16]);
        h
    }

    /// A minimal synthetic Vorbis comment packet (type 3) of adjustable size.
    fn synthetic_comment_header(padding: usize) -> Vec<u8> {
        let mut h = vec![3u8];
        h.extend_from_slice(b"vorbis");
        h.extend(vec![0u8; padding]);
        h
    }

    /// A minimal synthetic Vorbis setup packet (type 5) of adjustable size.
    fn synthetic_setup_header(padding: usize) -> Vec<u8> {
        let mut h = vec![5u8];
        h.extend_from_slice(b"vorbis");
        h.extend(vec![0xCDu8; padding]);
        h
    }

    #[test]
    fn test_split_vorbis_headers_xiph_laced_round_trip() {
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(5);
        let setup = synthetic_setup_header(40);
        let packed = pack_vorbis_headers_xiph(&id, &comment, &setup);

        let split = split_vorbis_headers(&packed).expect("valid xiph-laced framing must split");
        assert_eq!(split[0], id);
        assert_eq!(split[1], comment);
        assert_eq!(split[2], setup);
    }

    #[test]
    fn test_split_vorbis_headers_xiph_laced_multi_byte_length() {
        // A 300-byte comment header forces the Xiph-lacing continuation
        // byte (0xFF) to actually be exercised, not just a single length
        // byte (300 = 255 + 45, encoded as [0xFF, 45]).
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(300 - 7); // total packet len 300
        assert_eq!(comment.len(), 300);
        let setup = synthetic_setup_header(10);
        let packed = pack_vorbis_headers_xiph(&id, &comment, &setup);

        let split = split_vorbis_headers(&packed).expect("multi-byte xiph-laced length must split");
        assert_eq!(split[1], comment);
        assert_eq!(split[2], setup);
    }

    #[test]
    fn test_split_vorbis_headers_length_prefixed_round_trip() {
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(12);
        let setup = synthetic_setup_header(64);
        let packed = pack_vorbis_headers_length_prefixed(&id, &comment, &setup);

        let split =
            split_vorbis_headers(&packed).expect("valid length-prefixed framing must split");
        assert_eq!(split[0], id);
        assert_eq!(split[1], comment);
        assert_eq!(split[2], setup);
    }

    #[test]
    fn test_extract_vorbis_setup_matches_split_for_both_framings() {
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(5);
        let setup = synthetic_setup_header(30);

        let xiph_packed = pack_vorbis_headers_xiph(&id, &comment, &setup);
        assert_eq!(extract_vorbis_setup(&xiph_packed), Some(setup.clone()));

        let length_prefixed_packed = pack_vorbis_headers_length_prefixed(&id, &comment, &setup);
        assert_eq!(extract_vorbis_setup(&length_prefixed_packed), Some(setup));
    }

    #[test]
    fn test_extract_vorbis_setup_empty_extradata() {
        assert_eq!(extract_vorbis_setup(&[]), None);
    }

    #[test]
    fn test_extract_vorbis_setup_rejects_wrong_packet_count() {
        // packet_count - 1 must read back as 2 for Vorbis's fixed three
        // headers; a value of 1 (claiming only two packets) must not parse.
        let mut bogus = vec![1u8];
        bogus.extend(xiph_lace_len(5));
        bogus.extend_from_slice(&synthetic_id_header());
        assert_eq!(split_vorbis_headers_xiph_laced(&bogus), None);
        assert_eq!(extract_vorbis_setup(&bogus), None);
    }

    #[test]
    fn test_extract_vorbis_setup_rejects_truncated_length_prefix() {
        // A Xiph-laced length whose continuation run never terminates
        // (runs off the end of the buffer) must not panic and must parse
        // to `None`.
        let truncated = vec![2u8, 0xFF, 0xFF, 0xFF];
        assert_eq!(split_vorbis_headers_xiph_laced(&truncated), None);
        assert_eq!(extract_vorbis_setup(&truncated), None);
    }

    #[test]
    fn test_extract_vorbis_setup_rejects_lengths_overrunning_buffer() {
        // Declares an id-header length (200) far larger than the actual
        // remaining payload — must not panic on out-of-bounds slicing.
        let mut bogus = vec![2u8, 200, 5];
        bogus.extend_from_slice(&[0u8; 10]);
        assert_eq!(split_vorbis_headers_xiph_laced(&bogus), None);
        assert_eq!(extract_vorbis_setup(&bogus), None);
    }

    #[test]
    fn test_extract_vorbis_setup_rejects_bad_magic() {
        // Byte-correct lengths, but the "setup" packet doesn't actually
        // start with the vorbis magic — must be rejected, not returned as
        // if it were a real setup header.
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(4);
        let mut corrupt_setup = synthetic_setup_header(10);
        corrupt_setup[1] = b'X'; // corrupt the "vorbis" magic
        let packed = pack_vorbis_headers_xiph(&id, &comment, &corrupt_setup);
        assert_eq!(extract_vorbis_setup(&packed), None);
    }

    #[test]
    fn test_extract_vorbis_setup_rejects_length_prefixed_trailing_garbage() {
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(5);
        let setup = synthetic_setup_header(5);
        let mut packed = pack_vorbis_headers_length_prefixed(&id, &comment, &setup);
        packed.push(0xFF); // trailing byte that belongs to no packet
        assert_eq!(split_vorbis_headers_length_prefixed(&packed), None);
    }

    #[tokio::test]
    async fn test_muxer_writes_vorbis_setup_page_from_packed_extradata() {
        let id = synthetic_id_header();
        let comment = synthetic_comment_header(4);
        let setup = synthetic_setup_header(50);
        let extradata = pack_vorbis_headers_xiph(&id, &comment, &setup);

        let sink = MemorySource::new_writable(8192);
        let config = MuxerConfig::new();
        let mut muxer = OggMuxer::new(sink, config);

        let vorbis = create_vorbis_stream(extradata);
        muxer.add_stream(vorbis).expect("operation should succeed");
        muxer
            .write_header()
            .await
            .expect("operation should succeed");

        let written = muxer.sink().written_data();
        let found = written
            .windows(setup.len())
            .any(|window| window == setup.as_slice());
        assert!(
            found,
            "the exact setup header bytes must appear in the written Ogg header pages"
        );
    }

    #[test]
    fn test_ogg_page_new() {
        let page = OggPage::new(0x1234, 0);
        assert_eq!(page.serial_number, 0x1234);
        assert_eq!(page.page_sequence, 0);
        assert_eq!(page.version, 0);
    }

    #[test]
    fn test_ogg_page_to_bytes() {
        let mut page = OggPage::new(1, 0);
        page.flags = 0x02; // BOS
        page.granule_position = 0;
        page.segments = vec![5];
        page.data = vec![1, 2, 3, 4, 5];

        let bytes = page.to_bytes();

        // Check magic
        assert_eq!(&bytes[0..4], b"OggS");
        // Check version
        assert_eq!(bytes[4], 0);
        // Check flags
        assert_eq!(bytes[5], 0x02);
    }

    #[test]
    fn test_crc32_ogg() {
        let mut data = vec![0u8; 32];
        // Add some non-zero data
        data[10] = 0x42;
        data[20] = 0xFF;
        let crc = crc32_ogg(&data);
        // CRC of non-zero data should be non-zero
        assert_ne!(crc, 0);
    }

    #[test]
    fn test_build_opus_head() {
        let stream = create_opus_stream();
        let header = build_opus_head(&stream);

        assert!(header.len() >= 19);
        assert_eq!(&header[0..8], b"OpusHead");
        assert_eq!(header[8], 1); // Version
        assert_eq!(header[9], 2); // Channels
    }

    #[test]
    fn test_build_opus_tags() {
        let config = MuxerConfig::new().with_title("Test");
        let header = build_opus_tags(&config);

        assert!(header.len() >= 8);
        assert_eq!(&header[0..8], b"OpusTags");
    }

    #[test]
    fn test_build_vorbis_id_header() {
        let stream = create_opus_stream();
        let header = build_vorbis_id_header(&stream);

        assert!(header.len() >= 23);
        assert_eq!(header[0], 1); // Packet type
        assert_eq!(&header[1..7], b"vorbis");
    }

    #[tokio::test]
    async fn test_muxer_new() {
        let sink = MemorySource::new_writable(4096);
        let config = MuxerConfig::new();
        let muxer = OggMuxer::new(sink, config);

        assert!(!muxer.header_written);
        assert!(muxer.streams.is_empty());
    }

    #[tokio::test]
    async fn test_muxer_add_stream() {
        let sink = MemorySource::new_writable(4096);
        let config = MuxerConfig::new();
        let mut muxer = OggMuxer::new(sink, config);

        let opus = create_opus_stream();
        let idx = muxer.add_stream(opus).expect("operation should succeed");

        assert_eq!(idx, 0);
        assert_eq!(muxer.streams.len(), 1);
    }

    #[tokio::test]
    async fn test_muxer_add_unsupported_stream() {
        let sink = MemorySource::new_writable(4096);
        let config = MuxerConfig::new();
        let mut muxer = OggMuxer::new(sink, config);

        let video = StreamInfo::new(0, CodecId::Vp9, Rational::new(1, 1000));
        let result = muxer.add_stream(video);

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_muxer_write_header() {
        let sink = MemorySource::new_writable(4096);
        let config = MuxerConfig::new();
        let mut muxer = OggMuxer::new(sink, config);

        let opus = create_opus_stream();
        muxer.add_stream(opus).expect("operation should succeed");

        let result = muxer.write_header().await;
        assert!(result.is_ok());
        assert!(muxer.header_written);
    }

    #[tokio::test]
    async fn test_muxer_write_packet() {
        let sink = MemorySource::new_writable(4096);
        let config = MuxerConfig::new();
        let mut muxer = OggMuxer::new(sink, config);

        let opus = create_opus_stream();
        muxer.add_stream(opus).expect("operation should succeed");
        muxer
            .write_header()
            .await
            .expect("operation should succeed");

        let packet = Packet::new(
            0,
            Bytes::from_static(&[1, 2, 3, 4]),
            Timestamp::new(0, Rational::new(1, 48000)),
            crate::PacketFlags::KEYFRAME,
        );

        let result = muxer.write_packet(&packet).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_muxer_write_trailer() {
        let sink = MemorySource::new_writable(4096);
        let config = MuxerConfig::new();
        let mut muxer = OggMuxer::new(sink, config);

        let opus = create_opus_stream();
        muxer.add_stream(opus).expect("operation should succeed");
        muxer
            .write_header()
            .await
            .expect("operation should succeed");

        let result = muxer.write_trailer().await;
        assert!(result.is_ok());
    }
}

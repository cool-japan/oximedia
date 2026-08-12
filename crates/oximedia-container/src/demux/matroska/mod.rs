//! Matroska/`WebM` demuxer.
//!
//! This module provides a demuxer for Matroska (.mkv) and `WebM` (.webm)
//! container formats. Both use the EBML (Extensible Binary Meta Language)
//! format for structure.
//!
//! # `WebM` vs Matroska
//!
//! `WebM` is a subset of Matroska with restrictions:
//! - Video: VP8, VP9, or AV1 only
//! - Audio: Vorbis or Opus only
//! - No subtitles (except `WebVTT` in some implementations)
//!
//! # Example
//!
//! ```ignore
//! use oximedia_container::demux::MatroskaDemuxer;
//! use oximedia_io::FileSource;
//!
//! let source = FileSource::open("video.mkv").await?;
//! let mut demuxer = MatroskaDemuxer::new(source);
//!
//! let probe = demuxer.probe().await?;
//! println!("Detected: {:?}", probe.format);
//!
//! while let Ok(packet) = demuxer.read_packet().await {
//!     println!("Packet from stream {}: {} bytes",
//!              packet.stream_index, packet.size());
//! }
//! ```

pub mod ebml;
pub mod matroska_v4;
pub mod parser;
pub mod types;

use std::collections::HashMap;
use std::io::SeekFrom;

use async_trait::async_trait;
use bytes::Bytes;
use oximedia_core::{OxiError, OxiResult, Rational, Timestamp};
use oximedia_io::MediaSource;

use crate::demux::Demuxer;
use crate::DecodeSkipCursor;
use crate::{CodecParams, ContainerFormat, Metadata, Packet, PacketFlags, ProbeResult, StreamInfo};

use ebml::element_id;
use parser::MatroskaParser;
#[allow(clippy::wildcard_imports)]
use types::*;

// ============================================================================
// Constants
// ============================================================================

/// Default buffer size for reading.
const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64 KB

/// Maximum header size to buffer for initial parsing.
const MAX_HEADER_SIZE: usize = 16 * 1024 * 1024; // 16 MB

/// Nanoseconds per millisecond.
const NS_PER_MS: u64 = 1_000_000;

// ============================================================================
// Matroska Demuxer
// ============================================================================

/// Matroska/`WebM` demuxer.
///
/// Parses Matroska and `WebM` container formats, extracting stream
/// information and compressed packets.
pub struct MatroskaDemuxer<R> {
    /// The underlying reader.
    source: R,

    /// Read buffer.
    buffer: Vec<u8>,

    // ========================================================================
    // Parsed Metadata
    // ========================================================================
    /// EBML header.
    ebml_header: Option<EbmlHeader>,

    /// Segment information.
    segment_info: Option<SegmentInfo>,

    /// Parsed tracks.
    tracks: Vec<TrackEntry>,

    /// Cue points for seeking.
    cues: Vec<CuePoint>,

    /// Editions/chapters.
    editions: Vec<Edition>,

    /// Tags metadata.
    tags: Vec<Tag>,

    /// Seek head entries.
    seek_entries: Vec<SeekEntry>,

    // ========================================================================
    // Stream Mapping
    // ========================================================================
    /// Stream information for each track.
    streams: Vec<StreamInfo>,

    /// Map from Matroska track number to stream index.
    track_to_stream: HashMap<u64, usize>,

    // ========================================================================
    // Demuxing State
    // ========================================================================
    /// Position of segment data start (after EBML header).
    segment_start: u64,

    /// Current cluster state.
    current_cluster: Option<ClusterState>,

    /// Current position in the source.
    position: u64,

    /// Whether we've reached end of file.
    eof: bool,

    /// Whether headers have been parsed.
    header_parsed: bool,

    /// Detected format.
    format: Option<ContainerFormat>,

    /// Sample-accurate seek target: skip packets with pts < this value.
    ///
    /// Set by [`seek_sample_accurate`](MatroskaDemuxer::seek_sample_accurate) and
    /// cleared once a matching packet is found in [`read_packet`].
    skip_until: Option<u64>,
}

impl<R> MatroskaDemuxer<R> {
    /// Creates a new Matroska demuxer with the given source.
    ///
    /// After creation, call [`probe`](Demuxer::probe) to parse headers
    /// and detect streams.
    #[must_use]
    pub fn new(source: R) -> Self {
        Self {
            source,
            buffer: Vec::with_capacity(DEFAULT_BUFFER_SIZE),
            ebml_header: None,
            segment_info: None,
            tracks: Vec::new(),
            cues: Vec::new(),
            editions: Vec::new(),
            tags: Vec::new(),
            seek_entries: Vec::new(),
            streams: Vec::new(),
            track_to_stream: HashMap::new(),
            segment_start: 0,
            current_cluster: None,
            position: 0,
            eof: false,
            header_parsed: false,
            format: None,
            skip_until: None,
        }
    }

    /// Returns a reference to the underlying source.
    #[must_use]
    pub const fn source(&self) -> &R {
        &self.source
    }

    /// Returns a mutable reference to the underlying source.
    pub fn source_mut(&mut self) -> &mut R {
        &mut self.source
    }

    /// Consumes the demuxer and returns the underlying source.
    #[must_use]
    #[allow(dead_code)]
    pub fn into_source(self) -> R {
        self.source
    }

    /// Returns the segment info if parsed.
    #[must_use]
    pub fn segment_info(&self) -> Option<&SegmentInfo> {
        self.segment_info.as_ref()
    }

    /// Returns the parsed tracks.
    #[must_use]
    pub fn tracks(&self) -> &[TrackEntry] {
        &self.tracks
    }

    /// Returns the cue points.
    #[must_use]
    pub fn cues(&self) -> &[CuePoint] {
        &self.cues
    }

    /// Returns the editions/chapters.
    #[must_use]
    pub fn editions(&self) -> &[Edition] {
        &self.editions
    }

    /// Returns the tags.
    #[must_use]
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }
}

impl<R: MediaSource> MatroskaDemuxer<R> {
    /// Reads data into the buffer, ensuring at least `min_size` bytes are available.
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails.
    async fn ensure_buffer(&mut self, min_size: usize) -> OxiResult<()> {
        while self.buffer.len() < min_size && !self.eof {
            let mut temp = vec![0u8; DEFAULT_BUFFER_SIZE];
            let n = self.source.read(&mut temp).await?;
            if n == 0 {
                self.eof = true;
                break;
            }
            self.buffer.extend_from_slice(&temp[..n]);
            self.position += n as u64;
        }
        Ok(())
    }

    /// Seeks to a file position and clears the read buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if seeking fails.
    async fn seek_to_position(&mut self, position: u64) -> OxiResult<()> {
        self.source.seek(SeekFrom::Start(position)).await?;
        self.position = position;
        self.buffer.clear();
        self.eof = false;
        self.current_cluster = None;
        Ok(())
    }

    /// Consumes bytes from the buffer.
    fn consume_buffer(&mut self, n: usize) {
        self.buffer.drain(..n);
    }

    /// Reads a specific number of bytes from the source.
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails or EOF is reached.
    #[allow(dead_code)]
    async fn read_bytes(&mut self, size: usize) -> OxiResult<Vec<u8>> {
        self.ensure_buffer(size).await?;
        if self.buffer.len() < size {
            return Err(OxiError::UnexpectedEof);
        }
        let data = self.buffer[..size].to_vec();
        self.consume_buffer(size);
        Ok(data)
    }

    /// Seeks to a position in the source.
    ///
    /// # Errors
    ///
    /// Returns an error if seeking fails.
    #[allow(dead_code)]
    async fn seek_to(&mut self, pos: u64) -> OxiResult<()> {
        self.source.seek(SeekFrom::Start(pos)).await?;
        self.position = pos;
        self.buffer.clear();
        self.eof = false;
        Ok(())
    }

    /// Parses the EBML header and segment elements.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing fails.
    async fn parse_headers(&mut self) -> OxiResult<()> {
        // Read enough data to parse headers
        self.ensure_buffer(MAX_HEADER_SIZE.min(1024 * 1024)).await?;

        // Parse EBML header
        let (header, consumed) = parser::parse_ebml_header(&self.buffer)?;
        self.ebml_header = Some(header);
        self.consume_buffer(consumed);

        // Determine format from DocType
        if let Some(ref hdr) = self.ebml_header {
            self.format = Some(match hdr.doc_type {
                DocType::WebM => ContainerFormat::WebM,
                DocType::Matroska => ContainerFormat::Matroska,
            });
        }

        // Parse segment element header
        self.ensure_buffer(12).await?;
        let mut parser = MatroskaParser::new(&self.buffer);
        let segment = parser.read_element()?;

        if segment.id != element_id::SEGMENT {
            return Err(OxiError::Parse {
                offset: self.position,
                message: format!("Expected Segment, got 0x{:X}", segment.id),
            });
        }

        self.segment_start = consumed as u64 + segment.header_size as u64;
        self.consume_buffer(segment.header_size);

        // Parse segment children
        self.parse_segment_children().await?;

        // `parse_segment_children` stops at the first Cluster (so headers
        // can be parsed without buffering the whole file), so it only ever
        // sees a Cues element that precedes every Cluster. Muxers that
        // write Cues in the trailer (this crate's own `MatroskaMuxer`
        // included, matching common streaming-muxer practice) leave `cues`
        // empty at this point even though a valid seek index exists later
        // in the file. Recover it now, without disturbing the read
        // position used for subsequent packet reading.
        if self.cues.is_empty() {
            self.resolve_cues_via_seek_head().await?;
        }

        // Build stream info
        self.build_streams();

        self.header_parsed = true;
        Ok(())
    }

    /// Parses segment child elements.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing fails.
    async fn parse_segment_children(&mut self) -> OxiResult<()> {
        loop {
            self.ensure_buffer(12).await?;
            if self.buffer.is_empty() {
                break;
            }

            let mut parser = MatroskaParser::new(&self.buffer);
            let Ok(element) = parser.read_element() else {
                break;
            };

            // Stop at first cluster - we'll read packets from here
            if element.id == element_id::CLUSTER {
                break;
            }

            let header_size = element.header_size;

            // For unbounded elements, we need to scan for the next element
            if element.is_unbounded() {
                break;
            }

            #[allow(clippy::cast_possible_truncation)]
            let data_size = element.size as usize;

            // Read element data
            let total_size = header_size + data_size;
            self.ensure_buffer(total_size).await?;

            if self.buffer.len() < total_size {
                // Not enough data for this element, skip to clusters
                break;
            }

            let element_data = &self.buffer[header_size..total_size];

            match element.id {
                element_id::SEEK_HEAD => {
                    self.seek_entries = parser::parse_seek_head(element_data, element.size)?;
                }
                element_id::INFO => {
                    self.segment_info =
                        Some(parser::parse_segment_info(element_data, element.size)?);
                }
                element_id::TRACKS => {
                    self.tracks = parser::parse_tracks(element_data, element.size)?;
                }
                element_id::CUES => {
                    self.cues = parser::parse_cues(element_data, element.size)?;
                }
                element_id::CHAPTERS => {
                    self.editions = parser::parse_chapters(element_data, element.size)?;
                }
                element_id::TAGS => {
                    self.tags = parser::parse_tags(element_data, element.size)?;
                }
                // Skip attachments, void, CRC, and unknown elements
                _ => {}
            }

            self.consume_buffer(total_size);
        }

        Ok(())
    }

    /// Attempts to locate and parse the Cues (seek index) element when it
    /// was not found by the pre-Cluster metadata scan in
    /// [`Self::parse_segment_children`].
    ///
    /// Matroska muxers commonly write Cues in the trailer, after every
    /// Cluster (this crate's own `MatroskaMuxer` does exactly that, since
    /// cluster byte positions for the cue table aren't known until each
    /// cluster has actually been written). `parse_segment_children` stops
    /// at the first Cluster to avoid buffering the whole file into memory,
    /// so it only ever sees Cues written *before* every Cluster. This
    /// method recovers Cues written later by:
    ///
    /// 1. Following the `SeekHead` (if one was parsed) directly to the
    ///    Cues element's byte offset -- an O(1) jump.
    /// 2. Falling back to a bounded linear scan over top-level elements,
    ///    skipping past Cluster contents, looking for a trailing Cues
    ///    element -- used only when no `SeekHead` entry for Cues is
    ///    available (e.g. a foreign file that omits `SeekHead`).
    ///
    /// Either way, the source position and read buffer are fully restored
    /// to their pre-call state before returning, so the normal cluster
    /// reading that resumes right after header parsing is unaffected.
    ///
    /// # Errors
    ///
    /// Returns an error only for I/O failures. A Cues element that cannot
    /// be located is not an error: the demuxer simply falls back to its
    /// existing "no cues available" behavior (seek-to-segment-start).
    async fn resolve_cues_via_seek_head(&mut self) -> OxiResult<()> {
        if !self.source.is_seekable() {
            return Ok(());
        }

        // Save state so this side-excursion is invisible to the caller.
        let saved_position = self.position;
        let saved_buffer = std::mem::take(&mut self.buffer);
        let saved_eof = self.eof;
        let saved_cluster = std::mem::take(&mut self.current_cluster);

        let cues_offset = self
            .seek_entries
            .iter()
            .find(|entry| entry.id == element_id::CUES)
            .map(|entry| self.segment_start + entry.position);

        let found = if let Some(offset) = cues_offset {
            self.try_parse_cues_at(offset).await?
        } else {
            false
        };

        if !found {
            self.scan_for_trailing_cues().await?;
        }

        // Restore demuxer state so normal cluster reading is unaffected,
        // regardless of whether Cues were found.
        self.source.seek(SeekFrom::Start(saved_position)).await?;
        self.position = saved_position;
        self.buffer = saved_buffer;
        self.eof = saved_eof;
        self.current_cluster = saved_cluster;

        Ok(())
    }

    /// Attempts to parse a Cues element at an exact absolute byte offset,
    /// as pointed to by a `SeekHead` entry.
    ///
    /// Returns `Ok(true)` if a Cues element was found and parsed into
    /// `self.cues`, `Ok(false)` if the offset didn't actually contain one
    /// (a stale or foreign `SeekHead`), and `Err` only on I/O failure.
    async fn try_parse_cues_at(&mut self, offset: u64) -> OxiResult<bool> {
        self.seek_to_position(offset).await?;
        self.ensure_buffer(12).await?;
        if self.buffer.is_empty() {
            return Ok(false);
        }

        let element = {
            let mut parser = MatroskaParser::new(&self.buffer);
            let Ok(element) = parser.read_element() else {
                return Ok(false);
            };
            element
        };

        if element.id != element_id::CUES || element.is_unbounded() {
            return Ok(false);
        }

        #[allow(clippy::cast_possible_truncation)]
        let data_size = element.size as usize;
        let total_size = element.header_size + data_size;
        self.ensure_buffer(total_size).await?;
        if self.buffer.len() < total_size {
            return Ok(false);
        }

        let element_data = &self.buffer[element.header_size..total_size];
        self.cues = parser::parse_cues(element_data, element.size)?;
        Ok(true)
    }

    /// Bounded fallback: linearly scans top-level elements from the start
    /// of the segment, skipping over Cluster contents element-by-element,
    /// looking for a Cues element written without a `SeekHead` to point to
    /// it.
    ///
    /// This walks every element in the file once (an O(n) cost proportional
    /// to file size), so it is only used when [`Self::resolve_cues_via_seek_head`]
    /// has no `SeekHead` entry to follow directly. Gives up silently
    /// (leaving `self.cues` empty) at EOF or on the first parse error,
    /// rather than failing the whole probe -- a file with no usable Cues is
    /// simply not seekable by timestamp, which is not itself an error.
    async fn scan_for_trailing_cues(&mut self) -> OxiResult<()> {
        self.seek_to_position(self.segment_start).await?;

        loop {
            self.ensure_buffer(12).await?;
            if self.buffer.is_empty() {
                return Ok(());
            }

            let element = {
                let mut parser = MatroskaParser::new(&self.buffer);
                let Ok(element) = parser.read_element() else {
                    return Ok(());
                };
                element
            };

            if element.id == element_id::CUES && !element.is_unbounded() {
                #[allow(clippy::cast_possible_truncation)]
                let data_size = element.size as usize;
                let total_size = element.header_size + data_size;
                self.ensure_buffer(total_size).await?;
                if self.buffer.len() < total_size {
                    return Ok(());
                }
                let element_data = &self.buffer[element.header_size..total_size];
                self.cues = parser::parse_cues(element_data, element.size)?;
                return Ok(());
            }

            // Unbounded elements (Cluster, when streamed) have no declared
            // size -- step past just the header and let the loop walk
            // their children (Timestamp/SimpleBlock/BlockGroup/etc.) one at
            // a time via their own declared sizes below, exactly like the
            // normal packet-reading path does in `read_next_block`.
            if element.is_unbounded() {
                self.consume_buffer(element.header_size);
                continue;
            }

            #[allow(clippy::cast_possible_truncation)]
            let data_size = element.size as usize;
            let total_size = element.header_size + data_size;
            self.ensure_buffer(total_size).await?;
            if self.buffer.len() < total_size {
                return Ok(());
            }
            self.consume_buffer(total_size);
        }
    }

    /// Builds stream info from parsed tracks.
    #[allow(clippy::cast_possible_wrap)]
    fn build_streams(&mut self) {
        let timecode_scale = self
            .segment_info
            .as_ref()
            .map_or(NS_PER_MS, |info| info.timecode_scale);

        // Calculate timebase from timecode scale (nanoseconds)
        // timebase = timecode_scale / 1_000_000_000
        // Note: timecode_scale is always positive and fits in i64
        let timebase = Rational::new(timecode_scale as i64, 1_000_000_000);

        for (index, track) in self.tracks.iter().enumerate() {
            // Skip tracks without a supported codec
            let codec = match &track.oxi_codec {
                Some(c) => *c,
                None => continue, // Skip unsupported codecs
            };

            let mut stream = StreamInfo::new(index, codec, timebase);

            // Set duration if available
            if let Some(ref info) = self.segment_info {
                if let Some(duration) = info.duration {
                    // Duration is in timecode scale units
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        stream.duration = Some(duration as i64);
                    }
                }
            }

            // Set codec parameters
            match track.track_type {
                TrackType::Video => {
                    if let Some(ref video) = track.video {
                        stream.codec_params =
                            CodecParams::video(video.pixel_width, video.pixel_height);
                    }
                }
                TrackType::Audio => {
                    if let Some(ref audio) = track.audio {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        {
                            stream.codec_params =
                                CodecParams::audio(audio.sampling_frequency as u32, audio.channels);
                        }
                    }
                }
                _ => {}
            }

            // Set extradata from codec private
            if let Some(ref private) = track.codec_private {
                stream.codec_params.extradata = Some(Bytes::copy_from_slice(private));
            }
            stream.codec_params.block_addition_mappings = track.block_addition_mappings.clone();

            // Set metadata
            if let Some(ref name) = track.name {
                stream.metadata = Metadata::new().with_title(name);
            }
            if !track.language.is_empty() && track.language != "und" {
                stream.metadata = stream.metadata.with_entry("language", &track.language);
            }

            self.track_to_stream
                .insert(track.number, self.streams.len());
            self.streams.push(stream);
        }
    }

    /// Reads the next cluster or block.
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails or EOF is reached.
    #[allow(clippy::cast_possible_truncation)]
    async fn read_next_block(&mut self) -> OxiResult<(Block, u64)> {
        loop {
            self.ensure_buffer(12).await?;

            // Only return EOF when buffer is empty (eof flag just means source is exhausted)
            if self.buffer.is_empty() {
                return Err(OxiError::Eof);
            }

            let mut parser = MatroskaParser::new(&self.buffer);
            let element = parser.read_element()?;

            match element.id {
                element_id::CLUSTER => {
                    // New cluster - parse timestamp
                    self.consume_buffer(element.header_size);
                    self.current_cluster = Some(ClusterState {
                        timecode: 0,
                        position: self.position,
                        size: if element.is_unbounded() {
                            None
                        } else {
                            Some(element.size)
                        },
                        data_position: 0,
                    });
                }
                element_id::TIMESTAMP => {
                    // Cluster timestamp
                    let data_size = element.size as usize;
                    let total_size = element.header_size + data_size;
                    self.ensure_buffer(total_size).await?;
                    if self.buffer.len() < total_size {
                        // Declared element size overruns what the source
                        // actually has left — truncated/malformed input,
                        // not a slice to take out of bounds.
                        return Err(OxiError::UnexpectedEof);
                    }

                    let data = &self.buffer[element.header_size..total_size];
                    let (_, timecode) = ebml::read_uint(data).map_err(|e| OxiError::Parse {
                        offset: self.position,
                        message: format!("Failed to parse cluster timestamp: {e:?}"),
                    })?;

                    if let Some(ref mut cluster) = self.current_cluster {
                        cluster.timecode = timecode;
                    }
                    self.consume_buffer(total_size);
                }
                element_id::SIMPLE_BLOCK => {
                    // Simple block - parse and return
                    let data_size = element.size as usize;
                    let total_size = element.header_size + data_size;
                    self.ensure_buffer(total_size).await?;
                    if self.buffer.len() < total_size {
                        // Declared element size overruns what the source
                        // actually has left — truncated/malformed input,
                        // not a slice to take out of bounds.
                        return Err(OxiError::UnexpectedEof);
                    }

                    let data = &self.buffer[element.header_size..total_size];
                    let block = parser::parse_simple_block(data)?;

                    let cluster_time = self.current_cluster.as_ref().map_or(0, |c| c.timecode);

                    self.consume_buffer(total_size);
                    return Ok((block, cluster_time));
                }
                element_id::BLOCK_GROUP => {
                    // Block group - parse and return
                    let data_size = element.size as usize;
                    let total_size = element.header_size + data_size;
                    self.ensure_buffer(total_size).await?;
                    if self.buffer.len() < total_size {
                        // Declared element size overruns what the source
                        // actually has left — truncated/malformed input,
                        // not a slice to take out of bounds.
                        return Err(OxiError::UnexpectedEof);
                    }

                    let data = &self.buffer[element.header_size..total_size];
                    let block = parser::parse_block_group(data, element.size)?;

                    let cluster_time = self.current_cluster.as_ref().map_or(0, |c| c.timecode);

                    self.consume_buffer(total_size);
                    return Ok((block, cluster_time));
                }
                _ => {
                    // Skip unknown elements
                    if element.is_unbounded() {
                        // For unbounded elements at top level, try to find next known element
                        self.consume_buffer(element.header_size);
                    } else {
                        let total_size = element.header_size + element.size as usize;
                        self.ensure_buffer(total_size).await?;
                        self.consume_buffer(total_size.min(self.buffer.len()));
                    }
                }
            }
        }
    }

    /// Converts a block to a packet.
    #[allow(clippy::cast_possible_wrap)]
    fn block_to_packet(&self, block: &Block, cluster_time: u64) -> OxiResult<Packet> {
        // Find stream index
        let stream_index = self
            .track_to_stream
            .get(&block.header.track_number)
            .copied()
            .ok_or_else(|| OxiError::Parse {
                offset: 0,
                message: format!("Unknown track number: {}", block.header.track_number),
            })?;

        // Get stream for timebase
        let stream = &self.streams[stream_index];

        // Calculate timestamp
        // Block timecode is relative to cluster, in timecode scale units
        #[allow(clippy::cast_sign_loss)]
        let pts = cluster_time as i64 + i64::from(block.header.timecode);

        // Get frame data (use first frame if laced)
        let data = if block.frames.is_empty() {
            Bytes::new()
        } else {
            Bytes::copy_from_slice(&block.frames[0])
        };

        // Determine flags
        let mut flags = PacketFlags::empty();
        if block.is_keyframe() {
            flags |= PacketFlags::KEYFRAME;
        }
        if block.header.discardable {
            flags |= PacketFlags::DISCARD;
        }

        // Create timestamp.
        //
        // `BlockDuration` (0x9B) only ever appears inside a `BlockGroup`; a
        // `SimpleBlock` has no duration field. When present it is expressed in
        // TimestampScale units — the very same units as the cluster-relative
        // PTS computed above — so it can be forwarded verbatim. When absent we
        // fall back to the track's `DefaultDuration` (nanoseconds) converted
        // into TimestampScale units, which is what most muxers rely on for
        // constant-frame-rate video.
        let mut timestamp = Timestamp::new(pts, stream.timebase);
        timestamp.duration = block
            .duration
            .and_then(|d| i64::try_from(d).ok())
            .or_else(|| self.default_duration_in_scale(block.header.track_number));

        Ok(Packet::new(stream_index, data, timestamp, flags))
    }

    /// Converts a track's `DefaultDuration` (nanoseconds) into TimestampScale
    /// units, so it can be used as a packet duration when a block carries no
    /// explicit `BlockDuration`.
    fn default_duration_in_scale(&self, track_number: u64) -> Option<i64> {
        let default_duration = self
            .tracks
            .iter()
            .find(|t| t.number == track_number)
            .and_then(|t| t.default_duration)?;
        let scale = self
            .segment_info
            .as_ref()
            .map_or(NS_PER_MS, |info| info.timecode_scale);
        if scale == 0 {
            return None;
        }
        i64::try_from(default_duration / scale).ok()
    }

    /// Finds the best cue point for seeking to a target timestamp.
    ///
    /// Returns the cluster position for the cue point that is closest to
    /// (but not after) the target timestamp for the given track.
    ///
    /// # Arguments
    ///
    /// * `target_time` - Target timestamp in timecode scale units
    /// * `track_number` - Track number to seek on
    /// * `backward` - If true, find the cue point before target; otherwise after
    ///
    /// # Returns
    ///
    /// Returns `Some((cluster_position, cue_time))` if a suitable cue point is found,
    /// or `None` if no cue points are available for the track.
    #[allow(clippy::cast_possible_wrap)]
    fn find_cue_point(
        &self,
        target_time: u64,
        track_number: u64,
        backward: bool,
    ) -> Option<(u64, u64)> {
        if self.cues.is_empty() {
            return None;
        }

        let mut best_cue: Option<(u64, u64)> = None;

        for cue_point in &self.cues {
            // Find track position for this track
            let track_pos = cue_point
                .track_positions
                .iter()
                .find(|tp| tp.track == track_number)?;

            let cue_time = cue_point.time;
            let cluster_pos = self.segment_start + track_pos.cluster_position;

            if backward {
                // For backward seeks, we want the latest cue that's <= target
                if cue_time <= target_time {
                    match best_cue {
                        Some((_, best_time)) if cue_time > best_time => {
                            best_cue = Some((cluster_pos, cue_time));
                        }
                        None => {
                            best_cue = Some((cluster_pos, cue_time));
                        }
                        _ => {}
                    }
                }
            } else {
                // For forward seeks, we want the earliest cue that's >= target
                if cue_time >= target_time {
                    match best_cue {
                        Some((_, best_time)) if cue_time < best_time => {
                            best_cue = Some((cluster_pos, cue_time));
                        }
                        None => {
                            best_cue = Some((cluster_pos, cue_time));
                        }
                        _ => {}
                    }
                }
            }
        }

        best_cue
    }

    /// Seeks to the exact sample with the given presentation timestamp (PTS).
    ///
    /// This performs sample-accurate seeking, which guarantees that the next
    /// call to [`read_packet`](crate::Demuxer::read_packet) returns the packet
    /// with `pts == target_pts` (for the default stream), or the first packet
    /// with `pts >= target_pts` if no exact match is found.
    ///
    /// # Algorithm
    ///
    /// 1. Use Cue Points (if available) to seek to the cluster containing or
    ///    preceding `target_pts`.
    /// 2. Fall back to the segment start if no Cues are present.
    /// 3. Scan forward block-by-block looking for the first block from any
    ///    stream whose `pts == target_pts` (considering the default/first
    ///    stream).  If an exact match is found, the demuxer is positioned
    ///    *just before* that block, so the next `read_packet` returns it.
    /// 4. If no exact match is reached before a block with
    ///    `pts > target_pts`, set `skip_until = target_pts` so that
    ///    `read_packet` discards earlier-timestamped packets transparently.
    ///
    /// # Arguments
    ///
    /// * `target_pts` - Target PTS in timecode-scale units (same units used by
    ///   [`Packet::timestamp`](crate::Packet::timestamp)).
    ///
    /// # Errors
    ///
    /// - `OxiError::InvalidData` – headers have not been parsed yet, or the
    ///   source is not seekable.
    /// - Propagates I/O errors from the underlying `MediaSource`.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub async fn seek_sample_accurate(&mut self, target_pts: u64) -> OxiResult<DecodeSkipCursor> {
        if !self.source.is_seekable() {
            return Err(OxiError::unsupported("Source is not seekable"));
        }
        if !self.header_parsed {
            return Err(OxiError::InvalidData(
                "Cannot seek before probing".to_string(),
            ));
        }

        // Determine the reference track number (prefer video, else first track).
        let ref_track_number: u64 = {
            let video_track = self
                .tracks
                .iter()
                .find(|t| t.track_type == types::TrackType::Video)
                .or_else(|| self.tracks.first());
            video_track.map_or(1, |t| t.number)
        };

        // Step 1: Position via Cue Points (best-effort).
        let seek_pos = if let Some((cluster_pos, _)) =
            self.find_cue_point(target_pts, ref_track_number, true)
        {
            cluster_pos
        } else {
            // No Cues — start linear scan from segment start.
            self.segment_start
        };

        self.seek_to_position(seek_pos).await?;
        self.skip_until = None;
        let mut skip_samples = 0u32;

        // Step 2: Scan forward to find exact position.
        // We keep track of the last cluster/source position so we can
        // rewind to it when we overshoot.
        loop {
            let result = self.read_next_block().await;
            match result {
                Ok((block, cluster_time)) => {
                    // Only consider blocks from the reference track.
                    let is_ref = block.header.track_number == ref_track_number;
                    if !is_ref {
                        continue;
                    }

                    let block_pts = cluster_time as i64 + i64::from(block.header.timecode);
                    #[allow(clippy::cast_possible_truncation)]
                    let block_pts_u64 = if block_pts < 0 {
                        0u64
                    } else {
                        block_pts as u64
                    };

                    if block_pts_u64 >= target_pts {
                        // Reached or overshot the target.  Always rewind to
                        // the cluster start (`seek_pos`) so that the CLUSTER
                        // header and TIMESTAMP are re-parsed, ensuring
                        // `current_cluster` is properly initialized before
                        // the next `read_packet` call.  This avoids the
                        // `cluster_time = 0` default that would occur if we
                        // rewound to a mid-cluster position with
                        // `current_cluster = None`.
                        //
                        // `skip_until = target_pts` discards all blocks with
                        // `pts < target_pts`, so regardless of whether we
                        // found an exact match or overshot, the first packet
                        // returned by `read_packet` will have
                        // `pts >= target_pts`.
                        self.seek_to_position(seek_pos).await?;
                        self.skip_until = Some(target_pts);
                        return Ok(DecodeSkipCursor {
                            byte_offset: seek_pos,
                            sample_index: 0,
                            skip_samples,
                            target_pts: i64::try_from(target_pts).unwrap_or(i64::MAX),
                        });
                    }
                    // block_pts_u64 < target_pts → keep scanning.
                    skip_samples = skip_samples.saturating_add(1);
                }
                Err(OxiError::Eof) => {
                    // Reached EOF without finding the exact timestamp.
                    // Rewind to the last cluster and use skip_until.
                    self.seek_to_position(seek_pos).await?;
                    self.skip_until = Some(target_pts);
                    return Ok(DecodeSkipCursor {
                        byte_offset: seek_pos,
                        sample_index: 0,
                        skip_samples,
                        target_pts: i64::try_from(target_pts).unwrap_or(i64::MAX),
                    });
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Performs a seek operation using cue points and cluster scanning.
    ///
    /// This implements a multi-stage seeking algorithm:
    /// 1. Use cue points to find the nearest cluster
    /// 2. Scan clusters to find the exact keyframe position
    /// 3. Position the demuxer for reading from that point
    ///
    /// # Errors
    ///
    /// Returns an error if seeking fails or the source is not seekable.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    async fn perform_seek(&mut self, target: crate::SeekTarget) -> OxiResult<()> {
        // Check if source is seekable
        if !self.source.is_seekable() {
            return Err(OxiError::unsupported("Source is not seekable"));
        }

        // Ensure headers are parsed
        if !self.header_parsed {
            return Err(OxiError::InvalidData(
                "Cannot seek before probing".to_string(),
            ));
        }

        // Determine which stream to use for seeking
        let stream_index = if let Some(idx) = target.stream_index {
            if idx >= self.streams.len() {
                return Err(OxiError::InvalidData(format!(
                    "Stream index {idx} out of range"
                )));
            }
            idx
        } else {
            // Use first video stream, or first stream if no video
            self.streams
                .iter()
                .position(StreamInfo::is_video)
                .unwrap_or(0)
        };

        // Get track number for this stream
        let track_number = self
            .tracks
            .get(stream_index)
            .map_or(1, |track| track.number);

        // Convert target position to timecode scale units
        let timecode_scale = self
            .segment_info
            .as_ref()
            .map_or(NS_PER_MS, |info| info.timecode_scale);

        // Convert from seconds to timecode units
        // target_time = (target_seconds * 1_000_000_000) / timecode_scale
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let target_timecode = ((target.position * 1_000_000_000.0) / timecode_scale as f64) as u64;

        // Try to use cue points for efficient seeking
        if let Some((cluster_pos, _cue_time)) =
            self.find_cue_point(target_timecode, track_number, target.is_backward())
        {
            // Seek to the cluster position
            self.seek_to_position(cluster_pos).await?;

            // If we need frame-accurate seeking, scan forward to find exact position
            if target.is_frame_accurate() {
                return self
                    .scan_to_exact_position(target_timecode, stream_index)
                    .await;
            }

            return Ok(());
        }

        // No cue points available - fall back to binary search or linear scan
        // For now, just seek to the beginning as a safe fallback
        self.seek_to_position(self.segment_start).await?;

        Ok(())
    }

    /// Scans forward from current position to find exact timestamp.
    ///
    /// Used for frame-accurate seeking after positioning via cue points.
    ///
    /// # Errors
    ///
    /// Returns an error if scanning fails or the position is not found.
    async fn scan_to_exact_position(
        &mut self,
        target_timecode: u64,
        stream_index: usize,
    ) -> OxiResult<()> {
        // Read blocks until we find one with timestamp >= target
        loop {
            let result = self.read_next_block().await;

            match result {
                Ok((block, cluster_time)) => {
                    // Check if this block is from our target stream
                    if let Some(&block_stream_index) =
                        self.track_to_stream.get(&block.header.track_number)
                    {
                        if block_stream_index == stream_index {
                            // Calculate absolute timestamp
                            #[allow(clippy::cast_sign_loss)]
                            let block_time = cluster_time + block.header.timecode as u64;

                            // If we've reached or passed the target, we're done
                            if block_time >= target_timecode {
                                // We need to "put back" this block for the next read_packet call
                                // For now, we'll just return - the block will be re-read
                                return Ok(());
                            }
                        }
                    }
                }
                Err(OxiError::Eof) => {
                    // Reached end of file before finding target
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[async_trait]
impl<R: MediaSource> Demuxer for MatroskaDemuxer<R> {
    /// Probes the format and parses container headers.
    ///
    /// # Errors
    ///
    /// Returns an error if the format cannot be detected or headers are invalid.
    async fn probe(&mut self) -> OxiResult<ProbeResult> {
        if !self.header_parsed {
            self.parse_headers().await?;
        }

        let format = self.format.unwrap_or(ContainerFormat::Matroska);
        let confidence = if self.ebml_header.is_some() && !self.tracks.is_empty() {
            0.99
        } else if self.ebml_header.is_some() {
            0.95
        } else {
            0.5
        };

        Ok(ProbeResult::new(format, confidence))
    }

    /// Reads the next packet from the container.
    ///
    /// # Errors
    ///
    /// - Returns `OxiError::Eof` when there are no more packets
    /// - Returns other errors for parse failures or I/O errors
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
    async fn read_packet(&mut self) -> OxiResult<Packet> {
        if !self.header_parsed {
            self.parse_headers().await?;
        }

        loop {
            let (block, cluster_time) = self.read_next_block().await?;

            // Skip blocks from unknown tracks
            if !self
                .track_to_stream
                .contains_key(&block.header.track_number)
            {
                continue;
            }

            // Skip invisible blocks
            if block.header.invisible {
                continue;
            }

            // Honor sample-accurate seek: discard packets before the target PTS.
            if let Some(skip_until) = self.skip_until {
                let block_pts = cluster_time as i64 + i64::from(block.header.timecode);
                #[allow(clippy::cast_possible_truncation)]
                let block_pts_u64 = if block_pts < 0 {
                    0u64
                } else {
                    block_pts as u64
                };

                if block_pts_u64 < skip_until {
                    continue;
                }
                // We have reached or passed the target — clear the flag.
                self.skip_until = None;
            }

            return self.block_to_packet(&block, cluster_time);
        }
    }

    /// Returns information about all streams in the container.
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    /// Seeks to a target position in the container.
    async fn seek(&mut self, target: crate::SeekTarget) -> OxiResult<()> {
        self.perform_seek(target).await
    }

    /// Returns whether this demuxer supports seeking.
    fn is_seekable(&self) -> bool {
        self.source.is_seekable() && self.header_parsed
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_io::MemorySource;

    /// Creates a minimal valid WebM header for testing.
    fn create_webm_header() -> Vec<u8> {
        let mut data = Vec::new();

        // EBML header
        data.extend_from_slice(&[
            0x1A, 0x45, 0xDF, 0xA3, // EBML ID
            0x9F, // Size: 31 bytes
        ]);

        // EBMLVersion: 1
        data.extend_from_slice(&[0x42, 0x86, 0x81, 0x01]);

        // EBMLReadVersion: 1
        data.extend_from_slice(&[0x42, 0xF7, 0x81, 0x01]);

        // EBMLMaxIDLength: 4
        data.extend_from_slice(&[0x42, 0xF2, 0x81, 0x04]);

        // EBMLMaxSizeLength: 8
        data.extend_from_slice(&[0x42, 0xF3, 0x81, 0x08]);

        // DocType: "webm"
        data.extend_from_slice(&[0x42, 0x82, 0x84, b'w', b'e', b'b', b'm']);

        // DocTypeVersion: 4
        data.extend_from_slice(&[0x42, 0x87, 0x81, 0x04]);

        // DocTypeReadVersion: 2
        data.extend_from_slice(&[0x42, 0x85, 0x81, 0x02]);

        // Segment (unbounded size)
        data.extend_from_slice(&[
            0x18, 0x53, 0x80, 0x67, // Segment ID
            0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // Unknown size
        ]);

        // Info element
        data.extend_from_slice(&[
            0x15, 0x49, 0xA9, 0x66, // Info ID
            0x8E, // Size: 14 bytes
        ]);

        // TimecodeScale: 1000000 (1ms)
        data.extend_from_slice(&[
            0x2A, 0xD7, 0xB1, // TimecodeScale ID
            0x83, // Size: 3 bytes
            0x0F, 0x42, 0x40, // Value: 1000000
        ]);

        // Duration: 5000.0
        data.extend_from_slice(&[
            0x44, 0x89, // Duration ID
            0x84, // Size: 4 bytes
            0x45, 0x9C, 0x40, 0x00, // Float: 5000.0
        ]);

        // Tracks element
        data.extend_from_slice(&[
            0x16, 0x54, 0xAE, 0x6B, // Tracks ID
            0x9E, // Size: 30 bytes
        ]);

        // TrackEntry
        data.extend_from_slice(&[
            0xAE, // TrackEntry ID
            0x9C, // Size: 28 bytes
        ]);

        // TrackNumber: 1
        data.extend_from_slice(&[0xD7, 0x81, 0x01]);

        // TrackUID: 12345
        data.extend_from_slice(&[0x73, 0xC5, 0x82, 0x30, 0x39]);

        // TrackType: 1 (video)
        data.extend_from_slice(&[0x83, 0x81, 0x01]);

        // CodecID: V_VP9
        data.extend_from_slice(&[0x86, 0x85, b'V', b'_', b'V', b'P', b'9']);

        // Video element
        data.extend_from_slice(&[
            0xE0, // Video ID
            0x88, // Size: 8 bytes
        ]);

        // PixelWidth: 1920
        data.extend_from_slice(&[0xB0, 0x82, 0x07, 0x80]);

        // PixelHeight: 1080
        data.extend_from_slice(&[0xBA, 0x82, 0x04, 0x38]);

        // Cluster
        data.extend_from_slice(&[
            0x1F, 0x43, 0xB6, 0x75, // Cluster ID
            0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // Unknown size
        ]);

        // Cluster Timestamp: 0
        data.extend_from_slice(&[0xE7, 0x81, 0x00]);

        // SimpleBlock: track 1, timecode 0, keyframe
        let block_data = vec![0x01, 0x02, 0x03, 0x04]; // 4 bytes of data
        data.extend_from_slice(&[
            0xA3, // SimpleBlock ID
            0x88, // Size: 8 bytes
            0x81, // Track number: 1 (VINT)
            0x00, 0x00, // Timecode: 0
            0x80, // Flags: keyframe
        ]);
        data.extend_from_slice(&block_data);

        data
    }

    // ── BlockDuration propagation fixtures ────────────────────────────────

    /// Encodes an EBML element ID as its minimal big-endian byte sequence.
    fn ebml_id(id: u32) -> Vec<u8> {
        if id <= 0xFF {
            vec![id as u8]
        } else if id <= 0xFFFF {
            vec![(id >> 8) as u8, id as u8]
        } else if id <= 0x00FF_FFFF {
            vec![(id >> 16) as u8, (id >> 8) as u8, id as u8]
        } else {
            vec![
                (id >> 24) as u8,
                (id >> 16) as u8,
                (id >> 8) as u8,
                id as u8,
            ]
        }
    }

    /// Encodes a length as a 1- or 2-byte EBML VINT (enough for these fixtures).
    fn ebml_size(len: usize) -> Vec<u8> {
        assert!(len < 0x3FFF, "fixture elements stay small");
        if len < 0x7F {
            vec![(len as u8) | 0x80]
        } else {
            vec![0x40 | ((len >> 8) as u8), len as u8]
        }
    }

    /// Builds `id + size + content`.
    fn ebml_element(id: u32, content: &[u8]) -> Vec<u8> {
        let mut out = ebml_id(id);
        out.extend(ebml_size(content.len()));
        out.extend_from_slice(content);
        out
    }

    /// Builds an unsigned-integer EBML element with minimal-width payload.
    fn ebml_uint(id: u32, value: u64) -> Vec<u8> {
        let mut bytes = value.to_be_bytes().to_vec();
        while bytes.len() > 1 && bytes[0] == 0 {
            bytes.remove(0);
        }
        ebml_element(id, &bytes)
    }

    /// The fixed EBML header used by every fixture below (`webm`, DocType v4).
    fn ebml_header() -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&[0x42, 0x86, 0x81, 0x01]); // EBMLVersion
        header.extend_from_slice(&[0x42, 0xF7, 0x81, 0x01]); // EBMLReadVersion
        header.extend_from_slice(&[0x42, 0xF2, 0x81, 0x04]); // EBMLMaxIDLength
        header.extend_from_slice(&[0x42, 0xF3, 0x81, 0x08]); // EBMLMaxSizeLength
        header.extend_from_slice(&[0x42, 0x82, 0x84, b'w', b'e', b'b', b'm']); // DocType
        header.extend_from_slice(&[0x42, 0x87, 0x81, 0x04]); // DocTypeVersion
        header.extend_from_slice(&[0x42, 0x85, 0x81, 0x02]); // DocTypeReadVersion
        ebml_element(0x1A45_DFA3, &header)
    }

    /// Raw `Block`/`SimpleBlock` body: track VINT + s16 timecode + flags + data.
    fn block_body(track: u8, timecode: i16, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0x80 | track);
        out.extend_from_slice(&timecode.to_be_bytes());
        out.push(flags);
        out.extend_from_slice(data);
        out
    }

    /// Builds a complete one-track WebM whose cluster holds a `SimpleBlock`
    /// followed by a `BlockGroup` carrying an explicit `BlockDuration`.
    ///
    /// `default_duration_ns` is written as the track's `DefaultDuration` when
    /// `Some`, which is the fallback the demuxer should use for the
    /// `SimpleBlock` (blocks outside a `BlockGroup` have no duration field).
    fn webm_with_block_group(block_duration: u64, default_duration_ns: Option<u64>) -> Vec<u8> {
        let mut data = ebml_header();

        // Segment (unknown size)
        data.extend_from_slice(&[
            0x18, 0x53, 0x80, 0x67, // Segment ID
            0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);

        // Info: TimestampScale = 1 000 000 ns (1 ms)
        let info = ebml_uint(element_id::TIMECODE_SCALE, 1_000_000);
        data.extend(ebml_element(element_id::INFO, &info));

        // Tracks: one VP9 video track
        let mut track_entry = Vec::new();
        track_entry.extend(ebml_uint(element_id::TRACK_NUMBER, 1));
        track_entry.extend(ebml_uint(element_id::TRACK_UID, 12_345));
        track_entry.extend(ebml_uint(element_id::TRACK_TYPE, 1));
        track_entry.extend(ebml_element(element_id::CODEC_ID, b"V_VP9"));
        if let Some(default_duration) = default_duration_ns {
            track_entry.extend(ebml_uint(element_id::DEFAULT_DURATION, default_duration));
        }
        let mut video = Vec::new();
        video.extend(ebml_uint(element_id::PIXEL_WIDTH, 1920));
        video.extend(ebml_uint(element_id::PIXEL_HEIGHT, 1080));
        track_entry.extend(ebml_element(element_id::VIDEO, &video));
        let tracks = ebml_element(element_id::TRACK_ENTRY, &track_entry);
        data.extend(ebml_element(element_id::TRACKS, &tracks));

        // Cluster (unknown size)
        data.extend_from_slice(&[
            0x1F, 0x43, 0xB6, 0x75, // Cluster ID
            0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
        data.extend(ebml_uint(element_id::TIMESTAMP, 0));

        // SimpleBlock at t = 0 (keyframe), no duration of its own.
        data.extend(ebml_element(
            element_id::SIMPLE_BLOCK,
            &block_body(1, 0, 0x80, &[0x01, 0x02, 0x03, 0x04]),
        ));

        // BlockGroup at t = 40 ms carrying an explicit BlockDuration.
        let mut group = Vec::new();
        group.extend(ebml_element(
            element_id::BLOCK,
            &block_body(1, 40, 0x00, &[0x0A, 0x0B, 0x0C]),
        ));
        group.extend(ebml_uint(element_id::BLOCK_DURATION, block_duration));
        data.extend(ebml_element(element_id::BLOCK_GROUP, &group));

        data
    }

    async fn read_all(data: Vec<u8>) -> Vec<Packet> {
        let mut demuxer = MatroskaDemuxer::new(MemorySource::new(Bytes::from(data)));
        demuxer.probe().await.expect("probe should succeed");
        let mut packets = Vec::new();
        while let Ok(packet) = demuxer.read_packet().await {
            packets.push(packet);
            if packets.len() > 8 {
                break;
            }
        }
        packets
    }

    /// Regression: `block_to_packet` never forwarded `BlockDuration`, so every
    /// Matroska packet came back with `timestamp.duration == None` even when
    /// the file said otherwise (TODO.md "Container" section).
    #[tokio::test]
    async fn block_group_duration_reaches_the_packet() {
        let packets = read_all(webm_with_block_group(60, None)).await;
        assert_eq!(packets.len(), 2, "SimpleBlock + BlockGroup");

        assert_eq!(
            packets[0].timestamp.duration, None,
            "a SimpleBlock has no duration and no DefaultDuration to fall back on"
        );

        assert_eq!(packets[1].pts(), 40);
        assert_eq!(
            packets[1].timestamp.duration,
            Some(60),
            "BlockDuration must be forwarded verbatim in TimestampScale units"
        );
    }

    /// With a `DefaultDuration` on the track, a duration-less `SimpleBlock`
    /// inherits it — while an explicit `BlockDuration` still wins.
    #[tokio::test]
    async fn default_duration_is_the_fallback() {
        // 33 ms per frame, TimestampScale is 1 ms → 33 scale units.
        let packets = read_all(webm_with_block_group(60, Some(33_000_000))).await;
        assert_eq!(packets.len(), 2);
        assert_eq!(
            packets[0].timestamp.duration,
            Some(33),
            "DefaultDuration (ns) must be converted into TimestampScale units"
        );
        assert_eq!(
            packets[1].timestamp.duration,
            Some(60),
            "an explicit BlockDuration overrides DefaultDuration"
        );
    }

    #[tokio::test]
    async fn zero_block_duration_is_preserved() {
        let packets = read_all(webm_with_block_group(0, None)).await;
        assert_eq!(packets.len(), 2);
        assert_eq!(
            packets[1].timestamp.duration,
            Some(0),
            "an explicit zero duration is not the same as an absent one"
        );
    }

    #[tokio::test]
    async fn test_matroska_demuxer_new() {
        let source = MemorySource::new(Bytes::new());
        let demuxer = MatroskaDemuxer::new(source);
        assert!(!demuxer.header_parsed);
        assert!(demuxer.streams().is_empty());
    }

    #[tokio::test]
    async fn test_matroska_demuxer_probe() {
        let data = create_webm_header();
        let source = MemorySource::new(Bytes::from(data));
        let mut demuxer = MatroskaDemuxer::new(source);

        let result = demuxer.probe().await.expect("probe should succeed");
        assert_eq!(result.format, ContainerFormat::WebM);
        assert!(result.confidence > 0.9);
        assert!(demuxer.header_parsed);
    }

    #[tokio::test]
    async fn test_matroska_demuxer_streams() {
        let data = create_webm_header();
        let source = MemorySource::new(Bytes::from(data));
        let mut demuxer = MatroskaDemuxer::new(source);

        demuxer.probe().await.expect("probe should succeed");

        let streams = demuxer.streams();
        assert!(!streams.is_empty());
        assert_eq!(streams[0].codec, oximedia_core::CodecId::Vp9);
    }

    #[tokio::test]
    async fn test_matroska_demuxer_read_packet() {
        let data = create_webm_header();
        let source = MemorySource::new(Bytes::from(data));
        let mut demuxer = MatroskaDemuxer::new(source);

        demuxer.probe().await.expect("probe should succeed");

        let packet = demuxer
            .read_packet()
            .await
            .expect("operation should succeed");
        assert_eq!(packet.stream_index, 0);
        assert!(packet.is_keyframe());
        assert_eq!(packet.size(), 4);
    }

    #[tokio::test]
    async fn test_matroska_demuxer_eof() {
        let data = create_webm_header();
        let source = MemorySource::new(Bytes::from(data));
        let mut demuxer = MatroskaDemuxer::new(source);

        demuxer.probe().await.expect("probe should succeed");

        // Read first packet
        let _ = demuxer
            .read_packet()
            .await
            .expect("operation should succeed");

        // Second read should return EOF
        let result = demuxer.read_packet().await;
        assert!(matches!(result, Err(OxiError::Eof)));
    }

    #[tokio::test]
    async fn test_segment_info() {
        let data = create_webm_header();
        let source = MemorySource::new(Bytes::from(data));
        let mut demuxer = MatroskaDemuxer::new(source);

        demuxer.probe().await.expect("probe should succeed");

        let info = demuxer.segment_info().expect("operation should succeed");
        assert_eq!(info.timecode_scale, 1_000_000);
        assert!(info.duration.is_some());
    }

    #[tokio::test]
    async fn test_tracks_info() {
        let data = create_webm_header();
        let source = MemorySource::new(Bytes::from(data));
        let mut demuxer = MatroskaDemuxer::new(source);

        demuxer.probe().await.expect("probe should succeed");

        let tracks = demuxer.tracks();
        assert!(!tracks.is_empty());
        assert_eq!(tracks[0].number, 1);
        assert_eq!(tracks[0].codec_id, "V_VP9");
        assert!(tracks[0].video.is_some());

        let video = tracks[0].video.as_ref().expect("operation should succeed");
        assert_eq!(video.pixel_width, 1920);
        assert_eq!(video.pixel_height, 1080);
    }
}

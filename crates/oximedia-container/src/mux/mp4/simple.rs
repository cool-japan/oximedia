//! Simple, low-level MP4 muxer operating on raw sample data.
//!
//! Provides [`SimpleMp4Muxer`] (exported as the new API-compatible `Mp4Muxer`-style interface),
//! designed for use cases where the caller has already-encoded bitstream data and wants to write a
//! valid ISOBMFF file with minimal bookkeeping.
//!
//! # Differences from [`super::Mp4Muxer`]
//!
//! - Accepts raw `Vec<u8>` sample data via [`Mp4Sample`] instead of [`crate::Packet`].
//! - Uses [`TrackCodec`] (Video/Audio) rather than [`oximedia_core::CodecId`].
//! - `finalize` streams to any `std::io::Write` rather than returning `Vec<u8>`.
//! - Supports both progressive (`ftyp + moov + mdat`) and fragmented
//!   (`ftyp + moov[mvex] + moof+mdat...`) layouts.
//!
//! # Box layout (progressive)
//!
//! ```text
//! [ftyp][moov[mvhd][trak[tkhd][mdia[mdhd][hdlr][minf[vmhd|smhd][dinf[dref]][stbl[stsd][stts][stsc][stsz][stco][stss?]]]]]]][mdat]
//! ```
//!
//! # Example
//!
//! ```ignore
//! use oximedia_container::mux::mp4::simple::{
//!     SimpleMp4Muxer, SimpleMp4Config, TrackCodec, VideoCodecInfo, Mp4Sample,
//! };
//!
//! let config = SimpleMp4Config { fragmented: false, brand: *b"isom" };
//! let mut muxer = SimpleMp4Muxer::new(config);
//!
//! let track_id = muxer.add_track(TrackCodec::Video(VideoCodecInfo {
//!     width: 1920,
//!     height: 1080,
//!     timescale: 90000,
//!     fourcc: *b"av01",
//! }));
//!
//! muxer.write_sample(track_id, Mp4Sample {
//!     pts: 0, dts: 0, duration: 3000, is_sync: true,
//!     data: vec![0xAB; 512],
//! }).expect("sample written");
//!
//! let mut out = std::io::Cursor::new(Vec::<u8>::new());
//! muxer.finalize(&mut out).expect("finalized");
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

use std::collections::HashMap;
use std::io::{self, Write};

/// `esds` (MPEG-4 `ES_Descriptor`) construction, re-exported here so an
/// `mp4a` track can be described in one `use`.
///
/// See [`crate::mux::mp4::esds`] for the descriptor layout.
pub use super::esds::{
    build_audio_specific_config, build_esds_box, build_esds_payload, EsdsParams,
};

// ─── Error ───────────────────────────────────────────────────────────────────

/// Errors produced by [`SimpleMp4Muxer`].
#[derive(Debug, thiserror::Error)]
pub enum SimpleMp4Error {
    /// I/O error writing to the output sink.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The requested track ID does not exist.
    #[error("unknown track id {0}")]
    UnknownTrack(u32),
    /// The muxer has already been finalized.
    #[error("muxer already finalized")]
    AlreadyFinalized,
    /// No tracks have been added.
    #[error("no tracks added")]
    NoTracks,
}

// ─── FourCC ──────────────────────────────────────────────────────────────────

/// Four-character code used in ISOBMFF box types and codec identifiers.
pub type FourCC = [u8; 4];

// ─── Codec descriptors ───────────────────────────────────────────────────────

/// Codec-specific parameters for a video track.
#[derive(Debug, Clone)]
pub struct VideoCodecInfo {
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Track timescale (ticks per second). Typical: 90000.
    pub timescale: u32,
    /// Four-character codec code: `b"av01"`, `b"vp09"`, etc.
    pub fourcc: FourCC,
    /// Optional codec-specific configuration box payload (e.g. `av1C`, `vpcC`).
    pub config_data: Option<Vec<u8>>,
    /// Four-character code of the configuration box. Required when `config_data` is `Some`.
    pub config_fourcc: Option<FourCC>,
}

impl VideoCodecInfo {
    /// Creates a minimal [`VideoCodecInfo`] without codec config data.
    #[must_use]
    pub fn new(fourcc: FourCC, width: u32, height: u32, timescale: u32) -> Self {
        Self {
            width,
            height,
            timescale,
            fourcc,
            config_data: None,
            config_fourcc: None,
        }
    }

    /// Builder: attaches codec-specific configuration.
    #[must_use]
    pub fn with_config(mut self, config_fourcc: FourCC, data: Vec<u8>) -> Self {
        self.config_fourcc = Some(config_fourcc);
        self.config_data = Some(data);
        self
    }
}

/// Codec-specific parameters for an audio track.
#[derive(Debug, Clone)]
pub struct AudioCodecInfo {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channel_count: u16,
    /// Track timescale (ticks per second). Usually equals `sample_rate`.
    pub timescale: u32,
    /// Four-character codec code: `b"Opus"`, `b"fLaC"`, etc.
    pub fourcc: FourCC,
    /// Optional codec-specific configuration box payload.
    pub config_data: Option<Vec<u8>>,
    /// Four-character code of the configuration box. Required when `config_data` is `Some`.
    pub config_fourcc: Option<FourCC>,
}

impl AudioCodecInfo {
    /// Creates a minimal [`AudioCodecInfo`] without codec config data.
    #[must_use]
    pub fn new(fourcc: FourCC, sample_rate: u32, channel_count: u16) -> Self {
        Self {
            sample_rate,
            channel_count,
            timescale: sample_rate,
            fourcc,
            config_data: None,
            config_fourcc: None,
        }
    }

    /// Builder: attaches codec-specific configuration.
    #[must_use]
    pub fn with_config(mut self, config_fourcc: FourCC, data: Vec<u8>) -> Self {
        self.config_fourcc = Some(config_fourcc);
        self.config_data = Some(data);
        self
    }
}

/// Codec descriptor for a track.
#[derive(Debug, Clone)]
pub enum TrackCodec {
    /// A video track.
    Video(VideoCodecInfo),
    /// An audio track.
    Audio(AudioCodecInfo),
}

impl TrackCodec {
    /// Returns the timescale for this codec.
    #[must_use]
    pub fn timescale(&self) -> u32 {
        match self {
            Self::Video(v) => v.timescale,
            Self::Audio(a) => a.timescale,
        }
    }

    /// Returns the ISOBMFF handler type: `b"vide"` or `b"soun"`.
    #[must_use]
    pub fn handler_type(&self) -> &[u8; 4] {
        match self {
            Self::Video(_) => b"vide",
            Self::Audio(_) => b"soun",
        }
    }

    /// Returns the handler name string.
    #[must_use]
    pub fn handler_name(&self) -> &'static str {
        match self {
            Self::Video(_) => "OxiMedia Video Handler",
            Self::Audio(_) => "OxiMedia Audio Handler",
        }
    }
}

// ─── Sample ──────────────────────────────────────────────────────────────────

/// A single compressed media sample (audio or video).
#[derive(Debug, Clone)]
pub struct Mp4Sample {
    /// Presentation timestamp in the track's timescale.
    pub pts: i64,
    /// Decode timestamp in the track's timescale.
    pub dts: i64,
    /// Sample duration in the track's timescale.
    pub duration: u32,
    /// Whether this is a sync (key) sample.
    pub is_sync: bool,
    /// Raw compressed bitstream for this sample.
    pub data: Vec<u8>,
}

// ─── Track writer ────────────────────────────────────────────────────────────

/// Per-track state accumulated while writing.
#[derive(Debug)]
pub struct Mp4TrackWriter {
    /// 1-based track identifier.
    pub track_id: u32,
    /// Codec descriptor.
    pub codec: TrackCodec,
    /// All samples, in presentation order.
    pub samples: Vec<Mp4Sample>,
}

impl Mp4TrackWriter {
    fn new(track_id: u32, codec: TrackCodec) -> Self {
        Self {
            track_id,
            codec,
            samples: Vec::new(),
        }
    }

    /// Total duration in timescale ticks.
    fn total_duration(&self) -> u64 {
        self.samples.iter().map(|s| u64::from(s.duration)).sum()
    }
}

// ─── Muxer configuration ────────────────────────────────────────────────────

/// Configuration for [`SimpleMp4Muxer`].
#[derive(Debug, Clone)]
pub struct SimpleMp4Config {
    /// When `true`, writes a fragmented MP4 (`ftyp + moov[mvex] + moof+mdat...`).
    /// When `false` (default), writes a progressive MP4 (`ftyp + moov + mdat`).
    pub fragmented: bool,
    /// Major brand in the `ftyp` box (e.g. `*b"isom"`, `*b"av01"`).
    pub brand: FourCC,
    /// Compatible brands in addition to the major brand.
    pub compatible_brands: Vec<FourCC>,
    /// Minor version in the `ftyp` box.
    pub minor_version: u32,
}

impl Default for SimpleMp4Config {
    fn default() -> Self {
        Self {
            fragmented: false,
            brand: *b"isom",
            compatible_brands: vec![*b"isom", *b"iso6", *b"mp41"],
            minor_version: 0x200,
        }
    }
}

impl SimpleMp4Config {
    /// Creates a new [`SimpleMp4Config`] with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

// ─── SimpleMp4Muxer ──────────────────────────────────────────────────────────

/// Low-level MP4 muxer that operates on raw compressed sample data.
///
/// Use [`SimpleMp4Muxer::add_track`] to register each stream, then
/// [`SimpleMp4Muxer::write_sample`] to append samples, and finally
/// [`SimpleMp4Muxer::finalize`] to flush the complete ISOBMFF file to any
/// [`std::io::Write`] sink.
///
/// All ISOBMFF boxes are written with proper size prefixes and the sample
/// tables (`stts`, `stsc`, `stsz`, `stco`, `stss`) are populated from
/// the accumulated sample list.
pub struct SimpleMp4Muxer {
    config: SimpleMp4Config,
    /// Tracks keyed by their 1-based track_id.
    tracks: Vec<Mp4TrackWriter>,
    /// Maps track_id (1-based) → index in `tracks`.
    track_index: HashMap<u32, usize>,
    /// Next track_id to assign (starts at 1).
    next_track_id: u32,
    /// Whether [`finalize`] has been called.
    finalized: bool,
}

impl SimpleMp4Muxer {
    /// Creates a new muxer with the given configuration.
    #[must_use]
    pub fn new(config: SimpleMp4Config) -> Self {
        Self {
            config,
            tracks: Vec::new(),
            track_index: HashMap::new(),
            next_track_id: 1,
            finalized: false,
        }
    }

    /// Adds a new track and returns its 1-based track ID.
    ///
    /// Track IDs are assigned sequentially starting at 1.
    pub fn add_track(&mut self, codec: TrackCodec) -> u32 {
        let track_id = self.next_track_id;
        self.next_track_id += 1;
        let idx = self.tracks.len();
        self.tracks.push(Mp4TrackWriter::new(track_id, codec));
        self.track_index.insert(track_id, idx);
        track_id
    }

    /// Appends a sample to the specified track.
    ///
    /// # Errors
    ///
    /// Returns [`SimpleMp4Error::UnknownTrack`] if `track_id` does not exist,
    /// or [`SimpleMp4Error::AlreadyFinalized`] if `finalize` has already been called.
    pub fn write_sample(&mut self, track_id: u32, sample: Mp4Sample) -> Result<(), SimpleMp4Error> {
        if self.finalized {
            return Err(SimpleMp4Error::AlreadyFinalized);
        }
        let idx = self
            .track_index
            .get(&track_id)
            .copied()
            .ok_or(SimpleMp4Error::UnknownTrack(track_id))?;
        self.tracks[idx].samples.push(sample);
        Ok(())
    }

    /// Returns the number of registered tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Returns a reference to a [`Mp4TrackWriter`] by track ID, or `None`.
    #[must_use]
    pub fn track(&self, track_id: u32) -> Option<&Mp4TrackWriter> {
        self.track_index
            .get(&track_id)
            .and_then(|&i| self.tracks.get(i))
    }

    /// Finalizes the file and writes it to `writer`.
    ///
    /// For **progressive** mode this outputs: `ftyp` + `moov` + `mdat`.
    /// For **fragmented** mode this outputs: `ftyp` + `moov` (with `mvex`) + one `moof+mdat` per track.
    ///
    /// Chunk offsets in `stco` are computed with a two-pass approach to account for
    /// the actual size of the `moov` box.
    ///
    /// # Errors
    ///
    /// Returns [`SimpleMp4Error::NoTracks`] if no tracks were added, or
    /// [`SimpleMp4Error::Io`] on any write failure.
    pub fn finalize(&mut self, writer: &mut dyn Write) -> Result<(), SimpleMp4Error> {
        if self.finalized {
            return Err(SimpleMp4Error::AlreadyFinalized);
        }
        if self.tracks.is_empty() {
            return Err(SimpleMp4Error::NoTracks);
        }
        self.finalized = true;

        // 1. ftyp box
        let ftyp = build_ftyp(&self.config);
        writer.write_all(&ftyp)?;

        if self.config.fragmented {
            self.write_fragmented(writer, ftyp.len() as u64)?;
        } else {
            self.write_progressive(writer, ftyp.len() as u64)?;
        }

        Ok(())
    }

    // ── Progressive layout ────────────────────────────────────────────────

    fn write_progressive(
        &self,
        writer: &mut dyn Write,
        ftyp_size: u64,
    ) -> Result<(), SimpleMp4Error> {
        // Collect all sample data, interleaved across tracks in DTS order.
        let (mdat_payload, layouts) = self.collect_mdat_data();

        // The mdat header is 8 bytes normally, 16 bytes when the box needs the
        // 64-bit `largesize` form. This does not depend on the moov size, so it
        // can be settled before the offset fixup passes.
        let mdat_header = build_mdat_header(mdat_payload.len() as u64);
        let mdat_header_len = mdat_header.len() as u64;

        let use_co64 = self.resolve_co64(ftyp_size, mdat_header_len, &layouts);

        // First pass: build moov with placeholder offsets to measure its size.
        let placeholder_moov = self.build_moov_progressive(&layouts, 0, use_co64);
        let moov_size = placeholder_moov.len() as u64;

        // Sample data begins right after the mdat box header.
        let mdat_data_start = ftyp_size + moov_size + mdat_header_len;

        // Second pass: build moov with correct absolute offsets.
        let final_moov = self.build_moov_progressive(&layouts, mdat_data_start, use_co64);
        debug_assert_eq!(
            final_moov.len() as u64,
            moov_size,
            "moov size must not change between offset passes"
        );

        writer.write_all(&final_moov)?;
        writer.write_all(&mdat_header)?;
        writer.write_all(&mdat_payload)?;

        Ok(())
    }

    /// Decides whether chunk offsets must be written as 64-bit `co64` entries.
    ///
    /// This has to be settled *before* the moov is measured: switching
    /// `stco` → `co64` grows every entry from 4 to 8 bytes, so a placeholder
    /// built with `stco` would give a smaller moov than the file finally has
    /// and every chunk offset would point too early. Widening only ever pushes
    /// offsets further out, so the fixed point is reached in at most two
    /// iterations; the loop bound is a belt-and-braces guard.
    fn resolve_co64(&self, ftyp_size: u64, mdat_header_len: u64, layouts: &[TrackLayout]) -> bool {
        let max_relative_offset = layouts
            .iter()
            .filter_map(|l| l.chunk_offsets.last().copied())
            .max()
            .unwrap_or(0);

        let mut use_co64 = false;
        for _ in 0..4 {
            let moov_size = self.build_moov_progressive(layouts, 0, use_co64).len() as u64;
            let data_start = ftyp_size
                .saturating_add(moov_size)
                .saturating_add(mdat_header_len);
            let needs_co64 = data_start.saturating_add(max_relative_offset) > u64::from(u32::MAX);
            if needs_co64 == use_co64 {
                break;
            }
            use_co64 = needs_co64;
        }
        use_co64
    }

    /// Returns `(mdat_payload, per_track_layout)`.
    ///
    /// Samples from all tracks are **interleaved by decode timestamp** so a
    /// player only has to seek forward through `mdat`; consecutive samples that
    /// belong to the same track are grouped into one chunk, which is what the
    /// emitted `stsc`/`stco` tables describe.
    ///
    /// Tracks may use different timescales, so DTS values are compared as exact
    /// rationals (`dts_a / ts_a` vs `dts_b / ts_b`) via cross-multiplication in
    /// `i128` — never by comparing raw ticks. Ties keep the lower track index
    /// first, so the ordering is deterministic.
    ///
    /// All offsets are relative to the start of the mdat payload (the byte
    /// immediately after the mdat box header). Callers add the absolute position
    /// of that payload to obtain file-level chunk offsets.
    fn collect_mdat_data(&self) -> (Vec<u8>, Vec<TrackLayout>) {
        let mut layouts: Vec<TrackLayout> =
            self.tracks.iter().map(|_| TrackLayout::default()).collect();
        let mut payload = Vec::new();

        // k-way merge over the per-track sample lists.
        let mut cursors = vec![0usize; self.tracks.len()];
        let mut order: Vec<(usize, usize)> = Vec::new();
        let total: usize = self.tracks.iter().map(|t| t.samples.len()).sum();
        order.reserve(total);

        loop {
            let mut best: Option<(usize, i128, i128)> = None;
            for (track_idx, track) in self.tracks.iter().enumerate() {
                let Some(sample) = track.samples.get(cursors[track_idx]) else {
                    continue;
                };
                let timescale = i128::from(track.codec.timescale().max(1));
                let dts = i128::from(sample.dts);
                match best {
                    None => best = Some((track_idx, dts, timescale)),
                    Some((_, best_dts, best_ts)) => {
                        if dts * best_ts < best_dts * timescale {
                            best = Some((track_idx, dts, timescale));
                        }
                    }
                }
            }
            let Some((track_idx, _, _)) = best else {
                break;
            };
            order.push((track_idx, cursors[track_idx]));
            cursors[track_idx] += 1;
        }

        // Group consecutive same-track runs into chunks.
        let mut i = 0usize;
        while i < order.len() {
            let track_idx = order[i].0;
            let chunk_offset = payload.len() as u64;
            let mut samples_in_chunk = 0u32;
            while i < order.len() && order[i].0 == track_idx {
                let sample_idx = order[i].1;
                if let Some(sample) = self.tracks[track_idx].samples.get(sample_idx) {
                    payload.extend_from_slice(&sample.data);
                }
                samples_in_chunk += 1;
                i += 1;
            }
            layouts[track_idx].chunk_offsets.push(chunk_offset);
            layouts[track_idx]
                .chunk_sample_counts
                .push(samples_in_chunk);
        }

        (payload, layouts)
    }

    fn build_moov_progressive(
        &self,
        layouts: &[TrackLayout],
        mdat_data_start: u64,
        force_co64: bool,
    ) -> Vec<u8> {
        let mut content = Vec::new();
        content.extend(build_mvhd(self.max_duration_ms()));

        let empty = TrackLayout::default();
        for (i, track) in self.tracks.iter().enumerate() {
            let layout = layouts.get(i).unwrap_or(&empty);
            // Absolute offsets = relative + mdat_data_start
            let absolute = TrackLayout {
                chunk_offsets: layout
                    .chunk_offsets
                    .iter()
                    .map(|&o| o.saturating_add(mdat_data_start))
                    .collect(),
                chunk_sample_counts: layout.chunk_sample_counts.clone(),
            };
            content.extend(build_trak(track, &absolute, force_co64));
        }

        encode_box(b"moov", &content)
    }

    fn max_duration_ms(&self) -> u32 {
        const MOVIE_TIMESCALE: u32 = 1000;
        self.tracks
            .iter()
            .map(|t| {
                let ts = t.codec.timescale();
                if ts == 0 {
                    0u32
                } else {
                    let dur_ms = t.total_duration() * u64::from(MOVIE_TIMESCALE) / u64::from(ts);
                    u32::try_from(dur_ms).unwrap_or(u32::MAX)
                }
            })
            .max()
            .unwrap_or(0)
    }

    // ── Fragmented layout ─────────────────────────────────────────────────

    fn write_fragmented(
        &self,
        writer: &mut dyn Write,
        _ftyp_size: u64,
    ) -> Result<(), SimpleMp4Error> {
        // moov box (with mvex)
        let moov = self.build_moov_fragmented();
        writer.write_all(&moov)?;

        // One moof+mdat fragment per track
        for (seq, track) in self.tracks.iter().enumerate() {
            if track.samples.is_empty() {
                continue;
            }
            let frag = build_fragment(track, (seq + 1) as u32);
            writer.write_all(&frag)?;
        }

        Ok(())
    }

    fn build_moov_fragmented(&self) -> Vec<u8> {
        let mut content = Vec::new();
        content.extend(build_mvhd(0));

        // mvex with trex for each track
        let mut mvex_content = Vec::new();
        for track in &self.tracks {
            mvex_content.extend(build_trex(track.track_id));
        }
        content.extend(encode_box(b"mvex", &mvex_content));

        // Init-segment traks: ISO/IEC 14496-12 §8.8.1 requires the sample
        // tables in a fragmented file's `moov` to be empty — the samples are
        // described by the `trun` boxes in each fragment. Emitting the full
        // tables here would make every sample appear twice to a reader that
        // understands both.
        for track in &self.tracks {
            content.extend(build_trak_fragmented(track));
        }

        encode_box(b"moov", &content)
    }
}

// ─── Track chunk layout ──────────────────────────────────────────────────────

/// Where one track's samples ended up inside `mdat`.
///
/// `chunk_offsets[i]` is the byte offset of chunk `i` and
/// `chunk_sample_counts[i]` how many of this track's samples it holds; together
/// they drive the `stco`/`co64` and `stsc` tables.
#[derive(Debug, Default, Clone)]
struct TrackLayout {
    chunk_offsets: Vec<u64>,
    chunk_sample_counts: Vec<u32>,
}

// ─── Box encoding helpers ────────────────────────────────────────────────────

/// Builds an ISOBMFF box header for a payload of `payload_len` bytes.
///
/// Emits the compact 8-byte form (`size` + `type`) whenever the total box size
/// fits in 32 bits, and the 16-byte `largesize` form (`size == 1`, `type`,
/// 64-bit `largesize`) when it does not. Silently truncating a >4 GiB box to a
/// `u32` produces a file whose `mdat` claims a wrong length and whose sample
/// offsets all point past the end.
fn build_box_header(box_type: &[u8; 4], payload_len: u64) -> Vec<u8> {
    let compact_total = payload_len.saturating_add(8);
    let mut out = Vec::with_capacity(16);
    if let Ok(total) = u32::try_from(compact_total) {
        out.extend_from_slice(&total.to_be_bytes());
        out.extend_from_slice(box_type);
    } else {
        out.extend_from_slice(&1u32.to_be_bytes()); // size == 1 → largesize follows
        out.extend_from_slice(box_type);
        out.extend_from_slice(&payload_len.saturating_add(16).to_be_bytes());
    }
    out
}

/// Builds the header for the `mdat` box holding `payload_len` bytes.
///
/// Returns 8 bytes for the normal form and 16 bytes for the 64-bit
/// `largesize` form.
fn build_mdat_header(payload_len: u64) -> Vec<u8> {
    build_box_header(b"mdat", payload_len)
}

/// Encode a regular ISOBMFF box: header + payload.
///
/// Uses the 64-bit `largesize` header form when the box exceeds `u32::MAX`.
pub(super) fn encode_box(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let header = build_box_header(box_type, payload.len() as u64);
    let mut out = Vec::with_capacity(header.len() + payload.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    out
}

/// Encode a FullBox: 4-byte size + 4-byte type + 1-byte version + 3-byte flags + payload.
fn encode_full_box(box_type: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut full = Vec::with_capacity(4 + payload.len());
    full.push(version);
    full.push(((flags >> 16) & 0xFF) as u8);
    full.push(((flags >> 8) & 0xFF) as u8);
    full.push((flags & 0xFF) as u8);
    full.extend_from_slice(payload);
    encode_box(box_type, &full)
}

// ─── ftyp ───────────────────────────────────────────────────────────────────

fn build_ftyp(config: &SimpleMp4Config) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&config.brand);
    c.extend_from_slice(&config.minor_version.to_be_bytes());
    for brand in &config.compatible_brands {
        c.extend_from_slice(brand);
    }
    encode_box(b"ftyp", &c)
}

// ─── Identity matrix (3×3, fixed-point) ─────────────────────────────────────

const IDENTITY_MATRIX: [u8; 36] = [
    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x40, 0x00, 0x00, 0x00,
];

// ─── mvhd ───────────────────────────────────────────────────────────────────

fn build_mvhd(duration_ms: u32) -> Vec<u8> {
    const MOVIE_TIMESCALE: u32 = 1000;
    let mut c = Vec::new();
    c.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    c.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    c.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes()); // timescale
    c.extend_from_slice(&duration_ms.to_be_bytes()); // duration
    c.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate = 1.0
    c.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
    c.extend_from_slice(&[0u8; 10]); // reserved
    c.extend_from_slice(&IDENTITY_MATRIX);
    c.extend_from_slice(&[0u8; 24]); // pre_defined
    c.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // next_track_id
    encode_full_box(b"mvhd", 0, 0, &c)
}

// ─── trak ───────────────────────────────────────────────────────────────────

fn build_trak(track: &Mp4TrackWriter, layout: &TrackLayout, force_co64: bool) -> Vec<u8> {
    let mut content = Vec::new();
    content.extend(build_tkhd(track));
    content.extend(build_mdia(track, layout, force_co64, false));
    encode_box(b"trak", &content)
}

/// Builds a `trak` for a fragmented init segment: same headers, but with empty
/// sample tables.
fn build_trak_fragmented(track: &Mp4TrackWriter) -> Vec<u8> {
    let mut content = Vec::new();
    content.extend(build_tkhd(track));
    content.extend(build_mdia(track, &TrackLayout::default(), false, true));
    encode_box(b"trak", &content)
}

// ─── tkhd ───────────────────────────────────────────────────────────────────

fn build_tkhd(track: &Mp4TrackWriter) -> Vec<u8> {
    const MOVIE_TIMESCALE: u32 = 1000;
    let ts = track.codec.timescale();
    let dur_ms = if ts == 0 {
        0u32
    } else {
        let raw = track.total_duration() * u64::from(MOVIE_TIMESCALE) / u64::from(ts);
        u32::try_from(raw).unwrap_or(u32::MAX)
    };

    let mut c = Vec::new();
    c.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    c.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    c.extend_from_slice(&track.track_id.to_be_bytes());
    c.extend_from_slice(&0u32.to_be_bytes()); // reserved
    c.extend_from_slice(&dur_ms.to_be_bytes());
    c.extend_from_slice(&[0u8; 8]); // reserved
    c.extend_from_slice(&0i16.to_be_bytes()); // layer
    c.extend_from_slice(&0i16.to_be_bytes()); // alternate_group

    // volume: 0x0100 for audio, 0 for video
    match &track.codec {
        TrackCodec::Audio(_) => c.extend_from_slice(&0x0100u16.to_be_bytes()),
        TrackCodec::Video(_) => c.extend_from_slice(&0u16.to_be_bytes()),
    }
    c.extend_from_slice(&0u16.to_be_bytes()); // reserved
    c.extend_from_slice(&IDENTITY_MATRIX);

    // width / height (fixed-point 16.16)
    match &track.codec {
        TrackCodec::Video(v) => {
            c.extend_from_slice(&(v.width << 16).to_be_bytes());
            c.extend_from_slice(&(v.height << 16).to_be_bytes());
        }
        TrackCodec::Audio(_) => {
            c.extend_from_slice(&0u32.to_be_bytes());
            c.extend_from_slice(&0u32.to_be_bytes());
        }
    }

    encode_full_box(b"tkhd", 0, 3, &c) // flags=3: track_enabled | track_in_movie
}

// ─── mdia ───────────────────────────────────────────────────────────────────

fn build_mdia(
    track: &Mp4TrackWriter,
    layout: &TrackLayout,
    force_co64: bool,
    empty_tables: bool,
) -> Vec<u8> {
    let mut content = Vec::new();
    content.extend(build_mdhd(track));
    content.extend(build_hdlr(track));
    content.extend(build_minf(track, layout, force_co64, empty_tables));
    encode_box(b"mdia", &content)
}

// ─── mdhd ───────────────────────────────────────────────────────────────────

fn build_mdhd(track: &Mp4TrackWriter) -> Vec<u8> {
    let ts = track.codec.timescale();
    let dur = u32::try_from(track.total_duration()).unwrap_or(u32::MAX);
    let mut c = Vec::new();
    c.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    c.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    c.extend_from_slice(&ts.to_be_bytes()); // timescale
    c.extend_from_slice(&dur.to_be_bytes()); // duration
    c.extend_from_slice(&0x55C4u16.to_be_bytes()); // language = 'und'
    c.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    encode_full_box(b"mdhd", 0, 0, &c)
}

// ─── hdlr ───────────────────────────────────────────────────────────────────

fn build_hdlr(track: &Mp4TrackWriter) -> Vec<u8> {
    let handler = track.codec.handler_type();
    let name = track.codec.handler_name();
    let mut c = Vec::new();
    c.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
    c.extend_from_slice(handler);
    c.extend_from_slice(&[0u8; 12]); // reserved
    c.extend_from_slice(name.as_bytes());
    c.push(0); // null terminator
    encode_full_box(b"hdlr", 0, 0, &c)
}

// ─── minf ───────────────────────────────────────────────────────────────────

fn build_minf(
    track: &Mp4TrackWriter,
    layout: &TrackLayout,
    force_co64: bool,
    empty_tables: bool,
) -> Vec<u8> {
    let mut content = Vec::new();

    match &track.codec {
        TrackCodec::Video(_) => {
            // vmhd: graphicsMode=0, opcolor=0
            let vmhd_payload = [0u8; 8];
            content.extend(encode_full_box(b"vmhd", 0, 1, &vmhd_payload));
        }
        TrackCodec::Audio(_) => {
            // smhd: balance=0, reserved=0
            let smhd_payload = [0u8; 4];
            content.extend(encode_full_box(b"smhd", 0, 0, &smhd_payload));
        }
    }

    content.extend(build_dinf());
    content.extend(build_stbl(track, layout, force_co64, empty_tables));
    encode_box(b"minf", &content)
}

// ─── dinf / dref ────────────────────────────────────────────────────────────

fn build_dinf() -> Vec<u8> {
    let url_box = encode_full_box(b"url ", 0, 1, &[]); // flags=1 → self-contained
    let mut dref_payload = Vec::new();
    dref_payload.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    dref_payload.extend(url_box);
    let dref = encode_full_box(b"dref", 0, 0, &dref_payload);
    encode_box(b"dinf", &dref)
}

// ─── stbl ───────────────────────────────────────────────────────────────────

fn build_stbl(
    track: &Mp4TrackWriter,
    layout: &TrackLayout,
    force_co64: bool,
    empty_tables: bool,
) -> Vec<u8> {
    let mut content = Vec::new();

    content.extend(build_stsd(track));

    if empty_tables {
        content.extend(encode_full_box(b"stts", 0, 0, &0u32.to_be_bytes()));
        content.extend(encode_full_box(b"stsc", 0, 0, &0u32.to_be_bytes()));
        let mut stsz = Vec::new();
        stsz.extend_from_slice(&0u32.to_be_bytes()); // sample_size
        stsz.extend_from_slice(&0u32.to_be_bytes()); // sample_count
        content.extend(encode_full_box(b"stsz", 0, 0, &stsz));
        content.extend(encode_full_box(b"stco", 0, 0, &0u32.to_be_bytes()));
        return encode_box(b"stbl", &content);
    }

    content.extend(build_stts(track));

    // ctts: only if any PTS ≠ DTS
    if track.samples.iter().any(|s| s.pts != s.dts) {
        content.extend(build_ctts(track));
    }

    content.extend(build_stsc(&layout.chunk_sample_counts));
    content.extend(build_stsz(track));
    content.extend(build_stco(&layout.chunk_offsets, force_co64));

    // stss: sync sample table (only if not all samples are sync)
    if matches!(&track.codec, TrackCodec::Video(_)) && track.samples.iter().any(|s| !s.is_sync) {
        content.extend(build_stss(track));
    }

    encode_box(b"stbl", &content)
}

// ─── stsd ───────────────────────────────────────────────────────────────────

fn build_stsd(track: &Mp4TrackWriter) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&1u32.to_be_bytes()); // entry_count

    match &track.codec {
        TrackCodec::Video(v) => c.extend(build_video_sample_entry(v)),
        TrackCodec::Audio(a) => c.extend(build_audio_sample_entry(a)),
    }

    encode_full_box(b"stsd", 0, 0, &c)
}

fn build_video_sample_entry(info: &VideoCodecInfo) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&[0u8; 6]); // reserved
    c.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    c.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
    c.extend_from_slice(&(info.width as u16).to_be_bytes()); // width
    c.extend_from_slice(&(info.height as u16).to_be_bytes()); // height
    c.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // horiz_resolution (72 dpi)
    c.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // vert_resolution (72 dpi)
    c.extend_from_slice(&0u32.to_be_bytes()); // reserved
    c.extend_from_slice(&1u16.to_be_bytes()); // frame_count
                                              // compressor_name (32 bytes, Pascal string)
    let mut comp = [0u8; 32];
    let name = b"OxiMedia";
    comp[0] = name.len() as u8;
    comp[1..1 + name.len()].copy_from_slice(name);
    c.extend_from_slice(&comp);
    c.extend_from_slice(&0x0018u16.to_be_bytes()); // depth = 24
    c.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

    // Optional codec config box
    if let (Some(cfg_fourcc), Some(cfg_data)) = (info.config_fourcc, &info.config_data) {
        c.extend(encode_box(&cfg_fourcc, cfg_data));
    }

    encode_box(&info.fourcc, &c)
}

fn build_audio_sample_entry(info: &AudioCodecInfo) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&[0u8; 6]); // reserved
    c.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    c.extend_from_slice(&[0u8; 8]); // reserved
    c.extend_from_slice(&info.channel_count.to_be_bytes()); // channel_count
    c.extend_from_slice(&16u16.to_be_bytes()); // sample_size (16 bits)
    c.extend_from_slice(&[0u8; 4]); // pre_defined + reserved
                                    // sample_rate as fixed-point 16.16
    c.extend_from_slice(&(info.sample_rate << 16).to_be_bytes());

    // Optional codec config box
    if let (Some(cfg_fourcc), Some(cfg_data)) = (info.config_fourcc, &info.config_data) {
        c.extend(encode_box(&cfg_fourcc, cfg_data));
    }

    encode_box(&info.fourcc, &c)
}

// ─── stts (time-to-sample) ──────────────────────────────────────────────────

fn build_stts(track: &Mp4TrackWriter) -> Vec<u8> {
    // Run-length encode sample durations
    let mut entries: Vec<(u32, u32)> = Vec::new(); // (count, delta)
    for sample in &track.samples {
        match entries.last_mut() {
            Some(last) if last.1 == sample.duration => last.0 += 1,
            _ => entries.push((1, sample.duration)),
        }
    }

    let mut c = Vec::new();
    c.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (count, delta) in &entries {
        c.extend_from_slice(&count.to_be_bytes());
        c.extend_from_slice(&delta.to_be_bytes());
    }
    encode_full_box(b"stts", 0, 0, &c)
}

// ─── ctts (composition time offsets) ────────────────────────────────────────

fn build_ctts(track: &Mp4TrackWriter) -> Vec<u8> {
    let mut entries: Vec<(u32, i32)> = Vec::new(); // (count, offset)
    for sample in &track.samples {
        let offset = (sample.pts - sample.dts) as i32;
        match entries.last_mut() {
            Some(last) if last.1 == offset => last.0 += 1,
            _ => entries.push((1, offset)),
        }
    }

    let mut c = Vec::new();
    c.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (count, offset) in &entries {
        c.extend_from_slice(&count.to_be_bytes());
        // signed as u32 bytes
        c.extend_from_slice(&(*offset as u32).to_be_bytes());
    }
    encode_full_box(b"ctts", 1, 0, &c) // version=1 for signed offsets
}

// ─── stsc (sample-to-chunk) ─────────────────────────────────────────────────

/// Builds the `stsc` (sample-to-chunk) table from the per-chunk sample counts
/// produced by the interleaver.
///
/// Runs of chunks holding the same number of samples are collapsed into a
/// single entry, exactly as ISO/IEC 14496-12 §8.7.4 prescribes.
fn build_stsc(chunk_sample_counts: &[u32]) -> Vec<u8> {
    // (first_chunk 1-based, samples_per_chunk, sample_description_index)
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for (index, &count) in chunk_sample_counts.iter().enumerate() {
        if entries.last().map(|e| e.1) != Some(count) {
            entries.push(((index + 1) as u32, count));
        }
    }

    let mut c = Vec::new();
    c.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (first_chunk, samples_per_chunk) in &entries {
        c.extend_from_slice(&first_chunk.to_be_bytes());
        c.extend_from_slice(&samples_per_chunk.to_be_bytes());
        c.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
    }
    encode_full_box(b"stsc", 0, 0, &c)
}

// ─── stsz (sample sizes) ────────────────────────────────────────────────────

fn build_stsz(track: &Mp4TrackWriter) -> Vec<u8> {
    let first_size = track.samples.first().map_or(0, |s| s.data.len() as u32);
    let all_same = track
        .samples
        .iter()
        .all(|s| s.data.len() as u32 == first_size);

    let mut c = Vec::new();
    if all_same && !track.samples.is_empty() {
        c.extend_from_slice(&first_size.to_be_bytes()); // sample_size (constant)
        c.extend_from_slice(&(track.samples.len() as u32).to_be_bytes()); // sample_count
    } else {
        c.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0 (variable)
        c.extend_from_slice(&(track.samples.len() as u32).to_be_bytes());
        for sample in &track.samples {
            c.extend_from_slice(&(sample.data.len() as u32).to_be_bytes());
        }
    }
    encode_full_box(b"stsz", 0, 0, &c)
}

// ─── stco (chunk offsets) ───────────────────────────────────────────────────

fn build_stco(chunk_offsets: &[u64], force_co64: bool) -> Vec<u8> {
    let use_64 = force_co64 || chunk_offsets.iter().any(|&o| o > u64::from(u32::MAX));

    let mut c = Vec::new();
    c.extend_from_slice(&(chunk_offsets.len() as u32).to_be_bytes());
    if use_64 {
        for &off in chunk_offsets {
            c.extend_from_slice(&off.to_be_bytes());
        }
        encode_full_box(b"co64", 0, 0, &c)
    } else {
        for &off in chunk_offsets {
            c.extend_from_slice(&(off as u32).to_be_bytes());
        }
        encode_full_box(b"stco", 0, 0, &c)
    }
}

// ─── stss (sync sample table) ───────────────────────────────────────────────

fn build_stss(track: &Mp4TrackWriter) -> Vec<u8> {
    let sync_indices: Vec<u32> = track
        .samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_sync)
        .map(|(i, _)| (i + 1) as u32) // 1-based
        .collect();

    let mut c = Vec::new();
    c.extend_from_slice(&(sync_indices.len() as u32).to_be_bytes());
    for &idx in &sync_indices {
        c.extend_from_slice(&idx.to_be_bytes());
    }
    encode_full_box(b"stss", 0, 0, &c)
}

// ─── trex ───────────────────────────────────────────────────────────────────

fn build_trex(track_id: u32) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&track_id.to_be_bytes());
    c.extend_from_slice(&1u32.to_be_bytes()); // default_sample_description_index
    c.extend_from_slice(&0u32.to_be_bytes()); // default_sample_duration
    c.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size
    c.extend_from_slice(&0u32.to_be_bytes()); // default_sample_flags
    encode_full_box(b"trex", 0, 0, &c)
}

// ─── moof + mdat (fragmented) ────────────────────────────────────────────────

fn build_fragment(track: &Mp4TrackWriter, sequence_number: u32) -> Vec<u8> {
    // mfhd
    let mut mfhd_c = Vec::new();
    mfhd_c.extend_from_slice(&sequence_number.to_be_bytes());
    let mfhd = encode_full_box(b"mfhd", 0, 0, &mfhd_c);

    // tfhd — default-base-is-moof: trun.data_offset is relative to the first
    // byte of the enclosing moof box.
    let mut tfhd_c = Vec::new();
    tfhd_c.extend_from_slice(&track.track_id.to_be_bytes());
    let tfhd = encode_full_box(b"tfhd", 0, 0x0002_0000, &tfhd_c);

    // tfdt (base_media_decode_time = first sample DTS)
    let base_dts = track.samples.first().map_or(0i64, |s| s.dts).max(0);
    let mut tfdt_c = Vec::new();
    tfdt_c.extend_from_slice(&(base_dts as u64).to_be_bytes());
    let tfdt = encode_full_box(b"tfdt", 1, 0, &tfdt_c);

    // trun flags: data-offset(0x001) | sample-flags(0x400)
    //           | duration(0x100) | size(0x200) | composition-offset(0x800)
    let trun_flags: u32 = 0x0000_0F01;
    let build_trun = |data_offset: i32| -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(track.samples.len() as u32).to_be_bytes()); // sample_count
        c.extend_from_slice(&data_offset.to_be_bytes());
        for sample in &track.samples {
            c.extend_from_slice(&sample.duration.to_be_bytes());
            c.extend_from_slice(&(sample.data.len() as u32).to_be_bytes());
            // sample_flags: bit 16 is `sample_is_non_sync_sample`; sync samples
            // additionally advertise sample_depends_on = 2 (independent).
            let flags: u32 = if sample.is_sync {
                0x0200_0000
            } else {
                0x0101_0000
            };
            c.extend_from_slice(&flags.to_be_bytes());
            c.extend_from_slice(&((sample.pts - sample.dts) as i32).to_be_bytes());
        }
        // version=1 → signed composition offsets
        encode_full_box(b"trun", 1, trun_flags, &c)
    };

    let build_moof = |data_offset: i32| -> Vec<u8> {
        let mut traf_content = Vec::new();
        traf_content.extend_from_slice(&tfhd);
        traf_content.extend_from_slice(&tfdt);
        traf_content.extend(build_trun(data_offset));
        let traf = encode_box(b"traf", &traf_content);

        let mut moof_content = Vec::new();
        moof_content.extend_from_slice(&mfhd);
        moof_content.extend(traf);
        encode_box(b"moof", &moof_content)
    };

    // Two-pass: the trun data_offset counts from the start of the moof, so the
    // moof has to be sized before the value can be written. Writing a 0
    // placeholder and never patching it (as this builder used to) makes every
    // reader fetch sample bytes from inside the moof header.
    let moof_size = build_moof(0).len() as i32;
    let mdat_payload_len: u64 = track.samples.iter().map(|s| s.data.len() as u64).sum();
    let mdat_header = build_mdat_header(mdat_payload_len);
    let moof = build_moof(moof_size + mdat_header.len() as i32);

    let mut result = Vec::new();
    result.extend(moof);
    result.extend_from_slice(&mdat_header);
    for sample in &track.samples {
        result.extend_from_slice(&sample.data);
    }
    result
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ── Helpers ──────────────────────────────────────────────────────────

    fn read_u32_be(buf: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ])
    }

    fn find_box(buf: &[u8], box_type: &[u8; 4]) -> bool {
        buf.windows(4).any(|w| w == box_type)
    }

    fn count_box(buf: &[u8], box_type: &[u8; 4]) -> usize {
        buf.windows(4).filter(|w| *w == box_type).count()
    }

    fn default_video_codec() -> TrackCodec {
        TrackCodec::Video(VideoCodecInfo::new(*b"av01", 1920, 1080, 90000))
    }

    fn default_audio_codec() -> TrackCodec {
        TrackCodec::Audio(AudioCodecInfo::new(*b"Opus", 48000, 2))
    }

    fn video_sample(pts: i64, sync: bool) -> Mp4Sample {
        Mp4Sample {
            pts,
            dts: pts,
            duration: 3000,
            is_sync: sync,
            data: vec![0xAA; 100],
        }
    }

    fn audio_sample(pts: i64) -> Mp4Sample {
        Mp4Sample {
            pts,
            dts: pts,
            duration: 960,
            is_sync: true,
            data: vec![0xBB; 50],
        }
    }

    fn mux_to_vec(muxer: &mut SimpleMp4Muxer) -> Vec<u8> {
        let mut cur = Cursor::new(Vec::<u8>::new());
        muxer.finalize(&mut cur).expect("finalize ok");
        cur.into_inner()
    }

    // ── add_track ────────────────────────────────────────────────────────

    #[test]
    fn test_add_track_returns_sequential_ids() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id1 = muxer.add_track(default_video_codec());
        let id2 = muxer.add_track(default_audio_codec());
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(muxer.track_count(), 2);
    }

    #[test]
    fn test_track_lookup() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        let t = muxer.track(id).expect("track exists");
        assert_eq!(t.track_id, 1);
    }

    // ── write_sample errors ───────────────────────────────────────────────

    #[test]
    fn test_write_sample_unknown_track() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let err = muxer.write_sample(99, video_sample(0, true));
        assert!(matches!(err, Err(SimpleMp4Error::UnknownTrack(99))));
    }

    #[test]
    fn test_write_sample_after_finalize() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");
        let _ = mux_to_vec(&mut muxer);
        let err = muxer.write_sample(id, video_sample(3000, false));
        assert!(matches!(err, Err(SimpleMp4Error::AlreadyFinalized)));
    }

    #[test]
    fn test_finalize_no_tracks() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let mut out = Cursor::new(Vec::<u8>::new());
        let err = muxer.finalize(&mut out);
        assert!(matches!(err, Err(SimpleMp4Error::NoTracks)));
    }

    #[test]
    fn test_finalize_twice() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");
        let _ = mux_to_vec(&mut muxer);
        let mut out = Cursor::new(Vec::<u8>::new());
        let err = muxer.finalize(&mut out);
        assert!(matches!(err, Err(SimpleMp4Error::AlreadyFinalized)));
    }

    // ── Box structure ─────────────────────────────────────────────────────

    #[test]
    fn test_progressive_contains_required_boxes() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");
        let out = mux_to_vec(&mut muxer);

        assert!(find_box(&out, b"ftyp"), "missing ftyp");
        assert!(find_box(&out, b"moov"), "missing moov");
        assert!(find_box(&out, b"mvhd"), "missing mvhd");
        assert!(find_box(&out, b"trak"), "missing trak");
        assert!(find_box(&out, b"tkhd"), "missing tkhd");
        assert!(find_box(&out, b"mdia"), "missing mdia");
        assert!(find_box(&out, b"mdhd"), "missing mdhd");
        assert!(find_box(&out, b"hdlr"), "missing hdlr");
        assert!(find_box(&out, b"minf"), "missing minf");
        assert!(find_box(&out, b"vmhd"), "missing vmhd");
        assert!(find_box(&out, b"dinf"), "missing dinf");
        assert!(find_box(&out, b"dref"), "missing dref");
        assert!(find_box(&out, b"stbl"), "missing stbl");
        assert!(find_box(&out, b"stsd"), "missing stsd");
        assert!(find_box(&out, b"stts"), "missing stts");
        assert!(find_box(&out, b"stsc"), "missing stsc");
        assert!(find_box(&out, b"stsz"), "missing stsz");
        assert!(find_box(&out, b"mdat"), "missing mdat");
    }

    #[test]
    fn test_ftyp_brand() {
        let config = SimpleMp4Config {
            brand: *b"av01",
            ..Default::default()
        };
        let mut muxer = SimpleMp4Muxer::new(config);
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");
        let out = mux_to_vec(&mut muxer);
        // ftyp is the first box; major brand is at bytes 8..12
        assert_eq!(&out[8..12], b"av01");
    }

    #[test]
    fn test_audio_track_has_smhd() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_audio_codec());
        muxer.write_sample(id, audio_sample(0)).expect("ok");
        let out = mux_to_vec(&mut muxer);
        assert!(find_box(&out, b"smhd"), "missing smhd for audio track");
    }

    #[test]
    fn test_two_tracks_two_trak_boxes() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let vid = muxer.add_track(default_video_codec());
        let aud = muxer.add_track(default_audio_codec());
        muxer.write_sample(vid, video_sample(0, true)).expect("ok");
        muxer.write_sample(aud, audio_sample(0)).expect("ok");
        let out = mux_to_vec(&mut muxer);
        assert_eq!(count_box(&out, b"trak"), 2, "expected exactly 2 trak boxes");
    }

    #[test]
    fn test_stss_present_for_mixed_keyframes() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");
        muxer
            .write_sample(id, video_sample(3000, false))
            .expect("ok");
        muxer
            .write_sample(id, video_sample(6000, false))
            .expect("ok");
        let out = mux_to_vec(&mut muxer);
        assert!(find_box(&out, b"stss"), "missing stss for mixed keyframes");
    }

    #[test]
    fn test_stss_absent_when_all_keyframes() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_audio_codec());
        muxer.write_sample(id, audio_sample(0)).expect("ok");
        muxer.write_sample(id, audio_sample(960)).expect("ok");
        let out = mux_to_vec(&mut muxer);
        // Audio track: no stss even if all sync, because stss is only written for Video
        assert!(
            !find_box(&out, b"stss"),
            "stss should be absent for audio-only"
        );
    }

    // ── Sample data integrity ────────────────────────────────────────────

    #[test]
    fn test_sample_data_in_mdat() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        muxer
            .write_sample(
                id,
                Mp4Sample {
                    pts: 0,
                    dts: 0,
                    duration: 3000,
                    is_sync: true,
                    data: data.clone(),
                },
            )
            .expect("ok");
        let out = mux_to_vec(&mut muxer);

        // The sample data must appear somewhere in the output buffer.
        // The mdat payload begins 4 bytes after the "mdat" tag (which itself is
        // 4 bytes into the box: [size(4)][type(4)][payload]).
        let mdat_tag_pos = out
            .windows(4)
            .position(|w| w == b"mdat")
            .expect("mdat exists");
        let payload_start = mdat_tag_pos + 4; // skip the "mdat" type bytes
        let after_mdat = &out[payload_start..];
        assert!(
            after_mdat.windows(4).any(|w| w == data.as_slice()),
            "sample data not found in mdat region"
        );
    }

    #[test]
    fn test_multiple_samples_all_in_mdat() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        for i in 0u32..5 {
            let pts = (i * 3000) as i64;
            muxer
                .write_sample(id, video_sample(pts, i == 0))
                .expect("ok");
        }
        let out = mux_to_vec(&mut muxer);
        // There should be 5 × 100-byte samples = 500 bytes of payload in mdat
        let mdat_pos = out.windows(4).position(|w| w == b"mdat").expect("mdat");
        let mdat_size = read_u32_be(&out, mdat_pos - 4) as usize;
        assert_eq!(mdat_size, 8 + 5 * 100, "unexpected mdat size");
    }

    // ── Fragmented mode ───────────────────────────────────────────────────

    #[test]
    fn test_fragmented_mode_has_mvex() {
        let config = SimpleMp4Config {
            fragmented: true,
            ..Default::default()
        };
        let mut muxer = SimpleMp4Muxer::new(config);
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");
        let out = mux_to_vec(&mut muxer);
        assert!(find_box(&out, b"mvex"), "missing mvex in fragmented mode");
        assert!(find_box(&out, b"trex"), "missing trex in fragmented mode");
        assert!(find_box(&out, b"moof"), "missing moof in fragmented mode");
        assert!(find_box(&out, b"mfhd"), "missing mfhd in fragmented mode");
        assert!(find_box(&out, b"traf"), "missing traf in fragmented mode");
        assert!(find_box(&out, b"trun"), "missing trun in fragmented mode");
    }

    // ── Codec info ────────────────────────────────────────────────────────

    #[test]
    fn test_video_codec_info_new() {
        let v = VideoCodecInfo::new(*b"av01", 3840, 2160, 90000);
        assert_eq!(v.width, 3840);
        assert_eq!(v.height, 2160);
        assert_eq!(v.timescale, 90000);
        assert!(v.config_data.is_none());
    }

    #[test]
    fn test_video_codec_info_with_config() {
        let v =
            VideoCodecInfo::new(*b"av01", 1280, 720, 90000).with_config(*b"av1C", vec![0x81, 0x04]);
        assert!(v.config_data.is_some());
        assert_eq!(v.config_fourcc, Some(*b"av1C"));
    }

    #[test]
    fn test_audio_codec_info_new() {
        let a = AudioCodecInfo::new(*b"Opus", 48000, 2);
        assert_eq!(a.timescale, 48000);
        assert_eq!(a.channel_count, 2);
    }

    #[test]
    fn test_track_codec_timescale() {
        let v = TrackCodec::Video(VideoCodecInfo::new(*b"av01", 1920, 1080, 90000));
        let a = TrackCodec::Audio(AudioCodecInfo::new(*b"Opus", 48000, 2));
        assert_eq!(v.timescale(), 90000);
        assert_eq!(a.timescale(), 48000);
    }

    #[test]
    fn test_track_codec_handler_type() {
        let v = TrackCodec::Video(VideoCodecInfo::new(*b"av01", 1920, 1080, 90000));
        let a = TrackCodec::Audio(AudioCodecInfo::new(*b"Opus", 48000, 2));
        assert_eq!(v.handler_type(), b"vide");
        assert_eq!(a.handler_type(), b"soun");
    }

    // ── 64-bit box headers ────────────────────────────────────────────────

    #[test]
    fn mdat_header_uses_the_compact_form_when_it_fits() {
        let header = build_mdat_header(100);
        assert_eq!(header.len(), 8);
        assert_eq!(read_u32_be(&header, 0), 108);
        assert_eq!(&header[4..8], b"mdat");
    }

    #[test]
    fn mdat_header_switches_to_largesize_past_u32() {
        // 8 + payload must exceed u32::MAX for the compact form to be invalid.
        let payload_len = u64::from(u32::MAX);
        let header = build_mdat_header(payload_len);
        assert_eq!(header.len(), 16, "largesize header is 16 bytes");
        assert_eq!(read_u32_be(&header, 0), 1, "size == 1 signals largesize");
        assert_eq!(&header[4..8], b"mdat");
        let largesize = u64::from_be_bytes([
            header[8], header[9], header[10], header[11], header[12], header[13], header[14],
            header[15],
        ]);
        assert_eq!(largesize, payload_len + 16);
    }

    #[test]
    fn mdat_header_boundary_is_exact() {
        // Largest payload that still fits the 8-byte header.
        let fits = u64::from(u32::MAX) - 8;
        assert_eq!(build_mdat_header(fits).len(), 8);
        assert_eq!(build_mdat_header(fits + 1).len(), 16);
    }

    // ── co64 selection ────────────────────────────────────────────────────

    #[test]
    fn build_stco_emits_co64_when_forced() {
        let out = build_stco(&[0, 1024], true);
        assert_eq!(&out[4..8], b"co64");
        // FullBox header (12) + entry_count already consumed → 2 * 8 bytes
        assert_eq!(out.len(), 12 + 4 + 16);
    }

    #[test]
    fn build_stco_emits_co64_for_large_offsets() {
        let out = build_stco(&[u64::from(u32::MAX) + 1], false);
        assert_eq!(&out[4..8], b"co64");
    }

    #[test]
    fn build_stco_emits_stco_for_small_offsets() {
        let out = build_stco(&[0, 512], false);
        assert_eq!(&out[4..8], b"stco");
        assert_eq!(out.len(), 12 + 4 + 8);
    }

    #[test]
    fn resolve_co64_triggers_past_four_gib() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        muxer.write_sample(id, video_sample(0, true)).expect("ok");

        let small = vec![TrackLayout {
            chunk_offsets: vec![0, 4096],
            chunk_sample_counts: vec![1, 1],
        }];
        assert!(!muxer.resolve_co64(32, 8, &small));

        let huge = vec![TrackLayout {
            chunk_offsets: vec![0, 5 * 1024 * 1024 * 1024],
            chunk_sample_counts: vec![1, 1],
        }];
        assert!(
            muxer.resolve_co64(32, 16, &huge),
            "a 5 GiB mdat must select co64"
        );
    }

    // ── stsc run-length encoding ──────────────────────────────────────────

    #[test]
    fn build_stsc_collapses_equal_runs() {
        // 4 chunks: 3, 3, 1, 1 samples → two entries.
        let out = build_stsc(&[3, 3, 1, 1]);
        assert_eq!(&out[4..8], b"stsc");
        let entry_count = read_u32_be(&out, 12);
        assert_eq!(entry_count, 2);
        assert_eq!(read_u32_be(&out, 16), 1, "first_chunk of entry 0");
        assert_eq!(read_u32_be(&out, 20), 3, "samples_per_chunk of entry 0");
        assert_eq!(read_u32_be(&out, 24), 1, "sample_description_index");
        assert_eq!(read_u32_be(&out, 28), 3, "first_chunk of entry 1");
        assert_eq!(read_u32_be(&out, 32), 1, "samples_per_chunk of entry 1");
    }

    #[test]
    fn build_stsc_empty_track_has_no_entries() {
        let out = build_stsc(&[]);
        assert_eq!(read_u32_be(&out, 12), 0);
    }

    // ── DTS interleaving ──────────────────────────────────────────────────

    #[test]
    fn collect_mdat_data_interleaves_by_dts() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let video = muxer.add_track(TrackCodec::Video(VideoCodecInfo::new(
            *b"av01", 320, 240, 90_000,
        )));
        let audio = muxer.add_track(TrackCodec::Audio(AudioCodecInfo::new(*b"Opus", 48_000, 2)));

        // Two 33 ms video frames and four 20 ms audio frames, i.e. video at
        // 0 ms / 33 ms and audio at 0 / 20 / 40 / 60 ms.
        for i in 0i64..2 {
            muxer
                .write_sample(
                    video,
                    Mp4Sample {
                        pts: i * 3000,
                        dts: i * 3000,
                        duration: 3000,
                        is_sync: true,
                        data: vec![0x11; 4],
                    },
                )
                .expect("ok");
        }
        for i in 0i64..4 {
            muxer
                .write_sample(
                    audio,
                    Mp4Sample {
                        pts: i * 960,
                        dts: i * 960,
                        duration: 960,
                        is_sync: true,
                        data: vec![0x22; 2],
                    },
                )
                .expect("ok");
        }

        let (payload, layouts) = muxer.collect_mdat_data();
        assert_eq!(payload.len(), 2 * 4 + 4 * 2);

        // Expected merge order (ms): V0(0) A0(0) A1(20) V1(33) A2(40) A3(60).
        // Ties go to the lower track index, so V0 precedes A0.
        // Chunks: [V0] [A0,A1] [V1] [A2,A3]
        assert_eq!(layouts[0].chunk_sample_counts, vec![1, 1]);
        assert_eq!(layouts[1].chunk_sample_counts, vec![2, 2]);
        // Video samples are 4 bytes, audio samples 2 bytes.
        assert_eq!(layouts[0].chunk_offsets, vec![0, 8]);
        assert_eq!(layouts[1].chunk_offsets, vec![4, 12]);

        // And the bytes must be laid out in that same order.
        assert_eq!(
            payload,
            vec![
                0x11, 0x11, 0x11, 0x11, // V0
                0x22, 0x22, 0x22, 0x22, // A0, A1
                0x11, 0x11, 0x11, 0x11, // V1
                0x22, 0x22, 0x22, 0x22, // A2, A3
            ]
        );
    }

    #[test]
    fn collect_mdat_data_single_track_is_one_chunk() {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let id = muxer.add_track(default_video_codec());
        for i in 0i64..3 {
            muxer
                .write_sample(id, video_sample(i * 3000, i == 0))
                .expect("ok");
        }
        let (_, layouts) = muxer.collect_mdat_data();
        assert_eq!(layouts[0].chunk_offsets, vec![0]);
        assert_eq!(layouts[0].chunk_sample_counts, vec![3]);
    }
}

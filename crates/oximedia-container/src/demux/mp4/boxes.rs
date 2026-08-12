//! MP4 box (atom) parsing.
//!
//! This module provides parsers for various MP4 box types including:
//! - `ftyp` - File type and compatibility
//! - `moov` - Movie container (metadata)
//! - `mvhd` - Movie header
//! - `trak` - Track container
//! - `tkhd` - Track header
//! - Sample tables (`stts`, `stsc`, `stsz`, `stco`, `co64`, `stss`, `ctts`)

use super::atom::Mp4Atom;
use super::fragments::{parse_mvex, TrexBox};
use crate::track_header::TransformMatrix;
use oximedia_core::{OxiError, OxiResult};

/// Maximum container-box nesting depth accepted while recursing through
/// `trak` sub-containers (`mdia`/`minf`/`stbl`/`edts`).
///
/// A malicious file can nest these container boxes arbitrarily deep to exhaust
/// the call stack (stack overflow / abort). Legitimate MP4 nesting is only a
/// handful of levels deep, so a cap of 32 rejects hostile inputs without
/// affecting any real file.
const MAX_BOX_DEPTH: usize = 32;

/// Computes the exclusive end offset (`offset + box_total`) of a box during
/// sequential box iteration, returning `None` on `usize` overflow.
///
/// Defends against a malformed box that declares a 64-bit extended size close
/// to `u64::MAX`: once a preceding sibling box has advanced `offset` past 0,
/// the naive `offset + box_total` wraps around, silently passing the
/// `> data.len()` bounds check and letting the subsequent
/// `&data[offset + header_size..offset + box_total]` slice panic (start > end
/// or out of range). Callers must treat `None` as a malformed/oversized box and
/// stop parsing.
#[inline]
fn box_end(offset: usize, box_total: usize) -> Option<usize> {
    offset.checked_add(box_total)
}

/// Box header containing size and type information.
///
/// Every MP4 box starts with an 8-byte header (or 16 bytes for extended size).
#[derive(Clone, Debug)]
pub struct BoxHeader {
    /// Total box size including header (0 means extends to end of file).
    pub size: u64,
    /// Box type (4CC).
    pub box_type: BoxType,
    /// Header size in bytes (8 for normal, 16 for extended).
    pub header_size: u8,
}

/// Box type represented as a 4-byte code (4CC).
///
/// Common box types are available as constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoxType(pub [u8; 4]);

impl BoxType {
    /// Creates a box type from a 4-byte array.
    #[must_use]
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    /// Creates a box type from a string.
    ///
    /// The string should be exactly 4 ASCII characters.
    /// Missing bytes are padded with zeros.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        let bytes = s.as_bytes();
        Self([
            bytes.first().copied().unwrap_or(0),
            bytes.get(1).copied().unwrap_or(0),
            bytes.get(2).copied().unwrap_or(0),
            bytes.get(3).copied().unwrap_or(0),
        ])
    }

    /// Returns the box type as a string slice.
    ///
    /// Returns `"????"` if the bytes are not valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("????")
    }

    /// Returns the box type as a `u32` for easy comparison.
    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

impl std::fmt::Display for BoxType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// Common box type constants
impl BoxType {
    /// File type box.
    pub const FTYP: Self = Self::new(*b"ftyp");
    /// Movie container box.
    pub const MOOV: Self = Self::new(*b"moov");
    /// Movie header box.
    pub const MVHD: Self = Self::new(*b"mvhd");
    /// Track container box.
    pub const TRAK: Self = Self::new(*b"trak");
    /// Track header box.
    pub const TKHD: Self = Self::new(*b"tkhd");
    /// Media container box.
    pub const MDIA: Self = Self::new(*b"mdia");
    /// Media header box.
    pub const MDHD: Self = Self::new(*b"mdhd");
    /// Handler reference box.
    pub const HDLR: Self = Self::new(*b"hdlr");
    /// Media information box.
    pub const MINF: Self = Self::new(*b"minf");
    /// Sample table box.
    pub const STBL: Self = Self::new(*b"stbl");
    /// Sample description box.
    pub const STSD: Self = Self::new(*b"stsd");
    /// Time-to-sample box.
    pub const STTS: Self = Self::new(*b"stts");
    /// Sample-to-chunk box.
    pub const STSC: Self = Self::new(*b"stsc");
    /// Sample size box.
    pub const STSZ: Self = Self::new(*b"stsz");
    /// Chunk offset box (32-bit).
    pub const STCO: Self = Self::new(*b"stco");
    /// Chunk offset box (64-bit).
    pub const CO64: Self = Self::new(*b"co64");
    /// Sync sample box.
    pub const STSS: Self = Self::new(*b"stss");
    /// Composition time-to-sample box.
    pub const CTTS: Self = Self::new(*b"ctts");
    /// Media data box.
    pub const MDAT: Self = Self::new(*b"mdat");
    /// Free space box.
    pub const FREE: Self = Self::new(*b"free");
    /// Free space box (alternate).
    pub const SKIP: Self = Self::new(*b"skip");
    /// User data box.
    pub const UDTA: Self = Self::new(*b"udta");
    /// Metadata box.
    pub const META: Self = Self::new(*b"meta");
    /// Edit list container box.
    pub const EDTS: Self = Self::new(*b"edts");
    /// Edit list box.
    pub const ELST: Self = Self::new(*b"elst");
    /// Movie extends box (declares that the file is fragmented).
    pub const MVEX: Self = Self::new(*b"mvex");
    /// Track extends box (per-track fragment defaults).
    pub const TREX: Self = Self::new(*b"trex");
    /// Movie fragment box.
    pub const MOOF: Self = Self::new(*b"moof");
    /// Movie fragment header box.
    pub const MFHD: Self = Self::new(*b"mfhd");
    /// Track fragment box.
    pub const TRAF: Self = Self::new(*b"traf");
    /// Track fragment header box.
    pub const TFHD: Self = Self::new(*b"tfhd");
    /// Track fragment decode time box.
    pub const TFDT: Self = Self::new(*b"tfdt");
    /// Track fragment run box.
    pub const TRUN: Self = Self::new(*b"trun");
    /// Segment index box.
    pub const SIDX: Self = Self::new(*b"sidx");
    /// Segment type box (CMAF media segments).
    pub const STYP: Self = Self::new(*b"styp");
}

impl BoxHeader {
    /// Parses a box header from the beginning of a byte slice.
    ///
    /// The header is 8 bytes for normal boxes, or 16 bytes when using
    /// extended size (size field == 1).
    ///
    /// # Errors
    ///
    /// Returns an error if the data is too short.
    pub fn parse(data: &[u8]) -> OxiResult<Self> {
        if data.len() < 8 {
            return Err(OxiError::UnexpectedEof);
        }

        let size32 = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let mut box_type = [0u8; 4];
        box_type.copy_from_slice(&data[4..8]);

        let (size, header_size) = if size32 == 1 {
            // Extended size (64-bit)
            if data.len() < 16 {
                return Err(OxiError::UnexpectedEof);
            }
            let size64 = u64::from_be_bytes([
                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
            ]);
            (size64, 16)
        } else if size32 == 0 {
            // Box extends to end of file (size unknown)
            (0, 8)
        } else {
            (u64::from(size32), 8)
        };

        Ok(Self {
            size,
            box_type: BoxType(box_type),
            header_size,
        })
    }

    /// Returns the content size (excluding header).
    ///
    /// Returns 0 if the box extends to end of file.
    #[must_use]
    pub const fn content_size(&self) -> u64 {
        if self.size == 0 {
            0 // Unknown - extends to EOF
        } else {
            self.size - self.header_size as u64
        }
    }
}

/// File type box (`ftyp`).
///
/// Identifies the file type and lists compatible brands.
#[derive(Clone, Debug)]
pub struct FtypBox {
    /// Major brand (e.g., "isom", "mp42").
    pub major_brand: BoxType,
    /// Minor version number.
    pub minor_version: u32,
    /// List of compatible brands.
    pub compatible_brands: Vec<BoxType>,
}

impl FtypBox {
    /// Parses the content of an `ftyp` box.
    ///
    /// # Arguments
    ///
    /// * `data` - Box content (after the header)
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed.
    pub fn parse(data: &[u8]) -> OxiResult<Self> {
        if data.len() < 8 {
            return Err(OxiError::Parse {
                offset: 0,
                message: "ftyp box too short".into(),
            });
        }

        let mut major_brand = [0u8; 4];
        major_brand.copy_from_slice(&data[0..4]);
        let minor_version = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

        let mut compatible_brands = Vec::new();
        let mut offset = 8;
        while offset + 4 <= data.len() {
            let mut brand = [0u8; 4];
            brand.copy_from_slice(&data[offset..offset + 4]);
            compatible_brands.push(BoxType(brand));
            offset += 4;
        }

        Ok(Self {
            major_brand: BoxType(major_brand),
            minor_version,
            compatible_brands,
        })
    }

    /// Checks if this file is a valid MP4/ISOBMFF container.
    ///
    /// Returns `true` if the major brand or any compatible brand
    /// is a recognized MP4 brand.
    #[must_use]
    pub fn is_mp4(&self) -> bool {
        let mp4_brands = [
            BoxType::from_str("isom"),
            BoxType::from_str("iso2"),
            BoxType::from_str("iso3"),
            BoxType::from_str("iso4"),
            BoxType::from_str("iso5"),
            BoxType::from_str("iso6"),
            BoxType::from_str("mp41"),
            BoxType::from_str("mp42"),
            BoxType::from_str("M4V "),
            BoxType::from_str("M4A "),
            BoxType::from_str("M4P "),
            BoxType::from_str("av01"), // AV1
            BoxType::from_str("avis"), // AV1 image sequence
        ];
        mp4_brands.contains(&self.major_brand)
            || self
                .compatible_brands
                .iter()
                .any(|b| mp4_brands.contains(b))
    }
}

/// Movie box (`moov`) containing all metadata.
#[derive(Clone, Debug, Default)]
pub struct MoovBox {
    /// Movie header (`mvhd`).
    pub mvhd: Option<MvhdBox>,
    /// Track boxes (`trak`).
    pub traks: Vec<TrakBox>,
    /// `trex` entries from the `mvex` box.
    ///
    /// A non-empty vector means the file is *fragmented*: the sample tables in
    /// `moov` are empty (or absent) and the real sample descriptions live in
    /// `moof` boxes further down the file.
    pub trex: Vec<TrexBox>,
}

impl MoovBox {
    /// Returns `true` when this movie declares an `mvex` box, i.e. the file is
    /// a fragmented MP4 whose media samples are described by `moof` boxes.
    #[must_use]
    pub fn is_fragmented(&self) -> bool {
        !self.trex.is_empty()
    }
}

impl MoovBox {
    /// Parses the content of a `moov` box by iterating over its child boxes.
    ///
    /// # Arguments
    ///
    /// * `data` - Box content (after the header)
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed.
    pub fn parse(data: &[u8]) -> OxiResult<Self> {
        let mut moov = MoovBox::default();
        let mut offset = 0usize;

        while offset + 8 <= data.len() {
            let remaining = &data[offset..];
            let header = BoxHeader::parse(remaining)?;
            let header_size = header.header_size as usize;

            // Determine total box size (with protection against zero-size infinite loops)
            let box_total = if header.size == 0 {
                // Box extends to end of container data
                remaining.len()
            } else {
                header.size as usize
            };

            // Checked box advance: `offset + box_total` wraps when a malformed
            // 64-bit extended box size (near u64::MAX) follows a sibling box,
            // bypassing the `> data.len()` check and panicking the slice below.
            let Some(end_off) = box_end(offset, box_total) else {
                break;
            };
            if box_total < header_size || end_off > data.len() {
                // Malformed or truncated box: stop parsing gracefully
                break;
            }

            let content = &data[offset + header_size..end_off];

            match header.box_type {
                BoxType::MVHD => {
                    moov.mvhd = Some(MvhdBox::parse(content)?);
                }
                BoxType::TRAK => {
                    moov.traks.push(TrakBox::parse(content)?);
                }
                BoxType::MVEX => {
                    moov.trex = parse_mvex(content)?;
                }
                // Skip udta, meta, edts, free, skip, and unknown boxes
                _ => {}
            }

            offset = end_off;
        }

        Ok(moov)
    }
}

/// Movie header box (`mvhd`).
#[derive(Clone, Debug)]
pub struct MvhdBox {
    /// Version (0 for 32-bit times, 1 for 64-bit).
    pub version: u8,
    /// Creation time (seconds since 1904).
    pub creation_time: u64,
    /// Modification time (seconds since 1904).
    pub modification_time: u64,
    /// Time units per second.
    pub timescale: u32,
    /// Duration in timescale units.
    pub duration: u64,
    /// Preferred playback rate (1.0 = normal).
    pub rate: f64,
    /// Preferred volume (1.0 = full).
    pub volume: f64,
    /// Next track ID to use.
    pub next_track_id: u32,
}

impl MvhdBox {
    /// Parses the content of an `mvhd` box.
    ///
    /// # Arguments
    ///
    /// * `data` - Box content (after the header)
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed.
    pub fn parse(data: &[u8]) -> OxiResult<Self> {
        let mut atom = Mp4Atom::new(data);

        let version = atom.read_u8()?;
        atom.skip(3)?; // flags

        let (creation_time, modification_time, timescale, duration) = if version == 1 {
            (
                atom.read_u64()?,
                atom.read_u64()?,
                atom.read_u32()?,
                atom.read_u64()?,
            )
        } else {
            (
                u64::from(atom.read_u32()?),
                u64::from(atom.read_u32()?),
                atom.read_u32()?,
                u64::from(atom.read_u32()?),
            )
        };

        let rate = atom.read_fixed_16_16()?;
        let volume = atom.read_fixed_8_8()?;

        // Skip: reserved (2 bytes) + reserved (2 * 4 bytes)
        atom.skip(2 + 8)?;
        // Skip: matrix (9 * 4 bytes)
        atom.skip(36)?;
        // Skip: pre_defined (6 * 4 bytes)
        atom.skip(24)?;

        let next_track_id = atom.read_u32()?;

        Ok(Self {
            version,
            creation_time,
            modification_time,
            timescale,
            duration,
            rate,
            volume,
            next_track_id,
        })
    }

    /// Returns the duration in seconds.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn duration_seconds(&self) -> f64 {
        if self.timescale == 0 {
            0.0
        } else {
            self.duration as f64 / f64::from(self.timescale)
        }
    }
}

/// Track box (`trak`) containing track metadata.
#[derive(Clone, Debug, Default)]
pub struct TrakBox {
    /// Track header (`tkhd`).
    pub tkhd: Option<TkhdBox>,
    /// Media timescale (from `mdhd`).
    pub timescale: u32,
    /// Handler type ("vide", "soun", "text", etc.).
    pub handler_type: String,
    /// Codec tag from sample description.
    pub codec_tag: u32,
    /// Video width in pixels (if video track).
    pub width: Option<u32>,
    /// Video height in pixels (if video track).
    pub height: Option<u32>,
    /// Audio sample rate in Hz (if audio track).
    pub sample_rate: Option<u32>,
    /// Audio channel count (if audio track).
    pub channels: Option<u16>,
    /// Time-to-sample entries (`stts`).
    pub stts_entries: Vec<SttsEntry>,
    /// Sample-to-chunk entries (`stsc`).
    pub stsc_entries: Vec<StscEntry>,
    /// Individual sample sizes (if variable, from `stsz`).
    pub sample_sizes: Vec<u32>,
    /// Default sample size (if all samples are the same size).
    pub default_sample_size: u32,
    /// Chunk offsets (`stco` or `co64`).
    pub chunk_offsets: Vec<u64>,
    /// Sync sample numbers (`stss`), `None` if all samples are sync.
    pub sync_samples: Option<Vec<u32>>,
    /// Composition time offsets (`ctts`).
    pub ctts_entries: Vec<CttsEntry>,
    /// Codec-specific extradata (e.g., AV1 config).
    pub extradata: Option<Vec<u8>>,
    /// Number of encoder pre-roll (padding) samples to discard at the start
    /// of playback.  Populated from the `elst` (Edit List) box when the first
    /// edit entry has `media_time > 0`.  Zero means "no gapless info".
    pub preroll_samples: u32,
    /// Number of encoder padding samples to discard at the *end* of playback.
    /// Populated from the `elst` box or from an `iTunSMPB` tag when present.
    /// Zero means "no gapless info".
    pub padding_samples: u32,
}

/// Validates that a box-declared entry count can actually fit within the
/// number of bytes still available in the box content.
///
/// MP4 sample-table boxes (`stts`, `stsc`, `stsz`, `stco`, `co64`, `stss`,
/// `ctts`) each carry a 32-bit `entry_count`/`sample_count` field that is
/// fully attacker-controlled: a truncated or crafted box can declare up to
/// `u32::MAX` entries while providing only a handful of payload bytes. Using
/// that count directly in `Vec::with_capacity` would let a ~20-byte file
/// trigger a multi-gigabyte allocation (memory-exhaustion DoS) before any
/// per-entry read has a chance to fail. This check rejects such files with a
/// proper [`OxiError`] *before* any allocation is attempted.
///
/// # Errors
///
/// Returns [`OxiError::Parse`] if `entry_count * entry_size` exceeds
/// `available` (the number of bytes actually remaining in the box).
fn check_entry_count(
    box_name: &str,
    entry_count: usize,
    entry_size: usize,
    available: usize,
    offset: u64,
) -> OxiResult<()> {
    let needed = entry_count.saturating_mul(entry_size);
    if needed > available {
        return Err(OxiError::Parse {
            offset,
            message: format!(
                "{box_name}: declared entry_count {entry_count} needs {needed} bytes but only {available} bytes remain in box"
            ),
        });
    }
    Ok(())
}

impl TrakBox {
    /// Parses the content of a `trak` box by iterating its child boxes.
    ///
    /// Walks the `trak → mdia → minf → stbl` hierarchy to extract:
    /// - Track header (`tkhd`)
    /// - Handler type (`hdlr`)
    /// - Media timescale (`mdhd`)
    /// - Sample description / codec tag (`stsd`)
    /// - Sample tables (`stts`, `stsc`, `stsz`, `stco`, `co64`, `stss`, `ctts`)
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed.
    pub fn parse(data: &[u8]) -> OxiResult<Self> {
        let mut trak = TrakBox::default();
        Self::parse_container(data, &mut trak, 0)?;
        Ok(trak)
    }

    /// Iterates over child boxes in a container box and dispatches to handlers.
    ///
    /// `depth` tracks how deep the `mdia`/`minf`/`stbl`/`edts` recursion has
    /// gone so a maliciously deep nesting cannot overflow the stack.
    fn parse_container(data: &[u8], trak: &mut TrakBox, depth: usize) -> OxiResult<()> {
        // Reject pathologically deep container nesting before recursing further:
        // a crafted chain of nested container boxes would otherwise exhaust the
        // call stack (stack overflow / abort).
        if depth >= MAX_BOX_DEPTH {
            return Err(OxiError::Parse {
                offset: 0,
                message: format!("MP4 box nesting exceeds maximum depth {MAX_BOX_DEPTH}"),
            });
        }
        let mut offset = 0usize;

        while offset + 8 <= data.len() {
            let remaining = &data[offset..];
            let header = BoxHeader::parse(remaining)?;
            let header_size = header.header_size as usize;

            let box_total = if header.size == 0 {
                remaining.len()
            } else {
                header.size as usize
            };

            // Checked box advance guards against a wrapping 64-bit extended box
            // size (see `box_end`); an unchecked `offset + box_total` would
            // bypass the bounds check and panic the slice below.
            let Some(end_off) = box_end(offset, box_total) else {
                break;
            };
            if box_total < header_size || end_off > data.len() {
                break;
            }

            let content = &data[offset + header_size..end_off];

            match header.box_type {
                BoxType::TKHD => {
                    trak.tkhd = Some(TkhdBox::parse(content)?);
                }
                // Container boxes — recurse directly (depth-capped)
                BoxType::MDIA | BoxType::MINF | BoxType::STBL | BoxType::EDTS => {
                    Self::parse_container(content, trak, depth + 1)?;
                }
                BoxType::MDHD => {
                    Self::parse_mdhd(content, trak)?;
                }
                BoxType::HDLR => {
                    Self::parse_hdlr(content, trak)?;
                }
                BoxType::STSD => {
                    Self::parse_stsd(content, trak)?;
                }
                BoxType::STTS => {
                    Self::parse_stts(content, trak)?;
                }
                BoxType::STSC => {
                    Self::parse_stsc(content, trak)?;
                }
                BoxType::STSZ => {
                    Self::parse_stsz(content, trak)?;
                }
                BoxType::STCO => {
                    Self::parse_stco(content, trak)?;
                }
                BoxType::CO64 => {
                    Self::parse_co64(content, trak)?;
                }
                BoxType::STSS => {
                    Self::parse_stss(content, trak)?;
                }
                BoxType::CTTS => {
                    Self::parse_ctts(content, trak)?;
                }
                BoxType::ELST => {
                    Self::parse_elst(content, trak)?;
                }
                _ => {}
            }

            offset = end_off;
        }

        Ok(())
    }

    /// Parses an `elst` (Edit List) box to extract gapless-audio pre-roll and
    /// padding sample counts.
    ///
    /// # Format
    ///
    /// ```text
    /// version (1 byte) | flags (3 bytes) | entry_count (u32)
    /// For each entry:
    ///   version 0: segment_duration (u32) | media_time (i32) | media_rate (u32)
    ///   version 1: segment_duration (u64) | media_time (i64) | media_rate (u32)
    /// ```
    ///
    /// An entry with `media_time == -1` is an *empty edit* representing
    /// pre-roll silence; the entry that follows gives the actual start offset
    /// (`media_time` is the first sample PTS to play).  When such a pattern is
    /// detected we populate `preroll_samples` from the second entry's
    /// `media_time`.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed.
    fn parse_elst(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        let version = atom.read_u8()?;
        atom.skip(3)?; // flags

        let entry_count = atom.read_u32()? as usize;
        if entry_count == 0 {
            return Ok(());
        }

        // Collect all entries first, then interpret them.
        #[derive(Debug)]
        struct ElstEntry {
            /// Total duration of this edit in movie-timescale units.
            /// Retained for completeness; used by higher-level gapless-padding
            /// calculations (e.g. via `iTunSMPB`).
            #[allow(dead_code)]
            segment_duration: u64,
            media_time: i64,
        }

        let mut entries: Vec<ElstEntry> = Vec::with_capacity(entry_count.min(64));
        for _ in 0..entry_count {
            let (seg_dur, med_time) = if version == 1 {
                let sd = atom.read_u64()?;
                let mt = atom.read_i64()?;
                atom.skip(4)?; // media_rate (16.16 fixed-point)
                (sd, mt)
            } else {
                let sd = u64::from(atom.read_u32()?);
                let mt = i64::from(atom.read_i32()?);
                atom.skip(4)?; // media_rate (16.16 fixed-point)
                (sd, mt)
            };
            entries.push(ElstEntry {
                segment_duration: seg_dur,
                media_time: med_time,
            });
        }

        // Interpret the edit list for gapless audio info:
        //
        // Pattern A (empty-edit prefix):
        //   Entry[0]: media_time == -1  (empty edit, silence before media)
        //   Entry[1]: media_time >= 0   (actual start offset = pre-roll)
        //
        // Pattern B (single entry, media_time > 0):
        //   Entry[0]: media_time > 0    (pre-roll = media_time)
        //
        // In both cases the media_time of the first non-empty entry gives the
        // number of encoder pre-roll samples to discard.
        let first_media_entry = if entries.first().map_or(false, |e| e.media_time == -1) {
            // Skip the empty-edit prefix.
            entries.get(1)
        } else {
            entries.first()
        };

        if let Some(entry) = first_media_entry {
            if entry.media_time > 0 {
                // media_time is in the track's media timescale; treat it
                // directly as a sample count (valid when timescale == sample
                // rate, which is the common case for gapless AAC/Opus/FLAC).
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    trak.preroll_samples = entry.media_time as u32;
                }
            }
        }

        // Padding: the last entry's segment_duration relative to the total
        // track duration can encode trailing padding.  We leave padding_samples
        // at 0 here and let the iTunSMPB path (or higher-level decoder) fill
        // it in; the field is available for callers who wish to populate it.
        let _ = trak.padding_samples; // suppress unused warning

        Ok(())
    }

    /// Parses a `mdhd` box to extract media timescale.
    fn parse_mdhd(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        let version = atom.read_u8()?;
        atom.skip(3)?; // flags

        if version == 1 {
            atom.skip(8)?; // creation_time (u64)
            atom.skip(8)?; // modification_time (u64)
            trak.timescale = atom.read_u32()?;
        } else {
            atom.skip(4)?; // creation_time (u32)
            atom.skip(4)?; // modification_time (u32)
            trak.timescale = atom.read_u32()?;
        }

        Ok(())
    }

    /// Parses a `hdlr` box to extract the handler type string.
    fn parse_hdlr(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        atom.skip(4)?; // pre_defined
        let handler_type = atom.read_type()?;
        trak.handler_type = handler_type.trim_end_matches('\0').to_string();
        Ok(())
    }

    /// Parses a `stsd` (sample description) box to extract the codec tag and codec-specific params.
    ///
    /// STSD layout:
    ///   1-byte version, 3-byte flags, 4-byte entry count
    ///   For each entry: 4-byte size, 4-byte codec-tag, 6-byte reserved, 2-byte data-ref-index,
    ///                   then codec-specific fields.
    fn parse_stsd(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(1)?; // version
        atom.skip(3)?; // flags
        let entry_count = atom.read_u32()?;
        if entry_count == 0 {
            return Ok(());
        }

        // Parse only the first entry
        if atom.remaining().len() < 8 {
            return Ok(());
        }

        let entry_size = atom.read_u32()? as usize;
        if entry_size < 8 {
            return Ok(());
        }

        let codec_tag = atom.read_u32()?;
        trak.codec_tag = codec_tag;

        // Minimum entry body = 6 (reserved) + 2 (data-ref-idx) = 8 bytes after codec tag
        // Total consumed so far from entry: 4 (size) + 4 (codec_tag) = 8 bytes of header
        let entry_body_size = entry_size.saturating_sub(8); // remaining bytes in entry after header
        if atom.remaining().len() < entry_body_size {
            return Ok(());
        }
        let entry_body = atom.read_bytes(entry_body_size)?;

        // Dispatch based on handler type
        match trak.handler_type.as_str() {
            "vide" => Self::parse_visual_sample_entry(entry_body, trak)?,
            "soun" => Self::parse_audio_sample_entry(entry_body, trak)?,
            _ => {}
        }

        Ok(())
    }

    /// Parses the codec-specific portion of a visual sample entry.
    ///
    /// Layout (after codec-tag):
    ///   6 bytes reserved, 2 bytes data_ref_index,
    ///   2 bytes pre_defined, 2 bytes reserved, 12 bytes pre_defined,
    ///   2 bytes width (pixels), 2 bytes height (pixels),
    ///   4 bytes horizresolution, 4 bytes vertresolution,
    ///   4 bytes reserved, 2 bytes frame_count,
    ///   32 bytes compressorname,
    ///   2 bytes depth, 2 bytes pre_defined (-1),
    ///   then optional child boxes (e.g., av1C, vpcC)
    fn parse_visual_sample_entry(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);

        // 6 bytes reserved + 2 bytes data_ref_index
        if atom.remaining().len() < 70 {
            return Ok(());
        }
        atom.skip(6)?; // reserved
        atom.skip(2)?; // data_reference_index
        atom.skip(2)?; // pre_defined
        atom.skip(2)?; // reserved
        atom.skip(12)?; // pre_defined (3 * u32)
        let width = u32::from(atom.read_u16()?);
        let height = u32::from(atom.read_u16()?);
        atom.skip(4)?; // horizresolution
        atom.skip(4)?; // vertresolution
        atom.skip(4)?; // reserved
        atom.skip(2)?; // frame_count
        atom.skip(32)?; // compressorname
        atom.skip(2)?; // depth
        atom.skip(2)?; // pre_defined

        trak.width = Some(width);
        trak.height = Some(height);

        // Parse child boxes for extradata (av1C, vpcC, etc.)
        let extra_data = atom.remaining();
        if extra_data.len() >= 8 {
            Self::parse_codec_config(extra_data, trak)?;
        }

        Ok(())
    }

    /// Parses the codec-specific portion of an audio sample entry.
    ///
    /// Layout (after codec-tag):
    ///   6 bytes reserved, 2 bytes data_ref_index,
    ///   8 bytes reserved,
    ///   2 bytes channelcount, 2 bytes samplesize,
    ///   2 bytes pre_defined, 2 bytes reserved,
    ///   4 bytes samplerate (16.16 fixed-point),
    ///   then optional child boxes (e.g., dOps for Opus)
    fn parse_audio_sample_entry(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);

        if atom.remaining().len() < 20 {
            return Ok(());
        }

        atom.skip(6)?; // reserved
        atom.skip(2)?; // data_reference_index
        atom.skip(8)?; // reserved
        let channel_count = atom.read_u16()?;
        atom.skip(2)?; // samplesize
        atom.skip(2)?; // pre_defined
        atom.skip(2)?; // reserved
                       // samplerate is stored as 16.16 fixed-point; integer part is in top 2 bytes
        let sample_rate_fp = atom.read_u32()?;
        let sample_rate = sample_rate_fp >> 16;

        trak.channels = Some(channel_count);
        trak.sample_rate = Some(sample_rate);

        // Parse child boxes for extradata (dOps for Opus, dfLa for FLAC, etc.)
        let extra_data = atom.remaining();
        if extra_data.len() >= 8 {
            Self::parse_codec_config(extra_data, trak)?;
        }

        Ok(())
    }

    /// Scans child boxes within a codec-specific config area to extract extradata.
    ///
    /// Looks for known config boxes: `av1C`, `vpcC`, `dOps`, `dfLa`.
    fn parse_codec_config(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut offset = 0usize;

        while offset + 8 <= data.len() {
            let remaining = &data[offset..];
            let header = BoxHeader::parse(remaining).unwrap_or(BoxHeader {
                size: 0,
                box_type: BoxType::FREE,
                header_size: 8,
            });
            let header_size = header.header_size as usize;
            let box_total = if header.size == 0 {
                remaining.len()
            } else {
                header.size as usize
            };
            // Checked box advance guards against a wrapping 64-bit extended box
            // size (see `box_end`).
            let Some(end_off) = box_end(offset, box_total) else {
                break;
            };
            if box_total < header_size || end_off > data.len() {
                break;
            }

            let content = &data[offset + header_size..end_off];
            let tag = header.box_type.as_u32();

            // AV1 config: "av1C"
            // VP9 config: "vpcC"
            // Opus config: "dOps"
            // FLAC config: "dfLa"
            if matches!(
                tag,
                0x6176_3143 | // av1C
                0x7670_6343 | // vpcC
                0x644F_7073 | // dOps
                0x6466_4C61 // dfLa
            ) {
                trak.extradata = Some(content.to_vec());
                break;
            }

            offset = end_off;
        }

        Ok(())
    }

    /// Parses a `stts` (time-to-sample) box.
    fn parse_stts(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        let entry_count = atom.read_u32()? as usize;
        check_entry_count(
            "stts",
            entry_count,
            8, // sample_count (4) + sample_delta (4)
            atom.remaining().len(),
            atom.position() as u64,
        )?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let sample_count = atom.read_u32()?;
            let sample_delta = atom.read_u32()?;
            entries.push(SttsEntry {
                sample_count,
                sample_delta,
            });
        }
        trak.stts_entries = entries;
        Ok(())
    }

    /// Parses a `stsc` (sample-to-chunk) box.
    fn parse_stsc(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        let entry_count = atom.read_u32()? as usize;
        check_entry_count(
            "stsc",
            entry_count,
            12, // first_chunk (4) + samples_per_chunk (4) + sample_description_index (4)
            atom.remaining().len(),
            atom.position() as u64,
        )?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let first_chunk = atom.read_u32()?;
            let samples_per_chunk = atom.read_u32()?;
            let sample_description_index = atom.read_u32()?;
            entries.push(StscEntry {
                first_chunk,
                samples_per_chunk,
                sample_description_index,
            });
        }
        trak.stsc_entries = entries;
        Ok(())
    }

    /// Parses a `stsz` (sample size) box.
    fn parse_stsz(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        let sample_size = atom.read_u32()?;
        let sample_count = atom.read_u32()? as usize;

        if sample_size > 0 {
            // All samples have the same size
            trak.default_sample_size = sample_size;
        } else {
            // Per-sample sizes
            check_entry_count(
                "stsz",
                sample_count,
                4, // per-entry sample_size (4)
                atom.remaining().len(),
                atom.position() as u64,
            )?;
            let mut sizes = Vec::with_capacity(sample_count);
            for _ in 0..sample_count {
                sizes.push(atom.read_u32()?);
            }
            trak.sample_sizes = sizes;
        }

        Ok(())
    }

    /// Parses a `stco` (32-bit chunk offset) box.
    fn parse_stco(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        let entry_count = atom.read_u32()? as usize;
        check_entry_count(
            "stco",
            entry_count,
            4, // chunk_offset (4)
            atom.remaining().len(),
            atom.position() as u64,
        )?;
        let mut offsets = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            offsets.push(u64::from(atom.read_u32()?));
        }
        trak.chunk_offsets = offsets;
        Ok(())
    }

    /// Parses a `co64` (64-bit chunk offset) box.
    fn parse_co64(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        let entry_count = atom.read_u32()? as usize;
        check_entry_count(
            "co64",
            entry_count,
            8, // chunk_offset (8)
            atom.remaining().len(),
            atom.position() as u64,
        )?;
        let mut offsets = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            offsets.push(atom.read_u64()?);
        }
        trak.chunk_offsets = offsets;
        Ok(())
    }

    /// Parses a `stss` (sync sample) box.
    fn parse_stss(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        atom.skip(4)?; // version + flags
        let entry_count = atom.read_u32()? as usize;
        check_entry_count(
            "stss",
            entry_count,
            4, // sample_number (4)
            atom.remaining().len(),
            atom.position() as u64,
        )?;
        let mut samples = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            samples.push(atom.read_u32()?);
        }
        trak.sync_samples = Some(samples);
        Ok(())
    }

    /// Parses a `ctts` (composition time offset) box.
    fn parse_ctts(data: &[u8], trak: &mut TrakBox) -> OxiResult<()> {
        let mut atom = Mp4Atom::new(data);
        let version = atom.read_u8()?;
        atom.skip(3)?; // flags
        let entry_count = atom.read_u32()? as usize;
        check_entry_count(
            "ctts",
            entry_count,
            8, // sample_count (4) + sample_offset (4)
            atom.remaining().len(),
            atom.position() as u64,
        )?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let sample_count = atom.read_u32()?;
            // In version 1, the offset can be negative (signed)
            let sample_offset = if version == 0 {
                #[allow(clippy::cast_possible_wrap)]
                let v = atom.read_u32()? as i32;
                v
            } else {
                atom.read_i32()?
            };
            entries.push(CttsEntry {
                sample_count,
                sample_offset,
            });
        }
        trak.ctts_entries = entries;
        Ok(())
    }
}

/// Track header box (`tkhd`).
#[derive(Clone, Debug, PartialEq)]
pub struct TkhdBox {
    /// Box version (0 = 32-bit times, 1 = 64-bit times).
    pub version: u8,
    /// 24-bit `tkhd` flags (`track_enabled`, `track_in_movie`, …).
    pub flags: u32,
    /// Track ID (1-based).
    pub track_id: u32,
    /// Duration in movie timescale units.
    pub duration: u64,
    /// Visual layer ordering (lower = closer to the viewer).
    pub layer: i16,
    /// Alternate group identifier (0 = not in a group).
    pub alternate_group: u16,
    /// Audio volume (8.8 fixed-point in the file; 1.0 = full).
    pub volume: f64,
    /// Raw 3x3 display transformation matrix exactly as stored in the file.
    ///
    /// Row-major `[a, b, u, c, d, v, x, y, w]`. `a`, `b`, `c`, `d`, `x` and `y`
    /// are 16.16 fixed-point; `u`, `v` and `w` are 2.30 fixed-point.
    pub matrix: [i32; 9],
    /// Display width (16.16 fixed-point).
    pub width: f64,
    /// Display height (16.16 fixed-point).
    pub height: f64,
}

impl TkhdBox {
    /// The ISOBMFF unity matrix (no rotation, no scale, no translation).
    pub const UNITY_MATRIX: [i32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

    /// Parses the content of a `tkhd` box.
    ///
    /// # Arguments
    ///
    /// * `data` - Box content (after the header)
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed.
    pub fn parse(data: &[u8]) -> OxiResult<Self> {
        let mut atom = Mp4Atom::new(data);

        let version = atom.read_u8()?;
        let flags = u32::from(atom.read_u8()?) << 16
            | u32::from(atom.read_u8()?) << 8
            | u32::from(atom.read_u8()?);

        let (creation_time, modification_time, track_id, duration) = if version == 1 {
            let ct = atom.read_u64()?;
            let mt = atom.read_u64()?;
            let tid = atom.read_u32()?;
            atom.skip(4)?; // reserved
            let dur = atom.read_u64()?;
            (ct, mt, tid, dur)
        } else {
            let ct = u64::from(atom.read_u32()?);
            let mt = u64::from(atom.read_u32()?);
            let tid = atom.read_u32()?;
            atom.skip(4)?; // reserved
            let dur = u64::from(atom.read_u32()?);
            (ct, mt, tid, dur)
        };

        // Silence unused variable warnings
        let _ = (creation_time, modification_time);

        // reserved (2 * 4 bytes)
        atom.skip(8)?;
        #[allow(clippy::cast_possible_wrap)]
        let layer = atom.read_u16()? as i16;
        let alternate_group = atom.read_u16()?;
        let volume = atom.read_fixed_8_8()?;
        atom.skip(2)?; // reserved

        // matrix (9 * 4 bytes) — parsed, not skipped: phone/camera captures
        // encode portrait orientation here and downstream reframing needs it.
        let mut matrix = [0i32; 9];
        for slot in &mut matrix {
            *slot = atom.read_i32()?;
        }

        let width = atom.read_fixed_16_16()?;
        let height = atom.read_fixed_16_16()?;

        Ok(Self {
            version,
            flags,
            track_id,
            duration,
            layer,
            alternate_group,
            volume,
            matrix,
            width,
            height,
        })
    }

    /// Converts the raw fixed-point matrix into a floating-point
    /// [`TransformMatrix`].
    ///
    /// `a`, `b`, `c`, `d`, `x` and `y` are decoded as 16.16 fixed-point and
    /// `u`, `v`, `w` as 2.30 fixed-point, per ISO/IEC 14496-12 §6.2.2.
    #[must_use]
    pub fn transform_matrix(&self) -> TransformMatrix {
        const FP_16_16: f64 = 65536.0;
        const FP_2_30: f64 = 1_073_741_824.0;
        let scale = |index: usize| -> f64 {
            let divisor = if matches!(index, 2 | 5 | 8) {
                FP_2_30
            } else {
                FP_16_16
            };
            f64::from(self.matrix[index]) / divisor
        };
        let mut values = [0.0f64; 9];
        for (index, slot) in values.iter_mut().enumerate() {
            *slot = scale(index);
        }
        TransformMatrix::new(values)
    }

    /// Returns the display rotation encoded by the transformation matrix,
    /// snapped to the nearest quarter turn (`0`, `90`, `180` or `270` degrees).
    ///
    /// Returns `0` for the unity matrix and for any degenerate matrix whose
    /// first row is all zeros.
    #[must_use]
    pub fn rotation_degrees(&self) -> u16 {
        const FP_16_16: f64 = 65536.0;
        let a = f64::from(self.matrix[0]) / FP_16_16;
        let b = f64::from(self.matrix[1]) / FP_16_16;
        if a.abs() < 1e-6 && b.abs() < 1e-6 {
            return 0;
        }
        let degrees = b.atan2(a).to_degrees().rem_euclid(360.0);
        if !(45.0..315.0).contains(&degrees) {
            0
        } else if degrees < 135.0 {
            90
        } else if degrees < 225.0 {
            180
        } else {
            270
        }
    }

    /// Returns `true` when the matrix is the ISOBMFF unity matrix.
    #[must_use]
    pub fn has_unity_matrix(&self) -> bool {
        self.matrix == Self::UNITY_MATRIX
    }
}

impl Default for TkhdBox {
    fn default() -> Self {
        Self {
            version: 0,
            flags: 0x0000_0003,
            track_id: 0,
            duration: 0,
            layer: 0,
            alternate_group: 0,
            volume: 0.0,
            matrix: Self::UNITY_MATRIX,
            width: 0.0,
            height: 0.0,
        }
    }
}

/// Time-to-sample entry (from `stts` box).
#[derive(Clone, Debug)]
pub struct SttsEntry {
    /// Number of consecutive samples with this duration.
    pub sample_count: u32,
    /// Duration of each sample in timescale units.
    pub sample_delta: u32,
}

/// Sample-to-chunk entry (from `stsc` box).
#[derive(Clone, Debug)]
pub struct StscEntry {
    /// First chunk number using this entry (1-based).
    pub first_chunk: u32,
    /// Number of samples in each chunk.
    pub samples_per_chunk: u32,
    /// Sample description index (1-based).
    pub sample_description_index: u32,
}

/// Composition time offset entry (from `ctts` box).
#[derive(Clone, Debug)]
pub struct CttsEntry {
    /// Number of consecutive samples with this offset.
    pub sample_count: u32,
    /// Composition time offset (can be negative in version 1).
    pub sample_offset: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security regression (P0 #4): a sibling box followed by a box whose
    /// 64-bit extended size is near u64::MAX makes `offset + box_total` wrap,
    /// bypassing the bounds check and panicking the slice. Must stop gracefully.
    #[test]
    fn moov_parse_wrapping_sibling_box_no_panic() {
        let mut data = Vec::new();
        // A small valid 'free' box (size 8, empty body).
        data.extend_from_slice(&8u32.to_be_bytes());
        data.extend_from_slice(b"free");
        // An extended-size box whose size wraps when added to the current offset.
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(b"trak");
        data.extend_from_slice(&u64::MAX.to_be_bytes());
        // Must not panic (graceful break); result may be Ok with no traks.
        let _ = MoovBox::parse(&data);
    }

    /// Security regression (P1 #6): deeply nested container boxes must be
    /// rejected before the recursion overflows the stack.
    #[test]
    fn trak_parse_deeply_nested_containers_errors() {
        fn nested_mdia(depth: usize) -> Vec<u8> {
            let mut inner: Vec<u8> = Vec::new();
            for _ in 0..depth {
                let mut boxed = Vec::new();
                let size = (inner.len() + 8) as u32;
                boxed.extend_from_slice(&size.to_be_bytes());
                boxed.extend_from_slice(b"mdia");
                boxed.extend_from_slice(&inner);
                inner = boxed;
            }
            inner
        }
        // 40 levels exceeds MAX_BOX_DEPTH (32) → clean Err, no stack overflow.
        let data = nested_mdia(40);
        assert!(TrakBox::parse(&data).is_err());
    }

    #[test]
    fn test_box_header_normal() {
        let data = [0x00, 0x00, 0x00, 0x14, b'f', b't', b'y', b'p'];
        let header = BoxHeader::parse(&data).expect("operation should succeed");
        assert_eq!(header.size, 20);
        assert_eq!(header.box_type, BoxType::FTYP);
        assert_eq!(header.header_size, 8);
        assert_eq!(header.content_size(), 12);
    }

    #[test]
    fn test_box_header_extended() {
        let data = [
            0x00, 0x00, 0x00, 0x01, // size = 1 (extended)
            b'm', b'd', b'a', b't', 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            0x00, // extended size = 256
        ];
        let header = BoxHeader::parse(&data).expect("operation should succeed");
        assert_eq!(header.size, 256);
        assert_eq!(header.box_type, BoxType::MDAT);
        assert_eq!(header.header_size, 16);
        assert_eq!(header.content_size(), 240);
    }

    #[test]
    fn test_box_header_to_eof() {
        let data = [0x00, 0x00, 0x00, 0x00, b'm', b'd', b'a', b't'];
        let header = BoxHeader::parse(&data).expect("operation should succeed");
        assert_eq!(header.size, 0);
        assert_eq!(header.content_size(), 0);
    }

    #[test]
    fn test_box_type_constants() {
        assert_eq!(BoxType::FTYP.as_str(), "ftyp");
        assert_eq!(BoxType::MOOV.as_str(), "moov");
        assert_eq!(BoxType::MDAT.as_str(), "mdat");
    }

    #[test]
    fn test_box_type_from_str() {
        assert_eq!(BoxType::from_str("moov"), BoxType::MOOV);
        assert_eq!(BoxType::from_str("ftyp"), BoxType::FTYP);
    }

    #[test]
    fn test_box_type_display() {
        assert_eq!(format!("{}", BoxType::FTYP), "ftyp");
    }

    #[test]
    fn test_ftyp_parse() {
        // "isom" + version 0 + compatible brand "mp41"
        let data = [
            b'i', b's', b'o', b'm', // major brand
            0x00, 0x00, 0x00, 0x00, // minor version
            b'm', b'p', b'4', b'1', // compatible brand
        ];
        let ftyp = FtypBox::parse(&data).expect("operation should succeed");
        assert_eq!(ftyp.major_brand.as_str(), "isom");
        assert_eq!(ftyp.minor_version, 0);
        assert_eq!(ftyp.compatible_brands.len(), 1);
        assert_eq!(ftyp.compatible_brands[0].as_str(), "mp41");
        assert!(ftyp.is_mp4());
    }

    #[test]
    fn test_ftyp_is_mp4() {
        let ftyp = FtypBox {
            major_brand: BoxType::from_str("av01"),
            minor_version: 0,
            compatible_brands: vec![],
        };
        assert!(ftyp.is_mp4());
    }

    #[test]
    fn test_mvhd_parse_v0() {
        #[rustfmt::skip]
        let data = [
            0x00, // version
            0x00, 0x00, 0x00, // flags
            0x00, 0x00, 0x00, 0x01, // creation_time
            0x00, 0x00, 0x00, 0x02, // modification_time
            0x00, 0x00, 0x03, 0xE8, // timescale = 1000
            0x00, 0x00, 0x27, 0x10, // duration = 10000 (10 seconds)
            0x00, 0x01, 0x00, 0x00, // rate = 1.0
            0x01, 0x00, // volume = 1.0
            0x00, 0x00, // reserved
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // reserved
            // matrix (36 bytes)
            0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
            // pre_defined (24 bytes)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x02, // next_track_id = 2
        ];
        let mvhd = MvhdBox::parse(&data).expect("operation should succeed");
        assert_eq!(mvhd.version, 0);
        assert_eq!(mvhd.timescale, 1000);
        assert_eq!(mvhd.duration, 10000);
        assert!((mvhd.rate - 1.0).abs() < f64::EPSILON);
        assert!((mvhd.volume - 1.0).abs() < f64::EPSILON);
        assert_eq!(mvhd.next_track_id, 2);
        assert!((mvhd.duration_seconds() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tkhd_parse_v0() {
        #[rustfmt::skip]
        let data = [
            0x00, // version
            0x00, 0x00, 0x03, // flags (enabled, in_movie, in_preview)
            0x00, 0x00, 0x00, 0x01, // creation_time
            0x00, 0x00, 0x00, 0x02, // modification_time
            0x00, 0x00, 0x00, 0x01, // track_id = 1
            0x00, 0x00, 0x00, 0x00, // reserved
            0x00, 0x00, 0x27, 0x10, // duration = 10000
            // reserved (8 bytes)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // layer, alternate_group
            0x00, 0x00, 0x00, 0x00,
            // volume, reserved
            0x01, 0x00, 0x00, 0x00,
            // matrix (36 bytes)
            0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
            // width = 1920 (in 16.16 fixed-point)
            0x07, 0x80, 0x00, 0x00,
            // height = 1080 (in 16.16 fixed-point)
            0x04, 0x38, 0x00, 0x00,
        ];
        let tkhd = TkhdBox::parse(&data).expect("operation should succeed");
        assert_eq!(tkhd.track_id, 1);
        assert_eq!(tkhd.duration, 10000);
        assert!((tkhd.width - 1920.0).abs() < 1.0);
        assert!((tkhd.height - 1080.0).abs() < 1.0);
    }

    #[test]
    fn test_box_type_as_u32() {
        assert_eq!(BoxType::FTYP.as_u32(), 0x66747970); // "ftyp"
    }

    // ------------------------------------------------------------------
    // tkhd display matrix / rotation.
    //
    // `TkhdBox::parse` used to `skip(36)` straight over the 3x3 transform
    // matrix, so every phone/camera capture that records portrait footage as
    // landscape pixels plus a 90/270-degree rotation was silently demuxed as
    // landscape. These build a `tkhd` payload by hand for each quarter turn.
    // ------------------------------------------------------------------

    /// 16.16 fixed-point one.
    const FP_ONE: i32 = 0x0001_0000;
    /// 2.30 fixed-point one (the `w` element).
    const FP_W_ONE: i32 = 0x4000_0000;

    /// Builds a version-0 `tkhd` payload carrying `matrix`.
    fn tkhd_v0_payload(matrix: [i32; 9], width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::new();
        data.push(0x00); // version
        data.extend_from_slice(&[0x00, 0x00, 0x03]); // flags = enabled | in_movie
        data.extend_from_slice(&1u32.to_be_bytes()); // creation_time
        data.extend_from_slice(&2u32.to_be_bytes()); // modification_time
        data.extend_from_slice(&7u32.to_be_bytes()); // track_id
        data.extend_from_slice(&0u32.to_be_bytes()); // reserved
        data.extend_from_slice(&10_000u32.to_be_bytes()); // duration
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&(-1i16).to_be_bytes()); // layer
        data.extend_from_slice(&3u16.to_be_bytes()); // alternate_group
        data.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
        data.extend_from_slice(&[0u8; 2]); // reserved
        for value in matrix {
            data.extend_from_slice(&value.to_be_bytes());
        }
        data.extend_from_slice(&(width << 16).to_be_bytes());
        data.extend_from_slice(&(height << 16).to_be_bytes());
        data
    }

    /// The four quarter-turn matrices a camera actually writes, translation
    /// included (rotating about the origin has to be undone by a shift).
    fn rotation_matrix(degrees: u16, width: u32, height: u32) -> [i32; 9] {
        #[allow(clippy::cast_possible_wrap)]
        let (w, h) = ((width as i32) << 16, (height as i32) << 16);
        match degrees {
            90 => [0, FP_ONE, 0, -FP_ONE, 0, 0, h, 0, FP_W_ONE],
            180 => [-FP_ONE, 0, 0, 0, -FP_ONE, 0, w, h, FP_W_ONE],
            270 => [0, -FP_ONE, 0, FP_ONE, 0, 0, 0, w, FP_W_ONE],
            _ => TkhdBox::UNITY_MATRIX,
        }
    }

    #[test]
    fn tkhd_parses_rotation_zero() {
        let data = tkhd_v0_payload(rotation_matrix(0, 1920, 1080), 1920, 1080);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.rotation_degrees(), 0);
        assert!(tkhd.has_unity_matrix());
        assert!(tkhd.transform_matrix().is_identity());
    }

    #[test]
    fn tkhd_parses_rotation_ninety() {
        let data = tkhd_v0_payload(rotation_matrix(90, 1920, 1080), 1920, 1080);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.rotation_degrees(), 90);
        assert!(!tkhd.has_unity_matrix());

        let matrix = tkhd.transform_matrix();
        assert!((matrix.values[0] - 0.0).abs() < 1e-9, "a");
        assert!((matrix.values[1] - 1.0).abs() < 1e-9, "b");
        assert!((matrix.values[3] + 1.0).abs() < 1e-9, "c");
        assert!((matrix.values[4] - 0.0).abs() < 1e-9, "d");
        assert!((matrix.values[8] - 1.0).abs() < 1e-9, "w decodes as 2.30");
        let (tx, ty) = matrix.translation_xy();
        assert!((tx - 1080.0).abs() < 1e-6, "tx decodes as 16.16");
        assert!((ty - 0.0).abs() < 1e-9);
    }

    #[test]
    fn tkhd_parses_rotation_one_eighty() {
        let data = tkhd_v0_payload(rotation_matrix(180, 1920, 1080), 1920, 1080);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.rotation_degrees(), 180);
    }

    #[test]
    fn tkhd_parses_rotation_two_seventy() {
        let data = tkhd_v0_payload(rotation_matrix(270, 1920, 1080), 1920, 1080);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.rotation_degrees(), 270);
    }

    #[test]
    fn tkhd_rotation_snaps_slightly_off_matrices() {
        // A real-world matrix is rarely exactly 0x00010000; snap to the nearest
        // quarter turn rather than reporting an odd angle.
        let mut matrix = rotation_matrix(90, 1080, 1920);
        matrix[0] = 200; // a tiny non-zero a
        matrix[1] = FP_ONE - 40;
        let data = tkhd_v0_payload(matrix, 1080, 1920);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.rotation_degrees(), 90);
    }

    #[test]
    fn tkhd_degenerate_matrix_reports_zero_rotation() {
        let mut matrix = TkhdBox::UNITY_MATRIX;
        matrix[0] = 0;
        matrix[1] = 0;
        let data = tkhd_v0_payload(matrix, 640, 480);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.rotation_degrees(), 0);
    }

    #[test]
    fn tkhd_parses_header_fields_alongside_the_matrix() {
        let data = tkhd_v0_payload(rotation_matrix(270, 1920, 1080), 1920, 1080);
        let tkhd = TkhdBox::parse(&data).expect("tkhd parses");
        assert_eq!(tkhd.version, 0);
        assert_eq!(tkhd.flags, 0x0000_0003);
        assert_eq!(tkhd.track_id, 7);
        assert_eq!(tkhd.duration, 10_000);
        assert_eq!(tkhd.layer, -1);
        assert_eq!(tkhd.alternate_group, 3);
        assert!((tkhd.volume - 1.0).abs() < f64::EPSILON);
        assert!((tkhd.width - 1920.0).abs() < 1.0);
        assert!((tkhd.height - 1080.0).abs() < 1.0);
    }

    #[test]
    fn tkhd_v1_parses_the_matrix_too() {
        let mut data = Vec::new();
        data.push(0x01); // version = 1
        data.extend_from_slice(&[0x00, 0x00, 0x03]); // flags
        data.extend_from_slice(&0u64.to_be_bytes()); // creation_time
        data.extend_from_slice(&0u64.to_be_bytes()); // modification_time
        data.extend_from_slice(&2u32.to_be_bytes()); // track_id
        data.extend_from_slice(&0u32.to_be_bytes()); // reserved
        data.extend_from_slice(&123_456u64.to_be_bytes()); // duration
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&0i16.to_be_bytes()); // layer
        data.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        data.extend_from_slice(&0u16.to_be_bytes()); // volume
        data.extend_from_slice(&[0u8; 2]); // reserved
        for value in rotation_matrix(180, 3840, 2160) {
            data.extend_from_slice(&value.to_be_bytes());
        }
        data.extend_from_slice(&(3840u32 << 16).to_be_bytes());
        data.extend_from_slice(&(2160u32 << 16).to_be_bytes());

        let tkhd = TkhdBox::parse(&data).expect("v1 tkhd parses");
        assert_eq!(tkhd.version, 1);
        assert_eq!(tkhd.track_id, 2);
        assert_eq!(tkhd.duration, 123_456);
        assert_eq!(tkhd.rotation_degrees(), 180);
    }

    // ------------------------------------------------------------------
    // mvex / trex
    // ------------------------------------------------------------------

    #[test]
    fn moov_parse_detects_mvex() {
        fn plain_box(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let total = (payload.len() + 8) as u32;
            let mut out = Vec::new();
            out.extend_from_slice(&total.to_be_bytes());
            out.extend_from_slice(tag);
            out.extend_from_slice(payload);
            out
        }

        let mut trex_payload = Vec::new();
        trex_payload.extend_from_slice(&[0u8; 4]); // version + flags
        trex_payload.extend_from_slice(&1u32.to_be_bytes()); // track_id
        trex_payload.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
        trex_payload.extend_from_slice(&1024u32.to_be_bytes()); // default duration
        trex_payload.extend_from_slice(&512u32.to_be_bytes()); // default size
        trex_payload.extend_from_slice(&0u32.to_be_bytes()); // default flags
        let mvex = plain_box(b"mvex", &plain_box(b"trex", &trex_payload));

        let moov = MoovBox::parse(&mvex).expect("moov parses");
        assert!(moov.is_fragmented());
        assert_eq!(moov.trex.len(), 1);
        assert_eq!(moov.trex[0].default_sample_duration, 1024);
        assert_eq!(moov.trex[0].default_sample_size, 512);
    }

    #[test]
    fn moov_without_mvex_is_not_fragmented() {
        let moov = MoovBox::parse(&[]).expect("empty moov parses");
        assert!(!moov.is_fragmented());
    }

    // ------------------------------------------------------------------
    // Regression tests: BLOCKER FIX — an unvalidated 32-bit entry_count /
    // sample_count field must never drive an unbounded `Vec::with_capacity`
    // allocation. Each crafted box below declares `entry_count = u32::MAX`
    // while providing only a handful of trailing payload bytes; before the
    // fix this would attempt a multi-gigabyte allocation (memory-exhaustion
    // DoS) from a ~20-byte file. The parser must now return `Err` instead of
    // panicking or attempting the allocation.
    // ------------------------------------------------------------------

    #[test]
    fn test_parse_stts_valid_small_box() {
        // version(1)+flags(3) + entry_count(4)=1 + one entry (count=10, delta=1000)
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0x00, 0x00, 0x00, 0x01, // entry_count = 1
            0x00, 0x00, 0x00, 0x0A, // sample_count = 10
            0x00, 0x00, 0x03, 0xE8, // sample_delta = 1000
        ];
        let mut trak = TrakBox::default();
        TrakBox::parse_stts(&data, &mut trak).expect("valid stts box should parse");
        assert_eq!(trak.stts_entries.len(), 1);
        assert_eq!(trak.stts_entries[0].sample_count, 10);
        assert_eq!(trak.stts_entries[0].sample_delta, 1000);
    }

    #[test]
    fn test_parse_stts_huge_entry_count_rejected() {
        // A naive `Vec::with_capacity(entry_count)` here would try to
        // allocate ~34 GiB (0xFFFF_FFFF * 8 bytes) for a 12-byte input.
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0xFF, 0xFF, 0xFF, 0xFF, // entry_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_stts(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed stts box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_parse_stsz_huge_sample_count_rejected() {
        // sample_size = 0 selects the per-sample-size branch, whose
        // sample_count is then used for Vec::with_capacity.
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0x00, 0x00, 0x00, 0x00, // sample_size = 0 (per-sample sizes follow)
            0xFF, 0xFF, 0xFF, 0xFF, // sample_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_stsz(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed stsz box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_parse_stco_huge_entry_count_rejected() {
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0xFF, 0xFF, 0xFF, 0xFF, // entry_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_stco(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed stco box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_parse_co64_huge_entry_count_rejected() {
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0xFF, 0xFF, 0xFF, 0xFF, // entry_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_co64(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed co64 box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_parse_stsc_huge_entry_count_rejected() {
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0xFF, 0xFF, 0xFF, 0xFF, // entry_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_stsc(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed stsc box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_parse_stss_huge_entry_count_rejected() {
        let data = [
            0x00, 0x00, 0x00, 0x00, // version + flags
            0xFF, 0xFF, 0xFF, 0xFF, // entry_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_stss(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed stss box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_parse_ctts_huge_entry_count_rejected() {
        let data = [
            0x00, // version
            0x00, 0x00, 0x00, // flags
            0xFF, 0xFF, 0xFF, 0xFF, // entry_count = u32::MAX
            0x00, 0x00, 0x00, 0x00, // trailing bytes (nowhere near enough)
        ];
        let mut trak = TrakBox::default();
        let result = TrakBox::parse_ctts(&data, &mut trak);
        assert!(
            result.is_err(),
            "malformed ctts box must be rejected with Err, not panic/OOM"
        );
    }

    #[test]
    fn test_check_entry_count_boundary() {
        // Exactly fits: entry_count * entry_size == available -> Ok
        assert!(check_entry_count("test", 4, 8, 32, 0).is_ok());
        // One byte short -> Err
        assert!(check_entry_count("test", 4, 8, 31, 0).is_err());
        // Must not overflow/panic when entry_count is huge relative to entry_size.
        assert!(check_entry_count("test", usize::MAX, 8, 4, 0).is_err());
    }
}

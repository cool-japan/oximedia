//! Metadata writing to container formats.
//!
//! FLAC's writer lives in the sibling `flac` module (split out to keep
//! this file under the workspace's per-file size guideline); it is
//! re-exported here so `crate::metadata::writer::FlacMetadataWriter` keeps
//! resolving exactly as before.

use async_trait::async_trait;
use oximedia_core::{OxiError, OxiResult};
use oximedia_io::MediaSource;
use std::io::SeekFrom;

use super::tags::TagMap;
use super::vorbis::VorbisComments;
use crate::ogg_page::{serialize_ogg_page, OggPage, OggPageHeader};

mod flac;
pub use flac::FlacMetadataWriter;

/// Trait for writing metadata to a media file.
#[async_trait]
pub trait MetadataWriter: Sized {
    /// Writes metadata to the file.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    async fn write<R: MediaSource>(source: &mut R, tags: &TagMap) -> OxiResult<()>;
}

// ── Matroska / EBML metadata writer ──────────────────────────────────────────

/// Maximum EBML element ID this writer emits (4 bytes).
///
/// All Matroska/`WebM` element IDs used for tag elements fit within 4 bytes.
const EBML_MAX_ID_BYTES: usize = 4;

/// Encodes an EBML element ID to its on-wire byte form.
///
/// EBML element IDs already embed their length-marker bit in the value, so the
/// encoded form is simply the minimal big-endian byte run that reproduces the
/// constant.  The minimal byte count is derived from the position of the most
/// significant set byte — every ID constant in [`element_id`] is pre-encoded
/// this way (e.g. `0x1254_C367` → 4 bytes, `0x7373` → 2 bytes, `0xA3` → 1 byte).
///
/// [`element_id`]: crate::demux::matroska::ebml::element_id
fn encode_element_id(id: u32) -> Vec<u8> {
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

/// Returns the number of bytes required to EBML-VINT-encode `value` as an
/// element size, using the minimal representation.
///
/// An `n`-byte size VINT carries `7 * n` data bits, but the all-ones data
/// pattern is reserved as the "unknown size" sentinel.  Therefore the largest
/// *representable* value in `n` bytes is `2^(7n) - 2`.  This function never
/// returns a width whose all-ones pattern would collide with `value`, so a
/// round-trip through [`encode_vint`] is always unambiguous.
fn vint_size_len(value: u64) -> usize {
    let mut len = 1usize;
    while len <= 8 {
        // Largest non-sentinel value in `len` bytes: 2^(7*len) - 2.
        let bits = 7 * len;
        let capacity = if bits >= 64 {
            u64::MAX - 1
        } else {
            (1u64 << bits) - 2
        };
        if value <= capacity {
            return len;
        }
        len += 1;
    }
    // Values above 2^56-2 cannot be represented; clamp to the 8-byte width.
    // Callers in this module never produce sizes that large.
    8
}

/// Encodes `value` as an EBML element-size VINT using the minimal byte width.
///
/// The leading byte carries a length-marker bit (a single `1` whose position
/// from the MSB equals `len - 1`); the remaining `7 * len - 7` low bits plus
/// the marker byte's data bits hold the big-endian value.
fn encode_vint(value: u64) -> Vec<u8> {
    let len = vint_size_len(value);
    let mut out = vec![0u8; len];
    // Place the value big-endian across all `len` bytes.
    let mut v = value;
    for byte in out.iter_mut().rev() {
        *byte = (v & 0xFF) as u8;
        v >>= 8;
    }
    // Set the length-marker bit: bit (8 - len) of the first byte.
    out[0] |= 1u8 << (8 - len);
    out
}

/// Appends a complete EBML element (`ID` + size-VINT + `payload`) to `out`.
fn push_element(out: &mut Vec<u8>, id: u32, payload: &[u8]) {
    out.extend_from_slice(&encode_element_id(id));
    out.extend_from_slice(&encode_vint(payload.len() as u64));
    out.extend_from_slice(payload);
}

/// Builds a Matroska `Tags` element body (the bytes that follow the `Tags`
/// element ID and size) from a [`TagMap`].
///
/// Structure produced (ISO/Matroska §"Tags"):
///
/// ```text
/// Tag(0x7373)
///   Targets(0x63C0)              -- empty: applies to the whole file
///   SimpleTag(0x67C8)            -- one per (name, value) pair
///     TagName(0x45A3)   = key
///     TagString(0x4487) = value
///   SimpleTag(0x67C8) ...
/// ```
///
/// A single `Tag` with empty `Targets` (no `TargetTypeValue`, no UID links)
/// targets the entire segment, which is the correct scope for file-level
/// metadata such as `TITLE`/`ARTIST`.  Multi-valued keys emit one `SimpleTag`
/// each, preserving the [`TagMap`] multi-value semantics.
fn build_tags_body(tags: &TagMap) -> Vec<u8> {
    use crate::demux::matroska::ebml::element_id;

    // ── Tag → Targets (empty master element) ─────────────────────────────────
    let targets_body: Vec<u8> = Vec::new();

    // ── Tag → SimpleTag list ─────────────────────────────────────────────────
    // Sort keys for deterministic output (HashMap iteration order is random).
    let mut keys: Vec<&str> = tags.keys().collect();
    keys.sort_unstable();

    let mut tag_body: Vec<u8> = Vec::new();
    push_element(&mut tag_body, element_id::TARGETS, &targets_body);

    for key in keys {
        for value in tags.get_all(key) {
            let Some(text) = value.as_text() else {
                // Binary tag values are not representable as TagString; skip.
                continue;
            };
            let mut simple_tag: Vec<u8> = Vec::new();
            push_element(&mut simple_tag, element_id::TAG_NAME, key.as_bytes());
            push_element(&mut simple_tag, element_id::TAG_STRING, text.as_bytes());
            push_element(&mut tag_body, element_id::SIMPLE_TAG, &simple_tag);
        }
    }

    // ── Wrap the single Tag inside the Tags body ─────────────────────────────
    let mut tags_body: Vec<u8> = Vec::new();
    push_element(&mut tags_body, element_id::TAG, &tag_body);
    tags_body
}

/// Result of scanning the `Segment` body: the bounded "header" children that
/// precede the first `Cluster`, and the opaque tail (clusters and everything
/// after them) that must be copied byte-for-byte.
struct SegmentLayout {
    /// Raw bytes of all bounded children before the first cluster, in file
    /// order, with any existing `Tags` element already removed. Each entry is a
    /// complete element (`ID` + size-VINT + payload).
    header_children: Vec<Vec<u8>>,
    /// Byte offset (within the whole file) where the opaque tail starts.
    tail_start: usize,
}

/// Matroska metadata writer.
///
/// Updates or creates a `Tags` element in Matroska/`WebM` files.
///
/// # Strategy
///
/// Matroska is an EBML document.  This writer performs a **full-file rewrite**
/// (the same approach [`FlacMetadataWriter`] and [`OggMetadataWriter`] use):
///
/// 1. The entire bitstream is read into memory.
/// 2. The EBML header is left untouched and copied verbatim.
/// 3. The `Segment` (`0x1853_8067`) body is scanned.  Bounded child elements
///    before the first `Cluster` are collected; any pre-existing `Tags`
///    (`0x1254_C367`) element is **dropped** so it is replaced rather than
///    duplicated.  The first `Cluster` and everything after it form an opaque
///    tail copied byte-for-byte.
/// 4. A fresh `Tags` element is built from the [`TagMap`] and spliced in just
///    before the opaque tail, keeping it inside the segment-header region the
///    demuxer scans.
/// 5. The `Segment` size VINT is recomputed for the new body length.
/// 6. If the rewritten file would be shorter than the original, a `Void`
///    (`0xEC`) padding element is appended as the segment's last child so the
///    output length never shrinks — this keeps an in-place rewrite onto the
///    same backing store self-consistent (no stale trailing bytes).
///
/// # `SeekHead` / `Cues`
///
/// Splicing changes absolute offsets, so any `SeekHead` (`0x114D_9B74`) or
/// `Cues` (`0x1C53_BB6B`) byte positions become stale.  Per the Matroska
/// specification a `SeekHead` is an optional index; conforming players
/// (and this crate's own demuxer, which scans the segment linearly) tolerate a
/// stale or absent `SeekHead`.  Rewriting those offsets is therefore **not**
/// performed; the elements are copied verbatim.  A future revision could
/// recompute them for strict-seeking players.
pub struct MatroskaMetadataWriter;

impl MatroskaMetadataWriter {
    /// EBML header magic — first four bytes of every Matroska/`WebM` file.
    const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

    /// Reads the whole source into a single byte vector.
    async fn read_all<R: MediaSource>(source: &mut R) -> OxiResult<Vec<u8>> {
        source.seek(SeekFrom::Start(0)).await?;
        let mut bytes: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = source.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
        }
        Ok(bytes)
    }

    /// Parses one EBML element header at `data[offset..]`.
    ///
    /// Returns `(id, header_len, body_size)` where `body_size` is `None` for an
    /// unknown/unbounded size.
    fn parse_header(data: &[u8], offset: usize) -> OxiResult<(u32, usize, Option<u64>)> {
        use crate::demux::matroska::ebml::parse_element_header;

        let slice = data
            .get(offset..)
            .ok_or_else(|| OxiError::InvalidData("EBML element offset past end of file".into()))?;
        let (_, element) = parse_element_header(slice).map_err(|e| {
            OxiError::InvalidData(format!("Failed to parse EBML element header: {e:?}"))
        })?;
        let body = if element.is_unbounded() {
            None
        } else {
            Some(element.size)
        };
        Ok((element.id, element.header_size, body))
    }

    /// Scans the `Segment` body and produces a [`SegmentLayout`].
    ///
    /// `segment_body_start` is the file offset of the first byte after the
    /// `Segment` element's size VINT; `segment_body_end` is one past the last
    /// byte that belongs to the segment.
    fn scan_segment(
        data: &[u8],
        segment_body_start: usize,
        segment_body_end: usize,
    ) -> OxiResult<SegmentLayout> {
        use crate::demux::matroska::ebml::element_id;

        let mut header_children: Vec<Vec<u8>> = Vec::new();
        let mut cursor = segment_body_start;
        let mut tail_start = segment_body_end;

        while cursor < segment_body_end {
            let (id, header_len, body) = Self::parse_header(data, cursor)?;

            // The first Cluster (or any unbounded-size element) marks the start
            // of the opaque tail: media payload copied verbatim.
            if id == element_id::CLUSTER || body.is_none() {
                tail_start = cursor;
                break;
            }

            let body_len = usize::try_from(body.unwrap_or(0)).map_err(|_| {
                OxiError::InvalidData("EBML element size exceeds addressable range".into())
            })?;
            let total = header_len
                .checked_add(body_len)
                .ok_or_else(|| OxiError::InvalidData("EBML element size overflow".into()))?;
            let end = cursor.checked_add(total).ok_or_else(|| {
                OxiError::InvalidData("EBML element extends past file end".into())
            })?;
            if end > segment_body_end {
                return Err(OxiError::InvalidData(
                    "EBML element extends past segment end".into(),
                ));
            }

            // Drop any existing Tags element — it will be regenerated. Every
            // other child (SeekHead, Info, Tracks, Cues, Chapters, Void, …) is
            // copied verbatim.
            if id != element_id::TAGS {
                header_children.push(data[cursor..end].to_vec());
            }

            cursor = end;
        }

        Ok(SegmentLayout {
            header_children,
            tail_start,
        })
    }

    /// Builds the complete rewritten file bytes.
    fn build_output(data: &[u8], tags: &TagMap) -> OxiResult<Vec<u8>> {
        use crate::demux::matroska::ebml::element_id;

        // ── Validate the EBML magic ──────────────────────────────────────────
        if data.len() < EBML_MAX_ID_BYTES || data[..4] != Self::EBML_MAGIC {
            return Err(OxiError::UnknownFormat);
        }

        // ── Locate and measure the EBML header element ───────────────────────
        let (ebml_id, ebml_header_len, ebml_body) = Self::parse_header(data, 0)?;
        if ebml_id != element_id::EBML {
            return Err(OxiError::UnknownFormat);
        }
        let ebml_body_len = match ebml_body {
            Some(n) => usize::try_from(n).map_err(|_| {
                OxiError::InvalidData("EBML header size exceeds addressable range".into())
            })?,
            None => {
                return Err(OxiError::InvalidData(
                    "EBML header must have a known size".into(),
                ))
            }
        };
        let segment_offset = ebml_header_len
            .checked_add(ebml_body_len)
            .ok_or_else(|| OxiError::InvalidData("EBML header size overflow".into()))?;

        // ── Locate the Segment element ───────────────────────────────────────
        let (segment_id, segment_header_len, segment_body) =
            Self::parse_header(data, segment_offset)?;
        if segment_id != element_id::SEGMENT {
            return Err(OxiError::InvalidData(format!(
                "Expected Segment element, found 0x{segment_id:X}"
            )));
        }
        // Length of the Segment element ID alone (without its size VINT). The
        // original ID bytes are copied verbatim; only the size VINT is rebuilt.
        let segment_id_len = Self::element_id_len(data, segment_offset)?;
        let segment_body_start = segment_offset
            .checked_add(segment_header_len)
            .ok_or_else(|| OxiError::InvalidData("Segment header size overflow".into()))?;

        // A bounded Segment ends where its size says; an unbounded Segment runs
        // to end-of-file (valid per spec for streamed/unsized segments).
        let segment_body_end = match segment_body {
            Some(n) => {
                let len = usize::try_from(n).map_err(|_| {
                    OxiError::InvalidData("Segment size exceeds addressable range".into())
                })?;
                let end = segment_body_start
                    .checked_add(len)
                    .ok_or_else(|| OxiError::InvalidData("Segment size overflow".into()))?;
                if end > data.len() {
                    return Err(OxiError::InvalidData(
                        "Segment extends past end of file".into(),
                    ));
                }
                end
            }
            None => data.len(),
        };

        // ── Scan segment children, dropping any old Tags element ─────────────
        let layout = Self::scan_segment(data, segment_body_start, segment_body_end)?;
        let tail: &[u8] = &data[layout.tail_start..segment_body_end];

        // ── Build the new Tags element ───────────────────────────────────────
        let tags_body = build_tags_body(tags);
        let mut tags_element: Vec<u8> = Vec::new();
        push_element(&mut tags_element, element_id::TAGS, &tags_body);

        // ── Assemble the new Segment body (without padding) ──────────────────
        // Order: header children (verbatim) → new Tags → opaque tail.
        let mut segment_body_out: Vec<u8> = Vec::new();
        for child in &layout.header_children {
            segment_body_out.extend_from_slice(child);
        }
        segment_body_out.extend_from_slice(&tags_element);
        segment_body_out.extend_from_slice(tail);

        // ── Pad so the rewritten file is never shorter than the original ─────
        //
        // A full-file rewrite onto the same backing store cannot shrink the
        // file (the MediaSource trait has no truncate). If the new content is
        // shorter, append a Void element as the segment's last child to absorb
        // the difference; the demuxer and all conforming players skip Void.
        //
        // The fixed prefix is everything emitted before the (recomputed)
        // Segment size VINT: the verbatim EBML header plus the Segment ID
        // bytes. The original Segment size VINT is *not* part of it — it is
        // discarded and rebuilt below.
        let prefix_before_segment_size = segment_offset
            .checked_add(segment_id_len)
            .ok_or_else(|| OxiError::InvalidData("Segment prefix length overflow".into()))?;
        let original_len = data.len();
        let mut final_body = segment_body_out;
        // Iterate: appending the Void can widen both the Void's own size VINT
        // and the Segment size VINT, but the loop converges quickly because the
        // gap monotonically shrinks (each pass adds at least `gap` body bytes).
        loop {
            let segment_size_vint = encode_vint(final_body.len() as u64);
            let projected_len =
                prefix_before_segment_size + segment_size_vint.len() + final_body.len();
            if projected_len >= original_len {
                break;
            }
            let gap = original_len - projected_len;
            // A Void element needs at least 2 bytes (1-byte ID + 1-byte size
            // VINT, empty payload); `build_void_filler` clamps to that minimum
            // and otherwise produces a Void whose total length equals `gap`.
            let void = Self::build_void_filler(gap);
            final_body.extend_from_slice(&void);
        }

        // ── Recompute the Segment size VINT and emit the file ────────────────
        let segment_size_vint = encode_vint(final_body.len() as u64);
        let mut output: Vec<u8> = Vec::with_capacity(
            prefix_before_segment_size + segment_size_vint.len() + final_body.len(),
        );
        // EBML header element + Segment element ID, copied verbatim.
        output.extend_from_slice(&data[..prefix_before_segment_size]);
        // Freshly computed Segment size VINT (the segment is always emitted
        // with a known, bounded size so the demuxer locates the new Tags).
        output.extend_from_slice(&segment_size_vint);
        // New Segment body: verbatim header children + Tags + opaque tail.
        output.extend_from_slice(&final_body);

        Ok(output)
    }

    /// Returns the byte length of the EBML element ID at `data[offset..]`
    /// (the ID's own VINT length, excluding the element's size field).
    fn element_id_len(data: &[u8], offset: usize) -> OxiResult<usize> {
        use crate::demux::matroska::ebml::parse_element_id;

        let slice = data
            .get(offset..)
            .ok_or_else(|| OxiError::InvalidData("Element offset past end of file".into()))?;
        let before = slice.len();
        let (rest, _) = parse_element_id(slice)
            .map_err(|e| OxiError::InvalidData(format!("Failed to parse element ID: {e:?}")))?;
        Ok(before - rest.len())
    }

    /// Builds a `Void` element whose **total** on-wire length equals `total`
    /// bytes (ID + size-VINT + zero payload).
    ///
    /// `Void` (`0xEC`) has a 1-byte ID.  The function picks a size-VINT width
    /// such that `1 + vint_len + payload == total` holds exactly, so the
    /// padding fills the gap with no leftover bytes.  `total` is always `>= 2`
    /// at call sites (the smallest possible Void is `0xEC 0x80` — empty body).
    fn build_void_filler(total: usize) -> Vec<u8> {
        use crate::demux::matroska::ebml::element_id;

        // Smallest Void is 2 bytes (ID + 1-byte size VINT, empty payload).
        let total = total.max(2);
        // Try each size-VINT width 1..=8 and find the one where the payload is
        // non-negative and the size value fits that width.
        for vint_len in 1usize..=8 {
            // overhead = 1 (ID) + vint_len
            let overhead = 1 + vint_len;
            if total < overhead {
                continue;
            }
            let payload = total - overhead;
            // The chosen width must be exactly the minimal width for `payload`,
            // otherwise encode_vint would pick a different (shorter) width and
            // the totals would not match.
            if vint_size_len(payload as u64) == vint_len {
                let mut out = Vec::with_capacity(total);
                out.extend_from_slice(&encode_element_id(element_id::VOID));
                out.extend_from_slice(&encode_vint(payload as u64));
                out.resize(total, 0);
                return out;
            }
        }
        // Fallback: an empty Void (2 bytes). Unreachable for sane `total`.
        let mut out = Vec::with_capacity(2);
        out.extend_from_slice(&encode_element_id(element_id::VOID));
        out.extend_from_slice(&encode_vint(0));
        out
    }
}

#[async_trait]
impl MetadataWriter for MatroskaMetadataWriter {
    async fn write<R: MediaSource>(source: &mut R, tags: &TagMap) -> OxiResult<()> {
        if !source.is_writable() {
            return Err(OxiError::Unsupported(
                "Matroska metadata writing requires a writable MediaSource".into(),
            ));
        }

        // Read the whole file, rebuild it with the new Tags element, write back.
        let data = Self::read_all(source).await?;
        let output = Self::build_output(&data, tags)?;

        source.seek(SeekFrom::Start(0)).await?;
        source.write_all(&output).await?;

        Ok(())
    }
}

/// Ogg metadata writer.
///
/// Updates Vorbis comments in Ogg files using a full-file rewrite strategy.
///
/// The writer:
/// 1. Reads the entire Ogg bitstream into memory.
/// 2. Parses all Ogg pages sequentially.
/// 3. Locates the Vorbis comment header page (identified by the `\x03vorbis` packet prefix).
/// 4. Rebuilds that page with the new [`TagMap`], recalculating the segment table and CRC-32.
/// 5. If the comment packet now spans a different number of pages than before, all downstream
///    page sequence numbers are adjusted and their CRCs recomputed.
/// 6. Seeks to offset 0 and writes the modified bitstream back to the source.
pub struct OggMetadataWriter;

/// Vorbis comment packet type byte.
const VORBIS_COMMENT_PACKET_TYPE: u8 = 0x03;
/// Vorbis codec identifier: `b"vorbis"`.
const VORBIS_CODEC_ID: &[u8] = b"vorbis";
/// Length of the Vorbis packet header prefix (`\x03vorbis`).
const VORBIS_PACKET_HEADER_LEN: usize = 7;
/// Vorbis comment framing bit appended at the end of the comment packet (§5.2.3).
const VORBIS_FRAMING_BIT: u8 = 0x01;

/// Builds a lacing segment table for a packet of `payload_len` bytes.
///
/// Per RFC 3533, each segment carries at most 255 bytes.  A segment whose
/// lace value equals 255 means the packet continues; a smaller value (including
/// 0) terminates it.
#[allow(clippy::cast_possible_truncation)]
fn build_segment_table(payload_len: usize) -> Vec<u8> {
    let mut table = Vec::new();
    let mut remaining = payload_len;
    loop {
        if remaining >= 255 {
            table.push(255u8);
            remaining -= 255;
        } else {
            table.push(remaining as u8);
            break;
        }
    }
    table
}

#[async_trait]
impl MetadataWriter for OggMetadataWriter {
    async fn write<R: MediaSource>(source: &mut R, tags: &TagMap) -> OxiResult<()> {
        // ── Verify the source supports writing ───────────────────────────────
        if !source.is_writable() {
            return Err(OxiError::Unsupported(
                "Ogg metadata writing requires a writable MediaSource".into(),
            ));
        }

        // ── Read the entire bitstream ─────────────────────────────────────────
        source.seek(SeekFrom::Start(0)).await?;
        let mut file_bytes: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = source.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            file_bytes.extend_from_slice(&chunk[..n]);
        }

        // ── Parse all Ogg pages ───────────────────────────────────────────────
        let mut pages: Vec<OggPage> = Vec::new();
        let mut cursor = 0usize;
        while cursor < file_bytes.len() {
            let (page, consumed) =
                crate::ogg_page::parse_ogg_page(&file_bytes[cursor..]).map_err(|e| {
                    OxiError::InvalidData(format!("Ogg page parse error at offset {cursor}: {e}"))
                })?;
            pages.push(page);
            cursor += consumed;
        }

        if pages.is_empty() {
            return Err(OxiError::InvalidData("No Ogg pages found".into()));
        }

        // ── Locate the Vorbis comment page ────────────────────────────────────
        //
        // The comment page contains a packet that begins with `\x03vorbis`.
        // We reconstruct the leading bytes of the first packet on each page,
        // handling both single-page and multi-page packets.
        let comment_page_idx = Self::find_comment_page_index(&pages)?;

        // ── Build new comment packet bytes ────────────────────────────────────
        //
        // Format: [\x03vorbis] + VorbisComments::encode() + [\x01 framing bit]
        let new_comment_packet = Self::build_comment_packet(tags);

        // ── Determine how many old pages the comment packet occupies ──────────
        //
        // Starting at `comment_page_idx`, count pages until the packet ends
        // (last segment-table entry < 255 on the same logical bitstream).
        let old_comment_page_count = Self::count_comment_pages(&pages, comment_page_idx);

        // ── Rebuild comment page(s) ───────────────────────────────────────────
        let original_page = &pages[comment_page_idx];
        let serial = original_page.header.serial;
        let granule_pos = original_page.header.granule_pos;
        let header_type = original_page.header.header_type;
        let base_seq_num = original_page.header.seq_num;

        let new_pages = Self::build_pages_for_packet(
            &new_comment_packet,
            serial,
            base_seq_num,
            granule_pos,
            header_type,
        );

        // ── Calculate sequence number delta ───────────────────────────────────
        //
        // If the number of pages changed we must renumber all subsequent pages
        // on the same logical bitstream and recompute their CRCs.
        let seq_delta: i64 = new_pages.len() as i64 - old_comment_page_count as i64;

        // ── Assemble the final page list ──────────────────────────────────────
        let end_old = comment_page_idx + old_comment_page_count;
        let estimated_cap = (pages.len() as i64 + seq_delta).max(0) as usize;
        let mut result_pages: Vec<OggPage> = Vec::with_capacity(estimated_cap);

        // Pages before the comment page — unmodified
        for page in pages.iter().take(comment_page_idx) {
            result_pages.push(page.clone());
        }

        // New comment page(s)
        result_pages.extend(new_pages);

        // Pages after the comment page — update seq_num if needed.
        // serialize_ogg_page recomputes the CRC automatically, so we only
        // need to update the seq_num and zero the stale checksum field
        // (serialize_ogg_page ignores the checksum field in the header).
        for page in pages.iter().skip(end_old) {
            let mut p = page.clone();
            if p.header.serial == serial && seq_delta != 0 {
                let new_seq = (p.header.seq_num as i64 + seq_delta) as u32;
                p.header.seq_num = new_seq;
                p.header.checksum = 0; // will be recomputed by serialize_ogg_page
            }
            result_pages.push(p);
        }

        // ── Serialize all pages and write back ────────────────────────────────
        let mut output: Vec<u8> = Vec::with_capacity(file_bytes.len());
        for page in &result_pages {
            output.extend_from_slice(&serialize_ogg_page(page));
        }

        source.seek(SeekFrom::Start(0)).await?;
        source.write_all(&output).await?;

        Ok(())
    }
}

impl OggMetadataWriter {
    /// Finds the index of the Vorbis comment header page.
    ///
    /// Scans pages and looks for one whose first packet starts with `\x03vorbis`.
    fn find_comment_page_index(pages: &[OggPage]) -> OxiResult<usize> {
        for (idx, page) in pages.iter().enumerate() {
            // Skip continuation pages — the comment packet always starts fresh.
            if page.header.is_continued() {
                continue;
            }
            // Look at the beginning of the page data.
            if page.page_data.len() >= VORBIS_PACKET_HEADER_LEN
                && page.page_data[0] == VORBIS_COMMENT_PACKET_TYPE
                && &page.page_data[1..VORBIS_PACKET_HEADER_LEN] == VORBIS_CODEC_ID
            {
                return Ok(idx);
            }
        }
        Err(OxiError::InvalidData(
            "Vorbis comment header page not found in Ogg bitstream".into(),
        ))
    }

    /// Counts how many consecutive pages the comment packet spans starting at
    /// `start_idx`.
    ///
    /// A packet spans multiple pages when its last segment-table entry equals
    /// 255 (the "lace continuation" signal from RFC 3533).
    fn count_comment_pages(pages: &[OggPage], start_idx: usize) -> usize {
        let mut count = 0;
        for page in pages.iter().skip(start_idx) {
            count += 1;
            // If the last segment table entry is < 255 the packet ends here.
            if page.segment_table.last().map_or(true, |&last| last < 255) {
                break;
            }
        }
        count
    }

    /// Builds the raw Vorbis comment packet bytes.
    ///
    /// Packet format (Vorbis I spec §5.2.3):
    /// ```text
    /// [0x03] [v][o][r][b][i][s] VorbisComments::encode() [0x01]
    /// ```
    fn build_comment_packet(tags: &TagMap) -> Vec<u8> {
        let mut comments = VorbisComments::with_vendor("OxiMedia");
        comments.tags = tags.clone();

        let mut packet = Vec::new();
        // 7-byte Vorbis packet header: type byte + "vorbis"
        packet.push(VORBIS_COMMENT_PACKET_TYPE);
        packet.extend_from_slice(VORBIS_CODEC_ID);
        // Encoded Vorbis comments
        packet.extend_from_slice(&comments.encode());
        // Framing bit (Vorbis I spec §5.2.3)
        packet.push(VORBIS_FRAMING_BIT);
        packet
    }

    /// Splits a packet into one or more Ogg pages, each holding at most 255
    /// segments × 255 bytes payload.
    ///
    /// The `header_type` and `granule_pos` from the original comment page are
    /// preserved on the first replacement page; continuation pages carry
    /// `header_type = HEADER_TYPE_CONTINUATION (0x01)` and `granule_pos = 0`.
    #[allow(clippy::cast_possible_truncation)]
    fn build_pages_for_packet(
        packet: &[u8],
        serial: u32,
        base_seq_num: u32,
        granule_pos: i64,
        header_type: u8,
    ) -> Vec<OggPage> {
        const MAX_SEGMENTS: usize = 255;
        const MAX_SEGMENT_PAYLOAD: usize = 255 * MAX_SEGMENTS; // 65 025

        let mut pages = Vec::new();
        let mut offset = 0usize;
        let mut seq = base_seq_num;
        let mut is_first_page = true;

        while offset < packet.len() || (offset == 0 && packet.is_empty()) {
            let remaining = packet.len().saturating_sub(offset);
            let chunk_len = remaining.min(MAX_SEGMENT_PAYLOAD);
            let chunk = &packet[offset..offset + chunk_len];
            offset += chunk_len;

            let segment_table = build_segment_table(chunk_len);
            let seg_count = segment_table.len().min(MAX_SEGMENTS);
            let segment_table = segment_table[..seg_count].to_vec();

            let this_header_type = if is_first_page {
                header_type
            } else {
                // continuation page
                0x01
            };
            let this_granule_pos = if is_first_page { granule_pos } else { 0 };

            let header = OggPageHeader {
                version: 0,
                header_type: this_header_type,
                granule_pos: this_granule_pos,
                serial,
                seq_num: seq,
                checksum: 0, // will be filled by serialize_ogg_page
                segment_count: seg_count as u8,
            };
            pages.push(OggPage {
                header,
                segment_table,
                page_data: chunk.to_vec(),
            });

            seq = seq.wrapping_add(1);
            is_first_page = false;

            if offset >= packet.len() {
                break;
            }
        }

        pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ogg_page::{
        serialize_ogg_page, OggPage, OggPageHeader, HEADER_TYPE_BOS, HEADER_TYPE_EOS,
    };
    use oximedia_io::MemorySource;

    // ── Ogg helpers ──────────────────────────────────────────────────────────

    /// Build a minimal but structurally valid Ogg Vorbis file in memory.
    ///
    /// Contains three pages:
    /// 1. BOS page — `\x01vorbis` + 30-byte ident body (all zeroes; opaque to
    ///    the writer).
    /// 2. Comment page — `\x03vorbis` + empty VorbisComments + framing bit.
    /// 3. EOS page — empty audio data sentinel.
    fn build_minimal_ogg(serial: u32) -> Vec<u8> {
        let mut out = Vec::new();

        // Page 0: identification header (BOS)
        {
            let mut packet = Vec::new();
            packet.push(0x01u8); // packet type: ident
            packet.extend_from_slice(b"vorbis");
            packet.extend_from_slice(&[0u8; 30]); // minimal ident body
            let seg_table = build_segment_table(packet.len());
            let seg_count = seg_table.len() as u8;
            let page = OggPage {
                header: OggPageHeader {
                    version: 0,
                    header_type: HEADER_TYPE_BOS,
                    granule_pos: 0,
                    serial,
                    seq_num: 0,
                    checksum: 0,
                    segment_count: seg_count,
                },
                segment_table: seg_table,
                page_data: packet,
            };
            out.extend_from_slice(&serialize_ogg_page(&page));
        }

        // Page 1: comment header
        {
            let packet = OggMetadataWriter::build_comment_packet(&TagMap::new());
            let seg_table = build_segment_table(packet.len());
            let seg_count = seg_table.len() as u8;
            let page = OggPage {
                header: OggPageHeader {
                    version: 0,
                    header_type: 0,
                    granule_pos: 0,
                    serial,
                    seq_num: 1,
                    checksum: 0,
                    segment_count: seg_count,
                },
                segment_table: seg_table,
                page_data: packet,
            };
            out.extend_from_slice(&serialize_ogg_page(&page));
        }

        // Page 2: EOS
        {
            let page = OggPage {
                header: OggPageHeader {
                    version: 0,
                    header_type: HEADER_TYPE_EOS,
                    granule_pos: -1,
                    serial,
                    seq_num: 2,
                    checksum: 0,
                    segment_count: 1,
                },
                segment_table: vec![1],
                page_data: vec![0x00],
            };
            out.extend_from_slice(&serialize_ogg_page(&page));
        }

        out
    }

    // ── Ogg tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_ogg_write_updates_tags() {
        let serial = 0x1234u32;
        let initial = build_minimal_ogg(serial);

        // Build a writable MemorySource pre-seeded with the initial Ogg data.
        let mut source = {
            let mut s = MemorySource::new_writable(initial.len() + 512);
            s.seek(SeekFrom::Start(0))
                .await
                .expect("seek should succeed");
            s.write_all(&initial).await.expect("write should succeed");
            s.seek(SeekFrom::Start(0))
                .await
                .expect("seek should succeed");
            s
        };

        let mut tags = TagMap::new();
        tags.set("TITLE", "Ogg Test Title");
        tags.set("ARTIST", "Ogg Test Artist");

        OggMetadataWriter::write(&mut source, &tags)
            .await
            .expect("OggMetadataWriter::write should succeed");

        // Re-parse the output and verify the comment page
        source
            .seek(SeekFrom::Start(0))
            .await
            .expect("seek should succeed");
        let output = source.written_data().to_vec();

        // Parse pages from output
        let mut pages: Vec<crate::ogg_page::OggPage> = Vec::new();
        let mut cursor = 0usize;
        while cursor < output.len() {
            let (page, consumed) = crate::ogg_page::parse_ogg_page(&output[cursor..])
                .expect("output page should be valid");
            pages.push(page);
            cursor += consumed;
        }

        // Find the comment page and parse VorbisComments
        let comment_page_idx = OggMetadataWriter::find_comment_page_index(&pages)
            .expect("comment page must be present");
        let page = &pages[comment_page_idx];

        // Skip the 7-byte `\x03vorbis` header, trim the trailing framing bit
        assert!(page.page_data.len() > VORBIS_PACKET_HEADER_LEN + 1);
        let comment_data_end = page.page_data.len() - 1; // strip framing bit
        let comment_data = &page.page_data[VORBIS_PACKET_HEADER_LEN..comment_data_end];

        let parsed = VorbisComments::parse(comment_data).expect("comment data should parse");
        assert_eq!(parsed.tags.get_text("TITLE"), Some("Ogg Test Title"));
        assert_eq!(parsed.tags.get_text("ARTIST"), Some("Ogg Test Artist"));
    }

    #[tokio::test]
    async fn test_ogg_write_eos_page_intact() {
        // Verifies that page(s) after the comment page remain valid after a write.
        let serial = 0xABCDu32;
        let initial = build_minimal_ogg(serial);
        let page_count_before = {
            let mut count = 0usize;
            let mut cursor = 0usize;
            while cursor < initial.len() {
                let (_, consumed) = crate::ogg_page::parse_ogg_page(&initial[cursor..])
                    .expect("initial page should be valid");
                count += 1;
                cursor += consumed;
            }
            count
        };

        let mut source = {
            let mut s = MemorySource::new_writable(initial.len() + 512);
            s.write_all(&initial).await.expect("write should succeed");
            s.seek(SeekFrom::Start(0))
                .await
                .expect("seek should succeed");
            s
        };

        let mut tags = TagMap::new();
        tags.set("ALBUM", "Test Album");
        OggMetadataWriter::write(&mut source, &tags)
            .await
            .expect("write should succeed");

        let output = source.written_data().to_vec();
        let mut page_count_after = 0usize;
        let mut cursor = 0usize;
        while cursor < output.len() {
            let (_, consumed) = crate::ogg_page::parse_ogg_page(&output[cursor..])
                .expect("output page should be valid");
            page_count_after += 1;
            cursor += consumed;
        }
        // Page count should stay the same (comment fits in one page, EOS unchanged)
        assert_eq!(page_count_before, page_count_after);
    }

    #[tokio::test]
    async fn test_ogg_write_non_writable_source_returns_error() {
        let initial = build_minimal_ogg(0x1u32);
        // MemorySource::from_vec is NOT writable
        let mut source = MemorySource::from_vec(initial);
        let tags = TagMap::new();
        let result = OggMetadataWriter::write(&mut source, &tags).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_ogg_build_comment_packet_structure() {
        let mut tags = TagMap::new();
        tags.set("TITLE", "Hello");
        let packet = OggMetadataWriter::build_comment_packet(&tags);

        // First byte: type = 0x03
        assert_eq!(packet[0], VORBIS_COMMENT_PACKET_TYPE);
        // Next 6 bytes: "vorbis"
        assert_eq!(&packet[1..7], b"vorbis");
        // Last byte: framing bit
        assert_eq!(
            *packet.last().expect("packet must not be empty"),
            VORBIS_FRAMING_BIT
        );
        // Must be longer than the 8-byte minimum (7 header + at least empty vorbis)
        assert!(packet.len() > VORBIS_PACKET_HEADER_LEN + 1);
    }

    #[test]
    fn test_ogg_find_comment_page_index() {
        let serial = 0x0001u32;
        let ogg_bytes = build_minimal_ogg(serial);

        let mut pages = Vec::new();
        let mut cursor = 0usize;
        while cursor < ogg_bytes.len() {
            let (page, consumed) =
                crate::ogg_page::parse_ogg_page(&ogg_bytes[cursor..]).expect("page must parse");
            pages.push(page);
            cursor += consumed;
        }

        let idx =
            OggMetadataWriter::find_comment_page_index(&pages).expect("comment page must be found");
        assert_eq!(idx, 1, "comment page should be the second page (index 1)");
    }

    #[test]
    fn test_ogg_build_segment_table_small() {
        // 5 bytes → one segment with value 5
        let table = build_segment_table(5);
        assert_eq!(table, vec![5u8]);
    }

    #[test]
    fn test_ogg_build_segment_table_exactly_255() {
        // 255 bytes → one segment with value 255, then one with value 0
        let table = build_segment_table(255);
        assert_eq!(table, vec![255u8, 0u8]);
    }

    #[test]
    fn test_ogg_build_segment_table_large() {
        // 510 bytes → [255, 255, 0]
        let table = build_segment_table(510);
        assert_eq!(table, vec![255u8, 255u8, 0u8]);
    }

    #[test]
    fn test_ogg_build_segment_table_513() {
        // 513 bytes → [255, 255, 3]
        let table = build_segment_table(513);
        assert_eq!(table, vec![255u8, 255u8, 3u8]);
    }

    // ── Matroska / EBML writer ────────────────────────────────────────────────

    use crate::demux::matroska::ebml::element_id as mkv_id;
    use crate::demux::matroska::ebml::parse_element_header;
    use crate::demux::matroska::MatroskaDemuxer;
    use crate::demux::Demuxer;
    use crate::metadata::TagValue;

    // ── EBML primitive encoder tests ─────────────────────────────────────────

    #[test]
    fn test_mkv_encode_element_id() {
        // 1-byte ID (Void).
        assert_eq!(encode_element_id(0xEC), vec![0xEC]);
        // 2-byte ID (Tag).
        assert_eq!(encode_element_id(0x7373), vec![0x73, 0x73]);
        // 2-byte ID (SimpleTag).
        assert_eq!(encode_element_id(0x67C8), vec![0x67, 0xC8]);
        // 4-byte ID (Tags / Segment).
        assert_eq!(encode_element_id(0x1254_C367), vec![0x12, 0x54, 0xC3, 0x67]);
        assert_eq!(encode_element_id(0x1853_8067), vec![0x18, 0x53, 0x80, 0x67]);
    }

    #[test]
    fn test_mkv_vint_size_len_boundaries() {
        // 1 byte: 0 .. 2^7 - 2 (126); 127 is the unknown-size sentinel.
        assert_eq!(vint_size_len(0), 1);
        assert_eq!(vint_size_len(126), 1);
        // 127 collides with the 1-byte sentinel → must use 2 bytes.
        assert_eq!(vint_size_len(127), 2);
        // 2 bytes: up to 2^14 - 2 (16382).
        assert_eq!(vint_size_len(16_382), 2);
        assert_eq!(vint_size_len(16_383), 3);
        // 3 bytes: up to 2^21 - 2.
        assert_eq!(vint_size_len(2_097_150), 3);
        assert_eq!(vint_size_len(2_097_151), 4);
    }

    #[test]
    fn test_mkv_encode_vint_roundtrip() {
        // Every encoded size VINT must decode back to the same value.
        for value in [
            0u64,
            1,
            5,
            100,
            126,
            127,
            128,
            1000,
            16_382,
            16_383,
            16_384,
            100_000,
            2_097_150,
            2_097_151,
            5_000_000,
            100_000_000,
        ] {
            let encoded = encode_vint(value);
            // The leading length-marker bit must mark exactly `encoded.len()`.
            let len = encoded.len();
            let marker = 1u8 << (8 - len);
            assert!(
                encoded[0] & marker != 0,
                "value {value}: missing length marker for {len}-byte VINT"
            );
            // Decode using the production EBML parser. Prefix with a 1-byte
            // Void ID so the (ID + size) header parser can be reused.
            let mut probe = vec![0xECu8];
            probe.extend_from_slice(&encoded);
            let (rest, element) =
                parse_element_header(&probe).expect("encoded VINT must parse as an element size");
            assert!(rest.is_empty());
            assert_eq!(element.id, 0xEC);
            assert_eq!(element.size, value, "VINT roundtrip mismatch for {value}");
        }
    }

    #[test]
    fn test_mkv_encode_vint_not_unknown_sentinel() {
        // A size VINT of a real value must never decode to the "unknown size"
        // sentinel (u64::MAX) — that would silently corrupt the element.
        // The boundary values 126 / 16_382 / 2_097_150 are the largest values
        // that fit each VINT width and are the riskiest for sentinel collision.
        for value in [126u64, 127, 16_382, 16_383, 2_097_150, 2_097_151] {
            let encoded = encode_vint(value);
            let mut probe = vec![0xECu8];
            probe.extend_from_slice(&encoded);
            let (_, element) = parse_element_header(&probe).expect("encoded size VINT must parse");
            assert_ne!(
                element.size,
                u64::MAX,
                "value {value} was encoded as the unknown-size sentinel"
            );
            assert_eq!(element.size, value);
        }
    }

    // ── Minimal MKV synthesis helpers ────────────────────────────────────────

    /// Builds a complete EBML element: ID + size-VINT + payload.
    fn mkv_element(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        push_element(&mut out, id, payload);
        out
    }

    /// Builds a minimal EBML header declaring DocType "matroska".
    ///
    /// The demuxer's `parse_ebml_header` requires a recognised DocType, so the
    /// header carries `DocType`, `DocTypeVersion` and `DocTypeReadVersion`.
    fn mkv_ebml_header() -> Vec<u8> {
        let mut body = Vec::new();
        push_element(&mut body, mkv_id::EBML_VERSION, &[0x01]);
        push_element(&mut body, mkv_id::EBML_READ_VERSION, &[0x01]);
        push_element(&mut body, mkv_id::EBML_MAX_ID_LENGTH, &[0x04]);
        push_element(&mut body, mkv_id::EBML_MAX_SIZE_LENGTH, &[0x08]);
        push_element(&mut body, mkv_id::DOC_TYPE, b"matroska");
        push_element(&mut body, mkv_id::DOC_TYPE_VERSION, &[0x04]);
        push_element(&mut body, mkv_id::DOC_TYPE_READ_VERSION, &[0x02]);
        mkv_element(mkv_id::EBML, &body)
    }

    /// Builds a minimal `Info` element with only a `TimecodeScale`.
    fn mkv_info() -> Vec<u8> {
        let mut body = Vec::new();
        // TimecodeScale = 1_000_000 ns (the Matroska default).
        push_element(
            &mut body,
            mkv_id::TIMECODE_SCALE,
            &[0x0F, 0x42, 0x40], // 1_000_000 big-endian, minimal bytes
        );
        mkv_element(mkv_id::INFO, &body)
    }

    /// Builds a `Tags` element holding the given (name, value) pairs.
    fn mkv_tags(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut tag_body = Vec::new();
        push_element(&mut tag_body, mkv_id::TARGETS, &[]);
        for (name, value) in pairs {
            let mut simple = Vec::new();
            push_element(&mut simple, mkv_id::TAG_NAME, name.as_bytes());
            push_element(&mut simple, mkv_id::TAG_STRING, value.as_bytes());
            push_element(&mut tag_body, mkv_id::SIMPLE_TAG, &simple);
        }
        let mut tags_body = Vec::new();
        push_element(&mut tags_body, mkv_id::TAG, &tag_body);
        mkv_element(mkv_id::TAGS, &tags_body)
    }

    /// Assembles a minimal valid MKV file: EBML header + bounded Segment that
    /// contains `segment_children` concatenated in order.
    fn mkv_file(segment_children: &[u8]) -> Vec<u8> {
        let mut out = mkv_ebml_header();
        out.extend_from_slice(&mkv_element(mkv_id::SEGMENT, segment_children));
        out
    }

    /// Reads back all tags from an in-memory MKV using the production demuxer.
    async fn read_back_tags(mkv: &[u8]) -> TagMap {
        let source = MemorySource::from_vec(mkv.to_vec());
        let mut demuxer = MatroskaDemuxer::new(source);
        demuxer
            .probe()
            .await
            .expect("demuxer must probe the rewritten MKV");
        crate::metadata::reader::MatroskaMetadataReader::convert_tags(demuxer.tags())
    }

    /// Wraps `bytes` in a writable `MemorySource` positioned at offset 0.
    async fn writable_source(bytes: &[u8]) -> MemorySource {
        let mut s = MemorySource::new_writable(bytes.len() + 1024);
        s.write_all(bytes).await.expect("seed write must succeed");
        s.seek(SeekFrom::Start(0)).await.expect("seek must succeed");
        s
    }

    // ── End-to-end writer tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_mkv_write_creates_tags_when_absent() {
        // Segment has Info but NO Tags element.
        let mut segment = Vec::new();
        segment.extend_from_slice(&mkv_info());
        let initial = mkv_file(&segment);

        let mut source = writable_source(&initial).await;

        let mut tags = TagMap::new();
        tags.set("TITLE", "Created Title");
        tags.set("ARTIST", "Created Artist");

        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("Matroska write should succeed");

        let output = source.written_data().to_vec();
        let parsed = read_back_tags(&output).await;
        assert_eq!(parsed.get_text("TITLE"), Some("Created Title"));
        assert_eq!(parsed.get_text("ARTIST"), Some("Created Artist"));
    }

    #[tokio::test]
    async fn test_mkv_write_replaces_existing_tags() {
        // Segment already carries a Tags element with stale values.
        let mut segment = Vec::new();
        segment.extend_from_slice(&mkv_info());
        segment.extend_from_slice(&mkv_tags(&[
            ("TITLE", "Old Title"),
            ("ARTIST", "Old Artist"),
            ("COMMENT", "stale comment that should disappear"),
        ]));
        let initial = mkv_file(&segment);

        let mut source = writable_source(&initial).await;

        let mut tags = TagMap::new();
        tags.set("TITLE", "New Title");
        tags.set("ALBUM", "New Album");

        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("Matroska write should succeed");

        let output = source.written_data().to_vec();
        let parsed = read_back_tags(&output).await;
        assert_eq!(parsed.get_text("TITLE"), Some("New Title"));
        assert_eq!(parsed.get_text("ALBUM"), Some("New Album"));
        // The stale COMMENT and ARTIST must be gone (Tags fully replaced).
        assert_eq!(parsed.get_text("COMMENT"), None);
        assert_eq!(parsed.get_text("ARTIST"), None);
    }

    #[tokio::test]
    async fn test_mkv_write_no_duplicate_tags_element() {
        // After writing, the Segment must contain exactly ONE Tags element.
        let mut segment = Vec::new();
        segment.extend_from_slice(&mkv_info());
        segment.extend_from_slice(&mkv_tags(&[("TITLE", "First")]));
        let initial = mkv_file(&segment);

        let mut source = writable_source(&initial).await;
        let mut tags = TagMap::new();
        tags.set("TITLE", "Second");
        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("write should succeed");

        let output = source.written_data().to_vec();

        // Walk the Segment body and count Tags elements.
        let (_, ebml) = parse_element_header(&output).expect("ebml header");
        let seg_off = ebml.header_size + ebml.size as usize;
        let (_, seg) = parse_element_header(&output[seg_off..]).expect("segment header");
        let body_start = seg_off + seg.header_size;
        let body_end = body_start + seg.size as usize;

        let mut cursor = body_start;
        let mut tags_count = 0usize;
        while cursor < body_end {
            let (_, el) = parse_element_header(&output[cursor..]).expect("child header");
            if el.id == mkv_id::TAGS {
                tags_count += 1;
            }
            cursor += el.header_size + el.size as usize;
        }
        assert_eq!(tags_count, 1, "exactly one Tags element expected");
    }

    #[tokio::test]
    async fn test_mkv_write_preserves_segment_structure() {
        // Info must survive the rewrite untouched alongside the new Tags.
        let mut segment = Vec::new();
        segment.extend_from_slice(&mkv_info());
        let initial = mkv_file(&segment);

        let mut source = writable_source(&initial).await;
        let mut tags = TagMap::new();
        tags.set("TITLE", "Structured");
        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("write should succeed");

        let output = source.written_data().to_vec();

        let (_, ebml) = parse_element_header(&output).expect("ebml header");
        let seg_off = ebml.header_size + ebml.size as usize;
        let (_, seg) = parse_element_header(&output[seg_off..]).expect("segment header");
        let body_start = seg_off + seg.header_size;
        let body_end = body_start + seg.size as usize;

        let mut cursor = body_start;
        let mut saw_info = false;
        let mut saw_tags = false;
        while cursor < body_end {
            let (_, el) = parse_element_header(&output[cursor..]).expect("child header");
            if el.id == mkv_id::INFO {
                saw_info = true;
            }
            if el.id == mkv_id::TAGS {
                saw_tags = true;
            }
            cursor += el.header_size + el.size as usize;
        }
        // The cursor must land exactly on the segment body end (no corruption).
        assert_eq!(
            cursor, body_end,
            "segment children must tile the body exactly"
        );
        assert!(saw_info, "Info element must be preserved");
        assert!(saw_tags, "Tags element must be present");
    }

    #[tokio::test]
    async fn test_mkv_write_multivalue_tags_roundtrip() {
        // A key with multiple values must produce one SimpleTag per value.
        let initial = mkv_file(&mkv_info());
        let mut source = writable_source(&initial).await;

        let mut tags = TagMap::new();
        tags.add("ARTIST", "Artist One");
        tags.add("ARTIST", "Artist Two");
        tags.set("GENRE", "Soundtrack");

        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("write should succeed");

        let output = source.written_data().to_vec();
        let parsed = read_back_tags(&output).await;
        let artists: Vec<&str> = parsed
            .get_all("ARTIST")
            .iter()
            .filter_map(TagValue::as_text)
            .collect();
        assert_eq!(artists.len(), 2);
        assert!(artists.contains(&"Artist One"));
        assert!(artists.contains(&"Artist Two"));
        assert_eq!(parsed.get_text("GENRE"), Some("Soundtrack"));
    }

    #[tokio::test]
    async fn test_mkv_write_unicode_tag_values() {
        // UTF-8 multi-byte values must survive byte-exact.
        let initial = mkv_file(&mkv_info());
        let mut source = writable_source(&initial).await;

        let mut tags = TagMap::new();
        tags.set("TITLE", "日本語のタイトル");
        tags.set("ARTIST", "Æther — Œuvre ✦");

        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("write should succeed");

        let output = source.written_data().to_vec();
        let parsed = read_back_tags(&output).await;
        assert_eq!(parsed.get_text("TITLE"), Some("日本語のタイトル"));
        assert_eq!(parsed.get_text("ARTIST"), Some("Æther — Œuvre ✦"));
    }

    #[tokio::test]
    async fn test_mkv_write_rejects_non_writable_source() {
        let initial = mkv_file(&mkv_info());
        // from_vec yields a NON-writable source.
        let mut source = MemorySource::from_vec(initial);
        let tags = TagMap::new();
        let result = MatroskaMetadataWriter::write(&mut source, &tags).await;
        assert!(result.is_err(), "non-writable source must be rejected");
    }

    #[tokio::test]
    async fn test_mkv_write_rejects_non_ebml_input() {
        // Bytes that are not an EBML document must be rejected.
        let mut source = writable_source(b"NOT AN EBML FILE AT ALL........").await;
        let tags = TagMap::new();
        let result = MatroskaMetadataWriter::write(&mut source, &tags).await;
        assert!(result.is_err(), "non-EBML input must be rejected");
    }

    #[tokio::test]
    async fn test_mkv_write_idempotent_double_write() {
        // Writing the same tags twice must yield the same readable result and
        // must not duplicate or corrupt the Tags element.
        let initial = mkv_file(&mkv_info());
        let mut source = writable_source(&initial).await;

        let mut tags = TagMap::new();
        tags.set("TITLE", "Idempotent");

        MatroskaMetadataWriter::write(&mut source, &tags)
            .await
            .expect("first write should succeed");
        let after_first = source.written_data().to_vec();

        // Feed the first output back in and write again.
        let mut source2 = writable_source(&after_first).await;
        MatroskaMetadataWriter::write(&mut source2, &tags)
            .await
            .expect("second write should succeed");
        let after_second = source2.written_data().to_vec();

        let parsed = read_back_tags(&after_second).await;
        assert_eq!(parsed.get_text("TITLE"), Some("Idempotent"));
    }

    #[tokio::test]
    async fn test_mkv_write_file_roundtrip() {
        // Exercises the FileSource path end-to-end using a real temp file.
        use oximedia_io::source::FileSource;

        let mut path = std::env::temp_dir();
        path.push(format!(
            "oximedia_mkv_writer_test_{}.mkv",
            std::process::id()
        ));

        // Write a minimal MKV (with a pre-existing Tags element) to disk.
        let mut segment = Vec::new();
        segment.extend_from_slice(&mkv_info());
        segment.extend_from_slice(&mkv_tags(&[("TITLE", "Disk Old")]));
        let initial = mkv_file(&segment);
        tokio::fs::write(&path, &initial)
            .await
            .expect("temp file write should succeed");

        // Open the file read-write and rewrite the tags.
        {
            let file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .await
                .expect("temp file open should succeed");
            let mut source = FileSource::new_writable(file)
                .await
                .expect("writable FileSource should be created");

            let mut tags = TagMap::new();
            tags.set("TITLE", "Disk New");
            tags.set("ALBUM", "Disk Album");
            MatroskaMetadataWriter::write(&mut source, &tags)
                .await
                .expect("file write should succeed");
        }

        // Re-open and demux to verify the round-trip.
        let on_disk = tokio::fs::read(&path)
            .await
            .expect("temp file read should succeed");
        let parsed = read_back_tags(&on_disk).await;
        assert_eq!(parsed.get_text("TITLE"), Some("Disk New"));
        assert_eq!(parsed.get_text("ALBUM"), Some("Disk Album"));

        // Cleanup.
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_mkv_write_shrink_no_stale_bytes_on_file() {
        // Critical Void-padding test: a FileSource is NOT truncated on write, so
        // replacing a large Tags element with a tiny one must NOT leave stale
        // trailing bytes. The writer pads with a Void element so the file never
        // shrinks and the EBML structure stays self-consistent.
        use oximedia_io::source::FileSource;

        let mut path = std::env::temp_dir();
        path.push(format!(
            "oximedia_mkv_shrink_test_{}.mkv",
            std::process::id()
        ));

        // Build a file with a deliberately LARGE Tags element (many entries).
        let big_pairs: Vec<(String, String)> = (0..40)
            .map(|i| {
                (
                    format!("CUSTOM_FIELD_NUMBER_{i:03}"),
                    format!("a fairly long stale value for field {i} ........................"),
                )
            })
            .collect();
        let big_refs: Vec<(&str, &str)> = big_pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut segment = Vec::new();
        segment.extend_from_slice(&mkv_info());
        segment.extend_from_slice(&mkv_tags(&big_refs));
        let initial = mkv_file(&segment);
        let initial_len = initial.len();
        tokio::fs::write(&path, &initial)
            .await
            .expect("temp file write should succeed");

        // Replace with a single tiny tag — the new content is much shorter.
        {
            let file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .await
                .expect("temp file open should succeed");
            let mut source = FileSource::new_writable(file)
                .await
                .expect("writable FileSource should be created");
            let mut tags = TagMap::new();
            tags.set("TITLE", "tiny");
            MatroskaMetadataWriter::write(&mut source, &tags)
                .await
                .expect("shrink write should succeed");
        }

        let on_disk = tokio::fs::read(&path)
            .await
            .expect("temp file read should succeed");
        // The padded file must be at least as long as the original (Void fill).
        assert!(
            on_disk.len() >= initial_len,
            "padded file ({}) must not be shorter than original ({initial_len})",
            on_disk.len()
        );

        // The whole file must still parse: EBML header + bounded Segment whose
        // children tile the body exactly with no stale bytes.
        let (_, ebml) = parse_element_header(&on_disk).expect("ebml header");
        let seg_off = ebml.header_size + ebml.size as usize;
        let (_, seg) = parse_element_header(&on_disk[seg_off..]).expect("segment header");
        let body_start = seg_off + seg.header_size;
        let body_end = body_start + seg.size as usize;
        assert_eq!(
            body_end,
            on_disk.len(),
            "Segment must span exactly to end of file"
        );
        let mut cursor = body_start;
        while cursor < body_end {
            let (_, el) = parse_element_header(&on_disk[cursor..]).expect("every child must parse");
            cursor += el.header_size + el.size as usize;
        }
        assert_eq!(cursor, body_end, "children must tile the segment exactly");

        // The tags must read back as exactly the tiny replacement.
        let parsed = read_back_tags(&on_disk).await;
        assert_eq!(parsed.get_text("TITLE"), Some("tiny"));
        assert_eq!(parsed.len(), 1, "only the replacement tag must remain");

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[test]
    fn test_mkv_build_void_filler_exact_length() {
        // The Void filler must always have a total length equal to the request
        // (clamped to a 2-byte minimum) and must parse as a Void element.
        for total in [2usize, 3, 4, 10, 100, 130, 200, 17_000] {
            let void = MatroskaMetadataWriter::build_void_filler(total);
            assert_eq!(void.len(), total, "Void filler length mismatch for {total}");
            let (rest, el) = parse_element_header(&void).expect("Void must parse");
            assert_eq!(el.id, mkv_id::VOID);
            assert_eq!(
                el.header_size + el.size as usize,
                total,
                "Void element total size must equal request"
            );
            assert_eq!(rest.len(), el.size as usize);
        }
    }

    #[test]
    fn test_mkv_build_tags_body_is_parseable() {
        // The Tags body produced by build_tags_body must round-trip through the
        // demuxer's parse_tags.
        let mut tags = TagMap::new();
        tags.set("TITLE", "Parseable");
        tags.set("ARTIST", "Body Test");

        let body = build_tags_body(&tags);
        // parse_tags expects the Tags *body* and its size.
        let parsed = crate::demux::matroska::parser::parse_tags(&body, body.len() as u64)
            .expect("Tags body must parse");
        let map = crate::metadata::reader::MatroskaMetadataReader::convert_tags(&parsed);
        assert_eq!(map.get_text("TITLE"), Some("Parseable"));
        assert_eq!(map.get_text("ARTIST"), Some("Body Test"));
    }
}

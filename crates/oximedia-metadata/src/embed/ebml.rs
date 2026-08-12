//! EBML-aware Matroska `Tags` embedding.
//!
//! A Matroska (or `WebM`) file is an EBML document: an `EBML` header element
//! followed by a `Segment` whose children are the level-1 elements `SeekHead`,
//! `Info`, `Tracks`, `Cues`, `Cluster`, `Tags`, … Embedding tags therefore means
//! splicing a `Tags` master element into the `Segment` and repairing everything
//! that describes byte positions — which is exactly what makes naive
//! concatenation produce an unreadable file.
//!
//! # Strategy
//!
//! 1. The `EBML` header is copied verbatim; the `Segment` is rebuilt.
//! 2. Level-1 children are walked. Pre-existing `Tags` elements are dropped (so
//!    the result carries exactly one), and level-1 `Void` padding is reclaimed.
//! 3. The new `Tags` element is inserted **immediately before the first
//!    `Cluster`**, the layout muxers use and the only one a demuxer that stops
//!    scanning at the first cluster can find.
//! 4. That insertion shifts every following byte, so all stored positions are
//!    recomputed rather than patched: `SeekHead/Seek/SeekPosition`,
//!    `Cues/…/CueClusterPosition` and `Cluster/Position` are re-emitted as
//!    fixed-width 8-byte unsigned integers (legal for any EBML unsigned integer,
//!    which may be 0–8 octets). Fixing the width makes every element's size
//!    independent of the values, so the final layout is known before the
//!    positions are computed and one pass suffices — no fixpoint iteration, no
//!    stale offsets.
//! 5. Old positions are mapped to new ones through the level-1 element table, so
//!    a `SeekHead` entry that pointed at `Tracks` still points at `Tracks`.
//!    A `Seek` entry that pointed at the old `Tags` is retargeted to the new one,
//!    and a `SeekHead` that had no `Tags` entry gains one.
//! 6. The `Segment` size VINT is recomputed for the rebuilt body.
//!
//! # Unknown-size elements
//!
//! A `Segment` written with an *unknown* (unbounded) size — the streaming layout
//! — is rewritten with a known size, since the output is a complete file.
//! Unknown-size `Cluster`s are common (single-pass muxers, including this
//! workspace's own, always write them) and are walked properly: per EBML, such
//! an element ends where an element appears that cannot be one of its children,
//! i.e. at the next level-1 element. Their header, unbounded size VINT included,
//! is preserved verbatim.
//!
//! # Limits
//!
//! * An unbounded element that is *not* a `Cluster` has no defined extent here
//!   and is reported as an honest error instead of being guessed at.
//! * An EBML `CRC-32` element inside a master this module has to rewrite is
//!   reported as an honest error rather than left stale.

use crate::{Error, Metadata, MetadataValue};

/// First four bytes of every EBML document (the `EBML` header element ID).
pub(super) const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// Matroska element IDs, in the pre-encoded form EBML stores them in.
mod id {
    /// `EBML` header element.
    pub const EBML: u32 = 0x1A45_DFA3;
    /// `Segment` — the top-level container for everything else.
    pub const SEGMENT: u32 = 0x1853_8067;
    /// `SeekHead` — the optional index of level-1 element positions.
    pub const SEEK_HEAD: u32 = 0x114D_9B74;
    /// One `Seek` entry inside a `SeekHead`.
    pub const SEEK: u32 = 0x4DBB;
    /// `SeekID` — the element ID a `Seek` entry points at.
    pub const SEEK_ID: u32 = 0x53AB;
    /// `SeekPosition` — byte offset relative to the start of the segment body.
    pub const SEEK_POSITION: u32 = 0x53AC;
    /// `Cluster` — a run of frames.
    pub const CLUSTER: u32 = 0x1F43_B675;
    /// `Position` — a cluster's own offset in the segment (deprecated but legal).
    pub const POSITION: u32 = 0xA7;
    /// `Cues` — the seek index.
    pub const CUES: u32 = 0x1C53_BB6B;
    /// `CuePoint` — one cue entry.
    pub const CUE_POINT: u32 = 0xBB;
    /// `CueTrackPositions` — per-track positions of a cue point.
    pub const CUE_TRACK_POSITIONS: u32 = 0xB7;
    /// `CueReference` — reference to another cue.
    pub const CUE_REFERENCE: u32 = 0xDB;
    /// `CueClusterPosition` — byte offset of the cluster a cue points at.
    pub const CUE_CLUSTER_POSITION: u32 = 0xF1;
    /// `Tags` — the tag container this module writes.
    pub const TAGS: u32 = 0x1254_C367;
    /// One `Tag` inside `Tags`.
    pub const TAG: u32 = 0x7373;
    /// `Targets` — what a `Tag` applies to.
    pub const TARGETS: u32 = 0x63C0;
    /// One `SimpleTag` name/value pair.
    pub const SIMPLE_TAG: u32 = 0x67C8;
    /// `TagName`.
    pub const TAG_NAME: u32 = 0x45A3;
    /// `TagString`.
    pub const TAG_STRING: u32 = 0x4487;
    /// `Void` — reclaimable padding.
    pub const VOID: u32 = 0xEC;
    /// `CRC-32` — a checksum over a master element's content.
    pub const CRC32: u32 = 0xBF;
}

/// Master elements this module descends into. Everything else — including
/// `Info`, `Tracks` and the frame data inside a `Cluster` — is opaque and copied
/// byte for byte.
const RECURSED_MASTERS: [u32; 6] = [
    id::SEEK_HEAD,
    id::CUES,
    id::CUE_POINT,
    id::CUE_TRACK_POSITIONS,
    id::CUE_REFERENCE,
    id::CLUSTER,
];

/// Level-1 element IDs — the children a `Segment` may contain.
///
/// Encountering one of these ends an unknown-size `Cluster`, which is how the
/// EBML specification says an unbounded element's extent is determined: it runs
/// until an element appears that cannot be one of its children.
const LEVEL1_IDS: [u32; 8] = [
    id::SEEK_HEAD,
    0x1549_A966, // Info
    id::CLUSTER,
    0x1654_AE6B, // Tracks
    id::CUES,
    0x1941_A469, // Attachments
    0x1043_A770, // Chapters
    id::TAGS,
];

/// Element IDs that may appear as direct children of a `Cluster`.
const CLUSTER_CHILD_IDS: [u32; 9] = [
    0xE7,   // Timestamp (a.k.a. Timecode)
    0x5854, // SilentTracks
    id::POSITION,
    0xAB, // PrevSize
    0xA3, // SimpleBlock
    0xA0, // BlockGroup
    0xAF, // EncryptedBlock
    id::VOID,
    id::CRC32,
];

/// Byte width every rewritten position field is emitted with.
const POSITION_WIDTH: usize = 8;

/// What a position slot must be filled with once the final layout is known.
#[derive(Clone, Copy)]
enum SlotTarget {
    /// The position that the original file stored here.
    OldPosition(u64),
    /// The offset of the freshly written `Tags` element.
    NewTags,
}

/// A fixed-width position field awaiting its final value.
struct Slot {
    /// Offset of the field's first byte, relative to the element it lives in.
    offset: usize,
    /// What the field must end up pointing at.
    target: SlotTarget,
}

/// One level-1 child of the `Segment` in the rebuilt file.
struct Level1 {
    /// Element ID (used to find the first `Cluster` and the `SeekHead`).
    element_id: u32,
    /// Final bytes of the element (sizes already correct, positions pending).
    bytes: Vec<u8>,
    /// Position fields inside [`Self::bytes`].
    slots: Vec<Slot>,
    /// Offset in the *original* segment body, or `None` for the new `Tags`.
    old_offset: Option<u64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// EBML primitives
// ─────────────────────────────────────────────────────────────────────────────

/// Parses an element ID at `data[pos..]`, returning it and its byte width.
///
/// EBML element IDs keep their length-marker bits, so the returned value is the
/// same constant the specification lists (e.g. `Segment` → `0x1853_8067`).
fn parse_id(data: &[u8], pos: usize) -> Result<(u32, usize), Error> {
    let first = *data
        .get(pos)
        .ok_or_else(|| Error::ParseError("EBML: element ID past end of data".to_string()))?;
    if first == 0 {
        return Err(Error::ParseError(
            "EBML: invalid element ID (no length marker in the first byte)".to_string(),
        ));
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return Err(Error::ParseError(format!(
            "EBML: element ID at offset {pos} claims {len} bytes (maximum is 4)"
        )));
    }
    if pos + len > data.len() {
        return Err(Error::ParseError("EBML: truncated element ID".to_string()));
    }
    let mut value = 0u32;
    for byte in &data[pos..pos + len] {
        value = (value << 8) | u32::from(*byte);
    }
    Ok((value, len))
}

/// Parses an element size VINT at `data[pos..]`.
///
/// Returns `(None, width)` for the "unknown size" sentinel (all data bits set).
fn parse_size(data: &[u8], pos: usize) -> Result<(Option<u64>, usize), Error> {
    let first = *data
        .get(pos)
        .ok_or_else(|| Error::ParseError("EBML: element size past end of data".to_string()))?;
    if first == 0 {
        return Err(Error::ParseError(
            "EBML: invalid element size (no length marker in the first byte)".to_string(),
        ));
    }
    let len = first.leading_zeros() as usize + 1;
    if pos + len > data.len() {
        return Err(Error::ParseError(
            "EBML: truncated element size".to_string(),
        ));
    }
    // An 8-byte VINT has no data bits left in its first byte, and `0xFF >> 8`
    // would overflow, so that width is masked to zero explicitly.
    let mask = if len >= 8 { 0u8 } else { 0xFFu8 >> len };
    let mut value = u64::from(first & mask);
    for byte in &data[pos + 1..pos + len] {
        value = (value << 8) | u64::from(*byte);
    }
    // The all-ones data pattern is the "unknown size" sentinel.
    let capacity = if 7 * len >= 64 {
        u64::MAX
    } else {
        (1u64 << (7 * len)) - 1
    };
    Ok((if value == capacity { None } else { Some(value) }, len))
}

/// Encodes an element ID back to its on-wire bytes.
fn encode_id(element_id: u32) -> Vec<u8> {
    if element_id <= 0xFF {
        vec![element_id as u8]
    } else if element_id <= 0xFFFF {
        vec![(element_id >> 8) as u8, element_id as u8]
    } else if element_id <= 0x00FF_FFFF {
        vec![
            (element_id >> 16) as u8,
            (element_id >> 8) as u8,
            element_id as u8,
        ]
    } else {
        vec![
            (element_id >> 24) as u8,
            (element_id >> 16) as u8,
            (element_id >> 8) as u8,
            element_id as u8,
        ]
    }
}

/// Number of bytes needed to encode `value` as a size VINT.
///
/// An `n`-byte VINT carries `7n` data bits, but the all-ones pattern is the
/// unknown-size sentinel, so the largest representable value is `2^(7n) - 2`.
fn size_width(value: u64) -> usize {
    for len in 1usize..=8 {
        let bits = 7 * len;
        let capacity = if bits >= 64 {
            u64::MAX - 1
        } else {
            (1u64 << bits) - 2
        };
        if value <= capacity {
            return len;
        }
    }
    8
}

/// Encodes `value` as a minimal-width element size VINT.
fn encode_size(value: u64) -> Vec<u8> {
    let len = size_width(value);
    let mut out = vec![0u8; len];
    let mut remaining = value;
    for byte in out.iter_mut().rev() {
        *byte = (remaining & 0xFF) as u8;
        remaining >>= 8;
    }
    out[0] |= 1u8 << (8 - len);
    out
}

/// Appends a complete element (`ID` + size VINT + payload).
fn push_element(out: &mut Vec<u8>, element_id: u32, payload: &[u8]) {
    out.extend_from_slice(&encode_id(element_id));
    out.extend_from_slice(&encode_size(payload.len() as u64));
    out.extend_from_slice(payload);
}

/// Reads an EBML unsigned integer (big-endian, 0–8 octets).
fn read_uint(data: &[u8]) -> u64 {
    let mut value = 0u64;
    for byte in data.iter().take(8) {
        value = (value << 8) | u64::from(*byte);
    }
    value
}

/// Parses an element header at `data[pos..]`: `(id, header width, body size)`.
fn element_header(data: &[u8], pos: usize) -> Result<(u32, usize, Option<u64>), Error> {
    let (element_id, id_len) = parse_id(data, pos)?;
    let (size, size_len) = parse_size(data, pos + id_len)?;
    Ok((element_id, id_len + size_len, size))
}

/// Returns the total on-wire length of the element at `data[pos..]`.
fn element_len(data: &[u8], pos: usize) -> Result<usize, Error> {
    let (element_id, header_len, size) = element_header(data, pos)?;
    let size = size.ok_or_else(|| {
        Error::Unsupported(format!(
            "embed(Matroska): element 0x{element_id:X} at offset {pos} has an unknown (unbounded) \
             size — a streaming layout that cannot be rewritten into a complete file without \
             guessing where it ends"
        ))
    })?;
    let size = usize::try_from(size).map_err(|_| {
        Error::ParseError("EBML: element size exceeds the addressable range".to_string())
    })?;
    header_len
        .checked_add(size)
        .filter(|end| pos + *end <= data.len())
        .ok_or_else(|| {
            Error::ParseError(format!(
                "EBML: element 0x{element_id:X} at offset {pos} extends past the end of the data"
            ))
        })
}

// ─────────────────────────────────────────────────────────────────────────────
// Position-preserving rewrite
// ─────────────────────────────────────────────────────────────────────────────

/// Rewrites one element so every position field it contains becomes a
/// fixed-width slot, or returns `None` when the element contains none and can be
/// copied verbatim.
///
/// `element` must be a complete element (ID + size + body). Returned slot
/// offsets are relative to the start of the returned bytes.
fn rewrite(element: &[u8]) -> Result<Option<(Vec<u8>, Vec<Slot>)>, Error> {
    let (element_id, header_len, size) = element_header(element, 0)?;
    let size = size.ok_or_else(|| {
        Error::Unsupported(format!(
            "embed(Matroska): element 0x{element_id:X} has an unknown (unbounded) size and cannot \
             be rewritten safely"
        ))
    })?;
    let body_end = header_len
        + usize::try_from(size).map_err(|_| {
            Error::ParseError("EBML: element size exceeds the addressable range".to_string())
        })?;
    if body_end > element.len() {
        return Err(Error::ParseError(
            "EBML: element extends past the end of its parent".to_string(),
        ));
    }
    let body = &element[header_len..body_end];

    // A position field: re-emit at a fixed width and record where to fill it in.
    if element_id == id::CUE_CLUSTER_POSITION || element_id == id::POSITION {
        let old = read_uint(body);
        let (bytes, offset) = fixed_position_element(element_id);
        return Ok(Some((
            bytes,
            vec![Slot {
                offset,
                target: SlotTarget::OldPosition(old),
            }],
        )));
    }
    if element_id == id::SEEK {
        return Ok(Some(rewrite_seek(body)?));
    }
    if !RECURSED_MASTERS.contains(&element_id) {
        return Ok(None);
    }

    let mut new_body: Vec<u8> = Vec::with_capacity(body.len());
    let mut slots: Vec<Slot> = Vec::new();
    let mut changed = false;
    let mut has_crc = false;
    let mut pos = 0usize;
    while pos < body.len() {
        let child_len = element_len(body, pos)?;
        let (child_id, _, _) = element_header(body, pos)?;
        if child_id == id::CRC32 {
            has_crc = true;
        }
        let child = &body[pos..pos + child_len];
        match rewrite(child)? {
            Some((bytes, child_slots)) => {
                changed = true;
                let base = new_body.len();
                new_body.extend_from_slice(&bytes);
                slots.extend(child_slots.into_iter().map(|slot| Slot {
                    offset: slot.offset + base,
                    target: slot.target,
                }));
            }
            None => new_body.extend_from_slice(child),
        }
        pos += child_len;
    }

    if !changed {
        return Ok(None);
    }
    if has_crc {
        return Err(Error::Unsupported(format!(
            "embed(Matroska): element 0x{element_id:X} carries an EBML CRC-32 child and also \
             stores byte positions that this embed must update; recomputing that checksum is not \
             implemented, and leaving it stale would produce a file readers reject"
        )));
    }

    let mut out = Vec::with_capacity(header_len + new_body.len());
    out.extend_from_slice(&encode_id(element_id));
    out.extend_from_slice(&encode_size(new_body.len() as u64));
    let shift = out.len();
    out.extend_from_slice(&new_body);
    for slot in &mut slots {
        slot.offset += shift;
    }
    Ok(Some((out, slots)))
}

/// Finds where an unknown-size `Cluster` ends.
///
/// EBML gives an unbounded element's extent implicitly: it runs until an element
/// appears that cannot be one of its children. For a `Cluster` that means the
/// next level-1 element (another `Cluster`, `Cues`, `Tags`, …) or the end of the
/// `Segment`. This is the layout single-pass muxers write — including this
/// workspace's own [`MatroskaMuxer`](https://docs.rs/oximedia-container) — so it
/// has to be walked, not rejected.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if an element inside the cluster is neither a
/// known cluster child nor a level-1 element, since the extent would then be a
/// guess.
fn unbounded_cluster_end(data: &[u8], body_start: usize, limit: usize) -> Result<usize, Error> {
    let mut pos = body_start;
    while pos < limit {
        let (child_id, child_header_len, child_size) = element_header(data, pos)?;
        if LEVEL1_IDS.contains(&child_id) {
            return Ok(pos);
        }
        if !CLUSTER_CHILD_IDS.contains(&child_id) {
            return Err(Error::ParseError(format!(
                "Matroska: an unknown-size Cluster contains element 0x{child_id:X} at offset \
                 {pos}, which is neither a Cluster child nor a level-1 element, so where the \
                 cluster ends cannot be determined"
            )));
        }
        let child_size = child_size.ok_or_else(|| {
            Error::Unsupported(format!(
                "embed(Matroska): element 0x{child_id:X} inside an unknown-size Cluster also has \
                 an unknown size, which leaves the cluster's extent undefined"
            ))
        })?;
        let child_size = usize::try_from(child_size).map_err(|_| {
            Error::ParseError("EBML: element size exceeds the addressable range".to_string())
        })?;
        pos = pos
            .checked_add(child_header_len + child_size)
            .filter(|end| *end <= limit)
            .ok_or_else(|| {
                Error::ParseError(
                    "Matroska: a Cluster child extends past the end of the Segment".to_string(),
                )
            })?;
    }
    Ok(limit)
}

/// Rewrites an unknown-size `Cluster`: its header (including the unbounded size
/// VINT) is preserved verbatim and its children are rewritten individually.
///
/// Because the size stays "unknown" there is no size field to recompute, so a
/// widened `Position` child changes only the element's length — which the
/// caller's layout pass accounts for.
fn rewrite_unbounded_cluster(
    element: &[u8],
    header_len: usize,
) -> Result<(Vec<u8>, Vec<Slot>), Error> {
    let mut out = element[..header_len].to_vec();
    let mut slots: Vec<Slot> = Vec::new();
    let mut changed = false;
    let mut has_crc = false;
    let mut pos = header_len;
    while pos < element.len() {
        let child_len = element_len(element, pos)?;
        let (child_id, _, _) = element_header(element, pos)?;
        if child_id == id::CRC32 {
            has_crc = true;
        }
        let child = &element[pos..pos + child_len];
        match rewrite(child)? {
            Some((bytes, child_slots)) => {
                changed = true;
                let base = out.len();
                out.extend_from_slice(&bytes);
                slots.extend(child_slots.into_iter().map(|slot| Slot {
                    offset: slot.offset + base,
                    target: slot.target,
                }));
            }
            None => out.extend_from_slice(child),
        }
        pos += child_len;
    }
    if changed && has_crc {
        return Err(Error::Unsupported(
            "embed(Matroska): an unknown-size Cluster carries an EBML CRC-32 and also stores a \
             byte position that this embed must update; recomputing that checksum is not \
             implemented, and leaving it stale would produce a file readers reject"
                .to_string(),
        ));
    }
    Ok((out, slots))
}

/// Builds a position element of exactly [`POSITION_WIDTH`] bytes, returning the
/// bytes and the offset of the value field awaiting its final position.
fn fixed_position_element(element_id: u32) -> (Vec<u8>, usize) {
    let mut out = encode_id(element_id);
    out.extend_from_slice(&encode_size(POSITION_WIDTH as u64));
    let offset = out.len();
    out.extend_from_slice(&[0u8; POSITION_WIDTH]);
    (out, offset)
}

/// Rewrites a `Seek` entry, retargeting an entry that points at the old `Tags`
/// element to the newly written one.
fn rewrite_seek(body: &[u8]) -> Result<(Vec<u8>, Vec<Slot>), Error> {
    // First pass: which element does this entry index?
    let mut seek_id: Option<u32> = None;
    let mut pos = 0usize;
    while pos < body.len() {
        let child_len = element_len(body, pos)?;
        let (child_id, child_header, _) = element_header(body, pos)?;
        if child_id == id::SEEK_ID {
            let payload = &body[pos + child_header..pos + child_len];
            let mut value = 0u32;
            for byte in payload.iter().take(4) {
                value = (value << 8) | u32::from(*byte);
            }
            seek_id = Some(value);
        }
        pos += child_len;
    }

    // Second pass: rebuild, widening SeekPosition into a slot.
    let mut new_body: Vec<u8> = Vec::with_capacity(body.len() + POSITION_WIDTH);
    let mut slots: Vec<Slot> = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let child_len = element_len(body, pos)?;
        let (child_id, child_header, _) = element_header(body, pos)?;
        if child_id == id::SEEK_POSITION {
            let old = read_uint(&body[pos + child_header..pos + child_len]);
            let target = if seek_id == Some(id::TAGS) {
                SlotTarget::NewTags
            } else {
                SlotTarget::OldPosition(old)
            };
            let (bytes, offset) = fixed_position_element(id::SEEK_POSITION);
            let base = new_body.len();
            new_body.extend_from_slice(&bytes);
            slots.push(Slot {
                offset: base + offset,
                target,
            });
        } else {
            new_body.extend_from_slice(&body[pos..pos + child_len]);
        }
        pos += child_len;
    }

    let mut out = encode_id(id::SEEK);
    out.extend_from_slice(&encode_size(new_body.len() as u64));
    let shift = out.len();
    out.extend_from_slice(&new_body);
    for slot in &mut slots {
        slot.offset += shift;
    }
    Ok((out, slots))
}

/// Builds a `Seek` entry indexing the `Tags` element, plus the offset of its
/// still-unfilled `SeekPosition` slot.
fn build_tags_seek_entry() -> (Vec<u8>, usize) {
    let mut body = Vec::new();
    push_element(&mut body, id::SEEK_ID, &encode_id(id::TAGS));
    let position_start = body.len();
    let (position_element, slot_offset_in_element) = fixed_position_element(id::SEEK_POSITION);
    body.extend_from_slice(&position_element);

    let mut out = encode_id(id::SEEK);
    out.extend_from_slice(&encode_size(body.len() as u64));
    let header_len = out.len();
    out.extend_from_slice(&body);
    (out, header_len + position_start + slot_offset_in_element)
}

/// `true` when a `SeekHead` body already contains an entry indexing `Tags`.
fn seek_head_indexes_tags(body: &[u8]) -> Result<bool, Error> {
    let mut pos = 0usize;
    while pos < body.len() {
        let entry_len = element_len(body, pos)?;
        let (entry_id, entry_header, _) = element_header(body, pos)?;
        if entry_id == id::SEEK {
            let entry_body = &body[pos + entry_header..pos + entry_len];
            let mut inner = 0usize;
            while inner < entry_body.len() {
                let child_len = element_len(entry_body, inner)?;
                let (child_id, child_header, _) = element_header(entry_body, inner)?;
                if child_id == id::SEEK_ID {
                    let payload = &entry_body[inner + child_header..inner + child_len];
                    let mut value = 0u32;
                    for byte in payload.iter().take(4) {
                        value = (value << 8) | u32::from(*byte);
                    }
                    if value == id::TAGS {
                        return Ok(true);
                    }
                }
                inner += child_len;
            }
        }
        pos += entry_len;
    }
    Ok(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tags element construction
// ─────────────────────────────────────────────────────────────────────────────

/// Renders a metadata value as a Matroska `TagString`, or `None` when the value
/// has no textual form (pictures and binary blobs belong in `TagBinary`, which
/// this writer does not emit).
fn tag_string(value: &MetadataValue) -> Option<Vec<String>> {
    match value {
        MetadataValue::Text(text) | MetadataValue::DateTime(text) => Some(vec![text.clone()]),
        MetadataValue::TextList(items) => Some(items.clone()),
        MetadataValue::Integer(number) => Some(vec![number.to_string()]),
        MetadataValue::Float(number) => Some(vec![number.to_string()]),
        MetadataValue::Boolean(flag) => Some(vec![if *flag { "1" } else { "0" }.to_string()]),
        MetadataValue::Binary(_) | MetadataValue::Picture(_) | MetadataValue::Pictures(_) => None,
    }
}

/// Builds a complete `Tags` element from `metadata`.
///
/// One `Tag` with an empty `Targets` (which scopes it to the whole segment — the
/// right scope for file-level metadata) holds one `SimpleTag` per value; a
/// multi-valued field emits one `SimpleTag` per value.
///
/// Note that this deliberately does **not** call [`crate::matroska::write`]:
/// that function emits the Matroska *XML* tag interchange format, which is a
/// sidecar representation, not the binary EBML elements a `.mkv` file stores.
///
/// # Errors
///
/// Returns [`Error::WriteError`] if `metadata` has fields but none of them could
/// be rendered as a `TagString` — writing an empty `Tags` element and reporting
/// success would claim work that did not happen.
fn build_tags_element(metadata: &Metadata) -> Result<Vec<u8>, Error> {
    let mut keys: Vec<&String> = metadata.fields().keys().collect();
    keys.sort_unstable();

    let mut tag_body = Vec::new();
    push_element(&mut tag_body, id::TARGETS, &[]);
    let mut written = 0usize;
    let mut rejected: Vec<&str> = Vec::new();
    for key in keys {
        let Some(value) = metadata.get(key) else {
            continue;
        };
        let Some(strings) = tag_string(value) else {
            rejected.push(key.as_str());
            continue;
        };
        for text in strings {
            let mut simple_tag = Vec::new();
            push_element(&mut simple_tag, id::TAG_NAME, key.as_bytes());
            push_element(&mut simple_tag, id::TAG_STRING, text.as_bytes());
            push_element(&mut tag_body, id::SIMPLE_TAG, &simple_tag);
            written += 1;
        }
    }
    if written == 0 && !metadata.fields().is_empty() {
        return Err(Error::WriteError(format!(
            "embed(Matroska): none of the {} field(s) could be written as a Matroska SimpleTag, \
             so nothing would have been embedded. Binary and picture values need a `TagBinary` \
             element, which this writer does not emit. Rejected: {rejected:?}",
            metadata.fields().len()
        )));
    }

    let mut tags_body = Vec::new();
    push_element(&mut tags_body, id::TAG, &tag_body);

    let mut tags_element = Vec::new();
    push_element(&mut tags_element, id::TAGS, &tags_body);
    Ok(tags_element)
}

// ─────────────────────────────────────────────────────────────────────────────
// Embed
// ─────────────────────────────────────────────────────────────────────────────

/// Embeds `metadata` as the `Tags` element of a Matroska/`WebM` file.
///
/// See the [module documentation](self) for the full strategy. The input is
/// never modified: on any error the caller still holds its original bytes.
///
/// # Errors
///
/// * [`Error::Unsupported`] if `file_data` is not an EBML document, if it uses
///   unbounded elements inside the segment, or if a master element that stores
///   positions also carries an EBML `CRC-32`.
/// * [`Error::ParseError`] if the EBML structure is malformed or truncated.
pub fn embed(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    if file_data.len() < 4 || file_data[..4] != EBML_MAGIC {
        return Err(Error::Unsupported(
            "embed(Matroska) targets a Matroska/WebM (EBML) document; the given file_data does \
             not start with the EBML header magic 1A 45 DF A3, and appending a Tags element to \
             non-EBML bytes would corrupt them"
                .to_string(),
        ));
    }

    // ── EBML header (copied verbatim) ────────────────────────────────────────
    let (header_id, _, _) = element_header(file_data, 0)?;
    if header_id != id::EBML {
        return Err(Error::ParseError(format!(
            "Matroska: expected an EBML header element, found 0x{header_id:X}"
        )));
    }
    let segment_offset = element_len(file_data, 0)?;

    // ── Segment ──────────────────────────────────────────────────────────────
    let (segment_id, segment_header_len, segment_size) = element_header(file_data, segment_offset)?;
    if segment_id != id::SEGMENT {
        return Err(Error::ParseError(format!(
            "Matroska: expected a Segment element after the EBML header, found 0x{segment_id:X}"
        )));
    }
    let (_, segment_id_len) = parse_id(file_data, segment_offset)?;
    let body_start = segment_offset + segment_header_len;
    let body_end = match segment_size {
        Some(size) => {
            let size = usize::try_from(size).map_err(|_| {
                Error::ParseError(
                    "Matroska: Segment size exceeds the addressable range".to_string(),
                )
            })?;
            let end = body_start
                .checked_add(size)
                .ok_or_else(|| Error::ParseError("Matroska: Segment size overflows".to_string()))?;
            if end > file_data.len() {
                return Err(Error::ParseError(
                    "Matroska: Segment extends past the end of the file".to_string(),
                ));
            }
            end
        }
        // An unbounded Segment runs to end-of-file; the rewritten output always
        // gets a known size, because it is a complete file rather than a stream.
        None => file_data.len(),
    };

    // ── Walk the level-1 children ────────────────────────────────────────────
    let mut children: Vec<Level1> = Vec::new();
    let mut cursor = body_start;
    while cursor < body_end {
        let (child_id, child_header_len, child_size) = element_header(file_data, cursor)?;
        // An unknown-size Cluster (the single-pass muxer layout) ends where the
        // next level-1 element begins; anything else unbounded is a genuine
        // stream that cannot be turned into a complete file.
        let total = match child_size {
            Some(size) => {
                let size = usize::try_from(size).map_err(|_| {
                    Error::ParseError(
                        "EBML: element size exceeds the addressable range".to_string(),
                    )
                })?;
                child_header_len + size
            }
            None if child_id == id::CLUSTER => {
                unbounded_cluster_end(file_data, cursor + child_header_len, body_end)? - cursor
            }
            None => {
                return Err(Error::Unsupported(format!(
                    "embed(Matroska): level-1 element 0x{child_id:X} at offset {cursor} has an \
                     unknown (unbounded) size and is not a Cluster, so where it ends cannot be \
                     determined without guessing"
                )));
            }
        };
        if cursor + total > body_end {
            return Err(Error::ParseError(
                "Matroska: a level-1 element extends past the end of the Segment".to_string(),
            ));
        }
        let old_offset = (cursor - body_start) as u64;
        let child = &file_data[cursor..cursor + total];
        cursor += total;

        // Old Tags are replaced, level-1 Void padding is reclaimed.
        if child_id == id::TAGS || child_id == id::VOID {
            continue;
        }
        let (bytes, slots) = if child_size.is_none() {
            rewrite_unbounded_cluster(child, child_header_len)?
        } else {
            match rewrite(child)? {
                Some(rewritten) => rewritten,
                None => (child.to_vec(), Vec::new()),
            }
        };
        children.push(Level1 {
            element_id: child_id,
            bytes,
            slots,
            old_offset: Some(old_offset),
        });
    }

    // ── Make sure the SeekHead indexes the Tags element ──────────────────────
    if let Some(index) = children
        .iter()
        .position(|child| child.element_id == id::SEEK_HEAD)
    {
        let original = file_data
            .get(body_start..body_end)
            .and_then(|body| find_original_seek_head(body).ok().flatten());
        let already_indexed = match original {
            Some(body) => seek_head_indexes_tags(&body)?,
            None => false,
        };
        if !already_indexed {
            append_tags_seek_entry(&mut children[index])?;
        }
    }

    // ── Splice the new Tags element in front of the first Cluster ────────────
    let tags_element = build_tags_element(metadata)?;
    let insert_at = children
        .iter()
        .position(|child| child.element_id == id::CLUSTER)
        .unwrap_or(children.len());
    children.insert(
        insert_at,
        Level1 {
            element_id: id::TAGS,
            bytes: tags_element,
            slots: Vec::new(),
            old_offset: None,
        },
    );

    // ── Compute the final layout, then fill in every position slot ───────────
    let mut new_offsets: Vec<u64> = Vec::with_capacity(children.len());
    let mut running = 0u64;
    for child in &children {
        new_offsets.push(running);
        running += child.bytes.len() as u64;
    }
    let tags_offset = new_offsets[insert_at];

    // old → new offset table for the elements that survived, in file order.
    let mapping: Vec<(u64, u64)> = children
        .iter()
        .zip(&new_offsets)
        .filter_map(|(child, new)| child.old_offset.map(|old| (old, *new)))
        .collect();

    for child in &mut children {
        for slot in &child.slots {
            let value = match slot.target {
                SlotTarget::NewTags => tags_offset,
                SlotTarget::OldPosition(old) => remap_position(&mapping, old),
            };
            child.bytes[slot.offset..slot.offset + POSITION_WIDTH]
                .copy_from_slice(&value.to_be_bytes());
        }
    }

    // ── Emit ─────────────────────────────────────────────────────────────────
    let body_len: usize = children.iter().map(|child| child.bytes.len()).sum();
    let size_vint = encode_size(body_len as u64);
    let mut out = Vec::with_capacity(segment_offset + segment_id_len + size_vint.len() + body_len);
    out.extend_from_slice(&file_data[..segment_offset + segment_id_len]);
    out.extend_from_slice(&size_vint);
    for child in &children {
        out.extend_from_slice(&child.bytes);
    }
    Ok(out)
}

/// Returns the body of the first level-1 `SeekHead` in `segment_body`.
///
/// The walk stops at the first unbounded element (an unknown-size `Cluster`):
/// a `SeekHead` never follows the media data, so nothing is missed.
fn find_original_seek_head(segment_body: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let mut pos = 0usize;
    while pos < segment_body.len() {
        let (element_id, header_len, size) = element_header(segment_body, pos)?;
        let Some(size) = size else {
            return Ok(None);
        };
        let size = usize::try_from(size).map_err(|_| {
            Error::ParseError("EBML: element size exceeds the addressable range".to_string())
        })?;
        if element_id == id::SEEK_HEAD {
            return Ok(Some(
                segment_body[pos + header_len..pos + header_len + size].to_vec(),
            ));
        }
        pos += header_len + size;
    }
    Ok(None)
}

/// Appends a `Seek` entry for the new `Tags` element to an already-rewritten
/// `SeekHead`, keeping its size VINT and slot offsets consistent.
fn append_tags_seek_entry(seek_head: &mut Level1) -> Result<(), Error> {
    let (element_id, header_len, size) = element_header(&seek_head.bytes, 0)?;
    let size = size
        .ok_or_else(|| Error::ParseError("Matroska: SeekHead has an unknown size".to_string()))?;
    let old_body_len = usize::try_from(size).map_err(|_| {
        Error::ParseError("Matroska: SeekHead size exceeds the addressable range".to_string())
    })?;
    let (entry, entry_slot_offset) = build_tags_seek_entry();

    let mut body = seek_head.bytes[header_len..header_len + old_body_len].to_vec();
    let entry_base = body.len();
    body.extend_from_slice(&entry);

    let mut out = encode_id(element_id);
    out.extend_from_slice(&encode_size(body.len() as u64));
    let new_header_len = out.len();
    out.extend_from_slice(&body);

    // Existing slots shift with the (possibly wider) size VINT.
    for slot in &mut seek_head.slots {
        slot.offset = slot.offset + new_header_len - header_len;
    }
    seek_head.slots.push(Slot {
        offset: new_header_len + entry_base + entry_slot_offset,
        target: SlotTarget::NewTags,
    });
    seek_head.bytes = out;
    Ok(())
}

/// Maps a position stored in the original file to the equivalent position in the
/// rebuilt one.
///
/// `mapping` is the `(old, new)` offset table of surviving level-1 elements in
/// file order — and therefore sorted by old offset, so the enclosing element is
/// found by binary search rather than a scan (a file with thousands of clusters
/// and thousands of cue points would otherwise cost their product).
///
/// A position that names an element maps to that element's new offset; a
/// position that lands *inside* one keeps its relative distance. A position
/// before the first surviving element is left untouched.
fn remap_position(mapping: &[(u64, u64)], old: u64) -> u64 {
    // The last entry whose old offset is <= `old`.
    let index = match mapping.binary_search_by_key(&old, |(old_offset, _)| *old_offset) {
        Ok(index) => Some(index),
        Err(0) => None,
        Err(index) => Some(index - 1),
    };
    match index.and_then(|index| mapping.get(index)) {
        Some((old_offset, new_offset)) => new_offset + (old - old_offset),
        None => old,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetadataFormat;

    /// Builds a complete element from an ID and a payload.
    fn element(element_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        push_element(&mut out, element_id, payload);
        out
    }

    /// A minimal but well-formed EBML header declaring a Matroska document.
    fn ebml_header() -> Vec<u8> {
        let mut body = Vec::new();
        push_element(&mut body, 0x4286, &[1]); // EBMLVersion
        push_element(&mut body, 0x42F7, &[1]); // EBMLReadVersion
        push_element(&mut body, 0x4282, b"matroska"); // DocType
        push_element(&mut body, 0x4287, &[4]); // DocTypeVersion
        push_element(&mut body, 0x4285, &[2]); // DocTypeReadVersion
        element(id::EBML, &body)
    }

    /// Assembles a Segment around the given level-1 children.
    fn segment(children: &[u8]) -> Vec<u8> {
        let mut out = ebml_header();
        out.extend_from_slice(&element(id::SEGMENT, children));
        out
    }

    /// A `Seek` entry pointing `seek_id` at `position`.
    fn seek_entry(seek_id: u32, position: u64) -> Vec<u8> {
        let mut body = Vec::new();
        push_element(&mut body, id::SEEK_ID, &encode_id(seek_id));
        let bytes = position.to_be_bytes();
        let first_significant = bytes.iter().position(|b| *b != 0).unwrap_or(7);
        push_element(&mut body, id::SEEK_POSITION, &bytes[first_significant..]);
        element(id::SEEK, &body)
    }

    /// Walks the level-1 children of a rebuilt file: `(id, offset, total len)`.
    fn level1(file: &[u8]) -> Vec<(u32, u64, usize)> {
        let segment_offset = element_len(file, 0).expect("ebml header");
        let (_, header_len, size) = element_header(file, segment_offset).expect("segment header");
        let body_start = segment_offset + header_len;
        let body_end = body_start + size.expect("segment has a known size") as usize;
        let mut out = Vec::new();
        let mut pos = body_start;
        while pos < body_end {
            let (element_id, header_len, size) = element_header(file, pos).expect("level-1 header");
            // Mirror the production walk: an unknown-size Cluster runs to the
            // next level-1 element.
            let total = match size {
                Some(size) => header_len + size as usize,
                None => {
                    unbounded_cluster_end(file, pos + header_len, body_end)
                        .expect("unbounded cluster extent")
                        - pos
                }
            };
            out.push((element_id, (pos - body_start) as u64, total));
            pos += total;
        }
        out
    }

    /// Reads the `SimpleTag` name/value pairs out of a rebuilt file's `Tags`.
    fn read_tags(file: &[u8]) -> Vec<(String, String)> {
        let segment_offset = element_len(file, 0).expect("ebml header");
        let (_, header_len, size) = element_header(file, segment_offset).expect("segment header");
        let body_start = segment_offset + header_len;
        let body_end = body_start + size.expect("known size") as usize;

        let _ = body_end;
        let mut pairs = Vec::new();
        for (element_id, offset, total) in level1(file) {
            if element_id != id::TAGS {
                continue;
            }
            let pos = body_start + offset as usize;
            let (_, elem_header, _) = element_header(file, pos).expect("tags header");
            collect_simple_tags(&file[pos + elem_header..pos + total], &mut pairs);
        }
        pairs
    }

    /// Recursively collects `TagName`/`TagString` pairs.
    fn collect_simple_tags(body: &[u8], out: &mut Vec<(String, String)>) {
        let mut pos = 0usize;
        while pos < body.len() {
            let Ok(total) = element_len(body, pos) else {
                return;
            };
            let Ok((element_id, header_len, _)) = element_header(body, pos) else {
                return;
            };
            let inner = &body[pos + header_len..pos + total];
            match element_id {
                id::TAG | id::TAGS => collect_simple_tags(inner, out),
                id::SIMPLE_TAG => {
                    let mut name = String::new();
                    let mut value = String::new();
                    let mut inner_pos = 0usize;
                    while inner_pos < inner.len() {
                        let Ok(child_total) = element_len(inner, inner_pos) else {
                            break;
                        };
                        let Ok((child_id, child_header, _)) = element_header(inner, inner_pos)
                        else {
                            break;
                        };
                        let payload = &inner[inner_pos + child_header..inner_pos + child_total];
                        if child_id == id::TAG_NAME {
                            name = String::from_utf8_lossy(payload).into_owned();
                        } else if child_id == id::TAG_STRING {
                            value = String::from_utf8_lossy(payload).into_owned();
                        }
                        inner_pos += child_total;
                    }
                    out.push((name, value));
                }
                _ => {}
            }
            pos += total;
        }
    }

    fn sample_metadata() -> Metadata {
        let mut metadata = Metadata::new(MetadataFormat::Matroska);
        metadata.insert(
            "TITLE".to_string(),
            MetadataValue::Text("A Test Title".to_string()),
        );
        metadata.insert(
            "ARTIST".to_string(),
            MetadataValue::Text("Test Artist".to_string()),
        );
        metadata
    }

    #[test]
    fn test_vint_round_trip() {
        for value in [0u64, 1, 126, 127, 128, 16382, 16383, 1 << 20, (1 << 35) + 7] {
            let encoded = encode_size(value);
            let (decoded, width) = parse_size(&encoded, 0).expect("decode");
            assert_eq!(decoded, Some(value), "value {value} must round-trip");
            assert_eq!(width, encoded.len());
        }
    }

    #[test]
    fn test_unknown_size_sentinel_is_recognised() {
        assert_eq!(parse_size(&[0xFF], 0).expect("parse"), (None, 1));
        assert_eq!(parse_size(&[0x7F, 0xFF], 0).expect("parse"), (None, 2));
    }

    #[test]
    fn test_element_ids_round_trip() {
        for element_id in [id::VOID, id::SEEK, id::TAGS, id::SEGMENT, id::CLUSTER] {
            let encoded = encode_id(element_id);
            let (decoded, width) = parse_id(&encoded, 0).expect("decode");
            assert_eq!(decoded, element_id);
            assert_eq!(width, encoded.len());
        }
    }

    #[test]
    fn test_inserts_tags_before_the_first_cluster() {
        let mut children = Vec::new();
        children.extend_from_slice(&element(0x1549_A966, &[0x2A])); // Info
        children.extend_from_slice(&element(0x1654_AE6B, &[0x2A])); // Tracks
        children.extend_from_slice(&element(id::CLUSTER, &element(0xE7, &[0]))); // Cluster
        let file = segment(&children);

        let out = embed(&file, &sample_metadata()).expect("embed tags");
        let ids: Vec<u32> = level1(&out).iter().map(|(i, _, _)| *i).collect();
        assert_eq!(
            ids,
            vec![0x1549_A966, 0x1654_AE6B, id::TAGS, id::CLUSTER],
            "Tags must sit before the first Cluster so a demuxer that stops there still sees it"
        );

        let mut pairs = read_tags(&out);
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("ARTIST".to_string(), "Test Artist".to_string()),
                ("TITLE".to_string(), "A Test Title".to_string()),
            ]
        );
    }

    #[test]
    fn test_replaces_an_existing_tags_element() {
        let mut old_tags = Metadata::new(MetadataFormat::Matroska);
        old_tags.insert(
            "TITLE".to_string(),
            MetadataValue::Text("Stale".to_string()),
        );
        let mut children = Vec::new();
        children.extend_from_slice(&element(0x1549_A966, &[0x2A]));
        children.extend_from_slice(&build_tags_element(&old_tags).expect("build old tags"));
        children.extend_from_slice(&element(id::CLUSTER, &element(0xE7, &[0])));
        let file = segment(&children);

        let out = embed(&file, &sample_metadata()).expect("embed");
        let tags_count = level1(&out)
            .iter()
            .filter(|(element_id, _, _)| *element_id == id::TAGS)
            .count();
        assert_eq!(tags_count, 1, "exactly one Tags element after replacement");

        let pairs = read_tags(&out);
        assert!(
            !pairs.iter().any(|(_, value)| value == "Stale"),
            "the replaced Tags element must not leave stale values behind"
        );
    }

    #[test]
    fn test_seek_head_positions_are_recomputed_and_tags_indexed() {
        // SeekHead entries for Info and Tracks, with the positions they have in
        // the original layout.
        let seek_head_placeholder = {
            let mut body = Vec::new();
            body.extend_from_slice(&seek_entry(0x1549_A966, 0));
            body.extend_from_slice(&seek_entry(0x1654_AE6B, 0));
            element(id::SEEK_HEAD, &body)
        };
        let info = element(0x1549_A966, &[0x2A]);
        let tracks = element(0x1654_AE6B, &[0x2A; 4]);

        let info_offset = seek_head_placeholder.len() as u64;
        let tracks_offset = info_offset + info.len() as u64;
        let seek_head = {
            let mut body = Vec::new();
            body.extend_from_slice(&seek_entry(0x1549_A966, info_offset));
            body.extend_from_slice(&seek_entry(0x1654_AE6B, tracks_offset));
            element(id::SEEK_HEAD, &body)
        };
        assert_eq!(
            seek_head.len(),
            seek_head_placeholder.len(),
            "test fixture must keep the SeekHead length stable"
        );

        let mut children = Vec::new();
        children.extend_from_slice(&seek_head);
        children.extend_from_slice(&info);
        children.extend_from_slice(&tracks);
        children.extend_from_slice(&element(id::CLUSTER, &element(0xE7, &[0])));
        let file = segment(&children);

        let out = embed(&file, &sample_metadata()).expect("embed");
        let layout = level1(&out);

        // Every SeekPosition in the rebuilt SeekHead must name a real element.
        let (_, _, seek_head_len) = layout[0];
        let segment_offset = element_len(&out, 0).expect("header");
        let (_, segment_header, _) = element_header(&out, segment_offset).expect("segment");
        let body_start = segment_offset + segment_header;
        let (_, seek_header_len, _) = element_header(&out, body_start).expect("seek head");
        let seek_body = &out[body_start + seek_header_len..body_start + seek_head_len];

        let mut indexed: Vec<(u32, u64)> = Vec::new();
        let mut pos = 0usize;
        while pos < seek_body.len() {
            let total = element_len(seek_body, pos).expect("seek entry");
            let (_, header_len, _) = element_header(seek_body, pos).expect("seek header");
            let entry = &seek_body[pos + header_len..pos + total];
            let mut seek_id = 0u32;
            let mut position = 0u64;
            let mut inner = 0usize;
            while inner < entry.len() {
                let child_total = element_len(entry, inner).expect("child");
                let (child_id, child_header, _) = element_header(entry, inner).expect("child hdr");
                let payload = &entry[inner + child_header..inner + child_total];
                if child_id == id::SEEK_ID {
                    for byte in payload {
                        seek_id = (seek_id << 8) | u32::from(*byte);
                    }
                } else if child_id == id::SEEK_POSITION {
                    position = read_uint(payload);
                }
                inner += child_total;
            }
            indexed.push((seek_id, position));
            pos += total;
        }

        assert!(
            indexed.iter().any(|(seek_id, _)| *seek_id == id::TAGS),
            "the SeekHead must gain an entry for the new Tags element"
        );
        for (seek_id, position) in &indexed {
            let target = layout
                .iter()
                .find(|(_, offset, _)| offset == position)
                .unwrap_or_else(|| panic!("SeekPosition {position} names no level-1 element"));
            assert_eq!(
                target.0, *seek_id,
                "SeekPosition {position} must point at element 0x{seek_id:X}"
            );
        }
    }

    #[test]
    fn test_cue_cluster_positions_follow_their_clusters() {
        let cluster_a = element(id::CLUSTER, &element(0xE7, &[0]));
        let cluster_b = element(id::CLUSTER, &element(0xE7, &[1]));
        let info = element(0x1549_A966, &[0x2A]);

        let cluster_a_offset = info.len() as u64;
        let cluster_b_offset = cluster_a_offset + cluster_a.len() as u64;
        let cues = {
            let mut points = Vec::new();
            for offset in [cluster_a_offset, cluster_b_offset] {
                let mut track_positions = Vec::new();
                push_element(&mut track_positions, 0xF7, &[1]); // CueTrack
                let bytes = offset.to_be_bytes();
                let first = bytes.iter().position(|b| *b != 0).unwrap_or(7);
                push_element(
                    &mut track_positions,
                    id::CUE_CLUSTER_POSITION,
                    &bytes[first..],
                );
                let mut point = Vec::new();
                push_element(&mut point, 0xB3, &[0]); // CueTime
                push_element(&mut point, id::CUE_TRACK_POSITIONS, &track_positions);
                push_element(&mut points, id::CUE_POINT, &point);
            }
            element(id::CUES, &points)
        };

        let mut children = Vec::new();
        children.extend_from_slice(&info);
        children.extend_from_slice(&cluster_a);
        children.extend_from_slice(&cluster_b);
        children.extend_from_slice(&cues);
        let file = segment(&children);

        let out = embed(&file, &sample_metadata()).expect("embed");
        let layout = level1(&out);
        let cluster_offsets: Vec<u64> = layout
            .iter()
            .filter(|(element_id, _, _)| *element_id == id::CLUSTER)
            .map(|(_, offset, _)| *offset)
            .collect();
        assert_eq!(cluster_offsets.len(), 2);

        // Pull the rebuilt CueClusterPosition values back out.
        let (_, cues_offset, cues_len) = *layout
            .iter()
            .find(|(element_id, _, _)| *element_id == id::CUES)
            .expect("cues survive");
        let segment_offset = element_len(&out, 0).expect("header");
        let (_, segment_header, _) = element_header(&out, segment_offset).expect("segment");
        let body_start = segment_offset + segment_header;
        let cues_bytes =
            &out[body_start + cues_offset as usize..body_start + cues_offset as usize + cues_len];

        let mut found: Vec<u64> = Vec::new();
        collect_cue_positions(cues_bytes, &mut found);
        assert_eq!(
            found, cluster_offsets,
            "each CueClusterPosition must point at the cluster's new offset"
        );
    }

    /// Recursively collects `CueClusterPosition` values.
    fn collect_cue_positions(data: &[u8], out: &mut Vec<u64>) {
        let mut pos = 0usize;
        while pos < data.len() {
            let Ok(total) = element_len(data, pos) else {
                return;
            };
            let Ok((element_id, header_len, _)) = element_header(data, pos) else {
                return;
            };
            let payload = &data[pos + header_len..pos + total];
            match element_id {
                id::CUE_CLUSTER_POSITION => out.push(read_uint(payload)),
                id::CUES | id::CUE_POINT | id::CUE_TRACK_POSITIONS => {
                    collect_cue_positions(payload, out);
                }
                _ => {}
            }
            pos += total;
        }
    }

    #[test]
    fn test_level1_void_is_reclaimed() {
        let mut children = Vec::new();
        children.extend_from_slice(&element(0x1549_A966, &[0x2A]));
        children.extend_from_slice(&element(id::VOID, &[0u8; 64]));
        children.extend_from_slice(&element(id::CLUSTER, &element(0xE7, &[0])));
        let file = segment(&children);

        let out = embed(&file, &sample_metadata()).expect("embed");
        assert!(
            !level1(&out)
                .iter()
                .any(|(element_id, _, _)| *element_id == id::VOID),
            "reserved Void padding must be reclaimed rather than kept alongside the new Tags"
        );
    }

    #[test]
    fn test_unrepresentable_values_are_an_honest_error() {
        // Binary and picture values need a TagBinary element, which this writer
        // does not emit; writing an empty Tags element would be a silent no-op.
        let mut children = Vec::new();
        children.extend_from_slice(&element(0x1549_A966, &[0x2A]));
        children.extend_from_slice(&element(id::CLUSTER, &element(0xE7, &[0])));
        let file = segment(&children);

        let mut metadata = Metadata::new(MetadataFormat::Matroska);
        metadata.insert(
            "COVER".to_string(),
            MetadataValue::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        );
        match embed(&file, &metadata) {
            Err(Error::WriteError(message)) => {
                assert!(message.contains("TagBinary"), "message was: {message}");
                assert!(message.contains("COVER"), "message was: {message}");
            }
            other => panic!("expected an honest WriteError, got {other:?}"),
        }
    }

    #[test]
    fn test_rejects_non_ebml_input() {
        let result = embed(b"this is not an EBML document", &sample_metadata());
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn test_unknown_size_clusters_are_walked_not_rejected() {
        // Two unknown-size clusters — the layout single-pass muxers write.
        let mut children = Vec::new();
        children.extend_from_slice(&element(0x1549_A966, &[0x2A]));
        for timestamp in 0u8..2 {
            children.extend_from_slice(&encode_id(id::CLUSTER));
            children.push(0xFF); // unknown size
            children.extend_from_slice(&element(0xE7, &[timestamp]));
            children.extend_from_slice(&element(0xA3, &[0x81, 0, 0, 0x80, 0xAA]));
        }
        let file = segment(&children);

        let out = embed(&file, &sample_metadata()).expect("embed into unknown-size clusters");
        let ids: Vec<u32> = level1(&out).iter().map(|(id, _, _)| *id).collect();
        assert_eq!(
            ids,
            vec![0x1549_A966, id::TAGS, id::CLUSTER, id::CLUSTER],
            "both unbounded clusters must survive, with Tags spliced in before them"
        );
        // The clusters' bytes (header included) must be untouched.
        let original_clusters = file
            .windows(4)
            .filter(|window| *window == encode_id(id::CLUSTER).as_slice())
            .count();
        let new_clusters = out
            .windows(4)
            .filter(|window| *window == encode_id(id::CLUSTER).as_slice())
            .count();
        assert_eq!(original_clusters, new_clusters);
        assert_eq!(read_tags(&out).len(), 2);
    }

    #[test]
    fn test_rejects_unbounded_non_cluster_element() {
        // An unbounded element that is not a Cluster has no defined extent.
        let mut children = Vec::new();
        children.extend_from_slice(&element(0x1549_A966, &[0x2A]));
        children.extend_from_slice(&encode_id(0x1654_AE6B)); // Tracks
        children.push(0xFF); // unknown size
        children.extend_from_slice(&element(0xAE, &[0]));
        let file = segment(&children);

        match embed(&file, &sample_metadata()) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("unknown"), "message was: {message}");
            }
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn test_input_is_never_modified_on_error() {
        let original = b"not ebml at all, definitely".to_vec();
        let copy = original.clone();
        let _ = embed(&original, &sample_metadata());
        assert_eq!(original, copy);
    }

    #[test]
    fn test_remap_position_is_piecewise() {
        let mapping = vec![(0u64, 0u64), (100, 150), (200, 250)];
        assert_eq!(remap_position(&mapping, 0), 0);
        assert_eq!(remap_position(&mapping, 50), 50);
        assert_eq!(remap_position(&mapping, 100), 150);
        assert_eq!(remap_position(&mapping, 120), 170);
        assert_eq!(remap_position(&mapping, 200), 250);
        assert_eq!(remap_position(&mapping, 260), 310);
    }
}

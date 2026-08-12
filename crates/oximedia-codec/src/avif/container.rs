//! ISOBMFF container parsing for AVIF: signature check, `meta` box
//! metadata extraction, and `iloc` item-extent resolution.
//!
//! Split out of `avif/mod.rs` (which keeps the encoder-side box *builders*)
//! to stay under the workspace's per-file line budget and to separate
//! "write a container" from "read a container".

use super::AvifProbeResult;
use crate::error::CodecError;

/// Verify the byte stream starts with an AVIF `ftyp` box.
pub(super) fn check_avif_signature(data: &[u8]) -> Result<(), CodecError> {
    if data.len() < 12 {
        return Err(CodecError::InvalidBitstream(
            "file too short to be AVIF".into(),
        ));
    }
    let size = u32::from_be_bytes(
        data[0..4]
            .try_into()
            .map_err(|_| CodecError::InvalidBitstream("cannot read ftyp size".into()))?,
    ) as usize;
    if size < 12 || size > data.len() {
        return Err(CodecError::InvalidBitstream("invalid ftyp box size".into()));
    }
    if &data[4..8] != b"ftyp" {
        return Err(CodecError::InvalidBitstream("first box is not ftyp".into()));
    }
    // Check that 'avif' appears among the brands.
    let brands_region = &data[8..size];
    let has_avif = brands_region
        .chunks(4)
        .any(|c| c.len() == 4 && c == b"avif");
    if !has_avif {
        return Err(CodecError::InvalidBitstream(
            "ftyp does not contain 'avif' brand".into(),
        ));
    }
    Ok(())
}

/// Parse the `meta` box to extract spatial/colour metadata.
pub(super) fn parse_meta_for_probe(data: &[u8]) -> Result<AvifProbeResult, CodecError> {
    // Find meta box (typically immediately after ftyp, but walk to be safe).
    let meta_pos = find_top_level_box(data, b"meta")
        .ok_or_else(|| CodecError::InvalidBitstream("meta box not found".into()))?;
    let meta_size = u32::from_be_bytes(
        data[meta_pos..meta_pos + 4]
            .try_into()
            .map_err(|_| CodecError::InvalidBitstream("meta size read error".into()))?,
    ) as usize;
    let meta_end = meta_pos + meta_size;

    // meta is a FullBox: skip box header(8) + fullbox flags(4) = 12.
    let meta_body = meta_pos + 12;

    // ── ispe ──────────────────────────────────────────────────────────
    let (width, height) = parse_ispe(data, meta_body, meta_end)?;

    // ── colr ──────────────────────────────────────────────────────────
    let (color_primaries, transfer_characteristics) =
        parse_colr(data, meta_body, meta_end).unwrap_or((1, 1));

    // ── pixi ──────────────────────────────────────────────────────────
    let bit_depth = parse_pixi(data, meta_body, meta_end).unwrap_or(8);

    // ── alpha: check iinf for a second item with auxiliary type ───────
    let has_alpha = parse_iinf_has_alpha(data, meta_body, meta_end);

    Ok(AvifProbeResult {
        width,
        height,
        bit_depth,
        has_alpha,
        color_primaries,
        transfer_characteristics,
    })
}

fn parse_ispe(data: &[u8], start: usize, end: usize) -> Result<(u32, u32), CodecError> {
    let pos = find_box_in(data, start, end, b"iprp")
        .and_then(|iprp| {
            let iprp_end =
                iprp + u32::from_be_bytes(data[iprp..iprp + 4].try_into().ok()?) as usize;
            find_box_in(data, iprp + 8, iprp_end, b"ipco").and_then(|ipco| {
                let ipco_end =
                    ipco + u32::from_be_bytes(data[ipco..ipco + 4].try_into().ok()?) as usize;
                find_box_in(data, ipco + 8, ipco_end, b"ispe")
            })
        })
        .ok_or_else(|| CodecError::InvalidBitstream("ispe not found".into()))?;

    // ispe: FullBox(12) + width(4) + height(4)
    if pos + 20 > data.len() {
        return Err(CodecError::InvalidBitstream("ispe box truncated".into()));
    }
    let w = u32::from_be_bytes(
        data[pos + 12..pos + 16]
            .try_into()
            .map_err(|_| CodecError::InvalidBitstream("ispe width read error".into()))?,
    );
    let h = u32::from_be_bytes(
        data[pos + 16..pos + 20]
            .try_into()
            .map_err(|_| CodecError::InvalidBitstream("ispe height read error".into()))?,
    );
    Ok((w, h))
}

fn parse_colr(data: &[u8], start: usize, end: usize) -> Option<(u8, u8)> {
    let iprp = find_box_in(data, start, end, b"iprp")?;
    let iprp_end = iprp + u32::from_be_bytes(data[iprp..iprp + 4].try_into().ok()?) as usize;
    let ipco = find_box_in(data, iprp + 8, iprp_end, b"ipco")?;
    let ipco_end = ipco + u32::from_be_bytes(data[ipco..ipco + 4].try_into().ok()?) as usize;
    let pos = find_box_in(data, ipco + 8, ipco_end, b"colr")?;
    // colr: box(8) + colour_type(4) + nclx primaries(2) + transfer(2) + ...
    // The reads below touch bytes up to `pos+16`, so the guard must cover them;
    // a `pos+15` guard left `data[pos+14..pos+16]` able to panic on a truncated
    // box.
    if pos + 16 > data.len() {
        return None;
    }
    if &data[pos + 8..pos + 12] != b"nclx" {
        return None;
    }
    // nclx: colour_primaries(2) + transfer_characteristics(2) + ...
    let cp = u16::from_be_bytes(data[pos + 12..pos + 14].try_into().ok()?) as u8;
    let tc = u16::from_be_bytes(data[pos + 14..pos + 16].try_into().ok()?) as u8;
    Some((cp, tc))
}

fn parse_pixi(data: &[u8], start: usize, end: usize) -> Option<u8> {
    let iprp = find_box_in(data, start, end, b"iprp")?;
    let iprp_end = iprp + u32::from_be_bytes(data[iprp..iprp + 4].try_into().ok()?) as usize;
    let ipco = find_box_in(data, iprp + 8, iprp_end, b"ipco")?;
    let ipco_end = ipco + u32::from_be_bytes(data[ipco..ipco + 4].try_into().ok()?) as usize;
    let pos = find_box_in(data, ipco + 8, ipco_end, b"pixi")?;
    // pixi: FullBox(12) + num_channels(1) + depth[0](1)
    if pos + 14 > data.len() {
        return None;
    }
    Some(data[pos + 13])
}

fn parse_iinf_has_alpha(data: &[u8], start: usize, end: usize) -> bool {
    let pos = match find_box_in(data, start, end, b"iinf") {
        Some(p) => p,
        None => return false,
    };
    let iinf_size = u32::from_be_bytes(match data[pos..pos + 4].try_into() {
        Ok(b) => b,
        Err(_) => return false,
    }) as usize;
    // iinf FullBox version=0: box(8) + fullbox(4) + entry_count(2)
    let entry_count = u16::from_be_bytes(match data[pos + 12..pos + 14].try_into() {
        Ok(b) => b,
        Err(_) => return false,
    });
    // If there's more than one item, we treat the second as alpha.
    // (Loose heuristic: a real multi-item file carrying e.g. an Exif or
    // thumbnail item instead of alpha would be misread here. Neither
    // encoder this parser targets — this crate's own AvifEncoder or
    // ffmpeg/libaom's AVIF muxer — produces such files.)
    entry_count >= 2 && iinf_size >= 14
}

/// Read a big-endian unsigned integer of `nbytes` (0..=8) from `data` at
/// `pos`. `nbytes == 0` reads as the value `0` (used for an absent
/// `base_offset` field). Returns `None` — never panics — if the read would
/// run past the end of `data`.
fn read_uint_be(data: &[u8], pos: usize, nbytes: usize) -> Option<u64> {
    if nbytes == 0 {
        return Some(0);
    }
    let end = pos.checked_add(nbytes)?;
    let bytes = data.get(pos..end)?;
    let mut v: u64 = 0;
    for &b in bytes {
        v = (v << 8) | u64::from(b);
    }
    Some(v)
}

/// Locate the `mdat` box and return `(color_offset, color_len, alpha_offset, alpha_len)`.
///
/// Parses the real `iloc` item extents. Two `iloc` box layouts are
/// implemented, matching the encoders this parser has actually been tested
/// against:
///
/// - **version 0** — written by ffmpeg's (libaom) AVIF muxer: no
///   `construction_method` field.
/// - **version 1** — written by this crate's own `build_iloc` (the AVIF
///   encoder in `mod.rs`): adds a `construction_method` field, which must
///   be `0` (file offset).
///
/// Item extents are read positionally: the first listed item is the colour
/// image, the second (when `has_alpha`) is the alpha auxiliary image. Both
/// encoders observed in testing list items in `item_ID` order (color=1,
/// alpha=2), so this matches `pitm`/`iinf` without needing to cross-check
/// `iref`'s `auxl` reference.
///
/// # Errors
///
/// `InvalidBitstream` for malformed/truncated boxes. Honest
/// `UnsupportedFeature` for `iloc` layouts this parser does not implement
/// (version ≥ 2, a non-file-offset `construction_method`, multi-extent
/// items, or a non-zero `base_offset_size`) — none of which are produced by
/// the encoders this crate targets.
pub(super) fn locate_mdat_items(
    data: &[u8],
    has_alpha: bool,
) -> Result<(usize, usize, usize, usize), CodecError> {
    let meta_pos = find_top_level_box(data, b"meta")
        .ok_or_else(|| CodecError::InvalidBitstream("meta box not found".into()))?;
    let meta_size = u32::from_be_bytes(
        data[meta_pos..meta_pos + 4]
            .try_into()
            .map_err(|_| CodecError::InvalidBitstream("meta size".into()))?,
    ) as usize;
    let meta_end = meta_pos + meta_size;
    let meta_body = meta_pos + 12;

    let iloc_pos = find_box_in(data, meta_body, meta_end, b"iloc")
        .ok_or_else(|| CodecError::InvalidBitstream("iloc not found".into()))?;

    // box(8) + version(1) + flags(3) + sizes_byte(1) + sizes_byte(1) + item_count(2) = 16.
    if iloc_pos + 16 > data.len() {
        return Err(CodecError::InvalidBitstream("iloc box truncated".into()));
    }
    let version = data[iloc_pos + 8];
    if version > 1 {
        return Err(CodecError::UnsupportedFeature(format!(
            "iloc version {version} not supported (only versions 0 and 1 are implemented)"
        )));
    }
    let sizes_byte = data[iloc_pos + 12];
    let offset_size = (sizes_byte >> 4) as usize;
    let length_size = (sizes_byte & 0x0F) as usize;
    let base_offset_size = (data[iloc_pos + 13] >> 4) as usize;
    if base_offset_size > 0 {
        return Err(CodecError::UnsupportedFeature(
            "iloc base_offset_size > 0 not supported".into(),
        ));
    }

    let item_count = u16::from_be_bytes(
        data[iloc_pos + 14..iloc_pos + 16]
            .try_into()
            .map_err(|_| CodecError::InvalidBitstream("iloc item_count".into()))?,
    ) as usize;
    if item_count == 0 {
        return Err(CodecError::InvalidBitstream("iloc has no items".into()));
    }
    let needed_items = if has_alpha { 2 } else { 1 };
    if item_count < needed_items {
        return Err(CodecError::InvalidBitstream(format!(
            "iinf declared {needed_items} item(s) but iloc has only {item_count}"
        )));
    }

    // We only need the first two item entries (color, optionally alpha);
    // every read below is bounds-checked via `read_uint_be`; a truncated or
    // fuzzed prefix bails with an honest error on the first short read
    // rather than trusting `item_count` to iterate further.
    let mut pos = iloc_pos + 16;
    let mut extents: Vec<(usize, usize)> = Vec::with_capacity(2);
    for item_idx in 0..item_count.min(2) {
        pos += 2; // item_ID
        if version >= 1 {
            let cm = read_uint_be(data, pos, 2).ok_or_else(|| {
                CodecError::InvalidBitstream("iloc construction_method truncated".into())
            })?;
            if cm != 0 {
                return Err(CodecError::UnsupportedFeature(format!(
                    "iloc construction_method {cm} not supported (only file-offset (0) is implemented)"
                )));
            }
            pos += 2;
        }
        pos += 2; // data_reference_index
        let extent_count = read_uint_be(data, pos, 2)
            .ok_or_else(|| CodecError::InvalidBitstream("iloc extent_count truncated".into()))?;
        pos += 2;
        if extent_count != 1 {
            return Err(CodecError::UnsupportedFeature(format!(
                "iloc item {item_idx}: extent_count {extent_count} not supported (only single-extent items are implemented)"
            )));
        }
        let extent_offset = read_uint_be(data, pos, offset_size)
            .ok_or_else(|| CodecError::InvalidBitstream("iloc extent_offset truncated".into()))?;
        pos += offset_size;
        let extent_length = read_uint_be(data, pos, length_size)
            .ok_or_else(|| CodecError::InvalidBitstream("iloc extent_length truncated".into()))?;
        pos += length_size;

        extents.push((extent_offset as usize, extent_length as usize));
    }

    let (color_offset, color_len) = extents[0];
    let (alpha_offset, alpha_len) = if has_alpha { extents[1] } else { (0, 0) };

    Ok((color_offset, color_len, alpha_offset, alpha_len))
}

/// Walk the top-level box list to find a box by type.
pub(super) fn find_top_level_box(data: &[u8], box_type: &[u8; 4]) -> Option<usize> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        if size < 8 {
            break;
        }
        if &data[pos + 4..pos + 8] == box_type {
            return Some(pos);
        }
        pos += size;
    }
    None
}

/// Find a 4-byte box type within `data[start..end]`, return byte offset.
pub(super) fn find_box_in(
    data: &[u8],
    start: usize,
    end: usize,
    box_type: &[u8; 4],
) -> Option<usize> {
    let mut pos = start;
    while pos + 8 <= end.min(data.len()) {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        if size < 8 {
            break;
        }
        if &data[pos + 4..pos + 8] == box_type {
            return Some(pos);
        }
        pos += size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_uint_be_various_widths() {
        let data = [0x12, 0x34, 0x56, 0x78, 0x9A];
        assert_eq!(read_uint_be(&data, 0, 0), Some(0));
        assert_eq!(read_uint_be(&data, 0, 1), Some(0x12));
        assert_eq!(read_uint_be(&data, 0, 2), Some(0x1234));
        assert_eq!(read_uint_be(&data, 0, 4), Some(0x1234_5678));
        // Last valid 4-byte read: indices 1..5 (data.len() == 5).
        assert_eq!(read_uint_be(&data, 1, 4), Some(0x3456_789A));
        // One byte past the end: indices 2..6 overruns data.len() == 5.
        assert_eq!(read_uint_be(&data, 2, 4), None, "runs past end");
        assert_eq!(read_uint_be(&data, 10, 1), None, "pos already past end");
    }

    #[test]
    fn locate_mdat_items_rejects_iloc_version_2() {
        // A minimal, self-consistent ftyp+meta+iloc(v2) container: just
        // enough for `locate_mdat_items` to reach the version check (which
        // fires before item_count or any item entry is interpreted).
        let mut data = Vec::new();
        // ftyp
        data.extend_from_slice(&12u32.to_be_bytes());
        data.extend_from_slice(b"ftypavif");

        // iloc: box header(8) + fullbox(4) + sizes(1) + sizes(1) + item_count(2) = 16 bytes.
        let mut iloc = Vec::new();
        iloc.extend_from_slice(&16u32.to_be_bytes()); // size
        iloc.extend_from_slice(b"iloc");
        iloc.push(2); // version = 2 (unsupported)
        iloc.extend_from_slice(&[0, 0, 0]); // flags
        iloc.push(0x44); // offset_size=4, length_size=4
        iloc.push(0x00); // base_offset_size=0, index_size=0
        iloc.extend_from_slice(&[0, 1]); // item_count = 1 (unused; version check fires first)
        assert_eq!(iloc.len(), 16);

        let meta_size = 8u32 + 4 + iloc.len() as u32;
        data.extend_from_slice(&meta_size.to_be_bytes());
        data.extend_from_slice(b"meta");
        data.extend_from_slice(&[0, 0, 0, 0]); // version+flags
        data.extend_from_slice(&iloc);

        let err = locate_mdat_items(&data, false).unwrap_err();
        assert!(
            matches!(err, CodecError::UnsupportedFeature(_)),
            "iloc version 2 must be an honest UnsupportedFeature, got {err:?}"
        );
    }
}

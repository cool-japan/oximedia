//! MP4 / QuickTime atom-tree-aware metadata embedding.
//!
//! An ISO-BMFF file is a flat sequence of atoms (boxes), each `size` +
//! `type` + payload, nested arbitrarily deep. Metadata lives inside `moov`:
//!
//! ```text
//! moov
//!   udta
//!     meta            (FullBox: 4 bytes of version+flags before its children)
//!       hdlr          ('mdir'/'appl' — marks the metadata as iTunes-style)
//!       ilst          (the actual tag atoms)
//!     ©nam, ©ART, …   (classic QuickTime user-data atoms, no `meta` wrapper)
//! ```
//!
//! Splicing tags in therefore means rebuilding that sub-tree and fixing every
//! enclosing atom's size field — and, because the sample tables store **absolute
//! file offsets**, repairing `stco`/`co64` whenever the edit moves the media
//! data.
//!
//! # Chunk-offset strategy: shift-and-fix
//!
//! This module keeps `moov` where it is and patches the chunk offsets — the
//! "real, preferred" option — rather than relocating `moov` to the end of the
//! file:
//!
//! * `moov` **after** every `mdat`: the media data does not move, so no offset
//!   is touched.
//! * `moov` **before** every `mdat`: every `stco`/`co64` entry is shifted by the
//!   change in `moov`'s size. A 32-bit `stco` entry that would overflow is an
//!   honest error (promoting the table to `co64` would change `moov`'s size
//!   again, and this module never guesses).
//! * `mdat` atoms on **both** sides of `moov`: no single delta is correct, so
//!   that layout is reported rather than mis-patched.
//!
//! `stsc`, `stsz` and `stts` hold counts and durations, not offsets, and are
//! unaffected. Fragmented files (`moof`/`sidx`) index their media through
//! entirely different structures and are reported as unsupported.
//!
//! # Known gap
//!
//! `saio` (sample auxiliary information offsets, written by CENC-encrypted
//! progressive files) also stores absolute file offsets and is **not** patched
//! here. A `moov`-before-`mdat` encrypted file would therefore end up with stale
//! auxiliary offsets; plain, unencrypted MP4 — the overwhelming majority — has
//! no `saio` at all.

use crate::{Error, Metadata, MetadataFormat};

/// Atoms whose payload is a list of child atoms, and which therefore have to be
/// descended into when looking for the sample tables.
const CONTAINER_ATOMS: [&[u8; 4]; 6] = [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"edts"];

/// One parsed atom.
#[derive(Clone, Copy)]
struct Atom {
    /// Four-character atom type.
    atom_type: [u8; 4],
    /// Offset of the atom's first byte within the slice it was parsed from.
    start: usize,
    /// Length of the size+type (+extended size) header.
    header_len: usize,
    /// Total on-wire length, header included.
    total_len: usize,
}

impl Atom {
    /// Byte range of the atom's payload within its parent slice.
    const fn payload_range(&self) -> (usize, usize) {
        (self.start + self.header_len, self.start + self.total_len)
    }
}

/// Parses the atoms of `data` in order.
///
/// # Errors
///
/// Returns [`Error::ParseError`] on a truncated or nonsensical atom header.
fn parse_atoms(data: &[u8]) -> Result<Vec<Atom>, Error> {
    let mut atoms = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let mut atom_type = [0u8; 4];
        atom_type.copy_from_slice(&data[pos + 4..pos + 8]);
        let (header_len, total_len) = match size32 {
            // 1 → the real size follows the type as a 64-bit integer.
            1 => {
                if pos + 16 > data.len() {
                    return Err(Error::ParseError(format!(
                        "MP4: truncated 64-bit size field for atom at offset {pos}"
                    )));
                }
                let size64 =
                    u64::from_be_bytes(data[pos + 8..pos + 16].try_into().map_err(|_| {
                        Error::ParseError("MP4: malformed 64-bit atom size".to_string())
                    })?);
                let size = usize::try_from(size64).map_err(|_| {
                    Error::ParseError("MP4: atom size exceeds the addressable range".to_string())
                })?;
                (16usize, size)
            }
            // 0 → the atom runs to the end of the file.
            0 => (8usize, data.len() - pos),
            size => (8usize, size as usize),
        };
        if total_len < header_len || pos + total_len > data.len() {
            return Err(Error::ParseError(format!(
                "MP4: atom '{}' at offset {pos} claims {total_len} bytes but only {} remain",
                String::from_utf8_lossy(&atom_type),
                data.len() - pos
            )));
        }
        atoms.push(Atom {
            atom_type,
            start: pos,
            header_len,
            total_len,
        });
        pos += total_len;
    }
    if atoms.is_empty() {
        return Err(Error::ParseError(
            "MP4: no atoms found (the data is shorter than one atom header)".to_string(),
        ));
    }
    Ok(atoms)
}

/// Builds an atom from its type and payload, using the 32-bit size form.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] if the payload is too large for a 32-bit size
/// field (a metadata atom that big is not something this crate writes).
fn build_atom(atom_type: &[u8; 4], payload: &[u8]) -> Result<Vec<u8>, Error> {
    let total = payload.len() + 8;
    let size = u32::try_from(total).map_err(|_| {
        Error::Unsupported(format!(
            "MP4: the '{}' atom would be {total} bytes, which does not fit a 32-bit size field",
            String::from_utf8_lossy(atom_type)
        ))
    })?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(atom_type);
    out.extend_from_slice(payload);
    Ok(out)
}

/// The canonical iTunes metadata handler atom (`hdlr` with handler type `mdir`).
fn itunes_hdlr() -> Result<Vec<u8>, Error> {
    let mut payload = Vec::with_capacity(25);
    payload.extend_from_slice(&[0u8; 4]); // version + flags
    payload.extend_from_slice(&[0u8; 4]); // pre_defined
    payload.extend_from_slice(b"mdir"); // handler_type: metadata directory
    payload.extend_from_slice(b"appl"); // reserved[0]: Apple's manufacturer code
    payload.extend_from_slice(&[0u8; 8]); // reserved[1..3]
    payload.push(0); // empty handler name
    build_atom(b"hdlr", &payload)
}

/// Rebuilds `moov`'s `udta` sub-tree with the new metadata spliced in.
fn rebuild_udta(
    old_udta_payload: Option<&[u8]>,
    metadata: &Metadata,
    format: MetadataFormat,
) -> Result<Vec<u8>, Error> {
    let existing = match old_udta_payload {
        Some(payload) => parse_atoms(payload).unwrap_or_default(),
        None => Vec::new(),
    };
    let payload = old_udta_payload.unwrap_or(&[]);

    match format {
        MetadataFormat::iTunes => {
            let ilst = build_atom(b"ilst", &crate::itunes::write(metadata)?)?;

            // Rebuild `meta`, preserving every child except the old `ilst`.
            let old_meta = existing.iter().find(|atom| &atom.atom_type == b"meta");
            let mut meta_payload = Vec::new();
            meta_payload.extend_from_slice(&[0u8; 4]); // meta is a FullBox
            let mut kept_hdlr = false;
            if let Some(meta) = old_meta {
                let (start, end) = meta.payload_range();
                // Skip the FullBox version+flags before walking the children.
                let children_start = start + 4;
                if children_start <= end {
                    for child in parse_atoms(&payload[children_start..end]).unwrap_or_default() {
                        if &child.atom_type == b"ilst" {
                            continue;
                        }
                        if &child.atom_type == b"hdlr" {
                            kept_hdlr = true;
                        }
                        meta_payload.extend_from_slice(
                            &payload[children_start + child.start
                                ..children_start + child.start + child.total_len],
                        );
                    }
                }
            }
            if !kept_hdlr {
                meta_payload.extend_from_slice(&itunes_hdlr()?);
            }
            meta_payload.extend_from_slice(&ilst);
            let meta = build_atom(b"meta", &meta_payload)?;

            // Rebuild `udta`, preserving every child except the old `meta`.
            let mut udta_payload = Vec::new();
            for child in &existing {
                if &child.atom_type == b"meta" {
                    continue;
                }
                udta_payload
                    .extend_from_slice(&payload[child.start..child.start + child.total_len]);
            }
            udta_payload.extend_from_slice(&meta);
            build_atom(b"udta", &udta_payload)
        }
        MetadataFormat::QuickTime => {
            // Classic QuickTime user data: the atoms live directly under `udta`.
            let written = crate::quicktime::write(metadata)?;
            // `quicktime::write` silently skips any field it cannot represent, so
            // an all-unrepresentable input would otherwise rebuild `udta` with no
            // tags at all and still report success.
            if written.is_empty() && !metadata.fields().is_empty() {
                let rejected: Vec<&str> = metadata
                    .fields()
                    .iter()
                    .filter(|(key, value)| key.len() != 4 || value.as_text().is_none())
                    .map(|(key, _)| key.as_str())
                    .collect();
                return Err(Error::WriteError(format!(
                    "embed(QuickTime): none of the {} field(s) could be written as a QuickTime \
                     user-data atom, so nothing would have been embedded. An atom type is exactly \
                     four *bytes* and the value must be text — note that \"\u{a9}nam\" is five \
                     bytes in UTF-8 and must be given as the single byte 0xA9 followed by \"nam\". \
                     Rejected: {rejected:?}",
                    metadata.fields().len()
                )));
            }
            let new_atoms = parse_atoms(&written).unwrap_or_default();
            let replaced: Vec<[u8; 4]> = new_atoms.iter().map(|atom| atom.atom_type).collect();

            let mut udta_payload = Vec::new();
            for child in &existing {
                if replaced.contains(&child.atom_type) {
                    continue;
                }
                udta_payload
                    .extend_from_slice(&payload[child.start..child.start + child.total_len]);
            }
            udta_payload.extend_from_slice(&written);
            build_atom(b"udta", &udta_payload)
        }
        other => Err(Error::Unsupported(format!(
            "embed(MP4): {other} is not an atom metadata format"
        ))),
    }
}

/// Rebuilds `moov` with the new `udta` sub-tree.
fn rebuild_moov(
    moov: &[u8],
    metadata: &Metadata,
    format: MetadataFormat,
) -> Result<Vec<u8>, Error> {
    let atoms = parse_atoms(moov)?;
    let moov_atom = atoms
        .first()
        .filter(|atom| &atom.atom_type == b"moov")
        .ok_or_else(|| Error::ParseError("MP4: expected a 'moov' atom".to_string()))?;
    let (start, end) = moov_atom.payload_range();
    let children = parse_atoms(&moov[start..end])?;

    let old_udta = children.iter().find(|atom| &atom.atom_type == b"udta");
    let old_udta_payload = old_udta.map(|atom| {
        let (payload_start, payload_end) = atom.payload_range();
        &moov[start + payload_start..start + payload_end]
    });
    let new_udta = rebuild_udta(old_udta_payload, metadata, format)?;

    let mut payload = Vec::with_capacity(end - start + new_udta.len());
    for child in &children {
        if &child.atom_type == b"udta" {
            continue;
        }
        payload
            .extend_from_slice(&moov[start + child.start..start + child.start + child.total_len]);
    }
    payload.extend_from_slice(&new_udta);
    build_atom(b"moov", &payload)
}

/// Adds `delta` to every `stco`/`co64` entry inside `data`, in place.
///
/// The entries keep their width, so the atom sizes — and therefore the layout
/// this delta was computed from — do not change.
fn shift_chunk_offsets(data: &mut [u8], delta: i64) -> Result<(), Error> {
    let atoms = parse_atoms(data)?;
    for atom in atoms {
        let (start, end) = atom.payload_range();
        if CONTAINER_ATOMS.contains(&&atom.atom_type) {
            shift_chunk_offsets(&mut data[start..end], delta)?;
            continue;
        }
        let is_co64 = &atom.atom_type == b"co64";
        if !is_co64 && &atom.atom_type != b"stco" {
            continue;
        }
        // FullBox: version+flags (4), entry_count (4), then the entries.
        if end - start < 8 {
            return Err(Error::ParseError(format!(
                "MP4: '{}' atom is too short to hold its entry count",
                String::from_utf8_lossy(&atom.atom_type)
            )));
        }
        let count = u32::from_be_bytes([
            data[start + 4],
            data[start + 5],
            data[start + 6],
            data[start + 7],
        ]) as usize;
        let width = if is_co64 { 8usize } else { 4 };
        let entries_start = start + 8;
        if entries_start + count * width > end {
            return Err(Error::ParseError(format!(
                "MP4: '{}' declares {count} entries but the atom is too small",
                String::from_utf8_lossy(&atom.atom_type)
            )));
        }
        for index in 0..count {
            let at = entries_start + index * width;
            if is_co64 {
                let old = u64::from_be_bytes(
                    data[at..at + 8]
                        .try_into()
                        .map_err(|_| Error::ParseError("MP4: malformed co64 entry".to_string()))?,
                );
                let new = u64::try_from(old as i64 + delta).map_err(|_| {
                    Error::ParseError(
                        "MP4: shifting a co64 chunk offset produced a negative value".to_string(),
                    )
                })?;
                data[at..at + 8].copy_from_slice(&new.to_be_bytes());
            } else {
                let old = u32::from_be_bytes(
                    data[at..at + 4]
                        .try_into()
                        .map_err(|_| Error::ParseError("MP4: malformed stco entry".to_string()))?,
                );
                let shifted = i64::from(old) + delta;
                let new = u32::try_from(shifted).map_err(|_| {
                    Error::Unsupported(format!(
                        "MP4: shifting chunk offset {old} by {delta} yields {shifted}, which no \
                         longer fits the 32-bit 'stco' table. Rewriting the table as 'co64' would \
                         change 'moov' size again and is not implemented; re-mux the file with \
                         64-bit chunk offsets instead"
                    ))
                })?;
                data[at..at + 4].copy_from_slice(&new.to_be_bytes());
            }
        }
    }
    Ok(())
}

/// Embeds `metadata` into an MP4/QuickTime file's `moov/udta` sub-tree.
///
/// `format` selects the layout: [`MetadataFormat::iTunes`] writes
/// `udta/meta/ilst` (with the `mdir` handler), [`MetadataFormat::QuickTime`]
/// writes the classic user-data atoms straight into `udta`. Existing children
/// that are not being replaced are preserved.
///
/// # Errors
///
/// * [`Error::Unsupported`] if `file_data` is not an ISO-BMFF file, has no
///   `moov`, is fragmented, or has an `mdat` layout no single chunk-offset delta
///   can describe.
/// * [`Error::ParseError`] if the atom tree is malformed or truncated.
pub fn embed(
    file_data: &[u8],
    metadata: &Metadata,
    format: MetadataFormat,
) -> Result<Vec<u8>, Error> {
    if file_data.len() < 8 {
        return Err(Error::Unsupported(
            "embed(MP4): the given file_data is too short to contain a single atom".to_string(),
        ));
    }
    let atoms = parse_atoms(file_data).map_err(|error| {
        Error::Unsupported(format!(
            "embed(MP4) targets an ISO-BMFF (MP4/MOV) file; the given file_data does not parse as \
             an atom tree ({error})"
        ))
    })?;

    if atoms.iter().any(|atom| &atom.atom_type == b"moof") {
        return Err(Error::Unsupported(
            "embed(MP4): this is a fragmented MP4 ('moof' present). Fragmented files index their \
             media through 'moof'/'sidx' structures whose offsets this splice does not maintain, \
             so tagging one here would produce a file that no longer seeks correctly"
                .to_string(),
        ));
    }

    let moov_index = atoms
        .iter()
        .position(|atom| &atom.atom_type == b"moov")
        .ok_or_else(|| {
            Error::Unsupported(
                "embed(MP4): the file has no 'moov' atom, so there is no movie header to attach \
                 metadata to"
                    .to_string(),
            )
        })?;
    let moov = &atoms[moov_index];
    let new_moov = rebuild_moov(
        &file_data[moov.start..moov.start + moov.total_len],
        metadata,
        format,
    )?;
    let delta = new_moov.len() as i64 - moov.total_len as i64;

    // ── Decide whether the media data moves, and fix chunk offsets if so ─────
    let mdat_positions: Vec<usize> = atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| &atom.atom_type == b"mdat")
        .map(|(index, _)| index)
        .collect();
    let mut new_moov = new_moov;
    if delta != 0 && !mdat_positions.is_empty() {
        let all_after = mdat_positions.iter().all(|index| *index > moov_index);
        let all_before = mdat_positions.iter().all(|index| *index < moov_index);
        if all_after {
            shift_chunk_offsets(&mut new_moov, delta)?;
        } else if !all_before {
            return Err(Error::Unsupported(
                "embed(MP4): the file has 'mdat' atoms both before and after 'moov', so resizing \
                 'moov' moves some media data and not the rest; no single chunk-offset delta is \
                 correct. Re-mux the file so all media data sits on one side of 'moov'"
                    .to_string(),
            ));
        }
    }

    let mut out = Vec::with_capacity(file_data.len() + new_moov.len());
    for (index, atom) in atoms.iter().enumerate() {
        if index == moov_index {
            out.extend_from_slice(&new_moov);
        } else {
            out.extend_from_slice(&file_data[atom.start..atom.start + atom.total_len]);
        }
    }
    Ok(out)
}

/// Extracts the `ilst` payload (iTunes) or `udta` payload (QuickTime) from an
/// MP4/QuickTime file, ready for [`crate::itunes::parse`] or
/// [`crate::quicktime::parse`].
///
/// This is the inverse of [`embed`] and what the round-trip tests read back.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if the atom tree is malformed.
pub fn extract(file_data: &[u8], format: MetadataFormat) -> Result<Option<Vec<u8>>, Error> {
    let atoms = parse_atoms(file_data)?;
    let Some(moov) = atoms.iter().find(|atom| &atom.atom_type == b"moov") else {
        return Ok(None);
    };
    let (moov_start, moov_end) = moov.payload_range();
    let moov_payload = &file_data[moov_start..moov_end];
    let Some(udta) = parse_atoms(moov_payload)?
        .into_iter()
        .find(|atom| &atom.atom_type == b"udta")
    else {
        return Ok(None);
    };
    let (udta_start, udta_end) = udta.payload_range();
    let udta_payload = &moov_payload[udta_start..udta_end];

    if format == MetadataFormat::QuickTime {
        return Ok(Some(udta_payload.to_vec()));
    }

    let Some(meta) = parse_atoms(udta_payload)?
        .into_iter()
        .find(|atom| &atom.atom_type == b"meta")
    else {
        return Ok(None);
    };
    let (meta_start, meta_end) = meta.payload_range();
    // Skip the FullBox version+flags to reach `meta`'s children.
    let children = &udta_payload[meta_start + 4..meta_end];
    let Some(ilst) = parse_atoms(children)?
        .into_iter()
        .find(|atom| &atom.atom_type == b"ilst")
    else {
        return Ok(None);
    };
    let (ilst_start, ilst_end) = ilst.payload_range();
    Ok(Some(children[ilst_start..ilst_end].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetadataValue;

    /// Builds an `stbl` holding an `stco` with the given chunk offsets.
    fn stbl_with_stco(offsets: &[u32]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0u8; 4]); // version + flags
        payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
        for offset in offsets {
            payload.extend_from_slice(&offset.to_be_bytes());
        }
        let stco = build_atom(b"stco", &payload).expect("stco");
        build_atom(b"stbl", &stco).expect("stbl")
    }

    /// Builds `moov` containing one track whose sample table points at `offsets`.
    fn moov_with_offsets(offsets: &[u32]) -> Vec<u8> {
        let stbl = stbl_with_stco(offsets);
        let minf = build_atom(b"minf", &stbl).expect("minf");
        let mdia = build_atom(b"mdia", &minf).expect("mdia");
        let trak = build_atom(b"trak", &mdia).expect("trak");
        let mut payload = build_atom(b"mvhd", &[0u8; 100]).expect("mvhd");
        payload.extend_from_slice(&trak);
        build_atom(b"moov", &payload).expect("moov")
    }

    /// Assembles a file: `ftyp`, then the atoms given, in order.
    fn mp4_file(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut out = build_atom(b"ftyp", b"isom\0\0\x02\0isomiso2").expect("ftyp");
        for part in parts {
            out.extend_from_slice(part);
        }
        out
    }

    fn mdat(payload: &[u8]) -> Vec<u8> {
        build_atom(b"mdat", payload).expect("mdat")
    }

    fn itunes_metadata() -> Metadata {
        let mut metadata = Metadata::new(MetadataFormat::iTunes);
        // Four-byte ASCII atom names: the crate's iTunes writer requires exactly
        // four *bytes*, which the copyright-sign atoms ("\u{a9}nam") are not.
        metadata.insert(
            "desc".to_string(),
            MetadataValue::Text("Track Description".to_string()),
        );
        metadata.insert(
            "cprt".to_string(),
            MetadataValue::Text("(c) COOLJAPAN".to_string()),
        );
        metadata
    }

    /// Reads every `stco` entry out of a file.
    fn read_chunk_offsets(data: &[u8]) -> Vec<u32> {
        fn walk(data: &[u8], out: &mut Vec<u32>) {
            let Ok(atoms) = parse_atoms(data) else {
                return;
            };
            for atom in atoms {
                let (start, end) = atom.payload_range();
                if CONTAINER_ATOMS.contains(&&atom.atom_type) {
                    walk(&data[start..end], out);
                } else if &atom.atom_type == b"stco" {
                    let count = u32::from_be_bytes([
                        data[start + 4],
                        data[start + 5],
                        data[start + 6],
                        data[start + 7],
                    ]) as usize;
                    for index in 0..count {
                        let at = start + 8 + index * 4;
                        out.push(u32::from_be_bytes([
                            data[at],
                            data[at + 1],
                            data[at + 2],
                            data[at + 3],
                        ]));
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(data, &mut out);
        out
    }

    #[test]
    fn test_itunes_round_trips_through_the_crate_parser() {
        let file = mp4_file(&[moov_with_offsets(&[]), mdat(b"MEDIA DATA")]);
        let out = embed(&file, &itunes_metadata(), MetadataFormat::iTunes).expect("embed");

        let ilst = extract(&out, MetadataFormat::iTunes)
            .expect("extract")
            .expect("ilst present");
        let parsed = crate::itunes::parse(&ilst).expect("parse ilst");
        assert_eq!(
            parsed.get("desc").and_then(MetadataValue::as_text),
            Some("Track Description")
        );
        assert_eq!(
            parsed.get("cprt").and_then(MetadataValue::as_text),
            Some("(c) COOLJAPAN")
        );
    }

    #[test]
    fn test_atom_sizes_are_consistent_after_embedding() {
        let file = mp4_file(&[moov_with_offsets(&[]), mdat(b"MEDIA")]);
        let out = embed(&file, &itunes_metadata(), MetadataFormat::iTunes).expect("embed");

        // Every level of the tree must describe exactly the bytes it contains.
        let atoms = parse_atoms(&out).expect("top level");
        let total: usize = atoms.iter().map(|atom| atom.total_len).sum();
        assert_eq!(total, out.len(), "top-level atoms must tile the whole file");

        let moov = atoms
            .iter()
            .find(|atom| &atom.atom_type == b"moov")
            .expect("moov");
        let (start, end) = moov.payload_range();
        let children = parse_atoms(&out[start..end]).expect("moov children");
        let child_total: usize = children.iter().map(|atom| atom.total_len).sum();
        assert_eq!(
            child_total,
            end - start,
            "moov children must tile its payload"
        );
    }

    #[test]
    fn test_chunk_offsets_shift_when_moov_precedes_mdat() {
        let media_offset = {
            let ftyp = build_atom(b"ftyp", b"isom\0\0\x02\0isomiso2").expect("ftyp");
            let moov = moov_with_offsets(&[0]);
            (ftyp.len() + moov.len() + 8) as u32 // +8: past the mdat header
        };
        let file = mp4_file(&[moov_with_offsets(&[media_offset]), mdat(b"MEDIA DATA")]);
        assert_eq!(read_chunk_offsets(&file), vec![media_offset]);
        assert_eq!(
            &file[media_offset as usize..media_offset as usize + 5],
            b"MEDIA",
            "the fixture's chunk offset must really point at the media"
        );

        let out = embed(&file, &itunes_metadata(), MetadataFormat::iTunes).expect("embed");
        let new_offsets = read_chunk_offsets(&out);
        assert_eq!(new_offsets.len(), 1);
        assert_ne!(
            new_offsets[0], media_offset,
            "moov grew, so the chunk offset must have been patched"
        );
        assert_eq!(
            &out[new_offsets[0] as usize..new_offsets[0] as usize + 5],
            b"MEDIA",
            "the patched chunk offset must still point at the media data"
        );
    }

    #[test]
    fn test_chunk_offsets_are_untouched_when_mdat_precedes_moov() {
        let media_offset = {
            let ftyp = build_atom(b"ftyp", b"isom\0\0\x02\0isomiso2").expect("ftyp");
            (ftyp.len() + 8) as u32
        };
        let file = mp4_file(&[mdat(b"MEDIA DATA"), moov_with_offsets(&[media_offset])]);
        assert_eq!(
            &file[media_offset as usize..media_offset as usize + 5],
            b"MEDIA"
        );

        let out = embed(&file, &itunes_metadata(), MetadataFormat::iTunes).expect("embed");
        assert_eq!(
            read_chunk_offsets(&out),
            vec![media_offset],
            "media data before moov does not move, so offsets must not be touched"
        );
        assert_eq!(
            &out[media_offset as usize..media_offset as usize + 5],
            b"MEDIA"
        );
    }

    #[test]
    fn test_existing_udta_children_are_preserved() {
        let extra = build_atom(b"cprt", b"(c) COOLJAPAN").expect("cprt");
        let udta = build_atom(b"udta", &extra).expect("udta");
        let mut moov_payload = build_atom(b"mvhd", &[0u8; 100]).expect("mvhd");
        moov_payload.extend_from_slice(&udta);
        let moov = build_atom(b"moov", &moov_payload).expect("moov");
        let file = mp4_file(&[moov, mdat(b"MEDIA")]);

        let out = embed(&file, &itunes_metadata(), MetadataFormat::iTunes).expect("embed");
        let moov_atoms = parse_atoms(&out).expect("atoms");
        let moov = moov_atoms
            .iter()
            .find(|atom| &atom.atom_type == b"moov")
            .expect("moov");
        let (start, end) = moov.payload_range();
        let udta = parse_atoms(&out[start..end])
            .expect("children")
            .into_iter()
            .find(|atom| &atom.atom_type == b"udta")
            .expect("udta");
        let (udta_start, udta_end) = udta.payload_range();
        let children = parse_atoms(&out[start + udta_start..start + udta_end]).expect("udta kids");
        let types: Vec<String> = children
            .iter()
            .map(|atom| String::from_utf8_lossy(&atom.atom_type).into_owned())
            .collect();
        assert!(types.contains(&"cprt".to_string()), "got {types:?}");
        assert!(types.contains(&"meta".to_string()), "got {types:?}");
    }

    #[test]
    fn test_reembedding_replaces_rather_than_duplicates() {
        let file = mp4_file(&[moov_with_offsets(&[]), mdat(b"MEDIA")]);
        let first = embed(&file, &itunes_metadata(), MetadataFormat::iTunes).expect("first");

        let mut second_metadata = Metadata::new(MetadataFormat::iTunes);
        second_metadata.insert(
            "desc".to_string(),
            MetadataValue::Text("Replaced".to_string()),
        );
        let second = embed(&first, &second_metadata, MetadataFormat::iTunes).expect("second embed");

        let ilst = extract(&second, MetadataFormat::iTunes)
            .expect("extract")
            .expect("ilst");
        let parsed = crate::itunes::parse(&ilst).expect("parse");
        assert_eq!(
            parsed.get("desc").and_then(MetadataValue::as_text),
            Some("Replaced")
        );
        assert!(
            parsed.get("cprt").is_none(),
            "the replaced ilst must not retain stale atoms"
        );

        // And exactly one meta/ilst pair exists.
        let atoms = parse_atoms(&second).expect("atoms");
        let moov = atoms
            .iter()
            .find(|atom| &atom.atom_type == b"moov")
            .expect("moov");
        let (start, end) = moov.payload_range();
        let udta_count = parse_atoms(&second[start..end])
            .expect("children")
            .iter()
            .filter(|atom| &atom.atom_type == b"udta")
            .count();
        assert_eq!(udta_count, 1);
    }

    #[test]
    fn test_quicktime_layout_writes_udta_children_directly() {
        let file = mp4_file(&[moov_with_offsets(&[]), mdat(b"MEDIA")]);
        let mut metadata = Metadata::new(MetadataFormat::QuickTime);
        metadata.insert(
            "cmnt".to_string(),
            MetadataValue::Text("A comment".to_string()),
        );
        let out = embed(&file, &metadata, MetadataFormat::QuickTime).expect("embed");

        let udta = extract(&out, MetadataFormat::QuickTime)
            .expect("extract")
            .expect("udta present");
        let parsed = crate::quicktime::parse(&udta).expect("parse udta");
        assert_eq!(
            parsed.get("cmnt").and_then(MetadataValue::as_text),
            Some("A comment")
        );
    }

    #[test]
    fn test_quicktime_with_unrepresentable_keys_is_an_honest_error() {
        // "\u{a9}nam" is five bytes in UTF-8, so the QuickTime writer skips it.
        // Rebuilding `udta` with no tags at all and reporting success would be a
        // silent no-op.
        let file = mp4_file(&[moov_with_offsets(&[]), mdat(b"MEDIA")]);
        let mut metadata = Metadata::new(MetadataFormat::QuickTime);
        metadata.insert(
            "\u{a9}nam".to_string(),
            MetadataValue::Text("Title".to_string()),
        );
        metadata.insert("title".to_string(), MetadataValue::Text("T".to_string()));

        match embed(&file, &metadata, MetadataFormat::QuickTime) {
            Err(Error::WriteError(message)) => {
                assert!(message.contains("four"), "message was: {message}");
                assert!(message.contains("title"), "message was: {message}");
            }
            other => panic!("expected an honest WriteError, got {other:?}"),
        }
    }

    #[test]
    fn test_fragmented_mp4_is_an_honest_error() {
        let file = mp4_file(&[
            moov_with_offsets(&[]),
            build_atom(b"moof", &[0u8; 16]).expect("moof"),
            mdat(b"MEDIA"),
        ]);
        match embed(&file, &itunes_metadata(), MetadataFormat::iTunes) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("fragmented"), "message was: {message}");
            }
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn test_mdat_on_both_sides_of_moov_is_an_honest_error() {
        let file = mp4_file(&[mdat(b"BEFORE"), moov_with_offsets(&[]), mdat(b"AFTER")]);
        match embed(&file, &itunes_metadata(), MetadataFormat::iTunes) {
            Err(Error::Unsupported(message)) => {
                assert!(
                    message.contains("both before and after"),
                    "message was: {message}"
                );
            }
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn test_missing_moov_is_an_honest_error() {
        let file = mp4_file(&[mdat(b"MEDIA")]);
        match embed(&file, &itunes_metadata(), MetadataFormat::iTunes) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("'moov'"), "message was: {message}");
            }
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn test_non_mp4_input_is_rejected_without_modification() {
        let input = b"this is definitely not an ISO-BMFF file".to_vec();
        let copy = input.clone();
        assert!(matches!(
            embed(&input, &itunes_metadata(), MetadataFormat::iTunes),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(input, copy);
    }

    #[test]
    fn test_stco_overflow_is_an_honest_error() {
        // A chunk offset close enough to u32::MAX that growing moov overflows it.
        let file = mp4_file(&[moov_with_offsets(&[u32::MAX - 4]), mdat(b"MEDIA")]);
        match embed(&file, &itunes_metadata(), MetadataFormat::iTunes) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("co64"), "message was: {message}");
            }
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }
}

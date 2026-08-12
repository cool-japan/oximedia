//! JPEG marker-segment machinery shared by the Exif, XMP and IPTC embed paths.
//!
//! A JPEG file is `SOI` followed by a run of marker segments and finally the
//! entropy-coded scan (`SOS` …) and `EOI`. Every metadata format this crate
//! embeds into JPEG lives in one of those header segments:
//!
//! | Format | Marker | Payload identifier                     |
//! |--------|--------|----------------------------------------|
//! | Exif   | `APP1` | `"Exif\0\0"`                           |
//! | XMP    | `APP1` | `"http://ns.adobe.com/xap/1.0/\0"`     |
//! | IPTC   | `APP13`| `"Photoshop 3.0\0"` + 8BIM resource(s) |
//!
//! `splice_segments` is the single primitive all of them use: it removes the
//! segments a caller selects, inserts a new run at the right place, and copies
//! everything else — including the entropy-coded scan — byte for byte. Nothing
//! in this module ever mutates its input, so a parse failure leaves the caller's
//! bytes untouched by construction.

use crate::{Error, Metadata};

/// JPEG `APP0` marker byte (JFIF), following the `0xFF` marker prefix.
pub(super) const JPEG_APP0: u8 = 0xE0;
/// JPEG `APP1` marker byte (Exif / XMP), following the `0xFF` marker prefix.
pub(super) const JPEG_APP1: u8 = 0xE1;
/// JPEG `APP13` marker byte (Photoshop Image Resource Blocks / IPTC-IIM).
pub(super) const JPEG_APP13: u8 = 0xED;
/// JPEG start-of-scan marker byte: header segments never continue past this point.
pub(super) const JPEG_SOS: u8 = 0xDA;
/// JPEG end-of-image marker byte.
pub(super) const JPEG_EOI: u8 = 0xD9;

/// Largest payload a single JPEG marker segment can carry.
///
/// The 16-bit length field counts itself, so the payload is capped at
/// `0xFFFF - 2` bytes.
pub(super) const MAX_SEGMENT_PAYLOAD: usize = 0xFFFF - 2;

/// Identifier prefix of an APP13 segment carrying Photoshop Image Resource Blocks.
pub(super) const PHOTOSHOP_APP13_PREFIX: &[u8] = b"Photoshop 3.0\0";

/// Image Resource Block signature.
const IRB_SIGNATURE: &[u8; 4] = b"8BIM";

/// Photoshop Image Resource ID for an IPTC-IIM (NAA) record.
const IRB_ID_IPTC_IIM: u16 = 0x0404;

/// `true` when `data` begins with a JPEG Start-Of-Image marker (`FF D8 FF`).
pub(super) fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

/// One parsed JPEG header segment.
pub(super) struct Segment {
    /// Marker code byte (the byte after `0xFF`).
    pub(super) marker: u8,
    /// Byte offset of the segment's leading `0xFF`.
    pub(super) start: usize,
    /// One past the segment's last byte.
    pub(super) end: usize,
    /// Byte offset of the payload (just past the 2-byte length field).
    pub(super) payload_start: usize,
}

impl Segment {
    /// The segment's payload bytes within `file_data`.
    pub(super) fn payload<'a>(&self, file_data: &'a [u8]) -> &'a [u8] {
        &file_data[self.payload_start..self.end]
    }
}

/// Parses the run of marker segments between `SOI` and the scan.
///
/// Returns the segments in file order plus the offset at which the scan
/// (`SOS`/`EOI`) begins — the point past which nothing may be moved.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if `file_data` is not a JPEG or a segment is
/// malformed/truncated.
pub(super) fn parse_segments(file_data: &[u8]) -> Result<(Vec<Segment>, usize), Error> {
    if !is_jpeg(file_data) {
        return Err(Error::ParseError(
            "Not a JPEG file (missing SOI marker)".to_string(),
        ));
    }

    let mut segments = Vec::new();
    let mut pos = 2usize; // Just past SOI.
    loop {
        if pos + 2 > file_data.len() {
            return Err(Error::ParseError(
                "Truncated JPEG: ran out of data while scanning header segments".to_string(),
            ));
        }
        if file_data[pos] != 0xFF {
            return Err(Error::ParseError(format!(
                "Malformed JPEG: expected a marker (0xFF) at offset {pos}"
            )));
        }
        // Skip fill bytes (0xFF repeated) before the real marker code byte.
        let mut marker_pos = pos;
        while marker_pos + 1 < file_data.len() && file_data[marker_pos + 1] == 0xFF {
            marker_pos += 1;
        }
        let marker = file_data[marker_pos + 1];
        let header_end = marker_pos + 2;

        if marker == JPEG_SOS || marker == JPEG_EOI {
            return Ok((segments, pos));
        }
        // Standalone markers (TEM, RSTn) carry no length field.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            pos = header_end;
            continue;
        }

        if header_end + 2 > file_data.len() {
            return Err(Error::ParseError(
                "Truncated JPEG: segment length field missing".to_string(),
            ));
        }
        let seg_len = usize::from(u16::from_be_bytes([
            file_data[header_end],
            file_data[header_end + 1],
        ]));
        if seg_len < 2 || header_end + seg_len > file_data.len() {
            return Err(Error::ParseError(format!(
                "Malformed JPEG: invalid segment length at offset {header_end}"
            )));
        }
        segments.push(Segment {
            marker,
            start: pos,
            end: header_end + seg_len,
            payload_start: header_end + 2,
        });
        pos = header_end + seg_len;
    }
}

/// Serializes one marker segment: `FF <marker> <len:u16be> <payload>`.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] if `payload` exceeds [`MAX_SEGMENT_PAYLOAD`].
pub(super) fn encode_segment(marker: u8, payload: &[u8]) -> Result<Vec<u8>, Error> {
    if payload.len() > MAX_SEGMENT_PAYLOAD {
        return Err(Error::Unsupported(format!(
            "JPEG segment payload is {} bytes, which does not fit in a single marker segment \
             (max {MAX_SEGMENT_PAYLOAD} bytes)",
            payload.len()
        )));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.push(0xFF);
    out.push(marker);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Rewrites a JPEG's header-segment run: drops the segments `remove` selects,
/// inserts `new_segments` in their place, and copies everything else verbatim.
///
/// The insertion point is the position of the first removed segment (so an
/// update lands exactly where the old data was); when nothing is removed, the
/// new run is inserted after the leading run of `insert_after` markers — the
/// position mandated for Exif's `APP1` and conventional for XMP and APP13.
///
/// # Errors
///
/// Returns [`Error::ParseError`] for a malformed JPEG and [`Error::Unsupported`]
/// if any new payload exceeds one segment.
pub(super) fn splice_segments(
    file_data: &[u8],
    remove: &dyn Fn(u8, &[u8]) -> bool,
    new_segments: &[(u8, Vec<u8>)],
    insert_after: &[u8],
) -> Result<Vec<u8>, Error> {
    let (segments, scan_start) = parse_segments(file_data)?;

    let removed: Vec<bool> = segments
        .iter()
        .map(|s| remove(s.marker, s.payload(file_data)))
        .collect();

    // Insertion point: where the first replaced segment sat, else after the
    // leading run of `insert_after` markers.
    let insert_index = removed.iter().position(|r| *r).unwrap_or_else(|| {
        segments
            .iter()
            .position(|s| !insert_after.contains(&s.marker))
            .unwrap_or(segments.len())
    });

    let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(new_segments.len());
    for (marker, payload) in new_segments {
        encoded.push(encode_segment(*marker, payload)?);
    }

    let mut out = Vec::with_capacity(file_data.len() + encoded.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&file_data[..2]); // SOI
    for (i, segment) in segments.iter().enumerate() {
        if i == insert_index {
            for bytes in &encoded {
                out.extend_from_slice(bytes);
            }
        }
        if !removed[i] {
            out.extend_from_slice(&file_data[segment.start..segment.end]);
        }
    }
    if insert_index >= segments.len() {
        for bytes in &encoded {
            out.extend_from_slice(bytes);
        }
    }
    out.extend_from_slice(&file_data[scan_start..]);
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// APP13 / Photoshop Image Resource Blocks (IPTC-IIM)
// ─────────────────────────────────────────────────────────────────────────────

/// One Photoshop Image Resource Block.
struct ResourceBlock {
    /// Resource identifier (`0x0404` for IPTC-IIM).
    id: u16,
    /// Pascal-string resource name (without the length byte or padding).
    name: Vec<u8>,
    /// Resource body.
    data: Vec<u8>,
}

/// Parses an 8BIM Image Resource Block stream (APP13 identifiers already
/// stripped and, for a multi-segment stream, concatenated).
///
/// Returns the blocks together with **how many bytes were consumed**, so a
/// caller can tell a clean end-of-stream from a malformed tail. Silently
/// returning the blocks parsed so far would mean rewriting the segment without
/// whatever the tail held — losing data while reporting success.
fn parse_resource_blocks(data: &[u8]) -> (Vec<ResourceBlock>, usize) {
    let mut blocks = Vec::new();
    let mut consumed = 0usize;
    let mut data = data;
    while data.len() >= 12 && &data[..4] == IRB_SIGNATURE {
        let id = u16::from_be_bytes([data[4], data[5]]);
        let name_len = usize::from(data[6]);
        // The Pascal string (length byte + bytes) is padded to an even total.
        let name_field = if (name_len + 1) % 2 == 0 {
            name_len + 1
        } else {
            name_len + 2
        };
        let name_start = 7;
        let name_end = 6 + name_field;
        if name_end + 4 > data.len() {
            break;
        }
        let name = data[name_start..name_start + name_len].to_vec();
        let size = u32::from_be_bytes([
            data[name_end],
            data[name_end + 1],
            data[name_end + 2],
            data[name_end + 3],
        ]) as usize;
        let body_start = name_end + 4;
        let Some(body_end) = body_start.checked_add(size) else {
            break;
        };
        if body_end > data.len() {
            break;
        }
        blocks.push(ResourceBlock {
            id,
            name,
            data: data[body_start..body_end].to_vec(),
        });
        // Resource bodies are padded to an even length.
        let advance = body_end + usize::from(size % 2 == 1);
        if advance > data.len() {
            break;
        }
        data = &data[advance..];
        consumed += advance;
    }
    (blocks, consumed)
}

/// Concatenates the Image Resource Block streams of every `"Photoshop 3.0"`
/// `APP13` segment in a JPEG.
///
/// Photoshop splits an oversized resource stream across consecutive `APP13`
/// segments at arbitrary byte boundaries — a block may straddle the split — so
/// the segments have to be joined *before* parsing. Parsing each segment on its
/// own would drop the straddling block and everything after it.
fn collect_irb_stream(file_data: &[u8], segments: &[Segment]) -> Vec<u8> {
    let mut stream = Vec::new();
    for segment in segments {
        if segment.marker != JPEG_APP13 {
            continue;
        }
        let payload = segment.payload(file_data);
        if payload.starts_with(PHOTOSHOP_APP13_PREFIX) {
            stream.extend_from_slice(&payload[PHOTOSHOP_APP13_PREFIX.len()..]);
        }
    }
    stream
}

/// Parses a concatenated IRB stream, refusing to proceed on a malformed tail.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if the stream does not parse to its end, since
/// rewriting it would discard the unparsed remainder.
fn parse_irb_stream_strict(stream: &[u8]) -> Result<Vec<ResourceBlock>, Error> {
    let (blocks, consumed) = parse_resource_blocks(stream);
    if consumed != stream.len() {
        return Err(Error::ParseError(format!(
            "Malformed Photoshop APP13 data: {} of {} bytes parsed as 8BIM Image Resource Blocks; \
             rewriting the segment would discard the remaining {} bytes",
            consumed,
            stream.len(),
            stream.len() - consumed
        )));
    }
    Ok(blocks)
}

/// Serializes one 8BIM resource block with correct Pascal-string and body padding.
fn encode_resource_block(out: &mut Vec<u8>, block: &ResourceBlock) {
    out.extend_from_slice(IRB_SIGNATURE);
    out.extend_from_slice(&block.id.to_be_bytes());
    // Pascal string: length byte + bytes, padded so the field length is even.
    let name_len = block.name.len().min(255);
    out.push(name_len as u8);
    out.extend_from_slice(&block.name[..name_len]);
    if (name_len + 1) % 2 == 1 {
        out.push(0);
    }
    out.extend_from_slice(&(block.data.len() as u32).to_be_bytes());
    out.extend_from_slice(&block.data);
    if block.data.len() % 2 == 1 {
        out.push(0);
    }
}

/// Embeds `metadata` as an IPTC-IIM record inside a JPEG `APP13` segment.
///
/// The IIM datasets are produced by [`crate::iptc::write`] and wrapped in a
/// Photoshop Image Resource Block with resource ID `0x0404`, which is how IPTC
/// data is carried in JPEG. If the file already has a `"Photoshop 3.0"` APP13
/// segment, **all of its other resource blocks are preserved** and only the
/// `0x0404` block is replaced; the rebuilt segment is written back in the same
/// position.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if `file_data` is not a well-formed JPEG and
/// [`Error::Unsupported`] if the resulting APP13 segment would exceed the 64 KiB
/// JPEG segment limit (Photoshop splits huge IRB data across consecutive APP13
/// segments; this crate does not write payloads that large).
pub fn embed(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    if !is_jpeg(file_data) {
        return Err(Error::Unsupported(
            "embed(Iptc) targets a JPEG APP13 \"Photoshop 3.0\" segment; the given file_data is \
             not a JPEG, and naive concatenation would corrupt it"
                .to_string(),
        ));
    }

    let iim = crate::iptc::write(metadata)?;

    // Preserve every pre-existing resource block except the IPTC one. The
    // segments are joined first: a resource block may straddle an APP13 split.
    let (segments, _) = parse_segments(file_data)?;
    let mut blocks: Vec<ResourceBlock> =
        parse_irb_stream_strict(&collect_irb_stream(file_data, &segments))?
            .into_iter()
            .filter(|b| b.id != IRB_ID_IPTC_IIM)
            .collect();

    blocks.push(ResourceBlock {
        id: IRB_ID_IPTC_IIM,
        name: Vec::new(),
        data: iim,
    });
    // Emit resource blocks in ascending ID order so the output is deterministic
    // regardless of the order they were found in. Image Resource Blocks are
    // looked up by ID, so their order carries no meaning.
    blocks.sort_by_key(|b| b.id);

    let mut payload = Vec::with_capacity(PHOTOSHOP_APP13_PREFIX.len() + 64);
    payload.extend_from_slice(PHOTOSHOP_APP13_PREFIX);
    for block in &blocks {
        encode_resource_block(&mut payload, block);
    }

    if payload.len() > MAX_SEGMENT_PAYLOAD {
        return Err(Error::Unsupported(format!(
            "the IPTC APP13 payload is {} bytes, which exceeds the {MAX_SEGMENT_PAYLOAD}-byte JPEG \
             segment limit; splitting Photoshop Image Resource Blocks across consecutive APP13 \
             segments is not implemented",
            payload.len()
        )));
    }

    splice_segments(
        file_data,
        &|marker, payload| marker == JPEG_APP13 && payload.starts_with(PHOTOSHOP_APP13_PREFIX),
        &[(JPEG_APP13, payload)],
        &[JPEG_APP0, JPEG_APP1],
    )
}

/// Extracts the IPTC-IIM dataset stream from a JPEG's `APP13` segment, if present.
///
/// This is the inverse of [`embed`] and the helper the round-trip tests use.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if `file_data` is not a well-formed JPEG.
pub fn extract_iim(file_data: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let (segments, _) = parse_segments(file_data)?;
    let stream = collect_irb_stream(file_data, &segments);
    if stream.is_empty() {
        return Ok(None);
    }
    Ok(parse_irb_stream_strict(&stream)?
        .into_iter()
        .find(|block| block.id == IRB_ID_IPTC_IIM)
        .map(|block| block.data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::tests::minimal_jpeg;
    use crate::{MetadataFormat, MetadataValue};

    fn iptc_metadata() -> Metadata {
        let mut metadata = Metadata::new(MetadataFormat::Iptc);
        metadata.insert(
            "By-line".to_string(),
            MetadataValue::Text("Jane Doe".to_string()),
        );
        metadata.insert(
            "CopyrightNotice".to_string(),
            MetadataValue::Text("(c) 2026 COOLJAPAN".to_string()),
        );
        metadata
    }

    #[test]
    fn test_iptc_app13_round_trips_through_the_crate_parser() {
        let jpeg = minimal_jpeg();
        let embedded = embed(&jpeg, &iptc_metadata()).expect("embed iptc into jpeg");

        assert_eq!(&embedded[0..2], &[0xFF, 0xD8]);
        assert_eq!(&embedded[embedded.len() - 2..], &[0xFF, 0xD9]);

        let iim = extract_iim(&embedded)
            .expect("parse embedded jpeg")
            .expect("APP13 IPTC block present");
        let parsed = crate::iptc::parse(&iim).expect("parse IIM datasets");
        assert_eq!(
            parsed.get("By-line").and_then(MetadataValue::as_text),
            Some("Jane Doe")
        );
        assert_eq!(
            parsed
                .get("CopyrightNotice")
                .and_then(MetadataValue::as_text),
            Some("(c) 2026 COOLJAPAN")
        );
    }

    #[test]
    fn test_iptc_app13_is_inserted_after_the_app0_app1_run() {
        let jpeg = minimal_jpeg();
        let embedded = embed(&jpeg, &iptc_metadata()).expect("embed");
        let (segments, _) = parse_segments(&embedded).expect("parse");
        let markers: Vec<u8> = segments.iter().map(|s| s.marker).collect();
        assert_eq!(
            markers,
            vec![JPEG_APP0, JPEG_APP13],
            "APP13 must follow the leading APP0 run"
        );
    }

    #[test]
    fn test_iptc_replace_preserves_other_8bim_resources() {
        // Build an APP13 payload with a foreign resource (0x040A) plus an IPTC one.
        let mut payload = Vec::new();
        payload.extend_from_slice(PHOTOSHOP_APP13_PREFIX);
        encode_resource_block(
            &mut payload,
            &ResourceBlock {
                id: 0x040A,
                name: b"custom".to_vec(),
                data: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01], // odd length → padded
            },
        );
        encode_resource_block(
            &mut payload,
            &ResourceBlock {
                id: IRB_ID_IPTC_IIM,
                name: Vec::new(),
                data: crate::iptc::write(&{
                    let mut m = Metadata::new(MetadataFormat::Iptc);
                    m.insert(
                        "City".to_string(),
                        MetadataValue::Text("Tallinn".to_string()),
                    );
                    m
                })
                .expect("encode old iim"),
            },
        );
        let jpeg = minimal_jpeg();
        let with_app13 =
            splice_segments(&jpeg, &|_, _| false, &[(JPEG_APP13, payload)], &[JPEG_APP0])
                .expect("insert original APP13");

        let updated = embed(&with_app13, &iptc_metadata()).expect("replace IPTC block");

        // Exactly one APP13 segment, carrying both the foreign resource and the new IPTC one.
        let (segments, _) = parse_segments(&updated).expect("parse");
        let app13: Vec<&Segment> = segments.iter().filter(|s| s.marker == JPEG_APP13).collect();
        assert_eq!(app13.len(), 1, "expected exactly one APP13 segment");

        let blocks = parse_irb_stream_strict(&collect_irb_stream(&updated, &segments))
            .expect("resource stream parses to its end");
        let ids: Vec<u16> = blocks.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![IRB_ID_IPTC_IIM, 0x040A]);
        let foreign = blocks
            .iter()
            .find(|b| b.id == 0x040A)
            .expect("foreign resource preserved");
        assert_eq!(foreign.name, b"custom".to_vec());
        assert_eq!(foreign.data, vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01]);

        let iim = extract_iim(&updated).expect("parse").expect("iptc present");
        let parsed = crate::iptc::parse(&iim).expect("parse IIM");
        assert_eq!(
            parsed.get("By-line").and_then(MetadataValue::as_text),
            Some("Jane Doe")
        );
        assert!(
            parsed.get("City").is_none(),
            "the replaced IPTC record must not retain stale datasets"
        );
    }

    #[test]
    fn test_resource_block_padding_round_trip() {
        for (name, data_len) in [
            (b"".to_vec(), 0usize),
            (b"".to_vec(), 1),
            (b"x".to_vec(), 3),
            (b"ab".to_vec(), 4),
        ] {
            let block = ResourceBlock {
                id: 0x0404,
                name: name.clone(),
                data: vec![0x5A; data_len],
            };
            let mut encoded = Vec::new();
            encode_resource_block(&mut encoded, &block);
            assert_eq!(
                encoded.len() % 2,
                0,
                "an 8BIM block is always an even number of bytes"
            );
            let (parsed, consumed) = parse_resource_blocks(&encoded);
            assert_eq!(consumed, encoded.len(), "the whole block must be consumed");
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0].id, 0x0404);
            assert_eq!(parsed[0].name, name);
            assert_eq!(parsed[0].data.len(), data_len);
        }
    }

    #[test]
    fn test_resources_split_across_two_app13_segments_are_not_lost() {
        // Photoshop splits an oversized resource stream at an arbitrary byte
        // boundary, so a block can straddle two APP13 segments.
        let mut stream = Vec::new();
        encode_resource_block(
            &mut stream,
            &ResourceBlock {
                id: 0x040B,
                name: Vec::new(),
                data: vec![0x11; 40],
            },
        );
        encode_resource_block(
            &mut stream,
            &ResourceBlock {
                id: 0x040C,
                name: Vec::new(),
                data: vec![0x22; 40],
            },
        );
        // Split mid-way through the *first* block, the case that a per-segment
        // parser silently drops.
        let split = 20usize;
        let mut first = PHOTOSHOP_APP13_PREFIX.to_vec();
        first.extend_from_slice(&stream[..split]);
        let mut second = PHOTOSHOP_APP13_PREFIX.to_vec();
        second.extend_from_slice(&stream[split..]);

        let jpeg = minimal_jpeg();
        let with_split = splice_segments(
            &jpeg,
            &|_, _| false,
            &[(JPEG_APP13, first), (JPEG_APP13, second)],
            &[JPEG_APP0],
        )
        .expect("insert split APP13 pair");

        let updated = embed(&with_split, &iptc_metadata()).expect("embed into split APP13");
        let (segments, _) = parse_segments(&updated).expect("parse");
        let blocks = parse_irb_stream_strict(&collect_irb_stream(&updated, &segments))
            .expect("stream parses");
        let ids: Vec<u16> = blocks.iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            vec![IRB_ID_IPTC_IIM, 0x040B, 0x040C],
            "both pre-existing resources must survive a split-segment source"
        );
        assert_eq!(
            blocks.iter().find(|b| b.id == 0x040B).map(|b| b.data.len()),
            Some(40)
        );
    }

    #[test]
    fn test_malformed_resource_tail_is_an_honest_error() {
        let mut payload = PHOTOSHOP_APP13_PREFIX.to_vec();
        encode_resource_block(
            &mut payload,
            &ResourceBlock {
                id: 0x040A,
                name: Vec::new(),
                data: vec![0x33; 8],
            },
        );
        // Trailing bytes that are not a valid 8BIM block.
        payload.extend_from_slice(b"8BIMtruncated");

        let jpeg = minimal_jpeg();
        let with_app13 =
            splice_segments(&jpeg, &|_, _| false, &[(JPEG_APP13, payload)], &[JPEG_APP0])
                .expect("insert APP13");

        match embed(&with_app13, &iptc_metadata()) {
            Err(Error::ParseError(message)) => {
                assert!(message.contains("discard"), "message was: {message}");
            }
            other => panic!("expected an honest ParseError, got {other:?}"),
        }
        // And the same for the reader.
        assert!(matches!(
            extract_iim(&with_app13),
            Err(Error::ParseError(_))
        ));
    }

    #[test]
    fn test_iptc_rejects_non_jpeg() {
        let result = embed(b"definitely not a jpeg", &iptc_metadata());
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn test_splice_preserves_scan_bytes_exactly() {
        let jpeg = minimal_jpeg();
        let embedded = embed(&jpeg, &iptc_metadata()).expect("embed");
        let original_sos = jpeg
            .windows(2)
            .position(|w| w == [0xFF, 0xDA])
            .expect("original SOS");
        let new_sos = embedded
            .windows(2)
            .position(|w| w == [0xFF, 0xDA])
            .expect("embedded SOS");
        assert_eq!(&embedded[new_sos..], &jpeg[original_sos..]);
    }
}

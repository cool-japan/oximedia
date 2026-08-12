//! Oversized-XMP handling for JPEG: `ExtendedXMP` multi-segment splitting.
//!
//! A JPEG marker segment can carry at most 65533 payload bytes, so an XMP packet
//! larger than that cannot be written as a single `APP1`. The XMP specification
//! (Part 3, *Storage in Files* — JPEG) defines `ExtendedXMP` for exactly this:
//!
//! * The **standard** `APP1` keeps a normal XMP packet behind the
//!   `http://ns.adobe.com/xap/1.0/\0` identifier, carrying the properties that
//!   fit plus an `xmpNote:HasExtendedXMP` property whose value is the GUID of
//!   the extended part.
//! * The **extension** `APP1` segments carry the identifier
//!   `http://ns.adobe.com/xmp/extension/\0`, followed by that same 32-character
//!   uppercase-hex GUID, the total length of the extended serialization
//!   (`u32` big-endian), this chunk's offset into it (`u32` big-endian), and the
//!   chunk itself.
//!
//! The GUID is the MD5 digest of the **complete** extended serialization, which
//! is what lets a reader verify it reassembled the right bytes.
//!
//! Exif has no equivalent mechanism: an Exif `APP1` is a single TIFF structure
//! with no defined continuation, so an oversized Exif payload is genuinely
//! unrepresentable and [`super::embed`] reports that as an honest error rather
//! than writing something a decoder cannot read.

use md5::{Digest, Md5};

use super::jpeg::{splice_segments, JPEG_APP0, JPEG_APP1, MAX_SEGMENT_PAYLOAD};
use super::XMP_APP1_PREFIX;
use crate::{Error, Metadata, MetadataFormat};

/// Identifier prefix of an `ExtendedXMP` `APP1` segment.
pub(super) const EXTENDED_XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// Length of the ASCII-hex GUID that ties the extension segments to the main packet.
const GUID_LEN: usize = 32;

/// Per-segment `ExtendedXMP` header: identifier + GUID + total length + offset.
const EXT_HEADER_LEN: usize = EXTENDED_XMP_PREFIX.len() + GUID_LEN + 4 + 4;

/// Largest slice of the extended serialization that fits in one `APP1` segment.
const MAX_EXT_CHUNK: usize = MAX_SEGMENT_PAYLOAD - EXT_HEADER_LEN;

/// Largest XMP packet that fits in the standard `APP1` segment.
const MAX_STANDARD_PACKET: usize = MAX_SEGMENT_PAYLOAD - XMP_APP1_PREFIX.len();

/// XMP namespace URI for the `xmpNote` prefix.
const XMP_NOTE_NS: &str = "http://ns.adobe.com/xmp/note/";

/// A GUID-shaped placeholder used while measuring the main packet. Every real
/// GUID is exactly [`GUID_LEN`] characters, so the measurement stays exact.
const PLACEHOLDER_GUID: &str = "00000000000000000000000000000000";

/// `true` when an `APP1` payload belongs to XMP (standard packet or extension).
pub(super) fn is_xmp_app1(marker: u8, payload: &[u8]) -> bool {
    marker == JPEG_APP1
        && (payload.starts_with(XMP_APP1_PREFIX) || payload.starts_with(EXTENDED_XMP_PREFIX))
}

/// Embeds `metadata` as XMP into a JPEG, splitting across `ExtendedXMP`
/// segments when the serialized packet does not fit in one `APP1`.
///
/// Every pre-existing XMP segment — standard packet *and* stale extension
/// segments — is removed first, so the result never mixes two generations of
/// XMP.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if `file_data` is not a well-formed JPEG, and
/// propagates serialization errors from [`crate::xmp::write`].
pub(super) fn embed_into_jpeg(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    let packet = crate::xmp::write(metadata)?;

    let segments = if packet.len() <= MAX_STANDARD_PACKET {
        let mut payload = Vec::with_capacity(XMP_APP1_PREFIX.len() + packet.len());
        payload.extend_from_slice(XMP_APP1_PREFIX);
        payload.extend_from_slice(&packet);
        vec![(JPEG_APP1, payload)]
    } else {
        build_split_segments(metadata)?
    };

    splice_segments(
        file_data,
        &|marker, payload| is_xmp_app1(marker, payload),
        &segments,
        &[JPEG_APP0],
    )
}

/// Builds the full `APP1` run for an oversized packet: one standard segment
/// followed by as many `ExtendedXMP` segments as the remainder needs.
fn build_split_segments(metadata: &Metadata) -> Result<Vec<(u8, Vec<u8>)>, Error> {
    let (main_fields, extended_fields) = partition_fields(metadata)?;

    let extended_packet = crate::xmp::write(&extended_fields)?;
    let guid = md5_guid(&extended_packet);
    let main_packet = inject_extended_note(&crate::xmp::write(&main_fields)?, &guid)?;

    if main_packet.len() > MAX_STANDARD_PACKET {
        return Err(Error::WriteError(format!(
            "the standard XMP packet is {} bytes after the ExtendedXMP split, which still exceeds \
             the {MAX_STANDARD_PACKET}-byte JPEG APP1 limit",
            main_packet.len()
        )));
    }

    let mut segments = Vec::with_capacity(1 + extended_packet.len() / MAX_EXT_CHUNK + 1);

    let mut standard = Vec::with_capacity(XMP_APP1_PREFIX.len() + main_packet.len());
    standard.extend_from_slice(XMP_APP1_PREFIX);
    standard.extend_from_slice(&main_packet);
    segments.push((JPEG_APP1, standard));

    let total_len = u32::try_from(extended_packet.len()).map_err(|_| {
        Error::Unsupported(
            "the ExtendedXMP serialization exceeds the 4 GiB the per-segment length field can \
             describe"
                .to_string(),
        )
    })?;
    for (index, chunk) in extended_packet.chunks(MAX_EXT_CHUNK).enumerate() {
        let offset = (index * MAX_EXT_CHUNK) as u32;
        let mut payload = Vec::with_capacity(EXT_HEADER_LEN + chunk.len());
        payload.extend_from_slice(EXTENDED_XMP_PREFIX);
        payload.extend_from_slice(guid.as_bytes());
        payload.extend_from_slice(&total_len.to_be_bytes());
        payload.extend_from_slice(&offset.to_be_bytes());
        payload.extend_from_slice(chunk);
        segments.push((JPEG_APP1, payload));
    }

    Ok(segments)
}

/// Splits `metadata`'s fields into the largest prefix (by sorted key) whose
/// standard packet still fits in one `APP1`, and the remainder.
///
/// The packet length grows monotonically with the number of fields, so the
/// boundary is found by binary search — a handful of serializations rather than
/// one per field.
fn partition_fields(metadata: &Metadata) -> Result<(Metadata, Metadata), Error> {
    let mut keys: Vec<&String> = metadata.fields().keys().collect();
    keys.sort_unstable();

    let subset = |count: usize| -> Metadata {
        let mut subset = Metadata::new(MetadataFormat::Xmp);
        for key in keys.iter().take(count) {
            if let Some(value) = metadata.get(key) {
                subset.insert((*key).clone(), value.clone());
            }
        }
        subset
    };
    let measured = |count: usize| -> Result<usize, Error> {
        let packet = crate::xmp::write(&subset(count))?;
        Ok(inject_extended_note(&packet, PLACEHOLDER_GUID)?.len())
    };

    // Binary search the largest `count` whose main packet still fits. At least
    // one field must stay in the extended part, hence the `len() - 1` ceiling.
    let ceiling = keys.len().saturating_sub(1);
    let mut low = 0usize;
    let mut high = ceiling;
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if measured(mid)? <= MAX_STANDARD_PACKET {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    let main = subset(low);
    let mut extended = Metadata::new(MetadataFormat::Xmp);
    for key in keys.iter().skip(low) {
        if let Some(value) = metadata.get(key) {
            extended.insert((*key).clone(), value.clone());
        }
    }
    Ok((main, extended))
}

/// Returns the uppercase-hex MD5 digest of `data` — the `ExtendedXMP` GUID.
fn md5_guid(data: &[u8]) -> String {
    let digest = Md5::digest(data);
    let mut out = String::with_capacity(GUID_LEN);
    for byte in digest {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

/// Adds the `xmpNote:HasExtendedXMP` property (and its namespace declaration) to
/// a serialized XMP packet.
///
/// It is written as an RDF property attribute on the first `rdf:Description`,
/// which is the form Adobe's writer emits and the form readers scan for. When
/// the packet has no description element (all properties went to the extended
/// part), a dedicated `rdf:Description` is added so the note still has a home.
///
/// # Errors
///
/// Returns [`Error::WriteError`] if the packet has neither an `rdf:Description`
/// nor an `rdf:RDF` element to attach the note to.
fn inject_extended_note(packet: &[u8], guid: &str) -> Result<Vec<u8>, Error> {
    let attribute = format!(" xmlns:xmpNote=\"{XMP_NOTE_NS}\" xmpNote:HasExtendedXMP=\"{guid}\"");

    if let Some(pos) = find_subslice(packet, b"<rdf:Description") {
        let insert_at = pos + b"<rdf:Description".len();
        let mut out = Vec::with_capacity(packet.len() + attribute.len());
        out.extend_from_slice(&packet[..insert_at]);
        out.extend_from_slice(attribute.as_bytes());
        out.extend_from_slice(&packet[insert_at..]);
        return Ok(out);
    }

    if let Some(pos) = find_subslice(packet, b"</rdf:RDF>") {
        let element = format!("<rdf:Description rdf:about=\"\"{attribute}/>");
        let mut out = Vec::with_capacity(packet.len() + element.len());
        out.extend_from_slice(&packet[..pos]);
        out.extend_from_slice(element.as_bytes());
        out.extend_from_slice(&packet[pos..]);
        return Ok(out);
    }

    Err(Error::WriteError(
        "cannot attach xmpNote:HasExtendedXMP: the serialized XMP packet has no rdf:Description \
         and no rdf:RDF element"
            .to_string(),
    ))
}

/// Returns the offset of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reassembles the `ExtendedXMP` serialization from a JPEG's extension segments.
///
/// Returns `None` when the file has no extension segments. Verifying that the
/// reassembled bytes hash to the GUID advertised in the segments is what makes
/// this a real inverse of `embed_into_jpeg`, so a mismatch is an error rather
/// than a silently truncated result.
///
/// # Errors
///
/// Returns [`Error::ParseError`] for a malformed JPEG, for chunks that do not
/// tile the declared length, or for a GUID/MD5 mismatch.
pub fn extract_extended_xmp(file_data: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let (segments, _) = super::jpeg::parse_segments(file_data)?;

    let mut guid: Option<String> = None;
    let mut total: Option<usize> = None;
    let mut buffer: Vec<u8> = Vec::new();

    for segment in &segments {
        let payload = segment.payload(file_data);
        if segment.marker != JPEG_APP1 || !payload.starts_with(EXTENDED_XMP_PREFIX) {
            continue;
        }
        if payload.len() < EXT_HEADER_LEN {
            return Err(Error::ParseError(
                "Truncated ExtendedXMP segment: header does not fit in the segment".to_string(),
            ));
        }
        let guid_start = EXTENDED_XMP_PREFIX.len();
        let this_guid = String::from_utf8(payload[guid_start..guid_start + GUID_LEN].to_vec())
            .map_err(|e| Error::ParseError(format!("ExtendedXMP GUID is not ASCII: {e}")))?;
        let len_start = guid_start + GUID_LEN;
        let this_total = u32::from_be_bytes([
            payload[len_start],
            payload[len_start + 1],
            payload[len_start + 2],
            payload[len_start + 3],
        ]) as usize;
        let offset = u32::from_be_bytes([
            payload[len_start + 4],
            payload[len_start + 5],
            payload[len_start + 6],
            payload[len_start + 7],
        ]) as usize;
        let chunk = &payload[EXT_HEADER_LEN..];

        match (&guid, total) {
            (Some(existing), Some(existing_total))
                if existing != &this_guid || existing_total != this_total =>
            {
                return Err(Error::ParseError(
                    "ExtendedXMP segments disagree about GUID or total length".to_string(),
                ));
            }
            _ => {
                guid = Some(this_guid);
                total = Some(this_total);
            }
        }

        if buffer.len() < this_total {
            buffer.resize(this_total, 0);
        }
        let end = offset
            .checked_add(chunk.len())
            .ok_or_else(|| Error::ParseError("ExtendedXMP chunk offset overflows".to_string()))?;
        if end > this_total {
            return Err(Error::ParseError(format!(
                "ExtendedXMP chunk at offset {offset} runs past the declared total of {this_total} \
                 bytes"
            )));
        }
        buffer[offset..end].copy_from_slice(chunk);
    }

    let (Some(guid), Some(total)) = (guid, total) else {
        return Ok(None);
    };
    buffer.truncate(total);
    let actual = md5_guid(&buffer);
    if actual != guid {
        return Err(Error::ParseError(format!(
            "ExtendedXMP reassembly failed: MD5 of the reassembled data is {actual}, but the \
             segments advertise {guid}"
        )));
    }
    Ok(Some(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::jpeg::parse_segments;
    use crate::embed::tests::minimal_jpeg;
    use crate::MetadataValue;

    /// Builds XMP metadata whose serialization is comfortably over 64 KiB.
    fn oversized_metadata() -> Metadata {
        let mut metadata = Metadata::new(MetadataFormat::Xmp);
        // 40 fields x ~4 KiB each ≈ 160 KiB — three extension segments' worth.
        for i in 0..40 {
            metadata.insert(
                format!("dc:description{i:02}"),
                MetadataValue::Text("x".repeat(4096)),
            );
        }
        metadata
    }

    #[test]
    fn test_small_packet_uses_a_single_standard_segment() {
        let mut metadata = Metadata::new(MetadataFormat::Xmp);
        metadata.insert(
            "dc:title".to_string(),
            MetadataValue::Text("Small".to_string()),
        );
        let embedded = embed_into_jpeg(&minimal_jpeg(), &metadata).expect("embed");

        let (segments, _) = parse_segments(&embedded).expect("parse");
        let xmp: Vec<_> = segments
            .iter()
            .filter(|s| is_xmp_app1(s.marker, s.payload(&embedded)))
            .collect();
        assert_eq!(xmp.len(), 1);
        assert!(xmp[0].payload(&embedded).starts_with(XMP_APP1_PREFIX));
        assert_eq!(
            extract_extended_xmp(&embedded).expect("extract"),
            None,
            "a small packet must not emit ExtendedXMP segments"
        );
    }

    #[test]
    fn test_oversized_packet_splits_into_extended_segments() {
        let metadata = oversized_metadata();
        let embedded = embed_into_jpeg(&minimal_jpeg(), &metadata).expect("embed oversized xmp");

        let (segments, _) = parse_segments(&embedded).expect("parse");
        let xmp: Vec<_> = segments
            .iter()
            .filter(|s| is_xmp_app1(s.marker, s.payload(&embedded)))
            .collect();
        assert!(
            xmp.len() >= 3,
            "expected a standard segment plus several extension segments, got {}",
            xmp.len()
        );

        // Every emitted segment must fit the JPEG limit.
        for segment in &xmp {
            assert!(segment.payload(&embedded).len() <= MAX_SEGMENT_PAYLOAD);
        }

        // First is the standard packet; the rest are extensions.
        assert!(xmp[0].payload(&embedded).starts_with(XMP_APP1_PREFIX));
        for segment in &xmp[1..] {
            assert!(segment.payload(&embedded).starts_with(EXTENDED_XMP_PREFIX));
        }
    }

    #[test]
    fn test_extended_segments_reassemble_and_match_their_guid() {
        let metadata = oversized_metadata();
        let embedded = embed_into_jpeg(&minimal_jpeg(), &metadata).expect("embed");

        // `extract_extended_xmp` verifies the MD5 against the advertised GUID.
        let extended = extract_extended_xmp(&embedded)
            .expect("reassemble")
            .expect("extension segments present");
        let parsed = crate::xmp::parse(&extended).expect("parse reassembled xmp");

        // Every field that did not stay in the main packet must be readable here.
        let main_packet = {
            let (segments, _) = parse_segments(&embedded).expect("parse");
            let standard = segments
                .iter()
                .find(|s| {
                    s.marker == JPEG_APP1 && s.payload(&embedded).starts_with(XMP_APP1_PREFIX)
                })
                .expect("standard segment");
            standard.payload(&embedded)[XMP_APP1_PREFIX.len()..].to_vec()
        };
        let main = crate::xmp::parse(&main_packet).expect("parse main packet");

        for key in metadata.fields().keys() {
            assert!(
                main.get(key).is_some() || parsed.get(key).is_some(),
                "field {key} must survive the split"
            );
        }
    }

    #[test]
    fn test_main_packet_carries_the_has_extended_xmp_note() {
        let embedded =
            embed_into_jpeg(&minimal_jpeg(), &oversized_metadata()).expect("embed oversized");
        let (segments, _) = parse_segments(&embedded).expect("parse");
        let standard = segments
            .iter()
            .find(|s| s.marker == JPEG_APP1 && s.payload(&embedded).starts_with(XMP_APP1_PREFIX))
            .expect("standard segment");
        let packet = &standard.payload(&embedded)[XMP_APP1_PREFIX.len()..];

        let guid = md5_guid(
            &extract_extended_xmp(&embedded)
                .expect("reassemble")
                .expect("extended present"),
        );
        let expected = format!("xmpNote:HasExtendedXMP=\"{guid}\"");
        assert!(
            find_subslice(packet, expected.as_bytes()).is_some(),
            "the standard packet must advertise the extended GUID"
        );
        assert!(
            find_subslice(packet, XMP_NOTE_NS.as_bytes()).is_some(),
            "the xmpNote namespace must be declared"
        );
    }

    #[test]
    fn test_reembed_replaces_stale_extension_segments() {
        let first = embed_into_jpeg(&minimal_jpeg(), &oversized_metadata()).expect("first embed");

        let mut small = Metadata::new(MetadataFormat::Xmp);
        small.insert(
            "dc:title".to_string(),
            MetadataValue::Text("Now small".to_string()),
        );
        let second = embed_into_jpeg(&first, &small).expect("second embed");

        let (segments, _) = parse_segments(&second).expect("parse");
        let xmp: Vec<_> = segments
            .iter()
            .filter(|s| is_xmp_app1(s.marker, s.payload(&second)))
            .collect();
        assert_eq!(
            xmp.len(),
            1,
            "stale ExtendedXMP segments must be removed on re-embed"
        );
        assert_eq!(extract_extended_xmp(&second).expect("extract"), None);
    }

    #[test]
    fn test_corrupted_extension_chunk_is_detected() {
        let embedded = embed_into_jpeg(&minimal_jpeg(), &oversized_metadata()).expect("embed");
        let mut corrupted = embedded.clone();
        // Flip a byte inside the last extension segment's chunk data.
        let (segments, _) = parse_segments(&embedded).expect("parse");
        let last_ext = segments
            .iter()
            .rfind(|s| {
                s.marker == JPEG_APP1 && s.payload(&embedded).starts_with(EXTENDED_XMP_PREFIX)
            })
            .expect("an extension segment exists");
        let target = last_ext.payload_start + EXT_HEADER_LEN;
        corrupted[target] ^= 0xFF;

        assert!(
            matches!(extract_extended_xmp(&corrupted), Err(Error::ParseError(_))),
            "a corrupted chunk must fail the GUID check instead of returning garbage"
        );
    }

    #[test]
    fn test_md5_guid_is_uppercase_hex_of_the_right_length() {
        let guid = md5_guid(b"abc");
        assert_eq!(guid.len(), GUID_LEN);
        assert!(guid.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(guid, guid.to_uppercase());
        // RFC 1321 test vector for "abc".
        assert_eq!(guid, "900150983CD24FB0D6963F7D28E17F72");
    }
}

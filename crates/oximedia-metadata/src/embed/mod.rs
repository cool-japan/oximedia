//! Metadata embedding and extraction utilities.
//!
//! This module provides utilities for embedding metadata into media files
//! and extracting metadata from them.
//!
//! # Container-aware embedding
//!
//! [`embed`] never falls back to naive byte concatenation. Concatenating a raw
//! metadata payload onto an arbitrary file produces a *corrupt* file for every
//! format except ID3v2 (tag prepended to the audio stream) and APEv2 (tag
//! appended as a footer) — both of which are simple by design and do not require
//! understanding the rest of the file. Every other format is spliced into its
//! container for real:
//!
//! | Metadata format | Container | Strategy |
//! |---|---|---|
//! | [`Exif`](MetadataFormat::Exif) | JPEG / bare TIFF | `APP1` segment upsert, or field merge into the blob |
//! | [`Xmp`](MetadataFormat::Xmp) | JPEG / bare packet | `APP1` upsert, splitting oversized packets across `ExtendedXMP` segments ([`xmp_ext`]) |
//! | [`Iptc`](MetadataFormat::Iptc) | JPEG | `APP13` "Photoshop 3.0" segment with an `8BIM` `0x0404` resource ([`jpeg`]) |
//! | [`Matroska`](MetadataFormat::Matroska) | MKV / WebM | EBML `Tags` element upsert with `Segment` size and `SeekHead` fix-up ([`ebml`]) |
//! | [`VorbisComments`](MetadataFormat::VorbisComments) | Ogg | comment-packet rewrite with re-lacing and CRC recomputation ([`ogg`]) |
//! | [`VorbisComments`](MetadataFormat::VorbisComments) | FLAC | `VORBIS_COMMENT` `METADATA_BLOCK` upsert ([`flac`]) |
//! | [`iTunes`](MetadataFormat::iTunes) / [`QuickTime`](MetadataFormat::QuickTime) | MP4 / MOV | `moov/udta` atom-tree splice with chunk-offset fix-up ([`mp4`]) |
//!
//! When the target bytes are not a container this module understands, it returns
//! [`Error::Unsupported`] with a precise explanation. It never silently returns a
//! byte stream that looks successful but does not actually parse as the target
//! format.
//!
//! # Failure atomicity
//!
//! Every function here is pure: it takes `&[u8]` and returns a fresh `Vec<u8>`,
//! so a parse error cannot corrupt anything — on `Err` the caller still holds its
//! original bytes, untouched. Callers that persist the result (for example
//! [`crate::bulk_update`]) write to a sibling temp file and rename, so a failure
//! part-way through the *write* is atomic too.

use crate::{Error, Metadata, MetadataFormat};

pub mod ebml;
pub mod flac;
pub mod jpeg;
pub mod mp4;
pub mod ogg;
pub mod xmp_ext;

use jpeg::{is_jpeg, splice_segments, JPEG_APP0, JPEG_APP1, MAX_SEGMENT_PAYLOAD};

/// Identifier prefix for an Exif `APP1` segment payload (immediately followed by
/// the raw TIFF/Exif blob, i.e. exactly [`exif::write`](crate::exif::write)'s output).
pub(crate) const EXIF_APP1_PREFIX: &[u8] = b"Exif\0\0";
/// Identifier prefix for an XMP `APP1` segment payload (Adobe's registered GUID,
/// immediately followed by the XMP packet, i.e. exactly
/// [`xmp::write`](crate::xmp::write)'s output).
pub(crate) const XMP_APP1_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Metadata embedding and extraction utilities.
pub struct MetadataEmbed;

impl MetadataEmbed {
    /// Detect metadata format from file data.
    ///
    /// # Errors
    ///
    /// Returns an error if the format cannot be detected.
    pub fn detect_format(data: &[u8]) -> Result<MetadataFormat, Error> {
        detect_format(data)
    }

    /// Extract metadata from file data.
    ///
    /// # Errors
    ///
    /// Returns an error if extraction fails.
    pub fn extract(data: &[u8], format: MetadataFormat) -> Result<Metadata, Error> {
        extract(data, format)
    }

    /// Embed metadata into file data.
    ///
    /// # Errors
    ///
    /// Returns an error if embedding fails.
    pub fn embed(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
        embed(file_data, metadata)
    }
}

/// Detect metadata format from file data.
///
/// # Errors
///
/// Returns an error if the format cannot be detected.
pub fn detect_format(data: &[u8]) -> Result<MetadataFormat, Error> {
    if data.len() < 16 {
        return Err(Error::ParseError(
            "Data too short to detect format".to_string(),
        ));
    }

    // Check for ID3v2 (MP3)
    if data.len() >= 3 && &data[0..3] == b"ID3" {
        return Ok(MetadataFormat::Id3v2);
    }

    // Check for APEv2
    if data.len() >= 8 && &data[0..8] == b"APETAGEX" {
        return Ok(MetadataFormat::Apev2);
    }

    // Check for EXIF/TIFF
    if data.len() >= 2 && (&data[0..2] == b"II" || &data[0..2] == b"MM") {
        return Ok(MetadataFormat::Exif);
    }

    // Check for XMP (XML-based)
    if data.len() >= 5 && &data[0..5] == b"<?xpa" {
        return Ok(MetadataFormat::Xmp);
    }

    // Check for Matroska tags (XML-based)
    if data.len() >= 5 && &data[0..5] == b"<Tags" {
        return Ok(MetadataFormat::Matroska);
    }

    // Check for IPTC
    if !data.is_empty() && data[0] == 0x1C {
        return Ok(MetadataFormat::Iptc);
    }

    // Check for Vorbis Comments (requires more context)
    // This is a simplified check
    if data.len() >= 4 {
        // Check for common Vorbis field names
        let text = String::from_utf8_lossy(&data[..data.len().min(100)]);
        if text.contains("TITLE=") || text.contains("ARTIST=") || text.contains("ALBUM=") {
            return Ok(MetadataFormat::VorbisComments);
        }
    }

    Err(Error::Unsupported("Unknown metadata format".to_string()))
}

/// Extract metadata from file data.
///
/// # Errors
///
/// Returns an error if extraction fails.
pub fn extract(data: &[u8], format: MetadataFormat) -> Result<Metadata, Error> {
    Metadata::parse(data, format)
}

/// Extract metadata from file data with automatic format detection.
///
/// # Errors
///
/// Returns an error if extraction fails.
pub fn extract_auto(data: &[u8]) -> Result<Metadata, Error> {
    let format = detect_format(data)?;
    extract(data, format)
}

/// Embed metadata into file data, container-aware.
///
/// * [`MetadataFormat::Id3v2`] tags are prepended (ID3v2 is defined to sit at the
///   start of the audio stream).
/// * [`MetadataFormat::Apev2`] tags are appended as a footer (APEv2 is defined to
///   sit at the end of the file).
/// * Every other format is spliced into its container — see the table in the
///   [module documentation](self) for the strategy used per format.
///
/// The returned `Vec` is a complete new file; `file_data` is never modified, so
/// an error leaves the caller's bytes untouched.
///
/// # Errors
///
/// Returns an error if embedding fails, or [`Error::Unsupported`] if `file_data`
/// is not a container this crate can splice the given metadata format into.
pub fn embed(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    match metadata.format() {
        MetadataFormat::Id3v2 => {
            let metadata_bytes = metadata.write()?;
            // ID3v2 tags go at the beginning of the file.
            let mut result = Vec::with_capacity(metadata_bytes.len() + file_data.len());
            result.extend_from_slice(&metadata_bytes);
            result.extend_from_slice(file_data);
            Ok(result)
        }
        MetadataFormat::Apev2 => {
            let metadata_bytes = metadata.write()?;
            // APEv2 tags go at the end of the file.
            let mut result = Vec::with_capacity(file_data.len() + metadata_bytes.len());
            result.extend_from_slice(file_data);
            result.extend_from_slice(&metadata_bytes);
            Ok(result)
        }
        MetadataFormat::Exif => embed_exif(file_data, metadata),
        MetadataFormat::Xmp => embed_xmp(file_data, metadata),
        MetadataFormat::Matroska => ebml::embed(file_data, metadata),
        MetadataFormat::Iptc => jpeg::embed(file_data, metadata),
        MetadataFormat::VorbisComments => embed_vorbis_comments(file_data, metadata),
        format @ (MetadataFormat::iTunes | MetadataFormat::QuickTime) => {
            mp4::embed(file_data, metadata, format)
        }
    }
}

/// Routes a Vorbis-comment embed to the FLAC or Ogg implementation by container
/// magic.
fn embed_vorbis_comments(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    if file_data.starts_with(flac::FLAC_MAGIC) {
        return flac::embed(file_data, metadata);
    }
    if file_data.starts_with(ogg::OGG_MAGIC) {
        return ogg::embed(file_data, metadata);
    }
    Err(Error::Unsupported(
        "embed(VorbisComments) targets a native FLAC stream (\"fLaC\") or an Ogg bitstream \
         (\"OggS\"); the given file_data is neither, and a Vorbis comment block has no meaning \
         outside one of those containers"
            .to_string(),
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// JPEG APP1 (Exif / XMP) container-aware embedding
// ─────────────────────────────────────────────────────────────────────────────

/// Embed Exif metadata into `file_data`.
///
/// Two container shapes are recognised:
/// - **JPEG**: the Exif payload is written into an `APP1` segment, replacing an
///   existing Exif `APP1` segment in place if one is found, or inserting a new one
///   immediately after the leading `SOI`/`APP0` run (the position mandated by the
///   Exif specification).
/// - **Bare Exif/TIFF blob** (the shape [`exif::write`](crate::exif::write)
///   produces and [`exif::parse`](crate::exif::parse) accepts): the new fields are
///   merged into the existing blob's fields (new values win on conflict) and the
///   merged result is re-serialized as a standalone blob.
///
/// Any other container shape is rejected with [`Error::Unsupported`] rather than
/// risking a naive concatenation that would corrupt the file.
fn embed_exif(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    if is_jpeg(file_data) {
        let blob = crate::exif::write(metadata)?;
        let mut payload = Vec::with_capacity(EXIF_APP1_PREFIX.len() + blob.len());
        payload.extend_from_slice(EXIF_APP1_PREFIX);
        payload.extend_from_slice(&blob);
        if payload.len() > MAX_SEGMENT_PAYLOAD {
            // Unlike XMP, Exif has no continuation mechanism: an Exif APP1 is a
            // single self-contained TIFF structure and the specification defines
            // no way to span segments. Reporting that is the only honest option.
            return Err(Error::Unsupported(format!(
                "the Exif payload is {} bytes, which exceeds the {MAX_SEGMENT_PAYLOAD}-byte JPEG \
                 APP1 limit. Exif defines no multi-segment continuation (unlike XMP's \
                 ExtendedXMP), so an Exif blob this large is not representable in JPEG; store it \
                 as XMP or reduce the payload (thumbnails and MakerNotes dominate)",
                payload.len()
            )));
        }
        return splice_segments(
            file_data,
            &|marker, payload| marker == JPEG_APP1 && payload.starts_with(EXIF_APP1_PREFIX),
            &[(JPEG_APP1, payload)],
            &[JPEG_APP0],
        );
    }
    if file_data.len() >= 4 && (&file_data[0..2] == b"II" || &file_data[0..2] == b"MM") {
        let mut existing = crate::exif::parse(file_data)?;
        for (key, value) in metadata.fields() {
            existing.insert(key.clone(), value.clone());
        }
        return crate::exif::write(&existing);
    }
    Err(Error::Unsupported(
        "embed(Exif) only supports JPEG (APP1 segment) or a bare Exif/TIFF blob as the \
         target container; the given file_data matches neither shape, and naive \
         concatenation would corrupt it"
            .to_string(),
    ))
}

/// Embed XMP metadata into `file_data`.
///
/// Two container shapes are recognised:
/// - **JPEG**: the XMP packet is written into an `APP1` segment (Adobe's
///   registered `http://ns.adobe.com/xap/1.0/` GUID), replacing existing XMP
///   segments in place if any are found, or inserting a new run immediately
///   after the leading `SOI`/`APP0` run. Packets too large for one segment are
///   split across `ExtendedXMP` segments — see [`xmp_ext`].
/// - **Bare XMP packet** (the shape [`xmp::write`](crate::xmp::write) produces and
///   [`xmp::parse`](crate::xmp::parse) accepts, optionally without the `<?xpacket`
///   wrapper): the new fields are merged into the existing packet's fields (new
///   values win on conflict) and the merged result is re-serialized.
///
/// Any other container shape is rejected with [`Error::Unsupported`] rather than
/// risking a naive concatenation that would corrupt the file.
fn embed_xmp(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    if is_jpeg(file_data) {
        return xmp_ext::embed_into_jpeg(file_data, metadata);
    }
    if file_data.starts_with(b"<?xpacket") || file_data.starts_with(b"<x:xmpmeta") {
        let mut existing = crate::xmp::parse(file_data)?;
        for (key, value) in metadata.fields() {
            existing.insert(key.clone(), value.clone());
        }
        return crate::xmp::write(&existing);
    }
    Err(Error::Unsupported(
        "embed(Xmp) only supports JPEG (APP1 segment) or a bare XMP packet as the target \
         container; the given file_data matches neither shape, and naive concatenation \
         would corrupt it"
            .to_string(),
    ))
}

/// Remove metadata from file data.
///
/// # Errors
///
/// Returns an error if removal fails.
pub fn remove_metadata(file_data: &[u8], format: MetadataFormat) -> Result<Vec<u8>, Error> {
    match format {
        MetadataFormat::Id3v2 => {
            // Remove ID3v2 tags from beginning
            if file_data.len() >= 10 && &file_data[0..3] == b"ID3" {
                // Parse header to get tag size
                let tag_size = (u32::from(file_data[6] & 0x7F) << 21)
                    | (u32::from(file_data[7] & 0x7F) << 14)
                    | (u32::from(file_data[8] & 0x7F) << 7)
                    | u32::from(file_data[9] & 0x7F);

                let total_size = 10 + tag_size as usize;
                if total_size <= file_data.len() {
                    return Ok(file_data[total_size..].to_vec());
                }
            }
            Ok(file_data.to_vec())
        }
        MetadataFormat::Apev2 => {
            // Remove APEv2 tags from end
            if file_data.len() >= 32 {
                let footer_pos = file_data.len() - 32;
                if &file_data[footer_pos..footer_pos + 8] == b"APETAGEX" {
                    // Parse footer to get tag size
                    let tag_size = u32::from_le_bytes([
                        file_data[footer_pos + 12],
                        file_data[footer_pos + 13],
                        file_data[footer_pos + 14],
                        file_data[footer_pos + 15],
                    ]);

                    let total_size = tag_size as usize + 32;
                    if total_size <= file_data.len() {
                        return Ok(file_data[..file_data.len() - total_size].to_vec());
                    }
                }
            }
            Ok(file_data.to_vec())
        }
        _ => {
            // For other formats, return data as-is
            Ok(file_data.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetadataValue;
    use jpeg::{JPEG_EOI, JPEG_SOS};

    #[test]
    fn test_detect_format_id3v2() {
        let data = b"ID3\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(
            detect_format(data).expect("should succeed in test"),
            MetadataFormat::Id3v2
        );
    }

    #[test]
    fn test_detect_format_apev2() {
        let data = b"APETAGEX\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(
            detect_format(data).expect("should succeed in test"),
            MetadataFormat::Apev2
        );
    }

    #[test]
    fn test_detect_format_exif() {
        let data = b"II*\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(
            detect_format(data).expect("should succeed in test"),
            MetadataFormat::Exif
        );

        let data = b"MM\x00*\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(
            detect_format(data).expect("should succeed in test"),
            MetadataFormat::Exif
        );
    }

    #[test]
    fn test_embed_id3v2() {
        let mut metadata = Metadata::new(MetadataFormat::Id3v2);
        metadata.insert("TIT2".to_string(), MetadataValue::Text("Test".to_string()));

        let file_data = b"audio data here";
        let result = embed(file_data, &metadata).expect("Embed failed");

        // Result should start with ID3v2 tag
        assert!(result.len() > file_data.len());
        assert_eq!(&result[0..3], b"ID3");
    }

    #[test]
    fn test_remove_metadata_id3v2() {
        // Create a simple ID3v2 tag
        let mut tag_data = vec![b'I', b'D', b'3', 0x03, 0x00, 0x00];
        // Size: 20 bytes (synchsafe)
        tag_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x14]);
        // Tag data (20 bytes)
        tag_data.extend_from_slice(&[0u8; 20]);
        // File data
        tag_data.extend_from_slice(b"audio data");

        let result = remove_metadata(&tag_data, MetadataFormat::Id3v2).expect("Remove failed");
        assert_eq!(result, b"audio data");
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Build a minimal, structurally-valid JPEG: SOI, a JFIF APP0 segment, an SOS
    /// segment with a plausible (but not decoder-accurate — no real image data is
    /// needed for container-splicing tests) header, a few bytes of opaque "scan
    /// data", and EOI.
    pub(super) fn minimal_jpeg() -> Vec<u8> {
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
                                               // APP0 (JFIF): len=0x0010, "JFIF\0", version 1.1, units=0, density 1x1, no thumbnail.
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        jpeg.extend_from_slice(b"JFIF\0");
        jpeg.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        // SOS: len=0x0008, Ns=1, (component=1, tables=0), Ss=0, Se=0x3F, Ah/Al=0.
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08]);
        jpeg.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        // Opaque entropy-coded scan bytes (never interpreted by embed()).
        jpeg.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
        jpeg
    }

    /// Locate the `[start, end)` byte range of the first APP1 segment whose payload
    /// starts with `id_prefix`, or `None` if not present.
    fn find_app1_segment(jpeg: &[u8], id_prefix: &[u8]) -> Option<(usize, usize)> {
        let mut pos = 2usize;
        while pos + 4 <= jpeg.len() {
            assert_eq!(jpeg[pos], 0xFF, "expected marker at {pos}");
            let marker = jpeg[pos + 1];
            if marker == JPEG_SOS || marker == JPEG_EOI {
                break;
            }
            let seg_len = usize::from(u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]));
            let seg_end = pos + 2 + seg_len;
            let payload_start = pos + 4;
            if marker == JPEG_APP1 && jpeg[payload_start..seg_end].starts_with(id_prefix) {
                return Some((pos, seg_end));
            }
            pos = seg_end;
        }
        None
    }

    // ── Exif / JPEG ──────────────────────────────────────────────────────────

    #[test]
    fn test_embed_exif_into_jpeg_round_trips_via_temp_file() {
        let tmp = std::env::temp_dir().join("oximedia_metadata_embed_exif_test.jpg");
        std::fs::write(&tmp, minimal_jpeg()).expect("write temp jpeg");

        let mut metadata = Metadata::new(MetadataFormat::Exif);
        metadata.insert(
            "Artist".to_string(),
            MetadataValue::Text("Jane Doe".to_string()),
        );

        let file_data = std::fs::read(&tmp).expect("read temp jpeg");
        let embedded = embed(&file_data, &metadata).expect("embed exif into jpeg");
        std::fs::write(&tmp, &embedded).expect("write embedded jpeg");

        let round_tripped = std::fs::read(&tmp).expect("read embedded jpeg");
        let _ = std::fs::remove_file(&tmp);

        // The file must still be a well-formed JPEG (SOI at the start, EOI at the end).
        assert_eq!(&round_tripped[0..2], &[0xFF, 0xD8]);
        assert_eq!(&round_tripped[round_tripped.len() - 2..], &[0xFF, 0xD9]);

        // The opaque scan tail (SOS marker onward) must be byte-for-byte preserved.
        let original = minimal_jpeg();
        let original_sos = original
            .windows(2)
            .position(|w| w == [0xFF, 0xDA])
            .expect("original has SOS");
        let new_sos = round_tripped
            .windows(2)
            .position(|w| w == [0xFF, 0xDA])
            .expect("embedded has SOS");
        assert_eq!(&round_tripped[new_sos..], &original[original_sos..]);

        // The Exif APP1 segment must be present and parse back to the inserted field.
        let (start, end) =
            find_app1_segment(&round_tripped, EXIF_APP1_PREFIX).expect("exif app1 present");
        let tiff_blob = &round_tripped[start + 4 + EXIF_APP1_PREFIX.len()..end];
        let parsed = crate::exif::parse(tiff_blob).expect("parse embedded exif blob");
        assert_eq!(
            parsed.get("Artist").and_then(MetadataValue::as_text),
            Some("Jane Doe")
        );
    }

    #[test]
    fn test_embed_exif_replaces_existing_app1_in_place() {
        let jpeg = minimal_jpeg();

        let mut first = Metadata::new(MetadataFormat::Exif);
        first.insert(
            "Artist".to_string(),
            MetadataValue::Text("First Artist".to_string()),
        );
        let after_first = embed(&jpeg, &first).expect("first embed");

        let mut second = Metadata::new(MetadataFormat::Exif);
        second.insert(
            "Artist".to_string(),
            MetadataValue::Text("Second Artist".to_string()),
        );
        let after_second = embed(&after_first, &second).expect("second embed (replace)");

        // Only one Exif APP1 segment should exist, and it must carry the *second* value.
        let mut count = 0usize;
        let mut pos = 2usize;
        while pos + 4 <= after_second.len() {
            let marker = after_second[pos + 1];
            if marker == JPEG_SOS || marker == JPEG_EOI {
                break;
            }
            let seg_len = usize::from(u16::from_be_bytes([
                after_second[pos + 2],
                after_second[pos + 3],
            ]));
            let seg_end = pos + 2 + seg_len;
            if marker == JPEG_APP1 && after_second[pos + 4..seg_end].starts_with(EXIF_APP1_PREFIX) {
                count += 1;
            }
            pos = seg_end;
        }
        assert_eq!(
            count, 1,
            "expected exactly one Exif APP1 segment after replace"
        );

        let (start, end) =
            find_app1_segment(&after_second, EXIF_APP1_PREFIX).expect("exif app1 present");
        let tiff_blob = &after_second[start + 4 + EXIF_APP1_PREFIX.len()..end];
        let parsed = crate::exif::parse(tiff_blob).expect("parse replaced exif blob");
        assert_eq!(
            parsed.get("Artist").and_then(MetadataValue::as_text),
            Some("Second Artist")
        );
    }

    #[test]
    fn test_embed_exif_bare_tiff_blob_merges_fields() {
        let mut base = Metadata::new(MetadataFormat::Exif);
        base.insert("Make".to_string(), MetadataValue::Text("Acme".to_string()));
        let base_blob = crate::exif::write(&base).expect("write base exif blob");

        let mut addition = Metadata::new(MetadataFormat::Exif);
        addition.insert(
            "Artist".to_string(),
            MetadataValue::Text("Jane Doe".to_string()),
        );
        let merged_blob = embed(&base_blob, &addition).expect("merge into bare exif blob");

        let parsed = crate::exif::parse(&merged_blob).expect("parse merged exif blob");
        assert_eq!(
            parsed.get("Make").and_then(MetadataValue::as_text),
            Some("Acme"),
            "pre-existing field must survive the merge"
        );
        assert_eq!(
            parsed.get("Artist").and_then(MetadataValue::as_text),
            Some("Jane Doe"),
            "new field must be present after the merge"
        );
    }

    #[test]
    fn test_embed_exif_rejects_unrecognized_container() {
        let mut metadata = Metadata::new(MetadataFormat::Exif);
        metadata.insert("Artist".to_string(), MetadataValue::Text("X".to_string()));

        let random_bytes = b"this is not a jpeg or a tiff blob at all";
        let result = embed(random_bytes, &metadata);
        assert!(
            result.is_err(),
            "unrecognized container must be an honest error"
        );
        assert!(matches!(
            result.expect_err("checked above"),
            Error::Unsupported(_)
        ));
    }

    #[test]
    fn test_embed_exif_oversized_payload_is_an_honest_error() {
        let mut metadata = Metadata::new(MetadataFormat::Exif);
        // A single field far larger than one APP1 segment can hold.
        metadata.insert(
            "ImageDescription".to_string(),
            MetadataValue::Text("E".repeat(70_000)),
        );
        let result = embed(&minimal_jpeg(), &metadata);
        match result {
            Err(Error::Unsupported(message)) => assert!(
                message.contains("ExtendedXMP") && message.contains("APP1"),
                "the error must explain why Exif cannot be split: {message}"
            ),
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }

    // ── XMP / JPEG ───────────────────────────────────────────────────────────

    #[test]
    fn test_embed_xmp_into_jpeg_round_trips() {
        let jpeg = minimal_jpeg();

        let mut metadata = Metadata::new(MetadataFormat::Xmp);
        metadata.insert(
            "dc:creator".to_string(),
            MetadataValue::Text("Jane Doe".to_string()),
        );

        let embedded = embed(&jpeg, &metadata).expect("embed xmp into jpeg");

        assert_eq!(&embedded[0..2], &[0xFF, 0xD8]);
        assert_eq!(&embedded[embedded.len() - 2..], &[0xFF, 0xD9]);

        let (start, end) = find_app1_segment(&embedded, XMP_APP1_PREFIX).expect("xmp app1 present");
        let xmp_packet = &embedded[start + 4 + XMP_APP1_PREFIX.len()..end];
        let parsed = crate::xmp::parse(xmp_packet).expect("parse embedded xmp packet");
        assert_eq!(
            parsed.get("dc:creator").and_then(MetadataValue::as_text),
            Some("Jane Doe")
        );
    }

    #[test]
    fn test_embed_xmp_bare_packet_merges_fields() {
        let mut base = Metadata::new(MetadataFormat::Xmp);
        base.insert(
            "dc:title".to_string(),
            MetadataValue::Text("Original Title".to_string()),
        );
        let base_packet = crate::xmp::write(&base).expect("write base xmp packet");

        let mut addition = Metadata::new(MetadataFormat::Xmp);
        addition.insert(
            "dc:creator".to_string(),
            MetadataValue::Text("Jane Doe".to_string()),
        );
        let merged = embed(&base_packet, &addition).expect("merge into bare xmp packet");

        let parsed = crate::xmp::parse(&merged).expect("parse merged xmp packet");
        assert_eq!(
            parsed.get("dc:title").and_then(MetadataValue::as_text),
            Some("Original Title"),
            "pre-existing field must survive the merge"
        );
        assert_eq!(
            parsed.get("dc:creator").and_then(MetadataValue::as_text),
            Some("Jane Doe"),
            "new field must be present after the merge"
        );
    }

    // ── Honest errors when the target container is not the expected one ───────

    #[test]
    fn test_embed_matroska_into_non_mkv_is_an_honest_error() {
        let metadata = Metadata::new(MetadataFormat::Matroska);
        let result = embed(b"not a real matroska file", &metadata);
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn test_embed_iptc_into_non_jpeg_is_an_honest_error() {
        let metadata = Metadata::new(MetadataFormat::Iptc);
        let result = embed(b"not a real jpeg file", &metadata);
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn test_embed_vorbis_comments_into_unknown_container_is_an_honest_error() {
        let metadata = Metadata::new(MetadataFormat::VorbisComments);
        let result = embed(b"not a real ogg/flac file", &metadata);
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn test_embed_itunes_into_non_mp4_is_an_honest_error() {
        let metadata = Metadata::new(MetadataFormat::iTunes);
        let result = embed(b"not a real mp4 file", &metadata);
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }
}

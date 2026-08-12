//! Ogg-page-aware Vorbis-comment embedding.
//!
//! In an Ogg bitstream the comment header is a *packet*, not a region of the
//! file: it is split into 255-byte lacing segments that may span several pages,
//! each page carrying a CRC over its own bytes and a sequence number that must
//! stay contiguous. Rewriting it therefore means de-lacing the header pages back
//! into packets, substituting the comment packet, re-lacing, and recomputing
//! every page checksum.
//!
//! # What is preserved
//!
//! The header packets that are *not* the comment — the identification header and
//! (for Vorbis) the setup/codebook header — are de-laced and re-laced unchanged.
//! This matters: in the standard libvorbis layout the comment packet shares its
//! page run with the beginning of the setup header, so a writer that simply
//! replaced "the comment pages" would silently drop the codebooks and leave an
//! undecodable file.
//!
//! Audio pages are copied byte for byte; only their sequence numbers (and hence
//! their CRCs) are adjusted, and only when the rebuilt header occupies a
//! different number of pages. Pages belonging to other logical bitstreams are
//! untouched — sequence numbers are per-serial.
//!
//! Supported codecs are Vorbis (`\x01vorbis` identification, `\x03vorbis`
//! comment, framing bit) and Opus (`OpusHead`, `OpusTags`, no framing bit).
//! Anything else is an honest error rather than a guess.

use crate::{Error, Metadata};

/// Ogg page capture pattern.
pub(super) const OGG_MAGIC: &[u8; 4] = b"OggS";

/// Fixed part of an Ogg page header, before the segment table.
const PAGE_HEADER_LEN: usize = 27;

/// Header-type flag: this page opens with a continued packet.
const FLAG_CONTINUED: u8 = 0x01;
/// Header-type flag: beginning of a logical bitstream.
const FLAG_BOS: u8 = 0x02;

/// Vorbis identification packet prefix.
const VORBIS_IDENT: &[u8] = b"\x01vorbis";
/// Vorbis comment packet prefix.
const VORBIS_COMMENT: &[u8] = b"\x03vorbis";
/// Vorbis comment packets end with a framing bit.
const VORBIS_FRAMING_BIT: u8 = 0x01;
/// Opus identification packet prefix.
const OPUS_IDENT: &[u8] = b"OpusHead";
/// Opus comment packet prefix.
const OPUS_COMMENT: &[u8] = b"OpusTags";

/// Ogg's CRC-32 polynomial (normal, non-reflected form), init 0, no final xor.
const OGG_CRC32_POLY: u32 = 0x04C1_1DB7;

/// The codec carried by the logical bitstream being retagged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OggCodec {
    /// Vorbis: three header packets, comment packet ends with a framing bit.
    Vorbis,
    /// Opus: two header packets, no framing bit.
    Opus,
}

impl OggCodec {
    /// Number of header packets that precede the audio data.
    const fn header_packets(self) -> usize {
        match self {
            Self::Vorbis => 3,
            Self::Opus => 2,
        }
    }

    /// Prefix the comment packet must start with.
    const fn comment_prefix(self) -> &'static [u8] {
        match self {
            Self::Vorbis => VORBIS_COMMENT,
            Self::Opus => OPUS_COMMENT,
        }
    }
}

/// One parsed Ogg page.
struct Page {
    /// Header-type flags byte.
    header_type: u8,
    /// Granule position (codec-defined stream position).
    granule: i64,
    /// Logical bitstream serial number.
    serial: u32,
    /// Page sequence number within its logical bitstream.
    sequence: u32,
    /// Lacing values.
    laces: Vec<u8>,
    /// Page payload (its length is the sum of the lacing values).
    data: Vec<u8>,
}

/// Computes Ogg's CRC-32 over `data`.
fn ogg_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for byte in data {
        let mut value = crc ^ (u32::from(*byte) << 24);
        for _ in 0..8 {
            value = if value & 0x8000_0000 != 0 {
                (value << 1) ^ OGG_CRC32_POLY
            } else {
                value << 1
            };
        }
        crc = value;
    }
    crc
}

/// Parses every page of an Ogg bitstream.
fn parse_pages(data: &[u8]) -> Result<Vec<Page>, Error> {
    let mut pages = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        if pos + PAGE_HEADER_LEN > data.len() || &data[pos..pos + 4] != OGG_MAGIC {
            return Err(Error::ParseError(format!(
                "Ogg: expected a page capture pattern (\"OggS\") at offset {pos}"
            )));
        }
        if data[pos + 4] != 0 {
            return Err(Error::ParseError(format!(
                "Ogg: unsupported page structure version {} at offset {pos}",
                data[pos + 4]
            )));
        }
        let header_type = data[pos + 5];
        let granule = i64::from_le_bytes(
            data[pos + 6..pos + 14]
                .try_into()
                .map_err(|_| Error::ParseError("Ogg: truncated granule position".to_string()))?,
        );
        let serial = u32::from_le_bytes(
            data[pos + 14..pos + 18]
                .try_into()
                .map_err(|_| Error::ParseError("Ogg: truncated serial number".to_string()))?,
        );
        let sequence = u32::from_le_bytes(
            data[pos + 18..pos + 22]
                .try_into()
                .map_err(|_| Error::ParseError("Ogg: truncated sequence number".to_string()))?,
        );
        let lace_count = usize::from(data[pos + 26]);
        let table_end = pos + PAGE_HEADER_LEN + lace_count;
        if table_end > data.len() {
            return Err(Error::ParseError(
                "Ogg: truncated segment table".to_string(),
            ));
        }
        let laces = data[pos + PAGE_HEADER_LEN..table_end].to_vec();
        let payload_len: usize = laces.iter().map(|lace| usize::from(*lace)).sum();
        let payload_end = table_end + payload_len;
        if payload_end > data.len() {
            return Err(Error::ParseError(format!(
                "Ogg: page at offset {pos} claims {payload_len} payload bytes but only {} remain",
                data.len() - table_end
            )));
        }
        pages.push(Page {
            header_type,
            granule,
            serial,
            sequence,
            laces,
            data: data[table_end..payload_end].to_vec(),
        });
        pos = payload_end;
    }
    if pages.is_empty() {
        return Err(Error::ParseError(
            "Ogg: no pages found in the bitstream".to_string(),
        ));
    }
    Ok(pages)
}

/// Serializes a page, computing its CRC over the finished bytes.
fn serialize_page(page: &Page) -> Vec<u8> {
    let mut out = Vec::with_capacity(PAGE_HEADER_LEN + page.laces.len() + page.data.len());
    out.extend_from_slice(OGG_MAGIC);
    out.push(0); // stream structure version
    out.push(page.header_type);
    out.extend_from_slice(&page.granule.to_le_bytes());
    out.extend_from_slice(&page.serial.to_le_bytes());
    out.extend_from_slice(&page.sequence.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]); // CRC placeholder
    out.push(page.laces.len() as u8);
    out.extend_from_slice(&page.laces);
    out.extend_from_slice(&page.data);
    let crc = ogg_crc32(&out);
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

/// Lacing values for a packet of `len` bytes.
///
/// A 255 lace means "the packet continues"; a value below 255 terminates it, so a
/// packet whose length is an exact multiple of 255 needs a trailing zero lace.
fn lacing(len: usize) -> Vec<u8> {
    let mut laces = Vec::with_capacity(len / 255 + 1);
    let mut remaining = len;
    while remaining >= 255 {
        laces.push(255);
        remaining -= 255;
    }
    laces.push(remaining as u8);
    laces
}

/// Identifies the codec of a logical bitstream from its identification packet.
fn identify(packet: &[u8]) -> Option<OggCodec> {
    if packet.starts_with(VORBIS_IDENT) {
        Some(OggCodec::Vorbis)
    } else if packet.starts_with(OPUS_IDENT) {
        Some(OggCodec::Opus)
    } else {
        None
    }
}

/// Finds the logical bitstream to retag: the first one, in file order, whose
/// opening page carries a Vorbis or Opus identification packet.
///
/// The *first page of each serial* is examined rather than only pages flagged
/// `BOS`. The identification header is by definition the first packet of a
/// logical bitstream, and keying off the payload rather than the flag byte also
/// retags files whose writer mis-set the header-type flags — including output
/// from this workspace's own Ogg muxer, which passes `continuation = true` when
/// building its opening page and therefore never sets `BOS`.
fn find_target_stream(pages: &[Page]) -> Option<(u32, OggCodec)> {
    let mut seen: Vec<u32> = Vec::new();
    for page in pages {
        if seen.contains(&page.serial) {
            continue;
        }
        seen.push(page.serial);
        if let Some(codec) = identify(&page.data) {
            return Some((page.serial, codec));
        }
    }
    None
}

/// The header packets of one logical bitstream, and where they end.
struct HeaderRun {
    /// Complete header packets, in order.
    packets: Vec<Vec<u8>>,
    /// Number of pages (of this serial) the header run occupies.
    page_count: usize,
}

/// De-laces the header packets of `serial` from the page list.
fn collect_header_packets(
    pages: &[Page],
    serial: u32,
    codec: OggCodec,
) -> Result<HeaderRun, Error> {
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut page_count = 0usize;

    for page in pages.iter().filter(|page| page.serial == serial) {
        page_count += 1;
        let mut offset = 0usize;
        let mut completed_on_this_page = 0usize;
        for lace in &page.laces {
            let take = usize::from(*lace);
            current.extend_from_slice(&page.data[offset..offset + take]);
            offset += take;
            if *lace < 255 {
                packets.push(std::mem::take(&mut current));
                completed_on_this_page += 1;
                if packets.len() == codec.header_packets() {
                    // The remaining laces on this page would belong to audio
                    // packets, which this rewriter never moves.
                    let consumed: usize = page
                        .laces
                        .iter()
                        .scan(0usize, |seen, lace| {
                            *seen += usize::from(*lace < 255);
                            Some(*seen)
                        })
                        .position(|seen| seen == completed_on_this_page)
                        .map_or(page.laces.len(), |index| index + 1);
                    if consumed != page.laces.len() {
                        return Err(Error::Unsupported(
                            "embed(VorbisComments): this Ogg stream packs audio data onto the same \
                             page as its last header packet. Both the Vorbis and the Opus mapping \
                             require the header packets to end on a page boundary, and rewriting \
                             the comment header here would mean re-timing audio pages, which this \
                             embed never does"
                                .to_string(),
                        ));
                    }
                    return Ok(HeaderRun {
                        packets,
                        page_count,
                    });
                }
            }
        }
    }

    Err(Error::ParseError(format!(
        "Ogg: the bitstream ends after {} header packet(s); {} are required for {codec:?}",
        packets.len(),
        codec.header_packets()
    )))
}

/// Re-laces `packets` into pages for `serial`, starting at sequence 0.
fn build_header_pages(packets: &[Vec<u8>], serial: u32) -> Vec<Page> {
    // Flatten every packet into one lace/data stream, then cut it into pages of
    // at most 255 lacing values.
    let mut laces: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    for packet in packets {
        laces.extend_from_slice(&lacing(packet.len()));
        data.extend_from_slice(packet);
    }

    let mut pages = Vec::new();
    let mut lace_index = 0usize;
    let mut data_offset = 0usize;
    let mut sequence = 0u32;
    let mut continued = false;
    while lace_index < laces.len() {
        let end = (lace_index + 255).min(laces.len());
        let page_laces = laces[lace_index..end].to_vec();
        let payload_len: usize = page_laces.iter().map(|lace| usize::from(*lace)).sum();
        let mut header_type = 0u8;
        if sequence == 0 {
            header_type |= FLAG_BOS;
        }
        if continued {
            header_type |= FLAG_CONTINUED;
        }
        // A page whose last lace is 255 leaves its packet unfinished.
        continued = page_laces.last().copied() == Some(255);
        pages.push(Page {
            header_type,
            // Header pages carry no decoded audio, so their granule is zero for
            // both the Vorbis and the Opus mapping.
            granule: 0,
            serial,
            sequence,
            laces: page_laces,
            data: data[data_offset..data_offset + payload_len].to_vec(),
        });
        data_offset += payload_len;
        lace_index = end;
        sequence += 1;
    }
    pages
}

/// Builds the comment packet for `codec` from `metadata`.
fn build_comment_packet(codec: OggCodec, metadata: &Metadata) -> Result<Vec<u8>, Error> {
    let comments = crate::vorbis::write(metadata)?;
    let mut packet = Vec::with_capacity(comments.len() + 9);
    packet.extend_from_slice(codec.comment_prefix());
    packet.extend_from_slice(&comments);
    if codec == OggCodec::Vorbis {
        packet.push(VORBIS_FRAMING_BIT);
    }
    Ok(packet)
}

/// Embeds `metadata` as the Vorbis comment header of an Ogg bitstream.
///
/// # Errors
///
/// * [`Error::Unsupported`] if the bitstream is not Ogg, carries no
///   Vorbis/Opus logical stream, or packs audio onto a header page.
/// * [`Error::ParseError`] if a page or the header packet run is malformed.
pub fn embed(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    if !file_data.starts_with(OGG_MAGIC) {
        return Err(Error::Unsupported(
            "embed(VorbisComments) targets an Ogg bitstream; the given file_data does not start \
             with the \"OggS\" capture pattern"
                .to_string(),
        ));
    }
    let pages = parse_pages(file_data)?;

    // ── Pick the logical bitstream to retag ──────────────────────────────────
    let (serial, codec) = find_target_stream(&pages).ok_or_else(|| {
        Error::Unsupported(
            "embed(VorbisComments): no Vorbis (\"\\x01vorbis\") or Opus (\"OpusHead\") logical \
             bitstream was found in this Ogg file. Speex, Theora and FLAC-in-Ogg carry their \
             comments in a different packet layout, which this embed does not write"
                .to_string(),
        )
    })?;

    // ── De-lace the header run and substitute the comment packet ─────────────
    let header = collect_header_packets(&pages, serial, codec)?;
    let mut packets = header.packets;
    if !packets[1].starts_with(codec.comment_prefix()) {
        return Err(Error::ParseError(format!(
            "Ogg: the second header packet of the {codec:?} stream does not start with the \
             expected comment-header signature"
        )));
    }
    packets[1] = build_comment_packet(codec, metadata)?;

    // ── Re-lace the header run, then splice it back in ───────────────────────
    let new_header_pages = build_header_pages(&packets, serial);
    let sequence_delta = new_header_pages.len() as i64 - header.page_count as i64;

    // The replacement pages are emitted at the positions the old header pages
    // occupied, one for one, so that a multiplexed file keeps its page
    // interleaving (Ogg requires every stream's opening page before any other
    // stream's data). Surplus replacement pages go out at the last old header
    // page's position; a shorter run simply leaves those positions empty.
    let mut out: Vec<u8> = Vec::with_capacity(file_data.len() + 256);
    let mut pending = new_header_pages.iter();
    let mut seen_of_serial = 0usize;
    for page in &pages {
        if page.serial != serial {
            // Another logical bitstream: its pages and sequence numbers are
            // independent of ours.
            out.extend_from_slice(&serialize_page(page));
            continue;
        }
        seen_of_serial += 1;
        if seen_of_serial <= header.page_count {
            if seen_of_serial == header.page_count {
                for new_page in pending.by_ref() {
                    out.extend_from_slice(&serialize_page(new_page));
                }
            } else if let Some(new_page) = pending.next() {
                out.extend_from_slice(&serialize_page(new_page));
            }
            continue;
        }
        let sequence = u32::try_from(i64::from(page.sequence) + sequence_delta).map_err(|_| {
            Error::ParseError(
                "Ogg: renumbering the pages after the comment header underflowed".to_string(),
            )
        })?;
        out.extend_from_slice(&serialize_page(&Page {
            header_type: page.header_type,
            granule: page.granule,
            serial: page.serial,
            sequence,
            laces: page.laces.clone(),
            data: page.data.clone(),
        }));
    }
    Ok(out)
}

/// Extracts the raw Vorbis-comment block (vendor string + comment list, without
/// the codec-specific packet prefix) from an Ogg bitstream.
///
/// This is the inverse of [`embed`] and what the round-trip tests read back.
///
/// # Errors
///
/// Returns the same errors as [`embed`] for a malformed or unsupported stream.
pub fn extract_comments(file_data: &[u8]) -> Result<Vec<u8>, Error> {
    let pages = parse_pages(file_data)?;
    let (serial, codec) = pages
        .iter()
        .filter(|page| page.header_type & FLAG_BOS != 0)
        .find_map(|page| identify(&page.data).map(|codec| (page.serial, codec)))
        .ok_or_else(|| {
            Error::Unsupported(
                "extract_comments: no Vorbis or Opus logical bitstream found".to_string(),
            )
        })?;
    let header = collect_header_packets(&pages, serial, codec)?;
    let packet = &header.packets[1];
    let prefix = codec.comment_prefix();
    if !packet.starts_with(prefix) {
        return Err(Error::ParseError(
            "Ogg: second header packet is not a comment header".to_string(),
        ));
    }
    let body = &packet[prefix.len()..];
    let body = if codec == OggCodec::Vorbis {
        body.get(..body.len().saturating_sub(1)).unwrap_or(body)
    } else {
        body
    };
    Ok(body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetadataFormat, MetadataValue};

    /// Serializes packets into an Ogg stream: header packets on their own pages,
    /// then one audio page.
    fn build_ogg(codec: OggCodec, serial: u32, comment_body: &[u8], audio: &[u8]) -> Vec<u8> {
        let ident: Vec<u8> = match codec {
            OggCodec::Vorbis => {
                let mut packet = VORBIS_IDENT.to_vec();
                packet.extend_from_slice(&[0u8; 23]);
                packet
            }
            OggCodec::Opus => {
                let mut packet = OPUS_IDENT.to_vec();
                packet.extend_from_slice(&[0u8; 11]);
                packet
            }
        };
        let mut comment = codec.comment_prefix().to_vec();
        comment.extend_from_slice(comment_body);
        if codec == OggCodec::Vorbis {
            comment.push(VORBIS_FRAMING_BIT);
        }

        let mut packets = vec![ident, comment];
        if codec == OggCodec::Vorbis {
            // Setup/codebook header: opaque here, but it must survive the rewrite.
            let mut setup = b"\x05vorbis".to_vec();
            setup.extend_from_slice(&[0xA5; 600]);
            packets.push(setup);
        }

        let mut out = Vec::new();
        for page in build_header_pages(&packets, serial) {
            out.extend_from_slice(&serialize_page(&page));
        }
        let header_pages = build_header_pages(&packets, serial).len() as u32;
        out.extend_from_slice(&serialize_page(&Page {
            header_type: 0x04, // EOS
            granule: 4096,
            serial,
            sequence: header_pages,
            laces: lacing(audio.len()),
            data: audio.to_vec(),
        }));
        out
    }

    fn empty_comment_body() -> Vec<u8> {
        crate::vorbis::write(&Metadata::new(MetadataFormat::VorbisComments))
            .expect("encode empty comments")
    }

    fn tagged_metadata() -> Metadata {
        let mut metadata = Metadata::new(MetadataFormat::VorbisComments);
        metadata.insert(
            "TITLE".to_string(),
            MetadataValue::Text("Ogg Round Trip".to_string()),
        );
        metadata.insert(
            "ARTIST".to_string(),
            MetadataValue::Text("Test Artist".to_string()),
        );
        metadata
    }

    /// Verifies every page's stored CRC against a fresh computation.
    fn assert_crcs_valid(data: &[u8]) {
        let mut pos = 0usize;
        while pos < data.len() {
            assert_eq!(&data[pos..pos + 4], OGG_MAGIC, "capture pattern at {pos}");
            let lace_count = usize::from(data[pos + 26]);
            let table_end = pos + PAGE_HEADER_LEN + lace_count;
            let payload: usize = data[pos + PAGE_HEADER_LEN..table_end]
                .iter()
                .map(|lace| usize::from(*lace))
                .sum();
            let end = table_end + payload;
            let stored = u32::from_le_bytes([
                data[pos + 22],
                data[pos + 23],
                data[pos + 24],
                data[pos + 25],
            ]);
            let mut page = data[pos..end].to_vec();
            page[22..26].fill(0);
            assert_eq!(ogg_crc32(&page), stored, "CRC mismatch on page at {pos}");
            pos = end;
        }
    }

    #[test]
    fn test_crc_matches_the_known_ogg_polynomial() {
        // Ogg's CRC-32: polynomial 0x04C11DB7, initial value 0, no input or
        // output reflection, no final xor. Over the standard "123456789" check
        // string those parameters produce 0x89A1897F (verified independently of
        // this implementation, and cross-checked against a container-muxed Ogg
        // file in tests/embed_container_roundtrip.rs).
        assert_eq!(ogg_crc32(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn test_lacing_terminates_exact_multiples_of_255() {
        assert_eq!(lacing(0), vec![0]);
        assert_eq!(lacing(254), vec![254]);
        assert_eq!(lacing(255), vec![255, 0]);
        assert_eq!(lacing(256), vec![255, 1]);
        assert_eq!(lacing(510), vec![255, 255, 0]);
    }

    #[test]
    fn test_vorbis_round_trip_preserves_setup_header_and_audio() {
        let ogg = build_ogg(
            OggCodec::Vorbis,
            0x1234_5678,
            &empty_comment_body(),
            b"AUDIOPAYLOAD",
        );
        let before = collect_header_packets(
            &parse_pages(&ogg).expect("parse"),
            0x1234_5678,
            OggCodec::Vorbis,
        )
        .expect("collect");

        let out = embed(&ogg, &tagged_metadata()).expect("embed vorbis comments");
        assert_crcs_valid(&out);

        let after = collect_header_packets(
            &parse_pages(&out).expect("parse rewritten"),
            0x1234_5678,
            OggCodec::Vorbis,
        )
        .expect("collect rewritten");

        assert_eq!(after.packets.len(), 3);
        assert_eq!(
            after.packets[0], before.packets[0],
            "identification header must be preserved byte for byte"
        );
        assert_eq!(
            after.packets[2], before.packets[2],
            "setup/codebook header must survive the comment rewrite"
        );

        let comments = extract_comments(&out).expect("extract");
        let parsed = crate::vorbis::parse(&comments).expect("parse comments");
        assert_eq!(
            parsed.get("TITLE").and_then(MetadataValue::as_text),
            Some("Ogg Round Trip")
        );
        assert_eq!(
            parsed.get("ARTIST").and_then(MetadataValue::as_text),
            Some("Test Artist")
        );

        // The audio page must be preserved verbatim apart from its sequence number.
        let pages = parse_pages(&out).expect("parse");
        let audio = pages.last().expect("audio page");
        assert_eq!(audio.data, b"AUDIOPAYLOAD".to_vec());
        assert_eq!(audio.granule, 4096);
    }

    #[test]
    fn test_opus_round_trip_has_no_framing_bit() {
        let ogg = build_ogg(OggCodec::Opus, 7, &empty_comment_body(), b"OPUSAUDIO");
        let out = embed(&ogg, &tagged_metadata()).expect("embed opus tags");
        assert_crcs_valid(&out);

        let pages = parse_pages(&out).expect("parse");
        let header = collect_header_packets(&pages, 7, OggCodec::Opus).expect("collect");
        assert_eq!(header.packets.len(), 2);
        assert!(header.packets[1].starts_with(OPUS_COMMENT));

        let comments = extract_comments(&out).expect("extract");
        let parsed = crate::vorbis::parse(&comments).expect("parse comments");
        assert_eq!(
            parsed.get("TITLE").and_then(MetadataValue::as_text),
            Some("Ogg Round Trip")
        );
        // An Opus comment packet is exactly the prefix plus the comment block.
        let mut expected = OPUS_COMMENT.to_vec();
        expected.extend_from_slice(&comments);
        assert_eq!(header.packets[1], expected);
    }

    #[test]
    fn test_sequence_numbers_stay_contiguous_when_the_header_grows() {
        let ogg = build_ogg(OggCodec::Opus, 42, &empty_comment_body(), b"AUDIO");

        // A comment block far larger than one page's 255x255 payload.
        let mut metadata = Metadata::new(MetadataFormat::VorbisComments);
        for index in 0..40 {
            metadata.insert(
                format!("COMMENT{index:02}"),
                MetadataValue::Text("v".repeat(4000)),
            );
        }
        let out = embed(&ogg, &metadata).expect("embed oversized comments");
        assert_crcs_valid(&out);

        let pages = parse_pages(&out).expect("parse");
        assert!(
            pages.len() > 3,
            "an oversized comment packet must span several pages"
        );
        for (index, page) in pages.iter().enumerate() {
            assert_eq!(
                page.sequence, index as u32,
                "page sequence numbers must stay contiguous"
            );
        }
        // Continuation flags must mark every page that resumes a packet.
        for window in pages.windows(2) {
            let continues = window[0].laces.last().copied() == Some(255);
            assert_eq!(
                window[1].header_type & FLAG_CONTINUED != 0,
                continues,
                "continuation flag must match the previous page's final lace"
            );
        }

        let comments = extract_comments(&out).expect("extract");
        let parsed = crate::vorbis::parse(&comments).expect("parse");
        assert_eq!(
            parsed
                .get("COMMENT39")
                .and_then(MetadataValue::as_text)
                .map(str::len),
            Some(4000)
        );
    }

    #[test]
    fn test_other_logical_streams_are_untouched() {
        let mut ogg = build_ogg(OggCodec::Opus, 1, &empty_comment_body(), b"AUDIO");
        // A second, foreign logical bitstream (a single BOS page).
        let foreign = serialize_page(&Page {
            header_type: FLAG_BOS,
            granule: 0,
            serial: 999,
            sequence: 0,
            laces: lacing(9),
            data: b"otherdata".to_vec(),
        });
        ogg.extend_from_slice(&foreign);

        let out = embed(&ogg, &tagged_metadata()).expect("embed");
        assert_crcs_valid(&out);
        let pages = parse_pages(&out).expect("parse");
        let other = pages
            .iter()
            .find(|page| page.serial == 999)
            .expect("foreign stream preserved");
        assert_eq!(other.sequence, 0, "foreign sequence numbers must not shift");
        assert_eq!(other.data, b"otherdata".to_vec());
    }

    #[test]
    fn test_multiplexed_interleaving_is_preserved() {
        // A multiplexed file: both streams' opening pages come first, then the
        // rest of the headers. Ogg requires every stream's BOS page before any
        // other page, so the replacement run must not jump ahead of stream B.
        let opus = build_ogg(OggCodec::Opus, 1, &empty_comment_body(), b"AUDIO");
        let pages = parse_pages(&opus).expect("parse fixture");
        let foreign_bos = Page {
            header_type: FLAG_BOS,
            granule: 0,
            serial: 777,
            sequence: 0,
            laces: lacing(9),
            data: b"otherhdr!".to_vec(),
        };

        // Interleave: [A_ident, B_bos, A_comment, A_audio].
        let mut interleaved = Vec::new();
        interleaved.extend_from_slice(&serialize_page(&pages[0]));
        interleaved.extend_from_slice(&serialize_page(&foreign_bos));
        for page in &pages[1..] {
            interleaved.extend_from_slice(&serialize_page(page));
        }

        let out = embed(&interleaved, &tagged_metadata()).expect("embed into multiplexed ogg");
        assert_crcs_valid(&out);

        let result = parse_pages(&out).expect("parse rewritten");
        assert_eq!(
            result[0].serial, 1,
            "stream A's opening page must stay first"
        );
        assert_eq!(
            result[1].serial, 777,
            "the second stream's BOS page must still precede A's later header pages"
        );
        assert_eq!(result[1].data, b"otherhdr!".to_vec());
        assert_eq!(result[1].sequence, 0);

        let comments = extract_comments(&out).expect("extract");
        let parsed = crate::vorbis::parse(&comments).expect("parse comments");
        assert_eq!(
            parsed.get("TITLE").and_then(MetadataValue::as_text),
            Some("Ogg Round Trip")
        );
    }

    #[test]
    fn test_rejects_non_ogg_input() {
        let result = embed(b"not an ogg stream", &tagged_metadata());
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn test_rejects_unsupported_codec() {
        // A BOS page whose identification packet is neither Vorbis nor Opus.
        let stream = serialize_page(&Page {
            header_type: FLAG_BOS,
            granule: 0,
            serial: 5,
            sequence: 0,
            laces: lacing(7),
            data: b"\x80theora".to_vec(),
        });
        match embed(&stream, &tagged_metadata()) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("Vorbis"), "message was: {message}");
            }
            other => panic!("expected an honest Unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn test_truncated_page_is_a_parse_error_and_leaves_input_alone() {
        let ogg = build_ogg(OggCodec::Opus, 3, &empty_comment_body(), b"AUDIO");
        let truncated = ogg[..ogg.len() - 3].to_vec();
        let copy = truncated.clone();
        assert!(matches!(
            embed(&truncated, &tagged_metadata()),
            Err(Error::ParseError(_))
        ));
        assert_eq!(truncated, copy);
    }
}

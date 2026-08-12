//! FLAC `METADATA_BLOCK`-aware Vorbis-comment embedding.
//!
//! A native FLAC stream is `"fLaC"` followed by a chain of metadata blocks and
//! then the audio frames. Each block header is 4 bytes:
//!
//! ```text
//! bit  0      : last-metadata-block flag
//! bits 1..=7  : BLOCK_TYPE (0 = STREAMINFO, 1 = PADDING, 4 = VORBIS_COMMENT, …)
//! bytes 1..=3 : 24-bit big-endian body length
//! ```
//!
//! [`embed`] walks that chain, replaces (or inserts) the `VORBIS_COMMENT`
//! block, keeps `STREAMINFO` first as the format requires, maintains exactly one
//! last-block flag, and — when a `PADDING` block is available — absorbs the size
//! change into the padding so the audio frames never move.
//!
//! `SEEKTABLE` seek points are byte offsets *relative to the first audio frame*,
//! so they stay valid across metadata resizing and are copied verbatim.

use crate::{Error, Metadata};

/// FLAC stream marker.
pub(super) const FLAC_MAGIC: &[u8; 4] = b"fLaC";

/// `STREAMINFO` block type — mandatory, and mandatory *first*.
const BLOCK_STREAMINFO: u8 = 0;
/// `PADDING` block type — free space that may be resized at will.
const BLOCK_PADDING: u8 = 1;
/// `VORBIS_COMMENT` block type — the tag block this module writes.
const BLOCK_VORBIS_COMMENT: u8 = 4;

/// Largest body a single metadata block can declare (24-bit length field).
const MAX_BLOCK_LEN: usize = 0x00FF_FFFF;

/// One parsed metadata block.
struct Block {
    /// `BLOCK_TYPE` (7 bits).
    block_type: u8,
    /// Block body (header excluded).
    body: Vec<u8>,
}

/// Parses the metadata-block chain of a native FLAC stream.
///
/// Returns the blocks in file order plus the offset at which the audio frames
/// begin.
fn parse_blocks(data: &[u8]) -> Result<(Vec<Block>, usize), Error> {
    if data.len() < 4 || &data[..4] != FLAC_MAGIC {
        return Err(Error::ParseError(
            "Not a native FLAC stream (missing \"fLaC\" marker)".to_string(),
        ));
    }

    let mut blocks: Vec<Block> = Vec::new();
    let mut pos = 4usize;
    loop {
        if pos + 4 > data.len() {
            return Err(Error::ParseError(
                "Truncated FLAC: ran out of data inside the metadata-block chain".to_string(),
            ));
        }
        let is_last = data[pos] & 0x80 != 0;
        let block_type = data[pos] & 0x7F;
        let len = (usize::from(data[pos + 1]) << 16)
            | (usize::from(data[pos + 2]) << 8)
            | usize::from(data[pos + 3]);
        let body_start = pos + 4;
        let body_end = body_start
            .checked_add(len)
            .ok_or_else(|| Error::ParseError("FLAC metadata block length overflows".to_string()))?;
        if body_end > data.len() {
            return Err(Error::ParseError(format!(
                "Truncated FLAC: metadata block of type {block_type} claims {len} bytes but only \
                 {} remain",
                data.len() - body_start
            )));
        }
        if blocks.is_empty() && block_type != BLOCK_STREAMINFO {
            return Err(Error::ParseError(format!(
                "Malformed FLAC: first metadata block must be STREAMINFO (type 0), found type \
                 {block_type}"
            )));
        }
        blocks.push(Block {
            block_type,
            body: data[body_start..body_end].to_vec(),
        });
        pos = body_end;
        if is_last {
            break;
        }
    }

    if blocks.is_empty() {
        return Err(Error::ParseError(
            "Malformed FLAC: no metadata blocks found".to_string(),
        ));
    }
    Ok((blocks, pos))
}

/// Serializes one metadata block (4-byte header + body).
fn encode_block(out: &mut Vec<u8>, block: &Block, is_last: bool) {
    let len = block.body.len();
    let first = (u8::from(is_last) << 7) | (block.block_type & 0x7F);
    out.push(first);
    out.push(((len >> 16) & 0xFF) as u8);
    out.push(((len >> 8) & 0xFF) as u8);
    out.push((len & 0xFF) as u8);
    out.extend_from_slice(&block.body);
}

/// Embeds `metadata` as the `VORBIS_COMMENT` block of a native FLAC stream.
///
/// The existing `VORBIS_COMMENT` block is replaced in place if present;
/// otherwise a new one is inserted directly after `STREAMINFO`. Every other
/// block (`SEEKTABLE`, `PICTURE`, `APPLICATION`, `CUESHEET`, …) is preserved
/// byte-for-byte, and the audio frames are copied verbatim.
///
/// If the file has a `PADDING` block, its size is adjusted by the inverse of the
/// comment-block size change so that the audio frames keep their exact byte
/// offset. When that is impossible (no padding block, or the padding is too
/// small to absorb a growth) the audio simply shifts — which is legal, because
/// FLAC `SEEKTABLE` offsets are relative to the first audio frame.
///
/// # Errors
///
/// Returns [`Error::ParseError`] if `file_data` is not a well-formed FLAC
/// metadata chain (in which case the caller's bytes are left untouched — this
/// function is pure), and [`Error::Unsupported`] if the encoded comment block
/// would exceed FLAC's 24-bit block-length limit.
pub fn embed(file_data: &[u8], metadata: &Metadata) -> Result<Vec<u8>, Error> {
    let (mut blocks, audio_start) = parse_blocks(file_data)?;

    let comment_body = crate::vorbis::write(metadata)?;
    if comment_body.len() > MAX_BLOCK_LEN {
        return Err(Error::Unsupported(format!(
            "encoded Vorbis comment block is {} bytes, which exceeds FLAC's 24-bit \
             METADATA_BLOCK length limit ({MAX_BLOCK_LEN} bytes)",
            comment_body.len()
        )));
    }

    // Old size of the metadata region, so padding can absorb the delta.
    let old_metadata_len: usize = blocks.iter().map(|b| 4 + b.body.len()).sum();

    match blocks
        .iter()
        .position(|b| b.block_type == BLOCK_VORBIS_COMMENT)
    {
        Some(idx) => blocks[idx].body = comment_body,
        None => blocks.insert(
            1,
            Block {
                block_type: BLOCK_VORBIS_COMMENT,
                body: comment_body,
            },
        ),
    }

    // ── Absorb the size change into PADDING so the audio never moves ─────────
    let new_metadata_len: usize = blocks.iter().map(|b| 4 + b.body.len()).sum();
    if new_metadata_len != old_metadata_len {
        if let Some(pad) = blocks.iter_mut().find(|b| b.block_type == BLOCK_PADDING) {
            let target = (pad.body.len() + old_metadata_len).checked_sub(new_metadata_len);
            if let Some(target) = target {
                if target <= MAX_BLOCK_LEN {
                    pad.body.clear();
                    pad.body.resize(target, 0);
                }
            }
        }
    }

    // ── Emit: magic + blocks (exactly one last-flag) + verbatim audio ────────
    let mut out = Vec::with_capacity(file_data.len() + 128);
    out.extend_from_slice(FLAC_MAGIC);
    let last_index = blocks.len() - 1;
    for (i, block) in blocks.iter().enumerate() {
        encode_block(&mut out, block, i == last_index);
    }
    out.extend_from_slice(&file_data[audio_start..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetadataFormat, MetadataValue};

    /// A 34-byte STREAMINFO body (contents are opaque to this module).
    fn streaminfo_body() -> Vec<u8> {
        let mut body = vec![0u8; 34];
        body[0] = 0x10; // min block size high byte — plausible, never parsed here
        body
    }

    /// Builds a FLAC stream: `fLaC` + the given blocks + `audio` frames.
    fn build_flac(blocks: &[(u8, Vec<u8>)], audio: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(FLAC_MAGIC);
        for (i, (block_type, body)) in blocks.iter().enumerate() {
            let is_last = i + 1 == blocks.len();
            encode_block(
                &mut out,
                &Block {
                    block_type: *block_type,
                    body: body.clone(),
                },
                is_last,
            );
        }
        out.extend_from_slice(audio);
        out
    }

    fn comment_body(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut meta = Metadata::new(MetadataFormat::VorbisComments);
        for (k, v) in pairs {
            meta.insert((*k).to_string(), MetadataValue::Text((*v).to_string()));
        }
        crate::vorbis::write(&meta).expect("encode vorbis comments in test")
    }

    #[test]
    fn test_inserts_comment_block_after_streaminfo() {
        let flac = build_flac(&[(BLOCK_STREAMINFO, streaminfo_body())], b"AUDIOFRAMES");

        let mut meta = Metadata::new(MetadataFormat::VorbisComments);
        meta.insert(
            "TITLE".to_string(),
            MetadataValue::Text("Hello".to_string()),
        );
        let out = embed(&flac, &meta).expect("embed into flac");

        let (blocks, audio_start) = parse_blocks(&out).expect("re-parse embedded flac");
        assert_eq!(blocks[0].block_type, BLOCK_STREAMINFO);
        assert_eq!(blocks[1].block_type, BLOCK_VORBIS_COMMENT);
        assert_eq!(&out[audio_start..], b"AUDIOFRAMES");

        let parsed = crate::vorbis::parse(&blocks[1].body).expect("parse comment block");
        assert_eq!(
            parsed.get("TITLE").and_then(MetadataValue::as_text),
            Some("Hello")
        );
    }

    #[test]
    fn test_replaces_existing_comment_block_in_place() {
        let flac = build_flac(
            &[
                (BLOCK_STREAMINFO, streaminfo_body()),
                (BLOCK_VORBIS_COMMENT, comment_body(&[("TITLE", "Old")])),
                (3, vec![0u8; 18]), // SEEKTABLE — must survive verbatim
            ],
            b"AUDIO",
        );

        let mut meta = Metadata::new(MetadataFormat::VorbisComments);
        meta.insert("TITLE".to_string(), MetadataValue::Text("New".to_string()));
        let out = embed(&flac, &meta).expect("embed into flac");

        let (blocks, _) = parse_blocks(&out).expect("re-parse");
        assert_eq!(
            blocks
                .iter()
                .filter(|b| b.block_type == BLOCK_VORBIS_COMMENT)
                .count(),
            1,
            "exactly one VORBIS_COMMENT block after replace"
        );
        assert_eq!(blocks[2].block_type, 3, "SEEKTABLE preserved in position");
        assert_eq!(blocks[2].body.len(), 18);

        let parsed = crate::vorbis::parse(&blocks[1].body).expect("parse comment block");
        assert_eq!(
            parsed.get("TITLE").and_then(MetadataValue::as_text),
            Some("New")
        );
    }

    #[test]
    fn test_padding_absorbs_growth_so_audio_offset_is_stable() {
        let flac = build_flac(
            &[
                (BLOCK_STREAMINFO, streaminfo_body()),
                (BLOCK_VORBIS_COMMENT, comment_body(&[])),
                (BLOCK_PADDING, vec![0u8; 512]),
            ],
            b"AUDIOAUDIO",
        );
        let (_, original_audio_start) = parse_blocks(&flac).expect("parse original");

        let mut meta = Metadata::new(MetadataFormat::VorbisComments);
        meta.insert(
            "ARTIST".to_string(),
            MetadataValue::Text("A Rather Long Artist Name".to_string()),
        );
        let out = embed(&flac, &meta).expect("embed into flac");

        let (_, new_audio_start) = parse_blocks(&out).expect("re-parse");
        assert_eq!(
            new_audio_start, original_audio_start,
            "padding must absorb the comment-block growth"
        );
        assert_eq!(&out[new_audio_start..], b"AUDIOAUDIO");
    }

    #[test]
    fn test_exactly_one_last_block_flag() {
        let flac = build_flac(
            &[
                (BLOCK_STREAMINFO, streaminfo_body()),
                (BLOCK_PADDING, vec![0u8; 8]),
            ],
            b"AUDIO",
        );
        let mut meta = Metadata::new(MetadataFormat::VorbisComments);
        meta.insert("TITLE".to_string(), MetadataValue::Text("T".to_string()));
        let out = embed(&flac, &meta).expect("embed into flac");

        // Walk the raw chain and count last-flags.
        let mut pos = 4usize;
        let mut last_flags = 0usize;
        loop {
            let is_last = out[pos] & 0x80 != 0;
            if is_last {
                last_flags += 1;
            }
            let len = (usize::from(out[pos + 1]) << 16)
                | (usize::from(out[pos + 2]) << 8)
                | usize::from(out[pos + 3]);
            pos += 4 + len;
            if is_last {
                break;
            }
        }
        assert_eq!(last_flags, 1);
    }

    #[test]
    fn test_rejects_non_flac_without_touching_input() {
        let mut meta = Metadata::new(MetadataFormat::VorbisComments);
        meta.insert("TITLE".to_string(), MetadataValue::Text("T".to_string()));
        let input = b"this is not a flac stream".to_vec();
        let result = embed(&input, &meta);
        assert!(matches!(result, Err(Error::ParseError(_))));
        assert_eq!(input, b"this is not a flac stream".to_vec());
    }

    #[test]
    fn test_rejects_truncated_block_chain() {
        // STREAMINFO header claiming 34 bytes, but only 4 present.
        let mut flac = Vec::new();
        flac.extend_from_slice(FLAC_MAGIC);
        flac.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
        flac.extend_from_slice(&[0u8; 4]);
        let meta = Metadata::new(MetadataFormat::VorbisComments);
        assert!(matches!(embed(&flac, &meta), Err(Error::ParseError(_))));
    }

    #[test]
    fn test_rejects_first_block_not_streaminfo() {
        let flac = build_flac(&[(BLOCK_PADDING, vec![0u8; 4])], b"AUDIO");
        let meta = Metadata::new(MetadataFormat::VorbisComments);
        assert!(matches!(embed(&flac, &meta), Err(Error::ParseError(_))));
    }
}

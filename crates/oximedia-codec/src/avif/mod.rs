//! AVIF (AV1 Image File Format) encoder and decoder.
//!
//! AVIF stores still images using AV1 intra-frame compression inside an
//! ISOBMFF (ISO Base Media File Format) container.
//!
//! # Decode status
//!
//! - [`AvifDecoder::probe`] — parses the container and returns dimensions,
//!   bit depth, colour metadata, and alpha presence, without touching AV1
//!   pixel decode at all.
//! - [`AvifDecoder::extract_av1_payload`] — parses the container and
//!   returns the raw AV1 OBU bitstream(s), honestly labelled as bitstream
//!   data (for handoff to an external AV1 decoder, or for inspecting a
//!   payload this crate's own decoder can't yet decode).
//! - [`AvifDecoder::decode`] — decodes real pixels through
//!   [`crate::av1::Av1Decoder`]'s bit-exact keyframe/intra pipeline
//!   (verified against dav1d and aomdec; see `av1::kf`). That pipeline
//!   implements 8-bit 4:2:0 (profile 0) reconstruction only — 10/12-bit,
//!   non-4:2:0, and monochrome AV1 (which real-world encoders typically use
//!   for the alpha auxiliary image) fail honestly with
//!   [`CodecError::UnsupportedFeature`] rather than fabricating pixels.
//!   When the `av1` cargo feature is disabled, `decode` always returns
//!   `CodecError::UnsupportedFeature`.
//!
//! Note that this crate's own [`AvifEncoder`] does not implement real AV1
//! encoding: it writes a structurally valid container around a minimal
//! placeholder AV1 payload (sequence header only, no coded frame), so
//! `AvifDecoder::decode`-ing `AvifEncoder`'s own output always fails
//! honestly too (there is no frame to decode). See
//! `src/avif/testdata/README.md` for real AV1-encoded fixtures (produced by
//! ffmpeg + libaom) used to test real decoding.
//!
//! # Container structure
//!
//! ```text
//! ftyp  – file-type box  (brand = 'avif', compat = ['avif','mif1','miaf'])
//! meta  – metadata box
//!   hdlr  – handler reference ('pict')
//!   pitm  – primary item (item_ID = 1)
//!   iloc  – item location (points into mdat)
//!   iinf  – item information (one 'av01' entry)
//!   iprp  – item properties
//!     ipco  – property container
//!       ispe  – image spatial extents (width, height)
//!       colr  – colour information (nclx or restricted ICC)
//!       av1C  – AV1 codec configuration record
//!       pixi  – pixel information (bit depth)
//!     ipma  – item property association
//! mdat  – media data (AV1 OBU bitstream)
//! ```

use crate::error::CodecError;

mod container;
use container::{
    check_avif_signature, find_box_in, find_top_level_box, locate_mdat_items, parse_meta_for_probe,
};

#[cfg(feature = "av1")]
mod decode;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Encoding configuration for [`AvifEncoder`].
#[derive(Debug, Clone)]
pub struct AvifConfig {
    /// Perceptual quality, 0–100 (100 = lossless).
    pub quality: u8,
    /// Encoder speed preset, 0–10 (0 = slowest/best, 10 = fastest).
    pub speed: u8,
    /// Colour primaries (ISO 23091-2 / H.273).
    /// 1 = BT.709, 9 = BT.2020.
    pub color_primaries: u8,
    /// Transfer characteristics.
    /// 1 = BT.709, 16 = PQ (SMPTE ST 2084), 18 = HLG.
    pub transfer_characteristics: u8,
    /// Matrix coefficients.
    /// 1 = BT.709, 9 = BT.2020 NCL.
    pub matrix_coefficients: u8,
    /// Whether the YUV values use the full [0, 2^n-1] range.
    pub full_range: bool,
    /// If `Some(q)`, encode an alpha plane at quality `q`; otherwise omit it.
    pub alpha_quality: Option<u8>,
}

impl Default for AvifConfig {
    fn default() -> Self {
        Self {
            quality: 60,
            speed: 6,
            color_primaries: 1,
            transfer_characteristics: 1,
            matrix_coefficients: 1,
            full_range: false,
            alpha_quality: None,
        }
    }
}

/// Chroma subsampling format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvFormat {
    /// 4:2:0 – U/V planes are half width and half height.
    Yuv420,
    /// 4:2:2 – U/V planes are half width, full height.
    Yuv422,
    /// 4:4:4 – U/V planes are full width and full height.
    Yuv444,
}

/// In-memory AVIF image, in planar YCbCr form.
#[derive(Debug, Clone)]
pub struct AvifImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Bit depth: 8, 10, or 12.
    pub depth: u8,
    /// Chroma subsampling format.
    pub yuv_format: YuvFormat,
    /// Luma (Y) plane samples.  Each row is `stride_y` bytes wide.
    pub y_plane: Vec<u8>,
    /// Cb (U) plane samples.
    pub u_plane: Vec<u8>,
    /// Cr (V) plane samples.
    pub v_plane: Vec<u8>,
    /// Optional alpha plane samples (same dimensions as luma).
    pub alpha_plane: Option<Vec<u8>>,
}

/// Raw AV1 item payloads extracted from an AVIF container by
/// [`AvifDecoder::extract_av1_payload`].
///
/// These are **encoded AV1 OBU bitstreams**, not decoded pixels — they are
/// suitable for handing off to an AV1 decoder.
#[derive(Debug, Clone)]
pub struct AvifPayload {
    /// Raw AV1 OBU bitstream of the primary (colour) image item.
    pub color_obu: Vec<u8>,
    /// Raw AV1 OBU bitstream of the alpha auxiliary image item, if present.
    pub alpha_obu: Option<Vec<u8>>,
}

/// Lightweight metadata returned by [`AvifDecoder::probe`].
#[derive(Debug, Clone)]
pub struct AvifProbeResult {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Bit depth (8, 10, or 12).
    pub bit_depth: u8,
    /// Whether the file contains an alpha auxiliary image item.
    pub has_alpha: bool,
    /// Colour primaries code.
    pub color_primaries: u8,
    /// Transfer characteristics code.
    pub transfer_characteristics: u8,
}

// ─── Encoder ─────────────────────────────────────────────────────────────────

/// Encodes [`AvifImage`] frames to AVIF container bytes.
#[derive(Debug, Clone)]
pub struct AvifEncoder {
    config: AvifConfig,
}

impl AvifEncoder {
    /// Create a new encoder with the given configuration.
    pub fn new(config: AvifConfig) -> Self {
        Self { config }
    }

    /// Encode `image` to an AVIF byte stream.
    ///
    /// Returns the complete AVIF file as a `Vec<u8>`.
    pub fn encode(&self, image: &AvifImage) -> Result<Vec<u8>, CodecError> {
        validate_image(image)?;

        // Build the AV1 OBU payload (Sequence Header + one empty Frame).
        let av1_payload = build_av1_obu(image, &self.config);

        // Sizes we need to compute before writing boxes.
        let has_alpha = self.config.alpha_quality.is_some() && image.alpha_plane.is_some();
        let alpha_payload = if has_alpha {
            Some(build_alpha_av1_obu(image))
        } else {
            None
        };

        let mut out = Vec::with_capacity(4096 + av1_payload.len());

        // ── ftyp ──────────────────────────────────────────────────────────
        write_ftyp(&mut out);

        // ── meta ──────────────────────────────────────────────────────────
        // We need to know the absolute offset of mdat content ahead of time,
        // so we build meta into a temporary buffer first, then patch the
        // iloc offset once we know the final position.
        let meta_buf = build_meta(image, &self.config, &av1_payload, &alpha_payload)?;
        out.extend_from_slice(&meta_buf);

        // ── mdat ──────────────────────────────────────────────────────────
        let mdat_header_size = 8u32; // size(4) + 'mdat'(4)
        let mdat_size = mdat_header_size as usize
            + av1_payload.len()
            + alpha_payload.as_ref().map_or(0, |a| a.len());
        write_u32(&mut out, mdat_size as u32);
        out.extend_from_slice(b"mdat");
        out.extend_from_slice(&av1_payload);
        if let Some(ref ap) = alpha_payload {
            out.extend_from_slice(ap);
        }

        // Patch the iloc extents: we wrote placeholder offsets in build_meta,
        // now fix them up.
        patch_iloc_offsets(&mut out, &meta_buf, &av1_payload, alpha_payload.as_deref())?;

        Ok(out)
    }
}

// ─── Decoder ─────────────────────────────────────────────────────────────────

/// Probes AVIF container bytes and extracts raw AV1 payloads.
///
/// Full pixel decode is **not implemented** — see the module docs and
/// [`AvifDecoder::decode`].
#[derive(Debug, Default, Clone)]
pub struct AvifDecoder;

impl AvifDecoder {
    /// Create a new decoder.
    pub fn new() -> Self {
        Self
    }

    /// Decode a complete AVIF byte stream to pixels.
    ///
    /// Validates the ISOBMFF container, extracts the AV1 OBU payload(s),
    /// and decodes them through the crate's real, bit-exact AV1
    /// keyframe/intra pipeline ([`crate::av1::Av1Decoder`] — verified
    /// against dav1d and aomdec). If an alpha auxiliary item is present it
    /// is decoded too; an image whose alpha channel cannot be decoded is
    /// never silently returned as if it were fully opaque (see
    /// [`Self::extract_av1_payload`] to still recover the raw AV1
    /// bitstream(s) in that case).
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidBitstream` if the container is
    /// malformed. Returns the AV1 decoder's own honest errors otherwise —
    /// notably `CodecError::UnsupportedFeature` for surfaces the keyframe
    /// decoder does not (yet) implement: 10/12-bit depth, non-4:2:0 chroma,
    /// or monochrome AV1 (which is how real-world encoders typically encode
    /// the alpha auxiliary image, so files with alpha commonly hit this
    /// today even when their colour image decodes fine on its own). When
    /// the `av1` cargo feature is disabled, always returns
    /// `CodecError::UnsupportedFeature`.
    pub fn decode(data: &[u8]) -> Result<AvifImage, CodecError> {
        // Validate the container first so malformed input is still
        // reported as such (signature, meta/iloc structure, mdat extents).
        let payload = Self::extract_av1_payload(data)?;
        Self::decode_payload(payload)
    }

    #[cfg(feature = "av1")]
    fn decode_payload(payload: AvifPayload) -> Result<AvifImage, CodecError> {
        decode::decode_still_image(&payload)
    }

    #[cfg(not(feature = "av1"))]
    fn decode_payload(_payload: AvifPayload) -> Result<AvifImage, CodecError> {
        Err(CodecError::UnsupportedFeature(
            "AVIF pixel decode requires the `av1` cargo feature, which is disabled in this \
             build. Use AvifDecoder::extract_av1_payload for the raw AV1 OBU bitstream, or \
             AvifDecoder::probe for container metadata"
                .to_string(),
        ))
    }

    /// Parse the ISOBMFF container and return the raw AV1 OBU bitstream(s).
    ///
    /// The returned [`AvifPayload`] holds **encoded AV1 bitstream data**
    /// (honestly labelled — not decoded pixels), suitable for handoff to an
    /// AV1 decoder.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidBitstream` if the container is malformed
    /// or the `iloc` extents point outside the file.
    pub fn extract_av1_payload(data: &[u8]) -> Result<AvifPayload, CodecError> {
        let probe = Self::probe(data)?;
        let (color_offset, color_len, alpha_offset, alpha_len) =
            locate_mdat_items(data, probe.has_alpha)?;

        let color_obu = data
            .get(color_offset..color_offset + color_len)
            .ok_or_else(|| CodecError::InvalidBitstream("mdat color extent out of range".into()))?
            .to_vec();

        let alpha_obu = if probe.has_alpha {
            let slice = data
                .get(alpha_offset..alpha_offset + alpha_len)
                .ok_or_else(|| {
                    CodecError::InvalidBitstream("mdat alpha extent out of range".into())
                })?
                .to_vec();
            Some(slice)
        } else {
            None
        };

        Ok(AvifPayload {
            color_obu,
            alpha_obu,
        })
    }

    /// Probe a AVIF byte stream for basic metadata without full decoding.
    pub fn probe(data: &[u8]) -> Result<AvifProbeResult, CodecError> {
        check_avif_signature(data)?;
        parse_meta_for_probe(data)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Internal helpers – AV1 OBU builder
// ═════════════════════════════════════════════════════════════════════════════

/// Build a minimal AV1 bitstream containing a Sequence Header OBU.
///
/// The sequence header encodes width, height, bit depth, and colour info as
/// required by the AVIF specification (§4 "AV1 Image Items").
fn build_av1_obu(image: &AvifImage, config: &AvifConfig) -> Vec<u8> {
    let mut bits = BitWriter::new();

    // ── Sequence Header OBU ─────────────────────────────────────────────
    // obu_forbidden_bit(1=0) | obu_type(4=1) | obu_extension_flag(1=0)
    // | obu_has_size_field(1=1) | obu_reserved_1bit(1=0)
    let obu_header: u8 = (1 << 3) | (1 << 1); // type=1, has_size=1
    bits.write_byte(obu_header);

    // Build the payload separately so we can prefix it with its leb128 size.
    let mut seq = BitWriter::new();
    write_sequence_header_payload(&mut seq, image, config);
    let seq_bytes = seq.finish();

    // leb128 size of the payload
    let mut leb = Vec::new();
    write_leb128(&mut leb, seq_bytes.len() as u64);
    bits.extend_bytes(&leb);
    bits.extend_bytes(&seq_bytes);

    // ── Temporal Delimiter OBU (type = 2) ───────────────────────────────
    bits.write_byte((2 << 3) | (1 << 1)); // type=2, has_size=1
    bits.write_byte(0); // size = 0

    bits.finish()
}

/// Build a minimal alpha-plane AV1 OBU (same structure, monochrome).
fn build_alpha_av1_obu(image: &AvifImage) -> Vec<u8> {
    let alpha_config = AvifConfig {
        quality: 80,
        color_primaries: 1,
        transfer_characteristics: 1,
        matrix_coefficients: 0, // identity / monochrome
        full_range: true,
        ..AvifConfig::default()
    };
    // Treat alpha as a grayscale image.
    let mono = AvifImage {
        width: image.width,
        height: image.height,
        depth: image.depth,
        yuv_format: YuvFormat::Yuv444,
        y_plane: image.alpha_plane.clone().unwrap_or_default(),
        u_plane: Vec::new(),
        v_plane: Vec::new(),
        alpha_plane: None,
    };
    build_av1_obu(&mono, &alpha_config)
}

/// Write the Sequence Header OBU payload bits.
fn write_sequence_header_payload(bits: &mut BitWriter, image: &AvifImage, config: &AvifConfig) {
    // seq_profile: 0 = main (8/10-bit 4:2:0), 1 = high (4:4:4), 2 = pro
    let seq_profile: u8 = match image.yuv_format {
        YuvFormat::Yuv444 => 1,
        _ => 0,
    };
    bits.write_bits(seq_profile as u32, 3);

    // still_picture = 1
    bits.write_bits(1, 1);
    // reduced_still_picture_header = 1  (simplified header for still images)
    bits.write_bits(1, 1);

    // seq_level_idx[0]: level 5.1 = 13 (supports up to 4K)
    bits.write_bits(13, 5);

    // ── Colour config ───────────────────────────────────────────────────
    // high_bitdepth
    let high_bitdepth = image.depth >= 10;
    bits.write_bits(high_bitdepth as u32, 1);
    if seq_profile == 2 && high_bitdepth {
        // twelve_bit
        let twelve_bit = image.depth == 12;
        bits.write_bits(twelve_bit as u32, 1);
    }
    // mono_chrome
    bits.write_bits(0, 1);
    // color_description_present_flag = 1
    bits.write_bits(1, 1);
    // color_primaries (8 bits)
    bits.write_bits(config.color_primaries as u32, 8);
    // transfer_characteristics (8 bits)
    bits.write_bits(config.transfer_characteristics as u32, 8);
    // matrix_coefficients (8 bits)
    bits.write_bits(config.matrix_coefficients as u32, 8);
    // color_range
    bits.write_bits(config.full_range as u32, 1);

    // subsampling_x / subsampling_y based on yuv_format
    let (sub_x, sub_y): (u32, u32) = match image.yuv_format {
        YuvFormat::Yuv420 => (1, 1),
        YuvFormat::Yuv422 => (1, 0),
        YuvFormat::Yuv444 => (0, 0),
    };
    if seq_profile != 1 {
        if config.color_primaries == 1
            && config.transfer_characteristics == 1
            && config.matrix_coefficients == 1
        {
            // separate_uv_delta_q
            bits.write_bits(0, 1);
        } else {
            bits.write_bits(sub_x, 1);
            if sub_x == 1 {
                bits.write_bits(sub_y, 1);
            }
            if sub_x == 1 && sub_y == 1 {
                // chroma_sample_position
                bits.write_bits(0, 2); // CSP_UNKNOWN
            }
        }
    }
    // separate_uv_delta_q
    bits.write_bits(0, 1);

    // ── Frame size in the reduced header ───────────────────────────────
    // frame_width_bits_minus_1 / frame_height_bits_minus_1 each need
    // ceil(log2(width)) bits.
    let w_bits = bits_needed(image.width);
    let h_bits = bits_needed(image.height);
    bits.write_bits((w_bits - 1) as u32, 4); // frame_width_bits_minus_1
    bits.write_bits((h_bits - 1) as u32, 4); // frame_height_bits_minus_1
    bits.write_bits((image.width - 1) as u32, w_bits as u32);
    bits.write_bits((image.height - 1) as u32, h_bits as u32);

    // film_grain_params_present = 0
    bits.write_bits(0, 1);
}

/// Number of bits required to represent values up to `n` (i.e. ⌈log2(n+1)⌉).
fn bits_needed(n: u32) -> u8 {
    if n == 0 {
        return 1;
    }
    let mut bits = 0u8;
    let mut v = n;
    while v > 0 {
        bits += 1;
        v >>= 1;
    }
    bits
}

/// Write `value` as a LEB128-encoded unsigned integer into `buf`.
fn write_leb128(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Internal helpers – ISOBMFF box builder
// ═════════════════════════════════════════════════════════════════════════════

/// Write the `ftyp` box into `out`.
fn write_ftyp(out: &mut Vec<u8>) {
    // compatible brands: 'avif', 'mif1', 'miaf'
    let compat: &[&[u8; 4]] = &[b"avif", b"mif1", b"miaf"];
    let size = 4 + 4 + 4 + 4 + 4 * compat.len(); // size + 'ftyp' + major + minor_version + compat[]
    write_u32(out, size as u32);
    out.extend_from_slice(b"ftyp");
    out.extend_from_slice(b"avif"); // major_brand
    write_u32(out, 0); // minor_version
    for brand in compat {
        out.extend_from_slice(*brand);
    }
}

/// Build the complete `meta` box (FullBox, version 0) and return it as bytes.
///
/// The `iloc` extents use placeholder offsets (0) which are patched later by
/// [`patch_iloc_offsets`].
fn build_meta(
    image: &AvifImage,
    config: &AvifConfig,
    av1_payload: &[u8],
    alpha_payload: &Option<Vec<u8>>,
) -> Result<Vec<u8>, CodecError> {
    let has_alpha = alpha_payload.is_some();

    let mut body = Vec::<u8>::new();

    // hdlr box
    body.extend_from_slice(&build_hdlr());

    // pitm box (primary item id = 1)
    body.extend_from_slice(&build_pitm(1));

    // iloc box
    body.extend_from_slice(&build_iloc(
        has_alpha,
        av1_payload.len(),
        alpha_payload.as_ref().map_or(0, |a| a.len()),
    ));

    // iinf box
    body.extend_from_slice(&build_iinf(has_alpha));

    // iprp box
    body.extend_from_slice(&build_iprp(image, config, has_alpha)?);

    // Wrap in meta FullBox (version=0, flags=0)
    let meta_size = 4 + 4 + 4 + body.len(); // size + 'meta' + version/flags(4)
    let mut meta = Vec::with_capacity(meta_size);
    write_u32(&mut meta, meta_size as u32);
    meta.extend_from_slice(b"meta");
    write_u32(&mut meta, 0u32); // version(1) + flags(3)
    meta.extend_from_slice(&body);
    Ok(meta)
}

// ── hdlr ──────────────────────────────────────────────────────────────────

fn build_hdlr() -> Vec<u8> {
    // FullBox(version=0, flags=0) + pre_defined(4) + handler_type(4) +
    // reserved(12) + name(1 = '\0')
    // Box layout: size(4) + 'hdlr'(4) + fullbox(4) + pre_defined(4) +
    //             handler_type(4) + reserved(12) + name(1) = 33 bytes
    let size = 4 + 4 + 4 + 4 + 4 + 12 + 1; // 33
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"hdlr");
    write_u32(&mut b, 0); // version + flags
    write_u32(&mut b, 0); // pre_defined
    b.extend_from_slice(b"pict"); // handler_type
    b.extend_from_slice(&[0u8; 12]); // reserved
    b.push(0); // name (null-terminated empty string)
    b
}

// ── pitm ──────────────────────────────────────────────────────────────────

fn build_pitm(item_id: u16) -> Vec<u8> {
    let size = 4 + 4 + 4 + 2; // header(8) + fullbox(4) + item_ID(2)
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"pitm");
    write_u32(&mut b, 0); // version=0, flags=0
    write_u16(&mut b, item_id);
    b
}

// ── iloc ──────────────────────────────────────────────────────────────────

/// Build the `iloc` box with **placeholder** absolute offsets (0).
///
/// The real offsets are patched by [`patch_iloc_offsets`] after the
/// complete file layout is known.
fn build_iloc(has_alpha: bool, color_len: usize, alpha_len: usize) -> Vec<u8> {
    // iloc version=1 supports 32-bit offset_size=4, length_size=4,
    // base_offset_size=0, index_size=0.
    // Format: offset_size(4bits) | length_size(4bits) | base_offset_size(4bits) | index_size/reserved(4bits)
    // Then item_count(u16), then per-item: item_ID(u16), reserved(u16), data_ref_index(u16),
    // extent_count(u16), [extent_index(if index_size>0),] extent_offset(offset_size), extent_length(length_size)

    let item_count: u16 = if has_alpha { 2 } else { 1 };
    // Per-item fields for version=1:
    //   item_ID(2) + construction_method(2, version>=1) + data_ref_idx(2) + extent_count(2)
    //   + extent_offset(4) + extent_length(4) = 16 bytes each
    let item_entry_size = 2 + 2 + 2 + 2 + 4 + 4;
    let payload_size = 1 + 1 + 2 + item_count as usize * item_entry_size;
    // FullBox: version(1) + flags(3) = 4 bytes
    let size = 8 + 4 + payload_size;
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"iloc");
    write_u32(&mut b, 1 << 24); // version=1, flags=0

    // offset_size=4(bits), length_size=4(bits) packed into one byte
    b.push(0x44); // 0100_0100 -> offset_size=4, length_size=4
                  // base_offset_size=0, index_size=0 packed into one byte
    b.push(0x00);
    // item_count
    write_u16(&mut b, item_count);

    // Item 1: colour image (item_ID=1)
    write_u16(&mut b, 1); // item_ID
    write_u16(&mut b, 0); // construction_method = 0 (file offset)
    write_u16(&mut b, 0); // data_reference_index
    write_u16(&mut b, 1); // extent_count
    write_u32(&mut b, 0); // extent_offset PLACEHOLDER
    write_u32(&mut b, color_len as u32); // extent_length

    if has_alpha {
        // Item 2: alpha image (item_ID=2)
        write_u16(&mut b, 2); // item_ID
        write_u16(&mut b, 0); // construction_method
        write_u16(&mut b, 0); // data_reference_index
        write_u16(&mut b, 1); // extent_count
        write_u32(&mut b, 0); // extent_offset PLACEHOLDER
        write_u32(&mut b, alpha_len as u32); // extent_length
    }

    b
}

// ── iinf ──────────────────────────────────────────────────────────────────

fn build_iinf(has_alpha: bool) -> Vec<u8> {
    let item_count: u16 = if has_alpha { 2 } else { 1 };
    let entry1 = build_infe(1, b"av01", b"Color Image\0");
    let entry2 = if has_alpha {
        Some(build_infe(2, b"av01", b"Alpha Image\0"))
    } else {
        None
    };

    let entries_size = entry1.len() + entry2.as_ref().map_or(0, |e| e.len());
    let size = 8 + 4 + 2 + entries_size; // box(8) + fullbox(4) + entry_count(2) + entries
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"iinf");
    write_u32(&mut b, 0); // version=0, flags=0
    write_u16(&mut b, item_count);
    b.extend_from_slice(&entry1);
    if let Some(e2) = entry2 {
        b.extend_from_slice(&e2);
    }
    b
}

/// Build an `infe` (ItemInfoEntry) FullBox for version=2.
fn build_infe(item_id: u16, item_type: &[u8; 4], item_name: &[u8]) -> Vec<u8> {
    // version=2: item_ID(2) + item_protection_index(2) + item_type(4) + item_name(...)
    let payload = 2 + 2 + 4 + item_name.len();
    let size = 8 + 4 + payload; // box(8) + fullbox flags/version(4) + payload
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"infe");
    write_u32(&mut b, 2 << 24); // version=2, flags=0
    write_u16(&mut b, item_id);
    write_u16(&mut b, 0); // item_protection_index
    b.extend_from_slice(item_type);
    b.extend_from_slice(item_name);
    b
}

// ── iprp ──────────────────────────────────────────────────────────────────

fn build_iprp(
    image: &AvifImage,
    config: &AvifConfig,
    has_alpha: bool,
) -> Result<Vec<u8>, CodecError> {
    let ispe = build_ispe(image.width, image.height);
    let colr = build_colr(config);
    let av1c = build_av1c(image, config);
    let pixi = build_pixi(image.depth);

    // ipco – property container
    let ipco_payload_len = ispe.len() + colr.len() + av1c.len() + pixi.len();
    let ipco_size = 8 + ipco_payload_len;
    let mut ipco = Vec::with_capacity(ipco_size);
    write_u32(&mut ipco, ipco_size as u32);
    ipco.extend_from_slice(b"ipco");
    ipco.extend_from_slice(&ispe);
    ipco.extend_from_slice(&colr);
    ipco.extend_from_slice(&av1c);
    ipco.extend_from_slice(&pixi);

    // ipma – property association
    // Properties are 1-indexed within ipco.
    // prop 1 = ispe, prop 2 = colr, prop 3 = av1C, prop 4 = pixi
    let ipma = build_ipma(has_alpha);

    let iprp_size = 8 + ipco.len() + ipma.len();
    let mut b = Vec::with_capacity(iprp_size);
    write_u32(&mut b, iprp_size as u32);
    b.extend_from_slice(b"iprp");
    b.extend_from_slice(&ipco);
    b.extend_from_slice(&ipma);
    Ok(b)
}

/// `ispe` – image spatial extents.
fn build_ispe(width: u32, height: u32) -> Vec<u8> {
    let size = 8 + 4 + 4 + 4; // box(8) + fullbox(4) + w(4) + h(4)
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"ispe");
    write_u32(&mut b, 0); // version=0, flags=0
    write_u32(&mut b, width);
    write_u32(&mut b, height);
    b
}

/// `colr` – colour information (nclx type).
fn build_colr(config: &AvifConfig) -> Vec<u8> {
    // nclx: colour_type(4) + colour_primaries(2) + transfer_characteristics(2)
    //       + matrix_coefficients(2) + full_range_flag(1 bit) + reserved(7 bits)
    let payload_size = 4 + 2 + 2 + 2 + 1;
    let size = 8 + payload_size;
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"colr");
    b.extend_from_slice(b"nclx"); // colour_type
    write_u16(&mut b, config.color_primaries as u16);
    write_u16(&mut b, config.transfer_characteristics as u16);
    write_u16(&mut b, config.matrix_coefficients as u16);
    let full_range_byte: u8 = if config.full_range { 0x80 } else { 0x00 };
    b.push(full_range_byte);
    b
}

/// `av1C` – AV1 Codec Configuration Record.
///
/// Spec: <https://aomediacodec.github.io/av1-isobmff/#av1codecconfigurationbox>
fn build_av1c(image: &AvifImage, config: &AvifConfig) -> Vec<u8> {
    // marker(1=1) | version(7=1) = 0x81
    // seq_profile(3) | seq_level_idx_0(5)
    // seq_tier_0(1) | high_bitdepth(1) | twelve_bit(1) | monochrome(1)
    //   | chroma_subsampling_x(1) | chroma_subsampling_y(1) | chroma_sample_position(2)
    // reserved(3=0) | initial_presentation_delay_present(1=0) | reserved(4=0)

    let seq_profile: u8 = match image.yuv_format {
        YuvFormat::Yuv444 => 1,
        _ => 0,
    };
    let seq_level_idx_0: u8 = 13; // level 5.1

    let byte0: u8 = 0x81; // marker + version
    let byte1: u8 = (seq_profile << 5) | seq_level_idx_0;
    let high_bitdepth = image.depth >= 10;
    let twelve_bit = image.depth == 12;
    let (sub_x, sub_y): (u8, u8) = match image.yuv_format {
        YuvFormat::Yuv420 => (1, 1),
        YuvFormat::Yuv422 => (1, 0),
        YuvFormat::Yuv444 => (0, 0),
    };
    let byte2: u8 = (high_bitdepth as u8) << 6
        | (twelve_bit as u8) << 5
        | 0 << 4 // monochrome = 0
        | sub_x << 3
        | sub_y << 2
        | 0; // chroma_sample_position
    let _ = config; // seq_tier_0 etc. – not exposed in config, default 0
    let byte3: u8 = 0x00; // initial_presentation_delay_present = 0

    let size = 8 + 4; // box(8) + 4 payload bytes
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"av1C");
    b.push(byte0);
    b.push(byte1);
    b.push(byte2);
    b.push(byte3);
    b
}

/// `pixi` – pixel information (bit depth per channel).
fn build_pixi(depth: u8) -> Vec<u8> {
    // FullBox + num_channels(1=3 for YUV) + depth_in_bits × 3
    let num_channels: u8 = 3;
    let size = 8 + 4 + 1 + num_channels as usize;
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"pixi");
    write_u32(&mut b, 0); // version=0, flags=0
    b.push(num_channels);
    for _ in 0..num_channels {
        b.push(depth);
    }
    b
}

/// `ipma` – item property association.
///
/// Associates properties 1–4 (ispe, colr, av1C, pixi) with item 1 (and item 2
/// if alpha is present).
fn build_ipma(has_alpha: bool) -> Vec<u8> {
    // version=0, flags=0
    // entry_count(4 bytes)
    // Per entry: item_ID(2) + association_count(1) + [essential(1) | property_index(7)] × n
    let item_count: u32 = if has_alpha { 2 } else { 1 };
    // Each item references 4 properties (ispe, colr, av1C, pixi).
    // essential bit = 1 for av1C (index 3), others = 0.
    let assoc_per_item: &[(u8, u8)] = &[
        (0, 1), // ispe – not essential, prop index 1
        (0, 2), // colr – not essential, prop index 2
        (1, 3), // av1C – essential, prop index 3
        (0, 4), // pixi – not essential, prop index 4
    ];
    let per_item_size = 2 + 1 + assoc_per_item.len(); // ID(2) + count(1) + n×1
    let payload_size = 4 + item_count as usize * per_item_size;
    let size = 8 + 4 + payload_size;
    let mut b = Vec::with_capacity(size);
    write_u32(&mut b, size as u32);
    b.extend_from_slice(b"ipma");
    write_u32(&mut b, 0); // version=0, flags=0
    write_u32(&mut b, item_count);

    for item_id in 1..=item_count as u16 {
        write_u16(&mut b, item_id);
        b.push(assoc_per_item.len() as u8);
        for &(essential, prop_idx) in assoc_per_item {
            b.push((essential << 7) | (prop_idx & 0x7F));
        }
    }
    b
}

// ── Patch iloc offsets ─────────────────────────────────────────────────────

/// After the full layout is known, find the placeholder offsets in the already-
/// appended `meta` bytes and overwrite them with the correct absolute file
/// offsets.
///
/// Layout: `[ftyp][meta][mdat_header(8)][av1_payload][alpha_payload?]`
///
/// `out` contains ftyp + meta + mdat (already written).
/// `meta_buf` is the original meta bytes (before appending to out) — used to
/// compute the size of ftyp.
fn patch_iloc_offsets(
    out: &mut Vec<u8>,
    meta_buf: &[u8],
    av1_payload: &[u8],
    alpha_payload: Option<&[u8]>,
) -> Result<(), CodecError> {
    // ftyp size is stored in the first 4 bytes of `out`.
    let ftyp_size = u32::from_be_bytes(
        out.get(0..4)
            .ok_or_else(|| CodecError::Internal("output too short for ftyp size".into()))?
            .try_into()
            .map_err(|_| CodecError::Internal("slice conversion error".into()))?,
    ) as usize;

    let meta_size = meta_buf.len();
    // mdat content starts after ftyp + meta + mdat_header(8)
    let mdat_data_start = ftyp_size + meta_size + 8;

    let color_offset = mdat_data_start as u32;
    let alpha_offset = (mdat_data_start + av1_payload.len()) as u32;

    // Find the iloc box within `out` (it's inside meta).
    // meta starts at ftyp_size; inside meta: 8(box header) + 4(fullbox) = 12 bytes.
    // Then hdlr, pitm, then iloc.
    let meta_start = ftyp_size;
    let meta_body_start = meta_start + 12; // skip meta box header(8) + fullbox(4)

    // Walk meta body to find iloc.
    let iloc_pos = find_box_in(out, meta_body_start, meta_start + meta_size, b"iloc")
        .ok_or_else(|| CodecError::Internal("iloc box not found in output".into()))?;

    // iloc layout (version=1):
    // box_header(8) + fullbox(4) + offset_size/length_size(1) + base_offset_size/index_size(1)
    // + item_count(2) = 16 bytes before first item entry
    let item0_start = iloc_pos + 16;
    // Per item (version=1): item_ID(2) + construction_method(2) + data_ref_idx(2)
    //   + extent_count(2) + extent_offset(4) + extent_length(4) = 16 bytes
    let color_extent_offset_pos = item0_start + 6; // skip ID(2)+method(2)+ref(2)+count(2) = 8, then offset starts
                                                   // Actually: ID(2) + method(2) + ref(2) + extent_count(2) = 8 bytes
    let color_extent_offset_pos = item0_start + 8;

    patch_u32(out, color_extent_offset_pos, color_offset)?;

    if alpha_payload.is_some() {
        let item1_start = item0_start + 16;
        let alpha_extent_offset_pos = item1_start + 8;
        patch_u32(out, alpha_extent_offset_pos, alpha_offset)?;
    }

    Ok(())
}

/// Overwrite 4 bytes at `pos` in `buf` with big-endian `value`.
fn patch_u32(buf: &mut Vec<u8>, pos: usize, value: u32) -> Result<(), CodecError> {
    if pos + 4 > buf.len() {
        return Err(CodecError::Internal(format!(
            "patch_u32: pos={pos} out of range (buf.len={})",
            buf.len()
        )));
    }
    let bytes = value.to_be_bytes();
    buf[pos] = bytes[0];
    buf[pos + 1] = bytes[1];
    buf[pos + 2] = bytes[2];
    buf[pos + 3] = bytes[3];
    Ok(())
}

// Container parsing (signature check, `meta`/`iloc` extraction) lives in
// `container.rs`; see the `mod container;` + `use container::{...};`
// declarations near the top of this file.

// ═════════════════════════════════════════════════════════════════════════════
// Internal helpers – validation
// ═════════════════════════════════════════════════════════════════════════════

fn validate_image(image: &AvifImage) -> Result<(), CodecError> {
    if image.width == 0 || image.height == 0 {
        return Err(CodecError::InvalidParameter(
            "image dimensions must be non-zero".into(),
        ));
    }
    if ![8u8, 10, 12].contains(&image.depth) {
        return Err(CodecError::InvalidParameter(format!(
            "unsupported bit depth {}; must be 8, 10, or 12",
            image.depth
        )));
    }
    let luma_samples = image.width as usize * image.height as usize;
    let bytes_per_sample: usize = if image.depth > 8 { 2 } else { 1 };
    let min_y = luma_samples * bytes_per_sample;
    if image.y_plane.len() < min_y {
        return Err(CodecError::InvalidParameter(format!(
            "y_plane too small: need {min_y}, have {}",
            image.y_plane.len()
        )));
    }
    Ok(())
}

// ═════════════════════════════════════════════════════════════════════════════
// Internal helpers – I/O primitives
// ═════════════════════════════════════════════════════════════════════════════

fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

// ═════════════════════════════════════════════════════════════════════════════
// Internal helpers – bit writer
// ═════════════════════════════════════════════════════════════════════════════

/// MSB-first bit writer used for building AV1 OBU payloads.
struct BitWriter {
    buf: Vec<u8>,
    current: u8,
    bits_in_current: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            current: 0,
            bits_in_current: 0,
        }
    }

    /// Write the least-significant `n` bits of `value`, MSB first.
    fn write_bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            let bit = ((value >> i) & 1) as u8;
            self.current = (self.current << 1) | bit;
            self.bits_in_current += 1;
            if self.bits_in_current == 8 {
                self.buf.push(self.current);
                self.current = 0;
                self.bits_in_current = 0;
            }
        }
    }

    fn write_byte(&mut self, byte: u8) {
        self.write_bits(byte as u32, 8);
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_byte(b);
        }
    }

    /// Flush any remaining bits (zero-padded to byte boundary) and return buf.
    fn finish(mut self) -> Vec<u8> {
        if self.bits_in_current > 0 {
            self.current <<= 8 - self.bits_in_current;
            self.buf.push(self.current);
        }
        self.buf
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Real-fixture decode tests ───────────────────────────────────────
    //
    // `testdata/*.avif` are real ffmpeg + libaom-av1 output (NOT authored
    // by `AvifEncoder`, which only emits a structurally valid container
    // with a minimal placeholder AV1 payload — see `testdata/README.md`
    // for exact regeneration commands and why each fixture behaves as it
    // does).

    /// The one fixture the real AV1-backed decode path is expected to
    /// fully decode: 8-bit 4:2:0, no alpha. Verifies bit-exact agreement
    /// with ffmpeg's own (libdav1d) decode of the same file.
    #[cfg(feature = "av1")]
    #[test]
    fn decode_real_avif_fixture_bit_exact_vs_ffmpeg() {
        let bytes: &[u8] = include_bytes!("testdata/color_only.avif");
        let reference: &[u8] = include_bytes!("testdata/color_only_ref.yuv");

        let image = AvifDecoder::decode(bytes).expect("real AVIF fixture must decode");
        assert_eq!(image.width, 32);
        assert_eq!(image.height, 32);
        assert_eq!(image.depth, 8);
        assert_eq!(image.yuv_format, YuvFormat::Yuv420);
        assert!(image.alpha_plane.is_none(), "fixture has no alpha item");

        let (ry, ru, rv) = (
            &reference[..32 * 32],
            &reference[32 * 32..32 * 32 + 16 * 16],
            &reference[32 * 32 + 16 * 16..],
        );
        assert_eq!(
            image.y_plane.as_slice(),
            ry,
            "Y plane must match ffmpeg/dav1d reference bit-exactly"
        );
        assert_eq!(
            image.u_plane.as_slice(),
            ru,
            "U plane must match ffmpeg/dav1d reference bit-exactly"
        );
        assert_eq!(
            image.v_plane.as_slice(),
            rv,
            "V plane must match ffmpeg/dav1d reference bit-exactly"
        );
    }

    /// Real-world AVIF alpha auxiliary images are routinely encoded as
    /// monochrome AV1 (libaom's alpha output has no chroma planes), which
    /// this crate's keyframe decoder does not implement. `decode()` must
    /// fail honestly — naming the alpha image specifically — rather than
    /// silently returning the colour image as if it had no alpha.
    #[cfg(feature = "av1")]
    #[test]
    fn decode_real_avif_fixture_with_alpha_honest_err() {
        let bytes: &[u8] = include_bytes!("testdata/with_alpha.avif");

        // The container itself is well-formed and reports alpha.
        let probe = AvifDecoder::probe(bytes).expect("probe must succeed");
        assert!(probe.has_alpha, "fixture must be probed as having alpha");

        let err = AvifDecoder::decode(bytes)
            .expect_err("alpha image is monochrome AV1, which is not implemented");
        match err {
            CodecError::UnsupportedFeature(msg) => {
                assert!(
                    msg.contains("alpha"),
                    "error should name the alpha image, got: {msg}"
                );
            }
            other => panic!("expected honest UnsupportedFeature, got {other:?}"),
        }

        // The raw AV1 bitstreams (both items) remain available honestly.
        let payload = AvifDecoder::extract_av1_payload(bytes).expect("payload extraction");
        assert!(!payload.color_obu.is_empty());
        assert!(payload.alpha_obu.is_some_and(|a| !a.is_empty()));
    }

    /// 10-bit AV1 is unsupported by the keyframe decoder's reconstruction
    /// stage (8-bit only). This proves the *colour*-path propagates the
    /// AV1 decoder's honest error too (not just the alpha path), while the
    /// container-level probe (which doesn't touch AV1 pixel decode) still
    /// reports the real 10-bit depth.
    #[cfg(feature = "av1")]
    #[test]
    fn decode_real_avif_10bit_honest_err() {
        let bytes: &[u8] = include_bytes!("testdata/color_10bit.avif");

        let probe = AvifDecoder::probe(bytes).expect("probe must succeed");
        assert_eq!(probe.bit_depth, 10, "container must probe as 10-bit");

        let err = AvifDecoder::decode(bytes).expect_err("10-bit AV1 decode is not implemented");
        assert!(
            matches!(err, CodecError::UnsupportedFeature(_)),
            "expected honest UnsupportedFeature, got {err:?}"
        );
    }

    fn make_test_image(width: u32, height: u32, depth: u8, fmt: YuvFormat) -> AvifImage {
        let luma = width as usize * height as usize * if depth > 8 { 2 } else { 1 };
        let chroma = match fmt {
            YuvFormat::Yuv420 => (width as usize / 2) * (height as usize / 2),
            YuvFormat::Yuv422 => (width as usize / 2) * height as usize,
            YuvFormat::Yuv444 => width as usize * height as usize,
        } * if depth > 8 { 2 } else { 1 };
        AvifImage {
            width,
            height,
            depth,
            yuv_format: fmt,
            y_plane: vec![128u8; luma],
            u_plane: vec![128u8; chroma],
            v_plane: vec![128u8; chroma],
            alpha_plane: None,
        }
    }

    #[test]
    fn test_ftyp_box() {
        let mut out = Vec::new();
        write_ftyp(&mut out);
        assert!(out.len() >= 20, "ftyp must be at least 20 bytes");
        assert_eq!(&out[4..8], b"ftyp", "box type must be 'ftyp'");
        assert_eq!(&out[8..12], b"avif", "major brand must be 'avif'");
        // Compatible brands must include 'avif'
        let brands_region = &out[8..];
        let has_avif = brands_region
            .chunks(4)
            .any(|c| c.len() == 4 && c == b"avif");
        assert!(has_avif, "compatible brands must contain 'avif'");
    }

    #[test]
    fn test_encode_produces_valid_ftyp() {
        let image = make_test_image(64, 64, 8, YuvFormat::Yuv420);
        let config = AvifConfig::default();
        let encoder = AvifEncoder::new(config);
        let bytes = encoder.encode(&image).expect("encode failed");
        assert!(bytes.len() > 32, "encoded output too short");
        // First box must be ftyp
        assert_eq!(&bytes[4..8], b"ftyp");
        // Major brand must be 'avif'
        assert_eq!(&bytes[8..12], b"avif");
    }

    #[test]
    fn test_encode_contains_meta_and_mdat() {
        let image = make_test_image(128, 96, 8, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode failed");
        assert!(
            find_top_level_box(&bytes, b"meta").is_some(),
            "meta box missing"
        );
        assert!(
            find_top_level_box(&bytes, b"mdat").is_some(),
            "mdat box missing"
        );
    }

    #[test]
    fn test_probe_roundtrip_dimensions() {
        let image = make_test_image(320, 240, 8, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode failed");
        let probe = AvifDecoder::probe(&bytes).expect("probe failed");
        assert_eq!(probe.width, 320, "probed width mismatch");
        assert_eq!(probe.height, 240, "probed height mismatch");
        assert_eq!(probe.bit_depth, 8, "probed bit_depth mismatch");
        assert!(!probe.has_alpha, "should not have alpha");
    }

    #[test]
    fn test_probe_10bit() {
        let image = make_test_image(160, 120, 10, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode 10-bit failed");
        let probe = AvifDecoder::probe(&bytes).expect("probe 10-bit failed");
        assert_eq!(probe.bit_depth, 10);
    }

    #[test]
    fn test_probe_12bit() {
        let image = make_test_image(64, 64, 12, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode 12-bit failed");
        let probe = AvifDecoder::probe(&bytes).expect("probe 12-bit failed");
        assert_eq!(probe.bit_depth, 12);
    }

    /// `AvifEncoder::encode()` writes a structurally valid container but
    /// only a placeholder AV1 payload (sequence header + a zero-size
    /// temporal delimiter — no coded frame at all; see the module docs).
    /// `decode()` must not fabricate pixels from that: with the `av1`
    /// feature enabled it reaches the real decoder, which honestly reports
    /// that the payload produced no frame.
    #[cfg(feature = "av1")]
    #[test]
    fn test_decode_of_encoder_placeholder_errors_honestly() {
        let image = make_test_image(64, 48, 8, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode failed");

        let result = AvifDecoder::decode(&bytes);
        match result {
            Err(CodecError::InvalidBitstream(msg)) => {
                assert!(
                    msg.contains("no frame"),
                    "error must state the limitation clearly, got: {msg}"
                );
            }
            other => panic!("expected honest InvalidBitstream error, got {other:?}"),
        }
    }

    /// Same placeholder-decode scenario as
    /// [`test_decode_of_encoder_placeholder_errors_honestly`], but built
    /// without the `av1` cargo feature: `decode()` must still fail
    /// honestly, naming the disabled feature rather than silently
    /// succeeding or panicking.
    #[cfg(not(feature = "av1"))]
    #[test]
    fn test_decode_without_av1_feature_errors_honestly() {
        let image = make_test_image(64, 48, 8, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode failed");

        let result = AvifDecoder::decode(&bytes);
        match result {
            Err(CodecError::UnsupportedFeature(msg)) => {
                assert!(
                    msg.contains("av1"),
                    "error must name the missing av1 feature, got: {msg}"
                );
            }
            other => panic!("expected honest UnsupportedFeature error, got {other:?}"),
        }
    }

    #[test]
    fn test_extract_av1_payload_color_roundtrip() {
        let image = make_test_image(64, 48, 8, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("encode failed");

        // The honestly-labelled extraction API returns the raw AV1 OBU.
        let payload = AvifDecoder::extract_av1_payload(&bytes).expect("payload extraction failed");
        assert!(
            !payload.color_obu.is_empty(),
            "extracted colour AV1 OBU should not be empty"
        );
        assert!(payload.alpha_obu.is_none(), "no alpha item was encoded");

        // Metadata is still available via probe().
        let probe = AvifDecoder::probe(&bytes).expect("probe failed");
        assert_eq!(probe.width, 64);
        assert_eq!(probe.height, 48);
    }

    #[test]
    fn test_encode_with_alpha() {
        let mut image = make_test_image(64, 64, 8, YuvFormat::Yuv420);
        image.alpha_plane = Some(vec![255u8; 64 * 64]);
        let config = AvifConfig {
            alpha_quality: Some(80),
            ..AvifConfig::default()
        };
        let encoder = AvifEncoder::new(config);
        let bytes = encoder.encode(&image).expect("encode with alpha failed");
        let probe = AvifDecoder::probe(&bytes).expect("probe with alpha failed");
        assert!(probe.has_alpha, "probe should detect alpha");
    }

    #[test]
    fn test_extract_av1_payload_with_alpha() {
        let mut image = make_test_image(64, 64, 8, YuvFormat::Yuv420);
        image.alpha_plane = Some(vec![200u8; 64 * 64]);
        let config = AvifConfig {
            alpha_quality: Some(90),
            ..AvifConfig::default()
        };
        let encoder = AvifEncoder::new(config);
        let bytes = encoder.encode(&image).expect("encode failed");

        // decode() must not fabricate pixels: AvifEncoder's own output has
        // no real coded frame in either item (placeholder payload — see
        // the module docs), so decode() fails honestly regardless of
        // whether the `av1` feature is enabled (real decoder: "no frame
        // produced"; feature disabled: "av1 feature is disabled").
        assert!(
            AvifDecoder::decode(&bytes).is_err(),
            "decode of a placeholder-only AVIF must be an honest error"
        );

        // … while the raw alpha AV1 OBU stays available via extraction.
        let payload = AvifDecoder::extract_av1_payload(&bytes).expect("payload extraction failed");
        let alpha_obu = payload.alpha_obu.expect("alpha OBU should be present");
        assert!(!alpha_obu.is_empty());
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let garbage = b"not an avif file at all".to_vec();
        assert!(
            AvifDecoder::probe(&garbage).is_err(),
            "garbage input must be rejected"
        );
    }

    #[test]
    fn test_zero_dimension_rejected() {
        let image = AvifImage {
            width: 0,
            height: 100,
            depth: 8,
            yuv_format: YuvFormat::Yuv420,
            y_plane: vec![0u8; 100],
            u_plane: vec![],
            v_plane: vec![],
            alpha_plane: None,
        };
        let encoder = AvifEncoder::new(AvifConfig::default());
        assert!(encoder.encode(&image).is_err());
    }

    #[test]
    fn test_invalid_bit_depth_rejected() {
        let image = AvifImage {
            width: 8,
            height: 8,
            depth: 9, // invalid
            yuv_format: YuvFormat::Yuv420,
            y_plane: vec![0u8; 64],
            u_plane: vec![0u8; 16],
            v_plane: vec![0u8; 16],
            alpha_plane: None,
        };
        let encoder = AvifEncoder::new(AvifConfig::default());
        assert!(encoder.encode(&image).is_err());
    }

    #[test]
    fn test_colr_box_written() {
        let image = make_test_image(32, 32, 8, YuvFormat::Yuv420);
        let config = AvifConfig {
            color_primaries: 9,
            transfer_characteristics: 16,
            matrix_coefficients: 9,
            ..AvifConfig::default()
        };
        let encoder = AvifEncoder::new(config);
        let bytes = encoder.encode(&image).expect("encode failed");
        let probe = AvifDecoder::probe(&bytes).expect("probe failed");
        assert_eq!(probe.color_primaries, 9);
        assert_eq!(probe.transfer_characteristics, 16);
    }

    #[test]
    fn test_yuv444_encode() {
        let image = make_test_image(64, 64, 8, YuvFormat::Yuv444);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("yuv444 encode failed");
        let probe = AvifDecoder::probe(&bytes).expect("yuv444 probe failed");
        assert_eq!(probe.width, 64);
        assert_eq!(probe.height, 64);
    }

    #[test]
    fn test_leb128_encoding() {
        let mut buf = Vec::new();
        write_leb128(&mut buf, 0);
        assert_eq!(buf, &[0x00]);

        buf.clear();
        write_leb128(&mut buf, 127);
        assert_eq!(buf, &[0x7F]);

        buf.clear();
        write_leb128(&mut buf, 128);
        assert_eq!(buf, &[0x80, 0x01]);

        buf.clear();
        write_leb128(&mut buf, 300);
        assert_eq!(buf, &[0xAC, 0x02]);
    }

    #[test]
    fn test_bit_writer() {
        let mut bw = BitWriter::new();
        bw.write_bits(0b10110011, 8);
        let out = bw.finish();
        assert_eq!(out, &[0b10110011]);

        let mut bw = BitWriter::new();
        bw.write_bits(1, 1);
        bw.write_bits(0, 1);
        bw.write_bits(1, 1);
        bw.write_bits(0, 4);
        bw.write_bits(1, 1);
        let out = bw.finish();
        assert_eq!(out, &[0b10100001]);
    }

    #[test]
    fn test_av1c_box_structure() {
        let image = make_test_image(64, 64, 8, YuvFormat::Yuv420);
        let config = AvifConfig::default();
        let av1c = build_av1c(&image, &config);
        assert_eq!(av1c.len(), 12, "av1C box must be 12 bytes");
        assert_eq!(&av1c[4..8], b"av1C");
        assert_eq!(av1c[8], 0x81, "marker+version byte must be 0x81");
    }

    #[test]
    fn test_ispe_box_structure() {
        let ispe = build_ispe(1920, 1080);
        assert_eq!(ispe.len(), 20);
        assert_eq!(&ispe[4..8], b"ispe");
        let w = u32::from_be_bytes(ispe[12..16].try_into().expect("4-byte slice for width"));
        let h = u32::from_be_bytes(ispe[16..20].try_into().expect("4-byte slice for height"));
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn test_large_image_encode() {
        let image = make_test_image(3840, 2160, 8, YuvFormat::Yuv420);
        let encoder = AvifEncoder::new(AvifConfig::default());
        let bytes = encoder.encode(&image).expect("4K encode failed");
        let probe = AvifDecoder::probe(&bytes).expect("4K probe failed");
        assert_eq!(probe.width, 3840);
        assert_eq!(probe.height, 2160);
    }

    #[test]
    fn truncated_avif_never_panics_probe_or_decode() {
        // Regression for the truncated `colr`/`iloc` box out-of-bounds reads:
        // probing or decoding *any* prefix of a valid AVIF must return a
        // Result, never panic. The colr guard reads up to pos+16 and the iloc
        // guards up to iloc_pos+48, so some truncation length previously read
        // past the buffer end.
        let image = make_test_image(64, 64, 8, YuvFormat::Yuv420);
        let bytes = AvifEncoder::new(AvifConfig::default())
            .encode(&image)
            .expect("encode failed");
        for len in 0..bytes.len() {
            let prefix = &bytes[..len];
            let _ = AvifDecoder::probe(prefix);
            let _ = AvifDecoder::decode(prefix);
        }
        // The full buffer still probes correctly (no false rejection).
        assert!(AvifDecoder::probe(&bytes).is_ok());
    }

    /// Same invariant as [`truncated_avif_never_panics_probe_or_decode`],
    /// but over a *real* ffmpeg/libaom fixture rather than `AvifEncoder`'s
    /// own output.
    ///
    /// This matters because `AvifEncoder`'s own placeholder payload makes
    /// `color_offset + color_len == bytes.len()` exactly, so every `len in
    /// 0..bytes.len()` prefix in the test above fails inside
    /// `extract_av1_payload` and never reaches `decode_still_image` /
    /// `Av1Decoder` at all -- the real AV1 keyframe decode path (and the
    /// `iloc` version-0 item-extent loop in `container.rs`, which walks a
    /// bitstream-declared `item_count` with variable `offset_size`/
    /// `length_size`) gets zero truncation coverage from it. This fixture's
    /// real `iloc` v0 box and real OBU bitstream close that gap: long
    /// enough prefixes pass container validation and hand truncated OBU
    /// bytes to `Av1Decoder` itself.
    #[cfg(feature = "av1")]
    #[test]
    fn truncated_real_avif_fixture_never_panics() {
        let bytes: &[u8] = include_bytes!("testdata/color_only.avif");
        for len in 0..bytes.len() {
            let prefix = &bytes[..len];
            let _ = AvifDecoder::probe(prefix);
            let _ = AvifDecoder::decode(prefix);
        }
        assert!(AvifDecoder::probe(bytes).is_ok());
        assert!(AvifDecoder::decode(bytes).is_ok());
    }
}

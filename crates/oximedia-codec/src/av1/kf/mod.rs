//! Real AV1 keyframe / intra-only frame decoding (8-bit, 4:2:0, profile 0).
//!
//! This module is an exact port of the intra decode path of the AV1
//! specification (AOMediaCodec/av1-spec), verified bit-exact against
//! dav1d 1.5.1 and aomdec (libaom v3.12.1) reference decodes of real
//! encoder output (aomenc + SVT-AV1; see the tests at the bottom).
//!
//! Layout:
//! - [`bits`]: spec section 4.10 bit descriptors (`f/uvlc/le/leb128/su/ns`).
//! - [`msac`]: spec 8.2 symbol decoder (daala range coder, CDF adaptation).
//! - [`hdr`]: sequence-header and uncompressed frame-header parsing.
//! - [`cdfs`]: live per-tile CDF context, initialized per spec 7.20.
//! - [`coef`]: transform-coefficient decode (spec 5.11.39 + 8.3.2 contexts).
//! - [`pred`]: intra predictors, edge filtering/upsampling, CFL.
//! - [`itx`]: exact inverse transforms (spec 7.13).
//! - [`recon`]: tile/partition/block decode driver and reconstruction.
//! - [`lf`]: deblocking loop filter (spec 7.14).
//! - [`cdef`]: constrained directional enhancement filter (spec 7.15).
//! - [`lr`]: loop restoration — Wiener + self-guided (spec 7.17).
//! - [`tables_*`] / [`consts`]: tables mechanically extracted from the AV1
//!   specification text (scan orders, conversion tables, default CDFs,
//!   dequant lookups, constants) and cross-checked against libaom v3.12.1.

pub(crate) mod bits;
pub(crate) mod cdef;
pub(crate) mod cdfs;
pub(crate) mod coef;
pub(crate) mod consts;
pub(crate) mod hdr;
pub(crate) mod itx;
pub(crate) mod lf;
pub(crate) mod lr;
pub(crate) mod msac;
pub(crate) mod pred;
pub(crate) mod recon;
pub(crate) mod simd;
#[allow(dead_code)]
pub(crate) mod tables_cdf_coef;
#[allow(dead_code)]
pub(crate) mod tables_cdf_mode;
#[allow(dead_code)]
pub(crate) mod tables_conv;
#[allow(dead_code)]
pub(crate) mod tables_scan;

use crate::av1::obu::{ObuIterator, ObuType};
use crate::error::{CodecError, CodecResult};
use hdr::{FrameHdr, SeqHdr};
pub(crate) use recon::DecodedIntraFrame;

/// Outcome of decoding one temporal unit.
pub(crate) enum TuOutcome {
    /// A frame was decoded to pixels.
    Frame(Box<DecodedIntraFrame>, Box<FrameHdr>),
    /// `show_existing_frame`: present the frame in this reference slot.
    ShowExisting(u32),
    /// The temporal unit contained no frame (e.g. only a sequence header).
    NoFrame,
}

/// Splits one tile-group OBU payload into per-tile coded byte ranges
/// (spec 5.11.1 `tile_group_obu`). Appends to `tiles`.
fn parse_tile_group<'a>(
    payload: &'a [u8],
    fh: &FrameHdr,
    tiles: &mut Vec<&'a [u8]>,
) -> CodecResult<()> {
    let num_tiles = (fh.tiles.tile_cols * fh.tiles.tile_rows) as usize;
    let mut r = bits::BitRdr::new(payload);
    let mut tile_start_and_end_present_flag = false;
    if num_tiles > 1 {
        tile_start_and_end_present_flag = r.flag()?;
    }
    let (tg_start, tg_end) = if num_tiles == 1 || !tile_start_and_end_present_flag {
        (0usize, num_tiles - 1)
    } else {
        let tile_bits = fh.tiles.tile_cols_log2 + fh.tiles.tile_rows_log2;
        let s = r.f(tile_bits)? as usize;
        let e = r.f(tile_bits)? as usize;
        (s, e)
    };
    r.byte_alignment()?;
    let mut offset = r.byte_offset();
    let mut remaining = payload.len() - offset;

    for tile_num in tg_start..=tg_end {
        let last_tile = tile_num == tg_end;
        let tile_size = if last_tile {
            remaining
        } else {
            if offset + fh.tiles.tile_size_bytes as usize > payload.len() {
                return Err(CodecError::InvalidBitstream(
                    "AV1: truncated tile size field".into(),
                ));
            }
            let mut le = bits::BitRdr::new(&payload[offset..]);
            let sz = le.le(fh.tiles.tile_size_bytes)? as usize + 1;
            offset += fh.tiles.tile_size_bytes as usize;
            remaining -= fh.tiles.tile_size_bytes as usize;
            if sz > remaining {
                return Err(CodecError::InvalidBitstream(
                    "AV1: tile size exceeds tile group".into(),
                ));
            }
            sz
        };
        tiles.push(&payload[offset..offset + tile_size]);
        offset += tile_size;
        remaining -= tile_size;
    }
    Ok(())
}

/// Decodes one temporal unit worth of OBUs. `seq_state` carries the active
/// sequence header across calls (it may be replaced by a new sequence
/// header OBU).
///
/// # Errors
///
/// Honest `UnsupportedFeature` errors for inter frames and unimplemented
/// surfaces; `InvalidBitstream` for malformed data.
pub(crate) fn decode_temporal_unit(
    data: &[u8],
    seq_state: &mut Option<SeqHdr>,
) -> CodecResult<TuOutcome> {
    let mut frame_hdr: Option<FrameHdr> = None;
    let mut tile_payloads: Vec<&[u8]> = Vec::new();

    for obu in ObuIterator::new(data) {
        let (header, payload) = obu?;
        match header.obu_type {
            ObuType::SequenceHeader => {
                *seq_state = Some(SeqHdr::parse(payload)?);
            }
            ObuType::TemporalDelimiter | ObuType::Metadata | ObuType::Padding => {}
            ObuType::FrameHeader => {
                let seq = seq_state.as_ref().ok_or_else(|| {
                    CodecError::InvalidBitstream("AV1: frame header before sequence header".into())
                })?;
                let fh = FrameHdr::parse(payload, seq)?;
                if fh.show_existing_frame {
                    return Ok(TuOutcome::ShowExisting(fh.frame_to_show_map_idx));
                }
                frame_hdr = Some(fh);
            }
            ObuType::Frame => {
                let seq = seq_state.as_ref().ok_or_else(|| {
                    CodecError::InvalidBitstream("AV1: frame OBU before sequence header".into())
                })?;
                let fh = FrameHdr::parse(payload, seq)?;
                if fh.show_existing_frame {
                    return Ok(TuOutcome::ShowExisting(fh.frame_to_show_map_idx));
                }
                // frame_obu(): byte-align after the header, rest is the
                // tile group (spec 5.10).
                let header_bytes = fh.header_bits.div_ceil(8);
                if header_bytes > payload.len() {
                    return Err(CodecError::InvalidBitstream(
                        "AV1: frame OBU shorter than its header".into(),
                    ));
                }
                parse_tile_group(&payload[header_bytes..], &fh, &mut tile_payloads)?;
                frame_hdr = Some(fh);
            }
            ObuType::TileGroup => {
                let fh = frame_hdr.as_ref().ok_or_else(|| {
                    CodecError::InvalidBitstream("AV1: tile group before frame header".into())
                })?;
                parse_tile_group(payload, fh, &mut tile_payloads)?;
            }
            _ => {}
        }
    }

    let Some(fh) = frame_hdr else {
        return Ok(TuOutcome::NoFrame);
    };
    let seq = seq_state
        .as_ref()
        .ok_or_else(|| CodecError::InvalidBitstream("AV1: missing sequence header".into()))?;
    if tile_payloads.is_empty() {
        return Err(CodecError::InvalidBitstream(
            "AV1: frame header without tile group data".into(),
        ));
    }
    let frame = recon::decode_intra_frame(seq, &fh, &tile_payloads)?;
    Ok(TuOutcome::Frame(Box::new(frame), Box::new(fh)))
}

#[cfg(test)]
mod tests {
    use super::hdr::{FrameHdr, SeqHdr, KEY_FRAME};
    use super::{decode_temporal_unit, TuOutcome};
    use crate::av1::obu::{ObuIterator, ObuType};

    /// Parses the first sequence header + frame header from a temporal unit.
    fn parse_headers(tu: &[u8]) -> (SeqHdr, FrameHdr) {
        let mut seq = None;
        for obu in ObuIterator::new(tu) {
            let (header, payload) = obu.expect("obu parse");
            match header.obu_type {
                ObuType::SequenceHeader => {
                    seq = Some(SeqHdr::parse(payload).expect("seq parse"));
                }
                ObuType::FrameHeader | ObuType::Frame => {
                    let s = seq.expect("sequence header before frame");
                    let f = FrameHdr::parse(payload, &s).expect("frame parse");
                    return (s, f);
                }
                _ => {}
            }
        }
        panic!("no frame header found");
    }

    /// Oracle values below were transcribed from `ffmpeg -bsf:v trace_headers`
    /// dumps of the exact same embedded bitstreams (ffmpeg 7.x CBS reader).
    #[test]
    fn stage0_headers_aomenc_lossless_gray64() {
        let tu = include_bytes!("testdata/s1_ll_gray64.obu");
        let (s, f) = parse_headers(tu);
        assert_eq!(s.max_frame_width, 64);
        assert_eq!(s.max_frame_height, 64);
        assert!(!s.use_128x128_superblock);
        assert!(!s.enable_superres);
        assert!(s.enable_cdef);
        assert!(s.enable_restoration);
        assert_eq!(s.bit_depth, 8);
        assert!(!s.film_grain_params_present);
        assert_eq!(f.frame_type, KEY_FRAME);
        assert!(f.show_frame);
        assert!(!f.disable_cdf_update);
        assert_eq!(f.base_q_idx, 0);
        assert!(f.coded_lossless, "qindex 0 with no deltas is lossless");
        assert!(f.all_lossless);
        assert_eq!(f.lf.level, [0, 0, 0, 0], "lossless forces lf off");
        assert_eq!(f.cdef.bits, 0);
        assert_eq!(f.cdef.damping, 3);
        assert!(!f.using_qmatrix);
        assert!(!f.seg.enabled);
        assert!(!f.tx_mode_select, "lossless implies ONLY_4X4");
        assert!(!f.reduced_tx_set);
        assert_eq!(f.tiles.tile_cols_log2, 0);
        assert_eq!(f.tiles.tile_rows_log2, 0);
        assert_eq!(f.tiles.tile_cols, 1);
        assert_eq!(f.tiles.tile_rows, 1);
        assert_eq!(f.mi_cols, 16);
        assert_eq!(f.mi_rows, 16);
    }

    #[test]
    fn stage0_headers_svt64() {
        let tu = include_bytes!("testdata/s1_svt64.obu");
        let (s, f) = parse_headers(tu);
        assert_eq!((s.max_frame_width, s.max_frame_height), (64, 64));
        assert!(!s.use_128x128_superblock);
        assert!(!s.enable_cdef, "SVT vector encoded with --enable-cdef 0");
        assert!(!s.enable_restoration);
        assert_eq!(f.base_q_idx, 31);
        assert!(!f.allow_screen_content_tools);
        assert!(!f.allow_intrabc);
        assert!(!f.delta_q_present);
        assert_eq!(f.lf.level[0], 0);
        assert_eq!(f.lf.level[1], 0);
        assert_eq!(f.lf.sharpness, 0);
        assert!(f.tx_mode_select);
        assert!(!f.reduced_tx_set);
        assert!(!f.coded_lossless);
    }

    #[test]
    fn stage0_headers_aom128_sb128() {
        let tu = include_bytes!("testdata/s2_aom128.obu");
        let (s, f) = parse_headers(tu);
        assert_eq!((s.max_frame_width, s.max_frame_height), (128, 128));
        assert!(s.use_128x128_superblock);
        assert!(!s.enable_cdef);
        assert!(!s.enable_restoration);
        assert_eq!(f.base_q_idx, 80);
        assert!(f.allow_screen_content_tools);
        assert!(!f.allow_intrabc);
        assert_eq!(f.lf.level, [4, 4, 5, 10]);
        assert_eq!(f.lf.sharpness, 0);
        assert!(f.tx_mode_select);
        assert_eq!(f.mi_cols, 32);
        assert_eq!(f.mi_rows, 32);
    }

    #[test]
    fn stage0_headers_aom128_cdef() {
        let tu = include_bytes!("testdata/s3_aom128.obu");
        let (s, f) = parse_headers(tu);
        assert!(s.enable_cdef);
        assert!(!s.enable_restoration);
        assert!(!s.use_128x128_superblock);
        assert_eq!(f.base_q_idx, 120);
        assert_eq!(f.lf.level, [1, 6, 11, 15]);
        assert_eq!(f.cdef.damping, 4);
        assert_eq!(f.cdef.bits, 1);
        assert_eq!(f.cdef.y_pri_strength[0], 13);
        assert_eq!(f.cdef.y_sec_strength[0], 4, "coded 3 remaps to 4");
        assert_eq!(f.cdef.y_pri_strength[1], 1);
        assert_eq!(f.cdef.y_sec_strength[1], 2);
    }

    #[test]
    fn stage0_headers_svt256_two_tile_cols() {
        let tu = include_bytes!("testdata/s1_svt256tc.obu");
        let (s, f) = parse_headers(tu);
        assert_eq!((s.max_frame_width, s.max_frame_height), (256, 128));
        assert_eq!(f.tiles.tile_cols_log2, 1);
        assert_eq!(f.tiles.tile_rows_log2, 0);
        assert_eq!(f.tiles.tile_cols, 2);
        assert_eq!(f.tiles.tile_rows, 1);
        assert_eq!(f.tiles.context_update_tile_id, 1);
        assert_eq!(f.tiles.tile_size_bytes, 2);
        assert_eq!(f.tiles.mi_col_starts, vec![0, 32, 64]);
        assert_eq!(f.tiles.mi_row_starts, vec![0, 32]);
        assert_eq!(f.base_q_idx, 31);
    }

    #[test]
    fn stage0_all_vectors_parse() {
        let vectors: [(&[u8], u32, u32); 11] = [
            (include_bytes!("testdata/s1_ll_gray64.obu"), 64, 64),
            (include_bytes!("testdata/s1_ll_grad64.obu"), 64, 64),
            (include_bytes!("testdata/s1_ll64.obu"), 64, 64),
            (include_bytes!("testdata/s1_svt64.obu"), 64, 64),
            (include_bytes!("testdata/s1_svt128.obu"), 128, 128),
            (include_bytes!("testdata/s1_svt76x42.obu"), 76, 42),
            (include_bytes!("testdata/s1_svt256tc.obu"), 256, 128),
            (include_bytes!("testdata/s2_aom128.obu"), 128, 128),
            (include_bytes!("testdata/s2_aom76x42.obu"), 76, 42),
            (include_bytes!("testdata/s3_aom128.obu"), 128, 128),
            (include_bytes!("testdata/s4_aom128.obu"), 128, 128),
        ];
        for (i, (tu, w, h)) in vectors.iter().enumerate() {
            let (_s, f) = parse_headers(tu);
            assert_eq!(f.frame_width, *w, "vector {i} width");
            assert_eq!(f.frame_height, *h, "vector {i} height");
            assert_eq!(f.frame_type, KEY_FRAME, "vector {i} type");
            assert!(!f.use_superres, "vector {i} superres");
            assert!(!f.apply_grain, "vector {i} grain");
            assert!(!f.using_qmatrix, "vector {i} qm");
        }
    }

    // ------------------------------------------------------------ stage 1+

    /// Splits a raw planar YUV420 reference dump into (Y, U, V).
    fn split_yuv(data: &[u8], w: usize, h: usize) -> (&[u8], &[u8], &[u8]) {
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let ysz = w * h;
        let csz = cw * ch;
        assert_eq!(data.len(), ysz + 2 * csz, "reference YUV size");
        (
            &data[..ysz],
            &data[ysz..ysz + csz],
            &data[ysz + csz..ysz + 2 * csz],
        )
    }

    /// Compares one decoded (MI-aligned) plane against a cropped reference
    /// plane; returns mismatch count and prints the first few coordinates.
    fn diff_plane(
        dec: &super::recon::PlaneBuf,
        r#ref: &[u8],
        w: usize,
        h: usize,
        name: &str,
    ) -> usize {
        let mut bad = 0usize;
        for y in 0..h {
            for x in 0..w {
                let d = dec.data[y * dec.stride + x];
                let e = r#ref[y * w + x];
                if d != e {
                    if bad < 16 {
                        println!("{name} mismatch at ({x},{y}) dec={d} ref={e}");
                    }
                    bad += 1;
                }
            }
        }
        bad
    }

    fn assert_bit_exact(tu: &[u8], ref_yuv: &[u8], w: usize, h: usize, label: &str) {
        let mut seq = None;
        let outcome = decode_temporal_unit(tu, &mut seq).expect("decode");
        let TuOutcome::Frame(frame, _hdr) = outcome else {
            panic!("{label}: expected a decoded frame");
        };
        assert_eq!(frame.width, w);
        assert_eq!(frame.height, h);
        let (ry, ru, rv) = split_yuv(ref_yuv, w, h);
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let bad_y = diff_plane(&frame.planes[0], ry, w, h, "Y");
        let bad_u = diff_plane(&frame.planes[1], ru, cw, ch, "U");
        let bad_v = diff_plane(&frame.planes[2], rv, cw, ch, "V");
        assert!(
            bad_y == 0 && bad_u == 0 && bad_v == 0,
            "{label}: decode differs from dav1d/aomdec reference: \
             Y {bad_y} px, U {bad_u} px, V {bad_v} px"
        );
    }

    /// aomenc lossless, flat gray 64x64: the minimal symbol stream (all-skip
    /// DC blocks + WHT path).
    #[test]
    fn stage1_lossless_gray64_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_ll_gray64.obu"),
            include_bytes!("testdata/s1_ll_gray64.yuv"),
            64,
            64,
            "s1_ll_gray64",
        );
    }

    /// aomenc lossless gradient 64x64 (directional modes + WHT residuals).
    #[test]
    fn stage1_lossless_grad64_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_ll_grad64.obu"),
            include_bytes!("testdata/s1_ll_grad64.yuv"),
            64,
            64,
            "s1_ll_grad64",
        );
    }

    /// aomenc lossless testsrc2 64x64 (rich texture, all-mode WHT).
    #[test]
    fn stage1_lossless_testsrc64_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_ll64.obu"),
            include_bytes!("testdata/s1_ll64.yuv"),
            64,
            64,
            "s1_ll64",
        );
    }

    /// SVT-AV1 lossy q30 64x64, dlf/cdef/lr off (DCT/ADST/IDTX + dequant).
    #[test]
    fn stage1_svt64_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_svt64.obu"),
            include_bytes!("testdata/s1_svt64.yuv"),
            64,
            64,
            "s1_svt64",
        );
    }

    /// SVT-AV1 lossy q26 128x128, post-filters off.
    #[test]
    fn stage1_svt128_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_svt128.obu"),
            include_bytes!("testdata/s1_svt128.yuv"),
            128,
            128,
            "s1_svt128",
        );
    }

    /// SVT-AV1 lossy q30 76x42 (odd size, crop + partial-superblock paths).
    #[test]
    fn stage1_svt76x42_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_svt76x42.obu"),
            include_bytes!("testdata/s1_svt76x42.yuv"),
            76,
            42,
            "s1_svt76x42",
        );
    }

    /// SVT-AV1 256x128 with two tile columns (tile-size parsing, per-tile
    /// symbol decoders and context resets at the tile boundary).
    #[test]
    fn stage1_svt256_two_tiles_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s1_svt256tc.obu"),
            include_bytes!("testdata/s1_svt256tc.yuv"),
            256,
            128,
            "s1_svt256tc",
        );
    }

    /// aomenc CRF20 128x128 with 128x128 superblocks, cdef/lr disabled but
    /// deblocking ON (levels 4/4/5/10): stage 2.
    #[test]
    fn stage2_aom128_deblock_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s2_aom128.obu"),
            include_bytes!("testdata/s2_aom128.yuv"),
            128,
            128,
            "s2_aom128",
        );
    }

    /// aomenc CRF24 76x42 with deblocking ON (odd-size boundary handling).
    #[test]
    fn stage2_aom76x42_deblock_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s2_aom76x42.obu"),
            include_bytes!("testdata/s2_aom76x42.yuv"),
            76,
            42,
            "s2_aom76x42",
        );
    }

    /// aomenc CRF30 128x128 with CDEF ON (damping 4, 2 strength pairs),
    /// restoration off: stage 3.
    #[test]
    fn stage3_aom128_cdef_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s3_aom128.obu"),
            include_bytes!("testdata/s3_aom128.yuv"),
            128,
            128,
            "s3_aom128",
        );
    }

    /// aomenc CRF30 128x128 with CDEF ON and restoration enabled at the
    /// sequence level but RESTORE_NONE for every plane at the frame level.
    #[test]
    fn stage3_aom128_cdef_lr_none_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s4_aom128.obu"),
            include_bytes!("testdata/s4_aom128.yuv"),
            128,
            128,
            "s4_aom128",
        );
    }

    /// aomenc 320x192 noisy content, self-guided restoration (SGRPROJ) on
    /// all three planes, CDEF + deblock also active: stage 4.
    #[test]
    fn stage4_sgrproj_320x192_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s5_lr320_q30.obu"),
            include_bytes!("testdata/s5_lr320_q30.yuv"),
            320,
            192,
            "s5_lr320_q30",
        );
    }

    /// aomenc 320x192 blurred content: SWITCHABLE restoration on luma
    /// (per-unit restoration_type) and WIENER on both chroma planes.
    #[test]
    fn stage4_switchable_wiener_320x192_bit_exact() {
        assert_bit_exact(
            include_bytes!("testdata/s5_blur_q20.obu"),
            include_bytes!("testdata/s5_blur_q20.yuv"),
            320,
            192,
            "s5_blur_q20",
        );
    }

    /// Truncated/garbage streams must error, never emit a frame.
    #[test]
    fn garbage_and_truncated_streams_error_honestly() {
        let tu = include_bytes!("testdata/s1_svt64.obu");
        // Truncations at various points.
        for cut in [1usize, 5, 16, tu.len() / 2] {
            let mut seq = None;
            let r = decode_temporal_unit(&tu[..cut], &mut seq);
            assert!(
                !matches!(r, Ok(TuOutcome::Frame(..))),
                "truncated stream (len {cut}) must not produce a frame"
            );
        }
        // Pure garbage.
        let garbage = [0xA7u8; 64];
        let mut seq = None;
        let r = decode_temporal_unit(&garbage, &mut seq);
        assert!(
            !matches!(r, Ok(TuOutcome::Frame(..))),
            "garbage must not produce a frame"
        );
    }
}

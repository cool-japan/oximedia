//! H.264 slice header parsing.
//!
//! Implements the prefix of `slice_header()` from H.264 §7.3.3 that
//! every decoder needs in order to (a) place the slice in its picture,
//! (b) know which references to bring forward from the DPB, and
//! (c) recover the slice's effective QP.  This is enough for diagnostic
//! tools, conformance tests, and as the front-end for a future
//! reconstruction path.
//!
//! The parsing context comes from an already-parsed SPS and PPS — see
//! [`crate::h264::sps::SpsRbsp`] and [`crate::h264::pps::PpsRbsp`].
//!
//! ## Scope and intentional gaps
//!
//! The following slice header fields are parsed and returned:
//!
//! - `first_mb_in_slice`, `slice_type`, `pic_parameter_set_id`
//! - `colour_plane_id` (4:4:4 separate-plane streams)
//! - `frame_num`
//! - `field_pic_flag` / `bottom_field_flag`
//! - `idr_pic_id` (IDR only)
//! - `pic_order_cnt_lsb` / `delta_pic_order_cnt_bottom` (`pic_order_cnt_type == 0`)
//! - `delta_pic_order_cnt[0..1]` (`pic_order_cnt_type == 1`)
//! - `redundant_pic_cnt`
//! - `direct_spatial_mv_pred_flag` (B-slices)
//! - `num_ref_idx_active_override_flag` and the override values
//! - `slice_qp_delta` (yielding the effective slice QP)
//!
//! Sub-clauses bit-skipped without retention (consumed only to keep the
//! parser positioned correctly, when present):
//!
//! - Reference picture list modification.
//! - Prediction weight table.
//! - Decoded reference picture marking (the full MMCO loop).
//! - Cabac alignment / cabac_init_idc.
//! - Deblocking filter offsets.
//!
//! Once a future PR needs these for reconstruction, this module is the
//! place to extend them.

use crate::h264::bit_reader::BitReader;
use crate::h264::pps::PpsRbsp;
use crate::h264::sps::SpsRbsp;
use crate::CodecError;

/// H.264 slice type, after the spec's `% 5` collapse of values 5..=9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    /// Predicted slice.
    P,
    /// Bi-predicted slice.
    B,
    /// Intra-coded slice.
    I,
    /// Switching P slice.
    SP,
    /// Switching I slice.
    SI,
}

impl SliceType {
    /// Parses a raw `slice_type` syntax element (0..=9).
    pub fn from_raw(raw: u32) -> Result<Self, CodecError> {
        match raw % 5 {
            0 => Ok(Self::P),
            1 => Ok(Self::B),
            2 => Ok(Self::I),
            3 => Ok(Self::SP),
            4 => Ok(Self::SI),
            _ => Err(CodecError::InvalidData(format!(
                "h264 slice_header: invalid slice_type {raw}"
            ))),
        }
    }

    /// True for I and SI.
    #[must_use]
    pub fn is_intra(self) -> bool {
        matches!(self, Self::I | Self::SI)
    }

    /// True for B.
    #[must_use]
    pub fn is_bi_predictive(self) -> bool {
        matches!(self, Self::B)
    }
}

/// Information the caller must supply for an IDR slice; see H.264
/// §7.3.3 for the syntax of `idr_pic_id` which is only present when
/// the NAL unit type is 5 (IDR).
#[derive(Debug, Clone, Copy)]
pub struct NalContext {
    /// `nal_ref_idc` from the enclosing NAL unit header byte (bits 6..5).
    pub nal_ref_idc: u8,
    /// True iff `nal_unit_type` is 5.
    pub is_idr: bool,
}

/// Result of parsing a slice header against a known SPS / PPS pair.
#[derive(Debug, Clone)]
pub struct SliceHeader {
    /// Position of the first macroblock in raster order.
    pub first_mb_in_slice: u32,
    /// Slice type.
    pub slice_type: SliceType,
    /// PPS this slice uses.
    pub pic_parameter_set_id: u32,
    /// Present only when the SPS uses 4:4:4 with separately coded
    /// colour planes.
    pub colour_plane_id: Option<u8>,
    /// `frame_num`.
    pub frame_num: u32,
    /// `field_pic_flag`.
    pub field_pic_flag: bool,
    /// `bottom_field_flag` (only meaningful when `field_pic_flag` is true).
    pub bottom_field_flag: bool,
    /// `idr_pic_id` (only present for IDR slices).
    pub idr_pic_id: Option<u32>,
    /// `pic_order_cnt_lsb` (pic_order_cnt_type == 0).
    pub pic_order_cnt_lsb: Option<u32>,
    /// `delta_pic_order_cnt_bottom` (when present alongside
    /// `pic_order_cnt_lsb`).
    pub delta_pic_order_cnt_bottom: Option<i32>,
    /// `delta_pic_order_cnt[0]` (pic_order_cnt_type == 1).
    pub delta_pic_order_cnt: Option<[i32; 2]>,
    /// `redundant_pic_cnt` (only present when the PPS allows it).
    pub redundant_pic_cnt: Option<u32>,
    /// `direct_spatial_mv_pred_flag` (B-slices only).
    pub direct_spatial_mv_pred_flag: Option<bool>,
    /// Active reference list 0 size minus 1, after any override.
    pub num_ref_idx_l0_active_minus1: u32,
    /// Active reference list 1 size minus 1, after any override.
    pub num_ref_idx_l1_active_minus1: u32,
    /// `slice_qp_delta`.
    pub slice_qp_delta: i32,
    /// Effective slice QP: PPS `pic_init_qp_minus26 + 26 + slice_qp_delta`.
    pub slice_qp_y: i32,
}

/// Parses a slice header RBSP (emulation prevention already removed).
///
/// `sps` and `pps` provide the context that the H.264 syntax depends
/// on — picture coordinate model, ordering counts, default reference
/// list sizes, etc.
///
/// `ctx` carries fields the slice header inherits from its enclosing
/// NAL unit header (which is *not* part of the RBSP).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] on malformed bitstreams or when
/// the SPS / PPS context is inconsistent with the slice payload (for
/// example, the slice's `pic_parameter_set_id` not matching the PPS the
/// caller supplied).
pub fn parse_slice_header(
    rbsp: &[u8],
    sps: &SpsRbsp,
    pps: &PpsRbsp,
    ctx: NalContext,
) -> Result<SliceHeader, CodecError> {
    let mut r = BitReader::new(rbsp);

    let first_mb_in_slice = r.read_ue()?;
    let slice_type_raw = r.read_ue()?;
    let slice_type = SliceType::from_raw(slice_type_raw)?;
    let pic_parameter_set_id = r.read_ue()?;

    if pic_parameter_set_id != pps.pic_parameter_set_id {
        return Err(CodecError::InvalidData(format!(
            "h264 slice_header: slice's pic_parameter_set_id {pic_parameter_set_id} \
             does not match supplied PPS id {pps_id}",
            pps_id = pps.pic_parameter_set_id,
        )));
    }

    let colour_plane_id = if sps.separate_colour_plane_flag {
        Some(r.read_bits(2)? as u8)
    } else {
        None
    };

    let frame_num_bits = sps.log2_max_frame_num_minus4 + 4;
    let frame_num = r.read_bits(frame_num_bits)?;

    let (field_pic_flag, bottom_field_flag) = if sps.frame_mbs_only_flag {
        (false, false)
    } else {
        let fpf = r.read_bit()?;
        let bff = if fpf { r.read_bit()? } else { false };
        (fpf, bff)
    };

    let idr_pic_id = if ctx.is_idr {
        Some(r.read_ue()?)
    } else {
        None
    };

    let mut pic_order_cnt_lsb = None;
    let mut delta_pic_order_cnt_bottom = None;
    let mut delta_pic_order_cnt = None;
    match sps.pic_order_cnt_type {
        0 => {
            let lsb_bits = sps.log2_max_pic_order_cnt_lsb_minus4 + 4;
            pic_order_cnt_lsb = Some(r.read_bits(lsb_bits)?);
            if pps.bottom_field_pic_order_in_frame_present_flag && !field_pic_flag {
                delta_pic_order_cnt_bottom = Some(r.read_se()?);
            }
        }
        1 => {
            if !sps.delta_pic_order_always_zero_flag {
                let first = r.read_se()?;
                let second = if pps.bottom_field_pic_order_in_frame_present_flag
                    && !field_pic_flag
                {
                    r.read_se()?
                } else {
                    0
                };
                delta_pic_order_cnt = Some([first, second]);
            }
        }
        _ => {
            // pic_order_cnt_type == 2: no additional fields.
        }
    }

    let redundant_pic_cnt = if pps.redundant_pic_cnt_present_flag {
        Some(r.read_ue()?)
    } else {
        None
    };

    let direct_spatial_mv_pred_flag = if slice_type.is_bi_predictive() {
        Some(r.read_bit()?)
    } else {
        None
    };

    let mut num_ref_idx_l0 = pps.num_ref_idx_l0_default_active_minus1;
    let mut num_ref_idx_l1 = pps.num_ref_idx_l1_default_active_minus1;
    if matches!(slice_type, SliceType::P | SliceType::SP | SliceType::B) {
        let override_flag = r.read_bit()?;
        if override_flag {
            num_ref_idx_l0 = r.read_ue()?;
            if slice_type.is_bi_predictive() {
                num_ref_idx_l1 = r.read_ue()?;
            }
        }
    }

    // Reference picture list modification — bit-skipped here.  See the
    // module doc for scope notes.
    if !slice_type.is_intra() {
        consume_ref_pic_list_modification(&mut r)?;
    }
    if slice_type.is_bi_predictive() {
        consume_ref_pic_list_modification(&mut r)?;
    }

    // Prediction weight table.
    if (pps.weighted_pred_flag
        && matches!(slice_type, SliceType::P | SliceType::SP))
        || (pps.weighted_bipred_idc == 1 && slice_type.is_bi_predictive())
    {
        consume_pred_weight_table(&mut r, sps, num_ref_idx_l0, num_ref_idx_l1)?;
    }

    // Decoded reference picture marking.
    if ctx.nal_ref_idc != 0 {
        consume_dec_ref_pic_marking(&mut r, ctx.is_idr)?;
    }

    // cabac_init_idc when entropy mode is CABAC and we're not intra.
    if pps.entropy_coding_mode_flag && !slice_type.is_intra() {
        let _cabac_init_idc = r.read_ue()?;
    }

    let slice_qp_delta = r.read_se()?;
    let slice_qp_y = pps.pic_init_qp_minus26 + 26 + slice_qp_delta;

    Ok(SliceHeader {
        first_mb_in_slice,
        slice_type,
        pic_parameter_set_id,
        colour_plane_id,
        frame_num,
        field_pic_flag,
        bottom_field_flag,
        idr_pic_id,
        pic_order_cnt_lsb,
        delta_pic_order_cnt_bottom,
        delta_pic_order_cnt,
        redundant_pic_cnt,
        direct_spatial_mv_pred_flag,
        num_ref_idx_l0_active_minus1: num_ref_idx_l0,
        num_ref_idx_l1_active_minus1: num_ref_idx_l1,
        slice_qp_delta,
        slice_qp_y,
    })
}

fn consume_ref_pic_list_modification(r: &mut BitReader<'_>) -> Result<(), CodecError> {
    let modification_flag = r.read_bit()?;
    if !modification_flag {
        return Ok(());
    }
    loop {
        let op = r.read_ue()?;
        match op {
            0 | 1 => {
                let _abs_diff_pic_num_minus1 = r.read_ue()?;
            }
            2 => {
                let _long_term_pic_num = r.read_ue()?;
            }
            3 => return Ok(()),
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 slice_header: invalid ref_pic_list_modification op {op}"
                )));
            }
        }
    }
}

fn consume_pred_weight_table(
    r: &mut BitReader<'_>,
    sps: &SpsRbsp,
    num_ref_l0: u32,
    num_ref_l1: u32,
) -> Result<(), CodecError> {
    let _luma_log2_weight_denom = r.read_ue()?;
    if sps.chroma_format_idc != 0 {
        let _chroma_log2_weight_denom = r.read_ue()?;
    }
    consume_weights_for_list(r, sps, num_ref_l0)?;
    consume_weights_for_list(r, sps, num_ref_l1)?;
    Ok(())
}

fn consume_weights_for_list(
    r: &mut BitReader<'_>,
    sps: &SpsRbsp,
    num_ref_minus1: u32,
) -> Result<(), CodecError> {
    for _ in 0..=num_ref_minus1 {
        let luma_weight_flag = r.read_bit()?;
        if luma_weight_flag {
            let _luma_weight = r.read_se()?;
            let _luma_offset = r.read_se()?;
        }
        if sps.chroma_format_idc != 0 {
            let chroma_weight_flag = r.read_bit()?;
            if chroma_weight_flag {
                for _ in 0..2 {
                    let _chroma_weight = r.read_se()?;
                    let _chroma_offset = r.read_se()?;
                }
            }
        }
    }
    Ok(())
}

fn consume_dec_ref_pic_marking(
    r: &mut BitReader<'_>,
    is_idr: bool,
) -> Result<(), CodecError> {
    if is_idr {
        let _no_output_of_prior_pics_flag = r.read_bit()?;
        let _long_term_reference_flag = r.read_bit()?;
        return Ok(());
    }
    let adaptive = r.read_bit()?;
    if !adaptive {
        return Ok(());
    }
    loop {
        let op = r.read_ue()?;
        match op {
            0 => return Ok(()),
            1 | 3 => {
                let _difference_of_pic_nums_minus1 = r.read_ue()?;
                if op == 3 {
                    let _long_term_frame_idx = r.read_ue()?;
                }
            }
            2 => {
                let _long_term_pic_num = r.read_ue()?;
            }
            4 => {
                let _max_long_term_frame_idx_plus1 = r.read_ue()?;
            }
            5 => {
                // Mark all reference pictures as unused for reference.
            }
            6 => {
                let _long_term_frame_idx = r.read_ue()?;
            }
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 slice_header: invalid memory_management_control op {op}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::pps::PpsRbsp;
    use crate::h264::sps::SpsRbsp;

    fn baseline_sps() -> SpsRbsp {
        SpsRbsp {
            profile_idc: 66,
            constraint_set_flags: 0,
            level_idc: 31,
            seq_parameter_set_id: 0,
            chroma_format_idc: 1,
            separate_colour_plane_flag: false,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            qpprime_y_zero_transform_bypass_flag: false,
            seq_scaling_matrix_present_flag: false,
            log2_max_frame_num_minus4: 0, // -> frame_num is 4 bits
            pic_order_cnt_type: 0,
            log2_max_pic_order_cnt_lsb_minus4: 0, // -> pic_order_cnt_lsb is 4 bits
            delta_pic_order_always_zero_flag: false,
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            num_ref_frames: 1,
            gaps_in_frame_num_value_allowed_flag: false,
            pic_width_in_mbs_minus1: 19,
            pic_height_in_map_units_minus1: 14,
            frame_mbs_only_flag: true,
            mb_adaptive_frame_field_flag: false,
            direct_8x8_inference_flag: false,
            frame_cropping_flag: false,
            frame_crop_left_offset: 0,
            frame_crop_right_offset: 0,
            frame_crop_top_offset: 0,
            frame_crop_bottom_offset: 0,
            vui_parameters_present_flag: false,
        }
    }

    fn baseline_pps() -> PpsRbsp {
        PpsRbsp {
            pic_parameter_set_id: 0,
            seq_parameter_set_id: 0,
            entropy_coding_mode_flag: false,
            bottom_field_pic_order_in_frame_present_flag: false,
            num_slice_groups_minus1: 0,
            num_ref_idx_l0_default_active_minus1: 0,
            num_ref_idx_l1_default_active_minus1: 0,
            weighted_pred_flag: false,
            weighted_bipred_idc: 0,
            pic_init_qp_minus26: 2, // initial QP 28
            pic_init_qs_minus26: 0,
            chroma_qp_index_offset: 0,
            deblocking_filter_control_present_flag: true,
            constrained_intra_pred_flag: false,
            redundant_pic_cnt_present_flag: false,
            transform_8x8_mode_flag: false,
            pic_scaling_matrix_present_flag: false,
            second_chroma_qp_index_offset: 0,
        }
    }

    fn build_idr_i_slice_header() -> Vec<u8> {
        // For an IDR I-slice with the SPS/PPS above:
        //   first_mb_in_slice = 0  (ue: `1`)
        //   slice_type = 7         (ue codeword for 7 — collapsed to I)
        //   pic_parameter_set_id = 0
        //   frame_num = 0  (4 bits)
        //   idr_pic_id = 0
        //   pic_order_cnt_lsb = 0 (4 bits)
        //   dec_ref_pic_marking: no_output_of_prior_pics=0, long_term_reference=0
        //   slice_qp_delta = 0  (se: `1`)
        //   stop bit
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // first_mb_in_slice
        push_ue(&mut bits, 7); // slice_type (will collapse to I)
        push_ue(&mut bits, 0); // pic_parameter_set_id
        push_bits_msb(&mut bits, 0, 4); // frame_num
        push_ue(&mut bits, 0); // idr_pic_id
        push_bits_msb(&mut bits, 0, 4); // pic_order_cnt_lsb
        // (no redundant_pic_cnt — PPS flag is false)
        // I-slice → no num_ref_idx override, no ref list modification (intra), no pred weight.
        // dec_ref_pic_marking for IDR: 2 flags
        bits.push(false);
        bits.push(false);
        // entropy_coding_mode == CAVLC → no cabac_init_idc
        push_se(&mut bits, 0); // slice_qp_delta
        bits.push(true); // RBSP stop bit
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        pack_bits_msb(&bits)
    }

    #[test]
    fn parses_idr_i_slice_header() {
        let sps = baseline_sps();
        let pps = baseline_pps();
        let rbsp = build_idr_i_slice_header();
        let ctx = NalContext {
            nal_ref_idc: 3,
            is_idr: true,
        };
        let sh = parse_slice_header(&rbsp, &sps, &pps, ctx).expect("should parse");
        assert_eq!(sh.first_mb_in_slice, 0);
        assert_eq!(sh.slice_type, SliceType::I);
        assert_eq!(sh.idr_pic_id, Some(0));
        assert_eq!(sh.pic_order_cnt_lsb, Some(0));
        assert_eq!(sh.slice_qp_delta, 0);
        // pic_init_qp_minus26 = 2 -> initial QP 28, slice_qp_delta 0 -> 28.
        assert_eq!(sh.slice_qp_y, 28);
    }

    #[test]
    fn slice_type_collapse_works() {
        assert_eq!(SliceType::from_raw(0).unwrap(), SliceType::P);
        assert_eq!(SliceType::from_raw(5).unwrap(), SliceType::P);
        assert_eq!(SliceType::from_raw(2).unwrap(), SliceType::I);
        assert_eq!(SliceType::from_raw(7).unwrap(), SliceType::I);
        assert_eq!(SliceType::from_raw(1).unwrap(), SliceType::B);
    }

    #[test]
    fn ppsid_mismatch_rejected() {
        let sps = baseline_sps();
        let pps = baseline_pps();
        // Build a slice with pic_parameter_set_id = 1 but pass a PPS
        // whose id is 0.
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // first_mb_in_slice
        push_ue(&mut bits, 7); // slice_type I
        push_ue(&mut bits, 1); // pic_parameter_set_id mismatched
        // Pad enough bytes for the parser not to short-read before the
        // mismatch check fires.
        while bits.len() < 64 {
            bits.push(false);
        }
        let rbsp = pack_bits_msb(&bits);
        let ctx = NalContext {
            nal_ref_idc: 3,
            is_idr: true,
        };
        assert!(parse_slice_header(&rbsp, &sps, &pps, ctx).is_err());
    }

    // -- bit-building helpers --

    fn push_ue(bits: &mut Vec<bool>, value: u32) {
        let mut n = 0u32;
        while (1u32 << (n + 1)) - 1 <= value {
            n += 1;
        }
        for _ in 0..n {
            bits.push(false);
        }
        bits.push(true);
        let suffix = value + 1 - (1u32 << n);
        push_bits_msb(bits, suffix, n);
    }

    fn push_se(bits: &mut Vec<bool>, value: i32) {
        let mapped = if value <= 0 {
            (-(value as i64) * 2) as u32
        } else {
            (value as i64 * 2 - 1) as u32
        };
        push_ue(bits, mapped);
    }

    fn push_bits_msb(bits: &mut Vec<bool>, mut value: u32, n: u32) {
        let mask = if n == 0 { 0 } else { 1u32 << (n - 1) };
        for _ in 0..n {
            bits.push(value & mask != 0);
            value <<= 1;
        }
    }

    fn pack_bits_msb(bits: &[bool]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bits.len() / 8 + 1);
        let mut byte = 0u8;
        let mut count = 0u8;
        for &b in bits {
            byte = (byte << 1) | u8::from(b);
            count += 1;
            if count == 8 {
                out.push(byte);
                byte = 0;
                count = 0;
            }
        }
        if count > 0 {
            byte <<= 8 - count;
            out.push(byte);
        }
        out
    }
}

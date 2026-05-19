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

/// One entry in a reference-picture-list modification command stream.
///
/// Spec syntax: `modification_of_pic_nums_idc` plus its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefPicListModOp {
    /// `modification_of_pic_nums_idc == 0`: subtract
    /// `abs_diff_pic_num_minus1 + 1` from the current picture number.
    SubtractAbsDiffPicNum(u32),
    /// `modification_of_pic_nums_idc == 1`: add the same.
    AddAbsDiffPicNum(u32),
    /// `modification_of_pic_nums_idc == 2`: assign long-term picture
    /// number `long_term_pic_num`.
    LongTermPicNum(u32),
}

/// Reference picture list modification for one list (L0 or L1).
///
/// `present` is true iff the slice header signalled the surrounding
/// `ref_pic_list_modification_flag_lX = 1`. When false, the modification
/// loop was not entered and `ops` is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefPicListModification {
    /// True when the slice signalled the modification flag.
    pub present: bool,
    /// Modification operations in spec order. Terminated by an implicit
    /// `modification_of_pic_nums_idc == 3` that is not stored here.
    pub ops: Vec<RefPicListModOp>,
}

/// A single luma + optional chroma weight/offset pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeightEntry {
    /// Luma weight (signed). `None` means luma flag was 0 — caller
    /// uses the implied default.
    pub luma: Option<(i32, i32)>,
    /// Per-chroma-component (Cb, Cr) weight + offset pair. `None`
    /// means the chroma flag was 0 or the stream is monochrome.
    pub chroma: Option<[(i32, i32); 2]>,
}

/// `pred_weight_table()` from H.264 §7.3.3.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredWeightTable {
    /// `luma_log2_weight_denom`.
    pub luma_log2_weight_denom: u32,
    /// `chroma_log2_weight_denom`. Present iff stream isn't monochrome.
    pub chroma_log2_weight_denom: Option<u32>,
    /// Per-list weights for L0.
    pub weights_l0: Vec<WeightEntry>,
    /// Per-list weights for L1 (B-slices only; empty for P).
    pub weights_l1: Vec<WeightEntry>,
}

/// One memory management control operation from
/// `dec_ref_pic_marking()` (non-IDR adaptive path), H.264 §7.3.3.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmcoOp {
    /// MMCO 1: mark short-term picture indicated by
    /// `difference_of_pic_nums_minus1` as unused for reference.
    ShortTermUnused {
        /// `difference_of_pic_nums_minus1`.
        difference_of_pic_nums_minus1: u32,
    },
    /// MMCO 2: mark long-term picture as unused.
    LongTermUnused {
        /// `long_term_pic_num`.
        long_term_pic_num: u32,
    },
    /// MMCO 3: assign a long-term frame index to a short-term picture.
    AssignLongTerm {
        /// `difference_of_pic_nums_minus1`.
        difference_of_pic_nums_minus1: u32,
        /// `long_term_frame_idx`.
        long_term_frame_idx: u32,
    },
    /// MMCO 4: update `MaxLongTermFrameIdx`.
    MaxLongTermIdxPlus1 {
        /// `max_long_term_frame_idx_plus1`.
        max_long_term_frame_idx_plus1: u32,
    },
    /// MMCO 5: mark all reference pictures as unused.
    AllUnused,
    /// MMCO 6: assign long-term frame index to current picture.
    AssignCurrentLongTerm {
        /// `long_term_frame_idx`.
        long_term_frame_idx: u32,
    },
}

/// `dec_ref_pic_marking()` payload, broken out by IDR vs adaptive case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecRefPicMarking {
    /// IDR slice: two flags signal output suppression and whether the
    /// IDR picture should immediately become a long-term reference.
    Idr {
        /// `no_output_of_prior_pics_flag`.
        no_output_of_prior_pics: bool,
        /// `long_term_reference_flag`.
        long_term_reference: bool,
    },
    /// Non-IDR slice with no adaptive control — DPB sliding-window
    /// behaviour applies (`adaptive_ref_pic_marking_mode_flag == 0`).
    SlidingWindow,
    /// Non-IDR adaptive control: a sequence of MMCO operations
    /// terminated by an implicit MMCO 0 (not stored here).
    Adaptive {
        /// MMCO operations in spec order.
        ops: Vec<MmcoOp>,
    },
}

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
    /// Reference picture list modification for L0 (P / SP / B slices).
    pub ref_pic_list_modification_l0: RefPicListModification,
    /// Reference picture list modification for L1 (B slices only).
    pub ref_pic_list_modification_l1: RefPicListModification,
    /// Prediction weight table, when the PPS / slice combination
    /// signalled one.
    pub pred_weight_table: Option<PredWeightTable>,
    /// Decoded reference picture marking commands. Present iff the
    /// enclosing NAL unit was a reference (`nal_ref_idc != 0`).
    pub dec_ref_pic_marking: Option<DecRefPicMarking>,
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

    let ref_pic_list_modification_l0 = if !slice_type.is_intra() {
        read_ref_pic_list_modification(&mut r)?
    } else {
        RefPicListModification::default()
    };
    let ref_pic_list_modification_l1 = if slice_type.is_bi_predictive() {
        read_ref_pic_list_modification(&mut r)?
    } else {
        RefPicListModification::default()
    };

    let pred_weight_table = if (pps.weighted_pred_flag
        && matches!(slice_type, SliceType::P | SliceType::SP))
        || (pps.weighted_bipred_idc == 1 && slice_type.is_bi_predictive())
    {
        Some(read_pred_weight_table(
            &mut r,
            sps,
            num_ref_idx_l0,
            num_ref_idx_l1,
            slice_type.is_bi_predictive(),
        )?)
    } else {
        None
    };

    let dec_ref_pic_marking = if ctx.nal_ref_idc != 0 {
        Some(read_dec_ref_pic_marking(&mut r, ctx.is_idr)?)
    } else {
        None
    };

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
        ref_pic_list_modification_l0,
        ref_pic_list_modification_l1,
        pred_weight_table,
        dec_ref_pic_marking,
    })
}

fn read_ref_pic_list_modification(
    r: &mut BitReader<'_>,
) -> Result<RefPicListModification, CodecError> {
    let present = r.read_bit()?;
    let mut ops = Vec::new();
    if present {
        loop {
            let op = r.read_ue()?;
            match op {
                0 => ops.push(RefPicListModOp::SubtractAbsDiffPicNum(r.read_ue()?)),
                1 => ops.push(RefPicListModOp::AddAbsDiffPicNum(r.read_ue()?)),
                2 => ops.push(RefPicListModOp::LongTermPicNum(r.read_ue()?)),
                3 => break,
                _ => {
                    return Err(CodecError::InvalidData(format!(
                        "h264 slice_header: invalid ref_pic_list_modification op {op}"
                    )));
                }
            }
        }
    }
    Ok(RefPicListModification { present, ops })
}

fn read_pred_weight_table(
    r: &mut BitReader<'_>,
    sps: &SpsRbsp,
    num_ref_l0: u32,
    num_ref_l1: u32,
    is_bi: bool,
) -> Result<PredWeightTable, CodecError> {
    let luma_log2_weight_denom = r.read_ue()?;
    let chroma_log2_weight_denom = if sps.chroma_format_idc != 0 {
        Some(r.read_ue()?)
    } else {
        None
    };
    let weights_l0 = read_weight_list(r, sps, num_ref_l0)?;
    let weights_l1 = if is_bi {
        read_weight_list(r, sps, num_ref_l1)?
    } else {
        Vec::new()
    };
    Ok(PredWeightTable {
        luma_log2_weight_denom,
        chroma_log2_weight_denom,
        weights_l0,
        weights_l1,
    })
}

fn read_weight_list(
    r: &mut BitReader<'_>,
    sps: &SpsRbsp,
    num_ref_minus1: u32,
) -> Result<Vec<WeightEntry>, CodecError> {
    let mut entries = Vec::with_capacity((num_ref_minus1 as usize).saturating_add(1));
    for _ in 0..=num_ref_minus1 {
        let mut entry = WeightEntry::default();
        let luma_flag = r.read_bit()?;
        if luma_flag {
            let w = r.read_se()?;
            let o = r.read_se()?;
            entry.luma = Some((w, o));
        }
        if sps.chroma_format_idc != 0 {
            let chroma_flag = r.read_bit()?;
            if chroma_flag {
                let cb = (r.read_se()?, r.read_se()?);
                let cr = (r.read_se()?, r.read_se()?);
                entry.chroma = Some([cb, cr]);
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn read_dec_ref_pic_marking(
    r: &mut BitReader<'_>,
    is_idr: bool,
) -> Result<DecRefPicMarking, CodecError> {
    if is_idr {
        let no_output_of_prior_pics = r.read_bit()?;
        let long_term_reference = r.read_bit()?;
        return Ok(DecRefPicMarking::Idr {
            no_output_of_prior_pics,
            long_term_reference,
        });
    }
    let adaptive = r.read_bit()?;
    if !adaptive {
        return Ok(DecRefPicMarking::SlidingWindow);
    }
    let mut ops = Vec::new();
    loop {
        let op = r.read_ue()?;
        match op {
            0 => break,
            1 => ops.push(MmcoOp::ShortTermUnused {
                difference_of_pic_nums_minus1: r.read_ue()?,
            }),
            2 => ops.push(MmcoOp::LongTermUnused {
                long_term_pic_num: r.read_ue()?,
            }),
            3 => {
                let diff = r.read_ue()?;
                let idx = r.read_ue()?;
                ops.push(MmcoOp::AssignLongTerm {
                    difference_of_pic_nums_minus1: diff,
                    long_term_frame_idx: idx,
                });
            }
            4 => ops.push(MmcoOp::MaxLongTermIdxPlus1 {
                max_long_term_frame_idx_plus1: r.read_ue()?,
            }),
            5 => ops.push(MmcoOp::AllUnused),
            6 => ops.push(MmcoOp::AssignCurrentLongTerm {
                long_term_frame_idx: r.read_ue()?,
            }),
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 slice_header: invalid memory_management_control op {op}"
                )));
            }
        }
    }
    Ok(DecRefPicMarking::Adaptive { ops })
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
            vui: None,
            scaling_lists: None,
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
            scaling_lists: None,
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
        // Intra slice: no ref-list modification, no pred weight table.
        assert!(!sh.ref_pic_list_modification_l0.present);
        assert!(sh.ref_pic_list_modification_l1.ops.is_empty());
        assert!(sh.pred_weight_table.is_none());
        // IDR + nal_ref_idc != 0: marking is the Idr variant.
        match sh.dec_ref_pic_marking {
            Some(DecRefPicMarking::Idr {
                no_output_of_prior_pics,
                long_term_reference,
            }) => {
                assert!(!no_output_of_prior_pics);
                assert!(!long_term_reference);
            }
            other => panic!("expected Idr marking, got {other:?}"),
        }
    }

    #[test]
    fn retains_p_slice_ref_pic_list_modification_and_mmco() {
        // Build a P-slice with:
        //   first_mb_in_slice = 0
        //   slice_type = 0 (P)
        //   pic_parameter_set_id = 0
        //   frame_num = 5
        //   pic_order_cnt_lsb = 5
        //   num_ref_idx_active_override_flag = 0
        //   ref_pic_list_modification_l0:
        //     present = 1
        //     op codes: 0 (subtract), abs_diff_pic_num_minus1 = 2
        //               3 (end)
        //   dec_ref_pic_marking (non-IDR, adaptive):
        //     adaptive = 1
        //     MMCO 1 with difference_of_pic_nums_minus1 = 3
        //     MMCO 0 (end)
        //   slice_qp_delta = 0
        //   RBSP stop bit
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // first_mb_in_slice
        push_ue(&mut bits, 0); // slice_type = P
        push_ue(&mut bits, 0); // pic_parameter_set_id
        push_bits_msb(&mut bits, 5, 4); // frame_num
        push_bits_msb(&mut bits, 5, 4); // pic_order_cnt_lsb
        // num_ref_idx_active_override_flag = 0
        bits.push(false);
        // ref_pic_list_modification flag = 1
        bits.push(true);
        push_ue(&mut bits, 0); // op = 0 (Subtract)
        push_ue(&mut bits, 2); // abs_diff_pic_num_minus1 = 2
        push_ue(&mut bits, 3); // end op
        // adaptive_ref_pic_marking_mode_flag = 1
        bits.push(true);
        push_ue(&mut bits, 1); // MMCO 1
        push_ue(&mut bits, 3); // difference_of_pic_nums_minus1
        push_ue(&mut bits, 0); // MMCO 0 (end)
        push_se(&mut bits, 0); // slice_qp_delta
        bits.push(true); // stop bit
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let rbsp = pack_bits_msb(&bits);
        let sps = baseline_sps();
        let pps = baseline_pps();
        let ctx = NalContext {
            nal_ref_idc: 2,
            is_idr: false,
        };
        let sh = parse_slice_header(&rbsp, &sps, &pps, ctx).expect("should parse");
        assert_eq!(sh.slice_type, SliceType::P);
        assert_eq!(sh.frame_num, 5);
        assert!(sh.ref_pic_list_modification_l0.present);
        assert_eq!(
            sh.ref_pic_list_modification_l0.ops,
            vec![RefPicListModOp::SubtractAbsDiffPicNum(2)],
        );
        match sh.dec_ref_pic_marking {
            Some(DecRefPicMarking::Adaptive { ops }) => {
                assert_eq!(
                    ops,
                    vec![MmcoOp::ShortTermUnused {
                        difference_of_pic_nums_minus1: 3
                    }]
                );
            }
            other => panic!("expected Adaptive marking, got {other:?}"),
        }
    }

    #[test]
    fn sliding_window_marking_when_non_idr_and_no_adaptive() {
        // Non-IDR I-slice with nal_ref_idc != 0 and adaptive flag = 0.
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // first_mb_in_slice
        push_ue(&mut bits, 2); // slice_type = I
        push_ue(&mut bits, 0); // pic_parameter_set_id
        push_bits_msb(&mut bits, 1, 4); // frame_num
        push_bits_msb(&mut bits, 1, 4); // pic_order_cnt_lsb
        // adaptive_ref_pic_marking_mode_flag = 0 -> sliding window
        bits.push(false);
        push_se(&mut bits, 0); // slice_qp_delta
        bits.push(true); // stop bit
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let rbsp = pack_bits_msb(&bits);
        let sps = baseline_sps();
        let pps = baseline_pps();
        let ctx = NalContext {
            nal_ref_idc: 1,
            is_idr: false,
        };
        let sh = parse_slice_header(&rbsp, &sps, &pps, ctx).expect("should parse");
        assert!(matches!(
            sh.dec_ref_pic_marking,
            Some(DecRefPicMarking::SlidingWindow)
        ));
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

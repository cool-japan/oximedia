//! CAVLC slice-level decoder.
//!
//! Walks every macroblock in a CAVLC-coded P (or B) slice in
//! raster order.  Mirrors the API shape of
//! [`crate::h264::slice_cabac::parse_slice_cabac`] but uses the
//! CAVLC entropy path:
//!
//! - `mb_skip_run` ue(v) at the start of each P/B slice picks up
//!   runs of consecutive P_Skip / B_Skip macroblocks before any
//!   `mb_type` is signalled (spec § 7.3.4 step "if(slice_type !=
//!   I && slice_type != SI)").
//! - For each non-skipped macroblock, the existing
//!   [`crate::h264::macroblock::parse_macroblock_layer`] decodes
//!   `mb_type` + intra/motion info + CBP + qp_delta.
//! - Residual blocks are then decoded via the CAVLC residual
//!   primitives ([`crate::h264::cavlc::read_residual_block`]).
//!
//! ## Scope of this commit
//!
//! Inter P-slice macroblocks of type `P_L0_16x16`, `P_L0_L0_16x8`,
//! `P_L0_L0_8x16` and the `P_Skip` short-circuit return a fully
//! populated [`MbCavlcDecoded`].  Other inter P partition shapes
//! (`P_8x8`, `P_8x8ref0`) and any B-slice paths produce an entry
//! with `kind = Unsupported` so the slice still walks to
//! completion and the caller can render a placeholder.  B-slice
//! handling lands in a follow-up.

use crate::h264::bit_reader::BitReader;
use crate::h264::cavlc::{read_residual_block, BlockKind, ResidualBlock};
use crate::h264::macroblock::{parse_macroblock_layer, InterMotionInfo, MacroblockLayer, MbType};
use crate::h264::pps::PpsRbsp;
use crate::h264::slice_header::{SliceHeader, SliceType};
use crate::h264::sps::SpsRbsp;
use crate::CodecError;

/// Per-macroblock decoded data produced by [`parse_slice_cavlc`].
#[derive(Debug, Clone)]
pub struct MbCavlcDecoded {
    /// Macroblock column.
    pub mb_x: usize,
    /// Macroblock row.
    pub mb_y: usize,
    /// Kind of macroblock plus its parsed syntax / residual.
    pub kind: MbCavlcKind,
    /// Effective luma QP at this macroblock.
    pub qp_y: u8,
    /// Effective chroma QP at this macroblock.
    pub qp_chroma: u8,
}

/// Macroblock kind discriminator for the CAVLC slice walker.
#[derive(Debug, Clone)]
pub enum MbCavlcKind {
    /// P_Skip / B_Skip: inferred motion, no syntax beyond the
    /// `mb_skip_run` counter.
    Skip,
    /// Intra-coded (I_NxN / I_16x16 / I_PCM via the parser stack).
    Intra {
        /// The parsed macroblock layer.
        layer: MacroblockLayer,
        /// Luma residual blocks in scan order (one per 4×4 sub-MB).
        luma_blocks: [Option<ResidualBlock>; 16],
        /// Chroma DC blocks (Cb, Cr) when cbp_chroma >= 1.
        chroma_dc: [Option<ResidualBlock>; 2],
        /// Chroma AC blocks (4 Cb + 4 Cr) when cbp_chroma == 2.
        chroma_ac: [Option<ResidualBlock>; 8],
    },
    /// Inter P macroblock (16×16 / 16×8 / 8×16 / 8×8 partition
    /// shapes — recognised but residual decode is wired only for
    /// 16×16 in this commit).
    InterP {
        /// Parsed motion info.
        motion: InterMotionInfo,
        /// 4-bit luma + 2-bit chroma CBP.
        cbp: u8,
        /// Per-4×4 luma residual blocks (16 entries in scan order).
        luma_blocks: [Option<ResidualBlock>; 16],
        /// Chroma DC blocks per plane.
        chroma_dc: [Option<ResidualBlock>; 2],
        /// Chroma AC blocks (4 Cb + 4 Cr).
        chroma_ac: [Option<ResidualBlock>; 8],
        /// 16×16-only flag — true for `P_L0_16x16`, false for
        /// 16×8 / 8×16 / 8×8.  The reconstruction wiring in
        /// `pipeline.rs` only handles the 16×16 case.
        is_16x16: bool,
    },
    /// Recognised but not yet wired through CAVLC reconstruction.
    Unsupported,
}

/// Parses one CAVLC-coded slice from the bit-reader's current
/// position (immediately after the slice header).
///
/// `reader`'s state is consumed in place — the caller can inspect
/// `reader.bits_consumed()` afterwards to find any trailing data.
///
/// # Errors
///
/// Bubbles up parser errors from `parse_macroblock_layer` /
/// `read_residual_block`.  Returns [`CodecError::InvalidData`]
/// when the slice type is unsupported by this iteration
/// (B-slice).
pub fn parse_slice_cavlc(
    reader: &mut BitReader<'_>,
    sps: &SpsRbsp,
    pps: &PpsRbsp,
    sh: &SliceHeader,
) -> Result<Vec<MbCavlcDecoded>, CodecError> {
    if pps.entropy_coding_mode_flag {
        return Err(CodecError::InvalidData(
            "h264 slice_cavlc: PPS signals CABAC".into(),
        ));
    }
    if !matches!(sh.slice_type, SliceType::I | SliceType::SI | SliceType::P) {
        return Err(CodecError::InvalidData(format!(
            "h264 slice_cavlc: slice type {:?} not yet supported",
            sh.slice_type
        )));
    }

    let pic_width_mbs = sps.pic_width_in_mbs_minus1 as usize + 1;
    let pic_height_mbs = sps.pic_height_in_map_units_minus1 as usize + 1;
    let total = pic_width_mbs * pic_height_mbs;

    let qp_y_initial =
        (pps.pic_init_qp_minus26 + 26 + sh.slice_qp_delta).clamp(0, 51) as u8;
    let mut qp_y = qp_y_initial;
    let mut out = Vec::with_capacity(total);
    let mut mb_idx = sh.first_mb_in_slice as usize;
    let p_slice = matches!(sh.slice_type, SliceType::P);
    let mut mb_skip_run = if p_slice { reader.read_ue()? as usize } else { 0 };

    while mb_idx < total {
        let mb_x = mb_idx % pic_width_mbs;
        let mb_y = mb_idx / pic_width_mbs;

        if mb_skip_run > 0 {
            out.push(MbCavlcDecoded {
                mb_x,
                mb_y,
                kind: MbCavlcKind::Skip,
                qp_y,
                qp_chroma: qp_y,
            });
            mb_skip_run -= 1;
            mb_idx += 1;
            if mb_skip_run == 0 && p_slice && mb_idx < total {
                mb_skip_run = reader.read_ue()? as usize;
            }
            continue;
        }

        let layer = parse_macroblock_layer(reader, sps, pps, sh.slice_type)?;
        qp_y = clip_qp(qp_y as i32 + layer.mb_qp_delta) as u8;
        let qp_chroma = chroma_qp(qp_y, pps.chroma_qp_index_offset);

        let cbp_luma = layer.cbp_luma();
        let cbp_chroma = layer.cbp_chroma();

        // Decode residual blocks unconditionally for any MB whose
        // CBP signals at least one nonzero block, or for I_16x16
        // which always has a DC + AC layout.
        let mut luma_blocks: [Option<ResidualBlock>; 16] = Default::default();
        let mut chroma_dc: [Option<ResidualBlock>; 2] = Default::default();
        let mut chroma_ac: [Option<ResidualBlock>; 8] = Default::default();

        let needs_residual = cbp_luma != 0
            || cbp_chroma != 0
            || matches!(layer.mb_type, MbType::I16x16 { .. });

        if needs_residual {
            // Per-8x8 quadrant gating: each bit of cbp_luma covers
            // one 8x8 quadrant.  For I_16x16 we decode all 16
            // unconditionally.
            let force_luma = matches!(layer.mb_type, MbType::I16x16 { .. });
            for i8x8 in 0..4 {
                let active = force_luma || (cbp_luma & (1 << i8x8) != 0);
                for i4x4 in 0..4 {
                    let block_idx = 4 * i8x8 + i4x4;
                    if active {
                        let blk = read_residual_block(reader, BlockKind::Luma4x4, 0)?;
                        if blk.total_coeff > 0 {
                            luma_blocks[block_idx] = Some(blk);
                        }
                    }
                }
            }

            if cbp_chroma >= 1 {
                // 4:2:0 chroma DC: 4 entries per plane decoded as
                // a single residual block with the chroma DC scan.
                for plane in 0..2 {
                    let blk = read_residual_block(reader, BlockKind::ChromaDc, 0)?;
                    if blk.total_coeff > 0 {
                        chroma_dc[plane] = Some(blk);
                    }
                }
            }
            if cbp_chroma == 2 {
                for plane in 0..2 {
                    for i in 0..4 {
                        let block_idx = 4 * plane + i;
                        let blk = read_residual_block(reader, BlockKind::ChromaAc, 0)?;
                        if blk.total_coeff > 0 {
                            chroma_ac[block_idx] = Some(blk);
                        }
                    }
                }
            }
        }

        let kind = match layer.mb_type {
            MbType::PL0_16x16 | MbType::PL0L0_16x8 | MbType::PL0L0_8x16 => {
                let is_16x16 = matches!(layer.mb_type, MbType::PL0_16x16);
                let motion = layer.motion.clone().unwrap_or_default();
                MbCavlcKind::InterP {
                    motion,
                    cbp: layer.coded_block_pattern,
                    luma_blocks,
                    chroma_dc,
                    chroma_ac,
                    is_16x16,
                }
            }
            MbType::INxN | MbType::I16x16 { .. } => MbCavlcKind::Intra {
                layer,
                luma_blocks,
                chroma_dc,
                chroma_ac,
            },
            MbType::PSkip => MbCavlcKind::Skip,
            _ => MbCavlcKind::Unsupported,
        };
        out.push(MbCavlcDecoded { mb_x, mb_y, kind, qp_y, qp_chroma });
        mb_idx += 1;

        if p_slice && mb_idx < total {
            mb_skip_run = reader.read_ue()? as usize;
        }
    }

    Ok(out)
}

fn chroma_qp(qp_y: u8, chroma_offset: i32) -> u8 {
    const TABLE: [u8; 52] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38,
        39, 39, 39, 39,
    ];
    let qp = (qp_y as i32 + chroma_offset).clamp(0, 51) as usize;
    TABLE[qp]
}

fn clip_qp(qp: i32) -> i32 {
    qp.rem_euclid(52)
}

/// Position-order layout of a 4×4 block after applying the
/// standard zig-zag scan to the CAVLC `ResidualBlock`'s
/// `to_scan_order` output.  Suitable input for
/// [`crate::h264::transform::dequant_and_inverse_transform_4x4_pos`].
#[must_use]
pub fn cavlc_block_to_position_4x4(block: &ResidualBlock) -> [i32; 16] {
    let scan = block.to_scan_order();
    const ZIGZAG_4X4: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
    let mut out = [0i32; 16];
    for (i, &val) in scan.iter().enumerate().take(16) {
        out[ZIGZAG_4X4[i]] = val;
    }
    out
}

/// Position-order layout for the 4 entries of a 4:2:0 chroma DC
/// block.  Chroma DC scan is the identity [0, 1, 2, 3] in raster
/// inside a 2×2 layout.
#[must_use]
pub fn cavlc_chroma_dc_to_position(block: &ResidualBlock) -> [i32; 4] {
    let scan = block.to_scan_order();
    let mut out = [0i32; 4];
    for (i, &val) in scan.iter().enumerate().take(4) {
        out[i] = val;
    }
    out
}

/// Position-order layout for a chroma 4×4 AC block (15 entries
/// starting at scan position 1 — DC slot is filled separately by
/// the inverse-Hadamard chroma DC).
#[must_use]
pub fn cavlc_chroma_ac_to_position(block: &ResidualBlock) -> [i32; 16] {
    let scan = block.to_scan_order();
    const ZIGZAG_AC: [usize; 15] =
        [1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
    let mut out = [0i32; 16];
    for (i, &val) in scan.iter().enumerate().take(15) {
        out[ZIGZAG_AC[i]] = val;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::slice_header::SliceType;

    fn dummy_sps() -> SpsRbsp {
        SpsRbsp {
            profile_idc: 66,
            constraint_set_flags: 0,
            level_idc: 10,
            seq_parameter_set_id: 0,
            chroma_format_idc: 1,
            separate_colour_plane_flag: false,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            qpprime_y_zero_transform_bypass_flag: false,
            seq_scaling_matrix_present_flag: false,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 2,
            log2_max_pic_order_cnt_lsb_minus4: 0,
            delta_pic_order_always_zero_flag: false,
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            num_ref_frames: 1,
            gaps_in_frame_num_value_allowed_flag: false,
            pic_width_in_mbs_minus1: 0,
            pic_height_in_map_units_minus1: 0,
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

    fn dummy_pps() -> PpsRbsp {
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
            pic_init_qp_minus26: 0,
            pic_init_qs_minus26: 0,
            chroma_qp_index_offset: 0,
            deblocking_filter_control_present_flag: false,
            constrained_intra_pred_flag: false,
            redundant_pic_cnt_present_flag: false,
            transform_8x8_mode_flag: false,
            pic_scaling_matrix_present_flag: false,
            scaling_lists: None,
            second_chroma_qp_index_offset: 0,
        }
    }

    fn dummy_sh(slice_type: SliceType) -> SliceHeader {
        SliceHeader {
            first_mb_in_slice: 0,
            slice_type,
            pic_parameter_set_id: 0,
            colour_plane_id: None,
            frame_num: 0,
            field_pic_flag: false,
            bottom_field_flag: false,
            idr_pic_id: Some(0),
            pic_order_cnt_lsb: None,
            delta_pic_order_cnt_bottom: None,
            delta_pic_order_cnt: None,
            redundant_pic_cnt: None,
            direct_spatial_mv_pred_flag: None,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            slice_qp_delta: 0,
            slice_qp_y: 26,
            ref_pic_list_modification_l0: Default::default(),
            ref_pic_list_modification_l1: Default::default(),
            pred_weight_table: None,
            dec_ref_pic_marking: None,
        }
    }

    #[test]
    fn rejects_b_slice() {
        let sps = dummy_sps();
        let pps = dummy_pps();
        let sh = dummy_sh(SliceType::B);
        let bytes = [0u8; 4];
        let mut reader = BitReader::new(&bytes);
        assert!(parse_slice_cavlc(&mut reader, &sps, &pps, &sh).is_err());
    }

    #[test]
    fn rejects_cabac_pps() {
        let sps = dummy_sps();
        let mut pps = dummy_pps();
        pps.entropy_coding_mode_flag = true;
        let sh = dummy_sh(SliceType::P);
        let bytes = [0u8; 4];
        let mut reader = BitReader::new(&bytes);
        assert!(parse_slice_cavlc(&mut reader, &sps, &pps, &sh).is_err());
    }
}

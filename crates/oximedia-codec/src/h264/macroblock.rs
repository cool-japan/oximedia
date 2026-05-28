//! H.264 macroblock-layer syntax (§7.3.4).
//!
//! Parses a single macroblock's syntax elements out of the bitstream:
//! `mb_type` (with its slice-type-dependent interpretation), per-block
//! intra prediction mode signaling, sub-macroblock partition layout for
//! `P_8x8` / `B_8x8`, motion vector differences and reference indices
//! for inter macroblocks, the coded block pattern, and `mb_qp_delta`.
//!
//! ## Scope (phase 4a)
//!
//! - Full parsing for **I-slice** macroblocks (`I_NxN`, all 24 flavours
//!   of `I_16x16`, `I_PCM`).
//! - Full parsing for **P-slice** macroblocks including the
//!   `P_8x8` / `P_8x8ref0` sub-partition path.
//! - **B-slice** `mb_type` is recognised at the table-mapping level
//!   but reading the full B-macroblock body (with its much larger sub-
//!   partition zoo and bi-prediction MVDs) returns a "not yet
//!   implemented" `CodecError`. B-frame decode is phase 4l.
//! - **Residual coefficient blocks** are not parsed here.  The CAVLC
//!   entropy decoder is phase 4b; this module stops after
//!   `mb_qp_delta` and leaves the bit reader positioned at the start
//!   of `residual_block()`.
//! - **CABAC** entropy coding is not supported here.  When
//!   `entropy_coding_mode_flag == 1`, this module returns an error.
//!   CABAC is phase 4k.

use crate::h264::bit_reader::BitReader;
use crate::h264::pps::PpsRbsp;
use crate::h264::slice_header::SliceType;
use crate::h264::sps::SpsRbsp;
use crate::CodecError;

/// Intra-16x16 prediction direction (4 variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intra16x16PredMode {
    /// Predict each column from the top row.
    Vertical,
    /// Predict each row from the left column.
    Horizontal,
    /// Predict every pixel from the DC of available neighbours.
    Dc,
    /// Plane: fit a 2D plane through the corner samples.
    Plane,
}

impl Intra16x16PredMode {
    fn from_raw(v: u32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::Vertical,
            1 => Self::Horizontal,
            2 => Self::Dc,
            3 => Self::Plane,
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 macroblock: invalid Intra16x16 prediction mode {v}"
                )))
            }
        })
    }
}

/// Chroma intra prediction mode (4 variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntraChromaPredMode {
    /// DC prediction from neighbours.
    Dc,
    /// Horizontal (replicate left column).
    Horizontal,
    /// Vertical (replicate top row).
    Vertical,
    /// Plane: 2D plane fit.
    Plane,
}

impl IntraChromaPredMode {
    fn from_raw(v: u32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::Dc,
            1 => Self::Horizontal,
            2 => Self::Vertical,
            3 => Self::Plane,
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 macroblock: invalid intra_chroma_pred_mode {v}"
                )))
            }
        })
    }
}

/// Sub-macroblock partition for P_8x8 (§7.3.5.2 P-sub-mb mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubMbType {
    /// `P_L0_8x8` — single 8×8 motion vector.
    PL0_8x8,
    /// `P_L0_8x4` — two 8×4 motion vectors stacked vertically.
    PL0_8x4,
    /// `P_L0_4x8` — two 4×8 motion vectors side-by-side.
    PL0_4x8,
    /// `P_L0_4x4` — four 4×4 motion vectors.
    PL0_4x4,
}

impl SubMbType {
    fn from_p_raw(v: u32) -> Result<Self, CodecError> {
        Ok(match v {
            0 => Self::PL0_8x8,
            1 => Self::PL0_8x4,
            2 => Self::PL0_4x8,
            3 => Self::PL0_4x4,
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 macroblock: invalid P sub_mb_type {v}"
                )))
            }
        })
    }

    /// Number of independent motion vectors carried by this sub-MB
    /// type.  Used by the MVD reader.
    #[must_use]
    pub fn num_partitions(self) -> usize {
        match self {
            Self::PL0_8x8 => 1,
            Self::PL0_8x4 | Self::PL0_4x8 => 2,
            Self::PL0_4x4 => 4,
        }
    }
}

/// Macroblock type, normalised across slice types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbType {
    /// `I_NxN` — 4×4 or 8×8 intra prediction per sub-block (the
    /// `transform_size_8x8_flag` selects).
    INxN,
    /// `I_16x16` with one prediction direction and a CBP pair.
    I16x16 {
        /// Intra-16x16 prediction direction.
        pred_mode: Intra16x16PredMode,
        /// Luma CBP: 0 (no residual) or 15 (all four 8x8 blocks).
        cbp_luma: u8,
        /// Chroma CBP code: 0 (none), 1 (DC only), 2 (DC + AC).
        cbp_chroma: u8,
    },
    /// `I_PCM` — raw PCM macroblock (no transform / prediction).
    IPcm,
    /// `P_L0_16x16`.
    PL0_16x16,
    /// `P_L0_L0_16x8`.
    PL0L0_16x8,
    /// `P_L0_L0_8x16`.
    PL0L0_8x16,
    /// `P_8x8` — four 8x8 sub-partitions, each with its own sub_mb_type.
    P8x8,
    /// `P_8x8ref0` — variant of P_8x8 where all partitions use ref_idx 0.
    P8x8Ref0,
    /// Inferred skipped P macroblock.
    PSkip,
    /// Recognised but not fully decodable in phase 4a; the body is not
    /// parsed.  Carries the raw `mb_type` value from the bitstream so a
    /// later phase can dispatch.
    BSliceRaw(u32),
}

impl MbType {
    /// True for any intra macroblock.
    #[must_use]
    pub fn is_intra(self) -> bool {
        matches!(self, Self::INxN | Self::I16x16 { .. } | Self::IPcm)
    }

    /// True for any inter (motion-compensated) macroblock.
    #[must_use]
    pub fn is_inter(self) -> bool {
        matches!(
            self,
            Self::PL0_16x16
                | Self::PL0L0_16x8
                | Self::PL0L0_8x16
                | Self::P8x8
                | Self::P8x8Ref0
                | Self::PSkip
        ) || matches!(self, Self::BSliceRaw(_))
    }

    /// Number of motion partitions carried at the MB level.  For
    /// `P_8x8` the answer is 4 (each gets its own sub_mb_type).  For
    /// 16x8 / 8x16 it is 2.
    #[must_use]
    pub fn num_mb_partitions(self) -> usize {
        match self {
            Self::PL0_16x16 => 1,
            Self::PL0L0_16x8 | Self::PL0L0_8x16 => 2,
            Self::P8x8 | Self::P8x8Ref0 => 4,
            _ => 0,
        }
    }
}

/// Intra prediction mode signalling for an `I_NxN` macroblock.
#[derive(Debug, Clone, Default)]
pub struct IntraNxNPredInfo {
    /// `transform_size_8x8_flag` — when true, the macroblock uses
    /// 8x8 intra blocks; when false, 4x4.
    pub transform_size_8x8_flag: bool,
    /// `prev_intra4x4_pred_mode_flag` per 4×4 block (16 entries when
    /// `transform_size_8x8_flag` is false).
    pub prev_intra4x4_pred_mode_flag: Vec<bool>,
    /// `rem_intra4x4_pred_mode` value (0..=8) per 4×4 block where the
    /// corresponding flag was false.  Indexed in the same order as
    /// `prev_intra4x4_pred_mode_flag` with `None` for entries where
    /// the flag was true (use the most-probable mode instead).
    pub rem_intra4x4_pred_mode: Vec<Option<u8>>,
    /// `prev_intra8x8_pred_mode_flag` per 8×8 block (4 entries when
    /// `transform_size_8x8_flag` is true).
    pub prev_intra8x8_pred_mode_flag: Vec<bool>,
    /// `rem_intra8x8_pred_mode` value (0..=8) per 8×8 block.
    pub rem_intra8x8_pred_mode: Vec<Option<u8>>,
}

/// Motion vector difference value pair `(dx, dy)`.
pub type MotionVectorDelta = (i32, i32);

/// Motion-info payload for one inter macroblock.
#[derive(Debug, Clone, Default)]
pub struct InterMotionInfo {
    /// Reference index for each L0 partition.  Length matches the
    /// `mb_type`'s `num_mb_partitions()` (or four entries for
    /// `P_8x8` flattened from sub-partitions).
    pub ref_idx_l0: Vec<u32>,
    /// Reference index for each L1 partition (B-slice only; empty in
    /// phase 4a).
    pub ref_idx_l1: Vec<u32>,
    /// Motion vector deltas for L0 partitions.  Length depends on
    /// `mb_type` plus any sub-MB partitioning under `P_8x8`.
    pub mvd_l0: Vec<MotionVectorDelta>,
    /// Motion vector deltas for L1 partitions.
    pub mvd_l1: Vec<MotionVectorDelta>,
    /// `sub_mb_type` per 8x8 partition when the macroblock is
    /// `P_8x8` / `P_8x8ref0`.
    pub sub_mb_types: Vec<SubMbType>,
}

/// Fully parsed macroblock-layer syntax (phase 4a scope).
#[derive(Debug, Clone)]
pub struct MacroblockLayer {
    /// Normalised macroblock type.
    pub mb_type: MbType,
    /// Intra prediction info, present when `mb_type` is `I_NxN`.
    pub intra_nxn: Option<IntraNxNPredInfo>,
    /// Chroma intra prediction mode, present for any intra macroblock
    /// when the slice's chroma format is not monochrome.
    pub intra_chroma_pred_mode: Option<IntraChromaPredMode>,
    /// Motion info, present for inter macroblocks.
    pub motion: Option<InterMotionInfo>,
    /// Coded block pattern.  Encoded form is the spec's mapped
    /// exp-Golomb `me(v)` over Table 9-4 — this is the decoded 6-bit
    /// value `(cbp_chroma << 4) | cbp_luma_4x4_pattern`.
    pub coded_block_pattern: u8,
    /// `mb_qp_delta`.  Present iff `coded_block_pattern > 0` *or* the
    /// macroblock is `I_16x16` with any residual.
    pub mb_qp_delta: i32,
}

impl MacroblockLayer {
    /// Returns the 4-bit luma part of the coded block pattern.
    #[must_use]
    pub fn cbp_luma(&self) -> u8 {
        self.coded_block_pattern & 0x0F
    }

    /// Returns the 2-bit chroma part of the coded block pattern.
    #[must_use]
    pub fn cbp_chroma(&self) -> u8 {
        (self.coded_block_pattern >> 4) & 0x03
    }
}

/// Parses one macroblock layer at the bit reader's current position.
///
/// `entropy_coding_mode_flag` from the active PPS must be `false`
/// (CAVLC) — CABAC support arrives in phase 4k.  The reader is left
/// positioned at the start of `residual_block()`, which the caller
/// hands to the CAVLC entropy decoder (phase 4b) when implemented.
///
/// # Errors
///
/// - [`CodecError::InvalidData`] when the bitstream encodes an
///   `mb_type` value that's out of range for the current slice type.
/// - [`CodecError::InvalidData`] when called against a CABAC stream
///   (`entropy_coding_mode_flag == true`).
/// - [`CodecError::InvalidData`] when the macroblock is a B-slice
///   inter type (B-slice decode is phase 4l).
pub fn parse_macroblock_layer(
    r: &mut BitReader<'_>,
    sps: &SpsRbsp,
    pps: &PpsRbsp,
    slice_type: SliceType,
) -> Result<MacroblockLayer, CodecError> {
    if pps.entropy_coding_mode_flag {
        return Err(CodecError::InvalidData(
            "h264 macroblock: CABAC not yet supported (phase 4k)".into(),
        ));
    }

    let raw_mb_type = r.read_ue()?;
    let mb_type = decode_mb_type(raw_mb_type, slice_type)?;

    // I_PCM has its own short syntax — bit-align then read raw
    // samples.  Phase 4a recognises the type but defers the sample
    // read to a later phase that needs it (I_PCM is rare in
    // production streams and orthogonal to the main decode path).
    if matches!(mb_type, MbType::IPcm) {
        return Err(CodecError::InvalidData(
            "h264 macroblock: I_PCM body parse not yet supported".into(),
        ));
    }

    let mut intra_nxn: Option<IntraNxNPredInfo> = None;
    let mut intra_chroma_pred_mode: Option<IntraChromaPredMode> = None;
    let mut motion: Option<InterMotionInfo> = None;
    let chroma_present = sps.chroma_format_idc != 0;

    match mb_type {
        MbType::INxN => {
            intra_nxn = Some(read_intra_nxn_pred(r, pps)?);
            if chroma_present {
                intra_chroma_pred_mode = Some(IntraChromaPredMode::from_raw(r.read_ue()?)?);
            }
        }
        MbType::I16x16 { .. } => {
            // No per-block intra mode syntax for I_16x16: the mb_type
            // value itself carries the prediction direction.
            if chroma_present {
                intra_chroma_pred_mode = Some(IntraChromaPredMode::from_raw(r.read_ue()?)?);
            }
        }
        MbType::PL0_16x16 | MbType::PL0L0_16x8 | MbType::PL0L0_8x16 => {
            motion = Some(read_p_mb_motion(r, mb_type)?);
        }
        MbType::P8x8 | MbType::P8x8Ref0 => {
            motion = Some(read_p_8x8_motion(r, mb_type)?);
        }
        MbType::PSkip => {
            // Inferred skip; no further syntax.
            return Ok(MacroblockLayer {
                mb_type,
                intra_nxn: None,
                intra_chroma_pred_mode: None,
                motion: Some(InterMotionInfo::default()),
                coded_block_pattern: 0,
                mb_qp_delta: 0,
            });
        }
        MbType::BSliceRaw(_) => {
            return Err(CodecError::InvalidData(
                "h264 macroblock: B-slice decode is phase 4l".into(),
            ));
        }
        MbType::IPcm => unreachable!(),
    }

    // coded_block_pattern: me(v) — value depends on whether the MB is
    // intra or inter and on the chroma format.
    let (cbp, mb_qp_delta) = if matches!(mb_type, MbType::I16x16 { .. }) {
        // For I_16x16, the CBP is carried by mb_type itself.  mb_qp_delta
        // is always present.
        let (luma, chroma) = match mb_type {
            MbType::I16x16 {
                cbp_luma,
                cbp_chroma,
                ..
            } => (cbp_luma, cbp_chroma),
            _ => unreachable!(),
        };
        let cbp = (chroma << 4) | luma;
        let delta = r.read_se()?;
        (cbp, delta)
    } else {
        let me = r.read_ue()?;
        let cbp = decode_cbp(me, mb_type.is_intra(), sps.chroma_format_idc)?;
        let delta = if cbp != 0 { r.read_se()? } else { 0 };
        (cbp, delta)
    };

    // The PPS-level transform_8x8_mode_flag plus the presence of any
    // 8x8 luma residual triggers the transform_size_8x8_flag for
    // non-I_NxN intra macroblocks.  For I_NxN it was already read
    // inside `read_intra_nxn_pred` above.

    Ok(MacroblockLayer {
        mb_type,
        intra_nxn,
        intra_chroma_pred_mode,
        motion,
        coded_block_pattern: cbp,
        mb_qp_delta,
    })
}

fn decode_mb_type(raw: u32, slice_type: SliceType) -> Result<MbType, CodecError> {
    match slice_type {
        SliceType::I | SliceType::SI => decode_i_mb_type(raw),
        SliceType::P | SliceType::SP => {
            // P-slice mb_type space: 0..=4 are P types, 5..=30 are
            // I types offset by 5.
            if raw <= 4 {
                Ok(match raw {
                    0 => MbType::PL0_16x16,
                    1 => MbType::PL0L0_16x8,
                    2 => MbType::PL0L0_8x16,
                    3 => MbType::P8x8,
                    4 => MbType::P8x8Ref0,
                    _ => unreachable!(),
                })
            } else {
                decode_i_mb_type(raw - 5)
            }
        }
        SliceType::B => Ok(MbType::BSliceRaw(raw)),
    }
}

fn decode_i_mb_type(raw: u32) -> Result<MbType, CodecError> {
    match raw {
        0 => Ok(MbType::INxN),
        25 => Ok(MbType::IPcm),
        v if v <= 24 => {
            // I_16x16 mapping (H.264 Table 7-11):
            //   raw = 1 + Intra16x16PredMode
            //         + 4 * CodedBlockPatternChroma
            //         + 12 * CodedBlockPatternLuma
            let n = v - 1;
            let pred_mode = n % 4;
            let cbp_chroma = (n / 4) % 3;
            let cbp_luma_flag = n / 12;
            let cbp_luma = if cbp_luma_flag == 0 { 0 } else { 15 };
            Ok(MbType::I16x16 {
                pred_mode: Intra16x16PredMode::from_raw(pred_mode)?,
                cbp_luma,
                cbp_chroma: cbp_chroma as u8,
            })
        }
        _ => Err(CodecError::InvalidData(format!(
            "h264 macroblock: I-slice mb_type {raw} out of range"
        ))),
    }
}

fn read_intra_nxn_pred(
    r: &mut BitReader<'_>,
    pps: &PpsRbsp,
) -> Result<IntraNxNPredInfo, CodecError> {
    let transform_size_8x8_flag = if pps.transform_8x8_mode_flag {
        r.read_bit()?
    } else {
        false
    };

    let mut info = IntraNxNPredInfo {
        transform_size_8x8_flag,
        ..Default::default()
    };

    if transform_size_8x8_flag {
        info.prev_intra8x8_pred_mode_flag.reserve(4);
        info.rem_intra8x8_pred_mode.reserve(4);
        for _ in 0..4 {
            let flag = r.read_bit()?;
            info.prev_intra8x8_pred_mode_flag.push(flag);
            info.rem_intra8x8_pred_mode.push(if flag {
                None
            } else {
                Some(r.read_bits(3)? as u8)
            });
        }
    } else {
        info.prev_intra4x4_pred_mode_flag.reserve(16);
        info.rem_intra4x4_pred_mode.reserve(16);
        for _ in 0..16 {
            let flag = r.read_bit()?;
            info.prev_intra4x4_pred_mode_flag.push(flag);
            info.rem_intra4x4_pred_mode.push(if flag {
                None
            } else {
                Some(r.read_bits(3)? as u8)
            });
        }
    }

    Ok(info)
}

fn read_p_mb_motion(
    r: &mut BitReader<'_>,
    mb_type: MbType,
) -> Result<InterMotionInfo, CodecError> {
    let n_parts = mb_type.num_mb_partitions();
    let mut info = InterMotionInfo {
        ref_idx_l0: Vec::with_capacity(n_parts),
        mvd_l0: Vec::with_capacity(n_parts),
        ..Default::default()
    };
    for _ in 0..n_parts {
        info.ref_idx_l0.push(r.read_ue()?);
    }
    for _ in 0..n_parts {
        let dx = r.read_se()?;
        let dy = r.read_se()?;
        info.mvd_l0.push((dx, dy));
    }
    Ok(info)
}

fn read_p_8x8_motion(
    r: &mut BitReader<'_>,
    mb_type: MbType,
) -> Result<InterMotionInfo, CodecError> {
    let mut info = InterMotionInfo {
        sub_mb_types: Vec::with_capacity(4),
        ..Default::default()
    };
    for _ in 0..4 {
        let raw = r.read_ue()?;
        info.sub_mb_types.push(SubMbType::from_p_raw(raw)?);
    }
    // P_8x8ref0 fixes ref_idx_l0 = 0 for all partitions and skips the
    // ref_idx reads from the bitstream.
    let read_refs = matches!(mb_type, MbType::P8x8);
    for _ in 0..4 {
        info.ref_idx_l0
            .push(if read_refs { r.read_ue()? } else { 0 });
    }
    for sub in &info.sub_mb_types {
        for _ in 0..sub.num_partitions() {
            let dx = r.read_se()?;
            let dy = r.read_se()?;
            info.mvd_l0.push((dx, dy));
        }
    }
    Ok(info)
}

/// Decodes the `coded_block_pattern` value from its me(v) codeword
/// using H.264 Table 9-4.  `is_intra` selects the intra vs inter
/// column; `chroma_format_idc` selects whether the chroma column is
/// the full 0–47 table (for 4:2:0 / 4:2:2) or the 0–15 monochrome
/// subset.
///
/// Returns the 6-bit CBP value packed as `(chroma << 4) | luma`.
fn decode_cbp(
    me: u32,
    is_intra: bool,
    chroma_format_idc: u32,
) -> Result<u8, CodecError> {
    let table: &[u8] = match (chroma_format_idc, is_intra) {
        (1 | 2, true) => &CBP_INTRA_420_422,
        (1 | 2, false) => &CBP_INTER_420_422,
        (_, true) => &CBP_INTRA_MONOCHROME_OR_444,
        (_, false) => &CBP_INTER_MONOCHROME_OR_444,
    };
    table
        .get(me as usize)
        .copied()
        .ok_or_else(|| {
            CodecError::InvalidData(format!(
                "h264 macroblock: coded_block_pattern code {me} out of table"
            ))
        })
}

// H.264 Table 9-4 — 48-entry CBP mapping tables for 4:2:0 / 4:2:2.
// The values are the 6-bit CBP value where bits 0–3 are the luma
// 8×8 mask and bits 4–5 are the chroma_format-dependent chroma code.
const CBP_INTRA_420_422: [u8; 48] = [
    47, 31, 15, 0, 23, 27, 29, 30, 7, 11, 13, 14, 39, 43, 45, 46, 16, 3, 5, 10, 12, 19, 21, 26,
    28, 35, 37, 42, 44, 1, 2, 4, 8, 17, 18, 20, 24, 6, 9, 22, 25, 32, 33, 34, 36, 40, 38, 41,
];
const CBP_INTER_420_422: [u8; 48] = [
    0, 16, 1, 2, 4, 8, 32, 3, 5, 10, 12, 15, 47, 7, 11, 13, 14, 6, 9, 31, 35, 37, 42, 44, 33, 34,
    36, 40, 39, 43, 45, 46, 17, 18, 20, 24, 19, 21, 26, 28, 23, 27, 29, 30, 22, 25, 38, 41,
];
const CBP_INTRA_MONOCHROME_OR_444: [u8; 16] = [
    15, 0, 7, 11, 13, 14, 3, 5, 10, 12, 1, 2, 4, 8, 6, 9,
];
const CBP_INTER_MONOCHROME_OR_444: [u8; 16] = [
    0, 1, 2, 4, 8, 3, 5, 10, 12, 15, 7, 11, 13, 14, 6, 9,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::pps::PpsRbsp;
    use crate::h264::sps::SpsRbsp;

    fn make_sps() -> SpsRbsp {
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
            scaling_lists: None,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 0,
            log2_max_pic_order_cnt_lsb_minus4: 0,
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
            direct_8x8_inference_flag: true,
            frame_cropping_flag: false,
            frame_crop_left_offset: 0,
            frame_crop_right_offset: 0,
            frame_crop_top_offset: 0,
            frame_crop_bottom_offset: 0,
            vui_parameters_present_flag: false,
            vui: None,
        }
    }

    fn make_pps() -> PpsRbsp {
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
            deblocking_filter_control_present_flag: true,
            constrained_intra_pred_flag: false,
            redundant_pic_cnt_present_flag: false,
            transform_8x8_mode_flag: false,
            pic_scaling_matrix_present_flag: false,
            scaling_lists: None,
            second_chroma_qp_index_offset: 0,
        }
    }

    #[test]
    fn i16x16_mb_type_decoding_table() {
        // Spot-check a handful of I_16x16 values against the spec.
        //   raw = 1  -> pred=0(V), cbp_chroma=0, cbp_luma=0
        //   raw = 7  -> pred=2(DC), cbp_chroma=1, cbp_luma=0
        //   raw = 13 -> pred=0(V), cbp_chroma=0, cbp_luma=15
        //   raw = 24 -> pred=3(P), cbp_chroma=2, cbp_luma=15
        let cases = [
            (1u32, Intra16x16PredMode::Vertical, 0, 0),
            (7, Intra16x16PredMode::Dc, 0, 1),
            (13, Intra16x16PredMode::Vertical, 15, 0),
            (24, Intra16x16PredMode::Plane, 15, 2),
        ];
        for (raw, expected_mode, expected_luma, expected_chroma) in cases {
            match decode_i_mb_type(raw).unwrap() {
                MbType::I16x16 {
                    pred_mode,
                    cbp_luma,
                    cbp_chroma,
                } => {
                    assert_eq!(pred_mode, expected_mode, "raw = {raw}");
                    assert_eq!(cbp_luma, expected_luma, "raw = {raw}");
                    assert_eq!(cbp_chroma, expected_chroma, "raw = {raw}");
                }
                other => panic!("raw = {raw}, expected I_16x16, got {other:?}"),
            }
        }
    }

    #[test]
    fn i_slice_mb_type_endpoints() {
        assert_eq!(decode_i_mb_type(0).unwrap(), MbType::INxN);
        assert_eq!(decode_i_mb_type(25).unwrap(), MbType::IPcm);
        assert!(decode_i_mb_type(26).is_err());
    }

    #[test]
    fn p_slice_mb_type_translates_p_then_i_offset() {
        assert_eq!(
            decode_mb_type(0, SliceType::P).unwrap(),
            MbType::PL0_16x16,
        );
        assert_eq!(decode_mb_type(3, SliceType::P).unwrap(), MbType::P8x8);
        assert_eq!(decode_mb_type(5, SliceType::P).unwrap(), MbType::INxN);
        // raw = 30 -> I-offset 25 -> I_PCM
        assert_eq!(decode_mb_type(30, SliceType::P).unwrap(), MbType::IPcm);
    }

    #[test]
    fn b_slice_mb_type_returned_raw_in_phase_4a() {
        assert_eq!(
            decode_mb_type(7, SliceType::B).unwrap(),
            MbType::BSliceRaw(7),
        );
    }

    #[test]
    fn cbp_table_lookup_matches_spec() {
        // Table 9-4 spot checks:
        //   intra 4:2:0/4:2:2, code 0  -> 47
        //   inter 4:2:0/4:2:2, code 0  -> 0
        //   intra monochrome, code 0   -> 15
        //   inter monochrome, code 0   -> 0
        assert_eq!(decode_cbp(0, true, 1).unwrap(), 47);
        assert_eq!(decode_cbp(0, false, 1).unwrap(), 0);
        assert_eq!(decode_cbp(0, true, 0).unwrap(), 15);
        assert_eq!(decode_cbp(0, false, 0).unwrap(), 0);
    }

    #[test]
    fn cbp_table_rejects_out_of_range_code() {
        assert!(decode_cbp(48, true, 1).is_err());
        assert!(decode_cbp(16, true, 0).is_err());
    }

    #[test]
    fn parse_i_nxn_mb_with_no_residual() {
        // I-slice macroblock: I_NxN, all 16 4x4 blocks use the
        // most-probable mode (prev_flag = 1), intra_chroma_pred_mode
        // = 0 (DC), CBP = 0 (no residual), so mb_qp_delta is omitted.
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // mb_type = I_NxN
        // 16 × prev_intra4x4_pred_mode_flag = 1
        for _ in 0..16 {
            bits.push(true);
        }
        push_ue(&mut bits, 0); // intra_chroma_pred_mode = 0 (DC)
        push_ue(&mut bits, 3); // me=3 for intra 4:2:0 -> CBP = 0
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let buf = pack_bits_msb(&bits);
        let mut r = BitReader::new(&buf);
        let mb = parse_macroblock_layer(&mut r, &make_sps(), &make_pps(), SliceType::I)
            .expect("should parse");
        assert_eq!(mb.mb_type, MbType::INxN);
        let intra = mb.intra_nxn.unwrap();
        assert!(!intra.transform_size_8x8_flag);
        assert_eq!(intra.prev_intra4x4_pred_mode_flag.len(), 16);
        assert!(intra.prev_intra4x4_pred_mode_flag.iter().all(|&b| b));
        assert_eq!(
            mb.intra_chroma_pred_mode,
            Some(IntraChromaPredMode::Dc),
        );
        assert_eq!(mb.coded_block_pattern, 0);
        assert_eq!(mb.mb_qp_delta, 0);
    }

    #[test]
    fn parse_p_l0_16x16_mb() {
        // P-slice macroblock: P_L0_16x16, ref_idx_l0 = 0, mvd = (1, -1),
        // CBP = 0 (no residual), no mb_qp_delta.
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // mb_type = P_L0_16x16
        push_ue(&mut bits, 0); // ref_idx_l0 = 0
        push_se(&mut bits, 1); // mvd dx
        push_se(&mut bits, -1); // mvd dy
        push_ue(&mut bits, 0); // me=0 for inter 4:2:0 -> CBP = 0
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let buf = pack_bits_msb(&bits);
        let mut r = BitReader::new(&buf);
        let mb = parse_macroblock_layer(&mut r, &make_sps(), &make_pps(), SliceType::P)
            .expect("should parse");
        assert_eq!(mb.mb_type, MbType::PL0_16x16);
        let motion = mb.motion.unwrap();
        assert_eq!(motion.ref_idx_l0, vec![0]);
        assert_eq!(motion.mvd_l0, vec![(1, -1)]);
        assert_eq!(mb.coded_block_pattern, 0);
        assert_eq!(mb.mb_qp_delta, 0);
    }

    #[test]
    fn cabac_stream_rejected_until_phase_4k() {
        let mut pps = make_pps();
        pps.entropy_coding_mode_flag = true;
        let buf = [0x80u8; 4];
        let mut r = BitReader::new(&buf);
        let err = parse_macroblock_layer(&mut r, &make_sps(), &pps, SliceType::I)
            .expect_err("CABAC path should error in phase 4a");
        match err {
            CodecError::InvalidData(msg) => assert!(msg.contains("CABAC")),
            _ => panic!("expected InvalidData"),
        }
    }

    #[test]
    fn b_slice_mb_body_rejected_in_phase_4a() {
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // raw mb_type = 0
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let buf = pack_bits_msb(&bits);
        let mut r = BitReader::new(&buf);
        let err = parse_macroblock_layer(&mut r, &make_sps(), &make_pps(), SliceType::B)
            .expect_err("B-slice body should error in phase 4a");
        match err {
            CodecError::InvalidData(msg) => assert!(msg.contains("phase 4l")),
            _ => panic!("expected InvalidData"),
        }
    }

    #[test]
    fn sub_mb_partitions_count_matches_spec() {
        assert_eq!(SubMbType::PL0_8x8.num_partitions(), 1);
        assert_eq!(SubMbType::PL0_8x4.num_partitions(), 2);
        assert_eq!(SubMbType::PL0_4x8.num_partitions(), 2);
        assert_eq!(SubMbType::PL0_4x4.num_partitions(), 4);
    }

    // -- bit-building helpers --

    fn push_ue(bits: &mut Vec<bool>, value: u32) {
        let mut n = 0u32;
        while (1u32 << (n + 1)) - 1 <= value {
            n += 1;
            assert!(n <= 31);
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

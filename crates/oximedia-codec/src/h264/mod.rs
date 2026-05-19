//! H.264 bitstream-syntax parsers.
//!
//! This module provides the typed parsers that turn raw NAL-unit
//! payloads into structured data: SPS (with VUI and scaling lists),
//! PPS (with scaling lists), and slice header (with ref-list
//! modification, prediction weight table, and decoded reference
//! picture marking).  It is intentionally *parsing-only* — no
//! reconstruction is performed.
//!
//! ## Pipeline
//!
//! 1. Carve the byte stream into NAL units via the existing
//!    [`crate::nal_unit`] helpers or the RTP depacketizer in
//!    `oximedia-net`.
//! 2. Pass the NAL payload (after the 1-byte header) through
//!    [`rbsp::strip_emulation_prevention`] to recover the raw RBSP
//!    bytes.
//! 3. Dispatch on the NAL unit type:
//!    - Type 7 → [`sps::parse_sps`]
//!    - Type 8 → [`pps::parse_pps`]
//!    - Type 1 or 5 → [`slice_header::parse_slice_header`] (with the
//!      SPS / PPS / nal context produced above)
//!
//! ## What is retained
//!
//! - **SPS**: profile / level / dimensions / picture order count /
//!   reference frame count / cropping plus, when signalled, the full
//!   VUI body and any custom scaling matrices.
//! - **PPS**: entropy mode, ref-list defaults, QP, deblocking control,
//!   transform_8x8 mode, chroma QP offsets, plus any picture-level
//!   scaling matrices.
//! - **Slice header**: the prefix every decoder needs (`first_mb_in_slice`,
//!   `slice_type`, `frame_num`, IDR/field fields, picture order count
//!   info, num_ref_idx overrides, slice_qp_delta / effective QP) plus
//!   structured retention of reference-picture-list modification, the
//!   prediction weight table, and decoded reference-picture marking
//!   (IDR / sliding-window / adaptive MMCO variants).
//!
//! Future reconstruction (intra prediction, IDCT, deblocking, DPB
//! management) extends the workspace; this module stays parsing-only.

pub mod bit_reader;
pub mod cabac;
pub mod cabac_init_tables;
pub mod cabac_residual;
pub mod cabac_syntax;
pub mod cabac_tables;
pub mod cavlc;
pub mod cavlc_tables;
pub mod deblock;
pub mod decoder;
pub mod dpb;
pub mod frame;
pub mod intra_mode;
pub mod intra_pred;
pub mod macroblock;
pub mod motion;
pub mod mv_pred;
pub mod pps;
pub mod rbsp;
pub mod scaling_list;
pub mod slice_header;
pub mod sps;
pub mod transform;
pub mod vui;

pub use bit_reader::BitReader;
pub use deblock::{
    alpha_threshold, beta_threshold, boundary_strength, deblock_mb_luma, normal_filter_line,
    should_filter_line, strong_filter_line, DeblockBlockInfo,
};
pub use cabac::{
    init_context_state, init_contexts, init_contexts_i_slice, init_contexts_pb_slice,
    CabacContext, CABAC_STATE_LEN,
};
pub use cabac_residual::{
    coded_block_flag_ctx, decode_residual_dc, decode_residual_nondc, ResidualParams,
};
pub use cabac_syntax::{
    decode_b_sub_mb_type, decode_cbp_chroma, decode_cbp_luma, decode_intra4x4_pred_mode,
    decode_intra_chroma_pred_mode, decode_intra_mb_type, decode_mb_skip, decode_mvd,
    decode_p_sub_mb_type, decode_ref_idx, IntraMbType,
};
pub use cavlc::{
    decode_residual_block, read_coeff_token, read_level, read_residual_block, read_run_before,
    read_total_zeros_chroma_dc, read_total_zeros_luma, update_suffix_length, BlockKind,
    ResidualBlock,
};
pub use decoder::{
    decode_i_slice, decode_inter_p_l0_16x16_mb, decode_intra_16x16_mb, decode_intra_4x4_mb,
    decode_intra_chroma_8x8, decode_intra_slice_bitstream, IntraLumaSpec, IntraMacroblock,
    Residual4x4Scan,
};
pub use dpb::{Dpb, DpbEntry, DpbError};
pub use intra_mode::{
    most_probable_mode, resolve_intra4x4_mode, Intra4x4ModeContext,
};
pub use motion::{
    chroma_bilinear, fetch_chroma_4x4_subpel, fetch_luma_4x4_integer, fetch_luma_4x4_subpel,
    luma_6tap_unclipped, luma_half_pel, rounded_average,
};
pub use mv_pred::{
    apply_mv_delta, median3, predict_mv_16x8_bottom, predict_mv_16x8_top, predict_mv_8x16_left,
    predict_mv_8x16_right, predict_mv_median, MotionVector, MvPredictionContext,
};
pub use frame::{
    collect_chroma_8x8_neighbours, collect_intra16x16_neighbours, collect_intra4x4_neighbours,
    Frame,
};
pub use intra_pred::{
    predict_4x4, predict_16x16, predict_chroma_8x8, ChromaIntra8x8Neighbours, Intra16x16Neighbours,
    Intra4x4Mode, Intra4x4Neighbours,
};
pub use macroblock::{
    parse_macroblock_layer, InterMotionInfo, Intra16x16PredMode, IntraChromaPredMode,
    IntraNxNPredInfo, MacroblockLayer, MbType, MotionVectorDelta, SubMbType,
};
pub use pps::{parse_pps, PpsRbsp};
pub use rbsp::{strip_emulation_prevention, trailing_bits_len};
pub use scaling_list::{
    read_pic_scaling_matrix, read_seq_scaling_matrix, ScalingList4x4, ScalingList8x8,
    ScalingListChoice, ScalingLists,
};
pub use slice_header::{
    parse_slice_header, DecRefPicMarking, MmcoOp, NalContext, PredWeightTable, RefPicListModOp,
    RefPicListModification, SliceHeader, SliceType, WeightEntry,
};
pub use sps::{parse_sps, SpsRbsp};
pub use transform::{
    dequant_and_inverse_transform_4x4, dequantize_4x4, hadamard_1d_4, hadamard_2x2, hadamard_4x4,
    inverse_hadamard_2x2_chroma_dc, inverse_hadamard_4x4_luma_dc, inverse_scan_4x4,
    inverse_transform_1d_4, inverse_transform_4x4, level_scale_4x4,
};
pub use vui::{
    parse_vui, AspectRatioInfo, BitstreamRestriction, ChromaLocInfo, ColourDescription,
    CpbSchedule, ExtendedSar, HrdParameters, TimingInfo, VideoSignalType, VuiParameters,
};

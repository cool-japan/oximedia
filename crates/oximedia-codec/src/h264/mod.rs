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
pub mod cavlc;
pub mod macroblock;
pub mod pps;
pub mod rbsp;
pub mod scaling_list;
pub mod slice_header;
pub mod sps;
pub mod vui;

pub use bit_reader::BitReader;
pub use cavlc::{
    decode_residual_block, read_level, read_run_before, read_total_zeros_chroma_dc,
    read_total_zeros_luma, update_suffix_length, BlockKind, ResidualBlock,
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
pub use vui::{
    parse_vui, AspectRatioInfo, BitstreamRestriction, ChromaLocInfo, ColourDescription,
    CpbSchedule, ExtendedSar, HrdParameters, TimingInfo, VideoSignalType, VuiParameters,
};

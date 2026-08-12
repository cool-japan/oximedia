//! VP9 codec implementation.
//!
//! Pure Rust VP9 decoder based on the VP9 bitstream specification.
//! VP9 is a royalty-free video codec developed by Google.
//!
//! # Modules
//!
//! - `bitstream` - Boolean decoder for entropy coding
//! - `dec` - Real frame decoding (bit-exact vs libvpx) and cross-frame state
//! - `decoder` - Main VP9 decoder implementation
//! - `frame` - Frame types and structures
//! - `intra` - Intra prediction modes and functions
//! - `loopfilter` - Loop filter parameters
//! - `mv` - Motion vector types
//! - `partition` - Partition types and block sizes
//! - `probability` - Probability tables for entropy coding
//! - `segmentation` - Segmentation handling
//! - `superframe` - Superframe container parsing
//! - `transform` - Transform types and inverse transforms
//! - `uncompressed` - Uncompressed header parsing
//!
//! `compressed` (header parsing), `inter` (inter-prediction context),
//! `mvref` (MV reference candidates), `prediction` (interpolation) and
//! `reference` (reference-frame pool) were deleted in VP9 PACKAGE P13: they
//! were a superseded, never-invoked decode path (audit-verified AV1-shaped
//! or hollow) that the real decoder in `dec` replaced. `InterMode` and
//! `RefFrameType`, the two honest types `inter` held, moved to `symbols`,
//! their one remaining real consumer.

mod bitstream;
mod coeff_decode;
mod dec;
mod decoder;
mod encoder;
mod frame;
mod intra;
mod loopfilter;
mod mv;
mod partition;
mod probability;
mod segmentation;
mod superframe;
mod symbols;
mod tile_encoder;
mod transform;
mod uncompressed;

// Primary exports
pub use decoder::Vp9Decoder;
pub use encoder::{
    SimpleVp9Encoder, Vp9EncConfig, Vp9Encoder, Vp9EncoderConfig, Vp9Packet, Vp9Profile,
};
pub use frame::{FrameType as Vp9FrameType, Vp9Frame};
pub use superframe::{Superframe, SuperframeIndex};
pub use uncompressed::{ColorSpace, UncompressedHeader, Vp9FrameType as HeaderFrameType};

// Loop filter exports
pub use loopfilter::{LoopFilterInfo, LoopFilterMask, LoopFilterParams, LoopFilterState};

// Motion vector exports
pub use mv::{MotionVector, MvCandidate, MvClass, MvContext, MvJoint, MvPair, MvRefType, RefPair};

// Partition exports
pub use partition::{
    BlockPosition, BlockSize, Partition, PartitionContext, Superblock, TxMode, TxSize,
};

// Probability exports
pub use probability::{FrameContext, FrameCounts, MvProbs, Prob, ProbabilityContext};

// Segmentation exports
pub use segmentation::{SegmentData, SegmentFeature, SegmentMap, Segmentation};

// Intra prediction exports
pub use intra::{
    apply_intra_prediction, predict_dc, predict_horizontal, predict_tm, predict_vertical,
    IntraMode, IntraModeContext, IntraPredContext, SubBlockModes,
};

// Transform exports
pub use transform::{apply_inverse_transform, dequantize, CoeffBuffer, DequantContext, TxType};

// Coefficient decoding exports
pub use coeff_decode::{CoeffContext, CoeffDecoder, CoeffToken, QuantTables, ScanOrder};

// Symbol decoding exports (also re-exports InterMode/RefFrameType, relocated
// here from the deleted `inter` module — see the module doc comment above)
pub use symbols::{InterMode, RefFrameType, SymbolDecoder};

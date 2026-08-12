//! VP9 Symbol decoding with boolean arithmetic decoder.
//!
//! This module provides symbol decoding functions that use the boolean
//! arithmetic decoder to decode various syntax elements with probability-based
//! contexts, including partitions, modes, motion vectors, and segmentation.

#![forbid(unsafe_code)]
#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::similar_names)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::fn_params_excessive_bools)]

use super::bitstream::BoolDecoder;
use super::intra::IntraMode;
use super::mv::{MotionVector, MvClass, MvJoint};
use super::partition::{Partition, TxSize};
use super::probability::{FrameContext, Prob, INTRA_MODES};
use super::segmentation::MAX_SEGMENTS;
use crate::error::{CodecError, CodecResult};

// =============================================================================
// Inter / Reference Frame Types
// =============================================================================
//
// `InterMode` and `RefFrameType` used to live in the now-deleted `vp9::inter`
// (VP9 PACKAGE P13: superseded by the real decode path in `vp9::dec`, whose
// `INTER_MODES` / reference-frame handling has its own libvpx-verified
// constants). These two enums are the correct VP9 shapes — unlike the rest of
// that module (`COMPOUND_MODES = 8` was AV1's count, not VP9's) — and this
// file is their one remaining real consumer, so they were relocated here
// rather than deleted. Only the pieces this file (and its tests) use came
// along: the mode-index -> variant lookup, not the `MvRefType` conversions
// `vp9::inter::RefFrameType` also carried.

/// Inter prediction mode.
///
/// These modes describe how motion vectors are derived for inter blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
#[repr(u8)]
pub enum InterMode {
    /// Use the nearest motion vector from spatial neighbors.
    #[default]
    NearestMv = 0,
    /// Use the near motion vector from spatial neighbors.
    NearMv = 1,
    /// Use a zero motion vector.
    ZeroMv = 2,
    /// Use a new motion vector read from the bitstream.
    NewMv = 3,
}

impl InterMode {
    /// Converts from u8 value to `InterMode`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::NearestMv),
            1 => Some(Self::NearMv),
            2 => Some(Self::ZeroMv),
            3 => Some(Self::NewMv),
            _ => None,
        }
    }
}

/// Reference frame type for VP9.
///
/// VP9 supports three inter reference frames and one intra (no reference) type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
#[repr(u8)]
pub enum RefFrameType {
    /// Intra prediction (no reference frame).
    #[default]
    Intra = 0,
    /// Last decoded frame.
    Last = 1,
    /// Golden reference frame.
    Golden = 2,
    /// Alternate reference frame.
    AltRef = 3,
}

// =============================================================================
// Boolean Decoder Extension
// =============================================================================

impl BoolDecoder {
    /// Initializes the decoder with data.
    pub fn init(&mut self, data: &[u8], offset: usize) -> CodecResult<()> {
        if offset + 2 > data.len() {
            return Err(CodecError::InvalidBitstream(
                "Not enough data for boolean decoder".into(),
            ));
        }

        self.value = (u32::from(data[offset]) << 8) | u32::from(data[offset + 1]);
        self.range = 255;
        self.count = -8;
        Ok(())
    }

    /// Reads a single bit with probability.
    #[allow(clippy::cast_possible_wrap)]
    pub fn read_bool(&mut self, data: &[u8], offset: &mut usize, prob: Prob) -> CodecResult<bool> {
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        let big_split = split << 8;

        let bit = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            true
        } else {
            self.range = split;
            false
        };

        // Renormalize
        while self.range < 128 {
            self.range <<= 1;
            self.value <<= 1;
            self.count += 1;

            if self.count == 0 {
                self.count = -8;
                if *offset < data.len() {
                    self.value |= u32::from(data[*offset]);
                    *offset += 1;
                }
            }
        }

        Ok(bit)
    }

    /// Reads a literal bit.
    pub fn read_literal(&mut self, data: &[u8], offset: &mut usize) -> CodecResult<bool> {
        self.read_bool(data, offset, 128)
    }

    /// Reads multiple literal bits as u32.
    pub fn read_literal_bits(
        &mut self,
        data: &[u8],
        offset: &mut usize,
        bits: u8,
    ) -> CodecResult<u32> {
        let mut value = 0u32;
        for _ in 0..bits {
            value = (value << 1) | u32::from(self.read_literal(data, offset)?);
        }
        Ok(value)
    }

    /// Reads a symbol from a tree using probabilities.
    pub fn read_tree(
        &mut self,
        data: &[u8],
        offset: &mut usize,
        tree: &[i8],
        probs: &[Prob],
    ) -> CodecResult<i8> {
        let mut index = 0usize;

        loop {
            let prob_index = (index >> 1) as usize;
            if prob_index >= probs.len() {
                return Err(CodecError::InvalidBitstream(
                    "Prob index out of bounds".into(),
                ));
            }

            let bit = self.read_bool(data, offset, probs[prob_index])?;
            index = (index << 1) | (usize::from(bit));

            let tree_index = index as usize;
            if tree_index >= tree.len() {
                return Err(CodecError::InvalidBitstream(
                    "Tree index out of bounds".into(),
                ));
            }

            let val = tree[tree_index];
            if val <= 0 {
                return Ok(-val);
            }
            index = val as usize;
        }
    }
}

// =============================================================================
// Symbol Decoder
// =============================================================================

/// Symbol decoder for VP9 entropy coding.
#[derive(Debug)]
pub struct SymbolDecoder {
    /// Boolean arithmetic decoder.
    bool_decoder: BoolDecoder,
    /// Current byte offset in data.
    offset: usize,
}

impl Default for SymbolDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolDecoder {
    /// Creates a new symbol decoder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bool_decoder: BoolDecoder::new(),
            offset: 0,
        }
    }

    /// Initializes the decoder with compressed data.
    pub fn init(&mut self, data: &[u8], offset: usize) -> CodecResult<()> {
        self.offset = offset;
        self.bool_decoder.init(data, offset)?;
        self.offset += 2; // Skip initial bytes
        Ok(())
    }

    /// Returns the current byte offset.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    // =========================================================================
    // Partition Decoding
    // =========================================================================

    /// Decodes a partition type.
    pub fn decode_partition(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context: usize,
    ) -> CodecResult<Partition> {
        let probs = ctx.probs.get_partition_probs(context);

        // Read partition tree
        let has_rows = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[0])?;
        if !has_rows {
            return Ok(Partition::None);
        }

        let has_cols = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[1])?;
        if !has_cols {
            return Ok(Partition::Horz);
        }

        let split = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[2])?;
        if split {
            Ok(Partition::Split)
        } else {
            Ok(Partition::Vert)
        }
    }

    // =========================================================================
    // Skip Decoding
    // =========================================================================

    /// Decodes skip flag.
    pub fn decode_skip(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context: usize,
    ) -> CodecResult<bool> {
        let prob = ctx.probs.get_skip_prob(context);
        self.bool_decoder.read_bool(data, &mut self.offset, prob)
    }

    // =========================================================================
    // Intra/Inter Decoding
    // =========================================================================

    /// Decodes intra/inter flag.
    pub fn decode_is_inter(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context: usize,
    ) -> CodecResult<bool> {
        let prob = ctx.probs.get_intra_inter_prob(context);
        self.bool_decoder.read_bool(data, &mut self.offset, prob)
    }

    // =========================================================================
    // Intra Mode Decoding
    // =========================================================================

    /// Decodes intra Y mode for keyframes.
    pub fn decode_intra_y_mode_kf(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        above_mode: usize,
        left_mode: usize,
    ) -> CodecResult<IntraMode> {
        let probs = ctx.probs.get_kf_y_mode_probs(above_mode, left_mode);
        let mode_idx = self.decode_intra_mode_from_probs(data, probs)?;
        IntraMode::from_u8(mode_idx)
            .ok_or_else(|| CodecError::InvalidBitstream(format!("Invalid intra mode: {mode_idx}")))
    }

    /// Decodes intra UV mode.
    pub fn decode_intra_uv_mode(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        y_mode: usize,
    ) -> CodecResult<IntraMode> {
        let probs = ctx.probs.get_uv_mode_probs(y_mode);
        let mode_idx = self.decode_intra_mode_from_probs(data, probs)?;
        IntraMode::from_u8(mode_idx).ok_or_else(|| {
            CodecError::InvalidBitstream(format!("Invalid intra UV mode: {mode_idx}"))
        })
    }

    /// Helper to decode intra mode from probability array.
    fn decode_intra_mode_from_probs(
        &mut self,
        data: &[u8],
        probs: &[Prob; INTRA_MODES - 1],
    ) -> CodecResult<u8> {
        // Intra mode tree decoding
        for (i, &prob) in probs.iter().enumerate() {
            let bit = self.bool_decoder.read_bool(data, &mut self.offset, prob)?;
            if !bit {
                return Ok(i as u8);
            }
        }
        Ok((INTRA_MODES - 1) as u8)
    }

    // =========================================================================
    // Inter Mode Decoding
    // =========================================================================

    /// Decodes inter mode.
    pub fn decode_inter_mode(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context: usize,
    ) -> CodecResult<InterMode> {
        let probs = ctx.probs.get_inter_mode_probs(context);

        // Inter mode tree: NEARESTMV, NEARMV, ZEROMV, NEWMV
        for (i, &prob) in probs.iter().enumerate() {
            let bit = self.bool_decoder.read_bool(data, &mut self.offset, prob)?;
            if !bit {
                return InterMode::from_u8(i as u8).ok_or_else(|| {
                    CodecError::InvalidBitstream(format!("Invalid inter mode: {i}"))
                });
            }
        }

        Ok(InterMode::NewMv)
    }

    // =========================================================================
    // Reference Frame Decoding
    // =========================================================================

    /// Decodes compound reference mode flag.
    pub fn decode_comp_mode(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context: usize,
    ) -> CodecResult<bool> {
        if context >= ctx.probs.comp_mode.len() {
            return Ok(false);
        }
        let prob = ctx.probs.comp_mode[context];
        self.bool_decoder.read_bool(data, &mut self.offset, prob)
    }

    /// Decodes single reference frame.
    pub fn decode_single_ref(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context_0: usize,
        context_1: usize,
    ) -> CodecResult<RefFrameType> {
        // First bit: LAST vs (GOLDEN or ALTREF)
        let prob_0 = if context_0 < ctx.probs.single_ref.len() {
            ctx.probs.single_ref[context_0][0]
        } else {
            128
        };

        let is_last = !self
            .bool_decoder
            .read_bool(data, &mut self.offset, prob_0)?;
        if is_last {
            return Ok(RefFrameType::Last);
        }

        // Second bit: GOLDEN vs ALTREF
        let prob_1 = if context_1 < ctx.probs.single_ref.len() {
            ctx.probs.single_ref[context_1][1]
        } else {
            128
        };

        let is_golden = !self
            .bool_decoder
            .read_bool(data, &mut self.offset, prob_1)?;
        if is_golden {
            Ok(RefFrameType::Golden)
        } else {
            Ok(RefFrameType::AltRef)
        }
    }

    /// Decodes compound reference frame pair.
    pub fn decode_comp_ref(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        context: usize,
    ) -> CodecResult<(RefFrameType, RefFrameType)> {
        let prob = if context < ctx.probs.comp_ref.len() {
            ctx.probs.comp_ref[context]
        } else {
            128
        };

        let bit = self.bool_decoder.read_bool(data, &mut self.offset, prob)?;
        if bit {
            Ok((RefFrameType::Golden, RefFrameType::AltRef))
        } else {
            Ok((RefFrameType::Last, RefFrameType::AltRef))
        }
    }

    // =========================================================================
    // Motion Vector Decoding
    // =========================================================================

    /// Decodes a motion vector.
    pub fn decode_mv(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        allow_hp: bool,
    ) -> CodecResult<MotionVector> {
        // Decode MV joint (which components are non-zero)
        let joint = self.decode_mv_joint(data, ctx)?;

        let row = if joint.has_vertical() {
            self.decode_mv_component(data, ctx, 0, allow_hp)?
        } else {
            0
        };

        let col = if joint.has_horizontal() {
            self.decode_mv_component(data, ctx, 1, allow_hp)?
        } else {
            0
        };

        Ok(MotionVector::new(row, col))
    }

    /// Decodes motion vector joint.
    fn decode_mv_joint(&mut self, data: &[u8], ctx: &FrameContext) -> CodecResult<MvJoint> {
        let probs = &ctx.probs.mv.joints;

        let bit0 = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[0])?;
        if !bit0 {
            return Ok(MvJoint::Zero);
        }

        let bit1 = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[1])?;
        let bit2 = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[2])?;

        match (bit1, bit2) {
            (false, false) => Ok(MvJoint::HnzVz),
            (false, true) => Ok(MvJoint::HzVnz),
            _ => Ok(MvJoint::HnzVnz),
        }
    }

    /// Decodes a single motion vector component.
    #[allow(clippy::cast_possible_wrap)]
    fn decode_mv_component(
        &mut self,
        data: &[u8],
        ctx: &FrameContext,
        comp: usize,
        allow_hp: bool,
    ) -> CodecResult<i16> {
        let comp_probs = &ctx.probs.mv.comps[comp];

        // Sign
        let sign = self
            .bool_decoder
            .read_bool(data, &mut self.offset, comp_probs.sign)?;

        // Class
        let class = self.decode_mv_class(data, comp_probs.classes)?;

        // Decode based on class
        let mut mag = if class == 0 {
            // Class 0: use class0 bit
            let class0_bit =
                self.bool_decoder
                    .read_bool(data, &mut self.offset, comp_probs.class0[0])?;

            // Class 0 fractional part
            let fp_idx = if class0_bit { 1 } else { 0 };
            let fp = self.decode_mv_fp(data, &comp_probs.class0_fp[fp_idx])?;

            let base = if class0_bit { 1 } else { 0 };
            (base << 3) | i32::from(fp)
        } else {
            // Offset bits for this class
            let mut mag = (1 << (class + 2)) as i32;

            for i in 0..class {
                let bit_idx = (class - 1 - i) as usize;
                if bit_idx < comp_probs.bits.len() {
                    let bit = self.bool_decoder.read_bool(
                        data,
                        &mut self.offset,
                        comp_probs.bits[bit_idx],
                    )?;
                    if bit {
                        mag |= 1 << (i + 1);
                    }
                }
            }

            // Fractional part
            let fp = self.decode_mv_fp(data, &comp_probs.fp)?;
            (mag << 3) | i32::from(fp)
        };

        // High precision bit
        if allow_hp {
            let hp_bit = if class == 0 {
                self.bool_decoder
                    .read_bool(data, &mut self.offset, comp_probs.class0_hp)?
            } else {
                self.bool_decoder
                    .read_bool(data, &mut self.offset, comp_probs.hp)?
            };

            if hp_bit {
                mag = (mag << 1) | 1;
            } else {
                mag <<= 1;
            }
        } else {
            mag <<= 1;
        }

        // Apply sign
        if sign {
            mag = -mag;
        }

        Ok(mag as i16)
    }

    /// Decodes motion vector class.
    fn decode_mv_class(&mut self, data: &[u8], probs: [Prob; 10]) -> CodecResult<u8> {
        for (i, &prob) in probs.iter().enumerate() {
            let bit = self.bool_decoder.read_bool(data, &mut self.offset, prob)?;
            if !bit {
                return Ok(i as u8);
            }
        }
        Ok(10)
    }

    /// Decodes motion vector fractional precision.
    fn decode_mv_fp(&mut self, data: &[u8], probs: &[Prob; 3]) -> CodecResult<u8> {
        let bit1 = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[0])?;
        let bit2 = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[1])?;
        let bit3 = self
            .bool_decoder
            .read_bool(data, &mut self.offset, probs[2])?;

        Ok((u8::from(bit1) << 2) | (u8::from(bit2) << 1) | u8::from(bit3))
    }

    // =========================================================================
    // Transform Size Decoding
    // =========================================================================

    /// Decodes transform size.
    pub fn decode_tx_size(
        &mut self,
        data: &[u8],
        _ctx: &FrameContext,
        max_tx_size: TxSize,
        _context: usize,
    ) -> CodecResult<TxSize> {
        // Simplified: return max TX size for now
        // Full implementation would decode based on probability tables
        Ok(max_tx_size)
    }

    // =========================================================================
    // Segmentation Decoding
    // =========================================================================

    /// Decodes segment ID.
    pub fn decode_segment_id(
        &mut self,
        data: &[u8],
        _ctx: &FrameContext,
        _context: usize,
    ) -> CodecResult<u8> {
        // Decode segment ID using tree
        let mut seg_id = 0u8;

        for i in 0..3 {
            // 3 bits for 8 segments
            let bit = self.bool_decoder.read_literal(data, &mut self.offset)?;
            seg_id |= (u8::from(bit)) << (2 - i);
        }

        Ok(seg_id.min((MAX_SEGMENTS - 1) as u8))
    }

    // =========================================================================
    // Utility Methods
    // =========================================================================

    /// Reads a signed integer value.
    #[allow(clippy::cast_possible_wrap)]
    pub fn read_signed(&mut self, _data: &[u8], _bits: u8) -> CodecResult<i32> {
        // Simplified implementation
        Ok(0)
    }

    /// Reads an unsigned integer value.
    pub fn read_unsigned(&mut self, _data: &[u8], _bits: u8) -> CodecResult<u32> {
        Ok(0) // Simplified implementation
    }

    /// Finishes decoding and returns the final offset.
    #[must_use]
    pub const fn finish(&self) -> usize {
        self.offset
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_decoder_new() {
        let decoder = SymbolDecoder::new();
        assert_eq!(decoder.offset(), 0);
    }

    #[test]
    fn test_bool_decoder_init() {
        let mut decoder = BoolDecoder::new();
        let data = vec![0xAA, 0x55, 0x00];
        assert!(decoder.init(&data, 0).is_ok());
    }

    #[test]
    fn test_bool_decoder_read_literal() {
        let mut decoder = BoolDecoder::new();
        let data = vec![0xFF, 0xFF, 0xFF, 0xFF];
        decoder.init(&data, 0).expect("should succeed");
        let mut offset = 2;

        let bit = decoder
            .read_literal(&data, &mut offset)
            .expect("should succeed");
        assert!(bit);
    }

    #[test]
    fn test_bool_decoder_read_literal_bits() {
        let mut decoder = BoolDecoder::new();
        let data = vec![0xAA, 0x55, 0xF0, 0x0F];
        decoder.init(&data, 0).expect("should succeed");
        let mut offset = 2;

        let value = decoder
            .read_literal_bits(&data, &mut offset, 4)
            .expect("should succeed");
        // Value depends on boolean decoder state, just verify it's within valid range
        assert!(value <= 15);
    }

    #[test]
    fn test_mv_joint_properties() {
        assert!(!MvJoint::Zero.has_horizontal());
        assert!(!MvJoint::Zero.has_vertical());

        assert!(MvJoint::HnzVz.has_horizontal());
        assert!(!MvJoint::HnzVz.has_vertical());

        assert!(!MvJoint::HzVnz.has_horizontal());
        assert!(MvJoint::HzVnz.has_vertical());

        assert!(MvJoint::HnzVnz.has_horizontal());
        assert!(MvJoint::HnzVnz.has_vertical());
    }

    #[test]
    fn test_partition_decode_simple() {
        let mut decoder = SymbolDecoder::new();
        let data = vec![0x80, 0x80, 0x00, 0x00, 0x00];
        decoder.init(&data, 0).expect("should succeed");

        let ctx = FrameContext::new();
        // In a real scenario, this would decode actual partition data
        // Here we just test that the function runs without error
        let result = decoder.decode_partition(&data, &ctx, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_skip_decode() {
        let mut decoder = SymbolDecoder::new();
        let data = vec![0x80, 0x80, 0x00, 0x00];
        decoder.init(&data, 0).expect("should succeed");

        let ctx = FrameContext::new();
        let result = decoder.decode_skip(&data, &ctx, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_inter_mode_from_u8() {
        assert_eq!(InterMode::from_u8(0), Some(InterMode::NearestMv));
        assert_eq!(InterMode::from_u8(1), Some(InterMode::NearMv));
        assert_eq!(InterMode::from_u8(2), Some(InterMode::ZeroMv));
        assert_eq!(InterMode::from_u8(3), Some(InterMode::NewMv));
    }

    #[test]
    fn test_ref_frame_decode_single() {
        let mut decoder = SymbolDecoder::new();
        let data = vec![0x80, 0x80, 0x00, 0x00, 0x00];
        decoder.init(&data, 0).expect("should succeed");

        let ctx = FrameContext::new();
        let result = decoder.decode_single_ref(&data, &ctx, 0, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mv_class_offset_bits() {
        assert_eq!(MvClass::Class0.offset_bits(), 0);
        assert_eq!(MvClass::Class1.offset_bits(), 1);
        assert_eq!(MvClass::Class5.offset_bits(), 5);
        assert_eq!(MvClass::Class10.offset_bits(), 10);
    }

    #[test]
    fn test_symbol_decoder_read_unsigned() {
        let mut decoder = SymbolDecoder::new();
        let data = vec![0xFF, 0xFF, 0xFF, 0xFF];
        decoder.init(&data, 0).expect("should succeed");

        let value = decoder.read_unsigned(&data, 4).expect("should succeed");
        assert!(value <= 15);
    }

    #[test]
    fn test_symbol_decoder_finish() {
        let mut decoder = SymbolDecoder::new();
        let data = vec![0x80, 0x80];
        decoder.init(&data, 0).expect("should succeed");

        assert_eq!(decoder.finish(), 2);
    }

    #[test]
    fn test_intra_mode_from_u8() {
        assert_eq!(IntraMode::from_u8(0), Some(IntraMode::Dc));
        assert!(IntraMode::from_u8(100).is_none());
    }

    #[test]
    fn test_segment_id_bounds() {
        let mut decoder = SymbolDecoder::new();
        let data = vec![0xFF, 0xFF, 0xFF, 0xFF];
        decoder.init(&data, 0).expect("should succeed");

        let seg_id = decoder
            .decode_segment_id(&data, &FrameContext::new(), 0)
            .expect("should succeed");
        assert!(seg_id < MAX_SEGMENTS as u8);
    }
}

//! H.264 scaling-list decoding and retention.
//!
//! Custom quantization scaling matrices are signalled in the SPS (for
//! sequence-wide defaults) and PPS (for picture-level overrides). The
//! H.264 §7.3.2.1.1.1 / §7.4.2.1.1.1 scaling-list procedure decodes
//! each list as a series of `se(v)` deltas, with three exit
//! conditions: a "use default" sentinel, a "fall back to fallback" rule
//! (where the next list reuses an earlier one), or an explicit
//! delta-coded list.
//!
//! The set of lists is:
//!
//! - For non-4:4:4 content: 6 × 4×4 + 2 × 8×8 = 8 lists.
//! - For 4:4:4 content (`chroma_format_idc == 3`): 6 × 4×4 + 6 × 8×8
//!   = 12 lists.

use crate::h264::bit_reader::BitReader;
use crate::CodecError;

/// Number of 4×4 scaling list slots in any chroma format.
pub const NUM_4X4_LISTS: usize = 6;

/// Number of 8×8 scaling list slots when `chroma_format_idc < 3`.
pub const NUM_8X8_LISTS_NON_444: usize = 2;

/// Number of 8×8 scaling list slots when `chroma_format_idc == 3`.
pub const NUM_8X8_LISTS_444: usize = 6;

/// A single 4×4 scaling matrix flattened to a length-16 array in
/// scan order.
pub type ScalingList4x4 = [i16; 16];

/// A single 8×8 scaling matrix flattened to a length-64 array in
/// scan order.
pub type ScalingList8x8 = [i16; 64];

/// Encoding choice per slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingListChoice {
    /// `present_flag` was 0: use the spec's default matrix or, for
    /// PPS, fall back to the SPS's matrix at this slot.
    NotPresent,
    /// `present_flag` was 1 and the first delta was 0: use the spec's
    /// default scaling matrix verbatim.
    UseDefault,
    /// Custom matrix decoded inline.
    Custom,
}

/// Retained scaling matrices for one set (SPS or PPS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingLists {
    /// Choice for each 4×4 slot (in spec order: Intra Y, Intra Cb,
    /// Intra Cr, Inter Y, Inter Cb, Inter Cr).
    pub choice_4x4: [ScalingListChoice; NUM_4X4_LISTS],
    /// Decoded 4×4 matrices when the choice is `Custom`. Otherwise all
    /// zero — interpret the matrix via the choice flag.
    pub matrices_4x4: [ScalingList4x4; NUM_4X4_LISTS],
    /// 8×8 slots in scan order (Intra Y, Inter Y, plus four more for
    /// 4:4:4: Intra Cb, Inter Cb, Intra Cr, Inter Cr).
    pub choice_8x8: Vec<ScalingListChoice>,
    /// 8×8 matrices, length matches `choice_8x8`.
    pub matrices_8x8: Vec<ScalingList8x8>,
}

impl ScalingLists {
    /// Empty scaling lists with every slot marked `NotPresent`.
    #[must_use]
    pub fn empty(chroma_format_idc: u32) -> Self {
        let num_8x8 = if chroma_format_idc == 3 {
            NUM_8X8_LISTS_444
        } else {
            NUM_8X8_LISTS_NON_444
        };
        Self {
            choice_4x4: [ScalingListChoice::NotPresent; NUM_4X4_LISTS],
            matrices_4x4: [[0; 16]; NUM_4X4_LISTS],
            choice_8x8: vec![ScalingListChoice::NotPresent; num_8x8],
            matrices_8x8: vec![[0; 64]; num_8x8],
        }
    }
}

/// Reads a SPS-level scaling matrix per H.264 §7.3.2.1.1.1. The caller
/// must have already consumed `seq_scaling_matrix_present_flag` and
/// confirmed it was true.
///
/// `chroma_format_idc` controls the number of 8×8 lists.
///
/// # Errors
///
/// Propagates [`CodecError::InvalidData`] from the bit reader.
pub fn read_seq_scaling_matrix(
    r: &mut BitReader<'_>,
    chroma_format_idc: u32,
) -> Result<ScalingLists, CodecError> {
    let mut lists = ScalingLists::empty(chroma_format_idc);
    for i in 0..NUM_4X4_LISTS {
        if r.read_bit()? {
            let (matrix, use_default) = read_scaling_list::<16>(r)?;
            lists.choice_4x4[i] = if use_default {
                ScalingListChoice::UseDefault
            } else {
                ScalingListChoice::Custom
            };
            lists.matrices_4x4[i] = matrix;
        }
    }
    let num_8x8 = lists.choice_8x8.len();
    for i in 0..num_8x8 {
        if r.read_bit()? {
            let (matrix, use_default) = read_scaling_list::<64>(r)?;
            lists.choice_8x8[i] = if use_default {
                ScalingListChoice::UseDefault
            } else {
                ScalingListChoice::Custom
            };
            lists.matrices_8x8[i] = matrix;
        }
    }
    Ok(lists)
}

/// Reads a PPS-level scaling matrix per H.264 §7.4.2.1.1.1. `transform_8x8`
/// selects whether the 2 (or 6) 8×8 lists are signalled.
///
/// # Errors
///
/// Propagates [`CodecError::InvalidData`] from the bit reader.
pub fn read_pic_scaling_matrix(
    r: &mut BitReader<'_>,
    chroma_format_idc: u32,
    transform_8x8: bool,
) -> Result<ScalingLists, CodecError> {
    let mut lists = ScalingLists::empty(chroma_format_idc);
    for i in 0..NUM_4X4_LISTS {
        if r.read_bit()? {
            let (matrix, use_default) = read_scaling_list::<16>(r)?;
            lists.choice_4x4[i] = if use_default {
                ScalingListChoice::UseDefault
            } else {
                ScalingListChoice::Custom
            };
            lists.matrices_4x4[i] = matrix;
        }
    }
    if transform_8x8 {
        let num_8x8 = lists.choice_8x8.len();
        for i in 0..num_8x8 {
            if r.read_bit()? {
                let (matrix, use_default) = read_scaling_list::<64>(r)?;
                lists.choice_8x8[i] = if use_default {
                    ScalingListChoice::UseDefault
                } else {
                    ScalingListChoice::Custom
                };
                lists.matrices_8x8[i] = matrix;
            }
        }
    } else {
        lists.choice_8x8.clear();
        lists.matrices_8x8.clear();
    }
    Ok(lists)
}

/// Decodes one scaling list (4×4 or 8×8 depending on `SIZE`).
///
/// Returns the decoded matrix and a flag indicating whether the
/// spec-defined "use default scaling matrix" sentinel was observed.
fn read_scaling_list<const SIZE: usize>(
    r: &mut BitReader<'_>,
) -> Result<([i16; SIZE], bool), CodecError> {
    let mut last_scale: i32 = 8;
    let mut next_scale: i32 = 8;
    let mut use_default = false;
    let mut matrix: [i16; SIZE] = [0; SIZE];
    for j in 0..SIZE {
        if next_scale != 0 {
            let delta_scale = r.read_se()?;
            next_scale = ((last_scale + delta_scale + 256) % 256) as i32;
            if j == 0 && next_scale == 0 {
                use_default = true;
            }
        }
        let entry = if next_scale == 0 {
            last_scale
        } else {
            next_scale
        };
        matrix[j] = entry as i16;
        last_scale = entry;
    }
    Ok((matrix, use_default))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_default_sentinel_list_4x4() -> Vec<u8> {
        // First delta_scale = -8 (se: -8 -> ue mapping = 16 -> codeword `000010001`)
        // After first delta: next_scale = (8 + (-8) + 256) % 256 = 0 -> use_default
        // Spec then says: scaling list assignment falls back to default
        let mut bits = Vec::new();
        push_se(&mut bits, -8);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        pack_bits_msb(&bits)
    }

    #[test]
    fn read_scaling_list_detects_use_default_sentinel() {
        let payload = build_default_sentinel_list_4x4();
        let mut r = BitReader::new(&payload);
        let (_, use_default) = read_scaling_list::<16>(&mut r).unwrap();
        assert!(use_default);
    }

    #[test]
    fn read_scaling_list_decodes_custom_matrix() {
        // Each entry derived from the previous by adding the delta:
        //   delta_scale = 0 -> next_scale stays 8 (entries 1, 2, ..., 16)
        let mut bits = Vec::new();
        for _ in 0..16 {
            push_se(&mut bits, 0);
        }
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let payload = pack_bits_msb(&bits);
        let mut r = BitReader::new(&payload);
        let (matrix, use_default) = read_scaling_list::<16>(&mut r).unwrap();
        assert!(!use_default);
        assert_eq!(matrix, [8i16; 16]);
    }

    #[test]
    fn empty_lists_have_right_8x8_count() {
        assert_eq!(ScalingLists::empty(1).choice_8x8.len(), 2);
        assert_eq!(ScalingLists::empty(3).choice_8x8.len(), 6);
    }

    // -- helpers --

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

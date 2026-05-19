//! CAVLC entropy decode for H.264 residual blocks.
//!
//! H.264's Context-Adaptive Variable-Length Code maps a block of
//! quantized transform coefficients to a small set of well-defined
//! syntax elements:
//!
//! - `coeff_token` — encodes `(TotalCoeff, TrailingOnes)`.  Phase 4b-ii.
//! - `trailing_ones_sign_flag` — one bit per trailing ±1.
//! - `level_prefix` + `level_suffix` — magnitude and sign of every
//!   non-trailing-one coefficient.  This module.
//! - `total_zeros` — number of zero coefficients before the last
//!   non-zero.  This module.
//! - `run_before` — number of zeros immediately preceding each
//!   non-zero in scan order.  This module.
//!
//! ## Scope of this commit (phase 4b-i)
//!
//! Framework + algorithmic level decode + the `total_zeros` and
//! `run_before` lookup tables.  The remaining work — the four
//! `coeff_token` VLC tables that depend on neighbour context — lands
//! in phase 4b-ii so each table source is reviewable independently.
//! Until that lands, [`decode_residual_block`] accepts a caller-
//! supplied `(TotalCoeff, TrailingOnes)` pair instead of reading it
//! from the bitstream.

use crate::h264::bit_reader::BitReader;
use crate::h264::cavlc_tables::{
    COEFF_TOKEN_CHROMA_DC_BITS, COEFF_TOKEN_CHROMA_DC_LENS, COEFF_TOKEN_LUMA_BITS,
    COEFF_TOKEN_LUMA_LENS, RUN_BEFORE_BITS, RUN_BEFORE_LENS, TOTAL_ZEROS_CHROMA_DC_BITS,
    TOTAL_ZEROS_CHROMA_DC_LENS, TOTAL_ZEROS_LUMA_BITS, TOTAL_ZEROS_LUMA_LENS,
};
use crate::CodecError;

/// Which block in the macroblock this residual belongs to.  Determines
/// the maximum number of coefficients (and, in a later phase, the
/// scan order used to expand the run/level pairs into a 2D matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// 16-coefficient 4×4 luma block (intra or inter).
    Luma4x4,
    /// 16-coefficient luma DC block from an `I_16x16` macroblock.
    LumaDc16x16,
    /// 15-coefficient luma AC block from an `I_16x16` macroblock
    /// (the DC coefficient is coded separately).
    LumaAc16x16,
    /// 4-coefficient chroma DC block (one per chroma component).
    ChromaDc,
    /// 15-coefficient chroma AC block.
    ChromaAc,
}

impl BlockKind {
    /// Maximum number of coefficients this block can carry.
    #[must_use]
    pub fn max_coefficients(self) -> u8 {
        match self {
            Self::Luma4x4 | Self::LumaDc16x16 => 16,
            Self::LumaAc16x16 | Self::ChromaAc => 15,
            Self::ChromaDc => 4,
        }
    }

    /// True for the chroma-DC block, which uses its own coeff_token
    /// table (Table 9-5(e) in the spec, decoded in phase 4b-ii).
    #[must_use]
    pub fn is_chroma_dc(self) -> bool {
        matches!(self, Self::ChromaDc)
    }
}

/// One residual block's decoded contents.
///
/// Levels are stored in **reverse scan order** — the encoder writes
/// the highest-frequency non-zero coefficient first.  Pairing each
/// level with the run that *precedes* it in scan order requires
/// iterating over `runs.iter().rev().zip(levels.iter().rev())` (the
/// reverse-zip recovers natural scan order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResidualBlock {
    /// Number of non-zero coefficients in the block (0..=16).
    pub total_coeff: u8,
    /// Of those non-zeros, how many at the high-frequency end have
    /// magnitude 1 (capped at 3 — the spec only encodes the first
    /// three).
    pub trailing_ones: u8,
    /// Non-zero coefficient values, in reverse scan order (highest-
    /// frequency first).  Length equals `total_coeff`.
    pub levels: Vec<i32>,
    /// Run of zeros immediately preceding each non-zero, in reverse
    /// scan order.  Length is `total_coeff - 1` (the last non-zero
    /// has no run after it).  Empty when `total_coeff <= 1`.
    pub runs: Vec<u8>,
    /// Total number of zero coefficients before the last non-zero in
    /// scan order.  Equals the sum of `runs`.
    pub total_zeros: u8,
}

impl ResidualBlock {
    /// Expands the run/level encoding back into a flat array of
    /// coefficients in scan order.  Output length equals
    /// `total_coeff + total_zeros`.
    #[must_use]
    pub fn to_scan_order(&self) -> Vec<i32> {
        let mut out = Vec::with_capacity(self.total_coeff as usize + self.total_zeros as usize);
        if self.total_coeff == 0 {
            return out;
        }
        // The spec writes the encoded sequence from highest- to lowest-
        // frequency.  To recover natural (low → high) scan order, walk
        // the levels and runs from the end.
        for i in (0..self.levels.len()).rev() {
            out.push(self.levels[i]);
            if i > 0 {
                // `runs[i-1]` is the zeros between level i and level i-1
                // in the reverse-ordered storage — which corresponds to
                // the zeros between two adjacent non-zeros in scan order.
                let zeros = self.runs.get(i - 1).copied().unwrap_or(0);
                for _ in 0..zeros {
                    out.push(0);
                }
            }
        }
        // Leading zeros (between the last non-zero we just emitted and
        // the start of the block) — derived from total_zeros minus the
        // zeros already inserted.
        let inserted: u32 = self
            .runs
            .iter()
            .map(|&r| u32::from(r))
            .sum();
        let leading = u32::from(self.total_zeros).saturating_sub(inserted);
        for _ in 0..leading {
            out.push(0);
        }
        // The scan order convention here puts the high-frequency end
        // first; reverse so callers get low-frequency first.
        out.reverse();
        out
    }
}

/// Reads one non-trailing-one level using H.264's adaptive level VLC.
///
/// `suffix_length` is the encoder's running suffix-length state for
/// this block (see [`update_suffix_length`]).  Callers must thread
/// the updated value back through each subsequent level read.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the bitstream runs out
/// before a complete code is read or when the level prefix runs away
/// past the sanity bound.
pub fn read_level(r: &mut BitReader<'_>, suffix_length: u8) -> Result<i32, CodecError> {
    // level_prefix: unary count of leading zeros, terminated by the
    // first 1 bit.  The spec caps the practical prefix at 15 (with 16
    // reserved as the escape).
    let mut level_prefix: u32 = 0;
    loop {
        if level_prefix > 32 {
            return Err(CodecError::InvalidData(
                "h264 cavlc: level_prefix runaway".into(),
            ));
        }
        if r.read_bit()? {
            break;
        }
        level_prefix += 1;
    }

    let suffix_len = u32::from(suffix_length);

    let level_code: u32 = if level_prefix < 14 {
        // Standard path.
        let suffix = if suffix_len > 0 {
            r.read_bits(suffix_len)?
        } else {
            0
        };
        (level_prefix << suffix_len) + suffix
    } else if level_prefix == 14 && suffix_len == 0 {
        // Short fallback: 4-bit fixed-length suffix.
        14 + r.read_bits(4)?
    } else if level_prefix == 15 {
        // Escape path: 12-bit suffix.
        let suffix = r.read_bits(12)?;
        if suffix_len > 0 {
            (15u32 << suffix_len) + suffix
        } else {
            14 + 16 + suffix
        }
    } else {
        return Err(CodecError::InvalidData(format!(
            "h264 cavlc: invalid level_prefix {level_prefix}"
        )));
    };

    // Adjustment: when a level is read while suffix_length was 0, the
    // first non-trailing-one level cannot be 0.  Treat level_code 0 /
    // 1 specially per spec to recover ±1 / ±2 ... mapping.
    let signed_level = if level_code & 1 == 0 {
        ((level_code as i32) + 2) >> 1
    } else {
        -(((level_code as i32) + 1) >> 1)
    };

    Ok(signed_level)
}

/// Updates the adaptive `suffix_length` state after reading one
/// non-trailing-one level.  Encoder and decoder must apply the same
/// update so successive levels stay in sync.
///
/// Returns the new `suffix_length` for the next read.
#[must_use]
pub fn update_suffix_length(current: u8, level: i32) -> u8 {
    // After the first non-trailing-one level past a "suffix_length=0"
    // start, the spec forces suffix_length to at least 1.
    let mut next = if current == 0 { 1 } else { current };
    let threshold: u32 = 3u32 << (next - 1);
    if (level.unsigned_abs()) > threshold && next < 6 {
        next += 1;
    }
    next
}

/// Reads the `total_zeros` field for a non-chroma-DC block.
///
/// `total_coeff` (1..=15) selects which of the 15 VLC tables to use.
/// Returns 0 when total_coeff is 16 (the block is fully populated —
/// no zeros are possible) or when total_coeff is 0 (caller would not
/// invoke this).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] for malformed codes.
pub fn read_total_zeros_luma(
    r: &mut BitReader<'_>,
    total_coeff: u8,
) -> Result<u8, CodecError> {
    if total_coeff == 0 || total_coeff >= 16 {
        return Ok(0);
    }
    let idx = (total_coeff - 1) as usize;
    scan_flat_vlc(
        r,
        &TOTAL_ZEROS_LUMA_LENS[idx],
        &TOTAL_ZEROS_LUMA_BITS[idx],
        "total_zeros_luma",
    )
}

/// Reads the chroma-DC variant of `total_zeros`.
///
/// `total_coeff` is 1..=3 (the chroma DC block has at most 4 coeffs,
/// and total_coeff == 4 gives total_zeros == 0).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] for malformed codes.
pub fn read_total_zeros_chroma_dc(
    r: &mut BitReader<'_>,
    total_coeff: u8,
) -> Result<u8, CodecError> {
    if total_coeff == 0 || total_coeff >= 4 {
        return Ok(0);
    }
    let idx = (total_coeff - 1) as usize;
    scan_flat_vlc(
        r,
        &TOTAL_ZEROS_CHROMA_DC_LENS[idx],
        &TOTAL_ZEROS_CHROMA_DC_BITS[idx],
        "total_zeros_chroma_dc",
    )
}

/// Reads one `run_before` syntax element.
///
/// `zeros_left` is the encoder's remaining-zero count; the decoder
/// must thread it through (decrementing by each run that comes back).
/// Values of `zeros_left >= 7` share a single (slightly different)
/// VLC table — H.264 special-cases the long-run case with a unary
/// code beyond run=6.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the bit reader is exhausted
/// or a code is out of range.
pub fn read_run_before(
    r: &mut BitReader<'_>,
    zeros_left: u8,
) -> Result<u8, CodecError> {
    if zeros_left == 0 {
        return Ok(0);
    }
    let idx = (zeros_left.min(7) - 1) as usize;
    scan_flat_vlc(
        r,
        &RUN_BEFORE_LENS[idx],
        &RUN_BEFORE_BITS[idx],
        "run_before",
    )
}

/// Reads the `coeff_token` syntax element from the bitstream.
///
/// Selects one of four luma variable-length code tables based on
/// neighbour-context `nc`, or the dedicated 4:2:0 chroma DC table
/// when `kind` is [`BlockKind::ChromaDc`], and scans the chosen
/// table bit-by-bit to recover `(TotalCoeff, TrailingOnes)`.
///
/// `nc` selection (per H.264):
/// - `nc < 2`              → luma table 0
/// - `nc in [2, 4)`        → luma table 1
/// - `nc in [4, 8)`        → luma table 2
/// - `nc >= 8`             → luma table 3 (a fixed 6-bit code; the
///                            spec's "high nc" path)
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the bitstream is
/// exhausted before a complete codeword is read, or when the bits
/// don't match any entry in the selected table.
pub fn read_coeff_token(
    r: &mut BitReader<'_>,
    kind: BlockKind,
    nc: i32,
) -> Result<(u8, u8), CodecError> {
    if kind.is_chroma_dc() {
        return scan_coeff_token(
            r,
            &COEFF_TOKEN_CHROMA_DC_LENS,
            &COEFF_TOKEN_CHROMA_DC_BITS,
            5,
            "chroma DC",
        );
    }
    let table_idx = if nc < 2 {
        0
    } else if nc < 4 {
        1
    } else if nc < 8 {
        2
    } else {
        3
    };
    scan_coeff_token(
        r,
        &COEFF_TOKEN_LUMA_LENS[table_idx],
        &COEFF_TOKEN_LUMA_BITS[table_idx],
        17,
        "luma",
    )
}

/// Scans `coeff_token` bit-by-bit against the supplied `lens` /
/// `bits` arrays, both laid out as `[total_coeff * 4 + trailing_ones]`
/// indexed.  `rows` is the number of valid `total_coeff` rows
/// (17 for luma, 5 for 4:2:0 chroma DC).
fn scan_coeff_token(
    r: &mut BitReader<'_>,
    lens: &[u8],
    bits: &[u8],
    rows: usize,
    label: &str,
) -> Result<(u8, u8), CodecError> {
    // Compute max codeword length in this table.
    let max_len = lens.iter().copied().max().unwrap_or(0);
    let mut accumulated: u32 = 0;
    let mut cur_len: u8 = 0;
    while cur_len < max_len {
        accumulated = (accumulated << 1) | u32::from(r.read_bit()?);
        cur_len += 1;
        for tc in 0..rows {
            for to in 0..4 {
                let idx = tc * 4 + to;
                if lens[idx] == cur_len && u32::from(bits[idx]) == accumulated {
                    return Ok((tc as u8, to as u8));
                }
            }
        }
    }
    Err(CodecError::InvalidData(format!(
        "h264 cavlc: {label} coeff_token codeword not in table after {cur_len} bits"
    )))
}

/// Reads and decodes one complete residual block from the bitstream.
///
/// Composition: reads `coeff_token` via [`read_coeff_token`] then
/// forwards to [`decode_residual_block`] with the decoded
/// `(TotalCoeff, TrailingOnes)` pair.
///
/// # Errors
///
/// Propagates errors from [`read_coeff_token`] and
/// [`decode_residual_block`].
pub fn read_residual_block(
    r: &mut BitReader<'_>,
    kind: BlockKind,
    nc: i32,
) -> Result<ResidualBlock, CodecError> {
    let (total_coeff, trailing_ones) = read_coeff_token(r, kind, nc)?;
    decode_residual_block(r, kind, total_coeff, trailing_ones)
}

/// Decodes one residual block given an already-decoded `coeff_token`.
///
/// Exposed as a separate entry point so the level / total_zeros /
/// run_before machinery is testable independently of the
/// context-dependent `coeff_token` lookup tables (see
/// [`COEFF_TOKEN_TABLES_PENDING`]).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] for malformed bitstreams or
/// inconsistent parameters (e.g. `trailing_ones > total_coeff`).
pub fn decode_residual_block(
    r: &mut BitReader<'_>,
    block_kind: BlockKind,
    total_coeff: u8,
    trailing_ones: u8,
) -> Result<ResidualBlock, CodecError> {
    if total_coeff > block_kind.max_coefficients() {
        return Err(CodecError::InvalidData(format!(
            "h264 cavlc: total_coeff {total_coeff} exceeds block max {}",
            block_kind.max_coefficients()
        )));
    }
    if trailing_ones > 3 || trailing_ones > total_coeff {
        return Err(CodecError::InvalidData(format!(
            "h264 cavlc: trailing_ones {trailing_ones} invalid for total_coeff {total_coeff}"
        )));
    }

    let mut block = ResidualBlock {
        total_coeff,
        trailing_ones,
        ..Default::default()
    };

    if total_coeff == 0 {
        return Ok(block);
    }

    block.levels.reserve(total_coeff as usize);

    // Trailing ones: one sign bit each, in reverse scan order (highest
    // frequency first).
    for _ in 0..trailing_ones {
        let sign_bit = r.read_bit()?;
        block.levels.push(if sign_bit { -1 } else { 1 });
    }

    // Suffix length initialisation: per spec, the first non-trailing-one
    // level uses suffix_length = 0 unless TotalCoeff > 10 and
    // TrailingOnes < 3 (in which case the encoder skipped one
    // adaptation step, so start at 1).
    let mut suffix_length: u8 = if total_coeff > 10 && trailing_ones < 3 {
        1
    } else {
        0
    };

    let non_trailing = total_coeff - trailing_ones;
    for i in 0..non_trailing {
        let raw_level = read_level(r, suffix_length)?;
        // For the very first non-trailing-one level when there were
        // fewer than 3 trailing ones, the encoder adjusts the
        // magnitude so that the level can't be zero.  Apply the
        // matching decoder bias.
        let adjusted = if i == 0 && trailing_ones < 3 {
            if raw_level > 0 {
                raw_level + 1
            } else {
                raw_level - 1
            }
        } else {
            raw_level
        };
        block.levels.push(adjusted);
        suffix_length = update_suffix_length(suffix_length, adjusted);
    }

    // total_zeros — except when the block is full or has exactly one
    // coefficient (no zeros possible past the last non-zero).
    if total_coeff < block_kind.max_coefficients() {
        block.total_zeros = if block_kind.is_chroma_dc() {
            read_total_zeros_chroma_dc(r, total_coeff)?
        } else {
            read_total_zeros_luma(r, total_coeff)?
        };
    }

    // run_before — one fewer read than the number of levels.
    if total_coeff > 1 {
        block.runs.reserve((total_coeff - 1) as usize);
        let mut zeros_left = block.total_zeros;
        for _ in 0..(total_coeff - 1) {
            if zeros_left == 0 {
                block.runs.push(0);
                continue;
            }
            let run = read_run_before(r, zeros_left)?;
            block.runs.push(run);
            zeros_left = zeros_left.saturating_sub(run);
        }
    }

    Ok(block)
}

// ---------------------------------------------------------------------------
// VLC reader
// ---------------------------------------------------------------------------

/// Scans a flat `(len, bits)` table representation bit-by-bit
/// against the input stream.  Entry index `i` corresponds to decoded
/// value `i`.  Entries with `lens[i] == 0` are invalid and skipped.
fn scan_flat_vlc(
    r: &mut BitReader<'_>,
    lens: &[u8],
    bits: &[u8],
    label: &str,
) -> Result<u8, CodecError> {
    let max_len = lens.iter().copied().max().unwrap_or(0);
    let mut accumulated: u32 = 0;
    let mut cur_len: u8 = 0;
    while cur_len < max_len {
        accumulated = (accumulated << 1) | u32::from(r.read_bit()?);
        cur_len += 1;
        for (i, &len) in lens.iter().enumerate() {
            if len == cur_len && u32::from(bits[i]) == accumulated {
                return Ok(i as u8);
            }
        }
    }
    Err(CodecError::InvalidData(format!(
        "h264 cavlc: {label} codeword not in table after {cur_len} bits"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_bits(bits: &[bool]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bits.len().div_ceil(8));
        let mut byte = 0u8;
        let mut n = 0u8;
        for &b in bits {
            byte = (byte << 1) | u8::from(b);
            n += 1;
            if n == 8 {
                out.push(byte);
                byte = 0;
                n = 0;
            }
        }
        if n > 0 {
            out.push(byte << (8 - n));
        }
        out
    }

    fn push_bits(bits: &mut Vec<bool>, value: u32, len: u32) {
        for i in (0..len).rev() {
            bits.push((value >> i) & 1 != 0);
        }
    }

    #[test]
    fn max_coefficients_per_block_kind() {
        assert_eq!(BlockKind::Luma4x4.max_coefficients(), 16);
        assert_eq!(BlockKind::LumaDc16x16.max_coefficients(), 16);
        assert_eq!(BlockKind::LumaAc16x16.max_coefficients(), 15);
        assert_eq!(BlockKind::ChromaDc.max_coefficients(), 4);
        assert_eq!(BlockKind::ChromaAc.max_coefficients(), 15);
    }

    #[test]
    fn read_level_simple_positive_at_suffix_length_zero() {
        // suffix_length = 0, level_prefix = 0 -> level_code = 0 -> +1.
        // Encoding: codeword "1" (prefix 0 zeros, then terminator 1).
        let mut bits = Vec::new();
        bits.push(true); // level_prefix = 0
        let buf = pack_bits(&bits);
        let mut r = BitReader::new(&buf);
        let level = read_level(&mut r, 0).unwrap();
        assert_eq!(level, 1);
    }

    #[test]
    fn read_level_simple_negative_at_suffix_length_zero() {
        // level_prefix = 1 -> level_code = 1 -> -1.
        let mut bits = Vec::new();
        bits.push(false); // zero
        bits.push(true);  // terminator
        let buf = pack_bits(&bits);
        let mut r = BitReader::new(&buf);
        let level = read_level(&mut r, 0).unwrap();
        assert_eq!(level, -1);
    }

    #[test]
    fn read_level_with_suffix_at_suffix_length_one() {
        // suffix_length = 1, level_prefix = 0, suffix bit = 1
        //   -> level_code = (0 << 1) + 1 = 1 -> -1.
        let mut bits = Vec::new();
        bits.push(true);  // terminator
        bits.push(true);  // suffix = 1
        let buf = pack_bits(&bits);
        let mut r = BitReader::new(&buf);
        let level = read_level(&mut r, 1).unwrap();
        assert_eq!(level, -1);
    }

    #[test]
    fn suffix_length_increments_above_threshold() {
        // From suffix_length=1 (threshold = 3): reading a level of
        // magnitude 4 should bump suffix_length to 2.
        assert_eq!(update_suffix_length(1, 4), 2);
        // Magnitude 3 sits at the threshold -> no bump.
        assert_eq!(update_suffix_length(1, 3), 1);
        // From suffix_length=0, the spec forces a bump to 1 even before
        // the threshold check.
        assert_eq!(update_suffix_length(0, 1), 1);
        assert_eq!(update_suffix_length(0, 100), 2); // 0 -> 1, then bump
    }

    #[test]
    fn suffix_length_caps_at_six() {
        assert_eq!(update_suffix_length(6, 9999), 6);
    }

    #[test]
    fn read_run_before_short_table_zero() {
        // zeros_left = 1, codeword "1" -> run = 0.
        let buf = pack_bits(&[true]);
        let mut r = BitReader::new(&buf);
        assert_eq!(read_run_before(&mut r, 1).unwrap(), 0);
    }

    #[test]
    fn read_run_before_short_table_one() {
        // zeros_left = 1, codeword "0" -> run = 1.
        let buf = pack_bits(&[false]);
        let mut r = BitReader::new(&buf);
        assert_eq!(read_run_before(&mut r, 1).unwrap(), 1);
    }

    #[test]
    fn read_run_before_seven_plus_decodes_long_run() {
        // For zeros_left >= 7 the table has explicit entries for
        // runs 0..=14.  Run = 8 is codeword '00001' per the spec.
        let buf = pack_bits(&[false, false, false, false, true]);
        let mut r = BitReader::new(&buf);
        assert_eq!(read_run_before(&mut r, 10).unwrap(), 8);
    }

    #[test]
    fn run_before_zero_when_no_zeros_left() {
        let buf = [];
        let mut r = BitReader::new(&buf);
        assert_eq!(read_run_before(&mut r, 0).unwrap(), 0);
    }

    #[test]
    fn total_zeros_chroma_dc_tc1_round_trip() {
        // TC=1, codeword "001" -> total_zeros = 2.
        let buf = pack_bits(&[false, false, true]);
        let mut r = BitReader::new(&buf);
        assert_eq!(read_total_zeros_chroma_dc(&mut r, 1).unwrap(), 2);
    }

    #[test]
    fn total_zeros_chroma_dc_tc2_zero() {
        // TC=2, codeword "1" -> total_zeros = 0.
        let buf = pack_bits(&[true]);
        let mut r = BitReader::new(&buf);
        assert_eq!(read_total_zeros_chroma_dc(&mut r, 2).unwrap(), 0);
    }

    #[test]
    fn total_zeros_full_block_returns_zero_without_reading() {
        // total_coeff == max -> no read; should consume zero bits.
        let buf = [];
        let mut r = BitReader::new(&buf);
        assert_eq!(read_total_zeros_chroma_dc(&mut r, 4).unwrap(), 0);
        assert_eq!(r.bits_consumed(), 0);
    }

    #[test]
    fn empty_block_decodes_with_no_bits() {
        let buf = [];
        let mut r = BitReader::new(&buf);
        let block = decode_residual_block(&mut r, BlockKind::Luma4x4, 0, 0).unwrap();
        assert_eq!(block.total_coeff, 0);
        assert!(block.levels.is_empty());
        assert!(block.runs.is_empty());
        assert_eq!(block.total_zeros, 0);
        assert_eq!(r.bits_consumed(), 0);
    }

    #[test]
    fn single_trailing_one_block_just_reads_sign() {
        // total_coeff = 1, trailing_ones = 1, sign bit = 0 (positive).
        // Block is full of zeros except one ±1 at the end of scan order.
        // For a chroma DC block (max 4 coeffs) with total_coeff=1 the
        // total_zeros field IS present (could be 0..=3).  Encode
        // total_zeros = 0 ("1" in the TC=1 chroma DC table).
        let mut bits = Vec::new();
        bits.push(false); // trailing-one sign: positive
        bits.push(true);  // total_zeros = 0 codeword for chroma DC TC=1
        let buf = pack_bits(&bits);
        let mut r = BitReader::new(&buf);
        let block = decode_residual_block(&mut r, BlockKind::ChromaDc, 1, 1).unwrap();
        assert_eq!(block.total_coeff, 1);
        assert_eq!(block.trailing_ones, 1);
        assert_eq!(block.levels, vec![1]);
        assert_eq!(block.total_zeros, 0);
        assert!(block.runs.is_empty());
    }

    #[test]
    fn rejects_trailing_ones_exceeding_total_coeff() {
        let buf = [];
        let mut r = BitReader::new(&buf);
        assert!(
            decode_residual_block(&mut r, BlockKind::ChromaDc, 1, 2).is_err(),
            "trailing_ones > total_coeff must error"
        );
    }

    #[test]
    fn rejects_total_coeff_above_block_max() {
        let buf = [];
        let mut r = BitReader::new(&buf);
        // chroma DC has max 4 — try 5.
        assert!(
            decode_residual_block(&mut r, BlockKind::ChromaDc, 5, 0).is_err(),
            "total_coeff > block max must error"
        );
    }

    #[test]
    fn read_coeff_token_zero_zero_at_low_nc() {
        // For nC < 2 the (TotalCoeff=0, TrailingOnes=0) codeword is
        // the single bit '1'.
        let buf = pack_bits(&[true]);
        let mut r = BitReader::new(&buf);
        let (tc, to) = read_coeff_token(&mut r, BlockKind::Luma4x4, 0).expect("decode");
        assert_eq!((tc, to), (0, 0));
    }

    #[test]
    fn read_coeff_token_chroma_dc_zero_zero() {
        // For chroma DC the (0, 0) codeword is '01' (length 2).
        let buf = pack_bits(&[false, true]);
        let mut r = BitReader::new(&buf);
        let (tc, to) = read_coeff_token(&mut r, BlockKind::ChromaDc, -1).expect("decode");
        assert_eq!((tc, to), (0, 0));
    }

    #[test]
    fn read_coeff_token_high_nc_fixed_length() {
        // For nC >= 8 the table is a 6-bit fixed-length code.
        // Decimal 3 (binary 000011) decodes to (0, 0).
        let buf = pack_bits(&[false, false, false, false, true, true]);
        let mut r = BitReader::new(&buf);
        let (tc, to) = read_coeff_token(&mut r, BlockKind::Luma4x4, 8).expect("decode");
        assert_eq!((tc, to), (0, 0));
    }

    #[test]
    fn to_scan_order_for_empty_block() {
        let block = ResidualBlock::default();
        assert!(block.to_scan_order().is_empty());
    }

    #[test]
    fn to_scan_order_single_coefficient() {
        let block = ResidualBlock {
            total_coeff: 1,
            trailing_ones: 1,
            levels: vec![1],
            runs: vec![],
            total_zeros: 0,
        };
        // Result: low-frequency-first; one non-zero, no leading zeros.
        // The block has 1 non-zero + 0 zeros = length 1.
        assert_eq!(block.to_scan_order(), vec![1]);
    }
}

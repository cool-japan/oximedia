//! H.264 Picture Parameter Set (PPS) parsing.
//!
//! Implements the syntax of `pic_parameter_set_rbsp()` from
//! H.264 §7.3.2.2.  The PPS configures per-picture coding parameters
//! such as the entropy coder (CAVLC vs CABAC), default reference list
//! sizes, default QP, deblock filter controls, and constrained intra
//! prediction.
//!
//! Slice-group maps (FMO) are recognised but not decoded in detail —
//! `num_slice_groups_minus1 == 0` (the universal case for non-FMO
//! streams) is fully supported; non-zero values are accepted but the
//! slice-group syntax is bit-skipped without retention.

use crate::h264::bit_reader::BitReader;
use crate::CodecError;

/// Parsed H.264 Picture Parameter Set.
#[derive(Debug, Clone)]
pub struct PpsRbsp {
    /// Identifier referenced by slice headers.
    pub pic_parameter_set_id: u32,
    /// References an SPS by `seq_parameter_set_id`.
    pub seq_parameter_set_id: u32,
    /// True for CABAC, false for CAVLC.  Baseline-profile streams
    /// always set this false.
    pub entropy_coding_mode_flag: bool,
    /// True when `pic_order_cnt_lsb` is also signalled for the bottom
    /// field of a coded field pair.
    pub bottom_field_pic_order_in_frame_present_flag: bool,
    /// Number of slice groups - 1.  Almost always 0 (single slice group).
    pub num_slice_groups_minus1: u32,
    /// Default size for ref list 0 - 1 when the slice header does not
    /// override.
    pub num_ref_idx_l0_default_active_minus1: u32,
    /// Default size for ref list 1 - 1.
    pub num_ref_idx_l1_default_active_minus1: u32,
    /// Enables explicit weighted prediction for P-slices.
    pub weighted_pred_flag: bool,
    /// 0 = no weighted pred for B, 1 = explicit, 2 = implicit.
    pub weighted_bipred_idc: u8,
    /// Initial QP for a slice, computed as `pic_init_qp_minus26 + 26`.
    pub pic_init_qp_minus26: i32,
    /// Initial QSp; rarely used.
    pub pic_init_qs_minus26: i32,
    /// Offset applied to chroma QP relative to luma QP.
    pub chroma_qp_index_offset: i32,
    /// True when the slice header may override deblocking filter
    /// parameters.
    pub deblocking_filter_control_present_flag: bool,
    /// True when intra blocks may not reference inter neighbours
    /// (improves error resilience).
    pub constrained_intra_pred_flag: bool,
    /// True when `redundant_pic_cnt` is signalled in slice headers.
    pub redundant_pic_cnt_present_flag: bool,
    /// True when transform_8x8 mode is enabled (High Profile and up).
    /// Optional in the bitstream; defaults to `false`.
    pub transform_8x8_mode_flag: bool,
    /// True when explicit scaling lists follow.  The lists themselves
    /// are consumed but not currently retained.
    pub pic_scaling_matrix_present_flag: bool,
    /// Offset applied to the second chroma component.  Present only
    /// when transform_8x8 mode is enabled; defaults to
    /// `chroma_qp_index_offset` otherwise.
    pub second_chroma_qp_index_offset: i32,
}

/// Parses a PPS from its RBSP payload (after emulation prevention has
/// been stripped).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] on malformed bitstreams.
pub fn parse_pps(rbsp: &[u8]) -> Result<PpsRbsp, CodecError> {
    let mut r = BitReader::new(rbsp);

    let pic_parameter_set_id = r.read_ue()?;
    let seq_parameter_set_id = r.read_ue()?;
    let entropy_coding_mode_flag = r.read_bit()?;
    let bottom_field_pic_order_in_frame_present_flag = r.read_bit()?;
    let num_slice_groups_minus1 = r.read_ue()?;

    if num_slice_groups_minus1 > 0 {
        // FMO map types 0..6.  We consume them but do not retain the
        // map for now — virtually all production H.264 sets this to 0.
        let slice_group_map_type = r.read_ue()?;
        match slice_group_map_type {
            0 => {
                for _ in 0..=num_slice_groups_minus1 {
                    let _run_length_minus1 = r.read_ue()?;
                }
            }
            2 => {
                for _ in 0..num_slice_groups_minus1 {
                    let _top_left = r.read_ue()?;
                    let _bottom_right = r.read_ue()?;
                }
            }
            3..=5 => {
                let _slice_group_change_direction_flag = r.read_bit()?;
                let _slice_group_change_rate_minus1 = r.read_ue()?;
            }
            6 => {
                let pic_size_in_map_units_minus1 = r.read_ue()?;
                let bits_per_entry =
                    ceil_log2(num_slice_groups_minus1 + 1).max(1);
                for _ in 0..=pic_size_in_map_units_minus1 {
                    let _slice_group_id = r.read_bits(bits_per_entry)?;
                }
            }
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 pps: unsupported slice_group_map_type {slice_group_map_type}"
                )));
            }
        }
    }

    let num_ref_idx_l0_default_active_minus1 = r.read_ue()?;
    let num_ref_idx_l1_default_active_minus1 = r.read_ue()?;
    let weighted_pred_flag = r.read_bit()?;
    let weighted_bipred_idc = r.read_bits(2)? as u8;
    let pic_init_qp_minus26 = r.read_se()?;
    let pic_init_qs_minus26 = r.read_se()?;
    let chroma_qp_index_offset = r.read_se()?;
    let deblocking_filter_control_present_flag = r.read_bit()?;
    let constrained_intra_pred_flag = r.read_bit()?;
    let redundant_pic_cnt_present_flag = r.read_bit()?;

    // The fields beyond this point are optional High-profile extensions.
    // They are present iff `more_rbsp_data()` returns true (i.e. there
    // are still meaningful bits before the RBSP stop bit).
    let mut transform_8x8_mode_flag = false;
    let mut pic_scaling_matrix_present_flag = false;
    let mut second_chroma_qp_index_offset = chroma_qp_index_offset;
    if r.more_rbsp_data() {
        transform_8x8_mode_flag = r.read_bit()?;
        pic_scaling_matrix_present_flag = r.read_bit()?;
        if pic_scaling_matrix_present_flag {
            let lists = 6 + if transform_8x8_mode_flag { 2 } else { 0 };
            for i in 0..lists {
                let present = r.read_bit()?;
                if present {
                    let size = if i < 6 { 16 } else { 64 };
                    skip_scaling_list(&mut r, size)?;
                }
            }
        }
        second_chroma_qp_index_offset = r.read_se()?;
    }

    Ok(PpsRbsp {
        pic_parameter_set_id,
        seq_parameter_set_id,
        entropy_coding_mode_flag,
        bottom_field_pic_order_in_frame_present_flag,
        num_slice_groups_minus1,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        weighted_pred_flag,
        weighted_bipred_idc,
        pic_init_qp_minus26,
        pic_init_qs_minus26,
        chroma_qp_index_offset,
        deblocking_filter_control_present_flag,
        constrained_intra_pred_flag,
        redundant_pic_cnt_present_flag,
        transform_8x8_mode_flag,
        pic_scaling_matrix_present_flag,
        second_chroma_qp_index_offset,
    })
}

fn ceil_log2(v: u32) -> u32 {
    if v <= 1 {
        0
    } else {
        32 - (v - 1).leading_zeros()
    }
}

fn skip_scaling_list(r: &mut BitReader<'_>, size: u32) -> Result<(), CodecError> {
    let mut last_scale: i32 = 8;
    let mut next_scale: i32 = 8;
    for _ in 0..size {
        if next_scale != 0 {
            let delta_scale = r.read_se()?;
            next_scale = ((last_scale + delta_scale + 256) % 256) as i32;
        }
        if next_scale != 0 {
            last_scale = next_scale;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal Baseline-style PPS: CAVLC, single slice group,
    /// QP 28, no deblock override, no transform_8x8.
    fn build_synthetic_pps() -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        // pic_parameter_set_id = 0
        push_ue(&mut bits, 0);
        // seq_parameter_set_id = 0
        push_ue(&mut bits, 0);
        // entropy_coding_mode_flag = 0 (CAVLC)
        bits.push(false);
        // bottom_field_pic_order_in_frame_present_flag = 0
        bits.push(false);
        // num_slice_groups_minus1 = 0
        push_ue(&mut bits, 0);
        // num_ref_idx_l0_default_active_minus1 = 0
        push_ue(&mut bits, 0);
        // num_ref_idx_l1_default_active_minus1 = 0
        push_ue(&mut bits, 0);
        // weighted_pred_flag = 0
        bits.push(false);
        // weighted_bipred_idc = 0
        bits.push(false);
        bits.push(false);
        // pic_init_qp_minus26 = 2  (i.e. initial QP 28) -> se(2) = ue(3) = `00100`
        push_se(&mut bits, 2);
        // pic_init_qs_minus26 = 0
        push_se(&mut bits, 0);
        // chroma_qp_index_offset = 0
        push_se(&mut bits, 0);
        // deblocking_filter_control_present_flag = 1
        bits.push(true);
        // constrained_intra_pred_flag = 0
        bits.push(false);
        // redundant_pic_cnt_present_flag = 0
        bits.push(false);
        // RBSP stop bit
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        pack_bits_msb(&bits)
    }

    #[test]
    fn parses_minimal_pps() {
        let payload = build_synthetic_pps();
        let pps = parse_pps(&payload).expect("synthetic PPS should parse");
        assert_eq!(pps.pic_parameter_set_id, 0);
        assert_eq!(pps.seq_parameter_set_id, 0);
        assert!(!pps.entropy_coding_mode_flag);
        assert_eq!(pps.num_slice_groups_minus1, 0);
        assert_eq!(pps.pic_init_qp_minus26, 2);
        assert!(pps.deblocking_filter_control_present_flag);
        assert!(!pps.constrained_intra_pred_flag);
        // No High-profile extensions present.
        assert!(!pps.transform_8x8_mode_flag);
        assert!(!pps.pic_scaling_matrix_present_flag);
    }

    #[test]
    fn ceil_log2_matches_table() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(8), 3);
        assert_eq!(ceil_log2(9), 4);
    }

    // --- shared bit-building helpers (duplicated from sps.rs tests
    //     intentionally so each test module is self-contained) ---

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

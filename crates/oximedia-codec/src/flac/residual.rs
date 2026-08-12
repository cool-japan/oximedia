//! Partitioned Rice coding of FLAC residuals (RFC 9639 §9.2.7).
//!
//! ```text
//! <2>  residual coding method  (0 = 4-bit parameters, 1 = 5-bit parameters)
//! <4>  partition order p       (2^p partitions)
//! per partition:
//!   <4|5> Rice parameter, or the all-ones escape code
//!   escaped:   <5> raw bit width, then that many bits per residual
//!   otherwise: Rice codes (unary quotient terminated by a 1 bit + k low bits)
//! ```
//!
//! The first partition holds `(block_size >> p) - predictor_order` residuals;
//! every later partition holds `block_size >> p`.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

use super::bitio::{BitReader, BitWriter};
use super::rice::{read_rice_signed, write_rice_signed, zigzag_encode};
use crate::error::{CodecError, CodecResult};

/// Largest partition order representable in the 4-bit field.
pub const MAX_PARTITION_ORDER: u32 = 15;

/// Partition order ceiling used by the encoder's search.
pub const DEFAULT_MAX_PARTITION_ORDER: u32 = 8;

/// Largest Rice parameter for coding method 0 (4-bit field; 15 escapes).
const METHOD0_MAX_PARAM: u32 = 14;

/// Largest Rice parameter for coding method 1 (5-bit field; 31 escapes).
const METHOD1_MAX_PARAM: u32 = 30;

/// Largest raw bit width an escaped partition can declare (5-bit field).
const MAX_ESCAPE_BITS: u32 = 31;

// =============================================================================
// Plan
// =============================================================================

/// How one residual partition will be coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartitionPlan {
    /// Rice parameter (ignored when `raw_bits` is set).
    pub param: u32,
    /// `Some(n)` when the partition is escape-coded with `n` raw bits/sample.
    pub raw_bits: Option<u32>,
}

/// A fully determined coded-residual layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidualPlan {
    /// Residual coding method: 0 (4-bit parameters) or 1 (5-bit parameters).
    pub method: u32,
    /// Partition order (`2^order` partitions).
    pub partition_order: u32,
    /// Per-partition coding decisions, in stream order.
    pub partitions: Vec<PartitionPlan>,
    /// Exact serialised size in bits, including the 6-bit residual header.
    pub bits: usize,
}

/// Bit width needed to store `v` as a two's-complement signed integer.
#[must_use]
fn signed_bit_width(v: i32) -> u32 {
    if v == 0 {
        0
    } else if v > 0 {
        33 - (v as u32).leading_zeros()
    } else {
        33 - ((!v) as u32).leading_zeros()
    }
}

/// Choose the cheapest partitioning + Rice parameters for `residual`.
///
/// `residual` must hold exactly `block_size - predictor_order` values.
///
/// # Errors
///
/// Returns `CodecError::InvalidParameter` when the residual length does not
/// match `block_size`/`predictor_order`.
pub fn plan_residual(
    residual: &[i32],
    block_size: usize,
    predictor_order: usize,
    max_partition_order: u32,
) -> CodecResult<ResidualPlan> {
    if block_size < predictor_order || residual.len() + predictor_order != block_size {
        return Err(CodecError::InvalidParameter(format!(
            "FLAC residual length {} does not match block size {block_size} at order {predictor_order}",
            residual.len()
        )));
    }

    // Highest partition order that divides the block and leaves the first
    // partition non-empty.
    let mut order_max = 0u32;
    for p in 1..=max_partition_order.min(MAX_PARTITION_ORDER) {
        let parts = 1usize << p;
        if block_size % parts != 0 {
            break;
        }
        if block_size / parts <= predictor_order {
            break;
        }
        order_max = p;
    }

    let folded: Vec<u32> = residual.iter().map(|&r| zigzag_encode(r)).collect();

    // Statistics for the finest partitioning; coarser levels are sums of these.
    let fine_count = 1usize << order_max;
    let fine_len = block_size >> order_max;
    let mut fine_sums: Vec<Vec<u64>> = Vec::with_capacity(fine_count);
    let mut fine_lens: Vec<usize> = Vec::with_capacity(fine_count);
    let mut fine_raw: Vec<u32> = Vec::with_capacity(fine_count);
    for j in 0..fine_count {
        let start = if j == 0 {
            0
        } else {
            j * fine_len - predictor_order
        };
        let end = (j + 1) * fine_len - predictor_order;
        let slice = &folded[start..end];
        fine_lens.push(slice.len());
        fine_raw.push(
            residual[start..end]
                .iter()
                .map(|&v| signed_bit_width(v))
                .max()
                .unwrap_or(0),
        );

        // Σ (u >> k) for k = 0, 1, ... until the sum reaches zero.  The
        // sequence is non-increasing, so a trailing zero means every larger
        // parameter also sums to zero.
        let mut sums = Vec::with_capacity(8);
        let mut k = 0u32;
        loop {
            let s: u64 = slice.iter().map(|&u| u64::from(u >> k)).sum();
            sums.push(s);
            if s == 0 || k == METHOD1_MAX_PARAM {
                break;
            }
            k += 1;
        }
        fine_sums.push(sums);
    }

    let mut best: Option<ResidualPlan> = None;
    for p in 0..=order_max {
        let parts = 1usize << p;
        let group = fine_count >> p;
        let mut partitions = Vec::with_capacity(parts);
        let mut payload_bits = 0usize;
        let mut max_param = 0u32;

        for part in 0..parts {
            let range = part * group..(part + 1) * group;
            let count: usize = fine_lens[range.clone()].iter().sum();
            let raw = fine_raw[range.clone()].iter().copied().max().unwrap_or(0);
            let k_limit = fine_sums[range.clone()]
                .iter()
                .map(|s| s.len() as u32 - 1)
                .max()
                .unwrap_or(0);

            let mut best_param = 0u32;
            let mut best_cost = usize::MAX;
            for k in 0..=k_limit.min(METHOD1_MAX_PARAM) {
                let sum: u64 = fine_sums[range.clone()]
                    .iter()
                    .map(|s| s.get(k as usize).copied().unwrap_or(0))
                    .sum();
                let cost = sum as usize + count * (k as usize + 1);
                if cost < best_cost {
                    best_cost = cost;
                    best_param = k;
                }
            }
            // Escape-coded alternative: 5-bit width + fixed-width samples.
            let escape_cost = if raw <= MAX_ESCAPE_BITS {
                Some(5 + count * raw as usize)
            } else {
                None
            };
            match escape_cost {
                Some(ec) if ec < best_cost => {
                    partitions.push(PartitionPlan {
                        param: 0,
                        raw_bits: Some(raw),
                    });
                    payload_bits += ec;
                }
                _ => {
                    partitions.push(PartitionPlan {
                        param: best_param,
                        raw_bits: None,
                    });
                    payload_bits += best_cost;
                    max_param = max_param.max(best_param);
                }
            }
        }

        // Method 0 uses 4-bit parameter fields; method 1 uses 5-bit ones.
        // Escapes are expressible under either method (code 15 or 31).
        let method = if max_param <= METHOD0_MAX_PARAM { 0 } else { 1 };
        let param_field = if method == 0 { 4 } else { 5 };
        let bits = 2 + 4 + parts * param_field + payload_bits;

        if best.as_ref().is_none_or(|b| bits < b.bits) {
            best = Some(ResidualPlan {
                method,
                partition_order: p,
                partitions,
                bits,
            });
        }
    }

    best.ok_or_else(|| {
        CodecError::Internal("FLAC: no viable residual partitioning found".to_string())
    })
}

// =============================================================================
// Serialisation
// =============================================================================

/// Serialise `residual` according to `plan`.
///
/// # Errors
///
/// Returns `CodecError::Internal` when the plan does not match the data.
pub fn write_residual(
    w: &mut BitWriter,
    residual: &[i32],
    block_size: usize,
    predictor_order: usize,
    plan: &ResidualPlan,
) -> CodecResult<()> {
    let parts = 1usize << plan.partition_order;
    if plan.partitions.len() != parts {
        return Err(CodecError::Internal(
            "FLAC: residual plan partition count mismatch".to_string(),
        ));
    }
    let part_len = block_size >> plan.partition_order;
    let param_field = if plan.method == 0 { 4 } else { 5 };
    let escape_code = if plan.method == 0 { 0b1111 } else { 0b1_1111 };

    let start_bits = w.bit_len();
    w.write_bits(u64::from(plan.method), 2);
    w.write_bits(u64::from(plan.partition_order), 4);

    let mut pos = 0usize;
    for (index, part) in plan.partitions.iter().enumerate() {
        let count = if index == 0 {
            part_len - predictor_order
        } else {
            part_len
        };
        let end = pos + count;
        if end > residual.len() {
            return Err(CodecError::Internal(
                "FLAC: residual plan overruns the residual buffer".to_string(),
            ));
        }
        let slice = &residual[pos..end];
        pos = end;

        match part.raw_bits {
            Some(raw) => {
                w.write_bits(escape_code, param_field);
                w.write_bits(u64::from(raw), 5);
                if raw > 0 {
                    for &v in slice {
                        w.write_bits_signed(i64::from(v), raw);
                    }
                }
            }
            None => {
                w.write_bits(u64::from(part.param), param_field);
                for &v in slice {
                    write_rice_signed(w, v, part.param);
                }
            }
        }
    }

    if pos != residual.len() {
        return Err(CodecError::Internal(
            "FLAC: residual plan did not consume every residual".to_string(),
        ));
    }
    let written = w.bit_len() - start_bits;
    if written != plan.bits {
        return Err(CodecError::Internal(format!(
            "FLAC: residual plan predicted {} bits but wrote {written}",
            plan.bits
        )));
    }
    Ok(())
}

/// Parse a coded residual block.
///
/// # Errors
///
/// Returns `CodecError::InvalidData` for reserved methods, impossible
/// partitionings or truncated input.
pub fn read_residual(
    r: &mut BitReader<'_>,
    block_size: usize,
    predictor_order: usize,
) -> CodecResult<Vec<i32>> {
    let method = r
        .read_bits(2)
        .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading residual method".to_string()))?;
    if method > 1 {
        return Err(CodecError::InvalidData(format!(
            "FLAC: reserved residual coding method {method}"
        )));
    }
    let partition_order = r.read_bits(4).ok_or_else(|| {
        CodecError::InvalidData("FLAC: EOF reading residual partition order".to_string())
    })? as u32;

    let parts = 1usize << partition_order;
    if block_size % parts != 0 {
        return Err(CodecError::InvalidData(format!(
            "FLAC: block size {block_size} is not divisible into {parts} partitions"
        )));
    }
    let part_len = block_size / parts;
    if part_len < predictor_order {
        return Err(CodecError::InvalidData(format!(
            "FLAC: partition length {part_len} is shorter than predictor order {predictor_order}"
        )));
    }

    let param_field = if method == 0 { 4 } else { 5 };
    let escape_code = if method == 0 { 0b1111 } else { 0b1_1111 };

    let mut out = Vec::with_capacity(block_size - predictor_order);
    for index in 0..parts {
        let count = if index == 0 {
            part_len - predictor_order
        } else {
            part_len
        };
        let param = r.read_bits(param_field).ok_or_else(|| {
            CodecError::InvalidData("FLAC: EOF reading Rice parameter".to_string())
        })? as u32;

        if param == escape_code {
            let raw = r.read_bits(5).ok_or_else(|| {
                CodecError::InvalidData("FLAC: EOF reading escaped partition width".to_string())
            })? as u32;
            for _ in 0..count {
                let v = if raw == 0 {
                    0
                } else {
                    r.read_bits_signed(raw).ok_or_else(|| {
                        CodecError::InvalidData(
                            "FLAC: EOF reading escaped residual sample".to_string(),
                        )
                    })?
                };
                out.push(i32::try_from(v).map_err(|_| {
                    CodecError::InvalidData(
                        "FLAC: escaped residual sample exceeds 32 bits".to_string(),
                    )
                })?);
            }
        } else {
            for _ in 0..count {
                out.push(read_rice_signed(r, param).ok_or_else(|| {
                    CodecError::InvalidData("FLAC: truncated Rice-coded residual".to_string())
                })?);
            }
        }
    }

    Ok(out)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(residual: &[i32], block_size: usize, order: usize) -> ResidualPlan {
        let plan = plan_residual(residual, block_size, order, DEFAULT_MAX_PARTITION_ORDER)
            .expect("plan residual");
        let mut w = BitWriter::new();
        write_residual(&mut w, residual, block_size, order, &plan).expect("write residual");
        let bits = w.bit_len();
        assert_eq!(bits, plan.bits, "planned bits must match written bits");
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let decoded = read_residual(&mut r, block_size, order).expect("read residual");
        assert_eq!(decoded, residual, "residual must round-trip exactly");
        assert_eq!(r.bit_pos(), bits, "reader must consume exactly the payload");
        plan
    }

    #[test]
    fn residual_round_trips_all_zero() {
        let residual = vec![0i32; 1024 - 4];
        let plan = round_trip(&residual, 1024, 4);
        assert_eq!(plan.partitions[0].param, 0);
    }

    #[test]
    fn residual_round_trips_small_values() {
        let residual: Vec<i32> = (0..(4096 - 8))
            .map(|i| ((i * 37) % 21) as i32 - 10)
            .collect();
        round_trip(&residual, 4096, 8);
    }

    #[test]
    fn residual_round_trips_extreme_values() {
        let mut residual: Vec<i32> = vec![0; 256 - 2];
        residual[0] = i32::MAX / 2;
        residual[1] = -(i32::MAX / 2);
        residual[10] = 1 << 30;
        residual[11] = -(1 << 30);
        round_trip(&residual, 256, 2);
    }

    #[test]
    fn residual_round_trips_zero_order() {
        let residual: Vec<i32> = (0..192).map(|i| (i % 7) as i32 - 3).collect();
        round_trip(&residual, 192, 0);
    }

    #[test]
    fn residual_round_trips_uncommon_block_sizes() {
        for &(bs, order) in &[(17usize, 1usize), (33, 2), (100, 4), (999, 8), (1000, 3)] {
            let residual: Vec<i32> = (0..bs - order).map(|i| (i % 13) as i32 - 6).collect();
            round_trip(&residual, bs, order);
        }
    }

    #[test]
    fn residual_uses_escape_for_uniform_wide_values() {
        // Values whose folded magnitude is huge and uniform: escape coding is
        // dramatically cheaper than unary quotients.
        let residual: Vec<i32> = (0..1024 - 2).map(|i| 1 << 24 | (i as i32 & 0xFF)).collect();
        let plan = round_trip(&residual, 1024, 2);
        assert!(
            plan.partitions.iter().any(|p| p.raw_bits.is_some()),
            "expected at least one escaped partition, got {plan:?}"
        );
    }

    #[test]
    fn residual_partitioning_beats_single_partition_for_mixed_statistics() {
        // First half quiet, second half loud → partitioning should win.
        let mut residual: Vec<i32> = vec![0; 2048 - 4];
        for (i, v) in residual.iter_mut().enumerate() {
            *v = if i < 1000 { (i % 3) as i32 } else { 5000 };
        }
        let plan = round_trip(&residual, 2048, 4);
        assert!(
            plan.partition_order > 0,
            "expected partitioning, got order 0 ({} bits)",
            plan.bits
        );
    }

    #[test]
    fn residual_plan_rejects_length_mismatch() {
        let residual = vec![0i32; 10];
        assert!(plan_residual(&residual, 1024, 4, DEFAULT_MAX_PARTITION_ORDER).is_err());
    }

    #[test]
    fn read_rejects_reserved_method() {
        let mut w = BitWriter::new();
        w.write_bits(2, 2);
        w.write_bits(0, 4);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(read_residual(&mut r, 16, 0).is_err());
    }

    #[test]
    fn read_rejects_impossible_partitioning() {
        let mut w = BitWriter::new();
        w.write_bits(0, 2);
        w.write_bits(4, 4); // 16 partitions
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        // 17 samples cannot be split into 16 equal partitions.
        assert!(read_residual(&mut r, 17, 0).is_err());
    }

    #[test]
    fn signed_bit_width_matches_two_complement_range() {
        assert_eq!(signed_bit_width(0), 0);
        assert_eq!(signed_bit_width(-1), 1);
        assert_eq!(signed_bit_width(1), 2);
        assert_eq!(signed_bit_width(-2), 2);
        assert_eq!(signed_bit_width(2), 3);
        assert_eq!(signed_bit_width(-4), 3);
        assert_eq!(signed_bit_width(i32::MAX), 32);
        assert_eq!(signed_bit_width(i32::MIN), 32);
        for v in [-1000i32, -7, -1, 0, 1, 7, 1000, 65535, -65536] {
            let n = signed_bit_width(v);
            if n == 0 {
                assert_eq!(v, 0);
            } else if n < 32 {
                let lo = -(1i64 << (n - 1));
                let hi = (1i64 << (n - 1)) - 1;
                assert!((lo..=hi).contains(&i64::from(v)), "{v} in {n} bits");
                if n > 1 {
                    let lo2 = -(1i64 << (n - 2));
                    let hi2 = (1i64 << (n - 2)) - 1;
                    assert!(
                        !(lo2..=hi2).contains(&i64::from(v)),
                        "{v} should not fit in {} bits",
                        n - 1
                    );
                }
            }
        }
    }
}

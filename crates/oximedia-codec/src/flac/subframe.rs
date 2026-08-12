//! FLAC subframes — planning, serialisation and parsing (RFC 9639 §9.2).
//!
//! ```text
//! <1>     zero bit (subframe header sync)
//! <6>     subframe type
//! <1>     wasted-bits flag; when set, a unary count of (wasted - 1) follows
//! <?>     subframe body, per type:
//!           constant : one sample of `bit_depth` bits
//!           verbatim : `block_size` samples of `bit_depth` bits
//!           fixed    : `order` warm-up samples + coded residual
//!           LPC      : `order` warm-up samples, <4> precision-1, <5> shift,
//!                      `order` coefficients of `precision` bits, residual
//! ```
//!
//! Everything here is bit-packed with no byte alignment; only the frame footer
//! is aligned.  The encoder and decoder share the residual and predictor code
//! so the two directions cannot drift.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]

use super::bitio::{BitReader, BitWriter};
use super::lpc::{
    autocorrelate, expected_bits_per_residual_sample, fixed_residual, fixed_restore,
    lpc_coefficients_all, lpc_residual, lpc_restore_exact, quantise_lpc_coeffs, tukey_window,
    MAX_FIXED_ORDER, MAX_QLP_PRECISION, MAX_QLP_SHIFT,
};
use super::residual::{plan_residual, read_residual, write_residual, ResidualPlan};
use crate::error::{CodecError, CodecResult};

/// Largest LPC order the format can express (6-bit type field).
pub const MAX_LPC_ORDER: usize = 32;

const TYPE_CONSTANT: u32 = 0b00_0000;
const TYPE_VERBATIM: u32 = 0b00_0001;
const TYPE_FIXED_BASE: u32 = 0b00_1000;
const TYPE_LPC_BASE: u32 = 0b10_0000;

// =============================================================================
// Encoder-side plan
// =============================================================================

/// The chosen body for one subframe.
#[derive(Clone, Debug)]
pub enum SubframeBody {
    /// Every sample is identical.
    Constant(i32),
    /// Samples stored unencoded.
    Verbatim,
    /// Fixed polynomial predictor of `order` (0..=4).
    Fixed {
        /// Predictor order.
        order: usize,
        /// Prediction residuals.
        residual: Vec<i32>,
        /// Coded-residual layout.
        plan: ResidualPlan,
    },
    /// Quantised linear predictor.
    Lpc {
        /// Quantised coefficients, most-recent sample first.
        qlp: Vec<i32>,
        /// Coefficient precision in bits.
        precision: u32,
        /// Prediction right shift.
        shift: u32,
        /// Prediction residuals.
        residual: Vec<i32>,
        /// Coded-residual layout.
        plan: ResidualPlan,
    },
}

/// A fully planned subframe, ready to serialise.
#[derive(Clone, Debug)]
pub struct SubframePlan {
    /// Number of wasted (always-zero) low bits removed from every sample.
    pub wasted: u32,
    /// Exact serialised size in bits, including the subframe header.
    pub bits: usize,
    /// The chosen body.
    pub body: SubframeBody,
    /// Samples after removing the wasted low bits.
    shifted: Vec<i32>,
}

impl SubframePlan {
    /// Samples with the wasted low bits removed (what the body actually codes).
    #[must_use]
    pub fn shifted_samples(&self) -> &[i32] {
        &self.shifted
    }
}

/// Coefficient precision heuristic, following libFLAC's automatic mode.
#[must_use]
fn auto_precision(bits_per_sample: u32, block_size: usize) -> u32 {
    let precision = if bits_per_sample < 16 {
        (2 + bits_per_sample / 2).max(5)
    } else if bits_per_sample == 16 {
        match block_size {
            0..=192 => 7,
            193..=384 => 8,
            385..=576 => 9,
            577..=1152 => 10,
            1153..=2304 => 11,
            2305..=4608 => 12,
            _ => 13,
        }
    } else {
        match block_size {
            0..=384 => 13,
            385..=1152 => 14,
            _ => 15,
        }
    };
    precision.min(MAX_QLP_PRECISION)
}

fn header_bits(wasted: u32) -> usize {
    // zero bit + 6 type bits + wasted flag, then the unary count when present.
    1 + 6 + 1 + if wasted > 0 { wasted as usize } else { 0 }
}

/// Number of bits needed to hold `value` as a signed integer.
fn fits_signed(value: i32, bits: u32) -> bool {
    if bits == 0 {
        return value == 0;
    }
    if bits >= 32 {
        return true;
    }
    let lo = -(1i64 << (bits - 1));
    let hi = (1i64 << (bits - 1)) - 1;
    (lo..=hi).contains(&i64::from(value))
}

/// Plan the cheapest representation of one subframe.
///
/// `bits_per_sample` is the subframe bit depth, i.e. the frame bit depth plus
/// one for a side channel.
///
/// # Errors
///
/// Returns `CodecError::InvalidParameter` when a sample does not fit in
/// `bits_per_sample`, or the block is empty.
pub fn plan_subframe(
    samples: &[i32],
    bits_per_sample: u32,
    max_lpc_order: usize,
    max_partition_order: u32,
) -> CodecResult<SubframePlan> {
    if samples.is_empty() {
        return Err(CodecError::InvalidParameter(
            "FLAC: cannot encode an empty subframe".to_string(),
        ));
    }
    if !(1..=32).contains(&bits_per_sample) {
        return Err(CodecError::InvalidParameter(format!(
            "FLAC: unsupported subframe bit depth {bits_per_sample}"
        )));
    }
    if let Some(bad) = samples.iter().find(|&&s| !fits_signed(s, bits_per_sample)) {
        return Err(CodecError::InvalidParameter(format!(
            "FLAC: sample {bad} does not fit in {bits_per_sample} bits"
        )));
    }

    // A constant subframe already costs one sample; removing wasted bits can
    // only add the unary count, so check for it before shifting anything.
    if samples.iter().all(|&s| s == samples[0]) {
        return Ok(SubframePlan {
            wasted: 0,
            bits: header_bits(0) + bits_per_sample as usize,
            body: SubframeBody::Constant(samples[0]),
            shifted: samples.to_vec(),
        });
    }

    // Wasted bits: low-order zeros shared by every non-zero sample.
    let wasted = samples
        .iter()
        .filter(|&&s| s != 0)
        .map(|&s| s.trailing_zeros())
        .min()
        .unwrap_or(0)
        .min(bits_per_sample.saturating_sub(1));
    let shifted: Vec<i32> = if wasted > 0 {
        samples.iter().map(|&s| s >> wasted).collect()
    } else {
        samples.to_vec()
    };
    let depth = bits_per_sample - wasted;
    let n = shifted.len();

    let mut best = SubframePlan {
        wasted,
        bits: header_bits(wasted) + n * depth as usize,
        body: SubframeBody::Verbatim,
        shifted: shifted.clone(),
    };

    // --- Fixed predictors -------------------------------------------------
    if let Some((order, residual)) = best_fixed_predictor(&shifted, depth) {
        if let Ok(plan) = plan_residual(&residual, n, order, max_partition_order) {
            let bits = header_bits(wasted) + order * depth as usize + plan.bits;
            if bits < best.bits {
                best = SubframePlan {
                    wasted,
                    bits,
                    body: SubframeBody::Fixed {
                        order,
                        residual,
                        plan,
                    },
                    shifted: shifted.clone(),
                };
            }
        }
    }

    // --- Linear predictor -------------------------------------------------
    let max_order = max_lpc_order.min(MAX_LPC_ORDER).min(n.saturating_sub(1));
    if max_order >= 1 {
        let precision = auto_precision(depth, n);
        let windowed = tukey_window(&shifted, 0.5);
        let ac = autocorrelate(&windowed, max_order);
        let fits = lpc_coefficients_all(&ac, max_order);

        for order in lpc_candidate_orders(&fits, n, precision) {
            let Some((coeffs, _)) = fits.get(order - 1) else {
                continue;
            };
            let Some((qlp, shift)) = quantise_lpc_coeffs(coeffs, precision) else {
                continue;
            };
            if shift > MAX_QLP_SHIFT {
                continue;
            }
            let Some(residual) = lpc_residual(&shifted, &qlp, shift) else {
                continue;
            };
            let Ok(plan) = plan_residual(&residual, n, order, max_partition_order) else {
                continue;
            };
            let bits = header_bits(wasted)
                + order * depth as usize
                + 4
                + 5
                + order * precision as usize
                + plan.bits;
            if bits < best.bits {
                best = SubframePlan {
                    wasted,
                    bits,
                    body: SubframeBody::Lpc {
                        qlp,
                        precision,
                        shift,
                        residual,
                        plan,
                    },
                    shifted: shifted.clone(),
                };
            }
        }
    }

    Ok(best)
}

/// Pick the fixed-predictor order with the smallest estimated residual cost.
fn best_fixed_predictor(samples: &[i32], depth: u32) -> Option<(usize, Vec<i32>)> {
    let n = samples.len();
    let mut best: Option<(usize, Vec<i32>, f64)> = None;
    for order in 0..=MAX_FIXED_ORDER.min(n.saturating_sub(1)) {
        let Some(residual) = fixed_residual(samples, order) else {
            continue;
        };
        let count = residual.len();
        if count == 0 {
            continue;
        }
        let sum: f64 = residual.iter().map(|&r| f64::from(r).abs()).sum();
        let mean = (sum / count as f64).max(1.0 / 256.0);
        let est = count as f64 * (mean.log2() + 2.0) + order as f64 * f64::from(depth);
        if best.as_ref().is_none_or(|(_, _, b)| est < *b) {
            best = Some((order, residual, est));
        }
    }
    best.map(|(order, residual, _)| (order, residual))
}

/// Orders worth costing exactly, cheapest estimate first.
///
/// libFLAC's estimator (`0.5·log2(scale·error)`) saturates at zero once the
/// prediction error becomes small, at which point it can no longer separate
/// orders — and picking by coefficient cost alone then lands on an order that
/// is measurably worse.  A Rice code always spends at least one bit per sample
/// (the unary stop bit), so that floor is applied before ranking, and the top
/// few orders are then costed exactly rather than trusted.
fn lpc_candidate_orders(fits: &[(Vec<f64>, f64)], n: usize, precision: u32) -> Vec<usize> {
    let mut ranked: Vec<(usize, f64)> = fits
        .iter()
        .enumerate()
        .filter_map(|(index, (_, err))| {
            let order = index + 1;
            if order >= n {
                return None;
            }
            let count = n - order;
            let bps = expected_bits_per_residual_sample(*err, count).max(1.0);
            Some((
                order,
                bps * count as f64 + (order * precision as usize) as f64,
            ))
        })
        .collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
    ranked.truncate(3);
    ranked.into_iter().map(|(order, _)| order).collect()
}

// =============================================================================
// Serialisation
// =============================================================================

/// Serialise a planned subframe.
///
/// # Errors
///
/// Returns `CodecError::Internal` when the plan is inconsistent with itself.
pub fn write_subframe(
    w: &mut BitWriter,
    bits_per_sample: u32,
    plan: &SubframePlan,
) -> CodecResult<()> {
    let start = w.bit_len();
    let depth = bits_per_sample
        .checked_sub(plan.wasted)
        .ok_or_else(|| CodecError::Internal("FLAC: wasted bits exceed bit depth".to_string()))?;
    let samples = &plan.shifted;

    let type_code = match &plan.body {
        SubframeBody::Constant(_) => TYPE_CONSTANT,
        SubframeBody::Verbatim => TYPE_VERBATIM,
        SubframeBody::Fixed { order, .. } => TYPE_FIXED_BASE + *order as u32,
        SubframeBody::Lpc { qlp, .. } => TYPE_LPC_BASE + qlp.len() as u32 - 1,
    };

    w.write_bits(0, 1);
    w.write_bits(u64::from(type_code), 6);
    if plan.wasted > 0 {
        w.write_bits(1, 1);
        w.write_unary(plan.wasted - 1);
    } else {
        w.write_bits(0, 1);
    }

    match &plan.body {
        SubframeBody::Constant(value) => {
            w.write_bits_signed(i64::from(*value), depth);
        }
        SubframeBody::Verbatim => {
            for &s in samples {
                w.write_bits_signed(i64::from(s), depth);
            }
        }
        SubframeBody::Fixed {
            order,
            residual,
            plan: rplan,
        } => {
            for &s in &samples[..*order] {
                w.write_bits_signed(i64::from(s), depth);
            }
            write_residual(w, residual, samples.len(), *order, rplan)?;
        }
        SubframeBody::Lpc {
            qlp,
            precision,
            shift,
            residual,
            plan: rplan,
        } => {
            let order = qlp.len();
            for &s in &samples[..order] {
                w.write_bits_signed(i64::from(s), depth);
            }
            w.write_bits(u64::from(*precision - 1), 4);
            w.write_bits_signed(i64::from(*shift), 5);
            for &c in qlp {
                w.write_bits_signed(i64::from(c), *precision);
            }
            write_residual(w, residual, samples.len(), order, rplan)?;
        }
    }

    let written = w.bit_len() - start;
    if written != plan.bits {
        return Err(CodecError::Internal(format!(
            "FLAC: subframe plan predicted {} bits but wrote {written}",
            plan.bits
        )));
    }
    Ok(())
}

/// Parse one subframe into `block_size` samples.
///
/// `bits_per_sample` is the subframe bit depth (frame depth + 1 for a side
/// channel).
///
/// # Errors
///
/// Returns `CodecError::InvalidData` for reserved types, malformed headers or
/// truncated input.
pub fn read_subframe(
    r: &mut BitReader<'_>,
    block_size: usize,
    bits_per_sample: u32,
) -> CodecResult<Vec<i32>> {
    let pad = r
        .read_bits(1)
        .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading subframe header".to_string()))?;
    if pad != 0 {
        return Err(CodecError::InvalidData(
            "FLAC: subframe header padding bit must be zero".to_string(),
        ));
    }
    let type_code = r
        .read_bits(6)
        .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading subframe type".to_string()))?
        as u32;
    let has_wasted = r
        .read_bits(1)
        .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading wasted-bits flag".to_string()))?;
    let wasted = if has_wasted == 1 {
        r.read_unary(64)
            .ok_or_else(|| {
                CodecError::InvalidData("FLAC: malformed wasted-bits count".to_string())
            })?
            .checked_add(1)
            .ok_or_else(|| CodecError::InvalidData("FLAC: wasted-bits overflow".to_string()))?
    } else {
        0
    };
    if wasted >= bits_per_sample {
        return Err(CodecError::InvalidData(format!(
            "FLAC: {wasted} wasted bits leaves nothing of a {bits_per_sample}-bit sample"
        )));
    }
    let depth = bits_per_sample - wasted;

    let mut samples = match type_code {
        TYPE_CONSTANT => {
            let value = r.read_bits_signed(depth).ok_or_else(|| {
                CodecError::InvalidData("FLAC: EOF reading constant subframe".to_string())
            })?;
            vec![value as i32; block_size]
        }
        TYPE_VERBATIM => {
            let mut out = Vec::with_capacity(block_size);
            for _ in 0..block_size {
                out.push(r.read_bits_signed(depth).ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading verbatim sample".to_string())
                })? as i32);
            }
            out
        }
        t if (TYPE_FIXED_BASE..=TYPE_FIXED_BASE + MAX_FIXED_ORDER as u32).contains(&t) => {
            let order = (t - TYPE_FIXED_BASE) as usize;
            if block_size < order {
                return Err(CodecError::InvalidData(format!(
                    "FLAC: block size {block_size} is shorter than fixed order {order}"
                )));
            }
            let warmup = read_warmup(r, order, depth)?;
            let residual = read_residual(r, block_size, order)?;
            fixed_restore(&warmup, &residual, order).ok_or_else(|| {
                CodecError::InvalidData(
                    "FLAC: fixed-predictor reconstruction overflowed".to_string(),
                )
            })?
        }
        t if t >= TYPE_LPC_BASE => {
            let order = (t - TYPE_LPC_BASE) as usize + 1;
            if block_size < order {
                return Err(CodecError::InvalidData(format!(
                    "FLAC: block size {block_size} is shorter than LPC order {order}"
                )));
            }
            let warmup = read_warmup(r, order, depth)?;
            let precision = r.read_bits(4).ok_or_else(|| {
                CodecError::InvalidData("FLAC: EOF reading LPC precision".to_string())
            })? as u32
                + 1;
            if precision > MAX_QLP_PRECISION {
                return Err(CodecError::InvalidData(
                    "FLAC: forbidden LPC coefficient precision 0b1111".to_string(),
                ));
            }
            let shift = r.read_bits_signed(5).ok_or_else(|| {
                CodecError::InvalidData("FLAC: EOF reading LPC shift".to_string())
            })?;
            if shift < 0 {
                return Err(CodecError::InvalidData(format!(
                    "FLAC: negative LPC right shift {shift}"
                )));
            }
            let mut qlp = Vec::with_capacity(order);
            for _ in 0..order {
                qlp.push(r.read_bits_signed(precision).ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading LPC coefficient".to_string())
                })? as i32);
            }
            let residual = read_residual(r, block_size, order)?;
            lpc_restore_exact(&warmup, &residual, &qlp, shift as u32).ok_or_else(|| {
                CodecError::InvalidData("FLAC: LPC reconstruction overflowed".to_string())
            })?
        }
        t => {
            return Err(CodecError::InvalidData(format!(
                "FLAC: reserved subframe type {t:#08b}"
            )))
        }
    };

    if samples.len() != block_size {
        return Err(CodecError::InvalidData(format!(
            "FLAC: subframe produced {} of {block_size} samples",
            samples.len()
        )));
    }
    if wasted > 0 {
        for s in &mut samples {
            *s = ((i64::from(*s)) << wasted) as i32;
        }
    }
    Ok(samples)
}

fn read_warmup(r: &mut BitReader<'_>, order: usize, depth: u32) -> CodecResult<Vec<i32>> {
    let mut warmup = Vec::with_capacity(order);
    for _ in 0..order {
        warmup.push(r.read_bits_signed(depth).ok_or_else(|| {
            CodecError::InvalidData("FLAC: EOF reading warm-up sample".to_string())
        })? as i32);
    }
    Ok(warmup)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(samples: &[i32], depth: u32) -> SubframePlan {
        let plan = plan_subframe(samples, depth, 12, 8).expect("plan subframe");
        let mut w = BitWriter::new();
        write_subframe(&mut w, depth, &plan).expect("write subframe");
        assert_eq!(w.bit_len(), plan.bits, "planned bits must match written");
        let bits = w.bit_len();
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let decoded = read_subframe(&mut r, samples.len(), depth).expect("read subframe");
        assert_eq!(decoded, samples, "subframe must round-trip exactly");
        assert_eq!(
            r.bit_pos(),
            bits,
            "reader must consume exactly the subframe"
        );
        plan
    }

    #[test]
    fn constant_subframe_round_trips() {
        let plan = round_trip(&vec![1234i32; 512], 16);
        assert!(matches!(plan.body, SubframeBody::Constant(1234)));
        assert_eq!(plan.wasted, 0, "wasted bits never help a constant subframe");
        assert_eq!(plan.bits, 1 + 6 + 1 + 16);
    }

    #[test]
    fn silence_uses_a_constant_subframe() {
        let plan = round_trip(&vec![0i32; 4096], 16);
        assert!(matches!(plan.body, SubframeBody::Constant(0)));
        assert_eq!(plan.bits, 1 + 6 + 1 + 16);
    }

    #[test]
    fn ramp_round_trips() {
        let samples: Vec<i32> = (0..1024).map(|i| i - 512).collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn sine_round_trips_and_uses_a_predictor() {
        let samples: Vec<i32> = (0..4096)
            .map(|i| ((f64::from(i) * 0.05).sin() * 20000.0) as i32)
            .collect();
        let plan = round_trip(&samples, 16);
        assert!(
            matches!(
                plan.body,
                SubframeBody::Lpc { .. } | SubframeBody::Fixed { .. }
            ),
            "expected a predictive subframe, got {:?}",
            std::mem::discriminant(&plan.body)
        );
        assert!(
            plan.bits < 4096 * 16,
            "predictive coding must beat verbatim ({} bits)",
            plan.bits
        );
    }

    #[test]
    fn white_noise_round_trips() {
        let mut state = 0x2545_F491u32;
        let samples: Vec<i32> = (0..2048)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as i32) >> 17
            })
            .collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn wasted_bits_are_detected_and_restored() {
        let samples: Vec<i32> = (0..512).map(|i| ((i % 97) - 48) * 256).collect();
        let plan = round_trip(&samples, 24);
        assert_eq!(plan.wasted, 8, "8 low zero bits should be recognised");
    }

    #[test]
    fn full_scale_16_bit_extremes_round_trip() {
        let samples: Vec<i32> = (0..256)
            .map(|i| {
                if i % 2 == 0 {
                    i16::MIN.into()
                } else {
                    i16::MAX.into()
                }
            })
            .collect();
        round_trip(&samples, 16);
    }

    #[test]
    fn full_scale_24_bit_extremes_round_trip() {
        let lo = -(1i32 << 23);
        let hi = (1i32 << 23) - 1;
        let samples: Vec<i32> = (0..256).map(|i| if i % 3 == 0 { lo } else { hi }).collect();
        round_trip(&samples, 24);
    }

    #[test]
    fn side_channel_depth_round_trips() {
        // A 17-bit side channel from 16-bit sources.
        let samples: Vec<i32> = (0..1024).map(|i| ((i % 131) - 65) * 500).collect();
        round_trip(&samples, 17);
    }

    #[test]
    fn eight_bit_depth_round_trips() {
        let samples: Vec<i32> = (0..192).map(|i| ((i % 200) as i32) - 100).collect();
        round_trip(&samples, 8);
    }

    #[test]
    fn tiny_blocks_round_trip() {
        for n in [1usize, 2, 3, 5, 16, 17] {
            let samples: Vec<i32> = (0..n).map(|i| (i as i32 * 37) % 251 - 125).collect();
            round_trip(&samples, 16);
        }
    }

    #[test]
    fn plan_rejects_samples_wider_than_the_declared_depth() {
        let samples = vec![70000i32, 0, 0, 0];
        assert!(plan_subframe(&samples, 16, 8, 8).is_err());
    }

    #[test]
    fn plan_rejects_empty_blocks() {
        assert!(plan_subframe(&[], 16, 8, 8).is_err());
    }

    #[test]
    fn read_rejects_reserved_subframe_types() {
        for t in [0b00_0010u32, 0b00_0111, 0b00_1101, 0b01_1111] {
            let mut w = BitWriter::new();
            w.write_bits(0, 1);
            w.write_bits(u64::from(t), 6);
            w.write_bits(0, 1);
            w.write_bits(0, 64);
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            assert!(
                read_subframe(&mut r, 4, 16).is_err(),
                "type {t:#08b} must be rejected"
            );
        }
    }

    #[test]
    fn read_rejects_nonzero_padding_bit() {
        let bytes = [0xFFu8; 8];
        let mut r = BitReader::new(&bytes);
        assert!(read_subframe(&mut r, 4, 16).is_err());
    }

    #[test]
    fn lpc_predictor_is_bit_exact_end_to_end() {
        // A signal an LPC fit handles well: the encoder's integer residual and
        // the decoder's integer synthesis must agree exactly.
        let samples: Vec<i32> = (0..2048)
            .map(|i| {
                let t = f64::from(i) / 44100.0;
                ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0
                    + (t * 1310.0 * std::f64::consts::TAU).sin() * 3000.0) as i32
            })
            .collect();
        let plan = round_trip(&samples, 16);
        match plan.body {
            SubframeBody::Lpc {
                ref qlp,
                precision,
                shift,
                ..
            } => {
                assert!(!qlp.is_empty());
                assert!((2..=MAX_QLP_PRECISION).contains(&precision));
                assert!(shift <= MAX_QLP_SHIFT);
            }
            ref other => panic!("expected an LPC subframe, got {other:?}"),
        }
    }
}

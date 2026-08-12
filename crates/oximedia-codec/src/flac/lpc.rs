//! Linear Predictive Coding (LPC) for FLAC.
//!
//! FLAC uses LPC analysis to predict each audio sample from its `p` predecessors.
//! The prediction residuals are then Rice-coded.
//!
//! # Analysis helpers (floating point)
//!
//! - `autocorrelate` — compute autocorrelation lags.
//! - `levinson_durbin` / [`lpc_coefficients_all`] — fit LPC coefficients.
//! - `tukey_window` — taper a block before autocorrelation.
//! - `predict` / `compute_residuals` / `restore_signal` — a floating-point
//!   predictor pair. **These are analysis conveniences, not the FLAC
//!   predictor**: rounding a floating-point prediction is not reproducible
//!   across implementations, so a stream built with them would not be
//!   losslessly decodable. Use the integer functions below for anything that
//!   touches a bitstream.
//!
//! # Bitstream predictors (integer, RFC 9639 §9.2.5 / §9.2.6)
//!
//! - [`quantise_lpc_coeffs`] — coefficients + shift exactly as coded.
//! - [`lpc_residual`] / [`lpc_restore_exact`] — the LPC pair the encoder and
//!   decoder both use; identical `i64` accumulation and arithmetic right
//!   shift on both sides, so they are exact inverses by construction.
//! - [`fixed_residual`] / [`fixed_restore`] — the fixed polynomial predictors.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

/// Maximum supported LPC order.
pub const MAX_LPC_ORDER: usize = 32;

/// Compute the autocorrelation of `signal` at lags 0..=`order`.
///
/// Returns a vector of `order + 1` autocorrelation values.
#[must_use]
pub fn autocorrelate(signal: &[f64], order: usize) -> Vec<f64> {
    let n = signal.len();
    let mut ac = vec![0.0f64; order + 1];
    for lag in 0..=order {
        for i in lag..n {
            ac[lag] += signal[i] * signal[i - lag];
        }
    }
    ac
}

/// Fit LPC coefficients of order `p` using the Levinson-Durbin recursion.
///
/// Returns `(coeffs, error)` where `coeffs` has length `p` and `error` is the
/// residual prediction power.  Returns an empty vector if `ac[0]` is zero.
pub fn levinson_durbin(ac: &[f64], order: usize) -> (Vec<f64>, f64) {
    let p = order.min(ac.len().saturating_sub(1));
    if p == 0 || ac[0] == 0.0 {
        return (Vec::new(), 0.0);
    }

    let mut a = vec![0.0f64; p + 1];
    let mut err = ac[0];
    let mut km;

    for m in 1..=p {
        // Reflection coefficient
        let mut lambda = 0.0f64;
        for j in 1..m {
            lambda += a[j] * ac[m - j];
        }
        lambda = (ac[m] - lambda) / err;

        km = lambda;
        a[m] = km;

        // Update coefficients
        let half = m / 2;
        for j in 1..=half {
            let aj = a[j];
            let amj = a[m - j];
            a[j] = aj + km * amj;
            a[m - j] = amj + km * aj;
        }
        if m % 2 == 1 {
            a[(m + 1) / 2] *= 1.0 + km;
        }

        err *= 1.0 - km * km;
        if err <= 0.0 {
            err = 0.0;
            break;
        }
    }

    let coeffs = a[1..=p].to_vec();
    (coeffs, err)
}

/// Apply LPC predictor to `signal` using `coeffs`.
///
/// Returns predicted values starting at index `p = coeffs.len()`.
#[must_use]
pub fn predict(signal: &[i32], coeffs: &[f64]) -> Vec<i32> {
    let p = coeffs.len();
    let n = signal.len();
    if p == 0 || n <= p {
        return vec![0i32; n.saturating_sub(p)];
    }

    (p..n)
        .map(|i| {
            let pred: f64 = coeffs
                .iter()
                .enumerate()
                .map(|(j, &c)| c * f64::from(signal[i - 1 - j]))
                .sum();
            pred.round() as i32
        })
        .collect()
}

/// Compute LPC residuals: `residual[i] = signal[p+i] − prediction[i]`.
#[must_use]
pub fn compute_residuals(signal: &[i32], coeffs: &[f64]) -> Vec<i32> {
    let p = coeffs.len();
    let preds = predict(signal, coeffs);
    preds
        .iter()
        .enumerate()
        .map(|(i, &pred)| signal[p + i].wrapping_sub(pred))
        .collect()
}

/// Restore signal from residuals and LPC warmup samples.
///
/// `warmup` must be the first `p = coeffs.len()` original samples.
///
/// This is the inverse of [`compute_residuals`] and shares its floating-point
/// rounding, so it is **not** the FLAC bitstream predictor — use
/// [`lpc_restore_exact`] for anything decoded from or encoded into a stream.
#[must_use]
pub fn restore_signal(warmup: &[i32], residuals: &[i32], coeffs: &[f64]) -> Vec<i32> {
    let p = coeffs.len();
    let mut out: Vec<i32> = warmup.to_vec();

    for (i, &r) in residuals.iter().enumerate() {
        let base = out.len().saturating_sub(1);
        let pred: f64 = coeffs
            .iter()
            .enumerate()
            .map(|(j, &c)| {
                let idx = base - j;
                c * f64::from(out[idx])
            })
            .sum();
        let _ = i;
        out.push((pred.round() as i32).wrapping_add(r));
    }

    out
}

/// Quantise floating-point LPC coefficients to fixed-point integer coefficients.
///
/// Returns `(int_coeffs, shift)` where `int_coeffs[i] = round(coeffs[i] * 2^shift)`.
/// `shift` is chosen to maximise precision while fitting into `bits`-bit signed integers.
///
/// This helper does not clamp the result to the declared precision and can
/// saturate `shift` to 0; [`quantise_lpc_coeffs`] is the bitstream-safe
/// version and is what the encoder uses.
#[must_use]
pub fn quantise_coeffs(coeffs: &[f64], bits: u8) -> (Vec<i32>, u8) {
    if coeffs.is_empty() {
        return (Vec::new(), 0);
    }
    let max_abs = coeffs.iter().cloned().fold(0.0f64, |a, v| a.max(v.abs()));
    if max_abs < 1e-10 {
        return (vec![0i32; coeffs.len()], 0);
    }

    let max_val = (1i64 << (bits - 1)) - 1;
    let scale = max_val as f64 / max_abs;
    let shift = scale.log2().floor() as u8;
    let actual_scale = (1i64 << shift) as f64;

    let int_coeffs: Vec<i32> = coeffs
        .iter()
        .map(|&c| (c * actual_scale).round() as i32)
        .collect();

    (int_coeffs, shift)
}

// =============================================================================
// Spec-exact integer predictors (RFC 9639 §9.2.5 / §9.2.6)
// =============================================================================

/// Highest right shift a FLAC LPC subframe may declare (5-bit signed field,
/// negative values forbidden — but libFLAC and this encoder cap at 15).
pub const MAX_QLP_SHIFT: u32 = 15;

/// Highest coefficient precision a FLAC LPC subframe may declare.
///
/// The header field stores `precision - 1` in 4 bits and `0b1111` is
/// forbidden, so the precision ceiling is 15 bits.
pub const MAX_QLP_PRECISION: u32 = 15;

/// Largest fixed-predictor order defined by the format.
pub const MAX_FIXED_ORDER: usize = 4;

/// Run the Levinson–Durbin recursion once, returning every intermediate order.
///
/// Entry `m - 1` of the result holds `(coefficients, prediction_error)` for
/// order `m`, where the coefficients satisfy `x[n] ≈ Σ a[j]·x[n-1-j]`.
/// The vector is truncated at the order where the recursion becomes
/// degenerate (silence or a numerically exhausted fit).
#[must_use]
pub fn lpc_coefficients_all(ac: &[f64], max_order: usize) -> Vec<(Vec<f64>, f64)> {
    let mut out = Vec::new();
    if max_order == 0 || ac.len() <= max_order || !ac[0].is_finite() || ac[0] <= 0.0 {
        return out;
    }
    let mut a = vec![0.0f64; max_order + 1];
    let mut err = ac[0];

    for m in 1..=max_order {
        let mut acc = ac[m];
        for j in 1..m {
            acc -= a[j] * ac[m - j];
        }
        let k = acc / err;
        if !k.is_finite() {
            break;
        }
        let prev: Vec<f64> = a[1..m].to_vec();
        a[m] = k;
        for j in 1..m {
            a[j] = prev[j - 1] - k * prev[m - j - 1];
        }
        err *= 1.0 - k * k;
        let coeffs = a[1..=m].to_vec();
        if coeffs.iter().any(|c| !c.is_finite()) {
            break;
        }
        out.push((coeffs, err));
        if !err.is_finite() || err <= 0.0 {
            break;
        }
    }
    out
}

/// Solve the Levinson–Durbin recursion for prediction coefficients of `order`.
///
/// Returns `a[0..order]` such that `x[n] ≈ Σ a[j]·x[n-1-j]`, or `None` when the
/// autocorrelation is degenerate (silence, or a numerically exhausted fit).
#[must_use]
pub fn lpc_coefficients(ac: &[f64], order: usize) -> Option<Vec<f64>> {
    let all = lpc_coefficients_all(ac, order);
    all.into_iter()
        .nth(order.checked_sub(1)?)
        .map(|(coeffs, _)| coeffs)
}

/// Estimated bits per residual sample for a Levinson–Durbin prediction error.
///
/// Mirrors libFLAC's estimator: the residual is modelled as Laplacian with
/// variance `lpc_error / n`, giving `0.5·log2(e·ln2²·error/n)` bits.
#[must_use]
pub fn expected_bits_per_residual_sample(lpc_error: f64, total_samples: usize) -> f64 {
    if total_samples == 0 {
        return 0.0;
    }
    if lpc_error > 0.0 {
        let error_scale =
            0.5 * std::f64::consts::LN_2 * std::f64::consts::LN_2 / total_samples as f64;
        let bps = 0.5 * (error_scale * lpc_error).log2();
        if bps.is_finite() && bps > 0.0 {
            bps
        } else {
            0.0
        }
    } else if lpc_error < 0.0 {
        // Should not happen; treat as unusable rather than trusting it.
        f64::INFINITY
    } else {
        0.0
    }
}

/// Quantise LPC coefficients to `precision`-bit signed integers with a common
/// right shift, using running error feedback (as libFLAC does).
///
/// Returns `(coefficients, shift)`, or `None` when the coefficients are all
/// zero or cannot be represented with a non-negative shift.
#[must_use]
pub fn quantise_lpc_coeffs(coeffs: &[f64], precision: u32) -> Option<(Vec<i32>, u32)> {
    if coeffs.is_empty() || !(2..=MAX_QLP_PRECISION).contains(&precision) {
        return None;
    }
    let cmax = coeffs
        .iter()
        .fold(0.0f64, |m, &c| if c.abs() > m { c.abs() } else { m });
    if !cmax.is_finite() || cmax <= 0.0 {
        return None;
    }

    // floor(log2(cmax)); the extra -1 keeps one bit of headroom for the sign,
    // matching libFLAC's frexp-based derivation.
    let log2cmax = cmax.log2().floor() as i32;
    let mut shift = precision as i32 - 1 - log2cmax - 1;
    if shift > MAX_QLP_SHIFT as i32 {
        shift = MAX_QLP_SHIFT as i32;
    }
    if shift < 0 {
        return None;
    }

    let qmax = (1i64 << (precision - 1)) - 1;
    let qmin = -(1i64 << (precision - 1));
    let scale = (1u64 << shift) as f64;
    let mut error = 0.0f64;
    let mut out = Vec::with_capacity(coeffs.len());
    for &c in coeffs {
        error += c * scale;
        if !error.is_finite() {
            return None;
        }
        let q = (error.round() as i64).clamp(qmin, qmax);
        error -= q as f64;
        out.push(q as i32);
    }
    if out.iter().all(|&q| q == 0) {
        return None;
    }
    Some((out, shift as u32))
}

/// The LPC prediction for sample `history.len()` given the preceding samples.
///
/// `history` must end with the most recent sample; `qlp[0]` multiplies it, as
/// FLAC stores coefficients in reverse sample order.
#[must_use]
pub fn lpc_predict(history: &[i32], qlp: &[i32], shift: u32) -> i64 {
    let mut sum = 0i64;
    for (j, &c) in qlp.iter().enumerate() {
        if let Some(&s) = history.get(history.len().wrapping_sub(1 + j)) {
            sum += i64::from(c) * i64::from(s);
        }
    }
    sum >> shift
}

/// Compute LPC residuals with the exact integer arithmetic the decoder uses.
///
/// Returns `None` when any residual falls outside the `i32` range that FLAC
/// requires (RFC 9639 §9.2.7), so the caller can fall back to another
/// subframe type instead of emitting an unrepresentable frame.
#[must_use]
pub fn lpc_residual(samples: &[i32], qlp: &[i32], shift: u32) -> Option<Vec<i32>> {
    let order = qlp.len();
    if order == 0 || samples.len() <= order {
        return None;
    }
    let mut out = Vec::with_capacity(samples.len() - order);
    for i in order..samples.len() {
        let mut sum = 0i64;
        for (j, &c) in qlp.iter().enumerate() {
            sum += i64::from(c) * i64::from(samples[i - 1 - j]);
        }
        let residual = i64::from(samples[i]) - (sum >> shift);
        out.push(i32::try_from(residual).ok()?);
    }
    Some(out)
}

/// Reconstruct an LPC subframe exactly, mirroring [`lpc_residual`].
///
/// Returns `None` when a reconstructed sample overflows `i32`.
#[must_use]
pub fn lpc_restore_exact(
    warmup: &[i32],
    residual: &[i32],
    qlp: &[i32],
    shift: u32,
) -> Option<Vec<i32>> {
    let order = qlp.len();
    if warmup.len() != order {
        return None;
    }
    let mut out = Vec::with_capacity(order + residual.len());
    out.extend_from_slice(warmup);
    for &r in residual {
        let mut sum = 0i64;
        let base = out.len();
        for (j, &c) in qlp.iter().enumerate() {
            sum += i64::from(c) * i64::from(out[base - 1 - j]);
        }
        let value = (sum >> shift) + i64::from(r);
        out.push(i32::try_from(value).ok()?);
    }
    Some(out)
}

/// Fixed-predictor residuals for `order` (0..=4), per RFC 9639 §9.2.5.
///
/// Returns `None` for an unsupported order, too few samples, or a residual
/// that does not fit in `i32`.
#[must_use]
pub fn fixed_residual(samples: &[i32], order: usize) -> Option<Vec<i32>> {
    if order > MAX_FIXED_ORDER || samples.len() < order {
        return None;
    }
    let mut out = Vec::with_capacity(samples.len() - order);
    for i in order..samples.len() {
        let s = |k: usize| i64::from(samples[i - k]);
        let pred = match order {
            0 => 0,
            1 => s(1),
            2 => 2 * s(1) - s(2),
            3 => 3 * s(1) - 3 * s(2) + s(3),
            _ => 4 * s(1) - 6 * s(2) + 4 * s(3) - s(4),
        };
        out.push(i32::try_from(i64::from(samples[i]) - pred).ok()?);
    }
    Some(out)
}

/// Reconstruct a fixed-predictor subframe, mirroring [`fixed_residual`].
///
/// Returns `None` for an unsupported order, a warm-up/order mismatch, or an
/// overflowing sample.
#[must_use]
pub fn fixed_restore(warmup: &[i32], residual: &[i32], order: usize) -> Option<Vec<i32>> {
    if order > MAX_FIXED_ORDER || warmup.len() != order {
        return None;
    }
    let mut out: Vec<i32> = Vec::with_capacity(order + residual.len());
    out.extend_from_slice(warmup);
    for &r in residual {
        let n = out.len();
        let s = |k: usize| i64::from(out[n - k]);
        let pred = match order {
            0 => 0,
            1 => s(1),
            2 => 2 * s(1) - s(2),
            3 => 3 * s(1) - 3 * s(2) + s(3),
            _ => 4 * s(1) - 6 * s(2) + 4 * s(3) - s(4),
        };
        out.push(i32::try_from(pred + i64::from(r)).ok()?);
    }
    Some(out)
}

/// Apply a Tukey window (taper ratio `alpha`) to `signal`.
///
/// Windowing before autocorrelation markedly improves the LPC fit; the
/// residuals themselves are always computed on the unwindowed signal.
#[must_use]
pub fn tukey_window(signal: &[i32], alpha: f64) -> Vec<f64> {
    let n = signal.len();
    if n == 0 {
        return Vec::new();
    }
    let alpha = alpha.clamp(0.0, 1.0);
    let taper = (alpha * (n - 1) as f64 / 2.0).floor() as usize;
    (0..n)
        .map(|i| {
            let w = if taper == 0 {
                1.0
            } else if i < taper {
                0.5 * (1.0 - (std::f64::consts::PI * i as f64 / taper as f64).cos())
            } else if i >= n - taper {
                let k = n - 1 - i;
                0.5 * (1.0 - (std::f64::consts::PI * k as f64 / taper as f64).cos())
            } else {
                1.0
            };
            f64::from(signal[i]) * w
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autocorrelate_dc_signal() {
        let dc = vec![1.0f64; 100];
        let ac = autocorrelate(&dc, 4);
        // For a DC signal, all lags have the same autocorrelation as lag 0.
        assert_eq!(ac.len(), 5);
        assert!(ac[0] > 0.0);
        assert!(ac[1] > 0.0);
    }

    #[test]
    fn test_autocorrelate_zero_signal() {
        let zero = vec![0.0f64; 50];
        let ac = autocorrelate(&zero, 3);
        for v in &ac {
            assert_eq!(*v, 0.0);
        }
    }

    #[test]
    fn test_levinson_durbin_order1() {
        // Sine wave → LPC should fit a 2-pole predictor
        let n = 64;
        let signal: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * i as f64 / 16.0).sin())
            .collect();
        let ac = autocorrelate(&signal, 2);
        let (coeffs, err) = levinson_durbin(&ac, 2);
        assert_eq!(coeffs.len(), 2);
        assert!(err >= 0.0, "Residual error should be non-negative");
    }

    #[test]
    fn test_levinson_durbin_empty_ac() {
        let (coeffs, _) = levinson_durbin(&[], 2);
        assert!(coeffs.is_empty());
    }

    #[test]
    fn test_predict_simple() {
        let signal: Vec<i32> = vec![1, 2, 3, 4, 5];
        let coeffs = vec![1.0f64]; // order-1 predictor: predict[i] = signal[i-1]
        let preds = predict(&signal, &coeffs);
        assert_eq!(preds, vec![1, 2, 3, 4]); // pred[0]=signal[0]=1, ...
    }

    #[test]
    fn test_compute_residuals_lossless() {
        let signal: Vec<i32> = (0..32).map(|i| i * 3).collect();
        let coeffs = vec![1.0f64]; // naive predictor
        let residuals = compute_residuals(&signal, &coeffs);
        let warmup = &signal[..1];
        let restored = restore_signal(warmup, &residuals, &coeffs);
        assert_eq!(&restored, &signal, "Restore signal should be lossless");
    }

    #[test]
    fn test_restore_signal_dc() {
        // DC signal: each sample == 1000
        let signal: Vec<i32> = vec![1000i32; 16];
        let coeffs = vec![1.0f64];
        let residuals = compute_residuals(&signal, &coeffs);
        // Residuals should all be 0 (perfect DC prediction)
        let warmup = &signal[..1];
        let restored = restore_signal(warmup, &residuals, &coeffs);
        assert_eq!(&restored, &signal);
    }

    #[test]
    fn test_quantise_coeffs_basic() {
        let coeffs = vec![0.5f64, -0.25];
        let (int_c, shift) = quantise_coeffs(&coeffs, 12);
        assert_eq!(int_c.len(), 2);
        assert!(
            shift > 0,
            "Shift should be positive for non-trivial coefficients"
        );
    }

    #[test]
    fn test_quantise_coeffs_zero() {
        let coeffs = vec![0.0f64; 4];
        let (int_c, shift) = quantise_coeffs(&coeffs, 12);
        assert!(int_c.iter().all(|&v| v == 0));
        assert_eq!(shift, 0);
    }

    // -------------------------------------------------------------------
    // Spec-exact integer predictors
    // -------------------------------------------------------------------

    fn test_signal(n: usize) -> Vec<i32> {
        (0..n)
            .map(|i| {
                let t = f64::from(i as i32) / 44100.0;
                ((t * 440.0 * std::f64::consts::TAU).sin() * 15000.0
                    + (t * 2100.0 * std::f64::consts::TAU).sin() * 2500.0) as i32
            })
            .collect()
    }

    #[test]
    fn fixed_residual_matches_the_rfc_formulas() {
        let s = vec![10i32, 13, 19, 30, 48, 75];
        assert_eq!(fixed_residual(&s, 0).expect("order 0"), &s[..]);
        assert_eq!(
            fixed_residual(&s, 1).expect("order 1"),
            vec![3, 6, 11, 18, 27]
        );
        assert_eq!(fixed_residual(&s, 2).expect("order 2"), vec![3, 5, 7, 9]);
        assert_eq!(fixed_residual(&s, 3).expect("order 3"), vec![2, 2, 2]);
        assert_eq!(fixed_residual(&s, 4).expect("order 4"), vec![0, 0]);
        assert!(fixed_residual(&s, 5).is_none(), "order 5 is not defined");
    }

    #[test]
    fn fixed_residual_and_restore_are_exact_inverses() {
        let signal = test_signal(512);
        for order in 0..=MAX_FIXED_ORDER {
            let residual = fixed_residual(&signal, order).expect("residual");
            let restored = fixed_restore(&signal[..order], &residual, order).expect("restore");
            assert_eq!(restored, signal, "fixed order {order} must be lossless");
        }
    }

    #[test]
    fn lpc_residual_and_restore_are_exact_inverses() {
        let signal = test_signal(1024);
        let windowed = tukey_window(&signal, 0.5);
        let ac = autocorrelate(&windowed, 12);
        let fits = lpc_coefficients_all(&ac, 12);
        assert!(!fits.is_empty(), "LPC must fit a tonal signal");

        for (index, (coeffs, _)) in fits.iter().enumerate() {
            let order = index + 1;
            let Some((qlp, shift)) = quantise_lpc_coeffs(coeffs, 12) else {
                continue;
            };
            assert!(shift <= MAX_QLP_SHIFT, "shift {shift} out of range");
            assert!(
                qlp.iter().all(|&c| (-(1 << 11)..(1 << 11)).contains(&c)),
                "order {order}: coefficients must fit the declared precision"
            );
            let residual = lpc_residual(&signal, &qlp, shift).expect("residual");
            assert_eq!(residual.len(), signal.len() - order);
            let restored =
                lpc_restore_exact(&signal[..order], &residual, &qlp, shift).expect("restore");
            assert_eq!(restored, signal, "LPC order {order} must be lossless");
        }
    }

    #[test]
    fn lpc_predict_uses_reverse_sample_order() {
        // qlp[0] multiplies the most recent sample (RFC 9639 §9.2.6).
        let history = [1i32, 2, 4];
        let qlp = [8i32, 0, 0];
        assert_eq!(lpc_predict(&history, &qlp, 0), 32);
        let qlp = [0i32, 8, 0];
        assert_eq!(lpc_predict(&history, &qlp, 0), 16);
        let qlp = [0i32, 0, 8];
        assert_eq!(lpc_predict(&history, &qlp, 0), 8);
        // The shift is an arithmetic right shift of the accumulated sum.
        assert_eq!(lpc_predict(&[-3i32], &[1i32], 1), -2);
    }

    #[test]
    fn quantise_lpc_coeffs_respects_precision_and_shift_limits() {
        for precision in 2..=MAX_QLP_PRECISION {
            let coeffs = vec![1.87f64, -0.94, 0.31, -0.05];
            let Some((qlp, shift)) = quantise_lpc_coeffs(&coeffs, precision) else {
                continue;
            };
            let lo = -(1i32 << (precision - 1));
            let hi = (1i32 << (precision - 1)) - 1;
            assert!(
                qlp.iter().all(|&c| (lo..=hi).contains(&c)),
                "precision {precision}: {qlp:?} out of [{lo}, {hi}]"
            );
            assert!(
                shift <= MAX_QLP_SHIFT,
                "precision {precision}: shift {shift}"
            );
        }
    }

    #[test]
    fn quantise_lpc_coeffs_rejects_degenerate_input() {
        assert!(quantise_lpc_coeffs(&[], 12).is_none());
        assert!(quantise_lpc_coeffs(&[0.0, 0.0], 12).is_none());
        assert!(quantise_lpc_coeffs(&[f64::NAN], 12).is_none());
        assert!(quantise_lpc_coeffs(&[f64::INFINITY], 12).is_none());
        // A coefficient far above 2^15 cannot be scaled with a non-negative shift.
        assert!(quantise_lpc_coeffs(&[1.0e9], 12).is_none());
    }

    #[test]
    fn lpc_coefficients_all_is_degenerate_on_silence() {
        let ac = vec![0.0f64; 9];
        assert!(lpc_coefficients_all(&ac, 8).is_empty());
        assert!(lpc_coefficients(&ac, 4).is_none());
    }

    #[test]
    fn lpc_coefficients_agree_with_lpc_coefficients_all() {
        let ac = autocorrelate(&tukey_window(&test_signal(512), 0.5), 8);
        let all = lpc_coefficients_all(&ac, 8);
        for (index, (coeffs, _)) in all.iter().enumerate() {
            let order = index + 1;
            let single = lpc_coefficients(&ac, order).expect("single-order fit");
            assert_eq!(&single, coeffs, "order {order}");
        }
    }

    #[test]
    fn lpc_residual_reports_overflow_instead_of_wrapping() {
        // Huge coefficients with a zero shift push the prediction far outside
        // i32; the caller must be told rather than handed a wrapped value.
        let signal = vec![i32::MAX / 2; 64];
        let qlp = vec![32767i32; 4];
        assert!(lpc_residual(&signal, &qlp, 0).is_none());
    }

    #[test]
    fn tukey_window_tapers_the_edges_and_preserves_the_centre() {
        let signal = vec![1000i32; 64];
        let windowed = tukey_window(&signal, 0.5);
        assert_eq!(windowed.len(), 64);
        assert!(windowed[0].abs() < 1.0, "first sample must be tapered");
        assert!(
            (windowed[32] - 1000.0).abs() < 1e-9,
            "centre must be untouched, got {}",
            windowed[32]
        );
        // alpha = 0 is a rectangular window.
        let rect = tukey_window(&signal, 0.0);
        assert!(rect.iter().all(|&v| (v - 1000.0).abs() < 1e-9));
        assert!(tukey_window(&[], 0.5).is_empty());
    }

    #[test]
    fn expected_bits_estimator_is_monotonic_in_error() {
        let a = expected_bits_per_residual_sample(1.0e9, 4096);
        let b = expected_bits_per_residual_sample(1.0e6, 4096);
        assert!(a > b, "more error must mean more bits ({a} vs {b})");
        assert_eq!(expected_bits_per_residual_sample(0.0, 4096), 0.0);
        assert_eq!(expected_bits_per_residual_sample(1.0, 0), 0.0);
        assert!(expected_bits_per_residual_sample(-1.0, 4096).is_infinite());
    }
}

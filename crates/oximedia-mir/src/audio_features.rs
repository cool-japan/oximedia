//! Audio feature extraction for Music Information Retrieval.
//!
//! Provides MFCC coefficient accumulation, log-mel spectrogram computation,
//! and chroma vector analysis.
//!
//! The log-mel spectrogram is computed by
//! [`oximedia_audio::spectrum::compute_log_mel_spectrogram`], which runs a real
//! windowed FFT (via `oxifft`) through a triangular mel filterbank; this module
//! only reshapes the result. An earlier revision of
//! [`compute_log_mel_spectrogram`] returned a tensor in which every mel bin of a
//! frame held the same frame RMS — that placeholder shadowed the real
//! implementation and has been removed.

#![allow(dead_code)]

// ── MfccCoeffs ────────────────────────────────────────────────────────────────

/// Accumulates MFCC (Mel-Frequency Cepstral Coefficient) frames and provides
/// basic statistics across those frames.
#[derive(Debug, Clone)]
pub struct MfccCoeffs {
    /// Stored coefficient frames (each frame is a `Vec<f32>` of length `num_mfcc`).
    pub coefficients: Vec<Vec<f32>>,
    /// Number of MFCC coefficients per frame.
    pub num_mfcc: usize,
}

impl MfccCoeffs {
    /// Create a new, empty `MfccCoeffs` accumulator.
    ///
    /// # Arguments
    /// * `num_mfcc` – number of MFCC coefficients expected per frame.
    #[must_use]
    pub fn new(num_mfcc: usize) -> Self {
        Self {
            coefficients: Vec::new(),
            num_mfcc,
        }
    }

    /// Add a single MFCC frame.  Silently ignores frames whose length ≠ `num_mfcc`.
    pub fn add_frame(&mut self, coeffs: &[f32]) {
        if coeffs.len() == self.num_mfcc {
            self.coefficients.push(coeffs.to_vec());
        }
    }

    /// Compute the per-coefficient mean across all stored frames.
    ///
    /// Returns a zero vector of length `num_mfcc` if no frames have been added.
    #[must_use]
    pub fn mean(&self) -> Vec<f32> {
        if self.coefficients.is_empty() {
            return vec![0.0; self.num_mfcc];
        }
        let n = self.coefficients.len() as f32;
        let mut result = vec![0.0f32; self.num_mfcc];
        for frame in &self.coefficients {
            for (i, &v) in frame.iter().enumerate() {
                result[i] += v;
            }
        }
        for x in &mut result {
            *x /= n;
        }
        result
    }

    /// Compute the per-coefficient variance across all stored frames.
    ///
    /// Returns a zero vector of length `num_mfcc` if fewer than 2 frames exist.
    #[must_use]
    pub fn variance(&self) -> Vec<f32> {
        if self.coefficients.len() < 2 {
            return vec![0.0; self.num_mfcc];
        }
        let mean = self.mean();
        let n = self.coefficients.len() as f32;
        let mut var = vec![0.0f32; self.num_mfcc];
        for frame in &self.coefficients {
            for (i, &v) in frame.iter().enumerate() {
                let diff = v - mean[i];
                var[i] += diff * diff;
            }
        }
        for x in &mut var {
            *x /= n;
        }
        var
    }

    /// Compute the delta (first-order difference) for coefficient at `idx`.
    ///
    /// Uses the simple backward difference: `frame[last][idx] - frame[0][idx]`.
    /// Returns `0.0` if fewer than 2 frames are stored or `idx` is out of range.
    #[must_use]
    pub fn delta(&self, idx: usize) -> f32 {
        if self.coefficients.len() < 2 || idx >= self.num_mfcc {
            return 0.0;
        }
        let last = self.coefficients.len() - 1;
        self.coefficients[last][idx] - self.coefficients[0][idx]
    }
}

// ── compute_log_mel_spectrogram ────────────────────────────────────────────────

/// Default FFT window size relative to the hop length.
///
/// A window of four hops gives the usual 75 % overlap used for music analysis.
const DEFAULT_WINDOW_HOPS: usize = 4;

/// Compute a log-mel spectrogram from a mono audio signal.
///
/// Delegates to [`oximedia_audio::spectrum::compute_log_mel_spectrogram`] with
/// an FFT window of `4 × hop_length` (75 % overlap) and reshapes the row-major
/// result into per-frame vectors. Use
/// [`compute_log_mel_spectrogram_with_fft`] to choose the window size
/// explicitly.
///
/// # Conventions
///
/// * Mel scale: **HTK** — `mel = 2595 · log₁₀(1 + f / 700)`.
/// * Filterbank: `n_mels` triangular filters spanning 0 Hz … Nyquist, applied
///   to the power spectrum `|X[k]|²` of a Hann-windowed frame; **not**
///   Slaney-normalised (filters peak at 1.0, so wide high-frequency bands
///   integrate more energy than narrow low-frequency ones).
/// * Frames are centre-padded with `n_fft / 2` zeros at both ends.
/// * Compression: `ln(energy + 1e-10)` (natural log).
///
/// # Arguments
/// * `samples`     – mono audio samples (f32, nominally in `[-1.0, 1.0]`).
/// * `sample_rate` – sample rate in Hz; calibrates the filterbank frequencies.
/// * `n_mels`      – number of mel filterbank channels.
/// * `hop_length`  – hop length between frames in samples.
///
/// # Returns
///
/// A `Vec<Vec<f32>>` with shape `[n_frames][n_mels]` of log-energy values, or
/// an empty vector if any argument is degenerate.
#[must_use]
pub fn compute_log_mel_spectrogram(
    samples: &[f32],
    sample_rate: u32,
    n_mels: usize,
    hop_length: usize,
) -> Vec<Vec<f32>> {
    compute_log_mel_spectrogram_with_fft(
        samples,
        sample_rate,
        n_mels,
        hop_length.saturating_mul(DEFAULT_WINDOW_HOPS),
        hop_length,
    )
}

/// Compute a log-mel spectrogram with an explicit FFT window size.
///
/// See [`compute_log_mel_spectrogram`] for the mel/filterbank conventions.
///
/// # Arguments
/// * `samples`     – mono audio samples (f32).
/// * `sample_rate` – sample rate in Hz.
/// * `n_mels`      – number of mel filterbank channels.
/// * `n_fft`       – FFT window size in samples.
/// * `hop_length`  – hop length between frames in samples.
///
/// # Returns
///
/// A `Vec<Vec<f32>>` with shape `[n_frames][n_mels]`.
#[must_use]
pub fn compute_log_mel_spectrogram_with_fft(
    samples: &[f32],
    sample_rate: u32,
    n_mels: usize,
    n_fft: usize,
    hop_length: usize,
) -> Vec<Vec<f32>> {
    if samples.is_empty() || n_mels == 0 || hop_length == 0 || n_fft == 0 {
        return Vec::new();
    }

    let flat = oximedia_audio::spectrum::compute_log_mel_spectrogram(
        samples,
        sample_rate,
        n_mels,
        n_fft,
        hop_length,
    );

    flat.chunks_exact(n_mels).map(<[f32]>::to_vec).collect()
}

// ── ChromaVector ──────────────────────────────────────────────────────────────

/// A 12-element chroma vector representing the energy distribution across
/// the 12 pitch classes (C, C#, D, D#, E, F, F#, G, G#, A, A#, B).
#[derive(Debug, Clone)]
pub struct ChromaVector {
    /// Raw chroma values, one per pitch class.
    pub chroma: [f32; 12],
}

impl ChromaVector {
    /// Return a new `ChromaVector` normalized so that its maximum value is 1.0.
    ///
    /// If all values are zero (or negative), the original vector is returned unchanged.
    #[must_use]
    pub fn normalize(&self) -> Self {
        let max = self
            .chroma
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);

        if max <= 0.0 {
            return self.clone();
        }

        let mut normalized = self.chroma;
        for v in &mut normalized {
            *v /= max;
        }
        Self { chroma: normalized }
    }

    /// Return the index (0–11) of the pitch class with the highest energy.
    #[must_use]
    pub fn dominant_class(&self) -> usize {
        self.chroma
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map_or(0, |(i, _)| i)
    }

    /// Return the sharpness of the chroma vector, defined as `max - min`.
    ///
    /// A high value indicates a strongly peaked distribution (clear pitch class),
    /// while a low value indicates a flat, noisy distribution.
    #[must_use]
    pub fn sharpness(&self) -> f32 {
        let max = self
            .chroma
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let min = self.chroma.iter().copied().fold(f32::INFINITY, f32::min);
        max - min
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── MfccCoeffs ─────────────────────────────────────────────────────────────

    #[test]
    fn test_mfcc_new_empty() {
        let m = MfccCoeffs::new(13);
        assert_eq!(m.num_mfcc, 13);
        assert!(m.coefficients.is_empty());
    }

    #[test]
    fn test_mfcc_add_frame_correct_length() {
        let mut m = MfccCoeffs::new(3);
        m.add_frame(&[1.0, 2.0, 3.0]);
        assert_eq!(m.coefficients.len(), 1);
    }

    #[test]
    fn test_mfcc_add_frame_wrong_length_ignored() {
        let mut m = MfccCoeffs::new(3);
        m.add_frame(&[1.0, 2.0]); // wrong length
        assert!(m.coefficients.is_empty());
    }

    #[test]
    fn test_mfcc_mean_single_frame() {
        let mut m = MfccCoeffs::new(2);
        m.add_frame(&[4.0, 6.0]);
        let mean = m.mean();
        assert!((mean[0] - 4.0).abs() < 1e-5);
        assert!((mean[1] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn test_mfcc_mean_two_frames() {
        let mut m = MfccCoeffs::new(2);
        m.add_frame(&[2.0, 4.0]);
        m.add_frame(&[4.0, 8.0]);
        let mean = m.mean();
        assert!((mean[0] - 3.0).abs() < 1e-5);
        assert!((mean[1] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn test_mfcc_mean_empty_returns_zeros() {
        let m = MfccCoeffs::new(4);
        let mean = m.mean();
        assert_eq!(mean, vec![0.0; 4]);
    }

    #[test]
    fn test_mfcc_variance_two_frames() {
        let mut m = MfccCoeffs::new(1);
        m.add_frame(&[2.0]);
        m.add_frame(&[4.0]);
        // mean = 3.0; var = ((2-3)^2 + (4-3)^2) / 2 = 1.0
        let var = m.variance();
        assert!((var[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_mfcc_variance_one_frame_returns_zeros() {
        let mut m = MfccCoeffs::new(2);
        m.add_frame(&[1.0, 2.0]);
        let var = m.variance();
        assert_eq!(var, vec![0.0; 2]);
    }

    #[test]
    fn test_mfcc_delta_two_frames() {
        let mut m = MfccCoeffs::new(2);
        m.add_frame(&[1.0, 3.0]);
        m.add_frame(&[5.0, 7.0]);
        assert!((m.delta(0) - 4.0).abs() < 1e-5);
        assert!((m.delta(1) - 4.0).abs() < 1e-5);
    }

    #[test]
    fn test_mfcc_delta_one_frame_returns_zero() {
        let mut m = MfccCoeffs::new(2);
        m.add_frame(&[1.0, 2.0]);
        assert!((m.delta(0) - 0.0).abs() < 1e-5);
    }

    // ── compute_log_mel_spectrogram ─────────────────────────────────────────────

    #[test]
    fn test_log_mel_spectrogram_empty_input() {
        let result = compute_log_mel_spectrogram(&[], 44100, 40, 512);
        assert!(result.is_empty());
    }

    #[test]
    fn test_log_mel_spectrogram_output_shape() {
        let samples = vec![0.1f32; 2048];
        let result = compute_log_mel_spectrogram(&samples, 44100, 40, 512);
        // should produce multiple frames, each with 40 mel bins
        assert!(!result.is_empty());
        assert_eq!(result[0].len(), 40);
    }

    #[test]
    fn test_log_mel_spectrogram_zero_input_is_log_epsilon() {
        let samples = vec![0.0f32; 512];
        let result = compute_log_mel_spectrogram(&samples, 44100, 10, 512);
        assert!(!result.is_empty());
        // ln(0 + 1e-10) < 0
        for &v in &result[0] {
            assert!(v < 0.0);
        }
    }

    /// HTK mel scale, as used by the underlying filterbank.
    fn hz_to_mel(hz: f64) -> f64 {
        2595.0 * (1.0 + hz / 700.0).log10()
    }

    /// Index of the triangular filter whose centre is closest to `hz`.
    ///
    /// Filter `i` is centred on mel point `i + 1` of `n_mels + 2` equally
    /// spaced points spanning 0 Hz … Nyquist.
    fn expected_mel_bin(hz: f64, sample_rate: u32, n_mels: usize) -> usize {
        let max_mel = hz_to_mel(f64::from(sample_rate) / 2.0);
        let step = max_mel / (n_mels + 1) as f64;
        let centre_index = hz_to_mel(hz) / step;
        (centre_index.round() as usize)
            .saturating_sub(1)
            .min(n_mels - 1)
    }

    /// A 1 kHz sine must concentrate its energy in the mel band containing
    /// 1 kHz and leave distant bands near the log-epsilon floor.
    ///
    /// Regression guard: the previous implementation filled every mel bin of a
    /// frame with the same frame RMS, so this peak/floor structure was absent.
    #[test]
    fn test_log_mel_spectrogram_1khz_sine_peaks_in_correct_band() {
        const SR: u32 = 16_000;
        const N_MELS: usize = 40;
        const HOP: usize = 160;
        const TONE_HZ: f64 = 1000.0;

        let samples: Vec<f32> = (0..SR)
            .map(|i| {
                (2.0 * std::f64::consts::PI * TONE_HZ * f64::from(i) / f64::from(SR)).sin() as f32
            })
            .collect();

        let spec = compute_log_mel_spectrogram(&samples, SR, N_MELS, HOP);
        assert!(!spec.is_empty(), "spectrogram must not be empty");
        assert_eq!(spec[0].len(), N_MELS);

        // Average over the steady-state middle of the signal.
        let mid = &spec[spec.len() / 2];

        let (peak_bin, &peak_val) = mid
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .expect("non-empty frame");

        let expected_bin = expected_mel_bin(TONE_HZ, SR, N_MELS);
        assert!(
            peak_bin.abs_diff(expected_bin) <= 1,
            "1 kHz peak landed in mel bin {peak_bin}, expected {expected_bin} \
             (frame = {mid:?})"
        );

        // Bins far from the tone must hold essentially no energy. ln(1e-10) is
        // the floor; require at least 40 dB (≈ 9.2 nats of ln-power) below the
        // peak for every bin more than 5 bins away.
        let floor_margin = 9.2_f32;
        for (bin, &v) in mid.iter().enumerate() {
            if bin.abs_diff(peak_bin) > 5 {
                assert!(
                    v < peak_val - floor_margin,
                    "mel bin {bin} = {v:.3} is not ≥40 dB below the {peak_val:.3} peak"
                );
            }
        }
    }

    /// Neighbouring frequencies must land in different mel bands — i.e. the
    /// output actually depends on frequency, not just on frame energy.
    #[test]
    fn test_log_mel_spectrogram_tracks_frequency() {
        const SR: u32 = 16_000;
        const N_MELS: usize = 40;
        const HOP: usize = 160;

        let peak_bin_for = |freq: f64| -> usize {
            let samples: Vec<f32> = (0..SR)
                .map(|i| {
                    (2.0 * std::f64::consts::PI * freq * f64::from(i) / f64::from(SR)).sin() as f32
                })
                .collect();
            let spec = compute_log_mel_spectrogram(&samples, SR, N_MELS, HOP);
            let mid = &spec[spec.len() / 2];
            mid.iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map_or(0, |(i, _)| i)
        };

        let low = peak_bin_for(300.0);
        let high = peak_bin_for(4000.0);
        assert!(
            high > low,
            "4 kHz must peak in a higher mel bin than 300 Hz (got {high} vs {low})"
        );
    }

    /// The explicit-window entry point must honour its `n_fft` argument.
    #[test]
    fn test_log_mel_spectrogram_with_fft_shape() {
        let samples = vec![0.05f32; 16_000];
        let spec = compute_log_mel_spectrogram_with_fft(&samples, 16_000, 80, 400, 160);
        // Centre padding: (16000 + 400 - 400) / 160 + 1 = 101 frames.
        assert_eq!(spec.len(), 101);
        assert_eq!(spec[0].len(), 80);
    }

    // ── ChromaVector ────────────────────────────────────────────────────────────

    #[test]
    fn test_chroma_normalize_max_becomes_one() {
        let cv = ChromaVector {
            chroma: [0.0, 2.0, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let norm = cv.normalize();
        assert!((norm.chroma[1] - 1.0).abs() < 1e-5);
        assert!((norm.chroma[2] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_chroma_normalize_all_zeros_unchanged() {
        let cv = ChromaVector {
            chroma: [0.0f32; 12],
        };
        let norm = cv.normalize();
        assert_eq!(norm.chroma, [0.0f32; 12]);
    }

    #[test]
    fn test_chroma_dominant_class() {
        let mut chroma = [0.0f32; 12];
        chroma[7] = 3.5; // G (index 7) is dominant
        let cv = ChromaVector { chroma };
        assert_eq!(cv.dominant_class(), 7);
    }

    #[test]
    fn test_chroma_sharpness() {
        let cv = ChromaVector {
            chroma: [0.2, 0.9, 0.1, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let sharpness = cv.sharpness();
        assert!((sharpness - 0.9).abs() < 1e-5);
    }

    #[test]
    fn test_chroma_sharpness_uniform_is_zero() {
        let cv = ChromaVector {
            chroma: [1.0f32; 12],
        };
        assert!((cv.sharpness() - 0.0).abs() < 1e-5);
    }
}

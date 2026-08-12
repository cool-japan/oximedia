//! Automatic quality ladder generation for ABR streaming.
//!
//! Generates multi-resolution bitrate ladders for HLS/DASH delivery,
//! validates them for monotonicity, and optimises rungs using VMAF estimates.

use serde::{Deserialize, Serialize};

// ─── LadderPreset ─────────────────────────────────────────────────────────────

/// Named ladder configuration presets for common delivery scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LadderPreset {
    /// Broadcast-quality delivery with high bitrates.
    Broadcast,
    /// Web VOD streaming (balance of quality and bandwidth).
    WebVod,
    /// Mobile-first ladder with conservative bitrates.
    Mobile,
    /// Ultra-HD 4K delivery for premium platforms.
    Ultra4k,
    /// High-quality archival ladder.
    Archive,
    /// Preview / thumbnail quality for fast seeking.
    Preview,
}

impl LadderPreset {
    /// Returns a human-readable label for this preset.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Broadcast => "Broadcast",
            Self::WebVod => "WebVOD",
            Self::Mobile => "Mobile",
            Self::Ultra4k => "Ultra4K",
            Self::Archive => "Archive",
            Self::Preview => "Preview",
        }
    }
}

// ─── BitrateRung ──────────────────────────────────────────────────────────────

/// A single rung in a quality ladder representing one output rendition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitrateRung {
    /// Output height in pixels.
    pub height: u32,
    /// Output width in pixels.
    pub width: u32,
    /// Video bitrate in kbps.
    pub bitrate_kbps: u32,
    /// Video codec name.
    pub codec: String,
    /// Audio bitrate in kbps.
    pub audio_kbps: u32,
}

impl BitrateRung {
    /// Creates a new rung.
    #[must_use]
    pub fn new(
        height: u32,
        width: u32,
        bitrate_kbps: u32,
        codec: impl Into<String>,
        audio_kbps: u32,
    ) -> Self {
        Self {
            height,
            width,
            bitrate_kbps,
            codec: codec.into(),
            audio_kbps,
        }
    }

    /// Returns the total bitrate (video + audio) in kbps.
    #[must_use]
    pub fn total_kbps(&self) -> u32 {
        self.bitrate_kbps.saturating_add(self.audio_kbps)
    }

    /// Returns the pixel count for this rendition.
    #[must_use]
    pub fn pixels(&self) -> u64 {
        self.height as u64 * self.width as u64
    }
}

// ─── LadderSpec ───────────────────────────────────────────────────────────────

/// A complete quality ladder specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LadderSpec {
    /// Which preset was used to generate this ladder.
    pub preset: LadderPreset,
    /// The rendition rungs, ordered from highest to lowest bitrate.
    pub rungs: Vec<BitrateRung>,
    /// Minimum number of rungs required.
    pub min_rungs: u8,
    /// Maximum number of rungs allowed.
    pub max_rungs: u8,
}

impl LadderSpec {
    /// Returns the number of rungs in this ladder.
    #[must_use]
    pub fn rung_count(&self) -> usize {
        self.rungs.len()
    }

    /// Returns the highest quality (first) rung, if any.
    #[must_use]
    pub fn top_rung(&self) -> Option<&BitrateRung> {
        self.rungs.first()
    }

    /// Returns the lowest quality (last) rung, if any.
    #[must_use]
    pub fn bottom_rung(&self) -> Option<&BitrateRung> {
        self.rungs.last()
    }
}

// ─── vmaf_estimate_for_bitrate ─────────────────────────────────────────────────

/// Estimates a VMAF score for a given height and bitrate.
///
/// Uses the perceptual model:
///   `vmaf = 95 × (1 − exp(−bitrate / reference_bitrate))`
///
/// where `reference_bitrate = height² / 100` (kbps).
#[must_use]
pub fn vmaf_estimate_for_bitrate(height: u32, bitrate_kbps: u32) -> f32 {
    if height == 0 || bitrate_kbps == 0 {
        return 0.0;
    }
    let reference_bitrate = (height as f64 * height as f64) / 100.0;
    let exponent = -(bitrate_kbps as f64 / reference_bitrate);
    let vmaf = 95.0 * (1.0 - exponent.exp());
    vmaf.clamp(0.0, 100.0) as f32
}

// ─── LadderGenerator ─────────────────────────────────────────────────────────

/// Generates quality ladders from source dimensions and a preset.
#[derive(Debug, Clone, Default)]
pub struct LadderGenerator;

/// Internal representation of a candidate rung before filtering.
struct CandidateRung {
    height: u32,
    width: u32,
    bitrate_kbps: u32,
    audio_kbps: u32,
}

impl LadderGenerator {
    /// Creates a new generator.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Generates a `LadderSpec` for the given source resolution and preset.
    ///
    /// Rungs above the source height are automatically removed.
    #[must_use]
    pub fn generate(
        &self,
        input_height: u32,
        input_width: u32,
        preset: LadderPreset,
    ) -> LadderSpec {
        let codec = self.default_codec(preset);
        let candidates = self.candidate_rungs(preset);

        // Filter: only include rungs at or below source height
        let rungs: Vec<BitrateRung> = candidates
            .into_iter()
            .filter(|r| r.height <= input_height)
            .map(|r| {
                // Scale width proportionally if input is narrower than the
                // canonical 16:9 width for this height
                let canonical_width = r.height * 16 / 9;
                let effective_width = if input_width < canonical_width {
                    input_width
                } else {
                    r.width
                };
                BitrateRung::new(
                    r.height,
                    effective_width,
                    r.bitrate_kbps,
                    codec,
                    r.audio_kbps,
                )
            })
            .collect();

        let (min_rungs, max_rungs) = self.rung_limits(preset);

        LadderSpec {
            preset,
            rungs,
            min_rungs,
            max_rungs,
        }
    }

    /// Default codec for a preset.
    fn default_codec(&self, preset: LadderPreset) -> &'static str {
        match preset {
            LadderPreset::Archive => "av1",
            LadderPreset::Ultra4k => "av1",
            LadderPreset::Preview => "vp9",
            _ => "vp9",
        }
    }

    /// Minimum and maximum rungs for a preset.
    fn rung_limits(&self, preset: LadderPreset) -> (u8, u8) {
        match preset {
            LadderPreset::Mobile => (2, 4),
            LadderPreset::Preview => (1, 2),
            LadderPreset::Ultra4k => (3, 6),
            LadderPreset::Archive => (2, 5),
            _ => (2, 5),
        }
    }

    /// Returns ordered (high→low) candidate rungs for a preset.
    fn candidate_rungs(&self, preset: LadderPreset) -> Vec<CandidateRung> {
        match preset {
            LadderPreset::Broadcast => vec![
                CandidateRung {
                    height: 2160,
                    width: 3840,
                    bitrate_kbps: 20_000,
                    audio_kbps: 320,
                },
                CandidateRung {
                    height: 1080,
                    width: 1920,
                    bitrate_kbps: 8_000,
                    audio_kbps: 192,
                },
                CandidateRung {
                    height: 720,
                    width: 1280,
                    bitrate_kbps: 4_000,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 540,
                    width: 960,
                    bitrate_kbps: 2_000,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 360,
                    width: 640,
                    bitrate_kbps: 800,
                    audio_kbps: 96,
                },
            ],
            LadderPreset::WebVod => vec![
                CandidateRung {
                    height: 1080,
                    width: 1920,
                    bitrate_kbps: 4_500,
                    audio_kbps: 192,
                },
                CandidateRung {
                    height: 720,
                    width: 1280,
                    bitrate_kbps: 2_500,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 480,
                    width: 854,
                    bitrate_kbps: 1_200,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 360,
                    width: 640,
                    bitrate_kbps: 600,
                    audio_kbps: 96,
                },
                CandidateRung {
                    height: 240,
                    width: 426,
                    bitrate_kbps: 300,
                    audio_kbps: 64,
                },
            ],
            LadderPreset::Mobile => vec![
                CandidateRung {
                    height: 720,
                    width: 1280,
                    bitrate_kbps: 2_000,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 480,
                    width: 854,
                    bitrate_kbps: 1_000,
                    audio_kbps: 96,
                },
                CandidateRung {
                    height: 360,
                    width: 640,
                    bitrate_kbps: 500,
                    audio_kbps: 64,
                },
                CandidateRung {
                    height: 240,
                    width: 426,
                    bitrate_kbps: 200,
                    audio_kbps: 48,
                },
            ],
            LadderPreset::Ultra4k => vec![
                CandidateRung {
                    height: 2160,
                    width: 3840,
                    bitrate_kbps: 35_000,
                    audio_kbps: 320,
                },
                CandidateRung {
                    height: 1440,
                    width: 2560,
                    bitrate_kbps: 16_000,
                    audio_kbps: 256,
                },
                CandidateRung {
                    height: 1080,
                    width: 1920,
                    bitrate_kbps: 8_000,
                    audio_kbps: 192,
                },
                CandidateRung {
                    height: 720,
                    width: 1280,
                    bitrate_kbps: 4_000,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 480,
                    width: 854,
                    bitrate_kbps: 1_500,
                    audio_kbps: 128,
                },
            ],
            LadderPreset::Archive => vec![
                CandidateRung {
                    height: 2160,
                    width: 3840,
                    bitrate_kbps: 15_000,
                    audio_kbps: 256,
                },
                CandidateRung {
                    height: 1080,
                    width: 1920,
                    bitrate_kbps: 6_000,
                    audio_kbps: 192,
                },
                CandidateRung {
                    height: 720,
                    width: 1280,
                    bitrate_kbps: 3_000,
                    audio_kbps: 128,
                },
                CandidateRung {
                    height: 480,
                    width: 854,
                    bitrate_kbps: 1_200,
                    audio_kbps: 96,
                },
            ],
            LadderPreset::Preview => vec![
                CandidateRung {
                    height: 480,
                    width: 854,
                    bitrate_kbps: 400,
                    audio_kbps: 64,
                },
                CandidateRung {
                    height: 240,
                    width: 426,
                    bitrate_kbps: 150,
                    audio_kbps: 32,
                },
            ],
        }
    }
}

// ─── LadderOptimizer ─────────────────────────────────────────────────────────

/// Optimises a `LadderSpec` by removing VMAF-equivalent rungs and inserting
/// intermediate rungs where VMAF gaps are too large.
#[derive(Debug, Clone)]
pub struct LadderOptimizer {
    /// Rungs closer than this many VMAF points are considered equivalent.
    pub vmaf_equivalence_threshold: f32,
    /// A gap larger than this triggers insertion of an intermediate rung.
    pub vmaf_gap_threshold: f32,
}

impl Default for LadderOptimizer {
    fn default() -> Self {
        Self {
            vmaf_equivalence_threshold: 5.0,
            vmaf_gap_threshold: 10.0,
        }
    }
}

impl LadderOptimizer {
    /// Creates an optimizer with default thresholds (5 / 10 VMAF points).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an optimizer with custom thresholds.
    #[must_use]
    pub fn with_thresholds(equivalence: f32, gap: f32) -> Self {
        Self {
            vmaf_equivalence_threshold: equivalence,
            vmaf_gap_threshold: gap,
        }
    }

    /// Optimises `spec` in-place: removes near-equivalent rungs, adds
    /// intermediate rungs for large VMAF gaps.
    #[must_use]
    pub fn optimize(&self, spec: LadderSpec) -> LadderSpec {
        if spec.rungs.is_empty() {
            return spec;
        }

        let optimized_rungs = self.remove_equivalent_rungs(spec.rungs);
        let optimized_rungs = self.fill_large_gaps(optimized_rungs);

        LadderSpec {
            rungs: optimized_rungs,
            ..spec
        }
    }

    /// Removes adjacent rungs that differ by fewer than `vmaf_equivalence_threshold`.
    ///
    /// Delegates to [`Self::filter_equivalent`], which implements this
    /// correctly (this method previously stopped after keeping only the
    /// first rung — see `optimize` vs `optimize_full`, which are now
    /// equivalent).
    fn remove_equivalent_rungs(&self, rungs: Vec<BitrateRung>) -> Vec<BitrateRung> {
        self.filter_equivalent(rungs)
    }

    /// Fills large VMAF gaps by inserting a midpoint rung between adjacent pairs.
    fn fill_large_gaps(&self, rungs: Vec<BitrateRung>) -> Vec<BitrateRung> {
        if rungs.len() < 2 {
            return rungs;
        }

        let mut result: Vec<BitrateRung> = Vec::with_capacity(rungs.len() * 2);
        let mut iter = rungs.into_iter().peekable();

        while let Some(rung) = iter.next() {
            if let Some(next) = iter.peek() {
                let v_current = vmaf_estimate_for_bitrate(rung.height, rung.bitrate_kbps);
                let v_next = vmaf_estimate_for_bitrate(next.height, next.bitrate_kbps);
                let gap = (v_current - v_next).abs();

                if gap > self.vmaf_gap_threshold {
                    // Insert midpoint rung
                    let mid_height = (rung.height + next.height) / 2;
                    let mid_bitrate = (rung.bitrate_kbps + next.bitrate_kbps) / 2;
                    let mid_width = (rung.width + next.width) / 2;
                    let mid_audio = (rung.audio_kbps + next.audio_kbps) / 2;
                    let codec = rung.codec.clone();
                    result.push(rung);
                    result.push(BitrateRung::new(
                        mid_height,
                        mid_width,
                        mid_bitrate,
                        codec,
                        mid_audio,
                    ));
                    continue;
                }
            }
            result.push(rung);
        }
        result
    }

    /// Optimise with full equivalent-rung removal (non-broken version).
    #[must_use]
    pub fn optimize_full(&self, spec: LadderSpec) -> LadderSpec {
        if spec.rungs.is_empty() {
            return spec;
        }

        let filtered = self.filter_equivalent(spec.rungs);
        let filled = self.fill_large_gaps(filtered);

        LadderSpec {
            rungs: filled,
            ..spec
        }
    }

    /// Filter rungs, keeping only those that differ by >= threshold from
    /// the previously kept rung.
    fn filter_equivalent(&self, rungs: Vec<BitrateRung>) -> Vec<BitrateRung> {
        let mut kept: Vec<BitrateRung> = Vec::with_capacity(rungs.len());
        for rung in rungs {
            if let Some(last) = kept.last() {
                let v_last = vmaf_estimate_for_bitrate(last.height, last.bitrate_kbps);
                let v_cur = vmaf_estimate_for_bitrate(rung.height, rung.bitrate_kbps);
                let diff = (v_last - v_cur).abs();
                if diff < self.vmaf_equivalence_threshold {
                    // Skip — equivalent to previous rung
                    continue;
                }
            }
            kept.push(rung);
        }
        kept
    }
}

// ─── LadderValidator ─────────────────────────────────────────────────────────

/// Validates a `LadderSpec` for structural correctness.
#[derive(Debug, Clone, Default)]
pub struct LadderValidator;

/// Validation errors for a ladder specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LadderValidationError {
    /// Fewer than two rungs.
    TooFewRungs {
        /// Actual number of rungs present.
        count: usize,
    },
    /// A rung has the same height as another.
    DuplicateHeight {
        /// The repeated height value in pixels.
        height: u32,
    },
    /// Bitrates are not strictly decreasing from top to bottom.
    NonMonotonicBitrate {
        /// Index of the offending rung.
        index: usize,
        /// Bitrate of the offending rung.
        bitrate: u32,
        /// Bitrate of the preceding rung (should be strictly higher).
        prev_bitrate: u32,
    },
    /// Heights are not strictly decreasing from top to bottom.
    NonMonotonicHeight {
        /// Index of the offending rung.
        index: usize,
        /// Height of the offending rung.
        height: u32,
        /// Height of the preceding rung (should be strictly higher).
        prev_height: u32,
    },
}

impl std::fmt::Display for LadderValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewRungs { count } => write!(f, "Ladder has {count} rung(s); minimum is 2"),
            Self::DuplicateHeight { height } => write!(f, "Duplicate height {height}p in ladder"),
            Self::NonMonotonicBitrate {
                index,
                bitrate,
                prev_bitrate,
            } => {
                write!(
                    f,
                    "Rung {index}: bitrate {bitrate} >= previous {prev_bitrate}"
                )
            }
            Self::NonMonotonicHeight {
                index,
                height,
                prev_height,
            } => {
                write!(f, "Rung {index}: height {height} >= previous {prev_height}")
            }
        }
    }
}

impl LadderValidator {
    /// Creates a new validator.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Validates `spec`, returning a list of all errors found.
    ///
    /// An empty list means the ladder is valid.
    #[must_use]
    pub fn validate(&self, spec: &LadderSpec) -> Vec<LadderValidationError> {
        let mut errors = Vec::new();

        if spec.rungs.len() < 2 {
            errors.push(LadderValidationError::TooFewRungs {
                count: spec.rungs.len(),
            });
            return errors; // No point checking further
        }

        for (i, rung) in spec.rungs.iter().enumerate().skip(1) {
            let prev = &spec.rungs[i - 1];

            // Monotonic bitrates (descending)
            if rung.bitrate_kbps >= prev.bitrate_kbps {
                errors.push(LadderValidationError::NonMonotonicBitrate {
                    index: i,
                    bitrate: rung.bitrate_kbps,
                    prev_bitrate: prev.bitrate_kbps,
                });
            }

            // Monotonic heights (descending)
            if rung.height >= prev.height {
                errors.push(LadderValidationError::NonMonotonicHeight {
                    index: i,
                    height: rung.height,
                    prev_height: prev.height,
                });
            }
        }

        // Duplicate heights
        let mut seen_heights = std::collections::HashSet::new();
        for rung in &spec.rungs {
            if !seen_heights.insert(rung.height) {
                errors.push(LadderValidationError::DuplicateHeight {
                    height: rung.height,
                });
            }
        }

        errors
    }

    /// Returns `true` if the ladder passes all validation checks.
    #[must_use]
    pub fn is_valid(&self, spec: &LadderSpec) -> bool {
        self.validate(spec).is_empty()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── LadderGenerator ───────────────────────────────────────────────────────

    #[test]
    fn test_generate_webvod_1080p_source() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(1080, 1920, LadderPreset::WebVod);
        assert!(!spec.rungs.is_empty());
        // No rung should exceed 1080p
        assert!(spec.rungs.iter().all(|r| r.height <= 1080));
    }

    #[test]
    fn test_generate_broadcast_strips_above_source() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(720, 1280, LadderPreset::Broadcast);
        assert!(spec.rungs.iter().all(|r| r.height <= 720));
    }

    #[test]
    fn test_generate_mobile_ladder_has_enough_rungs() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(720, 1280, LadderPreset::Mobile);
        assert!(spec.rungs.len() >= 2);
    }

    #[test]
    fn test_generate_ultra4k_includes_4k_for_4k_source() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(2160, 3840, LadderPreset::Ultra4k);
        assert!(spec.rungs.iter().any(|r| r.height == 2160));
    }

    #[test]
    fn test_generate_no_rung_exceeds_source_height() {
        let gen = LadderGenerator::new();
        for preset in [
            LadderPreset::WebVod,
            LadderPreset::Mobile,
            LadderPreset::Broadcast,
        ] {
            let spec = gen.generate(480, 854, preset);
            for rung in &spec.rungs {
                assert!(
                    rung.height <= 480,
                    "{preset:?}: rung {}p exceeds source 480p",
                    rung.height
                );
            }
        }
    }

    #[test]
    fn test_ladder_spec_top_bottom_rungs() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(1080, 1920, LadderPreset::WebVod);
        assert!(spec.top_rung().is_some());
        assert!(spec.bottom_rung().is_some());
        let top = spec.top_rung().expect("top rung");
        let bottom = spec.bottom_rung().expect("bottom rung");
        assert!(top.bitrate_kbps >= bottom.bitrate_kbps);
    }

    #[test]
    fn test_bitrate_rung_total_kbps() {
        let rung = BitrateRung::new(720, 1280, 2500, "vp9", 128);
        assert_eq!(rung.total_kbps(), 2628);
    }

    #[test]
    fn test_bitrate_rung_pixels() {
        let rung = BitrateRung::new(1080, 1920, 4500, "vp9", 192);
        assert_eq!(rung.pixels(), 1080 * 1920);
    }

    // ── vmaf_estimate_for_bitrate ──────────────────────────────────────────────

    #[test]
    fn test_vmaf_zero_inputs() {
        assert_eq!(vmaf_estimate_for_bitrate(0, 1000), 0.0);
        assert_eq!(vmaf_estimate_for_bitrate(1080, 0), 0.0);
    }

    #[test]
    fn test_vmaf_approaches_95_at_high_bitrate() {
        let score = vmaf_estimate_for_bitrate(1080, 1_000_000);
        assert!(score > 94.0, "Expected near-95 VMAF, got {score}");
    }

    #[test]
    fn test_vmaf_increases_with_bitrate() {
        let low = vmaf_estimate_for_bitrate(720, 500);
        let high = vmaf_estimate_for_bitrate(720, 5000);
        assert!(high > low);
    }

    #[test]
    fn test_vmaf_lower_resolution_higher_score_at_same_bitrate() {
        let score_240 = vmaf_estimate_for_bitrate(240, 500);
        let score_1080 = vmaf_estimate_for_bitrate(1080, 500);
        assert!(
            score_240 > score_1080,
            "Lower res should have higher VMAF at same bitrate"
        );
    }

    // ── LadderOptimizer ───────────────────────────────────────────────────────

    #[test]
    fn test_optimizer_does_not_increase_rung_count_on_similar_ladder() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(1080, 1920, LadderPreset::WebVod);
        let original_count = spec.rungs.len();
        let opt = LadderOptimizer::new();
        let optimized = opt.optimize_full(spec);
        // Optimizer may remove rungs; it should not dramatically increase them
        // (gap filling could add at most N-1 rungs for N rungs)
        assert!(optimized.rungs.len() <= original_count * 2 + 1);
    }

    #[test]
    fn test_optimize_retains_distinct_rungs_not_just_first() {
        // Regression test: `remove_equivalent_rungs` (used by `optimize`)
        // used to push only the first rung and return, silently discarding
        // every other rendition in the ladder regardless of how different
        // it was from its neighbours.
        let spec = LadderSpec {
            preset: LadderPreset::WebVod,
            rungs: vec![
                BitrateRung::new(1080, 1920, 6000, "h264", 192),
                BitrateRung::new(720, 1280, 3000, "h264", 128),
                BitrateRung::new(480, 854, 1200, "h264", 96),
                BitrateRung::new(240, 426, 400, "h264", 64),
            ],
            min_rungs: 2,
            max_rungs: 5,
        };
        let opt = LadderOptimizer::new();

        let via_optimize = opt.optimize(spec.clone());
        assert!(
            via_optimize.rungs.len() > 1,
            "expected more than one surviving rung for four clearly distinct \
             renditions, got {}",
            via_optimize.rungs.len()
        );

        // `optimize` and `optimize_full` must now agree; the plain
        // `optimize` path previously stopped after the first rung.
        let via_optimize_full = opt.optimize_full(spec);
        assert_eq!(via_optimize.rungs.len(), via_optimize_full.rungs.len());
    }

    #[test]
    fn test_optimizer_empty_spec_passthrough() {
        let spec = LadderSpec {
            preset: LadderPreset::WebVod,
            rungs: vec![],
            min_rungs: 2,
            max_rungs: 5,
        };
        let opt = LadderOptimizer::new();
        let result = opt.optimize(spec);
        assert!(result.rungs.is_empty());
    }

    #[test]
    fn test_optimizer_with_thresholds() {
        let opt = LadderOptimizer::with_thresholds(3.0, 15.0);
        assert!((opt.vmaf_equivalence_threshold - 3.0).abs() < 1e-6);
        assert!((opt.vmaf_gap_threshold - 15.0).abs() < 1e-6);
    }

    // ── LadderValidator ───────────────────────────────────────────────────────

    #[test]
    fn test_validator_valid_webvod_ladder() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(1080, 1920, LadderPreset::WebVod);
        let validator = LadderValidator::new();
        let errors = validator.validate(&spec);
        assert!(
            errors.is_empty(),
            "WebVOD 1080p ladder should be valid; errors: {errors:?}"
        );
    }

    #[test]
    fn test_validator_too_few_rungs() {
        let spec = LadderSpec {
            preset: LadderPreset::WebVod,
            rungs: vec![BitrateRung::new(1080, 1920, 4500, "vp9", 128)],
            min_rungs: 2,
            max_rungs: 5,
        };
        let validator = LadderValidator::new();
        let errors = validator.validate(&spec);
        assert!(errors
            .iter()
            .any(|e| matches!(e, LadderValidationError::TooFewRungs { .. })));
    }

    #[test]
    fn test_validator_duplicate_height() {
        let spec = LadderSpec {
            preset: LadderPreset::WebVod,
            rungs: vec![
                BitrateRung::new(1080, 1920, 4500, "vp9", 128),
                BitrateRung::new(1080, 1920, 2000, "vp9", 128),
            ],
            min_rungs: 2,
            max_rungs: 5,
        };
        let validator = LadderValidator::new();
        let errors = validator.validate(&spec);
        assert!(errors
            .iter()
            .any(|e| matches!(e, LadderValidationError::DuplicateHeight { height: 1080 })));
    }

    #[test]
    fn test_validator_non_monotonic_bitrate() {
        let spec = LadderSpec {
            preset: LadderPreset::WebVod,
            rungs: vec![
                BitrateRung::new(1080, 1920, 1000, "vp9", 128), // low bitrate at top
                BitrateRung::new(720, 1280, 4500, "vp9", 128),  // higher bitrate at bottom
            ],
            min_rungs: 2,
            max_rungs: 5,
        };
        let validator = LadderValidator::new();
        let errors = validator.validate(&spec);
        assert!(errors
            .iter()
            .any(|e| matches!(e, LadderValidationError::NonMonotonicBitrate { .. })));
    }

    #[test]
    fn test_validator_is_valid_helper() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(720, 1280, LadderPreset::Mobile);
        let validator = LadderValidator::new();
        assert!(
            validator.is_valid(&spec),
            "Generated Mobile 720p ladder should be valid"
        );
    }

    #[test]
    fn test_ladder_preset_labels_are_non_empty() {
        for preset in [
            LadderPreset::Broadcast,
            LadderPreset::WebVod,
            LadderPreset::Mobile,
            LadderPreset::Ultra4k,
            LadderPreset::Archive,
            LadderPreset::Preview,
        ] {
            assert!(!preset.label().is_empty());
        }
    }

    #[test]
    fn test_archive_uses_av1_codec() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(1080, 1920, LadderPreset::Archive);
        for rung in &spec.rungs {
            assert_eq!(rung.codec, "av1");
        }
    }

    #[test]
    fn test_preview_ladder_has_at_most_2_rungs_for_480p() {
        let gen = LadderGenerator::new();
        let spec = gen.generate(480, 854, LadderPreset::Preview);
        assert!(spec.rungs.len() <= 2);
    }
}

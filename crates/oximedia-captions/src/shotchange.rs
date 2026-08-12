//! Shot change detection for caption timing

use crate::error::{CaptionError, Result};
use crate::types::{Caption, CaptionTrack, Duration, Timestamp};
use oximedia_container::demux::y4m::Y4mDemuxer;

/// Number of luma histogram bins used by [`frame_diff_score`].
const HISTOGRAM_BINS: usize = 64;

/// Trailing-window size (in frame-pairs) used to adapt the cut threshold to
/// local content motion.
const ADAPTIVE_WINDOW: usize = 10;

/// Local-window multiplier: a candidate cut must exceed the trailing
/// window's mean diff score by this factor (on top of clearing
/// [`ABSOLUTE_FLOOR`]) to be flagged. This raises the effective bar in
/// already-noisy/high-motion content without ever lowering it below the
/// floor, which is what keeps a single huge cut score from being judged
/// against a threshold inflated by itself.
const ADAPTIVE_MULTIPLIER: f64 = 3.0;

/// Minimum combined diff score, regardless of local content, below which a
/// frame pair is never considered a cut. Calibrated so a genuine hard cut
/// between visually distinct shots clears it with a wide margin while a
/// smooth pixel-level ramp (fade/dissolve) never does.
const ABSOLUTE_FLOOR: f64 = 0.20;

/// Shot change detector
pub struct ShotChangeDetector {
    /// Minimum shot duration (frames)
    min_shot_duration: u32,
    /// Frame rate
    fps: f64,
}

impl ShotChangeDetector {
    /// Create a new shot change detector
    #[must_use]
    pub fn new(fps: f64) -> Self {
        Self {
            min_shot_duration: 12, // Minimum 12 frames
            fps,
        }
    }

    /// Set minimum shot duration in frames
    pub fn set_min_shot_duration(&mut self, frames: u32) {
        self.min_shot_duration = frames;
    }

    /// Detect shot changes (hard cuts) in a raw Y4M video stream.
    ///
    /// `video_data` must be a complete YUV4MPEG2 stream (header + `FRAME`-
    /// tagged planar YUV frames), demuxed via [`Y4mDemuxer`] — the real,
    /// already-shipped Y4M decode path in `oximedia-container`. For each
    /// consecutive frame pair this compares the luma (Y) plane only — the
    /// first `width * height` bytes of every Y4M frame regardless of
    /// chroma subsampling mode — using a blend of a 64-bin luma histogram
    /// difference and a mean absolute luma difference (SAD), both
    /// normalized to `[0.0, 1.0]`.
    ///
    /// A cut is flagged where that combined score clears both an absolute
    /// floor and an adaptive threshold derived from the trailing window of
    /// recent scores (so a run of high-motion content raises the bar rather
    /// than producing false cuts), and where at least
    /// [`Self::set_min_shot_duration`] frames have elapsed since the last
    /// flagged cut. The returned timestamp for a detected cut is the
    /// *first frame of the new shot*, converted using the frame rate
    /// declared in the Y4M header itself (authoritative for the decoded
    /// bytes) — not `self.fps`. If the two differ, tolerance windows
    /// computed elsewhere against `self.fps` (e.g. in
    /// [`Self::snap_to_shots`]) will be measured on a different clock than
    /// the timestamps returned here; construct the detector with the same
    /// fps as the source video to keep them consistent.
    ///
    /// A stream with fewer than 2 frames (or containing zero internal
    /// cuts) legitimately returns `Ok(vec![])` — real analysis ran and
    /// found nothing, which is a different outcome from analysis not
    /// running at all (see the historical honesty note this replaced,
    /// still relevant to why this never silently substitutes a fabricated
    /// empty result for a real error below).
    ///
    /// # Errors
    ///
    /// Returns [`CaptionError::Other`] if `video_data` is not a valid Y4M
    /// stream (bad/missing header, truncated frame, or a frame smaller than
    /// its declared luma plane).
    pub fn detect_shot_changes(&self, video_data: &[u8]) -> Result<Vec<Timestamp>> {
        let mut demuxer = Y4mDemuxer::new(std::io::Cursor::new(video_data)).map_err(|e| {
            CaptionError::Other(format!("shot-change detection: invalid Y4M header: {e}"))
        })?;

        let width = demuxer.width() as usize;
        let height = demuxer.height() as usize;
        let luma_len = width * height;
        let fps = demuxer.header().fps();
        if luma_len == 0 || !fps.is_finite() || fps <= 0.0 {
            return Err(CaptionError::Other(
                "shot-change detection: Y4M stream has zero dimensions or an invalid frame rate"
                    .to_string(),
            ));
        }

        // Pass 1: decode every frame's luma plane and score each consecutive
        // pair. `scores[k] = (frame_index, score)` where `frame_index` is
        // the index of the SECOND (newer) frame of the pair — i.e. the
        // first frame of what would become the new shot if this is a cut.
        let mut prev_luma: Option<Vec<u8>> = None;
        let mut scores: Vec<(usize, f64)> = Vec::new();
        let mut frame_index: usize = 0;
        loop {
            let frame = demuxer.read_frame().map_err(|e| {
                CaptionError::Other(format!(
                    "shot-change detection: Y4M frame read error at frame {frame_index}: {e}"
                ))
            })?;
            let Some(frame) = frame else {
                break;
            };
            if frame.len() < luma_len {
                return Err(CaptionError::Other(format!(
                    "shot-change detection: frame {frame_index} is smaller than its declared \
                     luma plane ({} bytes < {luma_len})",
                    frame.len()
                )));
            }
            let luma = &frame[..luma_len];
            if let Some(prev) = &prev_luma {
                scores.push((frame_index, frame_diff_score(prev, luma)));
            }
            prev_luma = Some(luma.to_vec());
            frame_index += 1;
        }

        // Pass 2: flag cuts using a trailing-window adaptive threshold
        // clamped to never drop below ABSOLUTE_FLOOR, plus a
        // min-shot-duration debounce.
        let mut cuts = Vec::new();
        let mut last_cut_frame: i64 = 0;
        for (idx, &(candidate_frame, score)) in scores.iter().enumerate() {
            let window_start = idx.saturating_sub(ADAPTIVE_WINDOW);
            let window = &scores[window_start..idx];
            let local_mean = if window.is_empty() {
                0.0
            } else {
                window.iter().map(|&(_, s)| s).sum::<f64>() / window.len() as f64
            };
            let threshold = ABSOLUTE_FLOOR.max(local_mean * ADAPTIVE_MULTIPLIER);

            let far_enough =
                candidate_frame as i64 - last_cut_frame >= i64::from(self.min_shot_duration);
            if score > threshold && far_enough {
                cuts.push(candidate_frame);
                last_cut_frame = candidate_frame as i64;
            }
        }

        Ok(cuts
            .into_iter()
            .map(|frame| Timestamp::from_micros((frame as f64 * 1_000_000.0 / fps).round() as i64))
            .collect())
    }

    /// Snap caption boundaries to shot changes
    pub fn snap_to_shots(
        &self,
        track: &mut CaptionTrack,
        shot_changes: &[Timestamp],
        tolerance_frames: u32,
    ) -> Result<usize> {
        let tolerance =
            Duration::from_micros((f64::from(tolerance_frames) * 1_000_000.0 / self.fps) as i64);

        let mut snapped_count = 0;

        for caption in &mut track.captions {
            // Try to snap start time to nearest shot change
            if let Some(&shot_time) = self.find_nearest_shot(caption.start, shot_changes, tolerance)
            {
                caption.start = shot_time;
                snapped_count += 1;
            }

            // Try to snap end time to nearest shot change
            if let Some(&shot_time) = self.find_nearest_shot(caption.end, shot_changes, tolerance) {
                caption.end = shot_time;
                snapped_count += 1;
            }
        }

        Ok(snapped_count)
    }

    /// Find nearest shot change within tolerance
    fn find_nearest_shot<'a>(
        &self,
        timestamp: Timestamp,
        shot_changes: &'a [Timestamp],
        tolerance: Duration,
    ) -> Option<&'a Timestamp> {
        shot_changes
            .iter()
            .filter(|&&shot_time| {
                let diff = if shot_time > timestamp {
                    shot_time.duration_since(timestamp)
                } else {
                    timestamp.duration_since(shot_time)
                };
                diff <= tolerance
            })
            .min_by_key(|&&shot_time| {
                if shot_time > timestamp {
                    shot_time.duration_since(timestamp).as_micros()
                } else {
                    timestamp.duration_since(shot_time).as_micros()
                }
            })
    }

    /// Check if caption boundaries align with shots
    #[must_use]
    pub fn check_alignment(
        &self,
        caption: &Caption,
        shot_changes: &[Timestamp],
        tolerance_frames: u32,
    ) -> bool {
        let tolerance =
            Duration::from_micros((f64::from(tolerance_frames) * 1_000_000.0 / self.fps) as i64);

        let start_aligned = self
            .find_nearest_shot(caption.start, shot_changes, tolerance)
            .is_some();
        let end_aligned = self
            .find_nearest_shot(caption.end, shot_changes, tolerance)
            .is_some();

        start_aligned && end_aligned
    }

    /// Calculate shot-based metrics
    #[must_use]
    pub fn calculate_shot_metrics(
        &self,
        track: &CaptionTrack,
        shot_changes: &[Timestamp],
    ) -> ShotMetrics {
        let mut metrics = ShotMetrics::default();

        for caption in &track.captions {
            // Count shots spanned by this caption
            let shots_in_range: Vec<&Timestamp> = shot_changes
                .iter()
                .filter(|&&t| t >= caption.start && t < caption.end)
                .collect();

            let shot_count = shots_in_range.len() + 1; // +1 for the initial shot
            metrics.total_shots_spanned += shot_count;

            if shot_count > 1 {
                metrics.captions_spanning_shots += 1;
            }

            if shot_count > metrics.max_shots_per_caption {
                metrics.max_shots_per_caption = shot_count;
            }
        }

        if !track.captions.is_empty() {
            metrics.avg_shots_per_caption =
                metrics.total_shots_spanned as f64 / track.captions.len() as f64;
        }

        metrics
    }
}

/// Score how different two equal-length luma planes are, as a value in
/// `[0.0, 1.0]` (0 = identical, 1 = maximally different).
///
/// Blends two classic scene-cut signals equally:
/// - a 64-bin luma histogram's total-variation distance (`sum(|h1 -
///   h2|) / (2 * pixel_count)`, which is exactly `[0, 1]` since each
///   histogram sums to `pixel_count`), and
/// - mean absolute luma difference (SAD), normalized by 255.
///
/// The histogram term catches hard cuts between differently-lit/composed
/// shots even under global brightness shifts that SAD alone can
/// under-weight; the SAD term catches high-contrast local motion that two
/// frames with similar overall histograms could otherwise mask. Operating
/// on luma only (not chroma) matches how Y4M lays out frame data — the Y
/// plane is always first and full-resolution regardless of chroma
/// subsampling — so no color-space conversion is needed.
fn frame_diff_score(prev: &[u8], curr: &[u8]) -> f64 {
    let mut hist_prev = [0u32; HISTOGRAM_BINS];
    let mut hist_curr = [0u32; HISTOGRAM_BINS];
    let mut sad: u64 = 0;

    for (&p, &c) in prev.iter().zip(curr.iter()) {
        hist_prev[usize::from(p) * HISTOGRAM_BINS / 256] += 1;
        hist_curr[usize::from(c) * HISTOGRAM_BINS / 256] += 1;
        sad += u64::from(p.abs_diff(c));
    }

    let pixel_count = prev.len().max(1) as f64;
    let hist_diff = hist_prev
        .iter()
        .zip(hist_curr.iter())
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
        .sum::<f64>()
        / (2.0 * pixel_count);
    let sad_norm = (sad as f64 / pixel_count) / 255.0;

    0.5f64.mul_add(hist_diff, 0.5 * sad_norm)
}

/// Shot-based caption metrics
#[derive(Debug, Clone, Default)]
pub struct ShotMetrics {
    /// Total shots spanned by all captions
    pub total_shots_spanned: usize,
    /// Number of captions that span multiple shots
    pub captions_spanning_shots: usize,
    /// Maximum shots spanned by a single caption
    pub max_shots_per_caption: usize,
    /// Average shots per caption
    pub avg_shots_per_caption: f64,
}

/// Scene-based captioning utilities
pub struct SceneCaptioning;

impl SceneCaptioning {
    /// Split long captions at shot boundaries
    pub fn split_at_shots(
        track: &mut CaptionTrack,
        shot_changes: &[Timestamp],
        min_duration_ms: i64,
    ) -> Result<usize> {
        let mut new_captions = Vec::new();
        let mut split_count = 0;

        for caption in &track.captions {
            let shots_in_range: Vec<Timestamp> = shot_changes
                .iter()
                .filter(|&&t| t > caption.start && t < caption.end)
                .copied()
                .collect();

            if shots_in_range.is_empty() {
                // No shots to split at
                new_captions.push(caption.clone());
                continue;
            }

            // Split the caption at shot boundaries
            let words: Vec<&str> = caption.text.split_whitespace().collect();
            let mut current_start = caption.start;

            for &shot_time in &shots_in_range {
                let duration_ms = shot_time.duration_since(current_start).as_millis();
                if duration_ms < min_duration_ms {
                    continue; // Too short, skip
                }

                // Estimate how many words fit in this segment
                let total_duration = caption.end.duration_since(caption.start).as_millis();
                let segment_ratio = duration_ms as f64 / total_duration as f64;
                let words_in_segment = (words.len() as f64 * segment_ratio).ceil() as usize;

                if words_in_segment > 0 && words_in_segment < words.len() {
                    let segment_text = words[..words_in_segment].join(" ");
                    let mut new_caption = caption.clone();
                    new_caption.start = current_start;
                    new_caption.end = shot_time;
                    new_caption.text = segment_text;
                    new_captions.push(new_caption);

                    current_start = shot_time;
                    split_count += 1;
                }
            }

            // Add final segment
            if current_start < caption.end {
                let mut new_caption = caption.clone();
                new_caption.start = current_start;
                new_caption.end = caption.end;
                // Remaining text would need to be calculated
                new_captions.push(new_caption);
            }
        }

        track.captions = new_captions;
        Ok(split_count)
    }

    /// Merge consecutive captions within the same shot
    pub fn merge_within_shots(
        track: &mut CaptionTrack,
        shot_changes: &[Timestamp],
        max_chars: usize,
    ) -> Result<usize> {
        let mut merged_captions = Vec::new();
        let mut merge_count = 0;
        let mut i = 0;

        while i < track.captions.len() {
            let mut current = track.captions[i].clone();
            let mut j = i + 1;

            // Try to merge with following captions in the same shot
            while j < track.captions.len() {
                let next = &track.captions[j];

                // Check if they're in the same shot
                let shot_between = shot_changes
                    .iter()
                    .any(|&t| t > current.end && t < next.start);

                if shot_between {
                    break; // Different shots
                }

                // Check if merged text would be too long
                let merged_text = format!("{} {}", current.text, next.text);
                if merged_text.len() > max_chars {
                    break;
                }

                // Merge
                current.end = next.end;
                current.text = merged_text;
                merge_count += 1;
                j += 1;
            }

            merged_captions.push(current);
            i = j;
        }

        track.captions = merged_captions;
        Ok(merge_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Language;

    // ---------------------------------------------------------------------
    // Y4M fixture builders
    // ---------------------------------------------------------------------
    //
    // Frames use a per-pixel texture — not a single flat luma value — so
    // the 64-bin luma histogram has genuine spread instead of collapsing to
    // a single spike. A flat/constant-per-frame fixture is degenerate for a
    // histogram-based metric: with zero within-frame variance, *any* luma
    // shift that crosses a single bin boundary relocates the entire spike
    // and reads as a maximal (1.0) histogram difference — which would make
    // a slow fade indistinguishable from a hard cut. TEXTURE_SPREAD gives
    // each frame real width in the histogram so small inter-frame shifts
    // produce small (mostly-overlapping) histogram differences, matching
    // how a real gradual fade behaves.

    /// Luma spread (in levels) of the synthetic per-pixel texture pattern.
    const TEXTURE_SPREAD: i32 = 24;

    /// One mono (luma-only) Y4M frame: pixel `i` gets `base + (i %
    /// TEXTURE_SPREAD)`, clamped to a valid byte.
    fn textured_frame(base: i32, pixel_count: usize) -> Vec<u8> {
        (0..pixel_count)
            .map(|i| (base + (i as i32 % TEXTURE_SPREAD)).clamp(0, 255) as u8)
            .collect()
    }

    /// Build a complete mono-chroma Y4M byte stream, one frame per entry in
    /// `frame_bases` (each frame textured via [`textured_frame`]). Mono
    /// chroma keeps the fixture minimal — the detector only ever reads the
    /// luma plane, and Y4M's own doc comments confirm the Y plane is always
    /// first and full-resolution regardless of chroma subsampling, so this
    /// exercises the exact same luma-extraction path a chroma-subsampled
    /// real-world file would.
    fn build_mono_y4m(width: u32, height: u32, fps: u32, frame_bases: &[i32]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(
            format!("YUV4MPEG2 W{width} H{height} F{fps}:1 Ip Cmono\n").as_bytes(),
        );
        let pixel_count = (width as usize) * (height as usize);
        for &base in frame_bases {
            data.extend_from_slice(b"FRAME\n");
            data.extend(textured_frame(base, pixel_count));
        }
        data
    }

    const FIXTURE_W: u32 = 16;
    const FIXTURE_H: u32 = 16;
    const FIXTURE_FPS: u32 = 25;

    #[test]
    fn test_frame_diff_score_separates_cut_from_fade() {
        // Sanity check on the scoring primitive itself, independent of Y4M
        // demux or thresholding: a hard cut must score far above a gradual
        // per-frame fade step.
        let pixels = (FIXTURE_W as usize) * (FIXTURE_H as usize);
        let shot_a = textured_frame(20, pixels);
        let shot_b = textured_frame(200, pixels);
        let cut_score = frame_diff_score(&shot_a, &shot_b);

        let fade_a = textured_frame(20, pixels);
        let fade_b = textured_frame(23, pixels);
        let fade_score = frame_diff_score(&fade_a, &fade_b);

        assert!(cut_score > ABSOLUTE_FLOOR, "cut score {cut_score} too low");
        assert!(
            fade_score < ABSOLUTE_FLOOR,
            "fade step score {fade_score} unexpectedly high"
        );
        assert!(cut_score > fade_score * 5.0);
    }

    #[test]
    fn test_detect_shot_changes_hard_cut_within_one_frame() {
        // Shot A: frames 0..15 (base=20). Shot B: frames 15..30 (base=200).
        // The cut is expected at frame 15 (first frame of the new shot).
        let mut bases = vec![20; 15];
        bases.extend(std::iter::repeat_n(200, 15));
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("valid Y4M with a real cut should analyze successfully");

        assert_eq!(cuts.len(), 1, "expected exactly one detected cut: {cuts:?}");

        let expected =
            Timestamp::from_micros((15.0 * 1_000_000.0 / f64::from(FIXTURE_FPS)).round() as i64);
        let one_frame_us = (1_000_000.0 / f64::from(FIXTURE_FPS)) as i64;
        let diff = (cuts[0].as_micros() - expected.as_micros()).abs();
        assert!(
            diff <= one_frame_us,
            "detected cut {:?} not within +-1 frame of expected {expected:?}",
            cuts[0]
        );
    }

    #[test]
    fn test_detect_shot_changes_two_cuts_respect_min_shot_duration() {
        // Three 15-frame shots -> cuts expected at frame 15 and frame 30,
        // each >= the default 12-frame min_shot_duration apart.
        let mut bases = vec![20; 15];
        bases.extend(std::iter::repeat_n(120, 15));
        bases.extend(std::iter::repeat_n(220, 15));
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("valid multi-cut Y4M should analyze successfully");

        assert_eq!(cuts.len(), 2, "expected two detected cuts: {cuts:?}");
    }

    #[test]
    fn test_detect_shot_changes_gradual_fade_no_false_positive() {
        // A smooth 60-step ramp (delta 3/frame, comfortably below the
        // per-step score that would clear ABSOLUTE_FLOOR — see
        // test_frame_diff_score_separates_cut_from_fade) must produce zero
        // detected cuts.
        let bases: Vec<i32> = (0..=60).map(|i| 20 + i * 3).collect();
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("valid Y4M fade should analyze successfully");

        assert!(
            cuts.is_empty(),
            "gradual fade must not produce false-positive cuts: {cuts:?}"
        );
    }

    #[test]
    fn test_detect_shot_changes_constant_video_no_cuts() {
        let bases = vec![128; 20];
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("constant video should analyze successfully");
        assert!(cuts.is_empty());
    }

    #[test]
    fn test_detect_shot_changes_short_stream_is_ok_zero_cuts() {
        // Zero or one frame: nothing to diff, which is a real (not
        // fabricated) empty result.
        let empty = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &[]);
        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        assert!(detector
            .detect_shot_changes(&empty)
            .expect("header-only stream should analyze successfully")
            .is_empty());

        let one_frame = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &[42]);
        assert!(detector
            .detect_shot_changes(&one_frame)
            .expect("single-frame stream should analyze successfully")
            .is_empty());
    }

    #[test]
    fn test_detect_shot_changes_malformed_input_is_honest_err() {
        // CHANGED: detect_shot_changes() previously ignored the input bytes
        // entirely and always returned Err regardless of content (real
        // scene-cut detection was not implemented at all). It now performs
        // real Y4M decode + frame-diff analysis, so a non-Y4M buffer must
        // still fail — but for the honest reason that it cannot be demuxed,
        // not because the function is a permanent stub.
        let detector = ShotChangeDetector::new(25.0);
        let not_y4m = vec![0u8; 128];

        let result = detector.detect_shot_changes(&not_y4m);

        assert!(
            result.is_err(),
            "non-Y4M input must not silently analyze as zero cuts"
        );
        assert!(matches!(result.unwrap_err(), CaptionError::Other(_)));
    }

    #[test]
    fn test_detect_shot_changes_truncated_frame_is_honest_err() {
        let mut y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &[20]);
        // Corrupt: append a FRAME tag promising a second frame, then supply
        // far fewer bytes than its declared luma plane (16*16=256).
        y4m.extend_from_slice(b"FRAME\n");
        y4m.extend_from_slice(&[10u8; 5]);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let result = detector.detect_shot_changes(&y4m);
        assert!(
            result.is_err(),
            "truncated frame data must be an honest error"
        );
        assert!(matches!(result.unwrap_err(), CaptionError::Other(_)));
    }

    #[test]
    fn test_detect_shot_changes_wired_into_snap_to_shots() {
        // End-to-end: a real detected cut feeds directly into snap_to_shots
        // (previously the only way to exercise snap_to_shots was to hand it
        // a fabricated shot_changes list, since detect_shot_changes always
        // errored).
        let mut bases = vec![20; 15];
        bases.extend(std::iter::repeat_n(200, 15));
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("valid Y4M with a real cut should analyze successfully");
        assert_eq!(cuts.len(), 1);
        let detected_cut = cuts[0];

        // Frame 15 at 25fps = 600ms. Place a caption starting slightly
        // (2 frames = 80ms) before the real cut, well within a 3-frame
        // tolerance, and verify it snaps to the REAL detected timestamp.
        let mut track = CaptionTrack::new(Language::english());
        track
            .add_caption(Caption::new(
                Timestamp::from_millis(520),
                Timestamp::from_millis(1200),
                "Test".to_string(),
            ))
            .expect("add_caption should succeed in test");

        let snapped = detector
            .snap_to_shots(&mut track, &cuts, 3)
            .expect("snap_to_shots should succeed");

        assert!(snapped > 0, "expected the caption start to snap");
        assert_eq!(track.captions[0].start, detected_cut);
    }

    #[test]
    fn test_detect_shot_changes_adaptive_term_suppresses_repeated_jitter() {
        // Isolates the adaptive half of the threshold (`local_mean *
        // ADAPTIVE_MULTIPLIER`) from both the absolute floor and the
        // min-shot-duration debounce:
        //
        // - A 3-frame constant prefix, then 30 frames alternating between
        //   base 100 and 112. Every single alternation step scores ~0.28
        //   (see the jitter-vs-ADAPTIVE_MULTIPLIER relationship this
        //   value was chosen to demonstrate: individually it clears
        //   ABSOLUTE_FLOOR (0.20) with room to spare, so a threshold that
        //   used the floor alone would flag *every one* of the 30
        //   alternations as a separate cut.
        // - `min_shot_duration` is dropped to 1 frame, so the debounce
        //   cannot be the thing suppressing repeated flags either.
        //
        // What is left to explain a low cut count is the adaptive term:
        // once the trailing window fills with same-magnitude jitter
        // scores, `local_mean * 3.0` rises to roughly 3x each individual
        // step's own score, so no further step in a *stationary* noisy
        // run can clear its own self-raised bar. Only the very first
        // jitter transition — before any jitter has entered the trailing
        // window — is judged against the still-low floor and flagged;
        // every later alternation, despite individually clearing
        // ABSOLUTE_FLOOR, does not clear the adaptive bar its own recent
        // history sets.
        let mut bases = vec![100; 3];
        for i in 0..30 {
            bases.push(if i % 2 == 0 { 112 } else { 100 });
        }
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let mut detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        detector.set_min_shot_duration(1);
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("valid jittery Y4M should analyze successfully");

        assert_eq!(
            cuts.len(),
            1,
            "expected only the unavoidable cold-start transition to be flagged, not one \
             per jitter alternation (which the floor alone would produce): {cuts:?}"
        );
    }

    #[test]
    fn test_detect_shot_changes_debounce_suppresses_close_second_cut() {
        // Two unambiguous hard cuts only 5 frames apart (well under the
        // default 12-frame min_shot_duration): A(15,base=20) ->
        // B(5,base=200) -> C(15,base=20). Both transitions individually
        // score far above ABSOLUTE_FLOOR (same magnitude as the
        // hard-cut fixture elsewhere in this module), so debounce — not
        // score — must be what suppresses the second one.
        let mut bases = vec![20; 15];
        bases.extend(std::iter::repeat_n(200, 5));
        bases.extend(std::iter::repeat_n(20, 15));
        let y4m = build_mono_y4m(FIXTURE_W, FIXTURE_H, FIXTURE_FPS, &bases);

        let detector = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        let cuts = detector
            .detect_shot_changes(&y4m)
            .expect("valid Y4M should analyze successfully");
        assert_eq!(
            cuts.len(),
            1,
            "the second cut (5 frames after the first) is inside the default 12-frame \
             min_shot_duration and must be debounced: {cuts:?}"
        );

        // Lowering min_shot_duration below the 5-frame gap must let both
        // through, proving the debounce parameter itself is load-bearing
        // (not merely that the code happens to always suppress).
        let mut relaxed = ShotChangeDetector::new(f64::from(FIXTURE_FPS));
        relaxed.set_min_shot_duration(3);
        let cuts_relaxed = relaxed
            .detect_shot_changes(&y4m)
            .expect("valid Y4M should analyze successfully");
        assert_eq!(
            cuts_relaxed.len(),
            2,
            "with a relaxed min_shot_duration both real cuts must be detected: {cuts_relaxed:?}"
        );
    }

    #[test]
    fn test_detector_creation() {
        let detector = ShotChangeDetector::new(25.0);
        assert_eq!(detector.fps, 25.0);
        assert_eq!(detector.min_shot_duration, 12);
    }

    #[test]
    fn test_snap_to_shots() {
        let mut track = CaptionTrack::new(Language::english());
        track
            .add_caption(Caption::new(
                Timestamp::from_millis(1000),
                Timestamp::from_millis(3000),
                "Test".to_string(),
            ))
            .expect("operation should succeed in test");

        let shot_changes = vec![Timestamp::from_millis(990), Timestamp::from_millis(2995)];

        let detector = ShotChangeDetector::new(25.0);
        let snapped = detector
            .snap_to_shots(&mut track, &shot_changes, 5)
            .expect("operation should succeed in test");

        assert!(snapped > 0);
        // Caption should be snapped to shot changes
        assert_eq!(track.captions[0].start, Timestamp::from_millis(990));
    }

    #[test]
    fn test_shot_metrics() {
        let mut track = CaptionTrack::new(Language::english());
        track
            .add_caption(Caption::new(
                Timestamp::from_secs(0),
                Timestamp::from_secs(5),
                "Test".to_string(),
            ))
            .expect("operation should succeed in test");

        let shot_changes = vec![Timestamp::from_secs(2), Timestamp::from_secs(4)];

        let detector = ShotChangeDetector::new(25.0);
        let metrics = detector.calculate_shot_metrics(&track, &shot_changes);

        assert!(metrics.captions_spanning_shots > 0);
        assert!(metrics.max_shots_per_caption >= 1);
    }
}

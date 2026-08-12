//! Format negotiation: turning "what I want" into "what this device can do".
//!
//! [`negotiate`] is a pure, total function of `(available, config)`. It never
//! touches hardware, never allocates a device handle and never fails for any
//! reason other than "nothing in the list is acceptable". That is deliberate:
//! picking a capture format is the single most surprising part of any capture
//! API, so it is worth being able to test every branch of it exhaustively
//! without a camera on the desk.
//!
//! # Ranking
//!
//! Candidates are compared lexicographically. The first criterion that
//! separates two formats decides.
//!
//! 0. **Compression filter.** With [`CaptureConfig::allow_compressed`] set to
//!    `false`, compressed formats are removed *before* anything is scored, so
//!    they can never win by default and can never be resurrected by a later
//!    criterion.
//! 1. **Encoding.** Position in [`CaptureConfig::encoding_preference`] (which
//!    falls back to [`crate::DEFAULT_ENCODING_PREFERENCE`] when the caller
//!    supplied none). Encodings absent from the list are still eligible, but
//!    all rank equally *after* every listed one.
//! 2. **Size.** An exact match on every requested dimension beats everything.
//!    Otherwise the smallest frame that is at-or-above the request wins, so
//!    the caller downscales rather than upscales — inventing pixels that the
//!    sensor never sampled is the one resize direction that cannot be undone.
//!    Only if nothing reaches the request does the largest frame below it win.
//!    When neither dimension is requested, the largest area wins.
//! 3. **Frame rate.** Exact, then the nearest rate at-or-above the request,
//!    then the nearest below. When no rate is requested, the highest wins.
//! 4. **Tie-break.** Descending width, descending height, descending frame
//!    rate (compared as an exact rational, never as rounded floats), and
//!    finally ascending position in `available`.
//!
//! The enumeration-index tie-break is what makes the result reproducible.
//! Because it comes last, reordering the input can only change the outcome
//! between candidates that are indistinguishable under *every* earlier
//! criterion.

use std::cmp::Ordering;

use crate::config::CaptureConfig;
use crate::device::{CaptureEncoding, CaptureFormat};
use crate::error::CaptureError;

/// Tolerance for treating a requested frame rate as an exact match.
///
/// Frame rates are compared against a caller-supplied `f64`, so an exact
/// equality test would make `59.94` fail to match `60000/1001`. One
/// micro-frame-per-second is far below any rate a device actually reports and
/// far above the rounding error of the division.
const FPS_EPSILON: f64 = 1e-6;

/// Pick the best available format for a request.
///
/// See the [module documentation](self) for the exact ranking.
///
/// # Errors
///
/// Returns [`CaptureError::NoMatchingFormat`] when `available` is empty, or
/// when every entry was removed by the compression filter. The error's
/// `available` field carries the length of the *unfiltered* input — "the
/// device advertised N formats, none matched" — and its `device` field is left
/// empty for the caller to fill in, since this function does not know which
/// device the list came from.
///
/// # Examples
///
/// ```
/// use oximedia_capture::{negotiate, CaptureConfig, CaptureEncoding, CaptureFormat};
/// use oximedia_core::PixelFormat;
///
/// let available = [
///     CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 30, 1),
///     CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 1280, 720, 30, 1),
/// ];
/// let config = CaptureConfig::default().with_size(1280, 720);
/// let chosen = negotiate(&available, &config)?;
/// assert_eq!(chosen.width, 1280);
/// # Ok::<(), oximedia_capture::CaptureError>(())
/// ```
pub fn negotiate(
    available: &[CaptureFormat],
    config: &CaptureConfig,
) -> Result<CaptureFormat, CaptureError> {
    let preference = config.encoding_preference();
    let mut best: Option<(usize, CaptureFormat)> = None;

    for (index, &candidate) in available.iter().enumerate() {
        if !config.allow_compressed && candidate.encoding.is_compressed() {
            continue;
        }
        let replace = match best {
            None => true,
            Some((best_index, best_format)) => {
                compare(
                    (index, candidate),
                    (best_index, best_format),
                    config,
                    preference,
                ) == Ordering::Less
            }
        };
        if replace {
            best = Some((index, candidate));
        }
    }

    best.map(|(_, format)| format)
        .ok_or_else(|| CaptureError::NoMatchingFormat {
            device: String::new(),
            available: available.len(),
        })
}

/// Order two candidates. [`Ordering::Less`] means `lhs` is the better pick.
fn compare(
    lhs: (usize, CaptureFormat),
    rhs: (usize, CaptureFormat),
    config: &CaptureConfig,
    preference: &[CaptureEncoding],
) -> Ordering {
    let (lhs_index, lhs_format) = lhs;
    let (rhs_index, rhs_format) = rhs;

    encoding_rank(lhs_format.encoding, preference)
        .cmp(&encoding_rank(rhs_format.encoding, preference))
        .then_with(|| size_order(lhs_format, rhs_format, config))
        .then_with(|| fps_order(lhs_format, rhs_format, config))
        // Tie-breaks: bigger and faster first, then input order for stability.
        .then_with(|| rhs_format.width.cmp(&lhs_format.width))
        .then_with(|| rhs_format.height.cmp(&lhs_format.height))
        .then_with(|| rhs_format.cmp_fps(lhs_format))
        .then_with(|| lhs_index.cmp(&rhs_index))
}

/// Position of `encoding` in the preference list; unlisted encodings all share
/// the rank just past the end of the list.
fn encoding_rank(encoding: CaptureEncoding, preference: &[CaptureEncoding]) -> usize {
    preference
        .iter()
        .position(|&listed| listed == encoding)
        .unwrap_or(preference.len())
}

/// Size ranking. [`Ordering::Less`] means `lhs` is the better size.
fn size_order(lhs: CaptureFormat, rhs: CaptureFormat, config: &CaptureConfig) -> Ordering {
    if config.width.is_none() && config.height.is_none() {
        // No size requested: largest area wins.
        return rhs.area().cmp(&lhs.area());
    }
    let (lhs_class, lhs_metric) = size_key(lhs, config);
    let (rhs_class, rhs_metric) = size_key(rhs, config);
    lhs_class
        .cmp(&rhs_class)
        .then_with(|| lhs_metric.cmp(&rhs_metric))
}

/// `(class, metric)` for a candidate size, both ascending-is-better.
///
/// * class 0 — exact on every requested dimension.
/// * class 1 — at-or-above on every requested dimension; metric is the area,
///   so the *smallest* qualifying frame wins (least downscaling).
/// * class 2 — below the request on at least one dimension; metric is the
///   area complement, so the *largest* frame wins (least upscaling).
fn size_key(format: CaptureFormat, config: &CaptureConfig) -> (u8, u64) {
    let exact = config.width.is_none_or(|w| format.width == w)
        && config.height.is_none_or(|h| format.height == h);
    if exact {
        return (0, 0);
    }
    let at_or_above = config.width.is_none_or(|w| format.width >= w)
        && config.height.is_none_or(|h| format.height >= h);
    if at_or_above {
        (1, format.area())
    } else {
        (2, u64::MAX - format.area())
    }
}

/// Frame-rate ranking. [`Ordering::Less`] means `lhs` is the better rate.
fn fps_order(lhs: CaptureFormat, rhs: CaptureFormat, config: &CaptureConfig) -> Ordering {
    let Some(requested) = config.fps else {
        // No rate requested: highest wins, compared as an exact rational.
        return rhs.cmp_fps(lhs);
    };
    let (lhs_class, lhs_metric) = fps_key(lhs, requested);
    let (rhs_class, rhs_metric) = fps_key(rhs, requested);
    lhs_class
        .cmp(&rhs_class)
        .then_with(|| lhs_metric.total_cmp(&rhs_metric))
}

/// `(class, distance)` for a candidate frame rate, both ascending-is-better.
///
/// * class 0 — within [`FPS_EPSILON`] of the request.
/// * class 1 — above the request; distance is how far above.
/// * class 2 — below the request; distance is how far below.
fn fps_key(format: CaptureFormat, requested: f64) -> (u8, f64) {
    let delta = format.fps() - requested;
    if delta.abs() <= FPS_EPSILON {
        (0, 0.0)
    } else if delta > 0.0 {
        (1, delta)
    } else {
        (2, -delta)
    }
}

#[cfg(test)]
mod tests {
    use oximedia_core::PixelFormat;
    use proptest::prelude::*;

    use super::*;
    use crate::device::CaptureEncoding;

    const NV12: CaptureEncoding = CaptureEncoding::Raw(PixelFormat::Nv12);
    const YUYV: CaptureEncoding = CaptureEncoding::Raw(PixelFormat::Yuyv422);
    const RGB24: CaptureEncoding = CaptureEncoding::Raw(PixelFormat::Rgb24);
    const GRAY8: CaptureEncoding = CaptureEncoding::Raw(PixelFormat::Gray8);
    const MJPEG: CaptureEncoding = CaptureEncoding::Mjpeg;

    fn fmt(encoding: CaptureEncoding, width: u32, height: u32, fps: u32) -> CaptureFormat {
        CaptureFormat::new(encoding, width, height, fps, 1)
    }

    // ── Size ────────────────────────────────────────────────────────────────

    #[test]
    fn exact_size_match_beats_everything_else() {
        let available = [
            fmt(NV12, 1920, 1080, 30),
            fmt(NV12, 1280, 720, 30),
            fmt(NV12, 640, 480, 30),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!((chosen.width, chosen.height), (1280, 720));
    }

    #[test]
    fn upscale_only_device_picks_smallest_frame_at_or_above() {
        // Nothing is exactly 800x600; 1280x720 is the smallest that covers it.
        let available = [
            fmt(NV12, 1920, 1080, 30),
            fmt(NV12, 1280, 720, 30),
            fmt(NV12, 3840, 2160, 30),
        ];
        let cfg = CaptureConfig::default().with_size(800, 600);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!((chosen.width, chosen.height), (1280, 720));
    }

    #[test]
    fn downscale_only_device_picks_largest_frame_below() {
        let available = [
            fmt(NV12, 320, 240, 30),
            fmt(NV12, 640, 480, 30),
            fmt(NV12, 160, 120, 30),
        ];
        let cfg = CaptureConfig::default().with_size(1920, 1080);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!((chosen.width, chosen.height), (640, 480));
    }

    #[test]
    fn no_size_request_picks_largest_area() {
        let available = [
            fmt(NV12, 640, 480, 30),
            fmt(NV12, 3840, 2160, 30),
            fmt(NV12, 1920, 1080, 30),
        ];
        let chosen =
            negotiate(&available, &CaptureConfig::default()).expect("a format should match");
        assert_eq!((chosen.width, chosen.height), (3840, 2160));
    }

    #[test]
    fn a_single_requested_dimension_still_constrains() {
        let available = [
            fmt(NV12, 640, 480, 30),
            fmt(NV12, 1280, 720, 30),
            fmt(NV12, 1280, 960, 30),
        ];
        let mut cfg = CaptureConfig::default();
        cfg.width = Some(1280);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.width, 1280);
        // Both 1280-wide entries are exact on the only requested dimension, so
        // the descending-height tie-break decides.
        assert_eq!(chosen.height, 960);
    }

    // ── Frame rate ──────────────────────────────────────────────────────────

    #[test]
    fn exact_fps_match_wins() {
        let available = [
            fmt(NV12, 1280, 720, 15),
            fmt(NV12, 1280, 720, 30),
            fmt(NV12, 1280, 720, 60),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720).with_fps(30.0);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.fps_num, 30);
    }

    #[test]
    fn fps_falls_up_to_the_nearest_at_or_above() {
        let available = [
            fmt(NV12, 1280, 720, 15),
            fmt(NV12, 1280, 720, 50),
            fmt(NV12, 1280, 720, 60),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720).with_fps(30.0);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.fps_num, 50);
    }

    #[test]
    fn fps_falls_down_only_when_nothing_reaches_the_request() {
        let available = [
            fmt(NV12, 1280, 720, 5),
            fmt(NV12, 1280, 720, 24),
            fmt(NV12, 1280, 720, 10),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720).with_fps(60.0);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.fps_num, 24);
    }

    #[test]
    fn no_fps_request_picks_the_highest_rate() {
        let available = [
            fmt(NV12, 1280, 720, 30),
            fmt(NV12, 1280, 720, 120),
            fmt(NV12, 1280, 720, 60),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.fps_num, 120);
    }

    #[test]
    fn ntsc_rational_matches_its_decimal_request() {
        let available = [
            CaptureFormat::new(NV12, 1920, 1080, 30, 1),
            CaptureFormat::new(NV12, 1920, 1080, 30000, 1001),
        ];
        let cfg = CaptureConfig::default()
            .with_size(1920, 1080)
            .with_fps(29.970_03);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!((chosen.fps_num, chosen.fps_den), (30000, 1001));
    }

    // ── Encoding ────────────────────────────────────────────────────────────

    #[test]
    fn default_preference_ranks_nv12_over_mjpeg() {
        let available = [
            fmt(MJPEG, 1280, 720, 30),
            fmt(RGB24, 1280, 720, 30),
            fmt(NV12, 1280, 720, 30),
            fmt(YUYV, 1280, 720, 30),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.encoding, NV12);
    }

    #[test]
    fn explicit_preference_overrides_the_default_order() {
        let available = [fmt(NV12, 1280, 720, 30), fmt(MJPEG, 1280, 720, 30)];
        let cfg = CaptureConfig::default()
            .with_size(1280, 720)
            .with_preferred([MJPEG]);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.encoding, MJPEG);
    }

    #[test]
    fn unlisted_encodings_rank_after_every_listed_one() {
        // GRAY8 is absent from both the request and the default preference.
        let available = [fmt(GRAY8, 1920, 1080, 60), fmt(NV12, 320, 240, 5)];
        let cfg = CaptureConfig::default().with_preferred([NV12]);
        let chosen = negotiate(&available, &cfg).expect("a format should match");
        assert_eq!(chosen.encoding, NV12);
    }

    #[test]
    fn unlisted_encodings_are_still_eligible_when_nothing_is_listed() {
        let available = [fmt(GRAY8, 1920, 1080, 60)];
        let cfg = CaptureConfig::default().with_preferred([NV12]);
        let chosen = negotiate(&available, &cfg).expect("Gray8 is unlisted, not forbidden");
        assert_eq!(chosen.encoding, GRAY8);
    }

    // ── Compression filter ──────────────────────────────────────────────────

    #[test]
    fn mjpeg_only_device_with_compression_disallowed_is_an_error() {
        let available = [fmt(MJPEG, 1920, 1080, 30), fmt(MJPEG, 1280, 720, 60)];
        let cfg = CaptureConfig::default().with_allow_compressed(false);
        let err = negotiate(&available, &cfg).expect_err("compressed formats were filtered out");
        match err {
            CaptureError::NoMatchingFormat { device, available } => {
                assert!(device.is_empty(), "caller fills the device in");
                assert_eq!(available, 2, "reports the unfiltered advertised count");
            }
            other => panic!("expected NoMatchingFormat, got {other:?}"),
        }
    }

    #[test]
    fn compression_filter_runs_before_scoring() {
        // MJPEG is the only 4K mode, but it is filtered out before ranking, so
        // the raw 720p mode wins rather than MJPEG winning on size.
        let available = [fmt(MJPEG, 3840, 2160, 60), fmt(NV12, 1280, 720, 30)];
        let cfg = CaptureConfig::default().with_allow_compressed(false);
        let chosen = negotiate(&available, &cfg).expect("a raw format remains");
        assert_eq!(chosen.encoding, NV12);
    }

    #[test]
    fn empty_input_is_an_error() {
        let err = negotiate(&[], &CaptureConfig::default()).expect_err("nothing to choose from");
        match err {
            CaptureError::NoMatchingFormat { available, .. } => assert_eq!(available, 0),
            other => panic!("expected NoMatchingFormat, got {other:?}"),
        }
    }

    // ── Determinism ─────────────────────────────────────────────────────────

    #[test]
    fn result_is_independent_of_enumeration_order() {
        let base = [
            fmt(MJPEG, 3840, 2160, 30),
            fmt(NV12, 1280, 720, 60),
            fmt(YUYV, 1280, 720, 30),
            fmt(NV12, 640, 480, 30),
            fmt(RGB24, 1920, 1080, 30),
        ];
        let cfg = CaptureConfig::default().with_size(1280, 720).with_fps(60.0);
        let expected = negotiate(&base, &cfg).expect("a format should match");

        // Every rotation is a distinct enumeration order.
        for shift in 0..base.len() {
            let mut shuffled = base.to_vec();
            shuffled.rotate_left(shift);
            let chosen = negotiate(&shuffled, &cfg).expect("a format should match");
            assert_eq!(chosen, expected, "rotation by {shift} changed the winner");
        }

        // And so is full reversal.
        let mut reversed = base.to_vec();
        reversed.reverse();
        assert_eq!(
            negotiate(&reversed, &cfg).expect("a format should match"),
            expected
        );
    }

    #[test]
    fn repeated_calls_are_identical() {
        let available = [
            fmt(NV12, 1280, 720, 30),
            fmt(YUYV, 1280, 720, 30),
            fmt(MJPEG, 1920, 1080, 60),
        ];
        let cfg = CaptureConfig::default();
        let first = negotiate(&available, &cfg).expect("a format should match");
        for _ in 0..16 {
            assert_eq!(negotiate(&available, &cfg).expect("stable"), first);
        }
    }

    #[test]
    fn identical_duplicates_resolve_to_the_first_occurrence() {
        let available = [fmt(NV12, 1280, 720, 30), fmt(NV12, 1280, 720, 30)];
        let cfg = CaptureConfig::default();
        assert_eq!(
            negotiate(&available, &cfg).expect("a format should match"),
            available[0]
        );
    }

    // ── Property ────────────────────────────────────────────────────────────

    fn any_encoding() -> impl Strategy<Value = CaptureEncoding> {
        prop_oneof![
            Just(MJPEG),
            Just(NV12),
            Just(YUYV),
            Just(RGB24),
            Just(GRAY8),
            Just(CaptureEncoding::Raw(PixelFormat::Uyvy422)),
            Just(CaptureEncoding::Raw(PixelFormat::Yuv420p)),
        ]
    }

    fn any_format() -> impl Strategy<Value = CaptureFormat> {
        (
            any_encoding(),
            1u32..8000,
            1u32..5000,
            0u32..241,
            0u32..1002,
        )
            .prop_map(|(encoding, width, height, fps_num, fps_den)| {
                CaptureFormat::new(encoding, width, height, fps_num, fps_den)
            })
    }

    proptest! {
        /// The negotiated format is always one of the offered formats — the
        /// function selects, it never synthesizes a mode the device did not
        /// advertise.
        #[test]
        fn negotiate_always_returns_an_offered_format(
            available in prop::collection::vec(any_format(), 0..24),
            width in prop::option::of(1u32..8000),
            height in prop::option::of(1u32..5000),
            fps in prop::option::of(0.0f64..240.0),
            allow_compressed in any::<bool>(),
        ) {
            let mut cfg = CaptureConfig::default();
            cfg.width = width;
            cfg.height = height;
            cfg.fps = fps;
            cfg.allow_compressed = allow_compressed;

            match negotiate(&available, &cfg) {
                Ok(chosen) => {
                    prop_assert!(
                        available.contains(&chosen),
                        "negotiate returned a format that was not offered: {chosen:?}"
                    );
                    prop_assert!(
                        allow_compressed || !chosen.encoding.is_compressed(),
                        "compressed format selected despite allow_compressed = false"
                    );
                }
                Err(CaptureError::NoMatchingFormat { available: count, .. }) => {
                    prop_assert_eq!(count, available.len());
                    // Erring is only legitimate when nothing survived the filter.
                    prop_assert!(
                        available
                            .iter()
                            .all(|f| !allow_compressed && f.encoding.is_compressed()),
                        "negotiate rejected a viable format"
                    );
                }
                Err(other) => prop_assert!(false, "unexpected error variant: {:?}", other),
            }
        }
    }
}

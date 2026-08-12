// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Client-side glass-to-glass latency instrumentation for HLS playback.
//!
//! [`super::server`]'s segment handler measures real *server-side* serve
//! latency (time to locate and build the segment response). Glass-to-glass
//! (end-to-end) latency additionally needs the *client's* side: how long
//! after a segment's content was captured (its
//! `#EXT-X-PROGRAM-DATE-TIME`, "PDT") does the player actually have it in
//! hand?
//!
//! This module provides that half:
//!
//! - [`extract_program_date_times`] scans raw HLS media-playlist text for
//!   `#EXT-X-PROGRAM-DATE-TIME` tags and pairs each with the segment URI
//!   line that immediately follows it — pure, no I/O, directly testable
//!   against a synthetic playlist string.
//! - [`latency_from_pdt`] is the core measurement: wall-clock time observed
//!   minus a segment's PDT, in milliseconds.
//! - [`LatencyEstimator`] is a small rolling-window aggregator (count/min/
//!   max/average) fed by real samples, mirroring the shape of
//!   [`crate::live::analytics::Analytics`]'s existing latency API
//!   (`record_latency`, `summary`) on the client side.
//! - [`LatencyEstimator::record`] also forwards non-negative samples into
//!   an existing [`Analytics`] instance's `record_latency`, when one is
//!   supplied, so client- and server-side latency end up in the same
//!   metrics sink rather than two disconnected places.
//!
//! # Honest limitations
//!
//! **This estimator is not wired end-to-end in this codebase today.**
//! `crate::hls::playlist::MediaPlaylist::parse` (the crate's HLS playlist
//! parser, outside this module's scope) does not populate
//! `crate::hls::Segment::program_date_time` from `#EXT-X-PROGRAM-DATE-TIME`
//! tags, and [`super::server::HlsServer`] does not emit that tag when
//! building playlists (`MediaPlaylist::to_m3u8` never writes it). This is
//! verified, not assumed: neither function references the field or the tag
//! anywhere. [`extract_program_date_times`] works directly against *raw
//! playlist text*, independent of that gap, so it is real and usable the
//! moment a playlist source that does emit PDT is available — but nothing
//! in this crate currently produces one from a live stream. Real network
//! fetching (matching timestamps to actual HTTP requests against
//! [`super::server::HlsServer`]) is deliberately not implemented here
//! either: given the gap above it would silently measure nothing, which is
//! worse than not claiming it works.
//!
//! # Precision caveat
//!
//! "Glass-to-glass" ideally means capture-to-decoded-pixels-on-screen. This
//! only measures capture (PDT) to *fetch-complete* (or an optional
//! caller-supplied decode-complete timestamp) wall clock — it does not
//! model decode or render/compositor latency, and it is only as accurate
//! as the PDT value the origin actually published and the two systems'
//! clock synchronization. No fabricated precision is claimed beyond that.

use crate::error::{NetError, NetResult};
use crate::live::analytics::Analytics;
use chrono::{DateTime, Utc};
use std::collections::VecDeque;

/// Number of recent samples [`LatencyEstimator`] retains for min/max/average
/// by default.
const DEFAULT_WINDOW: usize = 200;

/// One real glass-to-glass latency measurement.
#[derive(Debug, Clone, Copy)]
pub struct LatencySample {
    /// The media sequence number this measurement is for, if known.
    pub sequence: Option<u64>,
    /// The segment's advertised `#EXT-X-PROGRAM-DATE-TIME`.
    pub pdt: DateTime<Utc>,
    /// Wall-clock time the segment was observed as ready (fetch complete,
    /// or decode complete if the caller has a real decoder loop).
    pub observed_at: DateTime<Utc>,
    /// `observed_at - pdt` in milliseconds. Negative when `observed_at`
    /// precedes `pdt` — clock skew between origin and client, or a PDT
    /// that does not actually reflect capture time — and is reported
    /// as-is rather than clamped, so the anomaly stays visible.
    pub latency_ms: i64,
}

/// Computes `observed_at - pdt` in milliseconds.
///
/// A negative result means `observed_at` is *before* `pdt`: the client and
/// origin clocks are not synchronized, or the PDT does not reflect a real
/// capture time. This function does not clamp or otherwise hide that — see
/// [`LatencyEstimator::record`] for how the aggregator handles it.
#[must_use]
pub fn latency_from_pdt(pdt: DateTime<Utc>, observed_at: DateTime<Utc>) -> i64 {
    (observed_at - pdt).num_milliseconds()
}

/// Scans raw HLS media-playlist text for `#EXT-X-PROGRAM-DATE-TIME` tags,
/// pairing each with the segment URI line that follows it.
///
/// Per the HLS spec, a `PROGRAM-DATE-TIME` tag technically applies to the
/// segment immediately following it and, unless overridden, to subsequent
/// segments until the next tag or a discontinuity. This scanner takes the
/// simpler, common-in-practice interpretation that each media segment URI
/// is preceded by its own tag, and only pairs a PDT with the *next* URI
/// line — a PDT with no following URI line (e.g. a trailing tag on an
/// otherwise-empty playlist) is dropped rather than guessed forward onto
/// segments it may not apply to.
///
/// # Errors
///
/// Returns [`NetError::Playlist`] if a `#EXT-X-PROGRAM-DATE-TIME` tag's
/// value is not a parseable RFC 3339 timestamp.
pub fn extract_program_date_times(playlist_text: &str) -> NetResult<Vec<(String, DateTime<Utc>)>> {
    let mut pairs = Vec::new();
    let mut pending_pdt: Option<DateTime<Utc>> = None;

    for raw_line in playlist_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(value) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            let pdt = DateTime::parse_from_rfc3339(value.trim())
                .map_err(|e| {
                    NetError::playlist(format!(
                        "invalid #EXT-X-PROGRAM-DATE-TIME value '{value}': {e}"
                    ))
                })?
                .with_timezone(&Utc);
            pending_pdt = Some(pdt);
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        // A non-comment, non-empty line is a segment URI.
        if let Some(pdt) = pending_pdt.take() {
            pairs.push((line.to_string(), pdt));
        }
    }

    Ok(pairs)
}

/// A rolling-window aggregator of real glass-to-glass latency samples.
///
/// Mirrors the shape of [`Analytics`]'s existing `record_latency`/summary
/// API (count, average) plus min/max, kept client-side so it can be used
/// without a full `LiveStream`/`Analytics` fixture, and optionally forwards
/// samples into an [`Analytics`] instance so client- and server-side
/// latency end up in the same place.
pub struct LatencyEstimator {
    samples: VecDeque<LatencySample>,
    window: usize,
}

impl LatencyEstimator {
    /// Creates an estimator retaining the `DEFAULT_WINDOW` most recent
    /// samples.
    #[must_use]
    pub fn new() -> Self {
        Self::with_window(DEFAULT_WINDOW)
    }

    /// Creates an estimator retaining at most `window` recent samples
    /// (clamped to at least 1).
    #[must_use]
    pub fn with_window(window: usize) -> Self {
        let window = window.max(1);
        Self {
            samples: VecDeque::with_capacity(window.min(1024)),
            window,
        }
    }

    /// Records a real measurement: `pdt` from the segment's advertised
    /// `#EXT-X-PROGRAM-DATE-TIME`, `observed_at` the wall-clock time it was
    /// observed ready (fetch or decode complete).
    ///
    /// When `analytics` is supplied and the computed latency is
    /// non-negative, it is also forwarded to
    /// [`Analytics::record_latency`]. A negative latency (clock skew, or a
    /// PDT that doesn't reflect real capture time) is kept in this
    /// estimator's own samples honestly, but is *not* forwarded — clamping
    /// it to `0` would misrepresent a measurement anomaly as "zero latency
    /// achieved", which is not what was measured.
    pub fn record(
        &mut self,
        sequence: Option<u64>,
        pdt: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        analytics: Option<&Analytics>,
    ) -> LatencySample {
        let latency_ms = latency_from_pdt(pdt, observed_at);
        let sample = LatencySample {
            sequence,
            pdt,
            observed_at,
            latency_ms,
        };

        if self.samples.len() >= self.window {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);

        if let Some(analytics) = analytics {
            if let Ok(non_negative_ms) = u64::try_from(latency_ms) {
                analytics.record_latency(non_negative_ms);
            }
        }

        sample
    }

    /// Number of samples currently retained.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Average latency across retained samples, in milliseconds. `None`
    /// when no samples have been recorded.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn average_latency_ms(&self) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: i64 = self.samples.iter().map(|s| s.latency_ms).sum();
        Some(sum as f64 / self.samples.len() as f64)
    }

    /// Minimum recorded latency, in milliseconds. `None` when empty.
    #[must_use]
    pub fn min_latency_ms(&self) -> Option<i64> {
        self.samples.iter().map(|s| s.latency_ms).min()
    }

    /// Maximum recorded latency, in milliseconds. `None` when empty.
    #[must_use]
    pub fn max_latency_ms(&self) -> Option<i64> {
        self.samples.iter().map(|s| s.latency_ms).max()
    }

    /// The most recently recorded sample, if any.
    #[must_use]
    pub fn latest(&self) -> Option<&LatencySample> {
        self.samples.back()
    }

    /// Clears all retained samples.
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

impl Default for LatencyEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    #[test]
    fn latency_from_pdt_positive_when_observed_after_pdt() {
        let pdt = Utc::now();
        let observed = pdt + ChronoDuration::milliseconds(1500);
        assert_eq!(latency_from_pdt(pdt, observed), 1500);
    }

    #[test]
    fn latency_from_pdt_negative_when_observed_before_pdt() {
        let pdt = Utc::now();
        let observed = pdt - ChronoDuration::milliseconds(200);
        assert_eq!(latency_from_pdt(pdt, observed), -200);
    }

    #[test]
    fn latency_from_pdt_zero_for_identical_timestamps() {
        let t = Utc::now();
        assert_eq!(latency_from_pdt(t, t), 0);
    }

    #[test]
    fn extract_program_date_times_pairs_tags_with_following_uri() -> NetResult<()> {
        let playlist = "#EXTM3U\n\
             #EXT-X-VERSION:3\n\
             #EXT-X-TARGETDURATION:6\n\
             #EXT-X-MEDIA-SEQUENCE:100\n\
             #EXT-X-PROGRAM-DATE-TIME:2026-08-12T10:00:00.000Z\n\
             #EXTINF:6.000,\n\
             seg_100.m4s\n\
             #EXT-X-PROGRAM-DATE-TIME:2026-08-12T10:00:06.000Z\n\
             #EXTINF:6.000,\n\
             seg_101.m4s\n";

        let pairs = extract_program_date_times(playlist)?;
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "seg_100.m4s");
        assert_eq!(pairs[1].0, "seg_101.m4s");
        assert!(pairs[1].1 > pairs[0].1);
        assert_eq!((pairs[1].1 - pairs[0].1).num_seconds(), 6);
        Ok(())
    }

    #[test]
    fn extract_program_date_times_no_tags_yields_empty() -> NetResult<()> {
        let playlist = "#EXTM3U\n#EXTINF:6.000,\nseg_0.m4s\n";
        let pairs = extract_program_date_times(playlist)?;
        assert!(pairs.is_empty());
        Ok(())
    }

    #[test]
    fn extract_program_date_times_trailing_tag_without_uri_is_dropped() -> NetResult<()> {
        let playlist = "#EXTM3U\n#EXT-X-PROGRAM-DATE-TIME:2026-08-12T10:00:00.000Z\n";
        let pairs = extract_program_date_times(playlist)?;
        assert!(pairs.is_empty());
        Ok(())
    }

    #[test]
    fn extract_program_date_times_rejects_malformed_timestamp() {
        let playlist = "#EXT-X-PROGRAM-DATE-TIME:not-a-timestamp\nseg_0.m4s\n";
        let result = extract_program_date_times(playlist);
        assert!(result.is_err());
    }

    #[test]
    fn latency_estimator_tracks_count_average_min_max() {
        let mut estimator = LatencyEstimator::new();
        let base = Utc::now();

        estimator.record(
            Some(1),
            base,
            base + ChronoDuration::milliseconds(100),
            None,
        );
        estimator.record(
            Some(2),
            base,
            base + ChronoDuration::milliseconds(300),
            None,
        );
        estimator.record(
            Some(3),
            base,
            base + ChronoDuration::milliseconds(200),
            None,
        );

        assert_eq!(estimator.sample_count(), 3);
        assert_eq!(estimator.min_latency_ms(), Some(100));
        assert_eq!(estimator.max_latency_ms(), Some(300));
        let avg = estimator
            .average_latency_ms()
            .expect("should have samples in test");
        assert!(
            (avg - 200.0).abs() < f64::EPSILON,
            "expected avg 200.0, got {avg}"
        );
    }

    #[test]
    fn latency_estimator_respects_window_size() {
        let mut estimator = LatencyEstimator::with_window(2);
        let base = Utc::now();

        estimator.record(
            Some(1),
            base,
            base + ChronoDuration::milliseconds(100),
            None,
        );
        estimator.record(
            Some(2),
            base,
            base + ChronoDuration::milliseconds(200),
            None,
        );
        estimator.record(
            Some(3),
            base,
            base + ChronoDuration::milliseconds(9000),
            None,
        );

        // The oldest sample (100ms) should have been evicted.
        assert_eq!(estimator.sample_count(), 2);
        assert_eq!(estimator.min_latency_ms(), Some(200));
        assert_eq!(estimator.max_latency_ms(), Some(9000));
    }

    #[test]
    fn latency_estimator_forwards_nonnegative_samples_to_analytics() {
        let analytics = Analytics::new(uuid::Uuid::new_v4());
        let mut estimator = LatencyEstimator::new();
        let base = Utc::now();

        estimator.record(
            Some(1),
            base,
            base + ChronoDuration::milliseconds(250),
            Some(&analytics),
        );

        let summary = analytics.summary();
        assert_eq!(summary.avg_latency, 250);
    }

    #[test]
    fn latency_estimator_does_not_forward_negative_samples_to_analytics() {
        let analytics = Analytics::new(uuid::Uuid::new_v4());
        let mut estimator = LatencyEstimator::new();
        let base = Utc::now();

        // observed_at *before* pdt: clock skew, negative latency.
        estimator.record(
            Some(1),
            base,
            base - ChronoDuration::milliseconds(500),
            Some(&analytics),
        );

        // Analytics never received a sample, so its average stays at the
        // zero-samples default rather than a fabricated "0ms".
        let summary = analytics.summary();
        assert_eq!(summary.avg_latency, 0);
        assert_eq!(estimator.sample_count(), 1);
        assert_eq!(estimator.latest().map(|s| s.latency_ms), Some(-500));
    }

    #[test]
    fn empty_estimator_reports_none() {
        let estimator = LatencyEstimator::new();
        assert_eq!(estimator.average_latency_ms(), None);
        assert_eq!(estimator.min_latency_ms(), None);
        assert_eq!(estimator.max_latency_ms(), None);
        assert!(estimator.latest().is_none());
    }
}

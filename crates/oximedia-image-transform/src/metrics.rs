// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Thread-safe metrics for image transform requests.
//!
//! [`TransformMetrics`] tracks:
//! - Total requests and cache hits/misses.
//! - Format distribution counters.
//! - Latency histogram (microseconds, power-of-2 buckets).
//! - Error rate counter.
//!
//! All counters use `AtomicU64` for lock-free concurrent updates.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::metrics::TransformMetrics;
//! use oximedia_image_transform::transform::OutputFormat;
//! use std::time::Duration;
//!
//! let m = TransformMetrics::new();
//! m.record_request(OutputFormat::WebP, false, Duration::from_millis(12), false);
//! m.record_request(OutputFormat::Avif, true, Duration::from_millis(5), false);
//! m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(8), true);
//!
//! let snapshot = m.snapshot();
//! assert_eq!(snapshot.total_requests, 3);
//! assert_eq!(snapshot.cache_hits, 1);
//! assert_eq!(snapshot.errors, 1);
//! assert!(snapshot.hit_rate() > 0.3);
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::transform::OutputFormat;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of latency histogram buckets.
///
/// Buckets cover microsecond durations in power-of-2 steps:
/// `[0,1µs), [1,2µs), [2,4µs), [4,8µs), …, [524288µs, ∞)`.
const LATENCY_BUCKET_COUNT: usize = 20;

/// Number of distinct output formats tracked in the distribution counters.
///
/// Order: Auto, Avif, WebP, Jpeg, Png, Gif, Baseline, Json.
const FORMAT_COUNT: usize = 8;

// ---------------------------------------------------------------------------
// TransformMetrics
// ---------------------------------------------------------------------------

/// Thread-safe, lock-free image transform metrics.
pub struct TransformMetrics {
    /// Total number of transform requests processed.
    total_requests: AtomicU64,
    /// Requests served from cache (cache hits).
    cache_hits: AtomicU64,
    /// Requests that resulted in an error.
    errors: AtomicU64,
    /// Per-format request count — indexed by [`format_index`].
    format_counts: [AtomicU64; FORMAT_COUNT],
    /// Latency histogram in microseconds using power-of-2 buckets.
    latency_buckets: [AtomicU64; LATENCY_BUCKET_COUNT],
    /// Sum of all latencies in microseconds (for mean calculation).
    total_latency_us: AtomicU64,
}

impl std::fmt::Debug for TransformMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.snapshot();
        f.debug_struct("TransformMetrics")
            .field("total_requests", &snap.total_requests)
            .field("cache_hits", &snap.cache_hits)
            .field("errors", &snap.errors)
            .finish()
    }
}

impl TransformMetrics {
    /// Create a new zeroed metrics instance.
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            format_counts: std::array::from_fn(|_| AtomicU64::new(0)),
            latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            total_latency_us: AtomicU64::new(0),
        }
    }

    /// Record a single transform request.
    ///
    /// - `format` — output format selected (after negotiation).
    /// - `cache_hit` — whether the result was served from cache.
    /// - `latency` — end-to-end request duration.
    /// - `is_error` — whether the request ended in an error.
    pub fn record_request(
        &self,
        format: OutputFormat,
        cache_hit: bool,
        latency: Duration,
        is_error: bool,
    ) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        if cache_hit {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
        }

        if is_error {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }

        // Format distribution
        let fi = format_index(format);
        self.format_counts[fi].fetch_add(1, Ordering::Relaxed);

        // Latency histogram
        let us = latency.as_micros() as u64;
        self.total_latency_us.fetch_add(us, Ordering::Relaxed);
        let bucket = latency_bucket(us);
        self.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.total_latency_us.store(0, Ordering::Relaxed);
        for fc in &self.format_counts {
            fc.store(0, Ordering::Relaxed);
        }
        for lb in &self.latency_buckets {
            lb.store(0, Ordering::Relaxed);
        }
    }

    /// Take a consistent (best-effort) snapshot of current counters.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total_requests.load(Ordering::Relaxed);
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        let total_lat_us = self.total_latency_us.load(Ordering::Relaxed);

        let format_distribution: Vec<FormatCount> = FORMAT_COUNT_ORDER
            .iter()
            .enumerate()
            .map(|(i, &fmt)| FormatCount {
                format: fmt,
                count: self.format_counts[i].load(Ordering::Relaxed),
            })
            .collect();

        let latency_histogram: Vec<LatencyBucket> = (0..LATENCY_BUCKET_COUNT)
            .map(|i| {
                let lower = if i == 0 { 0 } else { 1u64 << (i - 1) };
                let upper = if i + 1 < LATENCY_BUCKET_COUNT {
                    Some(1u64 << i)
                } else {
                    None // last bucket is open-ended
                };
                LatencyBucket {
                    lower_us: lower,
                    upper_us: upper,
                    count: self.latency_buckets[i].load(Ordering::Relaxed),
                }
            })
            .collect();

        let mean_latency_us = total_lat_us.checked_div(total).unwrap_or(0);

        // Estimate p95 from histogram
        let p95_us = estimate_percentile(&latency_histogram, total, 0.95);

        MetricsSnapshot {
            total_requests: total,
            cache_hits: hits,
            errors,
            format_distribution,
            latency_histogram,
            mean_latency_us,
            p95_latency_us: p95_us,
        }
    }
}

impl Default for TransformMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of transform metrics.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Total requests recorded.
    pub total_requests: u64,
    /// Requests served from cache.
    pub cache_hits: u64,
    /// Requests that ended in an error.
    pub errors: u64,
    /// Per-format request counts.
    pub format_distribution: Vec<FormatCount>,
    /// Latency histogram buckets.
    pub latency_histogram: Vec<LatencyBucket>,
    /// Mean latency in microseconds.
    pub mean_latency_us: u64,
    /// Estimated p95 latency in microseconds.
    pub p95_latency_us: u64,
}

impl MetricsSnapshot {
    /// Cache hit rate in the range [0.0, 1.0].
    pub fn hit_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.cache_hits as f64 / self.total_requests as f64
    }

    /// Error rate in the range [0.0, 1.0].
    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.errors as f64 / self.total_requests as f64
    }

    /// Cache miss count.
    pub fn cache_misses(&self) -> u64 {
        self.total_requests.saturating_sub(self.cache_hits)
    }
}

/// Request count for a specific output format.
#[derive(Debug, Clone)]
pub struct FormatCount {
    /// Output format.
    pub format: OutputFormat,
    /// Number of requests for this format.
    pub count: u64,
}

/// A single latency histogram bucket.
#[derive(Debug, Clone)]
pub struct LatencyBucket {
    /// Lower bound in microseconds (inclusive).
    pub lower_us: u64,
    /// Upper bound in microseconds (exclusive), or `None` for the last (open) bucket.
    pub upper_us: Option<u64>,
    /// Number of requests that fell into this bucket.
    pub count: u64,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Canonical order of [`OutputFormat`] variants for format distribution counters.
const FORMAT_COUNT_ORDER: [OutputFormat; FORMAT_COUNT] = [
    OutputFormat::Auto,
    OutputFormat::Avif,
    OutputFormat::WebP,
    OutputFormat::Jpeg,
    OutputFormat::Png,
    OutputFormat::Gif,
    OutputFormat::Baseline,
    OutputFormat::Json,
];

/// Map an [`OutputFormat`] to its slot in `FORMAT_COUNT_ORDER`.
fn format_index(fmt: OutputFormat) -> usize {
    match fmt {
        OutputFormat::Auto => 0,
        OutputFormat::Avif => 1,
        OutputFormat::WebP => 2,
        OutputFormat::Jpeg => 3,
        OutputFormat::Png => 4,
        OutputFormat::Gif => 5,
        OutputFormat::Baseline => 6,
        OutputFormat::Json => 7,
    }
}

/// Determine which power-of-2 histogram bucket a latency value falls into.
///
/// Bucket `i` covers `[2^(i-1), 2^i)` microseconds, with bucket 0 covering
/// `[0, 1)` and bucket 19 covering `[2^18, ∞)`.
fn latency_bucket(us: u64) -> usize {
    if us == 0 {
        return 0;
    }
    let bit = u64::BITS - us.leading_zeros(); // bit position (1-based)
    let bucket = bit as usize;
    bucket.min(LATENCY_BUCKET_COUNT - 1)
}

/// Estimate a percentile from a histogram.
fn estimate_percentile(buckets: &[LatencyBucket], total: u64, percentile: f64) -> u64 {
    if total == 0 {
        return 0;
    }
    let target = (total as f64 * percentile).ceil() as u64;
    let mut cumulative = 0u64;
    for bucket in buckets {
        cumulative += bucket.count;
        if cumulative >= target {
            // Return the midpoint of this bucket as an estimate
            return match bucket.upper_us {
                Some(upper) => (bucket.lower_us + upper) / 2,
                None => bucket.lower_us, // open bucket — return lower bound
            };
        }
    }
    // Fallback: return the lower bound of the last bucket
    buckets.last().map(|b| b.lower_us).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_initial_state() {
        let m = TransformMetrics::new();
        let s = m.snapshot();
        assert_eq!(s.total_requests, 0);
        assert_eq!(s.cache_hits, 0);
        assert_eq!(s.errors, 0);
        assert_eq!(s.hit_rate(), 0.0);
        assert_eq!(s.error_rate(), 0.0);
    }

    #[test]
    fn test_record_single_request() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::WebP, false, Duration::from_millis(10), false);
        let s = m.snapshot();
        assert_eq!(s.total_requests, 1);
        assert_eq!(s.cache_hits, 0);
        assert_eq!(s.errors, 0);
    }

    #[test]
    fn test_cache_hit_tracking() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::Avif, true, Duration::from_micros(100), false);
        m.record_request(OutputFormat::Avif, false, Duration::from_micros(500), false);
        let s = m.snapshot();
        assert_eq!(s.cache_hits, 1);
        assert!((s.hit_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_error_tracking() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(5), true);
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(5), false);
        let s = m.snapshot();
        assert_eq!(s.errors, 1);
        assert!((s.error_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_format_distribution() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::WebP, false, Duration::from_millis(1), false);
        m.record_request(OutputFormat::Avif, false, Duration::from_millis(1), false);
        m.record_request(OutputFormat::Avif, false, Duration::from_millis(1), false);
        let s = m.snapshot();
        let avif_count = s
            .format_distribution
            .iter()
            .find(|fc| fc.format == OutputFormat::Avif)
            .map(|fc| fc.count)
            .unwrap_or(0);
        let webp_count = s
            .format_distribution
            .iter()
            .find(|fc| fc.format == OutputFormat::WebP)
            .map(|fc| fc.count)
            .unwrap_or(0);
        assert_eq!(avif_count, 2);
        assert_eq!(webp_count, 1);
    }

    #[test]
    fn test_latency_histogram_populated() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(1), false);
        let s = m.snapshot();
        let total_in_histogram: u64 = s.latency_histogram.iter().map(|b| b.count).sum();
        assert_eq!(total_in_histogram, 1);
    }

    #[test]
    fn test_mean_latency() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(1), false);
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(3), false);
        let s = m.snapshot();
        assert_eq!(s.mean_latency_us, 2000);
    }

    #[test]
    fn test_reset() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::Png, true, Duration::from_millis(20), false);
        m.reset();
        let s = m.snapshot();
        assert_eq!(s.total_requests, 0);
        assert_eq!(s.cache_hits, 0);
    }

    #[test]
    fn test_cache_misses() {
        let m = TransformMetrics::new();
        m.record_request(OutputFormat::Jpeg, true, Duration::from_millis(1), false);
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(1), false);
        m.record_request(OutputFormat::Jpeg, false, Duration::from_millis(1), false);
        let s = m.snapshot();
        assert_eq!(s.cache_misses(), 2);
    }

    #[test]
    fn test_latency_bucket_zero() {
        assert_eq!(latency_bucket(0), 0);
    }

    #[test]
    fn test_latency_bucket_one_us() {
        // 1 µs — first non-zero bucket
        let b = latency_bucket(1);
        assert!(b < LATENCY_BUCKET_COUNT);
    }

    #[test]
    fn test_latency_bucket_large() {
        // Very large value should land in last bucket
        let b = latency_bucket(u64::MAX);
        assert_eq!(b, LATENCY_BUCKET_COUNT - 1);
    }

    #[test]
    fn test_thread_safe_concurrent_updates() {
        let m = Arc::new(TransformMetrics::new());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let m2 = Arc::clone(&m);
                thread::spawn(move || {
                    for _ in 0..100 {
                        m2.record_request(
                            OutputFormat::Jpeg,
                            false,
                            Duration::from_micros(500),
                            false,
                        );
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread panicked");
        }
        let s = m.snapshot();
        assert_eq!(s.total_requests, 800);
    }

    #[test]
    fn test_p95_latency_computed() {
        let m = TransformMetrics::new();
        // Insert 100 requests with various latencies
        for i in 0..100u64 {
            m.record_request(
                OutputFormat::Jpeg,
                false,
                Duration::from_micros(i * 100 + 100),
                false,
            );
        }
        let s = m.snapshot();
        // p95 should be greater than 0
        assert!(s.p95_latency_us > 0);
    }

    #[test]
    fn test_all_formats_tracked() {
        let m = TransformMetrics::new();
        for fmt in &[
            OutputFormat::Auto,
            OutputFormat::Avif,
            OutputFormat::WebP,
            OutputFormat::Jpeg,
            OutputFormat::Png,
            OutputFormat::Gif,
            OutputFormat::Baseline,
            OutputFormat::Json,
        ] {
            m.record_request(*fmt, false, Duration::from_millis(1), false);
        }
        let s = m.snapshot();
        let total_format: u64 = s.format_distribution.iter().map(|fc| fc.count).sum();
        assert_eq!(total_format, 8);
    }

    #[test]
    fn test_default_metrics() {
        let m = TransformMetrics::default();
        let s = m.snapshot();
        assert_eq!(s.total_requests, 0);
    }
}

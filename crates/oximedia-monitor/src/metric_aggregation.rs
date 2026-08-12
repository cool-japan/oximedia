//! Time-series metric aggregation for `OxiMedia` monitoring.
//!
//! Provides ring-buffered metric series with statistical aggregation,
//! downsampling (LTTB), moving averages, and exponential moving averages.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

// ─────────────────────────────────────────────────────────────────────────────
// AggregationResult
// ─────────────────────────────────────────────────────────────────────────────

/// Statistical summary of a metric over a time window.
#[derive(Debug, Clone)]
pub struct AggregationResult {
    /// Name of the metric.
    pub metric: String,
    /// Duration of the aggregation window in seconds.
    pub window_secs: u64,
    /// Number of data points included.
    pub count: usize,
    /// Minimum value in the window.
    pub min: f64,
    /// Maximum value in the window.
    pub max: f64,
    /// Arithmetic mean.
    pub mean: f64,
    /// 50th percentile (median).
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// 99th percentile.
    pub p99: f64,
    /// Sum of all values.
    pub sum: f64,
    /// Rate of change per second (last value minus first value, divided by elapsed).
    pub rate_per_sec: f64,
}

impl AggregationResult {
    /// Returns an empty/zero result when no data is available.
    fn empty(metric: impl Into<String>, window_secs: u64) -> Self {
        Self {
            metric: metric.into(),
            window_secs,
            count: 0,
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            p50: 0.0,
            p95: 0.0,
            p99: 0.0,
            sum: 0.0,
            rate_per_sec: 0.0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MetricSeries
// ─────────────────────────────────────────────────────────────────────────────

/// A bounded ring-buffered time series for a single named metric.
pub struct MetricSeries {
    /// Metric name.
    pub name: String,
    /// Chronological (oldest-first) sequence of `(timestamp, value)` points.
    pub points: VecDeque<(SystemTime, f64)>,
    /// Maximum number of points retained in the buffer.
    pub max_points: usize,
}

impl MetricSeries {
    /// Create a new series with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `max_points` is zero.
    #[must_use]
    pub fn new(name: &str, max_points: usize) -> Self {
        assert!(max_points > 0, "max_points must be greater than zero");
        Self {
            name: name.to_string(),
            points: VecDeque::with_capacity(max_points.min(1024)),
            max_points,
        }
    }

    /// Append a new data point stamped to now.
    ///
    /// If the buffer is at capacity the oldest point is evicted.
    pub fn push(&mut self, value: f64) {
        if self.points.len() >= self.max_points {
            self.points.pop_front();
        }
        self.points.push_back((SystemTime::now(), value));
    }

    /// Push a point with an explicit timestamp (useful for testing).
    pub fn push_at(&mut self, timestamp: SystemTime, value: f64) {
        if self.points.len() >= self.max_points {
            self.points.pop_front();
        }
        self.points.push_back((timestamp, value));
    }

    /// Compute aggregation statistics over points that fall within
    /// `window_secs` seconds before now.
    #[must_use]
    pub fn aggregate(&self, window_secs: u64) -> AggregationResult {
        let now = SystemTime::now();
        let window = Duration::from_secs(window_secs);
        let cutoff = now.checked_sub(window).unwrap_or(SystemTime::UNIX_EPOCH);

        let values: Vec<f64> = self
            .points
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .map(|(_, v)| *v)
            .collect();

        if values.is_empty() {
            return AggregationResult::empty(&self.name, window_secs);
        }

        let count = values.len();
        let sum: f64 = values.iter().sum();
        let mean = sum / count as f64;
        let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        // Percentiles via sorted copy.
        let mut sorted = values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let p50 = percentile(&sorted, 50.0);
        let p95 = percentile(&sorted, 95.0);
        let p99 = percentile(&sorted, 99.0);

        // Rate per second: (last - first) / elapsed.
        let in_window: Vec<(SystemTime, f64)> = self
            .points
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .cloned()
            .collect();

        let rate_per_sec = if in_window.len() >= 2 {
            let first = &in_window[0];
            let last = in_window.last().copied().unwrap_or(*first);
            let elapsed = last
                .0
                .duration_since(first.0)
                .unwrap_or(Duration::from_secs(1))
                .as_secs_f64()
                .max(1e-9);
            (last.1 - first.1) / elapsed
        } else {
            0.0
        };

        AggregationResult {
            metric: self.name.clone(),
            window_secs,
            count,
            min,
            max,
            mean,
            p50,
            p95,
            p99,
            sum,
            rate_per_sec,
        }
    }

    /// Downsample the series to at most `target_points` points using the
    /// Largest-Triangle-Three-Buckets (LTTB) algorithm.
    ///
    /// Returns a vector of `(unix_epoch_secs_f64, value)` suitable for charting.
    #[must_use]
    pub fn downsample(&self, target_points: usize) -> Vec<(f64, f64)> {
        let data: Vec<(f64, f64)> = self
            .points
            .iter()
            .map(|(ts, v)| {
                let epoch = ts
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_secs_f64();
                (epoch, *v)
            })
            .collect();

        if target_points == 0 || data.len() <= target_points {
            return data;
        }

        lttb(&data, target_points)
    }

    /// Compute a simple moving average with the given window size.
    ///
    /// The i-th output element is the mean of `points[i-window_size+1..=i]`.
    /// Positions before the first full window are computed over the available
    /// prefix.
    #[must_use]
    pub fn moving_average(&self, window_size: usize) -> Vec<f64> {
        if window_size == 0 || self.points.is_empty() {
            return Vec::new();
        }

        let values: Vec<f64> = self.points.iter().map(|(_, v)| *v).collect();
        let n = values.len();
        let mut result = Vec::with_capacity(n);
        let mut running_sum = 0.0_f64;

        for (i, &v) in values.iter().enumerate() {
            running_sum += v;
            if i >= window_size {
                running_sum -= values[i - window_size];
            }
            let divisor = (i + 1).min(window_size) as f64;
            result.push(running_sum / divisor);
        }

        result
    }

    /// Compute an exponential moving average with smoothing factor `alpha`.
    ///
    /// `alpha` should be in (0, 1]. A higher `alpha` puts more weight on
    /// recent observations.
    #[must_use]
    pub fn exponential_moving_average(&self, alpha: f64) -> Vec<f64> {
        let alpha = alpha.clamp(0.0, 1.0);
        if self.points.is_empty() {
            return Vec::new();
        }

        let values: Vec<f64> = self.points.iter().map(|(_, v)| *v).collect();
        let mut result = Vec::with_capacity(values.len());
        let mut ema = values[0];
        result.push(ema);

        for &v in &values[1..] {
            ema = alpha * v + (1.0 - alpha) * ema;
            result.push(ema);
        }

        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MetricStore
// ─────────────────────────────────────────────────────────────────────────────

/// A collection of named [`MetricSeries`] with a shared default capacity.
pub struct MetricStore {
    series: HashMap<String, MetricSeries>,
    default_max_points: usize,
}

impl MetricStore {
    /// Create a new store; each new series will have `default_max_points` capacity.
    #[must_use]
    pub fn new(default_max_points: usize) -> Self {
        let cap = default_max_points.max(1);
        Self {
            series: HashMap::new(),
            default_max_points: cap,
        }
    }

    /// Record a value for the named metric, creating the series if necessary.
    pub fn record(&mut self, metric: &str, value: f64) {
        let cap = self.default_max_points;
        self.series
            .entry(metric.to_string())
            .or_insert_with(|| MetricSeries::new(metric, cap))
            .push(value);
    }

    /// Return a reference to the named series, if it exists.
    #[must_use]
    pub fn get(&self, metric: &str) -> Option<&MetricSeries> {
        self.series.get(metric)
    }

    /// Compute aggregation statistics for the named metric and window.
    #[must_use]
    pub fn aggregate(&self, metric: &str, window_secs: u64) -> Option<AggregationResult> {
        self.series.get(metric).map(|s| s.aggregate(window_secs))
    }

    /// Return the names of all tracked metrics.
    #[must_use]
    pub fn metric_names(&self) -> Vec<&str> {
        self.series.keys().map(String::as_str).collect()
    }

    /// Remove all data points older than `older_than_secs` seconds from every
    /// series, and drop series that become empty.
    pub fn purge_old(&mut self, older_than_secs: u64) {
        let cutoff_dur = Duration::from_secs(older_than_secs);
        let now = SystemTime::now();
        let cutoff = now
            .checked_sub(cutoff_dur)
            .unwrap_or(SystemTime::UNIX_EPOCH);

        for series in self.series.values_mut() {
            series.points.retain(|(ts, _)| *ts >= cutoff);
        }

        // Drop series that are now empty.
        self.series.retain(|_, s| !s.points.is_empty());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistical helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compute a percentile from a **sorted** slice using linear interpolation.
fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = pct / 100.0 * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;

    if hi >= sorted.len() {
        // `sorted` has at least 2 elements here (both the empty and
        // single-element cases return above), so this fallback is never
        // actually reached — kept as a safe default rather than a panic.
        return sorted.last().copied().unwrap_or(0.0);
    }

    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

// ─────────────────────────────────────────────────────────────────────────────
// LTTB downsampling
// ─────────────────────────────────────────────────────────────────────────────

/// Largest-Triangle-Three-Buckets downsampling algorithm.
///
/// `data` must be `(x, y)` pairs in chronological order.
/// `target` must be ≥ 2.
fn lttb(data: &[(f64, f64)], target: usize) -> Vec<(f64, f64)> {
    let n = data.len();
    if target >= n || target < 2 {
        return data.to_vec();
    }

    let mut sampled: Vec<(f64, f64)> = Vec::with_capacity(target);

    // Always include the first point.
    sampled.push(data[0]);

    // The data between the first and last points is divided into (target - 2) buckets.
    let bucket_count = target - 2;
    // Inner data: indices 1 .. n-1.
    let inner_len = n - 2;

    for i in 0..bucket_count {
        // Bucket range in the inner data.
        let bucket_start = (i as f64 * inner_len as f64 / bucket_count as f64).floor() as usize;
        let bucket_end = ((i + 1) as f64 * inner_len as f64 / bucket_count as f64).floor() as usize;

        let bucket_start = bucket_start + 1; // +1 because inner starts at index 1
        let bucket_end = (bucket_end + 1).min(n - 1);

        // Average of the next bucket (used as "point C").
        let next_bucket_start =
            ((i + 1) as f64 * inner_len as f64 / bucket_count as f64).floor() as usize + 1;
        let next_bucket_end =
            ((i + 2) as f64 * inner_len as f64 / bucket_count as f64).floor() as usize + 1;
        let next_bucket_end = next_bucket_end.min(n);

        let (avg_x, avg_y) = if next_bucket_start < next_bucket_end {
            let slice = &data[next_bucket_start..next_bucket_end];
            let avg_x = slice.iter().map(|(x, _)| x).sum::<f64>() / slice.len() as f64;
            let avg_y = slice.iter().map(|(_, y)| y).sum::<f64>() / slice.len() as f64;
            (avg_x, avg_y)
        } else {
            data[n - 1]
        };

        // Point A is the last selected point. `sampled` always holds at
        // least the first data point pushed above, so this is never
        // actually reached — `data[0]` is a safe, semantically correct
        // fallback rather than a panic.
        let (ax, ay) = sampled.last().copied().unwrap_or(data[0]);

        // Find the point in the current bucket that maximises the triangle area.
        let mut max_area = -1.0_f64;
        let mut max_idx = bucket_start;

        for j in bucket_start..bucket_end {
            let (bx, by) = data[j];
            // Area of triangle ABC = 0.5 * |Ax(By-Cy) + Bx(Cy-Ay) + Cx(Ay-By)|
            let area = (ax * (by - avg_y) + bx * (avg_y - ay) + avg_x * (ay - by)).abs() * 0.5;
            if area > max_area {
                max_area = area;
                max_idx = j;
            }
        }

        sampled.push(data[max_idx]);
    }

    // Always include the last point. `n >= 3` here (the early return above
    // covers `target >= n || target < 2`, and `target >= 2`), so indexing
    // by `n - 1` mirrors the existing `data[n - 1]` fallback used above.
    sampled.push(data[n - 1]);

    sampled
}

// ─────────────────────────────────────────────────────────────────────────────
// Configurable aggregation granularity
// ─────────────────────────────────────────────────────────────────────────────

/// Predefined aggregation granularities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregationGranularity {
    /// 1-second granularity.
    OneSecond,
    /// 10-second granularity.
    TenSeconds,
    /// 1-minute granularity.
    OneMinute,
    /// 5-minute granularity.
    FiveMinutes,
    /// Custom granularity in seconds.
    Custom(u64),
}

impl AggregationGranularity {
    /// Return the window size in seconds.
    #[must_use]
    pub fn window_secs(self) -> u64 {
        match self {
            Self::OneSecond => 1,
            Self::TenSeconds => 10,
            Self::OneMinute => 60,
            Self::FiveMinutes => 300,
            Self::Custom(s) => s.max(1),
        }
    }

    /// Human-readable label.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::OneSecond => "1s".to_string(),
            Self::TenSeconds => "10s".to_string(),
            Self::OneMinute => "1m".to_string(),
            Self::FiveMinutes => "5m".to_string(),
            Self::Custom(s) => format!("{s}s"),
        }
    }
}

/// A time-aligned aggregation bucket.
#[derive(Debug, Clone)]
pub struct GranularBucket {
    /// Bucket start time.
    pub bucket_start: SystemTime,
    /// Bucket window in seconds.
    pub window_secs: u64,
    /// Number of data points in this bucket.
    pub count: usize,
    /// Minimum value.
    pub min: f64,
    /// Maximum value.
    pub max: f64,
    /// Sum of all values.
    pub sum: f64,
    /// Mean value.
    pub mean: f64,
}

/// Multi-granularity aggregator that can produce statistics at different
/// time resolutions from a single [`MetricSeries`].
pub struct GranularAggregator {
    /// The granularities to compute.
    granularities: Vec<AggregationGranularity>,
}

impl GranularAggregator {
    /// Create an aggregator with the default set of granularities (1s, 10s, 1m, 5m).
    #[must_use]
    pub fn default_set() -> Self {
        Self {
            granularities: vec![
                AggregationGranularity::OneSecond,
                AggregationGranularity::TenSeconds,
                AggregationGranularity::OneMinute,
                AggregationGranularity::FiveMinutes,
            ],
        }
    }

    /// Create an aggregator with a custom set of granularities.
    #[must_use]
    pub fn new(granularities: Vec<AggregationGranularity>) -> Self {
        Self { granularities }
    }

    /// Aggregate a series at a specific granularity.
    ///
    /// Points from the series are grouped into time-aligned buckets of the
    /// given granularity, and each bucket is summarized.
    #[must_use]
    pub fn aggregate_at(
        series: &MetricSeries,
        granularity: AggregationGranularity,
    ) -> Vec<GranularBucket> {
        let window = Duration::from_secs(granularity.window_secs());

        if series.points.is_empty() {
            return Vec::new();
        }

        // Group points into buckets aligned to the granularity.
        let mut buckets: HashMap<u64, Vec<f64>> = HashMap::new();

        for (ts, val) in &series.points {
            let epoch_secs = ts
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs();
            let bucket_key = epoch_secs / granularity.window_secs();
            buckets.entry(bucket_key).or_default().push(*val);
        }

        let mut result: Vec<GranularBucket> = buckets
            .into_iter()
            .map(|(key, values)| {
                let count = values.len();
                let sum: f64 = values.iter().sum();
                let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
                let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let mean = sum / count as f64;

                let bucket_epoch = key * granularity.window_secs();
                let bucket_start = SystemTime::UNIX_EPOCH + Duration::from_secs(bucket_epoch);

                GranularBucket {
                    bucket_start,
                    window_secs: window.as_secs(),
                    count,
                    min,
                    max,
                    sum,
                    mean,
                }
            })
            .collect();

        result.sort_by(|a, b| a.bucket_start.cmp(&b.bucket_start));
        result
    }

    /// Aggregate a series at all configured granularities.
    ///
    /// Returns a map of granularity -> buckets.
    #[must_use]
    pub fn aggregate_all(
        &self,
        series: &MetricSeries,
    ) -> HashMap<AggregationGranularity, Vec<GranularBucket>> {
        let mut result = HashMap::new();
        for &g in &self.granularities {
            result.insert(g, Self::aggregate_at(series, g));
        }
        result
    }

    /// Return the configured granularities.
    #[must_use]
    pub fn granularities(&self) -> &[AggregationGranularity] {
        &self.granularities
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    // ── Helper ────────────────────────────────────────────────────────────────

    /// Create a series prepopulated with `values` using synthetic timestamps
    /// spaced 1 second apart, ending 1 second ago (so all points are within
    /// any reasonable window).
    fn series_from_values(name: &str, values: &[f64]) -> MetricSeries {
        let mut s = MetricSeries::new(name, values.len().max(1) * 2);
        let now = SystemTime::now();
        for (i, &v) in values.iter().enumerate() {
            // oldest point is values.len()-1 seconds ago
            let offset = Duration::from_secs((values.len() - 1 - i) as u64);
            let ts = now.checked_sub(offset).unwrap_or(SystemTime::UNIX_EPOCH);
            s.push_at(ts, v);
        }
        s
    }

    // ── push / evict ──────────────────────────────────────────────────────────

    #[test]
    fn test_push_and_len() {
        let mut s = MetricSeries::new("m", 5);
        for i in 0..5 {
            s.push(i as f64);
        }
        assert_eq!(s.points.len(), 5);
    }

    #[test]
    fn test_push_evicts_oldest_at_capacity() {
        let mut s = MetricSeries::new("m", 3);
        s.push(1.0);
        s.push(2.0);
        s.push(3.0);
        s.push(4.0); // evicts 1.0
        assert_eq!(s.points.len(), 3);
        let values: Vec<f64> = s.points.iter().map(|(_, v)| *v).collect();
        assert_eq!(values[0], 2.0);
        assert_eq!(values[2], 4.0);
    }

    // ── aggregate stats ───────────────────────────────────────────────────────

    #[test]
    fn test_aggregate_basic_stats() {
        let s = series_from_values("cpu", &[10.0, 20.0, 30.0, 40.0, 50.0]);
        let result = s.aggregate(3600);
        assert_eq!(result.count, 5);
        assert!((result.min - 10.0).abs() < 1e-9);
        assert!((result.max - 50.0).abs() < 1e-9);
        assert!((result.sum - 150.0).abs() < 1e-9);
        assert!((result.mean - 30.0).abs() < 1e-9);
    }

    #[test]
    fn test_aggregate_empty_window_returns_zero() {
        let s = MetricSeries::new("m", 10);
        let result = s.aggregate(60);
        assert_eq!(result.count, 0);
        assert_eq!(result.min, 0.0);
    }

    #[test]
    fn test_aggregate_percentiles() {
        // With sorted [10,20,30,40,50]: p50=30, p95≈48, p99≈49.6
        let s = series_from_values("m", &[10.0, 20.0, 30.0, 40.0, 50.0]);
        let result = s.aggregate(3600);
        assert!((result.p50 - 30.0).abs() < 1e-6, "p50={}", result.p50);
        assert!(result.p95 > 40.0, "p95={}", result.p95);
        assert!(result.p99 > result.p95, "p99 should exceed p95");
    }

    #[test]
    fn test_aggregate_rate_per_sec() {
        // Two points: value 0.0 at t-10s, value 100.0 at now.
        // Rate = 100/10 = 10.0/s
        let mut s = MetricSeries::new("m", 10);
        let now = SystemTime::now();
        s.push_at(
            now.checked_sub(Duration::from_secs(10))
                .unwrap_or(SystemTime::UNIX_EPOCH),
            0.0,
        );
        s.push_at(now, 100.0);
        let result = s.aggregate(3600);
        assert!(
            (result.rate_per_sec - 10.0).abs() < 0.5,
            "rate={}",
            result.rate_per_sec
        );
    }

    // ── moving average ────────────────────────────────────────────────────────

    #[test]
    fn test_moving_average_window_1() {
        let s = series_from_values("m", &[1.0, 2.0, 3.0]);
        let ma = s.moving_average(1);
        assert_eq!(ma, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_moving_average_window_3() {
        let s = series_from_values("m", &[1.0, 2.0, 3.0, 4.0, 5.0]);
        let ma = s.moving_average(3);
        // Expected: 1.0, 1.5, 2.0, 3.0, 4.0
        assert_eq!(ma.len(), 5);
        assert!((ma[2] - 2.0).abs() < 1e-9, "ma[2]={}", ma[2]);
        assert!((ma[4] - 4.0).abs() < 1e-9, "ma[4]={}", ma[4]);
    }

    #[test]
    fn test_moving_average_empty() {
        let s = MetricSeries::new("m", 10);
        assert!(s.moving_average(3).is_empty());
    }

    // ── EMA ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_ema_single_point() {
        let s = series_from_values("m", &[42.0]);
        let ema = s.exponential_moving_average(0.5);
        assert_eq!(ema, vec![42.0]);
    }

    #[test]
    fn test_ema_converges_toward_new_value() {
        let s = series_from_values("m", &[0.0, 100.0, 100.0, 100.0, 100.0]);
        let ema = s.exponential_moving_average(0.9);
        // With alpha=0.9 the EMA should rapidly converge to 100.
        assert!(
            *ema.last().unwrap_or(&0.0) > 90.0,
            "ema last={}",
            ema.last().unwrap_or(&0.0)
        );
    }

    #[test]
    fn test_ema_empty() {
        let s = MetricSeries::new("m", 10);
        assert!(s.exponential_moving_average(0.5).is_empty());
    }

    // ── downsample (LTTB) ─────────────────────────────────────────────────────

    #[test]
    fn test_downsample_fewer_points_than_target_returns_all() {
        let s = series_from_values("m", &[1.0, 2.0, 3.0]);
        let result = s.downsample(10);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_downsample_reduces_to_target() {
        let values: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let s = series_from_values("m", &values);
        let result = s.downsample(10);
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_downsample_preserves_first_and_last() {
        let values: Vec<f64> = (0..50).map(|i| i as f64).collect();
        let s = series_from_values("m", &values);
        let result = s.downsample(5);
        // First point x-value should be ≤ second point x-value (chronological order).
        assert!(
            result[0].0 <= result[1].0,
            "first point should precede second"
        );
        assert!(result[result.len() - 1].0 >= result[result.len() - 2].0);
    }

    // ── MetricStore ───────────────────────────────────────────────────────────

    #[test]
    fn test_metric_store_record_and_get() {
        let mut store = MetricStore::new(100);
        store.record("cpu", 55.0);
        let series = store.get("cpu").expect("series should exist");
        assert_eq!(series.points.len(), 1);
    }

    #[test]
    fn test_metric_store_aggregate() {
        let mut store = MetricStore::new(100);
        for v in [10.0, 20.0, 30.0] {
            store.record("cpu", v);
        }
        let result = store.aggregate("cpu", 3600).expect("result should exist");
        assert_eq!(result.count, 3);
        assert!((result.mean - 20.0).abs() < 1e-9);
    }

    #[test]
    fn test_metric_store_metric_names() {
        let mut store = MetricStore::new(100);
        store.record("cpu", 1.0);
        store.record("mem", 2.0);
        let mut names = store.metric_names();
        names.sort();
        assert_eq!(names, vec!["cpu", "mem"]);
    }

    #[test]
    fn test_metric_store_purge_old() {
        let mut store = MetricStore::new(100);
        // Record with synthetic old timestamp directly.
        {
            let cap = store.default_max_points;
            let old_series = store
                .series
                .entry("cpu".to_string())
                .or_insert_with(|| MetricSeries::new("cpu", cap));
            old_series.push_at(SystemTime::UNIX_EPOCH, 1.0); // very old
        }
        store.record("cpu", 2.0); // recent

        // Purge data older than 1 hour — UNIX_EPOCH point removed, recent kept.
        store.purge_old(3600);
        let series = store.get("cpu").expect("series should remain");
        // The recent point should remain.
        assert!(series.points.len() >= 1);
        // The UNIX_EPOCH point (born ~56 years ago) should be gone.
        let has_old = series
            .points
            .iter()
            .any(|(ts, _)| *ts == SystemTime::UNIX_EPOCH);
        assert!(!has_old, "UNIX_EPOCH point should have been purged");
    }

    // ── AggregationGranularity ───────────────────────────────────────────

    #[test]
    fn test_granularity_window_secs() {
        assert_eq!(AggregationGranularity::OneSecond.window_secs(), 1);
        assert_eq!(AggregationGranularity::TenSeconds.window_secs(), 10);
        assert_eq!(AggregationGranularity::OneMinute.window_secs(), 60);
        assert_eq!(AggregationGranularity::FiveMinutes.window_secs(), 300);
        assert_eq!(AggregationGranularity::Custom(30).window_secs(), 30);
    }

    #[test]
    fn test_granularity_custom_min_1() {
        assert_eq!(AggregationGranularity::Custom(0).window_secs(), 1);
    }

    #[test]
    fn test_granularity_labels() {
        assert_eq!(AggregationGranularity::OneSecond.label(), "1s");
        assert_eq!(AggregationGranularity::TenSeconds.label(), "10s");
        assert_eq!(AggregationGranularity::OneMinute.label(), "1m");
        assert_eq!(AggregationGranularity::FiveMinutes.label(), "5m");
        assert_eq!(AggregationGranularity::Custom(120).label(), "120s");
    }

    // ── GranularAggregator ───────────────────────────────────────────────

    fn series_with_second_intervals(name: &str, values: &[f64]) -> MetricSeries {
        let mut s = MetricSeries::new(name, values.len().max(1) * 2);
        // Use a base time divisible by 300 (5 min) for predictable bucket alignment.
        // 1_700_001_000 / 300 = 5_666_670 exactly.
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_001_000);
        for (i, &v) in values.iter().enumerate() {
            s.push_at(base + Duration::from_secs(i as u64), v);
        }
        s
    }

    #[test]
    fn test_aggregate_at_one_second() {
        let s = series_with_second_intervals("cpu", &[10.0, 20.0, 30.0]);
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::OneSecond);
        assert_eq!(buckets.len(), 3); // each second is its own bucket
        for b in &buckets {
            assert_eq!(b.count, 1);
        }
    }

    #[test]
    fn test_aggregate_at_ten_seconds() {
        // 10 samples, 1 per second, all in the same 10-second bucket.
        let s = series_with_second_intervals(
            "cpu",
            &[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0],
        );
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::TenSeconds);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].count, 10);
        assert!((buckets[0].min - 10.0).abs() < 1e-9);
        assert!((buckets[0].max - 100.0).abs() < 1e-9);
        assert!((buckets[0].mean - 55.0).abs() < 1e-9);
    }

    #[test]
    fn test_aggregate_at_one_minute_spans_multiple_buckets() {
        // 120 samples = 2 minutes worth at 1/sec, spanning 2 minute buckets.
        let values: Vec<f64> = (0..120).map(|i| i as f64).collect();
        let s = series_with_second_intervals("m", &values);
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::OneMinute);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].count, 60);
        assert_eq!(buckets[1].count, 60);
    }

    #[test]
    fn test_aggregate_at_empty_series() {
        let s = MetricSeries::new("m", 10);
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::OneMinute);
        assert!(buckets.is_empty());
    }

    #[test]
    fn test_aggregate_all_returns_all_granularities() {
        let values: Vec<f64> = (0..60).map(|i| i as f64).collect();
        let s = series_with_second_intervals("m", &values);
        let agg = GranularAggregator::default_set();
        let result = agg.aggregate_all(&s);
        assert_eq!(result.len(), 4);
        assert!(result.contains_key(&AggregationGranularity::OneSecond));
        assert!(result.contains_key(&AggregationGranularity::TenSeconds));
        assert!(result.contains_key(&AggregationGranularity::OneMinute));
        assert!(result.contains_key(&AggregationGranularity::FiveMinutes));
    }

    #[test]
    fn test_aggregate_at_custom_granularity() {
        // 30 samples -> with 15-second granularity should produce 2 buckets.
        let values: Vec<f64> = (0..30).map(|i| i as f64).collect();
        let s = series_with_second_intervals("m", &values);
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::Custom(15));
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].count, 15);
        assert_eq!(buckets[1].count, 15);
    }

    #[test]
    fn test_granular_aggregator_granularities() {
        let agg = GranularAggregator::new(vec![
            AggregationGranularity::OneSecond,
            AggregationGranularity::Custom(30),
        ]);
        assert_eq!(agg.granularities().len(), 2);
    }

    #[test]
    fn test_granular_bucket_stats_correct() {
        let s = series_with_second_intervals("m", &[5.0, 15.0, 25.0]);
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::FiveMinutes);
        assert_eq!(buckets.len(), 1);
        let b = &buckets[0];
        assert_eq!(b.count, 3);
        assert!((b.min - 5.0).abs() < 1e-9);
        assert!((b.max - 25.0).abs() < 1e-9);
        assert!((b.sum - 45.0).abs() < 1e-9);
        assert!((b.mean - 15.0).abs() < 1e-9);
    }

    #[test]
    fn test_granular_buckets_sorted_by_time() {
        // Ensure output is chronologically sorted.
        let values: Vec<f64> = (0..120).map(|i| i as f64).collect();
        let s = series_with_second_intervals("m", &values);
        let buckets = GranularAggregator::aggregate_at(&s, AggregationGranularity::TenSeconds);
        for w in buckets.windows(2) {
            assert!(w[0].bucket_start <= w[1].bucket_start);
        }
    }
}

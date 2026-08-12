//! `oximedia.analytics` quantile-estimation and real-time-aggregation
//! bindings — real delegation to [`oximedia_analytics::quantile`] and
//! [`oximedia_analytics::realtime`].

use oximedia_analytics::quantile as quantile_core;
use oximedia_analytics::realtime as realtime_core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// TDigest
// ---------------------------------------------------------------------------

/// Streaming approximate-quantile estimator (t-digest, Dunning & Ertl 2019).
/// Suitable for P50/P95/P99 of watch-time, bitrate, or latency at scale.
#[pyclass(name = "TDigest")]
pub struct PyTDigest {
    inner: quantile_core::TDigest,
}

#[pymethods]
impl PyTDigest {
    /// `delta` is the compression parameter: larger values give more
    /// centroids (more accuracy). Typical range: 100 (moderate) to 1000
    /// (high accuracy).
    #[new]
    fn new(delta: f64) -> Self {
        Self {
            inner: quantile_core::TDigest::new(delta),
        }
    }

    /// Add a single data point with weight 1.
    fn add(&mut self, value: f64) {
        self.inner.add(value);
    }

    /// Add a data point with an explicit weight.
    fn add_weighted(&mut self, value: f64, weight: f64) {
        self.inner.add_weighted(value, weight);
    }

    /// Add every value from a list.
    fn add_all(&mut self, values: Vec<f64>) {
        self.inner.add_all(&values);
    }

    /// Merge another digest's data into this one.
    fn merge(&mut self, other: &PyTDigest) {
        self.inner.merge(&other.inner);
    }

    /// Estimate the value at quantile `q` in `[0.0, 1.0]`. Errors if `q` is
    /// out of range or the digest has no data yet.
    fn quantile(&mut self, q: f64) -> PyResult<f64> {
        self.inner.quantile(q).map_err(super::analytics_err)
    }

    /// Number of centroids currently held (compactness measure).
    fn centroid_count(&self) -> usize {
        self.inner.centroid_count()
    }

    /// Total weight (number of points added so far, including buffered).
    fn total_weight(&self) -> f64 {
        self.inner.total_weight()
    }

    #[getter]
    fn min(&self) -> f64 {
        self.inner.min
    }

    #[getter]
    fn max(&self) -> f64 {
        self.inner.max
    }

    fn __repr__(&self) -> String {
        format!(
            "TDigest(total_weight={}, centroids={})",
            self.inner.total_weight(),
            self.inner.centroid_count()
        )
    }
}

/// Compute multiple percentiles (each in `[0, 100]`) from a value slice in
/// one pass, using an internal `TDigest(delta=100)`.
#[pyfunction]
fn percentiles(values: Vec<f64>, percentiles: Vec<f64>) -> PyResult<Vec<f64>> {
    quantile_core::percentiles(&values, &percentiles).map_err(super::analytics_err)
}

// ---------------------------------------------------------------------------
// SlidingWindowAggregator
// ---------------------------------------------------------------------------

/// Bucketed metrics for one window slice.
#[pyclass(name = "BucketMetrics")]
pub struct PyBucketMetrics {
    #[pyo3(get)]
    pub bucket_start_ms: i64,
    #[pyo3(get)]
    pub peak_concurrent_viewers: u32,
    #[pyo3(get)]
    pub avg_bitrate_bps: f64,
    #[pyo3(get)]
    pub min_bitrate_bps: u64,
    #[pyo3(get)]
    pub max_bitrate_bps: u64,
    #[pyo3(get)]
    pub buffer_event_count: u32,
    #[pyo3(get)]
    pub buffer_stall_ms: u64,
    #[pyo3(get)]
    pub bitrate_sample_count: u32,
}

impl From<&realtime_core::BucketMetrics> for PyBucketMetrics {
    fn from(b: &realtime_core::BucketMetrics) -> Self {
        Self {
            bucket_start_ms: b.bucket_start_ms,
            peak_concurrent_viewers: b.peak_concurrent_viewers,
            avg_bitrate_bps: b.avg_bitrate_bps,
            min_bitrate_bps: b.min_bitrate_bps,
            max_bitrate_bps: b.max_bitrate_bps,
            buffer_event_count: b.buffer_event_count,
            buffer_stall_ms: b.buffer_stall_ms,
            bitrate_sample_count: b.bitrate_sample_count,
        }
    }
}

#[pymethods]
impl PyBucketMetrics {
    fn __repr__(&self) -> String {
        format!(
            "BucketMetrics(bucket_start_ms={}, peak_concurrent_viewers={})",
            self.bucket_start_ms, self.peak_concurrent_viewers
        )
    }
}

/// Rolling window aggregator for concurrent viewers, bitrate stats, and
/// buffer events, bucketed at `bucket_ms` granularity over a
/// `window_duration_ms` horizon. Old buckets are evicted automatically as
/// new events arrive.
#[pyclass(name = "SlidingWindowAggregator")]
pub struct PySlidingWindowAggregator {
    inner: realtime_core::SlidingWindowAggregator,
}

#[pymethods]
impl PySlidingWindowAggregator {
    #[new]
    fn new(window_duration_ms: i64, bucket_ms: i64) -> PyResult<Self> {
        let inner = realtime_core::SlidingWindowAggregator::new(window_duration_ms, bucket_ms)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Record a viewer joining (or resuming) at `timestamp_ms`.
    fn ingest_viewer_join(&mut self, viewer_id: String, timestamp_ms: i64) {
        self.inner.ingest(realtime_core::RealtimeEvent::ViewerJoin {
            viewer_id,
            timestamp_ms,
        });
    }

    /// Record a viewer leaving (or pausing/closing) at `timestamp_ms`.
    fn ingest_viewer_leave(&mut self, viewer_id: String, timestamp_ms: i64) {
        self.inner
            .ingest(realtime_core::RealtimeEvent::ViewerLeave {
                viewer_id,
                timestamp_ms,
            });
    }

    /// Record a bitrate sample (bits per second) from a player.
    fn ingest_bitrate_report(&mut self, viewer_id: String, timestamp_ms: i64, bitrate_bps: u64) {
        self.inner
            .ingest(realtime_core::RealtimeEvent::BitrateReport {
                viewer_id,
                timestamp_ms,
                bitrate_bps,
            });
    }

    /// Record a buffering stall of `duration_ms`.
    fn ingest_buffer_event(&mut self, viewer_id: String, timestamp_ms: i64, duration_ms: u32) {
        self.inner
            .ingest(realtime_core::RealtimeEvent::BufferEvent {
                viewer_id,
                timestamp_ms,
                duration_ms,
            });
    }

    /// Current instantaneous concurrent-viewer count.
    fn concurrent_viewers(&self) -> u32 {
        self.inner.concurrent_viewers()
    }

    /// `(avg_bps, min_bps, max_bps)` across all active buckets; `(0.0, 0,
    /// 0)` when no bitrate samples exist in the window.
    fn window_bitrate_stats(&self) -> (f64, u64, u64) {
        self.inner.window_bitrate_stats()
    }

    /// Total buffer events in the current window.
    fn window_buffer_events(&self) -> u32 {
        self.inner.window_buffer_events()
    }

    /// Peak concurrent viewers across all active buckets.
    fn window_peak_concurrent(&self) -> u32 {
        self.inner.window_peak_concurrent()
    }

    /// Snapshot of every active bucket, oldest first.
    fn buckets(&self) -> Vec<PyBucketMetrics> {
        self.inner
            .buckets()
            .iter()
            .map(PyBucketMetrics::from)
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "SlidingWindowAggregator(concurrent_viewers={}, buckets={})",
            self.inner.concurrent_viewers(),
            self.inner.buckets().len()
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyTDigest>()?;
    m.add_function(wrap_pyfunction!(percentiles, m)?)?;
    m.add_class::<PyBucketMetrics>()?;
    m.add_class::<PySlidingWindowAggregator>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tdigest_single_value_quantile() {
        let mut d = PyTDigest::new(100.0);
        d.add(42.0);
        let q50 = d.quantile(0.5).expect("quantile should succeed");
        assert!((q50 - 42.0).abs() < 1e-9);
    }

    #[test]
    fn tdigest_sequential_calls_do_not_borrow_conflict() {
        // Regression check: `&mut self` methods called back-to-back must not
        // panic with "already borrowed" — each call independently reacquires
        // the borrow.
        let mut d = PyTDigest::new(100.0);
        d.add(1.0);
        let _ = d.quantile(0.5).expect("first quantile");
        d.add(2.0);
        let _ = d.quantile(0.9).expect("second quantile");
    }

    #[test]
    fn tdigest_empty_errors() {
        let mut d = PyTDigest::new(100.0);
        assert!(d.quantile(0.5).is_err());
    }

    #[test]
    fn percentiles_basic() {
        let values: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let result = percentiles(values, vec![50.0, 95.0]).expect("should succeed");
        assert_eq!(result.len(), 2);
        assert!((result[0] - 50.0).abs() < 10.0);
    }

    #[test]
    fn percentiles_empty_errors() {
        assert!(percentiles(vec![], vec![50.0]).is_err());
    }

    #[test]
    fn aggregator_rejects_invalid_window() {
        assert!(PySlidingWindowAggregator::new(500, 1000).is_err());
    }

    #[test]
    fn aggregator_tracks_concurrent_viewers() {
        let mut agg = PySlidingWindowAggregator::new(60_000, 10_000).expect("valid");
        agg.ingest_viewer_join("a".to_string(), 1_000);
        agg.ingest_viewer_join("b".to_string(), 2_000);
        assert_eq!(agg.concurrent_viewers(), 2);
        agg.ingest_viewer_leave("a".to_string(), 3_000);
        assert_eq!(agg.concurrent_viewers(), 1);
    }

    #[test]
    fn aggregator_bitrate_stats() {
        let mut agg = PySlidingWindowAggregator::new(60_000, 10_000).expect("valid");
        for bps in [1_000_000u64, 3_000_000] {
            agg.ingest_bitrate_report("v".to_string(), 5_000, bps);
        }
        let (avg, min, max) = agg.window_bitrate_stats();
        assert!((avg - 2_000_000.0).abs() < 1.0);
        assert_eq!(min, 1_000_000);
        assert_eq!(max, 3_000_000);
        assert_eq!(agg.buckets().len(), 1);
    }
}

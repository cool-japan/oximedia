//! `oximedia.cache` predictive cache-warming bindings — real delegation to
//! [`oximedia_cache::cache_warming`].
//!
//! Records per-key access history, derives frequency/recency/periodicity
//! signals, and produces a byte-budgeted [`PyWarmupPlan`] of which keys to
//! pre-load.

use oximedia_cache::cache_warming as core;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// AccessPattern
// ---------------------------------------------------------------------------

/// Historical access record for a single cache key.
#[pyclass(name = "AccessPattern")]
#[derive(Clone)]
pub struct PyAccessPattern {
    inner: core::AccessPattern,
}

#[pymethods]
impl PyAccessPattern {
    #[new]
    fn new(key: String, access_times: Vec<u64>, size_bytes: usize) -> Self {
        Self {
            inner: core::AccessPattern {
                key,
                access_times,
                size_bytes,
            },
        }
    }

    #[getter]
    fn key(&self) -> String {
        self.inner.key.clone()
    }

    #[getter]
    fn access_times(&self) -> Vec<u64> {
        self.inner.access_times.clone()
    }

    #[getter]
    fn size_bytes(&self) -> usize {
        self.inner.size_bytes
    }

    /// Accesses per hour over the recorded history (`0.0` with fewer than
    /// two timestamps).
    fn frequency_per_hour(&self) -> f64 {
        self.inner.frequency_per_hour()
    }

    /// Predicted next-access Unix timestamp via EMA-smoothed inter-arrival
    /// times, or `None` with fewer than two data points.
    fn predict_next_access(&self) -> Option<u64> {
        self.inner.predict_next_access()
    }

    /// Dominant periodic inter-arrival time in seconds (auto-correlation
    /// based), or `None` if no clear periodicity is found.
    fn periodicity_secs(&self) -> Option<f64> {
        self.inner.periodicity_secs()
    }

    fn __repr__(&self) -> String {
        format!(
            "AccessPattern(key={:?}, accesses={}, size_bytes={})",
            self.inner.key,
            self.inner.access_times.len(),
            self.inner.size_bytes
        )
    }
}

// ---------------------------------------------------------------------------
// WarmupPlan
// ---------------------------------------------------------------------------

/// Prioritised warm-up plan produced by [`PyCacheWarmer::plan_warmup`].
#[pyclass(name = "WarmupPlan")]
pub struct PyWarmupPlan {
    #[pyo3(get)]
    pub entries_to_warm: Vec<String>,
    #[pyo3(get)]
    pub estimated_bytes: usize,
    #[pyo3(get)]
    pub estimated_hit_improvement: f64,
}

impl From<core::WarmupPlan> for PyWarmupPlan {
    fn from(p: core::WarmupPlan) -> Self {
        Self {
            entries_to_warm: p.entries_to_warm,
            estimated_bytes: p.estimated_bytes,
            estimated_hit_improvement: p.estimated_hit_improvement,
        }
    }
}

#[pymethods]
impl PyWarmupPlan {
    fn __repr__(&self) -> String {
        format!(
            "WarmupPlan(entries={}, estimated_bytes={})",
            self.entries_to_warm.len(),
            self.estimated_bytes
        )
    }
}

// ---------------------------------------------------------------------------
// CacheWarmer
// ---------------------------------------------------------------------------

/// Predictive cache warmer: records accesses, then scores and ranks keys
/// for pre-loading by `frequency × recency × size_efficiency`.
#[pyclass(name = "CacheWarmer")]
pub struct PyCacheWarmer {
    inner: core::CacheWarmer,
}

#[pymethods]
impl PyCacheWarmer {
    #[new]
    fn new() -> Self {
        Self {
            inner: core::CacheWarmer::new(),
        }
    }

    /// Look-ahead window (seconds): only warm entries whose predicted next
    /// access falls within this many seconds of `current_time`. Defaults to
    /// 300 (5 minutes).
    #[getter]
    fn look_ahead_secs(&self) -> u64 {
        self.inner.look_ahead_secs
    }

    #[setter]
    fn set_look_ahead_secs(&mut self, value: u64) {
        self.inner.look_ahead_secs = value;
    }

    /// Minimum accesses/hour for a key to be considered worth warming.
    /// Defaults to 1.0.
    #[getter]
    fn min_frequency(&self) -> f64 {
        self.inner.min_frequency
    }

    #[setter]
    fn set_min_frequency(&mut self, value: f64) {
        self.inner.min_frequency = value;
    }

    /// Record an access to `key` at Unix time `time` (seconds).
    fn record_access(&mut self, key: &str, size_bytes: usize, time: u64) {
        self.inner.record_access(key, size_bytes, time);
    }

    /// Build a warm-up plan for `current_time`, fitting within
    /// `available_bytes`.
    fn plan_warmup(&self, current_time: u64, available_bytes: usize) -> PyWarmupPlan {
        self.inner.plan_warmup(current_time, available_bytes).into()
    }

    /// Top `n` hottest keys by descending accesses/hour.
    fn top_hot_keys(&self, n: usize) -> Vec<(String, f64)> {
        self.inner
            .top_hot_keys(n)
            .into_iter()
            .map(|(k, f)| (k.to_string(), f))
            .collect()
    }

    /// Snapshot of every recorded access pattern.
    fn patterns(&self) -> Vec<PyAccessPattern> {
        self.inner
            .patterns
            .iter()
            .cloned()
            .map(|inner| PyAccessPattern { inner })
            .collect()
    }

    fn __repr__(&self) -> String {
        format!("CacheWarmer(tracked_keys={})", self.inner.patterns.len())
    }
}

impl Default for PyCacheWarmer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAccessPattern>()?;
    m.add_class::<PyWarmupPlan>()?;
    m.add_class::<PyCacheWarmer>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_pattern_frequency_per_hour() {
        let p = PyAccessPattern::new("k".to_string(), vec![0, 720, 1440, 2160, 2880, 3600], 128);
        let freq = p.frequency_per_hour();
        assert!((freq - 6.0).abs() < 0.01, "expected ~6/h, got {freq}");
    }

    #[test]
    fn access_pattern_periodicity_detected() {
        let times: Vec<u64> = (0..20).map(|i| i * 600).collect();
        let p = PyAccessPattern::new("k".to_string(), times, 64);
        let period = p.periodicity_secs().expect("should detect periodicity");
        assert!((period - 600.0).abs() < 5.0);
    }

    #[test]
    fn cache_warmer_records_and_ranks_hot_keys() {
        let mut warmer = PyCacheWarmer::new();
        for t in [0u64, 3600] {
            warmer.record_access("cold", 64, t);
        }
        for i in 0..10u64 {
            warmer.record_access("hot", 64, i * 360);
        }
        let top = warmer.top_hot_keys(2);
        assert_eq!(top[0].0, "hot");
        assert_eq!(top[1].0, "cold");
        assert_eq!(warmer.patterns().len(), 2);
    }

    #[test]
    fn cache_warmer_plan_respects_budget() {
        let mut warmer = PyCacheWarmer::new();
        warmer.set_look_ahead_secs(10_000);
        warmer.set_min_frequency(0.1);
        for i in 0..5u64 {
            warmer.record_access("big", 5000, i * 1800);
            warmer.record_access("small", 100, i * 1800);
        }
        let plan = warmer.plan_warmup(10_000, 200);
        assert!(plan.estimated_bytes <= 200);
        assert!(!plan.entries_to_warm.contains(&"big".to_string()));
    }
}

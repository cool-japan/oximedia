//! `oximedia.analytics` audience-retention bindings — real delegation to
//! [`oximedia_analytics::retention`].
//!
//! Covers retention curves (batch and incremental/streaming), drop-off
//! detection, benchmark comparison, re-watch segment detection, and
//! per-segment (chapter) retention.

use oximedia_analytics::retention as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::analytics_py::PyViewerSession;

fn sessions_of(sessions: &[PyRef<'_, PyViewerSession>]) -> Vec<oximedia_analytics::ViewerSession> {
    sessions.iter().map(|s| s.inner.clone()).collect()
}

// ---------------------------------------------------------------------------
// RetentionCurve
// ---------------------------------------------------------------------------

/// A full audience-retention curve with aggregate counts.
#[pyclass(name = "RetentionCurve")]
#[derive(Clone)]
pub struct PyRetentionCurve {
    inner: core::RetentionCurve,
}

#[pymethods]
impl PyRetentionCurve {
    /// `(position_pct, retention_pct)` for every checkpoint bucket.
    #[getter]
    fn buckets(&self) -> Vec<(f32, f32)> {
        self.inner
            .buckets
            .iter()
            .map(|b| (b.position_pct, b.retention_pct))
            .collect()
    }

    #[getter]
    fn total_starts(&self) -> u32 {
        self.inner.total_starts
    }

    #[getter]
    fn completed_views(&self) -> u32 {
        self.inner.completed_views
    }

    /// Average fraction of content watched (0.0-100.0), via trapezoidal
    /// integration of the retention curve.
    fn average_view_duration(&self) -> f32 {
        core::average_view_duration(&self.inner)
    }

    /// Positions (`position_pct`) where retention drops by more than
    /// `threshold_pct_drop` between consecutive buckets.
    fn drop_off_points(&self, threshold_pct_drop: f32) -> Vec<f32> {
        core::drop_off_points(&self.inner, threshold_pct_drop)
    }

    /// Compare against a named reference benchmark (`"broadcast"`, `"vod"`,
    /// or `"short_form"`) and return a quality score in `0.0-100.0`.
    fn compare_to_benchmark(&self, benchmark: &str) -> PyResult<f32> {
        let bench = match benchmark {
            "broadcast" => core::broadcast_benchmark(),
            "vod" => core::vod_benchmark(),
            "short_form" => core::short_form_benchmark(),
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown benchmark {other:?}; expected 'broadcast', 'vod', or 'short_form'"
                )))
            }
        };
        Ok(core::compare_to_benchmark(&self.inner, &bench))
    }

    fn __repr__(&self) -> String {
        format!(
            "RetentionCurve(buckets={}, total_starts={}, completed_views={})",
            self.inner.buckets.len(),
            self.inner.total_starts,
            self.inner.completed_views
        )
    }
}

/// Compute an audience-retention curve from viewer sessions, sampling
/// `num_buckets` evenly-spaced checkpoints.
#[pyfunction]
fn compute_retention(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    content_duration_ms: u64,
    num_buckets: usize,
) -> PyRetentionCurve {
    let inner = core::compute_retention(&sessions_of(&sessions), content_duration_ms, num_buckets);
    PyRetentionCurve { inner }
}

/// Same result as [`compute_retention`], computed by processing `sessions`
/// in `chunk_size`-sized batches to bound peak memory usage.
#[pyfunction]
fn compute_retention_incremental(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    content_duration_ms: u64,
    num_buckets: usize,
    chunk_size: usize,
) -> PyRetentionCurve {
    let inner = core::compute_retention_incremental(
        &sessions_of(&sessions),
        content_duration_ms,
        num_buckets,
        chunk_size,
    );
    PyRetentionCurve { inner }
}

// ---------------------------------------------------------------------------
// IncrementalRetentionState
// ---------------------------------------------------------------------------

/// Streaming retention-curve accumulator: add sessions one at a time (or in
/// batches) and call [`finalise`](Self::finalise) whenever a snapshot is
/// needed.
#[pyclass(name = "IncrementalRetentionState")]
pub struct PyIncrementalRetentionState {
    inner: core::IncrementalRetentionState,
}

#[pymethods]
impl PyIncrementalRetentionState {
    #[new]
    fn new(content_duration_ms: u64, num_buckets: usize) -> PyResult<Self> {
        core::IncrementalRetentionState::new(content_duration_ms, num_buckets)
            .map(|inner| Self { inner })
            .ok_or_else(|| {
                PyValueError::new_err("content_duration_ms and num_buckets must both be non-zero")
            })
    }

    fn add_session(&mut self, session: PyRef<'_, PyViewerSession>) {
        self.inner.add_session(&session.inner);
    }

    fn add_sessions(&mut self, sessions: Vec<PyRef<'_, PyViewerSession>>) {
        self.inner.add_sessions(&sessions_of(&sessions));
    }

    /// Compute the retention curve for all sessions added so far. May be
    /// called multiple times.
    fn finalise(&self) -> PyRetentionCurve {
        PyRetentionCurve {
            inner: self.inner.finalise(),
        }
    }

    fn sessions_processed(&self) -> u32 {
        self.inner.sessions_processed()
    }

    fn __repr__(&self) -> String {
        format!(
            "IncrementalRetentionState(sessions_processed={})",
            self.inner.sessions_processed()
        )
    }
}

// ---------------------------------------------------------------------------
// Re-watch segments
// ---------------------------------------------------------------------------

/// Content intervals `(start_ms, end_ms)` that were watched, on average,
/// more than once per viewer (indicates a re-watch hotspot).
#[pyfunction]
fn re_watch_segments(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    content_duration_ms: u64,
) -> Vec<(u64, u64)> {
    core::re_watch_segments(&sessions_of(&sessions), content_duration_ms)
}

// ---------------------------------------------------------------------------
// Segment retention
// ---------------------------------------------------------------------------

/// Retention statistics for a single named content segment.
#[pyclass(name = "SegmentRetentionResult")]
pub struct PySegmentRetentionResult {
    #[pyo3(get)]
    pub segment_name: String,
    #[pyo3(get)]
    pub start_ms: u64,
    #[pyo3(get)]
    pub end_ms: u64,
    #[pyo3(get)]
    pub entry_retention_pct: f32,
    #[pyo3(get)]
    pub exit_retention_pct: f32,
    #[pyo3(get)]
    pub avg_segment_completion: f32,
    #[pyo3(get)]
    pub viewers_entered: u32,
    #[pyo3(get)]
    pub viewers_completed: u32,
}

impl From<core::SegmentRetentionResult> for PySegmentRetentionResult {
    fn from(r: core::SegmentRetentionResult) -> Self {
        Self {
            segment_name: r.segment_name,
            start_ms: r.start_ms,
            end_ms: r.end_ms,
            entry_retention_pct: r.entry_retention_pct,
            exit_retention_pct: r.exit_retention_pct,
            avg_segment_completion: r.avg_segment_completion,
            viewers_entered: r.viewers_entered,
            viewers_completed: r.viewers_completed,
        }
    }
}

#[pymethods]
impl PySegmentRetentionResult {
    fn __repr__(&self) -> String {
        format!(
            "SegmentRetentionResult(name={:?}, entry_retention_pct={:.1}, exit_retention_pct={:.1})",
            self.segment_name, self.entry_retention_pct, self.exit_retention_pct
        )
    }
}

/// Compute per-segment (chapter) retention statistics.
///
/// `segments` is `(name, start_ms, end_ms)`. Returns an empty list if
/// `sessions` or `segments` is empty, or `content_duration_ms` is zero.
#[pyfunction]
fn compute_segment_retention(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    segments: Vec<(String, u64, u64)>,
    content_duration_ms: u64,
) -> Vec<PySegmentRetentionResult> {
    let segments: Vec<core::ContentSegment> = segments
        .into_iter()
        .map(|(name, start_ms, end_ms)| core::ContentSegment {
            name,
            start_ms,
            end_ms,
        })
        .collect();
    core::compute_segment_retention(&sessions_of(&sessions), &segments, content_duration_ms)
        .into_iter()
        .map(PySegmentRetentionResult::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyRetentionCurve>()?;
    m.add_class::<PyIncrementalRetentionState>()?;
    m.add_class::<PySegmentRetentionResult>()?;
    m.add_function(wrap_pyfunction!(compute_retention, m)?)?;
    m.add_function(wrap_pyfunction!(compute_retention_incremental, m)?)?;
    m.add_function(wrap_pyfunction!(re_watch_segments, m)?)?;
    m.add_function(wrap_pyfunction!(compute_segment_retention, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// All public functions here take `Vec<PyRef<PyViewerSession>>`, which needs a
// live Python object (GIL) — covered end-to-end in
// `tests/analytics_families_smoke.rs`. `PyRetentionCurve`'s query methods
// (`average_view_duration`, `drop_off_points`, `compare_to_benchmark`) are
// exercised there too, against curves built from real sessions.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_curve_benchmark_rejects_unknown_name() {
        let curve = PyRetentionCurve {
            inner: core::RetentionCurve {
                buckets: vec![],
                total_starts: 0,
                completed_views: 0,
            },
        };
        assert!(curve.compare_to_benchmark("made_up").is_err());
        assert!(curve.compare_to_benchmark("vod").is_ok());
    }

    #[test]
    fn incremental_state_rejects_zero_params() {
        assert!(PyIncrementalRetentionState::new(0, 10).is_err());
        assert!(PyIncrementalRetentionState::new(10_000, 0).is_err());
        assert!(PyIncrementalRetentionState::new(10_000, 10).is_ok());
    }
}

//! `oximedia.analytics` session-replay, anomaly-detection, and
//! watch-time-attribution bindings — real delegation to
//! [`oximedia_analytics::replay`], [`oximedia_analytics::anomaly`], and
//! [`oximedia_analytics::attribution`]. Bundled into one file because the
//! parent crate documents and evolves them together (all three consume a
//! `ViewerSession`'s raw event stream and reduce it to a different signal).

use oximedia_analytics::anomaly as anomaly_core;
use oximedia_analytics::attribution as attribution_core;
use oximedia_analytics::replay as replay_core;
use oximedia_analytics::retention::ContentSegment;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::analytics_py::PyViewerSession;

// ---------------------------------------------------------------------------
// Session replay
// ---------------------------------------------------------------------------

/// Configuration for [`PyReplayReconstructor`].
#[pyclass(name = "ReplayConfig")]
#[derive(Clone)]
pub struct PyReplayConfig {
    inner: replay_core::ReplayConfig,
}

#[pymethods]
impl PyReplayConfig {
    /// If `interpolation_step_ms` is given, synthetic frames are inserted
    /// every that many wall-clock milliseconds while playing, producing a
    /// smooth timeline. `playback_rate` is how many ms of content advance
    /// per ms of wall-clock time (1.0 = real-time).
    #[new]
    #[pyo3(signature = (interpolation_step_ms=None, playback_rate=1.0))]
    fn new(interpolation_step_ms: Option<u64>, playback_rate: f64) -> Self {
        Self {
            inner: replay_core::ReplayConfig {
                interpolation_step_ms,
                playback_rate,
            },
        }
    }
}

/// One frame in a reconstructed session-replay timeline.
#[pyclass(name = "ReplayFrame")]
#[derive(Clone)]
pub struct PyReplayFrame {
    inner: replay_core::ReplayFrame,
}

#[pymethods]
impl PyReplayFrame {
    #[getter]
    fn wall_ms(&self) -> i64 {
        self.inner.wall_ms
    }

    #[getter]
    fn content_pos_ms(&self) -> u64 {
        self.inner.content_pos_ms
    }

    /// `"idle"`, `"playing"`, `"paused"`, `"buffering"`, or `"ended"`.
    #[getter]
    fn state(&self) -> String {
        self.inner.state.to_string().to_ascii_lowercase()
    }

    #[getter]
    fn quality_height(&self) -> Option<u32> {
        self.inner.quality_height
    }

    #[getter]
    fn bitrate_bps(&self) -> Option<u32> {
        self.inner.bitrate_bps
    }

    #[getter]
    fn event_kind(&self) -> String {
        self.inner.event_kind.clone()
    }

    #[getter]
    fn interpolated(&self) -> bool {
        self.inner.interpolated
    }

    #[getter]
    fn source_event_index(&self) -> Option<usize> {
        self.inner.source_event_index
    }

    fn __repr__(&self) -> String {
        format!(
            "ReplayFrame(wall_ms={}, state={:?}, content_pos_ms={})",
            self.inner.wall_ms,
            self.state(),
            self.inner.content_pos_ms
        )
    }
}

/// High-level QoE summary derived from a reconstructed replay.
#[pyclass(name = "ReplaySummary")]
pub struct PyReplaySummary {
    #[pyo3(get)]
    pub frame_count: usize,
    #[pyo3(get)]
    pub seek_count: u32,
    #[pyo3(get)]
    pub stall_count: u32,
    #[pyo3(get)]
    pub total_stall_ms: u64,
    #[pyo3(get)]
    pub quality_change_count: u32,
    #[pyo3(get)]
    pub min_quality_height: Option<u32>,
    #[pyo3(get)]
    pub max_quality_height: Option<u32>,
    /// `"idle"`, `"playing"`, `"paused"`, `"buffering"`, or `"ended"`.
    #[pyo3(get)]
    pub final_state: String,
    #[pyo3(get)]
    pub max_content_pos_ms: u64,
}

impl From<replay_core::ReplaySummary> for PyReplaySummary {
    fn from(s: replay_core::ReplaySummary) -> Self {
        Self {
            frame_count: s.frame_count,
            seek_count: s.seek_count,
            stall_count: s.stall_count,
            total_stall_ms: s.total_stall_ms,
            quality_change_count: s.quality_change_count,
            min_quality_height: s.min_quality_height,
            max_quality_height: s.max_quality_height,
            final_state: s.final_state.to_string().to_ascii_lowercase(),
            max_content_pos_ms: s.max_content_pos_ms,
        }
    }
}

#[pymethods]
impl PyReplaySummary {
    fn __repr__(&self) -> String {
        format!(
            "ReplaySummary(frame_count={}, seek_count={}, stall_count={})",
            self.frame_count, self.seek_count, self.stall_count
        )
    }
}

/// Reconstructs a structured replay timeline from a `ViewerSession`'s raw
/// playback events.
#[pyclass(name = "ReplayReconstructor")]
pub struct PyReplayReconstructor {
    inner: replay_core::ReplayReconstructor,
}

#[pymethods]
impl PyReplayReconstructor {
    #[new]
    fn new(config: &PyReplayConfig) -> Self {
        Self {
            inner: replay_core::ReplayReconstructor::new(config.inner.clone()),
        }
    }

    /// Reconstruct the ordered replay timeline for `session`. Errors when
    /// the session has no events.
    fn reconstruct(&self, session: PyRef<'_, PyViewerSession>) -> PyResult<Vec<PyReplayFrame>> {
        self.inner
            .reconstruct(&session.inner)
            .map(|frames| {
                frames
                    .into_iter()
                    .map(|inner| PyReplayFrame { inner })
                    .collect()
            })
            .map_err(super::analytics_err)
    }

    /// Compute a QoE summary from a previously reconstructed `frames`
    /// timeline and the originating `session`.
    #[staticmethod]
    fn summarise(
        frames: Vec<PyRef<'_, PyReplayFrame>>,
        session: PyRef<'_, PyViewerSession>,
    ) -> PyReplaySummary {
        let owned_frames: Vec<replay_core::ReplayFrame> =
            frames.iter().map(|f| f.inner.clone()).collect();
        replay_core::ReplayReconstructor::summarise(&owned_frames, &session.inner).into()
    }
}

// ---------------------------------------------------------------------------
// Anomaly detection
// ---------------------------------------------------------------------------

/// Rolling z-score anomaly detector over a fixed-size sliding window.
#[pyclass(name = "ZScoreDetector")]
pub struct PyZScoreDetector {
    inner: anomaly_core::ZScoreDetector,
}

#[pymethods]
impl PyZScoreDetector {
    /// Create a detector with the given window size and the default
    /// threshold of 3.0σ. Errors when `window < 2`.
    #[new]
    fn new(window: usize) -> PyResult<Self> {
        anomaly_core::ZScoreDetector::new(window)
            .map(|inner| Self { inner })
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Create a detector with a custom z-score `threshold`. Errors when
    /// `window < 2` or `threshold <= 0`.
    #[staticmethod]
    fn with_threshold(window: usize, threshold: f64) -> PyResult<Self> {
        anomaly_core::ZScoreDetector::with_threshold(window, threshold)
            .map(|inner| Self { inner })
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Feed a new value; returns the signed z-score when it exceeds the
    /// threshold, `None` otherwise (including during warm-up).
    fn update(&mut self, value: f64) -> Option<f64> {
        self.inner.update(value)
    }

    /// Current rolling mean, or `None` if the window is empty.
    fn mean(&self) -> Option<f64> {
        self.inner.mean()
    }

    fn window_len(&self) -> usize {
        self.inner.window_len()
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn __repr__(&self) -> String {
        format!("ZScoreDetector(window_len={})", self.inner.window_len())
    }
}

// ---------------------------------------------------------------------------
// Watch-time attribution
// ---------------------------------------------------------------------------

/// Attribution credit assigned to one content segment.
#[pyclass(name = "SegmentAttribution")]
pub struct PySegmentAttribution {
    #[pyo3(get)]
    pub segment_name: String,
    #[pyo3(get)]
    pub start_ms: u64,
    #[pyo3(get)]
    pub end_ms: u64,
    #[pyo3(get)]
    pub raw_credit: f64,
    /// Normalised credit in `[0.0, 1.0]`; sums to ~1.0 across all segments.
    #[pyo3(get)]
    pub normalised_credit: f64,
    #[pyo3(get)]
    pub reach_pct: f32,
}

impl From<attribution_core::SegmentAttribution> for PySegmentAttribution {
    fn from(a: attribution_core::SegmentAttribution) -> Self {
        Self {
            segment_name: a.segment_name,
            start_ms: a.start_ms,
            end_ms: a.end_ms,
            raw_credit: a.raw_credit,
            normalised_credit: a.normalised_credit,
            reach_pct: a.reach_pct,
        }
    }
}

#[pymethods]
impl PySegmentAttribution {
    fn __repr__(&self) -> String {
        format!(
            "SegmentAttribution(segment_name={:?}, normalised_credit={:.4})",
            self.segment_name, self.normalised_credit
        )
    }
}

fn parse_attribution_model(model: &str) -> PyResult<attribution_core::AttributionModel> {
    match model {
        "uniform" => Ok(attribution_core::AttributionModel::Uniform),
        "position_weighted" => Ok(attribution_core::AttributionModel::PositionWeighted),
        "engagement_weighted" => Ok(attribution_core::AttributionModel::EngagementWeighted),
        other => Err(PyValueError::new_err(format!(
            "unknown attribution model {other:?}; expected 'uniform', 'position_weighted', \
             or 'engagement_weighted'"
        ))),
    }
}

/// Compute watch-time attribution credit for each content segment.
///
/// * `segments` — ordered `(name, start_ms, end_ms)` chapters.
/// * `model` — `"uniform"`, `"position_weighted"`, or
///   `"engagement_weighted"`.
///
/// Errors when `sessions`/`segments` is empty, `content_duration_ms` is
/// zero, or `model` is unrecognised.
#[pyfunction]
fn compute_attribution(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    segments: Vec<(String, u64, u64)>,
    content_duration_ms: u64,
    model: &str,
) -> PyResult<Vec<PySegmentAttribution>> {
    let model = parse_attribution_model(model)?;
    let inner_sessions: Vec<oximedia_analytics::ViewerSession> =
        sessions.iter().map(|s| s.inner.clone()).collect();
    let segments: Vec<ContentSegment> = segments
        .into_iter()
        .map(|(name, start_ms, end_ms)| ContentSegment {
            name,
            start_ms,
            end_ms,
        })
        .collect();
    attribution_core::compute_attribution(&inner_sessions, &segments, content_duration_ms, model)
        .map(|attrs| attrs.into_iter().map(PySegmentAttribution::from).collect())
        .map_err(super::analytics_err)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyReplayConfig>()?;
    m.add_class::<PyReplayFrame>()?;
    m.add_class::<PyReplaySummary>()?;
    m.add_class::<PyReplayReconstructor>()?;
    m.add_class::<PyZScoreDetector>()?;
    m.add_class::<PySegmentAttribution>()?;
    m.add_function(wrap_pyfunction!(compute_attribution, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zscore_detector_flags_outlier() {
        let mut d = PyZScoreDetector::new(20).expect("valid window");
        for i in 0..19 {
            let _ = d.update(10.0 + (i % 2) as f64 * 0.01);
        }
        let result = d.update(1000.0);
        assert!(result.is_some());
        assert!(result.expect("outlier flagged") > 0.0);
    }

    #[test]
    fn zscore_detector_rejects_tiny_window() {
        assert!(PyZScoreDetector::new(1).is_err());
        assert!(PyZScoreDetector::with_threshold(5, 0.0).is_err());
    }

    #[test]
    fn zscore_detector_reset_clears_state() {
        let mut d = PyZScoreDetector::new(5).expect("valid");
        for _ in 0..5 {
            let _ = d.update(1.0);
        }
        d.reset();
        assert_eq!(d.window_len(), 0);
        assert!(d.mean().is_none());
    }

    #[test]
    fn attribution_rejects_unknown_model() {
        assert!(parse_attribution_model("bogus").is_err());
        assert!(parse_attribution_model("uniform").is_ok());
    }

    // `compute_attribution`, `ReplayReconstructor.reconstruct`, and
    // `ReplayReconstructor.summarise` take `PyRef<PyViewerSession>` /
    // `Vec<PyRef<...>>`, which need a live Python object (GIL) — covered
    // end-to-end in `tests/analytics_families_smoke.rs`.
}

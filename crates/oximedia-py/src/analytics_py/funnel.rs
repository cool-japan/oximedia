//! `oximedia.analytics` funnel / churn / loyalty bindings — real delegation
//! to [`oximedia_analytics::funnel`].
//!
//! Covers three related analyses:
//! * **Funnel analysis** — viewer progression through content milestones
//!   ([`compute_funnel`]) or an event-driven step sequence
//!   ([`funnel_analyze`]).
//! * **Churn prediction** — linear-regression slope over an engagement-score
//!   time-series.
//! * **Loyalty scoring** — recency/frequency/duration composite score.

use oximedia_analytics::funnel as core;
use pyo3::prelude::*;

use super::analytics_err;
use crate::analytics_py::PyViewerSession;

// ---------------------------------------------------------------------------
// Milestone funnel (compute_funnel)
// ---------------------------------------------------------------------------

/// One step of a computed milestone funnel.
#[pyclass(name = "FunnelStep")]
#[derive(Clone)]
pub struct PyFunnelStep {
    #[pyo3(get)]
    pub milestone_name: String,
    #[pyo3(get)]
    pub position_ms: u64,
    #[pyo3(get)]
    pub viewers_reached: u32,
    #[pyo3(get)]
    pub conversion_from_prev: f32,
    #[pyo3(get)]
    pub overall_rate: f32,
}

impl From<&core::FunnelStep> for PyFunnelStep {
    fn from(s: &core::FunnelStep) -> Self {
        Self {
            milestone_name: s.milestone_name.clone(),
            position_ms: s.position_ms,
            viewers_reached: s.viewers_reached,
            conversion_from_prev: s.conversion_from_prev,
            overall_rate: s.overall_rate,
        }
    }
}

#[pymethods]
impl PyFunnelStep {
    fn __repr__(&self) -> String {
        format!(
            "FunnelStep(name={:?}, viewers_reached={}, overall_rate={:.3})",
            self.milestone_name, self.viewers_reached, self.overall_rate
        )
    }
}

/// Result of a milestone funnel analysis.
#[pyclass(name = "FunnelResult")]
pub struct PyFunnelResult {
    inner: core::FunnelResult,
}

#[pymethods]
impl PyFunnelResult {
    #[getter]
    fn total_starters(&self) -> u32 {
        self.inner.total_starters
    }

    fn steps(&self) -> Vec<PyFunnelStep> {
        self.inner.steps.iter().map(PyFunnelStep::from).collect()
    }

    /// Fraction of session starters reaching the last milestone.
    fn completion_rate(&self) -> f32 {
        self.inner.completion_rate()
    }

    /// Index of the step with the largest absolute drop-off, or `None` when
    /// there are fewer than two steps.
    fn biggest_drop_step(&self) -> Option<usize> {
        self.inner.biggest_drop_step()
    }

    fn __repr__(&self) -> String {
        format!(
            "FunnelResult(steps={}, total_starters={})",
            self.inner.steps.len(),
            self.inner.total_starters
        )
    }
}

/// Compute a viewer funnel from sessions against content-position
/// milestones. `milestones` is `(name, position_ms)`, ascending by
/// `position_ms`. Each milestone is independent — reaching a later one does
/// not require having reached an earlier one.
#[pyfunction]
fn compute_funnel(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    milestones: Vec<(String, u64)>,
    content_duration_ms: u64,
) -> PyResult<PyFunnelResult> {
    // `funnel` is a descendant module of `analytics_py`, so it may read the
    // module-private `PyViewerSession::inner` field directly (same rule the
    // parent module's own `attention_heatmap`/`compute_engagement` rely on).
    let inner_sessions: Vec<oximedia_analytics::ViewerSession> =
        sessions.iter().map(|s| s.inner.clone()).collect();
    let milestones: Vec<core::FunnelMilestone> = milestones
        .into_iter()
        .map(|(name, position_ms)| core::FunnelMilestone { name, position_ms })
        .collect();
    core::compute_funnel(&inner_sessions, &milestones, content_duration_ms)
        .map(|inner| PyFunnelResult { inner })
        .map_err(analytics_err)
}

// ---------------------------------------------------------------------------
// Churn prediction
// ---------------------------------------------------------------------------

/// Configuration for [`predict_churn`].
#[pyclass(name = "ChurnConfig")]
#[derive(Clone)]
pub struct PyChurnConfig {
    inner: core::ChurnConfig,
}

#[pymethods]
impl PyChurnConfig {
    #[new]
    #[pyo3(signature = (min_data_points=3, decline_slope_threshold=-1e-9, low_engagement_threshold=0.2))]
    fn new(
        min_data_points: usize,
        decline_slope_threshold: f32,
        low_engagement_threshold: f32,
    ) -> Self {
        Self {
            inner: core::ChurnConfig {
                min_data_points,
                decline_slope_threshold,
                low_engagement_threshold,
            },
        }
    }

    #[getter]
    fn min_data_points(&self) -> usize {
        self.inner.min_data_points
    }

    #[getter]
    fn decline_slope_threshold(&self) -> f32 {
        self.inner.decline_slope_threshold
    }

    #[getter]
    fn low_engagement_threshold(&self) -> f32 {
        self.inner.low_engagement_threshold
    }
}

/// Churn risk assessment for a single viewer.
#[pyclass(name = "ChurnAssessment")]
pub struct PyChurnAssessment {
    #[pyo3(get)]
    pub viewer_id: String,
    /// `"low"`, `"medium"`, or `"high"`.
    #[pyo3(get)]
    pub risk: String,
    #[pyo3(get)]
    pub engagement_slope: f32,
    #[pyo3(get)]
    pub latest_score: f32,
}

impl From<core::ChurnAssessment> for PyChurnAssessment {
    fn from(a: core::ChurnAssessment) -> Self {
        let risk = match a.risk {
            core::ChurnRisk::Low => "low",
            core::ChurnRisk::Medium => "medium",
            core::ChurnRisk::High => "high",
        };
        Self {
            viewer_id: a.viewer_id,
            risk: risk.to_string(),
            engagement_slope: a.engagement_slope,
            latest_score: a.latest_score,
        }
    }
}

#[pymethods]
impl PyChurnAssessment {
    fn __repr__(&self) -> String {
        format!(
            "ChurnAssessment(viewer_id={:?}, risk={:?}, latest_score={:.3})",
            self.viewer_id, self.risk, self.latest_score
        )
    }
}

/// Predict churn risk from an engagement-score time-series
/// (`(unix_epoch_ms, score)` pairs, scores in `[0.0, 1.0]`).
///
/// Errors when fewer than `config.min_data_points` points are given.
#[pyfunction]
fn predict_churn(
    viewer_id: &str,
    scores_over_time: Vec<(i64, f32)>,
    config: &PyChurnConfig,
) -> PyResult<PyChurnAssessment> {
    core::predict_churn(viewer_id, &scores_over_time, &config.inner)
        .map(PyChurnAssessment::from)
        .map_err(analytics_err)
}

// ---------------------------------------------------------------------------
// Loyalty scoring
// ---------------------------------------------------------------------------

/// Weights for the recency-frequency-duration loyalty model.
#[pyclass(name = "LoyaltyWeights")]
#[derive(Clone)]
pub struct PyLoyaltyWeights {
    inner: core::LoyaltyWeights,
}

#[pymethods]
impl PyLoyaltyWeights {
    #[new]
    #[pyo3(signature = (recency=0.35, frequency=0.35, duration=0.30))]
    fn new(recency: f32, frequency: f32, duration: f32) -> Self {
        Self {
            inner: core::LoyaltyWeights {
                recency,
                frequency,
                duration,
            },
        }
    }
}

/// Final loyalty assessment for a viewer.
#[pyclass(name = "LoyaltyScore")]
pub struct PyLoyaltyScore {
    #[pyo3(get)]
    pub viewer_id: String,
    #[pyo3(get)]
    pub score: f32,
    #[pyo3(get)]
    pub recency_score: f32,
    #[pyo3(get)]
    pub frequency_score: f32,
    #[pyo3(get)]
    pub duration_score: f32,
}

impl From<core::LoyaltyScore> for PyLoyaltyScore {
    fn from(s: core::LoyaltyScore) -> Self {
        Self {
            viewer_id: s.viewer_id,
            score: s.score,
            recency_score: s.components.recency_score,
            frequency_score: s.components.frequency_score,
            duration_score: s.components.duration_score,
        }
    }
}

#[pymethods]
impl PyLoyaltyScore {
    fn __repr__(&self) -> String {
        format!(
            "LoyaltyScore(viewer_id={:?}, score={:.3})",
            self.viewer_id, self.score
        )
    }
}

/// Compute a viewer's loyalty score from their session history.
///
/// `session_starts_ms` and `watch_durations_ms` must have equal length.
#[allow(clippy::too_many_arguments)]
#[pyfunction]
fn compute_loyalty(
    viewer_id: &str,
    session_starts_ms: Vec<i64>,
    watch_durations_ms: Vec<u64>,
    now_ms: i64,
    recency_window_ms: i64,
    freq_cap: usize,
    max_duration_ms: u64,
    weights: &PyLoyaltyWeights,
) -> PyResult<PyLoyaltyScore> {
    core::compute_loyalty(
        viewer_id,
        &session_starts_ms,
        &watch_durations_ms,
        now_ms,
        recency_window_ms,
        freq_cap,
        max_duration_ms,
        &weights.inner,
    )
    .map(PyLoyaltyScore::from)
    .map_err(analytics_err)
}

// ---------------------------------------------------------------------------
// Event-driven funnel (FunnelAnalyzer)
// ---------------------------------------------------------------------------

/// Report produced by [`funnel_analyze`].
#[pyclass(name = "FunnelReport")]
pub struct PyFunnelReport {
    inner: core::FunnelReport,
}

#[pymethods]
impl PyFunnelReport {
    #[getter]
    fn step_completions(&self) -> Vec<u64> {
        self.inner.step_completions.clone()
    }

    #[getter]
    fn conversion_rates(&self) -> Vec<f64> {
        self.inner.conversion_rates.clone()
    }

    #[getter]
    fn drop_offs(&self) -> Vec<f64> {
        self.inner.drop_offs.clone()
    }

    /// Fraction of users reaching step 0 who also reached the final step.
    fn overall_completion_rate(&self) -> f64 {
        self.inner.overall_completion_rate()
    }

    fn __repr__(&self) -> String {
        format!("FunnelReport(steps={})", self.inner.step_completions.len())
    }
}

/// Analyse an event-driven conversion funnel.
///
/// * `events` — `(user_id, event_type, timestamp_ms)`, any order.
/// * `steps` — ordered `(step_name, event_type)` pairs users must complete
///   in sequence.
/// * `max_time_between_steps_ms` — if a user takes longer than this between
///   two consecutive steps, their progress resets to step 0.
#[pyfunction]
fn funnel_analyze(
    events: Vec<(String, String, u64)>,
    steps: Vec<(String, String)>,
    max_time_between_steps_ms: u64,
) -> PyFunnelReport {
    let events: Vec<core::SessionEvent> = events
        .into_iter()
        .map(|(user_id, event_type, timestamp_ms)| core::SessionEvent {
            user_id,
            event_type,
            timestamp_ms,
        })
        .collect();
    let definition = core::FunnelDefinition {
        steps: steps
            .into_iter()
            .map(|(name, event_type)| core::FunnelStepDef { name, event_type })
            .collect(),
        max_time_between_steps_ms,
    };
    let inner = core::FunnelAnalyzer::analyze(&events, &definition);
    PyFunnelReport { inner }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFunnelStep>()?;
    m.add_class::<PyFunnelResult>()?;
    m.add_class::<PyChurnConfig>()?;
    m.add_class::<PyChurnAssessment>()?;
    m.add_class::<PyLoyaltyWeights>()?;
    m.add_class::<PyLoyaltyScore>()?;
    m.add_class::<PyFunnelReport>()?;
    m.add_function(wrap_pyfunction!(compute_funnel, m)?)?;
    m.add_function(wrap_pyfunction!(predict_churn, m)?)?;
    m.add_function(wrap_pyfunction!(compute_loyalty, m)?)?;
    m.add_function(wrap_pyfunction!(funnel_analyze, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // `compute_funnel` takes `Vec<PyRef<PyViewerSession>>`, which needs a live
    // Python object (GIL) — covered in `tests/analytics_families_smoke.rs`.

    #[test]
    fn predict_churn_high_risk_on_decline() {
        let scores: Vec<(i64, f32)> = (0..10)
            .map(|i| (i as i64 * 7 * 86_400_000, 1.0 - i as f32 * 0.09))
            .collect();
        let config = PyChurnConfig::new(3, -1e-9, 0.2);
        let result = predict_churn("v1", scores, &config).expect("should succeed");
        assert_ne!(result.risk, "low");
    }

    #[test]
    fn predict_churn_insufficient_data_errors() {
        let config = PyChurnConfig::new(3, -1e-9, 0.2);
        let err = predict_churn("v", vec![(0, 0.5), (1, 0.4)], &config);
        assert!(err.is_err());
    }

    #[test]
    fn compute_loyalty_perfect_viewer_scores_high() {
        let now_ms = 10 * 86_400_000i64;
        let starts: Vec<i64> = (0..10).map(|i| now_ms - i * 3_600_000).collect();
        let durations = vec![1_800_000u64; 10];
        let weights = PyLoyaltyWeights::new(0.35, 0.35, 0.30);
        let score = compute_loyalty(
            "v1",
            starts,
            durations,
            now_ms,
            7 * 86_400_000,
            10,
            3_600_000,
            &weights,
        )
        .expect("should succeed");
        assert!(score.score > 0.8, "score={}", score.score);
    }

    #[test]
    fn compute_loyalty_mismatched_lengths_errors() {
        let weights = PyLoyaltyWeights::new(0.35, 0.35, 0.30);
        let err = compute_loyalty(
            "v",
            vec![0, 1],
            vec![1000],
            1000,
            86_400_000,
            10,
            3_600_000,
            &weights,
        );
        assert!(err.is_err());
    }

    #[test]
    fn funnel_analyze_partial_conversion() {
        let events = vec![
            ("u1".to_string(), "view".to_string(), 0u64),
            ("u1".to_string(), "purchase".to_string(), 5_000),
            ("u2".to_string(), "view".to_string(), 0),
        ];
        let steps = vec![
            ("view".to_string(), "view".to_string()),
            ("purchase".to_string(), "purchase".to_string()),
        ];
        let report = funnel_analyze(events, steps, 300_000);
        assert_eq!(report.step_completions(), vec![2, 1]);
        assert!((report.conversion_rates()[1] - 0.5).abs() < 1e-9);
        assert!((report.overall_completion_rate() - 0.5).abs() < 1e-9);
    }
}

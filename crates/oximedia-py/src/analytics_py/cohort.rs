//! `oximedia.analytics` cohort-analysis bindings — real delegation to
//! [`oximedia_analytics::cohort`].
//!
//! Groups viewers by first-view period (day/week/month) and tracks how each
//! cohort's retention evolves over subsequent periods; also exposes the
//! simpler day-N retention-curve helper for a single pre-defined cohort.

use oximedia_analytics::cohort as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

fn parse_window(window: &str) -> PyResult<core::CohortWindow> {
    match window {
        "day" => Ok(core::CohortWindow::Day),
        "week" => Ok(core::CohortWindow::Week),
        "month" => Ok(core::CohortWindow::Month),
        other => Err(PyValueError::new_err(format!(
            "unknown cohort window {other:?}; expected 'day', 'week', or 'month'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// CohortMatrix
// ---------------------------------------------------------------------------

/// Full cohort-retention matrix for a set of viewer events.
#[pyclass(name = "CohortMatrix")]
pub struct PyCohortMatrix {
    inner: core::CohortMatrix,
}

#[pymethods]
impl PyCohortMatrix {
    /// The cohort window granularity: `"day"`, `"week"`, or `"month"`.
    #[getter]
    fn window(&self) -> &'static str {
        match self.inner.window {
            core::CohortWindow::Day => "day",
            core::CohortWindow::Week => "week",
            core::CohortWindow::Month => "month",
        }
    }

    #[getter]
    fn num_periods(&self) -> u32 {
        self.inner.num_periods
    }

    /// `(window_start_ms, viewer_ids)` for every cohort, sorted by
    /// `window_start_ms` ascending.
    fn cohorts(&self) -> Vec<(i64, Vec<String>)> {
        self.inner
            .cohorts
            .iter()
            .map(|c| (c.window_start_ms, c.viewer_ids.clone()))
            .collect()
    }

    /// `(cohort_window_ms, period_offset, active_viewers, retention_rate)`
    /// for every cell in the matrix.
    fn cells(&self) -> Vec<(i64, u32, u32, f32)> {
        self.inner
            .cells
            .iter()
            .map(|c| {
                (
                    c.cohort_window_ms,
                    c.period_offset,
                    c.active_viewers,
                    c.retention_rate,
                )
            })
            .collect()
    }

    /// Retention rate for a specific cohort + period offset, or `None` if no
    /// matching cell exists.
    fn retention_at(&self, cohort_window_ms: i64, period_offset: u32) -> Option<f32> {
        self.inner.retention_at(cohort_window_ms, period_offset)
    }

    /// Average retention at `period_offset` across all cohorts, weighted by
    /// cohort size.
    fn average_retention_at_period(&self, period_offset: u32) -> f32 {
        self.inner.average_retention_at_period(period_offset)
    }

    fn __repr__(&self) -> String {
        format!(
            "CohortMatrix(window={:?}, cohorts={}, num_periods={})",
            self.window(),
            self.inner.cohorts.len(),
            self.inner.num_periods
        )
    }
}

/// Build a cohort-retention matrix from `(viewer_id, event_ms)` events.
///
/// `window` must be `"day"`, `"week"`, or `"month"`. `num_periods` is how
/// many periods after the first-view period to track.
#[pyfunction]
fn build_cohort_matrix(
    events: Vec<(String, i64)>,
    window: &str,
    num_periods: u32,
) -> PyResult<PyCohortMatrix> {
    let window = parse_window(window)?;
    let events: Vec<core::ViewerEvent> = events
        .into_iter()
        .map(|(viewer_id, event_ms)| core::ViewerEvent {
            viewer_id,
            event_ms,
        })
        .collect();
    core::build_cohort_matrix(&events, window, num_periods)
        .map(|inner| PyCohortMatrix { inner })
        .map_err(super::analytics_err)
}

/// Compute day-N retention rates for a single pre-defined cohort.
///
/// `events` is `(user_id, timestamp_ms)`. Returns a list of length
/// `periods + 1` where index `i` is the fraction of `users` active on day
/// `i` relative to `cohort_date`. Returns all-zero when `users` is empty.
#[pyfunction]
fn cohort_retention_curve(
    cohort_date: u64,
    users: Vec<String>,
    events: Vec<(String, u64)>,
    periods: u32,
) -> Vec<f64> {
    let cohort = core::CohortDefinition { cohort_date, users };
    let events: Vec<core::UserEvent> = events
        .into_iter()
        .map(|(user_id, timestamp_ms)| core::UserEvent {
            user_id,
            timestamp_ms,
        })
        .collect();
    core::CohortAnalyzer::retention_curve(&cohort, &events, periods)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCohortMatrix>()?;
    m.add_function(wrap_pyfunction!(build_cohort_matrix, m)?)?;
    m.add_function(wrap_pyfunction!(cohort_retention_curve, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 86_400_000;

    #[test]
    fn build_cohort_matrix_basic_retention() {
        let events = vec![
            ("alice".to_string(), 0),
            ("bob".to_string(), 0),
            ("alice".to_string(), DAY_MS),
        ];
        let matrix = build_cohort_matrix(events, "day", 2).expect("should build");
        assert_eq!(matrix.cohorts().len(), 1);
        let r0 = matrix.retention_at(0, 0).expect("period 0 present");
        assert!((r0 - 1.0).abs() < 1e-6);
        let r1 = matrix.retention_at(0, 1).expect("period 1 present");
        assert!((r1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn build_cohort_matrix_rejects_unknown_window() {
        let err = build_cohort_matrix(vec![("a".to_string(), 0)], "fortnight", 1);
        assert!(err.is_err());
    }

    #[test]
    fn build_cohort_matrix_empty_events_errors() {
        let err = build_cohort_matrix(vec![], "day", 1);
        assert!(err.is_err());
    }

    #[test]
    fn cohort_retention_curve_gradual_decay() {
        let users = vec!["u0".to_string(), "u1".to_string()];
        let events = vec![
            ("u0".to_string(), 0u64),
            ("u0".to_string(), 86_400_000),
            ("u1".to_string(), 0u64),
        ];
        let curve = cohort_retention_curve(0, users, events, 1);
        assert_eq!(curve.len(), 2);
        assert!((curve[0] - 1.0).abs() < 1e-9);
        assert!((curve[1] - 0.5).abs() < 1e-9);
    }
}

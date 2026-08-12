//! `oximedia.analytics` A/B testing bindings — real delegation to
//! [`oximedia_analytics::ab_testing`].
//!
//! Exposes deterministic (FNV-1a hash based) variant assignment, per-variant
//! metric collection, frequentist winner selection (two-proportion z-test),
//! and Bayesian A/B testing (Beta-Binomial conjugacy + Monte Carlo).

use oximedia_analytics::ab_testing as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use super::analytics_err;

/// Metric names accepted by [`PyAbExperimentResults::winning_variant`] /
/// [`PyAbExperimentResults::winning_variant_with_alpha`]. Unlike the
/// underlying Rust helper (which silently falls back to `"ctr"` for an
/// unrecognised string), the Python binding rejects unknown metrics with a
/// `ValueError` — a typo must never be misread as CTR.
const WINNER_METRICS: &[&str] = &["ctr", "conversion", "completion", "watch_duration"];
/// Metric names accepted by [`PyAbExperimentResults::bayesian_winner`].
const BAYESIAN_METRICS: &[&str] = &["ctr", "click", "conversion", "completion"];

fn validate_metric(metric: &str, allowed: &[&str]) -> PyResult<()> {
    if allowed.contains(&metric) {
        Ok(())
    } else {
        Err(PyValueError::new_err(format!(
            "unknown metric {metric:?}; expected one of {allowed:?}"
        )))
    }
}

// ---------------------------------------------------------------------------
// AbVariant
// ---------------------------------------------------------------------------

/// One treatment arm in an A/B experiment.
#[pyclass(name = "AbVariant")]
#[derive(Clone)]
pub struct PyAbVariant {
    pub(crate) inner: core::Variant,
}

#[pymethods]
impl PyAbVariant {
    #[new]
    fn new(id: String, name: String, allocation_weight: f32) -> Self {
        Self {
            inner: core::Variant {
                id,
                name,
                allocation_weight,
            },
        }
    }

    #[getter]
    fn id(&self) -> String {
        self.inner.id.clone()
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[getter]
    fn allocation_weight(&self) -> f32 {
        self.inner.allocation_weight
    }

    fn __repr__(&self) -> String {
        format!(
            "AbVariant(id={:?}, name={:?}, allocation_weight={})",
            self.inner.id, self.inner.name, self.inner.allocation_weight
        )
    }
}

// ---------------------------------------------------------------------------
// AbExperiment
// ---------------------------------------------------------------------------

/// A configured A/B experiment: variants + scheduling metadata.
#[pyclass(name = "AbExperiment")]
#[derive(Clone)]
pub struct PyAbExperiment {
    pub(crate) inner: core::Experiment,
}

#[pymethods]
impl PyAbExperiment {
    #[new]
    #[pyo3(signature = (id, name, variants, start_ms, end_ms=None, min_sample_size=0))]
    fn new(
        id: String,
        name: String,
        variants: Vec<PyRef<'_, PyAbVariant>>,
        start_ms: i64,
        end_ms: Option<i64>,
        min_sample_size: u32,
    ) -> Self {
        Self {
            inner: core::Experiment {
                id,
                name,
                variants: variants.iter().map(|v| v.inner.clone()).collect(),
                start_ms,
                end_ms,
                min_sample_size,
            },
        }
    }

    #[getter]
    fn id(&self) -> String {
        self.inner.id.clone()
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[getter]
    fn start_ms(&self) -> i64 {
        self.inner.start_ms
    }

    #[getter]
    fn end_ms(&self) -> Option<i64> {
        self.inner.end_ms
    }

    #[getter]
    fn min_sample_size(&self) -> u32 {
        self.inner.min_sample_size
    }

    /// IDs of every configured variant, in definition order.
    fn variant_ids(&self) -> Vec<String> {
        self.inner.variants.iter().map(|v| v.id.clone()).collect()
    }

    /// Deterministically assign `user_id` to a variant via FNV-1a hashing of
    /// the user ID — the same user always lands in the same variant, and
    /// variants with a higher `allocation_weight` receive proportionally
    /// more users. Returns the assigned variant's ID.
    fn assign_variant(&self, user_id: &str) -> PyResult<String> {
        core::assign_variant(&self.inner, user_id, core::AssignmentMethod::Deterministic)
            .map(|v| v.id.clone())
            .map_err(analytics_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "AbExperiment(id={:?}, variants={})",
            self.inner.id,
            self.inner.variants.len()
        )
    }
}

// ---------------------------------------------------------------------------
// AbVariantMetrics
// ---------------------------------------------------------------------------

/// Collected metrics for a single experiment variant.
#[pyclass(name = "AbVariantMetrics")]
#[derive(Clone)]
pub struct PyAbVariantMetrics {
    pub(crate) inner: core::VariantMetrics,
}

#[pymethods]
impl PyAbVariantMetrics {
    #[getter]
    fn variant_id(&self) -> String {
        self.inner.variant_id.clone()
    }

    #[getter]
    fn impressions(&self) -> u32 {
        self.inner.impressions
    }

    #[getter]
    fn clicks(&self) -> u32 {
        self.inner.clicks
    }

    #[getter]
    fn conversions(&self) -> u32 {
        self.inner.conversions
    }

    #[getter]
    fn watch_duration_sum_ms(&self) -> u64 {
        self.inner.watch_duration_sum_ms
    }

    #[getter]
    fn completion_count(&self) -> u32 {
        self.inner.completion_count
    }

    /// `clicks / impressions` (0.0 if no impressions).
    fn click_through_rate(&self) -> f32 {
        core::click_through_rate(&self.inner)
    }

    /// `conversions / impressions` (0.0 if no impressions).
    fn conversion_rate(&self) -> f32 {
        core::conversion_rate(&self.inner)
    }

    /// Average watch duration per impression in milliseconds.
    fn average_watch_duration(&self) -> f32 {
        core::average_watch_duration(&self.inner)
    }

    /// `completion_count / impressions`.
    fn completion_rate(&self) -> f32 {
        core::completion_rate(&self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "AbVariantMetrics(variant_id={:?}, impressions={}, clicks={})",
            self.inner.variant_id, self.inner.impressions, self.inner.clicks
        )
    }
}

// ---------------------------------------------------------------------------
// BayesianAbResult
// ---------------------------------------------------------------------------

/// Result of a Bayesian A/B comparison between two variants.
#[pyclass(name = "BayesianAbResult")]
pub struct PyBayesianAbResult {
    #[pyo3(get)]
    pub variant_a_id: String,
    #[pyo3(get)]
    pub variant_b_id: String,
    /// Estimated probability that variant B has a higher true rate than A.
    #[pyo3(get)]
    pub prob_b_beats_a: f64,
    #[pyo3(get)]
    pub expected_uplift: f64,
    #[pyo3(get)]
    pub posterior_mean_a: f64,
    #[pyo3(get)]
    pub posterior_mean_b: f64,
}

impl From<core::BayesianAbResult> for PyBayesianAbResult {
    fn from(r: core::BayesianAbResult) -> Self {
        Self {
            variant_a_id: r.variant_a_id,
            variant_b_id: r.variant_b_id,
            prob_b_beats_a: r.prob_b_beats_a,
            expected_uplift: r.expected_uplift,
            posterior_mean_a: r.posterior_mean_a,
            posterior_mean_b: r.posterior_mean_b,
        }
    }
}

#[pymethods]
impl PyBayesianAbResult {
    fn __repr__(&self) -> String {
        format!(
            "BayesianAbResult(a={:?}, b={:?}, prob_b_beats_a={:.4})",
            self.variant_a_id, self.variant_b_id, self.prob_b_beats_a
        )
    }
}

// ---------------------------------------------------------------------------
// AbExperimentResults
// ---------------------------------------------------------------------------

/// Aggregate experiment results keyed by variant ID; records impressions,
/// clicks, conversions, completions, and watch time per variant, and
/// computes frequentist / Bayesian winners.
#[pyclass(name = "AbExperimentResults")]
pub struct PyAbExperimentResults {
    inner: core::ExperimentResults,
}

#[pymethods]
impl PyAbExperimentResults {
    #[new]
    fn new(experiment: PyRef<'_, PyAbExperiment>) -> Self {
        Self {
            inner: core::ExperimentResults::new(experiment.inner.clone()),
        }
    }

    fn record_impression(&mut self, variant_id: &str) {
        self.inner.record_impression(variant_id);
    }

    fn record_click(&mut self, variant_id: &str) {
        self.inner.record_click(variant_id);
    }

    fn record_conversion(&mut self, variant_id: &str) {
        self.inner.record_conversion(variant_id);
    }

    fn record_completion(&mut self, variant_id: &str, watch_duration_ms: u64) {
        self.inner.record_completion(variant_id, watch_duration_ms);
    }

    fn record_watch(&mut self, variant_id: &str, watch_duration_ms: u64) {
        self.inner.record_watch(variant_id, watch_duration_ms);
    }

    /// Current metrics snapshot for `variant_id`, or `None` if unknown.
    fn metrics(&self, variant_id: &str) -> Option<PyAbVariantMetrics> {
        self.inner
            .variant_metrics
            .get(variant_id)
            .cloned()
            .map(|inner| PyAbVariantMetrics { inner })
    }

    /// Winning variant ID at the default significance level (α = 0.05), or
    /// `None` if no variant has data or no winner is statistically
    /// significant.
    ///
    /// `metric` must be one of `"ctr"`, `"conversion"`, `"completion"`,
    /// `"watch_duration"`.
    fn winning_variant(&self, metric: &str) -> PyResult<Option<String>> {
        validate_metric(metric, WINNER_METRICS)?;
        Ok(core::winning_variant(&self.inner, metric).map(str::to_string))
    }

    /// Winning variant ID at significance level `alpha`
    /// (supported: 0.10, 0.05, 0.01, 0.001 — other values snap to the
    /// nearest supported level), or `None` if none is significant.
    fn winning_variant_with_alpha(&self, metric: &str, alpha: f32) -> PyResult<Option<String>> {
        validate_metric(metric, WINNER_METRICS)?;
        Ok(core::winning_variant_with_alpha(&self.inner, metric, alpha).map(str::to_string))
    }

    /// Bayesian A/B test between `variant_a_id` and `variant_b_id` using
    /// Beta-Binomial conjugacy with a Jeffreys prior; `prob_b_beats_a` is a
    /// Monte-Carlo estimate over `num_samples` draws seeded by `rng_seed`
    /// (deterministic/reproducible for a fixed seed).
    ///
    /// `metric` must be one of `"ctr"`, `"click"`, `"conversion"`,
    /// `"completion"`.
    fn bayesian_winner(
        &self,
        variant_a_id: &str,
        variant_b_id: &str,
        metric: &str,
        num_samples: usize,
        rng_seed: u64,
    ) -> PyResult<PyBayesianAbResult> {
        validate_metric(metric, BAYESIAN_METRICS)?;
        core::bayesian_winner(
            &self.inner,
            variant_a_id,
            variant_b_id,
            metric,
            num_samples,
            rng_seed,
        )
        .map(PyBayesianAbResult::from)
        .map_err(analytics_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "AbExperimentResults(experiment_id={:?}, variants={})",
            self.inner.experiment.id,
            self.inner.variant_metrics.len()
        )
    }
}

// ---------------------------------------------------------------------------
// Module-level statistical helpers
// ---------------------------------------------------------------------------

/// Two-proportion z-score for rates `p1` (from `n1` observations) and `p2`
/// (from `n2` observations). Returns `0.0` on degenerate input (empty
/// sample, or a pooled proportion of exactly 0 or 1).
#[pyfunction]
fn z_test(p1: f32, n1: u32, p2: f32, n2: u32) -> f32 {
    core::z_test(p1, n1, p2, n2)
}

/// Whether `z_score` is statistically significant at significance level
/// `alpha` (supported: 0.05 and 0.01; other values use the 0.05 critical
/// value).
#[pyfunction]
fn is_significant(z_score: f32, alpha: f32) -> bool {
    core::is_significant(z_score, alpha)
}

/// Convert a significance level to a two-tailed critical z-value
/// (0.10→1.645, 0.05→1.96, 0.01→2.576, 0.001→3.291; other values clamp to
/// the nearest supported level).
#[pyfunction]
fn alpha_to_critical_z(alpha: f32) -> f32 {
    core::alpha_to_critical_z(alpha)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAbVariant>()?;
    m.add_class::<PyAbExperiment>()?;
    m.add_class::<PyAbVariantMetrics>()?;
    m.add_class::<PyAbExperimentResults>()?;
    m.add_class::<PyBayesianAbResult>()?;
    m.add_function(wrap_pyfunction!(z_test, m)?)?;
    m.add_function(wrap_pyfunction!(is_significant, m)?)?;
    m.add_function(wrap_pyfunction!(alpha_to_critical_z, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn two_variant_experiment() -> PyAbExperiment {
        PyAbExperiment {
            inner: core::Experiment {
                id: "exp1".to_string(),
                name: "Thumbnail Test".to_string(),
                variants: vec![
                    core::Variant {
                        id: "A".to_string(),
                        name: "Control".to_string(),
                        allocation_weight: 1.0,
                    },
                    core::Variant {
                        id: "B".to_string(),
                        name: "Treatment".to_string(),
                        allocation_weight: 1.0,
                    },
                ],
                start_ms: 0,
                end_ms: None,
                min_sample_size: 100,
            },
        }
    }

    #[test]
    fn assign_variant_is_deterministic() {
        let exp = two_variant_experiment();
        let a = exp.assign_variant("user_42").expect("assign should work");
        let b = exp.assign_variant("user_42").expect("assign should work");
        assert_eq!(a, b);
    }

    #[test]
    fn experiment_results_records_and_wins() {
        let exp = two_variant_experiment();
        let mut results = PyAbExperimentResults {
            inner: core::ExperimentResults::new(exp.inner.clone()),
        };
        for _ in 0..500 {
            results.record_impression("A");
        }
        for _ in 0..25 {
            results.record_click("A");
        }
        for _ in 0..500 {
            results.record_impression("B");
        }
        for _ in 0..50 {
            results.record_click("B");
        }
        let winner = results
            .winning_variant("ctr")
            .expect("valid metric should not error");
        assert_eq!(winner, Some("B".to_string()));

        let m = results.metrics("A").expect("A metrics present");
        assert_eq!(m.impressions(), 500);
        assert!((m.click_through_rate() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn winning_variant_rejects_unknown_metric() {
        let exp = two_variant_experiment();
        let results = PyAbExperimentResults {
            inner: core::ExperimentResults::new(exp.inner.clone()),
        };
        let err = results.winning_variant("convrsion"); // typo, must not fall back to ctr
        assert!(err.is_err());
    }

    #[test]
    fn bayesian_winner_b_beats_a() {
        let exp = two_variant_experiment();
        let mut results = PyAbExperimentResults {
            inner: core::ExperimentResults::new(exp.inner.clone()),
        };
        for _ in 0..100 {
            results.record_impression("A");
        }
        for _ in 0..5 {
            results.record_click("A");
        }
        for _ in 0..100 {
            results.record_impression("B");
        }
        for _ in 0..30 {
            results.record_click("B");
        }
        let res = results
            .bayesian_winner("A", "B", "ctr", 10_000, 42)
            .expect("bayesian winner should succeed");
        assert!(res.prob_b_beats_a > 0.9, "prob={}", res.prob_b_beats_a);
    }

    #[test]
    fn stat_helpers_match_core() {
        assert!((alpha_to_critical_z(0.05) - 1.96).abs() < 0.01);
        assert!(is_significant(2.0, 0.05));
        assert!(!is_significant(1.0, 0.05));
        let z = z_test(0.10, 5000, 0.05, 5000);
        assert!(z > 1.96);
    }
}

//! `oximedia.analytics` multi-armed bandit bindings — real delegation to
//! [`oximedia_analytics::bandit`].
//!
//! Exposes epsilon-greedy and Thompson-sampling arm selection over
//! Beta-Binomial posteriors, plus cumulative-regret tracking.

use oximedia_analytics::bandit as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use super::analytics_err;

// ---------------------------------------------------------------------------
// BanditArmSnapshot
// ---------------------------------------------------------------------------

/// Point-in-time snapshot of one bandit arm's accumulated statistics.
#[pyclass(name = "BanditArmSnapshot")]
pub struct PyBanditArmSnapshot {
    #[pyo3(get)]
    pub id: String,
    #[pyo3(get)]
    pub pulls: u64,
    #[pyo3(get)]
    pub reward_sum: u64,
    #[pyo3(get)]
    pub beta_alpha: f64,
    #[pyo3(get)]
    pub beta_beta: f64,
    #[pyo3(get)]
    pub empirical_mean: f64,
    #[pyo3(get)]
    pub posterior_mean: f64,
}

impl From<&core::BanditArm> for PyBanditArmSnapshot {
    fn from(arm: &core::BanditArm) -> Self {
        Self {
            id: arm.id.clone(),
            pulls: arm.pulls,
            reward_sum: arm.reward_sum,
            beta_alpha: arm.beta_alpha,
            beta_beta: arm.beta_beta,
            empirical_mean: arm.empirical_mean(),
            posterior_mean: arm.posterior_mean(),
        }
    }
}

#[pymethods]
impl PyBanditArmSnapshot {
    fn __repr__(&self) -> String {
        format!(
            "BanditArmSnapshot(id={:?}, pulls={}, posterior_mean={:.4})",
            self.id, self.pulls, self.posterior_mean
        )
    }
}

// ---------------------------------------------------------------------------
// MultiArmedBandit
// ---------------------------------------------------------------------------

/// Multi-armed bandit experiment state machine.
///
/// `strategy` must be `"epsilon_greedy"` (requires `epsilon`) or
/// `"thompson"` (a.k.a. `"thompson_sampling"`). All operations are
/// deterministic given the same `seed` (xoshiro256\*\*).
#[pyclass(name = "MultiArmedBandit")]
pub struct PyMultiArmedBandit {
    inner: core::MultiArmedBandit,
}

fn parse_strategy(strategy: &str, epsilon: Option<f64>) -> PyResult<core::BanditStrategy> {
    match strategy {
        "epsilon_greedy" => {
            let epsilon = epsilon.ok_or_else(|| {
                PyValueError::new_err("strategy 'epsilon_greedy' requires an `epsilon` value")
            })?;
            Ok(core::BanditStrategy::EpsilonGreedy { epsilon })
        }
        "thompson" | "thompson_sampling" => Ok(core::BanditStrategy::ThompsonSampling),
        other => Err(PyValueError::new_err(format!(
            "unknown bandit strategy {other:?}; expected 'epsilon_greedy' or 'thompson'"
        ))),
    }
}

#[pymethods]
impl PyMultiArmedBandit {
    #[new]
    #[pyo3(signature = (arm_ids, strategy, seed, epsilon=None))]
    fn new(
        arm_ids: Vec<String>,
        strategy: &str,
        seed: u64,
        epsilon: Option<f64>,
    ) -> PyResult<Self> {
        let strategy = parse_strategy(strategy, epsilon)?;
        let ids: Vec<&str> = arm_ids.iter().map(String::as_str).collect();
        let inner = core::MultiArmedBandit::new(&ids, strategy, seed).map_err(analytics_err)?;
        Ok(Self { inner })
    }

    /// Select an arm to pull according to the configured strategy; returns
    /// its index.
    fn select_arm(&mut self) -> PyResult<usize> {
        self.inner.select_arm().map_err(analytics_err)
    }

    /// Record a Bernoulli outcome (`reward` 0 or 1) for the arm at
    /// `arm_index`.
    fn record_outcome(&mut self, arm_index: usize, reward: u32) -> PyResult<()> {
        self.inner
            .record_outcome(arm_index, reward)
            .map_err(analytics_err)
    }

    /// Index of the arm with the highest posterior mean, or `None` if there
    /// are no arms.
    fn best_arm_index(&self) -> Option<usize> {
        self.inner.best_arm_index()
    }

    /// `(arm_id, posterior_mean)` pairs sorted descending by posterior mean.
    fn arm_rankings(&self) -> Vec<(String, f64)> {
        self.inner
            .arm_rankings()
            .into_iter()
            .map(|(id, mean)| (id.to_string(), mean))
            .collect()
    }

    /// Snapshot of every arm's current statistics, in configuration order.
    fn arms(&self) -> Vec<PyBanditArmSnapshot> {
        self.inner
            .arms
            .iter()
            .map(PyBanditArmSnapshot::from)
            .collect()
    }

    #[getter]
    fn total_pulls(&self) -> u64 {
        self.inner.total_pulls
    }

    fn __len__(&self) -> usize {
        self.inner.arms.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "MultiArmedBandit(arms={}, total_pulls={})",
            self.inner.arms.len(),
            self.inner.total_pulls
        )
    }
}

// ---------------------------------------------------------------------------
// RegretTracker
// ---------------------------------------------------------------------------

/// Tracks cumulative pseudo-regret across a bandit simulation.
#[pyclass(name = "RegretTracker")]
#[derive(Default)]
pub struct PyRegretTracker {
    inner: core::RegretTracker,
}

#[pymethods]
impl PyRegretTracker {
    #[new]
    fn new() -> Self {
        Self {
            inner: core::RegretTracker::new(),
        }
    }

    /// Record one step: `true_best_rate` is μ\*, `chosen_arm_rate` is the
    /// rate of the arm actually chosen at this step.
    fn record_step(&mut self, true_best_rate: f64, chosen_arm_rate: f64) {
        self.inner.record_step(true_best_rate, chosen_arm_rate);
    }

    #[getter]
    fn cumulative_regret(&self) -> f64 {
        self.inner.cumulative_regret
    }

    #[getter]
    fn steps(&self) -> u64 {
        self.inner.steps
    }

    #[getter]
    fn regret_history(&self) -> Vec<f64> {
        self.inner.regret_history.clone()
    }

    fn average_regret(&self) -> f64 {
        self.inner.average_regret()
    }

    fn __repr__(&self) -> String {
        format!(
            "RegretTracker(steps={}, cumulative_regret={:.4})",
            self.inner.steps, self.inner.cumulative_regret
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBanditArmSnapshot>()?;
    m.add_class::<PyMultiArmedBandit>()?;
    m.add_class::<PyRegretTracker>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epsilon_greedy_requires_epsilon() {
        let err = PyMultiArmedBandit::new(vec!["a".into(), "b".into()], "epsilon_greedy", 1, None);
        assert!(err.is_err());
    }

    #[test]
    fn unknown_strategy_errors() {
        let err = PyMultiArmedBandit::new(vec!["a".into()], "bogus", 1, None);
        assert!(err.is_err());
    }

    #[test]
    fn thompson_select_and_record() {
        let mut bandit = PyMultiArmedBandit::new(
            vec!["a".into(), "b".into(), "c".into()],
            "thompson",
            7,
            None,
        )
        .expect("construction should succeed");
        let idx = bandit.select_arm().expect("select should succeed");
        assert!(idx < 3);
        bandit.record_outcome(idx, 1).expect("record should work");
        assert_eq!(bandit.total_pulls(), 1);
        assert_eq!(bandit.arms().len(), 3);
    }

    #[test]
    fn epsilon_greedy_exploits_dominant_arm() {
        let mut bandit = PyMultiArmedBandit::new(
            vec!["low".into(), "high".into()],
            "epsilon_greedy",
            99,
            Some(0.0),
        )
        .expect("construction should succeed");
        for _ in 0..50 {
            bandit.record_outcome(1, 1).expect("record should work");
        }
        let selected = bandit.select_arm().expect("select should succeed");
        assert_eq!(selected, 1);
        assert_eq!(bandit.best_arm_index(), Some(1));
    }

    #[test]
    fn regret_tracker_accumulates() {
        let mut tracker = PyRegretTracker::new();
        tracker.record_step(0.5, 0.4);
        tracker.record_step(0.5, 0.5);
        assert!((tracker.cumulative_regret() - 0.1).abs() < 1e-9);
        assert_eq!(tracker.steps(), 2);
        assert!((tracker.average_regret() - 0.05).abs() < 1e-9);
    }
}

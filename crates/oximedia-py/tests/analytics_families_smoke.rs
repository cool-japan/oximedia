//! Smoke tests for the SLICE 4E analytics family bindings — ab_testing,
//! bandit, cohort, funnel, retention, geo_device, quantile/realtime,
//! replay/anomaly/attribution, and `SocialSignals`/
//! `compute_engagement_with_social`.
//!
//! Same embedded-interpreter pattern as `tests/analytics_smoke.rs`. These
//! run real Python source through the actually-registered
//! `oximedia.analytics` module — unlike the native `#[cfg(test)]` unit
//! tests inside each `analytics_py/*.rs` file, this is what verifies the
//! PyO3 registration wiring itself (correct class/function names, working
//! `Vec<PyRef<...>>` extraction, etc.) end-to-end, one test per family.

use oximedia_py::analytics_py::register_submodule;
use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn prepare_env(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let parent = PyModule::new(py, "oximedia_test_parent")?;
    register_submodule(&parent)?;
    let analytics = parent.getattr("analytics")?;
    let globals = PyDict::new(py);
    globals.set_item("analytics", analytics)?;
    Ok(globals)
}

#[test]
fn ab_testing_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "v1 = analytics.AbVariant('A', 'Control', 1.0)\n\
                 v2 = analytics.AbVariant('B', 'Treatment', 1.0)\n\
                 exp = analytics.AbExperiment('exp1', 'Test', [v1, v2], 0)\n\
                 assert exp.assign_variant('user_42') == exp.assign_variant('user_42')\n\
                 results = analytics.AbExperimentResults(exp)\n\
                 for _ in range(500):\n\
                 \x20\x20\x20\x20results.record_impression('A')\n\
                 for _ in range(25):\n\
                 \x20\x20\x20\x20results.record_click('A')\n\
                 for _ in range(500):\n\
                 \x20\x20\x20\x20results.record_impression('B')\n\
                 for _ in range(50):\n\
                 \x20\x20\x20\x20results.record_click('B')\n\
                 assert results.winning_variant('ctr') == 'B'\n\
                 m = results.metrics('A')\n\
                 assert m.impressions == 500\n\
                 assert abs(m.click_through_rate() - 0.05) < 1e-6\n\
                 bay = results.bayesian_winner('A', 'B', 'ctr', 5000, 1)\n\
                 assert bay.prob_b_beats_a > 0.9\n\
                 try:\n\
                 \x20\x20\x20\x20results.winning_variant('convrsion')\n\
                 \x20\x20\x20\x20raise AssertionError('typo metric should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("ab_testing family runs");
    });
}

#[test]
fn bandit_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "bandit = analytics.MultiArmedBandit(['low', 'high'], 'epsilon_greedy', 1, epsilon=0.0)\n\
                 for _ in range(50):\n\
                 \x20\x20\x20\x20bandit.record_outcome(1, 1)\n\
                 assert bandit.select_arm() == 1\n\
                 assert bandit.best_arm_index() == 1\n\
                 assert len(bandit.arms()) == 2\n\
                 tracker = analytics.RegretTracker()\n\
                 tracker.record_step(0.5, 0.4)\n\
                 tracker.record_step(0.5, 0.5)\n\
                 assert tracker.steps == 2\n\
                 assert abs(tracker.cumulative_regret - 0.1) < 1e-9\n"
            ),
            Some(&globals),
            None,
        )
        .expect("bandit family runs");
    });
}

#[test]
fn cohort_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "events = [('alice', 0), ('bob', 0), ('alice', 86400000)]\n\
                 matrix = analytics.build_cohort_matrix(events, 'day', 2)\n\
                 assert len(matrix.cohorts()) == 1\n\
                 r0 = matrix.retention_at(0, 0)\n\
                 assert abs(r0 - 1.0) < 1e-6\n\
                 r1 = matrix.retention_at(0, 1)\n\
                 assert abs(r1 - 0.5) < 1e-6\n\
                 curve = analytics.cohort_retention_curve(0, ['u1'], [('u1', 0)], 0)\n\
                 assert abs(curve[0] - 1.0) < 1e-9\n\
                 try:\n\
                 \x20\x20\x20\x20analytics.build_cohort_matrix(events, 'fortnight', 2)\n\
                 \x20\x20\x20\x20raise AssertionError('bad window should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("cohort family runs");
    });
}

#[test]
fn funnel_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "s1 = analytics.ViewerSession('s1', 'c1', 0)\n\
                 s1.track_play(0)\n\
                 s1.track_end(10000, 10000)\n\
                 result = analytics.compute_funnel([s1], [('start', 0), ('mid', 5000)], 10000)\n\
                 assert result.total_starters == 1\n\
                 assert abs(result.completion_rate() - 1.0) < 1e-6\n\
                 config = analytics.ChurnConfig()\n\
                 assessment = analytics.predict_churn('v1', [(0, 1.0), (1, 0.9), (2, 0.8)], config)\n\
                 assert assessment.risk in ('low', 'medium', 'high')\n\
                 weights = analytics.LoyaltyWeights()\n\
                 loyalty = analytics.compute_loyalty('v1', [0], [1000], 1000, 86400000, 10, 3600000, weights)\n\
                 assert 0.0 <= loyalty.score <= 1.0\n\
                 events = [('u1', 'view', 0), ('u1', 'purchase', 1000)]\n\
                 steps = [('view', 'view'), ('purchase', 'purchase')]\n\
                 report = analytics.funnel_analyze(events, steps, 60000)\n\
                 assert report.step_completions == [1, 1]\n\
                 assert abs(report.overall_completion_rate() - 1.0) < 1e-9\n"
            ),
            Some(&globals),
            None,
        )
        .expect("funnel family runs");
    });
}

#[test]
fn retention_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "s1 = analytics.ViewerSession('s1', 'c1', 0)\n\
                 s1.track_play(0)\n\
                 s1.track_end(10000, 10000)\n\
                 curve = analytics.compute_retention([s1], 10000, 5)\n\
                 assert curve.total_starts == 1\n\
                 avg = curve.average_view_duration()\n\
                 assert avg >= 0.0\n\
                 score = curve.compare_to_benchmark('vod')\n\
                 assert 0.0 <= score <= 100.0\n\
                 try:\n\
                 \x20\x20\x20\x20curve.compare_to_benchmark('bogus')\n\
                 \x20\x20\x20\x20raise AssertionError('bad benchmark should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n\
                 segs = analytics.compute_segment_retention([s1], [('intro', 0, 5000)], 10000)\n\
                 assert len(segs) == 1\n\
                 assert segs[0].viewers_entered == 1\n\
                 state = analytics.IncrementalRetentionState(10000, 5)\n\
                 state.add_session(s1)\n\
                 curve2 = state.finalise()\n\
                 assert curve2.total_starts == 1\n\
                 assert state.sessions_processed() == 1\n\
                 rw = analytics.re_watch_segments([s1], 10000)\n\
                 assert isinstance(rw, list)\n\
                 curve3 = analytics.compute_retention_incremental([s1], 10000, 5, 10)\n\
                 assert curve3.total_starts == 1\n"
            ),
            Some(&globals),
            None,
        )
        .expect("retention family runs");
    });
}

#[test]
fn geo_device_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "a = analytics.BreakdownAnalyzer()\n\
                 a.ingest('v1', 'europe', 'desktop', 100.0)\n\
                 a.ingest('v2', 'asia_pacific', 'mobile', 500.0)\n\
                 assert a.session_count() == 2\n\
                 assert a.top_region_by_watch_time() == 'asia_pacific'\n\
                 report = a.build_report()\n\
                 assert report.total_sessions == 2\n\
                 share = report.region_share('europe')\n\
                 assert 0.0 <= share <= 1.0\n\
                 assert report.dominant_region_by_sessions() in ('europe', 'asia_pacific')\n\
                 try:\n\
                 \x20\x20\x20\x20a.ingest('v3', 'mars', 'desktop', 1.0)\n\
                 \x20\x20\x20\x20raise AssertionError('bad region should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n\
                 timestamped = [\n\
                 \x20\x20\x20\x20('v1', 'north_america', 'mobile', 60.0, 50),\n\
                 \x20\x20\x20\x20('v2', 'north_america', 'mobile', 600.0, 150),\n\
                 ]\n\
                 cmp = a.compare_region_periods(timestamped, 'north_america', 100)\n\
                 assert cmp.total_watch_seconds.is_growing()\n"
            ),
            Some(&globals),
            None,
        )
        .expect("geo_device family runs");
    });
}

#[test]
fn quantile_realtime_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "d = analytics.TDigest(100.0)\n\
                 for i in range(1, 1001):\n\
                 \x20\x20\x20\x20d.add(float(i))\n\
                 p50 = d.quantile(0.5)\n\
                 assert abs(p50 - 500.0) < 60.0\n\
                 d.add(1.0)\n\
                 p90 = d.quantile(0.9)\n\
                 assert p90 > 0.0\n\
                 pcts = analytics.percentiles([float(i) for i in range(1, 101)], [50.0, 95.0])\n\
                 assert len(pcts) == 2\n\
                 agg = analytics.SlidingWindowAggregator(60000, 10000)\n\
                 agg.ingest_viewer_join('a', 1000)\n\
                 agg.ingest_viewer_join('b', 2000)\n\
                 assert agg.concurrent_viewers() == 2\n\
                 agg.ingest_bitrate_report('a', 5000, 2_000_000)\n\
                 avg, lo, hi = agg.window_bitrate_stats()\n\
                 assert lo == hi == 2_000_000\n\
                 assert len(agg.buckets()) >= 1\n"
            ),
            Some(&globals),
            None,
        )
        .expect("quantile_realtime family runs");
    });
}

#[test]
fn replay_anomaly_attribution_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "s1 = analytics.ViewerSession('s1', 'c1', 0)\n\
                 s1.track_play(0)\n\
                 s1.track_end(10000, 10000)\n\
                 config = analytics.ReplayConfig()\n\
                 rec = analytics.ReplayReconstructor(config)\n\
                 frames = rec.reconstruct(s1)\n\
                 assert len(frames) > 0\n\
                 assert frames[-1].state == 'ended'\n\
                 summary = analytics.ReplayReconstructor.summarise(frames, s1)\n\
                 assert summary.frame_count == len(frames)\n\
                 zdet = analytics.ZScoreDetector(20)\n\
                 for i in range(19):\n\
                 \x20\x20\x20\x20zdet.update(10.0 + (i % 2) * 0.01)\n\
                 z = zdet.update(1000.0)\n\
                 assert z is not None and z > 0.0\n\
                 segments = [('first_half', 0, 5000), ('second_half', 5000, 10000)]\n\
                 attrs = analytics.compute_attribution([s1], segments, 10000, 'uniform')\n\
                 assert len(attrs) == 2\n\
                 total = sum(a.normalised_credit for a in attrs)\n\
                 assert abs(total - 1.0) < 1e-6\n\
                 try:\n\
                 \x20\x20\x20\x20analytics.compute_attribution([s1], segments, 10000, 'bogus')\n\
                 \x20\x20\x20\x20raise AssertionError('bad model should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("replay/anomaly/attribution family runs");
    });
}

#[test]
fn social_signals_and_engagement_weights() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register analytics");
        py.run(
            c_str!(
                "s1 = analytics.ViewerSession('s1', 'c1', 0)\n\
                 s1.track_play(0)\n\
                 s1.track_end(10000, 10000)\n\
                 social = analytics.SocialSignals(views=1000, likes=400, shares=200, comments=100)\n\
                 score = analytics.compute_engagement_with_social([s1], 10000, social)\n\
                 assert score.social_score > 0.9\n\
                 weights = analytics.EngagementWeights()\n\
                 score2 = analytics.compute_engagement([s1], 10000, weights)\n\
                 assert score2.score >= 0.0\n\
                 zero_social = analytics.SocialSignals()\n\
                 assert zero_social.engagement_score() == 0.0\n"
            ),
            Some(&globals),
            None,
        )
        .expect("social signals / engagement weights run");
    });
}

//! `oximedia.analytics` submodule — Python bindings for `oximedia-analytics`.
//!
//! Wraps viewer-session playback tracking, session-metric analysis,
//! engagement scoring, and attention-heatmap generation behind PyO3 classes
//! with real delegation to [`oximedia_analytics`]. Unlike the WASM
//! `SessionTracker`/heatmap bindings — which reimplement their own ad-hoc
//! watch-time bookkeeping because they predate this crate's stabilised
//! session model — this binding drives the actual `oximedia_analytics`
//! session/engagement algorithms, so results match what a server-side batch
//! analytics job would compute for the same events.

use oximedia_analytics::{
    analyze_session as core_analyze_session, attention_heatmap as core_attention_heatmap,
    compute_engagement as core_compute_engagement,
    compute_engagement_with_social as core_compute_engagement_with_social, EngagementWeights,
    PlaybackEvent, SessionMetrics, SocialSignals, ViewerSession,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

// Family submodules — each owns a `register(m)` that adds its classes and
// functions directly into the `oximedia.analytics` Python namespace built by
// `register_submodule` below. Living under `analytics_py/` (Rust 2018+ file
// module + sibling directory) keeps every family's binding code out of this
// file without requiring any change to `lib.rs`.
mod ab_testing;
mod bandit;
mod cohort;
mod funnel;
mod geo_device;
mod quantile_realtime;
mod replay_anomaly_attribution;
mod retention;

/// Map an [`oximedia_analytics::error::AnalyticsError`] to a Python
/// `ValueError` carrying the real error message (no fabricated text).
pub(crate) fn analytics_err(err: oximedia_analytics::error::AnalyticsError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

// ---------------------------------------------------------------------------
// ViewerSession
// ---------------------------------------------------------------------------

/// A single viewer's playback session — records timestamped playback events
/// (play / pause / seek / buffer / quality-change / end) for later analysis.
///
/// Real delegation to [`oximedia_analytics::ViewerSession`] /
/// [`oximedia_analytics::PlaybackEvent`].
#[pyclass(name = "ViewerSession")]
#[derive(Clone)]
pub struct PyViewerSession {
    inner: ViewerSession,
}

#[pymethods]
impl PyViewerSession {
    /// Create a new empty session.
    ///
    /// Args:
    ///     session_id: Unique identifier for this viewing session.
    ///     content_id: Identifier of the content being watched.
    ///     started_at_ms: Wall-clock session start time (Unix epoch ms).
    ///     user_id: Optional viewer identifier.
    #[new]
    #[pyo3(signature = (session_id, content_id, started_at_ms, user_id=None))]
    fn new(
        session_id: String,
        content_id: String,
        started_at_ms: i64,
        user_id: Option<String>,
    ) -> Self {
        Self {
            inner: ViewerSession::new(session_id, user_id, content_id, started_at_ms),
        }
    }

    /// Record a play event at `timestamp_ms` (wall-clock).
    fn track_play(&mut self, timestamp_ms: i64) {
        self.inner.push_event(PlaybackEvent::Play { timestamp_ms });
    }

    /// Record a pause event at `timestamp_ms` (wall-clock), content position
    /// `position_ms`.
    fn track_pause(&mut self, timestamp_ms: i64, position_ms: u64) {
        self.inner.push_event(PlaybackEvent::Pause {
            timestamp_ms,
            position_ms,
        });
    }

    /// Record a scrub from `from_ms` to `to_ms` (content positions).
    fn track_seek(&mut self, from_ms: u64, to_ms: u64) {
        self.inner
            .push_event(PlaybackEvent::Seek { from_ms, to_ms });
    }

    /// Record the start of a buffering stall at content position `position_ms`.
    fn track_buffer_start(&mut self, position_ms: u64) {
        self.inner
            .push_event(PlaybackEvent::BufferStart { position_ms });
    }

    /// Record the end of a buffering stall at `position_ms` that lasted
    /// `duration_ms`.
    fn track_buffer_end(&mut self, position_ms: u64, duration_ms: u32) {
        self.inner.push_event(PlaybackEvent::BufferEnd {
            position_ms,
            duration_ms,
        });
    }

    /// Record an adaptive-bitrate quality-level switch.
    fn track_quality_change(&mut self, from_height: u32, to_height: u32, bitrate: u32) {
        self.inner.push_event(PlaybackEvent::QualityChange {
            from_height,
            to_height,
            bitrate,
        });
    }

    /// Record the end of the session at `position_ms`, having watched
    /// `watch_duration_ms` in total.
    fn track_end(&mut self, position_ms: u64, watch_duration_ms: u64) {
        self.inner.push_event(PlaybackEvent::End {
            position_ms,
            watch_duration_ms,
        });
    }

    /// Number of events recorded so far.
    fn event_count(&self) -> usize {
        self.inner.events.len()
    }

    #[getter]
    fn session_id(&self) -> String {
        self.inner.session_id.clone()
    }

    #[getter]
    fn content_id(&self) -> String {
        self.inner.content_id.clone()
    }

    #[getter]
    fn user_id(&self) -> Option<String> {
        self.inner.user_id.clone()
    }

    /// Analyse this session and return aggregate [`SessionMetrics`].
    ///
    /// `content_duration_ms` is used to compute `completion_pct` and the
    /// unique-position count; pass `0` if unknown.
    fn analyze(&self, content_duration_ms: u64) -> PySessionMetrics {
        core_analyze_session(&self.inner, content_duration_ms).into()
    }

    fn __repr__(&self) -> String {
        format!(
            "ViewerSession(session_id={:?}, content_id={:?}, events={})",
            self.inner.session_id,
            self.inner.content_id,
            self.inner.events.len()
        )
    }
}

// ---------------------------------------------------------------------------
// SessionMetrics
// ---------------------------------------------------------------------------

/// Aggregate metrics derived from a single [`PyViewerSession::analyze`] call.
#[pyclass(name = "SessionMetrics")]
pub struct PySessionMetrics {
    /// Total milliseconds of content actually watched.
    #[pyo3(get)]
    pub total_watch_ms: u64,
    /// Number of unique 1-second positions watched.
    #[pyo3(get)]
    pub unique_positions_watched: u64,
    /// How many `Seek` events were recorded.
    #[pyo3(get)]
    pub seek_count: u32,
    /// How many buffering interruptions occurred.
    #[pyo3(get)]
    pub buffer_events: u32,
    /// Total stall time in milliseconds.
    #[pyo3(get)]
    pub buffer_time_ms: u64,
    /// How many quality-level switches happened.
    #[pyo3(get)]
    pub quality_changes: u32,
    /// Fraction of the content completed, `0.0-100.0`.
    #[pyo3(get)]
    pub completion_pct: f32,
}

impl From<SessionMetrics> for PySessionMetrics {
    fn from(m: SessionMetrics) -> Self {
        Self {
            total_watch_ms: m.total_watch_ms,
            unique_positions_watched: m.unique_positions_watched,
            seek_count: m.seek_count,
            buffer_events: m.buffer_events,
            buffer_time_ms: m.buffer_time_ms,
            quality_changes: m.quality_changes,
            completion_pct: m.completion_pct,
        }
    }
}

#[pymethods]
impl PySessionMetrics {
    fn __repr__(&self) -> String {
        format!(
            "SessionMetrics(watch_ms={}, completion_pct={:.1}, seeks={}, buffer_events={})",
            self.total_watch_ms, self.completion_pct, self.seek_count, self.buffer_events
        )
    }
}

// ---------------------------------------------------------------------------
// ContentEngagementScore
// ---------------------------------------------------------------------------

/// A per-content engagement score in `0.0-1.0`, decomposed into its
/// component signals. See [`compute_engagement`].
#[pyclass(name = "ContentEngagementScore")]
pub struct PyContentEngagementScore {
    /// The content ID the score was computed for (from the first session).
    #[pyo3(get)]
    pub content_id: String,
    /// Overall weighted engagement score, `0.0-1.0`.
    #[pyo3(get)]
    pub score: f32,
    /// Ratio of average watch time to content duration (capped at 1.0).
    #[pyo3(get)]
    pub watch_time_score: f32,
    /// Fraction of sessions that reached >= 95% completion.
    #[pyo3(get)]
    pub completion_score: f32,
    /// Fraction of sessions that rewatched any segment.
    #[pyo3(get)]
    pub rewatch_score: f32,
    /// Normalised social-interaction score; honestly `0.0` when no social
    /// data is available (this binding does not fabricate a placeholder).
    #[pyo3(get)]
    pub social_score: f32,
    /// Penalty term proportional to the forward-seek rate (lower is better).
    #[pyo3(get)]
    pub seek_forward_penalty: f32,
}

#[pymethods]
impl PyContentEngagementScore {
    fn __repr__(&self) -> String {
        format!(
            "ContentEngagementScore(content_id={:?}, score={:.3})",
            self.content_id, self.score
        )
    }
}

// ---------------------------------------------------------------------------
// SocialSignals / EngagementWeights
// ---------------------------------------------------------------------------

/// Raw social-interaction counts for a piece of content (views/likes/shares/
/// comments). Real delegation to [`oximedia_analytics::SocialSignals`].
///
/// `ViewerSession`/`PlaybackEvent` carry no social data, so these counts must
/// be supplied explicitly (e.g. from a CMS or comments service). Use
/// :meth:`engagement_score` to collapse them into a normalised `0.0-1.0`
/// score, or pass an instance to :func:`compute_engagement_with_social`.
#[pyclass(name = "SocialSignals")]
#[derive(Clone, Debug, Default)]
pub struct PySocialSignals {
    inner: SocialSignals,
}

#[pymethods]
impl PySocialSignals {
    #[new]
    #[pyo3(signature = (views=0, likes=0, shares=0, comments=0))]
    fn new(views: u64, likes: u64, shares: u64, comments: u64) -> Self {
        Self {
            inner: SocialSignals {
                views,
                likes,
                shares,
                comments,
            },
        }
    }

    #[getter]
    fn views(&self) -> u64 {
        self.inner.views
    }

    #[getter]
    fn likes(&self) -> u64 {
        self.inner.likes
    }

    #[getter]
    fn shares(&self) -> u64 {
        self.inner.shares
    }

    #[getter]
    fn comments(&self) -> u64 {
        self.inner.comments
    }

    /// Normalised social engagement score in `[0.0, 1.0]`. Honestly `0.0`
    /// when `views == 0` (undefined rate), never a fabricated midpoint.
    fn engagement_score(&self) -> f32 {
        self.inner.engagement_score()
    }

    fn __repr__(&self) -> String {
        format!(
            "SocialSignals(views={}, likes={}, shares={}, comments={})",
            self.inner.views, self.inner.likes, self.inner.shares, self.inner.comments
        )
    }
}

/// Weights controlling the relative importance of each engagement component.
/// Real delegation to [`oximedia_analytics::EngagementWeights`].
#[pyclass(name = "EngagementWeights")]
#[derive(Clone, Debug)]
pub struct PyEngagementWeights {
    inner: EngagementWeights,
}

#[pymethods]
impl PyEngagementWeights {
    /// All five components equally weighted at 0.2 unless overridden.
    #[new]
    #[pyo3(signature = (watch_time=0.2, completion=0.2, rewatch=0.2, social=0.2, forward_seek_penalty=0.2))]
    fn new(
        watch_time: f32,
        completion: f32,
        rewatch: f32,
        social: f32,
        forward_seek_penalty: f32,
    ) -> Self {
        Self {
            inner: EngagementWeights {
                watch_time,
                completion,
                rewatch,
                social,
                forward_seek_penalty,
            },
        }
    }

    #[getter]
    fn watch_time(&self) -> f32 {
        self.inner.watch_time
    }

    #[getter]
    fn completion(&self) -> f32 {
        self.inner.completion
    }

    #[getter]
    fn rewatch(&self) -> f32 {
        self.inner.rewatch
    }

    #[getter]
    fn social(&self) -> f32 {
        self.inner.social
    }

    #[getter]
    fn forward_seek_penalty(&self) -> f32 {
        self.inner.forward_seek_penalty
    }

    fn __repr__(&self) -> String {
        format!(
            "EngagementWeights(watch_time={:.3}, completion={:.3}, rewatch={:.3}, social={:.3}, forward_seek_penalty={:.3})",
            self.inner.watch_time, self.inner.completion, self.inner.rewatch,
            self.inner.social, self.inner.forward_seek_penalty
        )
    }
}

// ---------------------------------------------------------------------------
// Standalone functions
// ---------------------------------------------------------------------------

/// Analyse a [`PyViewerSession`] and return its [`PySessionMetrics`].
///
/// Module-level convenience equivalent to `session.analyze(content_duration_ms)`.
#[pyfunction]
pub fn analyze_session(session: &PyViewerSession, content_duration_ms: u64) -> PySessionMetrics {
    session.analyze(content_duration_ms)
}

/// Compute an attention heatmap across multiple sessions, bucketed by
/// `bucket_ms`.
///
/// Returns a list of `(position_ms, intensity)` pairs; `intensity` is
/// normalised so the peak bucket is `1.0`. Returns an empty list if
/// `sessions` is empty or `content_duration_ms`/`bucket_ms` is `0`.
#[pyfunction]
pub fn attention_heatmap(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    content_duration_ms: u64,
    bucket_ms: u32,
) -> Vec<(u64, f32)> {
    let inner_sessions: Vec<ViewerSession> = sessions.iter().map(|s| s.inner.clone()).collect();
    core_attention_heatmap(&inner_sessions, content_duration_ms, bucket_ms)
        .into_iter()
        .map(|hp| (hp.position_ms, hp.intensity))
        .collect()
}

/// Compute an overall engagement score for a content item from its viewer
/// sessions, using equally-weighted default components (watch time,
/// completion, rewatch, social, forward-seek penalty) unless `weights` is
/// given.
///
/// `ViewerSession`/`PlaybackEvent` carry no social-interaction data, so the
/// social channel's weight is honestly redistributed across the measurable
/// channels (real crate behaviour) rather than fabricated. Pass explicit
/// social interaction counts via :func:`compute_engagement_with_social`
/// instead when you have them.
#[pyfunction]
#[pyo3(signature = (sessions, content_duration_ms, weights=None))]
pub fn compute_engagement(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    content_duration_ms: u64,
    weights: Option<&PyEngagementWeights>,
) -> PyContentEngagementScore {
    let inner_sessions: Vec<ViewerSession> = sessions.iter().map(|s| s.inner.clone()).collect();
    let owned_default;
    let weights_ref = match weights {
        Some(w) => &w.inner,
        None => {
            owned_default = EngagementWeights::default();
            &owned_default
        }
    };
    let score = core_compute_engagement(&inner_sessions, content_duration_ms, weights_ref);
    PyContentEngagementScore {
        content_id: score.content_id,
        score: score.score,
        watch_time_score: score.components.watch_time_score,
        completion_score: score.components.completion_score,
        rewatch_score: score.components.rewatch_score,
        social_score: score.components.social_score,
        seek_forward_penalty: score.components.seek_forward_penalty,
    }
}

/// Compute an engagement score from viewer sessions **and** explicit social
/// signals. Real delegation to
/// [`oximedia_analytics::compute_engagement_with_social`].
///
/// Unlike [`compute_engagement`], the social component here is the real
/// normalised value from `social.engagement_score()`, and `weights` (if
/// given) are applied exactly as given — no redistribution.
#[pyfunction]
#[pyo3(signature = (sessions, content_duration_ms, social, weights=None))]
pub fn compute_engagement_with_social(
    sessions: Vec<PyRef<'_, PyViewerSession>>,
    content_duration_ms: u64,
    social: &PySocialSignals,
    weights: Option<&PyEngagementWeights>,
) -> PyContentEngagementScore {
    let inner_sessions: Vec<ViewerSession> = sessions.iter().map(|s| s.inner.clone()).collect();
    let owned_default;
    let weights_ref = match weights {
        Some(w) => &w.inner,
        None => {
            owned_default = EngagementWeights::default();
            &owned_default
        }
    };
    let score = core_compute_engagement_with_social(
        &inner_sessions,
        content_duration_ms,
        weights_ref,
        &social.inner,
    );
    PyContentEngagementScore {
        content_id: score.content_id,
        score: score.score,
        watch_time_score: score.components.watch_time_score,
        completion_score: score.components.completion_score,
        rewatch_score: score.components.rewatch_score,
        social_score: score.components.social_score,
        seek_forward_penalty: score.components.seek_forward_penalty,
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the `oximedia.analytics` submodule.
pub fn register_submodule(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "analytics")?;

    m.add_class::<PyViewerSession>()?;
    m.add_class::<PySessionMetrics>()?;
    m.add_class::<PyContentEngagementScore>()?;
    m.add_class::<PySocialSignals>()?;
    m.add_class::<PyEngagementWeights>()?;
    m.add_function(wrap_pyfunction!(analyze_session, &m)?)?;
    m.add_function(wrap_pyfunction!(attention_heatmap, &m)?)?;
    m.add_function(wrap_pyfunction!(compute_engagement, &m)?)?;
    m.add_function(wrap_pyfunction!(compute_engagement_with_social, &m)?)?;

    // Family submodules (each adds its own classes/functions into `m`).
    ab_testing::register(&m)?;
    bandit::register(&m)?;
    cohort::register(&m)?;
    funnel::register(&m)?;
    retention::register(&m)?;
    geo_device::register(&m)?;
    quantile_realtime::register(&m)?;
    replay_anomaly_attribution::register(&m)?;

    parent.add_submodule(&m)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn full_watch_session(id: &str) -> PyViewerSession {
        let mut s = PyViewerSession::new(id.to_string(), "content-1".to_string(), 0, None);
        s.track_play(0);
        s.track_end(10_000, 10_000);
        s
    }

    #[test]
    fn viewer_session_new_has_no_events() {
        let s = PyViewerSession::new(
            "s1".to_string(),
            "c1".to_string(),
            0,
            Some("u1".to_string()),
        );
        assert_eq!(s.event_count(), 0);
        assert_eq!(s.session_id(), "s1");
        assert_eq!(s.content_id(), "c1");
        assert_eq!(s.user_id(), Some("u1".to_string()));
    }

    #[test]
    fn track_events_increments_count() {
        let mut s = PyViewerSession::new("s2".to_string(), "c1".to_string(), 0, None);
        s.track_play(0);
        s.track_pause(1000, 5000);
        s.track_seek(5000, 8000);
        s.track_buffer_start(8000);
        s.track_buffer_end(8000, 500);
        s.track_quality_change(720, 1080, 5_000_000);
        s.track_end(10_000, 9500);
        assert_eq!(s.event_count(), 7);
    }

    #[test]
    fn analyze_full_watch_reports_full_completion() {
        let s = full_watch_session("s3");
        let metrics = s.analyze(10_000);
        assert!((metrics.completion_pct - 100.0).abs() < 1e-3);
        assert_eq!(metrics.total_watch_ms, 10_000);
    }

    #[test]
    fn analyze_no_events_is_zero_metrics() {
        let s = PyViewerSession::new("s4".to_string(), "c1".to_string(), 0, None);
        let metrics = s.analyze(10_000);
        assert_eq!(metrics.total_watch_ms, 0);
        assert_eq!(metrics.seek_count, 0);
    }

    #[test]
    fn module_level_analyze_session_matches_method() {
        let s = full_watch_session("s5");
        let via_method = s.analyze(10_000);
        let via_function = analyze_session(&s, 10_000);
        assert_eq!(via_method.total_watch_ms, via_function.total_watch_ms);
        assert_eq!(via_method.completion_pct, via_function.completion_pct);
        assert_eq!(via_method.seek_count, via_function.seek_count);
    }

    #[test]
    fn session_repr_contains_ids() {
        let s = full_watch_session("s6");
        let repr = s.__repr__();
        assert!(repr.contains("s6"));
        assert!(repr.contains("content-1"));
    }

    // `attention_heatmap` and `compute_engagement` take `Vec<PyRef<PyViewerSession>>`,
    // which requires a live Python object (GIL) to construct a `PyRef` from — those
    // are covered end-to-end via the embedded interpreter in
    // `tests/analytics_smoke.rs` instead of here.

    #[test]
    fn social_signals_zero_views_is_honest_zero() {
        let s = PySocialSignals::new(0, 1000, 500, 250);
        assert_eq!(s.engagement_score(), 0.0);
        assert_eq!(s.views(), 0);
        assert_eq!(s.likes(), 1000);
    }

    #[test]
    fn social_signals_high_engagement_near_one() {
        let s = PySocialSignals::new(1_000, 500, 300, 200);
        assert!(s.engagement_score() > 0.99);
    }

    #[test]
    fn social_signals_repr_contains_counts() {
        let s = PySocialSignals::new(10, 2, 1, 0);
        let repr = s.__repr__();
        assert!(repr.contains("views=10"));
        assert!(repr.contains("likes=2"));
    }

    #[test]
    fn engagement_weights_default_matches_core_default() {
        let w = PyEngagementWeights::new(0.2, 0.2, 0.2, 0.2, 0.2);
        assert!((w.watch_time() - 0.2).abs() < 1e-6);
        assert!((w.social() - 0.2).abs() < 1e-6);
    }
}

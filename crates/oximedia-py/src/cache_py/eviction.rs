//! `oximedia.cache` standalone eviction-policy bindings — real delegation to
//! [`oximedia_cache::eviction_policies`].
//!
//! Exposes the building blocks used to *implement* eviction policies,
//! decoupled from any specific cache backend: a decaying frequency counter,
//! an O(1)-amortised LFU tracker, a TinyLFU admission gate, and an ARC
//! ghost-list tracker.
//!
//! [`oximedia_cache::eviction_policies::EvictionPolicy`] itself (the
//! discriminated union naming the five/six strategies) carries no behaviour
//! of its own in this module — the strategy structs below are what actually
//! compute — so it is not separately bound here.

use oximedia_cache::eviction_policies as core;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// FrequencyCounter
// ---------------------------------------------------------------------------

/// Windowed frequency counter with periodic exponential decay (halves all
/// counters, dropping any that reach zero, to bound memory and avoid
/// permanent popularity inflation).
#[pyclass(name = "FrequencyCounter")]
pub struct PyFrequencyCounter {
    inner: core::FrequencyCounter,
}

#[pymethods]
impl PyFrequencyCounter {
    #[new]
    fn new(window_size: usize) -> Self {
        Self {
            inner: core::FrequencyCounter::new(window_size),
        }
    }

    /// Increment `key`'s count; auto-decays every key once
    /// `total_increments` reaches `window_size`.
    fn increment(&mut self, key: u64) {
        self.inner.increment(key);
    }

    fn frequency(&self, key: u64) -> u64 {
        self.inner.frequency(key)
    }

    fn decay_all(&mut self) {
        self.inner.decay_all();
    }

    fn tracked_keys(&self) -> usize {
        self.inner.tracked_keys()
    }

    fn clear(&mut self) {
        self.inner.clear();
    }

    fn __repr__(&self) -> String {
        format!(
            "FrequencyCounter(tracked_keys={})",
            self.inner.tracked_keys()
        )
    }
}

// ---------------------------------------------------------------------------
// LfuEvictionTracker
// ---------------------------------------------------------------------------

/// O(1)-amortised LFU eviction tracker (frequency buckets, FIFO within a
/// bucket).
#[pyclass(name = "LfuEvictionTracker")]
#[derive(Default)]
pub struct PyLfuEvictionTracker {
    inner: core::LfuEvictionTracker,
}

#[pymethods]
impl PyLfuEvictionTracker {
    #[new]
    fn new() -> Self {
        Self {
            inner: core::LfuEvictionTracker::new(),
        }
    }

    /// Insert a brand-new `key` at frequency 1 (no-op if already tracked —
    /// use [`promote`](Self::promote) to bump an existing key).
    fn insert(&mut self, key: u64) {
        self.inner.insert(key);
    }

    /// Record an access, moving `key` up one frequency bucket.
    fn promote(&mut self, key: u64) {
        self.inner.promote(key);
    }

    /// Remove and return the lowest-frequency key (FIFO tie-break), or
    /// `None` if empty.
    fn evict(&mut self) -> Option<u64> {
        self.inner.evict()
    }

    /// Remove a specific `key`. Returns `True` if it was present.
    fn remove(&mut self, key: u64) -> bool {
        self.inner.remove(key)
    }

    fn frequency(&self, key: u64) -> u64 {
        self.inner.frequency(key)
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn __repr__(&self) -> String {
        format!("LfuEvictionTracker(len={})", self.inner.len())
    }
}

// ---------------------------------------------------------------------------
// TinyLfuAdmission
// ---------------------------------------------------------------------------

/// TinyLFU admission gate: combines a Bloom-filter doorkeeper with a
/// counting-Bloom-filter frequency estimate to decide whether a candidate
/// should displace an about-to-be-evicted entry.
#[pyclass(name = "TinyLfuAdmission")]
pub struct PyTinyLfuAdmission {
    inner: core::TinyLfuAdmission,
}

#[pymethods]
impl PyTinyLfuAdmission {
    #[new]
    fn new(expected_items: usize) -> Self {
        Self {
            inner: core::TinyLfuAdmission::new(expected_items),
        }
    }

    fn record_access(&mut self, candidate_key: u64) {
        self.inner.record_access(candidate_key);
    }

    /// Whether `candidate_key` should be admitted in place of an entry
    /// whose eviction frequency estimate is `evicted_freq`. Also records the
    /// access.
    fn should_admit(&mut self, candidate_key: u64, evicted_freq: u64) -> bool {
        self.inner.should_admit(candidate_key, evicted_freq)
    }

    fn estimated_frequency(&self, key: u64) -> u64 {
        self.inner.estimated_frequency(key)
    }

    fn decay(&mut self) {
        self.inner.decay();
    }
}

// ---------------------------------------------------------------------------
// ArcTracker
// ---------------------------------------------------------------------------

/// Adaptive Replacement Cache ghost-list size/parameter tracker. Tracks only
/// the *sizes* of T1/T2/B1/B2 and the tuning parameter `p`; actual key
/// storage is left to the caller.
#[pyclass(name = "ArcTracker")]
pub struct PyArcTracker {
    inner: core::ArcTracker,
}

#[pymethods]
impl PyArcTracker {
    #[new]
    fn new(capacity: usize) -> Self {
        Self {
            inner: core::ArcTracker::new(capacity),
        }
    }

    #[getter]
    fn t1_size(&self) -> usize {
        self.inner.t1_size
    }

    #[getter]
    fn t2_size(&self) -> usize {
        self.inner.t2_size
    }

    #[getter]
    fn b1_size(&self) -> usize {
        self.inner.b1_size
    }

    #[getter]
    fn b2_size(&self) -> usize {
        self.inner.b2_size
    }

    #[getter]
    fn p(&self) -> usize {
        self.inner.p
    }

    fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Increase `p` on a B1 (recency-ghost) hit.
    fn adapt_on_hit_b1(&mut self) {
        self.inner.adapt_on_hit_b1();
    }

    /// Decrease `p` on a B2 (frequency-ghost) hit.
    fn adapt_on_hit_b2(&mut self) {
        self.inner.adapt_on_hit_b2();
    }

    fn on_admit_t1(&mut self) {
        self.inner.on_admit_t1();
    }

    fn on_promote_t1_to_t2(&mut self) {
        self.inner.on_promote_t1_to_t2();
    }

    fn on_evict_t1(&mut self) {
        self.inner.on_evict_t1();
    }

    fn on_evict_t2(&mut self) {
        self.inner.on_evict_t2();
    }

    fn on_remove_b1_ghost(&mut self) {
        self.inner.on_remove_b1_ghost();
    }

    fn on_remove_b2_ghost(&mut self) {
        self.inner.on_remove_b2_ghost();
    }

    fn live_size(&self) -> usize {
        self.inner.live_size()
    }

    fn ghost_size(&self) -> usize {
        self.inner.ghost_size()
    }

    fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// `True` to evict from T1, `False` to evict from T2, under the ARC
    /// policy.
    fn should_evict_t1(&self) -> bool {
        self.inner.should_evict_t1()
    }

    fn __repr__(&self) -> String {
        format!(
            "ArcTracker(t1={}, t2={}, b1={}, b2={}, p={})",
            self.inner.t1_size,
            self.inner.t2_size,
            self.inner.b1_size,
            self.inner.b2_size,
            self.inner.p
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFrequencyCounter>()?;
    m.add_class::<PyLfuEvictionTracker>()?;
    m.add_class::<PyTinyLfuAdmission>()?;
    m.add_class::<PyArcTracker>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_counter_increments_and_decays() {
        let mut fc = PyFrequencyCounter::new(100);
        fc.increment(42);
        fc.increment(42);
        assert_eq!(fc.frequency(42), 2);
        fc.decay_all();
        assert_eq!(fc.frequency(42), 1);
    }

    #[test]
    fn lfu_tracker_evicts_lowest_frequency() {
        let mut t = PyLfuEvictionTracker::new();
        t.insert(10);
        t.insert(20);
        t.promote(10);
        assert_eq!(t.evict(), Some(20));
        assert_eq!(t.__len__(), 1);
    }

    #[test]
    fn tiny_lfu_admits_popular_rejects_cold() {
        let mut gate = PyTinyLfuAdmission::new(100);
        for _ in 0..20 {
            gate.record_access(42);
        }
        assert!(gate.should_admit(42, 1));
        assert!(!gate.should_admit(999, 10));
    }

    #[test]
    fn arc_tracker_promotion_and_adaptation() {
        let mut arc = PyArcTracker::new(10);
        arc.on_admit_t1();
        arc.on_promote_t1_to_t2();
        assert_eq!(arc.t1_size(), 0);
        assert_eq!(arc.t2_size(), 1);

        let mut arc2 = PyArcTracker::new(100);
        arc2.on_admit_t1();
        arc2.on_evict_t1();
        // simulate a couple of B2 ghosts so adapt_on_hit_b1 has something to divide by.
        arc2.on_admit_t1();
        arc2.on_promote_t1_to_t2();
        arc2.on_evict_t2();
        arc2.on_evict_t2();
        let p_before = arc2.p();
        arc2.adapt_on_hit_b1();
        assert!(arc2.p() > p_before);
    }
}

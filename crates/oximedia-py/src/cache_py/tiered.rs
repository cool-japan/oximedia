//! `oximedia.cache` multi-tier cache bindings — real delegation to
//! [`oximedia_cache::tiered_cache`].
//!
//! Exposes L1/L2/... in-memory tiers with pluggable per-tier eviction
//! (LRU/LFU/FIFO/Random/TinyLFU), frequency-gated promotion, and optional
//! P²-adaptive promotion thresholds / arena allocation.
//!
//! **Scope note**: [`oximedia_cache::tiered_cache::TierConfig::disk`] (a
//! real file-backed tier) is intentionally *not* exposed here. Its `Drop`
//! impl deletes the backing directory on teardown, which is surprising
//! behaviour to hand to a caller-supplied path from a scripting language.
//! Use in-memory tiers from Python; build disk-backed tiers from Rust.

use oximedia_cache::tiered_cache as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

fn parse_eviction_policy(policy: &str) -> PyResult<core::EvictionPolicy> {
    match policy {
        "lru" => Ok(core::EvictionPolicy::Lru),
        "lfu" => Ok(core::EvictionPolicy::Lfu),
        "fifo" => Ok(core::EvictionPolicy::Fifo),
        "random" => Ok(core::EvictionPolicy::Random),
        "tiny_lfu" => Ok(core::EvictionPolicy::TinyLfu),
        other => Err(PyValueError::new_err(format!(
            "unknown eviction policy {other:?}; expected 'lru', 'lfu', 'fifo', 'random', or \
             'tiny_lfu'"
        ))),
    }
}

fn eviction_policy_name(policy: &core::EvictionPolicy) -> &'static str {
    match policy {
        core::EvictionPolicy::Lru => "lru",
        core::EvictionPolicy::Lfu => "lfu",
        core::EvictionPolicy::Fifo => "fifo",
        core::EvictionPolicy::Random => "random",
        core::EvictionPolicy::TinyLfu => "tiny_lfu",
    }
}

// ---------------------------------------------------------------------------
// TierConfig
// ---------------------------------------------------------------------------

/// Configuration for one in-memory cache tier.
#[pyclass(name = "TierConfig")]
#[derive(Clone)]
pub struct PyTierConfig {
    inner: core::TierConfig,
}

#[pymethods]
impl PyTierConfig {
    /// A minimal in-memory tier with LRU eviction and the given byte
    /// capacity.
    #[staticmethod]
    fn memory(name: String, capacity_bytes: usize) -> Self {
        Self {
            inner: core::TierConfig::memory(name, capacity_bytes),
        }
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[getter]
    fn capacity_bytes(&self) -> usize {
        self.inner.capacity_bytes
    }

    #[getter]
    fn eviction_policy(&self) -> &'static str {
        eviction_policy_name(&self.inner.eviction_policy)
    }

    /// Set the eviction policy: `"lru"`, `"lfu"`, `"fifo"`, `"random"`, or
    /// `"tiny_lfu"`.
    fn set_eviction_policy(&mut self, policy: &str) -> PyResult<()> {
        self.inner.eviction_policy = parse_eviction_policy(policy)?;
        Ok(())
    }

    #[getter]
    fn promotion_threshold(&self) -> u64 {
        self.inner.promotion_threshold
    }

    /// Minimum access frequency in this tier before a hit promotes the
    /// entry to the tier above (`0` = always promote).
    fn set_promotion_threshold(&mut self, threshold: u64) {
        self.inner.promotion_threshold = threshold;
    }

    #[getter]
    fn compress(&self) -> bool {
        self.inner.compress
    }

    fn set_compress(&mut self, enabled: bool) {
        self.inner.compress = enabled;
    }

    /// Enable P²-adaptive promotion threshold tuning (75th-percentile of
    /// observed per-key access frequency, after a 5-observation warm-up).
    fn enable_adaptive_promotion(&mut self, enabled: bool) {
        self.inner.adaptive_promotion = enabled;
    }

    #[getter]
    fn adaptive_promotion(&self) -> bool {
        self.inner.adaptive_promotion
    }

    /// Store tier entry bytes in a bump arena instead of individually owned
    /// `Vec<u8>`s.
    fn enable_arena(&mut self, enabled: bool) {
        self.inner.use_arena = enabled;
    }

    #[getter]
    fn use_arena(&self) -> bool {
        self.inner.use_arena
    }

    fn __repr__(&self) -> String {
        format!(
            "TierConfig(name={:?}, capacity_bytes={}, eviction_policy={:?})",
            self.inner.name,
            self.inner.capacity_bytes,
            self.eviction_policy()
        )
    }
}

// ---------------------------------------------------------------------------
// TierStats / TieredCacheStats
// ---------------------------------------------------------------------------

/// Per-tier statistics snapshot.
#[pyclass(name = "TierStats")]
pub struct PyTierStats {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub hits: u64,
    #[pyo3(get)]
    pub size_used_bytes: usize,
    #[pyo3(get)]
    pub entry_count: usize,
    #[pyo3(get)]
    pub promotions: u64,
    #[pyo3(get)]
    pub compressions: u64,
}

impl From<&core::TierStats> for PyTierStats {
    fn from(s: &core::TierStats) -> Self {
        Self {
            name: s.name.clone(),
            hits: s.hits,
            size_used_bytes: s.size_used_bytes,
            entry_count: s.entry_count,
            promotions: s.promotions,
            compressions: s.compressions,
        }
    }
}

#[pymethods]
impl PyTierStats {
    fn __repr__(&self) -> String {
        format!(
            "TierStats(name={:?}, hits={}, entry_count={})",
            self.name, self.hits, self.entry_count
        )
    }
}

/// Aggregate statistics snapshot for a whole [`PyTieredCache`].
#[pyclass(name = "TieredCacheStats")]
pub struct PyTieredCacheStats {
    inner: core::TieredCacheStats,
}

#[pymethods]
impl PyTieredCacheStats {
    #[getter]
    fn total_hits(&self) -> u64 {
        self.inner.total_hits
    }

    #[getter]
    fn total_misses(&self) -> u64 {
        self.inner.total_misses
    }

    #[getter]
    fn hit_rate(&self) -> f64 {
        self.inner.hit_rate
    }

    fn tier_stats(&self) -> Vec<PyTierStats> {
        self.inner
            .tier_stats
            .iter()
            .map(PyTierStats::from)
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "TieredCacheStats(total_hits={}, total_misses={}, hit_rate={:.3})",
            self.inner.total_hits, self.inner.total_misses, self.inner.hit_rate
        )
    }
}

// ---------------------------------------------------------------------------
// TieredCache
// ---------------------------------------------------------------------------

/// Multi-tier cache. Reads check tiers in order (L1 first); a hit in a
/// lower tier promotes the entry upward once its access frequency meets the
/// tier's (possibly adaptive) promotion threshold. Writes always target L1.
#[pyclass(name = "TieredCache")]
pub struct PyTieredCache {
    inner: core::TieredCache,
}

#[pymethods]
impl PyTieredCache {
    /// `tiers[0]` is L1 (fastest/smallest), last is the slowest.
    #[new]
    fn new(tiers: Vec<PyRef<'_, PyTierConfig>>) -> Self {
        let configs: Vec<core::TierConfig> = tiers.iter().map(|t| t.inner.clone()).collect();
        Self {
            inner: core::TieredCache::new(configs),
        }
    }

    /// Look up `key` across all tiers, promoting on a qualifying lower-tier
    /// hit. Returns `None` on a total miss.
    fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        self.inner.get(key)
    }

    /// Insert `(key, data)` into L1.
    fn put(&mut self, key: &str, data: Vec<u8>) {
        self.inner.put(key, data);
    }

    /// Insert `(key, data)` directly into tier `tier_idx` (e.g. to
    /// pre-populate a lower tier from a warm-up snapshot).
    fn put_at_tier(&mut self, tier_idx: usize, key: &str, data: Vec<u8>) {
        self.inner.put_at_tier(tier_idx, key, data);
    }

    /// Evict one entry from tier `tier_idx` per that tier's policy.
    fn evict_tier(&mut self, tier_idx: usize) -> Option<(String, Vec<u8>)> {
        self.inner.evict_tier(tier_idx)
    }

    fn stats(&self) -> PyTieredCacheStats {
        PyTieredCacheStats {
            inner: self.inner.stats(),
        }
    }

    /// Bulk-insert `entries` into L1 without triggering eviction.
    fn warmup(&mut self, entries: Vec<(String, Vec<u8>)>) {
        self.inner.warmup(&entries);
    }

    /// Remove `key` from every tier. Returns `True` if found in at least
    /// one.
    fn invalidate(&mut self, key: &str) -> bool {
        self.inner.invalidate(key)
    }

    fn tier_count(&self) -> usize {
        self.inner.tier_count()
    }

    fn tier_promotions(&self, tier_idx: usize) -> u64 {
        self.inner.tier_promotions(tier_idx)
    }

    fn tier_hit_count(&self, tier_idx: usize) -> u64 {
        self.inner.tier_hit_count(tier_idx)
    }

    /// Reset the bump arena of tier `tier_idx` (only meaningful when that
    /// tier was built with `enable_arena(True)`); invalidates previously
    /// stored arena handles.
    fn reset_tier_arena(&mut self, tier_idx: usize) {
        self.inner.reset_tier_arena(tier_idx);
    }

    fn __repr__(&self) -> String {
        format!("TieredCache(tiers={})", self.inner.tier_count())
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyTierConfig>()?;
    m.add_class::<PyTierStats>()?;
    m.add_class::<PyTieredCacheStats>()?;
    m.add_class::<PyTieredCache>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn two_tier_config(l1_bytes: usize, l2_bytes: usize) -> Vec<core::TierConfig> {
        let mut l1 = PyTierConfig::memory("L1".to_string(), l1_bytes);
        l1.set_eviction_policy("lru").expect("valid policy");
        let mut l2 = PyTierConfig::memory("L2".to_string(), l2_bytes);
        l2.set_eviction_policy("lfu").expect("valid policy");
        vec![l1.inner, l2.inner]
    }

    #[test]
    fn tier_config_memory_defaults() {
        let cfg = PyTierConfig::memory("L1".to_string(), 4096);
        assert_eq!(cfg.name(), "L1");
        assert_eq!(cfg.capacity_bytes(), 4096);
        assert_eq!(cfg.eviction_policy(), "lru");
        assert!(!cfg.compress());
    }

    #[test]
    fn tier_config_rejects_unknown_policy() {
        let mut cfg = PyTierConfig::memory("L1".to_string(), 4096);
        assert!(cfg.set_eviction_policy("bogus").is_err());
    }

    #[test]
    fn tiered_cache_put_get_roundtrip() {
        let mut cache = PyTieredCache {
            inner: core::TieredCache::new(two_tier_config(1024, 4096)),
        };
        cache.put("key1", b"hello".to_vec());
        assert_eq!(cache.get("key1"), Some(b"hello".to_vec()));
        assert_eq!(cache.stats().total_hits(), 1);
    }

    #[test]
    fn tiered_cache_miss_and_invalidate() {
        let mut cache = PyTieredCache {
            inner: core::TieredCache::new(two_tier_config(1024, 4096)),
        };
        assert_eq!(cache.get("absent"), None);
        cache.put("x", b"data".to_vec());
        assert!(cache.invalidate("x"));
        assert_eq!(cache.get("x"), None);
    }

    #[test]
    fn tiered_cache_warmup_and_stats() {
        let mut cache = PyTieredCache {
            inner: core::TieredCache::new(two_tier_config(1024, 4096)),
        };
        cache.warmup(vec![
            ("a".to_string(), b"AAA".to_vec()),
            ("b".to_string(), b"BBB".to_vec()),
        ]);
        assert_eq!(cache.get("a"), Some(b"AAA".to_vec()));
        let stats = cache.stats();
        assert_eq!(stats.tier_stats()[0].entry_count, 2);
    }
}

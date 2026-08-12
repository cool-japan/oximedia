//! `oximedia.cache` submodule — Python bindings for `oximedia-cache`.
//!
//! Wraps the arena-backed LRU cache (`oximedia_cache::lru_cache::LruCache`)
//! behind a PyO3 class with real delegation, keyed by UTF-8 strings and
//! valued by opaque byte buffers (`bytes`). Unlike the WASM `WasmLruCache`
//! binding — a standalone reimplementation that avoids `std::time::Instant`
//! because that type is unavailable in the browser — this binding drives the
//! actual production cache implementation, including TTL expiration and
//! eviction-proof pinning, which the WASM binding does not have.

use oximedia_cache::lru_cache::{CacheStats, LruCache};
use pyo3::prelude::*;
use std::time::Duration;

// Family submodules — each owns a `register(m)` that adds its classes and
// functions directly into the `oximedia.cache` Python namespace built by
// `register_submodule` below. Living under `cache_py/` (Rust 2018+ file
// module + sibling directory) keeps every family's binding code out of this
// file without requiring any change to `lib.rs`.
mod bloom;
mod content_aware_write_behind;
mod distributed;
mod eviction;
mod tiered;
mod warming;

// ---------------------------------------------------------------------------
// LruCache
// ---------------------------------------------------------------------------

/// Arena-backed O(1) LRU cache with TTL expiration, entry pinning, and
/// hit/miss/eviction statistics.
///
/// Keys are UTF-8 strings; values are opaque byte buffers. Real delegation
/// to `oximedia_cache::lru_cache::LruCache<String, Vec<u8>>`.
#[pyclass(name = "LruCache")]
pub struct PyLruCache {
    inner: LruCache<String, Vec<u8>>,
}

#[pymethods]
impl PyLruCache {
    /// Create a new LRU cache with the given entry capacity.
    #[new]
    fn new(capacity: usize) -> Self {
        Self {
            inner: LruCache::new(capacity),
        }
    }

    /// Insert or update `key` with `value`. The entry's tracked size is
    /// `len(value)` bytes. Evicts the least-recently-used entry first if the
    /// cache is at capacity.
    fn put(&mut self, key: String, value: Vec<u8>) {
        let size = value.len();
        self.inner.insert(key, value, size);
    }

    /// Insert `key` with an explicit TTL in milliseconds; the entry is
    /// lazily evicted the first time it is accessed after expiring (or
    /// eagerly via `purge_expired`).
    fn put_with_ttl_ms(&mut self, key: String, value: Vec<u8>, ttl_ms: u64) {
        let size = value.len();
        self.inner
            .insert_with_ttl(key, value, size, Duration::from_millis(ttl_ms));
    }

    /// Insert a pinned entry that will not be evicted by LRU pressure until
    /// explicitly unpinned (or removed).
    fn put_pinned(&mut self, key: String, value: Vec<u8>) {
        let size = value.len();
        self.inner.insert_pinned(key, value, size);
    }

    /// Look up `key`, promoting it to most-recently-used. Returns `None` on
    /// a miss (or if the entry's TTL has expired).
    fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        self.inner.get(&key.to_string()).cloned()
    }

    /// Look up `key` without affecting LRU order or hit/miss statistics.
    fn peek(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.peek(&key.to_string()).cloned()
    }

    /// Remove and return the value for `key`, if present.
    fn remove(&mut self, key: &str) -> Option<Vec<u8>> {
        self.inner.remove(&key.to_string())
    }

    /// Return `True` if `key` is currently in the cache.
    fn contains(&self, key: &str) -> bool {
        self.inner.contains(&key.to_string())
    }

    /// Pin `key` so it survives LRU eviction. Returns `True` if the key was
    /// found.
    fn pin(&mut self, key: &str) -> bool {
        self.inner.pin(&key.to_string())
    }

    /// Unpin `key`, making it eligible for LRU eviction again. Returns
    /// `True` if the key was found.
    fn unpin(&mut self, key: &str) -> bool {
        self.inner.unpin(&key.to_string())
    }

    /// Return `True` if `key` is currently pinned.
    fn is_pinned(&self, key: &str) -> bool {
        self.inner.is_pinned(&key.to_string())
    }

    /// Eagerly purge all TTL-expired entries. Returns the number removed.
    fn purge_expired(&mut self) -> usize {
        self.inner.purge_expired()
    }

    /// Resize the cache's capacity, evicting excess unpinned entries first
    /// if shrinking. Returns the number of entries evicted.
    fn resize(&mut self, new_capacity: usize) -> usize {
        self.inner.resize(new_capacity)
    }

    /// Remove all entries and reset statistics.
    fn clear(&mut self) {
        self.inner.clear();
    }

    /// Maximum number of entries before eviction.
    fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Number of entries currently resident (`len(cache)`).
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// `True` if the cache has no entries.
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Snapshot of hit/miss/eviction/TTL/pin statistics.
    fn stats(&self) -> PyCacheStats {
        self.inner.stats().into()
    }

    fn __repr__(&self) -> String {
        format!(
            "LruCache(len={}, capacity={})",
            self.inner.len(),
            self.inner.capacity()
        )
    }
}

// ---------------------------------------------------------------------------
// CacheStats
// ---------------------------------------------------------------------------

/// Point-in-time snapshot of [`PyLruCache`] statistics.
#[pyclass(name = "CacheStats")]
pub struct PyCacheStats {
    /// Total number of successful cache lookups.
    #[pyo3(get)]
    pub hits: u64,
    /// Total number of failed cache lookups.
    #[pyo3(get)]
    pub misses: u64,
    /// Total number of entries evicted to make room for new ones.
    #[pyo3(get)]
    pub evictions: u64,
    /// Sum of tracked sizes (bytes) for all currently resident entries.
    #[pyo3(get)]
    pub total_size_bytes: usize,
    /// Maximum number of entries before eviction.
    #[pyo3(get)]
    pub capacity: usize,
    /// Number of entries currently resident.
    #[pyo3(get)]
    pub entry_count: usize,
    /// Number of entries that expired via TTL.
    #[pyo3(get)]
    pub ttl_expirations: u64,
    /// Number of pinned entries currently in the cache.
    #[pyo3(get)]
    pub pinned_count: usize,
}

impl From<CacheStats> for PyCacheStats {
    fn from(s: CacheStats) -> Self {
        Self {
            hits: s.hits,
            misses: s.misses,
            evictions: s.evictions,
            total_size_bytes: s.total_size_bytes,
            capacity: s.capacity,
            entry_count: s.entry_count,
            ttl_expirations: s.ttl_expirations,
            pinned_count: s.pinned_count,
        }
    }
}

#[pymethods]
impl PyCacheStats {
    /// Hit rate in `0.0-1.0`; `0.0` if there have been no lookups at all.
    fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "CacheStats(hits={}, misses={}, evictions={}, entry_count={}/{})",
            self.hits, self.misses, self.evictions, self.entry_count, self.capacity
        )
    }
}

// Bound in `cache_py/`: tiered_cache (`tiered`), bloom_filter (`bloom`),
// distributed_cache (`distributed`), cache_warming (`warming`),
// eviction_policies (`eviction`), content_aware_cache + write_behind_cache
// (`content_aware_write_behind`).
//
// TODO(0.2.x): expose oximedia_cache::two_queue (2Q scan-resistant policy),
// sharded_lru, cache_partitioning, cache_serialization, slab_allocator,
// prefetch, cache_metrics, admission_filter, adaptive, negative,
// segment_cache, weighted_cache, write_through, and tier_compressor.

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the `oximedia.cache` submodule.
pub fn register_submodule(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "cache")?;

    m.add_class::<PyLruCache>()?;
    m.add_class::<PyCacheStats>()?;

    // Family submodules (each adds its own classes/functions into `m`).
    tiered::register(&m)?;
    bloom::register(&m)?;
    distributed::register(&m)?;
    warming::register(&m)?;
    eviction::register(&m)?;
    content_aware_write_behind::register(&m)?;

    parent.add_submodule(&m)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get_roundtrip() {
        let mut cache = PyLruCache::new(4);
        cache.put("a".to_string(), vec![1, 2, 3]);
        assert_eq!(cache.get("a"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn miss_returns_none() {
        let mut cache = PyLruCache::new(4);
        assert_eq!(cache.get("missing"), None);
    }

    #[test]
    fn eviction_at_capacity() {
        let mut cache = PyLruCache::new(2);
        cache.put("a".to_string(), vec![1]);
        cache.put("b".to_string(), vec![2]);
        cache.put("c".to_string(), vec![3]); // evicts "a" (LRU)
        assert!(!cache.contains("a"), "a should have been evicted");
        assert!(cache.contains("b"));
        assert!(cache.contains("c"));
        assert_eq!(cache.__len__(), 2);
    }

    #[test]
    fn len_and_is_empty() {
        let mut cache = PyLruCache::new(4);
        assert!(cache.is_empty());
        cache.put("x".to_string(), vec![0]);
        assert_eq!(cache.__len__(), 1);
        assert!(!cache.is_empty());
    }

    #[test]
    fn remove_deletes_entry() {
        let mut cache = PyLruCache::new(4);
        cache.put("x".to_string(), vec![42]);
        assert_eq!(cache.remove("x"), Some(vec![42]));
        assert!(!cache.contains("x"));
    }

    #[test]
    fn stats_track_hits_and_misses() {
        let mut cache = PyLruCache::new(4);
        cache.put("a".to_string(), vec![1]);
        cache.get("a");
        cache.get("a");
        cache.get("nope");
        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn hit_rate_with_no_lookups_is_zero() {
        let cache = PyLruCache::new(4);
        assert_eq!(cache.stats().hit_rate(), 0.0);
    }

    #[test]
    fn pin_survives_eviction() {
        let mut cache = PyLruCache::new(2);
        cache.put_pinned("keep".to_string(), vec![1]);
        cache.put("b".to_string(), vec![2]);
        cache.put("c".to_string(), vec![3]); // would evict "keep" if unpinned
        assert!(cache.contains("keep"), "pinned entry must survive eviction");
    }

    #[test]
    fn unpin_allows_eviction() {
        let mut cache = PyLruCache::new(1);
        cache.put_pinned("a".to_string(), vec![1]);
        assert!(cache.unpin("a"));
        assert!(!cache.is_pinned("a"));
    }

    #[test]
    fn ttl_expires_lazily_on_get() {
        let mut cache = PyLruCache::new(4);
        cache.put_with_ttl_ms("ephemeral".to_string(), vec![9], 0);
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(cache.get("ephemeral"), None, "expired entry should be gone");
    }

    #[test]
    fn purge_expired_counts_removed() {
        let mut cache = PyLruCache::new(4);
        cache.put_with_ttl_ms("a".to_string(), vec![1], 0);
        cache.put("b".to_string(), vec![2]);
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(cache.purge_expired(), 1);
        assert_eq!(cache.__len__(), 1);
    }

    #[test]
    fn resize_shrinks_and_evicts() {
        let mut cache = PyLruCache::new(10);
        for i in 0..10 {
            cache.put(format!("k{i}"), vec![i as u8]);
        }
        let evicted = cache.resize(5);
        assert_eq!(evicted, 5);
        assert_eq!(cache.capacity(), 5);
        assert_eq!(cache.__len__(), 5);
    }

    #[test]
    fn clear_resets_cache() {
        let mut cache = PyLruCache::new(4);
        cache.put("a".to_string(), vec![1]);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.stats().total_size_bytes, 0);
    }

    #[test]
    fn peek_does_not_affect_lru_order() {
        let mut cache = PyLruCache::new(2);
        cache.put("a".to_string(), vec![1]);
        cache.put("b".to_string(), vec![2]);
        let _ = cache.peek("a");
        cache.put("c".to_string(), vec![3]); // should still evict "a" (LRU tail)
        assert!(!cache.contains("a"));
    }
}

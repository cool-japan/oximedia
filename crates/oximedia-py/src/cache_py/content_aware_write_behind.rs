//! `oximedia.cache` media-content-aware cache and write-behind cache
//! bindings — real delegation to
//! [`oximedia_cache::content_aware_cache`] and
//! [`oximedia_cache::write_behind_cache`].
//!
//! **Write-behind backing store**: [`oximedia_cache::write_behind_cache::BackingStore`]
//! is a generic trait with associated types, which cannot be exposed to
//! Python directly. [`PyWriteBehindCache`] is backed by a concrete
//! in-process `HashMap<String, bytes>` store (real dirty tracking, real
//! flush semantics against that store — nothing here is faked); use
//! [`PyWriteBehindCache::store_snapshot`] to observe what has actually been
//! flushed.

use std::collections::HashMap;
use std::convert::Infallible;

use oximedia_cache::content_aware_cache as cac;
use oximedia_cache::write_behind_cache as wbc;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// MediaContentType
// ---------------------------------------------------------------------------

/// The media type of a cached entry; determines default priority and TTL.
/// Construct with one of the static factory methods.
#[pyclass(name = "MediaContentType")]
#[derive(Clone)]
pub struct PyMediaContentType {
    inner: cac::MediaContentType,
}

#[pymethods]
impl PyMediaContentType {
    #[staticmethod]
    fn video_segment(bitrate: u32, codec: String) -> Self {
        Self {
            inner: cac::MediaContentType::VideoSegment { bitrate, codec },
        }
    }

    #[staticmethod]
    fn audio_segment(bitrate: u32) -> Self {
        Self {
            inner: cac::MediaContentType::AudioSegment { bitrate },
        }
    }

    #[staticmethod]
    fn image(width: u32, height: u32) -> Self {
        Self {
            inner: cac::MediaContentType::Image { width, height },
        }
    }

    #[staticmethod]
    fn manifest() -> Self {
        Self {
            inner: cac::MediaContentType::Manifest,
        }
    }

    #[staticmethod]
    fn thumbnail() -> Self {
        Self {
            inner: cac::MediaContentType::Thumbnail,
        }
    }

    #[staticmethod]
    fn metadata() -> Self {
        Self {
            inner: cac::MediaContentType::Metadata,
        }
    }

    /// Discriminant tag: `"VideoSegment"`, `"AudioSegment"`, `"Image"`,
    /// `"Manifest"`, `"Thumbnail"`, or `"Metadata"` — the same tag used
    /// internally as the key into [`PyScoringWeights`]'s per-type priority
    /// overrides.
    fn kind(&self) -> &'static str {
        match self.inner {
            cac::MediaContentType::VideoSegment { .. } => "VideoSegment",
            cac::MediaContentType::AudioSegment { .. } => "AudioSegment",
            cac::MediaContentType::Image { .. } => "Image",
            cac::MediaContentType::Manifest => "Manifest",
            cac::MediaContentType::Thumbnail => "Thumbnail",
            cac::MediaContentType::Metadata => "Metadata",
        }
    }

    /// Default numeric priority for this type (higher = kept longer);
    /// real delegation to `ContentCachePriority::for_type(..).0`.
    fn priority(&self) -> u8 {
        cac::ContentCachePriority::for_type(&self.inner).0
    }

    /// Recommended TTL for this type, in milliseconds.
    fn ttl_ms(&self) -> u64 {
        cac::ttl_for_type(&self.inner).as_millis() as u64
    }

    fn __repr__(&self) -> String {
        format!("MediaContentType::{}", self.kind())
    }
}

// ---------------------------------------------------------------------------
// ScoringWeights
// ---------------------------------------------------------------------------

/// Configurable exponent weights for [`PyCacheEntry::score_for_eviction_weighted`].
#[pyclass(name = "ScoringWeights")]
#[derive(Clone)]
pub struct PyScoringWeights {
    inner: cac::ScoringWeights,
}

#[pymethods]
impl PyScoringWeights {
    #[new]
    #[pyo3(signature = (recency_exp=1.0, priority_exp=1.0, size_exp=1.0))]
    fn new(recency_exp: f64, priority_exp: f64, size_exp: f64) -> Self {
        Self {
            inner: cac::ScoringWeights {
                recency_exp,
                priority_exp,
                size_exp,
                per_type_priority: HashMap::new(),
            },
        }
    }

    #[getter]
    fn recency_exp(&self) -> f64 {
        self.inner.recency_exp
    }

    #[getter]
    fn priority_exp(&self) -> f64 {
        self.inner.priority_exp
    }

    #[getter]
    fn size_exp(&self) -> f64 {
        self.inner.size_exp
    }

    /// Override the priority multiplier for every [`PyMediaContentType`]
    /// sharing `content_type.kind()`. Values `< 1.0` make the type harder
    /// to evict; `> 1.0` make it easier.
    fn set_type_priority_multiplier(&mut self, content_type: &PyMediaContentType, multiplier: f64) {
        self.inner
            .per_type_priority
            .insert(content_type.kind().to_string(), multiplier);
    }

    fn priority_multiplier(&self, content_type: &PyMediaContentType) -> f64 {
        self.inner.priority_multiplier(&content_type.inner)
    }
}

// ---------------------------------------------------------------------------
// CacheEntry (owned snapshot; ContentAwareCache::get returns a borrow that
// cannot cross the Python boundary, so this is always a clone of the live
// entry at the moment it was read).
// ---------------------------------------------------------------------------

/// Snapshot of one entry held in a [`PyContentAwareCache`].
#[pyclass(name = "CacheEntry")]
pub struct PyCacheEntry {
    #[pyo3(get)]
    pub key: String,
    #[pyo3(get)]
    pub data: Vec<u8>,
    #[pyo3(get)]
    pub content_type: PyMediaContentType,
    #[pyo3(get)]
    pub access_count: u32,
    #[pyo3(get)]
    pub size_bytes: usize,
    /// Milliseconds since this entry was first inserted, at snapshot time.
    #[pyo3(get)]
    pub age_ms: u64,
    /// Milliseconds since this entry was last successfully looked up, at
    /// snapshot time.
    #[pyo3(get)]
    pub idle_ms: u64,
    inner_score_source: cac::CacheEntry,
}

impl From<cac::CacheEntry> for PyCacheEntry {
    fn from(e: cac::CacheEntry) -> Self {
        Self {
            key: e.key.clone(),
            data: e.data.clone(),
            content_type: PyMediaContentType {
                inner: e.content_type.clone(),
            },
            access_count: e.access_count,
            size_bytes: e.size_bytes,
            age_ms: e.inserted_at.elapsed().as_millis() as u64,
            idle_ms: e.last_accessed.elapsed().as_millis() as u64,
            inner_score_source: e,
        }
    }
}

#[pymethods]
impl PyCacheEntry {
    /// Eviction score using default weights (higher = better eviction
    /// candidate).
    fn score_for_eviction(&self) -> f32 {
        self.inner_score_source.score_for_eviction()
    }

    /// Eviction score using custom [`PyScoringWeights`].
    fn score_for_eviction_weighted(&self, weights: &PyScoringWeights) -> f32 {
        self.inner_score_source
            .score_for_eviction_weighted(&weights.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "CacheEntry(key={:?}, size_bytes={}, access_count={})",
            self.key, self.size_bytes, self.access_count
        )
    }
}

// ---------------------------------------------------------------------------
// ContentAwareCache
// ---------------------------------------------------------------------------

/// Media-aware cache that scores eviction candidates by
/// recency × priority × size rather than pure LRU order.
#[pyclass(name = "ContentAwareCache")]
pub struct PyContentAwareCache {
    inner: cac::ContentAwareCache,
}

#[pymethods]
impl PyContentAwareCache {
    /// `max_bytes`, if given, adds a byte-level capacity on top of the
    /// entry-count `capacity`.
    #[new]
    #[pyo3(signature = (capacity, max_bytes=None))]
    fn new(capacity: usize, max_bytes: Option<usize>) -> Self {
        let mut inner = cac::ContentAwareCache::new(capacity);
        if let Some(max_bytes) = max_bytes {
            inner = inner.with_max_bytes(max_bytes);
        }
        Self { inner }
    }

    fn set_scoring_weights(&mut self, weights: &PyScoringWeights) {
        self.inner.set_scoring_weights(weights.inner.clone());
    }

    fn scoring_weights(&self) -> PyScoringWeights {
        PyScoringWeights {
            inner: self.inner.scoring_weights().clone(),
        }
    }

    /// Insert a media entry, evicting the highest-scoring candidate first
    /// if at capacity.
    fn insert_media(&mut self, key: String, data: Vec<u8>, content_type: &PyMediaContentType) {
        self.inner
            .insert_media(key, data, content_type.inner.clone());
    }

    /// Look up `key`, updating its access metadata. Returns a snapshot, not
    /// a live reference.
    fn get(&mut self, key: &str) -> Option<PyCacheEntry> {
        self.inner.get(key).cloned().map(PyCacheEntry::from)
    }

    /// Look up `key` without updating access metadata or LRU order.
    fn peek(&self, key: &str) -> Option<PyCacheEntry> {
        self.inner.peek(key).cloned().map(PyCacheEntry::from)
    }

    fn remove(&mut self, key: &str) -> bool {
        self.inner.remove(key)
    }

    /// Remove every entry whose TTL (per [`PyMediaContentType::ttl_ms`]) has
    /// elapsed. Returns the number evicted.
    fn evict_expired(&mut self) -> usize {
        self.inner.evict_expired()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn total_bytes(&self) -> usize {
        self.inner.total_bytes()
    }

    fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    fn __repr__(&self) -> String {
        format!(
            "ContentAwareCache(len={}, capacity={}, total_bytes={})",
            self.inner.len(),
            self.inner.capacity(),
            self.inner.total_bytes()
        )
    }
}

// ---------------------------------------------------------------------------
// WriteBehindCache (in-process HashMap backing store — see module docs)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MemoryStore {
    data: HashMap<String, Vec<u8>>,
}

impl wbc::BackingStore for MemoryStore {
    type Key = String;
    type Value = Vec<u8>;
    type Error = Infallible;

    fn write(&mut self, key: &String, value: &Vec<u8>) -> Result<(), Infallible> {
        self.data.insert(key.clone(), value.clone());
        Ok(())
    }

    fn read(&self, key: &String) -> Result<Option<Vec<u8>>, Infallible> {
        Ok(self.data.get(key).cloned())
    }

    fn delete(&mut self, key: &String) -> Result<(), Infallible> {
        self.data.remove(key);
        Ok(())
    }
}

/// The in-memory store can never fail, so this conversion is unreachable in
/// practice; it exists only to satisfy the `Result<_, WriteBehindError<E>>`
/// return type. Matching on `Infallible` (an uninhabited type) is total —
/// no panic path exists here.
fn wb_err(e: wbc::WriteBehindError<Infallible>) -> PyErr {
    match e {
        wbc::WriteBehindError::StoreError(never) => match never {},
    }
}

/// Snapshot of write-behind cache statistics.
#[pyclass(name = "WriteBehindStats")]
pub struct PyWriteBehindStats {
    #[pyo3(get)]
    pub entry_count: usize,
    #[pyo3(get)]
    pub dirty_count: usize,
    #[pyo3(get)]
    pub capacity: usize,
    #[pyo3(get)]
    pub total_flushes: u64,
    #[pyo3(get)]
    pub total_entries_flushed: u64,
}

impl From<wbc::WriteBehindStats> for PyWriteBehindStats {
    fn from(s: wbc::WriteBehindStats) -> Self {
        Self {
            entry_count: s.entry_count,
            dirty_count: s.dirty_count,
            capacity: s.capacity,
            total_flushes: s.total_flushes,
            total_entries_flushed: s.total_entries_flushed,
        }
    }
}

/// Write-behind (write-back) cache backed by an in-process `dict`-like
/// store. Writes are marked dirty; call [`flush`](Self::flush) (or
/// `flush_if_needed`/`flush_older_than`) to persist them. Entries evicted
/// under capacity pressure are flushed first, so data is never silently
/// lost.
#[pyclass(name = "WriteBehindCache")]
pub struct PyWriteBehindCache {
    inner: wbc::WriteBehindCache<MemoryStore>,
}

#[pymethods]
impl PyWriteBehindCache {
    #[new]
    fn new(capacity: usize) -> Self {
        Self {
            inner: wbc::WriteBehindCache::new(capacity, MemoryStore::default()),
        }
    }

    /// Insert or update `(key, value)`; marks the entry dirty. Evicts (and
    /// flushes) the oldest entry first if at capacity.
    fn put(&mut self, key: String, value: Vec<u8>) -> PyResult<()> {
        self.inner.put(key, value).map_err(wb_err)
    }

    /// Look up `key`; on a cache miss, reads through to the backing store
    /// (cached as clean on success).
    fn get(&mut self, key: &str) -> PyResult<Option<Vec<u8>>> {
        self.inner
            .get(&key.to_string())
            .map(|v| v.cloned())
            .map_err(wb_err)
    }

    /// Remove `key` from the cache and the backing store.
    fn delete(&mut self, key: &str) -> PyResult<bool> {
        self.inner.delete(&key.to_string()).map_err(wb_err)
    }

    /// Flush all dirty entries to the backing store; returns the count
    /// flushed.
    fn flush(&mut self) -> PyResult<usize> {
        self.inner.flush().map_err(wb_err)
    }

    /// Flush only when the dirty-entry count reaches `threshold`.
    fn flush_if_needed(&mut self, threshold: usize) -> PyResult<usize> {
        self.inner.flush_if_needed(threshold).map_err(wb_err)
    }

    /// Flush only entries whose dirty age exceeds `max_age_ms`.
    fn flush_older_than(&mut self, max_age_ms: u64) -> PyResult<usize> {
        self.inner
            .flush_older_than(std::time::Duration::from_millis(max_age_ms))
            .map_err(wb_err)
    }

    fn dirty_count(&self) -> usize {
        self.inner.dirty_count()
    }

    fn dirty_keys(&self) -> Vec<String> {
        self.inner.dirty_keys()
    }

    fn is_dirty(&self, key: &str) -> bool {
        self.inner.is_dirty(&key.to_string())
    }

    fn contains(&self, key: &str) -> bool {
        self.inner.contains(&key.to_string())
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Mark `key` clean without writing to the backing store (e.g. when the
    /// caller knows the store is already current). Returns `True` if it was
    /// dirty.
    fn mark_clean(&mut self, key: &str) -> bool {
        self.inner.mark_clean(&key.to_string())
    }

    fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    fn stats(&self) -> PyWriteBehindStats {
        self.inner.stats().into()
    }

    /// Every `(key, value)` pair actually persisted to the backing store so
    /// far (i.e. what a real backing store would durably hold).
    fn store_snapshot(&self) -> Vec<(String, Vec<u8>)> {
        self.inner
            .store()
            .data
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "WriteBehindCache(len={}, dirty_count={}, capacity={})",
            self.inner.len(),
            self.inner.dirty_count(),
            self.inner.capacity()
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyMediaContentType>()?;
    m.add_class::<PyScoringWeights>()?;
    m.add_class::<PyCacheEntry>()?;
    m.add_class::<PyContentAwareCache>()?;
    m.add_class::<PyWriteBehindStats>()?;
    m.add_class::<PyWriteBehindCache>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_content_type_priority_and_ttl() {
        let manifest = PyMediaContentType::manifest();
        assert_eq!(manifest.priority(), 10);
        assert_eq!(manifest.ttl_ms(), 30_000);
        assert_eq!(manifest.kind(), "Manifest");

        let video = PyMediaContentType::video_segment(5_000_000, "av1".to_string());
        assert_eq!(video.priority(), 7);
        assert_eq!(video.kind(), "VideoSegment");
    }

    #[test]
    fn content_aware_cache_insert_and_get() {
        let mut cache = PyContentAwareCache::new(16, None);
        cache.insert_media(
            "seg1".to_string(),
            vec![0u8; 1024],
            &PyMediaContentType::video_segment(2_000_000, "av1".to_string()),
        );
        let entry = cache.get("seg1").expect("entry should be present");
        assert_eq!(entry.size_bytes, 1024);
        assert_eq!(entry.access_count, 1);
    }

    #[test]
    fn content_aware_cache_max_bytes_eviction() {
        let mut cache = PyContentAwareCache::new(100, Some(500));
        for i in 0..5u32 {
            cache.insert_media(
                format!("seg_{i}"),
                vec![0u8; 100],
                &PyMediaContentType::audio_segment(128_000),
            );
        }
        assert!(cache.total_bytes() <= 500);
        cache.insert_media(
            "extra".to_string(),
            vec![0u8; 100],
            &PyMediaContentType::metadata(),
        );
        assert!(cache.total_bytes() <= 500);
    }

    #[test]
    fn scoring_weights_per_type_multiplier_roundtrip() {
        let mut weights = PyScoringWeights::new(1.0, 1.0, 1.0);
        let video = PyMediaContentType::video_segment(1_000_000, "vp9".to_string());
        weights.set_type_priority_multiplier(&video, 0.5);
        assert!((weights.priority_multiplier(&video) - 0.5).abs() < 1e-9);
        let manifest = PyMediaContentType::manifest();
        assert!((weights.priority_multiplier(&manifest) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn write_behind_cache_put_get_and_flush() {
        let mut cache = PyWriteBehindCache::new(10);
        cache
            .put("k1".to_string(), b"v1".to_vec())
            .expect("put should succeed");
        assert!(cache.is_dirty("k1"));
        assert_eq!(
            cache.get("k1").expect("get should succeed"),
            Some(b"v1".to_vec())
        );
        let flushed = cache.flush().expect("flush should succeed");
        assert_eq!(flushed, 1);
        assert!(!cache.is_dirty("k1"));
        let snap = cache.store_snapshot();
        assert_eq!(snap, vec![("k1".to_string(), b"v1".to_vec())]);
    }

    #[test]
    fn write_behind_cache_eviction_flushes_dirty() {
        let mut cache = PyWriteBehindCache::new(2);
        cache.put("a".to_string(), b"1".to_vec()).expect("put");
        cache.put("b".to_string(), b"2".to_vec()).expect("put");
        cache.put("c".to_string(), b"3".to_vec()).expect("put"); // evicts "a"
        let snap: HashMap<String, Vec<u8>> = cache.store_snapshot().into_iter().collect();
        assert_eq!(snap.get("a"), Some(&b"1".to_vec()));
    }

    #[test]
    fn write_behind_cache_stats_and_mark_clean() {
        let mut cache = PyWriteBehindCache::new(10);
        cache.put("x".to_string(), b"val".to_vec()).expect("put");
        assert!(cache.mark_clean("x"));
        assert!(!cache.is_dirty("x"));
        assert_eq!(cache.dirty_count(), 0);
        // mark_clean must not have written to the backing store.
        assert!(cache.store_snapshot().is_empty());
        let stats = cache.stats();
        assert_eq!(stats.entry_count, 1);
    }
}

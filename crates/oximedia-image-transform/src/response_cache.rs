// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Content-addressable response cache for transformed images.
//!
//! [`ResponseCache`] stores fully-encoded image bytes keyed by a `u64`
//! content-address derived from the source identifier and the serialised
//! transform parameters.  The cache uses a simple FIFO eviction policy:
//! the oldest-inserted entry is dropped when capacity is exceeded.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::response_cache::ResponseCache;
//!
//! let mut cache = ResponseCache::new(256);
//! assert!(cache.is_empty());
//!
//! let key = ResponseCache::key("uploads/photo.jpg", b"width=800,format=webp");
//! cache.insert(key, vec![0u8; 1024], 800, 450);
//!
//! assert_eq!(cache.len(), 1);
//! let entry = cache.get(key).expect("entry must be present");
//! assert_eq!(entry.width, 800);
//! assert_eq!(entry.height, 450);
//! assert_eq!(entry.data.len(), 1024);
//! ```

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

/// A single cached transform result.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Encoded image bytes.
    pub data: Vec<u8>,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Number of cache hits since this entry was inserted.
    pub hit_count: u64,
}

// ---------------------------------------------------------------------------
// ResponseCache
// ---------------------------------------------------------------------------

/// Content-addressable LRU-like cache for transformed image responses.
///
/// Entries are keyed by a `u64` derived from the source identifier and the
/// serialised transform parameters (see [`ResponseCache::key`]).  When the
/// cache is full the oldest-inserted entry (FIFO order) is evicted to make
/// room for new entries.
///
/// The cache is intentionally **not** thread-safe (`!Sync`).  Wrap it in a
/// `Mutex` or `RwLock` when sharing across threads.
#[derive(Debug)]
pub struct ResponseCache {
    map: HashMap<u64, CacheEntry>,
    /// Insertion order — front = oldest.
    order: VecDeque<u64>,
    /// Maximum number of entries before eviction occurs.
    capacity: usize,
}

impl ResponseCache {
    /// Create a new empty cache with the given `capacity`.
    ///
    /// `capacity` must be at least 1.  If 0 is supplied it is silently
    /// clamped to 1.
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity.min(4096)),
            order: VecDeque::with_capacity(capacity.min(4096)),
            capacity: capacity.max(1),
        }
    }

    /// Compute a content-address key for `(source_id, param_bytes)`.
    ///
    /// The key is deterministic but not cryptographically secure.  It is
    /// suitable for in-process caching only.
    ///
    /// ```
    /// use oximedia_image_transform::response_cache::ResponseCache;
    ///
    /// let k1 = ResponseCache::key("photo.jpg", b"width=800");
    /// let k2 = ResponseCache::key("photo.jpg", b"width=800");
    /// assert_eq!(k1, k2);
    ///
    /// let k3 = ResponseCache::key("photo.jpg", b"width=400");
    /// assert_ne!(k1, k3);
    /// ```
    pub fn key(source_id: &str, param_bytes: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        source_id.hash(&mut hasher);
        param_bytes.hash(&mut hasher);
        hasher.finish()
    }

    /// Look up an entry by key, incrementing its [`hit_count`](CacheEntry::hit_count).
    ///
    /// Returns `None` if the key is not present.
    pub fn get(&mut self, key: u64) -> Option<&CacheEntry> {
        if let Some(entry) = self.map.get_mut(&key) {
            entry.hit_count += 1;
            // Re-borrow as shared reference.
            self.map.get(&key)
        } else {
            None
        }
    }

    /// Insert or replace an entry.
    ///
    /// If the cache is at capacity, the oldest-inserted entry is evicted.
    /// Replacing an existing key does not count against capacity.
    pub fn insert(&mut self, key: u64, data: Vec<u8>, width: u32, height: u32) {
        if self.map.contains_key(&key) {
            // Replace in-place — no capacity change needed.
            if let Some(entry) = self.map.get_mut(&key) {
                entry.data = data;
                entry.width = width;
                entry.height = height;
                entry.hit_count = 0;
            }
            return;
        }

        // Evict oldest when at capacity.
        while self.map.len() >= self.capacity {
            if let Some(evict_key) = self.order.pop_front() {
                self.map.remove(&evict_key);
            } else {
                break;
            }
        }

        self.map.insert(
            key,
            CacheEntry {
                data,
                width,
                height,
                hit_count: 0,
            },
        );
        self.order.push_back(key);
    }

    /// Remove an entry by key.  Returns `true` if the key was present.
    pub fn remove(&mut self, key: u64) -> bool {
        if self.map.remove(&key).is_some() {
            self.order.retain(|&k| k != key);
            true
        } else {
            false
        }
    }

    /// Returns the number of entries currently in the cache.
    #[inline]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns `true` if the cache contains no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// Returns the configured capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns `true` if an entry with the given key is present.
    #[inline]
    pub fn contains_key(&self, key: u64) -> bool {
        self.map.contains_key(&key)
    }

    /// Total bytes stored across all cached entries.
    pub fn total_bytes(&self) -> usize {
        self.map.values().map(|e| e.data.len()).sum()
    }

    /// Total hit count across all entries.
    pub fn total_hits(&self) -> u64 {
        self.map.values().map(|e| e.hit_count).sum()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(data_len: usize) -> Vec<u8> {
        vec![0u8; data_len]
    }

    #[test]
    fn test_new_cache_is_empty() {
        let cache = ResponseCache::new(32);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_insert_and_get() {
        let mut cache = ResponseCache::new(32);
        let key = ResponseCache::key("photo.jpg", b"width=800");
        cache.insert(key, make_entry(1024), 800, 450);

        let entry = cache.get(key).expect("entry should be present");
        assert_eq!(entry.width, 800);
        assert_eq!(entry.height, 450);
        assert_eq!(entry.data.len(), 1024);
    }

    #[test]
    fn test_hit_count_increments() {
        let mut cache = ResponseCache::new(32);
        let key = ResponseCache::key("img.png", b"q=85");
        cache.insert(key, make_entry(512), 100, 100);

        cache.get(key);
        cache.get(key);
        let entry = cache.get(key).expect("must be present");
        assert_eq!(entry.hit_count, 3);
    }

    #[test]
    fn test_miss_returns_none() {
        let mut cache = ResponseCache::new(32);
        assert!(cache.get(0xdeadbeef_u64).is_none());
    }

    #[test]
    fn test_capacity_eviction_fifo() {
        let mut cache = ResponseCache::new(3);
        let k1 = ResponseCache::key("a.jpg", b"w=100");
        let k2 = ResponseCache::key("b.jpg", b"w=100");
        let k3 = ResponseCache::key("c.jpg", b"w=100");
        let k4 = ResponseCache::key("d.jpg", b"w=100");

        cache.insert(k1, make_entry(10), 100, 100);
        cache.insert(k2, make_entry(10), 100, 100);
        cache.insert(k3, make_entry(10), 100, 100);
        assert_eq!(cache.len(), 3);

        // Inserting k4 should evict k1 (oldest).
        cache.insert(k4, make_entry(10), 100, 100);
        assert_eq!(cache.len(), 3);
        assert!(!cache.contains_key(k1), "k1 should have been evicted");
        assert!(cache.contains_key(k4), "k4 should be present");
    }

    #[test]
    fn test_replace_existing_no_eviction() {
        let mut cache = ResponseCache::new(2);
        let k1 = ResponseCache::key("a.jpg", b"w=100");
        let k2 = ResponseCache::key("b.jpg", b"w=100");

        cache.insert(k1, make_entry(10), 100, 100);
        cache.insert(k2, make_entry(10), 100, 100);
        assert_eq!(cache.len(), 2);

        // Replace k1 — should not evict k2.
        cache.insert(k1, make_entry(20), 200, 200);
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_key(k2), "k2 must survive replacement of k1");
        let entry = cache.get(k1).expect("k1 must still be present");
        assert_eq!(entry.data.len(), 20);
        assert_eq!(entry.width, 200);
    }

    #[test]
    fn test_remove() {
        let mut cache = ResponseCache::new(32);
        let key = ResponseCache::key("x.webp", b"format=webp");
        cache.insert(key, make_entry(64), 64, 64);
        assert!(cache.remove(key));
        assert!(cache.is_empty());
        assert!(!cache.remove(key), "double-remove must return false");
    }

    #[test]
    fn test_clear() {
        let mut cache = ResponseCache::new(32);
        for i in 0..10u64 {
            cache.insert(i, make_entry(16), 16, 16);
        }
        assert_eq!(cache.len(), 10);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_total_bytes() {
        let mut cache = ResponseCache::new(32);
        cache.insert(1, make_entry(100), 10, 10);
        cache.insert(2, make_entry(200), 20, 20);
        assert_eq!(cache.total_bytes(), 300);
    }

    #[test]
    fn test_total_hits() {
        let mut cache = ResponseCache::new(32);
        let k = ResponseCache::key("img.jpg", b"");
        cache.insert(k, make_entry(10), 10, 10);
        cache.get(k);
        cache.get(k);
        assert_eq!(cache.total_hits(), 2);
    }

    #[test]
    fn test_key_deterministic() {
        let k1 = ResponseCache::key("photo.jpg", b"width=800,format=webp");
        let k2 = ResponseCache::key("photo.jpg", b"width=800,format=webp");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_key_differs_by_source() {
        let k1 = ResponseCache::key("a.jpg", b"width=800");
        let k2 = ResponseCache::key("b.jpg", b"width=800");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_key_differs_by_params() {
        let k1 = ResponseCache::key("img.jpg", b"width=800");
        let k2 = ResponseCache::key("img.jpg", b"width=400");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_capacity_getter() {
        let cache = ResponseCache::new(64);
        assert_eq!(cache.capacity(), 64);
    }

    #[test]
    fn test_zero_capacity_clamped_to_one() {
        let mut cache = ResponseCache::new(0);
        assert_eq!(cache.capacity(), 1);
        // Should hold exactly one entry.
        let k1 = ResponseCache::key("a.jpg", b"");
        let k2 = ResponseCache::key("b.jpg", b"");
        cache.insert(k1, make_entry(4), 1, 1);
        cache.insert(k2, make_entry(4), 1, 1);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key(k2));
    }
}

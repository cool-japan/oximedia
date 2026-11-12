// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Origin image fetcher with in-memory response caching.
//!
//! Provides [`OriginFetcher`] which:
//! - Validates source URLs against an allowlist to prevent SSRF.
//! - Fetches image bytes from HTTP/HTTPS origins (or a pluggable
//!   [`FetchBackend`] for testing).
//! - Caches responses in memory with configurable per-entry TTL and maximum
//!   cache capacity.  Cache eviction uses a simple LRU-style pass: when the
//!   cache is full, the oldest entry (earliest `fetched_at`) is evicted.
//!
//! # Example (with the no-network stub)
//!
//! ```
//! use oximedia_image_transform::origin_fetch::{
//!     CacheConfig, FetchBackend, OriginFetcher, FetchResponse,
//! };
//!
//! // Build a stub backend that returns a fixed 4-byte PNG header.
//! struct Stub;
//! impl FetchBackend for Stub {
//!     fn fetch(&self, url: &str) -> Result<FetchResponse, String> {
//!         if url.starts_with("https://") {
//!             Ok(FetchResponse {
//!                 data: vec![0x89, 0x50, 0x4E, 0x47],
//!                 content_type: Some("image/png".to_string()),
//!                 content_length: Some(4),
//!             })
//!         } else {
//!             Err(format!("unsupported scheme: {url}"))
//!         }
//!     }
//! }
//!
//! let fetcher = OriginFetcher::new(
//!     CacheConfig { max_entries: 8, ttl_seconds: 60 },
//!     Box::new(Stub),
//! );
//!
//! let bytes = fetcher.fetch("https://example.com/photo.jpg").expect("fetch ok");
//! assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
//! // Second call is served from cache.
//! let cached = fetcher.fetch("https://example.com/photo.jpg").expect("cache hit");
//! assert_eq!(bytes, cached);
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// FetchResponse
// ---------------------------------------------------------------------------

/// Raw response from an origin server.
#[derive(Debug, Clone)]
pub struct FetchResponse {
    /// Image bytes.
    pub data: Vec<u8>,
    /// MIME type reported by the origin (e.g. `"image/jpeg"`), if known.
    pub content_type: Option<String>,
    /// `Content-Length` reported by the origin, if known.
    pub content_length: Option<usize>,
}

// ---------------------------------------------------------------------------
// FetchBackend trait
// ---------------------------------------------------------------------------

/// Pluggable HTTP backend for [`OriginFetcher`].
///
/// Implement this trait to provide a custom fetch implementation (e.g. for
/// testing with stub data, or to add authentication headers).
pub trait FetchBackend: Send + Sync {
    /// Fetch the given URL and return its byte payload.
    ///
    /// Returns `Err(message)` on any failure (network, HTTP 4xx/5xx, etc.).
    fn fetch(&self, url: &str) -> Result<FetchResponse, String>;
}

// ---------------------------------------------------------------------------
// CacheConfig
// ---------------------------------------------------------------------------

/// Configuration for the in-memory response cache.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum number of cached responses.  When the limit is reached, the
    /// oldest entry (by fetch time) is evicted before inserting a new one.
    pub max_entries: usize,
    /// Time-to-live for each cached response, in seconds.  Expired entries are
    /// lazily evicted on the next lookup for that URL.
    pub ttl_seconds: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 256,
            ttl_seconds: 300,
        }
    }
}

// ---------------------------------------------------------------------------
// CacheEntry (private)
// ---------------------------------------------------------------------------

struct CacheEntry {
    response: FetchResponse,
    fetched_at: Instant,
}

// ---------------------------------------------------------------------------
// OriginFetcher
// ---------------------------------------------------------------------------

/// Fetches images from remote origins and caches them in memory.
///
/// See the [module documentation](self) for a usage example.
pub struct OriginFetcher {
    config: CacheConfig,
    backend: Box<dyn FetchBackend>,
    cache: Mutex<HashMap<String, CacheEntry>>,
    /// Allowlisted hostname prefixes.  If non-empty, only URLs whose host
    /// starts with one of these prefixes are permitted.
    allowed_hosts: Vec<String>,
}

impl std::fmt::Debug for OriginFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginFetcher")
            .field("config", &self.config)
            .field("allowed_hosts", &self.allowed_hosts)
            .finish()
    }
}

impl OriginFetcher {
    /// Create a new fetcher with the given cache config and fetch backend.
    ///
    /// The allowlist is initially empty (all HTTPS URLs are permitted).
    pub fn new(config: CacheConfig, backend: Box<dyn FetchBackend>) -> Self {
        Self {
            config,
            backend,
            cache: Mutex::new(HashMap::new()),
            allowed_hosts: Vec::new(),
        }
    }

    /// Restrict origin fetching to URLs whose host matches one of the given
    /// prefixes (e.g. `"images.example.com"` or `.example.com`).
    ///
    /// Calling this replaces any previously set allowlist.
    pub fn with_allowed_hosts(mut self, hosts: impl IntoIterator<Item = String>) -> Self {
        self.allowed_hosts = hosts.into_iter().collect();
        self
    }

    /// Fetch an image from `url`, returning the raw bytes.
    ///
    /// The response is served from the in-memory cache when available and not
    /// expired.  On a cache miss, the configured backend is called and the
    /// result is stored.
    ///
    /// # Errors
    ///
    /// Returns an error string if:
    /// - The URL uses a non-HTTPS scheme.
    /// - The host is not on the allowlist (when one is configured).
    /// - The backend returns an error.
    /// - Internal cache lock is poisoned.
    pub fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        // Scheme check — only https:// is allowed.
        if !url.starts_with("https://") {
            return Err(format!(
                "origin_fetch: only https:// URLs are permitted, got: {url}"
            ));
        }

        // Host allowlist check.
        if !self.allowed_hosts.is_empty() {
            let host = extract_host(url);
            let allowed = self
                .allowed_hosts
                .iter()
                .any(|h| host == h.as_str() || host.ends_with(h.as_str()));
            if !allowed {
                return Err(format!("origin_fetch: host '{host}' is not allowlisted"));
            }
        }

        let ttl = Duration::from_secs(self.config.ttl_seconds);
        let now = Instant::now();

        // Check cache first.
        {
            let cache = self
                .cache
                .lock()
                .map_err(|e| format!("origin_fetch: cache lock poisoned: {e}"))?;
            if let Some(entry) = cache.get(url) {
                if now.duration_since(entry.fetched_at) < ttl {
                    return Ok(entry.response.data.clone());
                }
                // Entry exists but is expired — fall through to re-fetch.
            }
        }

        // Cache miss or expired — fetch from origin.
        let response = self.backend.fetch(url)?;
        let data = response.data.clone();

        // Insert into cache, evicting oldest entry if necessary.
        {
            let mut cache = self
                .cache
                .lock()
                .map_err(|e| format!("origin_fetch: cache lock poisoned: {e}"))?;

            // Remove expired entry if present (URL collision).
            cache.remove(url);

            // Evict oldest entry if at capacity.
            if cache.len() >= self.config.max_entries {
                if let Some(oldest_key) = cache
                    .iter()
                    .min_by_key(|(_, v)| v.fetched_at)
                    .map(|(k, _)| k.clone())
                {
                    cache.remove(&oldest_key);
                }
            }

            cache.insert(
                url.to_string(),
                CacheEntry {
                    response,
                    fetched_at: now,
                },
            );
        }

        Ok(data)
    }

    /// Invalidate a specific URL in the cache (e.g. after origin content changes).
    ///
    /// Returns `true` if the entry was present and removed, `false` otherwise.
    pub fn invalidate(&self, url: &str) -> bool {
        self.cache
            .lock()
            .map(|mut c| c.remove(url).is_some())
            .unwrap_or(false)
    }

    /// Flush all cached responses.
    pub fn flush(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Returns the number of entries currently in the cache.
    pub fn cache_size(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// Returns `true` if the given URL is currently in the cache and not expired.
    pub fn is_cached(&self, url: &str) -> bool {
        let ttl = Duration::from_secs(self.config.ttl_seconds);
        let now = Instant::now();
        self.cache
            .lock()
            .map(|c| {
                c.get(url)
                    .map(|e| now.duration_since(e.fetched_at) < ttl)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the hostname from an `https://host/path` URL.
///
/// Returns the portion between `://` and the next `/` (or end of string).
fn extract_host(url: &str) -> &str {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .split_once('/')
        .map(|(host, _)| host)
        .unwrap_or(after_scheme)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Stub backend ──

    struct StubBackend {
        payload: Vec<u8>,
        fail: bool,
    }

    impl StubBackend {
        fn ok(payload: Vec<u8>) -> Self {
            Self {
                payload,
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                payload: Vec::new(),
                fail: true,
            }
        }
    }

    impl FetchBackend for StubBackend {
        fn fetch(&self, _url: &str) -> Result<FetchResponse, String> {
            if self.fail {
                Err("stub: simulated network failure".to_string())
            } else {
                Ok(FetchResponse {
                    data: self.payload.clone(),
                    content_type: Some("image/jpeg".to_string()),
                    content_length: Some(self.payload.len()),
                })
            }
        }
    }

    fn make_fetcher(payload: Vec<u8>) -> OriginFetcher {
        OriginFetcher::new(
            CacheConfig {
                max_entries: 4,
                ttl_seconds: 60,
            },
            Box::new(StubBackend::ok(payload)),
        )
    }

    // ── Basic fetch ──

    #[test]
    fn test_fetch_returns_payload() {
        let fetcher = make_fetcher(vec![1, 2, 3, 4]);
        let data = fetcher.fetch("https://example.com/a.jpg").expect("ok");
        assert_eq!(data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_second_fetch_served_from_cache() {
        let fetcher = make_fetcher(vec![9, 8, 7]);
        let a = fetcher.fetch("https://example.com/b.jpg").expect("ok");
        let b = fetcher.fetch("https://example.com/b.jpg").expect("ok");
        assert_eq!(a, b);
        assert_eq!(fetcher.cache_size(), 1);
    }

    #[test]
    fn test_different_urls_cached_separately() {
        let fetcher = make_fetcher(vec![0]);
        fetcher.fetch("https://example.com/c1.jpg").expect("ok");
        fetcher.fetch("https://example.com/c2.jpg").expect("ok");
        assert_eq!(fetcher.cache_size(), 2);
    }

    // ── Scheme validation ──

    #[test]
    fn test_reject_http_scheme() {
        let fetcher = make_fetcher(vec![]);
        let err = fetcher
            .fetch("http://example.com/x.jpg")
            .expect_err("should fail");
        assert!(err.contains("https"));
    }

    #[test]
    fn test_reject_file_scheme() {
        let fetcher = make_fetcher(vec![]);
        let err = fetcher
            .fetch("file:///etc/passwd")
            .expect_err("should fail");
        assert!(err.contains("https"));
    }

    // ── Host allowlist ──

    #[test]
    fn test_allowlist_permits_matching_host() {
        let fetcher = make_fetcher(vec![42]).with_allowed_hosts(["example.com".to_string()]);
        let data = fetcher.fetch("https://example.com/img.jpg").expect("ok");
        assert_eq!(data, vec![42]);
    }

    #[test]
    fn test_allowlist_rejects_unknown_host() {
        let fetcher = make_fetcher(vec![]).with_allowed_hosts(["trusted.com".to_string()]);
        let err = fetcher
            .fetch("https://malicious.example.com/x.jpg")
            .expect_err("should be blocked");
        assert!(err.contains("not allowlisted"));
    }

    // ── Backend error ──

    #[test]
    fn test_backend_error_propagates() {
        let fetcher = OriginFetcher::new(CacheConfig::default(), Box::new(StubBackend::failing()));
        let err = fetcher
            .fetch("https://example.com/fail.jpg")
            .expect_err("should fail");
        assert!(err.contains("simulated"));
    }

    // ── Cache management ──

    #[test]
    fn test_invalidate_removes_entry() {
        let fetcher = make_fetcher(vec![1]);
        fetcher.fetch("https://example.com/e.jpg").expect("ok");
        assert_eq!(fetcher.cache_size(), 1);
        let removed = fetcher.invalidate("https://example.com/e.jpg");
        assert!(removed);
        assert_eq!(fetcher.cache_size(), 0);
    }

    #[test]
    fn test_invalidate_nonexistent_returns_false() {
        let fetcher = make_fetcher(vec![]);
        assert!(!fetcher.invalidate("https://example.com/nonexistent.jpg"));
    }

    #[test]
    fn test_flush_clears_all_entries() {
        let fetcher = make_fetcher(vec![5]);
        fetcher.fetch("https://example.com/f1.jpg").expect("ok");
        fetcher.fetch("https://example.com/f2.jpg").expect("ok");
        assert_eq!(fetcher.cache_size(), 2);
        fetcher.flush();
        assert_eq!(fetcher.cache_size(), 0);
    }

    #[test]
    fn test_is_cached_after_fetch() {
        let fetcher = make_fetcher(vec![0]);
        assert!(!fetcher.is_cached("https://example.com/g.jpg"));
        fetcher.fetch("https://example.com/g.jpg").expect("ok");
        assert!(fetcher.is_cached("https://example.com/g.jpg"));
    }

    #[test]
    fn test_evicts_oldest_when_full() {
        let fetcher = make_fetcher(vec![0]);
        // max_entries = 4
        for i in 0..5 {
            fetcher
                .fetch(&format!("https://example.com/h{i}.jpg"))
                .expect("ok");
        }
        // Should have evicted one entry to stay at max_entries.
        assert_eq!(fetcher.cache_size(), 4);
    }

    // ── extract_host helper ──

    #[test]
    fn test_extract_host_basic() {
        assert_eq!(extract_host("https://example.com/path"), "example.com");
    }

    #[test]
    fn test_extract_host_no_path() {
        assert_eq!(extract_host("https://example.com"), "example.com");
    }

    #[test]
    fn test_extract_host_with_port() {
        assert_eq!(
            extract_host("https://example.com:443/img"),
            "example.com:443"
        );
    }

    // ── CacheConfig default ──

    #[test]
    fn test_cache_config_default() {
        let c = CacheConfig::default();
        assert_eq!(c.max_entries, 256);
        assert_eq!(c.ttl_seconds, 300);
    }
}

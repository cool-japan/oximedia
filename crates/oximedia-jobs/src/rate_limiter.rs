#![allow(dead_code)]
//! Rate limiting — token bucket and fixed-window algorithms for controlling job throughput.

use std::time::{Duration, Instant};

/// Algorithm used for rate limiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitAlgorithm {
    /// Token bucket: tokens accumulate up to a capacity and are consumed per request.
    TokenBucket,
    /// Fixed window: allows N requests per window period, resets at each boundary.
    FixedWindow,
    /// Sliding window: smooth rate limiting based on a rolling time window.
    SlidingWindow,
    /// Leaky bucket: requests drain at a fixed rate regardless of burst.
    LeakyBucket,
}

impl RateLimitAlgorithm {
    /// Display name.
    pub fn name(&self) -> &'static str {
        match self {
            RateLimitAlgorithm::TokenBucket => "token_bucket",
            RateLimitAlgorithm::FixedWindow => "fixed_window",
            RateLimitAlgorithm::SlidingWindow => "sliding_window",
            RateLimitAlgorithm::LeakyBucket => "leaky_bucket",
        }
    }

    /// Whether this algorithm allows short bursts above the sustained rate.
    pub fn supports_bursting(&self) -> bool {
        matches!(
            self,
            RateLimitAlgorithm::TokenBucket | RateLimitAlgorithm::LeakyBucket
        )
    }
}

/// Configuration for a rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum requests (or tokens) per window.
    pub limit: u64,
    /// The window duration over which `limit` requests are allowed.
    pub window: Duration,
    /// The algorithm to use.
    pub algorithm: RateLimitAlgorithm,
    /// Whether to strictly enforce the limit (deny vs queue excess).
    pub strict: bool,
    /// Optional name/label for this limiter.
    pub name: String,
}

impl RateLimitConfig {
    /// Create a new config.
    pub fn new(
        name: impl Into<String>,
        limit: u64,
        window: Duration,
        algorithm: RateLimitAlgorithm,
    ) -> Self {
        Self {
            limit,
            window,
            algorithm,
            strict: true,
            name: name.into(),
        }
    }

    /// Set strict mode (deny excess requests rather than queuing them).
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Returns true if this config uses strict enforcement.
    pub fn is_strict(&self) -> bool {
        self.strict
    }

    /// Sustained rate in requests per second.
    #[allow(clippy::cast_precision_loss)]
    pub fn rate_per_second(&self) -> f64 {
        let secs = self.window.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.limit as f64 / secs
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self::new(
            "default",
            100,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        )
    }
}

/// A token-bucket rate limiter.
///
/// Tokens refill continuously at `limit / window` per second up to `limit` capacity.
#[derive(Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    /// Current number of available tokens (fractional for precision).
    tokens: f64,
    /// When the token count was last updated.
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a rate limiter from the given config.
    pub fn new(config: RateLimitConfig) -> Self {
        let tokens = config.limit as f64;
        Self {
            config,
            tokens,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let rate = self.config.rate_per_second();
        self.tokens = (self.tokens + elapsed * rate).min(self.config.limit as f64);
        self.last_refill = now;
    }

    /// Attempt to acquire `count` tokens. Returns true if acquired, false if denied.
    pub fn try_acquire(&mut self, count: u64) -> bool {
        self.refill();
        let needed = count as f64;
        if self.tokens >= needed {
            self.tokens -= needed;
            true
        } else {
            false
        }
    }

    /// Attempt to acquire one token.
    pub fn try_acquire_one(&mut self) -> bool {
        self.try_acquire(1)
    }

    /// Available tokens (floored to nearest integer for display).
    pub fn available_tokens(&self) -> u64 {
        self.tokens.floor() as u64
    }

    /// The current fractional token level (for tests/diagnostics).
    pub fn tokens_f64(&self) -> f64 {
        self.tokens
    }

    /// The configured limit.
    pub fn limit(&self) -> u64 {
        self.config.limit
    }

    /// Returns a reference to the config.
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }
}

/// A record of a single rate-limited request.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// Timestamp of the request.
    pub at: Instant,
    /// Whether the request was allowed.
    pub allowed: bool,
    /// Number of tokens requested.
    pub tokens_requested: u64,
}

/// Tracks rate-limit decisions over time for analysis.
#[derive(Debug, Default)]
pub struct RateLimitTracker {
    records: Vec<RequestRecord>,
}

impl RateLimitTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a rate-limit decision.
    pub fn record_request(&mut self, allowed: bool, tokens_requested: u64) {
        self.records.push(RequestRecord {
            at: Instant::now(),
            allowed,
            tokens_requested,
        });
    }

    /// Total requests recorded.
    pub fn total(&self) -> usize {
        self.records.len()
    }

    /// Number of allowed requests.
    pub fn allowed_count(&self) -> usize {
        self.records.iter().filter(|r| r.allowed).count()
    }

    /// Number of denied requests.
    pub fn denied_count(&self) -> usize {
        self.records.iter().filter(|r| !r.allowed).count()
    }

    /// Allow rate [0.0, 1.0]. Returns 0.0 if no records.
    #[allow(clippy::cast_precision_loss)]
    pub fn allow_rate(&self) -> f64 {
        if self.records.is_empty() {
            return 0.0;
        }
        self.allowed_count() as f64 / self.records.len() as f64
    }

    /// Total tokens that were successfully acquired.
    pub fn total_tokens_acquired(&self) -> u64 {
        self.records
            .iter()
            .filter(|r| r.allowed)
            .map(|r| r.tokens_requested)
            .sum()
    }

    /// Clear all records.
    pub fn clear(&mut self) {
        self.records.clear();
    }
}

// ===========================================================================
// Keyed (per-user / per-tag / global) rate limiter
// ===========================================================================

/// Discriminator for a rate-limit bucket.
///
/// A request may carry multiple keys (e.g. a global key plus a per-user key
/// plus one or more per-tag keys).  [`KeyedRateLimiter::check_all`] enforces
/// **all** matching limits — the most restrictive wins.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateKey {
    /// The global catch-all bucket.
    Global,
    /// A per-user bucket identified by an arbitrary string identifier.
    User(String),
    /// A per-tag bucket identified by a tag string.
    Tag(String),
}

/// A rate limiter that maintains independent token buckets for multiple
/// [`RateKey`]s.
///
/// # Map growth
///
/// One bucket is created per unique [`RateKey`] via [`add_limit`].  Callers
/// are responsible for removing obsolete keys with [`remove_limit`] when a
/// user or tag is no longer active.  The map is not automatically evicted to
/// avoid complexity; use a background sweep or TTL-aware wrapper when dealing
/// with large, churning key spaces.
///
/// [`add_limit`]: KeyedRateLimiter::add_limit
/// [`remove_limit`]: KeyedRateLimiter::remove_limit
#[derive(Debug)]
pub struct KeyedRateLimiter {
    buckets: std::collections::HashMap<RateKey, RateLimiter>,
}

impl KeyedRateLimiter {
    /// Create a new keyed limiter with a single `Global` bucket seeded from
    /// `global_config`.
    pub fn new(global_config: RateLimitConfig) -> Self {
        let mut buckets = std::collections::HashMap::new();
        buckets.insert(RateKey::Global, RateLimiter::new(global_config));
        Self { buckets }
    }

    /// Register an additional rate-limit bucket for the given key.
    ///
    /// If a bucket for `key` already exists it is **replaced**.
    pub fn add_limit(&mut self, key: RateKey, config: RateLimitConfig) {
        self.buckets.insert(key, RateLimiter::new(config));
    }

    /// Remove the bucket for `key`.  Returns `true` if a bucket was present.
    pub fn remove_limit(&mut self, key: &RateKey) -> bool {
        self.buckets.remove(key).is_some()
    }

    /// Check whether **all** supplied `keys` have tokens available, consuming
    /// one token from each bucket that has capacity.
    ///
    /// The check is done in two passes to keep the operation as atomic as the
    /// single-threaded token-bucket model allows:
    ///
    /// 1. **Probe pass** — refill all relevant buckets and check availability
    ///    without consuming.
    /// 2. **Consume pass** — consume one token from every bucket that had
    ///    capacity (only executed when *all* probes passed).
    ///
    /// Returns `false` (blocked) if *any* relevant bucket cannot satisfy the
    /// request.  Unknown keys (no registered bucket) are silently ignored —
    /// they do not block the request.
    pub fn check_all(&mut self, keys: &[RateKey]) -> bool {
        // Gather the subset of keys that have registered buckets.
        let relevant: Vec<&RateKey> = keys
            .iter()
            .filter(|k| self.buckets.contains_key(*k))
            .collect();

        // Probe: refill and check availability without consuming.  `relevant`
        // was pre-filtered to keys with a registered bucket; the `if let`
        // (rather than an expect) means a bucket that vanished between the
        // filter and this loop is silently skipped, matching this method's
        // documented "unknown keys are silently ignored" contract instead of
        // panicking.
        for key in &relevant {
            if let Some(limiter) = self.buckets.get_mut(*key) {
                // Trigger refill by peeking at tokens_f64 after calling refill via a
                // no-cost try_acquire(0) equivalent.  We use `tokens_f64()` after an
                // explicit refill call.
                limiter.refill_only();
                if limiter.tokens_f64() < 1.0 {
                    return false;
                }
            }
        }

        // Consume: all probes passed, burn one token from each bucket.
        for key in &relevant {
            if let Some(limiter) = self.buckets.get_mut(*key) {
                limiter.consume_one();
            }
        }
        true
    }

    /// Return the number of registered buckets (including `Global`).
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// Return the available tokens for `key`, or `None` if not registered.
    pub fn available_tokens(&self, key: &RateKey) -> Option<u64> {
        self.buckets.get(key).map(|l| l.available_tokens())
    }
}

// We need two additional methods on `RateLimiter` to enable the two-pass
// check.  They are intentionally not part of the public API of `RateLimiter`
// itself but are exposed here via an extension approach by adding methods
// to the struct (both are in the same module).
impl RateLimiter {
    /// Perform only the refill step without acquiring any tokens.
    pub(crate) fn refill_only(&mut self) {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let rate = self.config.rate_per_second();
        self.tokens = (self.tokens + elapsed * rate).min(self.config.limit as f64);
        self.last_refill = now;
    }

    /// Consume exactly one token.  Caller must ensure at least one token is
    /// available (i.e. call `refill_only` + check `tokens_f64() >= 1.0` first).
    pub(crate) fn consume_one(&mut self) {
        self.tokens = (self.tokens - 1.0).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_algorithm_name() {
        assert_eq!(RateLimitAlgorithm::TokenBucket.name(), "token_bucket");
        assert_eq!(RateLimitAlgorithm::FixedWindow.name(), "fixed_window");
        assert_eq!(RateLimitAlgorithm::SlidingWindow.name(), "sliding_window");
        assert_eq!(RateLimitAlgorithm::LeakyBucket.name(), "leaky_bucket");
    }

    #[test]
    fn test_algorithm_supports_bursting() {
        assert!(RateLimitAlgorithm::TokenBucket.supports_bursting());
        assert!(RateLimitAlgorithm::LeakyBucket.supports_bursting());
        assert!(!RateLimitAlgorithm::FixedWindow.supports_bursting());
        assert!(!RateLimitAlgorithm::SlidingWindow.supports_bursting());
    }

    #[test]
    fn test_config_is_strict() {
        let cfg = RateLimitConfig::new(
            "test",
            100,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        );
        assert!(cfg.is_strict()); // default strict
        let non_strict = cfg.with_strict(false);
        assert!(!non_strict.is_strict());
    }

    #[test]
    fn test_config_rate_per_second() {
        let cfg = RateLimitConfig::new(
            "rps",
            60,
            Duration::from_secs(60),
            RateLimitAlgorithm::TokenBucket,
        );
        assert!((cfg.rate_per_second() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_config_default() {
        let cfg = RateLimitConfig::default();
        assert_eq!(cfg.limit, 100);
        assert!(cfg.is_strict());
    }

    #[test]
    fn test_rate_limiter_initial_full() {
        let cfg = RateLimitConfig::new(
            "l1",
            10,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        );
        let limiter = RateLimiter::new(cfg);
        assert_eq!(limiter.available_tokens(), 10);
    }

    #[test]
    fn test_rate_limiter_try_acquire() {
        let cfg = RateLimitConfig::new(
            "l2",
            5,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        );
        let mut limiter = RateLimiter::new(cfg);
        assert!(limiter.try_acquire(3));
        assert!(limiter.try_acquire(2));
        // Tokens exhausted
        assert!(!limiter.try_acquire(1));
    }

    #[test]
    fn test_rate_limiter_try_acquire_one() {
        let cfg = RateLimitConfig::new(
            "l3",
            2,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        );
        let mut limiter = RateLimiter::new(cfg);
        assert!(limiter.try_acquire_one());
        assert!(limiter.try_acquire_one());
        assert!(!limiter.try_acquire_one());
    }

    #[test]
    fn test_rate_limiter_limit() {
        let cfg = RateLimitConfig::new(
            "l4",
            42,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        );
        let limiter = RateLimiter::new(cfg);
        assert_eq!(limiter.limit(), 42);
    }

    #[test]
    fn test_tracker_empty() {
        let t = RateLimitTracker::new();
        assert_eq!(t.total(), 0);
        assert_eq!(t.allow_rate(), 0.0);
        assert_eq!(t.total_tokens_acquired(), 0);
    }

    #[test]
    fn test_tracker_record_request() {
        let mut t = RateLimitTracker::new();
        t.record_request(true, 1);
        t.record_request(true, 2);
        t.record_request(false, 1);
        assert_eq!(t.total(), 3);
        assert_eq!(t.allowed_count(), 2);
        assert_eq!(t.denied_count(), 1);
    }

    #[test]
    fn test_tracker_allow_rate() {
        let mut t = RateLimitTracker::new();
        t.record_request(true, 1);
        t.record_request(false, 1);
        let rate = t.allow_rate();
        assert!((rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_tracker_total_tokens_acquired() {
        let mut t = RateLimitTracker::new();
        t.record_request(true, 5);
        t.record_request(true, 3);
        t.record_request(false, 10); // denied — not counted
        assert_eq!(t.total_tokens_acquired(), 8);
    }

    #[test]
    fn test_tracker_clear() {
        let mut t = RateLimitTracker::new();
        t.record_request(true, 1);
        t.clear();
        assert_eq!(t.total(), 0);
    }

    // -----------------------------------------------------------------------
    // KeyedRateLimiter tests
    // -----------------------------------------------------------------------

    fn make_cfg(name: &str, limit: u64) -> RateLimitConfig {
        RateLimitConfig::new(
            name,
            limit,
            Duration::from_secs(1),
            RateLimitAlgorithm::TokenBucket,
        )
    }

    /// Two user buckets with different capacities must be isolated — depleting
    /// user-A's bucket must not affect user-B.
    #[test]
    fn test_keyed_per_user_isolation() {
        let mut klr = KeyedRateLimiter::new(make_cfg("global", 1000));
        klr.add_limit(RateKey::User("alice".to_string()), make_cfg("alice", 2));
        klr.add_limit(RateKey::User("bob".to_string()), make_cfg("bob", 100));

        let alice_keys = vec![RateKey::Global, RateKey::User("alice".to_string())];
        let bob_keys = vec![RateKey::Global, RateKey::User("bob".to_string())];

        // Exhaust alice's 2 tokens.
        assert!(klr.check_all(&alice_keys), "alice request 1 should pass");
        assert!(klr.check_all(&alice_keys), "alice request 2 should pass");
        assert!(
            !klr.check_all(&alice_keys),
            "alice request 3 should be blocked"
        );

        // Bob is unaffected.
        assert!(
            klr.check_all(&bob_keys),
            "bob should still pass after alice is blocked"
        );
    }

    /// A per-tag bucket limits requests tagged with that tag while an
    /// unrelated tag flows freely.
    #[test]
    fn test_keyed_per_tag_independence() {
        let mut klr = KeyedRateLimiter::new(make_cfg("global", 1000));
        klr.add_limit(RateKey::Tag("gpu".to_string()), make_cfg("gpu", 1));
        klr.add_limit(RateKey::Tag("cpu".to_string()), make_cfg("cpu", 50));

        let gpu_keys = vec![RateKey::Tag("gpu".to_string())];
        let cpu_keys = vec![RateKey::Tag("cpu".to_string())];

        // Exhaust the gpu bucket.
        assert!(klr.check_all(&gpu_keys), "first gpu request should pass");
        assert!(
            !klr.check_all(&gpu_keys),
            "second gpu request should be blocked"
        );

        // CPU tag is independent.
        assert!(
            klr.check_all(&cpu_keys),
            "cpu request should pass regardless of gpu"
        );
    }

    /// When a request carries multiple keys the *most restrictive* bucket wins:
    /// if any one bucket is exhausted the whole request is denied.
    #[test]
    fn test_keyed_most_restrictive_wins() {
        let mut klr = KeyedRateLimiter::new(make_cfg("global", 1000));
        klr.add_limit(RateKey::User("dave".to_string()), make_cfg("dave", 5));
        klr.add_limit(RateKey::Tag("batch".to_string()), make_cfg("batch", 1));

        let all_keys = vec![
            RateKey::Global,
            RateKey::User("dave".to_string()),
            RateKey::Tag("batch".to_string()),
        ];

        // First request passes all three buckets.
        assert!(klr.check_all(&all_keys), "combined request 1 should pass");

        // The 'batch' bucket is now exhausted (limit=1).  Even though global
        // and user still have capacity, the combined request must be blocked.
        assert!(
            !klr.check_all(&all_keys),
            "combined request 2 should be blocked because 'batch' tag is exhausted"
        );

        // A request that does NOT carry the 'batch' tag still flows through.
        let user_only = vec![RateKey::Global, RateKey::User("dave".to_string())];
        assert!(
            klr.check_all(&user_only),
            "user-only request should pass because batch key is not checked"
        );
    }
}

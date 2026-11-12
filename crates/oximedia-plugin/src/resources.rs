// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Plugin resource tracking and limit enforcement.
//!
//! Provides [`ResourceUsage`] for measuring a plugin's resource consumption,
//! [`ResourceLimit`] for defining hard caps, and [`ResourceTracker`] for
//! monitoring multiple plugins and enforcing limits.
//!
//! # Example
//!
//! ```rust
//! use oximedia_plugin::resources::{ResourceLimit, ResourceTracker, ResourceUsage};
//!
//! let mut tracker = ResourceTracker::new();
//! let limit = ResourceLimit::new()
//!     .max_memory(1024 * 1024)  // 1 MB
//!     .max_cpu_ms(5000);        // 5 seconds
//!
//! tracker.register("my-plugin", limit);
//! tracker.record_allocation("my-plugin", 512).expect("within limits");
//! ```

use crate::error::{PluginError, PluginResult};
use std::collections::HashMap;

// ── ResourceUsage ────────────────────────────────────────────────────────────

/// Current resource consumption for a plugin.
#[derive(Debug, Clone, Default)]
pub struct ResourceUsage {
    /// Current memory usage in bytes.
    pub memory_bytes: u64,
    /// Accumulated CPU time in milliseconds.
    pub cpu_time_ms: u64,
    /// Total number of allocations made.
    pub allocations: u64,
    /// Total number of deallocations made.
    pub deallocations: u64,
    /// Peak memory usage observed in bytes.
    pub peak_memory_bytes: u64,
    /// Total bytes allocated (cumulative, not current).
    pub total_allocated_bytes: u64,
    /// Total bytes freed (cumulative).
    pub total_freed_bytes: u64,
}

impl ResourceUsage {
    /// Create a new zero-usage snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the net allocation count (allocations minus deallocations).
    #[must_use]
    pub fn net_allocations(&self) -> i64 {
        self.allocations as i64 - self.deallocations as i64
    }

    /// Reset all counters to zero.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl std::fmt::Display for ResourceUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "memory={}B (peak={}B), cpu={}ms, allocs={}, deallocs={}",
            self.memory_bytes,
            self.peak_memory_bytes,
            self.cpu_time_ms,
            self.allocations,
            self.deallocations
        )
    }
}

// ── ResourceLimit ────────────────────────────────────────────────────────────

/// Hard resource limits for a plugin.
///
/// Zero values mean "no limit" for that dimension.
#[derive(Debug, Clone)]
pub struct ResourceLimit {
    /// Maximum memory in bytes (0 = unlimited).
    pub max_memory_bytes: u64,
    /// Maximum CPU time in milliseconds (0 = unlimited).
    pub max_cpu_ms: u64,
    /// Maximum number of live allocations (0 = unlimited).
    pub max_allocations: u64,
}

impl ResourceLimit {
    /// Create a new limit with no restrictions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_memory_bytes: 0,
            max_cpu_ms: 0,
            max_allocations: 0,
        }
    }

    /// Set the maximum memory in bytes.
    #[must_use]
    pub fn max_memory(mut self, bytes: u64) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    /// Set the maximum CPU time in milliseconds.
    #[must_use]
    pub fn max_cpu_ms(mut self, ms: u64) -> Self {
        self.max_cpu_ms = ms;
        self
    }

    /// Set the maximum number of live allocations.
    #[must_use]
    pub fn max_allocations(mut self, count: u64) -> Self {
        self.max_allocations = count;
        self
    }

    /// Check whether all limits are zero (unrestricted).
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        self.max_memory_bytes == 0 && self.max_cpu_ms == 0 && self.max_allocations == 0
    }
}

impl Default for ResourceLimit {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ResourceLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mem = if self.max_memory_bytes == 0 {
            "unlimited".to_string()
        } else {
            format!("{}B", self.max_memory_bytes)
        };
        let cpu = if self.max_cpu_ms == 0 {
            "unlimited".to_string()
        } else {
            format!("{}ms", self.max_cpu_ms)
        };
        let alloc = if self.max_allocations == 0 {
            "unlimited".to_string()
        } else {
            format!("{}", self.max_allocations)
        };
        write!(f, "limits(memory={mem}, cpu={cpu}, allocs={alloc})")
    }
}

// ── LimitViolation ───────────────────────────────────────────────────────────

/// Describes which resource limit was exceeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitViolation {
    /// Memory limit exceeded.
    Memory {
        /// Current usage after the operation.
        current: u64,
        /// The configured limit.
        limit: u64,
    },
    /// CPU time limit exceeded.
    CpuTime {
        /// Current accumulated CPU time.
        current: u64,
        /// The configured limit.
        limit: u64,
    },
    /// Allocation count limit exceeded.
    Allocations {
        /// Current net allocation count.
        current: u64,
        /// The configured limit.
        limit: u64,
    },
}

impl std::fmt::Display for LimitViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory { current, limit } => {
                write!(f, "memory limit exceeded: {current}B > {limit}B")
            }
            Self::CpuTime { current, limit } => {
                write!(f, "CPU time limit exceeded: {current}ms > {limit}ms")
            }
            Self::Allocations { current, limit } => {
                write!(f, "allocation limit exceeded: {current} > {limit}")
            }
        }
    }
}

// ── ResourceEntry ────────────────────────────────────────────────────────────

/// Internal tracking state for a single plugin.
struct ResourceEntry {
    usage: ResourceUsage,
    limit: ResourceLimit,
    /// Whether the plugin has been suspended due to limit violations.
    suspended: bool,
    /// Number of limit violations recorded.
    violation_count: u64,
}

// ── ResourceTracker ──────────────────────────────────────────────────────────

/// Tracks resource consumption across multiple plugins and enforces limits.
///
/// Each plugin is associated with a [`ResourceLimit`]. Operations like
/// [`record_allocation`](Self::record_allocation) and
/// [`record_cpu_time`](Self::record_cpu_time) update the usage counters
/// and check against limits, returning an error if a limit is exceeded.
pub struct ResourceTracker {
    entries: HashMap<String, ResourceEntry>,
}

impl ResourceTracker {
    /// Create a new empty resource tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a plugin with the given resource limits.
    ///
    /// If the plugin is already registered, its limits and usage are replaced.
    pub fn register(&mut self, name: impl Into<String>, limit: ResourceLimit) {
        let entry = ResourceEntry {
            usage: ResourceUsage::new(),
            limit,
            suspended: false,
            violation_count: 0,
        };
        self.entries.insert(name.into(), entry);
    }

    /// Unregister a plugin, removing all tracked state.
    ///
    /// Returns `true` if the plugin was registered.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    /// Record a memory allocation for a plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    /// Returns [`PluginError::InitFailed`] with a description if the memory
    /// limit would be exceeded.
    pub fn record_allocation(&mut self, name: &str, bytes: u64) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;

        let new_memory = entry.usage.memory_bytes.saturating_add(bytes);

        if entry.limit.max_memory_bytes > 0 && new_memory > entry.limit.max_memory_bytes {
            entry.violation_count += 1;
            entry.suspended = true;
            let violation = LimitViolation::Memory {
                current: new_memory,
                limit: entry.limit.max_memory_bytes,
            };
            return Err(PluginError::InitFailed(format!(
                "Plugin '{}': {}",
                name, violation
            )));
        }

        entry.usage.memory_bytes = new_memory;
        entry.usage.allocations += 1;
        entry.usage.total_allocated_bytes = entry.usage.total_allocated_bytes.saturating_add(bytes);

        if entry.usage.memory_bytes > entry.usage.peak_memory_bytes {
            entry.usage.peak_memory_bytes = entry.usage.memory_bytes;
        }

        // Check allocation count limit.
        let net = entry.usage.net_allocations().max(0) as u64;
        if entry.limit.max_allocations > 0 && net > entry.limit.max_allocations {
            entry.violation_count += 1;
            entry.suspended = true;
            let violation = LimitViolation::Allocations {
                current: net,
                limit: entry.limit.max_allocations,
            };
            return Err(PluginError::InitFailed(format!(
                "Plugin '{}': {}",
                name, violation
            )));
        }

        Ok(())
    }

    /// Record a memory deallocation for a plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn record_deallocation(&mut self, name: &str, bytes: u64) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;

        entry.usage.memory_bytes = entry.usage.memory_bytes.saturating_sub(bytes);
        entry.usage.deallocations += 1;
        entry.usage.total_freed_bytes = entry.usage.total_freed_bytes.saturating_add(bytes);
        Ok(())
    }

    /// Record CPU time consumed by a plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    /// Returns [`PluginError::InitFailed`] if the CPU time limit is exceeded.
    pub fn record_cpu_time(&mut self, name: &str, ms: u64) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;

        let new_cpu = entry.usage.cpu_time_ms.saturating_add(ms);

        if entry.limit.max_cpu_ms > 0 && new_cpu > entry.limit.max_cpu_ms {
            entry.violation_count += 1;
            entry.suspended = true;
            let violation = LimitViolation::CpuTime {
                current: new_cpu,
                limit: entry.limit.max_cpu_ms,
            };
            return Err(PluginError::InitFailed(format!(
                "Plugin '{}': {}",
                name, violation
            )));
        }

        entry.usage.cpu_time_ms = new_cpu;
        Ok(())
    }

    /// Get the current resource usage for a plugin.
    #[must_use]
    pub fn usage(&self, name: &str) -> Option<&ResourceUsage> {
        self.entries.get(name).map(|e| &e.usage)
    }

    /// Get the resource limits for a plugin.
    #[must_use]
    pub fn limit(&self, name: &str) -> Option<&ResourceLimit> {
        self.entries.get(name).map(|e| &e.limit)
    }

    /// Check if a plugin is suspended due to limit violations.
    #[must_use]
    pub fn is_suspended(&self, name: &str) -> bool {
        self.entries.get(name).map(|e| e.suspended).unwrap_or(false)
    }

    /// Resume a suspended plugin, resetting its violation state.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn resume(&mut self, name: &str) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;
        entry.suspended = false;
        Ok(())
    }

    /// Reset a plugin's usage counters without changing its limits.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn reset_usage(&mut self, name: &str) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;
        entry.usage.reset();
        entry.suspended = false;
        entry.violation_count = 0;
        Ok(())
    }

    /// Update the resource limits for a plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn update_limit(&mut self, name: &str, limit: ResourceLimit) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;
        entry.limit = limit;
        Ok(())
    }

    /// Get the number of limit violations for a plugin.
    #[must_use]
    pub fn violation_count(&self, name: &str) -> Option<u64> {
        self.entries.get(name).map(|e| e.violation_count)
    }

    /// Get the number of tracked plugins.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the tracker has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// List all tracked plugin names.
    pub fn plugin_names(&self) -> Vec<&str> {
        self.entries.keys().map(|k| k.as_str()).collect()
    }

    /// List all suspended plugins.
    pub fn suspended_plugins(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, e)| e.suspended)
            .map(|(k, _)| k.as_str())
            .collect()
    }

    /// Get a summary of all plugins' resource usage.
    pub fn summary(&self) -> Vec<(String, ResourceUsage)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.clone(), entry.usage.clone()))
            .collect()
    }
}

impl Default for ResourceTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_usage_default() {
        let usage = ResourceUsage::new();
        assert_eq!(usage.memory_bytes, 0);
        assert_eq!(usage.cpu_time_ms, 0);
        assert_eq!(usage.allocations, 0);
        assert_eq!(usage.deallocations, 0);
        assert_eq!(usage.peak_memory_bytes, 0);
    }

    #[test]
    fn test_resource_usage_net_allocations() {
        let mut usage = ResourceUsage::new();
        usage.allocations = 10;
        usage.deallocations = 3;
        assert_eq!(usage.net_allocations(), 7);
    }

    #[test]
    fn test_resource_usage_reset() {
        let mut usage = ResourceUsage::new();
        usage.memory_bytes = 1024;
        usage.cpu_time_ms = 500;
        usage.allocations = 5;
        usage.reset();
        assert_eq!(usage.memory_bytes, 0);
        assert_eq!(usage.cpu_time_ms, 0);
        assert_eq!(usage.allocations, 0);
    }

    #[test]
    fn test_resource_limit_builder() {
        let limit = ResourceLimit::new()
            .max_memory(1024)
            .max_cpu_ms(5000)
            .max_allocations(100);
        assert_eq!(limit.max_memory_bytes, 1024);
        assert_eq!(limit.max_cpu_ms, 5000);
        assert_eq!(limit.max_allocations, 100);
        assert!(!limit.is_unrestricted());
    }

    #[test]
    fn test_resource_limit_unrestricted() {
        let limit = ResourceLimit::new();
        assert!(limit.is_unrestricted());
    }

    #[test]
    fn test_tracker_register_and_usage() {
        let mut tracker = ResourceTracker::new();
        tracker.register("plugin-a", ResourceLimit::new());
        assert_eq!(tracker.len(), 1);
        assert!(!tracker.is_empty());

        let usage = tracker.usage("plugin-a");
        assert!(usage.is_some());
        assert_eq!(usage.map(|u| u.memory_bytes), Some(0));
    }

    #[test]
    fn test_tracker_record_allocation() {
        let mut tracker = ResourceTracker::new();
        tracker.register("alloc-test", ResourceLimit::new().max_memory(2048));

        tracker
            .record_allocation("alloc-test", 512)
            .expect("within limit");
        tracker
            .record_allocation("alloc-test", 512)
            .expect("within limit");

        let usage = tracker.usage("alloc-test").expect("registered");
        assert_eq!(usage.memory_bytes, 1024);
        assert_eq!(usage.allocations, 2);
        assert_eq!(usage.peak_memory_bytes, 1024);
    }

    #[test]
    fn test_tracker_memory_limit_exceeded() {
        let mut tracker = ResourceTracker::new();
        tracker.register("limited", ResourceLimit::new().max_memory(100));

        let result = tracker.record_allocation("limited", 200);
        assert!(result.is_err());
        assert!(result
            .expect_err("should fail")
            .to_string()
            .contains("memory limit exceeded"));
        assert!(tracker.is_suspended("limited"));
    }

    #[test]
    fn test_tracker_cpu_time_limit() {
        let mut tracker = ResourceTracker::new();
        tracker.register("cpu-test", ResourceLimit::new().max_cpu_ms(1000));

        tracker
            .record_cpu_time("cpu-test", 500)
            .expect("within limit");
        tracker
            .record_cpu_time("cpu-test", 400)
            .expect("within limit");

        let result = tracker.record_cpu_time("cpu-test", 200);
        assert!(result.is_err());
        assert!(tracker.is_suspended("cpu-test"));
    }

    #[test]
    fn test_tracker_deallocation() {
        let mut tracker = ResourceTracker::new();
        tracker.register("dealloc-test", ResourceLimit::new());

        tracker.record_allocation("dealloc-test", 1024).expect("ok");
        tracker
            .record_deallocation("dealloc-test", 512)
            .expect("ok");

        let usage = tracker.usage("dealloc-test").expect("registered");
        assert_eq!(usage.memory_bytes, 512);
        assert_eq!(usage.deallocations, 1);
        assert_eq!(usage.total_freed_bytes, 512);
    }

    #[test]
    fn test_tracker_peak_memory() {
        let mut tracker = ResourceTracker::new();
        tracker.register("peak-test", ResourceLimit::new());

        tracker.record_allocation("peak-test", 1000).expect("ok");
        tracker.record_deallocation("peak-test", 800).expect("ok");
        tracker.record_allocation("peak-test", 500).expect("ok");

        let usage = tracker.usage("peak-test").expect("registered");
        assert_eq!(usage.peak_memory_bytes, 1000);
        assert_eq!(usage.memory_bytes, 700); // 1000 - 800 + 500
    }

    #[test]
    fn test_tracker_resume() {
        let mut tracker = ResourceTracker::new();
        tracker.register("resume-test", ResourceLimit::new().max_memory(100));

        // Exceed limit.
        let _ = tracker.record_allocation("resume-test", 200);
        assert!(tracker.is_suspended("resume-test"));

        // Resume.
        tracker.resume("resume-test").expect("resume ok");
        assert!(!tracker.is_suspended("resume-test"));
    }

    #[test]
    fn test_tracker_reset_usage() {
        let mut tracker = ResourceTracker::new();
        tracker.register("reset-test", ResourceLimit::new().max_memory(1000));

        tracker.record_allocation("reset-test", 500).expect("ok");
        tracker.reset_usage("reset-test").expect("reset ok");

        let usage = tracker.usage("reset-test").expect("registered");
        assert_eq!(usage.memory_bytes, 0);
        assert_eq!(usage.allocations, 0);
        assert_eq!(tracker.violation_count("reset-test"), Some(0));
    }

    #[test]
    fn test_tracker_update_limit() {
        let mut tracker = ResourceTracker::new();
        tracker.register("update-test", ResourceLimit::new().max_memory(100));

        // Increase limit.
        tracker
            .update_limit("update-test", ResourceLimit::new().max_memory(2000))
            .expect("update ok");

        let limit = tracker.limit("update-test").expect("registered");
        assert_eq!(limit.max_memory_bytes, 2000);
    }

    #[test]
    fn test_tracker_not_found() {
        let mut tracker = ResourceTracker::new();
        assert!(tracker.record_allocation("ghost", 100).is_err());
        assert!(tracker.record_deallocation("ghost", 100).is_err());
        assert!(tracker.record_cpu_time("ghost", 100).is_err());
        assert!(tracker.resume("ghost").is_err());
        assert!(tracker.reset_usage("ghost").is_err());
    }

    #[test]
    fn test_tracker_unregister() {
        let mut tracker = ResourceTracker::new();
        tracker.register("temp", ResourceLimit::new());
        assert!(tracker.unregister("temp"));
        assert!(!tracker.unregister("temp"));
        assert_eq!(tracker.len(), 0);
    }

    #[test]
    fn test_tracker_allocation_count_limit() {
        let mut tracker = ResourceTracker::new();
        tracker.register("alloc-limit", ResourceLimit::new().max_allocations(2));

        tracker
            .record_allocation("alloc-limit", 10)
            .expect("alloc 1");
        tracker
            .record_allocation("alloc-limit", 10)
            .expect("alloc 2");

        // Third allocation exceeds the limit of 2 net allocations.
        let result = tracker.record_allocation("alloc-limit", 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_tracker_violation_count() {
        let mut tracker = ResourceTracker::new();
        tracker.register("violations", ResourceLimit::new().max_memory(50));

        let _ = tracker.record_allocation("violations", 100);
        assert_eq!(tracker.violation_count("violations"), Some(1));
    }

    #[test]
    fn test_tracker_suspended_plugins() {
        let mut tracker = ResourceTracker::new();
        tracker.register("ok-plugin", ResourceLimit::new());
        tracker.register("bad-plugin", ResourceLimit::new().max_memory(10));

        let _ = tracker.record_allocation("bad-plugin", 100);

        let suspended = tracker.suspended_plugins();
        assert_eq!(suspended.len(), 1);
        assert!(suspended.contains(&"bad-plugin"));
    }

    #[test]
    fn test_tracker_summary() {
        let mut tracker = ResourceTracker::new();
        tracker.register("a", ResourceLimit::new());
        tracker.register("b", ResourceLimit::new());

        tracker.record_allocation("a", 100).expect("ok");
        tracker.record_cpu_time("b", 50).expect("ok");

        let summary = tracker.summary();
        assert_eq!(summary.len(), 2);
    }

    #[test]
    fn test_resource_usage_display() {
        let usage = ResourceUsage {
            memory_bytes: 1024,
            cpu_time_ms: 500,
            allocations: 10,
            deallocations: 3,
            peak_memory_bytes: 2048,
            total_allocated_bytes: 4096,
            total_freed_bytes: 3072,
        };
        let s = format!("{usage}");
        assert!(s.contains("1024B"));
        assert!(s.contains("500ms"));
        assert!(s.contains("peak=2048B"));
    }

    #[test]
    fn test_resource_limit_display() {
        let limit = ResourceLimit::new().max_memory(1024).max_cpu_ms(5000);
        let s = format!("{limit}");
        assert!(s.contains("1024B"));
        assert!(s.contains("5000ms"));
    }

    #[test]
    fn test_limit_violation_display() {
        let v = LimitViolation::Memory {
            current: 2000,
            limit: 1000,
        };
        assert!(format!("{v}").contains("memory limit exceeded"));

        let v2 = LimitViolation::CpuTime {
            current: 6000,
            limit: 5000,
        };
        assert!(format!("{v2}").contains("CPU time limit exceeded"));

        let v3 = LimitViolation::Allocations {
            current: 50,
            limit: 10,
        };
        assert!(format!("{v3}").contains("allocation limit exceeded"));
    }

    #[test]
    fn test_tracker_unlimited_allows_large_usage() {
        let mut tracker = ResourceTracker::new();
        tracker.register("unlimited", ResourceLimit::new()); // all zeros = unlimited

        tracker
            .record_allocation("unlimited", u64::MAX / 2)
            .expect("no limit");
        tracker
            .record_cpu_time("unlimited", u64::MAX / 2)
            .expect("no limit");

        assert!(!tracker.is_suspended("unlimited"));
    }
}

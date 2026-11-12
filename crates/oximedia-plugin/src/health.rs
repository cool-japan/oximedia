// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Plugin health check system.
//!
//! Provides a [`HealthCheck`] trait for plugins to implement liveness probes,
//! a [`HealthStatus`] enum representing possible plugin states, and a
//! [`HealthRegistry`] that tracks plugin health and can automatically disable
//! unhealthy plugins.
//!
//! # Example
//!
//! ```rust
//! use oximedia_plugin::health::{HealthStatus, HealthRegistry, SimpleHealthCheck};
//!
//! let mut registry = HealthRegistry::new();
//! registry.register("my-plugin", SimpleHealthCheck::always_healthy());
//! assert_eq!(registry.status("my-plugin"), Some(HealthStatus::Unknown));
//!
//! registry.check("my-plugin").expect("check should succeed");
//! assert_eq!(registry.status("my-plugin"), Some(HealthStatus::Healthy));
//! ```

use crate::error::{PluginError, PluginResult};
use std::collections::HashMap;
use std::time::Instant;

// ── HealthStatus ─────────────────────────────────────────────────────────────

/// The health state of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// The plugin is functioning correctly.
    Healthy,
    /// The plugin is operational but experiencing issues (e.g. high latency).
    Degraded,
    /// The plugin is not functioning and should not be used.
    Unhealthy,
    /// The plugin's health has not yet been determined.
    Unknown,
}

impl HealthStatus {
    /// Returns `true` if the plugin can still serve requests.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

// ── HealthCheck trait ────────────────────────────────────────────────────────

/// A health check that can probe a plugin's liveness.
///
/// Implementations should return the current health status of the plugin.
/// Checks should be fast (< 100ms) and side-effect-free.
pub trait HealthCheck: Send + Sync {
    /// Perform a health check and return the current status.
    fn check(&self) -> HealthStatus;

    /// Return a human-readable description of the check.
    fn description(&self) -> &str {
        "generic health check"
    }
}

// ── SimpleHealthCheck ────────────────────────────────────────────────────────

/// A simple health check that returns a configurable status.
///
/// Useful for testing and for plugins that report their own status.
pub struct SimpleHealthCheck {
    status: std::sync::Mutex<HealthStatus>,
    desc: String,
}

impl SimpleHealthCheck {
    /// Create a health check that always returns [`HealthStatus::Healthy`].
    #[must_use]
    pub fn always_healthy() -> Self {
        Self {
            status: std::sync::Mutex::new(HealthStatus::Healthy),
            desc: "always healthy".to_string(),
        }
    }

    /// Create a health check that always returns the given status.
    #[must_use]
    pub fn fixed(status: HealthStatus) -> Self {
        Self {
            status: std::sync::Mutex::new(status),
            desc: format!("fixed: {status}"),
        }
    }

    /// Create a health check with a custom description.
    #[must_use]
    pub fn with_description(status: HealthStatus, desc: impl Into<String>) -> Self {
        Self {
            status: std::sync::Mutex::new(status),
            desc: desc.into(),
        }
    }

    /// Update the status that this check will return.
    pub fn set_status(&self, status: HealthStatus) {
        if let Ok(mut guard) = self.status.lock() {
            *guard = status;
        }
    }
}

impl HealthCheck for SimpleHealthCheck {
    fn check(&self) -> HealthStatus {
        self.status
            .lock()
            .map(|g| *g)
            .unwrap_or(HealthStatus::Unknown)
    }

    fn description(&self) -> &str {
        &self.desc
    }
}

// ── HealthEntry ──────────────────────────────────────────────────────────────

/// Internal record for a plugin's health state.
struct HealthEntry {
    /// The health check implementation.
    checker: Box<dyn HealthCheck>,
    /// Current health status.
    status: HealthStatus,
    /// Whether the plugin has been disabled due to poor health.
    disabled: bool,
    /// Number of consecutive unhealthy checks.
    consecutive_failures: u32,
    /// Threshold: disable after this many consecutive unhealthy checks.
    failure_threshold: u32,
    /// When the last check was performed.
    last_checked: Option<Instant>,
    /// Total number of checks performed.
    check_count: u64,
}

// ── HealthRegistry ───────────────────────────────────────────────────────────

/// Registry that tracks plugin health and can automatically disable
/// unhealthy plugins.
///
/// Each plugin is associated with a [`HealthCheck`] implementation. The
/// registry tracks consecutive failures and can automatically mark a plugin
/// as disabled after a configurable number of consecutive unhealthy checks.
pub struct HealthRegistry {
    entries: HashMap<String, HealthEntry>,
    /// Default failure threshold for new entries.
    default_threshold: u32,
}

impl HealthRegistry {
    /// Create a new empty health registry with a default failure threshold of 3.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            default_threshold: 3,
        }
    }

    /// Create a health registry with a custom default failure threshold.
    #[must_use]
    pub fn with_threshold(threshold: u32) -> Self {
        Self {
            entries: HashMap::new(),
            default_threshold: threshold.max(1),
        }
    }

    /// Register a health check for a plugin.
    ///
    /// If a check is already registered for this plugin, it is replaced.
    pub fn register(&mut self, name: impl Into<String>, checker: impl HealthCheck + 'static) {
        let name = name.into();
        let entry = HealthEntry {
            checker: Box::new(checker),
            status: HealthStatus::Unknown,
            disabled: false,
            consecutive_failures: 0,
            failure_threshold: self.default_threshold,
            last_checked: None,
            check_count: 0,
        };
        self.entries.insert(name, entry);
    }

    /// Register a health check with a custom failure threshold.
    pub fn register_with_threshold(
        &mut self,
        name: impl Into<String>,
        checker: impl HealthCheck + 'static,
        threshold: u32,
    ) {
        let name = name.into();
        let entry = HealthEntry {
            checker: Box::new(checker),
            status: HealthStatus::Unknown,
            disabled: false,
            consecutive_failures: 0,
            failure_threshold: threshold.max(1),
            last_checked: None,
            check_count: 0,
        };
        self.entries.insert(name, entry);
    }

    /// Unregister a plugin's health check.
    ///
    /// Returns `true` if the plugin was registered, `false` otherwise.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    /// Perform a health check for a specific plugin.
    ///
    /// Updates the internal status and failure tracking. If the plugin
    /// exceeds the failure threshold, it is automatically disabled.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn check(&mut self, name: &str) -> PluginResult<HealthStatus> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;

        let new_status = entry.checker.check();
        entry.status = new_status;
        entry.last_checked = Some(Instant::now());
        entry.check_count += 1;

        match new_status {
            HealthStatus::Unhealthy => {
                entry.consecutive_failures += 1;
                if entry.consecutive_failures >= entry.failure_threshold {
                    entry.disabled = true;
                    tracing::warn!(
                        "Plugin '{}' disabled after {} consecutive failures",
                        name,
                        entry.consecutive_failures
                    );
                }
            }
            HealthStatus::Healthy => {
                entry.consecutive_failures = 0;
                // Re-enable if it was disabled and is now healthy.
                if entry.disabled {
                    entry.disabled = false;
                    tracing::info!("Plugin '{}' re-enabled (now healthy)", name);
                }
            }
            HealthStatus::Degraded => {
                // Degraded does not count as a failure, but doesn't reset either.
            }
            HealthStatus::Unknown => {
                // Unknown doesn't affect failure count.
            }
        }

        Ok(new_status)
    }

    /// Check all registered plugins and return their statuses.
    pub fn check_all(&mut self) -> Vec<(String, HealthStatus)> {
        let names: Vec<String> = self.entries.keys().cloned().collect();
        let mut results = Vec::with_capacity(names.len());
        for name in names {
            if let Ok(status) = self.check(&name) {
                results.push((name, status));
            }
        }
        results
    }

    /// Get the current status of a plugin without performing a check.
    #[must_use]
    pub fn status(&self, name: &str) -> Option<HealthStatus> {
        self.entries.get(name).map(|e| e.status)
    }

    /// Check if a plugin is currently disabled due to health failures.
    #[must_use]
    pub fn is_disabled(&self, name: &str) -> bool {
        self.entries.get(name).map(|e| e.disabled).unwrap_or(false)
    }

    /// Check if a plugin is usable (registered, not disabled, and status is usable).
    #[must_use]
    pub fn is_usable(&self, name: &str) -> bool {
        self.entries
            .get(name)
            .map(|e| !e.disabled && e.status.is_usable())
            .unwrap_or(false)
    }

    /// Manually enable a previously disabled plugin.
    ///
    /// Resets the consecutive failure counter.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn enable(&mut self, name: &str) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;
        entry.disabled = false;
        entry.consecutive_failures = 0;
        Ok(())
    }

    /// Manually disable a plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if the plugin is not registered.
    pub fn disable(&mut self, name: &str) -> PluginResult<()> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;
        entry.disabled = true;
        Ok(())
    }

    /// Get the number of consecutive failures for a plugin.
    #[must_use]
    pub fn consecutive_failures(&self, name: &str) -> Option<u32> {
        self.entries.get(name).map(|e| e.consecutive_failures)
    }

    /// Get the total number of checks performed for a plugin.
    #[must_use]
    pub fn check_count(&self, name: &str) -> Option<u64> {
        self.entries.get(name).map(|e| e.check_count)
    }

    /// Get the number of registered health checks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the registry has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// List all registered plugin names.
    pub fn plugin_names(&self) -> Vec<&str> {
        self.entries.keys().map(|k| k.as_str()).collect()
    }

    /// List all disabled plugins.
    pub fn disabled_plugins(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, e)| e.disabled)
            .map(|(k, _)| k.as_str())
            .collect()
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "healthy");
        assert_eq!(format!("{}", HealthStatus::Degraded), "degraded");
        assert_eq!(format!("{}", HealthStatus::Unhealthy), "unhealthy");
        assert_eq!(format!("{}", HealthStatus::Unknown), "unknown");
    }

    #[test]
    fn test_health_status_is_usable() {
        assert!(HealthStatus::Healthy.is_usable());
        assert!(HealthStatus::Degraded.is_usable());
        assert!(!HealthStatus::Unhealthy.is_usable());
        assert!(!HealthStatus::Unknown.is_usable());
    }

    #[test]
    fn test_health_status_default() {
        assert_eq!(HealthStatus::default(), HealthStatus::Unknown);
    }

    #[test]
    fn test_simple_health_check_always_healthy() {
        let check = SimpleHealthCheck::always_healthy();
        assert_eq!(check.check(), HealthStatus::Healthy);
    }

    #[test]
    fn test_simple_health_check_fixed() {
        let check = SimpleHealthCheck::fixed(HealthStatus::Degraded);
        assert_eq!(check.check(), HealthStatus::Degraded);
    }

    #[test]
    fn test_simple_health_check_set_status() {
        let check = SimpleHealthCheck::always_healthy();
        assert_eq!(check.check(), HealthStatus::Healthy);
        check.set_status(HealthStatus::Unhealthy);
        assert_eq!(check.check(), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_simple_health_check_description() {
        let check = SimpleHealthCheck::with_description(HealthStatus::Healthy, "my custom check");
        assert_eq!(check.description(), "my custom check");
    }

    #[test]
    fn test_registry_new_is_empty() {
        let registry = HealthRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_register_and_check() {
        let mut registry = HealthRegistry::new();
        registry.register("plugin-a", SimpleHealthCheck::always_healthy());
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.status("plugin-a"), Some(HealthStatus::Unknown));

        let result = registry.check("plugin-a");
        assert!(result.is_ok());
        assert_eq!(result.expect("checked"), HealthStatus::Healthy);
        assert_eq!(registry.status("plugin-a"), Some(HealthStatus::Healthy));
    }

    #[test]
    fn test_registry_check_not_found() {
        let mut registry = HealthRegistry::new();
        let result = registry.check("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_auto_disable_on_failures() {
        let mut registry = HealthRegistry::with_threshold(3);
        let check = SimpleHealthCheck::fixed(HealthStatus::Unhealthy);
        registry.register("bad-plugin", check);

        // First two failures should not disable.
        registry.check("bad-plugin").expect("check 1");
        assert!(!registry.is_disabled("bad-plugin"));
        registry.check("bad-plugin").expect("check 2");
        assert!(!registry.is_disabled("bad-plugin"));

        // Third failure triggers disable.
        registry.check("bad-plugin").expect("check 3");
        assert!(registry.is_disabled("bad-plugin"));
    }

    #[test]
    fn test_registry_healthy_resets_failures() {
        use std::sync::Arc;

        // Use Arc<SimpleHealthCheck> so we can set_status after registration.
        let check = Arc::new(SimpleHealthCheck::fixed(HealthStatus::Unhealthy));
        let check_clone = Arc::clone(&check);

        let mut registry = HealthRegistry::with_threshold(5);
        registry.register("flaky", ArcHealthCheck(check_clone));

        // Two failures.
        registry.check("flaky").expect("check 1");
        registry.check("flaky").expect("check 2");
        assert_eq!(registry.consecutive_failures("flaky"), Some(2));

        // Now it recovers.
        check.set_status(HealthStatus::Healthy);
        registry.check("flaky").expect("check 3");
        assert_eq!(registry.consecutive_failures("flaky"), Some(0));
    }

    /// Wrapper to use `Arc<SimpleHealthCheck>` as a `HealthCheck`.
    struct ArcHealthCheck(std::sync::Arc<SimpleHealthCheck>);

    impl HealthCheck for ArcHealthCheck {
        fn check(&self) -> HealthStatus {
            self.0.check()
        }

        fn description(&self) -> &str {
            self.0.description()
        }
    }

    #[test]
    fn test_registry_manual_enable_disable() {
        let mut registry = HealthRegistry::new();
        registry.register("plugin-x", SimpleHealthCheck::always_healthy());

        registry.disable("plugin-x").expect("disable");
        assert!(registry.is_disabled("plugin-x"));

        registry.enable("plugin-x").expect("enable");
        assert!(!registry.is_disabled("plugin-x"));
    }

    #[test]
    fn test_registry_enable_not_found() {
        let mut registry = HealthRegistry::new();
        assert!(registry.enable("ghost").is_err());
    }

    #[test]
    fn test_registry_disable_not_found() {
        let mut registry = HealthRegistry::new();
        assert!(registry.disable("ghost").is_err());
    }

    #[test]
    fn test_registry_is_usable() {
        let mut registry = HealthRegistry::new();
        registry.register("good", SimpleHealthCheck::always_healthy());

        // Before any check, status is Unknown -> not usable.
        assert!(!registry.is_usable("good"));

        // After check, status is Healthy -> usable.
        registry.check("good").expect("check");
        assert!(registry.is_usable("good"));

        // After disable, not usable even though healthy.
        registry.disable("good").expect("disable");
        assert!(!registry.is_usable("good"));
    }

    #[test]
    fn test_registry_unregister() {
        let mut registry = HealthRegistry::new();
        registry.register("temp", SimpleHealthCheck::always_healthy());
        assert_eq!(registry.len(), 1);

        assert!(registry.unregister("temp"));
        assert_eq!(registry.len(), 0);
        assert!(!registry.unregister("temp")); // already gone
    }

    #[test]
    fn test_registry_check_all() {
        let mut registry = HealthRegistry::new();
        registry.register("a", SimpleHealthCheck::always_healthy());
        registry.register("b", SimpleHealthCheck::fixed(HealthStatus::Degraded));

        let results = registry.check_all();
        assert_eq!(results.len(), 2);

        let statuses: HashMap<&str, HealthStatus> =
            results.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        assert_eq!(statuses.get("a"), Some(&HealthStatus::Healthy));
        assert_eq!(statuses.get("b"), Some(&HealthStatus::Degraded));
    }

    #[test]
    fn test_registry_check_count() {
        let mut registry = HealthRegistry::new();
        registry.register("counted", SimpleHealthCheck::always_healthy());

        assert_eq!(registry.check_count("counted"), Some(0));
        registry.check("counted").expect("c1");
        registry.check("counted").expect("c2");
        registry.check("counted").expect("c3");
        assert_eq!(registry.check_count("counted"), Some(3));
    }

    #[test]
    fn test_registry_plugin_names() {
        let mut registry = HealthRegistry::new();
        registry.register("alpha", SimpleHealthCheck::always_healthy());
        registry.register("beta", SimpleHealthCheck::always_healthy());

        let mut names = registry.plugin_names();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_registry_disabled_plugins() {
        let mut registry = HealthRegistry::new();
        registry.register("ok", SimpleHealthCheck::always_healthy());
        registry.register("bad", SimpleHealthCheck::always_healthy());
        registry.disable("bad").expect("disable");

        let disabled = registry.disabled_plugins();
        assert_eq!(disabled, vec!["bad"]);
    }

    #[test]
    fn test_registry_status_nonexistent() {
        let registry = HealthRegistry::new();
        assert_eq!(registry.status("ghost"), None);
    }

    #[test]
    fn test_registry_is_disabled_nonexistent() {
        let registry = HealthRegistry::new();
        assert!(!registry.is_disabled("ghost"));
    }

    #[test]
    fn test_registry_custom_threshold() {
        let mut registry = HealthRegistry::new();
        registry.register_with_threshold(
            "strict",
            SimpleHealthCheck::fixed(HealthStatus::Unhealthy),
            1,
        );

        // Single failure should disable with threshold=1.
        registry.check("strict").expect("check");
        assert!(registry.is_disabled("strict"));
    }

    #[test]
    fn test_degraded_does_not_increment_failures() {
        let mut registry = HealthRegistry::with_threshold(2);
        registry.register("degraded", SimpleHealthCheck::fixed(HealthStatus::Degraded));

        registry.check("degraded").expect("c1");
        registry.check("degraded").expect("c2");
        registry.check("degraded").expect("c3");

        // Degraded does not count as failure.
        assert_eq!(registry.consecutive_failures("degraded"), Some(0));
        assert!(!registry.is_disabled("degraded"));
    }
}

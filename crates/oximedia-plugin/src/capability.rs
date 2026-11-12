// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Plugin capability declaration and negotiation.
//!
//! [`PluginCapabilitySet`] allows a plugin to declare the features and
//! protocols it supports.  Two capability sets can be compared with
//! [`PluginCapabilitySet::negotiated_caps`] to obtain the intersection of
//! capabilities both parties support — a standard capability handshake
//! used when establishing a plugin session or validating plugin
//! compatibility.
//!
//! # Example
//!
//! ```rust
//! use oximedia_plugin::capability::PluginCapabilitySet;
//!
//! let mut host = PluginCapabilitySet::new();
//! host.declare("hardware-decode");
//! host.declare("hevc");
//!
//! let mut plugin = PluginCapabilitySet::new();
//! plugin.declare("hevc");
//! plugin.declare("av1");
//!
//! let common = host.negotiated_caps(&plugin);
//! assert_eq!(common, vec!["hevc".to_string()]);
//! ```

use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// PluginCapabilitySet
// ---------------------------------------------------------------------------

/// An ordered, de-duplicated set of capability strings.
///
/// Capability strings are case-sensitive; callers should normalise them
/// before use (e.g. all lower-case, kebab-case) to ensure consistent
/// matching.
#[derive(Debug, Clone, Default)]
pub struct PluginCapabilitySet {
    caps: BTreeSet<String>,
}

impl PluginCapabilitySet {
    /// Create an empty capability set.
    pub fn new() -> Self {
        Self {
            caps: BTreeSet::new(),
        }
    }

    /// Declare a capability by name.
    ///
    /// Declaring the same capability more than once is idempotent.
    pub fn declare(&mut self, cap: impl Into<String>) {
        self.caps.insert(cap.into());
    }

    /// Remove a previously declared capability.
    ///
    /// Returns `true` if the capability was present, `false` otherwise.
    pub fn retract(&mut self, cap: &str) -> bool {
        self.caps.remove(cap)
    }

    /// Returns `true` if this set contains the named capability.
    #[must_use]
    pub fn has(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }

    /// Return the intersection of `self` and `other` as a sorted `Vec`.
    ///
    /// The result contains only capabilities declared by **both** sides,
    /// which is the negotiated set that the two parties can actually use.
    #[must_use]
    pub fn negotiated_caps(&self, other: &PluginCapabilitySet) -> Vec<String> {
        self.caps.intersection(&other.caps).cloned().collect()
    }

    /// Return all declared capabilities as a sorted `Vec`.
    #[must_use]
    pub fn all_caps(&self) -> Vec<String> {
        self.caps.iter().cloned().collect()
    }

    /// Return the number of declared capabilities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.caps.len()
    }

    /// Returns `true` if no capabilities are declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_declare_and_has() {
        let mut caps = PluginCapabilitySet::new();
        caps.declare("hardware-decode");
        assert!(caps.has("hardware-decode"));
        assert!(!caps.has("software-decode"));
    }

    #[test]
    fn test_declare_idempotent() {
        let mut caps = PluginCapabilitySet::new();
        caps.declare("hevc");
        caps.declare("hevc");
        assert_eq!(caps.len(), 1);
    }

    #[test]
    fn test_retract_existing() {
        let mut caps = PluginCapabilitySet::new();
        caps.declare("av1");
        assert!(caps.retract("av1"));
        assert!(!caps.has("av1"));
    }

    #[test]
    fn test_retract_nonexistent() {
        let mut caps = PluginCapabilitySet::new();
        assert!(!caps.retract("ghost"));
    }

    #[test]
    fn test_negotiated_caps_intersection() {
        let mut host = PluginCapabilitySet::new();
        host.declare("hevc");
        host.declare("hardware-decode");
        host.declare("hdr");

        let mut plugin = PluginCapabilitySet::new();
        plugin.declare("hevc");
        plugin.declare("av1");

        let common = host.negotiated_caps(&plugin);
        assert_eq!(common, vec!["hevc".to_string()]);
    }

    #[test]
    fn test_negotiated_caps_empty_intersection() {
        let mut a = PluginCapabilitySet::new();
        a.declare("x");
        let mut b = PluginCapabilitySet::new();
        b.declare("y");
        assert!(a.negotiated_caps(&b).is_empty());
    }

    #[test]
    fn test_negotiated_caps_both_empty() {
        let a = PluginCapabilitySet::new();
        let b = PluginCapabilitySet::new();
        assert!(a.negotiated_caps(&b).is_empty());
    }

    #[test]
    fn test_all_caps_sorted() {
        let mut caps = PluginCapabilitySet::new();
        caps.declare("z-cap");
        caps.declare("a-cap");
        caps.declare("m-cap");
        let all = caps.all_caps();
        assert_eq!(all, vec!["a-cap", "m-cap", "z-cap"]);
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut caps = PluginCapabilitySet::new();
        assert!(caps.is_empty());
        caps.declare("one");
        assert_eq!(caps.len(), 1);
        assert!(!caps.is_empty());
    }

    #[test]
    fn test_negotiated_full_match() {
        let mut a = PluginCapabilitySet::new();
        a.declare("codec-av1");
        a.declare("codec-vp9");

        let mut b = PluginCapabilitySet::new();
        b.declare("codec-vp9");
        b.declare("codec-av1");

        let mut common = a.negotiated_caps(&b);
        common.sort();
        assert_eq!(common, vec!["codec-av1", "codec-vp9"]);
    }
}

// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! The crate's single error type.
//!
//! Hand-written `Display` + [`std::error::Error`] (no `thiserror`) to keep the
//! wasm dependency tree and binary size minimal. All fallible scope operations
//! return [`ScopeError`]; nothing in a production path panics.

use core::fmt;
use oximedia_web_core::CoreError;

/// Everything that can go wrong rendering a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeError {
    /// A geometry / buffer-length error bubbled up from [`oximedia_web_core`]
    /// (zero dimension, wrong input length, overflow, ...).
    Core(CoreError),
    /// The caller-provided output buffer is the wrong length for the configured
    /// scope dimensions. `expected` is `scope_w * scope_h * 4`.
    OutputLength {
        /// Required length in bytes.
        expected: usize,
        /// Length the caller actually supplied.
        actual: usize,
    },
    /// A `u32` mode selector at the wasm boundary did not name a known mode.
    InvalidMode(u32),
    /// A `u32` preset selector at the wasm boundary did not name a known preset.
    InvalidPreset(u32),
    /// The configured scope canvas is too small to host the requested layout
    /// (e.g. an RGB parade in fewer than three columns).
    ScopeTooSmall,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(e) => write!(f, "core error: {e}"),
            Self::OutputLength { expected, actual } => write!(
                f,
                "scope output buffer length {actual} does not match required {expected}"
            ),
            Self::InvalidMode(m) => write!(f, "unknown scope mode selector {m}"),
            Self::InvalidPreset(p) => write!(f, "unknown false-colour preset selector {p}"),
            Self::ScopeTooSmall => {
                write!(f, "scope canvas is too small for the requested layout")
            }
        }
    }
}

impl std::error::Error for ScopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(e) => Some(e),
            _ => None,
        }
    }
}

impl From<CoreError> for ScopeError {
    fn from(e: CoreError) -> Self {
        Self::Core(e)
    }
}

/// Crate result alias.
pub type Result<T> = core::result::Result<T, ScopeError>;

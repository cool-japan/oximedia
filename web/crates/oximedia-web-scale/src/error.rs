// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Error type for `oximedia-web-scale`.
//!
//! The crate never panics on malformed input (wrong buffer length, zero
//! dimensions, unknown filter name): every fallible entry point returns
//! `Result<_, ScaleError>`. This mirrors the hand-written `Display` +
//! `std::error::Error` style of [`oximedia_web_core::CoreError`] rather than
//! pulling in `thiserror`.

use core::fmt;

use oximedia_web_core::CoreError;

/// Errors returned by [`crate::Filter::parse`], [`crate::weights::WeightTable::build`]
/// and [`crate::Resizer`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScaleError {
    /// A frame-geometry or buffer-length error from `oximedia-web-core`
    /// (zero dimension, buffer length mismatch, dimension overflow).
    Core(CoreError),

    /// [`crate::Filter::parse`] was given a name that does not match any
    /// known filter.
    UnknownFilter {
        /// The unrecognized filter name, as supplied by the caller.
        name: String,
    },
}

impl fmt::Display for ScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(e) => write!(f, "{e}"),
            Self::UnknownFilter { name } => write!(
                f,
                "unknown scaling filter '{name}' (expected one of: bilinear, catmull-rom, mitchell, lanczos3)"
            ),
        }
    }
}

impl std::error::Error for ScaleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(e) => Some(e),
            Self::UnknownFilter { .. } => None,
        }
    }
}

impl From<CoreError> for ScaleError {
    fn from(e: CoreError) -> Self {
        Self::Core(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_non_empty_for_every_variant() {
        let variants = [
            ScaleError::Core(CoreError::ZeroDimension),
            ScaleError::UnknownFilter {
                name: "bogus".to_owned(),
            },
        ];
        for v in variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn unknown_filter_message_contains_the_name() {
        let e = ScaleError::UnknownFilter {
            name: "bogus".to_owned(),
        };
        assert!(e.to_string().contains("bogus"));
    }

    #[test]
    fn from_core_error_wraps() {
        let e: ScaleError = CoreError::ZeroDimension.into();
        assert_eq!(e, ScaleError::Core(CoreError::ZeroDimension));
    }

    #[test]
    fn implements_std_error_and_reports_source() {
        use std::error::Error;
        let e = ScaleError::Core(CoreError::ZeroDimension);
        assert!(e.source().is_some());
        let e = ScaleError::UnknownFilter {
            name: "x".to_owned(),
        };
        assert!(e.source().is_none());
    }
}

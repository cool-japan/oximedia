// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Error type for the dependency-free web core kernels.
//!
//! The crate has **zero** dependencies, so the error type is a small
//! hand-written enum that implements [`core::fmt::Display`] and
//! [`std::error::Error`] directly instead of pulling in `thiserror`.

use core::fmt;

/// Errors returned by the validation and conversion kernels.
///
/// Every fallible kernel returns `Result<_, CoreError>` and never panics on
/// malformed input: a mismatched buffer length yields
/// [`CoreError::BufferLength`] rather than an out-of-bounds index panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    /// A width or height of zero was supplied where a positive value is
    /// required.
    ZeroDimension,

    /// A buffer's length did not match the length implied by the frame
    /// dimensions.
    BufferLength {
        /// Length required by the frame dimensions.
        expected: usize,
        /// Length that was actually supplied.
        actual: usize,
    },

    /// Two operand buffers that must have equal length did not.
    LengthMismatch {
        /// Length of the left-hand operand.
        left: usize,
        /// Length of the right-hand operand.
        right: usize,
    },

    /// A dimension product overflowed `usize`.
    ///
    /// On `wasm32` `usize` is 32-bit, so `width * height * 4` can overflow for
    /// pathological inputs; the kernels use checked arithmetic and surface the
    /// overflow here instead of wrapping.
    DimensionOverflow,
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimension => f.write_str("frame width and height must both be non-zero"),
            Self::BufferLength { expected, actual } => write!(
                f,
                "buffer length mismatch: expected {expected} bytes/elements, got {actual}"
            ),
            Self::LengthMismatch { left, right } => {
                write!(f, "operand length mismatch: {left} != {right}")
            }
            Self::DimensionOverflow => {
                f.write_str("frame dimensions overflow usize when computing buffer length")
            }
        }
    }
}

impl std::error::Error for CoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_non_empty_for_every_variant() {
        let variants = [
            CoreError::ZeroDimension,
            CoreError::BufferLength {
                expected: 1,
                actual: 2,
            },
            CoreError::LengthMismatch { left: 3, right: 4 },
            CoreError::DimensionOverflow,
        ];
        for v in variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn implements_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&CoreError::ZeroDimension);
    }

    #[test]
    fn buffer_length_message_contains_numbers() {
        let msg = CoreError::BufferLength {
            expected: 64,
            actual: 48,
        }
        .to_string();
        assert!(msg.contains("64"));
        assert!(msg.contains("48"));
    }
}

// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Error type for `oximedia-web-quality`.
//!
//! Wraps [`oximedia_web_core::CoreError`] (buffer-length / dimension
//! failures from the shared frame/YUV/normalize kernels) and adds the one
//! failure mode specific to this crate: a frame too small to hold a single
//! 11x11 SSIM window.

use core::fmt;

use oximedia_web_core::CoreError;

/// Errors returned by the PSNR/SSIM kernels and [`crate::QualityAnalyzer`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityError {
    /// A buffer-length / dimension failure from `oximedia-web-core`.
    Core(CoreError),

    /// The frame is too small to fit a single `window x window` SSIM
    /// window: `width` or `height` is less than `window`.
    WindowTooLarge {
        /// Frame width in pixels.
        width: usize,
        /// Frame height in pixels.
        height: usize,
        /// SSIM window size (always 11 in this crate).
        window: usize,
    },
}

impl From<CoreError> for QualityError {
    fn from(err: CoreError) -> Self {
        Self::Core(err)
    }
}

impl fmt::Display for QualityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(err) => write!(f, "{err}"),
            Self::WindowTooLarge {
                width,
                height,
                window,
            } => write!(
                f,
                "SSIM window {window}x{window} does not fit in a {width}x{height} frame"
            ),
        }
    }
}

impl std::error::Error for QualityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(err) => Some(err),
            Self::WindowTooLarge { .. } => None,
        }
    }
}

/// Convenience alias used throughout this crate.
pub type Result<T> = core::result::Result<T, QualityError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn display_is_non_empty_for_every_variant() {
        let variants = [
            QualityError::Core(CoreError::ZeroDimension),
            QualityError::WindowTooLarge {
                width: 4,
                height: 4,
                window: 11,
            },
        ];
        for v in variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn implements_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&QualityError::WindowTooLarge {
            width: 1,
            height: 1,
            window: 11,
        });
    }

    #[test]
    fn from_core_error_wraps() {
        let err: QualityError = CoreError::ZeroDimension.into();
        assert!(matches!(err, QualityError::Core(CoreError::ZeroDimension)));
        assert!(err.source().is_some());
    }

    #[test]
    fn window_too_large_message_contains_numbers() {
        let msg = QualityError::WindowTooLarge {
            width: 4,
            height: 5,
            window: 11,
        }
        .to_string();
        assert!(msg.contains('4'));
        assert!(msg.contains('5'));
        assert!(msg.contains("11"));
    }
}

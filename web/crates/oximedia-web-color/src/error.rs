// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Error type for the `oximedia-web-color` crate.

/// Errors produced by the colour pipeline, gamut mapper, LUT engine and
/// `.cube` parser.
///
/// The enum is `#[non_exhaustive]` so new variants can be added without a
/// breaking change.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum ColorError {
    /// A pixel buffer had the wrong length for its declared dimensions.
    BufferLength {
        /// Required length in elements.
        expected: usize,
        /// Actual length in elements.
        actual: usize,
    },
    /// Source and destination buffers differ in length.
    LengthMismatch {
        /// Source length in elements.
        left: usize,
        /// Destination length in elements.
        right: usize,
    },
    /// A buffer length is not a multiple of 4 (RGBA interleaved).
    NotRgba {
        /// Offending length in elements.
        len: usize,
    },
    /// A width or height of zero was supplied.
    ZeroDimension,
    /// A numeric parameter was NaN or infinite.
    NonFinite {
        /// Which parameter was non-finite.
        what: &'static str,
    },
    /// A numeric parameter was outside its valid range.
    OutOfRange {
        /// Which parameter was out of range.
        what: &'static str,
    },
    /// An enum-selector string was not recognised.
    UnknownName {
        /// What kind of name was expected (e.g. `"tone-map operator"`).
        kind: &'static str,
        /// The unrecognised input.
        name: String,
    },
    /// A 3D LUT size was outside the supported `2..=129` range.
    LutSize {
        /// The rejected size.
        size: usize,
    },
    /// A 3D LUT data vector had the wrong length for its size.
    LutDataLength {
        /// Required length (`size³ × 3`).
        expected: usize,
        /// Actual length.
        actual: usize,
    },
    /// A 3D LUT contained a NaN or infinite entry.
    LutNonFinite,
    /// A LUT domain was degenerate (`DOMAIN_MAX ≤ DOMAIN_MIN` or non-finite).
    LutDomain,
    /// A `.cube` file failed to parse.
    CubeParse {
        /// 1-based line number of the offending line (0 = whole file).
        line: usize,
        /// Human-readable description of the problem.
        message: String,
    },
    /// The `.cube` file declares a 1D LUT (`LUT_1D_SIZE`), which this module
    /// does not apply — only 3D LUTs are supported.
    CubeIs1d,
}

impl core::fmt::Display for ColorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferLength { expected, actual } => {
                write!(f, "buffer length {actual} does not match expected {expected}")
            }
            Self::LengthMismatch { left, right } => {
                write!(f, "source length {left} != destination length {right}")
            }
            Self::NotRgba { len } => {
                write!(f, "buffer length {len} is not a multiple of 4 (RGBA)")
            }
            Self::ZeroDimension => write!(f, "width and height must be non-zero"),
            Self::NonFinite { what } => write!(f, "{what} must be a finite number"),
            Self::OutOfRange { what } => write!(f, "{what} is out of range"),
            Self::UnknownName { kind, name } => {
                write!(f, "unknown {kind}: {name:?}")
            }
            Self::LutSize { size } => {
                write!(f, "LUT_3D_SIZE {size} is outside the supported range 2..=129")
            }
            Self::LutDataLength { expected, actual } => {
                write!(f, "LUT data length {actual} does not match size^3*3 = {expected}")
            }
            Self::LutNonFinite => write!(f, "LUT data contains NaN or infinite values"),
            Self::LutDomain => {
                write!(f, "LUT domain is degenerate (DOMAIN_MAX must exceed DOMAIN_MIN)")
            }
            Self::CubeParse { line, message } => {
                if *line == 0 {
                    write!(f, ".cube parse error: {message}")
                } else {
                    write!(f, ".cube parse error at line {line}: {message}")
                }
            }
            Self::CubeIs1d => write!(
                f,
                ".cube file declares LUT_1D_SIZE; only 3D LUTs (LUT_3D_SIZE) are supported"
            ),
        }
    }
}

impl std::error::Error for ColorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_non_empty_for_every_variant() {
        let variants: Vec<ColorError> = vec![
            ColorError::BufferLength { expected: 4, actual: 3 },
            ColorError::LengthMismatch { left: 1, right: 2 },
            ColorError::NotRgba { len: 5 },
            ColorError::ZeroDimension,
            ColorError::NonFinite { what: "exposure" },
            ColorError::OutOfRange { what: "peak nits" },
            ColorError::UnknownName { kind: "transfer", name: "bogus".to_string() },
            ColorError::LutSize { size: 10000 },
            ColorError::LutDataLength { expected: 24, actual: 12 },
            ColorError::LutNonFinite,
            ColorError::LutDomain,
            ColorError::CubeParse { line: 3, message: "bad".to_string() },
            ColorError::CubeParse { line: 0, message: "empty".to_string() },
            ColorError::CubeIs1d,
        ];
        for v in variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn implements_std_error() {
        fn assert_err<E: std::error::Error>(_: &E) {}
        assert_err(&ColorError::ZeroDimension);
    }
}

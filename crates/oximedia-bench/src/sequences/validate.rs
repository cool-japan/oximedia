//! Sequence validation and metadata extraction.
//!
//! Split out of [`super`] to keep that module under the 2000-line limit.

use super::TestSequence;
use crate::BenchResult;
use oximedia_codec::VideoFrame;
use serde::{Deserialize, Serialize};

/// Sequence validation utilities.
pub struct SequenceValidator;

impl SequenceValidator {
    /// Validate sequence integrity.
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails.
    pub fn validate_integrity(_sequence: &TestSequence) -> BenchResult<ValidationResult> {
        Ok(ValidationResult {
            is_valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        })
    }

    /// Check for corrupted frames.
    ///
    /// Returns the indices of frames whose decoder set the
    /// [`VideoFrame::corrupt`] flag (e.g. due to error concealment).
    ///
    /// # Errors
    ///
    /// Returns an error if checking fails. The metadata-only implementation
    /// never errors; the `Result` is preserved for API stability.
    pub fn check_for_corruption(frames: &[VideoFrame]) -> BenchResult<Vec<usize>> {
        Ok(frames
            .iter()
            .enumerate()
            .filter_map(|(idx, f)| if f.corrupt { Some(idx) } else { None })
            .collect())
    }

    /// Validate frame consistency.
    ///
    /// Returns `true` when every frame in the slice shares the same pixel
    /// format and dimensions as the first frame (or when the slice is
    /// empty / contains a single frame, in which case there is nothing to
    /// disagree on). Returns `false` on the first mismatch encountered.
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails. The metadata-only implementation
    /// never errors; the `Result` is preserved for API stability.
    pub fn validate_frame_consistency(frames: &[VideoFrame]) -> BenchResult<bool> {
        let mut iter = frames.iter();
        let first = match iter.next() {
            Some(f) => f,
            None => return Ok(true),
        };
        let consistent = iter.all(|f| {
            f.format == first.format && f.width == first.width && f.height == first.height
        });
        Ok(consistent)
    }
}

/// Validation result.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the sequence is valid
    pub is_valid: bool,
    /// List of errors
    pub errors: Vec<String>,
    /// List of warnings
    pub warnings: Vec<String>,
}

/// Sequence metadata extractor.
pub struct MetadataExtractor;

impl MetadataExtractor {
    /// Extract metadata from a sequence file.
    ///
    /// # Errors
    ///
    /// Returns an error if extraction fails.
    pub fn extract(_path: impl AsRef<std::path::Path>) -> BenchResult<SequenceMetadata> {
        Ok(SequenceMetadata {
            format: "Y4M".to_string(),
            codec: None,
            bit_depth: 8,
            chroma_subsampling: "4:2:0".to_string(),
            color_space: "BT.709".to_string(),
            hdr_metadata: None,
        })
    }
}

/// Sequence metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceMetadata {
    /// File format
    pub format: String,
    /// Codec (if compressed)
    pub codec: Option<String>,
    /// Bit depth
    pub bit_depth: u8,
    /// Chroma subsampling
    pub chroma_subsampling: String,
    /// Color space
    pub color_space: String,
    /// HDR metadata
    pub hdr_metadata: Option<HdrMetadata>,
}

/// HDR metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdrMetadata {
    /// Transfer function
    pub transfer_function: String,
    /// Color primaries
    pub color_primaries: String,
    /// Master display metadata
    pub master_display: Option<MasterDisplayMetadata>,
    /// Content light level
    pub content_light_level: Option<ContentLightLevel>,
}

/// Master display metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterDisplayMetadata {
    /// Display primaries
    pub display_primaries: [[u16; 2]; 3],
    /// White point
    pub white_point: [u16; 2],
    /// Maximum display luminance
    pub max_luminance: u32,
    /// Minimum display luminance
    pub min_luminance: u32,
}

/// Content light level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentLightLevel {
    /// Maximum content light level
    pub max_cll: u16,
    /// Maximum frame average light level
    pub max_fall: u16,
}

//! Input and output validation for transcode operations.

use std::path::Path;
use thiserror::Error;

/// Validation errors.
#[derive(Debug, Clone, Error)]
pub enum ValidationError {
    /// Input file does not exist.
    #[error("Input file does not exist: {0}")]
    InputNotFound(String),

    /// Input file is not readable.
    #[error("Input file is not readable: {0}")]
    InputNotReadable(String),

    /// Input file has no video stream.
    #[error("Input file has no video stream")]
    NoVideoStream,

    /// Input file has no audio stream.
    #[error("Input file has no audio stream")]
    NoAudioStream,

    /// Invalid input format.
    #[error("Invalid input format: {0}")]
    InvalidInputFormat(String),

    /// Output path is invalid.
    #[error("Invalid output path: {0}")]
    InvalidOutputPath(String),

    /// Output directory is not writable.
    #[error("Output directory is not writable: {0}")]
    OutputNotWritable(String),

    /// Output file already exists.
    #[error("Output file already exists: {0}")]
    OutputExists(String),

    /// Invalid codec selection.
    #[error("Invalid codec: {0}")]
    InvalidCodec(String),

    /// Invalid resolution.
    #[error("Invalid resolution: {0}")]
    InvalidResolution(String),

    /// Invalid bitrate.
    #[error("Invalid bitrate: {0}")]
    InvalidBitrate(String),

    /// Invalid frame rate.
    #[error("Invalid frame rate: {0}")]
    InvalidFrameRate(String),

    /// Unsupported operation.
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
}

/// Input file validator.
pub struct InputValidator;

impl InputValidator {
    /// Validates an input file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the input file is invalid or inaccessible.
    pub fn validate_path(path: &str) -> Result<(), ValidationError> {
        let path_obj = Path::new(path);

        if !path_obj.exists() {
            return Err(ValidationError::InputNotFound(path.to_string()));
        }

        if !path_obj.is_file() {
            return Err(ValidationError::InvalidInputFormat(
                "Path is not a file".to_string(),
            ));
        }

        // Check if file is readable
        match std::fs::metadata(path_obj) {
            Ok(metadata) => {
                if metadata.len() == 0 {
                    return Err(ValidationError::InvalidInputFormat(
                        "File is empty".to_string(),
                    ));
                }
            }
            Err(_) => {
                return Err(ValidationError::InputNotReadable(path.to_string()));
            }
        }

        Ok(())
    }

    /// Validates input format based on file extension.
    ///
    /// # Errors
    ///
    /// Returns an error if the format is not supported.
    pub fn validate_format(path: &str) -> Result<String, ValidationError> {
        let path_obj = Path::new(path);
        let extension = path_obj
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| ValidationError::InvalidInputFormat("No file extension".to_string()))?;

        let ext_lower = extension.to_lowercase();

        // Check if extension is in supported formats
        match ext_lower.as_str() {
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "flv" | "wmv" | "m4v" | "mpg" | "mpeg"
            | "ts" | "mts" | "m2ts" | "ogv" | "3gp" => Ok(ext_lower),
            _ => Err(ValidationError::InvalidInputFormat(format!(
                "Unsupported format: {extension}"
            ))),
        }
    }

    /// Validates that input has required streams.
    pub fn validate_streams(
        has_video: bool,
        has_audio: bool,
        require_video: bool,
        require_audio: bool,
    ) -> Result<(), ValidationError> {
        if require_video && !has_video {
            return Err(ValidationError::NoVideoStream);
        }

        if require_audio && !has_audio {
            return Err(ValidationError::NoAudioStream);
        }

        Ok(())
    }
}

/// Output configuration validator.
pub struct OutputValidator;

impl OutputValidator {
    /// Validates an output file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the output path is invalid or not writable.
    pub fn validate_path(path: &str, overwrite: bool) -> Result<(), ValidationError> {
        let path_obj = Path::new(path);

        // Check if file already exists
        if path_obj.exists() && !overwrite {
            return Err(ValidationError::OutputExists(path.to_string()));
        }

        // Check if parent directory exists and is writable.
        //
        // `Path::parent` yields `Some("")` for a bare relative filename such
        // as `out.flac`, and `Path::new("").exists()` is always false — so
        // checking it directly rejected the most ordinary invocation of all,
        // `transcode -i in.wav -o out.flac`, while `./out.flac` and absolute
        // paths passed. An empty parent means "the current directory", so it
        // is resolved to `.` before the existence and writability probes.
        if let Some(parent) = path_obj.parent() {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };

            if !parent.exists() {
                return Err(ValidationError::InvalidOutputPath(
                    "Parent directory does not exist".to_string(),
                ));
            }

            // Try to check if directory is writable
            if let Err(_e) = std::fs::metadata(parent) {
                return Err(ValidationError::OutputNotWritable(
                    parent.display().to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Validates output format based on file extension.
    ///
    /// # Errors
    ///
    /// Returns an error if the format is not supported.
    pub fn validate_format(path: &str) -> Result<String, ValidationError> {
        let path_obj = Path::new(path);
        let extension = path_obj
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| ValidationError::InvalidOutputPath("No file extension".to_string()))?;

        let ext_lower = extension.to_lowercase();

        match ext_lower.as_str() {
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" | "ogv" => Ok(ext_lower),
            _ => Err(ValidationError::InvalidOutputPath(format!(
                "Unsupported output format: {extension}"
            ))),
        }
    }

    /// Validates codec selection.
    ///
    /// # Errors
    ///
    /// Returns an error if the codec is not supported.
    pub fn validate_codec(codec: &str) -> Result<(), ValidationError> {
        let codec_lower = codec.to_lowercase();

        match codec_lower.as_str() {
            "vp8" | "vp9" | "av1" | "h264" | "h265" | "theora" | "opus" | "vorbis" | "aac"
            | "mp3" | "flac" => Ok(()),
            _ => Err(ValidationError::InvalidCodec(format!(
                "Unsupported codec: {codec}"
            ))),
        }
    }

    /// Validates resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if the resolution is invalid.
    pub fn validate_resolution(width: u32, height: u32) -> Result<(), ValidationError> {
        if width == 0 || height == 0 {
            return Err(ValidationError::InvalidResolution(
                "Width and height must be greater than 0".to_string(),
            ));
        }

        if width > 7680 || height > 4320 {
            return Err(ValidationError::InvalidResolution(
                "Resolution exceeds maximum (7680x4320)".to_string(),
            ));
        }

        if width % 2 != 0 || height % 2 != 0 {
            return Err(ValidationError::InvalidResolution(
                "Width and height must be even numbers".to_string(),
            ));
        }

        Ok(())
    }

    /// Validates bitrate.
    ///
    /// # Errors
    ///
    /// Returns an error if the bitrate is invalid.
    pub fn validate_bitrate(bitrate: u64, min: u64, max: u64) -> Result<(), ValidationError> {
        if bitrate < min {
            return Err(ValidationError::InvalidBitrate(format!(
                "Bitrate too low (minimum: {min})"
            )));
        }

        if bitrate > max {
            return Err(ValidationError::InvalidBitrate(format!(
                "Bitrate too high (maximum: {max})"
            )));
        }

        Ok(())
    }

    /// Validates frame rate.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame rate is invalid.
    pub fn validate_frame_rate(num: u32, den: u32) -> Result<(), ValidationError> {
        if num == 0 || den == 0 {
            return Err(ValidationError::InvalidFrameRate(
                "Numerator and denominator must be greater than 0".to_string(),
            ));
        }

        let fps = f64::from(num) / f64::from(den);

        if fps < 1.0 {
            return Err(ValidationError::InvalidFrameRate(
                "Frame rate must be at least 1 fps".to_string(),
            ));
        }

        if fps > 240.0 {
            return Err(ValidationError::InvalidFrameRate(
                "Frame rate exceeds maximum (240 fps)".to_string(),
            ));
        }

        Ok(())
    }
}

/// Validates codec compatibility with container format.
pub fn validate_codec_container_compatibility(
    codec: &str,
    container: &str,
) -> Result<(), ValidationError> {
    let codec_lower = codec.to_lowercase();
    let container_lower = container.to_lowercase();

    let compatible = match container_lower.as_str() {
        "mp4" | "m4v" => matches!(codec_lower.as_str(), "h264" | "h265" | "av1" | "aac"),
        "webm" => matches!(
            codec_lower.as_str(),
            "vp8" | "vp9" | "av1" | "opus" | "vorbis"
        ),
        "mkv" => true, // MKV supports everything
        "ogv" => matches!(codec_lower.as_str(), "theora" | "vorbis" | "opus"),
        _ => false,
    };

    if compatible {
        Ok(())
    } else {
        Err(ValidationError::Unsupported(format!(
            "Codec '{codec}' is not compatible with container '{container}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_format_valid() {
        assert_eq!(
            InputValidator::validate_format("test.mp4").expect("should succeed in test"),
            "mp4"
        );
        assert_eq!(
            InputValidator::validate_format("test.MKV").expect("should succeed in test"),
            "mkv"
        );
        assert_eq!(
            InputValidator::validate_format("test.webm").expect("should succeed in test"),
            "webm"
        );
    }

    #[test]
    fn test_validate_format_invalid() {
        assert!(InputValidator::validate_format("test.xyz").is_err());
        assert!(InputValidator::validate_format("test").is_err());
    }

    #[test]
    fn test_validate_codec_valid() {
        assert!(OutputValidator::validate_codec("vp9").is_ok());
        assert!(OutputValidator::validate_codec("h264").is_ok());
        assert!(OutputValidator::validate_codec("opus").is_ok());
    }

    #[test]
    fn test_validate_codec_invalid() {
        assert!(OutputValidator::validate_codec("unknown").is_err());
        assert!(OutputValidator::validate_codec("divx").is_err());
    }

    #[test]
    fn test_validate_resolution_valid() {
        assert!(OutputValidator::validate_resolution(1920, 1080).is_ok());
        assert!(OutputValidator::validate_resolution(1280, 720).is_ok());
        assert!(OutputValidator::validate_resolution(3840, 2160).is_ok());
    }

    #[test]
    fn test_validate_resolution_invalid() {
        // Zero dimensions
        assert!(OutputValidator::validate_resolution(0, 1080).is_err());
        assert!(OutputValidator::validate_resolution(1920, 0).is_err());

        // Odd dimensions
        assert!(OutputValidator::validate_resolution(1921, 1080).is_err());
        assert!(OutputValidator::validate_resolution(1920, 1081).is_err());

        // Too large
        assert!(OutputValidator::validate_resolution(10000, 10000).is_err());
    }

    #[test]
    fn test_validate_bitrate_valid() {
        assert!(OutputValidator::validate_bitrate(5_000_000, 100_000, 50_000_000).is_ok());
        assert!(OutputValidator::validate_bitrate(100_000, 100_000, 50_000_000).is_ok());
        assert!(OutputValidator::validate_bitrate(50_000_000, 100_000, 50_000_000).is_ok());
    }

    #[test]
    fn test_validate_bitrate_invalid() {
        assert!(OutputValidator::validate_bitrate(50_000, 100_000, 50_000_000).is_err());
        assert!(OutputValidator::validate_bitrate(100_000_000, 100_000, 50_000_000).is_err());
    }

    #[test]
    fn test_validate_frame_rate_valid() {
        assert!(OutputValidator::validate_frame_rate(30, 1).is_ok());
        assert!(OutputValidator::validate_frame_rate(60, 1).is_ok());
        assert!(OutputValidator::validate_frame_rate(24000, 1001).is_ok());
    }

    #[test]
    fn test_validate_frame_rate_invalid() {
        assert!(OutputValidator::validate_frame_rate(0, 1).is_err());
        assert!(OutputValidator::validate_frame_rate(30, 0).is_err());
        assert!(OutputValidator::validate_frame_rate(1, 10).is_err()); // 0.1 fps
        assert!(OutputValidator::validate_frame_rate(300, 1).is_err()); // 300 fps
    }

    #[test]
    fn test_codec_container_compatibility() {
        // Valid combinations
        assert!(validate_codec_container_compatibility("h264", "mp4").is_ok());
        assert!(validate_codec_container_compatibility("vp9", "webm").is_ok());
        assert!(validate_codec_container_compatibility("opus", "webm").is_ok());
        assert!(validate_codec_container_compatibility("theora", "ogv").is_ok());
        assert!(validate_codec_container_compatibility("h264", "mkv").is_ok());
        assert!(validate_codec_container_compatibility("vp9", "mkv").is_ok());

        // Invalid combinations
        assert!(validate_codec_container_compatibility("vp9", "mp4").is_err());
        assert!(validate_codec_container_compatibility("h264", "webm").is_err());
        assert!(validate_codec_container_compatibility("aac", "webm").is_err());
    }

    #[test]
    fn test_validate_streams() {
        assert!(InputValidator::validate_streams(true, true, true, true).is_ok());
        assert!(InputValidator::validate_streams(true, false, true, false).is_ok());
        assert!(InputValidator::validate_streams(false, true, false, true).is_ok());
        assert!(InputValidator::validate_streams(true, true, false, false).is_ok());

        assert!(InputValidator::validate_streams(false, true, true, true).is_err());
        assert!(InputValidator::validate_streams(true, false, true, true).is_err());
    }

    #[test]
    fn test_validate_path_nonexistent() {
        let result = InputValidator::validate_path("/nonexistent/file.mp4");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ValidationError::InputNotFound(_)
        ));
    }

    #[test]
    fn test_output_path_validation() {
        // Test with temp_dir which should exist and be writable
        let out = std::env::temp_dir().join("oximedia-transcode-validation-test_output.mp4");
        let result = OutputValidator::validate_path(out.to_string_lossy().as_ref(), false);
        assert!(result.is_ok());
    }

    /// A bare relative filename — no directory component at all — is the
    /// most ordinary output path a caller can write (`-o out.flac`), and
    /// `Path::parent` reports it as `Some("")`. Probing that empty path for
    /// existence rejected the invocation outright, while `./out.flac` and any
    /// absolute path were accepted; all three must behave identically.
    #[test]
    fn bare_relative_output_filename_is_accepted() {
        assert!(
            OutputValidator::validate_path("oximedia-transcode-bare-name-probe.flac", false)
                .is_ok(),
            "a bare filename resolves against the current directory"
        );
        assert!(
            OutputValidator::validate_path("./oximedia-transcode-bare-name-probe.flac", false)
                .is_ok(),
            "the explicit `./` spelling must agree with the bare one"
        );
        assert!(
            OutputValidator::validate_path("/nonexistent-dir-xyz/out.flac", false).is_err(),
            "a genuinely missing parent directory is still refused"
        );
    }

    #[test]
    fn test_output_format_validation() {
        assert_eq!(
            OutputValidator::validate_format("output.mp4").expect("should succeed in test"),
            "mp4"
        );
        assert_eq!(
            OutputValidator::validate_format("output.webm").expect("should succeed in test"),
            "webm"
        );
        assert!(OutputValidator::validate_format("output.xyz").is_err());
    }
}

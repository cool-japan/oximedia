//! Image sequence handling and batch operations.

#![allow(clippy::cast_precision_loss)]
#![allow(clippy::unused_self)]

use crate::error::{ImageError, ImageResult};
use crate::parallel::try_map_slice;
use crate::pattern::SequencePattern;
use crate::ImageFrame;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// Image sequence with frame range and pattern.
#[derive(Clone, Debug)]
pub struct ImageSequence {
    /// Sequence pattern for generating filenames.
    pub pattern: SequencePattern,

    /// Frame range (inclusive).
    pub range: RangeInclusive<u32>,

    /// Detected gaps in the sequence (missing frames).
    pub gaps: Vec<u32>,

    /// Frame rate (if known).
    pub frame_rate: Option<f64>,
}

impl ImageSequence {
    /// Creates a new image sequence from a pattern and range.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_image::{ImageSequence, SequencePattern};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pattern = SequencePattern::parse("render.%04d.dpx")?;
    /// let sequence = ImageSequence::from_pattern(pattern, 1..=100)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the frame range is empty.
    pub fn from_pattern(pattern: SequencePattern, range: RangeInclusive<u32>) -> ImageResult<Self> {
        if range.is_empty() {
            return Err(ImageError::InvalidRange("Empty frame range".to_string()));
        }

        Ok(Self {
            pattern,
            range,
            gaps: Vec::new(),
            frame_rate: None,
        })
    }

    /// Detects an image sequence from a directory.
    ///
    /// Scans the directory for matching files and determines the frame range.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be read or no frames are found.
    ///
    /// # Panics
    ///
    /// Panics if the frames vector is empty after sorting (internal consistency check).
    pub fn detect(directory: &Path, pattern: SequencePattern) -> ImageResult<Self> {
        let mut frames = Vec::new();

        // Scan directory for matching files
        let entries = std::fs::read_dir(directory).map_err(ImageError::Io)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if let Some(frame) = pattern.extract_frame(&path) {
                frames.push(frame);
            }
        }

        if frames.is_empty() {
            return Err(ImageError::InvalidRange("No frames found".to_string()));
        }

        frames.sort_unstable();

        // frames is non-empty: the is_empty() check above would have returned Err.
        let min_frame = frames[0];
        let max_frame = frames[frames.len() - 1];

        // Detect gaps
        let mut gaps = Vec::new();
        for frame in min_frame..=max_frame {
            if !frames.contains(&frame) {
                gaps.push(frame);
            }
        }

        Ok(Self {
            pattern,
            range: min_frame..=max_frame,
            gaps,
            frame_rate: None,
        })
    }

    /// Returns the number of frames in this sequence (excluding gaps).
    #[must_use]
    pub fn frame_count(&self) -> usize {
        let total = (self.range.end() - self.range.start() + 1) as usize;
        total - self.gaps.len()
    }

    /// Returns true if the sequence has gaps (missing frames).
    #[must_use]
    pub fn has_gaps(&self) -> bool {
        !self.gaps.is_empty()
    }

    /// Returns the path for a specific frame.
    #[must_use]
    pub fn frame_path(&self, frame: u32) -> PathBuf {
        self.pattern.format(frame)
    }

    /// Checks if a frame exists in this sequence.
    #[must_use]
    pub fn has_frame(&self, frame: u32) -> bool {
        self.range.contains(&frame) && !self.gaps.contains(&frame)
    }

    /// Reads a single frame from the sequence.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame is not in the sequence or cannot be read.
    pub fn read_frame(&self, frame: u32) -> ImageResult<ImageFrame> {
        if !self.has_frame(frame) {
            return Err(ImageError::FrameNotFound(frame));
        }

        let path = self.frame_path(frame);
        self.read_frame_from_path(&path, frame)
    }

    /// Reads multiple frames in parallel.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use oximedia_image::{ImageSequence, SequencePattern};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pattern = SequencePattern::parse("render.%04d.dpx")?;
    /// let sequence = ImageSequence::from_pattern(pattern, 1..=100)?;
    ///
    /// // Read frames 1-10 in parallel
    /// let frames = sequence.read_frames_parallel(1..=10)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if any frame in the range cannot be read.
    pub fn read_frames_parallel(&self, range: RangeInclusive<u32>) -> ImageResult<Vec<ImageFrame>> {
        let present: Vec<u32> = range.filter(|f| self.has_frame(*f)).collect();
        try_map_slice(&present, |f| self.read_frame(*f))
    }

    /// Reads all frames in the sequence in parallel.
    ///
    /// # Errors
    ///
    /// Returns an error if any frame cannot be read.
    pub fn read_all_parallel(&self) -> ImageResult<Vec<ImageFrame>> {
        self.read_frames_parallel(self.range.clone())
    }

    /// Sets the frame rate for this sequence.
    pub fn set_frame_rate(&mut self, fps: f64) {
        self.frame_rate = Some(fps);
    }

    /// Returns the duration of the sequence in seconds (if frame rate is known).
    #[must_use]
    pub fn duration(&self) -> Option<f64> {
        self.frame_rate.map(|fps| self.frame_count() as f64 / fps)
    }

    fn read_frame_from_path(&self, path: &Path, frame_number: u32) -> ImageResult<ImageFrame> {
        // Determine format from extension
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| ImageError::invalid_format("No file extension"))?;

        match extension.to_lowercase().as_str() {
            "dpx" => crate::dpx::read_dpx(path, frame_number),
            "exr" => crate::exr::read_exr(path, frame_number),
            "tif" | "tiff" => crate::tiff::read_tiff(path, frame_number),
            _ => Err(ImageError::unsupported(format!(
                "Unsupported format: {extension}"
            ))),
        }
    }
}

/// Iterator over frames in a sequence.
pub struct SequenceIterator {
    sequence: ImageSequence,
    current: u32,
}

impl Iterator for SequenceIterator {
    type Item = ImageResult<ImageFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.current <= *self.sequence.range.end() {
            let frame = self.current;
            self.current += 1;

            if self.sequence.has_frame(frame) {
                return Some(self.sequence.read_frame(frame));
            }
        }

        None
    }
}

impl IntoIterator for ImageSequence {
    type Item = ImageResult<ImageFrame>;
    type IntoIter = SequenceIterator;

    fn into_iter(self) -> Self::IntoIter {
        let current = *self.range.start();
        SequenceIterator {
            sequence: self,
            current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequence_creation() {
        let pattern = SequencePattern::parse("test.%04d.dpx").expect("should succeed in test");
        let sequence =
            ImageSequence::from_pattern(pattern, 1..=100).expect("should succeed in test");

        assert_eq!(sequence.frame_count(), 100);
        assert!(!sequence.has_gaps());
        assert!(sequence.has_frame(50));
        assert!(!sequence.has_frame(101));
    }

    #[test]
    fn test_sequence_with_gaps() {
        let pattern = SequencePattern::parse("test.%04d.dpx").expect("should succeed in test");
        let mut sequence =
            ImageSequence::from_pattern(pattern, 1..=10).expect("should succeed in test");
        sequence.gaps = vec![3, 5, 7];

        assert_eq!(sequence.frame_count(), 7);
        assert!(sequence.has_gaps());
        assert!(sequence.has_frame(2));
        assert!(!sequence.has_frame(3));
    }

    #[test]
    fn test_frame_path() {
        let pattern = SequencePattern::parse("render.%04d.exr").expect("should succeed in test");
        let sequence =
            ImageSequence::from_pattern(pattern, 1..=100).expect("should succeed in test");

        let path = sequence.frame_path(42);
        let filename = path
            .file_name()
            .expect("should succeed in test")
            .to_str()
            .expect("should succeed in test");
        assert!(filename.contains("0042"));
    }

    #[test]
    fn test_duration_calculation() {
        let pattern = SequencePattern::parse("test.%04d.dpx").expect("should succeed in test");
        let mut sequence =
            ImageSequence::from_pattern(pattern, 1..=240).expect("should succeed in test");

        assert_eq!(sequence.duration(), None);

        sequence.set_frame_rate(24.0);
        assert_eq!(sequence.duration(), Some(10.0));
    }
}

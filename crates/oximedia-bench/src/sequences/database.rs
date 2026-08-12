//! In-memory database of test sequences with lookup and statistics.
//!
//! Split out of [`super`] to keep that module under the 2000-line limit.

use super::{ContentType, MotionCharacteristics, TestSequence};
use crate::BenchResult;

/// Sequence database for managing multiple test sequences.
#[derive(Debug, Clone)]
pub struct SequenceDatabase {
    sequences: Vec<TestSequence>,
    index_by_name: std::collections::HashMap<String, usize>,
    index_by_resolution: std::collections::HashMap<String, Vec<usize>>,
}

impl SequenceDatabase {
    /// Create a new sequence database.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequences: Vec::new(),
            index_by_name: std::collections::HashMap::new(),
            index_by_resolution: std::collections::HashMap::new(),
        }
    }

    /// Add a sequence to the database.
    pub fn add(&mut self, sequence: TestSequence) {
        let idx = self.sequences.len();
        let name = sequence.name.clone();
        let resolution = sequence.resolution_string();

        self.sequences.push(sequence);
        self.index_by_name.insert(name, idx);
        self.index_by_resolution
            .entry(resolution)
            .or_insert_with(Vec::new)
            .push(idx);
    }

    /// Get a sequence by name.
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&TestSequence> {
        self.index_by_name
            .get(name)
            .and_then(|&idx| self.sequences.get(idx))
    }

    /// Get sequences by resolution.
    #[must_use]
    pub fn get_by_resolution(&self, width: usize, height: usize) -> Vec<&TestSequence> {
        let resolution = format!("{width}x{height}");
        self.index_by_resolution
            .get(&resolution)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|&idx| self.sequences.get(idx))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all sequences.
    #[must_use]
    pub fn all(&self) -> &[TestSequence] {
        &self.sequences
    }

    /// Get sequences by content type.
    #[must_use]
    pub fn get_by_content_type(&self, content_type: ContentType) -> Vec<&TestSequence> {
        self.sequences
            .iter()
            .filter(|s| s.content_type == content_type)
            .collect()
    }

    /// Get sequences by motion characteristics.
    #[must_use]
    pub fn get_by_motion(&self, motion: MotionCharacteristics) -> Vec<&TestSequence> {
        self.sequences
            .iter()
            .filter(|s| s.motion == motion)
            .collect()
    }

    /// Load sequences from a directory.
    ///
    /// # Errors
    ///
    /// Returns an error if loading fails.
    pub fn load_from_directory(
        &mut self,
        _path: impl AsRef<std::path::Path>,
    ) -> BenchResult<usize> {
        // Placeholder for loading sequences from a directory
        Ok(0)
    }

    /// Export database to JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if export fails.
    pub fn export_to_json(&self, path: impl AsRef<std::path::Path>) -> BenchResult<()> {
        let json = serde_json::to_string_pretty(&self.sequences)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Import database from JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if import fails.
    pub fn import_from_json(&mut self, path: impl AsRef<std::path::Path>) -> BenchResult<()> {
        let json = std::fs::read_to_string(path)?;
        let sequences: Vec<TestSequence> = serde_json::from_str(&json)?;

        for sequence in sequences {
            self.add(sequence);
        }

        Ok(())
    }

    /// Get database statistics.
    #[must_use]
    pub fn statistics(&self) -> DatabaseStatistics {
        let mut stats = DatabaseStatistics {
            total_sequences: self.sequences.len(),
            total_frames: 0,
            total_duration_seconds: 0.0,
            resolutions: std::collections::HashMap::new(),
            content_types: std::collections::HashMap::new(),
        };

        for seq in &self.sequences {
            stats.total_frames += seq.frame_count;
            stats.total_duration_seconds += seq.frame_count as f64 / seq.frame_rate.to_f64();

            *stats
                .resolutions
                .entry(seq.resolution_string())
                .or_insert(0) += 1;
            *stats.content_types.entry(seq.content_type).or_insert(0) += 1;
        }

        stats
    }
}

impl Default for SequenceDatabase {
    fn default() -> Self {
        Self::new()
    }
}

/// Database statistics.
#[derive(Debug, Clone)]
pub struct DatabaseStatistics {
    /// Total number of sequences
    pub total_sequences: usize,
    /// Total number of frames
    pub total_frames: usize,
    /// Total duration in seconds
    pub total_duration_seconds: f64,
    /// Resolution distribution
    pub resolutions: std::collections::HashMap<String, usize>,
    /// Content type distribution
    pub content_types: std::collections::HashMap<ContentType, usize>,
}

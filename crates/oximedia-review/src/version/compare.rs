//! Version comparison.

use crate::{
    error::ReviewResult,
    version::{diff::VersionDiff, Version},
    VersionId,
};
use serde::{Deserialize, Serialize};

/// Comparison result between two versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionComparison {
    /// Version A (older).
    pub version_a: VersionId,
    /// Version B (newer).
    pub version_b: VersionId,
    /// List of differences.
    pub differences: Vec<VersionDiff>,
    /// Similarity score (0.0-1.0).
    pub similarity: f64,
    /// Total frames changed.
    pub frames_changed: usize,
    /// Metadata changes.
    pub metadata_changes: Vec<MetadataChange>,
}

/// Metadata change between versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataChange {
    /// Field name.
    pub field: String,
    /// Old value.
    pub old_value: String,
    /// New value.
    pub new_value: String,
}

/// Compare two versions.
///
/// # Errors
///
/// Returns error if comparison fails.
pub async fn compare_versions(
    version_a: &Version,
    version_b: &Version,
) -> ReviewResult<VersionComparison> {
    let mut metadata_changes = Vec::new();

    // Compare resolution
    if version_a.resolution != version_b.resolution {
        metadata_changes.push(MetadataChange {
            field: "resolution".to_string(),
            old_value: version_a.resolution_string(),
            new_value: version_b.resolution_string(),
        });
    }

    // Compare duration
    if version_a.duration_frames != version_b.duration_frames {
        metadata_changes.push(MetadataChange {
            field: "duration_frames".to_string(),
            old_value: version_a.duration_frames.to_string(),
            new_value: version_b.duration_frames.to_string(),
        });
    }

    // Compare frame rate
    if (version_a.frame_rate - version_b.frame_rate).abs() > 0.001 {
        metadata_changes.push(MetadataChange {
            field: "frame_rate".to_string(),
            old_value: version_a.frame_rate.to_string(),
            new_value: version_b.frame_rate.to_string(),
        });
    }

    // Calculate similarity (simplified)
    let similarity =
        if version_a.content_hash == version_b.content_hash && metadata_changes.is_empty() {
            1.0 // Same content and metadata
        } else if version_a.content_hash == version_b.content_hash {
            0.95 // Same content, different metadata
        } else if metadata_changes.is_empty() {
            0.90 // Different content, same metadata
        } else {
            0.80 // Different content and metadata
        };

    Ok(VersionComparison {
        version_a: version_a.id,
        version_b: version_b.id,
        differences: Vec::new(),
        similarity,
        frames_changed: 0,
        metadata_changes,
    })
}

/// Compare multiple versions.
///
/// # Errors
///
/// Returns error if comparison fails.
pub async fn compare_multiple(versions: &[Version]) -> ReviewResult<Vec<VersionComparison>> {
    let mut comparisons = Vec::new();

    for i in 0..versions.len().saturating_sub(1) {
        let comparison = compare_versions(&versions[i], &versions[i + 1]).await?;
        comparisons.push(comparison);
    }

    Ok(comparisons)
}

/// Find differences in frame range.
///
/// This crate has no decoder dependency (it is the review/collaboration
/// layer, not a codec), so it cannot decode `version_a`/`version_b`'s content
/// to compare frame pixels. A real implementation needs a decoder-backed
/// pipeline upstream (e.g. `oximedia-codec`) to produce per-frame hashes or
/// diffs, which could then be persisted and listed from here; until such a
/// pipeline is wired in, returning an empty list would misrepresent "no
/// differences found" as a real result, so this honestly reports the missing
/// capability instead.
///
/// # Errors
///
/// Always returns [`crate::error::ReviewError::Other`]: frame-level
/// differencing requires decoded video frames, which are not available to
/// this crate.
pub async fn find_frame_differences(
    _version_a: &Version,
    _version_b: &Version,
    _start_frame: i64,
    _end_frame: i64,
) -> ReviewResult<Vec<i64>> {
    Err(crate::error::ReviewError::Other(
        "find_frame_differences requires decoded video frames; oximedia-review has no \
         codec dependency to decode version content, so pixel-level frame comparison is \
         not available (metadata-level comparison is available via compare_versions)"
            .to_string(),
    ))
}

/// Calculate a perceptual difference score in `[0.0, 1.0]`.
///
/// `0.0` means the two versions are perceptually identical, `1.0` means they
/// are maximally different given the metadata available to this crate.
///
/// A full implementation would decode frames and run SSIM/VMAF; this version
/// uses the same metadata-derived heuristic as [`compare_versions`] (content
/// hash + resolution / duration / frame-rate match) so it is consistent with
/// the similarity score reported there: `difference = 1.0 - similarity`.
///
/// # Errors
///
/// Returns error if calculation fails. The metadata-based implementation
/// never errors; the `Result` is preserved for API stability with future
/// decoder-backed implementations.
pub async fn calculate_perceptual_difference(
    version_a: &Version,
    version_b: &Version,
) -> ReviewResult<f64> {
    let comparison = compare_versions(version_a, version_b).await?;
    // Clamp defensively – `compare_versions` produces values in [0.80, 1.0]
    // today, but callers should always see a value in [0.0, 1.0].
    let diff = (1.0 - comparison.similarity).clamp(0.0, 1.0);
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionId;
    use chrono::Utc;

    fn create_test_version(number: u32) -> Version {
        Version {
            id: VersionId::new(),
            session_id: SessionId::new(),
            number,
            label: format!("Version {}", number),
            description: None,
            content_url: String::new(),
            content_hash: "hash123".to_string(),
            file_size: 1000,
            duration_frames: 240,
            frame_rate: 24.0,
            resolution: (1920, 1080),
            created_by: "test".to_string(),
            created_at: Utc::now(),
            parent_id: None,
        }
    }

    #[tokio::test]
    async fn test_compare_versions_identical() {
        let version1 = create_test_version(1);
        let version2 = create_test_version(2);

        let comparison = compare_versions(&version1, &version2)
            .await
            .expect("should succeed in test");
        assert!((comparison.similarity - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_compare_versions_different_resolution() {
        let version1 = create_test_version(1);
        let mut version2 = create_test_version(2);
        version2.resolution = (3840, 2160);

        let comparison = compare_versions(&version1, &version2)
            .await
            .expect("should succeed in test");
        assert!(!comparison.metadata_changes.is_empty());
        assert!(comparison.similarity < 1.0);
    }

    #[tokio::test]
    async fn test_compare_multiple() {
        let versions = vec![
            create_test_version(1),
            create_test_version(2),
            create_test_version(3),
        ];

        let comparisons = compare_multiple(&versions)
            .await
            .expect("should succeed in test");
        assert_eq!(comparisons.len(), 2);
    }

    /// `compare_versions` reflecting real, persisted data end-to-end: create
    /// two real versions (different resolution) through the same free
    /// functions the rest of the crate uses, load them back from the store,
    /// and confirm the comparison reflects what was actually inserted.
    #[tokio::test]
    async fn test_compare_versions_reflects_real_inserted_data() {
        let session_id = SessionId::new();
        let v1 = crate::version::create_version(session_id, "v1".to_string(), "url1".to_string())
            .await
            .expect("v1 should succeed");
        let v2 = crate::version::create_version(session_id, "v2".to_string(), "url2".to_string())
            .await
            .expect("v2 should succeed");
        // The stub-created versions share identical metadata, so the
        // comparison should report full similarity until we mutate one.
        let loaded_v1 = crate::version::get_version(v1.id).await.expect("get v1");
        let loaded_v2 = crate::version::get_version(v2.id).await.expect("get v2");
        let identical = compare_versions(&loaded_v1, &loaded_v2)
            .await
            .expect("compare should succeed");
        assert!(identical.metadata_changes.is_empty());
        assert!((identical.similarity - 1.0).abs() < 0.001);

        let mut mutated_v2 = loaded_v2;
        mutated_v2.resolution = (3840, 2160);
        mutated_v2.duration_frames = 480;
        let diverged = compare_versions(&loaded_v1, &mutated_v2)
            .await
            .expect("compare should succeed");
        assert_eq!(diverged.metadata_changes.len(), 2);
        assert!(diverged
            .metadata_changes
            .iter()
            .any(|c| c.field == "resolution" && c.new_value == "3840x2160"));
        assert!(diverged
            .metadata_changes
            .iter()
            .any(|c| c.field == "duration_frames" && c.new_value == "480"));
        assert!(diverged.similarity < 1.0);
    }

    #[tokio::test]
    async fn test_find_frame_differences_is_honest_about_missing_capability() {
        let version1 = create_test_version(1);
        let version2 = create_test_version(2);
        let result = find_frame_differences(&version1, &version2, 0, 10).await;
        assert!(result.is_err(), "must not fabricate an empty diff result");
    }
}

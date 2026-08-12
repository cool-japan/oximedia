//! Version management and comparison.

use crate::{error::ReviewResult, SessionId, VersionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod compare;
pub mod diff;
pub mod timeline;

pub use compare::compare_versions;
pub use diff::{DiffType, VersionDiff};
pub use timeline::VersionTimeline;

/// Content version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    /// Version ID.
    pub id: VersionId,
    /// Session ID.
    pub session_id: SessionId,
    /// Version number (sequential).
    pub number: u32,
    /// Version label.
    pub label: String,
    /// Description of changes.
    pub description: Option<String>,
    /// Content URL or path.
    pub content_url: String,
    /// Content hash (for integrity).
    pub content_hash: String,
    /// File size in bytes.
    pub file_size: u64,
    /// Duration in frames.
    pub duration_frames: i64,
    /// Frame rate.
    pub frame_rate: f64,
    /// Resolution (width, height).
    pub resolution: (u32, u32),
    /// Creator user ID.
    pub created_by: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Parent version ID (if any).
    pub parent_id: Option<VersionId>,
}

impl Version {
    /// Check if this is the initial version.
    #[must_use]
    pub fn is_initial(&self) -> bool {
        self.parent_id.is_none() && self.number == 1
    }

    /// Get duration in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        self.duration_frames as f64 / self.frame_rate
    }

    /// Get formatted resolution string.
    #[must_use]
    pub fn resolution_string(&self) -> String {
        format!("{}x{}", self.resolution.0, self.resolution.1)
    }
}

/// Create a new version.
///
/// `number` is assigned as one past however many versions already exist for
/// `session_id`, and `parent_id` is set to the latest existing version (by
/// number) for the session, if any -- so repeated calls for the same session
/// build a linear version chain rather than every version claiming to be
/// version 1 with no parent. A [`timeline::EventType::Created`] event is
/// recorded for the new version (see [`timeline::get_timeline_events`]).
///
/// # Errors
///
/// Returns error if version creation fails.
pub async fn create_version(
    session_id: SessionId,
    label: String,
    content_url: String,
) -> ReviewResult<Version> {
    let store = crate::store::default_store().await?;
    let existing = store.list_versions_by_session(session_id).await?;
    let number = u32::try_from(existing.len())
        .map_err(|e| crate::error::ReviewError::Other(format!("too many versions: {e}")))?
        + 1;
    let parent_id = existing.last().map(|v| v.id);
    let created_by = "system".to_string();
    let created_at = Utc::now();

    let version = Version {
        id: VersionId::new(),
        session_id,
        number,
        label,
        description: None,
        content_url,
        content_hash: String::new(),
        file_size: 0,
        duration_frames: 0,
        frame_rate: 24.0,
        resolution: (1920, 1080),
        created_by: created_by.clone(),
        created_at,
        parent_id,
    };

    store.insert_version(&version).await?;

    let event = timeline::TimelineEvent {
        id: uuid::Uuid::new_v4().to_string(),
        version_id: version.id,
        event_type: timeline::EventType::Created,
        description: format!("Version {} created", version.number),
        user: created_by,
        timestamp: created_at,
    };
    store.insert_timeline_event(&event, session_id).await?;

    Ok(version)
}

/// Get version by ID.
///
/// # Errors
///
/// Returns error if version not found.
pub async fn get_version(version_id: VersionId) -> ReviewResult<Version> {
    let store = crate::store::default_store().await?;
    store.get_version(version_id).await
}

/// List all versions for a session, ordered by version number ascending.
///
/// # Errors
///
/// Returns error if listing fails.
pub async fn list_versions(session_id: SessionId) -> ReviewResult<Vec<Version>> {
    let store = crate::store::default_store().await?;
    store.list_versions_by_session(session_id).await
}

/// Delete a version.
///
/// # Errors
///
/// Returns error if deletion fails.
pub async fn delete_version(version_id: VersionId) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.delete_version(version_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_version() {
        let session_id = SessionId::new();
        let result = create_version(
            session_id,
            "Version 1".to_string(),
            "http://example.com/video.mp4".to_string(),
        )
        .await;

        assert!(result.is_ok());
        let version = result.expect("should succeed in test");
        assert_eq!(version.number, 1);
        assert!(version.is_initial());
    }

    #[tokio::test]
    async fn test_create_version_chains_number_and_parent() {
        // Uses a fresh, never-before-seen session ID, so this is
        // deterministic even when other tests share the process-wide
        // default store.
        let session_id = SessionId::new();

        let v1 = create_version(session_id, "v1".to_string(), "url1".to_string())
            .await
            .expect("v1 should succeed");
        assert_eq!(v1.number, 1);
        assert_eq!(v1.parent_id, None);

        let v2 = create_version(session_id, "v2".to_string(), "url2".to_string())
            .await
            .expect("v2 should succeed");
        assert_eq!(v2.number, 2);
        assert_eq!(v2.parent_id, Some(v1.id));

        let v3 = create_version(session_id, "v3".to_string(), "url3".to_string())
            .await
            .expect("v3 should succeed");
        assert_eq!(v3.number, 3);
        assert_eq!(v3.parent_id, Some(v2.id));

        let listed = list_versions(session_id)
            .await
            .expect("list should succeed");
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, v1.id);
        assert_eq!(listed[1].id, v2.id);
        assert_eq!(listed[2].id, v3.id);
    }

    #[tokio::test]
    async fn test_get_and_delete_version() {
        let session_id = SessionId::new();
        let version = create_version(session_id, "v1".to_string(), "url".to_string())
            .await
            .expect("create should succeed");

        let loaded = get_version(version.id).await.expect("get should succeed");
        assert_eq!(loaded.id, version.id);
        assert_eq!(loaded.label, "v1");

        delete_version(version.id)
            .await
            .expect("delete should succeed");
        let result = get_version(version.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_version_not_found() {
        let result = get_version(VersionId::new()).await;
        assert!(matches!(
            result,
            Err(crate::error::ReviewError::VersionNotFound(_))
        ));
    }

    #[tokio::test]
    async fn test_list_versions_empty_for_fresh_session() {
        let versions = list_versions(SessionId::new())
            .await
            .expect("list should succeed");
        assert!(versions.is_empty());
    }

    #[test]
    fn test_version_duration_seconds() {
        let version = Version {
            id: VersionId::new(),
            session_id: SessionId::new(),
            number: 1,
            label: "Test".to_string(),
            description: None,
            content_url: String::new(),
            content_hash: String::new(),
            file_size: 0,
            duration_frames: 240,
            frame_rate: 24.0,
            resolution: (1920, 1080),
            created_by: "test".to_string(),
            created_at: Utc::now(),
            parent_id: None,
        };

        assert!((version.duration_seconds() - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_version_resolution_string() {
        let version = Version {
            id: VersionId::new(),
            session_id: SessionId::new(),
            number: 1,
            label: "Test".to_string(),
            description: None,
            content_url: String::new(),
            content_hash: String::new(),
            file_size: 0,
            duration_frames: 0,
            frame_rate: 24.0,
            resolution: (3840, 2160),
            created_by: "test".to_string(),
            created_at: Utc::now(),
            parent_id: None,
        };

        assert_eq!(version.resolution_string(), "3840x2160");
    }
}

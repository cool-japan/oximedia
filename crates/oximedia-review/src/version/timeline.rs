//! Version timeline and history.

use crate::{error::ReviewResult, version::Version, SessionId, VersionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Version timeline showing history of changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionTimeline {
    /// Session ID.
    pub session_id: SessionId,
    /// All versions in chronological order.
    pub versions: Vec<Version>,
    /// Version tree (parent -> children).
    pub tree: HashMap<VersionId, Vec<VersionId>>,
    /// Current active version.
    pub current_version: Option<VersionId>,
}

impl VersionTimeline {
    /// Create a new timeline.
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            versions: Vec::new(),
            tree: HashMap::new(),
            current_version: None,
        }
    }

    /// Add a version to the timeline.
    pub fn add_version(&mut self, version: Version) {
        let version_id = version.id;

        // Add to tree
        if let Some(parent_id) = version.parent_id {
            self.tree.entry(parent_id).or_default().push(version_id);
        }

        self.versions.push(version);
        self.current_version = Some(version_id);
    }

    /// Get the root version (first version).
    #[must_use]
    pub fn root_version(&self) -> Option<&Version> {
        self.versions.first()
    }

    /// Get the latest version.
    #[must_use]
    pub fn latest_version(&self) -> Option<&Version> {
        self.versions.last()
    }

    /// Get version by ID.
    #[must_use]
    pub fn get_version(&self, id: VersionId) -> Option<&Version> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// Get children of a version.
    #[must_use]
    pub fn get_children(&self, id: VersionId) -> Vec<&Version> {
        self.tree
            .get(&id)
            .map(|children| {
                children
                    .iter()
                    .filter_map(|child_id| self.get_version(*child_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the full path from root to a version.
    #[must_use]
    pub fn get_version_path(&self, id: VersionId) -> Vec<&Version> {
        let mut path = Vec::new();
        let mut current_id = Some(id);

        while let Some(vid) = current_id {
            if let Some(version) = self.get_version(vid) {
                path.insert(0, version);
                current_id = version.parent_id;
            } else {
                break;
            }
        }

        path
    }

    /// Count total versions.
    #[must_use]
    pub fn version_count(&self) -> usize {
        self.versions.len()
    }

    /// Check if timeline has branches.
    #[must_use]
    pub fn has_branches(&self) -> bool {
        self.tree.values().any(|children| children.len() > 1)
    }
}

/// Timeline event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    /// Event ID.
    pub id: String,
    /// Version ID.
    pub version_id: VersionId,
    /// Event type.
    pub event_type: EventType,
    /// Event description.
    pub description: String,
    /// User who triggered the event.
    pub user: String,
    /// Timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Type of timeline event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    /// Version created.
    Created,
    /// Version uploaded.
    Uploaded,
    /// Version reviewed.
    Reviewed,
    /// Version approved.
    Approved,
    /// Version rejected.
    Rejected,
    /// Version archived.
    Archived,
}

/// Build a timeline for a session.
///
/// Loads every version for the session (already stored in chronological /
/// version-number order, see [`crate::version::list_versions`]) and folds
/// them into a [`VersionTimeline`], which derives the parent/child tree from
/// each version's `parent_id`.
///
/// # Errors
///
/// Returns error if building fails.
pub async fn build_timeline(session_id: SessionId) -> ReviewResult<VersionTimeline> {
    let store = crate::store::default_store().await?;
    let versions = store.list_versions_by_session(session_id).await?;

    let mut timeline = VersionTimeline::new(session_id);
    for version in versions {
        timeline.add_version(version);
    }
    Ok(timeline)
}

/// Get timeline events.
///
/// Events are recorded automatically as versions are created (see
/// [`crate::version::create_version`]); this returns everything recorded for
/// `session_id`, oldest first.
///
/// # Errors
///
/// Returns error if fetching fails.
pub async fn get_timeline_events(session_id: SessionId) -> ReviewResult<Vec<TimelineEvent>> {
    let store = crate::store::default_store().await?;
    store.list_timeline_events_by_session(session_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_version(number: u32, parent_id: Option<VersionId>) -> Version {
        Version {
            id: VersionId::new(),
            session_id: SessionId::new(),
            number,
            label: format!("Version {}", number),
            description: None,
            content_url: String::new(),
            content_hash: String::new(),
            file_size: 0,
            duration_frames: 240,
            frame_rate: 24.0,
            resolution: (1920, 1080),
            created_by: "test".to_string(),
            created_at: Utc::now(),
            parent_id,
        }
    }

    #[test]
    fn test_timeline_creation() {
        let session_id = SessionId::new();
        let timeline = VersionTimeline::new(session_id);
        assert_eq!(timeline.version_count(), 0);
        assert!(timeline.current_version.is_none());
    }

    #[test]
    fn test_timeline_add_version() {
        let session_id = SessionId::new();
        let mut timeline = VersionTimeline::new(session_id);

        let version1 = create_test_version(1, None);
        let version1_id = version1.id;

        timeline.add_version(version1);

        assert_eq!(timeline.version_count(), 1);
        assert_eq!(timeline.current_version, Some(version1_id));
    }

    #[test]
    fn test_timeline_root_and_latest() {
        let session_id = SessionId::new();
        let mut timeline = VersionTimeline::new(session_id);

        let version1 = create_test_version(1, None);
        let version2 = create_test_version(2, Some(version1.id));

        timeline.add_version(version1);
        timeline.add_version(version2);

        assert_eq!(
            timeline
                .root_version()
                .expect("should succeed in test")
                .number,
            1
        );
        assert_eq!(
            timeline
                .latest_version()
                .expect("should succeed in test")
                .number,
            2
        );
    }

    #[test]
    fn test_timeline_version_path() {
        let session_id = SessionId::new();
        let mut timeline = VersionTimeline::new(session_id);

        let version1 = create_test_version(1, None);
        let version2 = create_test_version(2, Some(version1.id));
        let version3 = create_test_version(3, Some(version2.id));

        let version3_id = version3.id;

        timeline.add_version(version1);
        timeline.add_version(version2);
        timeline.add_version(version3);

        let path = timeline.get_version_path(version3_id);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].number, 1);
        assert_eq!(path[1].number, 2);
        assert_eq!(path[2].number, 3);
    }

    #[test]
    fn test_event_type_equality() {
        assert_eq!(EventType::Created, EventType::Created);
        assert_ne!(EventType::Created, EventType::Approved);
    }

    #[tokio::test]
    async fn test_build_timeline_reflects_real_versions() {
        // Fresh session ID: deterministic regardless of what other tests
        // have written to the shared process-wide default store.
        let session_id = SessionId::new();
        assert!(build_timeline(session_id)
            .await
            .expect("empty timeline should succeed")
            .versions
            .is_empty());

        let v1 = crate::version::create_version(session_id, "v1".to_string(), "url1".to_string())
            .await
            .expect("v1 should succeed");
        let v2 = crate::version::create_version(session_id, "v2".to_string(), "url2".to_string())
            .await
            .expect("v2 should succeed");

        let timeline = build_timeline(session_id).await.expect("should succeed");
        assert_eq!(timeline.version_count(), 2);
        assert_eq!(timeline.root_version().expect("root present").id, v1.id);
        assert_eq!(timeline.latest_version().expect("latest present").id, v2.id);
        let children = timeline.get_children(v1.id);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, v2.id);
    }

    #[tokio::test]
    async fn test_get_timeline_events_reflects_created_events() {
        let session_id = SessionId::new();
        assert!(get_timeline_events(session_id)
            .await
            .expect("empty events should succeed")
            .is_empty());

        let v1 = crate::version::create_version(session_id, "v1".to_string(), "url1".to_string())
            .await
            .expect("v1 should succeed");
        let v2 = crate::version::create_version(session_id, "v2".to_string(), "url2".to_string())
            .await
            .expect("v2 should succeed");

        let events = get_timeline_events(session_id)
            .await
            .expect("should succeed");
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.event_type == EventType::Created));
        let event_version_ids: Vec<_> = events.iter().map(|e| e.version_id).collect();
        assert!(event_version_ids.contains(&v1.id));
        assert!(event_version_ids.contains(&v2.id));

        // Events for an unrelated session must not leak through.
        let unrelated = get_timeline_events(SessionId::new())
            .await
            .expect("should succeed");
        assert!(unrelated.is_empty());
    }
}

//! Persistence for [`Version`]s and their [`TimelineEvent`] history.

use super::{dt_to_text, text_to_dt, text_to_enum, ReviewStore};
use crate::error::{ReviewError, ReviewResult};
use crate::version::timeline::{EventType, TimelineEvent};
use crate::version::Version;
use crate::{SessionId, VersionId};
use oxisql_core::{Connection, Row};

const UPSERT_VERSION_SQL: &str = "
    INSERT OR REPLACE INTO review_versions (
        id, session_id, number, label, description,
        content_url, content_hash, file_size, duration_frames, frame_rate,
        resolution_w, resolution_h, created_by, created_at, parent_id
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
";

const INSERT_EVENT_SQL: &str = "
    INSERT OR REPLACE INTO review_timeline_events (
        id, session_id, version_id, event_type, description, user_name, timestamp
    ) VALUES ($1, $2, $3, $4, $5, $6, $7)
";

fn row_to_version(row: &Row) -> ReviewResult<Version> {
    let id_str: String = row.try_get("id")?;
    let id: VersionId = id_str
        .parse()
        .map_err(|e| ReviewError::Other(format!("invalid stored version id {id_str:?}: {e}")))?;

    let session_id_str: String = row.try_get("session_id")?;
    let session_id: SessionId = session_id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored session id {session_id_str:?}: {e}"))
    })?;

    let parent_id_str: Option<String> = row.try_get("parent_id")?;
    let parent_id = parent_id_str
        .map(|s| {
            s.parse::<VersionId>()
                .map_err(|e| ReviewError::Other(format!("invalid stored parent id {s:?}: {e}")))
        })
        .transpose()?;

    let number_i64: i64 = row.try_get("number")?;
    let number = u32::try_from(number_i64).map_err(|e| {
        ReviewError::Other(format!("invalid stored version number {number_i64}: {e}"))
    })?;

    let resolution_w: i64 = row.try_get("resolution_w")?;
    let resolution_h: i64 = row.try_get("resolution_h")?;
    let file_size: i64 = row.try_get("file_size")?;

    Ok(Version {
        id,
        session_id,
        number,
        label: row.try_get("label")?,
        description: row.try_get("description")?,
        content_url: row.try_get("content_url")?,
        content_hash: row.try_get("content_hash")?,
        file_size: file_size as u64,
        duration_frames: row.try_get("duration_frames")?,
        frame_rate: row.try_get("frame_rate")?,
        resolution: (resolution_w as u32, resolution_h as u32),
        created_by: row.try_get("created_by")?,
        created_at: text_to_dt(&row.try_get::<String>("created_at")?)?,
        parent_id,
    })
}

fn row_to_timeline_event(row: &Row) -> ReviewResult<TimelineEvent> {
    let version_id_str: String = row.try_get("version_id")?;
    let version_id: VersionId = version_id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored version id {version_id_str:?}: {e}"))
    })?;

    Ok(TimelineEvent {
        id: row.try_get("id")?,
        version_id,
        event_type: text_to_enum::<EventType>(&row.try_get::<String>("event_type")?)?,
        description: row.try_get("description")?,
        user: row.try_get("user_name")?,
        timestamp: text_to_dt(&row.try_get::<String>("timestamp")?)?,
    })
}

impl ReviewStore {
    /// Inserts a version, replacing any existing row with the same ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn insert_version(&self, version: &Version) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let parent_id = version.parent_id.map(|p| p.to_string());
        let created_at = dt_to_text(version.created_at);
        let number = i64::from(version.number);
        let file_size = version.file_size as i64;
        let resolution_w = i64::from(version.resolution.0);
        let resolution_h = i64::from(version.resolution.1);

        self.conn
            .execute(
                UPSERT_VERSION_SQL,
                &[
                    &version.id.to_string(),
                    &version.session_id.to_string(),
                    &number,
                    &version.label,
                    &version.description,
                    &version.content_url,
                    &version.content_hash,
                    &file_size,
                    &version.duration_frames,
                    &version.frame_rate,
                    &resolution_w,
                    &resolution_h,
                    &version.created_by,
                    &created_at,
                    &parent_id,
                ],
            )
            .await?;
        Ok(())
    }

    /// Loads a single version by ID.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::VersionNotFound`] if no such version exists.
    pub async fn get_version(&self, id: VersionId) -> ReviewResult<Version> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        let rows = self
            .conn
            .query("SELECT * FROM review_versions WHERE id = $1", &[&id_str])
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| ReviewError::VersionNotFound(id_str.clone()))?;
        row_to_version(row)
    }

    /// Lists every version for a session, ordered by version number
    /// ascending (i.e. chronological creation order).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_versions_by_session(
        &self,
        session_id: SessionId,
    ) -> ReviewResult<Vec<Version>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_versions WHERE session_id = $1 ORDER BY number ASC",
                &[&session_id_str],
            )
            .await?;
        rows.iter().map(row_to_version).collect()
    }

    /// Deletes a version by ID. Idempotent: deleting an already-absent
    /// version is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails.
    pub async fn delete_version(&self, id: VersionId) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        self.conn
            .execute("DELETE FROM review_versions WHERE id = $1", &[&id_str])
            .await?;
        Ok(())
    }

    /// Appends a timeline event for a version.
    ///
    /// `session_id` is stored alongside the event (it is not itself a field
    /// of [`TimelineEvent`]) so that [`list_timeline_events_by_session`]
    /// can filter efficiently without a join back to `review_versions`.
    ///
    /// [`list_timeline_events_by_session`]: Self::list_timeline_events_by_session
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn insert_timeline_event(
        &self,
        event: &TimelineEvent,
        session_id: SessionId,
    ) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let event_type = super::enum_to_text(&event.event_type)?;
        let timestamp = dt_to_text(event.timestamp);

        self.conn
            .execute(
                INSERT_EVENT_SQL,
                &[
                    &event.id,
                    &session_id.to_string(),
                    &event.version_id.to_string(),
                    &event_type,
                    &event.description,
                    &event.user,
                    &timestamp,
                ],
            )
            .await?;
        Ok(())
    }

    /// Lists every timeline event recorded for a session, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_timeline_events_by_session(
        &self,
        session_id: SessionId,
    ) -> ReviewResult<Vec<TimelineEvent>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_timeline_events WHERE session_id = $1 ORDER BY timestamp ASC",
                &[&session_id_str],
            )
            .await?;
        rows.iter().map(row_to_timeline_event).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_version(session_id: SessionId, number: u32, parent_id: Option<VersionId>) -> Version {
        Version {
            id: VersionId::new(),
            session_id,
            number,
            label: format!("v{number}"),
            description: None,
            content_url: "https://example.com/video.mp4".to_string(),
            content_hash: "hash".to_string(),
            file_size: 12345,
            duration_frames: 240,
            frame_rate: 24.0,
            resolution: (1920, 1080),
            created_by: "tester".to_string(),
            created_at: chrono::Utc::now(),
            parent_id,
        }
    }

    #[tokio::test]
    async fn test_empty_store_list_is_empty() {
        let store = ReviewStore::in_memory().await.expect("open");
        let versions = store
            .list_versions_by_session(SessionId::new())
            .await
            .expect("list");
        assert!(versions.is_empty());

        let events = store
            .list_timeline_events_by_session(SessionId::new())
            .await
            .expect("list events");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_get_missing_version_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.get_version(VersionId::new()).await;
        assert!(matches!(result, Err(ReviewError::VersionNotFound(_))));
    }

    #[tokio::test]
    async fn test_insert_get_list_roundtrip_preserves_fields() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let version = test_version(session_id, 1, None);
        store.insert_version(&version).await.expect("insert");

        let loaded = store.get_version(version.id).await.expect("get");
        assert_eq!(loaded.number, 1);
        assert_eq!(loaded.label, "v1");
        assert_eq!(loaded.file_size, 12345);
        assert_eq!(loaded.duration_frames, 240);
        assert!((loaded.frame_rate - 24.0).abs() < f64::EPSILON);
        assert_eq!(loaded.resolution, (1920, 1080));
        assert_eq!(loaded.parent_id, None);

        let listed = store
            .list_versions_by_session(session_id)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn test_list_versions_ordered_by_number() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let v1 = test_version(session_id, 1, None);
        let v2 = test_version(session_id, 2, Some(v1.id));
        let v3 = test_version(session_id, 3, Some(v2.id));

        // Insert out of order to prove the query orders by `number`, not
        // insertion order.
        store.insert_version(&v3).await.expect("insert v3");
        store.insert_version(&v1).await.expect("insert v1");
        store.insert_version(&v2).await.expect("insert v2");

        let listed = store
            .list_versions_by_session(session_id)
            .await
            .expect("list");
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].number, 1);
        assert_eq!(listed[1].number, 2);
        assert_eq!(listed[2].number, 3);
        assert_eq!(listed[1].parent_id, Some(v1.id));
        assert_eq!(listed[2].parent_id, Some(v2.id));
    }

    #[tokio::test]
    async fn test_delete_version_idempotent() {
        let store = ReviewStore::in_memory().await.expect("open");
        let version = test_version(SessionId::new(), 1, None);
        store.insert_version(&version).await.expect("insert");

        store.delete_version(version.id).await.expect("delete");
        assert!(store.get_version(version.id).await.is_err());
        store.delete_version(version.id).await.expect("re-delete");
    }

    #[tokio::test]
    async fn test_timeline_events_roundtrip() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let version = test_version(session_id, 1, None);
        store
            .insert_version(&version)
            .await
            .expect("insert version");

        let event = TimelineEvent {
            id: "event-1".to_string(),
            version_id: version.id,
            event_type: EventType::Created,
            description: "Version 1 created".to_string(),
            user: "tester".to_string(),
            timestamp: chrono::Utc::now(),
        };
        store
            .insert_timeline_event(&event, session_id)
            .await
            .expect("insert event");

        let events = store
            .list_timeline_events_by_session(session_id)
            .await
            .expect("list events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "event-1");
        assert_eq!(events[0].version_id, version.id);
        assert_eq!(events[0].event_type, EventType::Created);

        // Events for a different session must not leak through.
        let other_session_events = store
            .list_timeline_events_by_session(SessionId::new())
            .await
            .expect("list events");
        assert!(other_session_events.is_empty());
    }
}

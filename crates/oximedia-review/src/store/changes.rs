//! Persistence for [`ChangeRequest`]s.

use super::{dt_to_text, enum_to_text, opt_text_to_dt, text_to_dt, text_to_enum, ReviewStore};
use crate::change::{ChangePriority, ChangeRequest, ChangeRequestId, ChangeStatus};
use crate::error::{ReviewError, ReviewResult};
use crate::SessionId;
use chrono::Utc;
use oxisql_core::{Connection, Row};

const UPSERT_SQL: &str = "
    INSERT OR REPLACE INTO review_change_requests (
        id, session_id, title, description, priority, status,
        created_at, updated_at, completed_at, assigned_to
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
";

fn row_to_change_request(row: &Row) -> ReviewResult<ChangeRequest> {
    let id_str: String = row.try_get("id")?;
    let id: ChangeRequestId = id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored change request id {id_str:?}: {e}"))
    })?;

    let session_id_str: String = row.try_get("session_id")?;
    let session_id: SessionId = session_id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored session id {session_id_str:?}: {e}"))
    })?;

    Ok(ChangeRequest {
        id,
        session_id,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        priority: text_to_enum::<ChangePriority>(&row.try_get::<String>("priority")?)?,
        status: text_to_enum::<ChangeStatus>(&row.try_get::<String>("status")?)?,
        created_at: text_to_dt(&row.try_get::<String>("created_at")?)?,
        updated_at: text_to_dt(&row.try_get::<String>("updated_at")?)?,
        completed_at: opt_text_to_dt(row.try_get("completed_at")?)?,
        assigned_to: row.try_get("assigned_to")?,
    })
}

impl ReviewStore {
    /// Inserts a change request, replacing any existing row with the same
    /// ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn insert_change_request(&self, request: &ChangeRequest) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let priority = enum_to_text(&request.priority)?;
        let status = enum_to_text(&request.status)?;
        let created_at = dt_to_text(request.created_at);
        let updated_at = dt_to_text(request.updated_at);
        let completed_at = request.completed_at.map(dt_to_text);

        self.conn
            .execute(
                UPSERT_SQL,
                &[
                    &request.id.to_string(),
                    &request.session_id.to_string(),
                    &request.title,
                    &request.description,
                    &priority,
                    &status,
                    &created_at,
                    &updated_at,
                    &completed_at,
                    &request.assigned_to,
                ],
            )
            .await?;
        Ok(())
    }

    /// Loads a single change request by ID.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::ChangeRequestNotFound`] if no such request
    /// exists.
    pub async fn get_change_request(&self, id: ChangeRequestId) -> ReviewResult<ChangeRequest> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_change_requests WHERE id = $1",
                &[&id_str],
            )
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| ReviewError::ChangeRequestNotFound(id_str.clone()))?;
        row_to_change_request(row)
    }

    /// Lists every change request for a session, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_change_requests_by_session(
        &self,
        session_id: SessionId,
    ) -> ReviewResult<Vec<ChangeRequest>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_change_requests WHERE session_id = $1 ORDER BY created_at ASC",
                &[&session_id_str],
            )
            .await?;
        rows.iter().map(row_to_change_request).collect()
    }

    /// Updates a change request's status and `updated_at`, additionally
    /// stamping `completed_at` the first time the status becomes
    /// [`ChangeStatus::Completed`].
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::ChangeRequestNotFound`] if no such request
    /// exists.
    pub async fn update_change_request_status(
        &self,
        id: ChangeRequestId,
        status: ChangeStatus,
    ) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();

        let rows = self
            .conn
            .query(
                "SELECT completed_at FROM review_change_requests WHERE id = $1",
                &[&id_str],
            )
            .await?;
        let existing = rows
            .first()
            .ok_or_else(|| ReviewError::ChangeRequestNotFound(id_str.clone()))?;
        let existing_completed_at: Option<String> = existing.try_get("completed_at")?;

        let status_text = enum_to_text(&status)?;
        let now = dt_to_text(Utc::now());
        let completed_at = if status == ChangeStatus::Completed {
            Some(now.clone())
        } else {
            existing_completed_at
        };

        self.conn
            .execute(
                "UPDATE review_change_requests \
                 SET status = $1, updated_at = $2, completed_at = $3 \
                 WHERE id = $4",
                &[&status_text, &now, &completed_at, &id_str],
            )
            .await?;
        Ok(())
    }

    /// Deletes a change request by ID. Idempotent: deleting an
    /// already-absent request is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails.
    pub async fn delete_change_request(&self, id: ChangeRequestId) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        self.conn
            .execute(
                "DELETE FROM review_change_requests WHERE id = $1",
                &[&id_str],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_request(session_id: SessionId) -> ChangeRequest {
        let now = Utc::now();
        ChangeRequest {
            id: ChangeRequestId::new(),
            session_id,
            title: "Fix audio".to_string(),
            description: "Audio levels too low".to_string(),
            priority: ChangePriority::High,
            status: ChangeStatus::Pending,
            created_at: now,
            updated_at: now,
            completed_at: None,
            assigned_to: None,
        }
    }

    #[tokio::test]
    async fn test_empty_store_list_is_empty() {
        let store = ReviewStore::in_memory().await.expect("open");
        let requests = store
            .list_change_requests_by_session(SessionId::new())
            .await
            .expect("list");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn test_get_missing_request_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.get_change_request(ChangeRequestId::new()).await;
        assert!(matches!(result, Err(ReviewError::ChangeRequestNotFound(_))));
    }

    #[tokio::test]
    async fn test_insert_get_list_roundtrip() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let request = test_request(session_id);
        store.insert_change_request(&request).await.expect("insert");

        let loaded = store.get_change_request(request.id).await.expect("get");
        assert_eq!(loaded.title, "Fix audio");
        assert_eq!(loaded.priority, ChangePriority::High);
        assert_eq!(loaded.status, ChangeStatus::Pending);
        assert!(loaded.completed_at.is_none());

        let listed = store
            .list_change_requests_by_session(session_id)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn test_update_status_sets_completed_at_once() {
        let store = ReviewStore::in_memory().await.expect("open");
        let request = test_request(SessionId::new());
        store.insert_change_request(&request).await.expect("insert");

        store
            .update_change_request_status(request.id, ChangeStatus::InProgress)
            .await
            .expect("update");
        let loaded = store.get_change_request(request.id).await.expect("get");
        assert_eq!(loaded.status, ChangeStatus::InProgress);
        assert!(loaded.completed_at.is_none());

        store
            .update_change_request_status(request.id, ChangeStatus::Completed)
            .await
            .expect("update");
        let completed = store.get_change_request(request.id).await.expect("get");
        assert_eq!(completed.status, ChangeStatus::Completed);
        let completed_at = completed.completed_at.expect("completed_at should be set");

        // Moving to a different non-completed status must not erase the
        // historical completed_at timestamp.
        store
            .update_change_request_status(request.id, ChangeStatus::Deferred)
            .await
            .expect("update");
        let deferred = store.get_change_request(request.id).await.expect("get");
        assert_eq!(deferred.status, ChangeStatus::Deferred);
        assert_eq!(deferred.completed_at, Some(completed_at));
    }

    #[tokio::test]
    async fn test_update_missing_request_status_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store
            .update_change_request_status(ChangeRequestId::new(), ChangeStatus::InProgress)
            .await;
        assert!(matches!(result, Err(ReviewError::ChangeRequestNotFound(_))));
    }

    #[tokio::test]
    async fn test_delete_change_request_idempotent() {
        let store = ReviewStore::in_memory().await.expect("open");
        let request = test_request(SessionId::new());
        store.insert_change_request(&request).await.expect("insert");

        store
            .delete_change_request(request.id)
            .await
            .expect("delete");
        assert!(store.get_change_request(request.id).await.is_err());
        store
            .delete_change_request(request.id)
            .await
            .expect("re-delete");
    }
}

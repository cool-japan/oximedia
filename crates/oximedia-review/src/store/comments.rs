//! Persistence for [`Comment`]s (including replies, which are comments with
//! `parent_id: Some(..)`).

use super::{dt_to_text, enum_to_text, opt_text_to_dt, text_to_dt, text_to_enum, ReviewStore};
use crate::comment::{Comment, CommentPriority, CommentStatus};
use crate::error::{ReviewError, ReviewResult};
use crate::{AnnotationType, CommentId, SessionId, User, UserRole};
use oxisql_core::{Connection, Row};

/// Positional-parameter INSERT/REPLACE statement shared by every comment
/// write (comments and replies live in the same table).
const UPSERT_SQL: &str = "
    INSERT OR REPLACE INTO review_comments (
        id, session_id, frame, text, annotation_type,
        author_id, author_name, author_email, author_role,
        status, priority, parent_id,
        created_at, updated_at, resolved_at, resolved_by
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
";

fn row_to_comment(row: &Row) -> ReviewResult<Comment> {
    let id_str: String = row.try_get("id")?;
    let id: CommentId = id_str
        .parse()
        .map_err(|e| ReviewError::Other(format!("invalid stored comment id {id_str:?}: {e}")))?;

    let session_id_str: String = row.try_get("session_id")?;
    let session_id: SessionId = session_id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored session id {session_id_str:?}: {e}"))
    })?;

    let parent_id_str: Option<String> = row.try_get("parent_id")?;
    let parent_id = parent_id_str
        .map(|s| {
            s.parse::<CommentId>()
                .map_err(|e| ReviewError::Other(format!("invalid stored parent id {s:?}: {e}")))
        })
        .transpose()?;

    let author = User {
        id: row.try_get("author_id")?,
        name: row.try_get("author_name")?,
        email: row.try_get("author_email")?,
        role: text_to_enum::<UserRole>(&row.try_get::<String>("author_role")?)?,
    };

    Ok(Comment {
        id,
        session_id,
        frame: row.try_get("frame")?,
        text: row.try_get("text")?,
        annotation_type: text_to_enum::<AnnotationType>(
            &row.try_get::<String>("annotation_type")?,
        )?,
        author,
        status: text_to_enum::<CommentStatus>(&row.try_get::<String>("status")?)?,
        priority: text_to_enum::<CommentPriority>(&row.try_get::<String>("priority")?)?,
        parent_id,
        created_at: text_to_dt(&row.try_get::<String>("created_at")?)?,
        updated_at: text_to_dt(&row.try_get::<String>("updated_at")?)?,
        resolved_at: opt_text_to_dt(row.try_get("resolved_at")?)?,
        resolved_by: row.try_get("resolved_by")?,
    })
}

impl ReviewStore {
    /// Inserts a comment or reply, replacing any existing row with the same
    /// ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn insert_comment(&self, comment: &Comment) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let annotation_type = enum_to_text(&comment.annotation_type)?;
        let author_role = enum_to_text(&comment.author.role)?;
        let status = enum_to_text(&comment.status)?;
        let priority = enum_to_text(&comment.priority)?;
        let parent_id = comment.parent_id.map(|p| p.to_string());
        let created_at = dt_to_text(comment.created_at);
        let updated_at = dt_to_text(comment.updated_at);
        let resolved_at = comment.resolved_at.map(dt_to_text);

        self.conn
            .execute(
                UPSERT_SQL,
                &[
                    &comment.id.to_string(),
                    &comment.session_id.to_string(),
                    &comment.frame,
                    &comment.text,
                    &annotation_type,
                    &comment.author.id,
                    &comment.author.name,
                    &comment.author.email,
                    &author_role,
                    &status,
                    &priority,
                    &parent_id,
                    &created_at,
                    &updated_at,
                    &resolved_at,
                    &comment.resolved_by,
                ],
            )
            .await?;
        Ok(())
    }

    /// Loads a single comment (or reply) by ID.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::CommentNotFound`] if no such comment exists.
    pub async fn get_comment(&self, id: CommentId) -> ReviewResult<Comment> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        let rows = self
            .conn
            .query("SELECT * FROM review_comments WHERE id = $1", &[&id_str])
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| ReviewError::CommentNotFound(id_str.clone()))?;
        row_to_comment(row)
    }

    /// Lists every comment and reply for a session, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_comments_by_session(
        &self,
        session_id: SessionId,
    ) -> ReviewResult<Vec<Comment>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_comments WHERE session_id = $1 ORDER BY created_at ASC",
                &[&session_id_str],
            )
            .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// Lists root comments (i.e. `parent_id IS NULL`) for a session, oldest
    /// first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_root_comments_by_session(
        &self,
        session_id: SessionId,
    ) -> ReviewResult<Vec<Comment>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_comments WHERE session_id = $1 AND parent_id IS NULL \
                 ORDER BY created_at ASC",
                &[&session_id_str],
            )
            .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// Lists root comments for a specific frame within a session, oldest
    /// first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_root_comments_by_frame(
        &self,
        session_id: SessionId,
        frame: i64,
    ) -> ReviewResult<Vec<Comment>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_comments \
                 WHERE session_id = $1 AND frame = $2 AND parent_id IS NULL \
                 ORDER BY created_at ASC",
                &[&session_id_str, &frame],
            )
            .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// Lists replies to a comment, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_replies(&self, parent_id: CommentId) -> ReviewResult<Vec<Comment>> {
        let _guard = self.guard.lock().await;
        let parent_id_str = parent_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_comments WHERE parent_id = $1 ORDER BY created_at ASC",
                &[&parent_id_str],
            )
            .await?;
        rows.iter().map(row_to_comment).collect()
    }

    /// Updates a comment's text and `updated_at`.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::CommentNotFound`] if no such comment exists.
    pub async fn update_comment_text(&self, id: CommentId, new_text: &str) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();

        let rows = self
            .conn
            .query("SELECT id FROM review_comments WHERE id = $1", &[&id_str])
            .await?;
        if rows.is_empty() {
            return Err(ReviewError::CommentNotFound(id_str));
        }

        let updated_at = dt_to_text(chrono::Utc::now());
        self.conn
            .execute(
                "UPDATE review_comments SET text = $1, updated_at = $2 WHERE id = $3",
                &[&new_text, &updated_at, &id_str],
            )
            .await?;
        Ok(())
    }

    /// Updates a comment's status.
    ///
    /// `resolved_at`/`resolved_by` are set when transitioning to
    /// [`CommentStatus::Resolved`], cleared when transitioning to
    /// [`CommentStatus::Open`] (an explicit unresolve), and left untouched
    /// when transitioning to [`CommentStatus::Archived`] -- archiving is not
    /// un-resolving, so a comment that was resolved before being archived
    /// keeps its resolution audit trail (who resolved it and when).
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::CommentNotFound`] if no such comment exists.
    pub async fn set_comment_status(
        &self,
        id: CommentId,
        status: CommentStatus,
        resolved_by: Option<&str>,
    ) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();

        let rows = self
            .conn
            .query(
                "SELECT resolved_at, resolved_by FROM review_comments WHERE id = $1",
                &[&id_str],
            )
            .await?;
        let existing = rows
            .first()
            .ok_or_else(|| ReviewError::CommentNotFound(id_str.clone()))?;
        let existing_resolved_at: Option<String> = existing.try_get("resolved_at")?;
        let existing_resolved_by: Option<String> = existing.try_get("resolved_by")?;

        let status_text = enum_to_text(&status)?;
        let now = chrono::Utc::now();
        let (resolved_at, resolved_by): (Option<String>, Option<String>) = match status {
            CommentStatus::Resolved => (Some(dt_to_text(now)), resolved_by.map(str::to_string)),
            CommentStatus::Open => (None, None),
            CommentStatus::Archived => (existing_resolved_at, existing_resolved_by),
        };

        self.conn
            .execute(
                "UPDATE review_comments \
                 SET status = $1, updated_at = $2, resolved_at = $3, resolved_by = $4 \
                 WHERE id = $5",
                &[
                    &status_text,
                    &dt_to_text(now),
                    &resolved_at,
                    &resolved_by,
                    &id_str,
                ],
            )
            .await?;
        Ok(())
    }

    /// Deletes a comment (or reply) by ID. Idempotent: deleting an
    /// already-absent comment is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails.
    pub async fn delete_comment(&self, id: CommentId) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        self.conn
            .execute("DELETE FROM review_comments WHERE id = $1", &[&id_str])
            .await?;
        Ok(())
    }

    /// Deletes every reply to a comment. Idempotent.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails.
    pub async fn delete_replies(&self, parent_id: CommentId) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let parent_id_str = parent_id.to_string();
        self.conn
            .execute(
                "DELETE FROM review_comments WHERE parent_id = $1",
                &[&parent_id_str],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnnotationType, User, UserRole};

    fn test_comment(session_id: SessionId, frame: i64) -> Comment {
        let now = chrono::Utc::now();
        Comment {
            id: CommentId::new(),
            session_id,
            frame,
            text: "Test comment".to_string(),
            annotation_type: AnnotationType::Issue,
            author: User {
                id: "user-1".to_string(),
                name: "User One".to_string(),
                email: "user1@example.com".to_string(),
                role: UserRole::Reviewer,
            },
            status: CommentStatus::Open,
            priority: CommentPriority::High,
            parent_id: None,
            created_at: now,
            updated_at: now,
            resolved_at: None,
            resolved_by: None,
        }
    }

    #[tokio::test]
    async fn test_empty_store_list_is_empty() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let comments = store
            .list_comments_by_session(session_id)
            .await
            .expect("list should succeed");
        assert!(comments.is_empty());
    }

    #[tokio::test]
    async fn test_get_missing_comment_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.get_comment(CommentId::new()).await;
        assert!(matches!(result, Err(ReviewError::CommentNotFound(_))));
    }

    #[tokio::test]
    async fn test_insert_get_list_roundtrip() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let comment = test_comment(session_id, 42);
        store.insert_comment(&comment).await.expect("insert");

        let loaded = store.get_comment(comment.id).await.expect("get");
        assert_eq!(loaded.id, comment.id);
        assert_eq!(loaded.session_id, session_id);
        assert_eq!(loaded.frame, 42);
        assert_eq!(loaded.text, "Test comment");
        assert_eq!(loaded.annotation_type, AnnotationType::Issue);
        assert_eq!(loaded.priority, CommentPriority::High);
        assert_eq!(loaded.author.id, "user-1");

        let by_session = store
            .list_comments_by_session(session_id)
            .await
            .expect("list");
        assert_eq!(by_session.len(), 1);

        let roots = store
            .list_root_comments_by_session(session_id)
            .await
            .expect("list roots");
        assert_eq!(roots.len(), 1);

        let by_frame = store
            .list_root_comments_by_frame(session_id, 42)
            .await
            .expect("list by frame");
        assert_eq!(by_frame.len(), 1);

        let other_frame = store
            .list_root_comments_by_frame(session_id, 43)
            .await
            .expect("list by frame");
        assert!(other_frame.is_empty());
    }

    #[tokio::test]
    async fn test_update_comment_text() {
        let store = ReviewStore::in_memory().await.expect("open");
        let comment = test_comment(SessionId::new(), 1);
        store.insert_comment(&comment).await.expect("insert");

        store
            .update_comment_text(comment.id, "Updated")
            .await
            .expect("update");
        let loaded = store.get_comment(comment.id).await.expect("get");
        assert_eq!(loaded.text, "Updated");
        assert!(loaded.updated_at >= comment.updated_at);
    }

    #[tokio::test]
    async fn test_update_missing_comment_text_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.update_comment_text(CommentId::new(), "text").await;
        assert!(matches!(result, Err(ReviewError::CommentNotFound(_))));
    }

    #[tokio::test]
    async fn test_set_comment_status_resolve_and_unresolve() {
        let store = ReviewStore::in_memory().await.expect("open");
        let comment = test_comment(SessionId::new(), 1);
        store.insert_comment(&comment).await.expect("insert");

        store
            .set_comment_status(comment.id, CommentStatus::Resolved, Some("user-9"))
            .await
            .expect("resolve");
        let loaded = store.get_comment(comment.id).await.expect("get");
        assert_eq!(loaded.status, CommentStatus::Resolved);
        assert!(loaded.resolved_at.is_some());
        assert_eq!(loaded.resolved_by.as_deref(), Some("user-9"));

        store
            .set_comment_status(comment.id, CommentStatus::Open, None)
            .await
            .expect("unresolve");
        let loaded = store.get_comment(comment.id).await.expect("get");
        assert_eq!(loaded.status, CommentStatus::Open);
        assert!(loaded.resolved_at.is_none());
        assert!(loaded.resolved_by.is_none());
    }

    /// Archiving a resolved comment must not erase its resolution audit
    /// trail: archiving is not un-resolving (regression test for a bug
    /// caught in review, where `set_comment_status` originally cleared
    /// `resolved_at`/`resolved_by` for *any* non-Resolved status).
    #[tokio::test]
    async fn test_archiving_resolved_comment_preserves_resolution_audit_trail() {
        let store = ReviewStore::in_memory().await.expect("open");
        let comment = test_comment(SessionId::new(), 1);
        store.insert_comment(&comment).await.expect("insert");

        store
            .set_comment_status(comment.id, CommentStatus::Resolved, Some("user-9"))
            .await
            .expect("resolve");
        let resolved = store.get_comment(comment.id).await.expect("get");
        let resolved_at = resolved.resolved_at.expect("resolved_at should be set");

        store
            .set_comment_status(comment.id, CommentStatus::Archived, None)
            .await
            .expect("archive");
        let archived = store.get_comment(comment.id).await.expect("get");
        assert_eq!(archived.status, CommentStatus::Archived);
        assert_eq!(archived.resolved_at, Some(resolved_at));
        assert_eq!(archived.resolved_by.as_deref(), Some("user-9"));
    }

    #[tokio::test]
    async fn test_delete_comment_idempotent() {
        let store = ReviewStore::in_memory().await.expect("open");
        let comment = test_comment(SessionId::new(), 1);
        store.insert_comment(&comment).await.expect("insert");

        store.delete_comment(comment.id).await.expect("delete");
        assert!(store.get_comment(comment.id).await.is_err());
        // Deleting an already-absent comment is not an error.
        store.delete_comment(comment.id).await.expect("re-delete");
    }

    #[tokio::test]
    async fn test_replies_and_delete_replies() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let root = test_comment(session_id, 5);
        store.insert_comment(&root).await.expect("insert root");

        let mut reply = test_comment(session_id, 5);
        reply.parent_id = Some(root.id);
        store.insert_comment(&reply).await.expect("insert reply");

        let replies = store.list_replies(root.id).await.expect("list replies");
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].id, reply.id);

        // Replies must not show up as roots.
        let roots = store
            .list_root_comments_by_session(session_id)
            .await
            .expect("list roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, root.id);

        store.delete_replies(root.id).await.expect("delete replies");
        assert!(store
            .list_replies(root.id)
            .await
            .expect("list replies")
            .is_empty());
        // The root itself must survive deleting its replies.
        assert!(store.get_comment(root.id).await.is_ok());
    }
}

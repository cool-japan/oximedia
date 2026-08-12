//! Adding comments to review sessions.

use crate::{
    comment::{Comment, CommentPriority, CommentStatus},
    error::ReviewResult,
    AnnotationType, CommentId, SessionId, User, UserRole,
};
use chrono::Utc;

/// Add a comment to a specific frame.
///
/// # Arguments
///
/// * `session_id` - ID of the session
/// * `frame` - Frame number (0-indexed)
/// * `text` - Comment text
/// * `annotation_type` - Type of annotation
///
/// # Errors
///
/// Returns error if comment cannot be added.
pub async fn add_comment(
    session_id: SessionId,
    frame: i64,
    text: &str,
    annotation_type: AnnotationType,
) -> ReviewResult<CommentId> {
    // Default author; real callers that have an authenticated user should
    // use `add_comment_detailed` instead.
    let author = User {
        id: "system".to_string(),
        name: "System".to_string(),
        email: "system@oximedia.local".to_string(),
        role: UserRole::Reviewer,
    };

    add_comment_detailed(
        session_id,
        frame,
        text,
        annotation_type,
        author,
        CommentPriority::Normal,
    )
    .await
}

/// Add a comment with detailed information.
///
/// # Arguments
///
/// * `session_id` - ID of the session
/// * `frame` - Frame number
/// * `text` - Comment text
/// * `annotation_type` - Type of annotation
/// * `author` - Comment author
/// * `priority` - Comment priority
///
/// # Errors
///
/// Returns error if comment cannot be added.
pub async fn add_comment_detailed(
    session_id: SessionId,
    frame: i64,
    text: &str,
    annotation_type: AnnotationType,
    author: User,
    priority: CommentPriority,
) -> ReviewResult<CommentId> {
    let comment_id = CommentId::new();
    let now = Utc::now();

    let comment = Comment {
        id: comment_id,
        session_id,
        frame,
        text: text.to_string(),
        annotation_type,
        author,
        status: CommentStatus::Open,
        priority,
        parent_id: None,
        created_at: now,
        updated_at: now,
        resolved_at: None,
        resolved_by: None,
    };

    let store = crate::store::default_store().await?;
    store.insert_comment(&comment).await?;
    Ok(comment_id)
}

/// Add multiple comments in batch.
///
/// # Arguments
///
/// * `session_id` - ID of the session
/// * `comments` - List of (frame, text, `annotation_type`) tuples
///
/// # Errors
///
/// Returns error if any comment cannot be added.
pub async fn add_comments_batch(
    session_id: SessionId,
    comments: &[(i64, String, AnnotationType)],
) -> ReviewResult<Vec<CommentId>> {
    let mut comment_ids = Vec::new();

    for (frame, text, annotation_type) in comments {
        let id = add_comment(session_id, *frame, text, *annotation_type).await?;
        comment_ids.push(id);
    }

    Ok(comment_ids)
}

/// Update a comment's text.
///
/// # Arguments
///
/// * `comment_id` - ID of the comment
/// * `new_text` - New comment text
///
/// # Errors
///
/// Returns [`crate::error::ReviewError::CommentNotFound`] if `comment_id`
/// does not identify an existing comment.
pub async fn update_comment(comment_id: CommentId, new_text: &str) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.update_comment_text(comment_id, new_text).await
}

/// Delete a comment.
///
/// This permanently removes the comment (and, per [`crate::comment::CommentStatus::Archived`]
/// being a distinct status, differs from archiving -- to archive instead of
/// delete, resolve the comment's status via [`crate::comment::resolve::archive_comment`]).
/// Idempotent: deleting an already-absent comment is not an error.
///
/// # Arguments
///
/// * `comment_id` - ID of the comment
///
/// # Errors
///
/// Returns error if comment cannot be deleted.
pub async fn delete_comment(comment_id: CommentId) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.delete_comment(comment_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_comment() {
        let session_id = SessionId::new();
        let result = add_comment(session_id, 100, "Test comment", AnnotationType::Issue).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_add_comment_detailed() {
        let session_id = SessionId::new();
        let author = User {
            id: "user-1".to_string(),
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            role: UserRole::Reviewer,
        };

        let result = add_comment_detailed(
            session_id,
            100,
            "Test comment",
            AnnotationType::Issue,
            author,
            CommentPriority::High,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_add_comments_batch() {
        let session_id = SessionId::new();
        let comments = vec![
            (100, "Comment 1".to_string(), AnnotationType::Issue),
            (200, "Comment 2".to_string(), AnnotationType::Suggestion),
        ];

        let result = add_comments_batch(session_id, &comments).await;
        assert!(result.is_ok());
        assert_eq!(result.expect("should succeed in test").len(), 2);
    }

    #[tokio::test]
    async fn test_update_comment() {
        let session_id = SessionId::new();
        let comment_id = add_comment(session_id, 100, "Original", AnnotationType::Issue)
            .await
            .expect("add should succeed");

        let result = update_comment(comment_id, "Updated text").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_comment_not_found_is_honest_error() {
        // A comment ID that was never created must not silently succeed.
        let comment_id = CommentId::new();
        let result = update_comment(comment_id, "Updated text").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_comment_then_update_is_honest_error() {
        let session_id = SessionId::new();
        let comment_id = add_comment(session_id, 100, "Original", AnnotationType::Issue)
            .await
            .expect("add should succeed");

        delete_comment(comment_id)
            .await
            .expect("delete should succeed");
        // Deleting again is idempotent, not an error.
        delete_comment(comment_id)
            .await
            .expect("re-delete should be idempotent");

        let result = update_comment(comment_id, "Updated text").await;
        assert!(result.is_err());
    }
}

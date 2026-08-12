//! Reply to comments.

use crate::{
    comment::{Comment, CommentPriority, CommentStatus},
    error::{ReviewError, ReviewResult},
    AnnotationType, CommentId, User, UserRole,
};
use chrono::Utc;

/// Add a reply to a comment.
///
/// # Arguments
///
/// * `parent_id` - ID of the parent comment
/// * `text` - Reply text
///
/// # Errors
///
/// Returns error if reply cannot be added.
pub async fn add_reply(parent_id: CommentId, text: &str) -> ReviewResult<CommentId> {
    // Default author; real callers that have an authenticated user should
    // use `add_reply_with_author` instead.
    let author = User {
        id: "system".to_string(),
        name: "System".to_string(),
        email: "system@oximedia.local".to_string(),
        role: UserRole::Reviewer,
    };

    add_reply_with_author(parent_id, text, author).await
}

/// Add a reply with author information.
///
/// `session_id` and `frame` are inherited from the parent comment (a reply
/// necessarily belongs to the same session and frame as its parent), so the
/// parent must already exist.
///
/// # Arguments
///
/// * `parent_id` - ID of the parent comment
/// * `text` - Reply text
/// * `author` - Reply author
///
/// # Errors
///
/// Returns [`crate::error::ReviewError::CommentNotFound`] if `parent_id`
/// does not identify an existing comment.
pub async fn add_reply_with_author(
    parent_id: CommentId,
    text: &str,
    author: User,
) -> ReviewResult<CommentId> {
    let store = crate::store::default_store().await?;
    let parent = store.get_comment(parent_id).await?;

    let reply_id = CommentId::new();
    let now = Utc::now();

    let reply = Comment {
        id: reply_id,
        session_id: parent.session_id,
        frame: parent.frame,
        text: text.to_string(),
        annotation_type: AnnotationType::General,
        author,
        status: CommentStatus::Open,
        priority: CommentPriority::Normal,
        parent_id: Some(parent_id),
        created_at: now,
        updated_at: now,
        resolved_at: None,
        resolved_by: None,
    };

    store.insert_comment(&reply).await?;
    Ok(reply_id)
}

/// Get all replies to a comment.
///
/// # Arguments
///
/// * `parent_id` - ID of the parent comment
///
/// # Errors
///
/// Returns error if replies cannot be retrieved.
pub async fn get_replies(parent_id: CommentId) -> ReviewResult<Vec<Comment>> {
    let store = crate::store::default_store().await?;
    store.list_replies(parent_id).await
}

/// Count replies to a comment.
///
/// # Arguments
///
/// * `parent_id` - ID of the parent comment
///
/// # Errors
///
/// Returns error if count cannot be retrieved.
pub async fn count_replies(parent_id: CommentId) -> ReviewResult<usize> {
    let replies = get_replies(parent_id).await?;
    Ok(replies.len())
}

/// Check if a comment has replies.
///
/// # Arguments
///
/// * `comment_id` - ID of the comment
///
/// # Errors
///
/// Returns error if check fails.
pub async fn has_replies(comment_id: CommentId) -> ReviewResult<bool> {
    let count = count_replies(comment_id).await?;
    Ok(count > 0)
}

/// Delete all replies to a comment.
///
/// # Arguments
///
/// * `parent_id` - ID of the parent comment
///
/// # Errors
///
/// Returns error if deletion fails.
pub async fn delete_all_replies(parent_id: CommentId) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.delete_replies(parent_id).await
}

/// Validate that a reply can be added.
///
/// # Errors
///
/// Returns error if validation fails.
pub fn validate_reply(parent_status: CommentStatus) -> ReviewResult<()> {
    if parent_status == CommentStatus::Archived {
        return Err(ReviewError::InvalidStateTransition(
            "Cannot reply to archived comment".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionId;

    async fn create_test_parent() -> CommentId {
        let session_id = SessionId::new();
        crate::comment::add::add_comment(session_id, 100, "Parent comment", AnnotationType::Issue)
            .await
            .expect("parent comment creation should succeed")
    }

    #[tokio::test]
    async fn test_add_reply() {
        let parent_id = create_test_parent().await;
        let result = add_reply(parent_id, "Test reply").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_add_reply_not_found_is_honest_error() {
        // A parent that was never created must not silently succeed.
        let parent_id = CommentId::new();
        let result = add_reply(parent_id, "Test reply").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_reply_with_author() {
        let parent_id = create_test_parent().await;
        let author = User {
            id: "user-1".to_string(),
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            role: UserRole::Reviewer,
        };

        let result = add_reply_with_author(parent_id, "Test reply", author).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_reply_inherits_session_and_frame_from_parent() {
        let session_id = SessionId::new();
        let parent_id = crate::comment::add::add_comment(
            session_id,
            42,
            "Parent comment",
            AnnotationType::Issue,
        )
        .await
        .expect("parent comment creation should succeed");

        let reply_id = add_reply(parent_id, "Test reply")
            .await
            .expect("reply should succeed");

        let replies = get_replies(parent_id).await.expect("list should succeed");
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].id, reply_id);
        assert_eq!(replies[0].session_id, session_id);
        assert_eq!(replies[0].frame, 42);
        assert_eq!(replies[0].parent_id, Some(parent_id));
    }

    #[tokio::test]
    async fn test_get_replies() {
        let parent_id = create_test_parent().await;
        let result = get_replies(parent_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_count_replies() {
        let parent_id = create_test_parent().await;
        assert_eq!(count_replies(parent_id).await.expect("should succeed"), 0);

        add_reply(parent_id, "Reply 1")
            .await
            .expect("reply should succeed");
        add_reply(parent_id, "Reply 2")
            .await
            .expect("reply should succeed");
        assert_eq!(count_replies(parent_id).await.expect("should succeed"), 2);
        assert!(has_replies(parent_id).await.expect("should succeed"));
    }

    #[tokio::test]
    async fn test_delete_all_replies() {
        let parent_id = create_test_parent().await;
        add_reply(parent_id, "Reply 1")
            .await
            .expect("reply should succeed");
        add_reply(parent_id, "Reply 2")
            .await
            .expect("reply should succeed");
        assert_eq!(count_replies(parent_id).await.expect("should succeed"), 2);

        delete_all_replies(parent_id)
            .await
            .expect("delete should succeed");
        assert_eq!(count_replies(parent_id).await.expect("should succeed"), 0);
    }

    #[test]
    fn test_validate_reply() {
        assert!(validate_reply(CommentStatus::Open).is_ok());
        assert!(validate_reply(CommentStatus::Resolved).is_ok());
        assert!(validate_reply(CommentStatus::Archived).is_err());
    }
}

//! Comment resolution.

use crate::{
    comment::CommentStatus,
    error::{ReviewError, ReviewResult},
    CommentId,
};

/// Resolve a comment.
///
/// # Arguments
///
/// * `comment_id` - ID of the comment to resolve
/// * `user_id` - ID of the user resolving the comment
///
/// # Errors
///
/// Returns error if comment cannot be resolved.
pub async fn resolve_comment(comment_id: CommentId, user_id: &str) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    let comment = store.get_comment(comment_id).await?;
    validate_resolution(comment.status)?;
    store
        .set_comment_status(comment_id, CommentStatus::Resolved, Some(user_id))
        .await
}

/// Unresolve a comment.
///
/// # Arguments
///
/// * `comment_id` - ID of the comment to unresolve
///
/// # Errors
///
/// Returns error if comment cannot be unresolved.
pub async fn unresolve_comment(comment_id: CommentId) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    let comment = store.get_comment(comment_id).await?;
    validate_unresolve(comment.status)?;
    store
        .set_comment_status(comment_id, CommentStatus::Open, None)
        .await
}

/// Mark comment as archived.
///
/// # Arguments
///
/// * `comment_id` - ID of the comment
///
/// # Errors
///
/// Returns error if comment cannot be archived.
pub async fn archive_comment(comment_id: CommentId) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    // Ensure the comment exists before honoring the archive request.
    store.get_comment(comment_id).await?;
    store
        .set_comment_status(comment_id, CommentStatus::Archived, None)
        .await
}

/// Bulk resolve multiple comments.
///
/// # Arguments
///
/// * `comment_ids` - IDs of comments to resolve
/// * `user_id` - ID of the user resolving the comments
///
/// # Errors
///
/// Returns error if any comment cannot be resolved.
pub async fn resolve_comments_batch(comment_ids: &[CommentId], user_id: &str) -> ReviewResult<()> {
    for comment_id in comment_ids {
        resolve_comment(*comment_id, user_id).await?;
    }
    Ok(())
}

/// Check if all comments in a session are resolved.
///
/// # Arguments
///
/// * `unresolved_count` - Number of unresolved comments
///
/// # Errors
///
/// Returns error if check fails.
pub fn all_comments_resolved(unresolved_count: usize) -> ReviewResult<bool> {
    Ok(unresolved_count == 0)
}

/// Validate comment resolution.
///
/// # Errors
///
/// Returns error if validation fails.
pub fn validate_resolution(current_status: CommentStatus) -> ReviewResult<()> {
    if current_status == CommentStatus::Resolved {
        return Err(ReviewError::CommentAlreadyResolved);
    }

    if current_status == CommentStatus::Archived {
        return Err(ReviewError::InvalidStateTransition(
            "Cannot resolve archived comment".to_string(),
        ));
    }

    Ok(())
}

/// Validate unresolve operation.
///
/// # Errors
///
/// Returns error if validation fails.
pub fn validate_unresolve(current_status: CommentStatus) -> ReviewResult<()> {
    if current_status != CommentStatus::Resolved {
        return Err(ReviewError::InvalidStateTransition(
            "Can only unresolve resolved comments".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnnotationType, SessionId};

    async fn create_test_comment() -> CommentId {
        let session_id = SessionId::new();
        crate::comment::add::add_comment(session_id, 100, "Test comment", AnnotationType::Issue)
            .await
            .expect("comment creation should succeed")
    }

    #[tokio::test]
    async fn test_resolve_comment() {
        let comment_id = create_test_comment().await;
        let result = resolve_comment(comment_id, "user-123").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_resolve_comment_not_found_is_honest_error() {
        // A comment ID that was never created must not silently succeed.
        let comment_id = CommentId::new();
        let result = resolve_comment(comment_id, "user-123").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_comment_twice_is_error() {
        let comment_id = create_test_comment().await;
        resolve_comment(comment_id, "user-123")
            .await
            .expect("first resolve should succeed");
        let result = resolve_comment(comment_id, "user-123").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unresolve_comment() {
        let comment_id = create_test_comment().await;
        resolve_comment(comment_id, "user-123")
            .await
            .expect("resolve should succeed");

        let result = unresolve_comment(comment_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_unresolve_comment_not_yet_resolved_is_error() {
        // validate_unresolve requires the comment to currently be Resolved.
        let comment_id = create_test_comment().await;
        let result = unresolve_comment(comment_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_comments_batch() {
        let comment_ids = vec![create_test_comment().await, create_test_comment().await];
        let result = resolve_comments_batch(&comment_ids, "user-123").await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_all_comments_resolved() {
        assert!(all_comments_resolved(0).expect("should succeed in test"));
        assert!(!all_comments_resolved(5).expect("should succeed in test"));
    }

    #[test]
    fn test_validate_resolution() {
        assert!(validate_resolution(CommentStatus::Open).is_ok());
        assert!(validate_resolution(CommentStatus::Resolved).is_err());
        assert!(validate_resolution(CommentStatus::Archived).is_err());
    }

    #[test]
    fn test_validate_unresolve() {
        assert!(validate_unresolve(CommentStatus::Resolved).is_ok());
        assert!(validate_unresolve(CommentStatus::Open).is_err());
    }
}

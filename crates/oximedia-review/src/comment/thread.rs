//! Threaded comment discussions.

use crate::{
    comment::{Comment, CommentStatus},
    error::ReviewResult,
    CommentId,
};
use serde::{Deserialize, Serialize};

/// A threaded comment discussion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentThread {
    /// Root comment.
    pub root: Comment,
    /// Replies to the root comment.
    pub replies: Vec<Comment>,
    /// Total reply count.
    pub reply_count: usize,
    /// Unresolved reply count.
    pub unresolved_count: usize,
}

impl CommentThread {
    /// Check if the entire thread is resolved.
    #[must_use]
    pub fn is_fully_resolved(&self) -> bool {
        self.root.is_resolved() && self.unresolved_count == 0
    }

    /// Get the latest comment in the thread.
    #[must_use]
    pub fn latest_comment(&self) -> &Comment {
        self.replies
            .iter()
            .max_by_key(|c| c.created_at)
            .unwrap_or(&self.root)
    }

    /// Count participants in the thread.
    #[must_use]
    pub fn participant_count(&self) -> usize {
        let mut participants = std::collections::HashSet::new();
        participants.insert(&self.root.author.id);
        for reply in &self.replies {
            participants.insert(&reply.author.id);
        }
        participants.len()
    }
}

/// Builds a [`CommentThread`] for an already-loaded root comment by loading
/// its replies and deriving the reply/unresolved counts from them.
async fn build_thread(
    store: &crate::store::ReviewStore,
    root: Comment,
) -> ReviewResult<CommentThread> {
    let replies = store.list_replies(root.id).await?;
    let unresolved_count = replies.iter().filter(|c| !c.is_resolved()).count();
    Ok(CommentThread {
        root,
        reply_count: replies.len(),
        unresolved_count,
        replies,
    })
}

/// Create a comment thread.
///
/// # Arguments
///
/// * `root_id` - ID of the root comment
///
/// # Errors
///
/// Returns [`crate::error::ReviewError::CommentNotFound`] if `root_id` does
/// not identify an existing comment.
pub async fn create_thread(root_id: CommentId) -> ReviewResult<CommentThread> {
    let store = crate::store::default_store().await?;
    let root = store.get_comment(root_id).await?;
    build_thread(store, root).await
}

/// Get all threads for a session.
///
/// # Arguments
///
/// * `session_id` - ID of the session
///
/// # Errors
///
/// Returns error if threads cannot be retrieved.
pub async fn get_session_threads(session_id: crate::SessionId) -> ReviewResult<Vec<CommentThread>> {
    let store = crate::store::default_store().await?;
    let roots = store.list_root_comments_by_session(session_id).await?;
    let mut threads = Vec::with_capacity(roots.len());
    for root in roots {
        threads.push(build_thread(store, root).await?);
    }
    Ok(threads)
}

/// Get threads for a specific frame.
///
/// # Arguments
///
/// * `session_id` - ID of the session
/// * `frame` - Frame number
///
/// # Errors
///
/// Returns error if threads cannot be retrieved.
pub async fn get_frame_threads(
    session_id: crate::SessionId,
    frame: i64,
) -> ReviewResult<Vec<CommentThread>> {
    let store = crate::store::default_store().await?;
    let roots = store.list_root_comments_by_frame(session_id, frame).await?;
    let mut threads = Vec::with_capacity(roots.len());
    for root in roots {
        threads.push(build_thread(store, root).await?);
    }
    Ok(threads)
}

/// Resolve an entire thread.
///
/// # Arguments
///
/// * `root_id` - ID of the root comment
/// * `user_id` - ID of the user resolving the thread
///
/// # Errors
///
/// Returns [`crate::error::ReviewError::CommentNotFound`] if `root_id` does
/// not identify an existing comment.
pub async fn resolve_thread(root_id: CommentId, user_id: &str) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    // Ensure the root exists before touching anything.
    store.get_comment(root_id).await?;
    store
        .set_comment_status(root_id, CommentStatus::Resolved, Some(user_id))
        .await?;

    for reply in store.list_replies(root_id).await? {
        if reply.status != CommentStatus::Resolved {
            store
                .set_comment_status(reply.id, CommentStatus::Resolved, Some(user_id))
                .await?;
        }
    }
    Ok(())
}

/// Get unresolved threads count.
///
/// # Arguments
///
/// * `session_id` - ID of the session
///
/// # Errors
///
/// Returns error if count cannot be retrieved.
pub async fn get_unresolved_thread_count(session_id: crate::SessionId) -> ReviewResult<usize> {
    let threads = get_session_threads(session_id).await?;
    Ok(threads.iter().filter(|t| !t.is_fully_resolved()).count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnnotationType, SessionId, User, UserRole};
    use chrono::Utc;

    fn create_test_thread() -> CommentThread {
        let root = Comment {
            id: CommentId::new(),
            session_id: SessionId::new(),
            frame: 100,
            text: "Root comment".to_string(),
            annotation_type: AnnotationType::Issue,
            author: User {
                id: "user-1".to_string(),
                name: "User 1".to_string(),
                email: "user1@example.com".to_string(),
                role: UserRole::Reviewer,
            },
            status: CommentStatus::Open,
            priority: crate::comment::CommentPriority::Normal,
            parent_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            resolved_at: None,
            resolved_by: None,
        };

        CommentThread {
            root,
            replies: Vec::new(),
            reply_count: 0,
            unresolved_count: 0,
        }
    }

    #[test]
    fn test_thread_is_fully_resolved() {
        let mut thread = create_test_thread();
        assert!(!thread.is_fully_resolved());

        thread.root.status = CommentStatus::Resolved;
        thread.unresolved_count = 0;
        assert!(thread.is_fully_resolved());
    }

    #[test]
    fn test_thread_latest_comment() {
        let thread = create_test_thread();
        let latest = thread.latest_comment();
        assert_eq!(latest.id, thread.root.id);
    }

    #[test]
    fn test_thread_participant_count() {
        let thread = create_test_thread();
        assert_eq!(thread.participant_count(), 1);
    }

    #[tokio::test]
    async fn test_create_thread() {
        let session_id = SessionId::new();
        let root_id = crate::comment::add::add_comment(
            session_id,
            100,
            "Root comment",
            AnnotationType::Issue,
        )
        .await
        .expect("root comment creation should succeed");

        let thread = create_thread(root_id).await.expect("should succeed");
        assert_eq!(thread.root.id, root_id);
        assert_eq!(thread.reply_count, 0);
        assert_eq!(thread.unresolved_count, 0);
    }

    #[tokio::test]
    async fn test_create_thread_not_found_is_honest_error() {
        // A root comment that was never created must not fabricate a thread.
        let root_id = CommentId::new();
        let result = create_thread(root_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_and_frame_threads_reflect_real_data() {
        let session_id = SessionId::new();
        let root1 =
            crate::comment::add::add_comment(session_id, 10, "Root 1", AnnotationType::Issue)
                .await
                .expect("should succeed");
        let _root2 =
            crate::comment::add::add_comment(session_id, 20, "Root 2", AnnotationType::Suggestion)
                .await
                .expect("should succeed");
        crate::comment::reply::add_reply(root1, "Reply to root 1")
            .await
            .expect("reply should succeed");

        let session_threads = get_session_threads(session_id)
            .await
            .expect("should succeed");
        assert_eq!(session_threads.len(), 2);
        let root1_thread = session_threads
            .iter()
            .find(|t| t.root.id == root1)
            .expect("root1 thread present");
        assert_eq!(root1_thread.reply_count, 1);
        assert_eq!(root1_thread.unresolved_count, 1);

        let frame10_threads = get_frame_threads(session_id, 10)
            .await
            .expect("should succeed");
        assert_eq!(frame10_threads.len(), 1);
        assert_eq!(frame10_threads[0].root.id, root1);

        let frame_no_comments = get_frame_threads(session_id, 999)
            .await
            .expect("should succeed");
        assert!(frame_no_comments.is_empty());

        assert_eq!(
            get_unresolved_thread_count(session_id)
                .await
                .expect("should succeed"),
            2
        );

        resolve_thread(root1, "user-123")
            .await
            .expect("resolve_thread should succeed");
        let after = get_session_threads(session_id)
            .await
            .expect("should succeed");
        let root1_after = after
            .iter()
            .find(|t| t.root.id == root1)
            .expect("root1 thread present");
        assert!(root1_after.root.is_resolved());
        assert_eq!(root1_after.unresolved_count, 0);
        assert!(root1_after.is_fully_resolved());
    }
}

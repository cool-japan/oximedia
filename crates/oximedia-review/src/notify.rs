//! Notification system.

use crate::{error::ReviewResult, SessionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod email;
pub mod system;
pub mod webhook;

pub use email::send_email_notification;
pub use system::SystemNotification;
pub use webhook::send_webhook;

/// Notification type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationType {
    /// Comment added.
    CommentAdded,
    /// Comment resolved.
    CommentResolved,
    /// Task assigned.
    TaskAssigned,
    /// Task completed.
    TaskCompleted,
    /// Approval requested.
    ApprovalRequested,
    /// Approval granted.
    ApprovalGranted,
    /// Approval rejected.
    ApprovalRejected,
    /// Session invited.
    SessionInvited,
    /// Session closed.
    SessionClosed,
    /// Deadline approaching.
    DeadlineApproaching,
    /// Deadline passed.
    DeadlinePassed,
}

/// Notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Notification ID.
    pub id: String,
    /// Session ID.
    pub session_id: SessionId,
    /// Notification type.
    pub notification_type: NotificationType,
    /// Recipient user ID.
    pub recipient: String,
    /// Notification title.
    pub title: String,
    /// Notification message.
    pub message: String,
    /// Link/URL (if any).
    pub link: Option<String>,
    /// Read status.
    pub read: bool,
    /// Created timestamp.
    pub created_at: DateTime<Utc>,
}

impl Notification {
    /// Create a new notification.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        notification_type: NotificationType,
        recipient: String,
        title: String,
        message: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            notification_type,
            recipient,
            title,
            message,
            link: None,
            read: false,
            created_at: Utc::now(),
        }
    }

    /// Mark notification as read.
    pub fn mark_read(&mut self) {
        self.read = true;
    }

    /// Set link.
    #[must_use]
    pub fn with_link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }
}

/// Send a notification.
///
/// Persists `notification` to the review store so it shows up in
/// [`get_user_notifications`]. This does **not** attempt channel delivery --
/// dispatching over email or webhook is a separate concern handled by
/// [`crate::notify::email::send_email_notification`] and
/// [`crate::notify::webhook::send_webhook`] respectively; callers that want
/// both in-app persistence and outbound delivery must call both.
///
/// # Errors
///
/// Returns error if the notification cannot be persisted.
pub async fn send_notification(notification: Notification) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.insert_notification(&notification).await
}

/// Get notifications for a user.
///
/// # Errors
///
/// Returns error if retrieval fails.
pub async fn get_user_notifications(user_id: &str) -> ReviewResult<Vec<Notification>> {
    let store = crate::store::default_store().await?;
    store.list_notifications_by_user(user_id).await
}

/// Mark notification as read.
///
/// # Errors
///
/// Returns [`crate::error::ReviewError::NotificationNotFound`] if
/// `notification_id` does not identify an existing notification.
pub async fn mark_notification_read(notification_id: &str) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.mark_notification_read(notification_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_creation() {
        let session_id = SessionId::new();
        let notification = Notification::new(
            session_id,
            NotificationType::CommentAdded,
            "user-1".to_string(),
            "New Comment".to_string(),
            "A new comment was added".to_string(),
        );

        assert_eq!(notification.recipient, "user-1");
        assert!(!notification.read);
        assert!(notification.link.is_none());
    }

    #[test]
    fn test_notification_mark_read() {
        let session_id = SessionId::new();
        let mut notification = Notification::new(
            session_id,
            NotificationType::CommentAdded,
            "user-1".to_string(),
            "New Comment".to_string(),
            "A new comment was added".to_string(),
        );

        assert!(!notification.read);
        notification.mark_read();
        assert!(notification.read);
    }

    #[test]
    fn test_notification_with_link() {
        let session_id = SessionId::new();
        let notification = Notification::new(
            session_id,
            NotificationType::CommentAdded,
            "user-1".to_string(),
            "New Comment".to_string(),
            "A new comment was added".to_string(),
        )
        .with_link("https://example.com/comment/123");

        assert!(notification.link.is_some());
    }

    #[tokio::test]
    async fn test_send_notification() {
        let session_id = SessionId::new();
        let notification = Notification::new(
            session_id,
            NotificationType::CommentAdded,
            "user-1".to_string(),
            "Test".to_string(),
            "Test message".to_string(),
        );

        let result = send_notification(notification).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_user_notifications() {
        let result = get_user_notifications("user-1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mark_notification_read_roundtrip() {
        // Unique recipient so this test's list is not affected by other
        // tests concurrently writing through the same process-wide default
        // store.
        let recipient = format!("user-{}", uuid::Uuid::new_v4());
        let session_id = SessionId::new();
        let notification = Notification::new(
            session_id,
            NotificationType::CommentAdded,
            recipient.clone(),
            "Test".to_string(),
            "Test message".to_string(),
        );
        let notification_id = notification.id.clone();

        send_notification(notification)
            .await
            .expect("send should succeed");

        let before = get_user_notifications(&recipient)
            .await
            .expect("list should succeed");
        assert_eq!(before.len(), 1);
        assert!(!before[0].read);

        mark_notification_read(&notification_id)
            .await
            .expect("mark read should succeed");

        let after = get_user_notifications(&recipient)
            .await
            .expect("list should succeed");
        assert_eq!(after.len(), 1);
        assert!(after[0].read);
    }

    #[tokio::test]
    async fn test_mark_notification_read_not_found_is_honest_error() {
        let result = mark_notification_read("does-not-exist").await;
        assert!(result.is_err());
    }
}

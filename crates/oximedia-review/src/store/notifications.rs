//! Persistence for [`Notification`]s.

use super::{dt_to_text, enum_to_text, text_to_dt, text_to_enum, ReviewStore};
use crate::error::{ReviewError, ReviewResult};
use crate::notify::{Notification, NotificationType};
use crate::SessionId;
use oxisql_core::{Connection, Row};

const UPSERT_SQL: &str = "
    INSERT OR REPLACE INTO review_notifications (
        id, session_id, notification_type, recipient, title, message, link, read, created_at
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
";

fn row_to_notification(row: &Row) -> ReviewResult<Notification> {
    let session_id_str: String = row.try_get("session_id")?;
    let session_id: SessionId = session_id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored session id {session_id_str:?}: {e}"))
    })?;
    let read: i64 = row.try_get("read")?;

    Ok(Notification {
        id: row.try_get("id")?,
        session_id,
        notification_type: text_to_enum::<NotificationType>(
            &row.try_get::<String>("notification_type")?,
        )?,
        recipient: row.try_get("recipient")?,
        title: row.try_get("title")?,
        message: row.try_get("message")?,
        link: row.try_get("link")?,
        read: read != 0,
        created_at: text_to_dt(&row.try_get::<String>("created_at")?)?,
    })
}

impl ReviewStore {
    /// Inserts a notification, replacing any existing row with the same ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn insert_notification(&self, notification: &Notification) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let notification_type = enum_to_text(&notification.notification_type)?;
        let read = i64::from(notification.read);
        let created_at = dt_to_text(notification.created_at);

        self.conn
            .execute(
                UPSERT_SQL,
                &[
                    &notification.id,
                    &notification.session_id.to_string(),
                    &notification_type,
                    &notification.recipient,
                    &notification.title,
                    &notification.message,
                    &notification.link,
                    &read,
                    &created_at,
                ],
            )
            .await?;
        Ok(())
    }

    /// Loads a single notification by ID.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::NotificationNotFound`] if no such notification
    /// exists.
    pub async fn get_notification(&self, id: &str) -> ReviewResult<Notification> {
        let _guard = self.guard.lock().await;
        let rows = self
            .conn
            .query("SELECT * FROM review_notifications WHERE id = $1", &[&id])
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| ReviewError::NotificationNotFound(id.to_string()))?;
        row_to_notification(row)
    }

    /// Lists every notification addressed to `recipient`, newest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_notifications_by_user(
        &self,
        recipient: &str,
    ) -> ReviewResult<Vec<Notification>> {
        let _guard = self.guard.lock().await;
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_notifications WHERE recipient = $1 ORDER BY created_at DESC",
                &[&recipient],
            )
            .await?;
        rows.iter().map(row_to_notification).collect()
    }

    /// Marks a notification as read.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::NotificationNotFound`] if no such notification
    /// exists.
    pub async fn mark_notification_read(&self, id: &str) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let rows = self
            .conn
            .query("SELECT id FROM review_notifications WHERE id = $1", &[&id])
            .await?;
        if rows.is_empty() {
            return Err(ReviewError::NotificationNotFound(id.to_string()));
        }

        self.conn
            .execute(
                "UPDATE review_notifications SET read = 1 WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(())
    }

    /// Deletes a notification by ID. Idempotent: deleting an already-absent
    /// notification is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails.
    pub async fn delete_notification(&self, id: &str) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        self.conn
            .execute("DELETE FROM review_notifications WHERE id = $1", &[&id])
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_notification(session_id: SessionId, recipient: &str) -> Notification {
        Notification::new(
            session_id,
            NotificationType::CommentAdded,
            recipient.to_string(),
            "New Comment".to_string(),
            "A new comment was added".to_string(),
        )
    }

    #[tokio::test]
    async fn test_empty_store_list_is_empty() {
        let store = ReviewStore::in_memory().await.expect("open");
        let notifications = store
            .list_notifications_by_user("nobody")
            .await
            .expect("list");
        assert!(notifications.is_empty());
    }

    #[tokio::test]
    async fn test_get_missing_notification_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.get_notification("does-not-exist").await;
        assert!(matches!(result, Err(ReviewError::NotificationNotFound(_))));
    }

    #[tokio::test]
    async fn test_insert_get_list_roundtrip() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let notification = test_notification(session_id, "user-42");
        store
            .insert_notification(&notification)
            .await
            .expect("insert");

        let loaded = store.get_notification(&notification.id).await.expect("get");
        assert_eq!(loaded.recipient, "user-42");
        assert_eq!(loaded.title, "New Comment");
        assert!(!loaded.read);

        let listed = store
            .list_notifications_by_user("user-42")
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);

        // A different recipient must not see this notification.
        let other = store
            .list_notifications_by_user("someone-else")
            .await
            .expect("list");
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn test_mark_notification_read() {
        let store = ReviewStore::in_memory().await.expect("open");
        let notification = test_notification(SessionId::new(), "user-1");
        store
            .insert_notification(&notification)
            .await
            .expect("insert");

        store
            .mark_notification_read(&notification.id)
            .await
            .expect("mark read");
        let loaded = store.get_notification(&notification.id).await.expect("get");
        assert!(loaded.read);
    }

    #[tokio::test]
    async fn test_mark_missing_notification_read_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.mark_notification_read("does-not-exist").await;
        assert!(matches!(result, Err(ReviewError::NotificationNotFound(_))));
    }

    #[tokio::test]
    async fn test_delete_notification_idempotent() {
        let store = ReviewStore::in_memory().await.expect("open");
        let notification = test_notification(SessionId::new(), "user-1");
        store
            .insert_notification(&notification)
            .await
            .expect("insert");

        store
            .delete_notification(&notification.id)
            .await
            .expect("delete");
        assert!(store.get_notification(&notification.id).await.is_err());
        store
            .delete_notification(&notification.id)
            .await
            .expect("re-delete");
    }
}

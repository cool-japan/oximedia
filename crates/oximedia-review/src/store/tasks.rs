//! Persistence for [`Task`]s.

use super::{dt_to_text, enum_to_text, opt_text_to_dt, text_to_dt, text_to_enum, ReviewStore};
use crate::error::{ReviewError, ReviewResult};
use crate::task::{Task, TaskPriority, TaskStatus};
use crate::{SessionId, TaskId, User, UserRole};
use oxisql_core::{Connection, Row};

const UPSERT_SQL: &str = "
    INSERT OR REPLACE INTO review_tasks (
        id, session_id, title, description,
        assignee_id, assignee_name, assignee_email, assignee_role,
        creator_id, creator_name, creator_email, creator_role,
        status, priority, deadline, created_at, updated_at, completed_at
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
";

fn row_to_task(row: &Row) -> ReviewResult<Task> {
    let id_str: String = row.try_get("id")?;
    let id: TaskId = id_str
        .parse()
        .map_err(|e| ReviewError::Other(format!("invalid stored task id {id_str:?}: {e}")))?;

    let session_id_str: String = row.try_get("session_id")?;
    let session_id: SessionId = session_id_str.parse().map_err(|e| {
        ReviewError::Other(format!("invalid stored session id {session_id_str:?}: {e}"))
    })?;

    let assignee = User {
        id: row.try_get("assignee_id")?,
        name: row.try_get("assignee_name")?,
        email: row.try_get("assignee_email")?,
        role: text_to_enum::<UserRole>(&row.try_get::<String>("assignee_role")?)?,
    };
    let creator = User {
        id: row.try_get("creator_id")?,
        name: row.try_get("creator_name")?,
        email: row.try_get("creator_email")?,
        role: text_to_enum::<UserRole>(&row.try_get::<String>("creator_role")?)?,
    };

    Ok(Task {
        id,
        session_id,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        assignee,
        creator,
        status: text_to_enum::<TaskStatus>(&row.try_get::<String>("status")?)?,
        priority: text_to_enum::<TaskPriority>(&row.try_get::<String>("priority")?)?,
        deadline: opt_text_to_dt(row.try_get("deadline")?)?,
        created_at: text_to_dt(&row.try_get::<String>("created_at")?)?,
        updated_at: text_to_dt(&row.try_get::<String>("updated_at")?)?,
        completed_at: opt_text_to_dt(row.try_get("completed_at")?)?,
    })
}

impl ReviewStore {
    /// Inserts a task, replacing any existing row with the same ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn insert_task(&self, task: &Task) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;

        let assignee_role = enum_to_text(&task.assignee.role)?;
        let creator_role = enum_to_text(&task.creator.role)?;
        let status = enum_to_text(&task.status)?;
        let priority = enum_to_text(&task.priority)?;
        let deadline = task.deadline.map(dt_to_text);
        let created_at = dt_to_text(task.created_at);
        let updated_at = dt_to_text(task.updated_at);
        let completed_at = task.completed_at.map(dt_to_text);

        self.conn
            .execute(
                UPSERT_SQL,
                &[
                    &task.id.to_string(),
                    &task.session_id.to_string(),
                    &task.title,
                    &task.description,
                    &task.assignee.id,
                    &task.assignee.name,
                    &task.assignee.email,
                    &assignee_role,
                    &task.creator.id,
                    &task.creator.name,
                    &task.creator.email,
                    &creator_role,
                    &status,
                    &priority,
                    &deadline,
                    &created_at,
                    &updated_at,
                    &completed_at,
                ],
            )
            .await?;
        Ok(())
    }

    /// Loads a single task by ID.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::TaskNotFound`] if no such task exists.
    pub async fn get_task(&self, id: TaskId) -> ReviewResult<Task> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        let rows = self
            .conn
            .query("SELECT * FROM review_tasks WHERE id = $1", &[&id_str])
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| ReviewError::TaskNotFound(id_str.clone()))?;
        row_to_task(row)
    }

    /// Lists every task for a session, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_tasks_by_session(&self, session_id: SessionId) -> ReviewResult<Vec<Task>> {
        let _guard = self.guard.lock().await;
        let session_id_str = session_id.to_string();
        let rows = self
            .conn
            .query(
                "SELECT * FROM review_tasks WHERE session_id = $1 ORDER BY created_at ASC",
                &[&session_id_str],
            )
            .await?;
        rows.iter().map(row_to_task).collect()
    }

    /// Reassigns a task to a different user, updating `updated_at`.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewError::TaskNotFound`] if no such task exists.
    pub async fn update_task_assignee(&self, id: TaskId, new_assignee: &User) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();

        let rows = self
            .conn
            .query("SELECT id FROM review_tasks WHERE id = $1", &[&id_str])
            .await?;
        if rows.is_empty() {
            return Err(ReviewError::TaskNotFound(id_str));
        }

        let role = enum_to_text(&new_assignee.role)?;
        let updated_at = dt_to_text(chrono::Utc::now());
        self.conn
            .execute(
                "UPDATE review_tasks \
                 SET assignee_id = $1, assignee_name = $2, assignee_email = $3, \
                     assignee_role = $4, updated_at = $5 \
                 WHERE id = $6",
                &[
                    &new_assignee.id,
                    &new_assignee.name,
                    &new_assignee.email,
                    &role,
                    &updated_at,
                    &id_str,
                ],
            )
            .await?;
        Ok(())
    }

    /// Deletes a task by ID. Idempotent: deleting an already-absent task is
    /// not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails.
    pub async fn delete_task(&self, id: TaskId) -> ReviewResult<()> {
        let _guard = self.guard.lock().await;
        let id_str = id.to_string();
        self.conn
            .execute("DELETE FROM review_tasks WHERE id = $1", &[&id_str])
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;

    fn test_user(id: &str) -> User {
        User {
            id: id.to_string(),
            name: format!("User {id}"),
            email: format!("{id}@example.com"),
            role: UserRole::Reviewer,
        }
    }

    fn test_task(session_id: SessionId) -> Task {
        let now = chrono::Utc::now();
        Task {
            id: TaskId::new(),
            session_id,
            title: "Review pass".to_string(),
            description: None,
            assignee: test_user("assignee"),
            creator: test_user("creator"),
            status: TaskStatus::Open,
            priority: TaskPriority::Normal,
            deadline: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn test_empty_store_list_is_empty() {
        let store = ReviewStore::in_memory().await.expect("open");
        let tasks = store
            .list_tasks_by_session(SessionId::new())
            .await
            .expect("list");
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_get_missing_task_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store.get_task(TaskId::new()).await;
        assert!(matches!(result, Err(ReviewError::TaskNotFound(_))));
    }

    #[tokio::test]
    async fn test_insert_get_list_roundtrip() {
        let store = ReviewStore::in_memory().await.expect("open");
        let session_id = SessionId::new();
        let task = test_task(session_id);
        store.insert_task(&task).await.expect("insert");

        let loaded = store.get_task(task.id).await.expect("get");
        assert_eq!(loaded.title, "Review pass");
        assert_eq!(loaded.assignee.id, "assignee");
        assert_eq!(loaded.creator.id, "creator");
        assert_eq!(loaded.status, TaskStatus::Open);

        let listed = store.list_tasks_by_session(session_id).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, task.id);
    }

    #[tokio::test]
    async fn test_update_task_assignee() {
        let store = ReviewStore::in_memory().await.expect("open");
        let task = test_task(SessionId::new());
        store.insert_task(&task).await.expect("insert");

        let new_assignee = test_user("new-assignee");
        store
            .update_task_assignee(task.id, &new_assignee)
            .await
            .expect("reassign");

        let loaded = store.get_task(task.id).await.expect("get");
        assert_eq!(loaded.assignee.id, "new-assignee");
        assert!(loaded.updated_at >= task.updated_at);
    }

    #[tokio::test]
    async fn test_update_missing_task_assignee_is_not_found() {
        let store = ReviewStore::in_memory().await.expect("open");
        let result = store
            .update_task_assignee(TaskId::new(), &test_user("nobody"))
            .await;
        assert!(matches!(result, Err(ReviewError::TaskNotFound(_))));
    }

    #[tokio::test]
    async fn test_delete_task_idempotent() {
        let store = ReviewStore::in_memory().await.expect("open");
        let task = test_task(SessionId::new());
        store.insert_task(&task).await.expect("insert");

        store.delete_task(task.id).await.expect("delete");
        assert!(store.get_task(task.id).await.is_err());
        store.delete_task(task.id).await.expect("re-delete");
    }
}

//! Task assignment.

use crate::{error::ReviewResult, SessionId, TaskId, User};

/// Assign a task to a user.
///
/// Thin wrapper over [`crate::task::create_task`] that returns just the new
/// task's ID; the task is created already assigned to `assignee`.
///
/// # Errors
///
/// Returns error if assignment fails.
pub async fn assign_task(
    session_id: SessionId,
    title: String,
    assignee: User,
    creator: User,
) -> ReviewResult<TaskId> {
    let task = crate::task::create_task(session_id, title, assignee, creator).await?;
    Ok(task.id)
}

/// Reassign a task to a different user.
///
/// # Errors
///
/// Returns [`crate::error::ReviewError::TaskNotFound`] if `task_id` does not
/// identify an existing task.
pub async fn reassign_task(task_id: TaskId, new_assignee: User) -> ReviewResult<()> {
    let store = crate::store::default_store().await?;
    store.update_task_assignee(task_id, &new_assignee).await
}

/// Bulk assign tasks.
///
/// # Errors
///
/// Returns error if any assignment fails.
pub async fn assign_tasks_bulk(
    session_id: SessionId,
    assignments: &[(String, User)],
    creator: User,
) -> ReviewResult<Vec<TaskId>> {
    let mut task_ids = Vec::new();

    for (title, assignee) in assignments {
        let id = assign_task(session_id, title.clone(), assignee.clone(), creator.clone()).await?;
        task_ids.push(id);
    }

    Ok(task_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UserRole;

    fn create_test_user(id: &str) -> User {
        User {
            id: id.to_string(),
            name: format!("User {}", id),
            email: format!("{}@example.com", id),
            role: UserRole::Reviewer,
        }
    }

    #[tokio::test]
    async fn test_assign_task() {
        let session_id = SessionId::new();
        let assignee = create_test_user("assignee");
        let creator = create_test_user("creator");

        let result = assign_task(session_id, "Test task".to_string(), assignee, creator).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_reassign_task() {
        let session_id = SessionId::new();
        let assignee = create_test_user("assignee");
        let creator = create_test_user("creator");
        let task_id = assign_task(session_id, "Test task".to_string(), assignee, creator)
            .await
            .expect("assign should succeed");

        let new_assignee = create_test_user("newuser");
        let result = reassign_task(task_id, new_assignee).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_reassign_task_not_found_is_honest_error() {
        // A task ID that was never created must not silently succeed.
        let task_id = TaskId::new();
        let new_assignee = create_test_user("newuser");

        let result = reassign_task(task_id, new_assignee).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_assign_tasks_bulk() {
        let session_id = SessionId::new();
        let creator = create_test_user("creator");

        let assignments = vec![
            ("Task 1".to_string(), create_test_user("user1")),
            ("Task 2".to_string(), create_test_user("user2")),
        ];

        let result = assign_tasks_bulk(session_id, &assignments, creator).await;
        assert!(result.is_ok());
        assert_eq!(result.expect("should succeed in test").len(), 2);
    }
}

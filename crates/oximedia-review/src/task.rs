//! Task assignment and tracking.

use crate::{error::ReviewResult, SessionId, TaskId, User};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod assign;
pub mod deadline;
pub mod track;

pub use assign::assign_task;
pub use deadline::TaskDeadline;
pub use track::TaskTracker;

/// Review task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Task ID.
    pub id: TaskId,
    /// Session ID.
    pub session_id: SessionId,
    /// Task title.
    pub title: String,
    /// Task description.
    pub description: Option<String>,
    /// Assigned user.
    pub assignee: User,
    /// Task creator.
    pub creator: User,
    /// Task status.
    pub status: TaskStatus,
    /// Task priority.
    pub priority: TaskPriority,
    /// Deadline.
    pub deadline: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last updated timestamp.
    pub updated_at: DateTime<Utc>,
    /// Completion timestamp.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Task status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is open.
    Open,
    /// Task is in progress.
    InProgress,
    /// Task is blocked.
    Blocked,
    /// Task is completed.
    Completed,
    /// Task is cancelled.
    Cancelled,
}

/// Task priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TaskPriority {
    /// Low priority.
    Low,
    /// Normal priority.
    Normal,
    /// High priority.
    High,
    /// Urgent priority.
    Urgent,
}

impl Task {
    /// Check if task is overdue.
    #[must_use]
    pub fn is_overdue(&self) -> bool {
        if let Some(deadline) = self.deadline {
            Utc::now() > deadline && !self.is_complete()
        } else {
            false
        }
    }

    /// Check if task is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.status == TaskStatus::Completed
    }

    /// Mark task as complete.
    pub fn complete(&mut self) {
        self.status = TaskStatus::Completed;
        self.completed_at = Some(Utc::now());
        self.updated_at = Utc::now();
    }

    /// Get time remaining until deadline.
    #[must_use]
    pub fn time_remaining(&self) -> Option<chrono::Duration> {
        self.deadline.map(|deadline| deadline - Utc::now())
    }
}

/// Create a new task.
///
/// # Errors
///
/// Returns error if task creation fails.
pub async fn create_task(
    session_id: SessionId,
    title: String,
    assignee: User,
    creator: User,
) -> ReviewResult<Task> {
    let now = Utc::now();

    let task = Task {
        id: TaskId::new(),
        session_id,
        title,
        description: None,
        assignee,
        creator,
        status: TaskStatus::Open,
        priority: TaskPriority::Normal,
        deadline: None,
        created_at: now,
        updated_at: now,
        completed_at: None,
    };

    let store = crate::store::default_store().await?;
    store.insert_task(&task).await?;
    Ok(task)
}

/// List tasks for a session.
///
/// # Errors
///
/// Returns error if listing fails.
pub async fn list_tasks(session_id: SessionId) -> ReviewResult<Vec<Task>> {
    let store = crate::store::default_store().await?;
    store.list_tasks_by_session(session_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UserRole;
    use chrono::Duration;

    fn create_test_user(id: &str) -> User {
        User {
            id: id.to_string(),
            name: format!("User {}", id),
            email: format!("{}@example.com", id),
            role: UserRole::Reviewer,
        }
    }

    #[tokio::test]
    async fn test_create_task() {
        let session_id = SessionId::new();
        let assignee = create_test_user("assignee");
        let creator = create_test_user("creator");

        let task = create_task(session_id, "Review video".to_string(), assignee, creator)
            .await
            .expect("should succeed in test");

        assert_eq!(task.title, "Review video");
        assert_eq!(task.status, TaskStatus::Open);
    }

    #[test]
    fn test_task_complete() {
        let mut task = Task {
            id: TaskId::new(),
            session_id: SessionId::new(),
            title: "Test".to_string(),
            description: None,
            assignee: create_test_user("user1"),
            creator: create_test_user("user2"),
            status: TaskStatus::InProgress,
            priority: TaskPriority::Normal,
            deadline: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
        };

        assert!(!task.is_complete());

        task.complete();
        assert!(task.is_complete());
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn test_task_is_overdue() {
        let mut task = Task {
            id: TaskId::new(),
            session_id: SessionId::new(),
            title: "Test".to_string(),
            description: None,
            assignee: create_test_user("user1"),
            creator: create_test_user("user2"),
            status: TaskStatus::Open,
            priority: TaskPriority::Normal,
            deadline: Some(Utc::now() - Duration::days(1)),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
        };

        assert!(task.is_overdue());

        task.complete();
        assert!(!task.is_overdue());
    }

    #[test]
    fn test_priority_ordering() {
        assert!(TaskPriority::Low < TaskPriority::Normal);
        assert!(TaskPriority::Normal < TaskPriority::High);
        assert!(TaskPriority::High < TaskPriority::Urgent);
    }
}

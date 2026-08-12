//! Collaborative review and approval workflow for `OxiMedia`.
//!
//! This crate provides comprehensive review and approval capabilities for video content,
//! including:
//!
//! - Frame-accurate comments and annotations
//! - Real-time collaboration with multiple reviewers
//! - Version comparison and tracking
//! - Multi-stage approval workflows
//! - Task assignment and tracking
//! - Drawing tools for visual feedback
//! - Notification system (email, webhook)
//! - Export capabilities (PDF, CSV, EDL)
//!
//! # Example
//!
//! ```
//! use oximedia_review::{ReviewSession, SessionConfig, AnnotationType};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create a review session
//! let config = SessionConfig::builder()
//!     .title("Final Cut Review")
//!     .content_id("video-123")
//!     .workflow_type(oximedia_review::WorkflowType::MultiStage)
//!     .build()?;
//!
//! let session = ReviewSession::create(config).await?;
//!
//! // Add a frame-accurate comment
//! session.add_comment(
//!     1000, // frame number
//!     "Please adjust color grading",
//!     AnnotationType::Issue,
//! ).await?;
//!
//! // Invite reviewers
//! session.invite_user("reviewer@example.com").await?;
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::unused_async)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::similar_names)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::unused_self)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::format_push_string)]
#![allow(clippy::match_like_matches_macro)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub mod annotation;
pub mod annotation_export;
pub mod annotations;
pub mod approval;
pub mod approval_workflow;
pub mod batch_ops;
pub mod change;
pub mod comment;
pub mod comment_thread;
pub mod compare;
pub mod comparison_mode;
pub mod deadline;
pub mod delivery;
pub mod drawing;
pub mod export;
pub mod feedback_round;
pub mod marker;
pub mod notify;
pub mod offline_review;
pub mod realtime;
pub mod realtime_delta;
pub mod report;
pub mod review_api;
pub mod review_automation;
pub mod review_checklist;
pub mod review_comparator;
pub mod review_diff;
pub mod review_export;
pub mod review_history;
pub mod review_link;
pub mod review_metrics;
pub mod review_notification_rule;
pub mod review_permission;
pub mod review_playlist;
pub mod review_priority;
pub mod review_session;
pub mod review_snapshot;
pub mod review_status;
pub mod review_tag;
pub mod review_template;
pub mod session;
pub mod status;
pub mod store;
pub mod task;
pub mod timeline_note;
pub mod version;
pub mod version_compare;
pub mod version_lazy;

/// Error types for review operations.
pub mod error;

pub use compare::{
    apply_compare_filter, compare_cached, CompareCache, CompareFilter, CompareLayout,
    CompareResult, CompareVersion, DiffStats, MediaComparator, WipeAngle,
};
pub use error::{ReviewError, ReviewResult};
pub use session::ReviewSession;
pub use store::{set_default_store, ReviewStore};
pub use timeline_note::{NoteType, TimeRange, TimelineNote, TimelineNoteCollection};

/// Unique identifier for a review session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Create a new session ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID.
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// Unique identifier for a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommentId(Uuid);

impl CommentId {
    /// Create a new comment ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID.
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for CommentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CommentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for CommentId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// Unique identifier for a drawing/annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DrawingId(Uuid);

impl DrawingId {
    /// Create a new drawing ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID.
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for DrawingId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DrawingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(Uuid);

impl TaskId {
    /// Create a new task ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID.
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for TaskId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// Unique identifier for a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VersionId(Uuid);

impl VersionId {
    /// Create a new version ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID.
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for VersionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for VersionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for VersionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// User information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// User ID.
    pub id: String,
    /// User name.
    pub name: String,
    /// User email.
    pub email: String,
    /// User role in the review.
    pub role: UserRole,
}

/// User role in a review session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserRole {
    /// Session owner/creator.
    Owner,
    /// Reviewer with approval rights.
    Approver,
    /// Reviewer without approval rights.
    Reviewer,
    /// Observer (read-only).
    Observer,
}

/// Type of annotation/comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotationType {
    /// General feedback comment.
    General,
    /// Issue that needs to be fixed.
    Issue,
    /// Optional suggestion for improvement.
    Suggestion,
    /// Question requiring clarification.
    Question,
    /// Approval marker.
    Approval,
    /// Rejection marker.
    Rejection,
}

/// Workflow type for review sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowType {
    /// Simple workflow: Creator → Reviewer → Approved.
    Simple,
    /// Multi-stage workflow: Multiple sequential stages.
    MultiStage,
    /// Parallel workflow: Multiple reviewers simultaneously.
    Parallel,
    /// Sequential workflow: One reviewer after another.
    Sequential,
}

/// Configuration for creating a review session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Session title.
    pub title: String,
    /// ID of the content being reviewed.
    pub content_id: String,
    /// Workflow type.
    pub workflow_type: WorkflowType,
    /// Optional description.
    pub description: Option<String>,
    /// Optional deadline.
    pub deadline: Option<DateTime<Utc>>,
    /// Custom metadata.
    pub metadata: HashMap<String, String>,
}

impl SessionConfig {
    /// Create a new builder for session configuration.
    #[must_use]
    pub fn builder() -> SessionConfigBuilder {
        SessionConfigBuilder::default()
    }
}

/// Builder for `SessionConfig`.
#[derive(Default)]
pub struct SessionConfigBuilder {
    title: Option<String>,
    content_id: Option<String>,
    workflow_type: Option<WorkflowType>,
    description: Option<String>,
    deadline: Option<DateTime<Utc>>,
    metadata: HashMap<String, String>,
}

impl SessionConfigBuilder {
    /// Set the session title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the content ID.
    #[must_use]
    pub fn content_id(mut self, id: impl Into<String>) -> Self {
        self.content_id = Some(id.into());
        self
    }

    /// Set the workflow type.
    #[must_use]
    pub fn workflow_type(mut self, workflow: WorkflowType) -> Self {
        self.workflow_type = Some(workflow);
        self
    }

    /// Set the description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the deadline.
    #[must_use]
    pub fn deadline(mut self, deadline: DateTime<Utc>) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Add metadata key-value pair.
    #[must_use]
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Build the configuration.
    ///
    /// # Errors
    ///
    /// Returns `ReviewError::InvalidConfig` if required fields (`title`, `content_id`) are missing
    /// or empty.
    pub fn build(self) -> crate::error::ReviewResult<SessionConfig> {
        let title = self
            .title
            .filter(|t| !t.is_empty())
            .ok_or_else(|| crate::error::ReviewError::InvalidConfig("title is required".into()))?;
        let content_id = self.content_id.filter(|c| !c.is_empty()).ok_or_else(|| {
            crate::error::ReviewError::InvalidConfig("content_id is required".into())
        })?;
        Ok(SessionConfig {
            title,
            content_id,
            workflow_type: self.workflow_type.unwrap_or(WorkflowType::Simple),
            description: self.description,
            deadline: self.deadline,
            metadata: self.metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_creation() {
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_comment_id_creation() {
        let id1 = CommentId::new();
        let id2 = CommentId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_id_from_str_roundtrip() {
        let session_id = SessionId::new();
        let parsed: SessionId = session_id.to_string().parse().expect("should parse");
        assert_eq!(session_id, parsed);

        let comment_id = CommentId::new();
        let parsed: CommentId = comment_id.to_string().parse().expect("should parse");
        assert_eq!(comment_id, parsed);

        let task_id = TaskId::new();
        let parsed: TaskId = task_id.to_string().parse().expect("should parse");
        assert_eq!(task_id, parsed);

        let version_id = VersionId::new();
        let parsed: VersionId = version_id.to_string().parse().expect("should parse");
        assert_eq!(version_id, parsed);
    }

    #[test]
    fn test_id_from_str_rejects_garbage() {
        assert!("not-a-uuid".parse::<SessionId>().is_err());
    }

    #[test]
    fn test_session_config_builder() {
        let config = SessionConfig::builder()
            .title("Test Session")
            .content_id("video-123")
            .workflow_type(WorkflowType::Simple)
            .description("Test description")
            .metadata("key", "value")
            .build()
            .expect("valid config");

        assert_eq!(config.title, "Test Session");
        assert_eq!(config.content_id, "video-123");
        assert_eq!(config.workflow_type, WorkflowType::Simple);
        assert_eq!(config.description, Some("Test description".to_string()));
        assert_eq!(config.metadata.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_session_config_builder_missing_title_errors() {
        let result = SessionConfig::builder().content_id("video-123").build();
        assert!(result.is_err());
    }

    #[test]
    fn test_session_config_builder_missing_content_id_errors() {
        let result = SessionConfig::builder().title("My Review").build();
        assert!(result.is_err());
    }

    #[test]
    fn test_user_role_equality() {
        assert_eq!(UserRole::Owner, UserRole::Owner);
        assert_ne!(UserRole::Owner, UserRole::Reviewer);
    }

    #[test]
    fn test_annotation_type_equality() {
        assert_eq!(AnnotationType::Issue, AnnotationType::Issue);
        assert_ne!(AnnotationType::Issue, AnnotationType::Suggestion);
    }
}

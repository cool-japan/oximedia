//! Error types for review operations.

use thiserror::Error;

/// Result type for review operations.
pub type ReviewResult<T> = Result<T, ReviewError>;

/// Errors that can occur during review operations.
#[derive(Debug, Error)]
pub enum ReviewError {
    /// Session not found.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// Comment not found.
    #[error("Comment not found: {0}")]
    CommentNotFound(String),

    /// Drawing not found.
    #[error("Drawing not found: {0}")]
    DrawingNotFound(String),

    /// Task not found.
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    /// Version not found.
    #[error("Version not found: {0}")]
    VersionNotFound(String),

    /// Change request not found.
    #[error("Change request not found: {0}")]
    ChangeRequestNotFound(String),

    /// Notification not found.
    #[error("Notification not found: {0}")]
    NotificationNotFound(String),

    /// User not found.
    #[error("User not found: {0}")]
    UserNotFound(String),

    /// Permission denied.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Invalid frame number.
    #[error("Invalid frame number: {0}")]
    InvalidFrame(i64),

    /// Invalid state transition.
    #[error("Invalid state transition: {0}")]
    InvalidStateTransition(String),

    /// Session already closed.
    #[error("Session already closed")]
    SessionClosed,

    /// Comment already resolved.
    #[error("Comment already resolved")]
    CommentAlreadyResolved,

    /// Approval already submitted.
    #[error("Approval already submitted")]
    ApprovalAlreadySubmitted,

    /// Workflow validation error.
    #[error("Workflow validation error: {0}")]
    WorkflowValidation(String),

    /// Export error.
    #[error("Export error: {0}")]
    ExportError(String),

    /// Notification error.
    #[error("Notification error: {0}")]
    NotificationError(String),

    /// Real-time sync error.
    #[error("Real-time sync error: {0}")]
    SyncError(String),

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Persistence-layer (SQLite) error from the review store.
    #[error("Database error: {0}")]
    Database(#[from] oxisql_core::OxiSqlError),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Other error.
    #[error("Other error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ReviewError::SessionNotFound("session-123".to_string());
        assert_eq!(err.to_string(), "Session not found: session-123");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ReviewError = io_err.into();
        assert!(matches!(err, ReviewError::Io(_)));
    }
}

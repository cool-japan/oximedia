//! Real persistence for review sessions: comments, replies, threads, tasks,
//! versions, change requests, and notifications.
//!
//! Backed by the Pure-Rust OxiSQL engine (`oxisql-sqlite-compat`), so the
//! default build compiles zero C code (no `libsqlite3-sys`). Two modes are
//! supported:
//!
//! - **In-memory** ([`ReviewStore::in_memory`]) — a fresh, private database
//!   that lives only as long as the [`ReviewStore`] handle. This is what the
//!   crate's free functions (e.g. [`crate::comment::add_comment`],
//!   [`crate::task::list_tasks`]) use by default, lazily created on first use.
//! - **File-backed** ([`ReviewStore::open`]) — a database persisted to disk,
//!   surviving process restarts. Install one as the crate-wide default via
//!   [`set_default_store`] before any free function runs, or hold a
//!   [`ReviewStore`] directly and call its methods.
//!
//! # Schema
//!
//! Tables are created with `CREATE TABLE IF NOT EXISTS` on every open, so
//! opening an existing file-backed database is a no-op migration. There is
//! currently a single schema version; future column additions should keep
//! this "migrate on open" shape (e.g. `ALTER TABLE ... ADD COLUMN` guarded by
//! a `PRAGMA user_version` check) rather than introducing a separate
//! migration runner.

mod changes;
mod comments;
mod notifications;
mod tasks;
mod versions;

use crate::error::{ReviewError, ReviewResult};
use chrono::{DateTime, Utc};
use oxisql_core::Connection;
use oxisql_sqlite_compat::SqliteConnection;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::{Mutex, OnceCell};

/// Real, `SQLite`-backed persistence for every review entity.
///
/// Every public method acquires an internal lock for the duration of the
/// call, so a single [`ReviewStore`] is safe to share (e.g. via the
/// process-wide default installed by [`set_default_store`]) across
/// concurrently-running async tasks: multi-statement "load, check, write"
/// sequences (used by e.g. comment resolution) cannot interleave with each
/// other on the same store. The underlying `SqliteConnection` is itself
/// `Clone` + `Send` + `Sync` (see its own docs), but the coarser lock here
/// also protects against the affected-row-count quirks of the underlying
/// engine by preferring "read row, decide, write" over relying on `changes()`.
pub struct ReviewStore {
    conn: SqliteConnection,
    guard: Mutex<()>,
}

impl std::fmt::Debug for ReviewStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewStore").finish_non_exhaustive()
    }
}

impl ReviewStore {
    /// Opens a fresh, private in-memory database.
    ///
    /// # Errors
    ///
    /// Returns an error if the embedded engine cannot be initialized.
    pub async fn in_memory() -> ReviewResult<Self> {
        let conn = SqliteConnection::open_memory().await?;
        let store = Self {
            conn,
            guard: Mutex::new(()),
        };
        store.migrate().await?;
        Ok(store)
    }

    /// Opens (creating if necessary) a file-backed database at `path`.
    ///
    /// Pass `":memory:"` for an in-memory database, though
    /// [`in_memory`](Self::in_memory) is clearer for that case.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened, created, or migrated.
    pub async fn open(path: &str) -> ReviewResult<Self> {
        let conn = SqliteConnection::open(path).await?;
        let store = Self {
            conn,
            guard: Mutex::new(()),
        };
        store.migrate().await?;
        Ok(store)
    }

    /// Creates every table used by this crate if it does not already exist.
    async fn migrate(&self) -> ReviewResult<()> {
        const STATEMENTS: &[&str] = &[
            "CREATE TABLE IF NOT EXISTS review_comments (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                frame INTEGER NOT NULL,
                text TEXT NOT NULL,
                annotation_type TEXT NOT NULL,
                author_id TEXT NOT NULL,
                author_name TEXT NOT NULL,
                author_email TEXT NOT NULL,
                author_role TEXT NOT NULL,
                status TEXT NOT NULL,
                priority TEXT NOT NULL,
                parent_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                resolved_at TEXT,
                resolved_by TEXT
            )",
            "CREATE TABLE IF NOT EXISTS review_tasks (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT,
                assignee_id TEXT NOT NULL,
                assignee_name TEXT NOT NULL,
                assignee_email TEXT NOT NULL,
                assignee_role TEXT NOT NULL,
                creator_id TEXT NOT NULL,
                creator_name TEXT NOT NULL,
                creator_email TEXT NOT NULL,
                creator_role TEXT NOT NULL,
                status TEXT NOT NULL,
                priority TEXT NOT NULL,
                deadline TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT
            )",
            "CREATE TABLE IF NOT EXISTS review_versions (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                number INTEGER NOT NULL,
                label TEXT NOT NULL,
                description TEXT,
                content_url TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                duration_frames INTEGER NOT NULL,
                frame_rate REAL NOT NULL,
                resolution_w INTEGER NOT NULL,
                resolution_h INTEGER NOT NULL,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                parent_id TEXT
            )",
            "CREATE TABLE IF NOT EXISTS review_change_requests (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                priority TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT,
                assigned_to TEXT
            )",
            "CREATE TABLE IF NOT EXISTS review_notifications (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                notification_type TEXT NOT NULL,
                recipient TEXT NOT NULL,
                title TEXT NOT NULL,
                message TEXT NOT NULL,
                link TEXT,
                read INTEGER NOT NULL,
                created_at TEXT NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS review_timeline_events (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                version_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                description TEXT NOT NULL,
                user_name TEXT NOT NULL,
                timestamp TEXT NOT NULL
            )",
        ];

        for sql in STATEMENTS {
            self.conn.execute(sql, &[]).await?;
        }
        Ok(())
    }
}

// ── shared encoding helpers (used by the store/*.rs domain modules) ────────

/// Encodes a `serde`-derived unit-style enum (e.g. [`crate::comment::CommentStatus`])
/// as its JSON string representation (e.g. `"Open"`), for storage in a TEXT column.
pub(super) fn enum_to_text<T: Serialize>(value: &T) -> ReviewResult<String> {
    serde_json::to_string(value).map_err(ReviewError::Serialization)
}

/// Decodes a value previously stored via [`enum_to_text`].
pub(super) fn text_to_enum<T: DeserializeOwned>(text: &str) -> ReviewResult<T> {
    serde_json::from_str(text).map_err(ReviewError::Serialization)
}

/// Formats a timestamp as RFC 3339 for storage in a TEXT column.
pub(super) fn dt_to_text(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Parses a timestamp previously stored via [`dt_to_text`].
pub(super) fn text_to_dt(text: &str) -> ReviewResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| ReviewError::Other(format!("invalid stored timestamp {text:?}: {e}")))
}

/// Parses an optional timestamp previously stored via [`dt_to_text`].
pub(super) fn opt_text_to_dt(text: Option<String>) -> ReviewResult<Option<DateTime<Utc>>> {
    text.as_deref().map(text_to_dt).transpose()
}

// ── process-wide default store ──────────────────────────────────────────────

/// Lazily-initialized, process-wide default [`ReviewStore`].
///
/// Backs every scaffolded free function in this crate (e.g.
/// [`crate::change::create_change_request`], [`crate::comment::add_comment`]).
/// Defaults to a private in-memory database created on first use; install a
/// file-backed database instead via [`set_default_store`] before any free
/// function runs.
static DEFAULT_STORE: OnceCell<ReviewStore> = OnceCell::const_new();

/// Returns the process-wide default store, creating a private in-memory one
/// on first use if [`set_default_store`] was never called.
pub(crate) async fn default_store() -> ReviewResult<&'static ReviewStore> {
    DEFAULT_STORE.get_or_try_init(ReviewStore::in_memory).await
}

/// Installs `store` as the process-wide default used by this crate's free
/// functions.
///
/// Must be called before the first free-function call in the process (the
/// default in-memory store is otherwise created lazily on first use). Typical
/// use is switching the crate from its default in-memory store to a
/// file-backed one:
///
/// ```
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use oximedia_review::{set_default_store, ReviewStore};
///
/// # let path = std::env::temp_dir().join("oximedia-review-doctest.sqlite3");
/// let store = ReviewStore::open(path.to_str().expect("valid utf-8 path")).await?;
/// set_default_store(store).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`ReviewError::InvalidConfig`] if the default store was already
/// initialized (lazily or by a previous call to this function).
pub async fn set_default_store(store: ReviewStore) -> ReviewResult<()> {
    DEFAULT_STORE
        .set(store)
        .map_err(|_| ReviewError::InvalidConfig("default review store already initialized".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_store_opens_and_migrates() {
        let store = ReviewStore::in_memory().await.expect("should open");
        // Re-running migrate on an already-migrated store must be a no-op.
        store.migrate().await.expect("idempotent migration");
    }

    #[tokio::test]
    async fn test_open_file_backed_store() {
        let path =
            std::env::temp_dir().join(format!("oximedia-review-{}.sqlite3", uuid::Uuid::new_v4()));
        let path_str = path.to_str().expect("valid utf-8 path").to_string();
        let _ = std::fs::remove_file(&path);

        let store = ReviewStore::open(&path_str)
            .await
            .expect("should open file db");
        drop(store);

        let _ = std::fs::remove_file(&path);
    }

    /// File-mode persistence must survive closing and reopening the same
    /// database file -- this is what distinguishes it from the in-memory
    /// default the crate's free functions use.
    #[tokio::test]
    async fn test_file_backed_store_persists_across_reopen() {
        use crate::comment::{Comment, CommentPriority, CommentStatus};
        use crate::{AnnotationType, CommentId, SessionId, User, UserRole};

        let path = std::env::temp_dir().join(format!(
            "oximedia-review-reopen-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let path_str = path.to_str().expect("valid utf-8 path").to_string();
        let _ = std::fs::remove_file(&path);

        let session_id = SessionId::new();
        let comment_id = CommentId::new();
        let now = Utc::now();
        let comment = Comment {
            id: comment_id,
            session_id,
            frame: 7,
            text: "Survives reopen".to_string(),
            annotation_type: AnnotationType::Issue,
            author: User {
                id: "user-1".to_string(),
                name: "User One".to_string(),
                email: "user1@example.com".to_string(),
                role: UserRole::Reviewer,
            },
            status: CommentStatus::Open,
            priority: CommentPriority::Normal,
            parent_id: None,
            created_at: now,
            updated_at: now,
            resolved_at: None,
            resolved_by: None,
        };

        {
            let store = ReviewStore::open(&path_str).await.expect("open");
            store.insert_comment(&comment).await.expect("insert");
            // Store (and its connection) is dropped here, closing the file.
        }

        {
            let reopened = ReviewStore::open(&path_str).await.expect("reopen");
            let loaded = reopened
                .get_comment(comment_id)
                .await
                .expect("comment should survive reopen");
            assert_eq!(loaded.text, "Survives reopen");
            assert_eq!(loaded.session_id, session_id);

            let listed = reopened
                .list_comments_by_session(session_id)
                .await
                .expect("list should succeed");
            assert_eq!(listed.len(), 1);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_enum_roundtrip() {
        use crate::comment::CommentStatus;
        let text = enum_to_text(&CommentStatus::Resolved).expect("encode");
        assert_eq!(text, "\"Resolved\"");
        let back: CommentStatus = text_to_enum(&text).expect("decode");
        assert_eq!(back, CommentStatus::Resolved);
    }

    #[test]
    fn test_datetime_roundtrip() {
        let now = Utc::now();
        let text = dt_to_text(now);
        let back = text_to_dt(&text).expect("decode");
        // RFC 3339 round-trips to microsecond precision via chrono; compare
        // at second granularity to avoid sub-second formatting flakiness.
        assert_eq!(now.timestamp(), back.timestamp());
    }
}

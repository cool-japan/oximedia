//! `RESTful` media server for `OxiMedia`.
//!
//! `oximedia-server` provides a production-ready media server with:
//!
//! - **`RESTful` API**: File upload, transcoding, metadata, thumbnails
//! - **Media Library**: `SQLite` backend with full-text search
//! - **Streaming**: HLS/DASH adaptive bitrate, progressive download, range requests
//! - **Authentication**: JWT tokens, API keys, role-based access control
//! - **Real-time Updates**: `WebSocket` for job progress and events
//! - **Advanced Features**: Multi-part upload, thumbnail sprites, preview generation
//!
//! # Quick Start
//!
//! ```no_run
//! use oximedia_server::{Server, Config};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = Config::default();
//!     let server = Server::new(config).await?;
//!     server.serve("0.0.0.0:8080").await?;
//!     Ok(())
//! }
//! ```
//!
//! # Architecture
//!
//! The server is built on `axum` and uses:
//! - **`SQLx`** for database access with compile-time query verification
//! - **Tower** for middleware (rate limiting, compression, CORS)
//! - **JWT** for stateless authentication
//! - **`WebSocket`** for real-time updates
//!
//! # Security
//!
//! - Passwords hashed with Argon2
//! - JWT tokens with configurable expiration
//! - API key rotation support
//! - Rate limiting per user/IP
//! - CORS configuration
//!
//! # Patent-Free Only
//!
//! Like all `OxiMedia` components, this server only handles patent-free codecs
//! (AV1, VP9, VP8, Opus, Vorbis, FLAC, `WebVTT`, etc.).

#![warn(missing_docs)]
#![allow(missing_docs)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_arguments)]
// ── Suppressed clippy lints ──────────────────────────────────────────────────
//
// Pedantic doc lints – `# Errors` / `# Panics` on every handler is noisy.
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
// `HashMap<String, String>` in axum `Query` extractors cannot be generalised
// over arbitrary hashers without breaking the extractor API.
#![allow(clippy::implicit_hasher)]
// Format-string style: `"{}",  x` vs `"{x}"` – both are correct, many
// pre-existing call-sites use the older style.
#![allow(clippy::uninlined_format_args)]
// Async functions that currently have no await points are still declared
// async for API uniformity; they will gain await points as features grow.
#![allow(clippy::unused_async)]
// `format!(..)` appended to an existing `String` – cosmetic style issue.
// The lint name is `clippy::format_collect` in newer Clippy versions.
#![allow(clippy::string_extend_chars)]
#![allow(clippy::format_collect)]
// Lossless widening casts (`u32 as u64`) – also accepted as `u64::from(x)`.
#![allow(clippy::cast_lossless)]
// Narrowing/precision-losing casts that are intentional in media math.
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
// `map(f).unwrap_or_else(g)` / `map(f).unwrap_or(x)` – redundant but clear.
#![allow(clippy::map_unwrap_or)]
// Redundant closures and `items_after_statements` – minor style.
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::items_after_statements)]
// `cloned` vs `copied` on iterators – functionally identical for our types.
#![allow(clippy::cloned_instead_of_copied)]
// Raw string hashes – `r#"..."#` vs `r"..."` is author preference.
#![allow(clippy::needless_raw_string_hashes)]
// `unused_self` on methods that are part of a trait or API surface.
#![allow(clippy::unused_self)]
// Argument passed by value but not consumed – may be intentional API design.
#![allow(clippy::needless_pass_by_value)]
// Doc-markdown backtick warnings for well-known terms.
#![allow(clippy::doc_markdown)]
// `push_str(&format!(..))` – use `write!` suggestion fires in HLS/DASH
// playlist builders; rewriting is a larger refactor.
#![allow(clippy::format_push_string)]
// `Ok(x?)` – present in some library functions; harmless.
#![allow(clippy::needless_question_mark)]
// Wildcard imports in re-export modules.
#![allow(clippy::wildcard_imports)]
// Functions that are "too long" by the default threshold – media server
// handlers are inherently verbose.
#![allow(clippy::too_many_lines)]
// `MutexGuard` held across await – complex to refactor in existing code.
#![allow(clippy::await_holding_lock)]
// `match` that can be `if let` – prefer explicit match for readability.
#![allow(clippy::single_match)]
#![allow(clippy::match_like_matches_macro)]
// Struct with many bools – config structs are intentionally flat.
#![allow(clippy::struct_excessive_bools)]
// `must_use` on individual methods – handled at the crate level.
#![allow(clippy::must_use_candidate)]
// Unnecessary wrapping of return value in `Result`.
#![allow(clippy::unnecessary_wraps)]
// `impl Default` can be derived – cosmetic.
#![allow(clippy::derivable_impls)]
// `sort_by(|a, b| a.key.cmp(&b.key))` → `sort_by_key(|x| x.key)`.
#![allow(clippy::unnecessary_sort_by)]
// `Option<&T>` vs `&Option<T>` – API signature choice.
#![allow(clippy::ref_option)]
// `std::io::Error::other` – newer API, keep compat with older Rust.
#![allow(clippy::io_other_error)]
// `useless format!` – cosmetic in some generated code.
#![allow(clippy::useless_format)]
// Redundant closures in method calls – functional style preference.
#![allow(clippy::redundant_closure)]
// `if let` collapsing for readability – keep multi-level match arms.
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
// `match` with a single pattern + else – keep for symmetry with future arms.
#![allow(clippy::single_match_else)]

/// Structured access log (ring buffer, CLF formatting, summary statistics).
pub mod access_log;
/// Admin API: user management, audit log, stats, config, maintenance.
pub mod admin;
pub mod api;
/// API versioning registry (semantic versions, compatibility, deprecation).
pub mod api_versioning;
/// Append-only audit trail for security and compliance logging.
pub mod audit_trail;
pub mod auth;
/// Bearer / Basic / ApiKey authentication middleware with token registry.
pub mod auth_middleware;
/// HTTP response caching for the media server.
pub mod cache;
/// Circuit breaker pattern for protecting downstream services.
pub mod circuit_breaker;
/// Gzip HTTP response compression middleware (via `oxiarc-deflate`).
pub mod compression;
pub mod config;
/// Hierarchical server configuration loader with typed values and validation.
pub mod config_loader;
/// Connection pool statistics and monitoring.
pub mod conn_stats;
/// Connection pool registry (concurrency limits, per-IP counts, stale detection).
pub mod connection_pool;
pub mod db;
pub mod error;
pub mod handlers;
/// Health check and readiness monitoring (liveness/readiness probes, dependency checks).
pub mod health_monitor;
pub mod library;
/// HTTP middleware chain (logging, CORS, authentication, compression).
pub mod middleware;
pub mod models;
/// Rate limiting for the media server API.
pub mod rate_limit;
/// Token-bucket rate limiter with per-client tracking.
pub mod rate_limiter;
/// Ring-buffer HTTP request log with slow-request and error queries.
pub mod request_log;
/// Lightweight HTTP request router (method matching, pattern matching, route resolution).
pub mod request_router;
/// Composable HTTP request validation rules (content-type, body size, required headers).
pub mod request_validator;
/// Response-level caching with TTL, LRU eviction, and prefix invalidation.
pub mod response_cache;
/// Server-side session management.
pub mod session;
pub mod streaming;
pub mod upload;
pub mod websocket;
/// WebSocket frame parsing, ping/pong, connection lifecycle, and broadcast.
pub mod ws_handler;

// Streaming server modules
/// Batch operations (delete, transcode, metadata update) on multiple media items.
pub mod batch_ops;
pub mod cdn;
pub mod dash;
pub mod dvr;
pub mod hls;
/// Depacketizes Enhanced-RTMP ingest into elementary samples plus
/// container-ready decoder-configuration records.
pub mod ingest_depacketizer;
pub mod metrics;
/// Cursor-based pagination for stable, scalable API responses.
pub mod pagination;
pub mod record;
/// Request ID propagation for end-to-end request tracing.
pub mod request_id;
pub mod rtmp;
pub mod streaming_server;
/// W3C Trace Context distributed tracing (traceparent header).
pub mod tracing_ctx;
pub mod transcode;
/// Webhook delivery system for media and transcode events.
pub mod webhooks;

/// Background tasks module with persistent task queue for long-running operations.
pub mod background_tasks;
/// Header-based API content negotiation (Accept-Version) alongside URL versioning.
pub mod content_negotiation;
/// ETags and conditional GET (If-None-Match) for media metadata endpoints.
pub mod etag_cache;
/// Internal pub/sub event bus for decoupling handlers from side effects.
pub mod event_bus;
/// Lazy deserialization for large JSON request bodies.
pub mod lazy_deser;
/// Media processing pipeline (upload → analyze → transcode → notify).
pub mod media_pipeline;
/// Media proxy for external storage with caching.
pub mod media_proxy;
/// OAuth2/OIDC provider integration skeleton for SSO.
pub mod oauth2_provider;
/// Per-user storage and bandwidth quotas with enforcement.
pub mod quota_management;
/// Response streaming for large media file downloads.
pub mod response_streaming;
/// Server-Sent Events (SSE) endpoint support per RFC 6202.
pub mod sse;
/// Filmstrip-style thumbnail sprite generation for video scrubbing.
pub mod thumbnail_strip;
/// Circuit breaker integration with the transcoding backend.
pub mod transcode_circuit_breaker;

/// API gateway: rate limiting, throttling, and request routing for microservice architecture.
pub mod api_gateway;
/// Content delivery module with edge caching configuration for multi-region deployment.
pub mod content_delivery;
/// Database connection pool size tuning with CPU-aware sensible defaults.
pub mod db_pool_tuning;
/// GraphQL API module for flexible media queries alongside REST.
pub mod graphql;
/// HTTP/2 server push support for related resources.
pub mod http2_push;
/// Live SRT ingest module supporting SRT protocol alongside existing RTMP.
pub mod live_ingest;
/// Memory-mapped file I/O for efficient HLS/DASH segment serving.
pub mod mmap_segments;
/// Compact ring-buffer HTTP request log (alias for request_log).
pub mod req_log;
/// Graceful shutdown coordination for the media server.
pub mod shutdown;

pub use streaming_server::{StreamingServer, StreamingServerConfig};

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
    Router,
};
use std::{collections::VecDeque, sync::Arc};
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

pub use config::Config;
pub use error::{ServerError, ServerResult};

/// In-memory transcoding job queue for pending jobs.
///
/// Holds job IDs that are waiting to be processed. Workers dequeue
/// from the front; cancellation removes a job by ID.
pub struct JobQueue {
    /// Pending job IDs in FIFO order.
    pub pending: VecDeque<String>,
    /// Job IDs that have been requested to cancel while processing.
    pub cancelled: std::collections::HashSet<String>,
}

impl JobQueue {
    /// Creates a new empty job queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            cancelled: std::collections::HashSet::new(),
        }
    }

    /// Enqueues a job ID for processing.
    pub fn enqueue(&mut self, job_id: String) {
        self.pending.push_back(job_id);
    }

    /// Removes a job from the pending queue or marks it cancelled.
    ///
    /// Returns `true` if the job was found and removed/marked.
    pub fn cancel(&mut self, job_id: &str) -> bool {
        // Try to remove from pending queue first
        if let Some(pos) = self.pending.iter().position(|id| id == job_id) {
            self.pending.remove(pos);
            return true;
        }
        // If not in pending, mark as cancelled so a running worker stops
        self.cancelled.insert(job_id.to_string());
        true
    }

    /// Checks whether the given job has been cancelled (for in-progress workers).
    #[must_use]
    pub fn is_cancelled(&self, job_id: &str) -> bool {
        self.cancelled.contains(job_id)
    }

    /// Returns the number of pending jobs (read-only, safe for concurrent access).
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Returns `true` if the given job ID is in the pending queue (read-only).
    #[must_use]
    pub fn is_pending(&self, job_id: &str) -> bool {
        self.pending.iter().any(|id| id == job_id)
    }

    /// Returns a snapshot of all pending job IDs (read-only).
    #[must_use]
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending.iter().cloned().collect()
    }

    /// Dequeues the next pending job ID (FIFO order).
    pub fn dequeue(&mut self) -> Option<String> {
        self.pending.pop_front()
    }

    /// Removes a job ID from the cancelled set (after the worker has acknowledged it).
    pub fn clear_cancelled(&mut self, job_id: &str) -> bool {
        self.cancelled.remove(job_id)
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Main server instance.
///
/// Holds the application state including database connection pool,
/// authentication manager, and media library.
#[derive(Clone)]
pub struct Server {
    state: Arc<AppState>,
}

/// Application state shared across all handlers.
pub struct AppState {
    /// Database connection pool
    pub db: db::Database,
    /// Authentication manager
    pub auth: auth::AuthManager,
    /// Media library manager
    pub library: library::MediaLibrary,
    /// Server configuration
    pub config: Config,
    /// In-memory transcoding job queue (RwLock for concurrent read access to job status)
    pub job_queue: tokio::sync::RwLock<JobQueue>,
    /// Memory-mapped segment cache for zero-copy HLS/DASH segment serving.
    pub segment_store: Arc<mmap_segments::SegmentStore>,
}

impl Server {
    /// Creates a new server instance.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Database initialization fails
    /// - Required directories cannot be created
    /// - Configuration is invalid
    pub async fn new(config: Config) -> ServerResult<Self> {
        // Install the Pure-Rust `rustls-rustcrypto` crypto provider as the
        // process-wide default before any TLS-capable HTTP client (webhooks,
        // outbound API calls, ...) can be constructed. The workspace builds
        // `reqwest`/`rustls` without a compiled-in default provider, so this
        // is required or the first TLS use would panic. Idempotent — safe
        // even if the embedding binary already installed one.
        oximedia_net::install_default_crypto_provider();

        // Initialize database
        let db = db::Database::new(&config.database_url).await?;
        db.migrate().await?;

        // Create required directories
        std::fs::create_dir_all(&config.media_dir)?;
        std::fs::create_dir_all(&config.thumbnail_dir)?;
        std::fs::create_dir_all(&config.temp_dir)?;

        // Initialize components
        let auth = auth::AuthManager::new(&config.jwt_secret);
        let library = library::MediaLibrary::new(db.clone(), config.clone());

        let segment_store = Arc::new(mmap_segments::SegmentStore::new(
            mmap_segments::SegmentStoreConfig::default(),
        ));

        let state = Arc::new(AppState {
            db,
            auth,
            library,
            config,
            job_queue: tokio::sync::RwLock::new(JobQueue::new()),
            segment_store,
        });

        Ok(Self { state })
    }

    /// Builds the application router with all routes and middleware.
    #[allow(clippy::too_many_lines)]
    fn build_router(&self) -> Router {
        // API routes
        let api_routes = Router::new()
            // Authentication
            .route("/auth/register", post(api::auth::register))
            .route("/auth/login", post(api::auth::login))
            .route("/auth/refresh", post(api::auth::refresh_token))
            .route("/auth/logout", post(api::auth::logout))
            // User management
            .route("/users/me", get(api::users::get_current_user))
            .route("/users/me", post(api::users::update_current_user))
            .route("/users/me/password", post(api::users::change_password))
            .route("/users/me/api-keys", get(api::users::list_api_keys))
            .route("/users/me/api-keys", post(api::users::create_api_key))
            .route(
                "/users/me/api-keys/:key_id",
                post(api::users::revoke_api_key),
            )
            // Media upload and management
            .route("/media/upload", post(api::media::upload_file))
            .route(
                "/media/upload/multipart/init",
                post(api::media::init_multipart_upload),
            )
            .route(
                "/media/upload/multipart/:upload_id/part",
                post(api::media::upload_part),
            )
            .route(
                "/media/upload/multipart/:upload_id/complete",
                post(api::media::complete_multipart_upload),
            )
            .route(
                "/media/upload/multipart/:upload_id/abort",
                post(api::media::abort_multipart_upload),
            )
            .route("/media", get(api::media::list_media))
            .route("/media/:media_id", get(api::media::get_media))
            .route("/media/:media_id", post(api::media::update_media))
            .route(
                "/media/:media_id",
                axum::routing::delete(api::media::delete_media),
            )
            .route("/media/:media_id/metadata", get(api::media::get_metadata))
            .route("/media/:media_id/thumbnail", get(api::media::get_thumbnail))
            .route(
                "/media/:media_id/preview",
                get(api::media::generate_preview),
            )
            .route(
                "/media/:media_id/sprite",
                get(api::media::get_thumbnail_sprite),
            )
            // Transcoding jobs
            .route("/transcode", post(api::transcode::submit_job))
            .route("/transcode/:job_id", get(api::transcode::get_job))
            .route(
                "/transcode/:job_id/cancel",
                post(api::transcode::cancel_job),
            )
            .route(
                "/transcode/:job_id/status",
                get(api::transcode::get_job_status),
            )
            .route("/transcode", get(api::transcode::list_jobs))
            // Collections and playlists
            .route("/collections", get(api::collections::list_collections))
            .route("/collections", post(api::collections::create_collection))
            .route(
                "/collections/:collection_id",
                get(api::collections::get_collection),
            )
            .route(
                "/collections/:collection_id",
                post(api::collections::update_collection),
            )
            .route(
                "/collections/:collection_id",
                axum::routing::delete(api::collections::delete_collection),
            )
            .route(
                "/collections/:collection_id/items",
                post(api::collections::add_item),
            )
            .route(
                "/collections/:collection_id/items/:media_id",
                axum::routing::delete(api::collections::remove_item),
            )
            // Search
            .route("/search", get(api::search::search_media))
            .route("/search/suggest", get(api::search::suggest))
            // Statistics
            .route("/stats", get(api::stats::get_stats))
            .route("/stats/storage", get(api::stats::get_storage_stats))
            .route("/stats/bandwidth", get(api::stats::get_bandwidth_stats))
            // Batch operations
            .route("/media/batch/delete", post(batch_ops::batch_delete))
            .route("/media/batch/transcode", post(batch_ops::batch_transcode))
            .route(
                "/media/batch/update-metadata",
                post(batch_ops::batch_update_metadata),
            );

        // Admin routes (admin-only)
        let admin_routes = Router::new()
            .route("/admin/users", get(admin::list_users))
            .route("/admin/users/:id/role", put(admin::change_user_role))
            .route("/admin/users/:id", delete(admin::delete_user))
            .route("/admin/stats", get(admin::get_admin_stats))
            .route("/admin/audit", get(admin::get_audit_log))
            .route("/admin/config", get(admin::get_config))
            .route("/admin/maintenance/vacuum", post(admin::vacuum_db));

        // Streaming routes (no rate limiting for playback)
        let stream_routes = Router::new()
            .route(
                "/stream/:media_id/master.m3u8",
                get(streaming::handlers::serve_hls_master),
            )
            .route(
                "/stream/:media_id/:variant/playlist.m3u8",
                get(streaming::handlers::serve_hls_playlist),
            )
            .route(
                "/stream/:media_id/:variant/segment:segment.ts",
                get(streaming::handlers::serve_hls_segment),
            )
            .route(
                "/stream/:media_id/manifest.mpd",
                get(streaming::handlers::serve_dash_manifest),
            )
            .route(
                "/stream/:media_id/:variant/init.mp4",
                get(streaming::handlers::serve_dash_init),
            )
            .route(
                "/stream/:media_id/:variant/segment:segment.m4s",
                get(streaming::handlers::serve_dash_segment),
            )
            .route(
                "/stream/:media_id/progressive",
                get(streaming::handlers::serve_progressive),
            )
            .route(
                "/download/:media_id",
                get(streaming::handlers::serve_download),
            );

        // WebSocket route
        let ws_route = Router::new().route("/ws", get(websocket::handler::handle_websocket));

        // Webhook routes — use their own Arc<WebhookManager> as state
        let webhook_manager = Arc::new(webhooks::WebhookManager::new());
        let webhook_routes: Router = Router::new()
            .route("/webhooks", get(webhooks::list_webhooks))
            .route("/webhooks", post(webhooks::create_webhook))
            .route("/webhooks/:id", get(webhooks::get_webhook))
            .route("/webhooks/:id", put(webhooks::update_webhook))
            .route("/webhooks/:id", delete(webhooks::delete_webhook))
            .route(
                "/webhooks/:id/deliveries",
                get(webhooks::get_webhook_deliveries),
            )
            .with_state(webhook_manager);

        // Prometheus metrics endpoint — own Arc<ServerMetricsCollector> as state
        let metrics_collector = Arc::new(metrics::ServerMetricsCollector::new());
        let metrics_route: Router = Router::new()
            .route("/metrics", get(metrics::metrics_handler))
            .with_state(metrics_collector);

        // Apply AppState to all routes that need it, producing Router<()>.
        let api_v1: Router<()> = Router::new()
            .nest("/api/v1", api_routes)
            .nest("/api/v1", admin_routes)
            .nest("/api/v1", stream_routes)
            .nest("/api/v1", ws_route)
            .with_state(self.state.clone());

        // Health check routes (no state needed).
        let health_route: Router<()> = Router::new()
            .route("/health", get(handlers::health::health_check))
            .route("/ready", get(handlers::health::readiness_check))
            .with_state(self.state.clone());

        // Webhook sub-router (own state, already Router<()>).
        let webhook_api: Router<()> = Router::new().nest("/api/v1", webhook_routes);

        // Prometheus metrics (own state, already Router<()>).
        let metrics_api: Router<()> = Router::new().merge(metrics_route);

        // Merge all Router<()> routers together.
        Router::new()
            .merge(api_v1)
            .merge(health_route)
            .merge(webhook_api)
            .merge(metrics_api)
            .layer(
                ServiceBuilder::new()
                    // Tracing
                    .layer(
                        TraceLayer::new_for_http()
                            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                            .on_response(DefaultOnResponse::new().level(Level::INFO)),
                    )
                    // CORS
                    .layer(CorsLayer::permissive())
                    // Gzip response compression via oxiarc-deflate (pure Rust, no C deps).
                    // Compresses only compressible content-types (JSON/XML/text/*) and only
                    // when the client sends `Accept-Encoding: gzip`.  Binary media (video,
                    // audio, images) is skipped to avoid OOM-inducing body buffering.
                    .layer(compression::GzipLayer::new())
                    // Body size limit (configurable, default 5GB for chunked uploads)
                    .layer(DefaultBodyLimit::max(5 * 1024 * 1024 * 1024)),
            )
    }

    /// Starts the server on the specified address.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to bind to the address.
    pub async fn serve(self, addr: &str) -> ServerResult<()> {
        let listener = TcpListener::bind(addr).await?;
        tracing::info!("Server listening on {}", addr);

        let app = self.build_router();
        axum::serve(listener, app).await?;

        Ok(())
    }
}

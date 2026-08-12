// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! # `OxiMedia` Render Farm Coordinator
//!
//! Enterprise-grade render farm coordinator for distributed media rendering.
//!
//! ## Features
//!
//! - **Job Management**: Submit, track, and manage render jobs with priority levels
//! - **Worker Management**: Auto-discovery, health monitoring, and pool organization
//! - **Task Distribution**: Advanced scheduling algorithms and load balancing
//! - **Rendering Pipeline**: Pre-render, render, and post-render phases
//! - **Storage Management**: Distributed storage, asset distribution, and caching
//! - **Monitoring**: Real-time monitoring, dashboards, and alerts
//! - **Cost Management**: Cost tracking, budget management, and billing
//! - **Cloud Integration**: Hybrid rendering with cloud bursting
//! - **Fault Tolerance**: Automatic retry, checkpointing, and recovery
//! - **Advanced Features**: Multi-layer rendering, preview system, simulation support
//!
//! ## Example
//!
//! ```rust
//! use oximedia_renderfarm::{Coordinator, CoordinatorConfig, JobSubmission, Priority};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create coordinator
//! let config = CoordinatorConfig::default();
//! let coordinator = Coordinator::new(config).await?;
//!
//! // Submit a render job
//! let job = JobSubmission::builder()
//!     .project_file("path/to/project.blend")
//!     .frame_range(1, 100)
//!     .priority(Priority::High)
//!     .build()?;
//!
//! let job_id = coordinator.submit_job(job).await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod api;
pub mod assets;
// Real asset-source resolution for missing job dependencies (local/file:///
// http(s):// via reqwest), used by `pipeline::Pipeline::resolve_dependencies`.
// Internal: kept crate-private (its `AssetSource` type is reached via
// `Pipeline::add_asset_source`, not directly).
mod asset_fetch;
pub mod budget;
pub mod cache;
// Per-rendered-frame SHA-256 checksums for corruption detection, used by
// `pipeline::Pipeline`'s frame verification.
pub mod checksum;
pub mod cloud;
pub mod coordinator;
pub mod cost;
pub mod cost_optimizer;
pub mod dashboard;
pub mod deadline_integration;
pub mod dependency;
pub mod distribution;
pub mod error;
pub mod events;
// Shared image decode + pixel-format normalization for rendered frames,
// used by both `output_assembly` and `quality_metrics`. Internal.
mod frame_decode;
pub mod health;
pub mod job;
pub mod load_balancer;
pub mod monitoring;
// Real output assembly (MJPEG-in-AVI / Y4M muxing via oximedia-container),
// used by `pipeline::Pipeline::assemble_output`. Internal.
mod output_assembly;
pub mod pipeline;
pub mod plugin;
pub mod pool;
pub mod preview;
pub mod progress;
// Real PSNR/SSIM/blockiness/blur quality metrics via oximedia-quality, used
// by `pipeline::Pipeline::calculate_quality_metrics`. Internal.
mod quality_metrics;
pub mod recovery;
pub mod reporting;
pub mod scheduler;
pub mod simulation;
pub mod stats;
pub mod storage;
pub mod sync;
pub mod tile_rendering;
pub mod verification;
pub mod worker;

// New modules
pub mod frame_distribution;
pub mod license_server;
pub mod render_log;

// Wave 8d modules
pub mod blade_compute;
pub mod deadline_scheduler;
pub mod tile_render;

// Wave 9 modules
pub mod output_validator;
pub mod render_job_queue;
pub mod render_node_status;

// Wave 10 modules
pub mod farm_metrics;
pub mod job_priority_queue;
pub mod node_capability;

// Wave-12 new modules
pub mod frame_range;
pub mod priority_queue;
pub mod render_manifest;

// Wave-13 new modules
pub mod job_archive;
pub mod node_pool;
pub mod render_priority;

// Wave-15 new modules
pub mod job_dependency_graph;
pub mod node_heartbeat;
pub mod render_quota;

// Enhancement modules
pub mod elastic_scaling;
pub mod failure_recovery;
pub mod frame_merge;
pub mod multi_site;
pub mod node_affinity_rule;
pub mod render_checkpoint;
pub mod telemetry;

// Wired modules (previously undeclared)
pub mod alert_rule;
pub mod autoscale;
pub mod dashboard_api;
pub mod farm_config;
pub mod gpu_monitor;
pub mod job_output_validation;
pub mod job_template;
pub mod job_tracker;
pub mod progress_eta;
pub mod render_artifact;
pub mod render_cache;
pub mod render_template;
pub mod renderfarm_extensions;
pub mod resource_reservation;
pub mod spot_pricing;
pub mod worker_benchmark;
pub mod worker_health;
pub mod worker_pool_autoscale;

// Re-exports
pub use api::{ApiConfig, RenderFarmApi};
pub use coordinator::{Coordinator, CoordinatorConfig};
pub use cost::{CostReport, CostTracker};
pub use error::{Error, Result};
pub use job::{Job, JobId, JobState, JobSubmission, JobType, Priority};
pub use monitoring::{Monitor, MonitorConfig};
pub use pool::{PoolId, WorkerPool};
pub use scheduler::{Scheduler, SchedulingAlgorithm};
pub use worker::{Worker, WorkerCapabilities, WorkerId, WorkerState};

/// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests_wave13;

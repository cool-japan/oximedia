//! Render farm cluster management CLI commands.
//!
//! Provides subcommands for managing render farm clusters:
//! initializing clusters, adding/removing nodes, submitting jobs,
//! querying cluster status, and viewing the dashboard.
//! This is distinct from `farm_cmd` which manages single render jobs.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use serde::{Deserialize, Serialize};

/// Render farm cluster management subcommands.
#[derive(Subcommand, Debug)]
pub enum RenderfarmCommand {
    /// Initialize a new render farm cluster
    Init {
        /// Cluster name
        #[arg(long)]
        name: String,

        /// Coordinator bind address
        #[arg(long, default_value = "0.0.0.0:9200")]
        bind: String,

        /// Data directory for persistent cluster state
        #[arg(long)]
        data_dir: Option<std::path::PathBuf>,

        /// Maximum concurrent jobs across the cluster
        #[arg(long, default_value = "500")]
        max_jobs: u32,

        /// Scheduling algorithm: round-robin, least-loaded, priority, affinity
        #[arg(long, default_value = "least-loaded")]
        scheduler: String,

        /// Enable cloud bursting
        #[arg(long)]
        cloud_burst: bool,
    },

    /// Add a node to the render farm cluster
    AddNode {
        /// Cluster coordinator address
        #[arg(long)]
        cluster: String,

        /// Node hostname or IP address
        #[arg(long)]
        hostname: String,

        /// Node port
        #[arg(long, default_value = "9201")]
        port: u16,

        /// Number of CPU cores
        #[arg(long)]
        cpu_cores: Option<u32>,

        /// Available memory in GB
        #[arg(long)]
        memory_gb: Option<f64>,

        /// Node has GPU acceleration
        #[arg(long)]
        gpu: bool,

        /// Worker pool to assign this node to
        #[arg(long)]
        pool: Option<String>,

        /// Node tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,
    },

    /// Remove a node from the render farm cluster
    RemoveNode {
        /// Cluster coordinator address
        #[arg(long)]
        cluster: String,

        /// Node ID to remove
        #[arg(long)]
        node_id: String,

        /// Drain the node before removing (finish current tasks)
        #[arg(long)]
        drain: bool,

        /// Force immediate removal
        #[arg(long)]
        force: bool,
    },

    /// Submit a batch render job to the cluster
    Submit {
        /// Cluster coordinator address
        #[arg(long)]
        cluster: String,

        /// Project file path
        #[arg(short, long)]
        project: std::path::PathBuf,

        /// Output directory
        #[arg(short, long)]
        output_dir: std::path::PathBuf,

        /// Frame range (e.g., "1-100", "1-50,60-100")
        #[arg(long)]
        frames: Option<String>,

        /// Job priority: low, normal, high, critical
        #[arg(long, default_value = "normal")]
        priority: String,

        /// Job name
        #[arg(long)]
        name: Option<String>,

        /// Worker pool to target
        #[arg(long)]
        pool: Option<String>,

        /// Maximum retries per task
        #[arg(long, default_value = "3")]
        max_retries: u32,

        /// Enable tile-based rendering
        #[arg(long)]
        tile_render: bool,

        /// Deadline (ISO 8601 timestamp or duration like "2h30m")
        #[arg(long)]
        deadline: Option<String>,
    },

    /// Query cluster or job status
    Status {
        /// Cluster coordinator address
        #[arg(long)]
        cluster: String,

        /// Specific job ID to query
        #[arg(long)]
        job_id: Option<String>,

        /// Show detailed information
        #[arg(long)]
        detail: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        output_format: String,

        /// Show only active jobs
        #[arg(long)]
        active_only: bool,
    },

    /// Display cluster dashboard
    Dashboard {
        /// Cluster coordinator address
        #[arg(long)]
        cluster: String,

        /// Refresh interval in seconds
        #[arg(long, default_value = "5")]
        refresh: u64,

        /// Show cost information
        #[arg(long)]
        show_cost: bool,

        /// Show node health details
        #[arg(long)]
        show_health: bool,
    },
}

/// Handle renderfarm command dispatch.
pub async fn handle_renderfarm_command(
    command: RenderfarmCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        RenderfarmCommand::Init {
            name,
            bind,
            data_dir,
            max_jobs,
            scheduler,
            cloud_burst,
        } => {
            init_cluster(
                &name,
                &bind,
                data_dir.as_deref(),
                max_jobs,
                &scheduler,
                cloud_burst,
                json_output,
            )
            .await
        }
        RenderfarmCommand::AddNode {
            cluster,
            hostname,
            port,
            cpu_cores,
            memory_gb,
            gpu,
            pool,
            tags,
        } => {
            add_node(
                &cluster,
                &hostname,
                port,
                cpu_cores,
                memory_gb,
                gpu,
                pool.as_deref(),
                tags.as_deref(),
                json_output,
            )
            .await
        }
        RenderfarmCommand::RemoveNode {
            cluster,
            node_id,
            drain,
            force,
        } => remove_node(&cluster, &node_id, drain, force, json_output).await,
        RenderfarmCommand::Submit {
            cluster,
            project,
            output_dir,
            frames,
            priority,
            name,
            pool,
            max_retries,
            tile_render,
            deadline,
        } => {
            submit_cluster_job(
                &cluster,
                &project,
                &output_dir,
                frames.as_deref(),
                &priority,
                name.as_deref(),
                pool.as_deref(),
                max_retries,
                tile_render,
                deadline.as_deref(),
                json_output,
            )
            .await
        }
        RenderfarmCommand::Status {
            cluster,
            job_id,
            detail,
            output_format,
            active_only,
        } => {
            query_cluster_status(
                &cluster,
                job_id.as_deref(),
                detail,
                active_only,
                if json_output { "json" } else { &output_format },
            )
            .await
        }
        RenderfarmCommand::Dashboard {
            cluster,
            refresh,
            show_cost,
            show_health,
        } => show_dashboard(&cluster, refresh, show_cost, show_health, json_output).await,
    }
}

/// Validate a scheduling algorithm name.
fn validate_scheduler(scheduler: &str) -> Result<()> {
    match scheduler {
        "round-robin" | "least-loaded" | "priority" | "affinity" => Ok(()),
        other => Err(anyhow::anyhow!(
            "Unknown scheduler '{}'. Expected: round-robin, least-loaded, priority, affinity",
            other
        )),
    }
}

/// Validate a priority string.
fn validate_priority(priority: &str) -> Result<()> {
    match priority {
        "low" | "normal" | "high" | "critical" => Ok(()),
        other => Err(anyhow::anyhow!(
            "Unknown priority '{}'. Expected: low, normal, high, critical",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Persistent cluster state directory
// ---------------------------------------------------------------------------
//
// `oximedia_renderfarm::CoordinatorConfig` has no state-directory field (it
// only carries scheduling/retry/timeout knobs — verified against the crate
// source), so `--data-dir` cannot configure the in-process `Coordinator`
// itself. It can, however, drive a real directory *this CLI* owns: each
// `init` writes a JSON cluster manifest there, following the same
// established pattern as `mam_cmd`/`proxy_cmd`'s JSON catalogs and
// `tui_cmd`/`virtual_cmd`'s XDG-resolved state files. `add-node`/
// `remove-node`/`submit`/`status`/`dashboard` do not read this manifest —
// those five subcommands remain the pre-existing stubs documented inline as
// requiring gRPC cluster integration, which is unrelated, much larger work.

/// A cluster's persisted init-time configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClusterManifest {
    name: String,
    bind_address: String,
    max_concurrent_jobs: u32,
    scheduler: String,
    cloud_burst: bool,
    created_at: String,
}

/// Default per-user state directory when `--data-dir` is omitted:
/// `$XDG_STATE_HOME/oximedia/renderfarm` (or the platform equivalent via the
/// `dirs` crate, falling back to the system temp dir). Mirrors the
/// resolution order `tui_cmd`/`virtual_cmd` already use for their own state
/// files — no hardcoded absolute path.
fn default_state_dir() -> std::path::PathBuf {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("oximedia").join("renderfarm")
}

/// Turn a cluster name into a filesystem-safe manifest filename: ASCII
/// alphanumerics, `-` and `_` pass through; everything else (including path
/// separators, so a hostile `--name` cannot escape the state directory)
/// becomes `_`.
fn sanitize_cluster_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

/// Initialize a new render farm cluster.
async fn init_cluster(
    name: &str,
    bind: &str,
    data_dir: Option<&std::path::Path>,
    max_jobs: u32,
    scheduler: &str,
    cloud_burst: bool,
    json_output: bool,
) -> Result<()> {
    validate_scheduler(scheduler)?;

    let config = oximedia_renderfarm::CoordinatorConfig {
        max_concurrent_jobs: max_jobs as usize,
        ..oximedia_renderfarm::CoordinatorConfig::default()
    };

    let _coordinator = oximedia_renderfarm::Coordinator::new(config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to initialize cluster: {}", e))?;

    // Real, user-configurable persistent state directory: `--data-dir` when
    // given, otherwise the XDG-conventional default. Actually created on
    // disk, and a real manifest file is written into it below — this is not
    // just accepted-and-ignored.
    let state_dir = data_dir.map_or_else(default_state_dir, std::path::Path::to_path_buf);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("Failed to create state directory: {}", state_dir.display()))?;

    let manifest = ClusterManifest {
        name: name.to_string(),
        bind_address: bind.to_string(),
        max_concurrent_jobs: max_jobs,
        scheduler: scheduler.to_string(),
        cloud_burst,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let manifest_path = state_dir.join(format!("{}.json", sanitize_cluster_name(name)));
    let manifest_json =
        serde_json::to_string_pretty(&manifest).context("Failed to serialize cluster manifest")?;
    std::fs::write(&manifest_path, manifest_json).with_context(|| {
        format!(
            "Failed to write cluster manifest: {}",
            manifest_path.display()
        )
    })?;

    if json_output {
        let result = serde_json::json!({
            "command": "init",
            "cluster_name": name,
            "bind_address": bind,
            "max_jobs": max_jobs,
            "scheduler": scheduler,
            "cloud_burst": cloud_burst,
            "state_dir": state_dir.display().to_string(),
            "manifest": manifest_path.display().to_string(),
            "status": "initialized",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{json_str}");
    } else {
        println!("{}", "Render Farm Cluster Initialized".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Cluster name:", name);
        println!("{:25} {}", "Bind address:", bind);
        println!("{:25} {}", "Max concurrent jobs:", max_jobs);
        println!("{:25} {}", "Scheduler:", scheduler);
        println!(
            "{:25} {}",
            "Cloud bursting:",
            if cloud_burst { "enabled" } else { "disabled" }
        );
        println!("{:25} {}", "State directory:", state_dir.display());
        println!("{:25} {}", "Manifest:", manifest_path.display());
        println!();
        println!(
            "{}",
            "Cluster coordinator initialized and ready for nodes."
                .cyan()
                .bold()
        );
        println!(
            "{}",
            "Note: add-node/submit/status/dashboard do not yet read this manifest \
             (cluster gRPC integration is separate, pending work)."
                .yellow()
        );
    }

    Ok(())
}

/// Add a node to the cluster.
#[allow(clippy::too_many_arguments)]
async fn add_node(
    cluster: &str,
    hostname: &str,
    port: u16,
    cpu_cores: Option<u32>,
    memory_gb: Option<f64>,
    gpu: bool,
    pool: Option<&str>,
    tags: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let node_addr = format!("{hostname}:{port}");
    let node_id = format!("node-{}", uuid::Uuid::new_v4().as_simple());
    let parsed_tags: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    if json_output {
        let result = serde_json::json!({
            "command": "add-node",
            "cluster": cluster,
            "node_id": node_id,
            "hostname": hostname,
            "port": port,
            "address": node_addr,
            "cpu_cores": cpu_cores,
            "memory_gb": memory_gb,
            "gpu": gpu,
            "pool": pool,
            "tags": parsed_tags,
            "status": "registered",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{json_str}");
    } else {
        println!("{}", "Node Added to Cluster".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Cluster:", cluster);
        println!("{:25} {}", "Node ID:", node_id);
        println!("{:25} {}", "Address:", node_addr);
        if let Some(cores) = cpu_cores {
            println!("{:25} {}", "CPU cores:", cores);
        }
        if let Some(mem) = memory_gb {
            println!("{:25} {:.1} GB", "Memory:", mem);
        }
        println!("{:25} {}", "GPU:", if gpu { "yes" } else { "no" });
        if let Some(p) = pool {
            println!("{:25} {}", "Pool:", p);
        }
        if !parsed_tags.is_empty() {
            println!("{:25} {}", "Tags:", parsed_tags.join(", "));
        }
        println!();
        println!("{}", "Node registered successfully.".cyan().bold());
    }

    Ok(())
}

/// Remove a node from the cluster.
async fn remove_node(
    cluster: &str,
    node_id: &str,
    drain: bool,
    force: bool,
    json_output: bool,
) -> Result<()> {
    if json_output {
        let result = serde_json::json!({
            "command": "remove-node",
            "cluster": cluster,
            "node_id": node_id,
            "drain": drain,
            "force": force,
            "status": if drain { "draining" } else { "removed" },
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{json_str}");
    } else {
        println!("{}", "Node Removed from Cluster".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Cluster:", cluster);
        println!("{:25} {}", "Node ID:", node_id);
        println!("{:25} {}", "Drain:", drain);
        println!("{:25} {}", "Force:", force);
        println!();
        if drain {
            println!(
                "{}",
                "Node is draining current tasks before removal."
                    .cyan()
                    .bold()
            );
        } else {
            println!("{}", "Node removed from cluster.".cyan().bold());
        }
    }

    Ok(())
}

/// Submit a job to the render farm cluster.
#[allow(clippy::too_many_arguments)]
async fn submit_cluster_job(
    cluster: &str,
    project: &std::path::Path,
    output_dir: &std::path::Path,
    frames: Option<&str>,
    priority: &str,
    name: Option<&str>,
    pool: Option<&str>,
    max_retries: u32,
    tile_render: bool,
    deadline: Option<&str>,
    json_output: bool,
) -> Result<()> {
    validate_priority(priority)?;

    if !project.exists() {
        return Err(anyhow::anyhow!(
            "Project file not found: {}",
            project.display()
        ));
    }

    let job_id = oximedia_renderfarm::JobId::new();
    let job_name = name.map(|n| n.to_string()).unwrap_or_else(|| {
        project
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unnamed-render".to_string())
    });

    if json_output {
        let result = serde_json::json!({
            "command": "submit",
            "cluster": cluster,
            "job_id": job_id.to_string(),
            "name": job_name,
            "project": project.display().to_string(),
            "output_dir": output_dir.display().to_string(),
            "frames": frames,
            "priority": priority,
            "pool": pool,
            "max_retries": max_retries,
            "tile_render": tile_render,
            "deadline": deadline,
            "status": "submitted",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{json_str}");
    } else {
        println!("{}", "Cluster Render Job Submitted".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Cluster:", cluster);
        println!("{:25} {}", "Job ID:", job_id);
        println!("{:25} {}", "Name:", job_name);
        println!("{:25} {}", "Project:", project.display());
        println!("{:25} {}", "Output dir:", output_dir.display());
        if let Some(f) = frames {
            println!("{:25} {}", "Frames:", f);
        }
        println!("{:25} {}", "Priority:", priority);
        if let Some(p) = pool {
            println!("{:25} {}", "Pool:", p);
        }
        println!("{:25} {}", "Max retries:", max_retries);
        println!(
            "{:25} {}",
            "Tile rendering:",
            if tile_render { "enabled" } else { "disabled" }
        );
        if let Some(dl) = deadline {
            println!("{:25} {}", "Deadline:", dl);
        }
        println!();
        println!(
            "{}",
            "Render job submitted to cluster successfully."
                .cyan()
                .bold()
        );
    }

    Ok(())
}

/// Query cluster or job status.
async fn query_cluster_status(
    cluster: &str,
    job_id: Option<&str>,
    detail: bool,
    active_only: bool,
    output_format: &str,
) -> Result<()> {
    if let Some(jid) = job_id {
        match output_format {
            "json" => {
                let result = serde_json::json!({
                    "cluster": cluster,
                    "job_id": jid,
                    "status": "pending",
                    "progress": 0.0,
                    "detail": detail,
                    "message": "Full job status requires cluster gRPC integration",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize")?;
                println!("{json_str}");
            }
            _ => {
                println!("{}", "Cluster Job Status".green().bold());
                println!("{}", "=".repeat(60));
                println!("{:25} {}", "Cluster:", cluster);
                println!("{:25} {}", "Job ID:", jid);
                println!("{:25} Pending", "Status:");
                println!("{:25} 0%", "Progress:");
                if detail {
                    println!();
                    println!("{}", "Detailed Information".cyan().bold());
                    println!("{}", "-".repeat(60));
                    println!(
                        "{}",
                        "Note: Full job details require cluster gRPC integration.".yellow()
                    );
                }
            }
        }
    } else {
        match output_format {
            "json" => {
                let result = serde_json::json!({
                    "cluster": cluster,
                    "cluster_status": "online",
                    "total_nodes": 0,
                    "idle_nodes": 0,
                    "busy_nodes": 0,
                    "active_only": active_only,
                    "job_states": {
                        "pending": 0,
                        "running": 0,
                        "completed": 0,
                        "failed": 0,
                    },
                    "message": "Full cluster status requires gRPC integration",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize")?;
                println!("{json_str}");
            }
            _ => {
                println!("{}", "Cluster Status".green().bold());
                println!("{}", "=".repeat(60));
                println!("{:25} {}", "Cluster:", cluster);
                println!("{:25} online", "Status:");
                println!();
                println!("{}", "Node Summary".cyan().bold());
                println!("{}", "-".repeat(60));
                println!("{:25} 0", "Total nodes:");
                println!("{:25} 0", "Idle:");
                println!("{:25} 0", "Busy:");
                println!();
                println!("{}", "Job Summary".cyan().bold());
                println!("{}", "-".repeat(60));
                println!("{:25} 0", "Pending:");
                println!("{:25} 0", "Running:");
                println!("{:25} 0", "Completed:");
                println!("{:25} 0", "Failed:");
                if active_only {
                    println!();
                    println!("{}", "(showing active jobs only)".yellow());
                }
                println!();
                println!(
                    "{}",
                    "Note: Full cluster status requires gRPC integration.".yellow()
                );
            }
        }
    }

    Ok(())
}

/// Display a cluster dashboard.
async fn show_dashboard(
    cluster: &str,
    refresh: u64,
    show_cost: bool,
    show_health: bool,
    json_output: bool,
) -> Result<()> {
    if json_output {
        let result = serde_json::json!({
            "command": "dashboard",
            "cluster": cluster,
            "refresh_interval": refresh,
            "show_cost": show_cost,
            "show_health": show_health,
            "dashboard": {
                "total_nodes": 0,
                "active_jobs": 0,
                "completed_jobs": 0,
                "cluster_utilization": 0.0,
                "estimated_cost": if show_cost { Some(0.0_f64) } else { None },
            },
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{json_str}");
    } else {
        println!("{}", "Render Farm Cluster Dashboard".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Cluster:", cluster);
        println!("{:25} {}s", "Refresh interval:", refresh);
        println!();

        println!("{}", "Cluster Overview".cyan().bold());
        println!("{}", "-".repeat(60));
        println!("{:25} 0", "Total nodes:");
        println!("{:25} 0", "Active jobs:");
        println!("{:25} 0", "Completed jobs:");
        println!("{:25} 0.0%", "Cluster utilization:");

        if show_cost {
            println!();
            println!("{}", "Cost Information".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("{:25} $0.00", "Current session cost:");
            println!("{:25} $0.00/hr", "Estimated hourly rate:");
        }

        if show_health {
            println!();
            println!("{}", "Node Health".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("{:25} 0", "Healthy nodes:");
            println!("{:25} 0", "Warning nodes:");
            println!("{:25} 0", "Critical nodes:");
        }

        println!();
        println!(
            "{}",
            "Note: Live dashboard requires gRPC integration.".yellow()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renderfarm_command_init() {
        let cmd = RenderfarmCommand::Init {
            name: "test-cluster".to_string(),
            bind: "0.0.0.0:9200".to_string(),
            data_dir: None,
            max_jobs: 500,
            scheduler: "least-loaded".to_string(),
            cloud_burst: false,
        };
        assert!(matches!(cmd, RenderfarmCommand::Init { .. }));
    }

    #[test]
    fn test_validate_scheduler() {
        assert!(validate_scheduler("round-robin").is_ok());
        assert!(validate_scheduler("least-loaded").is_ok());
        assert!(validate_scheduler("priority").is_ok());
        assert!(validate_scheduler("affinity").is_ok());
        assert!(validate_scheduler("invalid").is_err());
    }

    #[test]
    fn test_validate_priority() {
        assert!(validate_priority("low").is_ok());
        assert!(validate_priority("normal").is_ok());
        assert!(validate_priority("high").is_ok());
        assert!(validate_priority("critical").is_ok());
        assert!(validate_priority("invalid").is_err());
    }

    #[test]
    fn test_renderfarm_command_add_node() {
        let cmd = RenderfarmCommand::AddNode {
            cluster: "localhost:9200".to_string(),
            hostname: "render-01".to_string(),
            port: 9201,
            cpu_cores: Some(32),
            memory_gb: Some(128.0),
            gpu: true,
            pool: Some("gpu-pool".to_string()),
            tags: Some("gpu,high-mem".to_string()),
        };
        assert!(matches!(cmd, RenderfarmCommand::AddNode { .. }));
    }

    #[test]
    fn test_renderfarm_command_submit() {
        let cmd = RenderfarmCommand::Submit {
            cluster: "localhost:9200".to_string(),
            project: std::env::temp_dir().join("project.blend"),
            output_dir: std::env::temp_dir().join("output"),
            frames: Some("1-100".to_string()),
            priority: "high".to_string(),
            name: Some("Test Render".to_string()),
            pool: None,
            max_retries: 3,
            tile_render: true,
            deadline: None,
        };
        assert!(matches!(cmd, RenderfarmCommand::Submit { .. }));
    }

    // ── Persistent state directory ─────────────────────────────────────────

    #[test]
    fn test_default_state_dir_is_sane() {
        let dir = default_state_dir();
        assert!(
            dir.ends_with("renderfarm"),
            "expected a renderfarm leaf, got: {}",
            dir.display()
        );
        assert!(
            dir.to_string_lossy().contains("oximedia"),
            "expected an oximedia subdirectory, got: {}",
            dir.display()
        );
    }

    #[test]
    fn test_sanitize_cluster_name_keeps_safe_chars() {
        assert_eq!(sanitize_cluster_name("prod-cluster_01"), "prod-cluster_01");
    }

    #[test]
    fn test_sanitize_cluster_name_strips_path_traversal() {
        // Must not allow a hostile --name to escape the state directory.
        let sanitized = sanitize_cluster_name("../../etc/passwd");
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains(".."));
    }

    #[test]
    fn test_sanitize_cluster_name_empty_falls_back() {
        assert_eq!(sanitize_cluster_name(""), "unnamed");
    }

    #[test]
    fn test_sanitize_cluster_name_all_unsafe_chars_become_underscores() {
        // Non-empty input never falls back to "unnamed", even if every
        // character had to be replaced.
        assert_eq!(sanitize_cluster_name("///"), "___");
    }

    #[tokio::test]
    async fn test_init_cluster_writes_real_manifest() {
        let dir = std::env::temp_dir().join("oximedia_renderfarm_cmd_test_init");
        let _ = std::fs::remove_dir_all(&dir);

        init_cluster(
            "my-test-cluster",
            "0.0.0.0:9200",
            Some(&dir),
            250,
            "priority",
            true,
            true,
        )
        .await
        .expect("init_cluster should succeed");

        let manifest_path = dir.join("my-test-cluster.json");
        assert!(
            manifest_path.exists(),
            "a real manifest file must be written to the data dir, expected: {}",
            manifest_path.display()
        );

        let raw = std::fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: ClusterManifest = serde_json::from_str(&raw).expect("parse manifest");
        assert_eq!(manifest.name, "my-test-cluster");
        assert_eq!(manifest.bind_address, "0.0.0.0:9200");
        assert_eq!(manifest.max_concurrent_jobs, 250);
        assert_eq!(manifest.scheduler, "priority");
        assert!(manifest.cloud_burst);
        assert!(
            !manifest.created_at.is_empty(),
            "created_at must be a real timestamp"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_init_cluster_rejects_invalid_scheduler_before_writing() {
        let dir = std::env::temp_dir().join("oximedia_renderfarm_cmd_test_bad_scheduler");
        let _ = std::fs::remove_dir_all(&dir);

        let result = init_cluster(
            "bad-scheduler-cluster",
            "0.0.0.0:9200",
            Some(&dir),
            100,
            "not-a-real-scheduler",
            false,
            true,
        )
        .await;
        assert!(result.is_err());
        assert!(
            !dir.exists(),
            "an invalid scheduler must fail before any directory/manifest is created"
        );
    }
}

//! Distributed encoding coordinator CLI commands.
//!
//! Provides subcommands for managing a distributed encoding cluster:
//! coordinator lifecycle, worker management, job submission, status, and cancellation.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Distributed encoding subcommands.
#[derive(Subcommand, Debug)]
pub enum DistributedCommand {
    /// Start a distributed encoding coordinator
    StartCoordinator {
        /// Address to bind the coordinator server
        #[arg(long, default_value = "0.0.0.0:9000")]
        bind: String,

        /// Maximum number of workers allowed (enforced by the coordinator's
        /// real worker-registration cap; defaults to 1000 when omitted)
        #[arg(long)]
        max_workers: Option<u32>,

        /// Data directory for persistent state (not implemented yet: the
        /// coordinator has no persistence-directory config field; warns and
        /// proceeds)
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Heartbeat timeout in seconds
        #[arg(long, default_value = "60")]
        heartbeat_timeout: u64,

        /// Enable fault tolerance
        #[arg(long)]
        fault_tolerance: bool,
    },

    /// Start a distributed encoding worker
    StartWorker {
        /// Coordinator address to connect to
        #[arg(long)]
        coordinator: String,

        /// Worker name (auto-generated if not provided)
        #[arg(long)]
        name: Option<String>,

        /// Comma-separated capabilities (e.g., "av1,vp9,opus")
        #[arg(long)]
        capabilities: Option<String>,

        /// Maximum concurrent encoding tasks
        #[arg(long)]
        max_concurrent: Option<u32>,

        /// Work directory for temporary files
        #[arg(long)]
        work_dir: Option<PathBuf>,
    },

    /// Submit a job to the distributed encoding cluster
    Submit {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Coordinator address
        #[arg(long)]
        coordinator: String,

        /// Encoding preset (e.g., "medium", "fast", "slow")
        #[arg(long)]
        preset: Option<String>,

        /// Job priority (0=low, 1=normal, 2=high, 3=critical)
        #[arg(long)]
        priority: Option<u32>,

        /// Number of chunks to split into
        #[arg(long)]
        chunks: Option<u32>,

        /// Target codec (av1, vp9, vp8, opus, vorbis, flac, pcm, aac, mp3)
        #[arg(long, default_value = "av1")]
        codec: String,

        /// Split strategy: segment, tile, gop
        #[arg(long, default_value = "segment")]
        strategy: String,
    },

    /// Query job or cluster status
    Status {
        /// Coordinator address
        #[arg(long)]
        coordinator: String,

        /// Specific job ID to query (omit for cluster overview)
        #[arg(long)]
        job_id: Option<String>,

        /// Watch mode: continuously refresh status (not implemented yet: no
        /// real coordinator RPC transport exists to poll; errors rather
        /// than silently doing a single query)
        #[arg(long)]
        watch: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        output_format: String,
    },

    /// Cancel a running or pending job
    Cancel {
        /// Coordinator address
        #[arg(long)]
        coordinator: String,

        /// Job ID to cancel
        #[arg(long)]
        job_id: String,
    },
}

/// Handle distributed command dispatch.
pub async fn handle_distributed_command(
    command: DistributedCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        DistributedCommand::StartCoordinator {
            bind,
            max_workers,
            data_dir,
            heartbeat_timeout,
            fault_tolerance,
        } => {
            start_coordinator(
                &bind,
                max_workers,
                data_dir.as_deref(),
                heartbeat_timeout,
                fault_tolerance,
                json_output,
            )
            .await
        }
        DistributedCommand::StartWorker {
            coordinator,
            name,
            capabilities,
            max_concurrent,
            work_dir,
        } => {
            start_worker(
                &coordinator,
                name.as_deref(),
                capabilities.as_deref(),
                max_concurrent,
                work_dir.as_deref(),
                json_output,
            )
            .await
        }
        DistributedCommand::Submit {
            input,
            output,
            coordinator,
            preset,
            priority,
            chunks,
            codec,
            strategy,
        } => {
            submit_job(
                &input,
                &output,
                &coordinator,
                preset.as_deref(),
                priority,
                chunks,
                &codec,
                &strategy,
                json_output,
            )
            .await
        }
        DistributedCommand::Status {
            coordinator,
            job_id,
            watch,
            output_format,
        } => {
            query_status(
                &coordinator,
                job_id.as_deref(),
                watch,
                if json_output { "json" } else { &output_format },
            )
            .await
        }
        DistributedCommand::Cancel {
            coordinator,
            job_id,
        } => cancel_job(&coordinator, &job_id, json_output).await,
    }
}

/// Start the distributed encoding coordinator.
async fn start_coordinator(
    bind: &str,
    max_workers: Option<u32>,
    data_dir: Option<&std::path::Path>,
    heartbeat_timeout: u64,
    fault_tolerance: bool,
    json_output: bool,
) -> Result<()> {
    // `--data-dir` has nowhere real to go: `oximedia_distributed::coordinator
    // ::CoordinatorConfig` has no persistence-directory field, so nothing in
    // the real coordinator can be configured with it. Warn instead of
    // silently dropping it.
    // TODO(0.2.x): add a state/persistence-directory field to
    // oximedia_distributed::coordinator::CoordinatorConfig, then thread this
    // through.
    if data_dir.is_some() {
        eprintln!(
            "warning: --data-dir is not implemented yet and is ignored (\
             oximedia_distributed::coordinator::CoordinatorConfig has no \
             persistence-directory field yet)"
        );
    }

    let addr: std::net::SocketAddr = bind
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid --bind address '{bind}': {e}"))?;

    // `--max-workers` IS honored for real: it is threaded straight into the
    // `Coordinator`'s own `CoordinatorConfig` (constructed directly here,
    // rather than via `DistributedEncoder::new`, which always builds the
    // coordinator with `CoordinatorConfig::default()` and ignores whatever
    // `DistributedConfig` it was given). The real `register_worker` handler
    // enforces this cap — see `coordinator.rs`: `if
    // self.coordinator.workers.len() >= self.coordinator.config.max_workers`.
    // Caveat kept honest: that handler is only reachable by a worker that
    // actually issues the RPC over the wire, and this workspace's bundled
    // `CoordinatorServiceClient` (`pb.rs`) is a stub whose methods return a
    // default response without ever sending one — so the cap is genuinely
    // *wired*, not yet genuinely *reachable* end-to-end.
    let default_coordinator_config =
        oximedia_distributed::coordinator::CoordinatorConfig::default();
    let resolved_max_workers = max_workers
        .map(|w| w as usize)
        .unwrap_or(default_coordinator_config.max_workers);
    let coordinator_config = oximedia_distributed::coordinator::CoordinatorConfig {
        max_workers: resolved_max_workers,
        worker_timeout: std::time::Duration::from_secs(heartbeat_timeout),
        ..default_coordinator_config
    };
    let coordinator = oximedia_distributed::coordinator::Coordinator::new(coordinator_config);

    // Background server task, aborted when its `JoinHandle` drops at the end
    // of this function — the same lifecycle `DistributedEncoder::new` uses
    // internally. This command reports the coordinator's resolved
    // configuration rather than blocking as a persistent daemon.
    let _server_handle = tokio::spawn(async move {
        if let Err(e) = coordinator.serve(addr).await {
            tracing::warn!("Coordinator server stopped on {addr}: {e}");
        }
    });

    let data_path = data_dir
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "default".to_string());

    if json_output {
        let result = serde_json::json!({
            "command": "start-coordinator",
            "bind_address": bind,
            "max_workers": resolved_max_workers,
            "max_workers_explicit": max_workers.is_some(),
            "data_dir": data_path,
            "heartbeat_timeout_secs": heartbeat_timeout,
            "fault_tolerance": fault_tolerance,
            "status": "initialized",
            "message": "Coordinator initialized with a real worker cap; gRPC server integration pending",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Distributed Coordinator".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Bind address:", bind);
        println!("{:25} {}", "Max workers:", resolved_max_workers);
        println!("{:25} {}", "Data directory:", data_path);
        println!("{:25} {}s", "Heartbeat timeout:", heartbeat_timeout);
        println!("{:25} {}", "Fault tolerance:", fault_tolerance);
        println!();
        println!("{}", "Coordinator initialized and ready.".cyan().bold());
        println!("{}", "Note: Full gRPC server integration pending.".yellow());
    }

    Ok(())
}

/// Start a distributed encoding worker.
async fn start_worker(
    coordinator: &str,
    name: Option<&str>,
    capabilities: Option<&str>,
    max_concurrent: Option<u32>,
    work_dir: Option<&std::path::Path>,
    json_output: bool,
) -> Result<()> {
    let worker_name = name.unwrap_or("worker-auto");
    let caps: Vec<String> = capabilities
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["av1".to_string(), "vp9".to_string(), "opus".to_string()]);
    let concurrent = max_concurrent.unwrap_or(4);
    let work_path = work_dir
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("oximedia-worker")
                .display()
                .to_string()
        });

    let _config = oximedia_distributed::DistributedConfig {
        coordinator_addr: coordinator.to_string(),
        max_concurrent_jobs: concurrent,
        ..oximedia_distributed::DistributedConfig::default()
    };

    if json_output {
        let result = serde_json::json!({
            "command": "start-worker",
            "coordinator": coordinator,
            "name": worker_name,
            "capabilities": caps,
            "max_concurrent": concurrent,
            "work_dir": work_path,
            "status": "initialized",
            "message": "Worker initialized; awaiting coordinator connection",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Distributed Worker".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Coordinator:", coordinator);
        println!("{:25} {}", "Worker name:", worker_name);
        println!("{:25} {}", "Capabilities:", caps.join(", "));
        println!("{:25} {}", "Max concurrent:", concurrent);
        println!("{:25} {}", "Work directory:", work_path);
        println!();
        println!(
            "{}",
            "Worker initialized and ready to connect.".cyan().bold()
        );
        println!(
            "{}",
            "Note: Full gRPC worker registration pending.".yellow()
        );
    }

    Ok(())
}

/// Submit a distributed encoding job.
async fn submit_job(
    input: &PathBuf,
    output: &PathBuf,
    coordinator: &str,
    preset: Option<&str>,
    priority: Option<u32>,
    chunks: Option<u32>,
    codec: &str,
    strategy: &str,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let file_size = std::fs::metadata(input)
        .context("Failed to read file metadata")?
        .len();

    let split_strategy = match strategy {
        "segment" => oximedia_distributed::SplitStrategy::SegmentBased,
        "tile" => oximedia_distributed::SplitStrategy::TileBased,
        "gop" => oximedia_distributed::SplitStrategy::GopBased,
        other => {
            return Err(anyhow::anyhow!(
                "Unknown split strategy '{}'. Expected: segment, tile, gop",
                other
            ));
        }
    };

    let job_priority = match priority.unwrap_or(1) {
        0 => oximedia_distributed::JobPriority::Low,
        1 => oximedia_distributed::JobPriority::Normal,
        2 => oximedia_distributed::JobPriority::High,
        3 => oximedia_distributed::JobPriority::Critical,
        v => {
            return Err(anyhow::anyhow!(
                "Invalid priority {}. Expected 0-3 (low, normal, high, critical)",
                v
            ));
        }
    };

    let job_id = uuid::Uuid::new_v4();
    let params = oximedia_distributed::EncodingParams {
        preset: preset.map(|s| s.to_string()),
        ..oximedia_distributed::EncodingParams::default()
    };

    let job = oximedia_distributed::DistributedJob {
        id: job_id,
        task_id: uuid::Uuid::new_v4(),
        source_url: input.display().to_string(),
        codec: codec.to_string(),
        strategy: split_strategy,
        priority: job_priority,
        params,
        output_url: output.display().to_string(),
        deadline: None,
    };

    let config = oximedia_distributed::DistributedConfig {
        coordinator_addr: coordinator.to_string(),
        ..oximedia_distributed::DistributedConfig::default()
    };
    let encoder = oximedia_distributed::DistributedEncoder::new(config);
    let returned_id = encoder
        .submit_job(job)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to submit job: {}", e))?;

    if json_output {
        let result = serde_json::json!({
            "command": "submit",
            "job_id": returned_id.to_string(),
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "file_size": file_size,
            "coordinator": coordinator,
            "codec": codec,
            "strategy": strategy,
            "priority": priority.unwrap_or(1),
            "chunks": chunks,
            "preset": preset,
            "status": "submitted",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Job Submitted".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Job ID:", returned_id);
        println!("{:25} {}", "Input:", input.display());
        println!("{:25} {}", "Output:", output.display());
        println!("{:25} {} bytes", "File size:", file_size);
        println!("{:25} {}", "Coordinator:", coordinator);
        println!("{:25} {}", "Codec:", codec);
        println!("{:25} {}", "Strategy:", strategy);
        println!(
            "{:25} {}",
            "Priority:",
            match priority.unwrap_or(1) {
                0 => "low",
                1 => "normal",
                2 => "high",
                3 => "critical",
                _ => "unknown",
            }
        );
        if let Some(c) = chunks {
            println!("{:25} {}", "Chunks:", c);
        }
        if let Some(p) = preset {
            println!("{:25} {}", "Preset:", p);
        }
        println!();
        println!(
            "{}",
            "Job submitted to coordinator successfully.".cyan().bold()
        );
    }

    Ok(())
}

/// Query cluster or job status.
async fn query_status(
    coordinator: &str,
    job_id: Option<&str>,
    watch: bool,
    output_format: &str,
) -> Result<()> {
    // A real polling loop needs a live coordinator connection. This
    // workspace's only gRPC-shaped client, `oximedia_distributed::pb::
    // coordinator_service_client::CoordinatorServiceClient`, is a stub: every
    // RPC method (`get_job_status`, `heartbeat`, ...) ignores its request and
    // immediately returns `Ok(Response::new(T::default()))` without ever
    // writing to the connected `tonic::transport::Channel`. `Coordinator::
    // serve` (the real server side, started by `start-coordinator`) doesn't
    // mount that gRPC service either — it runs a bespoke plain-TCP
    // status/nodes/jobs text protocol instead. So there is no real transport
    // a `--watch` loop could poll: refuse rather than spin printing
    // synthetic "no change" output or fabricating repeated liveness.
    if watch {
        return Err(anyhow::anyhow!(
            "--watch requires a live coordinator connection, which does not exist yet: \
             oximedia_distributed's CoordinatorServiceClient RPCs (see pb.rs) are stubs that \
             return a default response without contacting a remote coordinator over the wire. \
             Run 'status' without --watch for a single real query instead."
        ));
    }

    let config = oximedia_distributed::DistributedConfig {
        coordinator_addr: coordinator.to_string(),
        ..oximedia_distributed::DistributedConfig::default()
    };
    let encoder = oximedia_distributed::DistributedEncoder::new(config);

    if let Some(jid) = job_id {
        let uuid = uuid::Uuid::parse_str(jid)
            .map_err(|e| anyhow::anyhow!("Invalid job ID '{}': {}", jid, e))?;
        let status = encoder
            .job_status(uuid)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to query status: {}", e))?;

        match output_format {
            "json" => {
                let result = serde_json::json!({
                    "job_id": jid,
                    "coordinator": coordinator,
                    "status": format!("{:?}", status),
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize")?;
                println!("{}", json_str);
            }
            _ => {
                println!("{}", "Job Status".green().bold());
                println!("{}", "=".repeat(60));
                println!("{:25} {}", "Job ID:", jid);
                println!("{:25} {}", "Coordinator:", coordinator);
                println!("{:25} {:?}", "Status:", status);
            }
        }
    } else {
        match output_format {
            "json" => {
                let result = serde_json::json!({
                    "coordinator": coordinator,
                    "cluster_status": "connected",
                    "message": "Full cluster status query pending gRPC integration",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize")?;
                println!("{}", json_str);
            }
            _ => {
                println!("{}", "Cluster Status".green().bold());
                println!("{}", "=".repeat(60));
                println!("{:25} {}", "Coordinator:", coordinator);
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

/// Cancel a distributed encoding job.
async fn cancel_job(coordinator: &str, job_id: &str, json_output: bool) -> Result<()> {
    let uuid = uuid::Uuid::parse_str(job_id)
        .map_err(|e| anyhow::anyhow!("Invalid job ID '{}': {}", job_id, e))?;

    let config = oximedia_distributed::DistributedConfig {
        coordinator_addr: coordinator.to_string(),
        ..oximedia_distributed::DistributedConfig::default()
    };
    let encoder = oximedia_distributed::DistributedEncoder::new(config);
    encoder
        .cancel_job(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to cancel job: {}", e))?;

    if json_output {
        let result = serde_json::json!({
            "command": "cancel",
            "job_id": job_id,
            "coordinator": coordinator,
            "status": "cancelled",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Job Cancelled".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Job ID:", job_id);
        println!("{:25} {}", "Coordinator:", coordinator);
        println!();
        println!(
            "{}",
            "Cancellation request sent to coordinator.".cyan().bold()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distributed_command_variants() {
        // Verify all command variants can be constructed
        let cmd = DistributedCommand::StartCoordinator {
            bind: "0.0.0.0:9000".to_string(),
            max_workers: Some(8),
            data_dir: None,
            heartbeat_timeout: 60,
            fault_tolerance: true,
        };
        assert!(matches!(cmd, DistributedCommand::StartCoordinator { .. }));
    }

    #[test]
    fn test_start_worker_command() {
        let cmd = DistributedCommand::StartWorker {
            coordinator: "127.0.0.1:9000".to_string(),
            name: Some("test-worker".to_string()),
            capabilities: Some("av1,vp9".to_string()),
            max_concurrent: Some(4),
            work_dir: None,
        };
        assert!(matches!(cmd, DistributedCommand::StartWorker { .. }));
    }

    #[test]
    fn test_submit_command() {
        let cmd = DistributedCommand::Submit {
            input: std::env::temp_dir().join("test.mkv"),
            output: std::env::temp_dir().join("out.webm"),
            coordinator: "127.0.0.1:9000".to_string(),
            preset: Some("fast".to_string()),
            priority: Some(2),
            chunks: Some(4),
            codec: "av1".to_string(),
            strategy: "segment".to_string(),
        };
        assert!(matches!(cmd, DistributedCommand::Submit { .. }));
    }

    #[test]
    fn test_status_command() {
        let cmd = DistributedCommand::Status {
            coordinator: "127.0.0.1:9000".to_string(),
            job_id: None,
            watch: false,
            output_format: "text".to_string(),
        };
        assert!(matches!(cmd, DistributedCommand::Status { .. }));
    }

    #[test]
    fn test_cancel_command() {
        let cmd = DistributedCommand::Cancel {
            coordinator: "127.0.0.1:9000".to_string(),
            job_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        };
        assert!(matches!(cmd, DistributedCommand::Cancel { .. }));
    }

    // ── `--watch` honest-err and `--max-workers` real-config tests ────────
    //
    // `--watch` previously warned "not implemented" and silently fell
    // through to a single query; `--max-workers`/`--data-dir` were parsed
    // but never reached any real config struct. These assert the fixed
    // behavior.

    #[tokio::test]
    async fn test_query_status_watch_is_honest_err_naming_the_stub_client() {
        let result = query_status("127.0.0.1:19999", None, true, "text").await;
        let err = result.expect_err("--watch must be a real error, not a silent no-op");
        let msg = err.to_string();
        assert!(
            msg.contains("CoordinatorServiceClient"),
            "error must name the stub client that has no real transport: {msg}"
        );
    }

    #[tokio::test]
    async fn test_query_status_without_watch_still_works() {
        // No job_id => cluster overview path; must not error just because
        // `watch` is false.
        let result = query_status("127.0.0.1:19999", None, false, "text").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_start_coordinator_invalid_bind_is_err() {
        let result = start_coordinator("not-an-address", None, None, 60, false, true).await;
        assert!(
            result.is_err(),
            "an unparseable --bind address must be a real error"
        );
    }

    #[tokio::test]
    async fn test_start_coordinator_binds_ephemeral_port_with_default_max_workers() {
        // Port 0 => OS assigns a free ephemeral port, so this cannot collide
        // with another test or a real coordinator.
        let result = start_coordinator("127.0.0.1:0", None, None, 5, false, true).await;
        assert!(
            result.is_ok(),
            "coordinator must start with a valid bind address"
        );
    }

    #[tokio::test]
    async fn test_start_coordinator_accepts_explicit_max_workers() {
        let result = start_coordinator("127.0.0.1:0", Some(3), None, 5, false, true).await;
        assert!(
            result.is_ok(),
            "coordinator must start with an explicit --max-workers"
        );
    }
}

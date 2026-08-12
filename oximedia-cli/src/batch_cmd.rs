//! Batch engine subcommand: submit, status, list, cancel, report.
//!
//! Provides the `oximedia batch-engine` subcommand family using
//! `oximedia_batch::{BatchEngine, BatchJob, JobId, JobState}` backed by an
//! SQLite database (the `sqlite` feature of `oximedia-batch` is required and
//! is always enabled in this crate).
//!
//! Note: The existing `oximedia batch` command (in `batch.rs`) is a simple
//! file-to-file converter. This command exposes the full production batch
//! engine with persistent, database-backed job queuing.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

// Default database path used when `--db` is not specified
const DEFAULT_DB: &str = "oximedia_batch.db";

// ---------------------------------------------------------------------------
// Subcommand enum
// ---------------------------------------------------------------------------

/// Subcommands for `oximedia batch-engine`.
#[derive(Subcommand, Debug)]
pub enum BatchEngineCommand {
    /// Submit a new batch job from a JSON configuration file
    Submit {
        /// JSON file describing the job (name, operation, inputs, outputs)
        #[arg(long)]
        config: PathBuf,

        /// Job priority
        #[arg(long, default_value = "normal",
              value_parser = ["high", "normal", "low"])]
        priority: String,

        /// Path to the SQLite database file
        #[arg(long, default_value = DEFAULT_DB)]
        db: PathBuf,
    },

    /// Show status for a specific job
    Status {
        /// Job ID to query
        #[arg(long)]
        id: String,

        /// Path to the SQLite database file
        #[arg(long, default_value = DEFAULT_DB)]
        db: PathBuf,
    },

    /// List jobs, optionally filtered by state
    List {
        /// Filter by state
        #[arg(long, value_parser = ["pending", "running", "done", "failed", "all"],
              default_value = "all")]
        state: String,

        /// Path to the SQLite database file
        #[arg(long, default_value = DEFAULT_DB)]
        db: PathBuf,
    },

    /// Cancel a running or queued job
    Cancel {
        /// Job ID to cancel
        #[arg(long)]
        id: String,

        /// Path to the SQLite database file
        #[arg(long, default_value = DEFAULT_DB)]
        db: PathBuf,
    },

    /// Generate a summary report for all jobs in the database
    Report {
        /// Path to the SQLite database file
        #[arg(long, default_value = DEFAULT_DB)]
        db: PathBuf,

        /// Output format
        #[arg(long, default_value = "text",
              value_parser = ["text", "json"])]
        format: String,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point called from `main.rs`.
pub async fn run_batch_engine(command: BatchEngineCommand, json_output: bool) -> Result<()> {
    match command {
        BatchEngineCommand::Submit {
            config,
            priority,
            db,
        } => cmd_submit(&config, &priority, &db, json_output).await,
        BatchEngineCommand::Status { id, db } => cmd_status(&id, &db, json_output).await,
        BatchEngineCommand::List { state, db } => cmd_list(&state, &db, json_output).await,
        BatchEngineCommand::Cancel { id, db } => cmd_cancel(&id, &db, json_output).await,
        BatchEngineCommand::Report { db, format } => {
            let fmt = if json_output { "json" } else { &format };
            cmd_report(&db, fmt).await
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a priority string into `oximedia_batch::Priority`.
fn parse_priority(s: &str) -> oximedia_batch::Priority {
    match s {
        "high" => oximedia_batch::Priority::High,
        "low" => oximedia_batch::Priority::Low,
        _ => oximedia_batch::Priority::Normal,
    }
}

/// Parse the `operation` field of a submit config into a real
/// [`oximedia_batch::BatchOperation`].
///
/// Two forms are accepted:
///
/// - a shorthand string — `"copy"`, `"move"`, `"transcode"`,
///   `"quality-check"` — mapped onto the corresponding variant (transcode
///   reads an optional top-level `"preset"`, quality-check an optional
///   `"profile"`, copy/move an optional `"overwrite"` bool);
/// - a full serde object in `BatchOperation`'s external-tag form, e.g.
///   `{"Transcode": {"preset": "web"}}`, passed straight to serde so every
///   variant (Analyze, Custom, Pipeline, ...) is reachable.
///
/// A missing `operation` field defaults to a copy file-op (documented in the
/// submit help text); an unrecognized string is an error listing the
/// supported shorthands.
fn parse_operation(config: &serde_json::Value) -> Result<oximedia_batch::BatchOperation> {
    use oximedia_batch::operations::FileOperation;
    use oximedia_batch::BatchOperation;

    let overwrite = config
        .get("overwrite")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let Some(op) = config.get("operation") else {
        return Ok(BatchOperation::FileOp {
            operation: FileOperation::Copy { overwrite },
        });
    };

    if let Some(s) = op.as_str() {
        return match s.to_lowercase().as_str() {
            "copy" => Ok(BatchOperation::FileOp {
                operation: FileOperation::Copy { overwrite },
            }),
            "move" => Ok(BatchOperation::FileOp {
                operation: FileOperation::Move { overwrite },
            }),
            "transcode" => Ok(BatchOperation::Transcode {
                preset: config
                    .get("preset")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("default")
                    .to_string(),
            }),
            "quality-check" | "qc" => Ok(BatchOperation::QualityCheck {
                profile: config
                    .get("profile")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("default")
                    .to_string(),
            }),
            other => Err(anyhow::anyhow!(
                "Unknown operation '{other}' in job config. Supported shorthands: copy, move, \
                 transcode, quality-check; or pass a full BatchOperation object, e.g. \
                 {{\"Transcode\": {{\"preset\": \"web\"}}}}"
            )),
        };
    }

    serde_json::from_value::<BatchOperation>(op.clone())
        .context("Config field 'operation' is neither a known shorthand string nor a valid BatchOperation object")
}

/// Open (or create) a `BatchEngine` backed by the given SQLite database.
fn open_engine(db: &PathBuf) -> Result<oximedia_batch::BatchEngine> {
    let db_str = db
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Database path contains non-UTF-8 characters"))?;
    // Use 4 workers as a reasonable default for the CLI; for high-throughput usage
    // the engine can be configured via a config file.
    oximedia_batch::BatchEngine::new(db_str, 4)
        .map_err(|e| anyhow::anyhow!("Failed to open batch database '{}': {e}", db_str))
}

// ---------------------------------------------------------------------------
// Submit
// ---------------------------------------------------------------------------

async fn cmd_submit(
    config: &PathBuf,
    priority_str: &str,
    db: &PathBuf,
    json_output: bool,
) -> Result<()> {
    if !config.exists() {
        return Err(anyhow::anyhow!(
            "Config file not found: {}",
            config.display()
        ));
    }

    let raw = std::fs::read_to_string(config)
        .with_context(|| format!("Cannot read config: {}", config.display()))?;

    // Parse the job definition: { "name", "operation", "inputs", "outputs" }
    // (operation/inputs/outputs optional; see parse_operation for the
    // accepted operation forms).
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).context("Config file is not valid JSON")?;

    let job_name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed-job")
        .to_string();

    let operation = parse_operation(&parsed)?;

    // Build the job from the parsed config; --priority is genuinely applied
    // (visible later in `list`/`report`, and used by the queue ordering).
    let mut job = oximedia_batch::BatchJob::new(job_name.clone(), operation);
    job.set_priority(parse_priority(priority_str));

    if let Some(inputs) = parsed.get("inputs") {
        job.inputs = serde_json::from_value(inputs.clone())
            .context("Config field 'inputs' is not a valid InputSpec array")?;
    }
    if let Some(outputs) = parsed.get("outputs") {
        job.outputs = serde_json::from_value(outputs.clone())
            .context("Config field 'outputs' is not a valid OutputSpec array")?;
    }

    let engine = open_engine(db)?;
    let submitted_id = engine
        .submit_job(job)
        .await
        .map_err(|e| anyhow::anyhow!("Submit failed: {e}"))?;

    if json_output {
        let obj = serde_json::json!({
            "command": "batch-engine submit",
            "job_id": submitted_id.as_str(),
            "name": job_name,
            "priority": priority_str,
            "db": db.display().to_string(),
            "status": "queued",
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization failed")?
        );
    } else if !crate::progress::is_quiet() {
        println!("{}", "Job Submitted".green().bold());
        println!("{:20} {}", "Job ID:", submitted_id.as_str().cyan());
        println!("{:20} {}", "Name:", job_name);
        println!("{:20} {}", "Priority:", priority_str);
        println!("{:20} {}", "Database:", db.display());
        println!("{:20} {}", "Status:", "Queued".yellow());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

async fn cmd_status(id: &str, db: &PathBuf, json_output: bool) -> Result<()> {
    let engine = open_engine(db)?;
    let job_id = oximedia_batch::JobId::from(id);
    let state = engine
        .get_job_status(&job_id)
        .await
        .map_err(|e| anyhow::anyhow!("Status query failed: {e}"))?;

    if json_output {
        let obj = serde_json::json!({
            "command": "batch-engine status",
            "job_id": id,
            "state": state.to_string(),
            "db": db.display().to_string(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
    } else {
        println!("{}", "Job Status".green().bold());
        println!("{:20} {}", "Job ID:", id.cyan());
        println!("{:20} {}", "State:", state.to_string().yellow());
        println!("{:20} {}", "Database:", db.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// Whether a persisted status string matches the CLI `--state` filter value.
///
/// The database stores `Queued`/`Pending`/`Running`/`Completed`/`Failed`/
/// `Cancelled`; the CLI groups `Queued`+`Pending` under `pending`. Cancelled
/// jobs only appear under `all`.
fn state_matches(filter: &str, status: &str) -> bool {
    match filter {
        "all" => true,
        "pending" => matches!(status, "Queued" | "Pending"),
        "running" => status == "Running",
        "done" => status == "Completed",
        "failed" => status == "Failed",
        _ => true,
    }
}

async fn cmd_list(state_filter: &str, db: &PathBuf, json_output: bool) -> Result<()> {
    let engine = open_engine(db)?;
    let jobs = engine
        .list_jobs()
        .map_err(|e| anyhow::anyhow!("List failed: {e}"))?;

    // Resolve each job's persisted state from the database so the --state
    // filter operates on real data (the in-memory queue state is empty in a
    // fresh CLI process).
    let database = engine.database();
    let jobs_with_state: Vec<(oximedia_batch::BatchJob, String)> = jobs
        .into_iter()
        .map(|j| {
            let state = database
                .get_job_status_string(&j.id)
                .unwrap_or_else(|_| "unknown".to_string());
            (j, state)
        })
        .filter(|(_, state)| state_matches(state_filter, state))
        .collect();

    if json_output {
        let list: Vec<serde_json::Value> = jobs_with_state
            .iter()
            .map(|(j, state)| {
                serde_json::json!({
                    "id": j.id.as_str(),
                    "name": j.name,
                    "priority": j.priority.to_string(),
                    "state": state,
                })
            })
            .collect();
        let obj = serde_json::json!({
            "command": "batch-engine list",
            "filter": state_filter,
            "count": list.len(),
            "jobs": list,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
    } else {
        println!("{}", "Batch Jobs".green().bold());
        println!("{}", "=".repeat(82));
        if jobs_with_state.is_empty() {
            if state_filter == "all" {
                println!("  No jobs found.");
            } else {
                println!("  No jobs found in state '{state_filter}'.");
            }
        } else {
            println!("{:<40} {:<20} {:<10} Priority", "Job ID", "Name", "State");
            println!("{}", "-".repeat(82));
            for (j, state) in &jobs_with_state {
                println!(
                    "{:<40} {:<20} {:<10} {}",
                    j.id.as_str(),
                    j.name,
                    state,
                    j.priority
                );
            }
        }
        println!();
        println!(
            "Total: {} jobs{}",
            jobs_with_state.len(),
            if state_filter == "all" {
                String::new()
            } else {
                format!(" (filtered by state: {state_filter})")
            }
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------

async fn cmd_cancel(id: &str, db: &PathBuf, json_output: bool) -> Result<()> {
    let engine = open_engine(db)?;
    let job_id = oximedia_batch::JobId::from(id);
    engine
        .cancel_job(&job_id)
        .await
        .map_err(|e| anyhow::anyhow!("Cancel failed: {e}"))?;

    if json_output {
        let obj = serde_json::json!({
            "command": "batch-engine cancel",
            "job_id": id,
            "status": "cancelled",
            "db": db.display().to_string(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
    } else if !crate::progress::is_quiet() {
        println!("{}", "Job Cancelled".green().bold());
        println!("{:20} {}", "Job ID:", id.cyan());
        println!("{:20} {}", "Database:", db.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

async fn cmd_report(db: &PathBuf, output_format: &str) -> Result<()> {
    let engine = open_engine(db)?;
    let jobs = engine
        .list_jobs()
        .map_err(|e| anyhow::anyhow!("Report failed: {e}"))?;

    let total = jobs.len();
    let high_priority = jobs
        .iter()
        .filter(|j| j.priority == oximedia_batch::Priority::High)
        .count();
    let normal_priority = jobs
        .iter()
        .filter(|j| j.priority == oximedia_batch::Priority::Normal)
        .count();
    let low_priority = jobs
        .iter()
        .filter(|j| j.priority == oximedia_batch::Priority::Low)
        .count();

    if output_format == "json" {
        let obj = serde_json::json!({
            "command": "batch-engine report",
            "db": db.display().to_string(),
            "total": total,
            "high_priority": high_priority,
            "normal_priority": normal_priority,
            "low_priority": low_priority,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
    } else {
        println!("{}", "Batch Engine Report".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Database:", db.display());
        println!("{:20} {}", "Total jobs:", total);
        println!(
            "{:20} {}",
            "High priority:",
            high_priority.to_string().red()
        );
        println!(
            "{:20} {}",
            "Normal priority:",
            normal_priority.to_string().yellow()
        );
        println!(
            "{:20} {}",
            "Low priority:",
            low_priority.to_string().dimmed()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_priority_variants() {
        assert_eq!(parse_priority("high"), oximedia_batch::Priority::High);
        assert_eq!(parse_priority("normal"), oximedia_batch::Priority::Normal);
        assert_eq!(parse_priority("low"), oximedia_batch::Priority::Low);
        assert_eq!(parse_priority("unknown"), oximedia_batch::Priority::Normal);
    }

    #[test]
    fn test_parse_operation_shorthands() {
        use oximedia_batch::operations::FileOperation;
        use oximedia_batch::BatchOperation;

        let cfg = serde_json::json!({"operation": "transcode", "preset": "web"});
        match parse_operation(&cfg).expect("transcode shorthand") {
            BatchOperation::Transcode { preset } => assert_eq!(preset, "web"),
            other => panic!("expected Transcode, got {other:?}"),
        }

        let cfg = serde_json::json!({"operation": "copy", "overwrite": true});
        match parse_operation(&cfg).expect("copy shorthand") {
            BatchOperation::FileOp {
                operation: FileOperation::Copy { overwrite },
            } => assert!(overwrite),
            other => panic!("expected FileOp Copy, got {other:?}"),
        }

        let cfg = serde_json::json!({"operation": "quality-check", "profile": "broadcast"});
        match parse_operation(&cfg).expect("qc shorthand") {
            BatchOperation::QualityCheck { profile } => assert_eq!(profile, "broadcast"),
            other => panic!("expected QualityCheck, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_operation_object_form_and_default() {
        use oximedia_batch::operations::FileOperation;
        use oximedia_batch::BatchOperation;

        // Full serde external-tag object form.
        let cfg = serde_json::json!({"operation": {"Transcode": {"preset": "archive"}}});
        match parse_operation(&cfg).expect("object form") {
            BatchOperation::Transcode { preset } => assert_eq!(preset, "archive"),
            other => panic!("expected Transcode, got {other:?}"),
        }

        // Missing operation defaults to a copy file-op.
        let cfg = serde_json::json!({"name": "x"});
        match parse_operation(&cfg).expect("default") {
            BatchOperation::FileOp {
                operation: FileOperation::Copy { overwrite },
            } => assert!(!overwrite),
            other => panic!("expected FileOp Copy default, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_operation_rejects_unknown_string() {
        let cfg = serde_json::json!({"operation": "frobnicate"});
        let msg = parse_operation(&cfg)
            .expect_err("unknown shorthand must fail")
            .to_string();
        assert!(msg.contains("frobnicate"), "must name the input: {msg}");
        assert!(msg.contains("transcode"), "must list supported: {msg}");
    }

    #[test]
    fn test_state_matches_groups() {
        assert!(state_matches("all", "Cancelled"));
        assert!(state_matches("pending", "Queued"));
        assert!(state_matches("pending", "Pending"));
        assert!(!state_matches("pending", "Running"));
        assert!(state_matches("running", "Running"));
        assert!(state_matches("done", "Completed"));
        assert!(state_matches("failed", "Failed"));
        assert!(!state_matches("failed", "Cancelled"));
        assert!(!state_matches("done", "Failed"));
    }

    #[tokio::test]
    async fn test_submit_missing_config() {
        let cfg = std::env::temp_dir().join("oximedia_batch_missing_config.json");
        let db = std::env::temp_dir().join("oximedia_batch_test_missing.db");
        let result = cmd_submit(&cfg, "normal", &db, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_submit_invalid_json() {
        let dir = std::env::temp_dir();
        let cfg = dir.join("oximedia_batch_bad.json");
        std::fs::write(&cfg, b"not json").expect("write stub");
        let db = dir.join("oximedia_batch_test.db");
        let result = cmd_submit(&cfg, "normal", &db, false).await;
        assert!(result.is_err());
        std::fs::remove_file(&cfg).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_submit_valid_json() {
        let dir = std::env::temp_dir();
        let cfg = dir.join("oximedia_batch_submit_ok.json");
        std::fs::write(&cfg, br#"{"name":"test-job","operation":"transcode"}"#)
            .expect("write stub");
        let db = dir.join("oximedia_batch_submit_test.db");
        let result = cmd_submit(&cfg, "high", &db, true).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&cfg).ok();
        std::fs::remove_file(&db).ok();
    }

    /// End-to-end proof that `--priority` and the config's `operation` are
    /// genuinely persisted: submit with high priority, then read the job back
    /// from the same database and inspect it.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_submit_persists_priority_and_operation() {
        let dir = std::env::temp_dir();
        let cfg = dir.join("oximedia_batch_submit_prio.json");
        std::fs::write(
            &cfg,
            br#"{"name":"prio-job","operation":"transcode","preset":"web"}"#,
        )
        .expect("write cfg");
        let db = dir.join("oximedia_batch_submit_prio.db");
        std::fs::remove_file(&db).ok();

        cmd_submit(&cfg, "high", &db, true)
            .await
            .expect("submit must succeed");

        let engine = open_engine(&db).expect("reopen engine");
        let jobs = engine.list_jobs().expect("list jobs");
        let job = jobs
            .iter()
            .find(|j| j.name == "prio-job")
            .expect("submitted job must be persisted");
        assert_eq!(
            job.priority,
            oximedia_batch::Priority::High,
            "--priority high must be stored on the job"
        );
        match &job.operation {
            oximedia_batch::BatchOperation::Transcode { preset } => {
                assert_eq!(preset, "web", "config preset must be honoured");
            }
            other => panic!("config operation 'transcode' must persist, got {other:?}"),
        }

        // The state filter must operate on the persisted DB state.
        let state = engine
            .database()
            .get_job_status_string(&job.id)
            .expect("persisted state");
        assert!(
            state_matches("pending", &state),
            "fresh job state {state} must match 'pending'"
        );
        assert!(!state_matches("done", &state));

        std::fs::remove_file(&cfg).ok();
        std::fs::remove_file(&db).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_empty_db() {
        let db = std::env::temp_dir().join("oximedia_batch_list_empty.db");
        let result = cmd_list("all", &db, true).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&db).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_report_empty_db_json() {
        let db = std::env::temp_dir().join("oximedia_batch_report_json.db");
        let result = cmd_report(&db, "json").await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&db).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_report_empty_db_text() {
        let db = std::env::temp_dir().join("oximedia_batch_report_text.db");
        let result = cmd_report(&db, "text").await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&db).ok();
    }
}

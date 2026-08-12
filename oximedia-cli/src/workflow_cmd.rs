//! Workflow orchestration CLI commands.
//!
//! Provides subcommands for creating, submitting, monitoring, and managing
//! media processing workflows with DAG-based task dependencies.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Workflow command subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkflowCommand {
    /// Create a workflow definition (from template or inline task list)
    Create {
        /// Output workflow definition file (.json)
        #[arg(short, long)]
        output: PathBuf,

        /// Workflow name
        #[arg(long)]
        name: Option<String>,

        /// Template to use: transcode, ingest, qc, multi_pass, proxy
        #[arg(long)]
        template: Option<String>,

        /// Inline JSON task array, e.g. '[{"id":"t1","type":"transcode"}]'
        #[arg(long)]
        tasks: Option<String>,

        /// Source media path, recorded in the definition file
        #[arg(long)]
        source: Option<PathBuf>,

        /// Destination path, recorded in the definition file
        #[arg(long)]
        destination: Option<PathBuf>,
    },

    /// Submit a workflow from a JSON config file for execution
    Submit {
        /// Workflow config file path (.json)
        #[arg(long)]
        config: PathBuf,

        /// SQLite database path for persistence (real: `oximedia_workflow`'s
        /// `PersistenceManager`). Defaults to a per-user state directory
        /// (`$XDG_STATE_HOME/oximedia/workflow.db` or platform equivalent)
        /// when omitted, so `submit` followed by `status`/`list` without
        /// `--db` still see the same database.
        #[arg(long)]
        db: Option<PathBuf>,

        /// Maximum parallel tasks
        #[arg(long, default_value = "4")]
        parallelism: usize,

        /// Dry-run: validate only, do not execute (also skips persistence)
        #[arg(long)]
        dry_run: bool,
    },

    /// Check workflow execution status
    Status {
        /// Workflow ID to query (a UUID, as printed by `submit`/`run`)
        #[arg(long)]
        id: String,

        /// SQLite database path; see `submit --db` for the default.
        #[arg(long)]
        db: Option<PathBuf>,

        /// Show detailed per-task status
        #[arg(long)]
        detailed: bool,
    },

    /// List workflows, optionally filtered by state
    List {
        /// Filter by state: pending, running, done, failed
        #[arg(long)]
        state: Option<String>,

        /// SQLite database path; see `submit --db` for the default.
        #[arg(long)]
        db: Option<PathBuf>,
    },

    /// Cancel a running workflow
    Cancel {
        /// Workflow ID to cancel (a UUID, as printed by `submit`/`run`)
        #[arg(long)]
        id: String,

        /// SQLite database path; see `submit --db` for the default.
        #[arg(long)]
        db: Option<PathBuf>,

        /// Force cancellation without waiting for in-progress tasks. No
        /// executor runs tasks yet, so there is never an in-progress task to
        /// wait for; recorded for forward compatibility only.
        #[arg(long)]
        force: bool,
    },

    /// Show workflow execution logs
    Logs {
        /// Workflow ID to show logs for (a UUID, as printed by `submit`/`run`)
        #[arg(long)]
        id: String,

        /// Show at most the last N per-task status lines (0 = all). There is
        /// no execution-history event log yet (see the module-level note on
        /// `handle_logs`); this lists each task's current persisted state.
        #[arg(long, default_value = "50")]
        tail: usize,

        /// SQLite database path; see `submit --db` for the default.
        #[arg(long)]
        db: Option<PathBuf>,
    },

    /// List built-in workflow templates: transcode, qc, archive, ingest
    Templates,

    /// Execute a workflow from a definition file (alias for Submit)
    Run {
        /// Workflow definition file
        #[arg(short, long)]
        workflow: PathBuf,

        /// SQLite database path; see `submit --db` for the default.
        #[arg(long)]
        db_path: Option<PathBuf>,

        /// Run in dry-run mode (validate only)
        #[arg(long)]
        dry_run: bool,

        /// Maximum parallel tasks
        #[arg(long, default_value = "4")]
        parallelism: usize,
    },

    /// Manage workflow templates
    Template {
        /// Template action: list, show, export, validate
        #[arg(value_name = "ACTION")]
        action: String,

        /// Template name (for show/export/validate)
        #[arg(long)]
        name: Option<String>,

        /// Output file (for export)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

/// Handle workflow command dispatch.
pub async fn handle_workflow_command(command: WorkflowCommand, json_output: bool) -> Result<()> {
    match command {
        WorkflowCommand::Create {
            output,
            name,
            template,
            tasks,
            source,
            destination,
        } => {
            handle_create(
                &output,
                name.as_deref(),
                template.as_deref(),
                tasks.as_deref(),
                source.as_deref(),
                destination.as_deref(),
                json_output,
            )
            .await
        }
        WorkflowCommand::Submit {
            config,
            db,
            parallelism,
            dry_run,
        } => handle_submit(&config, db.as_deref(), parallelism, dry_run, json_output).await,

        WorkflowCommand::Status { id, db, detailed } => {
            handle_status(&id, db.as_deref(), detailed, json_output).await
        }

        WorkflowCommand::List { state, db } => {
            handle_list(state.as_deref(), db.as_deref(), json_output).await
        }

        WorkflowCommand::Cancel { id, db, force } => {
            handle_cancel(&id, db.as_deref(), force, json_output).await
        }

        WorkflowCommand::Logs { id, tail, db } => {
            handle_logs(&id, tail, db.as_deref(), json_output).await
        }

        WorkflowCommand::Templates => handle_templates(json_output).await,

        WorkflowCommand::Run {
            workflow,
            db_path,
            dry_run,
            parallelism,
        } => {
            handle_run(
                &workflow,
                db_path.as_deref(),
                dry_run,
                parallelism,
                json_output,
            )
            .await
        }

        WorkflowCommand::Template {
            action,
            name,
            output,
        } => handle_template(&action, name.as_deref(), output.as_deref(), json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Internal workflow definition model
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct WorkflowDef {
    name: String,
    template: Option<String>,
    /// Source media path recorded by `create --source` (consumed by steps
    /// at execution time). `serde(default)` keeps older definition files
    /// loadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// Destination path recorded by `create --destination`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    steps: Vec<WorkflowStepDef>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct WorkflowStepDef {
    id: String,
    task_type: String,
    description: String,
    depends_on: Vec<String>,
    params: serde_json::Value,
}

impl WorkflowDef {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            template: None,
            source: None,
            destination: None,
            steps: Vec::new(),
        }
    }

    fn load(path: &std::path::Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).context("Failed to read workflow definition file")?;
        let def: Self =
            serde_json::from_str(&content).context("Failed to parse workflow definition")?;
        Ok(def)
    }

    fn save(&self, path: &std::path::Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize workflow definition")?;
        std::fs::write(path, content).context("Failed to write workflow definition file")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Template definitions
// ---------------------------------------------------------------------------

fn get_template(name: &str) -> Result<WorkflowDef> {
    let mut def = WorkflowDef::new(name);
    def.template = Some(name.to_string());

    match name {
        "transcode" => {
            def.steps.push(WorkflowStepDef {
                id: "validate".to_string(),
                task_type: "qc".to_string(),
                description: "Validate source file".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"check": "format"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "transcode".to_string(),
                task_type: "transcode".to_string(),
                description: "Transcode to target format".to_string(),
                depends_on: vec!["validate".to_string()],
                params: serde_json::json!({"codec": "av1", "quality": 28}),
            });
            def.steps.push(WorkflowStepDef {
                id: "verify".to_string(),
                task_type: "qc".to_string(),
                description: "Verify output quality".to_string(),
                depends_on: vec!["transcode".to_string()],
                params: serde_json::json!({"check": "quality"}),
            });
        }
        "ingest" => {
            def.steps.push(WorkflowStepDef {
                id: "copy".to_string(),
                task_type: "transfer".to_string(),
                description: "Copy source to storage".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"protocol": "file"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "probe".to_string(),
                task_type: "analysis".to_string(),
                description: "Probe media format".to_string(),
                depends_on: vec!["copy".to_string()],
                params: serde_json::json!({"type": "probe"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "proxy".to_string(),
                task_type: "transcode".to_string(),
                description: "Generate proxy".to_string(),
                depends_on: vec!["probe".to_string()],
                params: serde_json::json!({"codec": "vp9", "quality": 40}),
            });
        }
        "qc" => {
            def.steps.push(WorkflowStepDef {
                id: "format_check".to_string(),
                task_type: "qc".to_string(),
                description: "Check container and codec format".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"check": "format"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "quality_check".to_string(),
                task_type: "qc".to_string(),
                description: "Check audio/video quality metrics".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"check": "quality"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "loudness_check".to_string(),
                task_type: "qc".to_string(),
                description: "Check loudness compliance".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"check": "loudness", "standard": "ebu_r128"}),
            });
        }
        "archive" => {
            def.steps.push(WorkflowStepDef {
                id: "checksum".to_string(),
                task_type: "hash".to_string(),
                description: "Compute file checksums".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"algorithm": "sha256"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "package".to_string(),
                task_type: "archive".to_string(),
                description: "Package into archive".to_string(),
                depends_on: vec!["checksum".to_string()],
                params: serde_json::json!({"format": "tar"}),
            });
            def.steps.push(WorkflowStepDef {
                id: "verify".to_string(),
                task_type: "hash".to_string(),
                description: "Verify archive integrity".to_string(),
                depends_on: vec!["package".to_string()],
                params: serde_json::json!({"verify": true}),
            });
        }
        "multi_pass" => {
            def.steps.push(WorkflowStepDef {
                id: "pass1".to_string(),
                task_type: "transcode".to_string(),
                description: "First pass analysis".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"pass": 1}),
            });
            def.steps.push(WorkflowStepDef {
                id: "pass2".to_string(),
                task_type: "transcode".to_string(),
                description: "Second pass encoding".to_string(),
                depends_on: vec!["pass1".to_string()],
                params: serde_json::json!({"pass": 2, "codec": "av1"}),
            });
        }
        "proxy" => {
            def.steps.push(WorkflowStepDef {
                id: "proxy_gen".to_string(),
                task_type: "transcode".to_string(),
                description: "Generate low-res proxy".to_string(),
                depends_on: vec![],
                params: serde_json::json!({"codec": "vp9", "width": 640, "height": 360}),
            });
        }
        other => {
            return Err(anyhow::anyhow!(
                "Unknown template '{}'. Valid: transcode, ingest, qc, archive, multi_pass, proxy",
                other
            ));
        }
    }

    Ok(def)
}

/// Template metadata for display.
struct TemplateInfo {
    name: &'static str,
    description: &'static str,
    steps: usize,
}

fn template_infos() -> Vec<TemplateInfo> {
    vec![
        TemplateInfo {
            name: "transcode",
            description: "Validate → Transcode → Verify output quality",
            steps: 3,
        },
        TemplateInfo {
            name: "qc",
            description: "Format + quality + loudness checks (parallel)",
            steps: 3,
        },
        TemplateInfo {
            name: "archive",
            description: "Checksum → Package → Verify archive integrity",
            steps: 3,
        },
        TemplateInfo {
            name: "ingest",
            description: "Copy to storage → Probe → Generate proxy",
            steps: 3,
        },
        TemplateInfo {
            name: "multi_pass",
            description: "Two-pass AV1 encoding (analysis + encode)",
            steps: 2,
        },
        TemplateInfo {
            name: "proxy",
            description: "Generate low-resolution VP9 proxy",
            steps: 1,
        },
    ]
}

// ---------------------------------------------------------------------------
// Handler: Create
// ---------------------------------------------------------------------------

async fn handle_create(
    output: &std::path::Path,
    name: Option<&str>,
    template: Option<&str>,
    tasks: Option<&str>,
    source: Option<&std::path::Path>,
    destination: Option<&std::path::Path>,
    json_output: bool,
) -> Result<()> {
    let workflow_name = name.unwrap_or("Untitled Workflow");

    let mut def = if let Some(tmpl) = template {
        let mut d = get_template(tmpl)?;
        d.name = workflow_name.to_string();
        d
    } else if let Some(tasks_json) = tasks {
        // Parse inline JSON task array
        let raw_tasks: Vec<serde_json::Value> =
            serde_json::from_str(tasks_json).context("Failed to parse --tasks JSON array")?;

        let mut d = WorkflowDef::new(workflow_name);
        for raw in &raw_tasks {
            let id = raw["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Each task must have an 'id' field"))?
                .to_string();
            let task_type = raw["type"].as_str().unwrap_or("custom").to_string();
            d.steps.push(WorkflowStepDef {
                id,
                task_type,
                description: raw["description"]
                    .as_str()
                    .unwrap_or("User-defined task")
                    .to_string(),
                depends_on: raw["depends_on"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                params: raw
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            });
        }
        d
    } else {
        WorkflowDef::new(workflow_name)
    };

    // Record --source / --destination in the definition file itself so
    // downstream `submit`/`run` consumers see the paths the user configured.
    if let Some(src) = source {
        def.source = Some(src.display().to_string());
    }
    if let Some(dst) = destination {
        def.destination = Some(dst.display().to_string());
    }

    def.save(output)?;

    if json_output {
        let result = serde_json::json!({
            "action": "create",
            "output": output.display().to_string(),
            "name": workflow_name,
            "template": template,
            "source": def.source,
            "destination": def.destination,
            "steps": def.steps.len(),
            "status": "created",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Workflow Created".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Name:", workflow_name);
        println!("{:20} {}", "Output:", output.display());
        if let Some(t) = template {
            println!("{:20} {}", "Template:", t);
        }
        if let Some(ref src) = def.source {
            println!("{:20} {}", "Source:", src);
        }
        if let Some(ref dst) = def.destination {
            println!("{:20} {}", "Destination:", dst);
        }
        println!("{:20} {}", "Steps:", def.steps.len());

        for step in &def.steps {
            let deps = if step.depends_on.is_empty() {
                "none".to_string()
            } else {
                step.depends_on.join(", ")
            };
            println!(
                "  [{}] {} ({}) deps=[{}]",
                step.id, step.description, step.task_type, deps,
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Real SQLite-backed persistence (oximedia_workflow::PersistenceManager)
// ---------------------------------------------------------------------------
//
// `--db` used to have no persistence layer wired behind it; every command
// warned and returned static, always-empty data. This now opens a real
// `oximedia_workflow::PersistenceManager` (Pure-Rust SQLite via OxiSQL — the
// crate is already linked with the `sqlite` feature), following the same
// real-persistence pattern `oximedia-review`'s `ReviewStore` established
// (see `crates/oximedia-review/src/store/mod.rs`): open-or-create on every
// call, `CREATE TABLE IF NOT EXISTS` migration, no separate migration
// runner.
//
// Scope note: this closes the *persistence* gap. There is still no DAG
// executor in `oximedia-cli` — `submit`/`run` persist a `Created`-state
// workflow and its tasks, but nothing transitions tasks to `Running` on its
// own. `status`/`list`/`logs` report exactly the state that was last
// persisted, never a fabricated "running"/"progressing" value.

/// Default per-user database path when `--db`/`--db-path` is omitted:
/// `$XDG_STATE_HOME/oximedia/workflow.db` (or the platform equivalent via
/// the `dirs` crate, falling back to the system temp dir). Mirrors the
/// resolution order already used by `tui_cmd`/`virtual_cmd` for their own
/// state files, so `submit` without `--db` and a later `status` without
/// `--db` land on the same database.
fn default_db_path() -> PathBuf {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("oximedia").join("workflow.db")
}

/// Resolve `--db`/`--db-path` to a concrete path, creating its parent
/// directory if needed.
///
/// Does **not** open the database — see [`with_store`] for why that must
/// happen inside `spawn_blocking`.
fn resolve_db_path(db: Option<&std::path::Path>) -> Result<PathBuf> {
    let path = db.map_or_else(default_db_path, std::path::Path::to_path_buf);

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create database directory: {}", parent.display())
            })?;
        }
    }

    Ok(path)
}

/// Open a `PersistenceManager` at `db_path` and run `f` against it on
/// Tokio's blocking thread pool.
///
/// `oximedia_workflow::PersistenceManager` is backed by
/// `oxisql_sqlite_compat::blocking::SqliteConnectionBlocking`, whose
/// `execute`/`query` methods build and drive a *fresh* Tokio runtime on the
/// calling thread with no reentrancy guard (`open` itself is reentrancy-safe
/// via a different internal helper, but every query is not). Calling any
/// `PersistenceManager` method directly from this CLI's async command
/// handlers — which always run inside the `#[tokio::main]` runtime — panics
/// with "Cannot start a runtime from within a runtime". `spawn_blocking`
/// moves the call onto a dedicated non-async-worker thread, where driving a
/// fresh nested runtime is safe. Every `PersistenceManager` access in this
/// module goes through here; do not call its methods directly elsewhere.
async fn with_store<T, F>(db_path: PathBuf, f: F) -> Result<T>
where
    F: FnOnce(
            &oximedia_workflow::PersistenceManager,
        ) -> std::result::Result<T, oximedia_workflow::WorkflowError>
        + Send
        + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let store = oximedia_workflow::PersistenceManager::new(&db_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to open workflow database at {}: {e}",
                db_path.display()
            )
        })?;
        f(&store).map_err(|e| anyhow::anyhow!("{e}"))
    })
    .await
    .map_err(|e| anyhow::anyhow!("workflow database task panicked: {e}"))?
}

/// Parse a `--id`/`--workflow-id` string as a UUID.
///
/// Workflow IDs are real `oximedia_workflow::WorkflowId`s (UUIDs) now that
/// `submit`/`run` persist through `PersistenceManager`; the earlier
/// `wf-<name>-<unix-timestamp>` cosmetic ID is gone.
fn parse_workflow_id(id: &str) -> Result<oximedia_workflow::WorkflowId> {
    uuid::Uuid::parse_str(id)
        .map(oximedia_workflow::WorkflowId::from)
        .map_err(|e| {
            anyhow::anyhow!(
                "Invalid workflow ID '{id}': {e} (workflow IDs are UUIDs, as printed by \
                 `workflow submit`/`workflow run`)"
            )
        })
}

/// Build a real `oximedia_workflow::Workflow` (with `Task`s and dependency
/// `Edge`s) from this CLI's own JSON [`WorkflowDef`].
///
/// `oximedia_workflow::TaskType` is a fixed, strongly-typed schema
/// (`Transcode`/`QualityControl`/`Transfer`/`Notification`/`CustomScript`/
/// `Analysis`/`Conditional`/`Wait`/`HttpRequest`) with no generic "custom
/// task kind + free-form params" variant, while this CLI's workflow
/// definitions use exactly that shape (`task_type: String`,
/// `params: serde_json::Value`) so `create --tasks`/templates can name any
/// step kind. Every task is therefore persisted as `TaskType::CustomScript`
/// — chosen only because it is the least semantically-loaded variant that
/// still round-trips without inventing input/output paths the CLI has no
/// real value for — with the original `task_type`/`description`/`params`
/// preserved verbatim in `Task::metadata` so nothing is silently dropped.
/// `status`/`list`/`logs` read `metadata["task_type"]`, never the
/// `CustomScript` tag, when describing a task to the user.
///
/// # Errors
///
/// Returns an error if a step's `depends_on` names a step ID that was not
/// itself defined (already checked by callers, but re-validated here since
/// this is the function that actually builds the `Edge`s).
fn build_workflow(def: &WorkflowDef) -> Result<oximedia_workflow::Workflow> {
    use oximedia_workflow::{Task, TaskId, TaskType, Workflow};
    use std::collections::HashMap as StdHashMap;

    let mut workflow = Workflow::new(def.name.clone());
    if let Some(ref src) = def.source {
        workflow.metadata.insert("source".to_string(), src.clone());
    }
    if let Some(ref dst) = def.destination {
        workflow
            .metadata
            .insert("destination".to_string(), dst.clone());
    }

    let mut id_map: StdHashMap<String, TaskId> = StdHashMap::new();

    for step in &def.steps {
        let params_json = serde_json::to_string(&step.params).unwrap_or_else(|_| "null".into());

        let mut task = Task::new(
            step.id.clone(),
            TaskType::CustomScript {
                script: std::path::PathBuf::from(format!("task:{}", step.task_type)),
                args: Vec::new(),
                env: StdHashMap::new(),
            },
        );
        task.metadata.insert("step_id".to_string(), step.id.clone());
        task.metadata
            .insert("task_type".to_string(), step.task_type.clone());
        task.metadata
            .insert("description".to_string(), step.description.clone());
        task.metadata.insert("params_json".to_string(), params_json);

        id_map.insert(step.id.clone(), task.id);
        workflow.add_task(task);
    }

    for step in &def.steps {
        let Some(&to_id) = id_map.get(&step.id) else {
            return Err(anyhow::anyhow!(
                "internal: step '{}' was not registered before edge-building",
                step.id
            ));
        };
        for dep in &step.depends_on {
            let from_id = *id_map.get(dep).ok_or_else(|| {
                anyhow::anyhow!("Step '{}' depends on unknown step '{}'", step.id, dep)
            })?;
            workflow
                .add_edge(from_id, to_id)
                .map_err(|e| anyhow::anyhow!("Failed to link '{}' -> '{}': {e}", dep, step.id))?;
        }
    }

    Ok(workflow)
}

/// Map a real `WorkflowState` onto this CLI's four-value `--state` filter
/// vocabulary (`pending`/`running`/`done`/`failed`), which predates
/// `oximedia_workflow`'s richer seven-state machine. `Created`/`Scheduled`
/// bucket under `pending`; `Running`/`Paused` under `running` (a paused
/// workflow is not finished); `Completed` is `done`; `Failed`/`Cancelled`
/// bucket under `failed`. The *actual* state name (e.g. "Paused",
/// "Cancelled") is always shown in full elsewhere in the same output — this
/// mapping only drives `--state <filter>` matching.
fn state_filter_bucket(state: oximedia_workflow::WorkflowState) -> &'static str {
    use oximedia_workflow::WorkflowState;
    match state {
        WorkflowState::Created | WorkflowState::Scheduled => "pending",
        WorkflowState::Running | WorkflowState::Paused => "running",
        WorkflowState::Completed => "done",
        WorkflowState::Failed | WorkflowState::Cancelled => "failed",
    }
}

/// Real state name for display (`Debug`-derived, e.g. "Created", "Running").
fn state_display(state: oximedia_workflow::WorkflowState) -> String {
    format!("{state:?}")
}

// ---------------------------------------------------------------------------
// Handler: Submit
// ---------------------------------------------------------------------------

async fn handle_submit(
    config: &std::path::Path,
    db: Option<&std::path::Path>,
    parallelism: usize,
    dry_run: bool,
    json_output: bool,
) -> Result<()> {
    if !config.exists() {
        return Err(anyhow::anyhow!(
            "Config file not found: {}",
            config.display()
        ));
    }

    let def = WorkflowDef::load(config)?;

    // Validate DAG: check for missing dependencies
    let step_ids: Vec<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
    for step in &def.steps {
        for dep in &step.depends_on {
            if !step_ids.contains(&dep.as_str()) {
                return Err(anyhow::anyhow!(
                    "Step '{}' depends on unknown step '{}'",
                    step.id,
                    dep
                ));
            }
        }
    }

    let workflow = build_workflow(&def)?;
    let workflow_id = workflow.id.to_string();

    // `dry_run` means "validate only" — the DAG and every step were already
    // validated above (and by `build_workflow`'s edge construction); skip
    // persistence so a dry run never creates a row `status`/`list` would
    // then report as a real, submitted workflow.
    let db_path = if dry_run {
        None
    } else {
        let path = resolve_db_path(db)?;
        with_store(path.clone(), move |store| store.save_workflow(&workflow))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to persist workflow {workflow_id}: {e}"))?;
        Some(path)
    };

    if json_output {
        let result = serde_json::json!({
            "action": "submit",
            "workflow_id": workflow_id,
            "config": config.display().to_string(),
            "name": def.name,
            "steps": def.steps.len(),
            "parallelism": parallelism,
            "dry_run": dry_run,
            "db": db_path.as_ref().map(|p| p.display().to_string()),
            "status": if dry_run { "validated" } else { "submitted" },
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        if dry_run {
            println!("{}", "Workflow Validated (dry run)".green().bold());
        } else {
            println!("{}", "Workflow Submitted".green().bold());
        }
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Workflow ID:", workflow_id.cyan());
        println!("{:20} {}", "Name:", def.name);
        println!("{:20} {}", "Config:", config.display());
        println!("{:20} {}", "Steps:", def.steps.len());
        println!("{:20} {}", "Parallelism:", parallelism);
        if let Some(ref path) = db_path {
            println!("{:20} {}", "Database:", path.display());
        }
        if dry_run {
            println!("{:20} {}", "Mode:", "dry-run (validate only)".yellow());
        }
        if !dry_run {
            println!();
            println!(
                "{}",
                format!("Use 'oximedia workflow status --id {workflow_id}' to check progress.")
                    .dimmed()
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Status
// ---------------------------------------------------------------------------

async fn handle_status(
    workflow_id: &str,
    db: Option<&std::path::Path>,
    detailed: bool,
    json_output: bool,
) -> Result<()> {
    let id = parse_workflow_id(workflow_id)?;
    let path = resolve_db_path(db)?;
    let workflow = with_store(path, move |store| store.load_workflow(id))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load workflow {workflow_id}: {e}"))?;

    let tasks_total = workflow.tasks.len();
    let tasks_completed = workflow
        .tasks
        .values()
        .filter(|t| t.state == oximedia_workflow::TaskState::Completed)
        .count();
    let progress = if tasks_total > 0 {
        tasks_completed as f64 / tasks_total as f64
    } else {
        0.0
    };
    let state_name = state_display(workflow.state);

    let mut tasks: Vec<&oximedia_workflow::Task> = workflow.tasks.values().collect();
    tasks.sort_by(|a, b| a.name.cmp(&b.name));

    if json_output {
        let result = serde_json::json!({
            "workflow_id": workflow_id,
            "name": workflow.name,
            "state": state_name,
            "progress": progress,
            "tasks_completed": tasks_completed,
            "tasks_total": tasks_total,
            "detailed": detailed,
            "tasks": if detailed {
                tasks.iter().map(|t| serde_json::json!({
                    "id": t.name,
                    "state": format!("{:?}", t.state),
                    "task_type": t.metadata.get("task_type"),
                    "description": t.metadata.get("description"),
                })).collect::<Vec<_>>()
            } else {
                Vec::new()
            },
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Workflow Status".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Workflow ID:", workflow_id);
        println!("{:20} {}", "Name:", workflow.name);
        println!("{:20} {}", "State:", state_name);
        println!("{:20} {:.0}%", "Progress:", progress * 100.0);
        println!("{:20} {} / {}", "Tasks:", tasks_completed, tasks_total);
        if detailed {
            println!();
            println!("{}", "Task Details".cyan().bold());
            println!("{}", "-".repeat(40));
            if tasks.is_empty() {
                println!("{}", "(No tasks found for this workflow ID.)".dimmed());
            }
            for task in &tasks {
                let ty = task
                    .metadata
                    .get("task_type")
                    .map(String::as_str)
                    .unwrap_or("unknown");
                println!("  [{:?}] {} ({ty})", task.state, task.name);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: List
// ---------------------------------------------------------------------------

async fn handle_list(
    state_filter: Option<&str>,
    db: Option<&std::path::Path>,
    json_output: bool,
) -> Result<()> {
    // Validate state filter if provided
    if let Some(s) = state_filter {
        match s {
            "pending" | "running" | "done" | "failed" => {}
            other => {
                return Err(anyhow::anyhow!(
                    "Invalid state '{}'. Valid values: pending, running, done, failed",
                    other
                ));
            }
        }
    }

    let path = resolve_db_path(db)?;
    // `list_workflows` returns bare IDs; each needs its own load to report a
    // real name/state/step-count. A workflow that fails to load (corrupt
    // row) is skipped with a stderr warning rather than aborting the whole
    // listing over one bad entry. Both steps run inside one `spawn_blocking`
    // call since `PersistenceManager` cannot cross an `.await` safely (see
    // `with_store`).
    let mut workflows: Vec<oximedia_workflow::Workflow> = with_store(path, move |store| {
        let ids = store.list_workflows()?;
        let mut workflows = Vec::new();
        for id in ids {
            match store.load_workflow(id) {
                Ok(wf) => workflows.push(wf),
                Err(e) => eprintln!("warning: skipping workflow {id}: {e}"),
            }
        }
        Ok(workflows)
    })
    .await
    .map_err(|e| anyhow::anyhow!("Failed to list workflows: {e}"))?;

    if let Some(filter) = state_filter {
        workflows.retain(|wf| state_filter_bucket(wf.state) == filter);
    }
    workflows.sort_by(|a, b| a.name.cmp(&b.name));

    if json_output {
        let result = serde_json::json!({
            "workflows": workflows.iter().map(|wf| serde_json::json!({
                "id": wf.id.to_string(),
                "name": wf.name,
                "state": state_display(wf.state),
                "steps": wf.tasks.len(),
            })).collect::<Vec<_>>(),
            "filter": state_filter,
            "total": workflows.len(),
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Workflows".green().bold());
        if let Some(s) = state_filter {
            println!("{}", format!("(filtered by state: {})", s).dimmed());
        }
        println!("{}", "=".repeat(60));
        if workflows.is_empty() {
            println!("{}", "No workflows found.".dimmed());
            println!();
            println!(
                "{}",
                "Submit a workflow with: oximedia workflow submit --config <file.json>".dimmed()
            );
        } else {
            for wf in &workflows {
                println!(
                    "  {} [{}] {} ({} step(s))",
                    wf.id.to_string().cyan(),
                    state_display(wf.state),
                    wf.name,
                    wf.tasks.len()
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Cancel
// ---------------------------------------------------------------------------

async fn handle_cancel(
    workflow_id: &str,
    db: Option<&std::path::Path>,
    force: bool,
    json_output: bool,
) -> Result<()> {
    use oximedia_workflow::WorkflowState;

    let id = parse_workflow_id(workflow_id)?;
    let path = resolve_db_path(db)?;
    // Load, mutate, and save inside one `spawn_blocking` call — splitting it
    // across two `with_store` calls would race against a concurrent cancel.
    let was_terminal = with_store(path, move |store| {
        let mut workflow = store.load_workflow(id)?;
        let was_terminal = workflow.state.is_terminal();
        workflow.state = WorkflowState::Cancelled;
        store.save_workflow(&workflow)?;
        Ok(was_terminal)
    })
    .await
    .map_err(|e| anyhow::anyhow!("Failed to cancel workflow {workflow_id}: {e}"))?;

    if json_output {
        let result = serde_json::json!({
            "action": "cancel",
            "workflow_id": workflow_id,
            "force": force,
            "was_already_terminal": was_terminal,
            "status": "cancelled",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Workflow Cancelled".green().bold());
        println!("{:20} {}", "Workflow ID:", workflow_id);
        println!("{:20} {}", "Force:", force);
        if was_terminal {
            println!(
                "{}",
                "Note: this workflow had already reached a terminal state.".yellow()
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Logs
// ---------------------------------------------------------------------------

/// `oximedia_workflow::persistence` already creates an `execution_history`
/// table (state transitions with timestamps) and a `task_results` table
/// (per-task output/error/duration) in its schema, but `PersistenceManager`
/// exposes no public read (or write) method for either yet — only
/// `save_workflow`/`load_workflow`/`list_workflows`/`delete_workflow`. That
/// is a real gap in `oximedia-workflow` itself (out of this CLI's edit
/// scope), so `logs` cannot show a real chronological event log. What *is*
/// real and persisted is each task's current state, which this reports as
/// one line per task — an honest, if coarser, substitute for a log tail,
/// not a fabricated event stream.
async fn handle_logs(
    workflow_id: &str,
    tail: usize,
    db: Option<&std::path::Path>,
    json_output: bool,
) -> Result<()> {
    let id = parse_workflow_id(workflow_id)?;
    let path = resolve_db_path(db)?;
    let workflow = with_store(path, move |store| store.load_workflow(id))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load workflow {workflow_id}: {e}"))?;

    let mut tasks: Vec<&oximedia_workflow::Task> = workflow.tasks.values().collect();
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    if tail > 0 && tasks.len() > tail {
        tasks = tasks.split_off(tasks.len() - tail);
    }

    if json_output {
        let result = serde_json::json!({
            "workflow_id": workflow_id,
            "tail": tail,
            "note": "per-task current state, not a chronological event log \
                     (oximedia_workflow::PersistenceManager exposes no execution-history read API yet)",
            "entries": tasks.iter().map(|t| serde_json::json!({
                "task": t.name,
                "state": format!("{:?}", t.state),
                "task_type": t.metadata.get("task_type"),
            })).collect::<Vec<_>>(),
            "total": tasks.len(),
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Workflow Logs".green().bold());
        println!("{:20} {}", "Workflow ID:", workflow_id);
        if tail > 0 {
            println!("{:20} {} entries", "Showing last:", tail);
        } else {
            println!("{:20} all entries", "Showing:");
        }
        println!("{}", "=".repeat(60));
        if tasks.is_empty() {
            println!("{}", "No tasks found for this workflow.".dimmed());
        } else {
            println!(
                "{}",
                "(per-task current state — no execution-history event log yet)".dimmed()
            );
            for task in &tasks {
                let ty = task
                    .metadata
                    .get("task_type")
                    .map(String::as_str)
                    .unwrap_or("unknown");
                println!("  [{:?}] {} ({ty})", task.state, task.name);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Templates
// ---------------------------------------------------------------------------

async fn handle_templates(json_output: bool) -> Result<()> {
    let infos = template_infos();

    if json_output {
        let templates: Vec<serde_json::Value> = infos
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "steps": t.steps,
                })
            })
            .collect();
        let result = serde_json::json!({ "templates": templates });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Built-in Workflow Templates".green().bold());
        println!("{}", "=".repeat(60));
        for info in &infos {
            println!(
                "  {} {}",
                info.name.cyan().bold(),
                format!("({} steps)", info.steps).dimmed()
            );
            println!("    {}", info.description);
        }
        println!();
        println!(
            "{}",
            "Use: oximedia workflow create --template <name> --output workflow.json".dimmed()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Run
// ---------------------------------------------------------------------------

async fn handle_run(
    workflow_path: &std::path::Path,
    db_path: Option<&std::path::Path>,
    dry_run: bool,
    parallelism: usize,
    json_output: bool,
) -> Result<()> {
    if !workflow_path.exists() {
        return Err(anyhow::anyhow!(
            "Workflow file not found: {}",
            workflow_path.display()
        ));
    }

    let def = WorkflowDef::load(workflow_path)?;

    // Validate DAG: check for missing dependencies
    let step_ids: Vec<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
    for step in &def.steps {
        for dep in &step.depends_on {
            if !step_ids.contains(&dep.as_str()) {
                return Err(anyhow::anyhow!(
                    "Step '{}' depends on unknown step '{}'",
                    step.id,
                    dep
                ));
            }
        }
    }

    let workflow = build_workflow(&def)?;
    let workflow_id = workflow.id.to_string();

    // Same "dry run skips persistence" rule as `submit` (see there for why).
    let resolved_db_path = if dry_run {
        None
    } else {
        let path = resolve_db_path(db_path)?;
        with_store(path.clone(), move |store| store.save_workflow(&workflow))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to persist workflow {workflow_id}: {e}"))?;
        Some(path)
    };

    if json_output {
        let result = serde_json::json!({
            "action": "run",
            "workflow_id": workflow_id,
            "workflow": workflow_path.display().to_string(),
            "name": def.name,
            "steps": def.steps.len(),
            "parallelism": parallelism,
            "dry_run": dry_run,
            "db": resolved_db_path.as_ref().map(|p| p.display().to_string()),
            "status": if dry_run { "validated" } else { "submitted" },
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        if dry_run {
            println!("{}", "Workflow Validated (dry run)".green().bold());
        } else {
            println!("{}", "Workflow Submitted".green().bold());
        }
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Workflow ID:", workflow_id.cyan());
        println!("{:20} {}", "Name:", def.name);
        println!("{:20} {}", "Steps:", def.steps.len());
        println!("{:20} {}", "Parallelism:", parallelism);
        println!("{:20} {}", "Dry run:", dry_run);
        if let Some(ref path) = resolved_db_path {
            println!("{:20} {}", "Database:", path.display());
        }

        if !dry_run {
            println!();
            println!(
                "{}",
                "Note: no executor runs tasks yet; this persists the workflow definition. \
                 Use 'oximedia workflow status --id <id>' to inspect it."
                    .yellow()
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Template
// ---------------------------------------------------------------------------

async fn handle_template(
    action: &str,
    name: Option<&str>,
    output: Option<&std::path::Path>,
    json_output: bool,
) -> Result<()> {
    match action {
        "list" => handle_templates(json_output).await,
        "show" => {
            let tmpl_name =
                name.ok_or_else(|| anyhow::anyhow!("Template name is required (--name)"))?;
            let def = get_template(tmpl_name)?;

            if json_output {
                let result = serde_json::json!({
                    "template": tmpl_name,
                    "steps": def.steps.iter().map(|s| {
                        serde_json::json!({
                            "id": s.id,
                            "type": s.task_type,
                            "description": s.description,
                            "depends_on": s.depends_on,
                        })
                    }).collect::<Vec<_>>(),
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else {
                println!("{}", format!("Template: {}", tmpl_name).green().bold());
                println!("{}", "=".repeat(60));
                for step in &def.steps {
                    let deps = if step.depends_on.is_empty() {
                        "none".to_string()
                    } else {
                        step.depends_on.join(", ")
                    };
                    println!(
                        "  [{}] {} ({}) deps=[{}]",
                        step.id, step.description, step.task_type, deps,
                    );
                }
            }
            Ok(())
        }
        "export" => {
            let tmpl_name =
                name.ok_or_else(|| anyhow::anyhow!("Template name is required (--name)"))?;
            let out =
                output.ok_or_else(|| anyhow::anyhow!("Output path is required (--output)"))?;
            let def = get_template(tmpl_name)?;
            def.save(out)?;

            if json_output {
                let result = serde_json::json!({
                    "action": "template_export",
                    "template": tmpl_name,
                    "output": out.display().to_string(),
                    "status": "exported",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else {
                println!("{}", "Template Exported".green().bold());
                println!("{:20} {}", "Template:", tmpl_name);
                println!("{:20} {}", "Output:", out.display());
            }
            Ok(())
        }
        "validate" => {
            let tmpl_name =
                name.ok_or_else(|| anyhow::anyhow!("Template name is required (--name)"))?;
            let def = get_template(tmpl_name)?;

            // Validate step dependencies
            let step_ids: Vec<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
            let mut issues: Vec<String> = Vec::new();
            for step in &def.steps {
                for dep in &step.depends_on {
                    if !step_ids.contains(&dep.as_str()) {
                        issues.push(format!(
                            "Step '{}' depends on unknown step '{}'",
                            step.id, dep
                        ));
                    }
                }
            }

            let valid = issues.is_empty();

            if json_output {
                let result = serde_json::json!({
                    "template": tmpl_name,
                    "valid": valid,
                    "issues": issues,
                    "steps": def.steps.len(),
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else if valid {
                println!(
                    "{} Template '{}' is valid ({} steps)",
                    "OK".green().bold(),
                    tmpl_name,
                    def.steps.len(),
                );
            } else {
                println!(
                    "{} Template '{}' has {} issue(s)",
                    "FAIL".red().bold(),
                    tmpl_name,
                    issues.len(),
                );
                for issue in &issues {
                    println!("  - {}", issue);
                }
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!(
            "Unknown template action '{}'. Valid: list, show, export, validate",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_def_new() {
        let def = WorkflowDef::new("Test");
        assert_eq!(def.name, "Test");
        assert!(def.steps.is_empty());
        assert!(def.template.is_none());
    }

    #[test]
    fn test_get_template_transcode() {
        let def = get_template("transcode").expect("transcode template should exist");
        assert_eq!(def.steps.len(), 3);
        assert_eq!(def.steps[0].id, "validate");
        assert_eq!(def.steps[1].id, "transcode");
        assert_eq!(def.steps[2].id, "verify");
        // validate has no deps, transcode depends on validate
        assert!(def.steps[0].depends_on.is_empty());
        assert_eq!(def.steps[1].depends_on, vec!["validate"]);
    }

    #[test]
    fn test_get_template_ingest() {
        let def = get_template("ingest").expect("ingest template should exist");
        assert_eq!(def.steps.len(), 3);
        assert_eq!(def.steps[0].id, "copy");
        assert_eq!(def.steps[1].id, "probe");
        assert_eq!(def.steps[2].id, "proxy");
    }

    #[test]
    fn test_get_template_qc() {
        let def = get_template("qc").expect("qc template should exist");
        assert_eq!(def.steps.len(), 3);
        // all qc checks are parallel (no deps between them)
        for step in &def.steps {
            assert!(step.depends_on.is_empty(), "qc steps should be parallel");
        }
    }

    #[test]
    fn test_get_template_archive() {
        let def = get_template("archive").expect("archive template should exist");
        assert_eq!(def.steps.len(), 3);
        assert_eq!(def.steps[0].id, "checksum");
        assert_eq!(def.steps[1].id, "package");
        assert_eq!(def.steps[2].id, "verify");
    }

    #[test]
    fn test_get_template_unknown() {
        let result = get_template("nonexistent");
        assert!(result.is_err());
        let msg = result.expect_err("should be Err").to_string();
        assert!(
            msg.contains("Unknown template"),
            "Error should mention unknown template"
        );
    }

    #[test]
    fn test_template_names_complete() {
        let names: Vec<&str> = template_infos().iter().map(|t| t.name).collect();
        assert!(names.contains(&"transcode"));
        assert!(names.contains(&"ingest"));
        assert!(names.contains(&"qc"));
        assert!(names.contains(&"archive"));
        assert!(names.contains(&"multi_pass"));
        assert!(names.contains(&"proxy"));
        assert_eq!(names.len(), 6, "should have exactly 6 built-in templates");
    }

    #[test]
    fn test_workflow_def_save_and_load_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_wf_cmd_roundtrip.json");

        let def = get_template("ingest").expect("ingest template should exist");
        def.save(&path).expect("save should succeed");

        let loaded = WorkflowDef::load(&path).expect("load should succeed");
        assert_eq!(loaded.name, "ingest");
        assert_eq!(loaded.steps.len(), 3);
        assert_eq!(loaded.steps[0].id, "copy");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_workflow_def_load_nonexistent_returns_err() {
        let path = std::env::temp_dir().join("oximedia_no_such_file_xyz.json");
        let result = WorkflowDef::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_template_infos_all_have_get_template() {
        // Every template_info entry must correspond to a valid get_template call
        let infos = template_infos();
        for info in &infos {
            let result = get_template(info.name);
            assert!(
                result.is_ok(),
                "get_template('{}') should succeed but got: {:?}",
                info.name,
                result.err()
            );
            let def = result.expect("checked above");
            assert_eq!(
                def.steps.len(),
                info.steps,
                "template '{}' step count mismatch",
                info.name
            );
        }
    }

    // ── Real SQLite persistence ────────────────────────────────────────────

    /// A unique-per-test scratch directory under `std::env::temp_dir()`.
    /// Each test uses its own literal name (not shared) so parallel test
    /// threads never contend over the same SQLite file.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oximedia_workflow_cmd_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn test_default_db_path_is_sane() {
        let path = default_db_path();
        assert!(
            path.ends_with("workflow.db"),
            "expected a workflow.db leaf, got: {}",
            path.display()
        );
        assert!(
            path.to_string_lossy().contains("oximedia"),
            "expected an oximedia subdirectory, got: {}",
            path.display()
        );
    }

    #[test]
    fn test_parse_workflow_id_rejects_non_uuid() {
        let err = parse_workflow_id("wf-001").expect_err("legacy-format ID must be rejected");
        assert!(
            err.to_string().contains("UUID"),
            "error should explain UUIDs are required: {err}"
        );
    }

    #[test]
    fn test_parse_workflow_id_accepts_uuid() {
        let id = oximedia_workflow::WorkflowId::new();
        let parsed = parse_workflow_id(&id.to_string()).expect("real UUID should parse");
        assert_eq!(parsed.to_string(), id.to_string());
    }

    #[test]
    fn test_state_filter_bucket_mapping() {
        use oximedia_workflow::WorkflowState;
        assert_eq!(state_filter_bucket(WorkflowState::Created), "pending");
        assert_eq!(state_filter_bucket(WorkflowState::Scheduled), "pending");
        assert_eq!(state_filter_bucket(WorkflowState::Running), "running");
        assert_eq!(state_filter_bucket(WorkflowState::Paused), "running");
        assert_eq!(state_filter_bucket(WorkflowState::Completed), "done");
        assert_eq!(state_filter_bucket(WorkflowState::Failed), "failed");
        assert_eq!(state_filter_bucket(WorkflowState::Cancelled), "failed");
    }

    #[test]
    fn test_build_workflow_creates_tasks_and_edges() {
        let mut def = WorkflowDef::new("build-test");
        def.steps.push(WorkflowStepDef {
            id: "a".to_string(),
            task_type: "qc".to_string(),
            description: "first".to_string(),
            depends_on: vec![],
            params: serde_json::json!({"check": "format"}),
        });
        def.steps.push(WorkflowStepDef {
            id: "b".to_string(),
            task_type: "transcode".to_string(),
            description: "second".to_string(),
            depends_on: vec!["a".to_string()],
            params: serde_json::json!({"codec": "av1"}),
        });

        let workflow = build_workflow(&def).expect("build_workflow should succeed");
        assert_eq!(workflow.tasks.len(), 2);
        assert_eq!(workflow.edges.len(), 1);

        let task_a = workflow
            .tasks
            .values()
            .find(|t| t.name == "a")
            .expect("task 'a' must exist");
        let task_b = workflow
            .tasks
            .values()
            .find(|t| t.name == "b")
            .expect("task 'b' must exist");
        assert_eq!(
            task_a.metadata.get("task_type").map(String::as_str),
            Some("qc")
        );
        assert_eq!(
            task_b.metadata.get("task_type").map(String::as_str),
            Some("transcode")
        );

        let edge = &workflow.edges[0];
        assert_eq!(edge.from, task_a.id, "edge must run a -> b");
        assert_eq!(edge.to, task_b.id, "edge must run a -> b");
    }

    #[test]
    fn test_build_workflow_unknown_dependency_errs() {
        let mut def = WorkflowDef::new("bad-dep");
        def.steps.push(WorkflowStepDef {
            id: "only".to_string(),
            task_type: "qc".to_string(),
            description: String::new(),
            depends_on: vec!["ghost".to_string()],
            params: serde_json::Value::Null,
        });

        let err = build_workflow(&def).expect_err("unknown dependency must be rejected");
        assert!(err.to_string().contains("ghost"));
    }

    #[tokio::test]
    async fn test_submit_persists_and_status_reads_it_back() {
        let dir = scratch_dir("submit_status");
        let db_path = dir.join("wf.db");
        let config_path = dir.join("wf.json");
        get_template("qc")
            .expect("qc template")
            .save(&config_path)
            .expect("save workflow def");

        handle_submit(&config_path, Some(&db_path), 4, false, true)
            .await
            .expect("submit should succeed");

        // Verify the real side effect directly against the persistence
        // layer, independent of what handle_submit printed. Loading must go
        // through `with_store` here too — see its doc comment.
        let (id0, workflow) = with_store(db_path.clone(), |store| {
            let ids = store.list_workflows()?;
            assert_eq!(ids.len(), 1, "exactly one workflow should be persisted");
            let workflow = store.load_workflow(ids[0])?;
            Ok((ids[0], workflow))
        })
        .await
        .expect("verify persisted workflow");
        assert_eq!(workflow.name, "qc");
        assert_eq!(workflow.tasks.len(), 3);
        assert_eq!(workflow.state, oximedia_workflow::WorkflowState::Created);

        // `status` must succeed against that same real, persisted ID.
        let id_str = id0.to_string();
        handle_status(&id_str, Some(&db_path), true, true)
            .await
            .expect("status should find the persisted workflow");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_submit_dry_run_does_not_persist() {
        let dir = scratch_dir("dry_run");
        let db_path = dir.join("wf.db");
        let config_path = dir.join("wf.json");
        get_template("proxy")
            .expect("proxy template")
            .save(&config_path)
            .expect("save workflow def");

        handle_submit(&config_path, Some(&db_path), 4, true, true)
            .await
            .expect("dry run should succeed");

        assert!(!db_path.exists(), "dry-run must not create a database file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_cancel_transitions_state_to_cancelled() {
        let dir = scratch_dir("cancel");
        let db_path = dir.join("wf.db");
        let config_path = dir.join("wf.json");
        get_template("archive")
            .expect("archive template")
            .save(&config_path)
            .expect("save workflow def");

        handle_submit(&config_path, Some(&db_path), 4, false, true)
            .await
            .expect("submit");

        let id0 = with_store(db_path.clone(), |store| Ok(store.list_workflows()?[0]))
            .await
            .expect("list persisted workflow");
        let id_str = id0.to_string();

        handle_cancel(&id_str, Some(&db_path), false, true)
            .await
            .expect("cancel should succeed");

        let reloaded = with_store(db_path.clone(), move |store| store.load_workflow(id0))
            .await
            .expect("reload after cancel");
        assert_eq!(reloaded.state, oximedia_workflow::WorkflowState::Cancelled);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_status_unknown_id_errs() {
        let dir = scratch_dir("status_unknown");
        let db_path = dir.join("wf.db");
        // Open (and thus create) an empty database first.
        with_store(db_path.clone(), |_store| Ok(()))
            .await
            .expect("create empty db");

        let random_id = oximedia_workflow::WorkflowId::new().to_string();
        let result = handle_status(&random_id, Some(&db_path), false, true).await;
        assert!(
            result.is_err(),
            "status for an ID absent from the database must error, not fabricate idle/0%"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_status_invalid_id_format_errs() {
        let result = handle_status("not-a-uuid", None, false, true).await;
        let err = result.expect_err("non-UUID id must be rejected before touching any database");
        assert!(err.to_string().contains("UUID"));
    }

    #[tokio::test]
    async fn test_logs_reports_real_task_states() {
        let dir = scratch_dir("logs");
        let db_path = dir.join("wf.db");
        let config_path = dir.join("wf.json");
        get_template("transcode")
            .expect("transcode template")
            .save(&config_path)
            .expect("save workflow def");

        handle_submit(&config_path, Some(&db_path), 4, false, true)
            .await
            .expect("submit");

        let id0 = with_store(db_path.clone(), |store| Ok(store.list_workflows()?[0]))
            .await
            .expect("list persisted workflow");
        let id_str = id0.to_string();

        handle_logs(&id_str, 0, Some(&db_path), true)
            .await
            .expect("logs should find the persisted workflow's tasks");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_list_reflects_persisted_workflows() {
        let dir = scratch_dir("list");
        let db_path = dir.join("wf.db");
        let config_path = dir.join("wf.json");
        get_template("qc")
            .expect("qc template")
            .save(&config_path)
            .expect("save workflow def");

        handle_submit(&config_path, Some(&db_path), 4, false, true)
            .await
            .expect("submit");

        // Unfiltered list must succeed and the store must show the row.
        handle_list(None, Some(&db_path), true)
            .await
            .expect("list should succeed");
        let count = with_store(db_path.clone(), |store| Ok(store.list_workflows()?.len()))
            .await
            .expect("list persisted workflows");
        assert_eq!(count, 1);

        // A freshly submitted workflow is `Created`, which buckets as
        // "pending"; filtering by "done" must exclude it, "pending" must not.
        handle_list(Some("pending"), Some(&db_path), true)
            .await
            .expect("pending filter should succeed");
        handle_list(Some("done"), Some(&db_path), true)
            .await
            .expect("done filter should succeed (even with zero matches)");
        let invalid = handle_list(Some("bogus"), Some(&db_path), true).await;
        assert!(invalid.is_err(), "unknown --state value must be rejected");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

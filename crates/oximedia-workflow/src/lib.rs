//! Comprehensive workflow orchestration engine for `OxiMedia`.
//!
//! This crate provides a complete workflow orchestration system with:
//! - DAG-based workflow definition
//! - Task dependencies and parallel execution
//! - State persistence with `SQLite`
//! - Cron-style scheduling
//! - REST API for workflow management
//! - Real-time monitoring and metrics
//! - Multiple task types (transcode, QC, transfer, etc.)
//!
//! # Examples
//!
//! ## Creating a Simple Workflow
//!
//! ```rust
//! use oximedia_workflow::{Workflow, Task, TaskType};
//! use std::time::Duration;
//!
//! let mut workflow = Workflow::new("my-workflow");
//!
//! let task = Task::new("wait-task", TaskType::Wait {
//!     duration: Duration::from_secs(5),
//! });
//!
//! workflow.add_task(task);
//! ```
//!
//! ## Creating a Multi-Pass Encoding Workflow
//!
//! ```rust
//! use oximedia_workflow::patterns::multi_pass_encoding;
//! use std::path::PathBuf;
//!
//! let workflow = multi_pass_encoding(
//!     PathBuf::from("/source.mp4"),
//!     PathBuf::from("/proxy.mp4"),
//!     PathBuf::from("/output.mp4"),
//!     "broadcast".to_string(),
//! );
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub mod api;
pub mod approval_gate;
pub mod audit_log;
pub mod batch_status;
pub mod builder;
pub mod checkpoint;
pub mod circuit_breaker;
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub mod cli;
pub mod cost_tracking;
pub mod dag;
pub mod error;
pub mod event_bus;
pub mod execution_trace;
#[cfg(not(target_arch = "wasm32"))]
pub mod executor;
#[cfg(not(target_arch = "wasm32"))]
pub mod fan_pattern;
pub mod fanout;
pub mod monitoring;
pub mod notification_sink;
pub mod notification_system;
pub mod parallel_steps;
pub mod patterns;
pub mod pause_resume;
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub mod persistence;
#[cfg(not(target_arch = "wasm32"))]
pub mod queue;
pub mod resource_pool;
pub mod retry_policy;
#[cfg(not(target_arch = "wasm32"))]
pub mod scheduler;
pub mod sla;
pub mod sla_tracking;
pub mod state_machine;
pub mod step_condition;
pub mod step_conditions;
pub mod step_result;
pub mod task;
pub mod task_dependency;
#[cfg(not(target_arch = "wasm32"))]
pub mod task_exec;
pub mod task_graph;
pub mod task_priority_queue;
pub mod task_template;
pub mod templates;
pub mod triggers;
pub mod utils;
pub mod validation;
pub mod versioning;
pub mod webhook;
pub mod webhook_trigger;
#[cfg(not(target_arch = "wasm32"))]
pub mod websocket;
pub mod workflow;
pub mod workflow_audit;
pub mod workflow_bundle;
pub mod workflow_checkpoint;
pub mod workflow_compose;
pub mod workflow_dashboard;
pub mod workflow_diff;
pub mod workflow_health_check;
pub mod workflow_import_export;
pub mod workflow_log;
pub mod workflow_marketplace;
pub mod workflow_metrics;
pub mod workflow_migration;
pub mod workflow_retry;
pub mod workflow_simulation;
pub mod workflow_snapshot;
pub mod workflow_template;
pub mod workflow_throttle;
pub mod workflow_version;

// Re-exports for convenience
pub use batch_status::{BatchStatusWriter, FlushResult, StatusUpdate};
pub use builder::{
    QcTaskBuilder, TaskBuilder, TranscodeTaskBuilder, TransferTaskBuilder, WorkflowBuilder,
};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerMetrics, CircuitBreakerRegistry,
    CircuitBreakerSummary, CircuitDecision, CircuitState,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub use cli::Cli;
pub use dag::{
    audio_normalize, ingest_transcode, subtitle_burn, BranchEvaluator, BranchNode, BranchType,
    DagError, DagRunStatus, DagWorkflowEngine, NodeId, NodeStatus, WorkflowDag, WorkflowEdge,
    WorkflowNode, WorkflowTemplate,
};
pub use error::{Result, WorkflowError};
pub use event_bus::{
    BusEvent, EventBus, EventBusConfig, EventBusStats, EventFilter, SubscriptionId,
};
#[cfg(not(target_arch = "wasm32"))]
pub use executor::{
    parse_condition, DefaultTaskExecutor, ExecutionContext, TaskExecutor, WorkflowControl,
    WorkflowExecutor,
};
pub use monitoring::{MonitoringService, SystemStatistics, TaskMetrics, WorkflowMetrics};
pub use parallel_steps::{
    execute_step, ParallelStepError, ParallelSteps, StepResult, StepType, WorkflowStage,
    WorkflowStep,
};
pub use patterns::*;
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub use persistence::PersistenceManager;
#[cfg(not(target_arch = "wasm32"))]
pub use queue::{QueueStatistics, TaskQueue};
pub use retry_policy::{ExponentialRetryPolicy, RetryDecision, RetryPolicyState};
#[cfg(not(target_arch = "wasm32"))]
pub use scheduler::{
    Clock, FileWatcher, ScheduledWorkflow, SystemClock, Trigger, WorkflowScheduler,
};
pub use task::{
    AnalysisType, HttpMethod, NotificationChannel, RetryPolicy, Task, TaskId, TaskPriority,
    TaskResult, TaskState, TaskType, TransferProtocol,
};
#[cfg(not(target_arch = "wasm32"))]
pub use task_exec::TaskOutcome;
pub use task_priority_queue::{PriorityEntry, PriorityLevel, TaskPriorityQueue};
pub use utils::{
    calculate_parallelism, clone_workflow, estimate_workflow_duration, expand_env_vars,
    expand_template, find_critical_path, format_duration, generate_task_name,
    get_workflow_statistics, merge_configs, normalize_paths, parse_duration, sanitize_task_name,
    WorkflowStatistics,
};
pub use validation::{
    ComplexityAnalyzer, ComplexityLevel, ComplexityMetrics, TaskValidator, ValidationReport,
    ValidationRule, WorkflowValidator,
};
pub use webhook::{WebhookConfig, WebhookEvent, WebhookNotifier, WorkflowContext};
#[cfg(not(target_arch = "wasm32"))]
pub use websocket::{WebSocketManager, WebSocketState, WorkflowEvent};
pub use workflow::{Edge, Workflow, WorkflowConfig, WorkflowId, WorkflowState};
pub use workflow_import_export::{
    deserialize_bundle, export_workflow, import_workflow, serialize_bundle, BundleFormat,
    DagBundle, WorkflowBundle, WorkflowDefinition,
};
pub use workflow_migration::{
    FieldChange, MigrationError, MigrationRegistry, MigrationStep, SchemaVersion,
};
pub use workflow_simulation::{
    quick_simulate, ConditionSource, EvaluatedCondition, SimulatedOutcome, SimulationConfig,
    SimulationResult, SimulationSummary, WorkflowSimulator,
};

/// Workflow engine - main entry point for the orchestration system.
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub struct WorkflowEngine {
    persistence: std::sync::Arc<PersistenceManager>,
    scheduler: std::sync::Arc<WorkflowScheduler>,
    monitoring: std::sync::Arc<MonitoringService>,
    executor: std::sync::Arc<dyn TaskExecutor>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
impl WorkflowEngine {
    /// Create a new workflow engine with the specified database path.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be initialized.
    pub fn new(db_path: impl AsRef<std::path::Path>) -> Result<Self> {
        let persistence = std::sync::Arc::new(PersistenceManager::new(db_path)?);
        let scheduler = std::sync::Arc::new(WorkflowScheduler::new());
        let monitoring = std::sync::Arc::new(MonitoringService::new());
        let executor = std::sync::Arc::new(DefaultTaskExecutor);

        Ok(Self {
            persistence,
            scheduler,
            monitoring,
            executor,
        })
    }

    /// Create an in-memory workflow engine (useful for testing).
    ///
    /// # Errors
    ///
    /// Returns an error if the in-memory database cannot be initialized.
    pub fn in_memory() -> Result<Self> {
        let persistence = std::sync::Arc::new(PersistenceManager::in_memory()?);
        let scheduler = std::sync::Arc::new(WorkflowScheduler::new());
        let monitoring = std::sync::Arc::new(MonitoringService::new());
        let executor = std::sync::Arc::new(DefaultTaskExecutor);

        Ok(Self {
            persistence,
            scheduler,
            monitoring,
            executor,
        })
    }

    /// Get persistence manager.
    #[must_use]
    pub fn persistence(&self) -> &std::sync::Arc<PersistenceManager> {
        &self.persistence
    }

    /// Get scheduler.
    #[must_use]
    pub fn scheduler(&self) -> &std::sync::Arc<WorkflowScheduler> {
        &self.scheduler
    }

    /// Get monitoring service.
    #[must_use]
    pub fn monitoring(&self) -> &std::sync::Arc<MonitoringService> {
        &self.monitoring
    }

    /// Get task executor.
    #[must_use]
    pub fn executor(&self) -> &std::sync::Arc<dyn TaskExecutor> {
        &self.executor
    }

    /// Submit a workflow for execution.
    ///
    /// # Errors
    ///
    /// Returns an error if the workflow cannot be saved or executed.
    pub async fn submit_workflow(&self, workflow: &Workflow) -> Result<WorkflowId> {
        // `SqliteConnectionBlocking` uses an internal current_thread runtime
        // (`block_local`). Calling it directly from an async worker thread
        // panics with "Cannot start a runtime from within a runtime."
        // `block_in_place` temporarily removes this thread from the executor
        // scheduler, allowing the nested runtime to run safely.
        tokio::task::block_in_place(|| self.persistence.save_workflow(workflow))?;
        Ok(workflow.id)
    }

    /// Execute a workflow immediately.
    ///
    /// # Errors
    ///
    /// Returns an error if the workflow cannot be loaded or executed.
    pub async fn execute_workflow(&self, workflow_id: WorkflowId) -> Result<()> {
        // Both load and save call SqliteConnectionBlocking methods that use
        // block_local (creates its own current_thread runtime). block_in_place
        // is required to avoid "cannot start a runtime from within a runtime".
        let mut workflow =
            tokio::task::block_in_place(|| self.persistence.load_workflow(workflow_id))?;

        let executor = WorkflowExecutor::new(self.executor.clone());

        self.monitoring
            .start_workflow(workflow_id, workflow.name.clone(), workflow.tasks.len());

        let result = executor.execute(&mut workflow).await?;

        self.monitoring
            .complete_workflow(workflow_id, result.state == WorkflowState::Completed);

        tokio::task::block_in_place(|| self.persistence.save_workflow(&workflow))?;

        Ok(())
    }

    /// Schedule a workflow with a trigger.
    ///
    /// # Errors
    ///
    /// Returns an error if the workflow cannot be scheduled.
    pub async fn schedule_workflow(
        &self,
        workflow: Workflow,
        trigger: Trigger,
    ) -> Result<WorkflowId> {
        let workflow_id = workflow.id;
        tokio::task::block_in_place(|| self.persistence.save_workflow(&workflow))?;
        self.scheduler.add_schedule(workflow, trigger).await?;
        Ok(workflow_id)
    }

    /// Start the workflow engine (scheduler and monitoring).
    ///
    /// # Errors
    ///
    /// Returns an error if the scheduler cannot be started.
    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        tracing::info!("Workflow engine started");
        Ok(())
    }

    /// Stop the workflow engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the scheduler cannot be stopped.
    pub async fn stop(&self) -> Result<()> {
        self.scheduler.stop().await?;
        tracing::info!("Workflow engine stopped");
        Ok(())
    }

    /// Process scheduled workflows (should be called periodically).
    pub async fn process_schedules(&self) -> Result<Vec<WorkflowId>> {
        let ready_workflows = self.scheduler.check_schedules().await;
        let mut executed = Vec::new();

        for workflow in ready_workflows {
            let workflow_id = workflow.id;
            tokio::task::block_in_place(|| self.persistence.save_workflow(&workflow))?;

            // Execute in background
            let engine = Self {
                persistence: self.persistence.clone(),
                scheduler: self.scheduler.clone(),
                monitoring: self.monitoring.clone(),
                executor: self.executor.clone(),
            };

            tokio::spawn(async move {
                if let Err(e) = engine.execute_workflow(workflow_id).await {
                    tracing::error!("Scheduled workflow execution failed: {}", e);
                }
            });

            executed.push(workflow_id);
        }

        Ok(executed)
    }

    /// Create API state for REST API server.
    #[must_use]
    pub fn api_state(&self) -> api::ApiState {
        api::ApiState {
            persistence: self.persistence.clone(),
            scheduler: self.scheduler.clone(),
            monitoring: self.monitoring.clone(),
            executor: self.executor.clone(),
            active_workflows: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Create API router for REST API server.
    pub fn api_router(&self) -> axum::Router {
        api::create_router(self.api_state())
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_workflow_engine_creation() {
        // WorkflowEngine::in_memory() is synchronous but calls
        // SqliteConnectionBlocking internally which uses block_local (builds a
        // fresh current_thread runtime). block_in_place lets the nested runtime
        // run by temporarily removing this worker from the executor pool.
        let engine = tokio::task::block_in_place(WorkflowEngine::in_memory);
        assert!(engine.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_submit_workflow() {
        let engine =
            tokio::task::block_in_place(WorkflowEngine::in_memory).expect("should succeed in test");
        let workflow = Workflow::new("test-workflow");

        let workflow_id = engine
            .submit_workflow(&workflow)
            .await
            .expect("should succeed in test");
        assert_eq!(workflow_id, workflow.id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_execute_workflow() {
        let engine =
            tokio::task::block_in_place(WorkflowEngine::in_memory).expect("should succeed in test");
        let mut workflow = Workflow::new("test-workflow");

        let task = Task::new(
            "wait-task",
            TaskType::Wait {
                duration: Duration::from_millis(10),
            },
        );
        workflow.add_task(task);

        let workflow_id = engine
            .submit_workflow(&workflow)
            .await
            .expect("should succeed in test");
        let result = engine.execute_workflow(workflow_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_schedule_workflow() {
        let engine =
            tokio::task::block_in_place(WorkflowEngine::in_memory).expect("should succeed in test");
        let workflow = Workflow::new("test-workflow");
        let trigger = Trigger::Manual;

        let workflow_id = engine
            .schedule_workflow(workflow, trigger)
            .await
            .expect("should succeed in test");

        let schedules = engine.scheduler.list_schedules().await;
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].0, workflow_id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_engine_start_stop() {
        let engine =
            tokio::task::block_in_place(WorkflowEngine::in_memory).expect("should succeed in test");

        engine.start().await.expect("should succeed in test");
        engine.stop().await.expect("should succeed in test");
    }
}

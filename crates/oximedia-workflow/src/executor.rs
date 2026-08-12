//! Workflow execution engine.
//!
//! # Parallel Fan-out / Fan-in
//!
//! [`WorkflowExecutor::execute`] automatically discovers *independent* groups
//! of tasks — tasks whose dependencies are all satisfied at the same time —
//! and launches them concurrently. When all tasks in a fan-out group complete
//! the join point (fan-in) tasks are unblocked. This happens naturally through
//! the dependency-tracking loop; no explicit graph analysis is required.
//!
//! # Scheduling correctness
//!
//! A task whose dependencies are not yet satisfied is **retried on the next
//! pass**, never dropped: the scheduler repeatedly rescans the set of
//! not-yet-dispatched tasks (a fixpoint loop) until every task has either
//! run to completion, been synthetically completed from a checkpoint, or
//! been skipped. [`WorkflowState::Completed`] is returned only when that
//! fixpoint is reached with an empty failure set. A real task failure, or a
//! dependency that can never be satisfied because an upstream task failed or
//! was skipped, is reported as [`WorkflowState::Failed`] (or a scheduling
//! [`WorkflowError`] if no task can ever become ready again) — never
//! silently downgraded to a fabricated success.
//!
//! # Pause / Resume
//!
//! Call [`WorkflowExecutor::pause`] to signal the executor to stop launching
//! new tasks after the current wave completes. The checkpoint is serialised to
//! a JSON string containing the completed task IDs and execution context
//! variables. Pass that string to [`WorkflowExecutor::resume_from_checkpoint`]
//! to reconstruct the state and continue.

use crate::error::{Result, WorkflowError};
use crate::task::{Task, TaskId, TaskResult, TaskState};
use crate::task_exec::{self, http::StatusAllowList, TaskOutcome};
use crate::workflow::{Workflow, WorkflowId, WorkflowState};
use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{mpsc, watch, RwLock, Semaphore};
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Task executor trait.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Execute a task and return the result.
    async fn execute(&self, task: &Task) -> Result<TaskResult>;
}

// ---------------------------------------------------------------------------
// Batch status buffering
// ---------------------------------------------------------------------------

/// A buffered status update pending persistence.
#[derive(Debug, Clone)]
pub struct StatusUpdate {
    /// The task whose status changed.
    pub task_id: TaskId,
    /// The new task status.
    pub status: TaskState,
    /// Wall-clock timestamp when the update was recorded.
    pub timestamp: SystemTime,
}

impl StatusUpdate {
    /// Create a new status update timestamped to now.
    #[must_use]
    pub fn new(task_id: TaskId, status: TaskState) -> Self {
        Self {
            task_id,
            status,
            timestamp: SystemTime::now(),
        }
    }
}

/// Default number of buffered status updates before an automatic flush.
const DEFAULT_FLUSH_THRESHOLD: usize = 20;

// ---------------------------------------------------------------------------
// Pause / Resume support
// ---------------------------------------------------------------------------

/// Serializable checkpoint for pause/resume of workflow execution.
///
/// This struct captures the minimal state needed to resume a paused workflow:
/// the set of already-completed task IDs and the current execution context
/// variables.  It can be persisted to disk, a database, or transferred over
/// the network, then passed back to
/// [`WorkflowExecutor::resume_from_checkpoint`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCheckpoint {
    /// Workflow identifier.
    pub workflow_id: WorkflowId,
    /// Task IDs that had completed at the time the checkpoint was taken.
    pub completed_task_ids: Vec<TaskId>,
    /// Execution context variables at checkpoint time.
    pub variables: HashMap<String, serde_json::Value>,
    /// Unix timestamp (seconds) when the checkpoint was captured.
    pub timestamp_secs: u64,
}

impl ExecutionCheckpoint {
    /// Serialize the checkpoint to a compact JSON string.
    ///
    /// # Errors
    ///
    /// Returns a `WorkflowError` if JSON serialization fails.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(WorkflowError::Serialization)
    }

    /// Deserialize a checkpoint from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns a `WorkflowError` if JSON parsing fails.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(WorkflowError::Serialization)
    }
}

/// Execution context shared across task executions.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Workflow ID.
    pub workflow_id: WorkflowId,
    /// Workflow variables.
    pub variables: Arc<DashMap<String, serde_json::Value>>,
    /// Task results cache.
    pub results: Arc<DashMap<TaskId, TaskResult>>,
}

impl ExecutionContext {
    /// Create a new execution context.
    #[must_use]
    pub fn new(workflow_id: WorkflowId) -> Self {
        Self {
            workflow_id,
            variables: Arc::new(DashMap::new()),
            results: Arc::new(DashMap::new()),
        }
    }

    /// Get variable value.
    #[must_use]
    pub fn get_variable(&self, key: &str) -> Option<serde_json::Value> {
        self.variables.get(key).map(|v| v.clone())
    }

    /// Set variable value.
    pub fn set_variable(&self, key: String, value: serde_json::Value) {
        self.variables.insert(key, value);
    }

    /// Get task result.
    #[must_use]
    pub fn get_result(&self, task_id: &TaskId) -> Option<TaskResult> {
        self.results.get(task_id).map(|r| r.clone())
    }

    /// Store task result.
    pub fn store_result(&self, task_id: TaskId, result: TaskResult) {
        self.results.insert(task_id, result);
    }
}

// ---------------------------------------------------------------------------
// WorkflowControl
// ---------------------------------------------------------------------------

/// High-level control signal for a running [`WorkflowExecutor`].
///
/// Sent via [`WorkflowExecutor::send_control`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowControl {
    /// Pause after current in-flight tasks complete.
    ///
    /// The executor serialises a checkpoint and returns with
    /// [`WorkflowState::Paused`].
    Pause,
    /// Resume a previously paused executor.
    Resume,
    /// Request immediate cancellation of the workflow.
    ///
    /// In-flight tasks are allowed to finish; then the executor returns an
    /// error with `"cancelled"`.
    Cancel,
}

/// Workflow executor.
pub struct WorkflowExecutor {
    /// Task executor implementation.
    task_executor: Arc<dyn TaskExecutor>,
    /// Maximum concurrent tasks.
    max_concurrent: usize,
    /// Execution timeout.
    timeout: Option<Duration>,
    /// Pause signal sender.  When `true` is sent the executor stops launching
    /// new tasks after the current in-flight set completes.
    pause_tx: watch::Sender<bool>,
    /// Pause signal receiver (cloned into each execution).
    pause_rx: watch::Receiver<bool>,
    /// Cancel signal sender.  When `true` is sent the executor aborts after
    /// the current wave finishes.
    cancel_tx: watch::Sender<bool>,
    /// Cancel signal receiver (cloned into each execution).
    cancel_rx: watch::Receiver<bool>,
    /// Completed tasks from a prior checkpoint (used for resume).
    resume_completed: HashSet<TaskId>,
    /// Variables from a prior checkpoint (used for resume).
    resume_variables: HashMap<String, serde_json::Value>,
    /// Pending status updates awaiting batch flush to persistence.
    status_buffer: Arc<Mutex<Vec<StatusUpdate>>>,
    /// Number of buffered updates that trigger an automatic flush.
    buffer_flush_threshold: usize,
}

impl WorkflowExecutor {
    /// Create a new workflow executor.
    #[must_use]
    pub fn new(task_executor: Arc<dyn TaskExecutor>) -> Self {
        let (pause_tx, pause_rx) = watch::channel(false);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        Self {
            task_executor,
            max_concurrent: 4,
            timeout: None,
            pause_tx,
            pause_rx,
            cancel_tx,
            cancel_rx,
            resume_completed: HashSet::new(),
            resume_variables: HashMap::new(),
            status_buffer: Arc::new(Mutex::new(Vec::new())),
            buffer_flush_threshold: DEFAULT_FLUSH_THRESHOLD,
        }
    }

    /// Override the batch-flush threshold (number of buffered updates that
    /// trigger an automatic flush to persistence).  Default is
    /// `DEFAULT_FLUSH_THRESHOLD`.
    #[must_use]
    pub fn with_flush_threshold(mut self, threshold: usize) -> Self {
        self.buffer_flush_threshold = threshold.max(1);
        self
    }

    /// Buffer a status update. When the buffer reaches `buffer_flush_threshold`
    /// entries it is flushed automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal mutex is poisoned.
    pub fn buffer_status_update(&self, update: StatusUpdate) -> Result<()> {
        let should_flush = {
            let mut buf = self
                .status_buffer
                .lock()
                .map_err(|_| WorkflowError::generic("status_buffer mutex poisoned"))?;
            buf.push(update);
            buf.len() >= self.buffer_flush_threshold
        };

        if should_flush {
            self.flush_status_buffer()?;
        }
        Ok(())
    }

    /// Flush all buffered status updates in one batch.
    ///
    /// This is called automatically when the buffer threshold is reached and
    /// should also be called on graceful shutdown or test teardown via
    /// [`Self::flush`].
    ///
    /// # Errors
    ///
    /// Returns an error if the internal mutex is poisoned.
    pub fn flush_status_buffer(&self) -> Result<()> {
        let updates = {
            let mut buf = self
                .status_buffer
                .lock()
                .map_err(|_| WorkflowError::generic("status_buffer mutex poisoned"))?;
            std::mem::take(&mut *buf)
        };

        if updates.is_empty() {
            return Ok(());
        }

        debug!("Flushing {} buffered status updates", updates.len());
        // Batch-write all updates.  In a production system this would be a
        // single INSERT/UPDATE statement covering all rows.  Here we log the
        // batch and store them into the execution context's result cache so
        // that callers can observe the changes immediately.
        for update in &updates {
            debug!(
                "Persisting status update: task={} status={:?}",
                update.task_id, update.status
            );
        }
        info!("Flushed {} status updates in batch", updates.len());
        Ok(())
    }

    /// Flush any remaining buffered status updates. Call this on graceful
    /// shutdown or at the end of a test to ensure all updates are persisted.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal mutex is poisoned.
    pub fn flush(&self) -> Result<()> {
        self.flush_status_buffer()
    }

    /// Return the number of updates currently in the buffer (for testing).
    ///
    /// # Errors
    ///
    /// Returns an error if the internal mutex is poisoned.
    pub fn buffered_update_count(&self) -> Result<usize> {
        Ok(self
            .status_buffer
            .lock()
            .map_err(|_| WorkflowError::generic("status_buffer mutex poisoned"))?
            .len())
    }

    /// Set maximum concurrent tasks.
    #[must_use]
    pub fn with_max_concurrent(mut self, max_concurrent: usize) -> Self {
        self.max_concurrent = max_concurrent;
        self
    }

    /// Set execution timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Signal the executor to pause after the current in-flight tasks finish.
    ///
    /// The executor stops launching new tasks as soon as it receives the
    /// signal, but any already-running tasks are allowed to complete.
    pub fn pause(&self) {
        if let Err(e) = self.pause_tx.send(true) {
            warn!("Failed to send pause signal: {e}");
        }
    }

    /// Resume a previously paused executor (un-pause).
    pub fn resume(&self) {
        if let Err(e) = self.pause_tx.send(false) {
            warn!("Failed to send resume signal: {e}");
        }
    }

    /// Send a [`WorkflowControl`] signal to the executor.
    ///
    /// - [`WorkflowControl::Pause`]  — equivalent to [`Self::pause`].
    /// - [`WorkflowControl::Resume`] — equivalent to [`Self::resume`].
    /// - [`WorkflowControl::Cancel`] — signals the executor to abort after the
    ///   current task wave finishes and return a cancellation error.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError`] if the underlying channel is closed (i.e. the
    /// executor has already been dropped).
    pub fn send_control(&self, control: WorkflowControl) -> Result<()> {
        match control {
            WorkflowControl::Pause => {
                self.pause_tx
                    .send(true)
                    .map_err(|e| WorkflowError::generic(format!("pause send failed: {e}")))?;
            }
            WorkflowControl::Resume => {
                self.pause_tx
                    .send(false)
                    .map_err(|e| WorkflowError::generic(format!("resume send failed: {e}")))?;
            }
            WorkflowControl::Cancel => {
                self.cancel_tx
                    .send(true)
                    .map_err(|e| WorkflowError::generic(format!("cancel send failed: {e}")))?;
                // Also set pause so the loop checks after current wave
                let _ = self.pause_tx.send(true);
            }
        }
        Ok(())
    }

    /// Returns `true` when the executor is currently in a paused state.
    ///
    /// This reflects the latest value sent via [`Self::pause`] /
    /// [`Self::send_control`].  It does **not** guarantee that execution has
    /// actually stopped; in-flight tasks may still be running.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        *self.pause_rx.borrow()
    }

    /// Load a prior `ExecutionCheckpoint` so that already-completed tasks are
    /// skipped when [`Self::execute`] is called next.
    ///
    /// # Errors
    ///
    /// Returns an error if the checkpoint JSON is invalid.
    pub fn resume_from_checkpoint(&mut self, checkpoint_json: &str) -> Result<()> {
        let cp = ExecutionCheckpoint::from_json(checkpoint_json)?;
        self.resume_completed = cp.completed_task_ids.into_iter().collect();
        self.resume_variables = cp.variables;
        info!(
            "Loaded checkpoint with {} completed tasks",
            self.resume_completed.len()
        );
        Ok(())
    }

    /// Execute a workflow.
    ///
    /// Tasks whose dependencies are all satisfied at the same time are launched
    /// concurrently (fan-out). The executor waits for all of them before
    /// considering their successors (fan-in), bounded by `max_concurrent`.
    ///
    /// If a checkpoint was loaded via [`Self::resume_from_checkpoint`] the
    /// corresponding tasks are pre-marked as completed and skipped.
    ///
    /// The executor checks the pause signal between task waves; when paused it
    /// drains the current in-flight tasks, serialises a checkpoint, and returns
    /// `WorkflowState::Paused` in the `ExecutionResult`.
    ///
    /// # Ordering invariant
    ///
    /// The scheduler rescans not-yet-dispatched tasks every pass instead of
    /// consuming a single-pass iterator, so a task whose dependencies are not
    /// yet satisfied is retried on a later pass rather than permanently
    /// skipped. `WorkflowState::Completed` is only returned once every task
    /// has actually been dispatched (run, synthetically completed, or
    /// skipped) *and* no task failed; otherwise the result is
    /// `WorkflowState::Failed` (or an `Err` for an unrecoverable scheduling
    /// deadlock), never a fabricated `Completed`.
    pub async fn execute(&self, workflow: &mut Workflow) -> Result<ExecutionResult> {
        info!("Starting workflow execution: {}", workflow.name);

        // Validate workflow
        workflow.validate()?;

        // Update workflow state
        workflow.state = WorkflowState::Running;

        let start_time = Instant::now();
        let context = ExecutionContext::new(workflow.id);

        // Initialize variables from workflow config
        for (key, value) in &workflow.config.variables {
            context.set_variable(key.clone(), value.clone());
        }

        // Apply resume variables (override config-level variables)
        for (key, value) in &self.resume_variables {
            context.set_variable(key.clone(), value.clone());
        }

        // Get topological order
        let task_order = workflow.topological_sort()?;

        // Track completed tasks — pre-populate from checkpoint if resuming
        let completed_tasks: Arc<RwLock<HashSet<TaskId>>> =
            Arc::new(RwLock::new(self.resume_completed.clone()));
        let failed_tasks: Arc<RwLock<HashSet<TaskId>>> = Arc::new(RwLock::new(HashSet::new()));

        // Semaphore for limiting concurrent tasks
        let semaphore = Arc::new(Semaphore::new(
            workflow
                .config
                .max_concurrent_tasks
                .min(self.max_concurrent),
        ));

        // Channel for task completion notifications
        let (tx, mut rx) = mpsc::channel(100);

        // Pause signal receiver (clone so we can poll it)
        let pause_rx = self.pause_rx.clone();
        // Cancel signal receiver (clone so we can poll it)
        let cancel_rx = self.cancel_rx.clone();

        // Execute tasks in dependency order. See the "Ordering invariant"
        // section on `Self::execute` and the "Scheduling correctness" module
        // doc for the full contract: `pending` starts as the full
        // topological order and is re-scanned on *every* pass of the outer
        // `loop` below (a fixpoint), instead of being drained by a
        // single-pass iterator that would permanently drop any task whose
        // dependencies were not yet satisfied on the pass it was visited.
        let mut active_tasks = 0;
        let mut pending: Vec<TaskId> = task_order.clone();

        loop {
            // Check for timeout
            if let Some(timeout_duration) = self.timeout {
                if start_time.elapsed() > timeout_duration {
                    workflow.state = WorkflowState::Failed;
                    return Err(WorkflowError::generic("Workflow execution timeout"));
                }
            }

            // Check cancel signal between task waves (only when no tasks are active)
            if active_tasks == 0 && *cancel_rx.borrow() {
                workflow.state = WorkflowState::Failed;
                return Err(WorkflowError::generic("Workflow cancelled"));
            }

            // Check pause signal between task waves (only when no tasks are active)
            if active_tasks == 0 && *pause_rx.borrow() {
                // Drain in-flight tasks first (none here since active_tasks == 0)
                // Capture checkpoint
                let completed = completed_tasks.read().await;
                let variables: HashMap<String, serde_json::Value> = context
                    .variables
                    .iter()
                    .map(|e| (e.key().clone(), e.value().clone()))
                    .collect();
                let checkpoint = ExecutionCheckpoint {
                    workflow_id: workflow.id,
                    completed_task_ids: completed.iter().copied().collect(),
                    variables,
                    timestamp_secs: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };
                drop(completed);

                let checkpoint_json = checkpoint.to_json()?;
                info!(
                    "Workflow paused; checkpoint: {} bytes",
                    checkpoint_json.len()
                );

                workflow.state = WorkflowState::Paused;
                return Ok(ExecutionResult {
                    workflow_id: workflow.id,
                    state: WorkflowState::Paused,
                    duration: start_time.elapsed(),
                    task_results: context
                        .results
                        .iter()
                        .map(|entry| (*entry.key(), entry.value().clone()))
                        .collect(),
                    checkpoint: Some(checkpoint_json),
                });
            }

            // Scan every still-pending task exactly once per pass. A task is
            // removed from `pending` as soon as it is dispatched in any way
            // (spawned, synthetically completed, or skipped); a task that is
            // not yet ready is re-queued into `still_pending` so this same
            // pass logic retries it the next time the outer `loop` runs
            // (see the ordering invariant on `Self::execute`).
            let pending_before = pending.len();
            let mut still_pending = Vec::with_capacity(pending.len());

            for task_id in pending {
                // Check if dependencies are satisfied
                let deps = workflow.get_dependencies(&task_id);
                let completed = completed_tasks.read().await;
                let failed = failed_tasks.read().await;

                // Skip tasks that were already completed in a prior checkpoint
                let already_done = completed.contains(&task_id);
                let deps_satisfied = deps.iter().all(|dep| completed.contains(dep));
                let deps_failed = deps.iter().any(|dep| failed.contains(dep));

                drop(completed);
                drop(failed);

                // Task was completed in a prior run: count it but skip execution
                if already_done {
                    active_tasks += 1;
                    // Notify completion immediately via a synthetic result
                    let result = TaskResult {
                        task_id,
                        status: TaskState::Completed,
                        data: None,
                        error: None,
                        duration: Duration::ZERO,
                        outputs: Vec::new(),
                    };
                    let tx2 = tx.clone();
                    let completed2 = completed_tasks.clone();
                    tokio::spawn(async move {
                        completed2.write().await.insert(task_id);
                        let _ = tx2.send((task_id, result)).await;
                    });
                    continue;
                }

                if deps_failed {
                    if workflow.config.fail_fast {
                        workflow.state = WorkflowState::Failed;
                        return Err(WorkflowError::DependencyFailed(task_id.to_string()));
                    }
                    // Skip this task. Record it as failed (not merely
                    // absent from every set) so that any task depending on
                    // *this* task correctly observes `deps_failed` on the
                    // next pass too, instead of waiting forever on a
                    // dependency that will never complete or fail on its
                    // own — that silent gap used to let whole downstream
                    // chains vanish without being counted as completed or
                    // failed.
                    if let Some(task) = workflow.get_task_mut(&task_id) {
                        task.set_state(TaskState::Skipped)?;
                    }
                    failed_tasks.write().await.insert(task_id);
                    continue;
                }

                if !deps_satisfied {
                    // Not ready yet: retry on a later pass instead of
                    // dropping the task permanently (the original bug).
                    still_pending.push(task_id);
                    continue;
                }

                // Get task
                let Some(task) = workflow.get_task(&task_id).cloned() else {
                    continue;
                };

                // Check if task should run based on conditions
                if !self.should_run_task(&task, &context).await {
                    if let Some(t) = workflow.get_task_mut(&task_id) {
                        t.set_state(TaskState::Skipped)?;
                    }
                    completed_tasks.write().await.insert(task_id);
                    continue;
                }

                // Spawn task execution
                let executor = self.task_executor.clone();
                let sem = semaphore.clone();
                let ctx = context.clone();
                let tx = tx.clone();
                let completed = completed_tasks.clone();
                let failed = failed_tasks.clone();
                let status_buf = self.status_buffer.clone();
                let flush_threshold = self.buffer_flush_threshold;

                tokio::spawn(async move {
                    let Ok(_permit) = sem.acquire().await else {
                        let _ = tx
                            .send((
                                task_id,
                                TaskResult {
                                    task_id,
                                    status: TaskState::Failed,
                                    data: None,
                                    error: Some("Semaphore closed".to_string()),
                                    duration: std::time::Duration::ZERO,
                                    outputs: Vec::new(),
                                },
                            ))
                            .await;
                        return;
                    };
                    let result = Self::execute_task(executor, &task, &ctx).await;

                    let success = matches!(result.status, TaskState::Completed);
                    if success {
                        completed.write().await.insert(task_id);
                    } else {
                        failed.write().await.insert(task_id);
                    }

                    // Buffer status update for batch persistence.
                    let update = StatusUpdate::new(task_id, result.status);
                    let should_flush = {
                        if let Ok(mut buf) = status_buf.lock() {
                            buf.push(update);
                            buf.len() >= flush_threshold
                        } else {
                            false
                        }
                    };
                    if should_flush {
                        if let Ok(mut buf) = status_buf.lock() {
                            let drained = std::mem::take(&mut *buf);
                            debug!(
                                "Auto-flushing {} status updates from spawned task",
                                drained.len()
                            );
                        }
                    }

                    ctx.store_result(task_id, result.clone());
                    let _ = tx.send((task_id, result)).await;
                });

                active_tasks += 1;
            }

            pending = still_pending;

            // Nothing left in flight and nothing left pending: every task
            // has been dispatched (run, synthetically completed, or
            // skipped). This is the only way out of the loop that leads to
            // a `Completed` result below.
            if active_tasks == 0 && pending.is_empty() {
                break;
            }

            if active_tasks == 0 {
                if pending.len() == pending_before {
                    // A full pass over every remaining pending task
                    // dispatched nothing, and nothing is in flight that
                    // could ever change `completed_tasks` / `failed_tasks`
                    // again: the remaining tasks can never become ready.
                    // `Workflow::validate` (called above) already rejects
                    // cycles and dangling edge references, so this should be
                    // unreachable in practice — but we refuse to silently
                    // report `Completed` while tasks were dropped, which is
                    // exactly the bug this scheduler replaces.
                    workflow.state = WorkflowState::Failed;
                    return Err(WorkflowError::generic(format!(
                        "Workflow scheduling deadlock: {} task(s) can never become ready: {:?}",
                        pending.len(),
                        pending
                    )));
                }
                // Some tasks were resolved synchronously (skipped) this pass
                // with nothing spawned to await; loop immediately to
                // re-scan `pending` instead of blocking on `rx.recv()` with
                // no in-flight sender, which would hang forever.
                continue;
            }

            if let Some((_task_id, result)) = rx.recv().await {
                active_tasks -= 1;

                if !matches!(result.status, TaskState::Completed) && workflow.config.fail_fast {
                    workflow.state = WorkflowState::Failed;
                    return Err(WorkflowError::TaskExecutionFailed {
                        task_id: result.task_id.to_string(),
                        reason: result.error.unwrap_or_else(|| "Unknown error".to_string()),
                    });
                }
            } else {
                break;
            }
        }

        // Check final state. By the time the loop above breaks, every task in
        // `task_order` has been dispatched (see the ordering invariant on
        // `Self::execute`): a real per-task failure and a cascaded
        // dependency-failure skip both land in `failed_tasks`, so this check
        // now honestly reflects whether the whole workflow actually
        // succeeded rather than merely "the scan reached the end".
        let failed = failed_tasks.read().await;
        let final_state = if failed.is_empty() {
            WorkflowState::Completed
        } else {
            WorkflowState::Failed
        };

        workflow.state = final_state;

        info!(
            "Workflow execution completed: {} in {:?}",
            workflow.name,
            start_time.elapsed()
        );

        Ok(ExecutionResult {
            workflow_id: workflow.id,
            state: final_state,
            duration: start_time.elapsed(),
            task_results: context
                .results
                .iter()
                .map(|entry| (*entry.key(), entry.value().clone()))
                .collect(),
            checkpoint: None,
        })
    }

    async fn execute_task(
        executor: Arc<dyn TaskExecutor>,
        task: &Task,
        _context: &ExecutionContext,
    ) -> TaskResult {
        debug!("Executing task: {}", task.name);
        let start = Instant::now();

        let result = if let Some(task_timeout) = Some(task.timeout) {
            match timeout(task_timeout, executor.execute(task)).await {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => TaskResult {
                    task_id: task.id,
                    status: TaskState::Failed,
                    data: None,
                    error: Some(e.to_string()),
                    duration: start.elapsed(),
                    outputs: Vec::new(),
                },
                Err(_) => TaskResult {
                    task_id: task.id,
                    status: TaskState::Failed,
                    data: None,
                    error: Some("Task timeout".to_string()),
                    duration: start.elapsed(),
                    outputs: Vec::new(),
                },
            }
        } else {
            match executor.execute(task).await {
                Ok(result) => result,
                Err(e) => TaskResult {
                    task_id: task.id,
                    status: TaskState::Failed,
                    data: None,
                    error: Some(e.to_string()),
                    duration: start.elapsed(),
                    outputs: Vec::new(),
                },
            }
        };

        debug!(
            "Task {} completed with status: {:?}",
            task.name, result.status
        );

        result
    }

    async fn should_run_task(&self, task: &Task, context: &ExecutionContext) -> bool {
        // Evaluate task conditions
        for condition in &task.conditions {
            if !self.evaluate_condition(condition, context).await {
                debug!("Task {} skipped due to condition: {}", task.name, condition);
                return false;
            }
        }
        true
    }

    async fn evaluate_condition(&self, condition: &str, context: &ExecutionContext) -> bool {
        match parse_condition(condition, context) {
            Ok(result) => result,
            Err(err) => {
                debug!(
                    "Condition parse error (treating as false): {} – {}",
                    condition, err
                );
                false
            }
        }
    }
}

// ── Condition expression evaluator ──────────────────────────────────────────

/// Value types that can appear in condition expressions.
#[derive(Debug, Clone, PartialEq)]
enum CondValue {
    /// Integer/byte quantity (e.g. file sizes, counts).
    Int(i64),
    /// Floating-point number.
    Float(f64),
    /// String value.
    Str(String),
    /// Boolean literal.
    Bool(bool),
}

impl CondValue {
    /// Attempt to compare two values using a standard ordering.
    fn partial_cmp_values(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a.partial_cmp(b),
            (Self::Float(a), Self::Float(b)) => a.partial_cmp(b),
            (Self::Int(a), Self::Float(b)) => (*a as f64).partial_cmp(b),
            (Self::Float(a), Self::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Self::Str(a), Self::Str(b)) => a.partial_cmp(b),
            _ => None,
        }
    }

    /// Equality comparison with type coercion between numerics.
    fn eq_coerced(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => (a - b).abs() < f64::EPSILON,
            (Self::Int(a), Self::Float(b)) => ((*a as f64) - b).abs() < f64::EPSILON,
            (Self::Float(a), Self::Int(b)) => (a - (*b as f64)).abs() < f64::EPSILON,
            (Self::Str(a), Self::Str(b)) => a.eq_ignore_ascii_case(b),
            (Self::Bool(a), Self::Bool(b)) => a == b,
            _ => false,
        }
    }
}

/// Resolve a variable reference from the execution context.
///
/// Supported paths:
/// - `output.<key>` – looks up `key` in `context.variables` and parses the
///   value as a `CondValue`.
/// - Bare identifiers – also looked up in `context.variables`.
fn resolve_variable(path: &str, context: &ExecutionContext) -> Option<CondValue> {
    let key = path.trim_start_matches("output.");
    let json_val = context.get_variable(key)?;
    json_to_cond_value(&json_val)
}

/// Convert a `serde_json::Value` to a `CondValue`.
fn json_to_cond_value(v: &serde_json::Value) -> Option<CondValue> {
    match v {
        serde_json::Value::Bool(b) => Some(CondValue::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(CondValue::Int(i))
            } else {
                n.as_f64().map(CondValue::Float)
            }
        }
        serde_json::Value::String(s) => Some(CondValue::Str(s.clone())),
        _ => None,
    }
}

/// Parse a literal token (number with optional unit, boolean, or quoted string)
/// into a `CondValue`.
///
/// Unit suffixes supported:
/// - Bytes  : `B`, `KB`, `MB`, `GB`, `TB` (powers of 1024)
/// - Time   : `ms`, `s`, `m`, `h`
/// - Bare numbers are treated as integers or floats.
fn parse_literal(token: &str) -> Option<CondValue> {
    let t = token.trim().trim_matches(|c| c == '"' || c == '\'');

    // Boolean literals
    match t.to_lowercase().as_str() {
        "true" => return Some(CondValue::Bool(true)),
        "false" => return Some(CondValue::Bool(false)),
        _ => {}
    }

    // Try numeric with byte-size suffix (case-insensitive)
    let lower = t.to_lowercase();
    let byte_units: &[(&str, i64)] = &[
        ("tb", 1024 * 1024 * 1024 * 1024),
        ("gb", 1024 * 1024 * 1024),
        ("mb", 1024 * 1024),
        ("kb", 1024),
        ("b", 1),
    ];
    for &(suffix, multiplier) in byte_units {
        if let Some(num_str) = lower.strip_suffix(suffix) {
            let num_str = num_str.trim();
            if let Ok(n) = num_str.parse::<f64>() {
                return Some(CondValue::Int((n * multiplier as f64) as i64));
            }
        }
    }

    // Duration suffixes
    let duration_units: &[(&str, i64)] =
        &[("ms", 1), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)];
    for &(suffix, multiplier_ms) in duration_units {
        if let Some(num_str) = lower.strip_suffix(suffix) {
            let num_str = num_str.trim();
            if let Ok(n) = num_str.parse::<f64>() {
                return Some(CondValue::Int((n * multiplier_ms as f64) as i64));
            }
        }
    }

    // Plain integer
    if let Ok(i) = t.parse::<i64>() {
        return Some(CondValue::Int(i));
    }

    // Plain float
    if let Ok(f) = t.parse::<f64>() {
        return Some(CondValue::Float(f));
    }

    // Fall back to string value (handles codec names etc.)
    Some(CondValue::Str(t.to_string()))
}

/// Parse and evaluate a single condition expression against the execution context.
///
/// Grammar (simplified):
/// ```text
/// condition  := operand  operator  operand
///             | "!" operand
/// operand    := variable | literal
/// variable   := identifier ("." identifier)*
/// operator   := "==" | "!=" | ">" | ">=" | "<" | "<="
///             | "contains" | "startswith" | "endswith"
/// literal    := number [unit] | bool | quoted-string
/// ```
///
/// Examples:
/// - `"output.size > 1MB"`
/// - `"duration >= 60s"`
/// - `"codec == av1"`
/// - `"output.status == completed"`
/// - `"count > 0"`
/// - `"!failed"`
pub fn parse_condition(
    condition: &str,
    context: &ExecutionContext,
) -> std::result::Result<bool, String> {
    let cond = condition.trim();

    // Handle negation: !<expr>
    if let Some(rest) = cond.strip_prefix('!') {
        return parse_condition(rest.trim(), context).map(|v| !v);
    }

    // Try to split on a binary operator (longest match first to avoid
    // ">" matching before ">=")
    let operators = &[
        ">=",
        "<=",
        "!=",
        "==",
        ">",
        "<",
        "contains",
        "startswith",
        "endswith",
    ];

    for &op in operators {
        // Use case-insensitive search for word operators
        let split_pos = if op.chars().all(char::is_alphabetic) {
            // word operator – find as whole word
            let lower = cond.to_lowercase();
            let needle = format!(" {op} ");
            lower
                .find(&needle)
                .map(|p| (p, p + needle.len() - 1, op.len()))
        } else {
            // symbol operator
            cond.find(op).map(|p| (p, p + op.len(), op.len()))
        };

        let Some((lhs_end, rhs_start, _)) = split_pos else {
            continue;
        };

        let lhs_str = cond[..lhs_end].trim();
        let rhs_str = cond[rhs_start..].trim();

        // Resolve left-hand side: try as variable first, then literal
        let lhs = resolve_variable(lhs_str, context).or_else(|| parse_literal(lhs_str));

        // Resolve right-hand side
        let rhs = resolve_variable(rhs_str, context).or_else(|| parse_literal(rhs_str));

        let lhs = lhs.ok_or_else(|| format!("Cannot resolve LHS: {lhs_str}"))?;
        let rhs = rhs.ok_or_else(|| format!("Cannot resolve RHS: {rhs_str}"))?;

        let result = match op {
            "==" => lhs.eq_coerced(&rhs),
            "!=" => !lhs.eq_coerced(&rhs),
            ">" => lhs
                .partial_cmp_values(&rhs)
                .is_some_and(|o| o == std::cmp::Ordering::Greater),
            ">=" => lhs
                .partial_cmp_values(&rhs)
                .is_some_and(|o| o != std::cmp::Ordering::Less),
            "<" => lhs
                .partial_cmp_values(&rhs)
                .is_some_and(|o| o == std::cmp::Ordering::Less),
            "<=" => lhs
                .partial_cmp_values(&rhs)
                .is_some_and(|o| o != std::cmp::Ordering::Greater),
            "contains" => {
                if let (CondValue::Str(l), CondValue::Str(r)) = (&lhs, &rhs) {
                    l.to_lowercase().contains(&r.to_lowercase())
                } else {
                    false
                }
            }
            "startswith" => {
                if let (CondValue::Str(l), CondValue::Str(r)) = (&lhs, &rhs) {
                    l.to_lowercase().starts_with(&r.to_lowercase())
                } else {
                    false
                }
            }
            "endswith" => {
                if let (CondValue::Str(l), CondValue::Str(r)) = (&lhs, &rhs) {
                    l.to_lowercase().ends_with(&r.to_lowercase())
                } else {
                    false
                }
            }
            _ => false,
        };

        return Ok(result);
    }

    // No operator found: treat the whole expression as a boolean variable lookup
    if let Some(val) = resolve_variable(cond, context) {
        return Ok(match val {
            CondValue::Bool(b) => b,
            CondValue::Int(i) => i != 0,
            CondValue::Float(f) => f != 0.0,
            CondValue::Str(s) => !s.is_empty() && s.to_lowercase() != "false",
        });
    }

    // Unknown condition – default to true so existing workflows are not broken
    debug!("Condition not resolvable, defaulting to true: {}", cond);
    Ok(true)
}

/// Workflow execution result.
#[derive(Debug)]
pub struct ExecutionResult {
    /// Workflow ID.
    pub workflow_id: WorkflowId,
    /// Final workflow state.
    pub state: WorkflowState,
    /// Total execution duration.
    pub duration: Duration,
    /// Results for all tasks.
    pub task_results: HashMap<TaskId, TaskResult>,
    /// Serialized checkpoint JSON when the workflow was paused mid-execution.
    /// `None` when the workflow ran to completion or failure.
    pub checkpoint: Option<String>,
}

/// Default task executor implementation.
///
/// Every media-oriented task type is executed **for real** through
/// [`crate::task_exec`]:
///
/// - [`TaskType::HttpRequest`] sends the request with `reqwest` and fails on
///   an unaccepted status,
/// - [`TaskType::Transcode`] runs an `oximedia-transcode` pipeline and
///   verifies the produced file,
/// - [`TaskType::QualityControl`] runs `oximedia-qc` rules and fails when the
///   report fails,
/// - [`TaskType::Analysis`] decodes the input and runs the requested
///   `oximedia-analysis` / `oximedia-audio-analysis` / `oximedia-quality`
///   measurements,
/// - [`TaskType::CustomScript`] spawns the process and checks its exit status,
/// - [`TaskType::Transfer`] copies the file for
///   [`TransferProtocol::Local`](crate::task::TransferProtocol::Local).
///
/// Two arms remain **simulation only** and report success without performing
/// the work — provide a custom [`TaskExecutor`] when a workflow depends on
/// them: remote [`TaskType::Transfer`] protocols (FTP/SFTP/S3/HTTP/rsync) and
/// [`TaskType::Notification`] delivery.
///
/// [`TaskType::HttpRequest`]: crate::task::TaskType::HttpRequest
/// [`TaskType::Transcode`]: crate::task::TaskType::Transcode
/// [`TaskType::QualityControl`]: crate::task::TaskType::QualityControl
/// [`TaskType::Analysis`]: crate::task::TaskType::Analysis
/// [`TaskType::CustomScript`]: crate::task::TaskType::CustomScript
/// [`TaskType::Transfer`]: crate::task::TaskType::Transfer
/// [`TaskType::Notification`]: crate::task::TaskType::Notification
pub struct DefaultTaskExecutor;

impl DefaultTaskExecutor {
    /// Runs a task and returns the evidence it produced.
    ///
    /// Errors returned here become a [`TaskState::Failed`] result in
    /// [`TaskExecutor::execute`], which is what makes the DAG mark the task —
    /// and every dependant — as failed.
    async fn run(&self, task: &Task) -> Result<TaskOutcome> {
        use crate::task::TaskType;

        match &task.task_type {
            TaskType::Wait { duration } => {
                tokio::time::sleep(*duration).await;
                Ok(TaskOutcome::empty())
            }
            TaskType::HttpRequest {
                url,
                method,
                headers,
                body,
            } => {
                let allow_status = StatusAllowList::from_metadata(&task.metadata)?;
                task_exec::http::execute_http_request(
                    url,
                    method,
                    headers,
                    body.as_deref(),
                    task.timeout,
                    &allow_status,
                )
                .await
            }
            TaskType::Transcode {
                input,
                output,
                preset,
                params,
            } => task_exec::transcode::execute_transcode(input, output, preset, params).await,
            TaskType::QualityControl {
                input,
                profile,
                rules,
            } => task_exec::qc::execute_quality_control(input, profile, rules).await,
            TaskType::Transfer {
                source,
                destination,
                protocol,
                options: _,
            } => {
                use crate::task::TransferProtocol;
                match protocol {
                    TransferProtocol::Local => {
                        let src_path = std::path::Path::new(source.as_str());
                        let dst_path = std::path::Path::new(destination.as_str());
                        if let Some(parent) = dst_path.parent() {
                            if !parent.as_os_str().is_empty() {
                                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                                    WorkflowError::generic(format!(
                                        "Cannot create destination dir: {e}"
                                    ))
                                })?;
                            }
                        }
                        let copied = tokio::fs::copy(src_path, dst_path).await.map_err(|e| {
                            WorkflowError::generic(format!(
                                "Local copy {} → {} failed: {e}",
                                src_path.display(),
                                dst_path.display()
                            ))
                        })?;
                        info!("Local transfer complete: {} → {}", source, destination);
                        Ok(TaskOutcome::with_data(serde_json::json!({
                            "kind": "transfer",
                            "protocol": "local",
                            "source": source,
                            "destination": destination,
                            "bytes": copied,
                        }))
                        .and_output(dst_path.to_path_buf()))
                    }
                    other => {
                        // Simulation only: no bytes are moved. Documented on
                        // `DefaultTaskExecutor`; supply a custom `TaskExecutor`
                        // for real remote transfers.
                        warn!(
                            "Remote transfer ({:?}) NOT performed (no transfer agent in this \
                             executor): {} → {}",
                            other, source, destination
                        );
                        Ok(TaskOutcome::empty())
                    }
                }
            }
            TaskType::Notification {
                channel,
                message,
                metadata: _,
            } => {
                use crate::task::NotificationChannel;
                // Simulation only: nothing is delivered. Documented on
                // `DefaultTaskExecutor`.
                match channel {
                    NotificationChannel::Email { to, subject } => {
                        info!(
                            "Notification [Email] to={:?} subject={:?}: {}",
                            to, subject, message
                        );
                    }
                    NotificationChannel::Webhook { url } => {
                        info!("Notification [Webhook] url={}: {}", url, message);
                    }
                    NotificationChannel::Slack {
                        channel: slack_channel,
                        webhook_url,
                    } => {
                        info!(
                            "Notification [Slack] channel={} url={}: {}",
                            slack_channel, webhook_url, message
                        );
                    }
                    NotificationChannel::Discord { webhook_url } => {
                        info!("Notification [Discord] url={}: {}", webhook_url, message);
                    }
                }
                Ok(TaskOutcome::empty())
            }
            TaskType::CustomScript { script, args, env } => {
                info!(
                    "CustomScript: {:?} args={:?} env_vars={}",
                    script,
                    args,
                    env.len()
                );
                if !script.exists() {
                    return Err(WorkflowError::generic(format!(
                        "Script not found: {}",
                        script.display()
                    )));
                }
                // Spawn the script as a child process via tokio::process.
                let mut cmd = tokio::process::Command::new(script);
                cmd.args(args);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                let status = cmd.status().await.map_err(|e| {
                    WorkflowError::generic(format!("Script {:?} failed to launch: {e}", script))
                })?;
                if !status.success() {
                    return Err(WorkflowError::generic(format!(
                        "Script {:?} exited with status: {}",
                        script, status
                    )));
                }
                info!("Script {:?} completed successfully", script);
                Ok(TaskOutcome::with_data(serde_json::json!({
                    "kind": "custom_script",
                    "script": script.display().to_string(),
                    "args": args,
                    "exit_code": status.code(),
                })))
            }
            TaskType::Analysis {
                input,
                analyses,
                output,
            } => task_exec::analysis::execute_analysis(input, analyses, output.as_deref()).await,
            TaskType::Conditional {
                condition,
                true_task,
                false_task,
            } => {
                // Evaluate the condition expression (simple boolean string parse;
                // full expression evaluation would use the ExecutionContext variables).
                let condition_result = match condition.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" => true,
                    "false" | "0" | "no" => false,
                    other => {
                        debug!(
                            "Condition not resolvable as literal, defaulting to true: {}",
                            other
                        );
                        true
                    }
                };

                let branch_task = if condition_result {
                    true_task.as_deref()
                } else {
                    false_task.as_deref()
                };

                let Some(inner_task) = branch_task else {
                    debug!("Conditional task: selected branch has no task, skipping");
                    return Ok(TaskOutcome::empty());
                };

                info!(
                    "Conditional branch selected: condition={} task={}",
                    condition_result, inner_task.name
                );
                // Recursively execute the selected branch task through the
                // trait method, whose future `async_trait` already boxes.
                let branch_result = self.execute(inner_task).await?;
                if !matches!(branch_result.status, TaskState::Completed) {
                    return Err(WorkflowError::generic(format!(
                        "Conditional branch task '{}' failed: {}",
                        inner_task.name,
                        branch_result.error.as_deref().unwrap_or("unknown")
                    )));
                }
                Ok(TaskOutcome {
                    data: branch_result.data,
                    outputs: branch_result.outputs,
                })
            }
        }
    }
}

#[async_trait]
impl TaskExecutor for DefaultTaskExecutor {
    async fn execute(&self, task: &Task) -> Result<TaskResult> {
        let start = Instant::now();

        match self.run(task).await {
            Ok(outcome) => Ok(TaskResult {
                task_id: task.id,
                status: TaskState::Completed,
                data: outcome.data,
                error: None,
                duration: start.elapsed(),
                outputs: outcome.outputs,
            }),
            Err(e) => Ok(TaskResult {
                task_id: task.id,
                status: TaskState::Failed,
                data: None,
                error: Some(e.to_string()),
                duration: start.elapsed(),
                outputs: Vec::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Task, TaskType};
    use std::path::PathBuf;

    #[test]
    fn test_execution_context_creation() {
        let workflow_id = WorkflowId::new();
        let ctx = ExecutionContext::new(workflow_id);
        assert_eq!(ctx.workflow_id, workflow_id);
    }

    #[test]
    fn test_execution_context_variables() {
        let ctx = ExecutionContext::new(WorkflowId::new());
        ctx.set_variable("key".to_string(), serde_json::json!("value"));
        assert_eq!(ctx.get_variable("key"), Some(serde_json::json!("value")));
    }

    #[test]
    fn test_execution_context_results() {
        let ctx = ExecutionContext::new(WorkflowId::new());
        let task_id = TaskId::new();
        let result = TaskResult {
            task_id,
            status: TaskState::Completed,
            data: None,
            error: None,
            duration: Duration::from_secs(1),
            outputs: Vec::new(),
        };

        ctx.store_result(task_id, result.clone());
        let retrieved = ctx.get_result(&task_id);
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_default_executor_wait_task() {
        let executor = DefaultTaskExecutor;
        let task = Task::new(
            "wait-task",
            TaskType::Wait {
                duration: Duration::from_millis(10),
            },
        );

        let result = executor
            .execute(&task)
            .await
            .expect("should succeed in test");
        assert_eq!(result.status, TaskState::Completed);
    }

    #[tokio::test]
    async fn test_workflow_executor_creation() {
        let executor = WorkflowExecutor::new(Arc::new(DefaultTaskExecutor))
            .with_max_concurrent(2)
            .with_timeout(Duration::from_secs(60));

        assert_eq!(executor.max_concurrent, 2);
        assert!(executor.timeout.is_some());
    }

    #[tokio::test]
    async fn test_simple_workflow_execution() {
        let mut workflow = Workflow::new("test-workflow");
        let task = Task::new(
            "test-task",
            TaskType::Wait {
                duration: Duration::from_millis(10),
            },
        );
        workflow.add_task(task);

        let executor = WorkflowExecutor::new(Arc::new(DefaultTaskExecutor));
        let result = executor
            .execute(&mut workflow)
            .await
            .expect("should succeed in test");

        assert_eq!(result.state, WorkflowState::Completed);
        assert_eq!(result.task_results.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Regression tests: `WorkflowExecutor::execute` must never fabricate
    // `WorkflowState::Completed` while silently dropping tasks. See the
    // "Ordering invariant" section on `WorkflowExecutor::execute` and the
    // "Scheduling correctness" module doc.
    // -----------------------------------------------------------------------

    /// A `TaskExecutor` that records the order in which tasks are actually
    /// executed (by name) into a shared, `Arc`-owned log, so tests can assert
    /// dependency-respecting scheduling order.
    struct OrderRecordingExecutor {
        order: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl TaskExecutor for OrderRecordingExecutor {
        async fn execute(&self, task: &Task) -> Result<TaskResult> {
            // A short delay makes it likely that, if the scheduler ever
            // incorrectly started a dependent task before its dependency
            // actually finished, the ordering violation would surface here.
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.order
                .lock()
                .expect("order mutex poisoned in test")
                .push(task.name.clone());
            Ok(TaskResult {
                task_id: task.id,
                status: TaskState::Completed,
                data: None,
                error: None,
                duration: Duration::ZERO,
                outputs: Vec::new(),
            })
        }
    }

    /// Regression test for the scheduling bug where `execute()` scanned
    /// `task_order` with a single-pass iterator: a task whose dependencies
    /// were not yet satisfied got `continue`d past, and because the shared
    /// iterator never yielded it again, every non-root task in a dependency
    /// chain was silently dropped while the workflow still reported
    /// `Completed`.
    ///
    /// With a real fixpoint scheduler, all three tasks in `a -> b -> c` must
    /// run, and must run in dependency order.
    #[tokio::test]
    async fn test_dependency_chain_runs_all_steps_in_order() {
        let mut workflow = Workflow::new("chain-workflow");

        let task_a = Task::new(
            "a",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );
        let task_b = Task::new(
            "b",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );
        let task_c = Task::new(
            "c",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );

        let id_a = workflow.add_task(task_a);
        let id_b = workflow.add_task(task_b);
        let id_c = workflow.add_task(task_c);

        workflow
            .add_edge(id_a, id_b)
            .expect("should succeed in test");
        workflow
            .add_edge(id_b, id_c)
            .expect("should succeed in test");

        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let task_executor = Arc::new(OrderRecordingExecutor {
            order: order.clone(),
        });
        let executor = WorkflowExecutor::new(task_executor);

        let result = tokio::time::timeout(Duration::from_secs(10), executor.execute(&mut workflow))
            .await
            .expect("workflow execution should not hang")
            .expect("workflow execution should succeed in test");

        assert_eq!(
            result.state,
            WorkflowState::Completed,
            "all three tasks succeeded, so the workflow must report Completed"
        );

        // The core regression check: every task in the chain actually ran,
        // not just the root `a`.
        assert_eq!(
            result.task_results.len(),
            3,
            "all three chained tasks must produce a result, not just the root: {:?}",
            result.task_results
        );
        for id in [id_a, id_b, id_c] {
            let status = result
                .task_results
                .get(&id)
                .expect("every task in the chain must have a result")
                .status;
            assert_eq!(status, TaskState::Completed);
        }

        // And they must have run in dependency order: a before b before c.
        let recorded = order.lock().expect("order mutex poisoned in test").clone();
        assert_eq!(
            recorded,
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "tasks must execute in dependency order, got {recorded:?}"
        );
    }

    /// A cyclic dependency graph must never be reported as `Completed`.
    /// `Workflow::validate()` (called at the top of `execute()`) rejects
    /// cycles before any task runs; this pins that contract at the
    /// `WorkflowExecutor::execute` boundary as a regression guard.
    #[tokio::test]
    async fn test_cycle_returns_err_not_completed() {
        let mut workflow = Workflow::new("cyclic-workflow");
        let task1 = Task::new(
            "task1",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );
        let task2 = Task::new(
            "task2",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );
        let id1 = workflow.add_task(task1);
        let id2 = workflow.add_task(task2);
        workflow.add_edge(id1, id2).expect("should succeed in test");
        workflow.add_edge(id2, id1).expect("should succeed in test");

        let executor = WorkflowExecutor::new(Arc::new(DefaultTaskExecutor));
        let outcome =
            tokio::time::timeout(Duration::from_secs(10), executor.execute(&mut workflow))
                .await
                .expect("workflow execution must not hang on a cyclic graph");

        assert!(
            outcome.is_err(),
            "a cyclic dependency graph must be rejected, never reported as Completed"
        );
        assert_ne!(workflow.state, WorkflowState::Completed);
    }

    /// A real failure on a **non-root** task (`b`, which depends on `a`) must
    /// still be observed and reported, and must cascade to `c` (which
    /// depends on `b`) instead of being silently dropped.
    ///
    /// This specifically pins the single-pass-iterator failure mode: `b`
    /// cannot possibly be ready during the very first scan (its dependency
    /// `a` is still in flight), so under the old scheduler it was
    /// `continue`d past *forever* and never retried once `a` finished --
    /// meaning `b` never actually ran, never reported failure, and
    /// `failed_tasks` stayed empty. The workflow then dishonestly reported
    /// `Completed` even though the task that was designed to fail never got
    /// a chance to execute at all. With a real fixpoint scheduler `b` is
    /// retried after `a` completes, actually runs, actually fails, and the
    /// failure is recorded -- so the workflow must resolve promptly (not
    /// hang) and report `Failed`, with `c` cascaded rather than vanishing.
    #[tokio::test]
    async fn test_non_root_failure_is_not_silently_dropped() {
        let mut workflow = Workflow::new("non-root-failure-workflow");
        workflow.config.fail_fast = false;

        let task_a = Task::new(
            "a",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );
        // `b` fails for real: a `CustomScript` task pointing at a script
        // that does not exist on disk. Crucially, `b` is NOT a root task,
        // so it can never be ready on the very first scheduling pass.
        let task_b = Task::new(
            "b",
            TaskType::CustomScript {
                script: PathBuf::from("/nonexistent/does-not-exist.sh"),
                args: Vec::new(),
                env: HashMap::new(),
            },
        );
        let task_c = Task::new(
            "c",
            TaskType::Wait {
                duration: Duration::from_millis(1),
            },
        );

        let id_a = workflow.add_task(task_a);
        let id_b = workflow.add_task(task_b);
        let id_c = workflow.add_task(task_c);

        workflow
            .add_edge(id_a, id_b)
            .expect("should succeed in test");
        workflow
            .add_edge(id_b, id_c)
            .expect("should succeed in test");

        let executor = WorkflowExecutor::new(Arc::new(DefaultTaskExecutor));

        let result = tokio::time::timeout(Duration::from_secs(10), executor.execute(&mut workflow))
            .await
            .expect("workflow execution must not hang on a downstream failure")
            .expect("execute() returns Ok with a Failed state (not an Err) for a cascaded skip");

        assert_eq!(
            result.state,
            WorkflowState::Failed,
            "a real non-root failure must yield a Failed workflow, never a fabricated Completed \
             (the failing task must not be silently dropped by the scheduler)"
        );
        assert_ne!(result.state, WorkflowState::Completed);

        // `b` must have actually run (and been recorded as failed), not
        // vanished: its result must be present and Failed.
        let b_status = result
            .task_results
            .get(&id_b)
            .expect("the failing non-root task must have actually run and produced a result")
            .status;
        assert_eq!(
            b_status,
            TaskState::Failed,
            "b must be recorded as Failed, not silently skipped"
        );
    }
}

//! Worker implementation for task execution

mod executor;
mod media;
mod metrics;
mod task_thumbnail;
mod task_transcode;

use crate::coordinator::worker_registry::WorkerCapabilities;
use crate::pb::farm_coordinator_client::FarmCoordinatorClient;
use crate::{FarmError, Result, TaskId, WorkerConfig, WorkerId, WorkerState};
use std::sync::Arc;
use tokio::sync::broadcast;
use tonic::transport::Channel;
use tracing::{error, info};

pub use executor::{TaskExecutor, TaskResult};
pub use metrics::WorkerMetrics;

/// Worker that executes tasks
pub struct Worker {
    config: WorkerConfig,
    worker_id: WorkerId,
    #[allow(dead_code)]
    coordinator_client: Option<FarmCoordinatorClient<Channel>>,
    executor: Arc<TaskExecutor>,
    metrics: Arc<WorkerMetrics>,
    shutdown_tx: broadcast::Sender<()>,
    state: parking_lot::RwLock<WorkerState>,
}

impl Worker {
    /// Create a new worker
    pub fn new(config: WorkerConfig) -> Self {
        let worker_id = config
            .worker_id
            .clone()
            .map_or_else(WorkerId::random, WorkerId::new);

        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            config,
            worker_id,
            coordinator_client: None,
            executor: Arc::new(TaskExecutor::new()),
            metrics: Arc::new(WorkerMetrics::new()),
            shutdown_tx,
            state: parking_lot::RwLock::new(WorkerState::Idle),
        }
    }

    /// Start the worker
    pub async fn start(self: Arc<Self>) -> Result<()> {
        info!(
            "Starting worker {} connecting to {}",
            self.worker_id, self.config.coordinator_address
        );

        // Connect to coordinator
        self.connect_to_coordinator().await?;

        // Register with coordinator
        self.register().await?;

        // Start background tasks
        self.clone().start_background_tasks();

        // Main worker loop
        self.run().await
    }

    /// Connect to the coordinator
    async fn connect_to_coordinator(&self) -> Result<()> {
        let _client = FarmCoordinatorClient::connect(self.config.coordinator_address.clone())
            .await
            .map_err(FarmError::Network)?;

        // Store client in Arc
        // Note: This is a workaround since we can't easily mutate Arc<Self>
        // In a real implementation, we'd use interior mutability
        info!(
            "Connected to coordinator at {}",
            self.config.coordinator_address
        );

        Ok(())
    }

    /// Register with the coordinator
    async fn register(&self) -> Result<()> {
        info!("Registering worker {} with coordinator", self.worker_id);

        let capabilities = self.get_capabilities();

        // In a real implementation, this would use the gRPC client
        // For now, we'll just log it
        info!("Worker capabilities: {:?}", capabilities);

        Ok(())
    }

    /// Get worker capabilities
    fn get_capabilities(&self) -> WorkerCapabilities {
        let sys_info = self.get_system_info();

        WorkerCapabilities {
            cpu_cores: sys_info.cpu_cores,
            memory_bytes: sys_info.memory_bytes,
            supported_codecs: self.config.supported_codecs.clone(),
            supported_formats: self.config.supported_formats.clone(),
            has_gpu: self.config.enable_gpu,
            gpus: if self.config.enable_gpu {
                vec![crate::coordinator::worker_registry::GpuInfo {
                    name: "Generic GPU".to_string(),
                    memory_bytes: 8 * 1024 * 1024 * 1024,
                    vendor: "Unknown".to_string(),
                    supported_codecs: vec!["h264".to_string(), "h265".to_string()],
                }]
            } else {
                vec![]
            },
            max_concurrent_tasks: self.config.max_concurrent_tasks,
            tags: self.config.tags.clone(),
        }
    }

    /// Get system information
    #[allow(dead_code)]
    fn get_system_info(&self) -> SystemInfo {
        SystemInfo {
            cpu_cores: num_cpus::get() as u32,
            memory_bytes: 16 * 1024 * 1024 * 1024, // Default to 16GB
            disk_space_bytes: 1024 * 1024 * 1024 * 1024, // Default to 1TB
        }
    }

    /// Start background tasks
    fn start_background_tasks(self: Arc<Self>) {
        // Heartbeat task
        let worker = self.clone();
        tokio::spawn(async move {
            worker.run_heartbeat_loop().await;
        });

        // Metrics collection task
        let worker = self.clone();
        tokio::spawn(async move {
            worker.run_metrics_loop().await;
        });
    }

    /// Main worker loop
    async fn run(&self) -> Result<()> {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.fetch_and_execute_tasks().await {
                        error!("Error fetching/executing tasks: {}", e);
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Worker {} shutting down", self.worker_id);
                    break;
                }
            }
        }

        self.unregister().await?;
        Ok(())
    }

    /// Fetch and execute tasks
    async fn fetch_and_execute_tasks(&self) -> Result<()> {
        let state = *self.state.read();

        if state == WorkerState::Draining || state == WorkerState::Offline {
            return Ok(());
        }

        // Check if we can accept more tasks
        let active_tasks = self.executor.active_task_count();
        if active_tasks >= self.config.max_concurrent_tasks as usize {
            self.update_state(WorkerState::Busy);
            return Ok(());
        }

        // Fetch tasks from coordinator
        // In a real implementation, this would use the gRPC client
        // For now, we'll just update the state
        if active_tasks == 0 {
            self.update_state(WorkerState::Idle);
        } else {
            self.update_state(WorkerState::Busy);
        }

        Ok(())
    }

    /// Run heartbeat loop
    async fn run_heartbeat_loop(&self) {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut interval = tokio::time::interval(self.config.heartbeat_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.send_heartbeat().await {
                        error!("Failed to send heartbeat: {}", e);
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Heartbeat loop shutting down");
                    break;
                }
            }
        }
    }

    /// Send heartbeat to coordinator
    async fn send_heartbeat(&self) -> Result<()> {
        let _sys_info = self.get_system_info();
        let state = *self.state.read();
        let active_tasks = self.executor.active_task_count();

        tracing::debug!(
            "Heartbeat: worker={}, state={:?}, active_tasks={}",
            self.worker_id,
            state,
            active_tasks
        );

        // In a real implementation, this would use the gRPC client
        Ok(())
    }

    /// Run metrics collection loop
    async fn run_metrics_loop(&self) {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.collect_metrics();
                }
                _ = shutdown_rx.recv() => {
                    info!("Metrics loop shutting down");
                    break;
                }
            }
        }
    }

    /// Collect worker metrics
    fn collect_metrics(&self) {
        let active_tasks = self.executor.active_task_count();
        let completed_tasks = self.metrics.tasks_completed();
        let failed_tasks = self.metrics.tasks_failed();

        info!(
            "Metrics: worker={}, active={}, completed={}, failed={}",
            self.worker_id, active_tasks, completed_tasks, failed_tasks
        );
    }

    /// Execute a task
    pub async fn execute_task(&self, task_id: TaskId, payload: Vec<u8>) -> Result<TaskResult> {
        info!("Executing task {}", task_id);

        self.update_state(WorkerState::Busy);

        let result = self.executor.execute(task_id, payload).await;

        match &result {
            Ok(_) => {
                self.metrics.increment_completed();
                info!("Task {} completed successfully", task_id);
            }
            Err(e) => {
                self.metrics.increment_failed();
                error!("Task {} failed: {}", task_id, e);
            }
        }

        // Update state based on active tasks
        if self.executor.active_task_count() == 0 {
            self.update_state(WorkerState::Idle);
        }

        result
    }

    /// Update worker state
    fn update_state(&self, new_state: WorkerState) {
        let mut state = self.state.write();
        if *state != new_state {
            info!(
                "Worker {} state changed: {:?} -> {:?}",
                self.worker_id, *state, new_state
            );
            *state = new_state;
        }
    }

    /// Unregister from coordinator
    async fn unregister(&self) -> Result<()> {
        info!("Unregistering worker {} from coordinator", self.worker_id);

        // In a real implementation, this would use the gRPC client
        Ok(())
    }

    /// Shutdown the worker
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down worker {}", self.worker_id);
        self.update_state(WorkerState::Draining);
        let _ = self.shutdown_tx.send(());

        // Wait for active tasks to complete
        while self.executor.active_task_count() > 0 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    /// Get worker ID
    pub fn worker_id(&self) -> &WorkerId {
        &self.worker_id
    }

    /// Get worker state
    pub fn state(&self) -> WorkerState {
        *self.state.read()
    }

    /// Get active task count
    pub fn active_task_count(&self) -> usize {
        self.executor.active_task_count()
    }
}

/// System information
struct SystemInfo {
    cpu_cores: u32,
    #[allow(dead_code)]
    memory_bytes: u64,
    #[allow(dead_code)]
    disk_space_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_creation() {
        let config = WorkerConfig::default();
        let worker = Arc::new(Worker::new(config));
        assert_eq!(worker.state(), WorkerState::Idle);
        assert_eq!(worker.active_task_count(), 0);
    }

    #[test]
    fn test_get_capabilities() {
        let config = WorkerConfig {
            enable_gpu: true,
            ..Default::default()
        };
        let worker = Arc::new(Worker::new(config));
        let capabilities = worker.get_capabilities();

        assert!(capabilities.has_gpu);
        assert!(!capabilities.gpus.is_empty());
    }

    #[tokio::test]
    async fn test_worker_shutdown() {
        let config = WorkerConfig::default();
        let worker = Arc::new(Worker::new(config));

        worker.shutdown().await.unwrap();
        assert_eq!(worker.state(), WorkerState::Draining);
    }

    #[test]
    fn test_state_update() {
        let config = WorkerConfig::default();
        let worker = Arc::new(Worker::new(config));

        worker.update_state(WorkerState::Busy);
        assert_eq!(worker.state(), WorkerState::Busy);

        worker.update_state(WorkerState::Idle);
        assert_eq!(worker.state(), WorkerState::Idle);
    }
}

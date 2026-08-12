#![allow(dead_code)]
//! Resource pool — GPU and hardware resource allocation across workers.
//!
//! Workers submit resource requests and the pool grants leases when resources
//! are available.  Leases are returned explicitly or by `Drop`.
//!
//! # Design
//! - `ResourcePool` owns a fixed set of named `ResourceSlot`s (e.g. GPU 0, GPU 1).
//! - Workers call `try_acquire` or `acquire_blocking` to get a `ResourceLease`.
//! - `ResourceLease` releases the slot when dropped.
//! - Stats are tracked per slot (total grants, total time held).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Error type for resource pool operations.
#[derive(Debug, thiserror::Error)]
pub enum ResourcePoolError {
    /// No slot with the given ID exists.
    #[error("Unknown slot: {0}")]
    UnknownSlot(String),
    /// The requested slot is currently in use.
    #[error("Slot busy: {0}")]
    SlotBusy(String),
    /// Timed out waiting for a slot.
    #[error("Timed out waiting for slot: {0}")]
    Timeout(String),
    /// Pool is at capacity (all slots busy).
    #[error("Pool at capacity")]
    AtCapacity,
    /// Internal lock was poisoned.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Type of hardware resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    /// CUDA/OpenCL GPU.
    Gpu,
    /// NPU / neural processing unit.
    Npu,
    /// Custom FPGA accelerator.
    Fpga,
    /// Generic hardware slot.
    Generic,
}

impl ResourceType {
    /// Short string label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            ResourceType::Gpu => "gpu",
            ResourceType::Npu => "npu",
            ResourceType::Fpga => "fpga",
            ResourceType::Generic => "generic",
        }
    }
}

/// Description of a single hardware resource slot.
#[derive(Debug, Clone)]
pub struct ResourceSlotDesc {
    /// Unique slot identifier (e.g. "gpu-0").
    pub id: String,
    /// Resource type.
    pub resource_type: ResourceType,
    /// Display name (e.g. "NVIDIA RTX 4090").
    pub display_name: String,
    /// Compute capacity rating (arbitrary unit, for scheduling hints).
    pub capacity: u32,
}

impl ResourceSlotDesc {
    /// Create a new slot descriptor.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        resource_type: ResourceType,
        display_name: impl Into<String>,
        capacity: u32,
    ) -> Self {
        Self {
            id: id.into(),
            resource_type,
            display_name: display_name.into(),
            capacity,
        }
    }
}

/// Statistics accumulated for one resource slot.
#[derive(Debug, Default, Clone)]
pub struct SlotStats {
    /// How many times this slot has been granted.
    pub total_grants: u64,
    /// Total duration this slot was held (cumulative).
    pub total_held: Duration,
}

/// Internal mutable state for one slot.
#[derive(Debug)]
struct SlotState {
    desc: ResourceSlotDesc,
    /// Whether the slot is currently occupied.
    busy: bool,
    /// Worker that currently holds the slot (if any).
    current_holder: Option<String>,
    /// When the current lease started.
    lease_started: Option<Instant>,
    stats: SlotStats,
}

impl SlotState {
    fn new(desc: ResourceSlotDesc) -> Self {
        Self {
            desc,
            busy: false,
            current_holder: None,
            lease_started: None,
            stats: SlotStats::default(),
        }
    }
}

struct PoolInner {
    slots: HashMap<String, SlotState>,
}

impl PoolInner {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    fn add_slot(&mut self, desc: ResourceSlotDesc) {
        self.slots.insert(desc.id.clone(), SlotState::new(desc));
    }

    fn try_acquire_slot(&mut self, slot_id: &str, holder: &str) -> Result<(), ResourcePoolError> {
        let slot = self
            .slots
            .get_mut(slot_id)
            .ok_or_else(|| ResourcePoolError::UnknownSlot(slot_id.to_string()))?;
        if slot.busy {
            return Err(ResourcePoolError::SlotBusy(slot_id.to_string()));
        }
        slot.busy = true;
        slot.current_holder = Some(holder.to_string());
        slot.lease_started = Some(Instant::now());
        slot.stats.total_grants += 1;
        Ok(())
    }

    fn try_acquire_any(
        &mut self,
        resource_type: Option<ResourceType>,
        holder: &str,
    ) -> Result<String, ResourcePoolError> {
        let slot_id = self
            .slots
            .iter()
            .filter(|(_, s)| !s.busy && resource_type.map_or(true, |rt| s.desc.resource_type == rt))
            .max_by_key(|(_, s)| s.desc.capacity)
            .map(|(id, _)| id.clone())
            .ok_or(ResourcePoolError::AtCapacity)?;
        self.try_acquire_slot(&slot_id, holder)?;
        Ok(slot_id)
    }

    fn release_slot(&mut self, slot_id: &str) {
        if let Some(slot) = self.slots.get_mut(slot_id) {
            if let Some(started) = slot.lease_started.take() {
                slot.stats.total_held += started.elapsed();
            }
            slot.busy = false;
            slot.current_holder = None;
        }
    }
}

/// A lease on a resource slot.
///
/// The slot is automatically released when the lease is dropped.
pub struct ResourceLease {
    slot_id: String,
    pool: Arc<(Mutex<PoolInner>, Condvar)>,
}

impl ResourceLease {
    /// The slot ID held by this lease.
    #[must_use]
    pub fn slot_id(&self) -> &str {
        &self.slot_id
    }

    /// Release the lease early (same as drop).
    pub fn release(self) {
        // Dropping self will call `drop`, which releases the slot.
    }
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.pool.0.lock() {
            guard.release_slot(&self.slot_id);
            self.pool.1.notify_all();
        }
    }
}

impl std::fmt::Debug for ResourceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceLease")
            .field("slot_id", &self.slot_id)
            .finish()
    }
}

/// Shared pool of hardware resource slots.
#[derive(Clone)]
pub struct ResourcePool {
    inner: Arc<(Mutex<PoolInner>, Condvar)>,
}

impl ResourcePool {
    /// Create an empty resource pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(PoolInner::new()), Condvar::new())),
        }
    }

    /// Add a slot to the pool.  Panics if the ID is already registered.
    pub fn add_slot(&self, desc: ResourceSlotDesc) {
        // Recover from lock poisoning rather than propagating the panic:
        // the pool data itself stays structurally valid even if a previous
        // holder panicked mid-operation, consistent with the other
        // `ResourcePool` accessors below that tolerate poisoning.
        let mut guard = self.inner.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.add_slot(desc);
    }

    /// Convenience: add a GPU slot.
    pub fn add_gpu(&self, id: impl Into<String>, display_name: impl Into<String>, capacity: u32) {
        self.add_slot(ResourceSlotDesc::new(
            id,
            ResourceType::Gpu,
            display_name,
            capacity,
        ));
    }

    /// Attempt to acquire a specific slot for `holder` without blocking.
    pub fn try_acquire(
        &self,
        slot_id: &str,
        holder: &str,
    ) -> Result<ResourceLease, ResourcePoolError> {
        let mut guard = self
            .inner
            .0
            .lock()
            .map_err(|e| ResourcePoolError::Internal(e.to_string()))?;
        guard.try_acquire_slot(slot_id, holder)?;
        Ok(ResourceLease {
            slot_id: slot_id.to_string(),
            pool: Arc::clone(&self.inner),
        })
    }

    /// Attempt to acquire any available slot of the given type.
    pub fn try_acquire_any(
        &self,
        resource_type: Option<ResourceType>,
        holder: &str,
    ) -> Result<ResourceLease, ResourcePoolError> {
        let mut guard = self
            .inner
            .0
            .lock()
            .map_err(|e| ResourcePoolError::Internal(e.to_string()))?;
        let slot_id = guard.try_acquire_any(resource_type, holder)?;
        Ok(ResourceLease {
            slot_id,
            pool: Arc::clone(&self.inner),
        })
    }

    /// Block until a specific slot is available or timeout elapses.
    pub fn acquire_with_timeout(
        &self,
        slot_id: &str,
        holder: &str,
        timeout: Duration,
    ) -> Result<ResourceLease, ResourcePoolError> {
        let deadline = Instant::now() + timeout;
        let (lock, cvar) = &*self.inner;
        let mut guard = lock
            .lock()
            .map_err(|e| ResourcePoolError::Internal(e.to_string()))?;
        loop {
            match guard.try_acquire_slot(slot_id, holder) {
                Ok(()) => {
                    return Ok(ResourceLease {
                        slot_id: slot_id.to_string(),
                        pool: Arc::clone(&self.inner),
                    });
                }
                Err(ResourcePoolError::SlotBusy(_)) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(ResourcePoolError::Timeout(slot_id.to_string()));
                    }
                    let (new_guard, _) = cvar
                        .wait_timeout(guard, remaining)
                        .map_err(|e| ResourcePoolError::Internal(e.to_string()))?;
                    guard = new_guard;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Total number of registered slots.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.inner.0.lock().map(|g| g.slots.len()).unwrap_or(0)
    }

    /// Number of busy slots.
    #[must_use]
    pub fn busy_count(&self) -> usize {
        self.inner
            .0
            .lock()
            .map(|g| g.slots.values().filter(|s| s.busy).count())
            .unwrap_or(0)
    }

    /// Number of available (not busy) slots.
    #[must_use]
    pub fn available_count(&self) -> usize {
        self.slot_count() - self.busy_count()
    }

    /// Get statistics for a specific slot.
    pub fn slot_stats(&self, slot_id: &str) -> Option<SlotStats> {
        self.inner
            .0
            .lock()
            .ok()
            .and_then(|g| g.slots.get(slot_id).map(|s| s.stats.clone()))
    }
}

impl Default for ResourcePool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_with_gpus(count: usize) -> ResourcePool {
        let pool = ResourcePool::new();
        for i in 0..count {
            pool.add_gpu(format!("gpu-{i}"), format!("GPU {i}"), (i as u32 + 1) * 100);
        }
        pool
    }

    #[test]
    fn test_add_and_count() {
        let pool = pool_with_gpus(2);
        assert_eq!(pool.slot_count(), 2);
        assert_eq!(pool.available_count(), 2);
        assert_eq!(pool.busy_count(), 0);
    }

    #[test]
    fn test_try_acquire_success() {
        let pool = pool_with_gpus(1);
        let lease = pool.try_acquire("gpu-0", "worker-1").expect("acquire");
        assert_eq!(lease.slot_id(), "gpu-0");
        assert_eq!(pool.busy_count(), 1);
    }

    #[test]
    fn test_try_acquire_busy_returns_error() {
        let pool = pool_with_gpus(1);
        let _lease = pool
            .try_acquire("gpu-0", "worker-1")
            .expect("first acquire");
        let result = pool.try_acquire("gpu-0", "worker-2");
        assert!(matches!(result, Err(ResourcePoolError::SlotBusy(_))));
    }

    #[test]
    fn test_lease_drop_releases_slot() {
        let pool = pool_with_gpus(1);
        {
            let _lease = pool.try_acquire("gpu-0", "worker-1").expect("acquire");
            assert_eq!(pool.busy_count(), 1);
        } // lease dropped
        assert_eq!(pool.busy_count(), 0);
    }

    #[test]
    fn test_try_acquire_any_prefers_higher_capacity() {
        let pool = ResourcePool::new();
        pool.add_gpu("gpu-low", "Low GPU", 100);
        pool.add_gpu("gpu-high", "High GPU", 800);
        let lease = pool
            .try_acquire_any(Some(ResourceType::Gpu), "worker-1")
            .expect("acquire_any");
        assert_eq!(lease.slot_id(), "gpu-high"); // higher capacity preferred
    }

    #[test]
    fn test_try_acquire_any_at_capacity_returns_error() {
        let pool = pool_with_gpus(1);
        let _lease = pool.try_acquire("gpu-0", "w1").expect("first");
        let result = pool.try_acquire_any(Some(ResourceType::Gpu), "w2");
        assert!(matches!(result, Err(ResourcePoolError::AtCapacity)));
    }

    #[test]
    fn test_unknown_slot_returns_error() {
        let pool = pool_with_gpus(1);
        let result = pool.try_acquire("nonexistent", "worker-1");
        assert!(matches!(result, Err(ResourcePoolError::UnknownSlot(_))));
    }

    #[test]
    fn test_slot_stats_updated_on_release() {
        let pool = pool_with_gpus(1);
        {
            let _lease = pool.try_acquire("gpu-0", "worker-1").expect("acquire");
        }
        let stats = pool.slot_stats("gpu-0").expect("stats");
        assert_eq!(stats.total_grants, 1);
    }

    #[test]
    fn test_acquire_with_timeout_immediate_success() {
        let pool = pool_with_gpus(1);
        let lease = pool
            .acquire_with_timeout("gpu-0", "w1", Duration::from_millis(100))
            .expect("timed acquire");
        assert_eq!(lease.slot_id(), "gpu-0");
    }

    #[test]
    fn test_acquire_with_timeout_fails_when_busy() {
        let pool = pool_with_gpus(1);
        let _lease = pool.try_acquire("gpu-0", "w1").expect("first");
        let result = pool.acquire_with_timeout("gpu-0", "w2", Duration::from_millis(20));
        assert!(matches!(result, Err(ResourcePoolError::Timeout(_))));
    }

    #[test]
    fn test_resource_type_label() {
        assert_eq!(ResourceType::Gpu.label(), "gpu");
        assert_eq!(ResourceType::Npu.label(), "npu");
        assert_eq!(ResourceType::Fpga.label(), "fpga");
        assert_eq!(ResourceType::Generic.label(), "generic");
    }
}

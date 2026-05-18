//! Shared memory for low-latency time access.
//!
#![allow(unsafe_code)]
//!
//! On Unix platforms the backing store is a regular file mapped with `memmap2`.
//! The `SharedTimeData` structure is placed at offset 0 of the mapping and
//! accessed via atomic operations so that readers and a single writer can
//! share it without any additional locking.
//!
//! On non-Unix targets (e.g. WASM) a purely in-process fallback is provided
//! that satisfies the same API but does not offer inter-process sharing.

use crate::error::{TimeSyncError, TimeSyncResult};
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Shared data structure
// ---------------------------------------------------------------------------

/// Shared memory time data structure.
///
/// This layout is `repr(C)` so that its byte offsets are stable across
/// compilations of producer and consumer processes.
#[repr(C)]
pub struct SharedTimeData {
    /// Sequence number (odd while writing, even when idle).
    sequence: AtomicU64,
    /// Timestamp (nanoseconds since Unix epoch).
    timestamp_ns: AtomicU64,
    /// Offset from reference clock (nanoseconds).
    offset_ns: AtomicI64,
    /// Frequency offset stored as `ppb * 1000` for integer precision.
    freq_offset_ppb_scaled: AtomicI64,
    /// Non-zero when clock is considered synchronized.
    synchronized: AtomicU64,
}

impl SharedTimeData {
    /// Initialize a new `SharedTimeData` in place (all fields zeroed).
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            timestamp_ns: AtomicU64::new(0),
            offset_ns: AtomicI64::new(0),
            freq_offset_ppb_scaled: AtomicI64::new(0),
            synchronized: AtomicU64::new(0),
        }
    }

    /// Write time data using a seqlock protocol (lock-free, single-writer safe).
    pub fn write(
        &self,
        timestamp_ns: u64,
        offset_ns: i64,
        freq_offset_ppb: f64,
        synchronized: bool,
    ) {
        // Mark as "writing" (odd sequence).
        let seq = self.sequence.fetch_add(1, Ordering::Release);

        self.timestamp_ns.store(timestamp_ns, Ordering::Relaxed);
        self.offset_ns.store(offset_ns, Ordering::Relaxed);
        self.freq_offset_ppb_scaled
            .store((freq_offset_ppb * 1000.0) as i64, Ordering::Relaxed);
        self.synchronized
            .store(u64::from(synchronized), Ordering::Relaxed);

        // Mark as "done" (even sequence = seq + 2).
        self.sequence.store(seq + 2, Ordering::Release);
    }

    /// Read time data with seqlock retry for consistency.
    pub fn read(&self) -> TimeSyncResult<TimeSnapshot> {
        const MAX_RETRIES: usize = 10;

        for _ in 0..MAX_RETRIES {
            let seq1 = self.sequence.load(Ordering::Acquire);

            // Spin while a write is in progress (odd sequence).
            if seq1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }

            let timestamp_ns = self.timestamp_ns.load(Ordering::Relaxed);
            let offset_ns = self.offset_ns.load(Ordering::Relaxed);
            let freq_offset_ppb_scaled = self.freq_offset_ppb_scaled.load(Ordering::Relaxed);
            let synchronized = self.synchronized.load(Ordering::Relaxed);

            let seq2 = self.sequence.load(Ordering::Acquire);

            if seq1 == seq2 {
                return Ok(TimeSnapshot {
                    timestamp_ns,
                    offset_ns,
                    freq_offset_ppb: freq_offset_ppb_scaled as f64 / 1000.0,
                    synchronized: synchronized != 0,
                });
            }

            std::hint::spin_loop();
        }

        Err(TimeSyncError::SharedMemory(
            "Failed to read consistent time data after retries".to_string(),
        ))
    }
}

impl Default for SharedTimeData {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Point-in-time snapshot of the shared time state.
#[derive(Debug, Clone, Copy)]
pub struct TimeSnapshot {
    /// Timestamp (nanoseconds since Unix epoch).
    pub timestamp_ns: u64,
    /// Offset from reference clock (nanoseconds).
    pub offset_ns: i64,
    /// Frequency offset (ppb).
    pub freq_offset_ppb: f64,
    /// Whether the clock is synchronized.
    pub synchronized: bool,
}

// ---------------------------------------------------------------------------
// Manager — Unix (memmap2-backed)
// ---------------------------------------------------------------------------

/// Minimum mapping size: one `SharedTimeData` worth of bytes, rounded up to
/// a reasonable page-aligned boundary.
#[cfg(unix)]
const MIN_MAP_SIZE: u64 = 4096;

/// Shared memory manager backed by a memory-mapped file.
///
/// On Unix the file can be placed on `tmpfs` (`/dev/shm` on Linux) or any
/// other path.  Multiple processes may open the same path; the writer calls
/// `update()` and readers call `read_snapshot()`.
#[cfg(unix)]
pub struct SharedMemoryManager {
    /// Memory-mapped region.  We keep the map alive for the lifetime of this
    /// struct and derive a `&SharedTimeData` pointer from it.
    _mmap: memmap2::MmapMut,
    /// Pointer into the mapped region, reinterpreted as `SharedTimeData`.
    data_ptr: *const SharedTimeData,
}

// SAFETY: `SharedTimeData` uses only atomics; sending/sharing across threads
// is fine as long as the pointer stays valid (it does, because `_mmap` keeps
// the mapping alive).
#[cfg(unix)]
unsafe impl Send for SharedMemoryManager {}
#[cfg(unix)]
unsafe impl Sync for SharedMemoryManager {}

#[cfg(unix)]
impl SharedMemoryManager {
    /// Create a new shared memory segment at `path`.
    ///
    /// If the file does not exist it is created.  The mapping is always at
    /// least `MIN_MAP_SIZE` bytes.  On creation the `SharedTimeData` region
    /// is zero-initialised by the OS (new file) or left as-is (existing file).
    pub fn new(path: &Path) -> TimeSyncResult<Self> {
        use std::fs::OpenOptions;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| {
                TimeSyncError::SharedMemory(format!(
                    "Cannot open/create shared memory file '{}': {e}",
                    path.display()
                ))
            })?;

        // Ensure the file is large enough.
        let meta = file.metadata().map_err(|e| {
            TimeSyncError::SharedMemory(format!("Cannot stat shared memory file: {e}"))
        })?;
        if meta.len() < MIN_MAP_SIZE {
            file.set_len(MIN_MAP_SIZE).map_err(|e| {
                TimeSyncError::SharedMemory(format!(
                    "Cannot resize shared memory file to {MIN_MAP_SIZE} bytes: {e}"
                ))
            })?;
        }

        // Map the file into memory.
        let mmap = unsafe {
            memmap2::MmapMut::map_mut(&file)
                .map_err(|e| TimeSyncError::SharedMemory(format!("mmap failed: {e}")))?
        };

        // Verify we have enough space for the structure.
        let needed = std::mem::size_of::<SharedTimeData>();
        if mmap.len() < needed {
            return Err(TimeSyncError::SharedMemory(format!(
                "Mapped region ({} bytes) is smaller than SharedTimeData ({needed} bytes)",
                mmap.len()
            )));
        }

        // Derive a pointer — the mapping's first bytes hold the SharedTimeData.
        // SAFETY: the mapping is at least `needed` bytes and is aligned to the
        // OS page size (which is always >= alignof(SharedTimeData) because
        // SharedTimeData only contains 8-byte atomics).
        #[allow(clippy::cast_ptr_alignment)]
        let data_ptr = mmap.as_ptr().cast::<SharedTimeData>();

        Ok(Self {
            _mmap: mmap,
            data_ptr,
        })
    }

    /// Get a reference to the shared data.
    pub fn data(&self) -> &SharedTimeData {
        // SAFETY: `data_ptr` was derived from a live mmap and the mapping
        // outlives any borrow of `self`.
        unsafe { &*self.data_ptr }
    }

    /// Write updated time data into shared memory.
    pub fn update(
        &self,
        timestamp_ns: u64,
        offset_ns: i64,
        freq_offset_ppb: f64,
        synchronized: bool,
    ) {
        self.data()
            .write(timestamp_ns, offset_ns, freq_offset_ppb, synchronized);
    }

    /// Read the current time snapshot from shared memory.
    pub fn read_snapshot(&self) -> TimeSyncResult<TimeSnapshot> {
        self.data().read()
    }
}

// ---------------------------------------------------------------------------
// Manager — non-Unix fallback (in-process only)
// ---------------------------------------------------------------------------

/// Non-Unix fallback: purely in-process, no inter-process sharing.
#[cfg(not(unix))]
pub struct SharedMemoryManager {
    data: SharedTimeData,
}

#[cfg(not(unix))]
impl SharedMemoryManager {
    /// Create a new (in-process only) shared memory manager.
    pub fn new(_path: &Path) -> TimeSyncResult<Self> {
        Ok(Self {
            data: SharedTimeData::new(),
        })
    }

    /// Get a reference to the shared data.
    pub fn data(&self) -> &SharedTimeData {
        &self.data
    }

    /// Write updated time data.
    pub fn update(
        &self,
        timestamp_ns: u64,
        offset_ns: i64,
        freq_offset_ppb: f64,
        synchronized: bool,
    ) {
        self.data
            .write(timestamp_ns, offset_ns, freq_offset_ppb, synchronized);
    }

    /// Read the current time snapshot.
    pub fn read_snapshot(&self) -> TimeSyncResult<TimeSnapshot> {
        self.data.read()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_time_data_roundtrip() {
        let data = SharedTimeData::new();
        data.write(1_000_000, 500, 12.5, true);
        let snap = data.read().expect("read must succeed");
        assert_eq!(snap.timestamp_ns, 1_000_000);
        assert_eq!(snap.offset_ns, 500);
        assert!((snap.freq_offset_ppb - 12.5).abs() < 0.01);
        assert!(snap.synchronized);
    }

    #[test]
    fn test_lock_free_sequential_writes() {
        let data = SharedTimeData::new();
        for i in 0..200u64 {
            data.write(i, i as i64, i as f64, true);
        }
        let snap = data.read().expect("read must succeed");
        assert!(snap.synchronized);
    }

    #[cfg(unix)]
    #[test]
    fn test_mmap_manager_create_and_read() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_timesync_test_shmem.bin");

        // Remove leftovers from previous runs.
        let _ = std::fs::remove_file(&path);

        let mgr = SharedMemoryManager::new(&path).expect("manager creation must succeed");
        mgr.update(9_999, -100, 0.25, false);

        let snap = mgr.read_snapshot().expect("read_snapshot must succeed");
        assert_eq!(snap.timestamp_ns, 9_999);
        assert_eq!(snap.offset_ns, -100);
        assert!((snap.freq_offset_ppb - 0.25).abs() < 0.01);
        assert!(!snap.synchronized);

        // Clean up.
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn test_mmap_manager_reopen() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_timesync_test_shmem_reopen.bin");
        let _ = std::fs::remove_file(&path);

        // Write via first handle.
        {
            let mgr = SharedMemoryManager::new(&path).expect("create must succeed");
            mgr.update(42_000, 1, 1.0, true);
        }
        // Re-open and read back.
        {
            let mgr2 = SharedMemoryManager::new(&path).expect("reopen must succeed");
            let snap = mgr2.read_snapshot().expect("read must succeed");
            assert_eq!(snap.timestamp_ns, 42_000);
            assert!(snap.synchronized);
        }

        let _ = std::fs::remove_file(&path);
    }
}

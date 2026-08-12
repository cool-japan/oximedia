//! Fixed-capacity byte-buffer pool for plane copies.
//!
//! Every capture backend has to copy samples out of a driver-owned buffer
//! before handing that buffer back to the device — the kernel needs its DMA
//! slot returned, and it needs it back within one frame period. Allocating a
//! fresh `Vec<u8>` for each of those copies is measurable at production rates
//! (60 fps of 4K NV12 is ~700 MB/s of allocation churn), so the buffers are
//! pre-allocated once and recycled through an RAII guard.
//!
//! ## Design
//!
//! Modelled on `oximedia-ndi`'s `frame_buffer_pool`, with the same names and
//! the same shape:
//!
//! * A free-list behind an `Arc<parking_lot::Mutex<…>>` — the critical section
//!   is a `Vec` push or pop, which is exactly the workload `parking_lot` is
//!   tuned for.
//! * [`PooledBuffer`] derefs to `[u8]` and returns its buffer to the free-list
//!   on drop.
//! * [`BufferPool::acquire`] returns `None` instead of blocking when the pool
//!   is exhausted, leaving back-pressure policy to the caller.
//!
//! It differs from the NDI original in one respect: [`PooledBuffer`] owns its
//! `Vec<u8>` outright instead of an `Option<…>`, and `Drop` reclaims it with
//! [`std::mem::take`]. The NDI version needs an `unreachable!()` in its `Deref`
//! to account for the `None` case; owning the `Vec` directly removes that
//! branch, so nothing on this path can panic.
//!
//! ## Who uses it
//!
//! No in-crate caller yet, hence the module-level `dead_code` allowance — again
//! mirroring the NDI original. The AVFoundation backend of package A5
//! deliberately does *not* route frames through this pool, and the reason is
//! worth recording so the next backend does not re-litigate it: a
//! [`PooledBuffer`] only helps when it comes back, and
//! [`PooledBuffer::into_inner`] takes a buffer out of the free-list for good.
//! An `oximedia_codec::Plane` owns its `Vec<u8>` and is handed to the consumer,
//! so a pooled buffer could never be returned; staging through the pool and
//! then copying into the plane would double the per-frame copy. A backend whose
//! platform API needs a scratch buffer it can hand straight back — a V4L2
//! `read()`-mode fallback, for instance — is the case this pool is shaped for.

#![allow(dead_code)]

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

// ── Pool statistics ──────────────────────────────────────────────────────────

/// Snapshot of pool utilization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolStats {
    /// Total number of buffers managed by this pool.
    pub(crate) total: usize,
    /// Number of buffers currently in the free-list.
    pub(crate) available: usize,
    /// Number of buffers currently held by callers.
    pub(crate) in_use: usize,
    /// Historical peak of `in_use` since pool creation.
    pub(crate) peak_in_use: usize,
}

// ── Internal state ───────────────────────────────────────────────────────────

/// Shared state, held behind an `Arc` so a [`PooledBuffer`] can return its
/// buffer without borrowing the [`BufferPool`] it came from.
#[derive(Debug)]
struct PoolInner {
    free: Mutex<Vec<Vec<u8>>>,
    total: usize,
    buffer_size: usize,
    peak_in_use: AtomicUsize,
}

impl PoolInner {
    fn new(capacity: usize, buffer_size: usize) -> Self {
        let free: Vec<Vec<u8>> = (0..capacity).map(|_| vec![0u8; buffer_size]).collect();
        Self {
            free: Mutex::new(free),
            total: capacity,
            buffer_size,
            peak_in_use: AtomicUsize::new(0),
        }
    }

    fn acquire(&self) -> Option<Vec<u8>> {
        let mut free = self.free.lock();
        let buffer = free.pop()?;
        let in_use = self.total.saturating_sub(free.len());
        self.peak_in_use.fetch_max(in_use, Ordering::Relaxed);
        Some(buffer)
    }

    fn release(&self, mut buffer: Vec<u8>) {
        // Restore the buffer to its nominal size and zero it, so a later
        // acquirer can never observe another frame's samples.
        buffer.clear();
        buffer.resize(self.buffer_size, 0);
        let mut free = self.free.lock();
        // Guard against a foreign buffer inflating the pool beyond `total`.
        if free.len() < self.total {
            free.push(buffer);
        }
    }

    fn stats(&self) -> PoolStats {
        let available = self.free.lock().len();
        let in_use = self.total.saturating_sub(available);
        PoolStats {
            total: self.total,
            available,
            in_use,
            peak_in_use: self.peak_in_use.load(Ordering::Relaxed),
        }
    }
}

// ── Pool ─────────────────────────────────────────────────────────────────────

/// A fixed-capacity pool of pre-allocated, equally sized byte buffers.
#[derive(Clone)]
pub(crate) struct BufferPool {
    inner: Arc<PoolInner>,
}

impl BufferPool {
    /// Pre-allocate `capacity` buffers of `buffer_size` bytes each.
    pub(crate) fn new(capacity: usize, buffer_size: usize) -> Self {
        Self {
            inner: Arc::new(PoolInner::new(capacity, buffer_size)),
        }
    }

    /// Take a buffer from the pool, or `None` when every buffer is in use.
    ///
    /// Never blocks. A caller that runs out of buffers is by definition behind
    /// the device, and the right response is the configured
    /// [`crate::DropPolicy`] — not a stall inside the capture loop.
    pub(crate) fn acquire(&self) -> Option<PooledBuffer> {
        let buffer = self.inner.acquire()?;
        Some(PooledBuffer {
            buffer,
            pool: Arc::clone(&self.inner),
        })
    }

    /// Current utilization.
    pub(crate) fn stats(&self) -> PoolStats {
        self.inner.stats()
    }

    /// Number of buffers managed by this pool.
    pub(crate) fn capacity(&self) -> usize {
        self.inner.total
    }

    /// Size in bytes of each buffer.
    pub(crate) fn buffer_size(&self) -> usize {
        self.inner.buffer_size
    }
}

impl std::fmt::Debug for BufferPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("BufferPool")
            .field("total", &stats.total)
            .field("available", &stats.available)
            .field("in_use", &stats.in_use)
            .field("buffer_size", &self.inner.buffer_size)
            .finish()
    }
}

// ── RAII guard ───────────────────────────────────────────────────────────────

/// A buffer borrowed from a [`BufferPool`], returned automatically on drop.
pub(crate) struct PooledBuffer {
    buffer: Vec<u8>,
    pool: Arc<PoolInner>,
}

impl PooledBuffer {
    /// The buffer's contents.
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.buffer
    }

    /// The buffer's contents, mutably.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buffer
    }

    /// Take the buffer out of the pool permanently.
    ///
    /// Use this when the bytes must outlive the guard — handing them to a
    /// [`crate::FramePayload`], for instance. The pool shrinks by one live
    /// buffer and will re-allocate on the next miss, so this is the slow path
    /// by design.
    pub(crate) fn into_inner(mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }
}

impl Deref for PooledBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buffer
    }
}

impl DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        // `take` leaves an empty `Vec`, which does not allocate.
        let buffer = std::mem::take(&mut self.buffer);
        if buffer.capacity() > 0 {
            self.pool.release(buffer);
        }
    }
}

impl std::fmt::Debug for PooledBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledBuffer")
            .field("len", &self.buffer.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acquire_or_fail(pool: &BufferPool) -> PooledBuffer {
        pool.acquire()
            .unwrap_or_else(|| panic!("pool unexpectedly exhausted"))
    }

    #[test]
    fn buffers_are_pre_allocated_at_the_requested_size() {
        let pool = BufferPool::new(3, 1024);
        assert_eq!(pool.capacity(), 3);
        assert_eq!(pool.buffer_size(), 1024);
        assert_eq!(acquire_or_fail(&pool).len(), 1024);
    }

    #[test]
    fn acquire_and_drop_cycles_the_buffer_back() {
        let pool = BufferPool::new(2, 64);
        assert_eq!(pool.stats().available, 2);
        let buffer = acquire_or_fail(&pool);
        assert_eq!(pool.stats().available, 1);
        assert_eq!(pool.stats().in_use, 1);
        drop(buffer);
        assert_eq!(pool.stats().available, 2);
        assert_eq!(pool.stats().in_use, 0);
    }

    #[test]
    fn exhaustion_returns_none_rather_than_blocking() {
        let pool = BufferPool::new(2, 32);
        let _first = acquire_or_fail(&pool);
        let _second = acquire_or_fail(&pool);
        assert!(pool.acquire().is_none());
    }

    #[test]
    fn peak_in_use_is_tracked() {
        let pool = BufferPool::new(5, 16);
        {
            let _a = acquire_or_fail(&pool);
            let _b = acquire_or_fail(&pool);
            let _c = acquire_or_fail(&pool);
        }
        let stats = pool.stats();
        assert_eq!(stats.in_use, 0);
        assert_eq!(stats.peak_in_use, 3);
    }

    #[test]
    fn returned_buffers_are_zeroed() {
        let pool = BufferPool::new(1, 8);
        {
            let mut buffer = acquire_or_fail(&pool);
            buffer.as_mut_slice().fill(0xAB);
        }
        let reused = acquire_or_fail(&pool);
        assert!(reused.as_slice().iter().all(|&byte| byte == 0));
    }

    #[test]
    fn deref_gives_slice_access() {
        let pool = BufferPool::new(1, 4);
        let mut buffer = acquire_or_fail(&pool);
        buffer[0] = 7;
        assert_eq!(buffer[0], 7);
        assert_eq!(buffer.len(), 4);
    }

    #[test]
    fn into_inner_removes_the_buffer_from_the_pool() {
        let pool = BufferPool::new(1, 16);
        let bytes = acquire_or_fail(&pool).into_inner();
        assert_eq!(bytes.len(), 16);
        assert_eq!(pool.stats().available, 0, "buffer was taken permanently");
        assert!(pool.acquire().is_none());
    }

    #[test]
    fn clones_share_one_free_list() {
        let first = BufferPool::new(3, 8);
        let second = first.clone();
        let _held = acquire_or_fail(&first);
        assert_eq!(second.stats().in_use, 1);
        assert_eq!(second.stats().available, 2);
    }

    #[test]
    fn steady_state_recycles_without_growing() {
        let pool = BufferPool::new(4, 128);
        for _ in 0..200 {
            let mut held = Vec::new();
            for _ in 0..4 {
                if let Some(buffer) = pool.acquire() {
                    held.push(buffer);
                }
            }
            assert!(pool.acquire().is_none());
            drop(held);
            assert_eq!(pool.stats().available, 4);
        }
        assert_eq!(pool.stats().peak_in_use, 4);
    }

    #[test]
    fn zero_capacity_pool_never_hands_out_a_buffer() {
        let pool = BufferPool::new(0, 128);
        assert_eq!(pool.capacity(), 0);
        assert!(pool.acquire().is_none());
    }

    #[test]
    fn debug_output_reports_utilization() {
        let pool = BufferPool::new(2, 64);
        let text = format!("{pool:?}");
        assert!(text.contains("total"), "{text}");
        assert!(text.contains("buffer_size"), "{text}");
    }

    #[test]
    fn stats_compare_by_value() {
        let pool = BufferPool::new(2, 8);
        assert_eq!(pool.stats(), pool.stats());
    }
}

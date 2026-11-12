// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Grow-once scratch buffers.
//!
//! The data-plane rules for the web modules forbid per-frame allocation: a
//! filter's `apply()` must not allocate once it is warm. [`Scratch`] is the
//! shared primitive that makes that easy — a module holds one `Scratch`, asks
//! it for a `u8` or `f32` slice of the length it needs each frame, and the
//! backing `Vec` only ever reallocates when a larger frame than any seen before
//! arrives ("grow once, reuse forever").

/// A pair of reusable byte / float buffers with monotonically growing capacity.
///
/// [`Scratch::bytes`] and [`Scratch::floats`] return a sub-slice of exactly the
/// requested length. The backing storage grows only when the request exceeds
/// the current capacity, so after the first (largest) frame there are no more
/// allocations. The returned contents are **not** cleared on reuse; callers are
/// expected to overwrite them via an `_into` kernel.
#[derive(Debug, Default)]
pub struct Scratch {
    bytes: Vec<u8>,
    floats: Vec<f32>,
}

impl Scratch {
    /// Creates an empty scratch pool (no allocation until first use).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            floats: Vec::new(),
        }
    }

    /// Pre-allocates capacity for `bytes` bytes and `floats` floats.
    #[must_use]
    pub fn with_capacity(bytes: usize, floats: usize) -> Self {
        Self {
            bytes: vec![0u8; bytes],
            floats: vec![0.0f32; floats],
        }
    }

    /// Ensures the byte buffer can hold at least `len` bytes, growing (once) if
    /// needed. New capacity is zero-filled.
    pub fn ensure_bytes(&mut self, len: usize) {
        if self.bytes.len() < len {
            self.bytes.resize(len, 0);
        }
    }

    /// Ensures the float buffer can hold at least `len` floats, growing (once)
    /// if needed. New capacity is zero-filled.
    pub fn ensure_floats(&mut self, len: usize) {
        if self.floats.len() < len {
            self.floats.resize(len, 0.0);
        }
    }

    /// Returns a mutable byte slice of exactly `len` bytes, growing the backing
    /// buffer only if `len` exceeds the current capacity.
    pub fn bytes(&mut self, len: usize) -> &mut [u8] {
        self.ensure_bytes(len);
        &mut self.bytes[..len]
    }

    /// Returns a mutable float slice of exactly `len` floats, growing the
    /// backing buffer only if `len` exceeds the current capacity.
    pub fn floats(&mut self, len: usize) -> &mut [f32] {
        self.ensure_floats(len);
        &mut self.floats[..len]
    }

    /// Current byte-buffer capacity (its allocated length).
    #[must_use]
    pub fn byte_capacity(&self) -> usize {
        self.bytes.len()
    }

    /// Current float-buffer capacity (its allocated length).
    #[must_use]
    pub fn float_capacity(&self) -> usize {
        self.floats.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_exact_length() {
        let mut s = Scratch::new();
        assert_eq!(s.bytes(10).len(), 10);
        assert_eq!(s.floats(7).len(), 7);
    }

    #[test]
    fn grows_once_then_reuses_same_allocation() {
        let mut s = Scratch::new();
        let _ = s.bytes(100);
        let cap_after_grow = s.byte_capacity();
        assert!(cap_after_grow >= 100);

        // A smaller request must not shrink or reallocate.
        let ptr_before = s.bytes(50).as_ptr();
        assert_eq!(s.byte_capacity(), cap_after_grow);

        // Requesting the larger size again reuses the same backing buffer.
        let ptr_again = s.bytes(50).as_ptr();
        assert_eq!(ptr_before, ptr_again);
    }

    #[test]
    fn with_capacity_preallocates() {
        let s = Scratch::with_capacity(32, 16);
        assert_eq!(s.byte_capacity(), 32);
        assert_eq!(s.float_capacity(), 16);
    }

    #[test]
    fn default_is_empty() {
        let s = Scratch::default();
        assert_eq!(s.byte_capacity(), 0);
        assert_eq!(s.float_capacity(), 0);
    }
}

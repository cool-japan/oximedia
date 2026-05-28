//! H.264 Decoded Picture Buffer.
//!
//! The DPB holds frames that future frames might reference (short-term
//! or long-term) and frames that have been decoded but not yet emitted
//! (because B-frames cause decode order to diverge from display
//! order).  Two state flags per entry determine when an entry can be
//! evicted:
//!
//! - **Reference flag** — frames still listed as a short-term or
//!   long-term reference must stay.
//! - **Output-pending flag** — frames whose presentation timestamp
//!   hasn't come up yet must stay.
//!
//! Only frames where both flags are clear can be evicted to make
//! room for new decodes.
//!
//! This module implements the data structure and the small set of
//! operations the slice-level reference-picture-marking logic uses:
//! insert, mark-unused (short-term and long-term), assign long-term,
//! mark-all-unused, and emit-in-order.

use crate::h264::frame::Frame;

/// One entry in the DPB.
#[derive(Debug)]
pub struct DpbEntry {
    /// The reconstructed frame.
    pub frame: Frame,
    /// Picture Order Count — the display-order index.
    pub poc: i32,
    /// `frame_num` from the bitstream, used as the short-term
    /// identifier.
    pub frame_num: i32,
    /// True while this entry is held as a short-term reference.
    pub is_short_term_reference: bool,
    /// True while this entry is held as a long-term reference.
    pub is_long_term_reference: bool,
    /// Long-term frame index when `is_long_term_reference` is true.
    pub long_term_idx: Option<u32>,
    /// True while the entry has not yet been emitted to display.
    pub output_pending: bool,
}

impl DpbEntry {
    /// True when this entry can be evicted — no longer a reference
    /// of any kind and already emitted.
    #[must_use]
    pub fn is_evictable(&self) -> bool {
        !self.is_short_term_reference
            && !self.is_long_term_reference
            && !self.output_pending
    }

    /// True while this entry counts toward the DPB's reference cap.
    #[must_use]
    pub fn is_reference(&self) -> bool {
        self.is_short_term_reference || self.is_long_term_reference
    }
}

/// Fixed-capacity buffer of decoded frames.
#[derive(Debug, Default)]
pub struct Dpb {
    /// Stored entries.  Length never exceeds `max_capacity`.
    pub entries: Vec<DpbEntry>,
    /// Maximum number of frames the buffer can hold simultaneously
    /// (set by the active SPS's `max_dec_frame_buffering`).
    pub max_capacity: usize,
}

impl Dpb {
    /// Creates an empty DPB with the given capacity.
    #[must_use]
    pub fn new(max_capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(max_capacity),
            max_capacity,
        }
    }

    /// Inserts a new entry, evicting an evictable existing entry if
    /// the buffer is at capacity.
    ///
    /// Returns the evicted entry if one was removed, or `None` if
    /// there was already room.  Returns `Err` if every existing
    /// entry is still in use and no eviction is possible.
    pub fn insert(&mut self, entry: DpbEntry) -> Result<Option<DpbEntry>, DpbError> {
        let evicted = if self.entries.len() >= self.max_capacity {
            let idx = self
                .entries
                .iter()
                .position(DpbEntry::is_evictable)
                .ok_or(DpbError::Full)?;
            Some(self.entries.remove(idx))
        } else {
            None
        };
        self.entries.push(entry);
        Ok(evicted)
    }

    /// Marks the short-term reference with the given frame number as
    /// unused.  No-op if no matching entry exists.
    pub fn mark_short_term_unused(&mut self, frame_num: i32) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.is_short_term_reference && e.frame_num == frame_num)
        {
            entry.is_short_term_reference = false;
        }
    }

    /// Marks the long-term reference at the given long-term index as
    /// unused.
    pub fn mark_long_term_unused(&mut self, long_term_idx: u32) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.long_term_idx == Some(long_term_idx))
        {
            entry.is_long_term_reference = false;
            entry.long_term_idx = None;
        }
    }

    /// Promotes a short-term reference (identified by frame number)
    /// to a long-term reference with the given long-term index.
    pub fn assign_long_term(&mut self, frame_num: i32, long_term_idx: u32) {
        // First clear any existing entry at this long-term index.
        for entry in &mut self.entries {
            if entry.long_term_idx == Some(long_term_idx) {
                entry.is_long_term_reference = false;
                entry.long_term_idx = None;
            }
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.is_short_term_reference && e.frame_num == frame_num)
        {
            entry.is_short_term_reference = false;
            entry.is_long_term_reference = true;
            entry.long_term_idx = Some(long_term_idx);
        }
    }

    /// Marks every reference picture as unused.  Triggered by an IDR
    /// with `no_output_of_prior_pics_flag = 0` and the MMCO 5
    /// command.
    pub fn mark_all_unused(&mut self) {
        for entry in &mut self.entries {
            entry.is_short_term_reference = false;
            entry.is_long_term_reference = false;
            entry.long_term_idx = None;
        }
    }

    /// Finds the entry with the lowest POC among those still pending
    /// output, marks it as emitted, and returns its index.
    ///
    /// Returns `None` when no entry is pending output.
    pub fn pop_lowest_poc_pending(&mut self) -> Option<usize> {
        let mut best_idx: Option<usize> = None;
        let mut best_poc: i32 = i32::MAX;
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.output_pending && entry.poc < best_poc {
                best_poc = entry.poc;
                best_idx = Some(i);
            }
        }
        if let Some(idx) = best_idx {
            self.entries[idx].output_pending = false;
        }
        best_idx
    }

    /// Returns the number of entries still held as references (short
    /// or long).
    #[must_use]
    pub fn num_references(&self) -> usize {
        self.entries.iter().filter(|e| e.is_reference()).count()
    }
}

/// Error type for DPB operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpbError {
    /// Tried to insert a frame but every existing entry is still in
    /// use as a reference or output-pending.  This indicates either
    /// a broken bitstream or a level-budget violation.
    Full,
}

impl core::fmt::Display for DpbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Full => write!(f, "DPB full and no entry is evictable"),
        }
    }
}

impl std::error::Error for DpbError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(poc: i32, frame_num: i32) -> DpbEntry {
        DpbEntry {
            frame: Frame::new(16, 16),
            poc,
            frame_num,
            is_short_term_reference: true,
            is_long_term_reference: false,
            long_term_idx: None,
            output_pending: true,
        }
    }

    #[test]
    fn insert_into_empty_dpb_returns_no_eviction() {
        let mut dpb = Dpb::new(4);
        let evicted = dpb.insert(make_entry(0, 0)).unwrap();
        assert!(evicted.is_none());
        assert_eq!(dpb.entries.len(), 1);
    }

    #[test]
    fn insert_evicts_evictable_entry_when_full() {
        let mut dpb = Dpb::new(2);
        // First entry: non-reference, already emitted -> evictable.
        let mut entry0 = make_entry(0, 0);
        entry0.is_short_term_reference = false;
        entry0.output_pending = false;
        dpb.insert(entry0).unwrap();
        dpb.insert(make_entry(1, 1)).unwrap();
        // Now insert a third: the first must be evicted.
        let evicted = dpb.insert(make_entry(2, 2)).unwrap();
        assert!(evicted.is_some());
        assert_eq!(evicted.unwrap().poc, 0);
        assert_eq!(dpb.entries.len(), 2);
    }

    #[test]
    fn insert_returns_full_when_nothing_is_evictable() {
        let mut dpb = Dpb::new(2);
        dpb.insert(make_entry(0, 0)).unwrap();
        dpb.insert(make_entry(1, 1)).unwrap();
        let err = dpb.insert(make_entry(2, 2)).unwrap_err();
        assert_eq!(err, DpbError::Full);
    }

    #[test]
    fn mark_short_term_unused_clears_reference_flag() {
        let mut dpb = Dpb::new(4);
        dpb.insert(make_entry(0, 7)).unwrap();
        dpb.mark_short_term_unused(7);
        assert!(!dpb.entries[0].is_short_term_reference);
    }

    #[test]
    fn assign_long_term_promotes_short_term_entry() {
        let mut dpb = Dpb::new(4);
        dpb.insert(make_entry(0, 7)).unwrap();
        dpb.assign_long_term(7, 0);
        assert!(!dpb.entries[0].is_short_term_reference);
        assert!(dpb.entries[0].is_long_term_reference);
        assert_eq!(dpb.entries[0].long_term_idx, Some(0));
    }

    #[test]
    fn assign_long_term_replaces_existing_long_term_idx() {
        let mut dpb = Dpb::new(4);
        dpb.insert(make_entry(0, 7)).unwrap();
        dpb.insert(make_entry(1, 8)).unwrap();
        dpb.assign_long_term(7, 0);
        dpb.assign_long_term(8, 0);
        // Entry 7 should have been demoted, entry 8 promoted.
        assert!(!dpb.entries[0].is_long_term_reference);
        assert!(dpb.entries[1].is_long_term_reference);
    }

    #[test]
    fn mark_all_unused_clears_every_reference() {
        let mut dpb = Dpb::new(4);
        dpb.insert(make_entry(0, 0)).unwrap();
        dpb.insert(make_entry(1, 1)).unwrap();
        dpb.assign_long_term(1, 0);
        dpb.mark_all_unused();
        assert_eq!(dpb.num_references(), 0);
    }

    #[test]
    fn pop_lowest_poc_pending_emits_in_display_order() {
        let mut dpb = Dpb::new(4);
        // Insert in decode order with mismatched POC.
        dpb.insert(make_entry(2, 0)).unwrap();
        dpb.insert(make_entry(0, 1)).unwrap();
        dpb.insert(make_entry(1, 2)).unwrap();
        // First pop returns POC 0.
        let first = dpb.pop_lowest_poc_pending().unwrap();
        assert_eq!(dpb.entries[first].poc, 0);
        // Second pop: POC 1.
        let second = dpb.pop_lowest_poc_pending().unwrap();
        assert_eq!(dpb.entries[second].poc, 1);
        // Third pop: POC 2.
        let third = dpb.pop_lowest_poc_pending().unwrap();
        assert_eq!(dpb.entries[third].poc, 2);
        // Nothing left.
        assert!(dpb.pop_lowest_poc_pending().is_none());
    }
}

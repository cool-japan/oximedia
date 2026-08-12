//! Lazy (deferred) essence data loading for large AAF files.
//!
//! AAF files can embed hundreds of gigabytes of raw essence (media) data.
//! Loading it all at parse time is impractical.  This module provides two
//! types:
//!
//! * [`LazyEssence`] — a single essence stream descriptor whose bytes are
//!   loaded from the source file the first time they are needed, then cached
//!   behind `Arc<Mutex<Option<Arc<Vec<u8>>>>>`.  Callers receive an
//!   `Arc<Vec<u8>>` — a shared, reference-counted pointer — so multiple
//!   callers can hold the data simultaneously without any extra copies.
//! * [`EssenceCollection`] — an indexed bag of `LazyEssence` items with
//!   aggregate statistics and a bulk-preload helper.
//!
//! # Thread safety
//!
//! Both types are `Send + Sync`.  The `Mutex` serialises the first load; once
//! populated the `Arc<Vec<u8>>` can be cloned cheaply by any number of
//! concurrent readers.
//!
//! # Example
//!
//! ```no_run
//! use oximedia_aaf::lazy_essence::{LazyEssence, EssenceCollection};
//! use std::path::PathBuf;
//!
//! let essence = LazyEssence::new(
//!     PathBuf::from("media.aaf"),
//!     1024,          // byte offset inside the file
//!     4096,          // byte length
//!     "Picture".to_string(),
//! );
//!
//! // Data is loaded on first access; returns Arc<Vec<u8>> (zero-copy share)
//! let bytes = essence.data().expect("load essence");
//! assert_eq!(bytes.len(), 4096);
//! assert!(essence.is_loaded());
//! ```

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Files at least this large use `mmap` for loading; smaller files use
/// a standard `read_exact` to avoid the overhead of mapping a tiny region.
const MMAP_THRESHOLD: u64 = 65_536;

// ─── LazyEssence ─────────────────────────────────────────────────────────────

/// A single essence stream that is loaded from disk only on first access.
///
/// Clone is `O(1)` — all clones share the same underlying cache.  Callers
/// receive an `Arc<Vec<u8>>` from [`data()`](Self::data), so the bytes are
/// shared rather than copied on every access.
#[derive(Clone)]
pub struct LazyEssence {
    /// Path to the file containing the essence (may be the same .aaf or an
    /// external media file).
    pub source_path: PathBuf,
    /// Byte offset within `source_path` where the essence begins.
    pub offset: u64,
    /// Byte length of the essence block.
    pub length: u64,
    /// Human-readable data-definition name (e.g. "Picture", "Sound").
    pub descriptor: String,
    /// Shared cache — `None` until first load.  The inner `Arc<Vec<u8>>`
    /// allows multiple callers to hold the data simultaneously without copying.
    data_cache: Arc<Mutex<Option<Arc<Vec<u8>>>>>,
}

impl std::fmt::Debug for LazyEssence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loaded = self.is_loaded();
        f.debug_struct("LazyEssence")
            .field("source_path", &self.source_path)
            .field("offset", &self.offset)
            .field("length", &self.length)
            .field("descriptor", &self.descriptor)
            .field("loaded", &loaded)
            .finish()
    }
}

impl LazyEssence {
    /// Create a new (unloaded) essence descriptor.
    #[must_use]
    pub fn new(source_path: PathBuf, offset: u64, length: u64, descriptor: String) -> Self {
        Self {
            source_path,
            offset,
            length,
            descriptor,
            data_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Load and return all essence bytes as a shared `Arc<Vec<u8>>`.
    ///
    /// On the first call the bytes are read from `source_path` and cached;
    /// subsequent calls clone the `Arc` — no extra I/O or buffer copies.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the file cannot be opened or the read
    /// fails.  A poisoned mutex is reported as an error with kind `Other`.
    pub fn data(&self) -> std::io::Result<Arc<Vec<u8>>> {
        let mut guard = self.data_cache.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "lazy essence mutex poisoned")
        })?;
        if let Some(ref cached) = *guard {
            return Ok(Arc::clone(cached));
        }
        let loaded = Arc::new(self.load_from_disk()?);
        *guard = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Return `true` if the data has already been loaded into the cache.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.data_cache.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Return the declared byte length (not necessarily the number of bytes
    /// actually cached — use [`Self::data()`] for that).
    #[must_use]
    pub fn byte_length(&self) -> u64 {
        self.length
    }

    /// Return the data-definition descriptor string.
    #[must_use]
    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }

    /// Return bytes `data[start..end]` as a new `Vec<u8>`.
    ///
    /// Unlike [`data()`](Self::data) this always copies the slice, but slices
    /// are typically small compared to the full essence buffer.  `end` is
    /// clamped to the actual data length; an empty `Vec` is returned when
    /// `start >= end` after clamping.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from the initial load.
    pub fn read_range(&self, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
        let all = self.data()?;
        let len = all.len() as u64;
        let clamped_start = start.min(len) as usize;
        let clamped_end = end.min(len) as usize;
        if clamped_start >= clamped_end {
            return Ok(Vec::new());
        }
        Ok(all[clamped_start..clamped_end].to_vec())
    }

    /// Internal: read `self.length` bytes from `self.source_path` starting at
    /// `self.offset`.
    ///
    /// For large blocks (≥ `MMAP_THRESHOLD` bytes) the file is memory-mapped
    /// so that the OS can page in only the required region without allocating a
    /// full user-space copy up front.  For small blocks a regular `read_exact`
    /// is used to avoid the fixed overhead of setting up an `mmap`.
    fn load_from_disk(&self) -> std::io::Result<Vec<u8>> {
        if self.length >= MMAP_THRESHOLD {
            Self::load_via_mmap(&self.source_path, self.offset, self.length)
        } else {
            Self::load_via_read(&self.source_path, self.offset, self.length)
        }
    }

    /// Read `length` bytes at `offset` using a memory-mapped view of the file.
    ///
    /// # Safety
    ///
    /// `memmap2::Mmap::map` requires that the underlying file is not
    /// concurrently truncated while the mapping is live.  For read-only AAF
    /// source files (the only callers of this function) that invariant holds.
    #[allow(unsafe_code)]
    fn load_via_mmap(path: &std::path::Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>> {
        let file = std::fs::File::open(path)?;
        // SAFETY: The file is opened read-only and we hold a shared reference
        // through the map's lifetime; no other code truncates it.
        let mmap = unsafe {
            memmap2::Mmap::map(&file)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
        };
        let start = offset as usize;
        let end = offset.checked_add(length).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "offset + length overflows",
            )
        })? as usize;
        let slice = mmap.get(start..end).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "mmap slice {start}..{end} out of bounds (file size {})",
                    mmap.len()
                ),
            )
        })?;
        Ok(slice.to_vec())
    }

    /// Read `length` bytes at `offset` using `seek` + `read_exact`.
    fn load_via_read(path: &std::path::Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;

        // Guard: reject a declared (offset, length) pair that extends beyond
        // the actual file size *before* allocating the read buffer. `length`
        // comes straight from the caller (e.g. an AAF index entry) and has
        // no inherent relationship to the real file on disk; an
        // over-declared length would otherwise drive an arbitrarily large
        // `vec![0u8; length]` allocation from a tiny (or nonexistent) file.
        // Mirrors the bounds check `load_via_mmap` gets for free from
        // `mmap.get(start..end)`.
        let file_len = file.metadata()?.len();
        let end = offset.checked_add(length).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "offset + length overflows",
            )
        })?;
        if end > file_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("declared range {offset}..{end} out of bounds (file size {file_len})"),
            ));
        }

        file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; length as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Load essence from an already-open `Read + Seek` source.
    ///
    /// This is useful when the caller already holds an open handle to the AAF
    /// compound file and wants to avoid a second `open()`.  If the cache is
    /// already populated this is a no-op.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from seeking or reading.
    pub fn load_from_reader<R: Read + Seek>(&self, reader: &mut R) -> std::io::Result<()> {
        let mut guard = self.data_cache.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "lazy essence mutex poisoned")
        })?;
        if guard.is_some() {
            return Ok(());
        }

        // Guard: bound the declared `self.length` against the reader's
        // actual stream size *before* allocating the read buffer.
        // `self.length` is caller-supplied (e.g. parsed from an AAF index
        // entry) and has no inherent relationship to how much data `reader`
        // can actually provide; an over-declared length would otherwise
        // attempt a multi-gigabyte `vec![0u8; length]` allocation before
        // `read_exact` ever discovers the short read. Mirrors the bounds
        // check `load_via_mmap` gets for free from `mmap.get(start..end)`.
        let stream_len = reader.seek(SeekFrom::End(0))?;
        let end = self.offset.checked_add(self.length).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "offset + length overflows",
            )
        })?;
        if end > stream_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "declared range {}..{end} out of bounds (reader stream length {stream_len})",
                    self.offset
                ),
            ));
        }

        reader.seek(SeekFrom::Start(self.offset))?;
        let mut buf = vec![0u8; self.length as usize];
        reader.read_exact(&mut buf)?;
        *guard = Some(Arc::new(buf));
        Ok(())
    }

    /// Pre-populate the cache with externally supplied bytes.
    ///
    /// Provided for testing and for cases where the bytes were obtained
    /// through a different mechanism (e.g. network).  If the cache is already
    /// populated this replaces the existing data.
    pub fn inject(&self, data: Vec<u8>) {
        if let Ok(mut guard) = self.data_cache.lock() {
            *guard = Some(Arc::new(data));
        }
    }
}

// ─── EssenceCollection ───────────────────────────────────────────────────────

/// An ordered collection of [`LazyEssence`] items.
///
/// Items are appended with [`add`](Self::add) and retrieved by zero-based
/// index with [`get`](Self::get).
#[derive(Debug, Clone, Default)]
pub struct EssenceCollection {
    items: Vec<LazyEssence>,
}

impl EssenceCollection {
    /// Create an empty collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an essence item.
    pub fn add(&mut self, essence: LazyEssence) {
        self.items.push(essence);
    }

    /// Return a reference to the item at `index`, or `None`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&LazyEssence> {
        self.items.get(index)
    }

    /// Number of items in the collection.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Return `true` if the collection has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of items whose data has been loaded into memory.
    #[must_use]
    pub fn loaded_count(&self) -> usize {
        self.items.iter().filter(|e| e.is_loaded()).count()
    }

    /// Sum of the declared byte lengths of all items.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|e| e.length).sum()
    }

    /// Eagerly load all unloaded items.
    ///
    /// Returns the first error encountered, but items that were already loaded
    /// before the failure remain cached.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] from the first item that fails to load.
    pub fn preload_all(&self) -> std::io::Result<()> {
        for item in &self.items {
            // Discard the Arc — we only care that the data is now cached.
            let _ = item.data()?;
        }
        Ok(())
    }

    /// Iterate over all items.
    pub fn iter(&self) -> std::slice::Iter<'_, LazyEssence> {
        self.items.iter()
    }

    /// Find the first item matching the given descriptor string.
    #[must_use]
    pub fn find_by_descriptor(&self, descriptor: &str) -> Option<&LazyEssence> {
        self.items.iter().find(|e| e.descriptor == descriptor)
    }

    /// Return a slice of all items.
    #[must_use]
    pub fn as_slice(&self) -> &[LazyEssence] {
        &self.items
    }
}

impl<'a> IntoIterator for &'a EssenceCollection {
    type Item = &'a LazyEssence;
    type IntoIter = std::slice::Iter<'a, LazyEssence>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::thread;

    /// Write `data` to a temp file and return its path.
    ///
    /// The name has to be unique against *every other process on the machine*,
    /// not just against the other calls in this one: the test harness runs each
    /// test in its own process, so a per-process counter alone restarts at zero
    /// in every one of them, and a sub-second timestamp alone is not the
    /// tiebreaker it looks like — processes launched together reach this line
    /// within the same narrow window and duplicate nanosecond readings are
    /// common there. Two tests that landed on one name would truncate each
    /// other's file, and the loser would fail reading a payload that is the
    /// wrong length rather than reporting a collision.
    ///
    /// So the process id carries the uniqueness — the kernel guarantees it is
    /// distinct across all *live* processes — the counter separates repeat
    /// calls within one test, and the timestamp is kept only so that a file
    /// orphaned by a killed run cannot be mistaken for ours after its pid is
    /// recycled. `create_new` then refuses to open anything that already
    /// exists, so a name that somehow still collides fails loudly here instead
    /// of quietly corrupting a payload.
    fn make_temp_file(data: &[u8]) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut path = std::env::temp_dir();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("oximedia_aaf_lazy_test_{pid}_{seq}_{now}.bin");
        path.push(name);
        let mut f = std::fs::File::create_new(&path).expect("create temp file");
        f.write_all(data).expect("write temp data");
        path
    }

    // ── LazyEssence basic ────────────────────────────────────────────────────

    #[test]
    fn test_lazy_essence_not_loaded_on_creation() {
        let path = make_temp_file(b"hello");
        let e = LazyEssence::new(path.clone(), 0, 5, "Picture".to_string());
        assert!(!e.is_loaded());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_loads_on_data_call() {
        let payload = b"LAZY_LOAD_TEST";
        let path = make_temp_file(payload);
        let e = LazyEssence::new(path.clone(), 0, payload.len() as u64, "Sound".to_string());
        let data = e.data().expect("data must load");
        assert_eq!(data.as_slice(), payload.as_ref());
        assert!(e.is_loaded());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_data_cached_after_first_load() {
        let payload = b"CACHED";
        let path = make_temp_file(payload);
        let e = LazyEssence::new(path.clone(), 0, payload.len() as u64, "Picture".to_string());
        let _ = e.data().expect("first load");
        // Remove the file — second call must still succeed from cache
        let _ = std::fs::remove_file(&path);
        let data2 = e.data().expect("second load must use cache");
        assert_eq!(data2.as_slice(), payload.as_ref());
    }

    #[test]
    fn test_lazy_essence_byte_length() {
        let e = LazyEssence::new(PathBuf::from("/dev/null"), 0, 42, "Data".to_string());
        assert_eq!(e.byte_length(), 42);
    }

    #[test]
    fn test_lazy_essence_descriptor() {
        let e = LazyEssence::new(PathBuf::from("/dev/null"), 0, 0, "Sound".to_string());
        assert_eq!(e.descriptor(), "Sound");
    }

    #[test]
    fn test_lazy_essence_offset_load() {
        // File: 0..5 = "AAAAA", 5..10 = "BBBBB"
        let mut data = vec![0u8; 10];
        data[..5].copy_from_slice(b"AAAAA");
        data[5..].copy_from_slice(b"BBBBB");
        let path = make_temp_file(&data);
        let e = LazyEssence::new(path.clone(), 5, 5, "Sound".to_string());
        let loaded = e.data().expect("load from offset");
        assert_eq!(loaded.as_slice(), b"BBBBB");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_read_range_full() {
        let payload = b"ABCDEFGH";
        let path = make_temp_file(payload);
        let e = LazyEssence::new(path.clone(), 0, payload.len() as u64, "Data".to_string());
        let slice = e.read_range(2, 5).expect("read_range");
        assert_eq!(slice, b"CDE");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_read_range_clamps_to_length() {
        let payload = b"XYZ";
        let path = make_temp_file(payload);
        let e = LazyEssence::new(path.clone(), 0, 3, "Data".to_string());
        let slice = e.read_range(1, 9999).expect("read_range clamped");
        assert_eq!(slice, b"YZ");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_read_range_empty_when_start_ge_end() {
        let payload = b"HELLO";
        let path = make_temp_file(payload);
        let e = LazyEssence::new(path.clone(), 0, 5, "Data".to_string());
        let slice = e.read_range(3, 3).expect("empty range");
        assert!(slice.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_inject_skips_disk_io() {
        // non-existent path — must not be opened because inject pre-populates
        let e = LazyEssence::new(
            PathBuf::from("/no/such/file.aaf"),
            0,
            4,
            "Picture".to_string(),
        );
        e.inject(b"FAKE".to_vec());
        let data = e.data().expect("injected data");
        assert_eq!(data.as_slice(), b"FAKE");
    }

    #[test]
    fn test_lazy_essence_clone_shares_cache() {
        let payload = b"SHARED";
        let path = make_temp_file(payload);
        let e1 = LazyEssence::new(path.clone(), 0, payload.len() as u64, "Sound".to_string());
        let e2 = e1.clone();
        assert!(!e2.is_loaded());
        let _ = e1.data().expect("load via e1");
        // e2 must now be loaded as well (shared Arc<Mutex<Option<…>>>)
        assert!(e2.is_loaded());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_error_on_missing_file() {
        let e = LazyEssence::new(
            PathBuf::from("/no/such/file_oximedia.aaf"),
            0,
            8,
            "Picture".to_string(),
        );
        assert!(e.data().is_err());
    }

    #[test]
    fn test_lazy_essence_load_from_reader() {
        let payload = b"READER_LOAD";
        let path = make_temp_file(payload);
        let e = LazyEssence::new(path.clone(), 0, payload.len() as u64, "Data".to_string());
        let mut file = std::fs::File::open(&path).expect("open");
        e.load_from_reader(&mut file).expect("load from reader");
        assert!(e.is_loaded());
        assert_eq!(e.data().expect("cached").as_slice(), payload.as_ref());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lazy_essence_debug_repr() {
        let e = LazyEssence::new(
            std::env::temp_dir().join("oximedia-aaf-lazy-essence-x.aaf"),
            0,
            10,
            "Sound".to_string(),
        );
        let s = format!("{e:?}");
        assert!(s.contains("LazyEssence"));
        assert!(s.contains("loaded: false"));
    }

    // ── EssenceCollection ────────────────────────────────────────────────────

    #[test]
    fn test_collection_empty_on_new() {
        let c = EssenceCollection::new();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert_eq!(c.loaded_count(), 0);
        assert_eq!(c.total_bytes(), 0);
    }

    #[test]
    fn test_collection_add_and_get() {
        let mut c = EssenceCollection::new();
        let e = LazyEssence::new(
            std::env::temp_dir().join("oximedia-aaf-lazy-essence-a.aaf"),
            0,
            100,
            "Picture".to_string(),
        );
        c.add(e);
        assert_eq!(c.len(), 1);
        assert!(c.get(0).is_some());
        assert!(c.get(1).is_none());
    }

    #[test]
    fn test_collection_total_bytes() {
        let mut c = EssenceCollection::new();
        c.add(LazyEssence::new(
            PathBuf::from("/x"),
            0,
            50,
            "A".to_string(),
        ));
        c.add(LazyEssence::new(
            PathBuf::from("/y"),
            0,
            75,
            "B".to_string(),
        ));
        assert_eq!(c.total_bytes(), 125);
    }

    #[test]
    fn test_collection_loaded_count() {
        let mut c = EssenceCollection::new();
        let e1 = LazyEssence::new(PathBuf::from("/x"), 0, 5, "A".to_string());
        let e2 = LazyEssence::new(PathBuf::from("/y"), 0, 3, "B".to_string());
        e1.inject(b"hello".to_vec());
        c.add(e1);
        c.add(e2);
        assert_eq!(c.loaded_count(), 1);
    }

    #[test]
    fn test_collection_preload_all() {
        let payload = b"PRELOAD";
        let path = make_temp_file(payload);
        let mut c = EssenceCollection::new();
        c.add(LazyEssence::new(
            path.clone(),
            0,
            payload.len() as u64,
            "Sound".to_string(),
        ));
        assert_eq!(c.loaded_count(), 0);
        c.preload_all().expect("preload must succeed");
        assert_eq!(c.loaded_count(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_collection_find_by_descriptor() {
        let mut c = EssenceCollection::new();
        c.add(LazyEssence::new(
            PathBuf::from("/a"),
            0,
            1,
            "Picture".to_string(),
        ));
        c.add(LazyEssence::new(
            PathBuf::from("/b"),
            0,
            2,
            "Sound".to_string(),
        ));
        assert!(c.find_by_descriptor("Sound").is_some());
        assert!(c.find_by_descriptor("Missing").is_none());
    }

    #[test]
    fn test_collection_iter() {
        let mut c = EssenceCollection::new();
        c.add(LazyEssence::new(PathBuf::from("/a"), 0, 1, "A".to_string()));
        c.add(LazyEssence::new(PathBuf::from("/b"), 0, 2, "B".to_string()));
        let total: u64 = c.iter().map(|e| e.byte_length()).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn test_collection_into_iter() {
        let mut c = EssenceCollection::new();
        c.add(LazyEssence::new(
            PathBuf::from("/a"),
            0,
            10,
            "X".to_string(),
        ));
        let mut count = 0;
        for _ in &c {
            count += 1;
        }
        assert_eq!(count, 1);
    }

    // ── Concurrency ──────────────────────────────────────────────────────────

    #[test]
    fn test_concurrent_first_load_is_safe() {
        let payload: Vec<u8> = (0u8..=255).collect();
        let path = make_temp_file(&payload);
        let e = Arc::new(LazyEssence::new(
            path.clone(),
            0,
            payload.len() as u64,
            "Picture".to_string(),
        ));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let ec = Arc::clone(&e);
                thread::spawn(move || ec.data().map(|d| d.len()))
            })
            .collect();

        for h in handles {
            let len = h.join().expect("thread join").expect("load ok");
            assert_eq!(len, 256);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_as_slice() {
        let mut c = EssenceCollection::new();
        c.add(LazyEssence::new(PathBuf::from("/a"), 0, 1, "A".to_string()));
        assert_eq!(c.as_slice().len(), 1);
    }

    // ── mmap vs read consistency ─────────────────────────────────────────────

    /// Write a 128 KiB temp file (above MMAP_THRESHOLD), load it twice via
    /// `data()` and assert both calls return byte-for-byte identical content.
    /// The first call triggers the real mmap-backed load; the second hits the
    /// cache — both must agree.
    #[test]
    fn test_lazy_essence_mmap_vs_read_consistency() {
        // Build deterministic 128 KiB payload.
        let size: usize = 131_072; // 128 KiB  (> MMAP_THRESHOLD = 64 KiB)
        let payload: Vec<u8> = (0u8..=255).cycle().take(size).collect();

        let path = make_temp_file(&payload);

        let e = LazyEssence::new(path.clone(), 0, size as u64, "Picture".to_string());

        // First call: triggers mmap load.
        let first = e.data().expect("first load via mmap");
        assert_eq!(first.len(), size, "first load length mismatch");
        assert_eq!(&first[..], &payload[..], "first load content mismatch");

        // Remove the file from disk to prove the second call uses the cache.
        let _ = std::fs::remove_file(&path);

        // Second call: must return the cached Arc — no further I/O.
        let second = e.data().expect("second load must use cache");
        assert_eq!(
            &first[..],
            &second[..],
            "cached data does not match original"
        );

        // Verify it was indeed cached (not re-read).
        assert!(
            e.is_loaded(),
            "essence must still be loaded after file deletion"
        );
    }

    /// Load from a file smaller than MMAP_THRESHOLD to verify the regular
    /// `read_exact` path is also correct.
    #[test]
    fn test_lazy_essence_small_file_load_via_read() {
        // 1 KiB — well below MMAP_THRESHOLD.
        let size: usize = 1024;
        let payload: Vec<u8> = (0u8..=255).cycle().take(size).collect();
        let path = make_temp_file(&payload);

        let e = LazyEssence::new(path.clone(), 0, size as u64, "Sound".to_string());
        let data = e.data().expect("small file load");
        assert_eq!(data.len(), size);
        assert_eq!(&data[..], &payload[..]);

        let _ = std::fs::remove_file(&path);
    }

    // ── OOM guards (over-declared length) ────────────────────────────────────

    /// `load_via_read` must reject a declared length that extends far
    /// beyond the real file size *before* allocating the read buffer,
    /// rather than attempting a huge allocation for a tiny file.
    #[test]
    fn test_load_via_read_rejects_over_declared_length() {
        let path = make_temp_file(b"tiny");
        let result = LazyEssence::load_via_read(&path, 0, 1024 * 1024 * 1024);
        assert!(
            result.is_err(),
            "over-declared length must be rejected before allocating"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// `load_via_read` must also reject a valid-looking length whose
    /// `offset + length` sum runs past end-of-file, even when `length`
    /// alone looks reasonable.
    #[test]
    fn test_load_via_read_rejects_offset_plus_length_beyond_file() {
        // offset (5) + length (10) = 15, but the file is only 8 bytes.
        let path = make_temp_file(b"12345678");
        let result = LazyEssence::load_via_read(&path, 5, 10);
        assert!(
            result.is_err(),
            "offset + length beyond file size must be rejected"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// End-to-end through the public `data()` API: a tiny file with a
    /// wildly over-declared length (kept below `MMAP_THRESHOLD` so this
    /// specifically exercises the `load_via_read` path) must surface a
    /// clean `Err`, not attempt a huge allocation or panic.
    #[test]
    fn test_lazy_essence_data_rejects_over_declared_length() {
        let path = make_temp_file(b"tiny");
        let e = LazyEssence::new(path.clone(), 0, 65_535, "Picture".to_string());
        let result = e.data();
        assert!(
            result.is_err(),
            "declared length far beyond the real file size must error, not allocate"
        );
        assert!(!e.is_loaded());
        let _ = std::fs::remove_file(&path);
    }

    /// `load_from_reader` has no file-size introspection available (the
    /// source is a generic `Read + Seek`), so it must bound `self.length`
    /// against the reader's actual stream length instead. A declared
    /// length of ~1 GiB against an 8-byte in-memory buffer must error
    /// before the read buffer is allocated.
    #[test]
    fn test_load_from_reader_rejects_over_declared_length() {
        let e = LazyEssence::new(
            PathBuf::from("/unused"),
            0,
            1_000_000_000,
            "Sound".to_string(),
        );
        let mut cursor = std::io::Cursor::new(vec![0u8; 8]);
        let result = e.load_from_reader(&mut cursor);
        assert!(
            result.is_err(),
            "over-declared length must be rejected before allocating"
        );
        assert!(!e.is_loaded());
    }

    /// Sanity check: a correctly bounded `(offset, length)` pair still
    /// loads successfully through `load_from_reader` after adding the
    /// stream-length guard — the fix must not break valid input.
    #[test]
    fn test_load_from_reader_accepts_length_within_stream() {
        let e = LazyEssence::new(PathBuf::from("/unused"), 2, 4, "Sound".to_string());
        let mut cursor = std::io::Cursor::new(b"ABCDEFGH".to_vec());
        e.load_from_reader(&mut cursor)
            .expect("length within stream bounds must succeed");
        assert_eq!(e.data().expect("cached").as_slice(), b"CDEF");
    }
}

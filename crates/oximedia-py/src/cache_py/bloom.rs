//! `oximedia.cache` Bloom filter bindings — real delegation to
//! [`oximedia_cache::bloom_filter`].
//!
//! Exposes the standard bit-array filter, the counting variant (supports
//! removal via 4-bit saturating counters), and the auto-growing scalable
//! filter, all using pure-Rust FNV-1a double hashing.
//!
//! The underlying Rust constructors `assert!` on invalid parameters
//! (`expected_items == 0` or a false-positive rate outside `(0, 1)`) rather
//! than returning `Result`. This binding validates those preconditions
//! itself and raises a clean `ValueError` *before* ever calling the
//! panicking constructor, so a bad call from Python never surfaces as an
//! opaque `PanicException`.

use oximedia_cache::bloom_filter as core;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

fn validate_bloom_params(expected_items: usize, false_positive_rate: f64) -> PyResult<()> {
    if expected_items == 0 {
        return Err(PyValueError::new_err("expected_items must be > 0"));
    }
    if !(false_positive_rate > 0.0 && false_positive_rate < 1.0) {
        return Err(PyValueError::new_err(
            "false_positive_rate must be in (0, 1)",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BloomFilter
// ---------------------------------------------------------------------------

/// Space-efficient probabilistic membership filter. False negatives are
/// impossible; false positives occur with probability approximately at most
/// `false_positive_rate` while the number of inserted items does not exceed
/// `expected_items`.
#[pyclass(name = "BloomFilter")]
pub struct PyBloomFilter {
    inner: core::BloomFilter,
}

#[pymethods]
impl PyBloomFilter {
    #[new]
    fn new(expected_items: usize, false_positive_rate: f64) -> PyResult<Self> {
        validate_bloom_params(expected_items, false_positive_rate)?;
        Ok(Self {
            inner: core::BloomFilter::new(expected_items, false_positive_rate),
        })
    }

    fn insert(&mut self, item: &[u8]) {
        self.inner.insert(item);
    }

    /// `True` if `item` may be in the set; `False` means it definitely is
    /// not.
    fn contains(&self, item: &[u8]) -> bool {
        self.inner.contains(item)
    }

    fn estimate_false_positive_rate(&self) -> f64 {
        self.inner.estimate_false_positive_rate()
    }

    fn item_count(&self) -> u64 {
        self.inner.item_count()
    }

    fn num_bits(&self) -> usize {
        self.inner.num_bits()
    }

    fn num_hash_functions(&self) -> u8 {
        self.inner.num_hash_functions()
    }

    fn __repr__(&self) -> String {
        format!(
            "BloomFilter(item_count={}, num_bits={})",
            self.inner.item_count(),
            self.inner.num_bits()
        )
    }
}

// ---------------------------------------------------------------------------
// CountingBloomFilter
// ---------------------------------------------------------------------------

/// Bloom filter with 4-bit saturating counters that supports deletion.
#[pyclass(name = "CountingBloomFilter")]
pub struct PyCountingBloomFilter {
    inner: core::CountingBloomFilter,
}

#[pymethods]
impl PyCountingBloomFilter {
    #[new]
    fn new(expected_items: usize, false_positive_rate: f64) -> PyResult<Self> {
        validate_bloom_params(expected_items, false_positive_rate)?;
        Ok(Self {
            inner: core::CountingBloomFilter::new(expected_items, false_positive_rate),
        })
    }

    fn insert(&mut self, item: &[u8]) {
        self.inner.insert(item);
    }

    fn contains(&self, item: &[u8]) -> bool {
        self.inner.contains(item)
    }

    /// Attempt to remove `item`. Returns `True` if it was (probably)
    /// present and every associated counter could be safely decremented.
    fn remove(&mut self, item: &[u8]) -> bool {
        self.inner.remove(item)
    }

    fn item_count(&self) -> u64 {
        self.inner.item_count()
    }

    fn estimate_false_positive_rate(&self) -> f64 {
        self.inner.estimate_false_positive_rate()
    }

    fn __repr__(&self) -> String {
        format!(
            "CountingBloomFilter(item_count={})",
            self.inner.item_count()
        )
    }
}

// ---------------------------------------------------------------------------
// ScalableBloomFilter
// ---------------------------------------------------------------------------

/// Auto-growing Bloom filter: adds a new geometrically-larger layer whenever
/// the active layer's estimated false-positive rate exceeds its target.
#[pyclass(name = "ScalableBloomFilter")]
pub struct PyScalableBloomFilter {
    inner: core::ScalableBloomFilter,
}

#[pymethods]
impl PyScalableBloomFilter {
    /// `initial_capacity` — expected items for the first layer (must be
    /// `> 0`). `target_fpr` — target per-layer false-positive rate in
    /// `(0, 1)`. `growth_factor` — capacity multiplier per new layer
    /// (values `<= 1.0` fall back to `2.0`).
    #[new]
    fn new(initial_capacity: usize, target_fpr: f64, growth_factor: f64) -> PyResult<Self> {
        validate_bloom_params(initial_capacity, target_fpr)?;
        Ok(Self {
            inner: core::ScalableBloomFilter::new(initial_capacity, target_fpr, growth_factor),
        })
    }

    fn insert(&mut self, item: &[u8]) {
        self.inner.insert(item);
    }

    fn contains(&self, item: &[u8]) -> bool {
        self.inner.contains(item)
    }

    /// Aggregate false-positive rate across all layers.
    fn estimate_false_positive_rate(&self) -> f64 {
        self.inner.estimate_false_positive_rate()
    }

    fn total_item_count(&self) -> u64 {
        self.inner.total_item_count()
    }

    fn layer_count(&self) -> usize {
        self.inner.layer_count()
    }

    /// Set the tightening ratio (silently ignored unless in `(0, 1)`).
    fn set_tightening_ratio(&mut self, ratio: f64) {
        self.inner.set_tightening_ratio(ratio);
    }

    fn tightening_ratio(&self) -> f64 {
        self.inner.tightening_ratio()
    }

    fn growth_factor(&self) -> f64 {
        self.inner.growth_factor()
    }

    /// `(item_count, num_bits, estimated_fpr)` for every layer.
    fn layer_stats(&self) -> Vec<(u64, usize, f64)> {
        self.inner.layer_stats()
    }

    /// Approximate remaining insert capacity of the active layer before a
    /// new layer is allocated.
    fn estimated_capacity_remaining(&self) -> usize {
        self.inner.estimated_capacity_remaining()
    }

    /// Discard all layers and reset to a single fresh layer.
    fn clear(&mut self) {
        self.inner.clear();
    }

    fn total_bits(&self) -> usize {
        self.inner.total_bits()
    }

    fn __repr__(&self) -> String {
        format!(
            "ScalableBloomFilter(total_item_count={}, layer_count={})",
            self.inner.total_item_count(),
            self.inner.layer_count()
        )
    }
}

// ---------------------------------------------------------------------------
// Module-level helpers
// ---------------------------------------------------------------------------

/// FNV-1a hash a batch of byte-string keys (vectorised across keys on
/// AVX2/NEON when available; identical results to hashing each key
/// individually).
#[pyfunction]
fn hash_batch_fnv1a(keys: Vec<Vec<u8>>) -> Vec<u64> {
    let refs: Vec<&[u8]> = keys.iter().map(Vec::as_slice).collect();
    core::hash_batch_fnv1a(&refs)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBloomFilter>()?;
    m.add_class::<PyCountingBloomFilter>()?;
    m.add_class::<PyScalableBloomFilter>()?;
    m.add_function(wrap_pyfunction!(hash_batch_fnv1a, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_filter_insert_contains() {
        let mut bf = PyBloomFilter::new(100, 0.01).expect("valid params");
        bf.insert(b"key1");
        assert!(bf.contains(b"key1"));
        assert!(!bf.contains(b"ghost"));
        assert_eq!(bf.item_count(), 1);
    }

    #[test]
    fn bloom_filter_rejects_zero_items() {
        assert!(PyBloomFilter::new(0, 0.01).is_err());
    }

    #[test]
    fn bloom_filter_rejects_invalid_fpr() {
        assert!(PyBloomFilter::new(100, 0.0).is_err());
        assert!(PyBloomFilter::new(100, 1.0).is_err());
    }

    #[test]
    fn counting_bloom_filter_insert_remove() {
        let mut cbf = PyCountingBloomFilter::new(200, 0.01).expect("valid params");
        cbf.insert(b"remove_me");
        assert!(cbf.contains(b"remove_me"));
        assert!(cbf.remove(b"remove_me"));
        assert!(!cbf.contains(b"remove_me"));
    }

    #[test]
    fn scalable_bloom_filter_grows_and_contains() {
        let mut sbf = PyScalableBloomFilter::new(10, 0.1, 2.0).expect("valid params");
        for i in 0u32..500 {
            sbf.insert(&i.to_le_bytes());
        }
        assert_eq!(sbf.total_item_count(), 500);
        assert!(sbf.layer_count() > 1);
        assert!(sbf.contains(&0u32.to_le_bytes()));
    }

    #[test]
    fn hash_batch_matches_scalar_semantics() {
        let keys = vec![b"a".to_vec(), b"bb".to_vec(), b"ccc".to_vec()];
        let hashes = hash_batch_fnv1a(keys);
        assert_eq!(hashes.len(), 3);
        // Hashing the same key twice must be deterministic.
        let again = hash_batch_fnv1a(vec![b"a".to_vec()]);
        assert_eq!(hashes[0], again[0]);
    }
}

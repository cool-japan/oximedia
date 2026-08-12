//! Smoke tests for the SLICE 4E cache family bindings — tiered_cache,
//! bloom_filter, distributed_cache, cache_warming, eviction_policies, and
//! content_aware_cache/write_behind_cache.
//!
//! Same embedded-interpreter pattern as `tests/cache_smoke.rs`. These run
//! real Python source through the actually-registered `oximedia.cache`
//! module, verifying the PyO3 registration wiring end-to-end.

use oximedia_py::cache_py::register_submodule;
use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn prepare_env(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let parent = PyModule::new(py, "oximedia_test_parent")?;
    register_submodule(&parent)?;
    let cache = parent.getattr("cache")?;
    let globals = PyDict::new(py);
    globals.set_item("cache", cache)?;
    Ok(globals)
}

#[test]
fn tiered_cache_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register cache");
        py.run(
            c_str!(
                "l1 = cache.TierConfig.memory('L1', 1024)\n\
                 l2 = cache.TierConfig.memory('L2', 4096)\n\
                 l2.set_eviction_policy('lfu')\n\
                 assert l2.eviction_policy == 'lfu'\n\
                 tc = cache.TieredCache([l1, l2])\n\
                 tc.put('key1', b'hello')\n\
                 assert tc.get('key1') == b'hello'\n\
                 stats = tc.stats()\n\
                 assert stats.total_hits == 1\n\
                 assert tc.tier_count() == 2\n\
                 try:\n\
                 \x20\x20\x20\x20l1.set_eviction_policy('bogus')\n\
                 \x20\x20\x20\x20raise AssertionError('bad policy should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("tiered cache family runs");
    });
}

#[test]
fn bloom_filter_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register cache");
        py.run(
            c_str!(
                "bf = cache.BloomFilter(100, 0.01)\n\
                 bf.insert(b'key1')\n\
                 assert bf.contains(b'key1')\n\
                 assert not bf.contains(b'ghost')\n\
                 cbf = cache.CountingBloomFilter(100, 0.01)\n\
                 cbf.insert(b'x')\n\
                 assert cbf.remove(b'x')\n\
                 assert not cbf.contains(b'x')\n\
                 sbf = cache.ScalableBloomFilter(10, 0.1, 2.0)\n\
                 for i in range(500):\n\
                 \x20\x20\x20\x20sbf.insert(i.to_bytes(4, 'little'))\n\
                 assert sbf.layer_count() > 1\n\
                 assert sbf.contains((0).to_bytes(4, 'little'))\n\
                 try:\n\
                 \x20\x20\x20\x20cache.BloomFilter(0, 0.01)\n\
                 \x20\x20\x20\x20raise AssertionError('zero items should have raised')\n\
                 except ValueError:\n\
                 \x20\x20\x20\x20pass\n\
                 hashes = cache.hash_batch_fnv1a([b'a', b'bb', b'ccc'])\n\
                 assert len(hashes) == 3\n"
            ),
            Some(&globals),
            None,
        )
        .expect("bloom filter family runs");
    });
}

#[test]
fn distributed_cache_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register cache");
        py.run(
            c_str!(
                "ring = cache.ConsistentHash(50)\n\
                 ring.add_node(1)\n\
                 ring.add_node(2)\n\
                 assert ring.real_node_count() == 2\n\
                 client = cache.DistributedCacheClient(1, ring)\n\
                 routed = client.route_key(b'key')\n\
                 assert routed in (1, 2)\n\
                 rf = cache.ReplicationFactor(2, 2)\n\
                 coord = cache.CacheCoordinator(rf)\n\
                 coord.add_client(client)\n\
                 coord.add_client(cache.DistributedCacheClient(2, ring))\n\
                 assert coord.node_count() == 2\n\
                 assert coord.can_write_quorum(b'key', [1, 2])\n\
                 assert not coord.can_write_quorum(b'key', [1])\n"
            ),
            Some(&globals),
            None,
        )
        .expect("distributed cache family runs");
    });
}

#[test]
fn cache_warming_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register cache");
        py.run(
            c_str!(
                "warmer = cache.CacheWarmer()\n\
                 warmer.look_ahead_secs = 100000\n\
                 warmer.min_frequency = 0.1\n\
                 for i in range(10):\n\
                 \x20\x20\x20\x20warmer.record_access('hot', 64, i * 360)\n\
                 for t in (0, 3600):\n\
                 \x20\x20\x20\x20warmer.record_access('cold', 64, t)\n\
                 top = warmer.top_hot_keys(2)\n\
                 assert top[0][0] == 'hot'\n\
                 plan = warmer.plan_warmup(3600, 1_000_000)\n\
                 assert isinstance(plan.entries_to_warm, list)\n\
                 assert 0.0 <= plan.estimated_hit_improvement <= 1.0\n"
            ),
            Some(&globals),
            None,
        )
        .expect("cache warming family runs");
    });
}

#[test]
fn eviction_policies_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register cache");
        py.run(
            c_str!(
                "fc = cache.FrequencyCounter(100)\n\
                 fc.increment(1)\n\
                 fc.increment(1)\n\
                 assert fc.frequency(1) == 2\n\
                 tracker = cache.LfuEvictionTracker()\n\
                 tracker.insert(10)\n\
                 tracker.insert(20)\n\
                 tracker.promote(10)\n\
                 assert tracker.evict() == 20\n\
                 gate = cache.TinyLfuAdmission(100)\n\
                 for _ in range(20):\n\
                 \x20\x20\x20\x20gate.record_access(42)\n\
                 assert gate.should_admit(42, 1)\n\
                 assert not gate.should_admit(999, 10)\n\
                 arc = cache.ArcTracker(10)\n\
                 arc.on_admit_t1()\n\
                 arc.on_promote_t1_to_t2()\n\
                 assert arc.t1_size == 0\n\
                 assert arc.t2_size == 1\n"
            ),
            Some(&globals),
            None,
        )
        .expect("eviction policies family runs");
    });
}

#[test]
fn content_aware_and_write_behind_family() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_env(py).expect("register cache");
        py.run(
            c_str!(
                "ct = cache.MediaContentType.manifest()\n\
                 assert ct.priority() == 10\n\
                 assert ct.ttl_ms() == 30000\n\
                 video = cache.MediaContentType.video_segment(2_000_000, 'av1')\n\
                 cac = cache.ContentAwareCache(16)\n\
                 cac.insert_media('seg1', bytes(1024), video)\n\
                 entry = cac.get('seg1')\n\
                 assert entry.size_bytes == 1024\n\
                 assert entry.access_count == 1\n\
                 assert len(cac) == 1\n\
                 weights = cache.ScoringWeights()\n\
                 weights.set_type_priority_multiplier(video, 0.5)\n\
                 assert abs(weights.priority_multiplier(video) - 0.5) < 1e-9\n\
                 wb = cache.WriteBehindCache(10)\n\
                 wb.put('k1', b'v1')\n\
                 assert wb.is_dirty('k1')\n\
                 assert wb.get('k1') == b'v1'\n\
                 flushed = wb.flush()\n\
                 assert flushed == 1\n\
                 assert not wb.is_dirty('k1')\n\
                 snap = wb.store_snapshot()\n\
                 assert ('k1', b'v1') in snap\n\
                 stats = wb.stats()\n\
                 assert stats.total_flushes == 1\n"
            ),
            Some(&globals),
            None,
        )
        .expect("content-aware / write-behind family runs");
    });
}

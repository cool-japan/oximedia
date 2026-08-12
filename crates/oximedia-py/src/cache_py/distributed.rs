//! `oximedia.cache` distributed-cache coordination bindings — real
//! delegation to [`oximedia_cache::distributed_cache`].
//!
//! **Honesty note**: this module contains no network I/O. It is pure
//! consistent-hash routing arithmetic (`ConsistentHash`) and quorum
//! bookkeeping (`ReplicationFactor`/`CacheCoordinator`) over node
//! identifiers and liveness lists the *caller* supplies — there is no
//! cluster to fake-contact, so nothing here pretends to talk to peers.
//! `CacheCoordinator.can_write_quorum(key, available_nodes)` is a decision
//! function: "given that these nodes are up, would a write quorum be met?"
//! — it never claims to have actually written anywhere.
//!
//! Node identifiers ([`oximedia_cache::distributed_cache::NodeId`]) are
//! plain `u64`s on the Python side; no wrapper class is needed.

use oximedia_cache::distributed_cache as core;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// ConsistentHash
// ---------------------------------------------------------------------------

/// Virtual-node consistent-hash ring for stable key routing across node
/// joins/leaves. Each real node occupies `virtual_nodes_per_node` positions
/// on the ring.
#[pyclass(name = "ConsistentHash")]
#[derive(Clone)]
pub struct PyConsistentHash {
    inner: core::ConsistentHash,
}

#[pymethods]
impl PyConsistentHash {
    #[new]
    fn new(virtual_nodes_per_node: u32) -> Self {
        Self {
            inner: core::ConsistentHash::new(virtual_nodes_per_node),
        }
    }

    fn add_node(&mut self, node_id: u64) {
        self.inner.add_node(core::NodeId(node_id));
    }

    fn remove_node(&mut self, node_id: u64) {
        self.inner.remove_node(core::NodeId(node_id));
    }

    /// The node that owns `key` under this ring, or `None` if the ring has
    /// no nodes.
    fn get_node(&self, key: &[u8]) -> Option<u64> {
        self.inner.get_node(key).map(|n| n.0)
    }

    /// Up to `n` distinct successor node IDs for `key` (for replica
    /// placement), starting at the primary owner and walking the ring.
    fn get_n_nodes(&self, key: &[u8], n: usize) -> Vec<u64> {
        self.inner
            .get_n_nodes(key, n)
            .into_iter()
            .map(|nid| nid.0)
            .collect()
    }

    fn virtual_node_count(&self) -> usize {
        self.inner.virtual_node_count()
    }

    fn real_node_count(&self) -> usize {
        self.inner.real_node_count()
    }

    fn __repr__(&self) -> String {
        format!(
            "ConsistentHash(real_nodes={}, virtual_nodes={})",
            self.inner.real_node_count(),
            self.inner.virtual_node_count()
        )
    }
}

// ---------------------------------------------------------------------------
// DistributedCacheClient
// ---------------------------------------------------------------------------

/// Per-node view of a [`PyConsistentHash`] ring: routes keys from the
/// perspective of `local_node`.
#[pyclass(name = "DistributedCacheClient")]
#[derive(Clone)]
pub struct PyDistributedCacheClient {
    pub(crate) inner: core::DistributedCacheClient,
}

#[pymethods]
impl PyDistributedCacheClient {
    #[new]
    fn new(local_node: u64, ring: &PyConsistentHash) -> Self {
        Self {
            inner: core::DistributedCacheClient::new(core::NodeId(local_node), ring.inner.clone()),
        }
    }

    #[getter]
    fn local_node(&self) -> u64 {
        self.inner.local_node.0
    }

    /// The node ID that should own `key` (falls back to `local_node` if the
    /// ring is empty).
    fn route_key(&self, key: &[u8]) -> u64 {
        self.inner.route_key(key).0
    }

    /// `True` if `key` routes to `local_node`.
    fn is_local_key(&self, key: &[u8]) -> bool {
        self.inner.is_local_key(key)
    }

    fn __repr__(&self) -> String {
        format!(
            "DistributedCacheClient(local_node={})",
            self.inner.local_node.0
        )
    }
}

// ---------------------------------------------------------------------------
// ReplicationFactor
// ---------------------------------------------------------------------------

/// Quorum sizing: a write quorum requires acknowledgement from at least
/// `writes` nodes; a read quorum requires responses from at least `reads`.
#[pyclass(name = "ReplicationFactor")]
#[derive(Clone, Copy)]
pub struct PyReplicationFactor {
    inner: core::ReplicationFactor,
}

#[pymethods]
impl PyReplicationFactor {
    #[new]
    fn new(reads: u8, writes: u8) -> Self {
        Self {
            inner: core::ReplicationFactor::new(reads, writes),
        }
    }

    /// Standard RF-3 cluster (R=2, W=2).
    #[staticmethod]
    fn rf3() -> Self {
        Self {
            inner: core::ReplicationFactor::rf3(),
        }
    }

    /// Strongly-consistent RF-3 cluster (R=3, W=3; R+W > N for N=3).
    #[staticmethod]
    fn rf3_strong() -> Self {
        Self {
            inner: core::ReplicationFactor::rf3_strong(),
        }
    }

    #[getter]
    fn reads(&self) -> u8 {
        self.inner.reads
    }

    #[getter]
    fn writes(&self) -> u8 {
        self.inner.writes
    }

    fn is_quorum_read_met(&self, responses: u8) -> bool {
        self.inner.is_quorum_read_met(responses)
    }

    fn is_quorum_write_met(&self, responses: u8) -> bool {
        self.inner.is_quorum_write_met(responses)
    }

    fn __repr__(&self) -> String {
        format!(
            "ReplicationFactor(reads={}, writes={})",
            self.inner.reads, self.inner.writes
        )
    }
}

// ---------------------------------------------------------------------------
// CacheCoordinator
// ---------------------------------------------------------------------------

/// Cluster-level routing/quorum coordinator. Tracks per-node clients and
/// the cluster's replication policy; every decision is computed locally
/// from caller-supplied node lists (see module docstring).
#[pyclass(name = "CacheCoordinator")]
pub struct PyCacheCoordinator {
    inner: core::CacheCoordinator,
}

#[pymethods]
impl PyCacheCoordinator {
    #[new]
    fn new(replication: PyReplicationFactor) -> Self {
        Self {
            inner: core::CacheCoordinator::new(replication.inner),
        }
    }

    fn add_client(&mut self, client: &PyDistributedCacheClient) {
        self.inner.add_client(client.inner.clone());
    }

    fn remove_client(&mut self, node_id: u64) {
        self.inner.remove_client(core::NodeId(node_id));
    }

    /// Primary owner of `key` according to the first registered client's
    /// ring, or `None` if no clients are registered.
    fn primary_node_for(&self, key: &[u8]) -> Option<u64> {
        self.inner.primary_node_for(key).map(|n| n.0)
    }

    /// Up to `n` replica node IDs for `key`.
    fn replica_nodes_for(&self, key: &[u8], n: usize) -> Vec<u64> {
        self.inner
            .replica_nodes_for(key, n)
            .into_iter()
            .map(|nid| nid.0)
            .collect()
    }

    /// Whether a write quorum for `key` could be formed given that
    /// `available_nodes` are currently up (no writes are actually
    /// performed).
    fn can_write_quorum(&self, key: &[u8], available_nodes: Vec<u64>) -> bool {
        let nodes: Vec<core::NodeId> = available_nodes.into_iter().map(core::NodeId).collect();
        self.inner.can_write_quorum(key, &nodes)
    }

    /// Whether a read quorum for `key` could be formed given that
    /// `available_nodes` are currently up.
    fn can_read_quorum(&self, key: &[u8], available_nodes: Vec<u64>) -> bool {
        let nodes: Vec<core::NodeId> = available_nodes.into_iter().map(core::NodeId).collect();
        self.inner.can_read_quorum(key, &nodes)
    }

    fn node_count(&self) -> usize {
        self.inner.node_count()
    }

    fn __repr__(&self) -> String {
        format!("CacheCoordinator(node_count={})", self.inner.node_count())
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyConsistentHash>()?;
    m.add_class::<PyDistributedCacheClient>()?;
    m.add_class::<PyReplicationFactor>()?;
    m.add_class::<PyCacheCoordinator>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_with_nodes(vn: u32, ids: &[u64]) -> PyConsistentHash {
        let mut ring = PyConsistentHash::new(vn);
        for &id in ids {
            ring.add_node(id);
        }
        ring
    }

    #[test]
    fn single_node_always_routes_there() {
        let ring = ring_with_nodes(20, &[1]);
        assert_eq!(ring.get_node(b"anything"), Some(1));
    }

    #[test]
    fn empty_ring_returns_none() {
        let ring = PyConsistentHash::new(10);
        assert!(ring.get_node(b"key").is_none());
    }

    #[test]
    fn get_n_nodes_returns_distinct_ids() {
        let ring = ring_with_nodes(100, &[1, 2, 3]);
        let nodes = ring.get_n_nodes(b"replicated_key", 3);
        assert_eq!(nodes.len(), 3);
        let unique: std::collections::HashSet<_> = nodes.iter().copied().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn client_routes_and_reports_locality() {
        let ring = ring_with_nodes(50, &[99]);
        let client = PyDistributedCacheClient::new(99, &ring);
        assert!(client.is_local_key(b"anything"));
        assert_eq!(client.route_key(b"anything"), 99);
    }

    #[test]
    fn replication_factor_quorum_checks() {
        let rf = PyReplicationFactor::new(2, 2);
        assert!(!rf.is_quorum_read_met(1));
        assert!(rf.is_quorum_read_met(2));
        let default_rf = PyReplicationFactor::rf3();
        assert_eq!(default_rf.reads(), 2);
        assert_eq!(default_rf.writes(), 2);
    }

    #[test]
    fn coordinator_write_quorum_from_available_nodes() {
        let ring = ring_with_nodes(100, &[1, 2, 3]);
        let mut coord = PyCacheCoordinator::new(PyReplicationFactor::new(2, 2));
        for id in 1..=3u64 {
            coord.add_client(&PyDistributedCacheClient::new(id, &ring));
        }
        assert_eq!(coord.node_count(), 3);
        assert!(coord.can_write_quorum(b"key", vec![1, 2, 3]));
        assert!(!coord.can_write_quorum(b"key", vec![1]));
    }

    #[test]
    fn coordinator_primary_node_for_empty_is_none() {
        let coord = PyCacheCoordinator::new(PyReplicationFactor::rf3());
        assert!(coord.primary_node_for(b"key").is_none());
    }
}

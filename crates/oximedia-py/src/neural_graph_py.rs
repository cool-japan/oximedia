//! `oximedia.neural` — declarative `Sequential` / `ModelGraph` model builders.
//!
//! Real delegation to [`oximedia_neural::graph`]. Unlike the rest of
//! `oximedia.neural`, the layers/nodes composed here are **Python
//! callables** rather than pre-built Rust layers: each callback receives
//! flat row-major tensor buffers (matching the module-wide `list[float]` +
//! shape convention, see [`crate::neural_py`]) and must return a tensor of
//! the same shape.
//!
//! * A `Sequential` layer callback has signature
//!   `(data: list[float], shape: list[int]) -> (list[float], list[int])`.
//! * A `ModelGraph` node callback has signature
//!   `(inputs: list[tuple[list[float], list[int]]]) -> (list[float], list[int])`,
//!   receiving one `(data, shape)` pair per upstream dependency, in order.
//!
//! `Sequential::forward` / `ModelGraph::forward` genuinely execute the
//! user-supplied Python callables in sequence via the real Rust
//! `oximedia_neural::graph` engine (each callback is wrapped as a real
//! `LayerFn` / `GraphNodeFn` Rust closure that re-enters the interpreter
//! with [`Python::attach`] — safe to nest since PyO3's attach-count
//! tracking treats an already-attached thread as a fast-path no-op).

use std::collections::HashMap;
use std::sync::Arc;

use oximedia_neural::graph::{GraphNodeFn, LayerFn, ModelGraph, Sequential};
use oximedia_neural::{NeuralError, Tensor};
use pyo3::prelude::*;

use crate::neural_py::neural_err;

// ---------------------------------------------------------------------------
// Python-callback adapters
// ---------------------------------------------------------------------------

/// Wraps a Python callable `(data, shape) -> (data, shape)` as a real
/// [`LayerFn`] Rust closure.
fn make_layer_fn(callback: Py<PyAny>) -> LayerFn {
    Arc::new(move |t: &Tensor| -> Result<Tensor, NeuralError> {
        Python::attach(|py| {
            let data = t.data().to_vec();
            let shape = t.shape().to_vec();
            let result = callback
                .call1(py, (data, shape))
                .map_err(|e| NeuralError::Io(format!("Python layer callback raised: {e}")))?;
            let (out_data, out_shape): (Vec<f32>, Vec<usize>) =
                result.extract(py).map_err(|e| {
                    NeuralError::Io(format!(
                        "Python layer callback must return (list[float], list[int]): {e}"
                    ))
                })?;
            Tensor::from_data(out_data, out_shape)
        })
    })
}

/// Wraps a Python callable `(inputs: list[(data, shape)]) -> (data, shape)`
/// as a real [`GraphNodeFn`] Rust closure.
fn make_graph_node_fn(callback: Py<PyAny>) -> GraphNodeFn {
    Arc::new(move |inputs: &[&Tensor]| -> Result<Tensor, NeuralError> {
        Python::attach(|py| {
            let py_inputs: Vec<(Vec<f32>, Vec<usize>)> = inputs
                .iter()
                .map(|t| (t.data().to_vec(), t.shape().to_vec()))
                .collect();
            let result = callback
                .call1(py, (py_inputs,))
                .map_err(|e| NeuralError::Io(format!("Python graph-node callback raised: {e}")))?;
            let (out_data, out_shape): (Vec<f32>, Vec<usize>) =
                result.extract(py).map_err(|e| {
                    NeuralError::Io(format!(
                        "Python graph-node callback must return (list[float], list[int]): {e}"
                    ))
                })?;
            Tensor::from_data(out_data, out_shape)
        })
    })
}

// ---------------------------------------------------------------------------
// Sequential
// ---------------------------------------------------------------------------

/// An ordered stack of named Python-callable layers.
///
/// Real delegation to [`oximedia_neural::graph::Sequential`]. Layers are
/// applied in insertion order: the output of layer `i` is the input to
/// layer `i+1`.
#[pyclass(name = "Sequential")]
pub struct PySequential {
    inner: Sequential,
}

#[pymethods]
impl PySequential {
    /// Creates an empty `Sequential` model.
    #[new]
    fn new() -> Self {
        Self {
            inner: Sequential::new(),
        }
    }

    /// Appends a named layer.
    ///
    /// ``layer_fn`` must be callable as
    /// ``(data: list[float], shape: list[int]) -> (list[float], list[int])``.
    fn add(&mut self, name: String, layer_fn: Py<PyAny>) {
        self.inner.add(name, make_layer_fn(layer_fn));
    }

    /// Returns ``True`` if no layers have been added.
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Layer names in insertion order.
    fn layer_names(&self) -> Vec<String> {
        self.inner
            .layer_names()
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// Runs the full forward pass through all layers.
    ///
    /// Raises ``ValueError`` if the input shape is invalid or any layer
    /// callback raises / returns a malformed result.
    fn forward(&self, data: Vec<f32>, shape: Vec<usize>) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let t = Tensor::from_data(data, shape).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    /// Runs a partial forward pass, stopping after the layer named
    /// ``stop_name``.
    ///
    /// Raises ``ValueError`` if ``stop_name`` is not a registered layer.
    fn forward_until(
        &self,
        data: Vec<f32>,
        shape: Vec<usize>,
        stop_name: &str,
    ) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let t = Tensor::from_data(data, shape).map_err(neural_err)?;
        let out = self
            .inner
            .forward_until(&t, stop_name)
            .map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __repr__(&self) -> String {
        format!("Sequential(layers={:?})", self.inner.layer_names())
    }
}

// ---------------------------------------------------------------------------
// ModelGraph
// ---------------------------------------------------------------------------

/// A directed acyclic computation graph of named, Python-callable nodes.
///
/// Real delegation to [`oximedia_neural::graph::ModelGraph`]. Input nodes
/// are declared with [`PyModelGraph::add_input`] and receive tensors
/// supplied at forward time; computation nodes are declared with
/// [`PyModelGraph::add_node`] and list the upstream node names they depend
/// on, enabling skip connections and multi-branch architectures.
///
/// Nodes must be added in topological order (all dependencies of a node
/// added before it) — this is the caller's responsibility, matching the
/// underlying Rust API.
#[pyclass(name = "ModelGraph")]
pub struct PyModelGraph {
    inner: ModelGraph,
}

#[pymethods]
impl PyModelGraph {
    /// Creates an empty `ModelGraph`.
    #[new]
    fn new() -> Self {
        Self {
            inner: ModelGraph::new(),
        }
    }

    /// Registers a named input node (receives a tensor at forward time).
    fn add_input(&mut self, name: String) {
        self.inner.add_input(name);
    }

    /// Adds a computation node depending on the listed upstream node names.
    ///
    /// ``func`` must be callable as
    /// ``(inputs: list[tuple[list[float], list[int]]]) -> (list[float], list[int])``,
    /// receiving one ``(data, shape)`` pair per entry in ``input_names``, in
    /// order.
    fn add_node(&mut self, name: String, input_names: Vec<String>, func: Py<PyAny>) {
        self.inner
            .add_node(name, input_names, make_graph_node_fn(func));
    }

    /// Runs a forward pass with a single named input tensor.
    ///
    /// Raises ``ValueError`` if ``output_name`` is unreachable or a node
    /// callback fails.
    fn forward_single(
        &self,
        input_name: &str,
        data: Vec<f32>,
        shape: Vec<usize>,
        output_name: &str,
    ) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let t = Tensor::from_data(data, shape).map_err(neural_err)?;
        let out = self
            .inner
            .forward_single(input_name, &t, output_name)
            .map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    /// Runs a forward pass with multiple named input tensors.
    ///
    /// ``inputs`` maps input-node-name to ``(data, shape)``. Raises
    /// ``ValueError`` if a declared input node is missing, ``output_name``
    /// is unreachable, or a node callback fails.
    fn forward(
        &self,
        inputs: HashMap<String, (Vec<f32>, Vec<usize>)>,
        output_name: &str,
    ) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let mut tensors = HashMap::with_capacity(inputs.len());
        for (name, (data, shape)) in inputs {
            let t = Tensor::from_data(data, shape).map_err(neural_err)?;
            tensors.insert(name, t);
        }
        let out = self
            .inner
            .forward(&tensors, output_name)
            .map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    /// All node names (including inputs) in insertion order.
    fn node_names(&self) -> Vec<String> {
        self.inner
            .node_names()
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// Returns ``True`` if the graph has no nodes.
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn __repr__(&self) -> String {
        format!("ModelGraph(nodes={:?})", self.inner.node_names())
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the graph-builder classes into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySequential>()?;
    m.add_class::<PyModelGraph>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Non-callback-requiring paths only; full callback execution is
    // exercised in `tests/neural_graph_smoke.rs` via an embedded
    // interpreter (constructing a real Python callable in-module without
    // `py.run`/`py.eval` scaffolding would just re-implement that harness).

    #[test]
    fn sequential_starts_empty() {
        let seq = PySequential::new();
        assert!(seq.is_empty());
        assert_eq!(seq.__len__(), 0);
        assert!(seq.layer_names().is_empty());
    }

    #[test]
    fn sequential_empty_forward_passes_through() {
        let seq = PySequential::new();
        let (data, shape) = seq.forward(vec![1.0, 2.0, 3.0], vec![3]).expect("forward");
        assert_eq!(data, vec![1.0, 2.0, 3.0]);
        assert_eq!(shape, vec![3]);
    }

    #[test]
    fn sequential_forward_invalid_shape_is_value_error() {
        let seq = PySequential::new();
        assert!(seq.forward(vec![1.0, 2.0, 3.0], vec![2, 2]).is_err());
    }

    #[test]
    fn sequential_forward_until_unknown_name_is_value_error() {
        let seq = PySequential::new();
        assert!(seq.forward_until(vec![1.0], vec![1], "missing").is_err());
    }

    #[test]
    fn sequential_repr_is_stable() {
        let seq = PySequential::new();
        assert_eq!(seq.__repr__(), "Sequential(layers=[])");
    }

    #[test]
    fn model_graph_starts_empty() {
        let graph = PyModelGraph::new();
        assert!(graph.is_empty());
        assert!(graph.node_names().is_empty());
    }

    #[test]
    fn model_graph_add_input_registers_node_name() {
        let mut graph = PyModelGraph::new();
        graph.add_input("x".to_string());
        assert!(!graph.is_empty());
        assert_eq!(graph.node_names(), vec!["x".to_string()]);
    }

    #[test]
    fn model_graph_forward_missing_input_is_value_error() {
        let graph = PyModelGraph::new();
        let inputs = HashMap::new();
        assert!(graph.forward(inputs, "out").is_err());
    }

    #[test]
    fn model_graph_forward_single_output_not_found_is_value_error() {
        let mut graph = PyModelGraph::new();
        graph.add_input("x".to_string());
        assert!(graph
            .forward_single("x", vec![1.0], vec![1], "nonexistent")
            .is_err());
    }
}

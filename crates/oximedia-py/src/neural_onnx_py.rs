//! `oximedia.neural` — ONNX model introspection and graph execution.
//!
//! Two tiers, both real (no fabricated results):
//!
//! * **Always available** (no extra Cargo feature): [`inspect_onnx`] for
//!   cheap header/graph metadata, and [`PyOnnxRuntime`] for full protobuf
//!   parsing + real per-node execution (`Relu`, `Sigmoid`, `Gemm`/`MatMul`,
//!   `Add`, `Reshape`, `Conv`, `BatchNormalization`, pooling, …) via
//!   [`oximedia_neural::onnx`] + [`oximedia_neural::onnx_runtime`] — a
//!   hand-written pure-Rust protobuf reader and node-executor graph, not a
//!   wrapper around a third-party ONNX runtime.
//! * **Feature-gated** (`onnx` Cargo feature on `oximedia-py`, which
//!   forwards to `oximedia-neural/onnx`): [`PyOnnxBackend`], a thin wrapper
//!   over the [`oxionnx`](https://crates.io/crates/oxionnx) pure-Rust
//!   engine for full-graph inference with broader operator coverage. When
//!   the feature is off, `OnnxBackend`'s methods raise a `RuntimeError`
//!   naming the flag — the class is always importable, but never returns a
//!   fabricated result.

use std::collections::HashMap;

use oximedia_neural::onnx::{inspect_onnx as inspect_onnx_bytes, OnnxModel, OnnxModelInfo};
use oximedia_neural::onnx_runtime::{ExecutionGraph, OnnxRuntime};
use oximedia_neural::Tensor;
#[cfg(not(feature = "onnx"))]
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::neural_py::neural_err;

// ---------------------------------------------------------------------------
// Shared tensor-dict marshaling
// ---------------------------------------------------------------------------

fn dict_to_tensors(
    inputs: HashMap<String, (Vec<f32>, Vec<usize>)>,
) -> PyResult<HashMap<String, Tensor>> {
    let mut tensors = HashMap::with_capacity(inputs.len());
    for (name, (data, shape)) in inputs {
        let t = Tensor::from_data(data, shape).map_err(neural_err)?;
        tensors.insert(name, t);
    }
    Ok(tensors)
}

fn tensors_to_dict(outputs: HashMap<String, Tensor>) -> HashMap<String, (Vec<f32>, Vec<usize>)> {
    outputs
        .into_iter()
        .map(|(name, t)| (name, (t.data().to_vec(), t.shape().to_vec())))
        .collect()
}

// ---------------------------------------------------------------------------
// OnnxModelInfo / inspect_onnx
// ---------------------------------------------------------------------------

/// Lightweight metadata extracted from an ONNX model file, without loading
/// any weight tensors (fast header/graph-shape introspection).
#[pyclass(name = "OnnxModelInfo")]
pub struct PyOnnxModelInfo {
    #[pyo3(get)]
    ir_version: i64,
    #[pyo3(get)]
    opset_version: i64,
    #[pyo3(get)]
    graph_name: String,
    #[pyo3(get)]
    node_types: Vec<String>,
    #[pyo3(get)]
    input_names: Vec<String>,
    #[pyo3(get)]
    output_names: Vec<String>,
}

#[pymethods]
impl PyOnnxModelInfo {
    fn __repr__(&self) -> String {
        format!(
            "OnnxModelInfo(ir_version={}, opset_version={}, graph_name={:?}, node_types={:?})",
            self.ir_version, self.opset_version, self.graph_name, self.node_types
        )
    }
}

fn model_info_to_py(info: OnnxModelInfo) -> PyOnnxModelInfo {
    PyOnnxModelInfo {
        ir_version: info.ir_version,
        opset_version: info.opset_version,
        graph_name: info.graph_name,
        node_types: info.node_types,
        input_names: info.input_names,
        output_names: info.output_names,
    }
}

/// Parses an ONNX protobuf byte buffer and returns lightweight model
/// metadata (IR/opset version, graph name, deduplicated operator types,
/// graph-level input/output names) without allocating any weight data.
///
/// Raises ``ValueError`` if the protobuf wire format is malformed. An
/// empty buffer is accepted and returns a zeroed `OnnxModelInfo`.
#[pyfunction]
fn inspect_onnx(data: Vec<u8>) -> PyResult<PyOnnxModelInfo> {
    let info = inspect_onnx_bytes(&data).map_err(neural_err)?;
    Ok(model_info_to_py(info))
}

// ---------------------------------------------------------------------------
// OnnxRuntime (always available — pure-Rust hand-written executor)
// ---------------------------------------------------------------------------

/// Parses an ONNX model and executes it via `oximedia_neural`'s own
/// hand-written protobuf reader + per-node executor graph (`Relu`,
/// `Sigmoid`, `Gemm`/`MatMul`, `Add`, `Reshape`, `Transpose`, `Conv`,
/// `BatchNormalization`, pooling, …). Always available — does not require
/// the `onnx` Cargo feature or the `oxionnx` engine.
#[pyclass(name = "OnnxRuntime")]
pub struct PyOnnxRuntime {
    inner: OnnxRuntime,
    input_names: Vec<String>,
    output_names: Vec<String>,
}

#[pymethods]
impl PyOnnxRuntime {
    /// Parses ``data`` as an ONNX model protobuf and builds an execution
    /// graph ready for [`run`].
    ///
    /// Raises ``ValueError`` if the protobuf is malformed or an operator's
    /// required weight initializer is missing.
    #[staticmethod]
    fn from_bytes(data: Vec<u8>) -> PyResult<Self> {
        let model = OnnxModel::from_bytes(&data).map_err(neural_err)?;
        let graph = ExecutionGraph::from_onnx_model(&model).map_err(neural_err)?;
        let input_names = graph.input_names.clone();
        let output_names = graph.output_names.clone();
        Ok(Self {
            inner: OnnxRuntime::new(graph),
            input_names,
            output_names,
        })
    }

    /// Parses the ONNX model at ``path`` (reads the file from disk) and
    /// builds an execution graph.
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        let model = OnnxModel::from_file(path).map_err(neural_err)?;
        let graph = ExecutionGraph::from_onnx_model(&model).map_err(neural_err)?;
        let input_names = graph.input_names.clone();
        let output_names = graph.output_names.clone();
        Ok(Self {
            inner: OnnxRuntime::new(graph),
            input_names,
            output_names,
        })
    }

    /// Names of the graph-level input tensors (derived from the ONNX
    /// graph: nodes' inputs that are not produced by any other node).
    fn input_names(&self) -> Vec<String> {
        self.input_names.clone()
    }

    /// Names of the graph-level output tensors.
    fn output_names(&self) -> Vec<String> {
        self.output_names.clone()
    }

    /// Runs inference. ``inputs`` maps tensor name to ``(data, shape)``;
    /// must supply a tensor for every name in [`input_names`].
    ///
    /// Returns a mapping of **every** tensor produced during execution
    /// (not just the declared graph outputs) — look up
    /// ``outputs[name]`` for any node's output by name, including
    /// [`output_names`].
    ///
    /// Raises ``ValueError`` if a required input is missing or a node
    /// fails (shape mismatch, unsupported operator, …).
    fn run(
        &self,
        inputs: HashMap<String, (Vec<f32>, Vec<usize>)>,
    ) -> PyResult<HashMap<String, (Vec<f32>, Vec<usize>)>> {
        let tensors = dict_to_tensors(inputs)?;
        let outputs = self.inner.run(tensors).map_err(neural_err)?;
        Ok(tensors_to_dict(outputs))
    }

    fn __repr__(&self) -> String {
        format!(
            "OnnxRuntime(input_names={:?}, output_names={:?})",
            self.input_names, self.output_names
        )
    }
}

// ---------------------------------------------------------------------------
// OnnxBackend (feature-gated — oxionnx full-graph engine)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "onnx"))]
fn onnx_feature_off_error() -> PyErr {
    PyRuntimeError::new_err(
        "oximedia.neural.OnnxBackend: the 'onnx' Cargo feature is not enabled for this build \
         of oximedia-py, so full-graph ONNX inference via the oxionnx engine is unavailable. \
         Rebuild with `--features onnx` to enable it. `inspect_onnx()` and `OnnxRuntime` \
         (this crate's own pure-Rust executor) remain available without this feature.",
    )
}

/// Full-graph ONNX inference backend wrapping an
/// [`oxionnx`](https://crates.io/crates/oxionnx) session (pure Rust).
///
/// Requires the `onnx` Cargo feature on `oximedia-py` (which forwards to
/// `oximedia-neural`'s own `onnx` feature). When built without it, every
/// method raises `RuntimeError` naming the flag — the class is always
/// importable, it just never returns a fabricated result.
#[pyclass(name = "OnnxBackend")]
pub struct PyOnnxBackend {
    #[cfg(feature = "onnx")]
    inner: oximedia_neural::OnnxBackend,
}

#[pymethods]
impl PyOnnxBackend {
    /// Loads an ONNX model from raw protobuf bytes.
    #[staticmethod]
    fn load_from_bytes(bytes: Vec<u8>) -> PyResult<Self> {
        #[cfg(feature = "onnx")]
        {
            let inner =
                oximedia_neural::OnnxBackend::load_from_bytes(&bytes).map_err(neural_err)?;
            Ok(Self { inner })
        }
        #[cfg(not(feature = "onnx"))]
        {
            let _ = bytes;
            Err(onnx_feature_off_error())
        }
    }

    /// Loads an ONNX model from a file path.
    #[staticmethod]
    fn load(path: &str) -> PyResult<Self> {
        #[cfg(feature = "onnx")]
        {
            let inner = oximedia_neural::OnnxBackend::load(std::path::Path::new(path))
                .map_err(neural_err)?;
            Ok(Self { inner })
        }
        #[cfg(not(feature = "onnx"))]
        {
            let _ = path;
            Err(onnx_feature_off_error())
        }
    }

    /// Runs full-graph inference. ``inputs`` maps tensor name to
    /// ``(data, shape)``; returns the same mapping shape for outputs.
    fn run(
        &self,
        inputs: HashMap<String, (Vec<f32>, Vec<usize>)>,
    ) -> PyResult<HashMap<String, (Vec<f32>, Vec<usize>)>> {
        #[cfg(feature = "onnx")]
        {
            let tensors = dict_to_tensors(inputs)?;
            let outputs = self.inner.run(&tensors).map_err(neural_err)?;
            Ok(tensors_to_dict(outputs))
        }
        #[cfg(not(feature = "onnx"))]
        {
            let _ = inputs;
            Err(onnx_feature_off_error())
        }
    }

    /// Names of the model's graph inputs.
    fn input_names(&self) -> PyResult<Vec<String>> {
        #[cfg(feature = "onnx")]
        {
            Ok(self.inner.input_names().to_vec())
        }
        #[cfg(not(feature = "onnx"))]
        {
            Err(onnx_feature_off_error())
        }
    }

    /// Names of the model's graph outputs.
    fn output_names(&self) -> PyResult<Vec<String>> {
        #[cfg(feature = "onnx")]
        {
            Ok(self.inner.output_names().to_vec())
        }
        #[cfg(not(feature = "onnx"))]
        {
            Err(onnx_feature_off_error())
        }
    }

    fn __repr__(&self) -> String {
        #[cfg(feature = "onnx")]
        {
            "OnnxBackend(feature='onnx' enabled)".to_string()
        }
        #[cfg(not(feature = "onnx"))]
        {
            "OnnxBackend(feature='onnx' not enabled for this build)".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the ONNX classes/functions into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyOnnxModelInfo>()?;
    m.add_class::<PyOnnxRuntime>()?;
    m.add_class::<PyOnnxBackend>()?;
    m.add_function(wrap_pyfunction!(inspect_onnx, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── inspect_onnx ─────────────────────────────────────────────────────

    #[test]
    fn inspect_onnx_empty_bytes_ok() {
        let info = inspect_onnx(vec![]).expect("inspect_onnx");
        assert_eq!(info.ir_version, 0);
        assert!(info.node_types.is_empty());
    }

    // ── OnnxRuntime ──────────────────────────────────────────────────────

    #[test]
    fn onnx_runtime_from_bytes_malformed_is_value_error() {
        // Not a valid protobuf-ish structure at all (random non-empty bytes
        // decode fine as *some* protobuf per the permissive wire format —
        // use a truncated varint tag instead to force a real parse error).
        let bad = vec![0x80u8]; // continuation bit set, no following byte
        assert!(PyOnnxRuntime::from_bytes(bad).is_err());
    }

    #[test]
    fn onnx_runtime_from_file_missing_path_is_value_error() {
        let missing = std::env::temp_dir().join("oximedia_py_nonexistent_model.onnx");
        let path = missing.to_string_lossy().to_string();
        assert!(PyOnnxRuntime::from_file(&path).is_err());
    }

    // ── OnnxBackend (feature off) ────────────────────────────────────────

    #[cfg(not(feature = "onnx"))]
    #[test]
    fn onnx_backend_honest_error_when_feature_off() {
        // `PyErr::to_string()` formats the underlying Python exception,
        // which needs an initialized interpreter (this crate builds with
        // PyO3's `extension-module` feature, so there is no auto-init) —
        // without this, the test is racy: it only passes if some other
        // test in this binary happened to initialize the interpreter
        // first. Match the sibling test below and initialize explicitly.
        pyo3::Python::initialize();
        // `PyOnnxBackend` isn't `Debug` (its feature-gated inner oxionnx
        // session may not be either), so match instead of `expect_err`.
        match PyOnnxBackend::load_from_bytes(vec![]) {
            Err(err) => {
                let msg = err.to_string();
                assert!(msg.contains("onnx"), "message should name the flag: {msg}");
            }
            Ok(_) => panic!("expected an error when the 'onnx' feature is off"),
        }
    }

    #[cfg(not(feature = "onnx"))]
    #[test]
    fn onnx_backend_honest_error_is_runtime_error_type() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let err = match PyOnnxBackend::load_from_bytes(vec![]) {
                Err(e) => e,
                Ok(_) => panic!("expected an error when the 'onnx' feature is off"),
            };
            assert!(
                err.is_instance_of::<PyRuntimeError>(py),
                "expected RuntimeError, got {err}"
            );
        });
    }

    #[cfg(not(feature = "onnx"))]
    #[test]
    fn onnx_backend_run_honest_error_when_feature_off() {
        // Construction itself fails, but even if it somehow existed,
        // run()/input_names()/output_names() must also fail honestly —
        // verified indirectly since load_from_bytes always errors first
        // in this build configuration.
        assert!(PyOnnxBackend::load_from_bytes(vec![1, 2, 3]).is_err());
    }

    // ── OnnxBackend (feature on) — only compiled/run with --features onnx ──

    #[cfg(feature = "onnx")]
    #[test]
    fn onnx_backend_malformed_bytes_is_value_error_when_feature_on() {
        let bad = vec![0x80u8];
        assert!(PyOnnxBackend::load_from_bytes(bad).is_err());
    }
}

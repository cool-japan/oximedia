//! Smoke tests for `oximedia.neural.inspect_onnx` / `OnnxRuntime` /
//! `OnnxBackend`.
//!
//! Exercises the `dict[str, (list[float], list[int])]` tensor-marshaling
//! convention (novel relative to the plain flat-list convention used
//! elsewhere in `oximedia.neural`) and the honest `RuntimeError` raised by
//! `OnnxBackend` when the crate is built without the `onnx` feature —
//! verified by actually catching it as `RuntimeError` in Python source,
//! not just checking the Rust-side exception type.

use oximedia_py::neural_py::register_submodule;
use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

fn prepare_neural_env(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let parent = PyModule::new(py, "oximedia_test_parent")?;
    register_submodule(&parent)?;
    let neural = parent.getattr("neural")?;
    let globals = PyDict::new(py);
    globals.set_item("neural", neural)?;
    Ok(globals)
}

// ---------------------------------------------------------------------------
// Minimal hand-rolled protobuf encoding — mirrors the wire-format helpers
// in `oximedia-neural`'s own `onnx.rs` test module (varint / length-delimited
// fields), reimplemented here since those are private to that crate's tests.
// ---------------------------------------------------------------------------

fn encode_varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
    out
}

fn encode_len_delim(field: u32, data: &[u8]) -> Vec<u8> {
    let tag = u64::from((field << 3) | 2);
    let mut out = encode_varint(tag);
    out.extend_from_slice(&encode_varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

/// Builds a minimal ONNX `ModelProto` containing a single `Relu` node
/// mapping input tensor `"x"` to output tensor `"y"`.
fn build_single_relu_node_model() -> Vec<u8> {
    // NodeProto: field 1 = input "x", field 2 = output "y", field 4 = op_type "Relu"
    let mut node = Vec::new();
    node.extend_from_slice(&encode_len_delim(1, b"x"));
    node.extend_from_slice(&encode_len_delim(2, b"y"));
    node.extend_from_slice(&encode_len_delim(4, b"Relu"));

    // GraphProto: field 1 = node (repeated)
    let graph = encode_len_delim(1, &node);

    // ModelProto: field 4 = graph
    encode_len_delim(4, &graph)
}

// ---------------------------------------------------------------------------
// inspect_onnx
// ---------------------------------------------------------------------------

#[test]
fn inspect_onnx_empty_bytes_returns_zeroed_info() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "info = neural.inspect_onnx(b'')\n\
                 assert info.ir_version == 0\n\
                 assert info.opset_version == 0\n\
                 assert info.node_types == []\n\
                 assert info.input_names == []\n"
            ),
            Some(&globals),
            None,
        )
        .expect("inspect_onnx runs on empty bytes");
    });
}

// ---------------------------------------------------------------------------
// OnnxRuntime — real end-to-end protobuf parse + execution
// ---------------------------------------------------------------------------

#[test]
fn onnx_runtime_parses_and_executes_a_real_relu_node() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        let model_bytes = build_single_relu_node_model();
        globals
            .set_item("model_bytes", PyBytes::new(py, &model_bytes))
            .expect("set model_bytes");
        py.run(
            c_str!(
                "runtime = neural.OnnxRuntime.from_bytes(model_bytes)\n\
                 assert runtime.input_names() == ['x'], runtime.input_names()\n\
                 assert runtime.output_names() == ['y'], runtime.output_names()\n\
                 outputs = runtime.run({'x': ([-1.0, 0.0, 2.0], [3])})\n\
                 data, shape = outputs['y']\n\
                 assert data == [0.0, 0.0, 2.0], data\n\
                 assert shape == [3]\n"
            ),
            Some(&globals),
            None,
        )
        .expect("onnx runtime parses and runs a real relu node");
    });
}

#[test]
fn onnx_runtime_malformed_bytes_raises_value_error() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "try:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}neural.OnnxRuntime.from_bytes(b'\\x80')\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise AssertionError('expected error')\n\
                 except ValueError:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("malformed protobuf raises ValueError");
    });
}

#[test]
fn onnx_runtime_missing_required_input_raises_value_error() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        let model_bytes = build_single_relu_node_model();
        globals
            .set_item("model_bytes", PyBytes::new(py, &model_bytes))
            .expect("set model_bytes");
        py.run(
            c_str!(
                "runtime = neural.OnnxRuntime.from_bytes(model_bytes)\n\
                 try:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}runtime.run({})\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise AssertionError('expected error')\n\
                 except ValueError:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("missing input raises ValueError");
    });
}

// ---------------------------------------------------------------------------
// OnnxBackend — honest RuntimeError when the `onnx` feature is off
// ---------------------------------------------------------------------------

#[cfg(not(feature = "onnx"))]
#[test]
fn onnx_backend_honest_runtime_error_catchable_in_python() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "try:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}neural.OnnxBackend.load_from_bytes(b'')\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise AssertionError('expected error')\n\
                 except RuntimeError as e:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}assert 'onnx' in str(e), str(e)\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}assert '--features onnx' in str(e), str(e)\n"
            ),
            Some(&globals),
            None,
        )
        .expect("OnnxBackend raises a real Python RuntimeError naming the flag");
    });
}

#[cfg(feature = "onnx")]
#[test]
fn onnx_backend_malformed_bytes_raises_value_error_when_feature_on() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "try:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}neural.OnnxBackend.load_from_bytes(b'\\x80')\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise AssertionError('expected error')\n\
                 except ValueError:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("malformed bytes raise ValueError even with the real oxionnx backend");
    });
}

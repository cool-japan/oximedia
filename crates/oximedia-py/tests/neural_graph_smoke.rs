//! Smoke tests for `oximedia.neural.Sequential` / `ModelGraph`.
//!
//! These are the one part of the `neural` submodule where the Rust side
//! genuinely calls back into arbitrary Python code (each layer / node is a
//! Python callable, invoked from inside a real
//! `oximedia_neural::graph::Sequential::forward` / `ModelGraph::forward`
//! call via a `Python::attach`-wrapped Rust closure). That round-trip can
//! only be exercised with a real embedded interpreter, so it is covered
//! here rather than as an in-module `#[cfg(test)]`.

use oximedia_py::neural_py::register_submodule;
use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn prepare_neural_env(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let parent = PyModule::new(py, "oximedia_test_parent")?;
    register_submodule(&parent)?;
    let neural = parent.getattr("neural")?;
    let globals = PyDict::new(py);
    globals.set_item("neural", neural)?;
    Ok(globals)
}

#[test]
fn sequential_runs_real_python_layers_in_order() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "def relu(data, shape):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return ([max(0.0, x) for x in data], shape)\n\
                 def scale2(data, shape):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return ([x * 2.0 for x in data], shape)\n\
                 seq = neural.Sequential()\n\
                 seq.add('relu', relu)\n\
                 seq.add('scale2', scale2)\n\
                 assert len(seq) == 2\n\
                 assert seq.layer_names() == ['relu', 'scale2']\n\
                 out_data, out_shape = seq.forward([-1.0, 2.0, -3.0], [3])\n\
                 assert out_data == [0.0, 4.0, 0.0], out_data\n\
                 assert out_shape == [3]\n"
            ),
            Some(&globals),
            None,
        )
        .expect("sequential forward runs real python layers");
    });
}

#[test]
fn sequential_forward_until_stops_early() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "def double(data, shape):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return ([x * 2.0 for x in data], shape)\n\
                 seq = neural.Sequential()\n\
                 seq.add('double', double)\n\
                 seq.add('double2', double)\n\
                 seq.add('double3', double)\n\
                 out_data, _ = seq.forward_until([1.0], [1], 'double2')\n\
                 assert out_data == [4.0], out_data\n"
            ),
            Some(&globals),
            None,
        )
        .expect("forward_until runs");
    });
}

#[test]
fn sequential_python_exception_propagates_as_value_error() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "def bad(data, shape):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise ValueError('intentional failure')\n\
                 seq = neural.Sequential()\n\
                 seq.add('bad', bad)\n\
                 try:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}seq.forward([1.0], [1])\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise AssertionError('expected error')\n\
                 except ValueError as e:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}assert 'intentional failure' in str(e), str(e)\n"
            ),
            Some(&globals),
            None,
        )
        .expect("python exception inside a layer propagates as ValueError");
    });
}

#[test]
fn sequential_malformed_layer_return_is_value_error() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "def malformed(data, shape):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return 'not a tuple'\n\
                 seq = neural.Sequential()\n\
                 seq.add('malformed', malformed)\n\
                 try:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}seq.forward([1.0], [1])\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}raise AssertionError('expected error')\n\
                 except ValueError:\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}pass\n"
            ),
            Some(&globals),
            None,
        )
        .expect("malformed callback return raises ValueError");
    });
}

#[test]
fn model_graph_skip_connection_with_real_python_nodes() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "def identity(inputs):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}data, shape = inputs[0]\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return (list(data), shape)\n\
                 def add(inputs):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}(da, sa), (db, _sb) = inputs\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return ([a + b for a, b in zip(da, db)], sa)\n\
                 graph = neural.ModelGraph()\n\
                 graph.add_input('x')\n\
                 graph.add_node('id', ['x'], identity)\n\
                 graph.add_node('out', ['x', 'id'], add)\n\
                 out_data, out_shape = graph.forward_single('x', [1.0, 2.0], [2], 'out')\n\
                 assert out_data == [2.0, 4.0], out_data\n\
                 assert out_shape == [2]\n\
                 assert 'out' in graph.node_names()\n"
            ),
            Some(&globals),
            None,
        )
        .expect("model graph skip connection runs real python nodes");
    });
}

#[test]
fn model_graph_multi_input_forward() {
    pyo3::Python::initialize();
    Python::attach(|py| {
        let globals = prepare_neural_env(py).expect("register neural");
        py.run(
            c_str!(
                "def sum_inputs(inputs):\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}(da, sa), (db, _sb) = inputs\n\
                 \u{0020}\u{0020}\u{0020}\u{0020}return ([a + b for a, b in zip(da, db)], sa)\n\
                 graph = neural.ModelGraph()\n\
                 graph.add_input('a')\n\
                 graph.add_input('b')\n\
                 graph.add_node('sum', ['a', 'b'], sum_inputs)\n\
                 out_data, _ = graph.forward({'a': ([1.0, 2.0], [2]), 'b': ([10.0, 20.0], [2])}, 'sum')\n\
                 assert out_data == [11.0, 22.0], out_data\n"
            ),
            Some(&globals),
            None,
        )
        .expect("model graph multi-input forward runs");
    });
}

//! `oximedia.neural` — GRU / LSTM recurrent sequence layers.
//!
//! Real delegation to [`oximedia_neural::recurrent`]. Weights are
//! Xavier-initialised at construction (matching the underlying Rust
//! default); no training loop is exposed, only inference forward passes.
//!
//! Inputs/outputs are plain flat `list[float]` buffers (row-major),
//! matching the underlying Rust API and the rest of `oximedia.neural`
//! (see [`crate::neural_py`]).

use oximedia_neural::recurrent::{GruLayer, LstmLayer};
use pyo3::prelude::*;

use crate::neural_py::neural_err;

// ---------------------------------------------------------------------------
// GruLayer
// ---------------------------------------------------------------------------

/// Single-layer Gated Recurrent Unit (GRU), inference only.
///
/// Real delegation to [`oximedia_neural::recurrent::GruLayer`]. Weights are
/// Xavier-uniform initialised at construction; biases start at zero.
#[pyclass(name = "GruLayer")]
pub struct PyGruLayer {
    inner: GruLayer,
}

#[pymethods]
impl PyGruLayer {
    /// Create a Xavier-initialised GRU layer with zero biases.
    ///
    /// Raises ``ValueError`` if either size is 0.
    #[new]
    fn new(input_size: usize, hidden_size: usize) -> PyResult<Self> {
        let inner = GruLayer::new(input_size, hidden_size).map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Number of input features.
    #[getter]
    fn input_size(&self) -> usize {
        self.inner.input_size
    }

    /// Number of hidden units.
    #[getter]
    fn hidden_size(&self) -> usize {
        self.inner.hidden_size
    }

    /// Runs the full sequence forward pass.
    ///
    /// ``inputs`` is a flat row-major ``seq_len * input_size`` buffer.
    /// ``initial_hidden``, if given, must have length ``hidden_size``
    /// (zero-filled otherwise).
    ///
    /// Returns ``(all_hidden, final_hidden)``: ``all_hidden`` has length
    /// ``seq_len * hidden_size`` (row-major ``[seq_len, hidden_size]``);
    /// ``final_hidden`` has length ``hidden_size``.
    ///
    /// Raises ``ValueError`` on any length mismatch.
    #[pyo3(signature = (inputs, seq_len, initial_hidden=None))]
    fn forward_sequence(
        &self,
        inputs: Vec<f32>,
        seq_len: usize,
        initial_hidden: Option<Vec<f32>>,
    ) -> PyResult<(Vec<f32>, Vec<f32>)> {
        self.inner
            .forward_sequence(&inputs, seq_len, initial_hidden.as_deref())
            .map_err(neural_err)
    }

    /// Runs a single time-step forward pass.
    ///
    /// ``input`` must have length ``input_size``; ``hidden`` (``h_{t-1}``)
    /// must have length ``hidden_size``.
    ///
    /// Returns the new hidden state ``h_t`` (length ``hidden_size``).
    fn forward_step(&self, input: Vec<f32>, hidden: Vec<f32>) -> PyResult<Vec<f32>> {
        self.inner.forward_step(&input, &hidden).map_err(neural_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "GruLayer(input_size={}, hidden_size={})",
            self.inner.input_size, self.inner.hidden_size
        )
    }
}

// ---------------------------------------------------------------------------
// LstmLayer
// ---------------------------------------------------------------------------

/// Single-layer Long Short-Term Memory (LSTM), inference only.
///
/// Real delegation to [`oximedia_neural::recurrent::LstmLayer`]. Weights are
/// Xavier-uniform initialised at construction; biases start at zero.
#[pyclass(name = "LstmLayer")]
pub struct PyLstmLayer {
    inner: LstmLayer,
}

#[pymethods]
impl PyLstmLayer {
    /// Create a Xavier-initialised LSTM layer with zero biases.
    ///
    /// Raises ``ValueError`` if either size is 0.
    #[new]
    fn new(input_size: usize, hidden_size: usize) -> PyResult<Self> {
        let inner = LstmLayer::new(input_size, hidden_size).map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Number of input features.
    #[getter]
    fn input_size(&self) -> usize {
        self.inner.input_size
    }

    /// Number of hidden units.
    #[getter]
    fn hidden_size(&self) -> usize {
        self.inner.hidden_size
    }

    /// Runs the full sequence forward pass.
    ///
    /// ``inputs`` is a flat row-major ``seq_len * input_size`` buffer.
    /// ``initial_hidden`` / ``initial_cell``, if given, must each have
    /// length ``hidden_size`` (zero-filled otherwise).
    ///
    /// Returns ``(all_hidden, final_hidden, final_cell)``: ``all_hidden``
    /// has length ``seq_len * hidden_size``; the other two have length
    /// ``hidden_size``.
    ///
    /// Raises ``ValueError`` on any length mismatch.
    #[pyo3(signature = (inputs, seq_len, initial_hidden=None, initial_cell=None))]
    fn forward_sequence(
        &self,
        inputs: Vec<f32>,
        seq_len: usize,
        initial_hidden: Option<Vec<f32>>,
        initial_cell: Option<Vec<f32>>,
    ) -> PyResult<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        self.inner
            .forward_sequence(
                &inputs,
                seq_len,
                initial_hidden.as_deref(),
                initial_cell.as_deref(),
            )
            .map_err(neural_err)
    }

    /// Runs a single time-step forward pass.
    ///
    /// ``input`` must have length ``input_size``; ``hidden`` and ``cell``
    /// must each have length ``hidden_size``.
    ///
    /// Returns ``(new_hidden, new_cell)``, each of length ``hidden_size``.
    fn forward_step(
        &self,
        input: Vec<f32>,
        hidden: Vec<f32>,
        cell: Vec<f32>,
    ) -> PyResult<(Vec<f32>, Vec<f32>)> {
        self.inner
            .forward_step(&input, &hidden, &cell)
            .map_err(neural_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "LstmLayer(input_size={}, hidden_size={})",
            self.inner.input_size, self.inner.hidden_size
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the recurrent-layer classes into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyGruLayer>()?;
    m.add_class::<PyLstmLayer>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    // ── GruLayer ─────────────────────────────────────────────────────────

    #[test]
    fn gru_construct_and_getters() {
        let gru = PyGruLayer::new(4, 8).expect("construct");
        assert_eq!(gru.input_size(), 4);
        assert_eq!(gru.hidden_size(), 8);
    }

    #[test]
    fn gru_zero_size_is_value_error() {
        assert!(PyGruLayer::new(0, 8).is_err());
        assert!(PyGruLayer::new(4, 0).is_err());
    }

    #[test]
    fn gru_forward_sequence_shapes() {
        let gru = PyGruLayer::new(3, 5).expect("construct");
        let inputs = vec![0.1_f32; 4 * 3];
        let (all_hidden, final_hidden) = gru
            .forward_sequence(inputs, 4, None)
            .expect("forward_sequence");
        assert_eq!(all_hidden.len(), 4 * 5);
        assert_eq!(final_hidden.len(), 5);
        // Final hidden must equal the last frame of all_hidden.
        assert_eq!(&all_hidden[3 * 5..], final_hidden.as_slice());
    }

    #[test]
    fn gru_forward_sequence_wrong_length_is_value_error() {
        let gru = PyGruLayer::new(3, 4).expect("construct");
        assert!(gru.forward_sequence(vec![0.0_f32; 14], 5, None).is_err());
    }

    #[test]
    fn gru_forward_step_matches_sequence_final() {
        let gru = PyGruLayer::new(2, 3).expect("construct");
        let inputs = vec![0.2_f32, -0.1, 0.05, 0.3, -0.2, 0.1];
        let mut h = vec![0.0_f32; 3];
        for step in 0..3 {
            let x = inputs[step * 2..(step + 1) * 2].to_vec();
            h = gru.forward_step(x, h).expect("forward_step");
        }
        let (_, final_hidden) = gru
            .forward_sequence(inputs, 3, None)
            .expect("forward_sequence");
        for (a, b) in h.iter().zip(final_hidden.iter()) {
            assert!(close(*a, *b));
        }
    }

    #[test]
    fn gru_repr_contains_sizes() {
        let gru = PyGruLayer::new(4, 8).expect("construct");
        let r = gru.__repr__();
        assert!(r.contains('4'));
        assert!(r.contains('8'));
    }

    // ── LstmLayer ────────────────────────────────────────────────────────

    #[test]
    fn lstm_construct_and_getters() {
        let lstm = PyLstmLayer::new(4, 8).expect("construct");
        assert_eq!(lstm.input_size(), 4);
        assert_eq!(lstm.hidden_size(), 8);
    }

    #[test]
    fn lstm_zero_size_is_value_error() {
        assert!(PyLstmLayer::new(0, 8).is_err());
        assert!(PyLstmLayer::new(4, 0).is_err());
    }

    #[test]
    fn lstm_forward_sequence_shapes() {
        let lstm = PyLstmLayer::new(2, 6).expect("construct");
        let inputs = vec![0.1_f32; 7 * 2];
        let (all_hidden, final_hidden, final_cell) = lstm
            .forward_sequence(inputs, 7, None, None)
            .expect("forward_sequence");
        assert_eq!(all_hidden.len(), 7 * 6);
        assert_eq!(final_hidden.len(), 6);
        assert_eq!(final_cell.len(), 6);
        assert_eq!(&all_hidden[6 * 6..], final_hidden.as_slice());
    }

    #[test]
    fn lstm_forward_sequence_wrong_length_is_value_error() {
        let lstm = PyLstmLayer::new(3, 4).expect("construct");
        assert!(lstm
            .forward_sequence(vec![0.0_f32; 14], 5, None, None)
            .is_err());
    }

    #[test]
    fn lstm_forward_step_matches_sequence_final() {
        let lstm = PyLstmLayer::new(1, 1).expect("construct");
        let inputs = vec![0.5_f32, -0.3, 0.1];
        let mut h = vec![0.0_f32];
        let mut c = vec![0.0_f32];
        for step in 0..3 {
            let x = inputs[step..step + 1].to_vec();
            let (h_new, c_new) = lstm.forward_step(x, h, c).expect("forward_step");
            h = h_new;
            c = c_new;
        }
        let (_, final_hidden, final_cell) = lstm
            .forward_sequence(inputs, 3, None, None)
            .expect("forward_sequence");
        assert!(close(h[0], final_hidden[0]));
        assert!(close(c[0], final_cell[0]));
    }

    #[test]
    fn lstm_repr_contains_sizes() {
        let lstm = PyLstmLayer::new(4, 8).expect("construct");
        let r = lstm.__repr__();
        assert!(r.contains('4'));
        assert!(r.contains('8'));
    }
}

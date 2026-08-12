//! `oximedia.neural` — multi-head attention and standalone attention /
//! positional-encoding functions.
//!
//! Real delegation to [`oximedia_neural::attention`]. `MultiHeadAttention`
//! weights are zero-initialised at construction (this crate ships
//! inference scaffolding, not pre-trained weights); use
//! [`PyMultiHeadAttention::load_weights`] to install pre-trained weights.
//!
//! Inputs/outputs are plain flat `list[float]` buffers (row-major) plus
//! explicit dimension integers, matching the rest of `oximedia.neural`
//! (see [`crate::neural_py`]).

use oximedia_neural::{NeuralError, Tensor};
use pyo3::prelude::*;

use crate::neural_py::neural_err;

// ---------------------------------------------------------------------------
// MultiHeadAttention
// ---------------------------------------------------------------------------

/// Multi-head self/cross-attention layer (inference mode).
///
/// Real delegation to [`oximedia_neural::MultiHeadAttention`]. `embed_dim`
/// must be divisible by `num_heads`. All projection weights/biases are
/// zero-initialised at construction.
#[pyclass(name = "MultiHeadAttention")]
pub struct PyMultiHeadAttention {
    inner: oximedia_neural::MultiHeadAttention,
}

#[pymethods]
impl PyMultiHeadAttention {
    /// Creates a zero-initialised `MultiHeadAttention`.
    ///
    /// Raises ``ValueError`` if ``embed_dim`` or ``num_heads`` is 0, or if
    /// ``embed_dim`` is not divisible by ``num_heads``.
    #[new]
    fn new(embed_dim: usize, num_heads: usize) -> PyResult<Self> {
        let inner =
            oximedia_neural::MultiHeadAttention::new(embed_dim, num_heads).map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Embedding (model) dimension.
    #[getter]
    fn embed_dim(&self) -> usize {
        self.inner.embed_dim
    }

    /// Number of attention heads.
    #[getter]
    fn num_heads(&self) -> usize {
        self.inner.num_heads
    }

    /// Per-head key/value dimension (``embed_dim / num_heads``).
    #[getter]
    fn head_dim(&self) -> usize {
        self.inner.head_dim
    }

    /// Installs pre-trained projection weights and biases.
    ///
    /// ``w_q``/``w_k``/``w_v``/``w_o`` must each have length
    /// ``embed_dim * embed_dim`` (row-major ``[embed_dim, embed_dim]``);
    /// ``b_q``/``b_k``/``b_v``/``b_o`` must each have length ``embed_dim``.
    ///
    /// Raises ``ValueError`` naming the first mismatched buffer.
    #[allow(clippy::too_many_arguments)]
    fn load_weights(
        &mut self,
        w_q: Vec<f32>,
        w_k: Vec<f32>,
        w_v: Vec<f32>,
        w_o: Vec<f32>,
        b_q: Vec<f32>,
        b_k: Vec<f32>,
        b_v: Vec<f32>,
        b_o: Vec<f32>,
    ) -> PyResult<()> {
        let d = self.inner.embed_dim;
        let d2 = d * d;
        for (name, len, expected) in [
            ("w_q", w_q.len(), d2),
            ("w_k", w_k.len(), d2),
            ("w_v", w_v.len(), d2),
            ("w_o", w_o.len(), d2),
            ("b_q", b_q.len(), d),
            ("b_k", b_k.len(), d),
            ("b_v", b_v.len(), d),
            ("b_o", b_o.len(), d),
        ] {
            if len != expected {
                return Err(neural_err(NeuralError::ShapeMismatch(format!(
                    "MultiHeadAttention::load_weights: {name} length {len} != expected {expected}"
                ))));
            }
        }
        self.inner.w_q = w_q;
        self.inner.w_k = w_k;
        self.inner.w_v = w_v;
        self.inner.w_o = w_o;
        self.inner.b_q = b_q;
        self.inner.b_k = b_k;
        self.inner.b_v = b_v;
        self.inner.b_o = b_o;
        Ok(())
    }

    /// Self-attention forward pass: query = key = value = ``x``.
    ///
    /// ``x`` is a flat row-major ``[seq_len, embed_dim]`` buffer.
    /// ``mask``, if given, is a flat row-major ``[seq_len, seq_len]``
    /// additive mask (e.g. ``-1e9`` for masked positions).
    ///
    /// Returns a flat row-major ``[seq_len, embed_dim]`` buffer.
    #[pyo3(signature = (x, seq_len, mask=None))]
    fn self_attention(
        &self,
        x: Vec<f32>,
        seq_len: usize,
        mask: Option<Vec<f32>>,
    ) -> PyResult<Vec<f32>> {
        let d = self.inner.embed_dim;
        let x_t = Tensor::from_data(x, vec![seq_len, d]).map_err(neural_err)?;
        let mask_t = mask
            .map(|m| Tensor::from_data(m, vec![seq_len, seq_len]))
            .transpose()
            .map_err(neural_err)?;
        let out = self
            .inner
            .self_attention(&x_t, mask_t.as_ref())
            .map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    /// Cross-attention forward pass: query differs from key/value context.
    ///
    /// ``query`` is a flat row-major ``[t_q, embed_dim]`` buffer; ``context``
    /// is a flat row-major ``[t_k, embed_dim]`` buffer. ``mask``, if given,
    /// is a flat row-major ``[t_q, t_k]`` additive mask.
    ///
    /// Returns a flat row-major ``[t_q, embed_dim]`` buffer.
    #[pyo3(signature = (query, t_q, context, t_k, mask=None))]
    #[allow(clippy::too_many_arguments)]
    fn cross_attention(
        &self,
        query: Vec<f32>,
        t_q: usize,
        context: Vec<f32>,
        t_k: usize,
        mask: Option<Vec<f32>>,
    ) -> PyResult<Vec<f32>> {
        let d = self.inner.embed_dim;
        let q_t = Tensor::from_data(query, vec![t_q, d]).map_err(neural_err)?;
        let c_t = Tensor::from_data(context, vec![t_k, d]).map_err(neural_err)?;
        let mask_t = mask
            .map(|m| Tensor::from_data(m, vec![t_q, t_k]))
            .transpose()
            .map_err(neural_err)?;
        let out = self
            .inner
            .cross_attention(&q_t, &c_t, mask_t.as_ref())
            .map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        format!(
            "MultiHeadAttention(embed_dim={}, num_heads={})",
            self.inner.embed_dim, self.inner.num_heads
        )
    }
}

// ---------------------------------------------------------------------------
// Standalone functions — scaled dot-product / flash attention
// ---------------------------------------------------------------------------

/// Scaled dot-product attention for a single head: `softmax(Q K^T / sqrt(d_k)) V`.
///
/// `q` is `[t_q, d_k]`, `k` is `[t_k, d_k]`, `v` is `[t_k, d_v]` (all flat
/// row-major). `mask`, if given, is a flat `[t_q, t_k]` additive mask.
///
/// Returns a flat row-major `[t_q, d_v]` buffer.
#[pyfunction]
#[pyo3(signature = (q, t_q, d_k, k, t_k, v, d_v, mask=None))]
#[allow(clippy::too_many_arguments)]
fn scaled_dot_product_attention(
    q: Vec<f32>,
    t_q: usize,
    d_k: usize,
    k: Vec<f32>,
    t_k: usize,
    v: Vec<f32>,
    d_v: usize,
    mask: Option<Vec<f32>>,
) -> PyResult<Vec<f32>> {
    let q_t = Tensor::from_data(q, vec![t_q, d_k]).map_err(neural_err)?;
    let k_t = Tensor::from_data(k, vec![t_k, d_k]).map_err(neural_err)?;
    let v_t = Tensor::from_data(v, vec![t_k, d_v]).map_err(neural_err)?;
    let mask_t = mask
        .map(|m| Tensor::from_data(m, vec![t_q, t_k]))
        .transpose()
        .map_err(neural_err)?;
    let out = oximedia_neural::scaled_dot_product_attention(&q_t, &k_t, &v_t, mask_t.as_ref())
        .map_err(neural_err)?;
    Ok(out.data().to_vec())
}

/// Flash-attention–style tiled scaled dot-product attention (numerically
/// equivalent to [`scaled_dot_product_attention`] but processes keys/values
/// in tiles of `block_size` columns; `block_size=0` means auto (32)).
///
/// Same shapes as [`scaled_dot_product_attention`].
#[pyfunction]
#[pyo3(signature = (q, t_q, d_k, k, t_k, v, d_v, block_size=0, mask=None))]
#[allow(clippy::too_many_arguments)]
fn flash_attention(
    q: Vec<f32>,
    t_q: usize,
    d_k: usize,
    k: Vec<f32>,
    t_k: usize,
    v: Vec<f32>,
    d_v: usize,
    block_size: usize,
    mask: Option<Vec<f32>>,
) -> PyResult<Vec<f32>> {
    let q_t = Tensor::from_data(q, vec![t_q, d_k]).map_err(neural_err)?;
    let k_t = Tensor::from_data(k, vec![t_k, d_k]).map_err(neural_err)?;
    let v_t = Tensor::from_data(v, vec![t_k, d_v]).map_err(neural_err)?;
    let mask_t = mask
        .map(|m| Tensor::from_data(m, vec![t_q, t_k]))
        .transpose()
        .map_err(neural_err)?;
    let out = oximedia_neural::flash_attention(&q_t, &k_t, &v_t, block_size, mask_t.as_ref())
        .map_err(neural_err)?;
    Ok(out.data().to_vec())
}

// ---------------------------------------------------------------------------
// Standalone functions — masks
// ---------------------------------------------------------------------------

/// Creates a causal (upper-triangular) attention mask of shape `[seq_len, seq_len]`.
///
/// Future positions (`k > q`) receive `-1e9`; past/current positions
/// receive `0.0`. Returns a flat row-major buffer.
#[pyfunction]
fn causal_mask(seq_len: usize) -> PyResult<Vec<f32>> {
    let m = oximedia_neural::causal_mask(seq_len).map_err(neural_err)?;
    Ok(m.data().to_vec())
}

/// Creates a rectangular causal mask `[t_q, t_k]` for cross-attention.
///
/// Position `(q, k)` is masked (`-1e9`) when `k > q`.
#[pyfunction]
fn causal_mask_rect(t_q: usize, t_k: usize) -> PyResult<Vec<f32>> {
    let m = oximedia_neural::causal_mask_rect(t_q, t_k).map_err(neural_err)?;
    Ok(m.data().to_vec())
}

/// Creates a sliding-window causal mask `[seq_len, seq_len]`: each query may
/// attend to at most `window_size` past keys (inclusive of itself).
#[pyfunction]
fn sliding_window_mask(seq_len: usize, window_size: usize) -> PyResult<Vec<f32>> {
    let m = oximedia_neural::sliding_window_mask(seq_len, window_size).map_err(neural_err)?;
    Ok(m.data().to_vec())
}

// ---------------------------------------------------------------------------
// Standalone functions — positional encodings
// ---------------------------------------------------------------------------

/// Computes the classic sinusoidal positional encoding (Vaswani et al., 2017).
///
/// Returns a flat row-major `[seq_len, d_model]` buffer. Raises
/// ``ValueError`` if `d_model` is odd or either dimension is 0.
#[pyfunction]
fn sinusoidal_positional_encoding(seq_len: usize, d_model: usize) -> PyResult<Vec<f32>> {
    oximedia_neural::sinusoidal_positional_encoding(seq_len, d_model).map_err(neural_err)
}

/// Adds sinusoidal positional encoding to a flat row-major
/// `[seq_len, d_model]` embedding buffer element-wise.
#[pyfunction]
fn add_positional_encoding(
    embeddings: Vec<f32>,
    seq_len: usize,
    d_model: usize,
) -> PyResult<Vec<f32>> {
    oximedia_neural::add_positional_encoding(&embeddings, seq_len, d_model).map_err(neural_err)
}

/// Applies Rotary Position Embedding (RoPE) to a flat row-major
/// `[seq_len, d]` buffer (`d` must be even). `base` is the frequency base
/// (typically `10000.0`).
///
/// Returns a new rotated buffer of the same shape.
#[pyfunction]
#[pyo3(signature = (x, seq_len, d, base=10000.0))]
fn apply_rope(x: Vec<f32>, seq_len: usize, d: usize, base: f32) -> PyResult<Vec<f32>> {
    let mut t = Tensor::from_data(x, vec![seq_len, d]).map_err(neural_err)?;
    oximedia_neural::apply_rope(&mut t, base).map_err(neural_err)?;
    Ok(t.data().to_vec())
}

/// Generates the RoPE `(cos, sin)` frequency table for a given `seq_len` and
/// `d` (even). Returns a flat row-major `[seq_len, d]` buffer.
#[pyfunction]
#[pyo3(signature = (seq_len, d, base=10000.0))]
fn rope_frequencies(seq_len: usize, d: usize, base: f32) -> PyResult<Vec<f32>> {
    let t = oximedia_neural::rope_frequencies(seq_len, d, base).map_err(neural_err)?;
    Ok(t.data().to_vec())
}

/// Computes Shaw-style (Shaw et al., 2018) relative position bias for
/// attention scores.
///
/// `rel_embeddings` is a flat row-major `[2*max_dist+1, d_k]` learnable
/// table. Returns a flat row-major `[seq_len, seq_len]` additive bias.
#[pyfunction]
fn relative_position_shaw(
    seq_len: usize,
    d_k: usize,
    max_dist: usize,
    rel_embeddings: Vec<f32>,
) -> PyResult<Vec<f32>> {
    let bias = oximedia_neural::relative_position_shaw(seq_len, d_k, max_dist, &rel_embeddings)
        .map_err(neural_err)?;
    Ok(bias.data().to_vec())
}

// ---------------------------------------------------------------------------
// Standalone functions — tiled matmul
// ---------------------------------------------------------------------------

/// Tiled (blocked) matrix multiplication `C = A @ B^T` for cache locality.
///
/// `a` is flat row-major `[m, k]`; `b` is flat row-major `[n, k]`
/// (**not** transposed — `B^T` is computed implicitly). `block_size=0`
/// means auto (32).
///
/// Returns a flat row-major `[m, n]` buffer. Raises ``ValueError`` if `a`
/// or `b` does not have exactly `m*k` / `n*k` elements (this is a Python
/// FFI-boundary safety check: the underlying Rust function has no length
/// validation of its own and indexes unconditionally).
#[pyfunction]
#[pyo3(signature = (a, b, m, n, k, block_size=0))]
fn tiled_matmul_t(
    a: Vec<f32>,
    b: Vec<f32>,
    m: usize,
    n: usize,
    k: usize,
    block_size: usize,
) -> PyResult<Vec<f32>> {
    if a.len() != m * k {
        return Err(neural_err(NeuralError::ShapeMismatch(format!(
            "tiled_matmul_t: len(a)={} != m*k={}",
            a.len(),
            m * k
        ))));
    }
    if b.len() != n * k {
        return Err(neural_err(NeuralError::ShapeMismatch(format!(
            "tiled_matmul_t: len(b)={} != n*k={}",
            b.len(),
            n * k
        ))));
    }
    Ok(oximedia_neural::tiled_matmul_t(&a, &b, m, n, k, block_size))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the attention classes/functions into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyMultiHeadAttention>()?;
    m.add_function(wrap_pyfunction!(scaled_dot_product_attention, m)?)?;
    m.add_function(wrap_pyfunction!(flash_attention, m)?)?;
    m.add_function(wrap_pyfunction!(causal_mask, m)?)?;
    m.add_function(wrap_pyfunction!(causal_mask_rect, m)?)?;
    m.add_function(wrap_pyfunction!(sliding_window_mask, m)?)?;
    m.add_function(wrap_pyfunction!(sinusoidal_positional_encoding, m)?)?;
    m.add_function(wrap_pyfunction!(add_positional_encoding, m)?)?;
    m.add_function(wrap_pyfunction!(apply_rope, m)?)?;
    m.add_function(wrap_pyfunction!(rope_frequencies, m)?)?;
    m.add_function(wrap_pyfunction!(relative_position_shaw, m)?)?;
    m.add_function(wrap_pyfunction!(tiled_matmul_t, m)?)?;
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

    // ── MultiHeadAttention ───────────────────────────────────────────────

    #[test]
    fn mha_construct_and_getters() {
        let mha = PyMultiHeadAttention::new(8, 2).expect("construct");
        assert_eq!(mha.embed_dim(), 8);
        assert_eq!(mha.num_heads(), 2);
        assert_eq!(mha.head_dim(), 4);
    }

    #[test]
    fn mha_not_divisible_is_value_error() {
        assert!(PyMultiHeadAttention::new(5, 2).is_err());
    }

    #[test]
    fn mha_self_attention_shape() {
        let mha = PyMultiHeadAttention::new(4, 2).expect("construct");
        let x = vec![1.0_f32; 3 * 4];
        let out = mha.self_attention(x, 3, None).expect("self_attention");
        assert_eq!(out.len(), 3 * 4);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn mha_cross_attention_shape() {
        let mha = PyMultiHeadAttention::new(8, 2).expect("construct");
        let query = vec![1.0_f32; 3 * 8];
        let context = vec![1.0_f32; 7 * 8];
        let out = mha
            .cross_attention(query, 3, context, 7, None)
            .expect("cross_attention");
        assert_eq!(out.len(), 3 * 8);
    }

    #[test]
    fn mha_wrong_input_len_is_value_error() {
        let mha = PyMultiHeadAttention::new(8, 2).expect("construct");
        let x = vec![1.0_f32; 5 * 4]; // feature dim 4 != embed_dim 8
        assert!(mha.self_attention(x, 5, None).is_err());
    }

    #[test]
    fn mha_load_weights_valid() {
        let mut mha = PyMultiHeadAttention::new(2, 1).expect("construct");
        let w = vec![0.0_f32; 4];
        let b = vec![0.0_f32; 2];
        assert!(mha
            .load_weights(
                w.clone(),
                w.clone(),
                w.clone(),
                w,
                b.clone(),
                b.clone(),
                b.clone(),
                b
            )
            .is_ok());
    }

    #[test]
    fn mha_load_weights_wrong_len_is_value_error() {
        let mut mha = PyMultiHeadAttention::new(2, 1).expect("construct");
        let bad_w = vec![0.0_f32; 3]; // should be 4
        let w = vec![0.0_f32; 4];
        let b = vec![0.0_f32; 2];
        assert!(mha
            .load_weights(
                bad_w,
                w.clone(),
                w.clone(),
                w,
                b.clone(),
                b.clone(),
                b.clone(),
                b
            )
            .is_err());
    }

    #[test]
    fn mha_repr_contains_dims() {
        let mha = PyMultiHeadAttention::new(8, 2).expect("construct");
        let r = mha.__repr__();
        assert!(r.contains('8'));
        assert!(r.contains('2'));
    }

    // ── scaled_dot_product_attention ─────────────────────────────────────

    #[test]
    fn sdpa_shape() {
        let out = scaled_dot_product_attention(
            vec![1.0; 3 * 4],
            3,
            4,
            vec![1.0; 5 * 4],
            5,
            vec![1.0; 5 * 6],
            6,
            None,
        )
        .expect("sdpa");
        assert_eq!(out.len(), 3 * 6);
    }

    #[test]
    fn sdpa_dk_mismatch_is_value_error() {
        assert!(scaled_dot_product_attention(
            vec![1.0; 3 * 4],
            3,
            4,
            vec![1.0; 5 * 3],
            5,
            vec![1.0; 5 * 4],
            4,
            None,
        )
        .is_err());
    }

    // ── flash_attention ──────────────────────────────────────────────────

    #[test]
    fn flash_matches_standard() {
        let q = vec![1.0_f32; 3 * 4];
        let k = vec![1.0_f32; 5 * 4];
        let v = vec![1.0_f32; 5 * 6];
        let std_out =
            scaled_dot_product_attention(q.clone(), 3, 4, k.clone(), 5, v.clone(), 6, None)
                .expect("sdpa");
        let flash_out = flash_attention(q, 3, 4, k, 5, v, 6, 2, None).expect("flash_attention");
        for (a, b) in flash_out.iter().zip(std_out.iter()) {
            assert!(close(*a, *b));
        }
    }

    // ── masks ─────────────────────────────────────────────────────────────

    #[test]
    fn causal_mask_shape_and_values() {
        let m = causal_mask(3).expect("causal_mask");
        assert_eq!(m.len(), 9);
        assert!(close(m[0], 0.0));
        assert!(close(m[1], -1e9));
    }

    #[test]
    fn causal_mask_zero_is_value_error() {
        assert!(causal_mask(0).is_err());
    }

    #[test]
    fn causal_mask_rect_shape() {
        let m = causal_mask_rect(2, 4).expect("causal_mask_rect");
        assert_eq!(m.len(), 8);
    }

    #[test]
    fn sliding_window_mask_full_window_matches_causal() {
        let sw = sliding_window_mask(3, 3).expect("sliding_window_mask");
        let cm = causal_mask(3).expect("causal_mask");
        assert_eq!(sw, cm);
    }

    // ── positional encodings ────────────────────────────────────────────

    #[test]
    fn sinusoidal_pe_shape() {
        let pe = sinusoidal_positional_encoding(10, 8).expect("pe");
        assert_eq!(pe.len(), 80);
    }

    #[test]
    fn sinusoidal_pe_odd_d_model_is_value_error() {
        assert!(sinusoidal_positional_encoding(4, 5).is_err());
    }

    #[test]
    fn add_positional_encoding_roundtrip() {
        let embeddings = vec![1.0_f32; 3 * 4];
        let out = add_positional_encoding(embeddings, 3, 4).expect("add_pe");
        let pe = sinusoidal_positional_encoding(3, 4).expect("pe");
        for (o, p) in out.iter().zip(pe.iter()) {
            assert!(close(*o, 1.0 + p));
        }
    }

    #[test]
    fn apply_rope_preserves_length() {
        let x = vec![1.0_f32; 4 * 8];
        let out = apply_rope(x, 4, 8, 10000.0).expect("rope");
        assert_eq!(out.len(), 32);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn apply_rope_odd_dim_is_value_error() {
        assert!(apply_rope(vec![1.0_f32; 15], 3, 5, 10000.0).is_err());
    }

    #[test]
    fn rope_frequencies_pos0() {
        let freq = rope_frequencies(4, 4, 10000.0).expect("rope_frequencies");
        assert!(close(freq[0], 1.0));
        assert!(close(freq[1], 0.0));
    }

    #[test]
    fn relative_position_shaw_shape() {
        let table = vec![0.1_f32; 5 * 2]; // max_dist=2 -> 2*2+1=5 rows
        let bias = relative_position_shaw(4, 2, 2, table).expect("shaw");
        assert_eq!(bias.len(), 16);
    }

    #[test]
    fn relative_position_shaw_wrong_table_len_is_value_error() {
        assert!(relative_position_shaw(4, 2, 2, vec![0.0_f32; 5]).is_err());
    }

    // ── tiled_matmul_t ────────────────────────────────────────────────────

    #[test]
    fn tiled_matmul_identity() {
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![1.0, 0.0, 0.0, 1.0];
        let c = tiled_matmul_t(a, b, 2, 2, 2, 2).expect("tiled_matmul_t");
        assert_eq!(c, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn tiled_matmul_wrong_a_len_is_value_error() {
        // a should be m*k = 2*2 = 4 but only has 3 elements.
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 0.0, 0.0, 1.0];
        assert!(tiled_matmul_t(a, b, 2, 2, 2, 2).is_err());
    }

    #[test]
    fn tiled_matmul_wrong_b_len_is_value_error() {
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![1.0, 0.0, 0.0]; // should be n*k = 4
        assert!(tiled_matmul_t(a, b, 2, 2, 2, 2).is_err());
    }
}

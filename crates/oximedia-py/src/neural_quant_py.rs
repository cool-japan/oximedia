//! `oximedia.neural` — INT`n` quantization for efficient inference.
//!
//! Two families, both real delegation (no fabricated results):
//!
//! * **Simple** — [`oximedia_neural::quantization`] (re-exported at the
//!   crate root): a single scalar scale per tensor, `bits` configurable in
//!   `[2, 8]`, `scale = max(|x|) / max_int`, `zero_point` always `0`.
//!   [`PyQuantizedTensor`] / [`PyQuantizedLinearLayer`] /
//!   [`quantize_tensor`] / [`dequantize_tensor`] / [`quantize_layer_weights`].
//! * **Advanced** — [`oximedia_neural::quantize`] (not re-exported at the
//!   crate root; imported directly from its module path here): fixed INT8
//!   range `[-128, 127]`, optional **per-channel** scales (one scale per
//!   first-dimension "channel", e.g. per output-channel row of a weight
//!   matrix or kernel), and a selectable calibration method (`"min_max"`
//!   or `"percentile"`, the latter robust to outliers). Conv2d weights are
//!   always quantized per-channel (matching the underlying Rust type,
//!   which ignores `per_channel=False` for that one constructor).
//!   [`PyPerChannelQuantizedTensor`] / [`PyQuantizedLinear`] /
//!   [`PyQuantizedConv2d`] / [`quantize_tensor_per_channel`] /
//!   [`dequantize_tensor_per_channel`].

use oximedia_neural::quantize::{
    dequantize as pc_dequantize, quantize as pc_quantize, CalibrationMethod, QuantizationConfig,
    QuantizedConv2d, QuantizedLinear, QuantizedTensor as PcQuantizedTensor,
};
use oximedia_neural::{QuantizedLinearLayer, SymmetricQuantizedTensor, Tensor};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::neural_layers_py::PyLinearLayer;
use crate::neural_py::neural_err;

/// Parses a calibration-method name (`"min_max"` or `"percentile"`).
///
/// Raises ``ValueError`` naming the unrecognised value otherwise (no silent
/// fallback to a default).
fn parse_calibration(name: &str) -> PyResult<CalibrationMethod> {
    match name {
        "min_max" => Ok(CalibrationMethod::MinMax),
        "percentile" => Ok(CalibrationMethod::Percentile),
        other => Err(PyValueError::new_err(format!(
            "unknown calibration method {other:?}: expected 'min_max' or 'percentile'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// QuantizedTensor
// ---------------------------------------------------------------------------

/// An INT`n`-quantized tensor with a single scalar scale and zero-point.
///
/// Only constructible via [`quantize_tensor`]; opaque otherwise.
#[pyclass(name = "QuantizedTensor")]
pub struct PyQuantizedTensor {
    inner: SymmetricQuantizedTensor,
}

#[pymethods]
impl PyQuantizedTensor {
    /// Raw quantized `int8` values, row-major, same logical shape as the
    /// source tensor.
    fn data(&self) -> Vec<i8> {
        self.inner.data.clone()
    }

    /// Shape of the tensor (same as the source tensor it was quantized from).
    fn shape(&self) -> Vec<usize> {
        self.inner.shape().to_vec()
    }

    /// Total number of elements.
    fn numel(&self) -> usize {
        self.inner.numel()
    }

    /// Quantization scale: `scale = max_abs / max_int`.
    #[getter]
    fn scale(&self) -> f32 {
        self.inner.scale
    }

    /// Zero-point offset (always `0` for this symmetric scheme).
    #[getter]
    fn zero_point(&self) -> i8 {
        self.inner.zero_point
    }

    fn __repr__(&self) -> String {
        format!(
            "QuantizedTensor(shape={:?}, scale={}, zero_point={})",
            self.inner.shape, self.inner.scale, self.inner.zero_point
        )
    }
}

/// Quantizes a flat row-major tensor to INT`bits` using symmetric
/// min-max quantization.
///
/// `bits` must be in `[2, 8]`. Raises ``ValueError`` if `bits` is out of
/// range, the tensor is empty, or `shape` does not match `len(data)`.
#[pyfunction]
fn quantize_tensor(data: Vec<f32>, shape: Vec<usize>, bits: u8) -> PyResult<PyQuantizedTensor> {
    let t = Tensor::from_data(data, shape).map_err(neural_err)?;
    let inner = oximedia_neural::quantize_tensor(&t, bits).map_err(neural_err)?;
    Ok(PyQuantizedTensor { inner })
}

/// De-quantizes an INT`n` tensor back to `f32`.
///
/// Returns `(data, shape)`.
#[pyfunction]
fn dequantize_tensor(qt: &PyQuantizedTensor) -> PyResult<(Vec<f32>, Vec<usize>)> {
    let t = oximedia_neural::dequantize_tensor(&qt.inner).map_err(neural_err)?;
    Ok((t.data().to_vec(), t.shape().to_vec()))
}

// ---------------------------------------------------------------------------
// QuantizedLinearLayer
// ---------------------------------------------------------------------------

/// A fully-connected layer whose weight matrix is stored as INT`n`; the
/// bias stays `f32`. Integer matrix products are accumulated in `i32`.
///
/// Only constructible via [`quantize_layer_weights`].
#[pyclass(name = "QuantizedLinearLayer")]
pub struct PyQuantizedLinearLayer {
    inner: QuantizedLinearLayer,
}

#[pymethods]
impl PyQuantizedLinearLayer {
    #[getter]
    fn in_features(&self) -> usize {
        self.inner.in_features
    }

    #[getter]
    fn out_features(&self) -> usize {
        self.inner.out_features
    }

    /// Forward inference using integer arithmetic internally. ``input``
    /// must have length ``in_features``.
    fn forward(&self, input: Vec<f32>) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![self.inner.in_features]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    /// Batched forward inference. ``input`` is a flat row-major
    /// ``[batch_size, in_features]`` buffer.
    fn forward_batch(&self, input: Vec<f32>, batch_size: usize) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![batch_size, self.inner.in_features])
            .map_err(neural_err)?;
        let out = self.inner.forward_batch(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        format!(
            "QuantizedLinearLayer(in_features={}, out_features={})",
            self.inner.in_features, self.inner.out_features
        )
    }
}

/// Quantizes the weight matrix of a [`LinearLayer`][crate::neural_layers_py::PyLinearLayer]
/// to INT`bits` (bias stays `f32`).
#[pyfunction]
fn quantize_layer_weights(layer: &PyLinearLayer, bits: u8) -> PyResult<PyQuantizedLinearLayer> {
    let inner = oximedia_neural::quantize_layer_weights(&layer.inner, bits).map_err(neural_err)?;
    Ok(PyQuantizedLinearLayer { inner })
}

// ---------------------------------------------------------------------------
// PerChannelQuantizedTensor (advanced `quantize` module)
// ---------------------------------------------------------------------------

/// An INT8-quantized tensor from the *advanced* per-channel quantization
/// API ([`oximedia_neural::quantize`]) — as opposed to
/// [`PyQuantizedTensor`] above (the simple single-scale API). Depending on
/// how it was produced, [`scales`][Self::scales] holds either one value
/// (per-tensor) or `shape()[0]` values (per-channel).
///
/// Only constructible via [`quantize_tensor_per_channel`].
#[pyclass(name = "PerChannelQuantizedTensor")]
pub struct PyPerChannelQuantizedTensor {
    inner: PcQuantizedTensor,
}

#[pymethods]
impl PyPerChannelQuantizedTensor {
    /// Raw quantized `int8` values, row-major, same logical shape as the
    /// source tensor.
    fn data(&self) -> Vec<i8> {
        self.inner.data.clone()
    }

    /// Shape of the tensor (same as the source tensor it was quantized from).
    fn shape(&self) -> Vec<usize> {
        self.inner.shape().to_vec()
    }

    /// Total number of elements.
    fn numel(&self) -> usize {
        self.inner.numel()
    }

    /// Quantization scale(s): length `1` for per-tensor, or length
    /// `shape()[0]` for per-channel.
    fn scales(&self) -> Vec<f32> {
        self.inner.scales.clone()
    }

    /// The scale that applies to the element at flat row-major index `flat`
    /// (derives the owning channel from `flat` when quantized per-channel).
    fn scale_for(&self, flat: usize) -> f32 {
        self.inner.scale_for(flat)
    }

    fn __repr__(&self) -> String {
        format!(
            "PerChannelQuantizedTensor(shape={:?}, scales={:?})",
            self.inner.shape, self.inner.scales
        )
    }
}

/// Quantizes a flat row-major tensor to INT8 (fixed `[-128, 127]` range)
/// using the advanced per-tensor-or-per-channel API.
///
/// With `per_channel=True` (and at least 2 dimensions), one scale is
/// computed per first-dimension "channel" (e.g. per output-channel row of
/// a weight matrix); otherwise a single scale covers the whole tensor.
/// `calibration` is `"min_max"` (default; scale from `max(|x|)`) or
/// `"percentile"` (99th percentile of `|x|`, robust to outliers).
///
/// Raises ``ValueError`` if the tensor is empty, `shape` does not match
/// `len(data)`, or `calibration` is not a recognised name.
#[pyfunction]
#[pyo3(signature = (data, shape, per_channel=false, calibration="min_max"))]
fn quantize_tensor_per_channel(
    data: Vec<f32>,
    shape: Vec<usize>,
    per_channel: bool,
    calibration: &str,
) -> PyResult<PyPerChannelQuantizedTensor> {
    let t = Tensor::from_data(data, shape).map_err(neural_err)?;
    let cfg = QuantizationConfig {
        per_channel,
        calibration: parse_calibration(calibration)?,
    };
    let inner = pc_quantize(&t, &cfg).map_err(neural_err)?;
    Ok(PyPerChannelQuantizedTensor { inner })
}

/// De-quantizes a [`PerChannelQuantizedTensor`] (from
/// [`quantize_tensor_per_channel`]) back to `f32`. Returns `(data, shape)`.
#[pyfunction]
fn dequantize_tensor_per_channel(
    qt: &PyPerChannelQuantizedTensor,
) -> PyResult<(Vec<f32>, Vec<usize>)> {
    let t = pc_dequantize(&qt.inner).map_err(neural_err)?;
    Ok((t.data().to_vec(), t.shape().to_vec()))
}

// ---------------------------------------------------------------------------
// QuantizedLinear (advanced `quantize` module — per-channel weight scales)
// ---------------------------------------------------------------------------

/// A fully-connected layer quantized via the *advanced* per-channel API:
/// the weight matrix carries one scale per output-feature row (not a
/// single scalar scale, unlike [`PyQuantizedLinearLayer`]); the bias stays
/// `f32`. Integer matrix products are accumulated in `i32`.
///
/// Only constructible via [`from_weights`][Self::from_weights].
#[pyclass(name = "QuantizedLinear")]
pub struct PyQuantizedLinear {
    inner: QuantizedLinear,
}

#[pymethods]
impl PyQuantizedLinear {
    /// Quantizes a `[out_features, in_features]` row-major weight matrix
    /// (plus an `[out_features]` bias) into a `QuantizedLinear`.
    ///
    /// `per_channel` (default `True`) computes one scale per output-feature
    /// row; `calibration` is `"min_max"` (default) or `"percentile"`.
    ///
    /// Raises ``ValueError`` if `weight`/`bias` lengths don't match
    /// `in_features`/`out_features`, or `calibration` is unrecognised.
    #[staticmethod]
    #[pyo3(signature = (weight, bias, in_features, out_features, per_channel=true, calibration="min_max"))]
    #[allow(clippy::too_many_arguments)]
    fn from_weights(
        weight: Vec<f32>,
        bias: Vec<f32>,
        in_features: usize,
        out_features: usize,
        per_channel: bool,
        calibration: &str,
    ) -> PyResult<Self> {
        let w = Tensor::from_data(weight, vec![out_features, in_features]).map_err(neural_err)?;
        let b = Tensor::from_data(bias, vec![out_features]).map_err(neural_err)?;
        let cfg = QuantizationConfig {
            per_channel,
            calibration: parse_calibration(calibration)?,
        };
        let inner = QuantizedLinear::from_weights(&w, &b, &cfg).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn in_features(&self) -> usize {
        self.inner.in_features
    }

    #[getter]
    fn out_features(&self) -> usize {
        self.inner.out_features
    }

    /// Forward inference using integer arithmetic internally. ``input``
    /// must have length ``in_features``.
    fn forward(&self, input: Vec<f32>) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![self.inner.in_features]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        format!(
            "QuantizedLinear(in_features={}, out_features={})",
            self.inner.in_features, self.inner.out_features
        )
    }
}

// ---------------------------------------------------------------------------
// QuantizedConv2d (advanced `quantize` module)
// ---------------------------------------------------------------------------

/// A 2-D convolution layer quantized via the *advanced* per-channel API:
/// kernels carry one scale per output channel (always per-channel — the
/// underlying Rust constructor forces this regardless of a `per_channel`
/// flag, so none is exposed here); the bias stays `f32`.
///
/// Only constructible via [`from_weights`][Self::from_weights].
#[pyclass(name = "QuantizedConv2d")]
pub struct PyQuantizedConv2d {
    inner: QuantizedConv2d,
}

#[pymethods]
impl PyQuantizedConv2d {
    /// Quantizes a `[out_channels, in_channels, kernel_h, kernel_w]`
    /// row-major kernel tensor (plus an `[out_channels]` bias).
    ///
    /// `calibration` is `"min_max"` (default) or `"percentile"`.
    ///
    /// Raises ``ValueError`` if `weight`/`bias` lengths are wrong, `stride`
    /// has a zero component, or `calibration` is unrecognised.
    #[staticmethod]
    #[pyo3(signature = (weight, bias, out_channels, in_channels, kernel_h, kernel_w, stride, padding, calibration="min_max"))]
    #[allow(clippy::too_many_arguments)]
    fn from_weights(
        weight: Vec<f32>,
        bias: Vec<f32>,
        out_channels: usize,
        in_channels: usize,
        kernel_h: usize,
        kernel_w: usize,
        stride: (usize, usize),
        padding: (usize, usize),
        calibration: &str,
    ) -> PyResult<Self> {
        let w = Tensor::from_data(weight, vec![out_channels, in_channels, kernel_h, kernel_w])
            .map_err(neural_err)?;
        let b = Tensor::from_data(bias, vec![out_channels]).map_err(neural_err)?;
        let cfg = QuantizationConfig {
            per_channel: true,
            calibration: parse_calibration(calibration)?,
        };
        let inner =
            QuantizedConv2d::from_weights(&w, &b, stride, padding, &cfg).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn in_channels(&self) -> usize {
        self.inner.in_channels
    }

    #[getter]
    fn out_channels(&self) -> usize {
        self.inner.out_channels
    }

    #[getter]
    fn kernel_h(&self) -> usize {
        self.inner.kernel_h
    }

    #[getter]
    fn kernel_w(&self) -> usize {
        self.inner.kernel_w
    }

    #[getter]
    fn stride(&self) -> (usize, usize) {
        self.inner.stride
    }

    #[getter]
    fn padding(&self) -> (usize, usize) {
        self.inner.padding
    }

    /// Forward pass. ``input`` is a flat row-major `[in_channels, height,
    /// width]` buffer. Returns ``(data, shape)`` where ``shape`` is
    /// ``[out_channels, out_height, out_width]``.
    fn forward(
        &self,
        input: Vec<f32>,
        height: usize,
        width: usize,
    ) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let t = Tensor::from_data(input, vec![self.inner.in_channels, height, width])
            .map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    fn __repr__(&self) -> String {
        format!(
            "QuantizedConv2d(in_channels={}, out_channels={}, kernel=({}, {}), stride={:?}, padding={:?})",
            self.inner.in_channels,
            self.inner.out_channels,
            self.inner.kernel_h,
            self.inner.kernel_w,
            self.inner.stride,
            self.inner.padding
        )
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the quantization classes/functions into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyQuantizedTensor>()?;
    m.add_class::<PyQuantizedLinearLayer>()?;
    m.add_class::<PyPerChannelQuantizedTensor>()?;
    m.add_class::<PyQuantizedLinear>()?;
    m.add_class::<PyQuantizedConv2d>()?;
    m.add_function(wrap_pyfunction!(quantize_tensor, m)?)?;
    m.add_function(wrap_pyfunction!(dequantize_tensor, m)?)?;
    m.add_function(wrap_pyfunction!(quantize_layer_weights, m)?)?;
    m.add_function(wrap_pyfunction!(quantize_tensor_per_channel, m)?)?;
    m.add_function(wrap_pyfunction!(dequantize_tensor_per_channel, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    // ── quantize_tensor / dequantize_tensor ────────────────────────────

    #[test]
    fn quantize_dequantize_roundtrip_close() {
        let data = vec![0.5, -0.5, 1.0, -1.0, 0.0, 0.25];
        let qt = quantize_tensor(data.clone(), vec![6], 8).expect("quantize_tensor");
        assert_eq!(qt.zero_point(), 0);
        let (dq, shape) = dequantize_tensor(&qt).expect("dequantize_tensor");
        assert_eq!(shape, vec![6]);
        for (orig, recon) in data.iter().zip(dq.iter()) {
            assert!(close(*orig, *recon, 0.02), "orig={orig} recon={recon}");
        }
    }

    #[test]
    fn quantize_bits_out_of_range_is_value_error() {
        assert!(quantize_tensor(vec![1.0, 2.0], vec![2], 1).is_err());
        assert!(quantize_tensor(vec![1.0, 2.0], vec![2], 9).is_err());
    }

    #[test]
    fn quantize_shape_mismatch_is_value_error() {
        assert!(quantize_tensor(vec![1.0, 2.0, 3.0], vec![2, 2], 8).is_err());
    }

    #[test]
    fn quantize_tensor_data_and_shape_accessors() {
        let qt = quantize_tensor(vec![1.0, -1.0, 0.5, -0.5], vec![2, 2], 8).expect("quantize");
        assert_eq!(qt.shape(), vec![2, 2]);
        assert_eq!(qt.numel(), 4);
        assert_eq!(qt.data().len(), 4);
    }

    #[test]
    fn quantize_tensor_repr_contains_shape() {
        let qt = quantize_tensor(vec![1.0, 2.0], vec![2], 8).expect("quantize");
        assert!(qt.__repr__().contains("[2]"));
    }

    // ── quantize_layer_weights / QuantizedLinearLayer ──────────────────
    //
    // `PyLinearLayer`'s `#[pymethods]` are intentionally not `pub` (PyO3
    // dispatch doesn't need Rust-level visibility; only Python callers are
    // meant to construct one). Build it directly from the underlying Rust
    // type via the `pub(crate)` `inner` field instead, exactly mirroring
    // what `neural_layers_py`'s own tests do internally.

    fn make_linear(in_features: usize, out_features: usize) -> PyLinearLayer {
        PyLinearLayer {
            inner: oximedia_neural::layers::LinearLayer::new(in_features, out_features)
                .expect("linear layer"),
        }
    }

    #[test]
    fn quantize_layer_weights_shape() {
        let layer = make_linear(4, 3);
        let qll = quantize_layer_weights(&layer, 8).expect("quantize_layer_weights");
        assert_eq!(qll.in_features(), 4);
        assert_eq!(qll.out_features(), 3);
    }

    #[test]
    fn quantized_linear_forward_identity_approx() {
        let mut layer = make_linear(2, 2);
        layer.inner.weight =
            Tensor::from_data(vec![1.0, 0.0, 0.0, 1.0], vec![2, 2]).expect("weight");
        layer.inner.bias = Tensor::from_data(vec![0.0, 0.0], vec![2]).expect("bias");
        let qll = quantize_layer_weights(&layer, 8).expect("quantize_layer_weights");
        let out = qll.forward(vec![0.5, -0.5]).expect("forward");
        assert!(close(out[0], 0.5, 0.05));
        assert!(close(out[1], -0.5, 0.05));
    }

    #[test]
    fn quantized_linear_forward_wrong_len_is_value_error() {
        let layer = make_linear(4, 2);
        let qll = quantize_layer_weights(&layer, 8).expect("quantize_layer_weights");
        assert!(qll.forward(vec![1.0, 2.0]).is_err());
    }

    #[test]
    fn quantized_linear_forward_batch_shape() {
        let layer = make_linear(4, 3);
        let qll = quantize_layer_weights(&layer, 8).expect("quantize_layer_weights");
        let out = qll
            .forward_batch(vec![1.0_f32; 5 * 4], 5)
            .expect("forward_batch");
        assert_eq!(out.len(), 5 * 3);
    }

    // ── parse_calibration ───────────────────────────────────────────────

    #[test]
    fn parse_calibration_known_names() {
        assert!(matches!(
            parse_calibration("min_max"),
            Ok(CalibrationMethod::MinMax)
        ));
        assert!(matches!(
            parse_calibration("percentile"),
            Ok(CalibrationMethod::Percentile)
        ));
    }

    #[test]
    fn parse_calibration_unknown_is_value_error() {
        assert!(parse_calibration("bogus").is_err());
    }

    // ── quantize_tensor_per_channel / dequantize_tensor_per_channel ────

    #[test]
    fn per_channel_roundtrip_close_per_tensor_mode() {
        let data = vec![0.5, -0.5, 1.0, -1.0, 0.0, 0.25];
        let qt = quantize_tensor_per_channel(data.clone(), vec![6], false, "min_max")
            .expect("quantize_tensor_per_channel");
        assert_eq!(qt.scales().len(), 1, "per_channel=false -> one scale");
        let (dq, shape) = dequantize_tensor_per_channel(&qt).expect("dequantize");
        assert_eq!(shape, vec![6]);
        for (orig, recon) in data.iter().zip(dq.iter()) {
            assert!(close(*orig, *recon, 0.02), "orig={orig} recon={recon}");
        }
    }

    #[test]
    fn per_channel_mode_yields_one_scale_per_row() {
        // Two rows with very different magnitudes.
        let data = vec![1.0_f32, 1.0, 100.0, 100.0];
        let qt = quantize_tensor_per_channel(data, vec![2, 2], true, "min_max")
            .expect("quantize_tensor_per_channel");
        assert_eq!(qt.scales().len(), 2);
        assert!(qt.scales()[0] < qt.scales()[1]);
        assert_eq!(qt.shape(), vec![2, 2]);
        assert_eq!(qt.numel(), 4);
        assert_eq!(qt.data().len(), 4);
    }

    #[test]
    fn per_channel_zero_dim_shape_is_value_error() {
        // `shape=[0]` is rejected by `Tensor::from_data` itself (no
        // dimension may be 0) before `quantize()`'s own `numel() == 0`
        // guard could run — a `Tensor` can never have `numel() == 0` once
        // constructed, so that guard is unreachable from this binding.
        // This still confirms the FFI boundary raises `ValueError`, same
        // as `neural_py::tests::tensor_rejects_zero_dimension`.
        assert!(quantize_tensor_per_channel(vec![], vec![0], false, "min_max").is_err());
    }

    #[test]
    fn per_channel_unknown_calibration_is_value_error() {
        assert!(quantize_tensor_per_channel(vec![1.0, 2.0], vec![2], false, "bogus").is_err());
    }

    #[test]
    fn per_channel_repr_contains_shape() {
        let qt = quantize_tensor_per_channel(vec![1.0, 2.0], vec![2], false, "min_max")
            .expect("quantize");
        assert!(qt.__repr__().contains("[2]"));
    }

    // ── QuantizedLinear (advanced, per-channel) ─────────────────────────

    #[test]
    fn quantized_linear_advanced_forward_identity_approx() {
        // 2x2 identity weight, zero bias -> output approx input.
        let ql = PyQuantizedLinear::from_weights(
            vec![1.0, 0.0, 0.0, 1.0],
            vec![0.0, 0.0],
            2,
            2,
            true,
            "min_max",
        )
        .expect("from_weights");
        assert_eq!(ql.in_features(), 2);
        assert_eq!(ql.out_features(), 2);
        let out = ql.forward(vec![0.5, -0.5]).expect("forward");
        assert!(close(out[0], 0.5, 0.05));
        assert!(close(out[1], -0.5, 0.05));
    }

    #[test]
    fn quantized_linear_advanced_wrong_input_len_is_value_error() {
        let ql = PyQuantizedLinear::from_weights(
            vec![0.0_f32; 4 * 3],
            vec![0.0_f32; 3],
            4,
            3,
            true,
            "min_max",
        )
        .expect("from_weights");
        assert!(ql.forward(vec![1.0, 2.0]).is_err());
    }

    #[test]
    fn quantized_linear_advanced_unknown_calibration_is_value_error() {
        assert!(PyQuantizedLinear::from_weights(
            vec![0.0_f32; 4],
            vec![0.0_f32; 2],
            2,
            2,
            true,
            "bogus",
        )
        .is_err());
    }

    #[test]
    fn quantized_linear_advanced_repr_contains_features() {
        let ql = PyQuantizedLinear::from_weights(
            vec![0.0_f32; 8],
            vec![0.0_f32; 2],
            4,
            2,
            true,
            "min_max",
        )
        .expect("from_weights");
        let r = ql.__repr__();
        assert!(r.contains('4'));
        assert!(r.contains('2'));
    }

    // ── QuantizedConv2d (advanced, always per-channel) ──────────────────

    #[test]
    fn quantized_conv2d_advanced_output_shape() {
        let qc = PyQuantizedConv2d::from_weights(
            vec![1.0_f32; 2 * 1 * 3 * 3],
            vec![0.0_f32; 2],
            2,
            1,
            3,
            3,
            (1, 1),
            (0, 0),
            "min_max",
        )
        .expect("from_weights");
        assert_eq!(qc.in_channels(), 1);
        assert_eq!(qc.out_channels(), 2);
        assert_eq!(qc.kernel_h(), 3);
        assert_eq!(qc.kernel_w(), 3);
        let (data, shape) = qc.forward(vec![1.0_f32; 1 * 5 * 5], 5, 5).expect("forward");
        assert_eq!(shape, vec![2, 3, 3]);
        assert_eq!(data.len(), 2 * 3 * 3);
    }

    #[test]
    fn quantized_conv2d_advanced_zero_stride_is_value_error() {
        assert!(PyQuantizedConv2d::from_weights(
            vec![0.0_f32; 2 * 1 * 3 * 3],
            vec![0.0_f32; 2],
            2,
            1,
            3,
            3,
            (0, 1),
            (0, 0),
            "min_max",
        )
        .is_err());
    }

    #[test]
    fn quantized_conv2d_advanced_wrong_channels_is_value_error() {
        let qc = PyQuantizedConv2d::from_weights(
            vec![0.0_f32; 2 * 1 * 3 * 3],
            vec![0.0_f32; 2],
            2,
            1,
            3,
            3,
            (1, 1),
            (0, 0),
            "min_max",
        )
        .expect("from_weights");
        // Input claims 2 channels but the layer expects 1.
        assert!(qc.forward(vec![0.0_f32; 2 * 4 * 4], 4, 4).is_err());
    }
}

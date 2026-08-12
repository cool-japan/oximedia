//! `oximedia.neural` — individual neural network layers.
//!
//! Real delegation to [`oximedia_neural::layers`]. These are the building
//! blocks the four pre-built [`crate::neural_py`] media models are made
//! from, exposed directly for custom architectures. All layers are
//! zero-initialised at construction; use each class's `load_weights` /
//! `load_params` method to install pre-trained parameters.
//!
//! Inputs/outputs are plain flat `list[float]` buffers (row-major) plus
//! explicit dimension integers. Layers whose output spatial shape is not
//! trivially derivable by the caller (`Conv2dLayer`, `MaxPool2d`,
//! `AvgPool2d`) return `(data, shape)` pairs; shape-preserving layers
//! return a flat buffer only. Only single-sample (`[C, H, W]`, no batch
//! dimension) forward passes are bound — batch inference is a Python-side
//! loop over `forward()`.

use oximedia_neural::layers::{
    AvgPool2d, BatchNorm1d, BatchNorm2d, Conv2dLayer, GlobalAvgPool, LinearLayer, MaxPool2d,
};
use oximedia_neural::Tensor;
use pyo3::prelude::*;

use crate::neural_py::neural_err;

// ---------------------------------------------------------------------------
// LinearLayer
// ---------------------------------------------------------------------------

/// Fully-connected (dense) layer: `output = W * input + b`.
#[pyclass(name = "LinearLayer")]
pub struct PyLinearLayer {
    pub(crate) inner: LinearLayer,
}

#[pymethods]
impl PyLinearLayer {
    /// Creates a zero-initialised `LinearLayer`. Raises ``ValueError`` if
    /// either size is 0.
    #[new]
    fn new(in_features: usize, out_features: usize) -> PyResult<Self> {
        let inner = LinearLayer::new(in_features, out_features).map_err(neural_err)?;
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

    /// Installs pre-trained weight/bias. ``weight`` must have length
    /// ``out_features * in_features`` (row-major ``[out_features, in_features]``);
    /// ``bias`` must have length ``out_features``.
    fn load_weights(&mut self, weight: Vec<f32>, bias: Vec<f32>) -> PyResult<()> {
        let (in_f, out_f) = (self.inner.in_features, self.inner.out_features);
        self.inner.weight = Tensor::from_data(weight, vec![out_f, in_f]).map_err(neural_err)?;
        self.inner.bias = Tensor::from_data(bias, vec![out_f]).map_err(neural_err)?;
        Ok(())
    }

    /// Forward pass. ``input`` must have length ``in_features``.
    /// Returns a buffer of length ``out_features``.
    fn forward(&self, input: Vec<f32>) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![self.inner.in_features]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        format!(
            "LinearLayer(in_features={}, out_features={})",
            self.inner.in_features, self.inner.out_features
        )
    }
}

// ---------------------------------------------------------------------------
// Conv2dLayer
// ---------------------------------------------------------------------------

/// 2-D convolution layer (im2col + GEMM). Input is `[in_channels, H, W]`.
#[pyclass(name = "Conv2dLayer")]
pub struct PyConv2dLayer {
    inner: Conv2dLayer,
}

#[pymethods]
impl PyConv2dLayer {
    /// Creates a zero-initialised `Conv2dLayer`. Raises ``ValueError`` if
    /// any size is 0.
    #[new]
    #[allow(clippy::too_many_arguments)]
    fn new(
        in_channels: usize,
        out_channels: usize,
        kernel_h: usize,
        kernel_w: usize,
        stride: (usize, usize),
        padding: (usize, usize),
    ) -> PyResult<Self> {
        let inner = Conv2dLayer::new(
            in_channels,
            out_channels,
            kernel_h,
            kernel_w,
            stride,
            padding,
        )
        .map_err(neural_err)?;
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

    /// Installs pre-trained weight/bias. ``weight`` must have length
    /// ``out_channels * in_channels * kernel_h * kernel_w``; ``bias`` must
    /// have length ``out_channels``.
    fn load_weights(&mut self, weight: Vec<f32>, bias: Vec<f32>) -> PyResult<()> {
        let (oc, ic, kh, kw) = (
            self.inner.out_channels,
            self.inner.in_channels,
            self.inner.kernel_h,
            self.inner.kernel_w,
        );
        self.inner.weight = Tensor::from_data(weight, vec![oc, ic, kh, kw]).map_err(neural_err)?;
        self.inner.bias = Tensor::from_data(bias, vec![oc]).map_err(neural_err)?;
        Ok(())
    }

    /// Forward pass. ``input`` is a flat row-major `[in_channels, height,
    /// width]` buffer.
    ///
    /// Returns ``(data, shape)`` where ``shape`` is
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
            "Conv2dLayer(in_channels={}, out_channels={}, kernel=({}, {}), stride={:?}, padding={:?})",
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
// BatchNorm1d
// ---------------------------------------------------------------------------

/// 1-D batch normalisation (inference mode, uses running statistics).
#[pyclass(name = "BatchNorm1d")]
pub struct PyBatchNorm1d {
    inner: BatchNorm1d,
}

#[pymethods]
impl PyBatchNorm1d {
    /// Creates a `BatchNorm1d` that is a no-op until [`load_params`] is
    /// called (γ=1, β=0, mean=0, var=1). Raises ``ValueError`` if
    /// ``num_features`` is 0.
    #[new]
    fn new(num_features: usize) -> PyResult<Self> {
        let inner = BatchNorm1d::new(num_features).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn num_features(&self) -> usize {
        self.inner.num_features
    }

    /// Installs pre-computed scale/shift/running statistics. Each buffer
    /// must have length ``num_features``.
    fn load_params(
        &mut self,
        weight: Vec<f32>,
        bias: Vec<f32>,
        running_mean: Vec<f32>,
        running_var: Vec<f32>,
        eps: f32,
    ) -> PyResult<()> {
        let n = self.inner.num_features;
        for (name, len) in [
            ("weight", weight.len()),
            ("bias", bias.len()),
            ("running_mean", running_mean.len()),
            ("running_var", running_var.len()),
        ] {
            if len != n {
                return Err(neural_err(oximedia_neural::NeuralError::ShapeMismatch(
                    format!("BatchNorm1d::load_params: {name} length {len} != num_features {n}"),
                )));
            }
        }
        self.inner.weight = weight;
        self.inner.bias = bias;
        self.inner.running_mean = running_mean;
        self.inner.running_var = running_var;
        self.inner.eps = eps;
        Ok(())
    }

    /// Forward pass. ``input`` must have length ``num_features``.
    fn forward(&self, input: Vec<f32>) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![self.inner.num_features]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        format!("BatchNorm1d(num_features={})", self.inner.num_features)
    }
}

// ---------------------------------------------------------------------------
// BatchNorm2d
// ---------------------------------------------------------------------------

/// 2-D (spatial) batch normalisation (inference mode). Input is
/// `[channels, H, W]`.
#[pyclass(name = "BatchNorm2d")]
pub struct PyBatchNorm2d {
    inner: BatchNorm2d,
}

#[pymethods]
impl PyBatchNorm2d {
    /// Creates a `BatchNorm2d` that is a no-op until [`load_params`] is
    /// called. Raises ``ValueError`` if ``num_features`` is 0.
    #[new]
    fn new(num_features: usize) -> PyResult<Self> {
        let inner = BatchNorm2d::new(num_features).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn num_features(&self) -> usize {
        self.inner.num_features
    }

    /// Installs pre-computed scale/shift/running statistics. Each buffer
    /// must have length ``num_features``.
    fn load_params(
        &mut self,
        weight: Vec<f32>,
        bias: Vec<f32>,
        running_mean: Vec<f32>,
        running_var: Vec<f32>,
        eps: f32,
    ) -> PyResult<()> {
        let n = self.inner.num_features;
        for (name, len) in [
            ("weight", weight.len()),
            ("bias", bias.len()),
            ("running_mean", running_mean.len()),
            ("running_var", running_var.len()),
        ] {
            if len != n {
                return Err(neural_err(oximedia_neural::NeuralError::ShapeMismatch(
                    format!("BatchNorm2d::load_params: {name} length {len} != num_features {n}"),
                )));
            }
        }
        self.inner.weight = weight;
        self.inner.bias = bias;
        self.inner.running_mean = running_mean;
        self.inner.running_var = running_var;
        self.inner.eps = eps;
        Ok(())
    }

    /// Forward pass. ``input`` is a flat row-major `[channels, height,
    /// width]` buffer. Returns a buffer of the same shape.
    fn forward(&self, input: Vec<f32>, height: usize, width: usize) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![self.inner.num_features, height, width])
            .map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        format!("BatchNorm2d(num_features={})", self.inner.num_features)
    }
}

// ---------------------------------------------------------------------------
// MaxPool2d
// ---------------------------------------------------------------------------

/// 2-D max pooling. Input is `[channels, H, W]`.
#[pyclass(name = "MaxPool2d")]
pub struct PyMaxPool2d {
    inner: MaxPool2d,
}

#[pymethods]
impl PyMaxPool2d {
    /// Creates a `MaxPool2d` layer. Raises ``ValueError`` if any kernel /
    /// stride dimension is 0.
    #[new]
    fn new(kernel: (usize, usize), stride: (usize, usize)) -> PyResult<Self> {
        let inner = MaxPool2d::new(kernel, stride).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn kernel(&self) -> (usize, usize) {
        self.inner.kernel
    }

    #[getter]
    fn stride(&self) -> (usize, usize) {
        self.inner.stride
    }

    /// Forward pass. ``input`` is a flat row-major `[channels, height,
    /// width]` buffer, ``channels`` inferred from ``len(input) / (height *
    /// width)`` via shape validation.
    ///
    /// Returns ``(data, shape)`` where ``shape`` is
    /// ``[channels, out_height, out_width]``.
    fn forward(
        &self,
        input: Vec<f32>,
        channels: usize,
        height: usize,
        width: usize,
    ) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let t = Tensor::from_data(input, vec![channels, height, width]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    fn __repr__(&self) -> String {
        format!(
            "MaxPool2d(kernel={:?}, stride={:?})",
            self.inner.kernel, self.inner.stride
        )
    }
}

// ---------------------------------------------------------------------------
// AvgPool2d
// ---------------------------------------------------------------------------

/// 2-D average pooling. Input is `[channels, H, W]`.
#[pyclass(name = "AvgPool2d")]
pub struct PyAvgPool2d {
    inner: AvgPool2d,
}

#[pymethods]
impl PyAvgPool2d {
    /// Creates an `AvgPool2d` layer. Raises ``ValueError`` if any kernel /
    /// stride dimension is 0.
    #[new]
    fn new(kernel: (usize, usize), stride: (usize, usize)) -> PyResult<Self> {
        let inner = AvgPool2d::new(kernel, stride).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn kernel(&self) -> (usize, usize) {
        self.inner.kernel
    }

    #[getter]
    fn stride(&self) -> (usize, usize) {
        self.inner.stride
    }

    /// Forward pass over a single-sample `[channels, height, width]`
    /// buffer. Returns ``(data, shape)``.
    fn forward(
        &self,
        input: Vec<f32>,
        channels: usize,
        height: usize,
        width: usize,
    ) -> PyResult<(Vec<f32>, Vec<usize>)> {
        let t = Tensor::from_data(input, vec![channels, height, width]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok((out.data().to_vec(), out.shape().to_vec()))
    }

    fn __repr__(&self) -> String {
        format!(
            "AvgPool2d(kernel={:?}, stride={:?})",
            self.inner.kernel, self.inner.stride
        )
    }
}

// ---------------------------------------------------------------------------
// GlobalAvgPool
// ---------------------------------------------------------------------------

/// Global average pooling: collapses `[channels, H, W]` to `[channels]`.
#[pyclass(name = "GlobalAvgPool")]
pub struct PyGlobalAvgPool {
    inner: GlobalAvgPool,
}

#[pymethods]
impl PyGlobalAvgPool {
    /// Creates a `GlobalAvgPool` layer (stateless; construction is
    /// infallible).
    #[new]
    fn new() -> Self {
        Self {
            inner: GlobalAvgPool::new(),
        }
    }

    /// Forward pass. ``input`` is a flat row-major `[channels, height,
    /// width]` buffer. Returns a buffer of length ``channels``.
    fn forward(
        &self,
        input: Vec<f32>,
        channels: usize,
        height: usize,
        width: usize,
    ) -> PyResult<Vec<f32>> {
        let t = Tensor::from_data(input, vec![channels, height, width]).map_err(neural_err)?;
        let out = self.inner.forward(&t).map_err(neural_err)?;
        Ok(out.data().to_vec())
    }

    fn __repr__(&self) -> String {
        "GlobalAvgPool()".to_string()
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the individual-layer classes into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyLinearLayer>()?;
    m.add_class::<PyConv2dLayer>()?;
    m.add_class::<PyBatchNorm1d>()?;
    m.add_class::<PyBatchNorm2d>()?;
    m.add_class::<PyMaxPool2d>()?;
    m.add_class::<PyAvgPool2d>()?;
    m.add_class::<PyGlobalAvgPool>()?;
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

    // ── LinearLayer ──────────────────────────────────────────────────────

    #[test]
    fn linear_zero_init_output_is_zero() {
        let layer = PyLinearLayer::new(4, 3).expect("construct");
        let out = layer.forward(vec![1.0; 4]).expect("forward");
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|&v| close(v, 0.0)));
    }

    #[test]
    fn linear_zero_features_is_value_error() {
        assert!(PyLinearLayer::new(0, 3).is_err());
    }

    #[test]
    fn linear_load_weights_identity() {
        let mut layer = PyLinearLayer::new(2, 2).expect("construct");
        layer
            .load_weights(vec![1.0, 0.0, 0.0, 1.0], vec![0.0, 0.0])
            .expect("load_weights");
        let out = layer.forward(vec![3.0, 5.0]).expect("forward");
        assert!(close(out[0], 3.0));
        assert!(close(out[1], 5.0));
    }

    #[test]
    fn linear_load_weights_wrong_len_is_value_error() {
        let mut layer = PyLinearLayer::new(2, 2).expect("construct");
        assert!(layer
            .load_weights(vec![1.0, 0.0, 0.0], vec![0.0, 0.0])
            .is_err());
    }

    #[test]
    fn linear_forward_wrong_len_is_value_error() {
        let layer = PyLinearLayer::new(4, 3).expect("construct");
        assert!(layer.forward(vec![1.0; 5]).is_err());
    }

    // ── Conv2dLayer ──────────────────────────────────────────────────────

    #[test]
    fn conv2d_output_shape() {
        let layer = PyConv2dLayer::new(3, 8, 3, 3, (1, 1), (1, 1)).expect("construct");
        let (data, shape) = layer
            .forward(vec![0.0_f32; 3 * 8 * 8], 8, 8)
            .expect("forward");
        assert_eq!(shape, vec![8, 8, 8]);
        assert_eq!(data.len(), 8 * 8 * 8);
    }

    #[test]
    fn conv2d_load_weights_wrong_len_is_value_error() {
        let mut layer = PyConv2dLayer::new(3, 8, 3, 3, (1, 1), (1, 1)).expect("construct");
        assert!(layer
            .load_weights(vec![0.0_f32; 5], vec![0.0_f32; 8])
            .is_err());
    }

    #[test]
    fn conv2d_getters() {
        let layer = PyConv2dLayer::new(3, 8, 3, 3, (2, 2), (1, 1)).expect("construct");
        assert_eq!(layer.in_channels(), 3);
        assert_eq!(layer.out_channels(), 8);
        assert_eq!(layer.stride(), (2, 2));
        assert_eq!(layer.padding(), (1, 1));
    }

    // ── BatchNorm1d ──────────────────────────────────────────────────────

    #[test]
    fn batchnorm1d_identity_is_noop() {
        let bn = PyBatchNorm1d::new(4).expect("construct");
        let out = bn.forward(vec![1.0, 2.0, 3.0, 4.0]).expect("forward");
        for (a, b) in out.iter().zip([1.0, 2.0, 3.0, 4.0].iter()) {
            assert!(close(*a, *b));
        }
    }

    #[test]
    fn batchnorm1d_load_params_wrong_len_is_value_error() {
        let mut bn = PyBatchNorm1d::new(4).expect("construct");
        assert!(bn
            .load_params(vec![1.0; 3], vec![0.0; 4], vec![0.0; 4], vec![1.0; 4], 1e-5)
            .is_err());
    }

    // ── BatchNorm2d ──────────────────────────────────────────────────────

    #[test]
    fn batchnorm2d_identity_is_noop() {
        let bn = PyBatchNorm2d::new(2).expect("construct");
        let input = vec![1.0_f32; 2 * 3 * 3];
        let out = bn.forward(input.clone(), 3, 3).expect("forward");
        for (a, b) in out.iter().zip(input.iter()) {
            assert!(close(*a, *b));
        }
    }

    // ── MaxPool2d ────────────────────────────────────────────────────────

    #[test]
    fn maxpool2d_output_shape_and_value() {
        let pool = PyMaxPool2d::new((2, 2), (2, 2)).expect("construct");
        let input = vec![1.0, 3.0, 2.0, 4.0]; // single 2x2 channel
        let (data, shape) = pool.forward(input, 1, 2, 2).expect("forward");
        assert_eq!(shape, vec![1, 1, 1]);
        assert!(close(data[0], 4.0));
    }

    #[test]
    fn maxpool2d_kernel_larger_than_input_is_value_error() {
        let pool = PyMaxPool2d::new((4, 4), (1, 1)).expect("construct");
        assert!(pool.forward(vec![0.0_f32; 1 * 2 * 2], 1, 2, 2).is_err());
    }

    // ── AvgPool2d ────────────────────────────────────────────────────────

    #[test]
    fn avgpool2d_output_value() {
        let pool = PyAvgPool2d::new((2, 2), (2, 2)).expect("construct");
        let input = vec![1.0, 3.0, 2.0, 4.0];
        let (data, shape) = pool.forward(input, 1, 2, 2).expect("forward");
        assert_eq!(shape, vec![1, 1, 1]);
        assert!(close(data[0], 2.5));
    }

    // ── GlobalAvgPool ────────────────────────────────────────────────────

    #[test]
    fn global_avg_pool_output_len() {
        let pool = PyGlobalAvgPool::new();
        let input = vec![2.0_f32; 3 * 4 * 4];
        let out = pool.forward(input, 3, 4, 4).expect("forward");
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|&v| close(v, 2.0)));
    }
}

//! `oximedia.neural` submodule — Python bindings for `oximedia-neural`.
//!
//! This file wraps the pure-Rust tensor type and the pre-built media
//! inference models (scene classifier, thumbnail ranker, 2x
//! super-resolution upscaler, and a HOG-like feature extractor) behind
//! PyO3 classes with real delegation to [`oximedia_neural`]. All weights
//! are zero-initialised at construction (this crate ships inference
//! scaffolding, not pre-trained weights or a training loop), so
//! `classify`/`score`/`extract`/`upscale_2x` are exact passthroughs to the
//! underlying Rust math rather than fabricated results.
//!
//! Inputs/outputs are plain flat `list[float]` buffers (row-major), matching
//! the underlying Rust API exactly (no numpy dependency required for this
//! surface) — a convention every sibling submodule below follows too.
//!
//! The rest of `oximedia.neural`'s surface lives in sibling files, each
//! registered into this same Python submodule by
//! [`register_submodule`]: [`crate::neural_attention_py`] (multi-head /
//! scaled-dot-product / flash attention, positional encodings),
//! [`crate::neural_recurrent_py`] (GRU / LSTM), [`crate::neural_quant_py`]
//! (INT`n` quantization), [`crate::neural_layers_py`] (`LinearLayer`,
//! `Conv2dLayer`, batch norm, pooling), [`crate::neural_graph_py`]
//! (`Sequential` / `ModelGraph` model builders driven by Python-callable
//! layers), [`crate::neural_detect_py`] (object/face detection, optical
//! flow), [`crate::neural_zoo_py`] (`MediaModelZoo` architecture
//! catalogue), and [`crate::neural_onnx_py`] (ONNX introspection +
//! execution, with an `onnx`-feature-gated full engine).

use oximedia_neural::{
    FeatureExtractor, NeuralError, SceneClass, SceneClassifier, SrUpscaler, Tensor, ThumbnailRanker,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

pub(crate) fn neural_err(err: NeuralError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

// ---------------------------------------------------------------------------
// Tensor
// ---------------------------------------------------------------------------

/// A dense n-dimensional f32 tensor (row-major / C-contiguous).
///
/// Mirrors [`oximedia_neural::Tensor`]. The higher-level media models below
/// accept plain flat `list[float]` buffers directly, so `Tensor` is only
/// needed when you want explicit shape validation.
#[pyclass(name = "Tensor")]
pub struct PyTensor {
    inner: Tensor,
}

#[pymethods]
impl PyTensor {
    /// Create a tensor from a flat row-major data buffer and a shape.
    ///
    /// Raises ``ValueError`` if ``len(data) != product(shape)`` or any
    /// dimension is 0.
    #[new]
    fn new(data: Vec<f32>, shape: Vec<usize>) -> PyResult<Self> {
        let inner = Tensor::from_data(data, shape).map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Create a zero-filled tensor with the given shape.
    #[staticmethod]
    fn zeros(shape: Vec<usize>) -> PyResult<Self> {
        let inner = Tensor::zeros(shape).map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Create a one-filled tensor with the given shape.
    #[staticmethod]
    fn ones(shape: Vec<usize>) -> PyResult<Self> {
        let inner = Tensor::ones(shape).map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Shape of each dimension.
    fn shape(&self) -> Vec<usize> {
        self.inner.shape().to_vec()
    }

    /// Number of dimensions (rank).
    fn ndim(&self) -> usize {
        self.inner.ndim()
    }

    /// Total number of elements.
    fn numel(&self) -> usize {
        self.inner.numel()
    }

    /// Return a copy of the flat row-major data buffer.
    fn to_list(&self) -> Vec<f32> {
        self.inner.data().to_vec()
    }

    fn __len__(&self) -> usize {
        self.inner.numel()
    }

    fn __repr__(&self) -> String {
        format!("Tensor(shape={:?})", self.inner.shape())
    }
}

// ---------------------------------------------------------------------------
// SceneClassifier
// ---------------------------------------------------------------------------

/// Lightweight two-layer MLP scene classifier (`128 -> 64 -> 10`, softmax).
///
/// Real delegation to [`oximedia_neural::SceneClassifier`]. Weights are
/// zero-initialised; loading pre-trained weights is not yet bound (see the
/// module TODOs).
#[pyclass(name = "SceneClassifier")]
pub struct PySceneClassifier {
    inner: SceneClassifier,
}

#[pymethods]
impl PySceneClassifier {
    /// Create a zero-initialised classifier.
    #[new]
    fn new() -> PyResult<Self> {
        let inner = SceneClassifier::new().map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Classify a 128-dimensional feature vector.
    ///
    /// Returns ``(class_index, confidence)``. Raises ``ValueError`` if
    /// ``features`` is empty or not exactly ``input_dim()`` elements long.
    fn classify(&self, features: Vec<f32>) -> PyResult<(usize, f32)> {
        self.inner.classify(&features).map_err(neural_err)
    }

    /// Expected input feature dimensionality (128).
    #[staticmethod]
    fn input_dim() -> usize {
        SceneClassifier::INPUT_DIM
    }

    /// Number of output classes (10).
    #[staticmethod]
    fn num_classes() -> usize {
        SceneClassifier::NUM_CLASSES
    }

    fn __repr__(&self) -> String {
        "SceneClassifier()".to_string()
    }
}

// ---------------------------------------------------------------------------
// ThumbnailRanker
// ---------------------------------------------------------------------------

/// Linear aesthetic-quality ranker for video thumbnails.
///
/// Real delegation to [`oximedia_neural::ThumbnailRanker`]; scores a
/// 64-dimensional feature vector in `[0, 1]`.
#[pyclass(name = "ThumbnailRanker")]
pub struct PyThumbnailRanker {
    inner: ThumbnailRanker,
}

#[pymethods]
impl PyThumbnailRanker {
    /// Create a zero-initialised ranker.
    #[new]
    fn new() -> PyResult<Self> {
        let inner = ThumbnailRanker::new().map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Score a 64-dimensional thumbnail feature vector in `[0, 1]`.
    ///
    /// Raises ``ValueError`` if ``thumbnail_features`` is empty or not
    /// exactly ``input_dim()`` elements long.
    fn score(&self, thumbnail_features: Vec<f32>) -> PyResult<f32> {
        self.inner.score(&thumbnail_features).map_err(neural_err)
    }

    /// Expected input feature dimensionality (64).
    #[staticmethod]
    fn input_dim() -> usize {
        ThumbnailRanker::INPUT_DIM
    }

    fn __repr__(&self) -> String {
        "ThumbnailRanker()".to_string()
    }
}

// ---------------------------------------------------------------------------
// SrUpscaler
// ---------------------------------------------------------------------------

/// Simplified 2x super-resolution upscaler (bilinear + 3-layer conv sharpen).
///
/// Real delegation to [`oximedia_neural::SrUpscaler`]. Single-channel
/// (luminance) only; apply per-channel for multi-channel images.
#[pyclass(name = "SrUpscaler")]
pub struct PySrUpscaler {
    inner: SrUpscaler,
}

#[pymethods]
impl PySrUpscaler {
    /// Create a zero-initialised upscaler.
    #[new]
    fn new() -> PyResult<Self> {
        let inner = SrUpscaler::new().map_err(neural_err)?;
        Ok(Self { inner })
    }

    /// Upscale a single-channel `height x width` row-major frame by 2x.
    ///
    /// Returns a row-major `(2*height) x (2*width)` flat list. Raises
    /// ``ValueError`` if ``frame`` is empty or its length does not match
    /// ``height * width``.
    fn upscale_2x(&self, frame: Vec<f32>, height: usize, width: usize) -> PyResult<Vec<f32>> {
        self.inner
            .upscale_2x(&frame, height, width)
            .map_err(neural_err)
    }

    fn __repr__(&self) -> String {
        "SrUpscaler()".to_string()
    }
}

// ---------------------------------------------------------------------------
// FeatureExtractor
// ---------------------------------------------------------------------------

/// HOG-like 128-dimensional feature extractor (4x4 grid x 8-bin gradient
/// histograms).
///
/// Real delegation to [`oximedia_neural::FeatureExtractor`]; useful for
/// visual embedding / retrieval / deduplication.
#[pyclass(name = "FeatureExtractor")]
pub struct PyFeatureExtractor {
    inner: FeatureExtractor,
}

#[pymethods]
impl PyFeatureExtractor {
    /// Create a feature extractor (stateless; construction is infallible).
    #[new]
    fn new() -> Self {
        Self {
            inner: FeatureExtractor::new(),
        }
    }

    /// Extract a 128-dim feature vector from a single-channel `width x
    /// height` row-major frame (values normalised to `[0, 1]`).
    ///
    /// Raises ``ValueError`` if ``frame`` is empty, its length does not
    /// match ``width * height``, or either dimension is smaller than 4.
    fn extract(&self, frame: Vec<f32>, width: usize, height: usize) -> PyResult<Vec<f32>> {
        self.inner
            .extract(&frame, width, height)
            .map_err(neural_err)
    }

    /// Output feature dimensionality (128).
    #[staticmethod]
    fn feature_dim() -> usize {
        FeatureExtractor::FEATURE_DIM
    }

    fn __repr__(&self) -> String {
        "FeatureExtractor()".to_string()
    }
}

// ---------------------------------------------------------------------------
// Standalone functions
// ---------------------------------------------------------------------------

/// Human-readable name for a `SceneClassifier` output class index
/// (e.g. `0 -> "Static"`, out-of-range -> `"Unknown"`).
#[pyfunction]
pub fn scene_class_name(idx: usize) -> String {
    format!("{:?}", SceneClass::from_index(idx))
}

// The neural module families below (ONNX loading/execution, attention,
// recurrent layers, quantization, declarative graph builders, individual
// `layers`, object/face/flow detectors, and the model zoo) are implemented
// as sibling submodules and registered in `register_submodule` below —
// see `neural_attention_py`, `neural_recurrent_py`, `neural_quant_py`,
// `neural_graph_py`, `neural_layers_py`, `neural_detect_py`, `neural_onnx_py`,
// and `neural_zoo_py`.

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the `oximedia.neural` submodule.
pub fn register_submodule(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "neural")?;

    m.add_class::<PyTensor>()?;
    m.add_class::<PySceneClassifier>()?;
    m.add_class::<PyThumbnailRanker>()?;
    m.add_class::<PySrUpscaler>()?;
    m.add_class::<PyFeatureExtractor>()?;
    m.add_function(wrap_pyfunction!(scene_class_name, &m)?)?;

    crate::neural_recurrent_py::register(&m)?;
    crate::neural_attention_py::register(&m)?;
    crate::neural_quant_py::register(&m)?;
    crate::neural_layers_py::register(&m)?;
    crate::neural_graph_py::register(&m)?;
    crate::neural_detect_py::register(&m)?;
    crate::neural_zoo_py::register(&m)?;
    crate::neural_onnx_py::register(&m)?;

    parent.add_submodule(&m)?;
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

    // ── Tensor ───────────────────────────────────────────────────────────

    #[test]
    fn tensor_from_data_round_trips() {
        let t = PyTensor::new(vec![1.0, 2.0, 3.0, 4.0], vec![2, 2]).expect("valid tensor");
        assert_eq!(t.shape(), vec![2, 2]);
        assert_eq!(t.ndim(), 2);
        assert_eq!(t.numel(), 4);
        assert_eq!(t.__len__(), 4);
        assert_eq!(t.to_list(), vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn tensor_rejects_shape_mismatch() {
        let result = PyTensor::new(vec![1.0, 2.0, 3.0], vec![2, 2]);
        assert!(result.is_err(), "3 elements cannot fill a [2,2] tensor");
    }

    #[test]
    fn tensor_rejects_zero_dimension() {
        let result = PyTensor::new(vec![], vec![0, 4]);
        assert!(result.is_err());
    }

    #[test]
    fn tensor_zeros_and_ones() {
        let z = PyTensor::zeros(vec![3]).expect("zeros");
        assert_eq!(z.to_list(), vec![0.0, 0.0, 0.0]);
        let o = PyTensor::ones(vec![2]).expect("ones");
        assert_eq!(o.to_list(), vec![1.0, 1.0]);
    }

    #[test]
    fn tensor_repr_contains_shape() {
        let t = PyTensor::new(vec![0.0; 6], vec![2, 3]).expect("valid");
        assert!(t.__repr__().contains("[2, 3]"));
    }

    // ── SceneClassifier ──────────────────────────────────────────────────

    #[test]
    fn scene_classifier_zero_weights_uniform_softmax() {
        let clf = PySceneClassifier::new().expect("construct");
        let features = vec![0.0_f32; PySceneClassifier::input_dim()];
        let (idx, conf) = clf.classify(features).expect("classify");
        assert!(
            close(conf, 0.1),
            "uniform softmax over 10 classes ~= 0.1, got {conf}"
        );
        assert!(idx < PySceneClassifier::num_classes());
    }

    #[test]
    fn scene_classifier_wrong_dim_is_value_error() {
        let clf = PySceneClassifier::new().expect("construct");
        let result = clf.classify(vec![0.0_f32; 4]);
        assert!(result.is_err());
    }

    #[test]
    fn scene_classifier_empty_input_is_value_error() {
        let clf = PySceneClassifier::new().expect("construct");
        assert!(clf.classify(vec![]).is_err());
    }

    // ── ThumbnailRanker ──────────────────────────────────────────────────

    #[test]
    fn thumbnail_ranker_zero_weights_score_half() {
        let ranker = PyThumbnailRanker::new().expect("construct");
        let features = vec![0.0_f32; PyThumbnailRanker::input_dim()];
        let score = ranker.score(features).expect("score");
        assert!(close(score, 0.5), "sigmoid(0) == 0.5, got {score}");
    }

    #[test]
    fn thumbnail_ranker_wrong_dim_is_value_error() {
        let ranker = PyThumbnailRanker::new().expect("construct");
        assert!(ranker.score(vec![0.0_f32; 8]).is_err());
    }

    // ── SrUpscaler ───────────────────────────────────────────────────────

    #[test]
    fn sr_upscaler_doubles_dimensions() {
        let upscaler = PySrUpscaler::new().expect("construct");
        let frame = vec![0.5_f32; 8 * 8];
        let out = upscaler.upscale_2x(frame, 8, 8).expect("upscale");
        assert_eq!(out.len(), 16 * 16);
    }

    #[test]
    fn sr_upscaler_empty_frame_is_value_error() {
        let upscaler = PySrUpscaler::new().expect("construct");
        assert!(upscaler.upscale_2x(vec![], 0, 0).is_err());
    }

    // ── FeatureExtractor ─────────────────────────────────────────────────

    #[test]
    fn feature_extractor_output_dim() {
        let extractor = PyFeatureExtractor::new();
        let frame = vec![0.5_f32; 32 * 32];
        let features = extractor.extract(frame, 32, 32).expect("extract");
        assert_eq!(features.len(), PyFeatureExtractor::feature_dim());
    }

    #[test]
    fn feature_extractor_too_small_is_value_error() {
        let extractor = PyFeatureExtractor::new();
        let frame = vec![0.0_f32; 3 * 3];
        assert!(extractor.extract(frame, 3, 3).is_err());
    }

    // ── scene_class_name ─────────────────────────────────────────────────

    #[test]
    fn scene_class_name_known_and_unknown() {
        assert_eq!(scene_class_name(0), "Static");
        assert_eq!(scene_class_name(99), "Unknown");
    }
}

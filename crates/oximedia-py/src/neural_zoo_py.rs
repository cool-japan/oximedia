//! `oximedia.neural` — pre-configured media-model architecture catalogue.
//!
//! Real delegation to [`oximedia_neural::model_zoo::MediaModelZoo`]. Each
//! catalogue entry is a **descriptive** architecture (layer shapes only,
//! no weights) — useful for allocating matching weight tensors or mapping
//! the architecture onto an inference backend; it does not itself run
//! inference (compose the individual layers from
//! [`crate::neural_layers_py`], or build a [`crate::neural_graph_py`]
//! `Sequential`, to actually execute one of these architectures).

use oximedia_neural::model_zoo::{LayerConfig, MediaModelZoo, ModelInfo};
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// LayerConfig
// ---------------------------------------------------------------------------

/// A purely descriptive layer specification (shape/type only, no weights).
///
/// Exactly one of the optional fields group is populated, matching
/// `kind`:
///
/// * `"conv2d"` — `in_channels`, `out_channels`, `kernel_size`, `stride`, `padding`
/// * `"linear"` — `in_features`, `out_features`
/// * `"batch_norm2d"` — `num_features`
/// * `"max_pool2d"` — `kernel_size`, `stride`
/// * `"dropout"` — `p`
/// * `"global_avg_pool"` / `"relu"` / `"sigmoid"` / `"softmax"` / `"flatten"` — none
#[pyclass(name = "LayerConfig")]
#[derive(Clone)]
pub struct PyLayerConfig {
    #[pyo3(get)]
    kind: String,
    #[pyo3(get)]
    in_channels: Option<usize>,
    #[pyo3(get)]
    out_channels: Option<usize>,
    #[pyo3(get)]
    kernel_size: Option<usize>,
    #[pyo3(get)]
    stride: Option<usize>,
    #[pyo3(get)]
    padding: Option<usize>,
    #[pyo3(get)]
    in_features: Option<usize>,
    #[pyo3(get)]
    out_features: Option<usize>,
    #[pyo3(get)]
    num_features: Option<usize>,
    #[pyo3(get)]
    p: Option<f32>,
}

impl PyLayerConfig {
    fn empty(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            in_channels: None,
            out_channels: None,
            kernel_size: None,
            stride: None,
            padding: None,
            in_features: None,
            out_features: None,
            num_features: None,
            p: None,
        }
    }
}

#[pymethods]
impl PyLayerConfig {
    fn __repr__(&self) -> String {
        format!(
            "LayerConfig(kind={:?}, in_channels={:?}, out_channels={:?}, kernel_size={:?}, \
             stride={:?}, padding={:?}, in_features={:?}, out_features={:?}, \
             num_features={:?}, p={:?})",
            self.kind,
            self.in_channels,
            self.out_channels,
            self.kernel_size,
            self.stride,
            self.padding,
            self.in_features,
            self.out_features,
            self.num_features,
            self.p
        )
    }
}

fn layer_config_to_py(cfg: &LayerConfig) -> PyLayerConfig {
    match cfg {
        LayerConfig::Conv2d {
            in_channels,
            out_channels,
            kernel_size,
            stride,
            padding,
        } => {
            let mut c = PyLayerConfig::empty("conv2d");
            c.in_channels = Some(*in_channels);
            c.out_channels = Some(*out_channels);
            c.kernel_size = Some(*kernel_size);
            c.stride = Some(*stride);
            c.padding = Some(*padding);
            c
        }
        LayerConfig::Linear {
            in_features,
            out_features,
        } => {
            let mut c = PyLayerConfig::empty("linear");
            c.in_features = Some(*in_features);
            c.out_features = Some(*out_features);
            c
        }
        LayerConfig::BatchNorm2d { num_features } => {
            let mut c = PyLayerConfig::empty("batch_norm2d");
            c.num_features = Some(*num_features);
            c
        }
        LayerConfig::MaxPool2d {
            kernel_size,
            stride,
        } => {
            let mut c = PyLayerConfig::empty("max_pool2d");
            c.kernel_size = Some(*kernel_size);
            c.stride = Some(*stride);
            c
        }
        LayerConfig::GlobalAvgPool => PyLayerConfig::empty("global_avg_pool"),
        LayerConfig::Relu => PyLayerConfig::empty("relu"),
        LayerConfig::Sigmoid => PyLayerConfig::empty("sigmoid"),
        LayerConfig::Softmax => PyLayerConfig::empty("softmax"),
        LayerConfig::Dropout { p } => {
            let mut c = PyLayerConfig::empty("dropout");
            c.p = Some(*p);
            c
        }
        LayerConfig::Flatten => PyLayerConfig::empty("flatten"),
    }
}

// ---------------------------------------------------------------------------
// ModelInfo
// ---------------------------------------------------------------------------

/// Metadata describing a model architecture from the [`PyMediaModelZoo`].
#[pyclass(name = "ModelInfo")]
#[derive(Clone)]
pub struct PyModelInfo {
    /// Short identifier usable with `MediaModelZoo.get_model`.
    #[pyo3(get)]
    pub name: String,
    /// Human-readable description of the model's purpose.
    #[pyo3(get)]
    pub description: String,
    /// Expected input tensor shape (excluding batch dimension).
    #[pyo3(get)]
    pub input_shape: Vec<usize>,
    /// Expected output tensor shape (excluding batch dimension).
    #[pyo3(get)]
    pub output_shape: Vec<usize>,
    /// Approximate total number of learnable parameters.
    #[pyo3(get)]
    pub parameter_count: usize,
}

#[pymethods]
impl PyModelInfo {
    fn __repr__(&self) -> String {
        format!(
            "ModelInfo(name={:?}, input_shape={:?}, output_shape={:?}, parameter_count={})",
            self.name, self.input_shape, self.output_shape, self.parameter_count
        )
    }
}

fn model_info_to_py(info: &ModelInfo) -> PyModelInfo {
    PyModelInfo {
        name: info.name.clone(),
        description: info.description.clone(),
        input_shape: info.input_shape.clone(),
        output_shape: info.output_shape.clone(),
        parameter_count: info.parameter_count,
    }
}

fn arch_to_py(pair: (ModelInfo, Vec<LayerConfig>)) -> (PyModelInfo, Vec<PyLayerConfig>) {
    let (info, layers) = pair;
    (
        model_info_to_py(&info),
        layers.iter().map(layer_config_to_py).collect(),
    )
}

// ---------------------------------------------------------------------------
// MediaModelZoo
// ---------------------------------------------------------------------------

/// A catalogue of pre-configured neural network architectures for media
/// tasks. Every method is a `@staticmethod` (mirroring the underlying
/// Rust unit-struct API), returning `(ModelInfo, list[LayerConfig])`.
#[pyclass(name = "MediaModelZoo")]
pub struct PyMediaModelZoo;

#[pymethods]
impl PyMediaModelZoo {
    /// 3-layer CNN for 10-class scene classification. Input `[3, 64, 64]`,
    /// output `[10]` (softmax).
    #[staticmethod]
    fn scene_classifier() -> (PyModelInfo, Vec<PyLayerConfig>) {
        arch_to_py(MediaModelZoo::scene_classifier())
    }

    /// MLP regressor predicting a Mean Opinion Score in `[1, 5]` from a
    /// 256-dim feature vector. Input `[256]`, output `[1]` (sigmoid).
    #[staticmethod]
    fn quality_estimator() -> (PyModelInfo, Vec<PyLayerConfig>) {
        arch_to_py(MediaModelZoo::quality_estimator())
    }

    /// Binary classifier detecting scene cuts from a pair of frame
    /// feature vectors. Input `[512]`, output `[2]` (softmax).
    #[staticmethod]
    fn shot_boundary_detector() -> (PyModelInfo, Vec<PyLayerConfig>) {
        arch_to_py(MediaModelZoo::shot_boundary_detector())
    }

    /// Lightweight MobileNet-style feature-extraction backbone. Input
    /// `[3, 224, 224]`, output `[1000]` (softmax).
    #[staticmethod]
    fn object_detector_backbone() -> (PyModelInfo, Vec<PyLayerConfig>) {
        arch_to_py(MediaModelZoo::object_detector_backbone())
    }

    /// Returns `ModelInfo` for every model in the zoo (without layer configs).
    #[staticmethod]
    fn list_models() -> Vec<PyModelInfo> {
        MediaModelZoo::list_models()
            .iter()
            .map(model_info_to_py)
            .collect()
    }

    /// Looks up a model by name (`"scene_classifier"`, `"quality_estimator"`,
    /// `"shot_boundary_detector"`, `"object_detector_backbone"`). Returns
    /// `None` if the name is not recognised.
    #[staticmethod]
    fn get_model(name: &str) -> Option<(PyModelInfo, Vec<PyLayerConfig>)> {
        MediaModelZoo::get_model(name).map(arch_to_py)
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the model-zoo classes into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyLayerConfig>()?;
    m.add_class::<PyModelInfo>()?;
    m.add_class::<PyMediaModelZoo>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_classifier_layer_count_and_shapes() {
        let (info, layers) = PyMediaModelZoo::scene_classifier();
        assert_eq!(layers.len(), 14);
        assert_eq!(info.input_shape, vec![3, 64, 64]);
        assert_eq!(info.output_shape, vec![10]);
        assert!(info.parameter_count > 0);
    }

    #[test]
    fn scene_classifier_first_layer_is_conv2d() {
        let (_, layers) = PyMediaModelZoo::scene_classifier();
        assert_eq!(layers[0].kind, "conv2d");
        assert_eq!(layers[0].in_channels, Some(3));
        assert_eq!(layers[0].out_channels, Some(16));
    }

    #[test]
    fn scene_classifier_last_layer_is_softmax() {
        let (_, layers) = PyMediaModelZoo::scene_classifier();
        let last = layers.last().expect("non-empty");
        assert_eq!(last.kind, "softmax");
    }

    #[test]
    fn quality_estimator_has_dropout_with_p() {
        let (_, layers) = PyMediaModelZoo::quality_estimator();
        let dropout = layers
            .iter()
            .find(|l| l.kind == "dropout")
            .expect("has dropout");
        assert!(dropout.p.is_some());
    }

    #[test]
    fn list_models_returns_four_unique_names() {
        let models = PyMediaModelZoo::list_models();
        assert_eq!(models.len(), 4);
        let mut names: Vec<String> = models.iter().map(|m| m.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn get_model_known_name() {
        let result = PyMediaModelZoo::get_model("scene_classifier");
        assert!(result.is_some());
        let (info, layers) = result.expect("some");
        assert_eq!(info.name, "scene_classifier");
        assert!(!layers.is_empty());
    }

    #[test]
    fn get_model_unknown_name_is_none() {
        assert!(PyMediaModelZoo::get_model("nonexistent").is_none());
    }

    #[test]
    fn layer_config_repr_contains_kind() {
        let (_, layers) = PyMediaModelZoo::scene_classifier();
        assert!(layers[0].__repr__().contains("conv2d"));
    }
}

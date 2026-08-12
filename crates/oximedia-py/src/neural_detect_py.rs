//! `oximedia.neural` — object detection, face detection, and optical flow.
//!
//! Real delegation to [`oximedia_neural::object_detector`],
//! [`oximedia_neural::face_detection`], and [`oximedia_neural::optical_flow`].
//!
//! `ObjectDetector` is **rule-based** (derives candidate detections from raw
//! image statistics — mean luminance, spatial variance, gradient energy —
//! not a trained ML model); this matches the honest documentation on the
//! underlying Rust type. `FaceDetector` and `OpticalFlowEstimator` are real
//! (zero-initialised) convolutional pipelines identical in spirit to the
//! four built-in media models in [`crate::neural_py`].

use oximedia_neural::face_detection::{self, DetectionBox, FaceDetector};
use oximedia_neural::object_detector::{
    BoundingBox, Detection, DetectionClass, NmsConfig, ObjectDetector,
};
use oximedia_neural::optical_flow::{self, FlowVector, OpticalFlowEstimator};
use oximedia_neural::Tensor;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::neural_py::neural_err;

// ---------------------------------------------------------------------------
// BoundingBox (object_detector)
// ---------------------------------------------------------------------------

/// An axis-aligned, top-left-origin bounding box normalised to `[0, 1]`.
#[pyclass(name = "BoundingBox")]
#[derive(Clone)]
pub struct PyBoundingBox {
    inner: BoundingBox,
}

#[pymethods]
impl PyBoundingBox {
    /// Creates a new box; all values are clamped to `[0, 1]` and negative
    /// extents are treated as zero (infallible, matching the Rust type).
    #[new]
    fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            inner: BoundingBox::new(x, y, width, height),
        }
    }

    #[getter]
    fn x(&self) -> f32 {
        self.inner.x
    }

    #[getter]
    fn y(&self) -> f32 {
        self.inner.y
    }

    #[getter]
    fn width(&self) -> f32 {
        self.inner.width
    }

    #[getter]
    fn height(&self) -> f32 {
        self.inner.height
    }

    /// Area in normalised units².
    fn area(&self) -> f32 {
        self.inner.area()
    }

    /// Intersection-over-Union with another box, in `[0, 1]`.
    fn iou(&self, other: &PyBoundingBox) -> f32 {
        self.inner.iou(&other.inner)
    }

    /// `True` if the box has positive area.
    fn is_valid(&self) -> bool {
        self.inner.is_valid()
    }

    fn __repr__(&self) -> String {
        format!(
            "BoundingBox(x={}, y={}, width={}, height={})",
            self.inner.x, self.inner.y, self.inner.width, self.inner.height
        )
    }
}

// ---------------------------------------------------------------------------
// ObjectDetector
// ---------------------------------------------------------------------------

/// Maps a `DetectionClass` label string back to a `DetectionClass`. Known
/// labels round-trip to their named variant; anything else becomes
/// `DetectionClass::Object(label)` — which itself round-trips losslessly
/// (`Object("dark_region").label() == "dark_region"`).
fn parse_detection_class(label: &str) -> DetectionClass {
    match label {
        "person" => DetectionClass::Person,
        "face" => DetectionClass::Face,
        "vehicle" => DetectionClass::Vehicle,
        "animal" => DetectionClass::Animal,
        "text" => DetectionClass::Text,
        "logo" => DetectionClass::Logo,
        other => DetectionClass::Object(other.to_string()),
    }
}

fn detection_to_tuple(d: Detection) -> (String, f32, PyBoundingBox) {
    let label = d.class.label().to_string();
    (label, d.confidence, PyBoundingBox { inner: d.bbox })
}

fn tuple_to_detection((label, confidence, bbox): (String, f32, PyBoundingBox)) -> Detection {
    Detection::new(parse_detection_class(&label), confidence, bbox.inner)
}

/// Rule-based object detector deriving candidate detections from raw image
/// statistics (mean luminance, spatial variance, gradient energy, hue
/// uniformity) — **not** a trained ML model. Useful for exercising a
/// detection pipeline (NMS, IoU filtering, coordinate normalisation)
/// against deterministic inputs.
#[pyclass(name = "ObjectDetector")]
pub struct PyObjectDetector {
    inner: ObjectDetector,
}

#[pymethods]
impl PyObjectDetector {
    /// Creates a detector with the given NMS thresholds (defaults `0.45` /
    /// `0.30`, matching the Rust type's `Default`).
    #[new]
    #[pyo3(signature = (iou_threshold=0.45, score_threshold=0.30))]
    fn new(iou_threshold: f32, score_threshold: f32) -> Self {
        let cfg = NmsConfig::new(iou_threshold, score_threshold);
        Self {
            inner: ObjectDetector::with_config(cfg),
        }
    }

    /// IoU threshold above which two boxes are considered duplicates.
    #[getter]
    fn iou_threshold(&self) -> f32 {
        self.inner.nms_config().iou_threshold
    }

    /// Minimum confidence required to keep a detection before NMS.
    #[getter]
    fn score_threshold(&self) -> f32 {
        self.inner.nms_config().score_threshold
    }

    /// Derives candidate detections (before NMS) from packed RGB
    /// (`width * height * 3` bytes) or single-channel (`width * height`
    /// bytes) image data.
    ///
    /// Returns a list of `(class_label, confidence, bbox)` tuples.
    ///
    /// Raises ``ValueError`` if ``image`` is non-empty but shorter than
    /// ``width * height`` bytes (the minimum for any valid interpretation
    /// the underlying detector accepts; this check exists at the Python
    /// FFI boundary because the underlying statistics pass indexes the
    /// buffer without its own length validation).
    fn detect(
        &self,
        image: Vec<u8>,
        width: u32,
        height: u32,
    ) -> PyResult<Vec<(String, f32, PyBoundingBox)>> {
        if !image.is_empty() {
            let min_len = u64::from(width) * u64::from(height);
            if (image.len() as u64) < min_len {
                return Err(PyValueError::new_err(format!(
                    "ObjectDetector.detect: image buffer has {} bytes, need at least \
                     width*height={} (or width*height*3 for packed RGB)",
                    image.len(),
                    min_len
                )));
            }
        }
        let detections = self.inner.detect(&image, width, height);
        Ok(detections.into_iter().map(detection_to_tuple).collect())
    }

    /// Applies greedy non-maximum suppression to a list of
    /// `(class_label, confidence, bbox)` tuples (e.g. from [`detect`]),
    /// returning the surviving subset.
    #[staticmethod]
    #[pyo3(signature = (detections, iou_threshold=0.45, score_threshold=0.30))]
    fn nms(
        detections: Vec<(String, f32, PyBoundingBox)>,
        iou_threshold: f32,
        score_threshold: f32,
    ) -> Vec<(String, f32, PyBoundingBox)> {
        let cfg = NmsConfig::new(iou_threshold, score_threshold);
        let rust_detections: Vec<Detection> =
            detections.into_iter().map(tuple_to_detection).collect();
        ObjectDetector::nms(rust_detections, &cfg)
            .into_iter()
            .map(detection_to_tuple)
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "ObjectDetector(iou_threshold={}, score_threshold={})",
            self.iou_threshold(),
            self.score_threshold()
        )
    }
}

// ---------------------------------------------------------------------------
// DetectionBox / FaceDetector
// ---------------------------------------------------------------------------

/// A detected face: centre-based bounding box plus confidence, normalised
/// to `[0, 1]` relative to the input image dimensions.
#[pyclass(name = "DetectionBox")]
#[derive(Clone)]
pub struct PyDetectionBox {
    inner: DetectionBox,
}

#[pymethods]
impl PyDetectionBox {
    #[getter]
    fn confidence(&self) -> f32 {
        self.inner.confidence
    }

    #[getter]
    fn cx(&self) -> f32 {
        self.inner.cx
    }

    #[getter]
    fn cy(&self) -> f32 {
        self.inner.cy
    }

    #[getter]
    fn w(&self) -> f32 {
        self.inner.w
    }

    #[getter]
    fn h(&self) -> f32 {
        self.inner.h
    }

    /// `True` if the box has non-degenerate area (`w > 0 and h > 0`).
    fn is_valid(&self) -> bool {
        self.inner.is_valid()
    }

    /// Intersection-over-Union with another box, in `[0, 1]`.
    fn iou(&self, other: &PyDetectionBox) -> f32 {
        self.inner.iou(&other.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "DetectionBox(confidence={}, cx={}, cy={}, w={}, h={})",
            self.inner.confidence, self.inner.cx, self.inner.cy, self.inner.w, self.inner.h
        )
    }
}

/// Applies greedy non-maximum suppression to a list of `DetectionBox`
/// values, sorted descending by confidence.
#[pyfunction]
fn face_nms(detections: Vec<PyDetectionBox>, iou_threshold: f32) -> Vec<PyDetectionBox> {
    let raw: Vec<DetectionBox> = detections.into_iter().map(|b| b.inner).collect();
    face_detection::nms(raw, iou_threshold)
        .into_iter()
        .map(|inner| PyDetectionBox { inner })
        .collect()
}

/// Lightweight convolutional face detector (3 stride-2 `Conv2d` stages +
/// global average pooling + a linear anchor head). Weights are
/// zero-initialised at construction; use [`load_weights`] to install
/// pre-trained parameters.
#[pyclass(name = "FaceDetector")]
pub struct PyFaceDetector {
    inner: FaceDetector,
}

#[pymethods]
impl PyFaceDetector {
    /// Creates a zero-initialised detector with `num_anchors` anchor boxes
    /// per forward pass. Raises ``ValueError`` if `num_anchors` is 0.
    #[new]
    fn new(num_anchors: usize) -> PyResult<Self> {
        let inner = FaceDetector::new(num_anchors).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn num_anchors(&self) -> usize {
        self.inner.num_anchors
    }

    /// Installs pre-trained weights for all four stages (three `Conv2d`
    /// stages + the linear detection head), in order:
    /// `stage1_w [16,3,3,3]`, `stage1_b [16]`, `stage2_w [32,16,3,3]`,
    /// `stage2_b [32]`, `stage3_w [64,32,3,3]`, `stage3_b [64]`,
    /// `head_w [num_anchors*5, 64]`, `head_b [num_anchors*5]`.
    ///
    /// Raises ``ValueError`` if any buffer length is wrong.
    #[allow(clippy::too_many_arguments)]
    fn load_weights(
        &mut self,
        stage1_w: Vec<f32>,
        stage1_b: Vec<f32>,
        stage2_w: Vec<f32>,
        stage2_b: Vec<f32>,
        stage3_w: Vec<f32>,
        stage3_b: Vec<f32>,
        head_w: Vec<f32>,
        head_b: Vec<f32>,
    ) -> PyResult<()> {
        // `FaceDetector::with_weights` consumes `self`; swap in a cheap
        // zero-initialised placeholder of the same shape while we rebuild.
        let placeholder = FaceDetector::new(self.inner.num_anchors).map_err(neural_err)?;
        let current = std::mem::replace(&mut self.inner, placeholder);
        self.inner = current
            .with_weights(
                stage1_w, stage1_b, stage2_w, stage2_b, stage3_w, stage3_b, head_w, head_b,
            )
            .map_err(neural_err)?;
        Ok(())
    }

    /// Runs face detection on a single `[3, height, width]` RGB image
    /// buffer (flat, row-major). Returns boxes with confidence >=
    /// `conf_threshold` after greedy NMS (IoU threshold `0.45`).
    fn forward(
        &self,
        input: Vec<f32>,
        height: usize,
        width: usize,
        conf_threshold: f32,
    ) -> PyResult<Vec<PyDetectionBox>> {
        let t = Tensor::from_data(input, vec![3, height, width]).map_err(neural_err)?;
        let boxes = self.inner.forward(&t, conf_threshold).map_err(neural_err)?;
        Ok(boxes
            .into_iter()
            .map(|inner| PyDetectionBox { inner })
            .collect())
    }

    /// Minimum supported input spatial size `(height, width)`.
    #[staticmethod]
    fn min_input_size() -> (usize, usize) {
        FaceDetector::min_input_size()
    }

    fn __repr__(&self) -> String {
        format!("FaceDetector(num_anchors={})", self.inner.num_anchors)
    }
}

// ---------------------------------------------------------------------------
// OpticalFlowEstimator
// ---------------------------------------------------------------------------

/// PWC-Net–inspired coarse-to-fine optical flow estimator: shared
/// stride-2 feature encoder, local correlation volume, and a small
/// convolutional decoder. Weights are zero-initialised at construction.
#[pyclass(name = "OpticalFlowEstimator")]
pub struct PyOpticalFlowEstimator {
    inner: OpticalFlowEstimator,
}

#[pymethods]
impl PyOpticalFlowEstimator {
    /// Creates a zero-initialised estimator. `d` is the correlation
    /// displacement radius (search window `(2*d+1) x (2*d+1)`); must be
    /// `>= 1`.
    #[new]
    fn new(d: usize) -> PyResult<Self> {
        let inner = OpticalFlowEstimator::new(d).map_err(neural_err)?;
        Ok(Self { inner })
    }

    #[getter]
    fn d(&self) -> usize {
        self.inner.d
    }

    /// Estimates dense optical flow between two `[3, height, width]` RGB
    /// frames (flat, row-major); both frames must share the same
    /// dimensions, at least `4x4`.
    ///
    /// Returns a flat row-major `[2, height, width]` buffer (`dx` then
    /// `dy` planes, in pixels).
    fn forward(
        &self,
        frame_a: Vec<f32>,
        frame_b: Vec<f32>,
        height: usize,
        width: usize,
    ) -> PyResult<Vec<f32>> {
        let a = Tensor::from_data(frame_a, vec![3, height, width]).map_err(neural_err)?;
        let b = Tensor::from_data(frame_b, vec![3, height, width]).map_err(neural_err)?;
        let flow = self.inner.forward(&a, &b).map_err(neural_err)?;
        Ok(flow.data().to_vec())
    }

    /// Averages a `[2, height, width]` flow field into a single dominant
    /// `(dx, dy)` motion vector (e.g. for global motion compensation).
    #[staticmethod]
    fn mean_flow(flow: Vec<f32>, height: usize, width: usize) -> PyResult<(f32, f32)> {
        let t = Tensor::from_data(flow, vec![2, height, width]).map_err(neural_err)?;
        let FlowVector { dx, dy } = OpticalFlowEstimator::mean_flow(&t).map_err(neural_err)?;
        Ok((dx, dy))
    }

    fn __repr__(&self) -> String {
        format!("OpticalFlowEstimator(d={})", self.inner.d)
    }
}

/// Computes the local cross-correlation volume between two `[channels,
/// height, width]` feature maps (flat, row-major, identical shapes).
///
/// Returns `(data, shape)` where `shape` is
/// `[(2*d+1)**2, height, width]`.
#[pyfunction]
fn correlation_volume(
    feat_a: Vec<f32>,
    feat_b: Vec<f32>,
    channels: usize,
    height: usize,
    width: usize,
    d: usize,
) -> PyResult<(Vec<f32>, Vec<usize>)> {
    let a = Tensor::from_data(feat_a, vec![channels, height, width]).map_err(neural_err)?;
    let b = Tensor::from_data(feat_b, vec![channels, height, width]).map_err(neural_err)?;
    let out = optical_flow::correlation_volume(&a, &b, d).map_err(neural_err)?;
    Ok((out.data().to_vec(), out.shape().to_vec()))
}

/// Bilinearly upsamples a `[channels, height, width]` buffer (flat,
/// row-major) by an integer `scale` factor (`align_corners=False`
/// convention, matching PyTorch).
///
/// Returns `(data, shape)` where `shape` is
/// `[channels, height*scale, width*scale]`.
#[pyfunction]
fn bilinear_upsample(
    input: Vec<f32>,
    channels: usize,
    height: usize,
    width: usize,
    scale: usize,
) -> PyResult<(Vec<f32>, Vec<usize>)> {
    let t = Tensor::from_data(input, vec![channels, height, width]).map_err(neural_err)?;
    let out = optical_flow::bilinear_upsample(&t, scale).map_err(neural_err)?;
    Ok((out.data().to_vec(), out.shape().to_vec()))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Registers the detection classes/functions into the `oximedia.neural` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBoundingBox>()?;
    m.add_class::<PyObjectDetector>()?;
    m.add_class::<PyDetectionBox>()?;
    m.add_class::<PyFaceDetector>()?;
    m.add_class::<PyOpticalFlowEstimator>()?;
    m.add_function(wrap_pyfunction!(face_nms, m)?)?;
    m.add_function(wrap_pyfunction!(correlation_volume, m)?)?;
    m.add_function(wrap_pyfunction!(bilinear_upsample, m)?)?;
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

    // ── BoundingBox ──────────────────────────────────────────────────────

    #[test]
    fn bbox_area_and_iou() {
        let a = PyBoundingBox::new(0.0, 0.0, 0.5, 0.5);
        let b = PyBoundingBox::new(0.0, 0.0, 0.5, 0.5);
        assert!(close(a.area(), 0.25));
        assert!(close(a.iou(&b), 1.0));
        assert!(a.is_valid());
    }

    #[test]
    fn bbox_clamps_out_of_range() {
        let b = PyBoundingBox::new(-1.0, -1.0, 3.0, 3.0);
        assert!(b.x() >= 0.0 && b.y() >= 0.0);
        assert!(b.x() + b.width() <= 1.0 + 1e-6);
    }

    // ── ObjectDetector ───────────────────────────────────────────────────

    #[test]
    fn object_detector_bright_image_produces_logo() {
        let detector = PyObjectDetector::new(0.45, 0.30);
        let pixels = vec![255u8; 16 * 16 * 3];
        let detections = detector.detect(pixels, 16, 16).expect("detect");
        assert!(detections.iter().any(|(label, _, _)| label == "logo"));
    }

    #[test]
    fn object_detector_empty_image_returns_empty() {
        let detector = PyObjectDetector::new(0.45, 0.30);
        let detections = detector.detect(vec![], 0, 0).expect("detect");
        assert!(detections.is_empty());
    }

    #[test]
    fn object_detector_too_short_buffer_is_value_error() {
        let detector = PyObjectDetector::new(0.45, 0.30);
        // width*height = 10000 but only 10 bytes supplied — would otherwise
        // panic deep inside the Rust gradient-energy loop.
        assert!(detector.detect(vec![1u8; 10], 100, 100).is_err());
    }

    #[test]
    fn object_detector_nms_dedupes_overlapping() {
        let a = (
            "person".to_string(),
            0.9,
            PyBoundingBox::new(0.1, 0.1, 0.5, 0.5),
        );
        let b = (
            "person".to_string(),
            0.85,
            PyBoundingBox::new(0.11, 0.11, 0.49, 0.49),
        );
        let kept = PyObjectDetector::nms(vec![a, b], 0.45, 0.3);
        assert_eq!(kept.len(), 1);
        assert!(close(kept[0].1, 0.9));
    }

    #[test]
    fn object_detector_custom_label_roundtrips() {
        let d = (
            "dark_region".to_string(),
            0.5,
            PyBoundingBox::new(0.0, 0.0, 0.5, 0.5),
        );
        let kept = PyObjectDetector::nms(vec![d], 0.45, 0.0);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, "dark_region");
    }

    #[test]
    fn object_detector_getters() {
        let detector = PyObjectDetector::new(0.5, 0.2);
        assert!(close(detector.iou_threshold(), 0.5));
        assert!(close(detector.score_threshold(), 0.2));
    }

    // ── FaceDetector ─────────────────────────────────────────────────────

    #[test]
    fn face_detector_zero_weights_within_bounds() {
        let detector = PyFaceDetector::new(4).expect("construct");
        let img = vec![0.0_f32; 3 * 32 * 32];
        let boxes = detector.forward(img, 32, 32, 0.5).expect("forward");
        assert!(boxes.len() <= 4);
    }

    #[test]
    fn face_detector_zero_anchors_is_value_error() {
        assert!(PyFaceDetector::new(0).is_err());
    }

    #[test]
    fn face_detector_wrong_channels_is_value_error() {
        let detector = PyFaceDetector::new(2).expect("construct");
        let img = vec![0.0_f32; 1 * 32 * 32]; // wrong channel count
        assert!(detector.forward(img, 32, 32, 0.5).is_err());
    }

    #[test]
    fn face_detector_load_weights_wrong_len_is_value_error() {
        let mut detector = PyFaceDetector::new(2).expect("construct");
        assert!(detector
            .load_weights(
                vec![0.0_f32; 3], // wrong length
                vec![0.0_f32; 16],
                vec![0.0_f32; 16 * 32 * 3 * 3],
                vec![0.0_f32; 32],
                vec![0.0_f32; 32 * 64 * 3 * 3],
                vec![0.0_f32; 64],
                vec![0.0_f32; 64 * 10],
                vec![0.0_f32; 10],
            )
            .is_err());
    }

    #[test]
    fn face_detector_min_input_size() {
        assert_eq!(PyFaceDetector::min_input_size(), (8, 8));
    }

    #[test]
    fn face_nms_keeps_highest_confidence() {
        let a = PyDetectionBox {
            inner: DetectionBox {
                confidence: 0.9,
                cx: 0.5,
                cy: 0.5,
                w: 0.4,
                h: 0.4,
            },
        };
        let b = PyDetectionBox {
            inner: DetectionBox {
                confidence: 0.7,
                cx: 0.5,
                cy: 0.5,
                w: 0.4,
                h: 0.4,
            },
        };
        let kept = face_nms(vec![a, b], 0.5);
        assert_eq!(kept.len(), 1);
        assert!(close(kept[0].confidence(), 0.9));
    }

    // ── OpticalFlowEstimator ─────────────────────────────────────────────

    #[test]
    fn optical_flow_output_len() {
        let est = PyOpticalFlowEstimator::new(2).expect("construct");
        let a = vec![0.0_f32; 3 * 16 * 16];
        let b = vec![0.0_f32; 3 * 16 * 16];
        let flow = est.forward(a, b, 16, 16).expect("forward");
        assert_eq!(flow.len(), 2 * 16 * 16);
    }

    #[test]
    fn optical_flow_zero_d_is_value_error() {
        assert!(PyOpticalFlowEstimator::new(0).is_err());
    }

    #[test]
    fn optical_flow_mismatched_frames_is_value_error() {
        let est = PyOpticalFlowEstimator::new(1).expect("construct");
        let a = vec![0.0_f32; 3 * 16 * 16];
        let b = vec![0.0_f32; 3 * 8 * 8];
        assert!(est.forward(a, b, 16, 16).is_err());
    }

    #[test]
    fn optical_flow_mean_flow_zero_field() {
        let flow = vec![0.0_f32; 2 * 8 * 8];
        let (dx, dy) = PyOpticalFlowEstimator::mean_flow(flow, 8, 8).expect("mean_flow");
        assert_eq!(dx, 0.0);
        assert_eq!(dy, 0.0);
    }

    #[test]
    fn correlation_volume_shape() {
        let a = vec![0.0_f32; 8 * 4 * 4];
        let b = vec![0.0_f32; 8 * 4 * 4];
        let (data, shape) = correlation_volume(a, b, 8, 4, 4, 2).expect("correlation_volume");
        assert_eq!(shape, vec![25, 4, 4]);
        assert_eq!(data.len(), 25 * 4 * 4);
    }

    #[test]
    fn bilinear_upsample_shape() {
        let t = vec![0.0_f32; 2 * 4 * 4];
        let (data, shape) = bilinear_upsample(t, 2, 4, 4, 4).expect("bilinear_upsample");
        assert_eq!(shape, vec![2, 16, 16]);
        assert_eq!(data.len(), 2 * 16 * 16);
    }
}

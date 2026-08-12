//! Video scaling filter.
//!
//! This filter rescales video frames to a target resolution using various
//! resampling algorithms including Lanczos, Bicubic, Bilinear, and Nearest Neighbor.

#![forbid(unsafe_code)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::similar_names)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::unused_self)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::no_effect_underscore_binding)]
#![allow(clippy::unreadable_literal)]
#![allow(dead_code)]

use std::f64::consts::PI;

use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;

use crate::error::{GraphError, GraphResult};
use crate::frame::FilterFrame;
use crate::node::{Node, NodeId, NodeState, NodeType};
use crate::port::{InputPort, OutputPort, PortFormat, PortId, PortType, VideoPortFormat};
use oximedia_codec::{Plane, VideoFrame};

/// Minimum number of samples a resampling pass must produce before it is handed to rayon.
///
/// Row parallelism is bit-exact (rows never read each other's output), so this threshold is a
/// pure scheduling decision: below roughly a 256x256 pass the fork/join cost dominates the work.
/// Thumbnail-sized planes therefore stay on the calling thread while a 1080x1920 plane -- ~2.1 M
/// samples -- is split across the pool.
const PARALLEL_SAMPLE_THRESHOLD: usize = 64 * 1024;

/// Scaling algorithm for image resampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScaleAlgorithm {
    /// Nearest neighbor - fastest, lowest quality, good for pixel art.
    Nearest,
    /// Bilinear interpolation - fast, moderate quality.
    Bilinear,
    /// Bicubic interpolation using Mitchell-Netravali coefficients.
    #[default]
    Bicubic,
    /// Bicubic interpolation using Catmull-Rom spline.
    CatmullRom,
    /// Lanczos-2 - high quality, 2-tap sinc window.
    Lanczos2,
    /// Lanczos-3 - higher quality, 3-tap sinc window.
    Lanczos3,
    /// Lanczos-4 - highest quality, 4-tap sinc window.
    Lanczos4,
}

impl ScaleAlgorithm {
    /// Get the filter support (radius in source pixels).
    #[must_use]
    pub fn support(&self) -> f64 {
        match self {
            Self::Nearest => 0.5,
            Self::Bilinear => 1.0,
            Self::Bicubic | Self::CatmullRom => 2.0,
            Self::Lanczos2 => 2.0,
            Self::Lanczos3 => 3.0,
            Self::Lanczos4 => 4.0,
        }
    }

    /// Calculate the kernel value at position x.
    #[must_use]
    pub fn kernel(&self, x: f64) -> f64 {
        let x = x.abs();
        match self {
            Self::Nearest => {
                if x < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Bilinear => bilinear_kernel(x),
            Self::Bicubic => mitchell_netravali_kernel(x),
            Self::CatmullRom => catmull_rom_kernel(x),
            Self::Lanczos2 => lanczos_kernel(x, 2.0),
            Self::Lanczos3 => lanczos_kernel(x, 3.0),
            Self::Lanczos4 => lanczos_kernel(x, 4.0),
        }
    }
}

/// Bilinear interpolation kernel.
fn bilinear_kernel(x: f64) -> f64 {
    if x < 1.0 {
        1.0 - x
    } else {
        0.0
    }
}

/// Mitchell-Netravali bicubic kernel with B=1/3, C=1/3.
fn mitchell_netravali_kernel(x: f64) -> f64 {
    const B: f64 = 1.0 / 3.0;
    const C: f64 = 1.0 / 3.0;

    let x2 = x * x;
    let x3 = x2 * x;

    if x < 1.0 {
        ((12.0 - 9.0 * B - 6.0 * C) * x3 + (-18.0 + 12.0 * B + 6.0 * C) * x2 + (6.0 - 2.0 * B))
            / 6.0
    } else if x < 2.0 {
        ((-B - 6.0 * C) * x3
            + (6.0 * B + 30.0 * C) * x2
            + (-12.0 * B - 48.0 * C) * x
            + (8.0 * B + 24.0 * C))
            / 6.0
    } else {
        0.0
    }
}

/// Catmull-Rom bicubic kernel (B=0, C=0.5).
fn catmull_rom_kernel(x: f64) -> f64 {
    let x2 = x * x;
    let x3 = x2 * x;

    if x < 1.0 {
        1.5 * x3 - 2.5 * x2 + 1.0
    } else if x < 2.0 {
        -0.5 * x3 + 2.5 * x2 - 4.0 * x + 2.0
    } else {
        0.0
    }
}

/// Lanczos windowed sinc kernel.
fn lanczos_kernel(x: f64, a: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else if x < a {
        sinc(x) * sinc(x / a)
    } else {
        0.0
    }
}

/// Normalized sinc function.
fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let pix = PI * x;
        pix.sin() / pix
    }
}

/// Configuration for the scale filter.
#[derive(Clone, Debug)]
pub struct ScaleConfig {
    /// Target width.
    pub width: u32,
    /// Target height.
    pub height: u32,
    /// Scaling algorithm.
    pub algorithm: ScaleAlgorithm,
    /// Enable anti-aliasing for downscaling.
    pub antialias: bool,
    /// Preserve aspect ratio (letterbox/pillarbox as needed).
    pub preserve_aspect: bool,
}

impl ScaleConfig {
    /// Create a new scale configuration.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            algorithm: ScaleAlgorithm::default(),
            antialias: true,
            preserve_aspect: false,
        }
    }

    /// Set the scaling algorithm.
    #[must_use]
    pub fn with_algorithm(mut self, algorithm: ScaleAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    /// Enable or disable anti-aliasing.
    #[must_use]
    pub fn with_antialias(mut self, enabled: bool) -> Self {
        self.antialias = enabled;
        self
    }

    /// Enable or disable aspect ratio preservation.
    #[must_use]
    pub fn with_preserve_aspect(mut self, enabled: bool) -> Self {
        self.preserve_aspect = enabled;
        self
    }
}

/// Video scaling filter.
///
/// Rescales video frames to a target resolution using configurable
/// resampling algorithms.
///
/// # Example
///
/// ```ignore
/// use oximedia_graph::filters::video::{ScaleFilter, ScaleConfig, ScaleAlgorithm};
/// use oximedia_graph::node::NodeId;
///
/// let config = ScaleConfig::new(1280, 720)
///     .with_algorithm(ScaleAlgorithm::Lanczos3)
///     .with_antialias(true);
///
/// let filter = ScaleFilter::new(NodeId(0), "scale", config);
/// ```
pub struct ScaleFilter {
    id: NodeId,
    name: String,
    state: NodeState,
    inputs: Vec<InputPort>,
    outputs: Vec<OutputPort>,
    config: ScaleConfig,
    /// Cached coefficient tables for planes with the frame's full (luma) geometry.
    luma_coefficients: Option<PlaneCoefficients>,
    /// Cached coefficient tables for the subsampled (chroma) plane geometry.
    ///
    /// Both chroma planes of a 4:2:0/4:2:2 frame share one geometry, so a single entry serves
    /// them for the whole lifetime of the filter instead of being rebuilt per plane per frame.
    chroma_coefficients: Option<PlaneCoefficients>,
}

/// Identity of a coefficient table pair: everything the weights depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CoefficientKey {
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
    algorithm: ScaleAlgorithm,
    antialias: bool,
}

/// Horizontal + vertical coefficient tables for one plane geometry.
#[derive(Clone, Debug)]
struct PlaneCoefficients {
    /// Geometry these tables were built for.
    key: CoefficientKey,
    /// Weights mapping source columns to destination columns.
    horizontal: CoefficientTable,
    /// Weights mapping source rows to destination rows.
    vertical: CoefficientTable,
}

impl PlaneCoefficients {
    /// Build both passes for a geometry.
    fn build(key: CoefficientKey) -> Self {
        Self {
            key,
            horizontal: CoefficientTable::build(
                key.src_width,
                key.dst_width,
                key.algorithm,
                key.antialias,
            ),
            vertical: CoefficientTable::build(
                key.src_height,
                key.dst_height,
                key.algorithm,
                key.antialias,
            ),
        }
    }
}

/// Tap window for one output position.
///
/// Deliberately three `u32`s (12 bytes) rather than `usize`s: the whole table stays small enough
/// to sit in L2 next to the weights even for 4K geometries.
#[derive(Clone, Copy, Debug)]
struct TapSpan {
    /// First source sample the window reads.
    src_start: u32,
    /// Index of the first weight inside [`CoefficientTable::weights`].
    weight_offset: u32,
    /// Number of taps.
    len: u32,
}

/// Flat 1-D resampling coefficients.
///
/// All weights live in one contiguous allocation; `spans` slices into it. This replaces a
/// `Vec<Vec<f64>>`, which cost a pointer chase and a separate cache line per output position and
/// forced per-tap bounds checks in the innermost loop.
#[derive(Clone, Debug)]
struct CoefficientTable {
    /// One entry per output position.
    spans: Vec<TapSpan>,
    /// Normalised weights, packed back to back in `spans` order.
    weights: Vec<f64>,
}

impl CoefficientTable {
    /// Compute normalised 1-D filter coefficients for `src_size -> dst_size`.
    fn build(src_size: u32, dst_size: u32, algorithm: ScaleAlgorithm, antialias: bool) -> Self {
        // `Nearest` is a point sampler, not a convolution, and the generic windowing below cannot
        // express it. Its support is exactly 0.5, so an output centre that lands on a source-pixel
        // boundary leaves every candidate tap at distance exactly 0.5 -- outside the `x < 0.5`
        // kernel -- and the window comes out all zero, which the (skipped) normalisation leaves
        // alone and the resampler turns into a black output sample. An exact 2x downscale puts
        // *every* centre on a boundary. Widening the kernel by an epsilon would not fix it either:
        // on the antialias path the support is multiplied by the downscale factor, which turns
        // `Nearest` into a box average over the whole footprint. Build the taps directly instead.
        if matches!(algorithm, ScaleAlgorithm::Nearest) {
            return Self::build_nearest(src_size, dst_size);
        }

        let dst_len = dst_size as usize;
        let scale = if dst_size == 0 {
            1.0
        } else {
            src_size as f64 / dst_size as f64
        };

        // For downscaling with antialiasing, expand the filter support
        let filter_scale = if antialias && scale > 1.0 { scale } else { 1.0 };

        let support = algorithm.support() * filter_scale;

        let mut spans = Vec::with_capacity(dst_len);
        let taps_estimate = (2.0 * support).ceil() as usize + 2;
        let mut weights = Vec::with_capacity(dst_len.saturating_mul(taps_estimate));

        for dst_pos in 0..dst_size {
            let center = (dst_pos as f64 + 0.5) * scale - 0.5;
            let start = ((center - support).floor() as i64).max(0) as usize;
            let end = ((center + support).ceil() as i64).min(src_size as i64) as usize;
            let end = end.max(start);

            let weight_offset = weights.len();
            let mut sum = 0.0;

            for src_pos in start..end {
                let distance = (src_pos as f64 - center) / filter_scale;
                let weight = algorithm.kernel(distance);
                weights.push(weight);
                sum += weight;
            }

            // Normalize weights
            if sum != 0.0 {
                if let Some(window) = weights.get_mut(weight_offset..) {
                    for w in window {
                        *w /= sum;
                    }
                }
            }

            spans.push(TapSpan {
                src_start: start as u32,
                weight_offset: weight_offset as u32,
                len: (end - start) as u32,
            });
        }

        Self { spans, weights }
    }

    /// Point-sampling taps for [`ScaleAlgorithm::Nearest`]: one source sample, weight 1.0.
    ///
    /// The sampled index is `floor((dst_pos + 0.5) * src_size / dst_size)` clamped into the
    /// source -- the same formula [`NearestNeighborScaler`] uses -- so the generic two-pass
    /// resampler and the dedicated scaler produce identical bytes. `antialias` is deliberately not
    /// a parameter: widening the footprint of a point sampler is what turned it into a box filter.
    fn build_nearest(src_size: u32, dst_size: u32) -> Self {
        let dst_len = dst_size as usize;

        let Some(last_index) = src_size.checked_sub(1) else {
            // Nothing to sample. Emit empty windows so the resampler reads the missing samples as
            // 0, exactly as the generic path does for a zero-width/height source.
            return Self {
                spans: vec![
                    TapSpan {
                        src_start: 0,
                        weight_offset: 0,
                        len: 0,
                    };
                    dst_len
                ],
                weights: Vec::new(),
            };
        };

        let scale = if dst_size == 0 {
            1.0
        } else {
            src_size as f64 / dst_size as f64
        };

        let mut spans = Vec::with_capacity(dst_len);
        let mut weights = Vec::with_capacity(dst_len);

        for dst_pos in 0..dst_size {
            // `f64 -> u32` saturates at 0 for negative inputs, so this is the full clamp into
            // `[0, src_size - 1]`; the product is non-negative anyway.
            let index = (((dst_pos as f64 + 0.5) * scale).floor() as u32).min(last_index);
            spans.push(TapSpan {
                src_start: index,
                weight_offset: weights.len() as u32,
                len: 1,
            });
            weights.push(1.0);
        }

        Self { spans, weights }
    }

    /// Weight slice for one output position (empty when the span is out of range).
    fn window(&self, span: TapSpan) -> &[f64] {
        let offset = span.weight_offset as usize;
        self.weights
            .get(offset..offset + span.len as usize)
            .unwrap_or(&[])
    }
}

/// Horizontal pass for a single source row.
///
/// Writes `dst_width` f64 samples. The accumulation order per output sample is unchanged from the
/// original scalar implementation (taps ascending, starting from `0.0`), so results are
/// bit-identical.
fn resample_row_horizontal(
    table: &CoefficientTable,
    src: &Plane,
    y: usize,
    src_width: usize,
    scratch: &mut Vec<u8>,
    out: &mut [f64],
) {
    let row = src.row(y);
    let source: &[u8] = if row.len() >= src_width {
        // Fast path: the row covers the full plane width, so every tap window is in bounds and
        // the inner loop is a straight slice walk.
        row.get(..src_width).unwrap_or(row)
    } else {
        // `Plane::row` yields a short (or empty) slice when the backing buffer is truncated. The
        // reference implementation read those missing samples as `0`, so pad explicitly instead
        // of re-checking every tap.
        scratch.clear();
        scratch.extend_from_slice(row);
        scratch.resize(src_width, 0);
        scratch.as_slice()
    };

    for (out_value, &span) in out.iter_mut().zip(table.spans.iter()) {
        let start = span.src_start as usize;
        let pixels = source
            .get(start..start + span.len as usize)
            .unwrap_or(&[][..]);
        let weights = table.window(span);

        let mut sum = 0.0f64;
        for (&pixel, &weight) in pixels.iter().zip(weights.iter()) {
            sum += pixel as f64 * weight;
        }
        *out_value = sum;
    }
}

/// Vertical pass for a single destination row.
///
/// Accumulates whole intermediate rows into `acc` instead of walking a column per output pixel.
/// For a fixed output pixel the taps are still applied in ascending order onto an accumulator
/// that starts at `0.0`, so the result is bit-identical to the per-pixel loop -- but the memory
/// access pattern becomes sequential instead of striding `dst_width * 8` bytes per tap.
fn resample_row_vertical(
    table: &CoefficientTable,
    intermediate: &[f64],
    dst_width: usize,
    y: usize,
    acc: &mut [f64],
    out: &mut [u8],
) {
    acc.fill(0.0);

    if let Some(&span) = table.spans.get(y) {
        let start = span.src_start as usize;
        for (tap, &weight) in table.window(span).iter().enumerate() {
            let row_start = (start + tap) * dst_width;
            let row = intermediate
                .get(row_start..row_start + dst_width)
                .unwrap_or(&[][..]);
            for (slot, &value) in acc.iter_mut().zip(row.iter()) {
                *slot += value * weight;
            }
        }
    }

    for (byte, &value) in out.iter_mut().zip(acc.iter()) {
        *byte = value.round().clamp(0.0, 255.0) as u8;
    }
}

/// Execution policy for the two resampling passes.
///
/// Kept explicit (rather than deciding inline) so tests can force both settings and assert that
/// they produce identical bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PassParallelism {
    /// Run the horizontal pass across the rayon pool.
    horizontal: bool,
    /// Run the vertical pass across the rayon pool.
    vertical: bool,
}

impl PassParallelism {
    /// Both passes on the calling thread.
    const SERIAL: Self = Self {
        horizontal: false,
        vertical: false,
    };

    /// Both passes on the rayon pool.
    const PARALLEL: Self = Self {
        horizontal: true,
        vertical: true,
    };

    /// Pick per pass based on how many samples that pass produces.
    fn automatic(dst_width: usize, src_height: usize, dst_height: usize) -> Self {
        Self {
            horizontal: dst_width.saturating_mul(src_height) >= PARALLEL_SAMPLE_THRESHOLD,
            vertical: dst_width.saturating_mul(dst_height) >= PARALLEL_SAMPLE_THRESHOLD,
        }
    }
}

/// Resample one plane with a precomputed coefficient pair.
fn scale_plane_with(
    coefficients: &PlaneCoefficients,
    src: &Plane,
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Plane {
    let parallelism =
        PassParallelism::automatic(dst_width as usize, src_height as usize, dst_height as usize);
    resample_plane(
        coefficients,
        src,
        src_width,
        src_height,
        dst_width,
        dst_height,
        parallelism,
    )
}

/// Resample one plane with an explicit execution policy.
fn resample_plane(
    coefficients: &PlaneCoefficients,
    src: &Plane,
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
    parallelism: PassParallelism,
) -> Plane {
    let src_w = src_width as usize;
    let src_h = src_height as usize;
    let dst_w = dst_width as usize;
    let dst_h = dst_height as usize;

    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Plane::new(vec![0u8; dst_w * dst_h], dst_w);
    }

    // Horizontal pass: `src_h` independent rows of `dst_w` f64 samples.
    let mut intermediate = vec![0.0f64; dst_w * src_h];
    let horizontal = &coefficients.horizontal;

    if parallelism.horizontal {
        intermediate
            .par_chunks_mut(dst_w)
            .enumerate()
            .for_each_init(Vec::<u8>::new, |scratch, (y, out_row)| {
                resample_row_horizontal(horizontal, src, y, src_w, scratch, out_row);
            });
    } else {
        let mut scratch = Vec::<u8>::new();
        for (y, out_row) in intermediate.chunks_mut(dst_w).enumerate() {
            resample_row_horizontal(horizontal, src, y, src_w, &mut scratch, out_row);
        }
    }

    // Vertical pass: `dst_h` independent rows, each reading whole intermediate rows.
    let mut dst_data = vec![0u8; dst_w * dst_h];
    let vertical = &coefficients.vertical;
    let intermediate: &[f64] = &intermediate;

    if parallelism.vertical {
        dst_data.par_chunks_mut(dst_w).enumerate().for_each_init(
            || vec![0.0f64; dst_w],
            |acc, (y, out_row)| {
                resample_row_vertical(vertical, intermediate, dst_w, y, acc, out_row);
            },
        );
    } else {
        let mut acc = vec![0.0f64; dst_w];
        for (y, out_row) in dst_data.chunks_mut(dst_w).enumerate() {
            resample_row_vertical(vertical, intermediate, dst_w, y, &mut acc, out_row);
        }
    }

    Plane::new(dst_data, dst_w)
}

impl ScaleFilter {
    /// Create a new scale filter.
    #[must_use]
    pub fn new(id: NodeId, name: impl Into<String>, config: ScaleConfig) -> Self {
        let output_format =
            PortFormat::Video(VideoPortFormat::any().with_dimensions(config.width, config.height));

        Self {
            id,
            name: name.into(),
            state: NodeState::Idle,
            inputs: vec![InputPort::new(PortId(0), "input", PortType::Video)
                .with_format(PortFormat::Video(VideoPortFormat::any()))],
            outputs: vec![
                OutputPort::new(PortId(0), "output", PortType::Video).with_format(output_format)
            ],
            config,
            luma_coefficients: None,
            chroma_coefficients: None,
        }
    }

    /// Get the current configuration.
    #[must_use]
    pub fn config(&self) -> &ScaleConfig {
        &self.config
    }

    /// Update the target dimensions.
    pub fn set_dimensions(&mut self, width: u32, height: u32) {
        if self.config.width != width || self.config.height != height {
            self.config.width = width;
            self.config.height = height;
            self.luma_coefficients = None;
            self.chroma_coefficients = None;
        }
    }

    /// Return the cached coefficient pair for `key`, rebuilding it only when the geometry changed.
    ///
    /// Taking the slot as a plain `&mut Option<_>` keeps this independent of `&mut self`, so the
    /// caller can hold the returned borrow while reading other fields.
    fn slot_entry(slot: &mut Option<PlaneCoefficients>, key: CoefficientKey) -> &PlaneCoefficients {
        if !slot.as_ref().is_some_and(|entry| entry.key == key) {
            *slot = None;
        }
        slot.get_or_insert_with(|| PlaneCoefficients::build(key))
    }

    /// Scale a video frame.
    fn scale_frame(&mut self, input: &VideoFrame) -> VideoFrame {
        let mut output = VideoFrame::new(input.format, self.config.width, self.config.height);
        output.timestamp = input.timestamp;
        output.frame_type = input.frame_type;
        output.color_info = input.color_info;

        let algorithm = self.config.algorithm;
        let antialias = self.config.antialias;

        // Scale each plane
        for (i, src_plane) in input.planes.iter().enumerate() {
            let (src_w, src_h) = input.plane_dimensions(i);
            let (dst_w, dst_h) = output.plane_dimensions(i);

            let key = CoefficientKey {
                src_width: src_w,
                src_height: src_h,
                dst_width: dst_w,
                dst_height: dst_h,
                algorithm,
                antialias,
            };

            // Subsampled chroma planes need their own coefficients; both of them share one
            // geometry, so they share one cache slot.
            let coefficients = if i > 0 && input.format.is_yuv() {
                Self::slot_entry(&mut self.chroma_coefficients, key)
            } else {
                Self::slot_entry(&mut self.luma_coefficients, key)
            };

            let plane = scale_plane_with(coefficients, src_plane, src_w, src_h, dst_w, dst_h);
            output.planes.push(plane);
        }

        output
    }
}

impl Node for ScaleFilter {
    fn id(&self) -> NodeId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> NodeType {
        NodeType::Filter
    }

    fn state(&self) -> NodeState {
        self.state
    }

    fn set_state(&mut self, state: NodeState) -> GraphResult<()> {
        if !self.state.can_transition_to(state) {
            return Err(GraphError::InvalidStateTransition {
                node: self.id,
                from: self.state.to_string(),
                to: state.to_string(),
            });
        }
        self.state = state;
        Ok(())
    }

    fn inputs(&self) -> &[InputPort] {
        &self.inputs
    }

    fn outputs(&self) -> &[OutputPort] {
        &self.outputs
    }

    fn process(&mut self, input: Option<FilterFrame>) -> GraphResult<Option<FilterFrame>> {
        match input {
            Some(FilterFrame::Video(frame)) => {
                let scaled = self.scale_frame(&frame);
                Ok(Some(FilterFrame::Video(scaled)))
            }
            Some(_) => Err(GraphError::PortTypeMismatch {
                expected: "Video".to_string(),
                actual: "Audio".to_string(),
            }),
            None => Ok(None),
        }
    }
}

/// Nearest neighbor scaler for fast, low-quality scaling.
#[derive(Debug)]
pub struct NearestNeighborScaler {
    dst_width: u32,
    dst_height: u32,
}

impl NearestNeighborScaler {
    /// Create a new nearest neighbor scaler.
    #[must_use]
    pub fn new(dst_width: u32, dst_height: u32) -> Self {
        Self {
            dst_width,
            dst_height,
        }
    }

    /// Scale a plane using nearest neighbor interpolation.
    #[must_use]
    pub fn scale_plane(&self, src: &Plane, src_width: u32, src_height: u32) -> Plane {
        let mut dst_data = vec![0u8; self.dst_width as usize * self.dst_height as usize];

        let x_ratio = src_width as f64 / self.dst_width as f64;
        let y_ratio = src_height as f64 / self.dst_height as f64;

        for y in 0..self.dst_height as usize {
            let src_y = ((y as f64 + 0.5) * y_ratio).floor() as usize;
            let src_y = src_y.min(src_height as usize - 1);
            let src_row = src.row(src_y);

            for x in 0..self.dst_width as usize {
                let src_x = ((x as f64 + 0.5) * x_ratio).floor() as usize;
                let src_x = src_x.min(src_width as usize - 1);
                dst_data[y * self.dst_width as usize + x] =
                    src_row.get(src_x).copied().unwrap_or(0);
            }
        }

        Plane::new(dst_data, self.dst_width as usize)
    }
}

/// Bilinear scaler for moderate quality scaling.
#[derive(Debug)]
pub struct BilinearScaler {
    dst_width: u32,
    dst_height: u32,
}

impl BilinearScaler {
    /// Create a new bilinear scaler.
    #[must_use]
    pub fn new(dst_width: u32, dst_height: u32) -> Self {
        Self {
            dst_width,
            dst_height,
        }
    }

    /// Scale a plane using bilinear interpolation.
    #[must_use]
    pub fn scale_plane(&self, src: &Plane, src_width: u32, src_height: u32) -> Plane {
        let mut dst_data = vec![0u8; self.dst_width as usize * self.dst_height as usize];

        let x_ratio = (src_width as f64 - 1.0) / (self.dst_width as f64 - 1.0).max(1.0);
        let y_ratio = (src_height as f64 - 1.0) / (self.dst_height as f64 - 1.0).max(1.0);

        for y in 0..self.dst_height as usize {
            let src_y = y as f64 * y_ratio;
            let y0 = src_y.floor() as usize;
            let y1 = (y0 + 1).min(src_height as usize - 1);
            let y_frac = src_y - y0 as f64;

            let row0 = src.row(y0);
            let row1 = src.row(y1);

            for x in 0..self.dst_width as usize {
                let src_x = x as f64 * x_ratio;
                let x0 = src_x.floor() as usize;
                let x1 = (x0 + 1).min(src_width as usize - 1);
                let x_frac = src_x - x0 as f64;

                let p00 = row0.get(x0).copied().unwrap_or(0) as f64;
                let p10 = row0.get(x1).copied().unwrap_or(0) as f64;
                let p01 = row1.get(x0).copied().unwrap_or(0) as f64;
                let p11 = row1.get(x1).copied().unwrap_or(0) as f64;

                let top = p00 * (1.0 - x_frac) + p10 * x_frac;
                let bottom = p01 * (1.0 - x_frac) + p11 * x_frac;
                let value = top * (1.0 - y_frac) + bottom * y_frac;

                dst_data[y * self.dst_width as usize + x] = value.round().clamp(0.0, 255.0) as u8;
            }
        }

        Plane::new(dst_data, self.dst_width as usize)
    }
}

/// Calculate the aspect ratio preserving dimensions.
#[must_use]
pub fn calculate_aspect_fit(
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> (u32, u32) {
    let src_aspect = src_width as f64 / src_height as f64;
    let dst_aspect = dst_width as f64 / dst_height as f64;

    if src_aspect > dst_aspect {
        // Width limited
        let new_height = (dst_width as f64 / src_aspect).round() as u32;
        (dst_width, new_height)
    } else {
        // Height limited
        let new_width = (dst_height as f64 * src_aspect).round() as u32;
        (new_width, dst_height)
    }
}

/// Calculate the aspect ratio preserving dimensions for fill mode.
#[must_use]
pub fn calculate_aspect_fill(
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> (u32, u32) {
    let src_aspect = src_width as f64 / src_height as f64;
    let dst_aspect = dst_width as f64 / dst_height as f64;

    if src_aspect < dst_aspect {
        // Width limited
        let new_height = (dst_width as f64 / src_aspect).round() as u32;
        (dst_width, new_height)
    } else {
        // Height limited
        let new_width = (dst_height as f64 * src_aspect).round() as u32;
        (new_width, dst_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    fn create_test_frame(width: u32, height: u32) -> VideoFrame {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
        frame.allocate();

        // Fill with a gradient pattern
        if let Some(plane) = frame.planes.get_mut(0) {
            let mut data = vec![0u8; width as usize * height as usize];
            for y in 0..height as usize {
                for x in 0..width as usize {
                    data[y * width as usize + x] = ((x + y) % 256) as u8;
                }
            }
            *plane = Plane::new(data, width as usize);
        }

        frame
    }

    #[test]
    fn test_scale_filter_creation() {
        let config = ScaleConfig::new(1280, 720)
            .with_algorithm(ScaleAlgorithm::Lanczos3)
            .with_antialias(true);

        let filter = ScaleFilter::new(NodeId(0), "scale", config);

        assert_eq!(filter.id(), NodeId(0));
        assert_eq!(filter.name(), "scale");
        assert_eq!(filter.config().width, 1280);
        assert_eq!(filter.config().height, 720);
        assert_eq!(filter.config().algorithm, ScaleAlgorithm::Lanczos3);
    }

    #[test]
    fn test_scale_algorithms() {
        assert_eq!(ScaleAlgorithm::Nearest.support(), 0.5);
        assert_eq!(ScaleAlgorithm::Bilinear.support(), 1.0);
        assert_eq!(ScaleAlgorithm::Bicubic.support(), 2.0);
        assert_eq!(ScaleAlgorithm::Lanczos2.support(), 2.0);
        assert_eq!(ScaleAlgorithm::Lanczos3.support(), 3.0);
        assert_eq!(ScaleAlgorithm::Lanczos4.support(), 4.0);
    }

    #[test]
    fn test_kernel_values() {
        // Nearest at center should be 1
        assert!((ScaleAlgorithm::Nearest.kernel(0.0) - 1.0).abs() < 0.001);
        assert!((ScaleAlgorithm::Nearest.kernel(0.6) - 0.0).abs() < 0.001);

        // Bilinear at center should be 1
        assert!((ScaleAlgorithm::Bilinear.kernel(0.0) - 1.0).abs() < 0.001);
        assert!((ScaleAlgorithm::Bilinear.kernel(0.5) - 0.5).abs() < 0.001);
        assert!((ScaleAlgorithm::Bilinear.kernel(1.0) - 0.0).abs() < 0.001);

        // Lanczos at center should be 1
        assert!((ScaleAlgorithm::Lanczos3.kernel(0.0) - 1.0).abs() < 0.001);
    }

    /// Deterministic single-plane test pattern: gradient + checkerboard + a non-separable ripple.
    fn create_test_plane(width: u32, height: u32) -> Plane {
        let mut data = vec![0u8; width as usize * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let gradient = (x * 3 + y * 5) % 200;
                let checker = if ((x / 6) + (y / 6)) % 2 == 0 { 48 } else { 0 };
                let ripple = ((x * 7 + y * 13) % 17) * 3;
                if let Some(slot) = data.get_mut(y * width as usize + x) {
                    *slot = ((gradient + checker + ripple) % 256) as u8;
                }
            }
        }
        Plane::new(data, width as usize)
    }

    #[test]
    fn test_parallel_and_serial_passes_are_byte_identical() {
        // Row parallelism must be a pure scheduling change: every output pixel still accumulates
        // its taps in ascending order onto an accumulator seeded at 0.0, so the two execution
        // policies have to agree bit for bit. Production frames are far above the automatic
        // threshold, so this is the only place the parallel path is compared directly.
        let algorithms = [
            ScaleAlgorithm::Nearest,
            ScaleAlgorithm::Bilinear,
            ScaleAlgorithm::Bicubic,
            ScaleAlgorithm::CatmullRom,
            ScaleAlgorithm::Lanczos2,
            ScaleAlgorithm::Lanczos3,
            ScaleAlgorithm::Lanczos4,
        ];
        // (src_w, src_h, dst_w, dst_h, antialias)
        let geometries = [
            (160u32, 120u32, 96u32, 72u32, true),
            (160, 120, 96, 72, false),
            (61, 37, 128, 96, true),
            (152, 90, 90, 160, true),
            (97, 61, 97, 61, true),
        ];

        for algorithm in algorithms {
            for (src_w, src_h, dst_w, dst_h, antialias) in geometries {
                let key = CoefficientKey {
                    src_width: src_w,
                    src_height: src_h,
                    dst_width: dst_w,
                    dst_height: dst_h,
                    algorithm,
                    antialias,
                };
                let coefficients = PlaneCoefficients::build(key);
                let src = create_test_plane(src_w, src_h);

                let serial = resample_plane(
                    &coefficients,
                    &src,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                    PassParallelism::SERIAL,
                );
                let parallel = resample_plane(
                    &coefficients,
                    &src,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                    PassParallelism::PARALLEL,
                );
                let mixed = resample_plane(
                    &coefficients,
                    &src,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                    PassParallelism {
                        horizontal: true,
                        vertical: false,
                    },
                );

                assert_eq!(
                    serial.data, parallel.data,
                    "{algorithm:?} {src_w}x{src_h}->{dst_w}x{dst_h} aa={antialias}: \
                     parallel output differs from serial"
                );
                assert_eq!(
                    serial.data, mixed.data,
                    "{algorithm:?} {src_w}x{src_h}->{dst_w}x{dst_h} aa={antialias}: \
                     mixed-policy output differs from serial"
                );
            }
        }
    }

    #[test]
    fn test_automatic_parallelism_threshold() {
        // A thumbnail-sized pass stays on the calling thread; a 1080x1920 pass does not.
        assert_eq!(
            PassParallelism::automatic(96, 120, 72),
            PassParallelism::SERIAL
        );
        assert_eq!(
            PassParallelism::automatic(1080, 1080, 1920),
            PassParallelism::PARALLEL
        );
        // Horizontal above, vertical below: the two passes are decided independently.
        let mixed = PassParallelism::automatic(1024, 128, 32);
        assert!(mixed.horizontal);
        assert!(!mixed.vertical);
    }

    #[test]
    fn test_short_source_row_reads_as_zero() {
        // `Plane::row` returns a short/empty slice for a truncated buffer. The scaler must treat
        // the missing samples as 0 rather than panicking or reading a neighbouring row.
        let key = CoefficientKey {
            src_width: 32,
            src_height: 8,
            dst_width: 16,
            dst_height: 4,
            algorithm: ScaleAlgorithm::Lanczos3,
            antialias: true,
        };
        let coefficients = PlaneCoefficients::build(key);

        // Only three full rows of data behind a 32-byte stride, eight rows claimed.
        let truncated = Plane::new(vec![200u8; 32 * 3], 32);
        let scaled = resample_plane(
            &coefficients,
            &truncated,
            32,
            8,
            16,
            4,
            PassParallelism::SERIAL,
        );
        assert_eq!(scaled.data.len(), 16 * 4);
        assert_eq!(scaled.stride, 16);

        let parallel = resample_plane(
            &coefficients,
            &truncated,
            32,
            8,
            16,
            4,
            PassParallelism::PARALLEL,
        );
        assert_eq!(scaled.data, parallel.data);
    }

    #[test]
    fn test_coefficient_table_is_normalized_and_in_bounds() {
        for algorithm in [
            ScaleAlgorithm::Nearest,
            ScaleAlgorithm::Bilinear,
            ScaleAlgorithm::Bicubic,
            ScaleAlgorithm::CatmullRom,
            ScaleAlgorithm::Lanczos2,
            ScaleAlgorithm::Lanczos3,
            ScaleAlgorithm::Lanczos4,
        ] {
            // `(608, 1080)` and the exact-ratio pairs put output centres exactly on source-pixel
            // boundaries, which is where the tap window used to degenerate for `Nearest`. Every
            // kernel is held to the same standard here: a usable, in-bounds, normalised window for
            // every output position, with and without antialiasing.
            for (src, dst) in [
                (608u32, 1080u32),
                (1080, 608),
                (37, 37),
                (1, 64),
                (64, 1),
                (8, 4),
                (16, 4),
                (120, 60),
                (2, 3),
            ] {
                for antialias in [true, false] {
                    let table = CoefficientTable::build(src, dst, algorithm, antialias);
                    assert_eq!(table.spans.len(), dst as usize);
                    for span in &table.spans {
                        assert!(
                            span.len >= 1,
                            "{algorithm:?} {src}->{dst} aa={antialias}: empty tap window"
                        );
                        assert!(
                            span.src_start + span.len <= src,
                            "{algorithm:?} {src}->{dst} aa={antialias}: tap window runs past the \
                             source"
                        );
                        let weights = table.window(*span);
                        assert_eq!(weights.len(), span.len as usize);

                        let sum: f64 = weights.iter().sum();
                        assert!(
                            (sum - 1.0).abs() < 1e-9,
                            "{algorithm:?} {src}->{dst} aa={antialias}: weights sum to {sum}, not 1"
                        );
                        assert!(
                            weights.iter().any(|weight| *weight != 0.0),
                            "{algorithm:?} {src}->{dst} aa={antialias}: all-zero tap window"
                        );
                    }
                }
            }
        }
    }

    /// Reference point-sample index: the one `Nearest` is defined by.
    fn nearest_source_index(dst_pos: u32, src_size: u32, dst_size: u32) -> u32 {
        let scale = src_size as f64 / dst_size as f64;
        (((dst_pos as f64 + 0.5) * scale).floor() as u32).min(src_size - 1)
    }

    #[test]
    fn test_nearest_taps_are_a_single_source_pixel() {
        // `Nearest` is a point sampler, so every output position must resolve to exactly one
        // source sample with weight 1.0 -- independent of the antialias flag, which only ever
        // widened the window into a box average.
        //
        // Regression: with the generic tap builder an output centre that lands exactly on a
        // source-pixel boundary produced an all-zero window. `8 -> 4` (an exact 2x downscale,
        // antialias off) puts *every* centre there: centre = (d + 0.5) * 2 - 0.5 = 2d + 0.5, whose
        // only candidate tap sits at distance exactly 0.5, and the kernel is `x < 0.5`. The whole
        // plane came out black. `608 -> 1080` hits the same boundary on 6 of its 1080 rows.
        for (src, dst, antialias) in [
            (8u32, 4u32, false),
            (8, 4, true),
            (16, 4, false),
            (120, 60, false),
            (2, 3, false),
            (2, 3, true),
            (608, 1080, true),
            (1080, 608, true),
            (160, 96, true),
            (96, 224, true),
            (37, 37, true),
            (1, 64, true),
            (64, 1, true),
        ] {
            let table = CoefficientTable::build(src, dst, ScaleAlgorithm::Nearest, antialias);
            assert_eq!(table.spans.len(), dst as usize);
            for (dst_pos, span) in table.spans.iter().enumerate() {
                let weights = table.window(*span);
                assert_eq!(
                    weights,
                    &[1.0],
                    "Nearest {src}->{dst} aa={antialias} d={dst_pos}: expected one unit tap, \
                     got {weights:?}"
                );
                assert_eq!(
                    span.src_start,
                    nearest_source_index(dst_pos as u32, src, dst),
                    "Nearest {src}->{dst} aa={antialias} d={dst_pos}: wrong source pixel"
                );
                assert!(span.src_start < src);
            }
        }
    }

    #[test]
    fn test_nearest_resample_matches_dedicated_scaler() {
        // The generic two-pass resampler and [`NearestNeighborScaler`] must agree byte for byte:
        // both are point samplers over the same index formula, so any divergence is a bug in one
        // of them. Before the tap builder special-cased `Nearest`, the 2x cases below came out
        // entirely black and the antialiased downscales came out box-filtered.
        for (src_w, src_h, dst_w, dst_h, antialias) in [
            (8u32, 8u32, 4u32, 4u32, false),
            (8, 8, 4, 4, true),
            (160, 120, 80, 60, false),
            (160, 120, 80, 60, true),
            (160, 120, 96, 72, true),
            (152, 90, 90, 160, true),
            (61, 37, 128, 96, true),
            (96, 72, 224, 168, true),
        ] {
            let key = CoefficientKey {
                src_width: src_w,
                src_height: src_h,
                dst_width: dst_w,
                dst_height: dst_h,
                algorithm: ScaleAlgorithm::Nearest,
                antialias,
            };
            let coefficients = PlaneCoefficients::build(key);
            let src = create_test_plane(src_w, src_h);

            let resampled = resample_plane(
                &coefficients,
                &src,
                src_w,
                src_h,
                dst_w,
                dst_h,
                PassParallelism::SERIAL,
            );
            let expected = NearestNeighborScaler::new(dst_w, dst_h).scale_plane(&src, src_w, src_h);

            assert_eq!(
                resampled.data, expected.data,
                "Nearest {src_w}x{src_h}->{dst_w}x{dst_h} aa={antialias}: the generic resampler \
                 does not point-sample"
            );
            assert!(
                resampled.data.iter().any(|byte| *byte != 0),
                "Nearest {src_w}x{src_h}->{dst_w}x{dst_h} aa={antialias}: output is entirely black"
            );
        }
    }

    #[test]
    fn test_chroma_coefficients_cached_across_planes() {
        let config = ScaleConfig::new(128, 96).with_algorithm(ScaleAlgorithm::Lanczos3);
        let mut filter = ScaleFilter::new(NodeId(0), "scale", config);
        assert!(filter.luma_coefficients.is_none());
        assert!(filter.chroma_coefficients.is_none());

        let input = create_test_frame(61, 37);
        let _ = filter.scale_frame(&input);

        let luma_key = filter
            .luma_coefficients
            .as_ref()
            .map(|entry| entry.key)
            .expect("luma coefficients must be cached after a frame");
        let chroma_key = filter
            .chroma_coefficients
            .as_ref()
            .map(|entry| entry.key)
            .expect("chroma coefficients must be cached after a frame");

        assert_eq!((luma_key.src_width, luma_key.src_height), (61, 37));
        assert_eq!((luma_key.dst_width, luma_key.dst_height), (128, 96));
        // 4:2:0 chroma of a 61x37 frame is div_ceil-rounded to 31x19 -> 64x48.
        assert_eq!((chroma_key.src_width, chroma_key.src_height), (31, 19));
        assert_eq!((chroma_key.dst_width, chroma_key.dst_height), (64, 48));

        // Retargeting must invalidate both slots.
        filter.set_dimensions(64, 48);
        assert!(filter.luma_coefficients.is_none());
        assert!(filter.chroma_coefficients.is_none());
    }

    #[test]
    fn test_scale_downscale() {
        let config = ScaleConfig::new(80, 60);
        let mut filter = ScaleFilter::new(NodeId(0), "scale", config);

        let input = create_test_frame(160, 120);
        let result = filter.scale_frame(&input);

        assert_eq!(result.width, 80);
        assert_eq!(result.height, 60);
        assert_eq!(result.planes.len(), input.planes.len());
    }

    #[test]
    fn test_scale_upscale() {
        let config = ScaleConfig::new(320, 240);
        let mut filter = ScaleFilter::new(NodeId(0), "scale", config);

        let input = create_test_frame(160, 120);
        let result = filter.scale_frame(&input);

        assert_eq!(result.width, 320);
        assert_eq!(result.height, 240);
    }

    #[test]
    fn test_nearest_neighbor_scaler() {
        let scaler = NearestNeighborScaler::new(320, 240);

        let src_data = vec![128u8; 640 * 480];
        let src_plane = Plane::new(src_data, 640);

        let result = scaler.scale_plane(&src_plane, 640, 480);
        assert_eq!(result.stride, 320);
    }

    #[test]
    fn test_bilinear_scaler() {
        let scaler = BilinearScaler::new(320, 240);

        let src_data = vec![128u8; 640 * 480];
        let src_plane = Plane::new(src_data, 640);

        let result = scaler.scale_plane(&src_plane, 640, 480);
        assert_eq!(result.stride, 320);
    }

    #[test]
    fn test_aspect_fit() {
        // 16:9 source into 4:3 container
        let (w, h) = calculate_aspect_fit(1920, 1080, 640, 480);
        assert_eq!(w, 640);
        assert!(h <= 480);

        // 4:3 source into 16:9 container
        let (w, h) = calculate_aspect_fit(640, 480, 1920, 1080);
        assert!(w <= 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn test_aspect_fill() {
        // 16:9 source into 4:3 container (will crop width)
        let (w, h) = calculate_aspect_fill(1920, 1080, 640, 480);
        assert!(w >= 640 || h >= 480);
    }

    #[test]
    fn test_sinc_function() {
        assert!((sinc(0.0) - 1.0).abs() < 0.001);
        // sinc(1) should be 0
        assert!(sinc(1.0).abs() < 0.001);
    }

    #[test]
    fn test_node_trait_implementation() {
        let config = ScaleConfig::new(1280, 720);
        let mut filter = ScaleFilter::new(NodeId(42), "test_scale", config);

        assert_eq!(filter.node_type(), NodeType::Filter);
        assert_eq!(filter.state(), NodeState::Idle);
        assert_eq!(filter.inputs().len(), 1);
        assert_eq!(filter.outputs().len(), 1);

        filter
            .set_state(NodeState::Processing)
            .expect("set_state should succeed");
        assert_eq!(filter.state(), NodeState::Processing);
    }

    #[test]
    fn test_process_none_input() {
        let config = ScaleConfig::new(1280, 720);
        let mut filter = ScaleFilter::new(NodeId(0), "scale", config);

        let result = filter.process(None).expect("process should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn test_scale_config_builder() {
        let config = ScaleConfig::new(1920, 1080)
            .with_algorithm(ScaleAlgorithm::CatmullRom)
            .with_antialias(false)
            .with_preserve_aspect(true);

        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.algorithm, ScaleAlgorithm::CatmullRom);
        assert!(!config.antialias);
        assert!(config.preserve_aspect);
    }
}

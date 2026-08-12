//! Panorama image stitching support.
//!
//! Provides data structures for describing image patches, computing their
//! overlap regions, and generating blend weights for seamless panorama output.

#![allow(dead_code)]
#![allow(clippy::cast_precision_loss)]

/// Describes a single image patch placed on a panorama canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagePatch {
    /// Unique identifier.
    pub id: u32,
    /// Horizontal offset on the canvas (can be negative for cropped sources).
    pub x_offset: i32,
    /// Vertical offset on the canvas.
    pub y_offset: i32,
    /// Width of the patch in pixels.
    pub width: u32,
    /// Height of the patch in pixels.
    pub height: u32,
    /// Rotation in radians (positive = counter-clockwise).
    pub rotation: f64,
}

impl ImagePatch {
    /// Creates a new `ImagePatch`.
    #[must_use]
    pub fn new(id: u32, x_offset: i32, y_offset: i32, width: u32, height: u32) -> Self {
        Self {
            id,
            x_offset,
            y_offset,
            width,
            height,
            rotation: 0.0,
        }
    }

    /// Returns the area of this patch in pixels.
    #[must_use]
    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Returns the right-edge x coordinate (exclusive).
    #[must_use]
    pub fn x_end(&self) -> i32 {
        self.x_offset + self.width as i32
    }

    /// Returns the bottom-edge y coordinate (exclusive).
    #[must_use]
    pub fn y_end(&self) -> i32 {
        self.y_offset + self.height as i32
    }

    /// Computes the overlap rectangle with another patch.
    ///
    /// Returns `Some((x, y, w, h))` where (x, y) is the top-left corner of the
    /// overlap and (w, h) are its dimensions, or `None` if there is no overlap.
    #[must_use]
    pub fn overlap_with(&self, other: &ImagePatch) -> Option<(i32, i32, u32, u32)> {
        find_overlap_region(self, other)
    }
}

/// Configuration for the panorama stitcher.
#[derive(Debug, Clone)]
pub struct StitchConfig {
    /// Width of the feathering blend zone at patch boundaries (pixels).
    pub blend_width: u32,
    /// Apply warp/lens-distortion correction before blending.
    pub warp_correction: bool,
    /// Blend exposure levels between patches.
    pub exposure_blend: bool,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            blend_width: 64,
            warp_correction: false,
            exposure_blend: true,
        }
    }
}

/// Computes per-pixel linear blend weights across an overlap zone.
///
/// Returns `overlap_pixels` weights linearly ramping from `0.0` to `1.0`.
/// If `overlap_pixels == 0`, returns an empty vector.
/// The `blend_width` parameter limits how quickly the weights saturate; if
/// `overlap_pixels > blend_width` the outer pixels receive full weight 1.0.
#[must_use]
pub fn compute_overlap_blend_weights(overlap_pixels: u32, blend_width: u32) -> Vec<f64> {
    if overlap_pixels == 0 {
        return Vec::new();
    }
    let effective = blend_width.min(overlap_pixels);
    let mut weights = Vec::with_capacity(overlap_pixels as usize);

    for i in 0..overlap_pixels {
        let w = if effective == 0 {
            1.0
        } else if i < effective {
            i as f64 / effective as f64
        } else {
            1.0
        };
        weights.push(w);
    }
    weights
}

/// Computes the axis-aligned overlap rectangle between two patches.
///
/// Returns `Some((x, y, w, h))` or `None` if there is no overlap.
#[must_use]
pub fn find_overlap_region(a: &ImagePatch, b: &ImagePatch) -> Option<(i32, i32, u32, u32)> {
    let x_start = a.x_offset.max(b.x_offset);
    let y_start = a.y_offset.max(b.y_offset);
    let x_end = a.x_end().min(b.x_end());
    let y_end = a.y_end().min(b.y_end());

    if x_end > x_start && y_end > y_start {
        Some((
            x_start,
            y_start,
            (x_end - x_start) as u32,
            (y_end - y_start) as u32,
        ))
    } else {
        None
    }
}

/// Builds a panorama from a collection of `ImagePatch` objects.
pub struct PanoramaBuilder {
    /// All registered patches.
    pub patches: Vec<ImagePatch>,
    /// Desired output canvas width.
    pub output_width: u32,
    /// Desired output canvas height.
    pub output_height: u32,
}

impl PanoramaBuilder {
    /// Creates a new `PanoramaBuilder` with a fixed output canvas size.
    #[must_use]
    pub fn new(output_w: u32, output_h: u32) -> Self {
        Self {
            patches: Vec::new(),
            output_width: output_w,
            output_height: output_h,
        }
    }

    /// Registers a patch with the builder.
    pub fn add_patch(&mut self, patch: ImagePatch) {
        self.patches.push(patch);
    }

    /// Computes the tightest bounding box that contains all registered patches.
    ///
    /// Returns `(width, height)` of the canvas needed.
    /// Returns `(0, 0)` when no patches are registered.
    #[must_use]
    pub fn compute_canvas_size(&self) -> (u32, u32) {
        if self.patches.is_empty() {
            return (0, 0);
        }
        let min_x = self.patches.iter().map(|p| p.x_offset).min().unwrap_or(0);
        let min_y = self.patches.iter().map(|p| p.y_offset).min().unwrap_or(0);
        let max_x = self.patches.iter().map(|p| p.x_end()).max().unwrap_or(0);
        let max_y = self.patches.iter().map(|p| p.y_end()).max().unwrap_or(0);

        let w = (max_x - min_x).max(0) as u32;
        let h = (max_y - min_y).max(0) as u32;
        (w, h)
    }

    /// Returns patch IDs in back-to-front render order (leftmost first).
    #[must_use]
    pub fn patch_order(&self) -> Vec<u32> {
        let mut indexed: Vec<(i32, u32)> =
            self.patches.iter().map(|p| (p.x_offset, p.id)).collect();
        indexed.sort_by_key(|&(x, _)| x);
        indexed.into_iter().map(|(_, id)| id).collect()
    }

    /// Returns a reference to the patch with the given id, if it exists.
    #[must_use]
    pub fn get_patch(&self, id: u32) -> Option<&ImagePatch> {
        self.patches.iter().find(|p| p.id == id)
    }

    /// Returns the number of patches registered.
    #[must_use]
    pub fn patch_count(&self) -> usize {
        self.patches.len()
    }
}

// ── Sub-pixel alignment ───────────────────────────────────────────────────────

/// Sub-pixel displacement result from [`subpixel_align`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubPixelOffset {
    /// Refined horizontal displacement (fractional pixels).
    pub dx: f64,
    /// Refined vertical displacement (fractional pixels).
    pub dy: f64,
    /// Estimated normalised cross-correlation in `[0, 1]`.
    pub correlation: f64,
}

impl SubPixelOffset {
    /// Returns `true` if the correlation exceeds `min_correlation`.
    #[must_use]
    pub fn is_reliable(&self, min_correlation: f64) -> bool {
        self.correlation >= min_correlation
    }

    /// Adds this offset to an integer position, returning a fractional canvas
    /// coordinate.
    #[must_use]
    pub fn apply_to(&self, x: i32, y: i32) -> (f64, f64) {
        (x as f64 + self.dx, y as f64 + self.dy)
    }
}

/// Compute the normalised cross-correlation (NCC) between two same-size patches.
///
/// Returns a value in `[-1, 1]`. Returns `0.0` if either patch has zero
/// variance (uniform colour).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn normalised_cross_correlation(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "patch sizes must match");
    let n = a.len();
    if n == 0 {
        return 0.0;
    }
    let mean_a: f64 = a.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let mean_b: f64 = b.iter().map(|&v| v as f64).sum::<f64>() / n as f64;

    let mut num = 0.0_f64;
    let mut denom_a = 0.0_f64;
    let mut denom_b = 0.0_f64;
    for (&va, &vb) in a.iter().zip(b.iter()) {
        let da = va as f64 - mean_a;
        let db = vb as f64 - mean_b;
        num += da * db;
        denom_a += da * da;
        denom_b += db * db;
    }
    let denom = (denom_a * denom_b).sqrt();
    if denom < 1e-12 {
        return 0.0;
    }
    (num / denom).clamp(-1.0, 1.0)
}

/// Extract a rectangular patch from a larger image, clamping out-of-bounds
/// coordinates to the nearest edge pixel.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn extract_patch(
    img: &[f32],
    img_width: usize,
    img_height: usize,
    patch_x: i64,
    patch_y: i64,
    patch_w: usize,
    patch_h: usize,
) -> Vec<f32> {
    let mut patch = Vec::with_capacity(patch_w * patch_h);
    let iw = img_width as i64;
    let ih = img_height as i64;
    for py in 0..patch_h as i64 {
        for px in 0..patch_w as i64 {
            let sx = (patch_x + px).clamp(0, iw - 1) as usize;
            let sy = (patch_y + py).clamp(0, ih - 1) as usize;
            patch.push(img[sy * img_width + sx]);
        }
    }
    patch
}

/// Refine an integer-pixel offset to sub-pixel accuracy using parabolic
/// interpolation of the NCC surface.
///
/// Searches over a `±search_radius` integer window centred on
/// `(initial_x, initial_y)` in the *moving* image to find the highest NCC
/// against the *fixed* template patch. A 1-D parabola is then fitted through
/// the peak and its two axis-aligned neighbours to estimate a fractional
/// displacement correction.
///
/// # Returns
///
/// A [`SubPixelOffset`] with `dx`/`dy` relative to `(initial_x, initial_y)`
/// and the peak NCC value.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn subpixel_align(
    fixed: &[f32],
    fixed_w: usize,
    fixed_h: usize,
    moving: &[f32],
    moving_w: usize,
    moving_h: usize,
    initial_x: i64,
    initial_y: i64,
    search_radius: usize,
) -> SubPixelOffset {
    let r = search_radius as i64;
    let diameter = 2 * search_radius + 1;
    let mut scores = vec![0.0_f64; diameter * diameter];

    for dy in -r..=r {
        for dx in -r..=r {
            let patch = extract_patch(
                moving,
                moving_w,
                moving_h,
                initial_x + dx,
                initial_y + dy,
                fixed_w,
                fixed_h,
            );
            let ncc = normalised_cross_correlation(fixed, &patch);
            let gx = (dx + r) as usize;
            let gy = (dy + r) as usize;
            scores[gy * diameter + gx] = ncc;
        }
    }

    // Find the grid-level peak
    let mut best_score = f64::NEG_INFINITY;
    let mut best_gx = r as usize;
    let mut best_gy = r as usize;
    for gy in 0..diameter {
        for gx in 0..diameter {
            let s = scores[gy * diameter + gx];
            if s > best_score {
                best_score = s;
                best_gx = gx;
                best_gy = gy;
            }
        }
    }

    // Parabolic sub-pixel refinement in x
    let sub_dx = if best_gx > 0 && best_gx + 1 < diameter {
        let sm = scores[best_gy * diameter + (best_gx - 1)];
        let s0 = scores[best_gy * diameter + best_gx];
        let sp = scores[best_gy * diameter + (best_gx + 1)];
        let denom = 2.0 * (2.0 * s0 - sm - sp);
        if denom.abs() > 1e-12 {
            (sm - sp) / denom
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Parabolic sub-pixel refinement in y
    let sub_dy = if best_gy > 0 && best_gy + 1 < diameter {
        let sm = scores[(best_gy - 1) * diameter + best_gx];
        let s0 = scores[best_gy * diameter + best_gx];
        let sp = scores[(best_gy + 1) * diameter + best_gx];
        let denom = 2.0 * (2.0 * s0 - sm - sp);
        if denom.abs() > 1e-12 {
            (sm - sp) / denom
        } else {
            0.0
        }
    } else {
        0.0
    };

    let dx = (best_gx as f64 - r as f64) + sub_dx;
    let dy = (best_gy as f64 - r as f64) + sub_dy;

    SubPixelOffset {
        dx,
        dy,
        correlation: best_score.clamp(0.0, 1.0),
    }
}

/// Refine all patch offsets in a `PanoramaBuilder` to sub-pixel accuracy.
///
/// Each patch (except the first, which is the fixed reference) is compared
/// against the first patch via [`subpixel_align`]. Patches whose correlation
/// falls below `min_correlation` are returned with their original integer
/// offset.
///
/// `pixel_data` is a slice of `(patch_id, pixels)` pairs where `pixels` is a
/// single-channel `f32` buffer of length `patch.width * patch.height`.
///
/// Returns a `Vec<(patch_id, SubPixelOffset)>` — one entry per non-reference patch.
#[must_use]
pub fn refine_patch_offsets(
    builder: &PanoramaBuilder,
    pixel_data: &[(u32, Vec<f32>)],
    search_radius: usize,
    min_correlation: f64,
) -> Vec<(u32, SubPixelOffset)> {
    if builder.patches.is_empty() || pixel_data.is_empty() {
        return Vec::new();
    }

    let first = &builder.patches[0];
    let first_pixels = match pixel_data.iter().find(|(id, _)| *id == first.id) {
        Some((_, data)) => data.as_slice(),
        None => return Vec::new(),
    };

    let ref_w = first.width as usize;
    let ref_h = first.height as usize;

    builder.patches[1..]
        .iter()
        .filter_map(|patch| {
            let moving_pixels = pixel_data
                .iter()
                .find(|(id, _)| *id == patch.id)
                .map(|(_, data)| data.as_slice())?;
            let moving_w = patch.width as usize;
            let moving_h = patch.height as usize;
            let initial_x = (patch.x_offset - first.x_offset) as i64;
            let initial_y = (patch.y_offset - first.y_offset) as i64;

            let offset = subpixel_align(
                first_pixels,
                ref_w,
                ref_h,
                moving_pixels,
                moving_w,
                moving_h,
                initial_x,
                initial_y,
                search_radius,
            );

            if offset.is_reliable(min_correlation) {
                Some((patch.id, offset))
            } else {
                Some((
                    patch.id,
                    SubPixelOffset {
                        dx: initial_x as f64,
                        dy: initial_y as f64,
                        correlation: offset.correlation,
                    },
                ))
            }
        })
        .collect()
}

// ── Feature descriptor matching ───────────────────────────────────────────────

/// A feature descriptor: a fixed-length vector of `f32` values describing a
/// local image region, together with its image-space location.
///
/// Descriptor vectors are typically L2-normalised (unit vectors).
#[derive(Debug, Clone)]
pub struct Descriptor {
    /// Column (x) of the keypoint in the image.
    pub x: f32,
    /// Row (y) of the keypoint in the image.
    pub y: f32,
    /// Feature vector. Length must be consistent within a match call.
    pub values: Vec<f32>,
}

impl Descriptor {
    /// Create a new descriptor at the given image position.
    #[must_use]
    pub fn new(x: f32, y: f32, values: Vec<f32>) -> Self {
        Self { x, y, values }
    }

    /// Squared Euclidean distance between this and another descriptor.
    ///
    /// Returns `f32::INFINITY` if lengths differ.
    #[must_use]
    pub fn sq_dist(&self, other: &Self) -> f32 {
        if self.values.len() != other.values.len() {
            return f32::INFINITY;
        }
        self.values
            .iter()
            .zip(other.values.iter())
            .map(|(&a, &b)| (a - b) * (a - b))
            .sum()
    }
}

/// A correspondence between a descriptor in image A and one in image B.
#[derive(Debug, Clone)]
pub struct DescriptorMatch {
    /// Index into `descriptors_a`.
    pub idx_a: usize,
    /// Index into `descriptors_b`.
    pub idx_b: usize,
    /// Squared Euclidean distance (lower = more similar).
    pub distance: f32,
}

/// Find the nearest-neighbour in `candidates` for descriptor `query`.
///
/// Returns `None` if `candidates` is empty.
fn find_nearest(
    query: &Descriptor,
    candidates: &[Descriptor],
    query_idx: usize,
) -> Option<DescriptorMatch> {
    candidates
        .iter()
        .enumerate()
        .map(|(bi, b)| DescriptorMatch {
            idx_a: query_idx,
            idx_b: bi,
            distance: query.sq_dist(b),
        })
        .min_by(|x, y| {
            x.distance
                .partial_cmp(&y.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// Match descriptors from image A to image B using nearest-neighbour search,
/// parallelised with rayon so that each descriptor in A is processed
/// independently.
///
/// Each descriptor in `descriptors_a` is matched to the single closest
/// descriptor in `descriptors_b` (by squared Euclidean distance). The results
/// are returned in the same order as `descriptors_a`; unmatched queries
/// (e.g. when `descriptors_b` is empty) are omitted.
///
/// # Returns
///
/// A `Vec<DescriptorMatch>` with at most `descriptors_a.len()` entries, sorted
/// by ascending distance.
pub fn match_descriptors_parallel(
    descriptors_a: &[Descriptor],
    descriptors_b: &[Descriptor],
) -> Vec<DescriptorMatch> {
    use crate::parallel::filter_map_enumerated;

    if descriptors_b.is_empty() {
        return Vec::new();
    }

    let mut matches: Vec<DescriptorMatch> =
        filter_map_enumerated(descriptors_a, |ai, da| find_nearest(da, descriptors_b, ai));

    matches.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches
}

/// Match descriptors sequentially (reference implementation used for testing).
pub fn match_descriptors_sequential(
    descriptors_a: &[Descriptor],
    descriptors_b: &[Descriptor],
) -> Vec<DescriptorMatch> {
    if descriptors_b.is_empty() {
        return Vec::new();
    }

    let mut matches: Vec<DescriptorMatch> = descriptors_a
        .iter()
        .enumerate()
        .filter_map(|(ai, da)| find_nearest(da, descriptors_b, ai))
        .collect();

    matches.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_patch(id: u32, x: i32, y: i32, w: u32, h: u32) -> ImagePatch {
        ImagePatch::new(id, x, y, w, h)
    }

    #[test]
    fn test_patch_area() {
        let p = make_patch(1, 0, 0, 100, 200);
        assert_eq!(p.area(), 20_000);
    }

    #[test]
    fn test_patch_x_end() {
        let p = make_patch(1, 50, 0, 200, 100);
        assert_eq!(p.x_end(), 250);
    }

    #[test]
    fn test_patch_y_end() {
        let p = make_patch(1, 0, 30, 100, 70);
        assert_eq!(p.y_end(), 100);
    }

    #[test]
    fn test_overlap_region_adjacent_no_overlap() {
        let a = make_patch(1, 0, 0, 100, 100);
        let b = make_patch(2, 100, 0, 100, 100);
        assert!(find_overlap_region(&a, &b).is_none());
    }

    #[test]
    fn test_overlap_region_partial() {
        let a = make_patch(1, 0, 0, 150, 100);
        let b = make_patch(2, 100, 0, 150, 100);
        let overlap = find_overlap_region(&a, &b);
        assert_eq!(overlap, Some((100, 0, 50, 100)));
    }

    #[test]
    fn test_overlap_region_full_containment() {
        let outer = make_patch(1, 0, 0, 200, 200);
        let inner = make_patch(2, 50, 50, 50, 50);
        let overlap = find_overlap_region(&outer, &inner);
        assert_eq!(overlap, Some((50, 50, 50, 50)));
    }

    #[test]
    fn test_patch_overlap_method_delegates() {
        let a = make_patch(1, 0, 0, 150, 100);
        let b = make_patch(2, 100, 0, 150, 100);
        assert_eq!(a.overlap_with(&b), find_overlap_region(&a, &b));
    }

    #[test]
    fn test_blend_weights_length() {
        let w = compute_overlap_blend_weights(50, 64);
        assert_eq!(w.len(), 50);
    }

    #[test]
    fn test_blend_weights_ramp() {
        let w = compute_overlap_blend_weights(10, 10);
        // First weight should be near 0, last near 1
        assert!(w[0] < 0.2);
        assert!(w[9] > 0.8);
    }

    #[test]
    fn test_blend_weights_empty() {
        let w = compute_overlap_blend_weights(0, 64);
        assert!(w.is_empty());
    }

    #[test]
    fn test_panorama_builder_canvas_size() {
        let mut builder = PanoramaBuilder::new(1920, 1080);
        builder.add_patch(make_patch(1, 0, 0, 1000, 800));
        builder.add_patch(make_patch(2, 800, 0, 1000, 800));
        let (w, h) = builder.compute_canvas_size();
        assert_eq!(w, 1800);
        assert_eq!(h, 800);
    }

    #[test]
    fn test_panorama_builder_patch_order() {
        let mut builder = PanoramaBuilder::new(3000, 1000);
        builder.add_patch(make_patch(10, 500, 0, 500, 500));
        builder.add_patch(make_patch(20, 0, 0, 500, 500));
        builder.add_patch(make_patch(30, 1000, 0, 500, 500));
        let order = builder.patch_order();
        assert_eq!(order, vec![20, 10, 30]);
    }

    #[test]
    fn test_panorama_builder_empty_canvas() {
        let builder = PanoramaBuilder::new(1920, 1080);
        assert_eq!(builder.compute_canvas_size(), (0, 0));
    }

    #[test]
    fn test_panorama_builder_patch_count() {
        let mut builder = PanoramaBuilder::new(1920, 1080);
        builder.add_patch(make_patch(1, 0, 0, 100, 100));
        builder.add_patch(make_patch(2, 100, 0, 100, 100));
        assert_eq!(builder.patch_count(), 2);
    }

    // ── Sub-pixel alignment tests ───────────────────────────────────────────

    #[test]
    fn test_ncc_identical_patches() {
        let a = vec![0.1f32, 0.5, 0.9, 0.3];
        let ncc = normalised_cross_correlation(&a, &a);
        assert!((ncc - 1.0).abs() < 1e-9, "identical patches NCC={ncc}");
    }

    #[test]
    fn test_ncc_uniform_patches_returns_zero() {
        // Both patches uniform => both std-devs are 0 => returns 0.0
        let a = vec![0.5_f32; 9];
        let b = vec![0.5_f32; 9];
        let ncc = normalised_cross_correlation(&a, &b);
        assert!(ncc.abs() < 1e-9, "uniform NCC should be 0, got {ncc}");
    }

    #[test]
    fn test_ncc_inverted_patches() {
        let a: Vec<f32> = vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let b: Vec<f32> = a.iter().map(|&v| 1.0 - v).collect();
        let ncc = normalised_cross_correlation(&a, &b);
        assert!(ncc < -0.9, "inverted NCC should be near -1, got {ncc}");
    }

    #[test]
    fn test_extract_patch_within_bounds() {
        let img: Vec<f32> = (0..16).map(|i| i as f32).collect(); // 4x4
        let patch = extract_patch(&img, 4, 4, 1, 1, 2, 2);
        assert_eq!(patch.len(), 4);
        // Pixel (1,1)=5, (2,1)=6, (1,2)=9, (2,2)=10
        assert!((patch[0] - 5.0).abs() < 1e-6);
        assert!((patch[1] - 6.0).abs() < 1e-6);
    }

    #[test]
    fn test_extract_patch_clamped_border() {
        let img = vec![0.5_f32; 4 * 4];
        let patch = extract_patch(&img, 4, 4, -2, -2, 3, 3);
        assert_eq!(patch.len(), 9);
        for &v in &patch {
            assert!((v - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_subpixel_align_zero_offset() {
        // When fixed == moving, best integer offset should be (0, 0)
        let img: Vec<f32> = (0..100).map(|i| (i as f32 / 100.0).sin()).collect();
        let offset = subpixel_align(&img, 10, 10, &img, 10, 10, 0, 0, 2);
        assert!(
            offset.dx.abs() < 1.5,
            "dx should be near 0, got {}",
            offset.dx
        );
        assert!(
            offset.dy.abs() < 1.5,
            "dy should be near 0, got {}",
            offset.dy
        );
        assert!(
            offset.correlation > 0.9,
            "correlation={}",
            offset.correlation
        );
    }

    #[test]
    fn test_subpixel_align_correlation_clamped() {
        let a = vec![0.3_f32; 16];
        let b = vec![0.7_f32; 16];
        let offset = subpixel_align(&a, 4, 4, &b, 4, 4, 0, 0, 1);
        assert!(offset.correlation >= 0.0);
        assert!(offset.correlation <= 1.0);
    }

    #[test]
    fn test_subpixel_offset_is_reliable() {
        let off = SubPixelOffset {
            dx: 0.3,
            dy: -0.2,
            correlation: 0.85,
        };
        assert!(off.is_reliable(0.8));
        assert!(!off.is_reliable(0.9));
    }

    #[test]
    fn test_subpixel_offset_apply_to() {
        let off = SubPixelOffset {
            dx: 0.5,
            dy: -0.25,
            correlation: 1.0,
        };
        let (rx, ry) = off.apply_to(10, 20);
        assert!((rx - 10.5).abs() < 1e-9);
        assert!((ry - 19.75).abs() < 1e-9);
    }

    #[test]
    fn test_refine_patch_offsets_empty_builder() {
        let builder = PanoramaBuilder::new(1920, 1080);
        let data: Vec<(u32, Vec<f32>)> = Vec::new();
        let result = refine_patch_offsets(&builder, &data, 2, 0.5);
        assert!(result.is_empty());
    }

    #[test]
    fn test_refine_patch_offsets_single_reference() {
        // Only one patch => no moving patches => result empty
        let mut builder = PanoramaBuilder::new(1920, 1080);
        builder.add_patch(make_patch(1, 0, 0, 4, 4));
        let data = vec![(1u32, vec![0.5_f32; 16])];
        let result = refine_patch_offsets(&builder, &data, 1, 0.0);
        assert!(result.is_empty());
    }

    // ── Descriptor matching tests ─────────────────────────────────────────

    fn make_desc(x: f32, y: f32, v: Vec<f32>) -> Descriptor {
        Descriptor::new(x, y, v)
    }

    /// Build a set of N 8-D unit descriptors where descriptor i has a large
    /// component in dimension i%8, making nearest neighbours deterministic.
    fn synthetic_descriptors(n: usize, offset: f32) -> Vec<Descriptor> {
        (0..n)
            .map(|i| {
                let mut v = vec![0.0f32; 8];
                v[i % 8] = 1.0 + (i as f32) * 0.01 + offset;
                make_desc(i as f32, 0.0, v)
            })
            .collect()
    }

    #[test]
    fn test_stitch_parallel_matches_sequential() {
        // 200 descriptors in A, 200 in B; parallel and sequential must agree
        let descs_a = synthetic_descriptors(200, 0.0);
        let descs_b = synthetic_descriptors(200, 0.5);

        let par = match_descriptors_parallel(&descs_a, &descs_b);
        let seq = match_descriptors_sequential(&descs_a, &descs_b);

        assert_eq!(par.len(), seq.len(), "match count should be equal");
        for (p, s) in par.iter().zip(seq.iter()) {
            assert_eq!(p.idx_a, s.idx_a, "idx_a mismatch");
            assert_eq!(p.idx_b, s.idx_b, "idx_b mismatch");
            assert!((p.distance - s.distance).abs() < 1e-5, "distance mismatch");
        }
    }

    #[test]
    fn test_descriptor_sq_dist_identical() {
        let d = make_desc(0.0, 0.0, vec![1.0, 0.0, 0.0]);
        assert!((d.sq_dist(&d)).abs() < 1e-9);
    }

    #[test]
    fn test_descriptor_sq_dist_length_mismatch() {
        let a = make_desc(0.0, 0.0, vec![1.0, 0.0]);
        let b = make_desc(0.0, 0.0, vec![1.0, 0.0, 0.0]);
        assert_eq!(a.sq_dist(&b), f32::INFINITY);
    }

    #[test]
    fn test_match_empty_b() {
        let descs_a = synthetic_descriptors(10, 0.0);
        let descs_b: Vec<Descriptor> = Vec::new();
        let m = match_descriptors_parallel(&descs_a, &descs_b);
        assert!(m.is_empty());
    }

    #[test]
    fn test_match_sorted_by_distance() {
        let descs_a = synthetic_descriptors(50, 0.0);
        let descs_b = synthetic_descriptors(50, 0.1);
        let matches = match_descriptors_parallel(&descs_a, &descs_b);
        for w in matches.windows(2) {
            assert!(w[0].distance <= w[1].distance, "matches not sorted");
        }
    }
}

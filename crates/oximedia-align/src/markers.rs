//! Synchronization marker detection.
//!
//! This module provides tools for detecting visual and audio sync markers:
//!
//! - Clapper board detection
//! - Flash detection
//! - LED marker patterns
//! - Audio spike detection
//! - Timecode display recognition

use crate::{AlignError, AlignResult, Point2D};

/// Detected synchronization marker
#[derive(Debug, Clone)]
pub struct SyncMarker {
    /// Frame index where marker was detected
    pub frame: usize,
    /// Type of marker
    pub marker_type: MarkerType,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
    /// Optional location in frame
    pub location: Option<Point2D>,
}

/// Types of synchronization markers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerType {
    /// Clapper board closure
    ClapperClosure,
    /// Camera flash
    Flash,
    /// LED marker
    LedMarker,
    /// Audio spike/clap
    AudioSpike,
    /// Timecode display
    TimecodeDisplay,
}

impl SyncMarker {
    /// Create a new sync marker
    #[must_use]
    pub fn new(
        frame: usize,
        marker_type: MarkerType,
        confidence: f32,
        location: Option<Point2D>,
    ) -> Self {
        Self {
            frame,
            marker_type,
            confidence,
            location,
        }
    }
}

/// Flash detector for bright, sudden changes in luminance
pub struct FlashDetector {
    /// Brightness threshold (0.0 to 1.0)
    pub threshold: f32,
    /// Minimum flash duration in frames
    pub min_duration: usize,
    /// Maximum flash duration in frames
    pub max_duration: usize,
}

impl Default for FlashDetector {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            min_duration: 1,
            max_duration: 3,
        }
    }
}

impl FlashDetector {
    /// Create a new flash detector
    #[must_use]
    pub fn new(threshold: f32, min_duration: usize, max_duration: usize) -> Self {
        Self {
            threshold,
            min_duration,
            max_duration,
        }
    }

    /// Detect flashes in a sequence of frames
    #[must_use]
    pub fn detect(&self, frames: &[&[u8]], width: usize, height: usize) -> Vec<SyncMarker> {
        let mut markers = Vec::new();
        let brightness_values: Vec<f32> = frames
            .iter()
            .map(|frame| self.compute_brightness(frame, width, height))
            .collect();

        let mut in_flash = false;
        let mut flash_start = 0;

        for (i, &brightness) in brightness_values.iter().enumerate() {
            if !in_flash && brightness > self.threshold {
                in_flash = true;
                flash_start = i;
            } else if in_flash && brightness <= self.threshold {
                let duration = i - flash_start;
                if duration >= self.min_duration && duration <= self.max_duration {
                    let confidence = brightness_values[flash_start];
                    markers.push(SyncMarker::new(
                        flash_start,
                        MarkerType::Flash,
                        confidence,
                        None,
                    ));
                }
                in_flash = false;
            }
        }

        markers
    }

    /// Compute average brightness from RGB frame
    fn compute_brightness(&self, rgb: &[u8], width: usize, height: usize) -> f32 {
        if rgb.len() != width * height * 3 {
            return 0.0;
        }

        let sum: u32 = rgb
            .chunks_exact(3)
            .map(|pixel| {
                let r = u32::from(pixel[0]);
                let g = u32::from(pixel[1]);
                let b = u32::from(pixel[2]);
                (299 * r + 587 * g + 114 * b) / 1000
            })
            .sum();

        (sum as f32 / (width * height) as f32) / 255.0
    }

    /// Detect local flashes in specific regions
    #[must_use]
    pub fn detect_local(
        &self,
        frames: &[&[u8]],
        width: usize,
        _height: usize,
        region: &Region,
    ) -> Vec<SyncMarker> {
        let mut markers = Vec::new();

        for (frame_idx, frame) in frames.iter().enumerate() {
            let brightness = self.compute_region_brightness(frame, width, region);

            if brightness > self.threshold {
                let center = Point2D::new(
                    (region.x + region.width / 2) as f64,
                    (region.y + region.height / 2) as f64,
                );

                markers.push(SyncMarker::new(
                    frame_idx,
                    MarkerType::Flash,
                    brightness,
                    Some(center),
                ));
            }
        }

        markers
    }

    /// Compute brightness in a specific region
    fn compute_region_brightness(&self, rgb: &[u8], width: usize, region: &Region) -> f32 {
        let mut sum = 0u32;
        let mut count = 0u32;

        for y in region.y..region.y + region.height {
            for x in region.x..region.x + region.width {
                let idx = (y * width + x) * 3;
                if idx + 2 < rgb.len() {
                    let r = u32::from(rgb[idx]);
                    let g = u32::from(rgb[idx + 1]);
                    let b = u32::from(rgb[idx + 2]);
                    sum += (299 * r + 587 * g + 114 * b) / 1000;
                    count += 1;
                }
            }
        }

        if count > 0 {
            (sum as f32 / count as f32) / 255.0
        } else {
            0.0
        }
    }
}

/// Region of interest
#[derive(Debug, Clone, Copy)]
pub struct Region {
    /// X coordinate
    pub x: usize,
    /// Y coordinate
    pub y: usize,
    /// Width
    pub width: usize,
    /// Height
    pub height: usize,
}

impl Region {
    /// Create a new region
    #[must_use]
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Clapper board detector using motion detection
pub struct ClapperDetector {
    /// Motion threshold
    pub motion_threshold: f32,
    /// Minimum motion area (fraction of frame)
    pub min_motion_area: f32,
}

impl Default for ClapperDetector {
    fn default() -> Self {
        Self {
            motion_threshold: 30.0,
            min_motion_area: 0.1,
        }
    }
}

impl ClapperDetector {
    /// Create a new clapper detector
    #[must_use]
    pub fn new(motion_threshold: f32, min_motion_area: f32) -> Self {
        Self {
            motion_threshold,
            min_motion_area,
        }
    }

    /// Detect clapper closure by analyzing motion between frames
    ///
    /// # Errors
    /// Returns error if frames are invalid
    pub fn detect(
        &self,
        frames: &[&[u8]],
        width: usize,
        height: usize,
    ) -> AlignResult<Vec<SyncMarker>> {
        if frames.len() < 2 {
            return Err(AlignError::InsufficientData(
                "Need at least 2 frames".to_string(),
            ));
        }

        let mut markers = Vec::new();

        for i in 1..frames.len() {
            let motion = self.compute_motion(frames[i - 1], frames[i], width, height);

            if motion > self.min_motion_area {
                markers.push(SyncMarker::new(i, MarkerType::ClapperClosure, motion, None));
            }
        }

        Ok(markers)
    }

    /// Compute motion between two frames
    fn compute_motion(&self, frame1: &[u8], frame2: &[u8], width: usize, height: usize) -> f32 {
        let mut motion_pixels = 0;
        let total_pixels = width * height;

        for i in 0..total_pixels {
            let idx = i * 3;
            if idx + 2 < frame1.len() && idx + 2 < frame2.len() {
                let diff_r = (i16::from(frame1[idx]) - i16::from(frame2[idx])).abs();
                let diff_g = (i16::from(frame1[idx + 1]) - i16::from(frame2[idx + 1])).abs();
                let diff_b = (i16::from(frame1[idx + 2]) - i16::from(frame2[idx + 2])).abs();

                let diff = (diff_r + diff_g + diff_b) / 3;

                if f32::from(diff) > self.motion_threshold {
                    motion_pixels += 1;
                }
            }
        }

        motion_pixels as f32 / total_pixels as f32
    }
}

/// LED marker detector for coded light patterns
pub struct LedMarkerDetector {
    /// Expected LED color (RGB, 0-1 range)
    pub expected_color: [f32; 3],
    /// Color tolerance
    pub color_tolerance: f32,
    /// Minimum blob size (pixels)
    pub min_blob_size: usize,
}

impl Default for LedMarkerDetector {
    fn default() -> Self {
        Self {
            expected_color: [1.0, 0.0, 0.0], // Red LED
            color_tolerance: 0.2,
            min_blob_size: 10,
        }
    }
}

impl LedMarkerDetector {
    /// Create a new LED marker detector
    #[must_use]
    pub fn new(color: [f32; 3], tolerance: f32) -> Self {
        Self {
            expected_color: color,
            color_tolerance: tolerance,
            min_blob_size: 10,
        }
    }

    /// Detect LED markers in frame
    #[must_use]
    pub fn detect(&self, frame: &[u8], width: usize, height: usize) -> Vec<SyncMarker> {
        let mut markers = Vec::new();

        // Simple blob detection
        let mut visited = vec![false; width * height];

        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                if !visited[idx] && self.is_led_color(frame, width, x, y) {
                    let blob = self.flood_fill(frame, width, height, x, y, &mut visited);

                    if blob.len() >= self.min_blob_size {
                        let center = self.compute_centroid(&blob);
                        markers.push(SyncMarker::new(0, MarkerType::LedMarker, 1.0, Some(center)));
                    }
                }
            }
        }

        markers
    }

    /// Check if pixel matches expected LED color
    fn is_led_color(&self, frame: &[u8], width: usize, x: usize, y: usize) -> bool {
        let idx = (y * width + x) * 3;
        if idx + 2 >= frame.len() {
            return false;
        }

        let r = f32::from(frame[idx]) / 255.0;
        let g = f32::from(frame[idx + 1]) / 255.0;
        let b = f32::from(frame[idx + 2]) / 255.0;

        let diff_r = (r - self.expected_color[0]).abs();
        let diff_g = (g - self.expected_color[1]).abs();
        let diff_b = (b - self.expected_color[2]).abs();

        diff_r < self.color_tolerance
            && diff_g < self.color_tolerance
            && diff_b < self.color_tolerance
    }

    /// Flood fill to find connected component
    fn flood_fill(
        &self,
        frame: &[u8],
        width: usize,
        height: usize,
        start_x: usize,
        start_y: usize,
        visited: &mut [bool],
    ) -> Vec<Point2D> {
        let mut blob = Vec::new();
        let mut stack = vec![(start_x, start_y)];

        while let Some((x, y)) = stack.pop() {
            let idx = y * width + x;

            if visited[idx] {
                continue;
            }

            if !self.is_led_color(frame, width, x, y) {
                continue;
            }

            visited[idx] = true;
            blob.push(Point2D::new(x as f64, y as f64));

            // Add neighbors
            if x > 0 {
                stack.push((x - 1, y));
            }
            if x + 1 < width {
                stack.push((x + 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if y + 1 < height {
                stack.push((x, y + 1));
            }
        }

        blob
    }

    /// Compute centroid of blob
    fn compute_centroid(&self, blob: &[Point2D]) -> Point2D {
        let n = blob.len() as f64;
        let sum_x: f64 = blob.iter().map(|p| p.x).sum();
        let sum_y: f64 = blob.iter().map(|p| p.y).sum();

        Point2D::new(sum_x / n, sum_y / n)
    }
}

/// Audio spike detector for sharp transients
pub struct AudioSpikeDetector {
    /// Threshold for spike detection (0.0 to 1.0)
    pub threshold: f32,
    /// Window size for analysis
    pub window_size: usize,
}

impl Default for AudioSpikeDetector {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            window_size: 512,
        }
    }
}

impl AudioSpikeDetector {
    /// Create a new audio spike detector
    #[must_use]
    pub fn new(threshold: f32, window_size: usize) -> Self {
        Self {
            threshold,
            window_size,
        }
    }

    /// Detect audio spikes in signal
    #[must_use]
    pub fn detect(&self, audio: &[f32], sample_rate: u32) -> Vec<SyncMarker> {
        let mut markers = Vec::new();

        // Compute envelope
        let envelope = self.compute_envelope(audio);

        // Find peaks
        for i in 1..envelope.len().saturating_sub(1) {
            if envelope[i] > self.threshold
                && envelope[i] > envelope[i - 1]
                && envelope[i] > envelope[i + 1]
            {
                // Convert sample to frame (assuming 24fps)
                let frame = (i * 24) / sample_rate as usize;

                markers.push(SyncMarker::new(
                    frame,
                    MarkerType::AudioSpike,
                    envelope[i],
                    None,
                ));
            }
        }

        markers
    }

    /// Compute audio envelope
    fn compute_envelope(&self, audio: &[f32]) -> Vec<f32> {
        let mut envelope = Vec::new();

        for chunk in audio.chunks(self.window_size) {
            let max = chunk.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
            envelope.push(max);
        }

        envelope
    }
}

/// Timecode display detector (OCR-based)
#[derive(Default)]
pub struct TimecodeDetector {
    /// Region where timecode is expected
    pub region: Option<Region>,
}

impl TimecodeDetector {
    /// Create a new timecode detector
    #[must_use]
    pub fn new(region: Option<Region>) -> Self {
        Self { region }
    }

    /// Detect timecode in frame (simplified - just checks for presence)
    #[must_use]
    pub fn detect(&self, frame: &[u8], width: usize, height: usize) -> Option<SyncMarker> {
        let region = self
            .region
            .unwrap_or_else(|| Region::new(0, height.saturating_sub(100), width, 100));

        // Simplified: check for high contrast in expected region
        let contrast = self.compute_contrast(frame, width, &region);

        if contrast > 0.5 {
            Some(SyncMarker::new(
                0,
                MarkerType::TimecodeDisplay,
                contrast,
                Some(Point2D::new(
                    (region.x + region.width / 2) as f64,
                    (region.y + region.height / 2) as f64,
                )),
            ))
        } else {
            None
        }
    }

    /// Compute contrast in region
    fn compute_contrast(&self, frame: &[u8], width: usize, region: &Region) -> f32 {
        let mut min_val = 255u8;
        let mut max_val = 0u8;

        for y in region.y..region.y + region.height {
            for x in region.x..region.x + region.width {
                let idx = (y * width + x) * 3;
                if idx < frame.len() {
                    let gray = ((u16::from(frame[idx])
                        + u16::from(frame[idx + 1])
                        + u16::from(frame[idx + 2]))
                        / 3) as u8;
                    min_val = min_val.min(gray);
                    max_val = max_val.max(gray);
                }
            }
        }

        f32::from(max_val - min_val) / 255.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Automatic marker interpolation
// ─────────────────────────────────────────────────────────────────────────────

/// Method used for interpolating frame positions between known markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolationMethod {
    /// Linear interpolation between neighbouring markers
    Linear,
    /// Cubic Hermite (Catmull-Rom) spline – needs ≥ 4 control points for full
    /// smoothness; falls back to linear at the ends
    Cubic,
    /// Cubic Bézier with automatic control-point placement (smooth tangents)
    Bezier,
}

/// Interpolate virtual marker positions between a set of known markers.
///
/// The returned `Vec<SyncMarker>` contains one synthetic marker for each frame
/// in `[first_frame, last_frame]`.  Confidence values are linearly blended
/// between the anchor points.
///
/// # Panics
/// Does not panic, but returns an empty vector when fewer than 2 markers are
/// supplied or when the frame range is empty.
#[allow(dead_code)]
#[must_use]
pub fn interpolate_markers(anchors: &[SyncMarker], method: InterpolationMethod) -> Vec<SyncMarker> {
    if anchors.len() < 2 {
        return Vec::new();
    }

    // Sort anchors by frame
    let mut sorted = anchors.to_vec();
    sorted.sort_by_key(|m| m.frame);

    let first = sorted[0].frame;
    let last = sorted[sorted.len() - 1].frame;
    if first >= last {
        return Vec::new();
    }

    let total = last - first + 1;
    let mut result = Vec::with_capacity(total);

    // Build frame positions (f64) and confidence values for interpolation
    let xs: Vec<f64> = sorted.iter().map(|m| m.frame as f64).collect();
    let cs: Vec<f64> = sorted.iter().map(|m| f64::from(m.confidence)).collect();
    // Location x/y (use NaN when absent)
    let lx: Vec<f64> = sorted
        .iter()
        .map(|m| m.location.map_or(f64::NAN, |p| p.x))
        .collect();
    let ly: Vec<f64> = sorted
        .iter()
        .map(|m| m.location.map_or(f64::NAN, |p| p.y))
        .collect();

    let marker_type = sorted[0].marker_type;

    for frame in first..=last {
        let t = frame as f64;

        let (conf, px, py) = match method {
            InterpolationMethod::Linear => {
                let (c, x, y) = interpolate_linear(&xs, &cs, &lx, &ly, t);
                (c, x, y)
            }
            InterpolationMethod::Cubic => {
                let (c, x, y) = interpolate_cubic(&xs, &cs, &lx, &ly, t);
                (c, x, y)
            }
            InterpolationMethod::Bezier => {
                let (c, x, y) = interpolate_bezier(&xs, &cs, &lx, &ly, t);
                (c, x, y)
            }
        };

        let location = if px.is_finite() && py.is_finite() {
            Some(Point2D::new(px, py))
        } else {
            None
        };

        result.push(SyncMarker::new(frame, marker_type, conf as f32, location));
    }

    result
}

/// Linear piecewise interpolation helper.
fn interpolate_linear(xs: &[f64], cs: &[f64], lx: &[f64], ly: &[f64], t: f64) -> (f64, f64, f64) {
    // Find surrounding segment
    for i in 0..xs.len().saturating_sub(1) {
        if t >= xs[i] && t <= xs[i + 1] {
            let alpha = (t - xs[i]) / (xs[i + 1] - xs[i]);
            let c = cs[i] + alpha * (cs[i + 1] - cs[i]);
            let x = lerp_nan(lx[i], lx[i + 1], alpha);
            let y = lerp_nan(ly[i], ly[i + 1], alpha);
            return (c, x, y);
        }
    }
    (cs[cs.len() - 1], lx[lx.len() - 1], ly[ly.len() - 1])
}

/// Catmull-Rom cubic spline helper.
fn interpolate_cubic(xs: &[f64], cs: &[f64], lx: &[f64], ly: &[f64], t: f64) -> (f64, f64, f64) {
    if xs.len() < 4 {
        return interpolate_linear(xs, cs, lx, ly, t);
    }

    for i in 0..xs.len().saturating_sub(1) {
        if t >= xs[i] && t <= xs[i + 1] {
            let alpha = (t - xs[i]) / (xs[i + 1] - xs[i]);

            let i0 = i.saturating_sub(1);
            let i1 = i;
            let i2 = (i + 1).min(xs.len() - 1);
            let i3 = (i + 2).min(xs.len() - 1);

            let c = catmull_rom(cs[i0], cs[i1], cs[i2], cs[i3], alpha);
            let x = catmull_rom_nan(lx[i0], lx[i1], lx[i2], lx[i3], alpha);
            let y = catmull_rom_nan(ly[i0], ly[i1], ly[i2], ly[i3], alpha);
            return (c.clamp(0.0, 1.0), x, y);
        }
    }
    (cs[cs.len() - 1], lx[lx.len() - 1], ly[ly.len() - 1])
}

/// Bézier-based interpolation using automatic control points (Catmull-Rom
/// tangents mapped into Bézier form).  For 2 anchors this degenerates to
/// linear.
fn interpolate_bezier(xs: &[f64], cs: &[f64], lx: &[f64], ly: &[f64], t: f64) -> (f64, f64, f64) {
    if xs.len() < 3 {
        return interpolate_linear(xs, cs, lx, ly, t);
    }

    for i in 0..xs.len().saturating_sub(1) {
        if t >= xs[i] && t <= xs[i + 1] {
            let alpha = (t - xs[i]) / (xs[i + 1] - xs[i]);

            // Control-point tangents (Catmull-Rom → Bézier conversion)
            let prev_c = if i == 0 { cs[i] } else { cs[i - 1] };
            let next_c = if i + 2 < cs.len() {
                cs[i + 2]
            } else {
                cs[i + 1]
            };
            let cp1_c = cs[i] + (cs[i + 1] - prev_c) / 6.0;
            let cp2_c = cs[i + 1] - (next_c - cs[i]) / 6.0;
            let c = cubic_bezier(cs[i], cp1_c, cp2_c, cs[i + 1], alpha).clamp(0.0, 1.0);

            let x = bezier_nan(lx, i, alpha);
            let y = bezier_nan(ly, i, alpha);
            return (c, x, y);
        }
    }
    (cs[cs.len() - 1], lx[lx.len() - 1], ly[ly.len() - 1])
}

// Helpers

fn lerp_nan(a: f64, b: f64, t: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a + t * (b - a)
    }
}

fn catmull_rom(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}

fn catmull_rom_nan(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    if p0.is_nan() || p1.is_nan() || p2.is_nan() || p3.is_nan() {
        f64::NAN
    } else {
        catmull_rom(p0, p1, p2, p3, t)
    }
}

fn cubic_bezier(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let mt = 1.0 - t;
    mt * mt * mt * p0 + 3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t * p3
}

fn bezier_nan(vals: &[f64], i: usize, t: f64) -> f64 {
    if vals[i].is_nan() || vals[i + 1].is_nan() {
        f64::NAN
    } else {
        let prev = if i == 0 { vals[i] } else { vals[i - 1] };
        let next = if i + 2 < vals.len() {
            vals[i + 2]
        } else {
            vals[i + 1]
        };
        let cp1 = vals[i] + (vals[i + 1] - prev) / 6.0;
        let cp2 = vals[i + 1] - (next - vals[i]) / 6.0;
        cubic_bezier(vals[i], cp1, cp2, vals[i + 1], t)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Marker clustering
// ─────────────────────────────────────────────────────────────────────────────

/// Cluster nearby markers into groups based on temporal proximity.
///
/// Markers within `max_gap_frames` of each other are placed in the same
/// cluster.  Returns a `Vec` of clusters, each cluster being a `Vec<SyncMarker>`.
#[allow(dead_code)]
#[must_use]
pub fn cluster_markers(markers: &[SyncMarker], max_gap_frames: usize) -> Vec<Vec<SyncMarker>> {
    if markers.is_empty() {
        return Vec::new();
    }

    let mut sorted = markers.to_vec();
    sorted.sort_by_key(|m| m.frame);

    let mut clusters: Vec<Vec<SyncMarker>> = Vec::new();
    let mut last_frame = sorted[0].frame;
    let mut current: Vec<SyncMarker> = vec![sorted[0].clone()];

    for m in sorted.into_iter().skip(1) {
        // `last_frame` is tracked alongside `current` instead of re-derived
        // via `current.last()`, so there is no panic path if `current` were
        // ever empty.
        if m.frame.saturating_sub(last_frame) <= max_gap_frames {
            last_frame = m.frame;
            current.push(m);
        } else {
            clusters.push(current);
            last_frame = m.frame;
            current = vec![m];
        }
    }
    clusters.push(current);

    clusters
}

/// Return the best marker from each cluster (highest confidence).
#[allow(dead_code)]
#[must_use]
pub fn cluster_best_markers(markers: &[SyncMarker], max_gap_frames: usize) -> Vec<SyncMarker> {
    cluster_markers(markers, max_gap_frames)
        .into_iter()
        .filter_map(|cluster| {
            cluster.into_iter().max_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Temporal marker alignment
// ─────────────────────────────────────────────────────────────────────────────

/// The result of aligning two marker sequences temporally.
#[derive(Debug, Clone)]
pub struct TemporalAlignment {
    /// Frame offset to apply to the second stream (add to stream-B frames)
    pub frame_offset: i64,
    /// Confidence of the alignment (average pairwise confidence)
    pub confidence: f32,
    /// Number of matched marker pairs used
    pub matched_pairs: usize,
}

/// Align two sets of markers temporally by finding the frame offset that
/// maximises the number of close marker pairs.
///
/// `tolerance_frames` is the maximum frame difference for two markers to be
/// considered a match.
///
/// Returns `None` when no alignment can be found (e.g., empty inputs).
#[allow(dead_code)]
#[must_use]
pub fn align_markers_temporal(
    reference: &[SyncMarker],
    target: &[SyncMarker],
    tolerance_frames: usize,
    search_range: i64,
) -> Option<TemporalAlignment> {
    if reference.is_empty() || target.is_empty() {
        return None;
    }

    let mut best_offset = 0i64;
    let mut best_matches = 0usize;
    let mut best_conf = 0.0f32;

    let mut best_total_dist = u64::MAX;

    for delta in -search_range..=search_range {
        let mut matches = 0usize;
        let mut conf_sum = 0.0f32;
        let mut total_dist = 0u64;

        for ref_marker in reference {
            let shifted_frame = ref_marker.frame as i64 - delta;
            // Find closest target marker to shifted_frame
            if let Some(closest) = target
                .iter()
                .min_by_key(|m| (m.frame as i64 - shifted_frame).unsigned_abs())
            {
                let dist = (closest.frame as i64 - shifted_frame).unsigned_abs() as usize;
                if dist <= tolerance_frames {
                    matches += 1;
                    conf_sum += ref_marker.confidence * closest.confidence;
                    total_dist += dist as u64;
                }
            }
        }

        let is_better = matches > best_matches
            || (matches == best_matches && conf_sum > best_conf)
            || (matches == best_matches
                && (conf_sum - best_conf).abs() < 1e-6
                && total_dist < best_total_dist);

        if is_better {
            best_matches = matches;
            best_conf = conf_sum;
            best_total_dist = total_dist;
            best_offset = delta;
        }
    }

    if best_matches == 0 {
        return None;
    }

    let avg_conf = best_conf / best_matches as f32;

    Some(TemporalAlignment {
        frame_offset: best_offset,
        confidence: avg_conf,
        matched_pairs: best_matches,
    })
}

/// Multi-marker synchronizer that combines different marker types
pub struct MultiMarkerSync {
    /// Flash detector
    flash: FlashDetector,
    /// Clapper detector
    clapper: ClapperDetector,
    /// Audio spike detector
    audio: AudioSpikeDetector,
}

impl Default for MultiMarkerSync {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiMarkerSync {
    /// Create a new multi-marker synchronizer
    #[must_use]
    pub fn new() -> Self {
        Self {
            flash: FlashDetector::default(),
            clapper: ClapperDetector::default(),
            audio: AudioSpikeDetector::default(),
        }
    }

    /// Detect all marker types
    ///
    /// # Errors
    /// Returns error if detection fails
    pub fn detect_all(
        &self,
        video_frames: &[&[u8]],
        width: usize,
        height: usize,
        audio: &[f32],
        sample_rate: u32,
    ) -> AlignResult<Vec<SyncMarker>> {
        let mut markers = Vec::new();

        // Detect visual markers
        markers.extend(self.flash.detect(video_frames, width, height));
        markers.extend(self.clapper.detect(video_frames, width, height)?);

        // Detect audio markers
        markers.extend(self.audio.detect(audio, sample_rate));

        // Sort by frame and confidence
        markers.sort_by(|a, b| {
            a.frame.cmp(&b.frame).then_with(|| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        Ok(markers)
    }

    /// Find the best sync marker (highest confidence)
    #[must_use]
    pub fn find_best_marker<'a>(&self, markers: &'a [SyncMarker]) -> Option<&'a SyncMarker> {
        markers.iter().max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Interpolation tests ───────────────────────────────────────────────

    #[test]
    fn test_interpolate_linear_count() {
        let anchors = vec![
            SyncMarker::new(0, MarkerType::Flash, 0.8, None),
            SyncMarker::new(10, MarkerType::Flash, 0.6, None),
        ];
        let result = interpolate_markers(&anchors, InterpolationMethod::Linear);
        assert_eq!(result.len(), 11); // frames 0..=10
    }

    #[test]
    fn test_interpolate_linear_endpoints() {
        let anchors = vec![
            SyncMarker::new(0, MarkerType::Flash, 1.0, None),
            SyncMarker::new(10, MarkerType::Flash, 0.0, None),
        ];
        let result = interpolate_markers(&anchors, InterpolationMethod::Linear);
        // First confidence should be ≈ 1.0
        assert!((result[0].confidence - 1.0).abs() < 1e-5);
        // Last confidence should be ≈ 0.0
        assert!((result[10].confidence).abs() < 1e-5);
    }

    #[test]
    fn test_interpolate_cubic_count() {
        let anchors = vec![
            SyncMarker::new(0, MarkerType::Flash, 1.0, None),
            SyncMarker::new(5, MarkerType::Flash, 0.8, None),
            SyncMarker::new(10, MarkerType::Flash, 0.9, None),
            SyncMarker::new(15, MarkerType::Flash, 0.6, None),
        ];
        let result = interpolate_markers(&anchors, InterpolationMethod::Cubic);
        assert_eq!(result.len(), 16);
    }

    #[test]
    fn test_interpolate_bezier_count() {
        let anchors = vec![
            SyncMarker::new(0, MarkerType::Flash, 0.9, None),
            SyncMarker::new(4, MarkerType::Flash, 0.7, None),
            SyncMarker::new(8, MarkerType::Flash, 0.8, None),
        ];
        let result = interpolate_markers(&anchors, InterpolationMethod::Bezier);
        assert_eq!(result.len(), 9);
    }

    #[test]
    fn test_interpolate_empty_returns_empty() {
        let result = interpolate_markers(&[], InterpolationMethod::Linear);
        assert!(result.is_empty());
    }

    #[test]
    fn test_interpolate_single_returns_empty() {
        let result = interpolate_markers(
            &[SyncMarker::new(5, MarkerType::Flash, 0.9, None)],
            InterpolationMethod::Linear,
        );
        assert!(result.is_empty());
    }

    #[test]
    fn test_interpolate_with_locations() {
        let loc_a = Some(Point2D::new(0.0, 0.0));
        let loc_b = Some(Point2D::new(10.0, 20.0));
        let anchors = vec![
            SyncMarker::new(0, MarkerType::Flash, 1.0, loc_a),
            SyncMarker::new(10, MarkerType::Flash, 1.0, loc_b),
        ];
        let result = interpolate_markers(&anchors, InterpolationMethod::Linear);
        let mid = &result[5];
        let loc = mid.location.expect("loc should be valid");
        assert!((loc.x - 5.0).abs() < 0.5);
        assert!((loc.y - 10.0).abs() < 1.0);
    }

    // ── Clustering tests ──────────────────────────────────────────────────

    #[test]
    fn test_cluster_markers_two_clusters() {
        let markers = vec![
            SyncMarker::new(0, MarkerType::Flash, 0.9, None),
            SyncMarker::new(2, MarkerType::Flash, 0.8, None),
            SyncMarker::new(100, MarkerType::Flash, 0.7, None),
            SyncMarker::new(102, MarkerType::Flash, 0.6, None),
        ];
        let clusters = cluster_markers(&markers, 5);
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn test_cluster_markers_one_cluster() {
        let markers = vec![
            SyncMarker::new(0, MarkerType::Flash, 0.9, None),
            SyncMarker::new(3, MarkerType::Flash, 0.8, None),
            SyncMarker::new(6, MarkerType::Flash, 0.7, None),
        ];
        let clusters = cluster_markers(&markers, 5);
        assert_eq!(clusters.len(), 1);
    }

    #[test]
    fn test_cluster_markers_empty() {
        let clusters = cluster_markers(&[], 5);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_cluster_best_markers_picks_highest_confidence() {
        let markers = vec![
            SyncMarker::new(0, MarkerType::Flash, 0.5, None),
            SyncMarker::new(2, MarkerType::Flash, 0.9, None),
            SyncMarker::new(100, MarkerType::Flash, 0.3, None),
        ];
        let best = cluster_best_markers(&markers, 5);
        assert_eq!(best.len(), 2);
        // First cluster best should be the 0.9 marker
        assert!((best[0].confidence - 0.9).abs() < 1e-5);
    }

    #[test]
    fn test_cluster_markers_single() {
        let markers = vec![SyncMarker::new(42, MarkerType::AudioSpike, 1.0, None)];
        let clusters = cluster_markers(&markers, 5);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].len(), 1);
    }

    // ── Temporal alignment tests ──────────────────────────────────────────

    #[test]
    fn test_align_markers_perfect_match() {
        let reference = vec![
            SyncMarker::new(10, MarkerType::Flash, 1.0, None),
            SyncMarker::new(50, MarkerType::Flash, 1.0, None),
        ];
        // Target is shifted by +5 frames
        let target = vec![
            SyncMarker::new(15, MarkerType::Flash, 1.0, None),
            SyncMarker::new(55, MarkerType::Flash, 1.0, None),
        ];
        let alignment =
            align_markers_temporal(&reference, &target, 2, 20).expect("alignment should be valid");
        assert_eq!(alignment.frame_offset, -5);
        assert_eq!(alignment.matched_pairs, 2);
    }

    #[test]
    fn test_align_markers_no_match() {
        let reference = vec![SyncMarker::new(0, MarkerType::Flash, 1.0, None)];
        let target = vec![SyncMarker::new(1000, MarkerType::Flash, 1.0, None)];
        // Search range too small to find the match
        let result = align_markers_temporal(&reference, &target, 2, 10);
        assert!(result.is_none());
    }

    #[test]
    fn test_align_markers_empty_inputs() {
        let reference: Vec<SyncMarker> = vec![];
        let target = vec![SyncMarker::new(10, MarkerType::Flash, 1.0, None)];
        assert!(align_markers_temporal(&reference, &target, 5, 20).is_none());
    }

    #[test]
    fn test_align_markers_confidence_nonzero() {
        let reference = vec![SyncMarker::new(5, MarkerType::Flash, 0.8, None)];
        let target = vec![SyncMarker::new(5, MarkerType::Flash, 0.9, None)];
        let result =
            align_markers_temporal(&reference, &target, 1, 5).expect("result should be valid");
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_temporal_alignment_fields() {
        let reference = vec![SyncMarker::new(0, MarkerType::Flash, 1.0, None)];
        let target = vec![SyncMarker::new(3, MarkerType::Flash, 1.0, None)];
        let result =
            align_markers_temporal(&reference, &target, 5, 10).expect("result should be valid");
        assert_eq!(result.matched_pairs, 1);
        assert!(result.confidence > 0.0);
    }

    // ── Original tests ────────────────────────────────────────────────────

    #[test]
    fn test_sync_marker_creation() {
        let marker = SyncMarker::new(100, MarkerType::Flash, 0.95, None);
        assert_eq!(marker.frame, 100);
        assert_eq!(marker.marker_type, MarkerType::Flash);
        assert_eq!(marker.confidence, 0.95);
    }

    #[test]
    fn test_flash_detector() {
        let detector = FlashDetector::default();
        assert_eq!(detector.threshold, 0.8);
        assert_eq!(detector.min_duration, 1);
    }

    #[test]
    fn test_brightness_computation() {
        let detector = FlashDetector::default();
        let frame = vec![255u8; 300]; // White frame
        let brightness = detector.compute_brightness(&frame, 10, 10);
        assert!((brightness - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_region_creation() {
        let region = Region::new(10, 20, 100, 200);
        assert_eq!(region.x, 10);
        assert_eq!(region.y, 20);
        assert_eq!(region.width, 100);
        assert_eq!(region.height, 200);
    }

    #[test]
    fn test_clapper_detector() {
        let detector = ClapperDetector::default();
        let frame1 = vec![100u8; 300];
        let frame2 = vec![200u8; 300];
        let motion = detector.compute_motion(&frame1, &frame2, 10, 10);
        assert!(motion > 0.0);
    }

    #[test]
    fn test_led_marker_detector() {
        let detector = LedMarkerDetector::new([1.0, 0.0, 0.0], 0.2);
        assert_eq!(detector.expected_color[0], 1.0);
        assert_eq!(detector.color_tolerance, 0.2);
    }

    #[test]
    fn test_audio_spike_detector() {
        // Use a small window_size so the envelope has enough elements for peak detection
        let detector = AudioSpikeDetector::new(0.8, 50);
        let mut audio = vec![0.0f32; 1000];
        audio[500] = 1.0; // Spike at sample 500
        let markers = detector.detect(&audio, 48000);
        assert!(!markers.is_empty());
    }

    #[test]
    fn test_audio_envelope() {
        let detector = AudioSpikeDetector::new(0.5, 100);
        let audio = vec![0.5f32; 1000];
        let envelope = detector.compute_envelope(&audio);
        assert!(!envelope.is_empty());
        assert!((envelope[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_multi_marker_sync() {
        let sync = MultiMarkerSync::new();
        let marker1 = SyncMarker::new(100, MarkerType::Flash, 0.8, None);
        let marker2 = SyncMarker::new(101, MarkerType::AudioSpike, 0.9, None);
        let markers = vec![marker1, marker2];

        let best = sync.find_best_marker(&markers);
        assert!(best.is_some());
        assert_eq!(best.expect("test expectation failed").confidence, 0.9);
    }
}

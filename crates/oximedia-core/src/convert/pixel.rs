//! Pixel format conversion functions.
//!
//! This module provides functions to convert between different pixel formats,
//! including YUV and RGB color spaces with proper color matrix support.
#![allow(
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::trivially_copy_pass_by_ref
)]

/// Color matrix standard for YUV<->RGB conversion.
///
/// Different standards define different coefficients for converting between
/// YUV and RGB color spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMatrix {
    /// ITU-R BT.601 standard (Standard Definition TV).
    ///
    /// Used for SD content (480p, 576p).
    Bt601,

    /// ITU-R BT.709 standard (High Definition TV).
    ///
    /// Used for HD content (720p, 1080p).
    Bt709,
}

impl ColorMatrix {
    /// Returns the Y coefficient for RGB to YUV conversion.
    const fn kr(&self) -> f32 {
        match self {
            Self::Bt601 => 0.299,
            Self::Bt709 => 0.2126,
        }
    }

    /// Returns the Y coefficient for RGB to YUV conversion.
    const fn kb(&self) -> f32 {
        match self {
            Self::Bt601 => 0.114,
            Self::Bt709 => 0.0722,
        }
    }

    /// Returns the derived green coefficient.
    fn kg(&self) -> f32 {
        1.0 - self.kr() - self.kb()
    }
}

/// Pixel format converter with pre-computed lookup tables.
///
/// This struct provides efficient conversion using pre-computed tables
/// to avoid repeated floating-point calculations.
#[derive(Clone, Debug)]
pub struct PixelConverter {
    /// Lookup table for Y to RGB conversion.
    y_table: [i32; 256],
    /// Lookup table for U to R conversion.
    u_r_table: [i32; 256],
    /// Lookup table for U to G conversion.
    u_g_table: [i32; 256],
    /// Lookup table for V to G conversion.
    v_g_table: [i32; 256],
    /// Lookup table for V to B conversion.
    v_b_table: [i32; 256],
    /// RGB to Y coefficients (scaled).
    r_y: i32,
    /// RGB to Y coefficients (scaled).
    g_y: i32,
    /// RGB to Y coefficients (scaled).
    b_y: i32,
    /// RGB to U coefficients (scaled).
    r_u: i32,
    /// RGB to U coefficients (scaled).
    g_u: i32,
    /// RGB to U coefficients (scaled).
    b_u: i32,
    /// RGB to V coefficients (scaled).
    r_v: i32,
    /// RGB to V coefficients (scaled).
    g_v: i32,
    /// RGB to V coefficients (scaled).
    b_v: i32,
}

impl PixelConverter {
    /// Creates a new pixel converter for the specified color matrix.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::convert::pixel::{PixelConverter, ColorMatrix};
    ///
    /// let converter = PixelConverter::new(ColorMatrix::Bt709);
    /// ```
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn new(matrix: ColorMatrix) -> Self {
        let kr = matrix.kr();
        let kb = matrix.kb();
        let kg = matrix.kg();

        // Pre-compute YUV to RGB tables
        let mut y_table = [0i32; 256];
        let mut u_r_table = [0i32; 256];
        let mut u_g_table = [0i32; 256];
        let mut v_g_table = [0i32; 256];
        let mut v_b_table = [0i32; 256];

        for i in 0..256 {
            let y = i as f32 - 16.0;
            let u = i as f32 - 128.0;
            let v = i as f32 - 128.0;

            y_table[i] = (y * 1.164).round() as i32;
            u_r_table[i] = (v * 2.0 * (1.0 - kr)).round() as i32;
            v_b_table[i] = (u * 2.0 * (1.0 - kb)).round() as i32;
            u_g_table[i] = (u * 2.0 * (1.0 - kb) * kb / kg).round() as i32;
            v_g_table[i] = (v * 2.0 * (1.0 - kr) * kr / kg).round() as i32;
        }

        // Pre-compute RGB to YUV coefficients (scaled by 65536 for fixed-point)
        let scale = 65536.0;
        let r_y = (kr * scale).round() as i32;
        let g_y = (kg * scale).round() as i32;
        let b_y = (kb * scale).round() as i32;

        let r_u = ((-0.5 * kr / (1.0 - kb)) * scale).round() as i32;
        let g_u = ((-0.5 * kg / (1.0 - kb)) * scale).round() as i32;
        let b_u = ((0.5) * scale).round() as i32;

        let r_v = ((0.5) * scale).round() as i32;
        let g_v = ((-0.5 * kg / (1.0 - kr)) * scale).round() as i32;
        let b_v = ((-0.5 * kb / (1.0 - kr)) * scale).round() as i32;

        Self {
            y_table,
            u_r_table,
            u_g_table,
            v_g_table,
            v_b_table,
            r_y,
            g_y,
            b_y,
            r_u,
            g_u,
            b_u,
            r_v,
            g_v,
            b_v,
        }
    }

    /// Converts a single YUV pixel to RGB.
    ///
    /// Returns (R, G, B) in range [0, 255].
    #[must_use]
    #[inline]
    #[allow(clippy::too_many_lines, clippy::similar_names)]
    pub fn yuv_to_rgb(&self, y: u8, u: u8, v: u8) -> (u8, u8, u8) {
        let y_val = self.y_table[y as usize];
        let u_r = self.u_r_table[v as usize];
        let u_g = self.u_g_table[u as usize];
        let v_g = self.v_g_table[v as usize];
        let v_b = self.v_b_table[u as usize];

        let r = (y_val + u_r).clamp(0, 255);
        let g = (y_val - u_g - v_g).clamp(0, 255);
        let b = (y_val + v_b).clamp(0, 255);

        (r as u8, g as u8, b as u8)
    }

    /// Converts a single RGB pixel to YUV.
    ///
    /// Returns (Y, U, V) in range [0, 255].
    #[must_use]
    #[inline]
    #[allow(clippy::similar_names)]
    pub fn rgb_to_yuv(&self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let r = i32::from(r);
        let g = i32::from(g);
        let b = i32::from(b);

        let y = ((r * self.r_y + g * self.g_y + b * self.b_y) >> 16) + 16;
        let u = ((r * self.r_u + g * self.g_u + b * self.b_u) >> 16) + 128;
        let v = ((r * self.r_v + g * self.g_v + b * self.b_v) >> 16) + 128;

        (
            y.clamp(0, 255) as u8,
            u.clamp(0, 255) as u8,
            v.clamp(0, 255) as u8,
        )
    }
}

impl Default for PixelConverter {
    fn default() -> Self {
        Self::new(ColorMatrix::Bt709)
    }
}

/// Converts `YUV420p` to RGB24.
///
/// `YUV420p` is a planar format with Y plane at full resolution and U/V planes
/// at half resolution in both dimensions.
///
/// # Arguments
///
/// * `y_plane` - Y (luma) plane data
/// * `u_plane` - U (chroma) plane data (`div_ceil(width, 2) * div_ceil(height, 2)`)
/// * `v_plane` - V (chroma) plane data (`div_ceil(width, 2) * div_ceil(height, 2)`)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `matrix` - Color matrix to use for conversion
///
/// # Returns
///
/// RGB24 packed data (width * height * 3 bytes)
///
/// # Panics
///
/// Panics if input planes have incorrect sizes. Chroma plane sizes are
/// ceiling-divided (VP8/WebP odd-dimension convention:
/// `div_ceil(width, 2) * div_ceil(height, 2)`), not floor-divided -- for
/// even `width`/`height` this is identical to `(width / 2) * (height / 2)`.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::{yuv420p_to_rgb24, ColorMatrix};
///
/// let width: usize = 4;
/// let height: usize = 4;
/// let y_plane = vec![128u8; width * height];
/// let u_plane = vec![128u8; width.div_ceil(2) * height.div_ceil(2)];
/// let v_plane = vec![128u8; width.div_ceil(2) * height.div_ceil(2)];
///
/// let rgb = yuv420p_to_rgb24(&y_plane, &u_plane, &v_plane, width, height, ColorMatrix::Bt709);
/// assert_eq!(rgb.len(), width * height * 3);
/// ```
#[must_use]
pub fn yuv420p_to_rgb24(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    width: usize,
    height: usize,
    matrix: ColorMatrix,
) -> Vec<u8> {
    // 4:2:0 chroma is ceiling-divided (VP8/WebP odd-dimension convention),
    // not floor-divided: identical to floor division for even width/height.
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    assert_eq!(
        y_plane.len(),
        width * height,
        "y_plane length must equal width * height"
    );
    assert_eq!(
        u_plane.len(),
        chroma_width * chroma_height,
        "u_plane length must equal the ceil-divided 4:2:0 chroma size \
         div_ceil(width, 2) * div_ceil(height, 2)"
    );
    assert_eq!(
        v_plane.len(),
        chroma_width * chroma_height,
        "v_plane length must equal the ceil-divided 4:2:0 chroma size \
         div_ceil(width, 2) * div_ceil(height, 2)"
    );

    let converter = PixelConverter::new(matrix);
    let mut rgb = vec![0u8; width * height * 3];

    for y in 0..height {
        for x in 0..width {
            let y_val = y_plane[y * width + x];
            let u_val = u_plane[(y / 2) * chroma_width + (x / 2)];
            let v_val = v_plane[(y / 2) * chroma_width + (x / 2)];

            let (r, g, b) = converter.yuv_to_rgb(y_val, u_val, v_val);

            let offset = (y * width + x) * 3;
            rgb[offset] = r;
            rgb[offset + 1] = g;
            rgb[offset + 2] = b;
        }
    }

    rgb
}

/// Converts RGB24 to `YUV420p`.
///
/// RGB24 is a packed format with R, G, B bytes interleaved.
/// `YUV420p` has separate planes with chroma subsampling.
///
/// # Arguments
///
/// * `rgb` - RGB24 packed data (width * height * 3 bytes)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `matrix` - Color matrix to use for conversion
///
/// # Returns
///
/// Tuple of (Y plane, U plane, V plane). The chroma planes are sized by
/// ceiling division (`div_ceil(2)`) of width and height (VP8/WebP
/// odd-dimension convention) -- identical to floor division for even
/// `width`/`height`.
///
/// # Panics
///
/// Panics if RGB data has incorrect size.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::{rgb24_to_yuv420p, ColorMatrix};
///
/// let width: usize = 4;
/// let height: usize = 4;
/// let rgb = vec![128u8; width * height * 3];
///
/// let (y_plane, u_plane, v_plane) = rgb24_to_yuv420p(&rgb, width, height, ColorMatrix::Bt709);
/// assert_eq!(y_plane.len(), width * height);
/// assert_eq!(u_plane.len(), width.div_ceil(2) * height.div_ceil(2));
/// assert_eq!(v_plane.len(), width.div_ceil(2) * height.div_ceil(2));
/// ```
#[must_use]
#[allow(clippy::similar_names)]
pub fn rgb24_to_yuv420p(
    rgb: &[u8],
    width: usize,
    height: usize,
    matrix: ColorMatrix,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert_eq!(
        rgb.len(),
        width * height * 3,
        "rgb length must equal width * height * 3"
    );

    // 4:2:0 chroma is ceiling-divided (VP8/WebP odd-dimension convention),
    // not floor-divided -- identical to floor division for even
    // width/height. Before this fix, a floor-divided allocation and index
    // stride here panicked out of bounds for any odd width/height (e.g.
    // 3x3: `u_plane[(y / 2) * (width / 2) + x / 2]` indexes 1 into a
    // 1-byte plane once `x` reaches 2).
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);

    let converter = PixelConverter::new(matrix);
    let mut y_plane = vec![0u8; width * height];
    let mut u_plane = vec![0u8; chroma_width * chroma_height];
    let mut v_plane = vec![0u8; chroma_width * chroma_height];

    // Convert RGB to YUV and downsample chroma
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 3;
            let r = rgb[offset];
            let g = rgb[offset + 1];
            let b = rgb[offset + 2];

            let (y_val, u_val, v_val) = converter.rgb_to_yuv(r, g, b);
            y_plane[y * width + x] = y_val;

            // Subsample chroma (average 2x2 blocks). A chroma sample at the
            // right/bottom edge of an odd-width/height frame has no
            // right/bottom neighbor pixel to average, so -- exactly as this
            // loop already did for even dimensions' unreachable edge case --
            // it averages only the pixels that exist via the `x + 1 <
            // width` / `y + 1 < height` guards below; that averaging
            // behavior is unchanged by this fix, only the chroma plane
            // allocation size and index stride (`chroma_width`, ceil- not
            // floor-divided) are.
            if y % 2 == 0 && x % 2 == 0 {
                let mut u_sum = u32::from(u_val);
                let mut v_sum = u32::from(v_val);
                let mut count = 1;

                // Average with neighboring pixels if they exist
                if x + 1 < width {
                    let offset = (y * width + x + 1) * 3;
                    let (_, u, v) =
                        converter.rgb_to_yuv(rgb[offset], rgb[offset + 1], rgb[offset + 2]);
                    u_sum += u32::from(u);
                    v_sum += u32::from(v);
                    count += 1;
                }
                if y + 1 < height {
                    let offset = ((y + 1) * width + x) * 3;
                    let (_, u, v) =
                        converter.rgb_to_yuv(rgb[offset], rgb[offset + 1], rgb[offset + 2]);
                    u_sum += u32::from(u);
                    v_sum += u32::from(v);
                    count += 1;
                }
                if x + 1 < width && y + 1 < height {
                    let offset = ((y + 1) * width + x + 1) * 3;
                    let (_, u, v) =
                        converter.rgb_to_yuv(rgb[offset], rgb[offset + 1], rgb[offset + 2]);
                    u_sum += u32::from(u);
                    v_sum += u32::from(v);
                    count += 1;
                }

                let u_idx = (y / 2) * chroma_width + (x / 2);
                u_plane[u_idx] = (u_sum / count) as u8;
                v_plane[u_idx] = (v_sum / count) as u8;
            }
        }
    }

    (y_plane, u_plane, v_plane)
}

/// Converts `YUV420p` to `YUV444p`.
///
/// `YUV444p` has full chroma resolution (no subsampling).
/// This upsamples the chroma planes using bilinear interpolation.
///
/// # Arguments
///
/// * `y_plane` - Y (luma) plane data
/// * `u_plane` - U (chroma) plane data (`div_ceil(width, 2) * div_ceil(height, 2)`)
/// * `v_plane` - V (chroma) plane data (`div_ceil(width, 2) * div_ceil(height, 2)`)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
///
/// Tuple of (Y plane, U plane, V plane) all at full resolution
///
/// # Panics
///
/// Panics if input planes have incorrect sizes. Chroma plane sizes are
/// ceiling-divided (VP8/WebP odd-dimension convention:
/// `div_ceil(width, 2) * div_ceil(height, 2)`), not floor-divided -- for
/// even `width`/`height` this is identical to `(width / 2) * (height / 2)`.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::yuv420p_to_yuv444p;
///
/// let width: usize = 4;
/// let height: usize = 4;
/// let y_plane = vec![128u8; width * height];
/// let u_plane = vec![100u8; width.div_ceil(2) * height.div_ceil(2)];
/// let v_plane = vec![150u8; width.div_ceil(2) * height.div_ceil(2)];
///
/// let (y_out, u_out, v_out) = yuv420p_to_yuv444p(&y_plane, &u_plane, &v_plane, width, height);
/// assert_eq!(y_out.len(), width * height);
/// assert_eq!(u_out.len(), width * height);
/// assert_eq!(v_out.len(), width * height);
/// ```
#[must_use]
pub fn yuv420p_to_yuv444p(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    width: usize,
    height: usize,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    // 4:2:0 chroma is ceiling-divided (VP8/WebP odd-dimension convention),
    // not floor-divided -- identical to floor division for even
    // width/height.
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    assert_eq!(
        y_plane.len(),
        width * height,
        "y_plane length must equal width * height"
    );
    assert_eq!(
        u_plane.len(),
        chroma_width * chroma_height,
        "u_plane length must equal the ceil-divided 4:2:0 chroma size \
         div_ceil(width, 2) * div_ceil(height, 2)"
    );
    assert_eq!(
        v_plane.len(),
        chroma_width * chroma_height,
        "v_plane length must equal the ceil-divided 4:2:0 chroma size \
         div_ceil(width, 2) * div_ceil(height, 2)"
    );

    let y_out = y_plane.to_vec();
    let mut u_out = vec![0u8; width * height];
    let mut v_out = vec![0u8; width * height];

    // Upsample chroma using bilinear interpolation
    for y in 0..height {
        for x in 0..width {
            let cx = x / 2;
            let cy = y / 2;
            #[allow(clippy::cast_precision_loss)]
            let fx = (x % 2) as f32 * 0.5;
            #[allow(clippy::cast_precision_loss)]
            let fy = (y % 2) as f32 * 0.5;

            let c00_idx = cy * chroma_width + cx;
            let c10_idx = if cx + 1 < chroma_width {
                c00_idx + 1
            } else {
                c00_idx
            };
            let c01_idx = if cy + 1 < chroma_height {
                (cy + 1) * chroma_width + cx
            } else {
                c00_idx
            };
            let c11_idx = if cx + 1 < chroma_width && cy + 1 < chroma_height {
                (cy + 1) * chroma_width + cx + 1
            } else {
                c00_idx
            };

            // Bilinear interpolation for U
            let u00 = f32::from(u_plane[c00_idx]);
            let u10 = f32::from(u_plane[c10_idx]);
            let u01 = f32::from(u_plane[c01_idx]);
            let u11 = f32::from(u_plane[c11_idx]);

            let u_top = u00 * (1.0 - fx) + u10 * fx;
            let u_bottom = u01 * (1.0 - fx) + u11 * fx;
            let u_val = u_top * (1.0 - fy) + u_bottom * fy;

            // Bilinear interpolation for V
            let v00 = f32::from(v_plane[c00_idx]);
            let v10 = f32::from(v_plane[c10_idx]);
            let v01 = f32::from(v_plane[c01_idx]);
            let v11 = f32::from(v_plane[c11_idx]);

            let v_top = v00 * (1.0 - fx) + v10 * fx;
            let v_bottom = v01 * (1.0 - fx) + v11 * fx;
            let v_val = v_top * (1.0 - fy) + v_bottom * fy;

            let out_idx = y * width + x;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                u_out[out_idx] = u_val.round().clamp(0.0, 255.0) as u8;
                v_out[out_idx] = v_val.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    (y_out, u_out, v_out)
}

/// Converts `YUV444p` to `YUV420p`.
///
/// This downsamples the chroma planes by averaging 2x2 blocks.
///
/// # Arguments
///
/// * `y_plane` - Y (luma) plane data
/// * `u_plane` - U (chroma) plane data at full resolution (`width * height`)
/// * `v_plane` - V (chroma) plane data at full resolution (`width * height`)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
///
/// Tuple of (Y plane, U plane, V plane) with subsampled chroma. The
/// returned chroma planes are ceiling-divided (VP8/WebP odd-dimension
/// convention: `div_ceil(width, 2) * div_ceil(height, 2)`), not
/// floor-divided -- for even `width`/`height` this is identical to
/// `(width / 2) * (height / 2)`.
///
/// # Panics
///
/// Panics if input planes have incorrect sizes. Note that `u_plane` and
/// `v_plane` are `YUV444p` (full chroma resolution), so their required
/// length is always `width * height` regardless of whether `width`/`height`
/// are even or odd.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::yuv444p_to_yuv420p;
///
/// let width = 4;
/// let height = 4;
/// let y_plane = vec![128u8; width * height];
/// let u_plane = vec![100u8; width * height];
/// let v_plane = vec![150u8; width * height];
///
/// let (y_out, u_out, v_out) = yuv444p_to_yuv420p(&y_plane, &u_plane, &v_plane, width, height);
/// assert_eq!(y_out.len(), width * height);
/// assert_eq!(u_out.len(), width.div_ceil(2) * height.div_ceil(2));
/// assert_eq!(v_out.len(), width.div_ceil(2) * height.div_ceil(2));
/// ```
#[must_use]
pub fn yuv444p_to_yuv420p(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    width: usize,
    height: usize,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert_eq!(
        y_plane.len(),
        width * height,
        "y_plane length must equal width * height"
    );
    assert_eq!(
        u_plane.len(),
        width * height,
        "u_plane length must equal width * height (YUV444p input chroma is full resolution, \
         not subsampled)"
    );
    assert_eq!(
        v_plane.len(),
        width * height,
        "v_plane length must equal width * height (YUV444p input chroma is full resolution, \
         not subsampled)"
    );

    // Output 4:2:0 chroma is ceiling-divided (VP8/WebP odd-dimension
    // convention), not floor-divided -- identical to floor division for
    // even width/height. Before this fix, the output chroma planes were
    // floor-allocated as `(width/2) * (height/2)` (e.g. a 3x3 source
    // needed a 2x2 = 4 byte output but got a 1x1 = 1 byte one), and the
    // unconditional `(y + dy) * width + (x + dx)` block read for `dy, dx`
    // in `0..2` overran the *source* planes for odd height/width -- e.g.
    // at `y = 2` (the last valid row of a 3-row source) `dy = 1` computes
    // row 3, one past the end.
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);

    let y_out = y_plane.to_vec();
    let mut u_out = vec![0u8; chroma_width * chroma_height];
    let mut v_out = vec![0u8; chroma_width * chroma_height];

    // Downsample chroma by averaging 2x2 blocks. A block at the
    // right/bottom edge of an odd-width/height frame has no right/bottom
    // neighbor pixel to average -- mirroring how the fixed
    // `rgb24_to_yuv420p` handles its own partial 2x2 blocks, average only
    // the pixels that exist via the `x + 1 < width` / `y + 1 < height`
    // guards below (dynamic divisor, not a hardcoded `/ 4`). Even-dimension
    // output is bit-identical: within a `step_by(2)` walk every block
    // always has all four neighbors in bounds, so `count` is always 4 and
    // the sum is the same four terms in the same order as the original
    // unconditional `dy`/`dx` loop.
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let idx00 = y * width + x;
            let mut u_sum = u32::from(u_plane[idx00]);
            let mut v_sum = u32::from(v_plane[idx00]);
            let mut count = 1u32;

            if x + 1 < width {
                let idx = y * width + (x + 1);
                u_sum += u32::from(u_plane[idx]);
                v_sum += u32::from(v_plane[idx]);
                count += 1;
            }
            if y + 1 < height {
                let idx = (y + 1) * width + x;
                u_sum += u32::from(u_plane[idx]);
                v_sum += u32::from(v_plane[idx]);
                count += 1;
            }
            if x + 1 < width && y + 1 < height {
                let idx = (y + 1) * width + (x + 1);
                u_sum += u32::from(u_plane[idx]);
                v_sum += u32::from(v_plane[idx]);
                count += 1;
            }

            let out_idx = (y / 2) * chroma_width + (x / 2);
            #[allow(clippy::cast_possible_truncation)]
            {
                u_out[out_idx] = (u_sum / count) as u8;
                v_out[out_idx] = (v_sum / count) as u8;
            }
        }
    }

    (y_out, u_out, v_out)
}

/// Converts `YUV420p` to grayscale (8-bit).
///
/// Simply extracts the Y (luma) plane.
///
/// # Arguments
///
/// * `y_plane` - Y (luma) plane data
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
///
/// Grayscale image data
///
/// # Panics
///
/// Panics if Y plane has incorrect size.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::yuv420p_to_gray8;
///
/// let width = 4;
/// let height = 4;
/// let y_plane = vec![128u8; width * height];
///
/// let gray = yuv420p_to_gray8(&y_plane, width, height);
/// assert_eq!(gray.len(), width * height);
/// ```
#[must_use]
pub fn yuv420p_to_gray8(y_plane: &[u8], width: usize, height: usize) -> Vec<u8> {
    assert_eq!(y_plane.len(), width * height);
    y_plane.to_vec()
}

/// Converts RGB24 to grayscale (8-bit).
///
/// Uses standard luminance formula: Y = 0.299*R + 0.587*G + 0.114*B
///
/// # Arguments
///
/// * `rgb` - RGB24 packed data
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
///
/// Grayscale image data
///
/// # Panics
///
/// Panics if RGB data has incorrect size.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::rgb24_to_gray8;
///
/// let width = 4;
/// let height = 4;
/// let rgb = vec![128u8; width * height * 3];
///
/// let gray = rgb24_to_gray8(&rgb, width, height);
/// assert_eq!(gray.len(), width * height);
/// ```
#[must_use]
pub fn rgb24_to_gray8(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
    assert_eq!(rgb.len(), width * height * 3);

    let mut gray = vec![0u8; width * height];

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    for (i, gray_val) in gray.iter_mut().enumerate().take(width * height) {
        let offset = i * 3;
        let r = f32::from(rgb[offset]);
        let g = f32::from(rgb[offset + 1]);
        let b = f32::from(rgb[offset + 2]);

        let y = (0.299 * r + 0.587 * g + 0.114 * b)
            .round()
            .clamp(0.0, 255.0);
        *gray_val = y as u8;
    }

    gray
}

/// Converts grayscale (8-bit) to RGB24.
///
/// Replicates the grayscale value across all three channels.
///
/// # Arguments
///
/// * `gray` - Grayscale image data
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
///
/// RGB24 packed data
///
/// # Panics
///
/// Panics if grayscale data has incorrect size.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::gray8_to_rgb24;
///
/// let width = 4;
/// let height = 4;
/// let gray = vec![128u8; width * height];
///
/// let rgb = gray8_to_rgb24(&gray, width, height);
/// assert_eq!(rgb.len(), width * height * 3);
/// ```
#[must_use]
pub fn gray8_to_rgb24(gray: &[u8], width: usize, height: usize) -> Vec<u8> {
    assert_eq!(gray.len(), width * height);

    let mut rgb = vec![0u8; width * height * 3];

    for (i, &val) in gray.iter().enumerate().take(width * height) {
        let offset = i * 3;
        rgb[offset] = val;
        rgb[offset + 1] = val;
        rgb[offset + 2] = val;
    }

    rgb
}

/// Converts grayscale (8-bit) to `YUV420p`.
///
/// Sets Y plane to grayscale values and U/V planes to neutral (128).
///
/// # Arguments
///
/// * `gray` - Grayscale image data
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
///
/// # Returns
///
/// Tuple of (Y plane, U plane, V plane). The chroma planes are sized by
/// ceiling division (`div_ceil(2)`) of width and height (VP8/WebP
/// odd-dimension convention) -- identical to floor division for even
/// `width`/`height` -- matching every other 4:2:0 function in this module.
///
/// # Panics
///
/// Panics if grayscale data has incorrect size.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::gray8_to_yuv420p;
///
/// let width = 4;
/// let height = 4;
/// let gray = vec![128u8; width * height];
///
/// let (y_plane, u_plane, v_plane) = gray8_to_yuv420p(&gray, width, height);
/// assert_eq!(y_plane.len(), width * height);
/// assert_eq!(u_plane.len(), width.div_ceil(2) * height.div_ceil(2));
/// assert_eq!(v_plane.len(), width.div_ceil(2) * height.div_ceil(2));
/// ```
#[must_use]
pub fn gray8_to_yuv420p(gray: &[u8], width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert_eq!(
        gray.len(),
        width * height,
        "gray length must equal width * height"
    );

    // 4:2:0 chroma is ceiling-divided (VP8/WebP odd-dimension convention),
    // not floor-divided -- identical to floor division for even
    // width/height. There is no indexed write into u_plane/v_plane here
    // (they are a constant 128 fill), so a floor-sized allocation never
    // panicked -- but the returned planes were inconsistent with the
    // div_ceil(2) convention every other 4:2:0 function in this module now
    // follows: e.g. a 3x3 gray image produced a 1-byte chroma plane, and
    // the now ceil-aware `yuv420p_to_rgb24` requires 2x2 = 4 bytes for that
    // size and would reject it.
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);

    let y_plane = gray.to_vec();
    let u_plane = vec![128u8; chroma_width * chroma_height];
    let v_plane = vec![128u8; chroma_width * chroma_height];

    (y_plane, u_plane, v_plane)
}

// ─────────────────────────────────────────────────────────────────────────────
// SIMD-accelerated u8 ↔ f32 conversion and YUV420→RGB (BT.601)
// ─────────────────────────────────────────────────────────────────────────────

/// Converts a slice of `u8` pixels to normalised `f32` in `[0.0, 1.0]`.
///
/// The implementation is written in a style that LLVM auto-vectorises to
/// AVX2 or NEON when those features are enabled at compile time.  Use
/// `RUSTFLAGS="-C target-cpu=native"` to unlock wider SIMD widths.
///
/// # Panics
///
/// Panics if `src.len() != dst.len()`.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::u8_to_f32_slice;
///
/// let src = [0_u8, 128, 255];
/// let mut dst = [0.0_f32; 3];
/// u8_to_f32_slice(&src, &mut dst);
/// assert!((dst[0] - 0.0).abs() < 1e-6);
/// assert!((dst[1] - 128.0 / 255.0).abs() < 1e-5);
/// assert!((dst[2] - 1.0).abs() < 1e-6);
/// ```
#[inline]
pub fn u8_to_f32_slice(src: &[u8], dst: &mut [f32]) {
    assert_eq!(src.len(), dst.len(), "u8_to_f32_slice: length mismatch");
    u8_to_f32_scalar(src, dst);
}

/// Converts a slice of normalised `f32` pixels back to `u8` with saturating
/// clamp-and-round.
///
/// Values outside `[0.0, 1.0]` are clamped before rounding.
///
/// # Panics
///
/// Panics if `src.len() != dst.len()`.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::f32_to_u8_slice;
///
/// let src = [0.0_f32, 0.5, 1.0, -0.5, 1.5];
/// let mut dst = [0_u8; 5];
/// f32_to_u8_slice(&src, &mut dst);
/// assert_eq!(dst, [0, 128, 255, 0, 255]);
/// ```
#[inline]
pub fn f32_to_u8_slice(src: &[f32], dst: &mut [u8]) {
    assert_eq!(src.len(), dst.len(), "f32_to_u8_slice: length mismatch");
    f32_to_u8_scalar(src, dst);
}

/// Converts `YUV420p` planar data to packed `RGB24` using **BT.601** coefficients.
///
/// This is a standalone function independent of [`PixelConverter`] that is
/// tuned for the common broadcast SD colour space.  For HD content use
/// [`yuv420p_to_rgb24`] with [`ColorMatrix::Bt709`].
///
/// The Y, U, and V planes follow the standard `YUV420p` layout:
/// - Y plane: `width × height` bytes (full resolution)
/// - U / V planes: `div_ceil(width, 2) × div_ceil(height, 2)` bytes each
///   (4:2:0 subsampling, VP8/WebP odd-dimension convention -- identical to
///   `(width/2) × (height/2)` for even `width`/`height`)
///
/// Output is packed RGB, stride = `width × 3`.
///
/// # Panics
///
/// Panics if the input plane sizes do not match the expected dimensions.
/// Chroma plane sizes are ceiling-divided (`div_ceil(width, 2) *
/// div_ceil(height, 2)`), not floor-divided -- for even `width`/`height`
/// this is identical to `(width / 2) * (height / 2)`.
///
/// # Examples
///
/// ```
/// use oximedia_core::convert::pixel::yuv420_to_rgb;
///
/// let w = 4_u32;
/// let h = 4_u32;
/// let y = vec![128_u8; (w * h) as usize];
/// let u = vec![128_u8; (w.div_ceil(2) * h.div_ceil(2)) as usize];
/// let v = vec![128_u8; (w.div_ceil(2) * h.div_ceil(2)) as usize];
/// let rgb = yuv420_to_rgb(&y, &u, &v, w, h);
/// assert_eq!(rgb.len(), (w * h * 3) as usize);
/// ```
#[must_use]
pub fn yuv420_to_rgb(y_plane: &[u8], u_plane: &[u8], v_plane: &[u8], w: u32, h: u32) -> Vec<u8> {
    let width = w as usize;
    let height = h as usize;

    // 4:2:0 chroma is ceiling-divided (VP8/WebP odd-dimension convention),
    // not floor-divided -- identical to floor division for even
    // width/height. Before this fix, a floor-divided chroma stride here was
    // the same panic class as the (now fixed) `yuv420p_to_rgb24`: a
    // correctly ceil-sized `u_plane`/`v_plane` failed the (unlabeled)
    // length assert outright, while a plane sized to the old floor
    // contract passed that assert and then panicked out of bounds inside
    // the `(row/2)*(width/2)+col/2` read once `row`/`col` reached the
    // ceil-only edge (a 3x3 frame: the old assert required a 1-byte
    // `u_plane`, and at `row=0, col=2` the index is `0*1 + 1 = 1`, already
    // past the end).
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);

    assert_eq!(
        y_plane.len(),
        width * height,
        "y_plane length must equal width * height"
    );
    assert_eq!(
        u_plane.len(),
        chroma_width * chroma_height,
        "u_plane length must equal the ceil-divided 4:2:0 chroma size \
         div_ceil(width, 2) * div_ceil(height, 2)"
    );
    assert_eq!(
        v_plane.len(),
        chroma_width * chroma_height,
        "v_plane length must equal the ceil-divided 4:2:0 chroma size \
         div_ceil(width, 2) * div_ceil(height, 2)"
    );

    // BT.601 fixed-point coefficients (scaled × 1024).
    // Y' contribution:  1.164 × (Y - 16)   → scale 1164/1000
    // V→R:              1.596 × (V - 128)
    // U→G:              0.392 × (U - 128)
    // V→G:              0.813 × (V - 128)
    // U→B:              2.017 × (U - 128)
    const SCALE: i32 = 1024;
    const Y_SCALE: i32 = (1.164 * SCALE as f64) as i32; // 1192
    const VR: i32 = (1.596 * SCALE as f64) as i32; // 1634
    const UG: i32 = (0.392 * SCALE as f64) as i32; // 401
    const VG: i32 = (0.813 * SCALE as f64) as i32; // 832
    const UB: i32 = (2.017 * SCALE as f64) as i32; // 2066

    let mut rgb = vec![0u8; width * height * 3];

    for row in 0..height {
        for col in 0..width {
            let y_val = i32::from(y_plane[row * width + col]) - 16;
            let u_val = i32::from(u_plane[(row / 2) * chroma_width + col / 2]) - 128;
            let v_val = i32::from(v_plane[(row / 2) * chroma_width + col / 2]) - 128;

            let y_scaled = y_val * Y_SCALE;
            let r = (y_scaled + v_val * VR) >> 10;
            let g = (y_scaled - u_val * UG - v_val * VG) >> 10;
            let b = (y_scaled + u_val * UB) >> 10;

            let out = (row * width + col) * 3;
            rgb[out] = r.clamp(0, 255) as u8;
            rgb[out + 1] = g.clamp(0, 255) as u8;
            rgb[out + 2] = b.clamp(0, 255) as u8;
        }
    }

    rgb
}

// ─────────────────────────────────────────────────────────────────────────────
// Scalar fallback implementations
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn u8_to_f32_scalar(src: &[u8], dst: &mut [f32]) {
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        *d = f32::from(*s) / 255.0;
    }
}

#[inline]
fn f32_to_u8_scalar(src: &[f32], dst: &mut [u8]) {
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        *d = (s.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_matrix_coefficients() {
        let bt601 = ColorMatrix::Bt601;
        let bt709 = ColorMatrix::Bt709;

        // BT.601 coefficients
        assert!((bt601.kr() - 0.299).abs() < f32::EPSILON);
        assert!((bt601.kb() - 0.114).abs() < f32::EPSILON);

        // BT.709 coefficients
        assert!((bt709.kr() - 0.2126).abs() < f32::EPSILON);
        assert!((bt709.kb() - 0.0722).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pixel_converter_yuv_to_rgb() {
        let converter = PixelConverter::new(ColorMatrix::Bt709);

        // Test neutral gray (Y=128, U=128, V=128)
        let (r, g, b) = converter.yuv_to_rgb(128, 128, 128);
        // Should be approximately gray
        assert!((i16::from(r) - i16::from(g)).abs() < 20);
        assert!((i16::from(g) - i16::from(b)).abs() < 20);
    }

    #[test]
    fn test_yuv420p_to_rgb24() {
        let width = 4;
        let height = 4;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![128u8; (width / 2) * (height / 2)];
        let v_plane = vec![128u8; (width / 2) * (height / 2)];

        let rgb = yuv420p_to_rgb24(
            &y_plane,
            &u_plane,
            &v_plane,
            width,
            height,
            ColorMatrix::Bt709,
        );
        assert_eq!(rgb.len(), width * height * 3);
    }

    /// Odd-dimension (3x3) regression test: chroma is ceil-divided to 2x2,
    /// and the row/column that only exists under ceil division must be read
    /// at the correct chroma index (not aliased or panicking).
    #[test]
    fn test_yuv420p_to_rgb24_odd_dims_3x3() {
        let width = 3;
        let height = 3;
        let y_plane: Vec<u8> = (0..9u32).map(|i| (20 + i * 20) as u8).collect();
        let u_plane = vec![10u8, 60, 110, 160]; // 2x2 ceil-divided chroma
        let v_plane = vec![30u8, 80, 130, 180];

        let rgb = yuv420p_to_rgb24(
            &y_plane,
            &u_plane,
            &v_plane,
            width,
            height,
            ColorMatrix::Bt709,
        );
        assert_eq!(rgb.len(), width * height * 3);

        let converter = PixelConverter::new(ColorMatrix::Bt709);

        // (0,0) -> chroma index 0: baseline sanity check.
        let expected00 = converter.yuv_to_rgb(y_plane[0], u_plane[0], v_plane[0]);
        assert_eq!((rgb[0], rgb[1], rgb[2]), expected00);

        // (0,2) -> chroma row 1, col 0 = index 2. A floor-divided
        // chroma_width (1) would instead compute index `1*1+0 = 1`
        // (in-bounds but wrong) -- the silent-shear case, not a panic.
        let idx02 = (2 * width) * 3;
        let expected02 = converter.yuv_to_rgb(y_plane[6], u_plane[2], v_plane[2]);
        assert_eq!((rgb[idx02], rgb[idx02 + 1], rgb[idx02 + 2]), expected02);

        // (2,2) -> chroma row 1, col 1 = index 3, the sample a floor
        // row-stride would miscompute as index 2.
        let idx22 = (2 * width + 2) * 3;
        let expected22 = converter.yuv_to_rgb(y_plane[8], u_plane[3], v_plane[3]);
        assert_eq!((rgb[idx22], rgb[idx22 + 1], rgb[idx22 + 2]), expected22);
    }

    /// Even-dimension regression pin: div_ceil(2) == floor division here, so
    /// this proves the fix is a no-op for even width/height.
    #[test]
    fn test_yuv420p_to_rgb24_even_dims_6x4_unchanged() {
        let width = 6;
        let height = 4;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![64u8; (width / 2) * (height / 2)];
        let v_plane = vec![192u8; (width / 2) * (height / 2)];

        let rgb = yuv420p_to_rgb24(
            &y_plane,
            &u_plane,
            &v_plane,
            width,
            height,
            ColorMatrix::Bt709,
        );
        assert_eq!(rgb.len(), width * height * 3);

        let converter = PixelConverter::new(ColorMatrix::Bt709);
        let expected = converter.yuv_to_rgb(128, 64, 192);
        for px in rgb.chunks_exact(3) {
            assert_eq!((px[0], px[1], px[2]), expected);
        }
    }

    #[test]
    fn test_rgb24_to_yuv420p() {
        let width = 4;
        let height = 4;
        let rgb = vec![128u8; width * height * 3];

        let (y_plane, u_plane, v_plane) = rgb24_to_yuv420p(&rgb, width, height, ColorMatrix::Bt709);
        assert_eq!(y_plane.len(), width * height);
        assert_eq!(u_plane.len(), (width / 2) * (height / 2));
        assert_eq!(v_plane.len(), (width / 2) * (height / 2));
    }

    /// Odd-dimension (3x3) regression test: before the fix, the chroma
    /// index `(y / 2) * (width / 2) + x / 2` combined with a floor-sized
    /// (1-byte) `u_plane`/`v_plane` panicked out of bounds once `x` or `y`
    /// reached 2. This must now succeed and produce ceil-sized (2x2) planes.
    /// Also checks the 2x2 averaging at the odd bottom-right edge: that
    /// logic (average only the neighboring pixels that exist) was already
    /// correct and is intentionally unchanged by this fix -- only the
    /// chroma plane size/stride were floor- vs ceil-divided -- so this pins
    /// it as a value check, not just a length check.
    #[test]
    fn test_rgb24_to_yuv420p_odd_dims_3x3_no_panic() {
        let width = 3;
        let height = 3;
        let rgb: Vec<u8> = (0..(width * height * 3))
            .map(|i| (i * 11 % 256) as u8)
            .collect();

        let (y_plane, u_plane, v_plane) = rgb24_to_yuv420p(&rgb, width, height, ColorMatrix::Bt709);
        assert_eq!(y_plane.len(), 9);
        assert_eq!(u_plane.len(), 4); // ceil(3/2) * ceil(3/2) = 2*2
        assert_eq!(v_plane.len(), 4);

        // Bottom-right chroma sample (row 1, col 1 of the 2x2 plane =
        // index 3) is anchored at pixel (x=2, y=2), the frame's last row
        // and column, so it has no right/bottom/diagonal neighbor to
        // average with -- it must equal that single pixel's own U/V
        // exactly, not an average diluted by phantom neighbors.
        let converter = PixelConverter::new(ColorMatrix::Bt709);
        let corner_offset = (2 * width + 2) * 3;
        let (_, expected_u, expected_v) = converter.rgb_to_yuv(
            rgb[corner_offset],
            rgb[corner_offset + 1],
            rgb[corner_offset + 2],
        );
        assert_eq!(u_plane[3], expected_u, "bottom-right chroma sample (U)");
        assert_eq!(v_plane[3], expected_v, "bottom-right chroma sample (V)");
    }

    /// Even-dimension regression pin: div_ceil(2) == floor division here, so
    /// this proves the fix is a no-op for even width/height.
    #[test]
    fn test_rgb24_to_yuv420p_even_dims_6x4_unchanged() {
        let width = 6;
        let height = 4;
        let rgb = vec![100u8; width * height * 3];

        let (y_plane, u_plane, v_plane) = rgb24_to_yuv420p(&rgb, width, height, ColorMatrix::Bt709);
        assert_eq!(y_plane.len(), 24);
        assert_eq!(u_plane.len(), 6); // 3x2, ceil == floor for even dims
        assert_eq!(v_plane.len(), 6);
    }

    #[test]
    fn test_yuv420p_to_yuv444p() {
        let width = 4;
        let height = 4;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![100u8; (width / 2) * (height / 2)];
        let v_plane = vec![150u8; (width / 2) * (height / 2)];

        let (y_out, u_out, v_out) = yuv420p_to_yuv444p(&y_plane, &u_plane, &v_plane, width, height);
        assert_eq!(y_out.len(), width * height);
        assert_eq!(u_out.len(), width * height);
        assert_eq!(v_out.len(), width * height);
    }

    /// Odd-dimension (3x3) regression test: chroma is ceil-divided to 2x2.
    #[test]
    fn test_yuv420p_to_yuv444p_odd_dims_3x3() {
        let width = 3;
        let height = 3;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![10u8, 60, 110, 160]; // 2x2 ceil-divided chroma
        let v_plane = vec![30u8, 80, 130, 180];

        let (y_out, u_out, v_out) = yuv420p_to_yuv444p(&y_plane, &u_plane, &v_plane, width, height);
        assert_eq!(y_out.len(), 9);
        assert_eq!(u_out.len(), 9);
        assert_eq!(v_out.len(), 9);

        // (0,2): output row 2, col 0 -> cx=0, cy=1, fx=0, fy=0, so the
        // result must equal the raw chroma sample at row 1, col 0 = index
        // 2. A floor-divided chroma_width (1) would instead read index 1.
        assert_eq!(u_out[2 * width], u_plane[2]);
        assert_eq!(v_out[2 * width], v_plane[2]);

        // (2,2): bottom-right output pixel maps to chroma index (1,1) = 3,
        // clamped at the plane edge (no further neighbor to interpolate
        // towards), so it must equal the raw chroma sample exactly.
        assert_eq!(u_out[2 * width + 2], u_plane[3]);
        assert_eq!(v_out[2 * width + 2], v_plane[3]);
    }

    /// Even-dimension regression pin: div_ceil(2) == floor division here, so
    /// this proves the fix is a no-op for even width/height.
    #[test]
    fn test_yuv420p_to_yuv444p_even_dims_6x4_unchanged() {
        let width = 6;
        let height = 4;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![100u8; 3 * 2]; // ceil(6/2)*ceil(4/2), 3x2
        let v_plane = vec![150u8; 3 * 2];

        let (y_out, u_out, v_out) = yuv420p_to_yuv444p(&y_plane, &u_plane, &v_plane, width, height);
        assert_eq!(y_out.len(), 24);
        assert_eq!(u_out.len(), 24);
        assert_eq!(v_out.len(), 24);
        assert!(u_out.iter().all(|&v| v == 100));
        assert!(v_out.iter().all(|&v| v == 150));
    }

    #[test]
    fn test_yuv444p_to_yuv420p() {
        let width = 4;
        let height = 4;
        let y_plane = vec![128u8; width * height];
        let u_plane = vec![100u8; width * height];
        let v_plane = vec![150u8; width * height];

        let (y_out, u_out, v_out) = yuv444p_to_yuv420p(&y_plane, &u_plane, &v_plane, width, height);
        assert_eq!(y_out.len(), width * height);
        assert_eq!(u_out.len(), (width / 2) * (height / 2));
        assert_eq!(v_out.len(), (width / 2) * (height / 2));
    }

    /// Odd-dimension (3x3) regression test: before the fix, the output
    /// chroma planes were floor-allocated to 1x1 (1 byte) instead of the
    /// required 2x2 (4 bytes), and the unconditional `(y+dy)*width+(x+dx)`
    /// block read overran the 9-byte source planes once the block touched
    /// the last row/column (e.g. `y=2, dy=1` computes source row 3, one
    /// past the end). This must now succeed, produce ceil-sized (2x2)
    /// planes, and -- mirroring how the fixed `rgb24_to_yuv420p` handles
    /// its own partial 2x2 blocks -- average only the source pixels that
    /// exist at each of the plane's four blocks (full, right-edge,
    /// bottom-edge, and single-corner), not a hardcoded 4-pixel divisor.
    /// Every output byte is value-asserted against an independently
    /// hand-selected set of source indices per block, not just lengths.
    #[test]
    fn test_yuv444p_to_yuv420p_odd_dims_3x3_no_panic() {
        let width = 3;
        let height = 3;
        // Flat index = y * 3 + x:
        //   0(0,0) 1(1,0) 2(2,0)
        //   3(0,1) 4(1,1) 5(2,1)
        //   6(0,2) 7(1,2) 8(2,2)
        let y_plane: Vec<u8> = (0..9u32).map(|i| (10 + i * 15) as u8).collect();
        let u_plane: Vec<u8> = vec![20, 40, 100, 60, 80, 140, 90, 110, 200];
        let v_plane: Vec<u8> = vec![210, 190, 150, 170, 130, 90, 120, 100, 30];

        let (y_out, u_out, v_out) = yuv444p_to_yuv420p(&y_plane, &u_plane, &v_plane, width, height);
        assert_eq!(y_out, y_plane);
        assert_eq!(u_out.len(), 4); // ceil(3/2) * ceil(3/2) = 2*2, not 1x1
        assert_eq!(v_out.len(), 4);

        // Average exactly the given source indices -- a small, obviously
        // correct helper; the actual assertion is that these are the
        // *right* indices (block membership/edge-clamping) and the *right*
        // divisor (their count, not a hardcoded 4), not a re-derivation of
        // the algorithm under test.
        let avg = |plane: &[u8], px: &[usize]| -> u8 {
            let sum: u32 = px.iter().map(|&i| u32::from(plane[i])).sum();
            (sum / px.len() as u32) as u8
        };

        // Block (cx=0,cy=0) -> out index 0: full 2x2, source (0,0),(1,0),
        // (0,1),(1,1) = flat indices 0,1,3,4.
        assert_eq!(u_out[0], avg(&u_plane, &[0, 1, 3, 4]), "block(0,0) U");
        assert_eq!(v_out[0], avg(&v_plane, &[0, 1, 3, 4]), "block(0,0) V");

        // Block (cx=1,cy=0) -> out index 1: right edge, only 2 source
        // pixels exist: (2,0),(2,1) = flat indices 2,5. A hardcoded `/4`
        // divisor (the pre-fix behavior) would silently halve this instead
        // of averaging just the two real samples.
        assert_eq!(
            u_out[1],
            avg(&u_plane, &[2, 5]),
            "block(1,0) U (right edge)"
        );
        assert_eq!(
            v_out[1],
            avg(&v_plane, &[2, 5]),
            "block(1,0) V (right edge)"
        );

        // Block (cx=0,cy=1) -> out index 2: bottom edge, only 2 source
        // pixels exist: (0,2),(1,2) = flat indices 6,7.
        assert_eq!(
            u_out[2],
            avg(&u_plane, &[6, 7]),
            "block(0,1) U (bottom edge)"
        );
        assert_eq!(
            v_out[2],
            avg(&v_plane, &[6, 7]),
            "block(0,1) V (bottom edge)"
        );

        // Block (cx=1,cy=1) -> out index 3: bottom-right corner, only 1
        // source pixel exists: (2,2) = flat index 8. Must equal that pixel
        // exactly, not an average diluted by phantom neighbors.
        assert_eq!(u_out[3], avg(&u_plane, &[8]), "block(1,1) U (corner)");
        assert_eq!(v_out[3], avg(&v_plane, &[8]), "block(1,1) V (corner)");
    }

    /// Even-dimension regression pin: div_ceil(2) == floor division here,
    /// and every 2x2 block is fully in-bounds, so this proves the fix is a
    /// true no-op for even width/height -- not just same lengths, but the
    /// exact same averaged byte per block. Uses non-uniform per-pixel data
    /// (not a constant fill) so that a regression which summed the wrong
    /// four source pixels, or reverted to the wrong divisor, would be
    /// caught; a constant fill cannot distinguish those cases.
    #[test]
    fn test_yuv444p_to_yuv420p_even_dims_6x4_unchanged() {
        let width = 6;
        let height = 4;
        let y_plane: Vec<u8> = (0..24u32).map(|i| (i * 5 % 256) as u8).collect();
        let u_plane: Vec<u8> = (0..24u32).map(|i| (i * 7 % 256) as u8).collect();
        let v_plane: Vec<u8> = (0..24u32).map(|i| (i * 13 % 256) as u8).collect();

        let (y_out, u_out, v_out) = yuv444p_to_yuv420p(&y_plane, &u_plane, &v_plane, width, height);
        assert_eq!(y_out, y_plane);
        assert_eq!(u_out.len(), 6); // 3x2, ceil == floor for even dims
        assert_eq!(v_out.len(), 6);

        let avg = |plane: &[u8], px: &[usize]| -> u8 {
            let sum: u32 = px.iter().map(|&i| u32::from(plane[i])).sum();
            (sum / px.len() as u32) as u8
        };

        // Block (0,0) -> out index 0: source pixels (0,0),(1,0),(0,1),(1,1)
        // = flat indices 0,1,6,7.
        assert_eq!(u_out[0], avg(&u_plane, &[0, 1, 6, 7]), "block(0,0) U");
        assert_eq!(v_out[0], avg(&v_plane, &[0, 1, 6, 7]), "block(0,0) V");

        // Block (2,1) (bottom-right, out index = 1*3+2 = 5): source pixels
        // (4,2),(5,2),(4,3),(5,3) = flat indices 16,17,22,23.
        assert_eq!(u_out[5], avg(&u_plane, &[16, 17, 22, 23]), "block(2,1) U");
        assert_eq!(v_out[5], avg(&v_plane, &[16, 17, 22, 23]), "block(2,1) V");
    }

    #[test]
    fn test_grayscale_conversions() {
        let width = 4;
        let height = 4;

        // Test YUV420p to gray
        let y_plane = vec![128u8; width * height];
        let gray = yuv420p_to_gray8(&y_plane, width, height);
        assert_eq!(gray.len(), width * height);
        assert_eq!(gray[0], 128);

        // Test RGB to gray
        let rgb = vec![128u8; width * height * 3];
        let gray = rgb24_to_gray8(&rgb, width, height);
        assert_eq!(gray.len(), width * height);

        // Test gray to RGB
        let rgb = gray8_to_rgb24(&gray, width, height);
        assert_eq!(rgb.len(), width * height * 3);

        // Test gray to YUV420p
        let (y, u, v) = gray8_to_yuv420p(&gray, width, height);
        assert_eq!(y.len(), width * height);
        assert_eq!(u.len(), (width / 2) * (height / 2));
        assert_eq!(v.len(), (width / 2) * (height / 2));
        assert_eq!(u[0], 128); // Neutral chroma
        assert_eq!(v[0], 128); // Neutral chroma
    }

    /// Odd-dimension (3x3) regression test: chroma allocation is now
    /// ceil-divided (2x2 = 4 bytes) instead of floor-divided (1x1 = 1
    /// byte). There is no indexed write into the chroma planes (constant
    /// 128 fill), so the floor-sized allocation never panicked here -- but
    /// it produced planes inconsistent with the div_ceil(2) convention
    /// every other 4:2:0 function in this module follows. This is
    /// demonstrated end-to-end: the now ceil-aware `yuv420p_to_rgb24` must
    /// accept this function's output directly for an odd-sized image.
    #[test]
    fn test_gray8_to_yuv420p_odd_dims_3x3() {
        let width = 3;
        let height = 3;
        let gray: Vec<u8> = (0..9u32).map(|i| (i * 25) as u8).collect();

        let (y_plane, u_plane, v_plane) = gray8_to_yuv420p(&gray, width, height);
        assert_eq!(y_plane, gray);
        assert_eq!(u_plane.len(), 4); // ceil(3/2) * ceil(3/2) = 2*2, not 1x1
        assert_eq!(v_plane.len(), 4);
        assert!(
            u_plane.iter().all(|&val| val == 128),
            "U plane must stay neutral 128"
        );
        assert!(
            v_plane.iter().all(|&val| val == 128),
            "V plane must stay neutral 128"
        );

        // The now-ceil-aware yuv420p_to_rgb24 must accept this output
        // directly instead of panicking on a length mismatch.
        let rgb = yuv420p_to_rgb24(
            &y_plane,
            &u_plane,
            &v_plane,
            width,
            height,
            ColorMatrix::Bt709,
        );
        assert_eq!(rgb.len(), width * height * 3);
    }

    /// Even-dimension regression pin: div_ceil(2) == floor division here, so
    /// this proves the fix is a no-op for even width/height.
    #[test]
    fn test_gray8_to_yuv420p_even_dims_6x4_unchanged() {
        let width = 6;
        let height = 4;
        let gray: Vec<u8> = (0..24u32).map(|i| (i * 9 % 256) as u8).collect();

        let (y_plane, u_plane, v_plane) = gray8_to_yuv420p(&gray, width, height);
        assert_eq!(y_plane, gray);
        assert_eq!(u_plane.len(), 6); // 3x2, ceil == floor for even dims
        assert_eq!(v_plane.len(), 6);
        assert!(
            u_plane.iter().all(|&val| val == 128),
            "U plane must stay neutral 128"
        );
        assert!(
            v_plane.iter().all(|&val| val == 128),
            "V plane must stay neutral 128"
        );
    }

    #[test]
    fn test_roundtrip_rgb_yuv() {
        let width = 4;
        let height = 4;
        let mut rgb = vec![0u8; width * height * 3];

        // Create a simple pattern
        for i in 0..rgb.len() {
            rgb[i] = ((i * 50) % 256) as u8;
        }

        // Convert RGB -> YUV -> RGB
        let (y, u, v) = rgb24_to_yuv420p(&rgb, width, height, ColorMatrix::Bt709);
        let rgb2 = yuv420p_to_rgb24(&y, &u, &v, width, height, ColorMatrix::Bt709);

        assert_eq!(rgb.len(), rgb2.len());
        // Due to chroma subsampling, we expect some loss.
        // Verify rgb2 is non-empty (all u8 values are inherently <= 255).
        assert!(!rgb2.is_empty());
    }

    // ── SIMD u8 ↔ f32 tests ─────────────────────────────────────────────────

    #[test]
    fn test_u8_to_f32_slice_boundary_values() {
        let src = [0_u8, 128, 255];
        let mut dst = [0.0_f32; 3];
        u8_to_f32_slice(&src, &mut dst);
        assert!((dst[0] - 0.0).abs() < 1e-6);
        assert!((dst[1] - 128.0 / 255.0).abs() < 1e-5);
        assert!((dst[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_f32_to_u8_slice_clamping() {
        let src = [0.0_f32, 0.5, 1.0, -0.5, 1.5];
        let mut dst = [0_u8; 5];
        f32_to_u8_slice(&src, &mut dst);
        assert_eq!(dst[0], 0);
        assert_eq!(dst[2], 255);
        assert_eq!(dst[3], 0); // clamped from -0.5
        assert_eq!(dst[4], 255); // clamped from 1.5
    }

    /// Round-trip: u8 → f32 → u8 must be within ±1 code value.
    #[test]
    fn test_u8_f32_round_trip_accuracy() {
        let src: Vec<u8> = (0_u8..=255).collect();
        let mut f32_buf = vec![0.0_f32; src.len()];
        u8_to_f32_slice(&src, &mut f32_buf);

        let mut back = vec![0_u8; src.len()];
        f32_to_u8_slice(&f32_buf, &mut back);

        for (orig, restored) in src.iter().zip(back.iter()) {
            let diff = (*orig as i16 - *restored as i16).abs();
            assert!(
                diff <= 1,
                "round-trip error > ±1: orig={orig}, restored={restored}"
            );
        }
    }

    /// Correctness against scalar implementation.
    #[test]
    fn test_u8_to_f32_matches_scalar() {
        let src: Vec<u8> = (0_u8..=255).collect();
        let mut scalar_dst = vec![0.0_f32; src.len()];
        super::u8_to_f32_scalar(&src, &mut scalar_dst);

        let mut simd_dst = vec![0.0_f32; src.len()];
        u8_to_f32_slice(&src, &mut simd_dst);

        for (s, d) in scalar_dst.iter().zip(simd_dst.iter()) {
            assert!((s - d).abs() < 1e-6, "scalar={s} vs simd={d}");
        }
    }

    /// f32→u8 correctness against scalar.
    #[test]
    fn test_f32_to_u8_matches_scalar() {
        let src: Vec<f32> = (0_u16..=255).map(|v| v as f32 / 255.0).collect();
        let mut scalar_dst = vec![0_u8; src.len()];
        super::f32_to_u8_scalar(&src, &mut scalar_dst);

        let mut simd_dst = vec![0_u8; src.len()];
        f32_to_u8_slice(&src, &mut simd_dst);

        for (s, d) in scalar_dst.iter().zip(simd_dst.iter()) {
            let diff = (*s as i16 - *d as i16).abs();
            assert!(diff <= 1, "scalar={s} vs simd={d}");
        }
    }

    // ── yuv420_to_rgb BT.601 tests ───────────────────────────────────────────

    #[test]
    fn test_yuv420_to_rgb_output_size() {
        let w = 8_u32;
        let h = 8_u32;
        let y = vec![128_u8; (w * h) as usize];
        let u = vec![128_u8; (w / 2 * (h / 2)) as usize];
        let v = vec![128_u8; (w / 2 * (h / 2)) as usize];
        let rgb = yuv420_to_rgb(&y, &u, &v, w, h);
        assert_eq!(rgb.len(), (w * h * 3) as usize);
    }

    /// Neutral gray (Y=128, U=128, V=128) should give approximately equal R,G,B.
    #[test]
    fn test_yuv420_to_rgb_neutral_gray() {
        let w = 4_u32;
        let h = 4_u32;
        let y = vec![128_u8; (w * h) as usize];
        let u = vec![128_u8; (w / 2 * (h / 2)) as usize];
        let v = vec![128_u8; (w / 2 * (h / 2)) as usize];
        let rgb = yuv420_to_rgb(&y, &u, &v, w, h);

        for px in rgb.chunks_exact(3) {
            let r = px[0] as i16;
            let g = px[1] as i16;
            let b = px[2] as i16;
            assert!(
                (r - g).abs() <= 4,
                "R={r} G={g} differ by >{}",
                (r - g).abs()
            );
            assert!(
                (g - b).abs() <= 4,
                "G={g} B={b} differ by >{}",
                (g - b).abs()
            );
        }
    }

    /// Odd-dimension (3x3) regression test: chroma is ceil-divided to 2x2,
    /// and the row/column that only exists under ceil division must be read
    /// at the correct chroma index (not aliased or panicking) -- the same
    /// shear/panic class as the fixed `yuv420p_to_rgb24`. The expected
    /// pixel for a given (y, u, v) triple is obtained by calling
    /// `yuv420_to_rgb` itself on a trivial 1x1 image, where the chroma
    /// index is unambiguous (`chroma_width == chroma_height == 1`, the only
    /// valid index is 0) -- this isolates the *indexing* bug under test
    /// from the fixed-point arithmetic, which is identical in both calls.
    #[test]
    fn test_yuv420_to_rgb_odd_dims_3x3_no_panic() {
        let w = 3_u32;
        let h = 3_u32;
        // Flat index = y * 3 + x, matching `yuv420p_to_rgb24`'s own
        // odd-dim test fixture.
        let y_plane: Vec<u8> = (0..9u32).map(|i| (20 + i * 20) as u8).collect();
        let u_plane = vec![10u8, 60, 110, 160]; // 2x2 ceil-divided chroma
        let v_plane = vec![30u8, 80, 130, 180];

        let rgb = yuv420_to_rgb(&y_plane, &u_plane, &v_plane, w, h);
        assert_eq!(rgb.len(), 3 * 3 * 3);

        let reference = |y: u8, u: u8, v: u8| -> (u8, u8, u8) {
            let out = yuv420_to_rgb(&[y], &[u], &[v], 1, 1);
            (out[0], out[1], out[2])
        };

        // (0,0) -> chroma index 0: baseline sanity check.
        let expected00 = reference(y_plane[0], u_plane[0], v_plane[0]);
        assert_eq!((rgb[0], rgb[1], rgb[2]), expected00);

        // (0,2) -> chroma row 1, col 0 = index 2. A floor-divided
        // chroma_width (1) would instead compute index `1*1+0 = 1`
        // (in-bounds but wrong) -- the silent-shear case, not a panic.
        let idx02 = (2 * 3) * 3;
        let expected02 = reference(y_plane[6], u_plane[2], v_plane[2]);
        assert_eq!((rgb[idx02], rgb[idx02 + 1], rgb[idx02 + 2]), expected02);

        // (2,2) -> chroma row 1, col 1 = index 3, the sample a floor
        // row-stride would miscompute as index 2.
        let idx22 = (2 * 3 + 2) * 3;
        let expected22 = reference(y_plane[8], u_plane[3], v_plane[3]);
        assert_eq!((rgb[idx22], rgb[idx22 + 1], rgb[idx22 + 2]), expected22);
    }

    /// Even-dimension regression pin: div_ceil(2) == floor division here, so
    /// this proves the fix is a no-op for even width/height.
    #[test]
    fn test_yuv420_to_rgb_even_dims_6x4_unchanged() {
        let w = 6_u32;
        let h = 4_u32;
        let y_plane = vec![128u8; 24];
        let u_plane = vec![64u8; 6]; // 3x2, ceil == floor for even dims
        let v_plane = vec![192u8; 6];

        let rgb = yuv420_to_rgb(&y_plane, &u_plane, &v_plane, w, h);
        assert_eq!(rgb.len(), 24 * 3);

        let expected = {
            let out = yuv420_to_rgb(&[128u8], &[64u8], &[192u8], 1, 1);
            (out[0], out[1], out[2])
        };
        for px in rgb.chunks_exact(3) {
            assert_eq!((px[0], px[1], px[2]), expected);
        }
    }
}

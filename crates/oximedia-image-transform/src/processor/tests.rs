// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Tests for the processor module.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::{
        apply_transforms, bilinear_resize, build_pipeline,
        color::{apply_brightness, apply_contrast, apply_gamma, apply_rotation},
        filters::{apply_blur, apply_sharpen, build_gaussian_kernel},
        geometry::{
            apply_border, apply_padding, apply_trim, calculate_crop_rect, gravity_to_fractions,
        },
        lanczos_resize,
        resize::{apply_resize, fit_contain_dims, fit_cover_dims, lanczos3_kernel},
        PipelineStep, PixelBuffer,
    };
    use crate::transform::{
        Border, Color, FitMode, Gravity, OutputFormat, Padding, Rotation, TransformParams, Trim,
    };

    // ── Test helpers ──

    fn make_test_buffer(width: u32, height: u32) -> PixelBuffer {
        let mut buf = PixelBuffer::new(width, height, 4);
        for y in 0..height {
            for x in 0..width {
                let r = ((x * 255) / width.max(1)) as u8;
                let g = ((y * 255) / height.max(1)) as u8;
                buf.set_pixel(x, y, &[r, g, 128, 255]);
            }
        }
        buf
    }

    fn make_solid_buffer(width: u32, height: u32, color: [u8; 4]) -> PixelBuffer {
        let mut buf = PixelBuffer::new(width, height, 4);
        for y in 0..height {
            for x in 0..width {
                buf.set_pixel(x, y, &color);
            }
        }
        buf
    }

    // ── PixelBuffer construction ──

    #[test]
    fn test_pixel_buffer_new() {
        let buf = PixelBuffer::new(10, 20, 4);
        assert_eq!(buf.width, 10);
        assert_eq!(buf.height, 20);
        assert_eq!(buf.channels, 4);
        assert_eq!(buf.data.len(), 10 * 20 * 4);
    }

    #[test]
    fn test_pixel_buffer_from_rgba_valid() {
        let data = vec![0u8; 4 * 4 * 4];
        assert!(PixelBuffer::from_rgba(data, 4, 4).is_ok());
    }

    #[test]
    fn test_pixel_buffer_from_rgba_invalid() {
        let data = vec![0u8; 10];
        assert!(PixelBuffer::from_rgba(data, 4, 4).is_err());
    }

    #[test]
    fn test_pixel_buffer_from_rgb_valid() {
        let data = vec![0u8; 3 * 3 * 3];
        assert!(PixelBuffer::from_rgb(data, 3, 3).is_ok());
    }

    #[test]
    fn test_pixel_buffer_from_rgb_invalid() {
        let data = vec![0u8; 5];
        assert!(PixelBuffer::from_rgb(data, 3, 3).is_err());
    }

    #[test]
    fn test_get_set_pixel() {
        let mut buf = PixelBuffer::new(4, 4, 4);
        buf.set_pixel(2, 3, &[100, 150, 200, 255]);
        let p = buf.get_pixel(2, 3).expect("pixel exists");
        assert_eq!(p, &[100, 150, 200, 255]);
    }

    #[test]
    fn test_get_pixel_out_of_bounds() {
        let buf = PixelBuffer::new(4, 4, 4);
        assert!(buf.get_pixel(4, 0).is_none());
        assert!(buf.get_pixel(0, 4).is_none());
        assert!(buf.get_pixel(100, 100).is_none());
    }

    #[test]
    fn test_set_pixel_out_of_bounds_noop() {
        let mut buf = PixelBuffer::new(4, 4, 4);
        buf.set_pixel(10, 10, &[255, 0, 0, 255]); // should not panic
    }

    #[test]
    fn test_single_pixel_buffer() {
        let mut buf = PixelBuffer::new(1, 1, 4);
        buf.set_pixel(0, 0, &[42, 84, 126, 255]);
        assert_eq!(buf.get_pixel(0, 0).expect("pixel"), &[42, 84, 126, 255]);
    }

    // ── Bilinear sampling ──

    #[test]
    fn test_sample_bilinear_exact_corners() {
        let mut buf = PixelBuffer::new(2, 2, 4);
        buf.set_pixel(0, 0, &[100, 0, 0, 255]);
        buf.set_pixel(1, 0, &[200, 0, 0, 255]);
        assert_eq!(buf.sample_bilinear(0.0, 0.0)[0], 100);
        assert_eq!(buf.sample_bilinear(1.0, 0.0)[0], 200);
    }

    #[test]
    fn test_sample_bilinear_interpolated() {
        let mut buf = PixelBuffer::new(2, 1, 4);
        buf.set_pixel(0, 0, &[0, 0, 0, 255]);
        buf.set_pixel(1, 0, &[200, 0, 0, 255]);
        let p = buf.sample_bilinear(0.5, 0.0);
        assert!((p[0] as i32 - 100).abs() <= 1);
    }

    #[test]
    fn test_sample_bilinear_empty_buffer() {
        let buf = PixelBuffer::new(0, 0, 4);
        assert_eq!(buf.sample_bilinear(0.0, 0.0), [0, 0, 0, 255]);
    }

    // ── Bilinear resize ──

    #[test]
    fn test_bilinear_resize_identity() {
        let buf = make_test_buffer(10, 10);
        let resized = bilinear_resize(&buf, 10, 10);
        assert_eq!(resized.data, buf.data);
    }

    #[test]
    fn test_bilinear_resize_downscale() {
        let buf = make_test_buffer(100, 100);
        let resized = bilinear_resize(&buf, 50, 50);
        assert_eq!(resized.width, 50);
        assert_eq!(resized.height, 50);
        assert_eq!(resized.data.len(), 50 * 50 * 4);
    }

    #[test]
    fn test_bilinear_resize_upscale() {
        let buf = make_test_buffer(10, 10);
        let resized = bilinear_resize(&buf, 20, 20);
        assert_eq!(resized.width, 20);
        assert_eq!(resized.height, 20);
    }

    #[test]
    fn test_bilinear_resize_to_single_pixel() {
        let buf = make_test_buffer(10, 10);
        let resized = bilinear_resize(&buf, 1, 1);
        assert_eq!(resized.width, 1);
        assert_eq!(resized.height, 1);
    }

    #[test]
    fn test_bilinear_resize_zero_target() {
        let buf = make_test_buffer(10, 10);
        let resized = bilinear_resize(&buf, 0, 0);
        assert_eq!(resized.width, 0);
        assert!(resized.data.is_empty());
    }

    // ── Lanczos resize ──

    #[test]
    fn test_lanczos_resize_downscale() {
        let buf = make_test_buffer(100, 100);
        let resized = lanczos_resize(&buf, 50, 50);
        assert_eq!(resized.width, 50);
        assert_eq!(resized.height, 50);
    }

    #[test]
    fn test_lanczos_resize_identity() {
        let buf = make_test_buffer(10, 10);
        let resized = lanczos_resize(&buf, 10, 10);
        assert_eq!(resized.data, buf.data);
    }

    #[test]
    fn test_lanczos_resize_upscale() {
        let buf = make_test_buffer(10, 10);
        let resized = lanczos_resize(&buf, 30, 30);
        assert_eq!(resized.width, 30);
        assert_eq!(resized.height, 30);
    }

    // ── Fit mode calculations ──

    #[test]
    fn test_fit_contain_landscape() {
        assert_eq!(fit_contain_dims(200, 100, 100, 100), (100, 50));
    }

    #[test]
    fn test_fit_contain_portrait() {
        assert_eq!(fit_contain_dims(100, 200, 100, 100), (50, 100));
    }

    #[test]
    fn test_fit_cover_landscape() {
        assert_eq!(fit_cover_dims(200, 100, 100, 100), (200, 100));
    }

    // ── Resize with fit modes ──

    #[test]
    fn test_apply_resize_scale_down_no_change() {
        let buf = make_test_buffer(50, 50);
        let out = apply_resize(buf, 100, 100, FitMode::ScaleDown, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 50);
        assert_eq!(out.height, 50);
    }

    #[test]
    fn test_apply_resize_scale_down_shrinks() {
        let buf = make_test_buffer(100, 100);
        let out = apply_resize(buf, 50, 50, FitMode::ScaleDown, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 50);
        assert_eq!(out.height, 50);
    }

    #[test]
    fn test_apply_resize_contain() {
        let buf = make_test_buffer(200, 100);
        let out = apply_resize(buf, 100, 100, FitMode::Contain, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 100);
        assert_eq!(out.height, 50);
    }

    #[test]
    fn test_apply_resize_cover() {
        let buf = make_test_buffer(200, 100);
        let out = apply_resize(buf, 100, 100, FitMode::Cover, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 100);
        assert_eq!(out.height, 100);
    }

    #[test]
    fn test_apply_resize_fill() {
        let buf = make_test_buffer(200, 100);
        let out = apply_resize(buf, 50, 75, FitMode::Fill, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 50);
        assert_eq!(out.height, 75);
    }

    #[test]
    fn test_apply_resize_pad() {
        let buf = make_test_buffer(100, 50);
        let out = apply_resize(buf, 100, 100, FitMode::Pad, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 100);
        assert_eq!(out.height, 100);
    }

    #[test]
    fn test_apply_resize_crop() {
        let buf = make_test_buffer(200, 200);
        let out = apply_resize(buf, 100, 100, FitMode::Crop, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 100);
        assert_eq!(out.height, 100);
    }

    #[test]
    fn test_apply_resize_width_only() {
        let buf = make_test_buffer(200, 100);
        let out = apply_resize(buf, 100, 0, FitMode::Contain, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 100);
        assert_eq!(out.height, 50);
    }

    #[test]
    fn test_apply_resize_height_only() {
        let buf = make_test_buffer(200, 100);
        let out = apply_resize(buf, 0, 50, FitMode::Contain, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 100);
        assert_eq!(out.height, 50);
    }

    #[test]
    fn test_apply_resize_empty_source() {
        let buf = make_test_buffer(0, 0);
        let out = apply_resize(buf, 100, 100, FitMode::Contain, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 0);
    }

    // ── Rotation ──

    #[test]
    fn test_rotate_deg0_identity() {
        let buf = make_test_buffer(10, 20);
        let out = apply_rotation(buf.clone(), Rotation::Deg0).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_rotate_90() {
        let buf = make_test_buffer(10, 20);
        let out = apply_rotation(buf, Rotation::Deg90).expect("ok");
        assert_eq!(out.width, 20);
        assert_eq!(out.height, 10);
    }

    #[test]
    fn test_rotate_180() {
        let buf = make_test_buffer(10, 20);
        let out = apply_rotation(buf, Rotation::Deg180).expect("ok");
        assert_eq!(out.width, 10);
        assert_eq!(out.height, 20);
    }

    #[test]
    fn test_rotate_270() {
        let buf = make_test_buffer(10, 20);
        let out = apply_rotation(buf, Rotation::Deg270).expect("ok");
        assert_eq!(out.width, 20);
        assert_eq!(out.height, 10);
    }

    #[test]
    fn test_rotate_auto_noop() {
        let buf = make_test_buffer(10, 20);
        let out = apply_rotation(buf.clone(), Rotation::Auto).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_rotate_90_pixel_mapping() {
        let mut buf = PixelBuffer::new(3, 2, 4);
        buf.set_pixel(0, 0, &[255, 0, 0, 255]);
        let rotated = apply_rotation(buf, Rotation::Deg90).expect("ok");
        assert_eq!(rotated.width, 2);
        assert_eq!(rotated.height, 3);
        assert_eq!(rotated.get_pixel(1, 0).expect("pixel")[0], 255);
    }

    #[test]
    fn test_rotate_single_pixel() {
        let mut buf = PixelBuffer::new(1, 1, 4);
        buf.set_pixel(0, 0, &[42, 84, 126, 200]);
        for rot in [Rotation::Deg90, Rotation::Deg180, Rotation::Deg270] {
            let rotated = apply_rotation(buf.clone(), rot).expect("ok");
            assert_eq!(rotated.get_pixel(0, 0).expect("pixel"), &[42, 84, 126, 200]);
        }
    }

    // ── Brightness ──

    #[test]
    fn test_brightness_zero_is_noop() {
        let buf = make_solid_buffer(4, 4, [128, 128, 128, 255]);
        let out = apply_brightness(buf.clone(), 0.0).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_brightness_positive() {
        let buf = make_solid_buffer(2, 2, [100, 100, 100, 255]);
        let out = apply_brightness(buf, 0.5).expect("ok");
        let p = out.get_pixel(0, 0).expect("pixel");
        assert_eq!(p[0], 228); // 100 + round(0.5*255) = 100 + 128 = 228
        assert_eq!(p[3], 255); // alpha unchanged
    }

    #[test]
    fn test_brightness_negative_clamps_to_zero() {
        let buf = make_solid_buffer(2, 2, [100, 100, 100, 255]);
        let out = apply_brightness(buf, -0.5).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 0);
    }

    #[test]
    fn test_brightness_clamp_to_white() {
        let buf = make_solid_buffer(2, 2, [200, 200, 200, 255]);
        let out = apply_brightness(buf, 1.0).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 255);
    }

    // ── Contrast ──

    #[test]
    fn test_contrast_zero_is_noop() {
        let buf = make_solid_buffer(4, 4, [128, 128, 128, 255]);
        let out = apply_contrast(buf.clone(), 0.0).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_contrast_positive_amplifies() {
        let buf = make_solid_buffer(2, 2, [200, 200, 200, 255]);
        let out = apply_contrast(buf, 0.5).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 236);
    }

    #[test]
    fn test_contrast_negative_reduces() {
        let buf = make_solid_buffer(2, 2, [200, 200, 200, 255]);
        let out = apply_contrast(buf, -0.5).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 164);
    }

    #[test]
    fn test_contrast_midpoint_unchanged() {
        let buf = make_solid_buffer(2, 2, [128, 128, 128, 255]);
        let out = apply_contrast(buf, 1.0).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 128);
    }

    // ── Gamma ──

    #[test]
    fn test_gamma_identity() {
        let buf = make_solid_buffer(2, 2, [100, 100, 100, 255]);
        let out = apply_gamma(buf, 1.0).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 100);
    }

    #[test]
    fn test_gamma_brightens() {
        let buf = make_solid_buffer(2, 2, [100, 100, 100, 255]);
        let out = apply_gamma(buf, 2.2).expect("ok");
        assert!(out.get_pixel(0, 0).expect("pixel")[0] > 100);
    }

    #[test]
    fn test_gamma_darkens() {
        let buf = make_solid_buffer(2, 2, [200, 200, 200, 255]);
        let out = apply_gamma(buf, 0.5).expect("ok");
        assert!(out.get_pixel(0, 0).expect("pixel")[0] < 200);
    }

    #[test]
    fn test_gamma_zero_error() {
        let buf = make_solid_buffer(2, 2, [100, 100, 100, 255]);
        assert!(apply_gamma(buf, 0.0).is_err());
    }

    #[test]
    fn test_gamma_preserves_alpha() {
        let buf = make_solid_buffer(2, 2, [100, 100, 100, 128]);
        let out = apply_gamma(buf, 2.2).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[3], 128);
    }

    #[test]
    fn test_gamma_fixed_points_black_white() {
        let mut buf = PixelBuffer::new(2, 1, 4);
        buf.set_pixel(0, 0, &[0, 0, 0, 255]);
        buf.set_pixel(1, 0, &[255, 255, 255, 255]);
        let out = apply_gamma(buf, 2.2).expect("ok");
        assert_eq!(out.get_pixel(0, 0).expect("pixel")[0], 0);
        assert_eq!(out.get_pixel(1, 0).expect("pixel")[0], 255);
    }

    // ── Blur ──

    #[test]
    fn test_blur_zero_is_noop() {
        let buf = make_test_buffer(10, 10);
        let out = apply_blur(buf.clone(), 0.0).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_blur_preserves_dimensions() {
        let buf = make_test_buffer(50, 30);
        let out = apply_blur(buf, 5.0).expect("ok");
        assert_eq!(out.width, 50);
        assert_eq!(out.height, 30);
    }

    #[test]
    fn test_blur_solid_color_unchanged() {
        let buf = make_solid_buffer(10, 10, [128, 128, 128, 255]);
        let out = apply_blur(buf, 3.0).expect("ok");
        let p = out.get_pixel(5, 5).expect("pixel");
        assert_eq!(p[0], 128);
    }

    #[test]
    fn test_blur_reduces_sharp_edge() {
        let mut buf = PixelBuffer::new(20, 1, 4);
        for x in 0..10 {
            buf.set_pixel(x, 0, &[0, 0, 0, 255]);
        }
        for x in 10..20 {
            buf.set_pixel(x, 0, &[255, 255, 255, 255]);
        }
        let out = apply_blur(buf, 2.0).expect("ok");
        assert!(out.get_pixel(9, 0).expect("pixel")[0] > 0);
        assert!(out.get_pixel(10, 0).expect("pixel")[0] < 255);
    }

    // ── Sharpen ──

    #[test]
    fn test_sharpen_zero_is_noop() {
        let buf = make_test_buffer(10, 10);
        let out = apply_sharpen(buf.clone(), 0.0).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_sharpen_preserves_dimensions() {
        let buf = make_test_buffer(20, 20);
        let out = apply_sharpen(buf, 1.0).expect("ok");
        assert_eq!(out.width, 20);
        assert_eq!(out.height, 20);
    }

    #[test]
    fn test_sharpen_solid_color_unchanged() {
        let buf = make_solid_buffer(10, 10, [128, 128, 128, 255]);
        let out = apply_sharpen(buf, 2.0).expect("ok");
        let p = out.get_pixel(5, 5).expect("pixel");
        assert!((p[0] as i32 - 128).abs() <= 1);
    }

    // ── Trim ──

    #[test]
    fn test_trim_all_sides() {
        let buf = make_test_buffer(20, 20);
        let trim = Trim {
            top: 2,
            right: 3,
            bottom: 4,
            left: 5,
        };
        let out = apply_trim(buf, &trim).expect("ok");
        assert_eq!(out.width, 12); // 20 - 5 - 3
        assert_eq!(out.height, 14); // 20 - 2 - 4
    }

    #[test]
    fn test_trim_uniform() {
        let buf = make_test_buffer(20, 20);
        let trim = Trim::uniform(3);
        let out = apply_trim(buf, &trim).expect("ok");
        assert_eq!(out.width, 14);
        assert_eq!(out.height, 14);
    }

    #[test]
    fn test_trim_exceeds_dimensions() {
        let buf = make_test_buffer(10, 10);
        let trim = Trim::uniform(100);
        let out = apply_trim(buf, &trim).expect("ok");
        assert_eq!(out.width, 0);
    }

    #[test]
    fn test_trim_empty_buffer() {
        let buf = PixelBuffer::new(0, 0, 4);
        let out = apply_trim(buf, &Trim::uniform(5)).expect("ok");
        assert_eq!(out.width, 0);
    }

    // ── Border ──

    #[test]
    fn test_border_adds_size() {
        let buf = make_test_buffer(10, 10);
        let border = Border::uniform(5, Color::new(255, 0, 0, 255));
        let out = apply_border(buf, &border).expect("ok");
        assert_eq!(out.width, 20);
        assert_eq!(out.height, 20);
    }

    #[test]
    fn test_border_color_applied() {
        let buf = make_test_buffer(4, 4);
        let border = Border::uniform(2, Color::new(255, 0, 0, 255));
        let out = apply_border(buf, &border).expect("ok");
        let p = out.get_pixel(0, 0).expect("pixel");
        assert_eq!(p[0], 255);
        assert_eq!(p[1], 0);
        assert_eq!(p[2], 0);
    }

    #[test]
    fn test_border_asymmetric() {
        let buf = make_test_buffer(10, 10);
        let border = Border {
            color: Color::black(),
            top: 1,
            right: 2,
            bottom: 3,
            left: 4,
        };
        let out = apply_border(buf, &border).expect("ok");
        assert_eq!(out.width, 16); // 10 + 4 + 2
        assert_eq!(out.height, 14); // 10 + 1 + 3
    }

    // ── Padding ──

    #[test]
    fn test_padding_uniform() {
        let buf = make_test_buffer(100, 100);
        let padding = Padding::uniform(0.1); // 10% each side
        let out = apply_padding(buf, &padding, Color::white()).expect("ok");
        assert_eq!(out.width, 120); // 100 + 10 + 10
        assert_eq!(out.height, 120);
    }

    #[test]
    fn test_padding_asymmetric() {
        let buf = make_test_buffer(100, 100);
        let padding = Padding {
            top: 0.05,
            right: 0.1,
            bottom: 0.15,
            left: 0.2,
        };
        let out = apply_padding(buf, &padding, Color::black()).expect("ok");
        // left: round(0.2*100)=20, right: round(0.1*100)=10 -> 130
        assert_eq!(out.width, 130);
        // top: round(0.05*100)=5, bottom: round(0.15*100)=15 -> 120
        assert_eq!(out.height, 120);
    }

    // ── Crop rect calculations ──

    #[test]
    fn test_crop_rect_center() {
        let (x, y, w, h) = calculate_crop_rect(200, 200, 100, 100, &Gravity::Center);
        assert_eq!((x, y, w, h), (50, 50, 100, 100));
    }

    #[test]
    fn test_crop_rect_top_left() {
        let (x, y, w, h) = calculate_crop_rect(200, 200, 100, 100, &Gravity::TopLeft);
        assert_eq!((x, y, w, h), (0, 0, 100, 100));
    }

    #[test]
    fn test_crop_rect_bottom_right() {
        let (x, y, w, h) = calculate_crop_rect(200, 200, 100, 100, &Gravity::BottomRight);
        assert_eq!((x, y, w, h), (100, 100, 100, 100));
    }

    #[test]
    fn test_crop_rect_focal_point() {
        let gravity = Gravity::FocalPoint(0.25, 0.75);
        let (x, y, w, h) = calculate_crop_rect(200, 200, 100, 100, &gravity);
        assert_eq!((w, h), (100, 100));
        assert_eq!(x, 25);
        assert_eq!(y, 75);
    }

    #[test]
    fn test_crop_rect_larger_than_source() {
        let (x, y, w, h) = calculate_crop_rect(50, 50, 100, 100, &Gravity::Center);
        assert_eq!((x, y, w, h), (0, 0, 50, 50));
    }

    // ── Gravity fractions ──

    #[test]
    fn test_gravity_to_fractions() {
        assert_eq!(gravity_to_fractions(&Gravity::TopLeft), (0.0, 0.0));
        assert_eq!(gravity_to_fractions(&Gravity::Center), (0.5, 0.5));
        assert_eq!(gravity_to_fractions(&Gravity::BottomRight), (1.0, 1.0));
        assert_eq!(gravity_to_fractions(&Gravity::Top), (0.5, 0.0));
        assert_eq!(gravity_to_fractions(&Gravity::Bottom), (0.5, 1.0));
        assert_eq!(gravity_to_fractions(&Gravity::Left), (0.0, 0.5));
        assert_eq!(gravity_to_fractions(&Gravity::Right), (1.0, 0.5));
    }

    #[test]
    fn test_gravity_focal_point_fractions() {
        let (fx, fy) = gravity_to_fractions(&Gravity::FocalPoint(0.3, 0.7));
        assert!((fx - 0.3).abs() < 1e-6);
        assert!((fy - 0.7).abs() < 1e-6);
    }

    // ── Gaussian kernel ──

    #[test]
    fn test_gaussian_kernel_sums_to_one() {
        let kernel = build_gaussian_kernel(2.0);
        let sum: f64 = kernel.iter().sum();
        assert!((sum - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_gaussian_kernel_symmetric() {
        let kernel = build_gaussian_kernel(3.0);
        let n = kernel.len();
        for i in 0..n / 2 {
            assert!((kernel[i] - kernel[n - 1 - i]).abs() < 1e-10);
        }
    }

    #[test]
    fn test_gaussian_kernel_peak_at_center() {
        let kernel = build_gaussian_kernel(2.0);
        let center = kernel.len() / 2;
        for (i, &v) in kernel.iter().enumerate() {
            if i != center {
                assert!(v <= kernel[center]);
            }
        }
    }

    // ── Lanczos kernel ──

    #[test]
    fn test_lanczos3_kernel_at_zero() {
        assert!((lanczos3_kernel(0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_lanczos3_kernel_at_boundary() {
        assert!(lanczos3_kernel(3.0).abs() < 1e-10);
        assert!(lanczos3_kernel(-3.0).abs() < 1e-10);
    }

    #[test]
    fn test_lanczos3_kernel_outside_support() {
        assert_eq!(lanczos3_kernel(4.0), 0.0);
        assert_eq!(lanczos3_kernel(-5.0), 0.0);
    }

    // ── Pipeline building and ordering ──

    #[test]
    fn test_build_pipeline_empty() {
        let params = TransformParams::default();
        let pipeline = build_pipeline(&params, OutputFormat::Auto);
        assert!(pipeline.is_empty());
    }

    #[test]
    fn test_build_pipeline_ordering() {
        let mut params = TransformParams::default();
        params.trim = Some(Trim::uniform(5));
        params.width = Some(100);
        params.height = Some(100);
        params.rotate = Rotation::Deg90;
        params.brightness = 0.5;
        params.contrast = 0.2;
        params.gamma = 2.2;
        params.sharpen = 1.0;
        params.blur = 2.0;
        params.border = Some(Border::uniform(5, Color::black()));
        params.pad = Some(Padding::uniform(0.1));

        let pipeline = build_pipeline(&params, OutputFormat::Auto);
        assert!(pipeline.len() >= 10);

        assert!(matches!(pipeline[0], PipelineStep::Trim(_)));
        assert!(matches!(pipeline[1], PipelineStep::Resize { .. }));
        assert!(matches!(pipeline[2], PipelineStep::Rotate(_)));
        assert!(matches!(pipeline[3], PipelineStep::Brightness(_)));
        assert!(matches!(pipeline[4], PipelineStep::Contrast(_)));
        assert!(matches!(pipeline[5], PipelineStep::Gamma(_)));
        assert!(matches!(pipeline[6], PipelineStep::Sharpen(_)));
        assert!(matches!(pipeline[7], PipelineStep::Blur(_)));
        assert!(matches!(pipeline[8], PipelineStep::AddBorder(_)));
        assert!(matches!(pipeline[9], PipelineStep::AddPadding(_, _)));
    }

    #[test]
    fn test_build_pipeline_skips_identity_values() {
        let params = TransformParams::default();
        let pipeline = build_pipeline(&params, OutputFormat::Auto);
        assert!(pipeline.is_empty());
    }

    // ── Full pipeline integration ──

    #[test]
    fn test_apply_transforms_identity() {
        let mut buf = make_test_buffer(50, 50);
        let params = TransformParams::default();
        let out = apply_transforms(&mut buf, &params).expect("ok");
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn test_apply_transforms_resize_and_rotate() {
        let mut buf = make_test_buffer(100, 50);
        let mut params = TransformParams::default();
        params.width = Some(50);
        params.height = Some(25);
        params.fit = FitMode::Fill;
        params.rotate = Rotation::Deg90;

        let out = apply_transforms(&mut buf, &params).expect("ok");
        assert_eq!(out.width, 25);
        assert_eq!(out.height, 50);
    }

    #[test]
    fn test_apply_transforms_color_adjustments() {
        let mut buf = make_solid_buffer(10, 10, [128, 128, 128, 255]);
        let mut params = TransformParams::default();
        params.brightness = 0.1;
        params.contrast = 0.2;
        params.gamma = 1.5;
        assert!(apply_transforms(&mut buf, &params).is_ok());
    }

    #[test]
    fn test_apply_transforms_border_and_padding() {
        let mut buf = make_test_buffer(10, 10);
        let mut params = TransformParams::default();
        params.border = Some(Border::uniform(2, Color::new(255, 0, 0, 255)));
        params.pad = Some(Padding::uniform(0.5)); // 50% of current dims

        let out = apply_transforms(&mut buf, &params).expect("ok");
        // After border: 14x14
        // Padding: 50% of 14 = 7 each side -> 14 + 14 = 28
        assert_eq!(out.width, 28);
        assert_eq!(out.height, 28);
    }

    // ── Edge cases ──

    #[test]
    fn test_large_resize_from_single_pixel() {
        let buf = make_test_buffer(1, 1);
        let out = apply_resize(buf, 1000, 1000, FitMode::Fill, &Gravity::Center).expect("ok");
        assert_eq!(out.width, 1000);
        assert_eq!(out.height, 1000);
    }

    #[test]
    fn test_rgb_buffer_brightness() {
        let data = vec![128u8; 10 * 10 * 3];
        let buf = PixelBuffer::from_rgb(data, 10, 10).expect("valid");
        let out = apply_brightness(buf, 0.1).expect("ok");
        assert_eq!(out.channels, 3);
        assert!(out.data[0] > 128);
    }

    #[test]
    fn test_rgb_buffer_resize() {
        let data = vec![128u8; 10 * 10 * 3];
        let buf = PixelBuffer::from_rgb(data, 10, 10).expect("valid");
        let resized = bilinear_resize(&buf, 5, 5);
        assert_eq!(resized.width, 5);
        assert_eq!(resized.channels, 3);
    }

    #[test]
    fn test_negative_brightness_per_channel() {
        let buf = make_solid_buffer(4, 4, [50, 100, 150, 255]);
        let out = apply_brightness(buf, -0.3).expect("ok");
        let p = out.get_pixel(0, 0).expect("pixel");
        assert_eq!(p[0], 0); // 50 - 77 < 0 -> 0
        assert_eq!(p[1], 23); // 100 - 77
        assert_eq!(p[2], 73); // 150 - 77
    }

    // ── Integration tests: actual pixel data through the full parse → process pipeline ──

    /// Parse a CDN transform URL and apply the resulting transforms to a
    /// synthetic image buffer.  Verifies the full pipeline without I/O.
    #[test]
    fn test_integration_parse_and_apply_resize() {
        use crate::parser::parse_cdn_url;

        let req =
            parse_cdn_url("/cdn-cgi/image/width=8,height=8/photo.jpg").expect("parse must succeed");

        // 16×16 RGBA gradient image.
        let mut buf = PixelBuffer::new(16, 16, 4);
        for y in 0u32..16 {
            for x in 0u32..16 {
                let r = (x * 16) as u8;
                let g = (y * 16) as u8;
                buf.set_pixel(x, y, &[r, g, 64, 255]);
            }
        }

        let out = apply_transforms(&mut buf, &req.params).expect("apply_transforms must succeed");

        assert_eq!(out.width, 8, "output width should be 8");
        assert_eq!(out.height, 8, "output height should be 8");
        assert_eq!(out.channels, 4, "channels must be preserved");
    }

    /// Verify that quality=100 is accepted and produces a valid transform.
    #[test]
    fn test_integration_quality_bounds() {
        let mut params = TransformParams::default();
        params.quality = 100;
        params.width = Some(4);
        params.height = Some(4);

        let mut buf = make_solid_buffer(8, 8, [200, 150, 100, 255]);
        let out = apply_transforms(&mut buf, &params).expect("quality=100 must be accepted");
        assert_eq!(out.width, 4);
        assert_eq!(out.height, 4);
    }

    /// Verify that a 1×1 source image can be upscaled without panicking.
    #[test]
    fn test_integration_upscale_single_pixel() {
        let mut params = TransformParams::default();
        params.width = Some(64);
        params.height = Some(64);
        params.fit = FitMode::Fill;

        let mut buf = make_solid_buffer(1, 1, [128, 64, 32, 255]);
        let out = apply_transforms(&mut buf, &params).expect("1×1 upscale must succeed");
        assert_eq!(out.width, 64);
        assert_eq!(out.height, 64);
    }

    // ── Early-termination fast-path ──

    /// Resize to a sub-threshold target (8×8 = 64 pixels < 4096 threshold).
    /// Verifies that the early-termination path is taken and output dimensions
    /// are correct without panicking.
    #[test]
    fn test_early_termination_small_output() {
        // 200×150 source → 8×8 target (well below the 64×64 pixel threshold)
        let src = make_test_buffer(200, 150);
        let result = apply_resize(src, 8, 8, FitMode::Fill, &Gravity::Center)
            .expect("small target resize must succeed");
        assert_eq!(result.width, 8);
        assert_eq!(result.height, 8);
        assert_eq!(result.data.len(), 8 * 8 * 4);
    }

    /// Target is below threshold for each dimension separately: 32×32 = 1024 px.
    #[test]
    fn test_early_termination_contain_small_output() {
        let src = make_test_buffer(400, 300);
        let result = apply_resize(src, 32, 32, FitMode::Contain, &Gravity::Center)
            .expect("small contain resize must succeed");
        // Contain preserves aspect ratio: 400×300 → fits in 32×32.
        // Width-limited: 32×(300/400*32) = 32×24.
        assert!(result.width <= 32 && result.height <= 32);
        assert!(result.data.len() == result.width as usize * result.height as usize * 4);
    }

    /// Resize to a target above the threshold (256×256 = 65536 pixels > 4096).
    /// Verifies that the normal dispatch path is taken and the output is correct.
    #[test]
    fn test_above_threshold_no_early_exit() {
        // 512×512 source → 256×256 target (above threshold, normal path)
        let src = make_test_buffer(512, 512);
        let result = apply_resize(src, 256, 256, FitMode::Fill, &Gravity::Center)
            .expect("above-threshold resize must succeed");
        assert_eq!(result.width, 256);
        assert_eq!(result.height, 256);
        assert_eq!(result.data.len(), 256 * 256 * 4);
    }

    /// Threshold boundary: exactly 64×64 = 4096 pixels is NOT below the threshold
    /// (we use strict `<`), so the normal path applies.
    #[test]
    fn test_threshold_boundary_exact() {
        let src = make_test_buffer(512, 512);
        let result = apply_resize(src, 64, 64, FitMode::Fill, &Gravity::Center)
            .expect("boundary resize must succeed");
        assert_eq!(result.width, 64);
        assert_eq!(result.height, 64);
    }
}

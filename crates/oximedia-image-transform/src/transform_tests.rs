// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Unit tests for [`super`] (the `transform` module).
//!
//! Split out of `transform.rs` to keep that file under the 2000-line limit.

use super::*;

// ── Default / identity ──

#[test]
fn test_default_params() {
    let p = TransformParams::default();
    assert_eq!(p.quality, 85);
    assert_eq!(p.fit, FitMode::ScaleDown);
    assert_eq!(p.metadata, MetadataMode::None);
    assert!(p.anim);
    assert_eq!(p.gravity, Gravity::Center);
    assert!((p.dpr - 1.0).abs() < f64::EPSILON);
    assert!((p.gamma - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_identity() {
    let p = TransformParams::default();
    assert!(p.is_identity());

    let mut p2 = TransformParams::default();
    p2.width = Some(800);
    assert!(!p2.is_identity());
}

// ── Effective dimensions ──

#[test]
fn test_effective_width_no_dpr() {
    let mut p = TransformParams::default();
    p.width = Some(800);
    assert_eq!(p.effective_width(), Some(800));
}

#[test]
fn test_effective_height_no_dpr() {
    let mut p = TransformParams::default();
    p.height = Some(600);
    assert_eq!(p.effective_height(), Some(600));
}

#[test]
fn test_effective_width_with_dpr() {
    let mut p = TransformParams::default();
    p.width = Some(400);
    p.dpr = 2.0;
    assert_eq!(p.effective_width(), Some(800));
}

#[test]
fn test_effective_height_with_dpr() {
    let mut p = TransformParams::default();
    p.height = Some(300);
    p.dpr = 2.0;
    assert_eq!(p.effective_height(), Some(600));
}

#[test]
fn test_effective_width_clamped() {
    let mut p = TransformParams::default();
    p.width = Some(10000);
    p.dpr = 3.0;
    // 10000 * 3 = 30000 > MAX_DIMENSION, clamped
    assert_eq!(p.effective_width(), Some(MAX_DIMENSION));
}

#[test]
fn test_effective_none() {
    let p = TransformParams::default();
    assert!(p.effective_width().is_none());
    assert!(p.effective_height().is_none());
}

// ── Validation ──

#[test]
fn test_validate_valid() {
    let mut p = TransformParams::default();
    p.width = Some(800);
    p.height = Some(600);
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_zero_width() {
    let mut p = TransformParams::default();
    p.width = Some(0);
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_exceed_dimension() {
    let mut p = TransformParams::default();
    p.width = Some(20000);
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_quality_zero() {
    let mut p = TransformParams::default();
    p.quality = 0;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_quality_101() {
    let mut p = TransformParams::default();
    p.quality = 101;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_quality_100() {
    let mut p = TransformParams::default();
    p.quality = 100;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_dpr_low() {
    let mut p = TransformParams::default();
    p.dpr = 0.5;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_dpr_high() {
    let mut p = TransformParams::default();
    p.dpr = 5.0;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_dpr_ok() {
    let mut p = TransformParams::default();
    p.dpr = 2.0;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_sharpen_ok() {
    let mut p = TransformParams::default();
    p.sharpen = 5.0;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_sharpen_too_high() {
    let mut p = TransformParams::default();
    p.sharpen = 11.0;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_sharpen_negative() {
    let mut p = TransformParams::default();
    p.sharpen = -0.1;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_blur_ok() {
    let mut p = TransformParams::default();
    p.blur = 100.0;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_blur_too_high() {
    let mut p = TransformParams::default();
    p.blur = 251.0;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_brightness_ok() {
    let mut p = TransformParams::default();
    p.brightness = 0.5;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_brightness_too_low() {
    let mut p = TransformParams::default();
    p.brightness = -1.1;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_brightness_too_high() {
    let mut p = TransformParams::default();
    p.brightness = 1.1;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_contrast_ok() {
    let mut p = TransformParams::default();
    p.contrast = -1.0;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_contrast_too_high() {
    let mut p = TransformParams::default();
    p.contrast = 1.5;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_gamma_ok() {
    let mut p = TransformParams::default();
    p.gamma = 2.2;
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_gamma_too_high() {
    let mut p = TransformParams::default();
    p.gamma = 11.0;
    assert!(p.validate().is_err());
}

#[test]
fn test_validate_focal_point_ok() {
    let mut p = TransformParams::default();
    p.gravity = Gravity::FocalPoint(0.5, 0.5);
    assert!(p.validate().is_ok());
}

#[test]
fn test_validate_focal_point_out_of_range() {
    let mut p = TransformParams::default();
    p.gravity = Gravity::FocalPoint(1.1, 0.5);
    assert!(p.validate().is_err());
}

// ── FitMode ──

#[test]
fn test_fit_parse() {
    assert_eq!(
        FitMode::from_str_loose("scale-down").ok(),
        Some(FitMode::ScaleDown)
    );
    assert_eq!(
        FitMode::from_str_loose("contain").ok(),
        Some(FitMode::Contain)
    );
    assert_eq!(FitMode::from_str_loose("cover").ok(), Some(FitMode::Cover));
    assert_eq!(FitMode::from_str_loose("crop").ok(), Some(FitMode::Crop));
    assert_eq!(FitMode::from_str_loose("pad").ok(), Some(FitMode::Pad));
    assert_eq!(FitMode::from_str_loose("fill").ok(), Some(FitMode::Fill));
    assert!(FitMode::from_str_loose("stretch").is_err());
}

#[test]
fn test_fit_as_str() {
    assert_eq!(FitMode::ScaleDown.as_str(), "scale-down");
    assert_eq!(FitMode::Contain.as_str(), "contain");
    assert_eq!(FitMode::Cover.as_str(), "cover");
    assert_eq!(FitMode::Crop.as_str(), "crop");
    assert_eq!(FitMode::Pad.as_str(), "pad");
    assert_eq!(FitMode::Fill.as_str(), "fill");
}

#[test]
fn test_fit_display() {
    assert_eq!(format!("{}", FitMode::Cover), "cover");
}

// ── Gravity ──

#[test]
fn test_gravity_parse_named() {
    assert_eq!(Gravity::from_str_loose("auto").ok(), Some(Gravity::Auto));
    assert_eq!(
        Gravity::from_str_loose("center").ok(),
        Some(Gravity::Center)
    );
    assert_eq!(Gravity::from_str_loose("top").ok(), Some(Gravity::Top));
    assert_eq!(Gravity::from_str_loose("face").ok(), Some(Gravity::Face));
    assert_eq!(
        Gravity::from_str_loose("bottom-right").ok(),
        Some(Gravity::BottomRight)
    );
}

#[test]
fn test_gravity_focal_point_parse() {
    let g = Gravity::from_str_loose("0.3x0.7");
    assert!(g.is_ok());
    if let Ok(Gravity::FocalPoint(x, y)) = g {
        assert!((x - 0.3).abs() < 0.001);
        assert!((y - 0.7).abs() < 0.001);
    }
}

#[test]
fn test_gravity_focal_point_out_of_range() {
    assert!(Gravity::from_str_loose("1.5x0.5").is_err());
    assert!(Gravity::from_str_loose("0.5x-0.1").is_err());
}

#[test]
fn test_gravity_as_str() {
    assert_eq!(Gravity::Center.as_str(), "center");
    assert_eq!(Gravity::FocalPoint(0.5, 0.5).as_str(), "0.5x0.5");
}

#[test]
fn test_gravity_display() {
    assert_eq!(format!("{}", Gravity::TopLeft), "top-left");
}

// ── OutputFormat ──

#[test]
fn test_format_parse() {
    assert_eq!(
        OutputFormat::from_str_loose("auto").ok(),
        Some(OutputFormat::Auto)
    );
    assert_eq!(
        OutputFormat::from_str_loose("AVIF").ok(),
        Some(OutputFormat::Avif)
    );
    assert_eq!(
        OutputFormat::from_str_loose("webp").ok(),
        Some(OutputFormat::WebP)
    );
    assert_eq!(
        OutputFormat::from_str_loose("JPEG").ok(),
        Some(OutputFormat::Jpeg)
    );
    assert_eq!(
        OutputFormat::from_str_loose("jpg").ok(),
        Some(OutputFormat::Jpeg)
    );
    assert_eq!(
        OutputFormat::from_str_loose("png").ok(),
        Some(OutputFormat::Png)
    );
    assert_eq!(
        OutputFormat::from_str_loose("gif").ok(),
        Some(OutputFormat::Gif)
    );
    assert_eq!(
        OutputFormat::from_str_loose("baseline").ok(),
        Some(OutputFormat::Baseline)
    );
    assert_eq!(
        OutputFormat::from_str_loose("json").ok(),
        Some(OutputFormat::Json)
    );
    assert!(OutputFormat::from_str_loose("bmp").is_err());
}

#[test]
fn test_format_mime() {
    assert_eq!(OutputFormat::Avif.mime_type(), "image/avif");
    assert_eq!(OutputFormat::WebP.mime_type(), "image/webp");
    assert_eq!(OutputFormat::Jpeg.mime_type(), "image/jpeg");
    assert_eq!(OutputFormat::Png.mime_type(), "image/png");
    assert_eq!(OutputFormat::Gif.mime_type(), "image/gif");
    assert_eq!(OutputFormat::Json.mime_type(), "application/json");
}

#[test]
fn test_format_extension() {
    assert_eq!(OutputFormat::Avif.file_extension(), "avif");
    assert_eq!(OutputFormat::WebP.file_extension(), "webp");
    assert_eq!(OutputFormat::Jpeg.file_extension(), "jpg");
    assert_eq!(OutputFormat::Json.file_extension(), "json");
}

#[test]
fn test_format_animation() {
    assert!(OutputFormat::Gif.supports_animation());
    assert!(OutputFormat::WebP.supports_animation());
    assert!(OutputFormat::Avif.supports_animation());
    assert!(!OutputFormat::Jpeg.supports_animation());
    assert!(!OutputFormat::Png.supports_animation());
    assert!(!OutputFormat::Json.supports_animation());
}

#[test]
fn test_format_transparency() {
    assert!(OutputFormat::Png.supports_transparency());
    assert!(OutputFormat::WebP.supports_transparency());
    assert!(!OutputFormat::Jpeg.supports_transparency());
    assert!(!OutputFormat::Baseline.supports_transparency());
}

#[test]
fn test_format_as_str() {
    assert_eq!(OutputFormat::Auto.as_str(), "auto");
    assert_eq!(OutputFormat::Avif.as_str(), "avif");
    assert_eq!(OutputFormat::WebP.as_str(), "webp");
    assert_eq!(OutputFormat::Json.as_str(), "json");
}

#[test]
fn test_format_display() {
    assert_eq!(format!("{}", OutputFormat::Avif), "avif");
}

// ── MetadataMode ──

#[test]
fn test_metadata_parse() {
    assert_eq!(
        MetadataMode::from_str_loose("keep").ok(),
        Some(MetadataMode::Keep)
    );
    assert_eq!(
        MetadataMode::from_str_loose("copyright").ok(),
        Some(MetadataMode::Copyright)
    );
    assert_eq!(
        MetadataMode::from_str_loose("none").ok(),
        Some(MetadataMode::None)
    );
    assert_eq!(
        MetadataMode::from_str_loose("strip").ok(),
        Some(MetadataMode::None)
    );
}

// ── Color ──

#[test]
fn test_color_from_hex_6() {
    let c = Color::from_hex("#ff8800").expect("valid hex");
    assert_eq!(c.r, 255);
    assert_eq!(c.g, 136);
    assert_eq!(c.b, 0);
    assert_eq!(c.a, 255);
}

#[test]
fn test_color_from_hex_8() {
    let c = Color::from_hex("#ff880080").expect("valid hex");
    assert_eq!(c.r, 255);
    assert_eq!(c.g, 136);
    assert_eq!(c.b, 0);
    assert_eq!(c.a, 128);
}

#[test]
fn test_color_from_hex_no_hash() {
    let c = Color::from_hex("00ff00").expect("valid hex");
    assert_eq!(c, Color::new(0, 255, 0, 255));
}

#[test]
fn test_color_from_css_rgb() {
    let c = Color::from_css("rgb(128,64,32)").expect("valid rgb");
    assert_eq!(c, Color::new(128, 64, 32, 255));
}

#[test]
fn test_color_from_css_rgba() {
    let c = Color::from_css("rgba(128,64,32,0.5)").expect("valid rgba");
    assert_eq!(c.r, 128);
    assert_eq!(c.g, 64);
    assert_eq!(c.b, 32);
    assert_eq!(c.a, 128); // 0.5 * 255 = 127.5 -> 128 (rounded)
}

#[test]
fn test_color_invalid() {
    assert!(Color::from_hex("xyz").is_err());
    assert!(Color::from_hex("#gg0000").is_err());
    assert!(Color::from_hex("#ff00").is_err());
}

#[test]
fn test_color_to_hex_opaque() {
    let c = Color::new(255, 0, 128, 255);
    assert_eq!(c.to_hex(), "ff0080");
}

#[test]
fn test_color_to_hex_transparent() {
    let c = Color::new(255, 0, 128, 128);
    assert_eq!(c.to_hex(), "ff008080");
}

#[test]
fn test_color_display() {
    let c = Color::new(255, 0, 0, 255);
    assert_eq!(format!("{c}"), "#ff0000");
}

#[test]
fn test_color_presets() {
    assert_eq!(Color::transparent().a, 0);
    assert_eq!(Color::white(), Color::new(255, 255, 255, 255));
    assert_eq!(Color::black(), Color::new(0, 0, 0, 255));
}

// ── Rotation ──

#[test]
fn test_rotation_from_degrees() {
    assert_eq!(Rotation::from_degrees(0).ok(), Some(Rotation::Deg0));
    assert_eq!(Rotation::from_degrees(90).ok(), Some(Rotation::Deg90));
    assert_eq!(Rotation::from_degrees(180).ok(), Some(Rotation::Deg180));
    assert_eq!(Rotation::from_degrees(270).ok(), Some(Rotation::Deg270));
    assert!(Rotation::from_degrees(45).is_err());
}

#[test]
fn test_rotation_from_str() {
    assert_eq!(Rotation::from_str_loose("auto").ok(), Some(Rotation::Auto));
    assert_eq!(Rotation::from_str_loose("90").ok(), Some(Rotation::Deg90));
}

#[test]
fn test_rotation_to_degrees() {
    assert_eq!(Rotation::Deg90.to_degrees(), Some(90));
    assert_eq!(Rotation::Auto.to_degrees(), None);
}

#[test]
fn test_rotation_display() {
    assert_eq!(format!("{}", Rotation::Deg90), "90");
    assert_eq!(format!("{}", Rotation::Auto), "auto");
}

// ── Compression ──

#[test]
fn test_compression_parse() {
    assert_eq!(
        Compression::from_str_loose("fast").ok(),
        Some(Compression::Fast)
    );
    assert_eq!(
        Compression::from_str_loose("default").ok(),
        Some(Compression::Default)
    );
    assert_eq!(
        Compression::from_str_loose("best").ok(),
        Some(Compression::Best)
    );
    assert!(Compression::from_str_loose("invalid").is_err());
}

// ── Border ──

#[test]
fn test_border_uniform() {
    let b = Border::uniform(5, Color::black());
    assert_eq!(b.top, 5);
    assert_eq!(b.right, 5);
    assert_eq!(b.bottom, 5);
    assert_eq!(b.left, 5);
}

// ── Padding ──

#[test]
fn test_padding_uniform() {
    let p = Padding::uniform(0.05);
    assert!((p.top - 0.05).abs() < 1e-9);
    assert!((p.right - 0.05).abs() < 1e-9);
    assert!((p.bottom - 0.05).abs() < 1e-9);
    assert!((p.left - 0.05).abs() < 1e-9);
}

// ── Trim ──

#[test]
fn test_trim_uniform() {
    let t = Trim::uniform(10);
    assert_eq!(t.top, 10);
    assert_eq!(t.right, 10);
    assert_eq!(t.bottom, 10);
    assert_eq!(t.left, 10);
}

// ── Display / cache key ──

#[test]
fn test_display_default_is_empty() {
    let p = TransformParams::default();
    assert_eq!(format!("{p}"), "");
}

#[test]
fn test_display_with_params() {
    let mut p = TransformParams::default();
    p.width = Some(800);
    p.height = Some(600);
    p.quality = 90;
    let s = format!("{p}");
    assert!(s.contains("width=800"));
    assert!(s.contains("height=600"));
    assert!(s.contains("quality=90"));
}

#[test]
fn test_cache_key_deterministic() {
    let mut p = TransformParams::default();
    p.width = Some(800);
    p.quality = 90;
    let k1 = p.cache_key();
    let k2 = p.cache_key();
    assert_eq!(k1, k2);
}

#[test]
fn test_cache_key_excludes_onerror() {
    let mut p = TransformParams::default();
    p.width = Some(800);
    p.onerror = Some("redirect".to_string());
    let key = p.cache_key();
    assert!(!key.contains("onerror"));
}

// ── enforce_aspect_ratio ──

#[test]
fn test_enforce_aspect_ratio_contain_landscape() {
    // 1600×900 inside 800×600: width-limited → 800×450
    let (w, h) = enforce_aspect_ratio(1600, 900, 800, 600, FitMode::Contain);
    assert_eq!(w, 800);
    assert_eq!(h, 450);
}

#[test]
fn test_enforce_aspect_ratio_contain_portrait() {
    // 400×800 inside 800×600: height-limited → 300×600
    let (w, h) = enforce_aspect_ratio(400, 800, 800, 600, FitMode::Contain);
    assert_eq!(w, 300);
    assert_eq!(h, 600);
}

#[test]
fn test_enforce_aspect_ratio_scale_down_behaves_like_contain() {
    let (w1, h1) = enforce_aspect_ratio(1600, 900, 800, 600, FitMode::ScaleDown);
    let (w2, h2) = enforce_aspect_ratio(1600, 900, 800, 600, FitMode::Contain);
    assert_eq!((w1, h1), (w2, h2));
}

#[test]
fn test_enforce_aspect_ratio_cover() {
    // 1600×900 covering 400×400: height-scale (400/900≈0.444) < width-scale (400/1600=0.25)
    // so we use height scale → 1600*(400/900)≈711 × 400
    let (w, h) = enforce_aspect_ratio(1600, 900, 400, 400, FitMode::Cover);
    assert_eq!(h, 400);
    assert!(w >= 400, "cover width {w} must be ≥ 400");
}

#[test]
fn test_enforce_aspect_ratio_crop_behaves_like_cover() {
    let (w1, h1) = enforce_aspect_ratio(800, 600, 200, 200, FitMode::Crop);
    let (w2, h2) = enforce_aspect_ratio(800, 600, 200, 200, FitMode::Cover);
    assert_eq!((w1, h1), (w2, h2));
}

#[test]
fn test_enforce_aspect_ratio_fill_returns_request() {
    let (w, h) = enforce_aspect_ratio(1600, 900, 400, 300, FitMode::Fill);
    assert_eq!(w, 400);
    assert_eq!(h, 300);
}

#[test]
fn test_enforce_aspect_ratio_pad_returns_request() {
    let (w, h) = enforce_aspect_ratio(1600, 900, 400, 300, FitMode::Pad);
    assert_eq!(w, 400);
    assert_eq!(h, 300);
}

#[test]
fn test_enforce_aspect_ratio_zero_src_returns_request() {
    let (w, h) = enforce_aspect_ratio(0, 900, 400, 300, FitMode::Contain);
    assert_eq!(w, 400);
    assert_eq!(h, 300);
}

#[test]
fn test_enforce_aspect_ratio_square_src_cover_square_box() {
    // 200×200 covering 100×100 — already fits exactly at scale 0.5
    let (w, h) = enforce_aspect_ratio(200, 200, 100, 100, FitMode::Cover);
    assert_eq!(w, 100);
    assert_eq!(h, 100);
}

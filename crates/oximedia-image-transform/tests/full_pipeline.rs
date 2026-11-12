// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Integration tests exercising the full `parser -> negotiation -> processor`
//! pipeline against actual (synthetically generated) in-memory image data.
//!
//! These tests build a `PixelBuffer` directly (equivalent to a decoded source
//! image), run it through parsed `/cdn-cgi/image/...` transform strings, and
//! assert on the resulting pixel buffer's dimensions/channel layout -- the
//! same contract a real decode->transform->encode HTTP handler would rely on.

use oximedia_image_transform::negotiation::negotiate_format;
use oximedia_image_transform::parser::parse_cdn_url;
use oximedia_image_transform::processor::{apply_transforms, PixelBuffer};
use oximedia_image_transform::transform::{FitMode, OutputFormat, Rotation};

/// Generate a synthetic RGBA gradient image, standing in for a decoded source
/// photo. Deterministic and dependency-free (no external image files needed).
fn generate_test_image(width: u32, height: u32) -> PixelBuffer {
    let mut buf = PixelBuffer::new(width, height, 4);
    for y in 0..height {
        for x in 0..width {
            let r = ((x * 255) / width.max(1)) as u8;
            let g = ((y * 255) / height.max(1)) as u8;
            let b = 128u8;
            buf.set_pixel(x, y, &[r, g, b, 255]);
        }
    }
    buf
}

#[test]
fn test_pipeline_basic_resize_to_exact_dimensions() {
    // Cover fit fills and crops to the exact requested dimensions.
    let req = parse_cdn_url("/cdn-cgi/image/w=200,h=150,fit=cover/photos/sample.jpg")
        .expect("valid cdn url should parse");
    assert_eq!(req.source_path, "photos/sample.jpg");
    assert_eq!(req.params.width, Some(200));
    assert_eq!(req.params.height, Some(150));
    assert_eq!(req.params.fit, FitMode::Cover);

    let mut source = generate_test_image(800, 600);
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    assert_eq!(output.width, 200);
    assert_eq!(output.height, 150);
    assert_eq!(output.channels, 4);
    assert_eq!(output.data.len(), 200 * 150 * 4);
}

#[test]
fn test_pipeline_fill_stretches_to_exact_dimensions_ignoring_aspect() {
    let req = parse_cdn_url("/cdn-cgi/image/w=400,h=100,fit=fill/banners/wide.png")
        .expect("valid cdn url should parse");

    let mut source = generate_test_image(300, 300); // square source
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    // Fill mode stretches to the exact target regardless of source aspect ratio.
    assert_eq!(output.width, 400);
    assert_eq!(output.height, 100);
}

#[test]
fn test_pipeline_scale_down_never_enlarges() {
    // ScaleDown (the Cloudflare default) must not enlarge a smaller source.
    let req = parse_cdn_url("/cdn-cgi/image/w=1000,h=1000/thumb.jpg")
        .expect("valid cdn url should parse");
    assert_eq!(req.params.fit, FitMode::ScaleDown);

    let mut source = generate_test_image(100, 50);
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    // Source (100x50) already fits within 1000x1000, so dimensions are unchanged.
    assert_eq!(output.width, 100);
    assert_eq!(output.height, 50);
}

#[test]
fn test_pipeline_scale_down_shrinks_oversized_source_preserving_aspect() {
    let req =
        parse_cdn_url("/cdn-cgi/image/w=100,h=100/photo.jpg").expect("valid cdn url should parse");

    let mut source = generate_test_image(800, 400); // 2:1 aspect ratio
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    // Contain-style scale-down: fits within 100x100 while preserving 2:1 aspect.
    assert_eq!(output.width, 100);
    assert_eq!(output.height, 50);
}

#[test]
fn test_pipeline_width_only_derives_height_from_aspect_ratio() {
    let req = parse_cdn_url("/cdn-cgi/image/w=160/photo.jpg").expect("valid cdn url should parse");
    assert_eq!(req.params.width, Some(160));
    assert_eq!(req.params.height, None);

    let mut source = generate_test_image(640, 480); // 4:3 aspect ratio
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    assert_eq!(output.width, 160);
    assert_eq!(output.height, 120); // 640:480 == 160:120
}

#[test]
fn test_pipeline_rotate_90_swaps_dimensions() {
    let req = parse_cdn_url("/cdn-cgi/image/w=120,h=80,fit=fill,rotate=90/photo.jpg")
        .expect("valid cdn url should parse");
    assert_eq!(req.params.rotate, Rotation::Deg90);

    let mut source = generate_test_image(300, 300);
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    // Resize (fill -> exactly 120x80) runs before rotation in the pipeline;
    // a 90-degree rotation then swaps width/height of that resized result.
    assert_eq!(output.width, 80);
    assert_eq!(output.height, 120);
}

#[test]
fn test_pipeline_query_string_and_cdn_path_agree() {
    use oximedia_image_transform::parser::parse_query_params;

    let cdn_req = parse_cdn_url("/cdn-cgi/image/w=300,h=200,fit=cover,q=70,f=webp/img.jpg")
        .expect("valid cdn url should parse");
    let query_params = parse_query_params("width=300&height=200&fit=cover&quality=70&format=webp")
        .expect("valid query string should parse");

    assert_eq!(cdn_req.params.width, query_params.width);
    assert_eq!(cdn_req.params.height, query_params.height);
    assert_eq!(cdn_req.params.fit, query_params.fit);
    assert_eq!(cdn_req.params.quality, query_params.quality);
    assert_eq!(cdn_req.params.format, query_params.format);

    let mut source_a = generate_test_image(1000, 700);
    let mut source_b = generate_test_image(1000, 700);
    let out_a = apply_transforms(&mut source_a, &cdn_req.params).expect("transform ok");
    let out_b = apply_transforms(&mut source_b, &query_params).expect("transform ok");

    assert_eq!((out_a.width, out_a.height), (out_b.width, out_b.height));
    assert_eq!(out_a.width, 300);
    assert_eq!(out_a.height, 200);
}

#[test]
fn test_pipeline_auto_format_negotiates_with_accept_header() {
    let req = parse_cdn_url("/cdn-cgi/image/w=50,h=50,f=auto/img.jpg")
        .expect("valid cdn url should parse");
    assert_eq!(req.params.format, OutputFormat::Auto);

    let negotiated = negotiate_format(
        "image/avif,image/webp;q=0.9,image/jpeg;q=0.8",
        req.params.format,
    );
    assert_eq!(negotiated, OutputFormat::Avif);

    // Pixel pipeline is independent of output container format; it still
    // must produce the requested geometry regardless of negotiated format.
    let mut source = generate_test_image(200, 200);
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");
    assert_eq!(output.width, 50);
    assert_eq!(output.height, 50);
}

#[test]
fn test_pipeline_border_and_padding_increase_output_dimensions() {
    let req =
        parse_cdn_url("/cdn-cgi/image/w=100,h=100,fit=fill,border=5:000000,padding=0.1/photo.jpg")
            .expect("valid cdn url should parse");
    assert!(req.params.border.is_some());
    assert!(req.params.pad.is_some());

    let mut source = generate_test_image(100, 100);
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    // Padding (10% of 100 = 10px each side) is applied after the border
    // (5px each side), so the final canvas is strictly larger than the
    // resized 100x100 target in both dimensions.
    assert!(output.width > 100);
    assert!(output.height > 100);
}

#[test]
fn test_pipeline_rejects_malformed_cdn_url() {
    // Missing the /cdn-cgi/image/ prefix entirely.
    assert!(parse_cdn_url("/not-a-valid-path/photo.jpg").is_err());
}

#[test]
fn test_pipeline_preset_thumbnail_produces_small_output() {
    let req = parse_cdn_url("/cdn-cgi/image/preset=thumbnail/photo.jpg")
        .expect("preset should resolve to concrete params");

    let mut source = generate_test_image(1200, 900);
    let output = apply_transforms(&mut source, &req.params).expect("transform should succeed");

    // Whatever the thumbnail preset resolves to, it must shrink a large
    // source down (never enlarge it), and produce a valid non-empty buffer.
    assert!(output.width <= 1200);
    assert!(output.height <= 900);
    assert!(!output.data.is_empty());
}

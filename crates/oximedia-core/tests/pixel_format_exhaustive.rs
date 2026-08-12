//! Exhaustive tests for all `PixelFormat` variants.
//!
//! Covers every variant in the enum and verifies:
//! - `plane_count()` — number of distinct memory planes
//! - `bits_per_component()` — significant bits per sample
//! - `chroma_subsampling()` — (horizontal, vertical) reduction factors
//! - Layout flags: `is_planar`, `is_semi_planar`, `is_yuv`, `is_rgb`, `has_alpha`

use oximedia_core::types::PixelFormat;

/// Compact per-variant expectations.
struct Expected {
    format: PixelFormat,
    planes: u32,
    bpc: u32,           // bits_per_component
    chroma: (u32, u32), // chroma_subsampling
    is_planar: bool,
    is_semi_planar: bool,
    is_yuv: bool,
    is_rgb: bool,
    has_alpha: bool,
}

/// Ground-truth table covering every PixelFormat variant.
fn exhaustive_table() -> Vec<Expected> {
    vec![
        // ── 8-bit planar YUV ──────────────────────────────────────────
        Expected {
            format: PixelFormat::Yuv420p,
            planes: 3,
            bpc: 8,
            chroma: (2, 2),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv422p,
            planes: 3,
            bpc: 8,
            chroma: (2, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv444p,
            planes: 3,
            bpc: 8,
            chroma: (1, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── 10-bit planar YUV ─────────────────────────────────────────
        Expected {
            format: PixelFormat::Yuv420p10le,
            planes: 3,
            bpc: 10,
            chroma: (2, 2),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv422p10le,
            planes: 3,
            bpc: 10,
            chroma: (2, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv444p10le,
            planes: 3,
            bpc: 10,
            chroma: (1, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── 12-bit planar YUV ─────────────────────────────────────────
        Expected {
            format: PixelFormat::Yuv420p12le,
            planes: 3,
            bpc: 12,
            chroma: (2, 2),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv422p12le,
            planes: 3,
            bpc: 12,
            chroma: (2, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv444p12le,
            planes: 3,
            bpc: 12,
            chroma: (1, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── 16-bit planar YUV ─────────────────────────────────────────
        Expected {
            format: PixelFormat::Yuv420p16le,
            planes: 3,
            bpc: 16,
            chroma: (2, 2),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv422p16le,
            planes: 3,
            bpc: 16,
            chroma: (2, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Yuv444p16le,
            planes: 3,
            bpc: 16,
            chroma: (1, 1),
            is_planar: true,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── 8-bit semi-planar YUV (NV12 / NV21) ─────────────────────
        Expected {
            format: PixelFormat::Nv12,
            planes: 2,
            bpc: 8,
            chroma: (2, 2),
            is_planar: false,
            is_semi_planar: true,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Nv21,
            planes: 2,
            bpc: 8,
            chroma: (2, 2),
            is_planar: false,
            is_semi_planar: true,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── high-bit-depth semi-planar YUV (P010 / P016) ─────────────
        Expected {
            format: PixelFormat::P010,
            planes: 2,
            bpc: 10,
            chroma: (2, 2),
            is_planar: false,
            is_semi_planar: true,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::P016,
            planes: 2,
            bpc: 16,
            chroma: (2, 2),
            is_planar: false,
            is_semi_planar: true,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── packed YUV 4:2:2 (single interleaved plane) ──────────────
        Expected {
            format: PixelFormat::Yuyv422,
            planes: 1,
            bpc: 8,
            chroma: (2, 1),
            is_planar: false,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Uyvy422,
            planes: 1,
            bpc: 8,
            chroma: (2, 1),
            is_planar: false,
            is_semi_planar: false,
            is_yuv: true,
            is_rgb: false,
            has_alpha: false,
        },
        // ── packed RGB ────────────────────────────────────────────────
        Expected {
            format: PixelFormat::Rgb24,
            planes: 1,
            bpc: 8,
            chroma: (1, 1), // no subsampling
            is_planar: false,
            is_semi_planar: false,
            is_yuv: false,
            is_rgb: true,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Rgba32,
            planes: 1,
            bpc: 8,
            chroma: (1, 1),
            is_planar: false,
            is_semi_planar: false,
            is_yuv: false,
            is_rgb: true,
            has_alpha: true,
        },
        // ── grayscale ─────────────────────────────────────────────────
        Expected {
            format: PixelFormat::Gray8,
            planes: 1,
            bpc: 8,
            chroma: (1, 1),
            is_planar: false,
            is_semi_planar: false,
            is_yuv: false,
            is_rgb: false,
            has_alpha: false,
        },
        Expected {
            format: PixelFormat::Gray16,
            planes: 1,
            bpc: 16,
            chroma: (1, 1),
            is_planar: false,
            is_semi_planar: false,
            is_yuv: false,
            is_rgb: false,
            has_alpha: false,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Verify plane_count for every variant.
#[test]
fn test_all_variants_plane_count() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.plane_count(),
            row.planes,
            "{:?}.plane_count() should be {}",
            row.format,
            row.planes
        );
    }
}

/// Verify bits_per_component (bit depth) for every variant.
#[test]
fn test_all_variants_bits_per_component() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.bits_per_component(),
            row.bpc,
            "{:?}.bits_per_component() should be {}",
            row.format,
            row.bpc
        );
    }
}

/// Verify chroma_subsampling for every variant.
#[test]
fn test_all_variants_chroma_subsampling() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.chroma_subsampling(),
            row.chroma,
            "{:?}.chroma_subsampling() should be {:?}",
            row.format,
            row.chroma
        );
    }
}

/// Verify is_planar for every variant.
#[test]
fn test_all_variants_is_planar() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.is_planar(),
            row.is_planar,
            "{:?}.is_planar() should be {}",
            row.format,
            row.is_planar
        );
    }
}

/// Verify is_semi_planar for every variant.
#[test]
fn test_all_variants_is_semi_planar() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.is_semi_planar(),
            row.is_semi_planar,
            "{:?}.is_semi_planar() should be {}",
            row.format,
            row.is_semi_planar
        );
    }
}

/// Verify is_yuv for every variant.
#[test]
fn test_all_variants_is_yuv() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.is_yuv(),
            row.is_yuv,
            "{:?}.is_yuv() should be {}",
            row.format,
            row.is_yuv
        );
    }
}

/// Verify is_rgb for every variant.
#[test]
fn test_all_variants_is_rgb() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.is_rgb(),
            row.is_rgb,
            "{:?}.is_rgb() should be {}",
            row.format,
            row.is_rgb
        );
    }
}

/// Verify has_alpha for every variant.
#[test]
fn test_all_variants_has_alpha() {
    for row in exhaustive_table() {
        assert_eq!(
            row.format.has_alpha(),
            row.has_alpha,
            "{:?}.has_alpha() should be {}",
            row.format,
            row.has_alpha
        );
    }
}

/// A format cannot be both fully planar and semi-planar at the same time.
#[test]
fn test_planar_and_semi_planar_are_mutually_exclusive() {
    for row in exhaustive_table() {
        if row.is_planar {
            assert!(
                !row.format.is_semi_planar(),
                "{:?} is both planar and semi-planar — impossible",
                row.format
            );
        }
        if row.is_semi_planar {
            assert!(
                !row.format.is_planar(),
                "{:?} is both planar and semi-planar — impossible",
                row.format
            );
        }
    }
}

/// Planar / semi-planar YUV formats have at least 2 planes (one for luma,
/// at least one for chroma). Packed YUV formats (e.g. YUYV/UYVY 4:2:2)
/// interleave luma and chroma into a single plane, so they are excluded
/// from this invariant.
#[test]
fn test_yuv_formats_have_at_least_two_planes() {
    for row in exhaustive_table() {
        if row.is_yuv && (row.is_planar || row.is_semi_planar) {
            assert!(
                row.format.plane_count() >= 2,
                "{:?} is YUV but has only {} plane(s)",
                row.format,
                row.format.plane_count()
            );
        }
    }
}

/// RGB formats have exactly 1 plane.
#[test]
fn test_rgb_formats_have_one_plane() {
    for row in exhaustive_table() {
        if row.is_rgb {
            assert_eq!(
                row.format.plane_count(),
                1,
                "{:?} is RGB but has {} plane(s)",
                row.format,
                row.format.plane_count()
            );
        }
    }
}

/// Only Rgba32 has an alpha channel.
#[test]
fn test_only_rgba32_has_alpha() {
    for row in exhaustive_table() {
        if row.has_alpha {
            assert_eq!(
                row.format,
                PixelFormat::Rgba32,
                "unexpected format with alpha: {:?}",
                row.format
            );
        }
    }
}

/// frame_buffer_size grows monotonically with both width and height.
#[test]
fn test_frame_buffer_size_monotone_all_variants() {
    // Use a small base size to keep memory usage trivial in this test.
    let w_small: u32 = 16;
    let h_small: u32 = 16;
    let w_large: u32 = 32;
    let h_large: u32 = 32;

    for row in exhaustive_table() {
        let size_small = row.format.frame_buffer_size(w_small, h_small);
        let size_large = row.format.frame_buffer_size(w_large, h_large);
        assert!(
            size_large > size_small,
            "{:?}: larger frame should need more bytes ({} > {})",
            row.format,
            size_large,
            size_small
        );
        // Must be non-zero even for the small frame
        assert!(
            size_small > 0,
            "{:?}: frame_buffer_size must be non-zero",
            row.format
        );
    }
}

/// stride_for_width(w, plane) must return Some for every valid plane,
/// None for an out-of-range plane index.
#[test]
fn test_stride_for_width_valid_planes_all_variants() {
    let w: u32 = 64;
    for row in exhaustive_table() {
        let num_planes = row.format.plane_count();
        for p in 0..num_planes {
            assert!(
                row.format.stride_for_width(w, p).is_some(),
                "{:?}: stride_for_width(plane={}) returned None for a valid plane",
                row.format,
                p
            );
        }
        // Plane at index `num_planes` must be out-of-range
        assert!(
            row.format.stride_for_width(w, num_planes).is_none(),
            "{:?}: stride_for_width(plane={}) should return None (out of range)",
            row.format,
            num_planes
        );
    }
}

/// Display → FromStr roundtrip for every variant.
#[test]
fn test_display_fromstr_roundtrip_all_variants() {
    for row in exhaustive_table() {
        let s = format!("{}", row.format);
        let parsed: PixelFormat = s
            .parse()
            .unwrap_or_else(|_| panic!("{:?} roundtrip failed via \"{}\"", row.format, s));
        assert_eq!(
            parsed, row.format,
            "{:?} roundtrip mismatch: expected {:?} but got {:?}",
            row.format, row.format, parsed
        );
    }
}

/// 4:2:0 formats: chroma plane is half the luma width in both dimensions.
#[test]
fn test_420_chroma_half_both_dimensions() {
    let yuv420_variants = [
        PixelFormat::Yuv420p,
        PixelFormat::Yuv420p10le,
        PixelFormat::Yuv420p12le,
        PixelFormat::Yuv420p16le,
    ];
    for fmt in yuv420_variants {
        let (h_sub, v_sub) = fmt.chroma_subsampling();
        assert_eq!(h_sub, 2, "{fmt:?} should have horizontal subsampling of 2");
        assert_eq!(v_sub, 2, "{fmt:?} should have vertical subsampling of 2");
        // Luma stride at w=64: plane 0
        let luma_stride = fmt.stride_for_width(64, 0).expect("luma stride");
        let chroma_stride = fmt.stride_for_width(64, 1).expect("chroma stride");
        // chroma stride = luma stride / 2 (in sample units; both use same bpc)
        assert_eq!(
            chroma_stride,
            luma_stride / 2,
            "{fmt:?}: chroma stride {chroma_stride} should be half of luma stride {luma_stride}"
        );
    }
}

/// 4:2:2 formats: chroma plane is half the luma width but full height.
#[test]
fn test_422_chroma_half_horizontal_only() {
    let yuv422_variants = [
        PixelFormat::Yuv422p,
        PixelFormat::Yuv422p10le,
        PixelFormat::Yuv422p12le,
        PixelFormat::Yuv422p16le,
    ];
    for fmt in yuv422_variants {
        let (h_sub, v_sub) = fmt.chroma_subsampling();
        assert_eq!(h_sub, 2, "{fmt:?} should have horizontal subsampling of 2");
        assert_eq!(v_sub, 1, "{fmt:?} should have vertical subsampling of 1");
        let luma_stride = fmt.stride_for_width(64, 0).expect("luma stride");
        let chroma_stride = fmt.stride_for_width(64, 1).expect("chroma stride");
        assert_eq!(
            chroma_stride,
            luma_stride / 2,
            "{fmt:?}: chroma stride {chroma_stride} should be half of luma stride {luma_stride}"
        );
    }
}

/// 4:4:4 formats: no chroma subsampling; all planes have the same stride.
#[test]
fn test_444_no_chroma_subsampling() {
    let yuv444_variants = [
        PixelFormat::Yuv444p,
        PixelFormat::Yuv444p10le,
        PixelFormat::Yuv444p12le,
        PixelFormat::Yuv444p16le,
    ];
    for fmt in yuv444_variants {
        let (h_sub, v_sub) = fmt.chroma_subsampling();
        assert_eq!(h_sub, 1, "{fmt:?} should have no horizontal subsampling");
        assert_eq!(v_sub, 1, "{fmt:?} should have no vertical subsampling");
        let stride_y = fmt.stride_for_width(64, 0).expect("Y stride");
        let stride_cb = fmt.stride_for_width(64, 1).expect("Cb stride");
        let stride_cr = fmt.stride_for_width(64, 2).expect("Cr stride");
        assert_eq!(
            stride_y, stride_cb,
            "{fmt:?}: Y and Cb strides must match for 4:4:4"
        );
        assert_eq!(
            stride_y, stride_cr,
            "{fmt:?}: Y and Cr strides must match for 4:4:4"
        );
    }
}

/// Semi-planar formats (NV12 / NV21 / P010 / P016) always have exactly 2 planes.
#[test]
fn test_semi_planar_always_two_planes() {
    let semi_planar = [
        PixelFormat::Nv12,
        PixelFormat::Nv21,
        PixelFormat::P010,
        PixelFormat::P016,
    ];
    for fmt in semi_planar {
        assert_eq!(fmt.plane_count(), 2, "{fmt:?} must have exactly 2 planes");
        assert!(fmt.is_semi_planar(), "{fmt:?} must report is_semi_planar()");
        assert!(!fmt.is_planar(), "{fmt:?} must not report is_planar()");
    }
}

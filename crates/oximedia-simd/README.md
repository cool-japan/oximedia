# oximedia-simd

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Hand-written assembly SIMD kernels and high-performance pixel operations for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

This crate provides highly optimized SIMD implementations of critical performance paths in the OxiMedia video codec, including:

- **DCT Transforms**: 4x4, 8x8, 16x16, 32x32 forward and inverse DCT
- **Interpolation**: Bilinear, bicubic, and 8-tap filters for motion compensation
- **Motion Estimation**: Sum of Absolute Differences (SAD) for 16x16, 32x32, 64x64 blocks
- **Pixel Operations**: Blending, color conversion, YUV operations, histogram
- **Audio Operations**: SIMD-accelerated audio processing primitives
- **Math Operations**: Vector math, dot products, matrix operations

## Performance

These optimized implementations provide 2-5x speedup over compiler-generated code:

- AVX2 (x86-64): ~3-4x faster than auto-vectorization
- AVX-512 (x86-64): ~4-5x faster for SAD operations
- NEON (AArch64): ~2-3x faster than auto-vectorization

## Architecture Support

### x86-64
- **AVX2**: Full support for all operations
- **AVX-512**: Optimized SAD implementations using 512-bit registers

### AArch64
- **NEON**: Full support for DCT, interpolation, and SAD

### Fallback
- Scalar implementations for all operations when SIMD not available

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-simd = "0.2.0"
```

```rust
use oximedia_simd::{detect_cpu_features, forward_dct, DctSize};

let features = detect_cpu_features();
println!("AVX2 available: {}", features.avx2);
println!("NEON available: {}", features.neon);

// API automatically selects best implementation
let input = vec![0i16; 64];
let mut output = vec![0i16; 64];
forward_dct(&input, &mut output, DctSize::Dct8x8).unwrap();
```

## API Overview

- `detect_cpu_features()` — Runtime CPU feature detection
- `CpuFeatures` — Detected feature flags: avx2, avx512f, avx512bw, neon
- `forward_dct()` / `inverse_dct()` — DCT transforms with automatic SIMD dispatch
- `interpolate()` — Motion compensation interpolation
- `sad()` — Sum of Absolute Differences for motion estimation
- `DctSize` — Dct4x4, Dct8x8, Dct16x16, Dct32x32
- `InterpolationFilter` — Bilinear, Bicubic, EightTap
- `BlockSize` — Block16x16, Block32x32, Block64x64
- `SimdError` / `Result` — Error and result types
- `validate_avx2_alignment()` / `validate_avx512_alignment()` / `validate_neon_alignment()` — Buffer alignment validation
- Modules: `accumulator`, `alpha_premul`, `audio_ops`, `bitwise_ops`, `blend`, `blend_simd`, `color_convert_simd`, `color_space`, `convolution`, `filter`, `fixed_point`, `gather_scatter`, `histogram`, `interleave`, `lookup_table`, `math_ops`, `matrix`, `min_max`, `pack_unpack`, `pixel_ops`, `prefix_sum`, `reduce`, `saturate`, `threshold`, `transpose`, `vector_math`, `yuv_ops`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

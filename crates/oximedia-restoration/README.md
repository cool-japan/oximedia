# oximedia-restoration

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Image restoration algorithms for OxiMedia: blind deconvolution and content-aware inpainting.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — 6 tests

> Note: this crate is distinct from `oximedia-restore`, which covers audio/video
> *artifact* restoration (click removal, hum removal, deband, dropout fix, etc.).
> `oximedia-restoration` focuses specifically on two classical single-image
> inverse problems: recovering a sharp image from an unknown blur, and filling
> in damaged/missing regions from surrounding texture.

## Features

- **Blind Deconvolution** — Richardson-Lucy iterative algorithm that jointly
  estimates the latent sharp image *and* the unknown point spread function
  (PSF) from a single blurry observation, using frequency-domain convolutions
  via `OxiFFT` (`oxifft::rfft2d` / `irfft2d`) for O(N log N) performance. The
  PSF estimate is clamped non-negative and renormalized to sum to 1 after
  every update so it remains a proper blur kernel.
- **Content-Aware Inpainting** — PatchMatch-based texture synthesis (Barnes et
  al., SIGGRAPH 2009) that fills masked (damaged/missing) regions by finding
  approximate nearest-neighbour source patches via randomized propagation +
  exponentially-decreasing-radius random search, then reconstructs each pixel
  as a distance-weighted, Gaussian-blended average of all covering patches.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-restoration = "0.2.0"
```

```rust
use oximedia_restoration::blind_deconv::{blind_deconvolve, BlindDeconvConfig};
use oximedia_restoration::inpaint::{inpaint_patchmatch, InpaintConfig};

// --- Blind deconvolution (single-channel f32 image, values in [0, 1]) ---
let (w, h) = (8u32, 8u32);
let blurry = vec![0.5f32; (w * h) as usize];
let cfg = BlindDeconvConfig { iterations: 15, psf_size: 5, regularization: 1e-6 };
let (sharp, psf) = blind_deconvolve(&blurry, w, h, &cfg);
assert_eq!(sharp.len(), (w * h) as usize);

// --- Inpainting (packed RGB u8 image) ---
let image = vec![128u8; (w * h * 3) as usize];
let mask = vec![false; (w * h) as usize]; // true == pixel needs inpainting
let restored = inpaint_patchmatch(&image, &mask, w, h, &InpaintConfig::default());
assert_eq!(restored.len(), image.len());
```

## API Overview

**Core types:**
- `BlindDeconvConfig` — `iterations` (default 15), `psf_size` (default 5,
  kernel side length `2*psf_size+1`), `regularization` (default `1e-6`)
- `InpaintConfig` — `patch_size` (default 3, window side `2*patch_size+1`),
  `iterations` (default 5 PatchMatch propagation+search passes),
  `blend_radius` (default 1, seam-smoothing overlap radius)

**Modules:**
- `blind_deconv` — `blind_deconvolve(blurry, w, h, cfg) -> (sharp, psf)`;
  Richardson-Lucy alternating image/PSF update via FFT-domain convolution
- `inpaint` — `inpaint_patchmatch(image, mask, w, h, cfg) -> restored`;
  ANN-field PatchMatch with propagation, random search, and weighted blend
  reconstruction

## Status notes

Both algorithms are real, fully implemented numerical pipelines validated by
round-trip / self-consistency unit tests — neither is a stub. They have not
yet been benchmarked against an external reference implementation or a
standard deblurring/inpainting corpus; see `docs/codec_status.md` at the
workspace root for the project-wide convention used to label that kind of
gap on decoders (this crate is not a decoder, so it is not formally listed
there, but the same "real reconstruction, no third-party conformance proof
yet" caveat applies).

## Dependencies

- `oxifft` — Pure-Rust FFT for frequency-domain convolution (blind
  deconvolution forward/inverse transforms)
- `rayon` — Data-parallelism
- `rand` — PatchMatch random search / initialization
- `oximedia-core` — Shared workspace types
- `thiserror` — Error types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

# oximedia-image-transform

![Version: 0.2.1](https://img.shields.io/badge/version-0.2.1-blue)
![Tests: 501](https://img.shields.io/badge/tests-501-brightgreen)
![Updated: 2026-08-12](https://img.shields.io/badge/updated-2026--08--12-blue)

Cloudflare Images-compatible URL image transformation for the [OxiMedia](https://github.com/cool-japan/oximedia) Sovereign Media Framework.

## Overview

`oximedia-image-transform` provides a complete, pure-Rust implementation of Cloudflare Images' URL-based image transformation API. Parse transformation URLs, negotiate output formats via HTTP Accept headers, build processing pipelines, and validate requests against security policies -- all without any C/Fortran dependencies.

## Features

- **URL Parsing** -- parse `/cdn-cgi/image/` paths, query strings, and comma-separated transform strings
- **Content Negotiation** -- Accept-header-based format selection (AVIF > WebP > JPEG/PNG)
- **Processing Pipeline** -- ordered step-based transformation (decode, trim, resize, rotate, color, sharpen/blur, border/pad, encode)
- **Security Validation** -- SSRF prevention, path traversal detection, dimension limits
- **Cache Key Generation** -- deterministic FNV-1a based cache keys

## Supported Parameters

See [`TransformParams`](src/transform.rs) module docs for the full field-by-field reference (types and valid ranges). Defaults shown below are what applies when a parameter is omitted from the URL/query string entirely.

| Parameter | Short | Default | Description |
|-----------|-------|---------|-------------|
| `width` | `w` | (none) | Target width in pixels, `1..=12000` |
| `height` | `h` | (none) | Target height in pixels, `1..=12000` |
| `quality` | `q` | `85` | Output quality (1-100) |
| `format` | `f` | `auto` | Output format: `auto`, `avif`, `webp`, `jpeg`, `png`, `gif`, `baseline`, `json` |
| `fit` | -- | `scale-down` | Resize mode: `scale-down`, `contain`, `cover`, `crop`, `pad`, `fill` |
| `gravity` | `g` | `center` | Crop anchor: `auto`, `center`, `top`, `bottom`, `left`, `right`, `face`, `0.5x0.5` |
| `sharpen` | -- | `0.0` (off) | Sharpen amount (0.0-10.0) |
| `blur` | -- | `0.0` (off) | Gaussian blur radius (0.0-250.0) |
| `brightness` | -- | `0.0` (no change) | Brightness adjustment (-1.0 to 1.0) |
| `contrast` | -- | `0.0` (no change) | Contrast adjustment (-1.0 to 1.0) |
| `gamma` | -- | `1.0` (no change) | Gamma correction (0.0-10.0, exclusive of 0) |
| `rotate` | -- | `0` | Rotation: `0`, `90`, `180`, `270`, `auto` |
| `dpr` | -- | `1.0` | Device pixel ratio (1.0-4.0) |
| `trim` | -- | (none) | Edge trimming in pixels |
| `background` | `bg` | transparent | Background color (CSS hex) |
| `border` | -- | (none) | Border: `width:color` or `t,r,b,l:color` |
| `padding` | `pad` | (none) | Fractional padding (0.0-1.0) |
| `metadata` | -- | `none` | Metadata handling: `keep`, `copyright`, `none` |
| `anim` | -- | `true` | Animate GIFs: `true` / `false` |
| `compression` | -- | (none) | Compression strategy: `fast`, `default`, `slow` |
| `onerror` | -- | (none) | Fallback URL on error |
| `preset` | -- | (none) | Named preset: `thumbnail`, `preview`, `hd_ready` (sets width/height/quality) |

## URL Formats

```text
# CDN path format (Cloudflare-compatible)
/cdn-cgi/image/width=800,height=600,format=auto/path/to/image.jpg

# Short aliases
/cdn-cgi/image/w=800,h=600,f=webp,fit=cover/photo.jpg

# Query string format
?width=800&height=600&quality=85&format=auto
```

## Usage

```rust
use oximedia_image_transform::parser::parse_cdn_url;
use oximedia_image_transform::transform::{OutputFormat, FitMode};
use oximedia_image_transform::negotiation::negotiate_format;
use oximedia_image_transform::security::{validate_request, SecurityConfig};

// Parse a Cloudflare-style URL
let req = parse_cdn_url("/cdn-cgi/image/w=800,f=auto,fit=cover/photo.jpg").unwrap();
assert_eq!(req.params.width, Some(800));
assert_eq!(req.params.fit, FitMode::Cover);

// Negotiate output format from Accept header
let format = negotiate_format(
    "image/avif,image/webp;q=0.9,image/jpeg;q=0.8",
    req.params.format,
);
assert_eq!(format, OutputFormat::Avif);

// Validate against security policy
let config = SecurityConfig::default();
assert!(validate_request(&req.source_path, &req.params, &config).is_ok());
```

## Security

Built-in protections against:

- **SSRF** -- detects private/reserved IPs (RFC 1918, RFC 4193, CGNAT RFC 6598, link-local, documentation ranges RFC 5737)
- **Path Traversal** -- blocks `../`, encoded variants (`%2e%2e`, `%2f`, `%5c`), null bytes, backslashes, tilde home references
- **Resource Exhaustion** -- configurable dimension limits (default 12000x12000), file size limits (100 MB input, 50 MB output)

## Processing Pipeline

The processor module builds an ordered pipeline from transform parameters:

1. Decode
2. Trim (edge removal)
3. Resize (bilinear interpolation)
4. Rotate (90/180/270/auto)
5. Color adjustments (brightness, contrast, gamma)
6. Sharpen (unsharp mask) / Blur (separable Gaussian)
7. Border / Padding
8. Background (alpha flattening)
9. Encode

## Architecture

| Module | Description |
|--------|-------------|
| `transform` / `transform_types` | Strongly-typed parameter structs and enums (`TransformParams`, `Border`, `Padding`, `Trim`, `Rotation`, `Compression`, `OutputOptions`) |
| `parser` | URL / query-string parsing, presets, and cache key generation |
| `negotiation` | Accept header parsing, format selection, client hints, cache-control generation |
| `processor` | Image processing pipeline and pixel operations (resize, geometry, color, filters, streaming) |
| `security` | SSRF prevention, path-traversal detection, signed-URL (HMAC-SHA256) verification |
| `quality` | SSIM-guided quality auto-tuning from image complexity analysis |
| `watermark` | Text/image watermark overlay (9 positions, opacity, scaling) |
| `face_detect` | Skin-tone + saliency-based face detection for smart gravity cropping |
| `blur_region` | Selective rectangular region blurring for privacy/NSFW redaction |
| `responsive` | `srcset` / `<picture>` HTML generation across breakpoints and formats |
| `animation` | GIF/WebP animation frame resize, frame-rate reduction, optimisation |
| `origin_fetch` | Remote source-image fetching with SSRF-checked allowlisting and in-memory caching |
| `metrics` | Thread-safe transform request statistics (hit rate, latency histogram, format distribution) |
| `batch_transform` | Multiple transform variants processed in parallel (`rayon`) from a single request |
| `image_analysis` | Dominant-colour extraction (median-cut) and BlurHash encoding |
| `response_cache` | Content-addressable in-memory cache for fully-encoded transform output |
| `compose` / `inverse` | 2-D affine transform matrix composition and analytical inversion |

## Cloudflare Images Compatibility Matrix

Cross-reference against the [Cloudflare Images transformation reference](https://developers.cloudflare.com/images/transform-images/transform-via-url/). "Extension" marks functionality with no direct Cloudflare Images equivalent.

| Feature | Cloudflare Images | oximedia-image-transform | Module |
|---------|:---:|:---:|--------|
| `/cdn-cgi/image/...` URL transform syntax | Yes | Yes | `parser` |
| Query-string transform syntax | No (path-only) | Yes (extension) | `parser` |
| `width` / `height` resize | Yes | Yes | `transform`, `processor` |
| `fit` modes (scale-down/contain/cover/crop/pad/fill) | Yes | Yes | `transform`, `processor` |
| `gravity` (center/side/`x,y` focal point) | Yes | Yes | `transform`, `processor` |
| `gravity=auto` (saliency-based smart crop) | Yes | Yes | `face_detect` |
| `gravity=face` | Yes | Yes (skin-tone + saliency heuristic, not ML face detection) | `face_detect` |
| `quality` | Yes | Yes | `transform` |
| SSIM-guided automatic quality tuning | No | Yes (extension) | `quality` |
| `format=auto` content negotiation (AVIF/WebP/JPEG) | Yes | Yes | `negotiation` |
| HTTP Client Hints (`DPR`, `Width`, `Viewport-Width`) | Yes | Yes | `negotiation` |
| `background` colour | Yes | Yes | `transform`, `processor` |
| `blur` | Yes | Yes | `transform`, `processor` |
| `sharpen` | Yes | Yes | `transform`, `processor` |
| `brightness` / `contrast` / `gamma` | Partial (Cloudflare exposes `brightness`/`contrast`/`gamma` on Enterprise plans) | Yes | `transform`, `processor` |
| `rotate` (incl. `auto` via EXIF) | Yes | Yes | `transform`, `processor` |
| `trim` | Yes | Yes | `transform`, `processor` |
| `border` | Yes | Yes | `transform`, `processor` |
| `dpr` | Yes | Yes | `transform` |
| `metadata` (keep/copyright/none) | Yes | Yes | `transform` |
| `anim` (animated GIF passthrough toggle) | Yes | Yes | `transform` |
| Animation frame resize / frame-rate reduction | No (Cloudflare resizes the whole animated asset opaquely) | Yes (extension) | `animation` |
| `compression` hint (`fast`/`best`) | No | Yes (extension) | `transform` |
| `onerror` fallback | Yes | Yes | `transform` |
| Named variants (dashboard-configured presets) | Yes | Yes, via `preset=` URL parameter (`thumbnail`, `preview`, `hd_ready`) | `parser` |
| Signed URLs (token-based access control) | Yes | Yes (HMAC-SHA256) | `security` |
| SSRF / private-IP source validation | Yes (managed internally) | Yes | `security` |
| Draw / watermark overlay (text or image) | Yes (Enterprise "draw" array) | Yes | `watermark` |
| Selective region blur (privacy/NSFW redaction) | No | Yes (extension) | `blur_region` |
| `srcset` / `<picture>` responsive HTML generation | No (left to the caller) | Yes (extension) | `responsive` |
| Dominant colour extraction / BlurHash placeholders | No | Yes (extension) | `image_analysis` |
| Batch/multi-variant transform in one request | No (one URL = one variant) | Yes (extension, parallelised) | `batch_transform` |
| Request metrics (latency, hit rate, format mix) | Yes (dashboard analytics, hosted) | Yes (local, embeddable) | `metrics` |
| Origin image fetch with caching | Yes (managed internally) | Yes | `origin_fetch` |
| Response cache keyed by transform params | Yes (managed internally, CDN-level) | Yes (in-process, content-addressable) | `response_cache` |
| `format=json` (metadata-only response) | Yes | Recognised as an [`OutputFormat`](src/transform.rs) variant; JSON body construction is left to the calling service | `transform` |

## License

Apache-2.0

Copyright (c) COOLJAPAN OU (Team Kitasan)

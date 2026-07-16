# oximedia-cv

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Computer vision module for the OxiMedia multimedia framework.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-cv` provides computer vision algorithms and image processing capabilities:

- **Image Processing**: Resize, color conversion, filtering, edge detection
- **Detection**: Face detection, motion detection, corner detection, YOLO object detection
- **Transforms**: Affine and perspective transformations
- **Enhancement**: Super-resolution and denoising (with ONNX)
- **Tracking**: Optical flow, feature tracking, object tracking (KCF, CSRT, MOSSE, MedianFlow)
- **Stabilization**: Video stabilization with motion smoothing
- **Scene Detection**: Video scene changes and shot boundary detection
- **Quality Metrics**: PSNR, SSIM, VMAF, temporal quality assessment
- **Frame Interpolation**: Optical flow-based frame rate conversion
- **Chroma Key**: Green/blue screen with spill suppression and compositing
- **Content-aware Scaling**: Seam carving and saliency-guided resizing
- **Interlace Detection**: Interlacing and telecine detection with IVTC recommendations
- **Motion Blur**: Synthesis and removal via deconvolution
- **Fingerprinting**: Perceptual content fingerprinting for identification
- **Depth Estimation**: Monocular depth estimation
- **Pose Estimation**: Human pose keypoint detection
- **Segmentation**: Image/video segmentation
- **Superpixel**: SLIC and compact superpixel algorithms
- **Morphology**: Erosion, dilation, open/close operations
- **Lane Detection**: Road lane detection

## Features

### Image Processing

| Operation | Description |
|-----------|-------------|
| Resize | Nearest, Bilinear, Bicubic, Lanczos |
| Color Conversion | RGB, YUV, HSV, Lab, Grayscale |
| Histogram | Equalization, CLAHE |
| Blur | Gaussian, Bilateral |
| Edge Detection | Sobel, Canny, Laplacian |

### Detection

| Algorithm | Description |
|-----------|-------------|
| Corner Detection | Harris, Shi-Tomasi, FAST |
| Face Detection | Haar cascades, CNN (with ONNX) |
| Motion Detection | Frame differencing, background subtraction |
| Object Detection | YOLO (with ONNX) |

### Tracking

| Algorithm | Description |
|-----------|-------------|
| Object Trackers | KCF, CSRT, MOSSE, MedianFlow |
| Optical Flow | Lucas-Kanade, Farneback |

### Enhancement

| Feature | Description |
|---------|-------------|
| Super Resolution | ESRGAN-style upscaling (with ONNX) |
| Denoising | CNN-based denoising (with ONNX) |
| CPU Upscaling | Software-based upscaling |

## Usage

```rust
use oximedia_cv::image::{ResizeMethod, ColorSpace};
use oximedia_cv::detect::BoundingBox;

// Create a bounding box
let bbox = BoundingBox::new(10.0, 20.0, 100.0, 150.0);
assert!(bbox.area() > 0.0);
```

## Module Structure

```
src/
├── lib.rs                  # Crate root with re-exports
├── error.rs                # CvError and CvResult
├── bounding_box.rs         # Bounding box types
├── image/                  # Image processing operations
├── detect/                 # Detection algorithms (face, motion, YOLO)
├── transform/              # Geometric transformations
├── enhance/                # Enhancement (super-resolution, denoising, CPU upscale)
├── tracking/               # Object tracking (KCF, CSRT, MOSSE, MedianFlow)
├── stabilize/              # Video stabilization
├── scene/                  # Scene and shot boundary detection
├── quality/                # Quality metrics (PSNR, SSIM, temporal)
├── interpolate/            # Frame interpolation
├── chroma_key/             # Chroma keying and compositing
├── scale/                  # Content-aware scaling
├── interlace/              # Interlace/telecine detection
├── motion_blur/            # Motion blur synthesis and removal
├── fingerprint/            # Perceptual fingerprinting
├── depth_estimation.rs     # Depth estimation
├── pose_estimation.rs      # Pose estimation
├── segmentation.rs         # Image segmentation
├── superpixel.rs           # Superpixel algorithms
├── morphology.rs           # Morphological operations
├── contour.rs              # Contour detection
├── feature_extract.rs      # Feature extraction
├── feature_match.rs        # Feature matching
├── keypoint.rs             # Keypoint management
├── motion_vector.rs        # Motion vector computation
├── optical_flow_field.rs   # Optical flow field
├── hough_transform.rs      # Hough transform
├── histogram_backproject.rs # Histogram back-projection
├── color_cluster.rs        # Color clustering
├── obj_tracking.rs         # Object tracking orchestration
├── texture_analysis.rs     # Texture analysis
└── ml/                     # Machine learning (ONNX, feature-gated)
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `onnx` | ONNX Runtime integration for ML model inference |
| `cuda` | CUDA GPU acceleration (requires `onnx`) |
| `rocm` | ROCm GPU acceleration (requires `onnx`) |
| `tensorrt` | TensorRT acceleration (requires `onnx`) |

```toml
[dependencies]
oximedia-cv = { version = "0.2.0", features = ["onnx"] }
```

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

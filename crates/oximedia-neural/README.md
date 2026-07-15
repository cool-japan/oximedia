# oximedia-neural

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Lightweight neural network inference for media processing — pure Rust tensor ops, conv2d, and pre-defined media models

[![Crates.io](https://img.shields.io/crates/v/oximedia-neural.svg)](https://crates.io/crates/oximedia-neural)
[![Documentation](https://docs.rs/oximedia-neural/badge.svg)](https://docs.rs/oximedia-neural)
[![License](https://img.shields.io/crates/l/oximedia-neural.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

## Features

- Pure Rust n-dimensional f32 tensor with row-major (C-contiguous) storage; batch dimension `[N,C,H,W]` support
- Matrix multiplication, element-wise add/mul, NumPy-style shape broadcasting, dimension reduction
- In-place tensor operations (`relu_inplace`, `add_inplace`) to minimize allocations during inference
- Neural network layers: `LinearLayer`, `Conv2dLayer`, `DepthwiseConv2d`, `BatchNorm1d`, `MaxPool2d`, `AvgPool2d`, `GlobalAvgPool`, `ConvTranspose2d`
- Activation functions: ReLU, Leaky ReLU, GELU, Sigmoid, Swish, Tanh, Softmax
- Pre-built media models: scene classifier, thumbnail ranker, super-resolution upscaler, HOG feature extractor
- Zero C/Fortran dependencies

## Quick Start

```toml
[dependencies]
oximedia-neural = "0.2.0"
```

```rust
use oximedia_neural::tensor::Tensor;
use oximedia_neural::layers::LinearLayer;
use oximedia_neural::activations::{relu, ActivationFn, apply_activation};

// Create a fully-connected layer and run a forward pass.
let layer = LinearLayer::new(4, 2).unwrap();
let input = Tensor::ones(vec![4]).unwrap();
let output = layer.forward(&input).unwrap();
assert_eq!(output.shape(), &[2]);

// Apply ReLU activation element-wise.
let activated = apply_activation(&output, &ActivationFn::Relu);
```

### Scene Classification

```rust
use oximedia_neural::media_models::{SceneClassifier, FeatureExtractor};

// Extract HOG features from a single-channel image.
let extractor = FeatureExtractor::new();
let frame = vec![0.5_f32; 64 * 64];
let features = extractor.extract(&frame, 64, 64).unwrap();
assert_eq!(features.len(), 128);

// Classify the scene (populate weights for real use).
let classifier = SceneClassifier::new().unwrap();
let (class_idx, confidence) = classifier.classify(&features).unwrap();
```

### Super-Resolution Upscaling

```rust
use oximedia_neural::media_models::SrUpscaler;

let upscaler = SrUpscaler::new().unwrap();
let frame = vec![0.5_f32; 8 * 8];
let upscaled = upscaler.upscale_2x(&frame, 8, 8).unwrap();
assert_eq!(upscaled.len(), 16 * 16); // 2x in each dimension
```

## Modules

### `tensor`

Core n-dimensional `Tensor` type with row-major storage. Supports construction (`new`, `zeros`, `ones`, `from_data`, `from_chw`), indexing (`get`, `set`), shape manipulation (`reshape`, `slice`, `transpose_2d`), batch dimension `[N,C,H,W]` iteration, NumPy-style shape broadcasting for element-wise ops, in-place mutations (`relu_inplace`, `add_inplace`), and free functions for `matmul`, `add` (with bias broadcasting), `mul`, and `sum_along`.

### `activations`

Scalar activation functions (`relu`, `leaky_relu`, `gelu`, `sigmoid`, `swish`, `tanh_act`) and numerically stable `softmax`. The `ActivationFn` enum wraps these for use with `apply_activation`, which maps an activation element-wise over a tensor.

### `layers`

Inference-only neural network layers:

- **`LinearLayer`** -- fully-connected `W*x + b` with configurable input/output features
- **`Conv2dLayer`** -- 2D convolution with stride, padding, and per-channel bias; input shape `[C, H, W]`
- **`DepthwiseConv2d`** -- depthwise separable convolution (one kernel per channel)
- **`BatchNorm1d`** -- 1D batch normalization using running statistics
- **`MaxPool2d`** -- 2D max pooling with configurable kernel and stride
- **`AvgPool2d`** -- 2D average pooling with configurable kernel and stride
- **`GlobalAvgPool`** -- reduces spatial dimensions to 1x1 by averaging
- **`ConvTranspose2d`** -- transposed convolution (dilate-then-convolve) for decoder and upsampling networks

### `media_models`

Pre-built inference pipelines for common media tasks:

- **`SceneClassifier`** -- 2-layer MLP classifying 128-dim features into 10 scene categories (`Static`, `Action`, `Talking`, `Nature`, `Sports`, `Concert`, `News`, `Animation`, `Documentary`, `Unknown`)
- **`ThumbnailRanker`** -- single linear layer + sigmoid producing an aesthetic quality score in [0, 1]
- **`SrUpscaler`** -- bilinear 2x upsampling followed by a 3-layer conv sharpening pipeline for single-channel luminance
- **`FeatureExtractor`** -- HOG-like gradient histogram descriptor producing a 128-dimensional feature vector from a 4x4 grid of 8-bin histograms

### `error`

`NeuralError` enum covering shape mismatches, index out of bounds, empty inputs, and invalid shapes.

## Architecture

All layers are stateless inference-only -- they hold weight tensors but perform no training. Models are zero-initialized at construction; load pre-trained weights by assigning directly to the public `weight` and `bias` fields. The crate has no external dependencies beyond `thiserror`.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)

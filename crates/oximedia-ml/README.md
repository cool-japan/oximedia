# oximedia-ml

Sovereign ML pipelines for OxiMedia — Pure-Rust ONNX inference (OxiONNX)

**Status: [Stable]** | Version: 0.2.0 | Updated: 2026-07-14

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Overview

`oximedia-ml` wraps the [Pure-Rust OxiONNX](https://crates.io/crates/oxionnx) runtime in a set of typed pipelines tailored to multimedia workloads: scene classification, shot boundary detection, aesthetic scoring, object detection, face embedding, and more as the feature-gated zoo grows.

The default build pulls in **zero** ONNX symbols; enable the `onnx` feature to opt in to inference.

## Installation

```toml
[dependencies]
oximedia-ml = { version = "0.2.0", features = ["scene-classifier"] }
```

To enable all pipelines:

```toml
oximedia-ml = { version = "0.2.0", features = ["all-pipelines"] }
```

## Quick Start

Load a Places365-compatible scene classifier, run it on a 224x224 RGB frame, and print the top-5 predictions:

```rust
# #[cfg(all(feature = "onnx", feature = "scene-classifier"))]
# fn demo() -> oximedia_ml::MlResult<()> {
use oximedia_ml::pipelines::{SceneClassifier, SceneImage};
use oximedia_ml::{DeviceType, TypedPipeline};

let classifier = SceneClassifier::load("places365.onnx", DeviceType::auto())?;
let image = SceneImage::new(vec![0u8; 224 * 224 * 3], 224, 224)?;
for pred in classifier.run(image)? {
    println!("class {} @ {:.3}", pred.class_index, pred.score);
}
# Ok(())
# }
```

## Feature Matrix

Backend features control which ONNX execution providers are compiled in; pipeline features enable individual domain adapters. Everything except `cuda` is WASM-compatible.

| Feature              | Purpose                                                           | Notes                         |
|----------------------|-------------------------------------------------------------------|-------------------------------|
| `onnx`               | Enables the real `OnnxModel` backed by OxiONNX.                  | Required for any inference.   |
| `cuda`               | Additionally compile `oxionnx-cuda` for NVIDIA GPU execution.     | Native only (no WASM).        |
| `webgpu`             | Additionally compile `oxionnx-gpu` (wgpu backend).                | Works on native + browsers.   |
| `directml`           | Additionally compile `oxionnx-directml`.                          | Stub outside Windows.         |
| `serde`              | Derives `Serialize` on pipeline info/value types.                 | Opt-in; no runtime cost.      |
| `scene-classifier`   | Builds the `pipelines::SceneClassifier` pipeline.                 | Places365-compatible.         |
| `shot-boundary`      | Builds the `pipelines::ShotBoundaryDetector` pipeline.            | TransNet V2-compatible.       |
| `aesthetic-score`    | Builds the `pipelines::AestheticScorer` pipeline.                 | NIMA-compatible.              |
| `object-detector`    | Builds the `pipelines::ObjectDetector` pipeline.                  | YOLOv8-compatible.            |
| `face-embedder`      | Builds the `pipelines::FaceEmbedder` pipeline.                    | ArcFace-compatible.           |
| `all-pipelines`      | Shortcut enabling every pipeline above.                           | Implies `onnx`.               |

## Pipeline Ecosystem

All pipelines implement the `TypedPipeline` trait. Each is gated behind its own feature so apps only compile what they use:

| Pipeline                              | Feature             | Input             | Output                        | Reference model   |
|---------------------------------------|---------------------|-------------------|-------------------------------|-------------------|
| `pipelines::SceneClassifier`          | `scene-classifier`  | 224x224 RGB frame | `Vec<SceneClassification>`    | Places365/ResNet  |
| `pipelines::ShotBoundaryDetector`     | `shot-boundary`     | 48x27 RGB window  | `Vec<ShotBoundary>`           | TransNet V2       |
| `pipelines::AestheticScorer`          | `aesthetic-score`   | 224x224 RGB frame | `AestheticScore`              | NIMA              |
| `pipelines::ObjectDetector`           | `object-detector`   | 640x640 RGB frame | `Vec<Detection>`              | YOLOv8 (80 COCO)  |
| `pipelines::FaceEmbedder`             | `face-embedder`     | 112x112 RGB face  | `FaceEmbedding` (512-dim)     | ArcFace           |

## Device Selection

`DeviceType::auto()` probes capabilities once (memoised in an `OnceLock`) and returns the strongest available device (CUDA > DirectML > WebGPU > CPU):

```rust
use oximedia_ml::{DeviceCapabilities, DeviceType};

let device = DeviceType::auto();

for cap in DeviceCapabilities::probe_all() {
    println!(
        "{:?}: {}",
        cap.device_type,
        if cap.is_available { "available" } else { "unavailable" },
    );
}
```

Pass `DeviceType::Cpu` to force the pure-Rust path, or pick a specific GPU backend when you know the deployment target.

## WebAssembly Support

| Feature set                              | `wasm32-unknown-unknown` |
|------------------------------------------|--------------------------|
| default (no features)                    | builds                   |
| `onnx`                                   | builds                   |
| `onnx` + any pipeline features           | builds                   |
| `webgpu`                                 | builds                   |
| `directml`                               | builds                   |
| `cuda`                                   | does not build           |

The `cuda` feature is native-only due to `oxicuda-driver` requiring a GPU driver loader. On WASM, all inference runs the pure-Rust CPU path unless `webgpu` is enabled.

## Links

- [Full API docs](https://docs.rs/oximedia-ml)
- [ML Guide](../../docs/ml_guide.md)
- [OxiMedia workspace](https://github.com/cool-japan/oximedia)

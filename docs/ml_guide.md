# OxiMedia ML Guide

*Sovereign, Pure-Rust machine learning for multimedia workloads.*

OxiMedia 0.1.5 introduces the [`oximedia-ml`](../crates/oximedia-ml/) crate: a
typed-pipeline layer over the [OxiONNX](https://crates.io/crates/oxionnx)
runtime that delivers scene classification, shot-boundary detection, aesthetic
scoring, object detection, and face embeddings with zero C/Fortran in the
default build.

This guide is the reference for application authors who want to consume those
pipelines from Rust, the `oximedia` facade, the CLI, or WebAssembly.

---

## Table of contents

1. [Introduction](#1-introduction)
2. [Quick start](#2-quick-start)
3. [Feature matrix](#3-feature-matrix)
4. [Device selection](#4-device-selection)
5. [Typed pipelines](#5-typed-pipelines)
6. [Custom ONNX models](#6-custom-onnx-models)
7. [CLI (`oximedia ml ...`)](#7-cli-oximedia-ml-)
8. [WebAssembly support](#8-webassembly-support)
9. [Roadmap](#9-roadmap)
10. [See also](#10-see-also)

---

## 1. Introduction

OxiMedia's ML layer is built around three invariants:

- **Pure-Rust by default.** The default build pulls in *zero* ONNX symbols.
  Inference is opt-in via the `onnx` feature (which depends on
  [`oxionnx`](https://crates.io/crates/oxionnx) 0.1.x). Everything else in the
  crate — types, traits, preprocessing, postprocessing — compiles without it.
- **Typed inputs and outputs.** Callers speak in domain terms (`SceneImage`,
  `ShotFrame`, `Detection`, `FaceEmbedding`) rather than raw tensors. Every
  pipeline implements the same [`TypedPipeline`](../crates/oximedia-ml/src/pipeline.rs)
  trait so they can be slotted into the workflow engine uniformly.
- **No hidden failure modes.** Every fallible operation returns `MlResult<T>`
  (a `Result<T, MlError>`). The crate is compiled under
  `deny(clippy::unwrap_used, clippy::expect_used)` for non-test code.

At a glance the stack looks like:

```
 ┌───────────────────────────────────────────────┐
 │   Typed pipelines (SceneClassifier, ...)      │  oximedia-ml::pipelines::*
 ├───────────────────────────────────────────────┤
 │   Pre / post helpers, ModelCache, ModelZoo    │  oximedia-ml::{preprocess, postprocess, cache, zoo}
 ├───────────────────────────────────────────────┤
 │   OnnxModel + DeviceType (auto probe)         │  oximedia-ml::{model, device}
 ├───────────────────────────────────────────────┤
 │   oxionnx (Pure-Rust ONNX runtime)            │  feature = "onnx"
 └───────────────────────────────────────────────┘
```

You never have to touch `oxionnx` directly unless you want to: the pipeline
constructors wrap model loading, preprocessing, and postprocessing into
single-call APIs.

---

## 2. Quick start

All ML features are **off by default** — both on `oximedia-ml` and on the
`oximedia` facade. Opt in explicitly.

### 2.1 Via the facade (`oximedia`)

```toml
[dependencies]
oximedia = { version = "0.1.5", features = ["ml", "ml-scene-classifier", "ml-onnx"] }
```

```rust,no_run
use oximedia::ml::pipelines::{SceneClassifier, SceneImage};
use oximedia::ml::{DeviceType, TypedPipeline};

fn classify_frame(rgb: Vec<u8>) -> oximedia::ml::MlResult<()> {
    let classifier = SceneClassifier::load("places365.onnx", DeviceType::auto())?;
    let image = SceneImage::new(rgb, 224, 224)?;
    for pred in classifier.run(image)? {
        println!("class {} -> {:.3}", pred.class_index, pred.score);
    }
    Ok(())
}
```

### 2.2 Via `oximedia-ml` directly

```toml
[dependencies]
oximedia-ml = { version = "0.1.5", features = ["onnx", "scene-classifier"] }
```

```rust,no_run
use oximedia_ml::pipelines::{SceneClassifier, SceneImage};
use oximedia_ml::{DeviceType, TypedPipeline};

fn top_scene(rgb: Vec<u8>) -> oximedia_ml::MlResult<usize> {
    let classifier = SceneClassifier::load("places365.onnx", DeviceType::auto())?;
    let image = SceneImage::new(rgb, 224, 224)?;
    let top = classifier.run(image)?;
    Ok(top.first().map(|p| p.class_index).unwrap_or(0))
}
```

### 2.3 Without `onnx`

With `onnx` disabled, `OnnxModel::load` returns `MlError::FeatureDisabled` and
every pipeline surfaces the same error from `run`. This lets a caller compile
their code unchanged, detect the error at runtime, and fall back to a
heuristic path:

```rust,ignore
match classifier.run(image) {
    Ok(top) => use_predictions(top),
    Err(MlError::FeatureDisabled(_)) => fallback_heuristic(),
    Err(e) => return Err(e),
}
```

The `ShotBoundaryDetector` has a built-in heuristic mode: construct it with
[`ShotBoundaryDetector::heuristic`](../crates/oximedia-ml/src/pipelines/shot_boundary.rs)
and you get frame-difference scoring with no ONNX runtime at all.

---

## 3. Feature matrix

### 3.1 `oximedia-ml` crate features

| Feature              | Purpose                                                           | Implies         |
|----------------------|-------------------------------------------------------------------|-----------------|
| *(default)*          | Types, traits, preprocess/postprocess helpers, `OnnxModel` / `ModelCache` scaffolding. `run()` returns `MlError::FeatureDisabled`. | — |
| `onnx`               | Compile the real `oxionnx` runtime. Enables every `run()` path.   | —               |
| `cuda`               | Add `oxionnx-cuda` for NVIDIA GPU execution (native-only).        | `onnx`          |
| `webgpu`             | Add `oxionnx-gpu` (wgpu backend). Works native + browser.         | `onnx`          |
| `directml`           | Add `oxionnx-directml`. Windows runtime, stub elsewhere.          | `onnx`          |
| `serde`              | Derive `Serialize` on metadata / value types.                     | —               |
| `scene-classifier`   | Build `SceneClassifier`.                                          | —               |
| `shot-boundary`      | Build `ShotBoundaryDetector` (also supplies a heuristic mode).    | —               |
| `aesthetic-score`    | Build `AestheticScorer`.                                          | `onnx`          |
| `object-detector`    | Build `ObjectDetector` (YOLOv8-compatible).                       | `onnx`          |
| `face-embedder`      | Build `FaceEmbedder` (ArcFace-compatible).                        | `onnx`          |
| `all-pipelines`      | Enable every pipeline feature above.                              | transitive `onnx` |

Two subtleties worth naming:

- **`OnnxModel` and `ModelCache` always exist.** They are always re-exported
  from the crate root. Without `onnx`, `OnnxModel::load` simply returns
  `MlError::FeatureDisabled` so callers can code against the types whether
  or not the runtime is linked.
- **Pipeline features do not all imply `onnx`.** `scene-classifier` and
  `shot-boundary` leave the decision to you — useful if you want the types
  and postprocessors compiled in a Pure-Rust-only environment.

### 3.2 `oximedia` facade features

The top-level `oximedia` meta-crate exposes a reduced, application-shaped API:

| Feature                | Effect                                                      |
|------------------------|-------------------------------------------------------------|
| *(default)*            | No ML. The `ml` module is not compiled.                     |
| `ml`                   | Enable `oximedia::ml` → re-exports `oximedia-ml` types/trait. |
| `ml-onnx`              | Forward to `oximedia-ml/onnx`.                              |
| `ml-scene-classifier`  | Forward to `oximedia-ml/scene-classifier`.                  |
| `ml-shot-boundary`     | Forward to `oximedia-ml/shot-boundary`.                     |
| `ml-aesthetic-score`   | Forward to `oximedia-ml/aesthetic-score` (implies `onnx`).  |
| `ml-object-detector`   | Forward to `oximedia-ml/object-detector` (implies `onnx`).  |
| `ml-face-embedder`     | Forward to `oximedia-ml/face-embedder` (implies `onnx`).    |
| `full`                 | Enable every `ml-*` feature above + general framework surface. |

### 3.3 Downstream crate integrations

Several domain crates gain an `onnx` feature that activates an ML-backed
fast path while keeping the Pure-Rust implementation intact:

| Crate                     | Feature  | Type / function surfaced                               |
|---------------------------|----------|--------------------------------------------------------|
| `oximedia-scene`          | `onnx`   | `MlSceneEnricher`                                      |
| `oximedia-shots`          | `onnx`   | `MlShotDetector`                                       |
| `oximedia-caption-gen`    | `onnx`   | `CaptionEncoder`, `greedy_decode`, `top_k_sample`      |
| `oximedia-recommend`      | `onnx`   | `EmbeddingExtractor`, `rank_by_similarity`             |
| `oximedia-mir`            | `onnx`   | `MusicTagger`, `TagActivationScore`                    |

Each of these keeps their own deterministic fallback so a Pure-Rust build
remains complete.

---

## 4. Device selection

OxiMedia probes available backends at runtime and memoises the result. Users
rarely have to hard-code a backend.

### 4.1 Cascade

`DeviceType::auto()` walks the following order, first hit wins:

1. **CUDA** — `oxionnx_cuda::CudaContext::try_new()` (feature `cuda`).
2. **DirectML** — `oxionnx_directml::DirectMLContext::try_new()` (feature `directml`).
3. **WebGPU** — `oxionnx_gpu::GpuContext::try_new()` (feature `webgpu`).
4. **CPU** — always available.

Every probe is wrapped in `std::panic::catch_unwind`, so a misbehaving driver
cannot unwind through your call stack. The result is cached in a
`OnceLock`, so repeated calls are cheap.

### 4.2 GPU backend table

| `DeviceType` | Feature flag | Runtime requirement                         | WASM build | Notes                                   |
|--------------|--------------|---------------------------------------------|------------|-----------------------------------------|
| `Cpu`        | *(always)*   | Pure Rust                                   | yes        | Baseline; INT8 supported.               |
| `Cuda`       | `cuda`       | NVIDIA driver + CUDA runtime on host        | **no**     | `libloading`-based; native-only.        |
| `DirectMl`   | `directml`   | Windows 10+ with DX12 device                | yes (stub) | Runtime on Windows, stub elsewhere.     |
| `WebGpu`     | `webgpu`     | wgpu-compatible GPU or browser WebGPU       | yes        | Works in browsers + native.             |
| `CoreMl`     | *(reserved)* | *(no `coreml` feature yet)*                 | yes        | Always reports unavailable.             |

Rich capability reports are available via `DeviceCapabilities`:

```rust,no_run
use oximedia_ml::{DeviceCapabilities, DeviceType};

for cap in DeviceCapabilities::probe_all() {
    println!(
        "{} :: {} (fp16={}, int8={})",
        cap.device_type,
        if cap.is_available { "available" } else { "unavailable" },
        cap.supports_fp16,
        cap.supports_int8,
    );
}

let best = DeviceCapabilities::best_available();
println!("auto => {}", best.device_name);
```

### 4.3 Forcing a backend

Pass a specific `DeviceType` to any pipeline constructor when you need a
hard guarantee:

```rust,ignore
let classifier = SceneClassifier::load("places365.onnx", DeviceType::Cpu)?;
```

If the requested backend is unavailable (feature compiled out, no GPU, etc.)
the underlying `OnnxModel::load` returns `MlError::FeatureDisabled` — callers
should re-try with `DeviceType::auto()` as a fallback.

---

## 5. Typed pipelines

All pipelines live under [`pipelines`](../crates/oximedia-ml/src/pipelines/)
and implement `TypedPipeline<Input, Output>`.

| Pipeline                              | Feature             | Input (RGB u8)     | Output                        | Reference model   |
|---------------------------------------|---------------------|--------------------|-------------------------------|-------------------|
| `pipelines::SceneClassifier`          | `scene-classifier`  | 224×224 `SceneImage` | `Vec<SceneClassification>`  | Places365 / ResNet |
| `pipelines::ShotBoundaryDetector`     | `shot-boundary`     | 48×27 `&[ShotFrame]` window | `Vec<ShotBoundary>`     | TransNet V2       |
| `pipelines::AestheticScorer`          | `aesthetic-score`   | 224×224 `AestheticImage` | `AestheticScore`          | NIMA              |
| `pipelines::ObjectDetector`           | `object-detector`   | 640×640 `DetectorImage` | `Vec<Detection>` (NMS)     | YOLOv8 (80 COCO)  |
| `pipelines::FaceEmbedder`             | `face-embedder`     | 112×112 `FaceImage` | `FaceEmbedding` (512-dim, L2-normed) | ArcFace |

### 5.1 I/O contracts (summary)

| Stage        | SceneClassifier        | ShotBoundaryDetector    | AestheticScorer          | ObjectDetector            | FaceEmbedder                |
|--------------|------------------------|-------------------------|--------------------------|---------------------------|-----------------------------|
| Preprocess   | NCHW f32, ImageNet μ/σ | NCHW f32 in `[0, 1]`    | NCHW f32, ImageNet μ/σ   | NCHW f32 + letterbox      | NCHW f32, ImageNet μ/σ      |
| ONNX input   | `[1, 3, 224, 224]`     | `[1, W, 3, H, W]`       | `[1, 3, 224, 224]`       | `[1, 3, 640, 640]`        | `[1, 3, 112, 112]`          |
| ONNX output  | logits `[1, C]`        | per-frame logits `[1, W]` | distribution `[1, 10]` | YOLOv8 head               | embedding `[1, D]` (D=512)  |
| Postprocess  | `softmax` + `top_k`    | `sigmoid_slice` + threshold + `min_gap` | weighted mean over bins | `decode_yolov8_output` + NMS | `l2_normalize` |
| Output       | `Vec<SceneClassification>` | `Vec<ShotBoundary>` | `AestheticScore`            | `Vec<Detection>`          | `FaceEmbedding`             |

Full per-pipeline contracts (tensor names, compatible exports, config knobs,
and `# Examples` blocks) live in each submodule's rustdoc.

### 5.2 Common patterns

**Shared models via `ModelCache`**:

```rust,ignore
use oximedia_ml::{ModelCache, DeviceType};
use oximedia_ml::pipelines::{AestheticImage, AestheticScorer, AestheticScorerConfig};

let cache = ModelCache::new(4);
let model = cache.get_or_load("nima.onnx", DeviceType::auto())?;
let scorer = AestheticScorer::from_shared(model, AestheticScorerConfig::default(), "nima.onnx".into());
```

**Batch inference over a timeline**:

```rust,ignore
use oximedia_ml::TypedPipeline;

let scorer = AestheticScorer::load("nima.onnx", DeviceType::auto())?;
let scores: Vec<_> = frames
    .into_iter()
    .map(|img| scorer.run(img))
    .collect::<oximedia_ml::MlResult<_>>()?;
```

---

## 6. Custom ONNX models

The built-in pipelines are templated against specific tensor shapes, but
their configs are public and every pipeline ships a `load_with_config`
constructor.

- **Override tensor names.** `SceneClassifierConfig::input_name` and
  `SceneClassifierConfig::output_name` let you point at a non-default tensor
  when your exporter picked different names.
- **Override input size and normalization.** Every config exposes
  `input_size`, `mean`, `std` — useful if you fine-tuned a ResNet with
  different statistics, or exported at a non-ImageNet resolution.
- **Override embedding dimensionality.** `FaceEmbedderConfig::embedding_dim`
  accepts any positive value; the `FaceEmbedding` output type stores a
  `Vec<f32>` and enforces L2-normalization downstream.
- **Register your model in the zoo.** `ModelZoo::with_defaults().add(...)`
  puts your entry into the same catalog `oximedia ml list` uses.

For fully bespoke models, drop down to `OnnxModel::load` directly and drive
the `oxionnx::Session` via `model.run(&inputs)`. Typed pipelines are the
recommended path, but raw access is always available.

---

## 7. CLI (`oximedia ml ...`)

The `oximedia-cli` binary ships three ML subcommands. They all honour
`--json` for machine-readable output.

### 7.1 `oximedia ml list`

Enumerate every built-in pipeline and the entries registered in the default
model zoo.

```bash
oximedia ml list
oximedia ml list --json | jq '.pipelines[].id'
```

Pipelines whose feature is not compiled-in still show up, flagged
`disabled` — so users can see the full zoo before recompiling.

### 7.2 `oximedia ml probe`

Probe every compiled-in execution backend and print a capability table.

```bash
oximedia ml probe
oximedia ml probe --device cuda --json
```

The summary line at the bottom reports what `DeviceType::auto()` would
pick *right now*, so CI diagnostics can capture it in one place.

### 7.3 `oximedia ml run`

Run a typed pipeline end-to-end against a model + input file.

```bash
# Dry-run: validate inputs + device without touching the ONNX runtime
oximedia ml run \
    --pipeline scene-classifier \
    --model places365.onnx \
    --input frame.png \
    --device auto \
    --top-k 5 \
    --dry-run
```

Arguments:

- `--pipeline` — one of `scene-classifier`, `shot-boundary`,
  `aesthetic-score`, `object-detector`, `face-embedder`.
- `--device` — `auto` (default), `cpu`, `cuda`, `webgpu`, `directml`, `coreml`.
- `--top-k`, `--threshold` — pipeline-specific knobs (validated up-front).
- `--json` — emit the whole summary as JSON.
- `--dry-run` — validate everything and exit before the runtime is invoked.

Non-dry-run `run` across every pipeline is delivered in waves alongside the
per-format media decoders; until then, the CLI errors clearly and directs
users at the `oximedia-ml` Rust API.

The `oximedia ml ...` namespace is always accepted by the clap dispatcher;
when built without `--features ml` it reports "rebuild with `--features ml`"
instead of clap reporting an unknown subcommand.

---

## 8. WebAssembly support

`oximedia-ml` is validated on `wasm32-unknown-unknown` on every release:

| Feature set                                                              | `wasm32-unknown-unknown` |
|--------------------------------------------------------------------------|--------------------------|
| *default* (no features)                                                  | builds                   |
| `onnx`                                                                   | builds                   |
| `onnx` + any of `scene-classifier` / `shot-boundary` / `aesthetic-score` / `object-detector` / `face-embedder` / `all-pipelines` | builds                   |
| `webgpu` (wgpu browser backend)                                          | builds                   |
| `directml` (stub on non-Windows)                                         | builds                   |
| `cuda`                                                                   | **does not build**       |

The `cuda` feature transitively depends on `oxicuda-driver`, which uses
[`libloading`](https://crates.io/crates/libloading) to bind the NVIDIA driver
at runtime. `libloading::Library` is gated on `cfg(any(unix, windows))`, so
the crate will never compile on `wasm32-unknown-unknown`. This is a
fundamental property of GPU driver loading rather than a limitation of
OxiMedia, so `cuda` is treated as a **native-only** feature.

In the browser, the CPU path (always Pure-Rust) is what users almost always
want. Opt into `webgpu` when you have a WebGPU-capable environment; the same
typed pipelines work unchanged.

---

## 9. Roadmap

Items planned for follow-up waves in the 0.1.5 cycle and beyond:

- **AutoCaption pipeline** — ONNX-backed speech/caption alignment typed
  pipeline (currently only the `oximedia-caption-gen::CaptionEncoder` helpers
  ship; the high-level `AutoCaption` pipeline is queued).
- **`oximedia ml run` full data path** — wire per-format media decoders into
  every pipeline's non-dry-run path so `run` returns real predictions from
  the CLI.
- **Python bindings (`oximedia.ml`)** — PyO3 surface mirroring the Rust API
  (Wave 5 Slice B).
- **`oximedia-neural` ONNX backend** — route the homegrown `oximedia-neural`
  runtime onto `oxionnx` so the workspace ships a single ML stack.
- **CoreML backend** — reserved `DeviceType::CoreMl` variant is ready; the
  Apple backend itself is not yet implemented.

Status for each of these items is tracked in [`TODO.md`](../TODO.md) under
the 0.1.5 Wave headings.

---

## 10. See also

- Crate rustdoc: [`oximedia-ml`](https://docs.rs/oximedia-ml) — canonical
  reference for every type, trait, and feature gate described here.
- Crate rustdoc: [`oximedia`](https://docs.rs/oximedia) — top-level facade,
  including `oximedia::ml` module and prelude.
- Examples:
  [`examples/ml_scene_classify.rs`](../examples/ml_scene_classify.rs),
  [`examples/ml_auto_caption.rs`](../examples/ml_auto_caption.rs),
  [`examples/ml_model_zoo.rs`](../examples/ml_model_zoo.rs).
- Runtime: [OxiONNX](https://crates.io/crates/oxionnx) — Pure-Rust ONNX
  runtime that powers every pipeline.
- Project roadmap: [`TODO.md`](../TODO.md) for waves and slice-level status.
- Release notes: [`CHANGELOG.md`](../CHANGELOG.md) for per-version detail.

---

*Copyright 2026 COOLJAPAN OU (Team KitaSan). Apache-2.0.*

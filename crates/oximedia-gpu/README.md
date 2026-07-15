# oximedia-gpu

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Cross-platform GPU compute pipeline for OxiMedia using WGPU.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Backend selection

`oximedia-gpu` uses [wgpu](https://wgpu.rs/) to access the GPU.  The backend is
chosen **at runtime** — no compile-time feature flags are needed:

| Platform | Backend chosen |
|----------|----------------|
| Linux    | Vulkan (preferred), OpenGL ES (fallback) |
| macOS    | Metal |
| Windows  | DirectX 12, Vulkan (fallback) |
| Web      | WebGPU |
| All      | CPU software fallback when no GPU adapter is present |

## Features

**Color operations:**
- **Color Space Conversions** — RGB ↔ YUV with BT.601, BT.709, BT.2020 matrices
- **Chroma Subsampling** — 4:2:0, 4:2:2, 4:4:4 subsampling/upsampling
- **Tone Mapping** — Reinhard, Hable, ACES, Drago algorithms

**Geometry and scale:**
- **Image Scaling** — Bilinear, bicubic, and Lanczos-3 interpolation on GPU
- **Convolution Filters** — Blur, sharpen, edge-detect, custom kernels
- **Transform Operations** — DCT and FFT on GPU
- **Perspective Transform** — Projective image warping
- **Mipmap Generation** — Automatic mipmap chain computation

**Signal and media processing:**
- **Histogram Equalization** — CLAHE (Contrast-Limited Adaptive HE)
- **Motion Detection** — GPU-accelerated motion analysis with sensitivity levels
- **Optical Flow** — Dense optical flow estimation
- **Film Grain** — Perceptual grain synthesis
- **Denoising** — Bilateral and NLM denoising kernels

**Quality metrics:**
- **PSNR, SSIM, MS-SSIM** — Compute image quality metrics on GPU

**Infrastructure:**
- **TexturePool** — LRU-evicting byte-budget pool (see below)
- **Shader Cache** — Two-level in-memory + disk-persistent cache (see below)
- **Pipeline DAG** — Barrier-managed processing pipeline
- **SubAllocator** — Bump-pointer GPU buffer sub-allocator with defragmentation
- **BatchedComputePass** — Recorded dispatch queue for compute workloads
- **Automatic CPU Fallback** — Graceful degradation when GPU unavailable
- **Multi-GPU Support** — Enumerate and select GPU devices
- **Command Buffer** — Batched GPU command recording
- **Compute Pass** — Structured compute pass dispatch
- **Descriptor Sets** — Resource binding management
- **Render Pass** — GPU render pass management
- **Fence Pool** — GPU fence lifecycle management
- **Vertex Buffer** — Vertex data management
- **Sampler** — Texture sampler configuration
- **Profiling** — GPU timer, stats, and profiler
- **Occupancy** — Compute occupancy analysis
- **Workgroup** — Automatic workgroup sizing and dispatch

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-gpu = "0.2.0"
```

```rust
use oximedia_gpu::GpuContext;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = GpuContext::new()?;

    let input = vec![0u8; 1920 * 1080 * 4];
    let mut output = vec![0u8; 1920 * 1080 * 4];

    ctx.rgb_to_yuv(&input, &mut output)?;
    Ok(())
}
```

## TexturePool — LRU eviction

`TexturePool::new(max_gb)` creates a pool bounded by a byte budget and a slot
count.  When both limits are exhausted,
`TexturePool::allocate_with_lru_eviction()` evicts textures in a loop until
enough capacity is reclaimed.

LRU order is tracked with a monotonic `access_clock` counter stored per slot.
`lru_handle()` returns the slot with the smallest timestamp.  Call
`TexturePool::touch(handle)` after each access to refresh the timestamp.

Supported texture formats: `Rgba8`, `Rgba16f`, `Rgb10A2`, `R8`, `Rg8`,
`Yuv420`, `Nv12`.

```rust
use oximedia_gpu::texture::{TexturePool, TextureDescriptor, TextureFormat};

let mut pool = TexturePool::new(2.0); // 2 GiB budget
let desc = TextureDescriptor {
    width: 1920,
    height: 1080,
    format: TextureFormat::Rgba8,
    label: Some("frame".to_string()),
};
if let Some(handle) = pool.allocate(&desc) {
    pool.touch(handle);
}
```

## Shader cache

`shader_cache::GpuShaderCache` provides two caching levels:

**In-memory cache:**
- Configurable eviction policy via `EvictionPolicy`: `Lru`, `Lfu`, or `OldestFirst`
- Hit/miss counters accessible via `GpuShaderCache::stats()`
- Cache key: `ShaderVersion { source_hash: u64, backend: String, feature_flags: u32 }`

**Disk-persistent cache:**
- Compiled bytecode stored as `<hex_hash>_<backend>_<flags>.shd`
- Metadata sidecar at `<hex_hash>_<backend>_<flags>.meta`
- Cache is invalidated when any component of `ShaderVersion` changes

## API Overview

**Core types:**
- `GpuContext` — Main GPU context and entry point
- `GpuBuffer`, `GpuFence` — GPU resource types

**Device and backend:**
- `device` — GPU device enumeration and selection
- `backend` — Backend type information (`BackendType`: Vulkan, Metal, DX12, CPU)
- `accelerator` — High-level acceleration interface (`WgpuAccelerator`, `CpuAccelerator`)

**Buffer and memory:**
- `buffer`, `gpu_buffer` — Buffer allocation and management
- `memory`, `memory_pool` — GPU memory pool with `SubAllocator` defragmentation
- `vertex_buffer` — Vertex buffer management
- `buffer_copy` — Buffer copy operations
- `upload_queue` — Staging buffer upload queue

**Shader management:**
- `shader`, `shader_cache`, `shader_params` — Shader compilation and caching
- `compiler` — `ShaderCompiler` with `OptimizationLevel` (None/Speed/Size)

**Compute pipeline:**
- `compute`, `compute_pass`, `compute_dispatch` — Compute operations
- `pipeline` — `GpuPipeline` DAG; `BarrierBatcher` (Eager/Batched/Deferred)
- `kernels`, `kernel` — Compute kernel definitions
- `descriptor_set` — Resource descriptor binding
- `workgroup` — `WorkgroupAutoTuner` for optimal dispatch sizing

**Ops (high-level media kernels):**
- `ops::colorspace` — `ColorSpaceConversion` (BT601/709/2020)
- `ops::chroma` — `ChromaOps` subsampling/upsampling
- `ops::scale` — `ScaleOperation` with `ScaleFilter` (Bilinear/Bicubic/Lanczos3)
- `ops::filter` — `FilterOperation` convolution kernels
- `ops::tonemap` — `TonemapAlgorithm` (Reinhard/Hable/ACES/Drago)
- `ops::denoise` — `DenoiseKernel`
- `ops::histogram_eq` — `HistogramEqualizer` with `ClaheConfig`
- `ops::quality_metrics` — `compute_psnr`, `compute_ssim`, `compute_ms_ssim`
- `ops::transform` — `TransformOperation` DCT/FFT
- `ops::composite` — Layer compositing

**Texture and rendering:**
- `texture` — `TexturePool` with LRU eviction, `TextureFormat` enum
- `render_pass` — GPU render pass
- `sampler` — Sampler configuration
- `viewport` — Viewport configuration
- `texture_atlas`, `texture_cache`, `mipmap_gen` — Texture utilities

**Synchronization:**
- `queue` — Command queue management
- `sync`, `sync_primitive` — Fence and semaphore synchronization
- `fence_pool` — Fence lifecycle management

**Video processing:**
- `video_process` — `VideoFrameProcessor` frame pipeline
- `histogram` — `ImageHistogram` / `ChannelHistogram`
- `motion_detect` — `MotionDetector` with `Sensitivity` levels
- `optical_flow` — Dense optical flow

**Profiling:**
- `gpu_profiler` — GPU profiling
- `gpu_timer` — GPU timing queries
- `gpu_stats` — GPU statistics collection
- `resource_manager` — GPU resource lifecycle tracking
- `occupancy` — Compute occupancy analysis

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

# oximedia-accel

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Hardware acceleration layer for OxiMedia. **Pure Rust by default** (CPU
fallback, plus an optional `wgpu`-backed WebGPU path), with an **opt-in**
Vulkan compute backend for systems that want real GPU dispatch via `vulkano`.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 409+ tests

## Features

- Pure-Rust CPU fallback for all operations (always available, default build)
- Optional `webgpu` feature: real WebGPU compute dispatch via `wgpu`
- Optional `vulkan-backend` feature: automatic GPU device enumeration/selection,
  buffer management, and compute kernels (image scaling, color conversion,
  motion estimation) via Vulkan/`vulkano`
- Optional `metal-backend` feature: native Metal compute on macOS/iOS
- Task graph scheduling for concurrent GPU operations
- Memory arena and pool management
- Fence timeline for GPU synchronization
- Pipeline acceleration abstractions
- Profiling and performance statistics
- Prefetch and cache management

## Pure Rust default / opt-in Vulkan backend (`vulkan-backend` feature)

**The default build of this crate compiles zero C/C++ code.** Vulkan support
is gated behind the non-default `vulkan-backend` Cargo feature because
[`vulkano-shaders`](https://crates.io/crates/vulkano-shaders) uses the
`vulkano_shaders::shader!` proc-macro to compile GLSL compute shaders to
SPIR-V **at build time**, and that macro pulls in `shaderc-sys`. When no
pre-built `shaderc` library is found on the host (the typical case on a fresh
developer machine), `shaderc-sys` builds `shaderc` from source, which
requires the following native tools to be on `PATH`:

| Tool | Purpose | Install |
|---|---|---|
| **cmake** (>= 3.17) | Drives the `shaderc` C++ build | `brew install cmake` / `apt install cmake` / `winget install Kitware.CMake` |
| **Python 3** | Used by `glslang`'s build scripts | `brew install python` / `apt install python3` / `winget install Python.Python.3` |
| **C++ compiler** | Compiles `shaderc` / `glslang` / `SPIRV-Tools` | Xcode CLT / `apt install build-essential` / MSVC Build Tools |
| **Git** | `shaderc-sys` clones `shaderc` sources | `brew install git` / `apt install git` / `winget install Git.Git` |

These prerequisites are only needed when explicitly opting in:

```toml
[dependencies]
oximedia-accel = { version = "0.1.9", features = ["vulkan-backend"] }
```

Without `vulkan-backend` (the default), `vulkano`/`vulkano-shaders` are not
even dependencies — `cargo tree -p oximedia-accel` shows no trace of them —
and every Vulkan-only module (`device`, `buffer`, `vulkan`, `kernels`,
`descriptor_pool`) is entirely absent from the compiled crate.
[`AccelContext::new`] always selects the Pure-Rust CPU fallback (or `webgpu`,
when that feature is enabled) directly; it never attempts to load Vulkan. A
runtime request that specifically requires the Vulkan backend (e.g.
`compute_backend::VulkanComputeBackend::is_available()`) simply reports
unavailable rather than panicking.

The related `vulkan-detect` feature (runtime Vulkan availability probing)
implies `vulkan-backend`, since detection requires the real `vulkano` types.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-accel = "0.2.0"
```

```rust
use oximedia_accel::{AccelContext, HardwareAccel, ScaleFilter};
use oximedia_core::types::PixelFormat;

fn example() -> Result<(), Box<dyn std::error::Error>> {
    // Create acceleration context (automatically selects GPU or CPU)
    let accel = AccelContext::new()?;

    // Perform image scaling
    let input = vec![0u8; 1920 * 1080 * 3];
    let output = accel.scale_image(
        &input,
        1920, 1080,
        1280, 720,
        PixelFormat::Rgb24,
        ScaleFilter::Bilinear,
    )?;
    Ok(())
}
```

## API Overview

**Core types:**
- `AccelContext` — Main entry point; selects GPU or CPU backend automatically
- `HardwareAccel` (trait) — Unified interface for GPU and CPU implementations
- `ScaleFilter` — Scaling filter variants (nearest, bilinear, bicubic)
- `AccelError`, `AccelResult` — Error types

**Backends:**
- `VulkanAccel` — Vulkan compute backend (requires the `vulkan-backend` feature)
- `CpuFallback` — Pure-CPU fallback implementation (always available)

**Modules (37 source files, 401 public items):**
- `device`, `buffer`, `vulkan`, `kernels`, `descriptor_pool` — GPU device
  management, buffer transfer, and Vulkan compute kernels (all require the
  `vulkan-backend` feature; absent from the default Pure-Rust build)
- `device_caps`, `pool`, `memory_arena`, `memory_bandwidth` — Memory management
  and capability detection (always available)
- `shaders` — SPIR-V compute shader sources; the raw GLSL string constants and
  their structural tests are always available, while the `vulkano_shaders`
  macro-compiled shader modules require `vulkan-backend`
- `task_graph`, `task_scheduler`, `dispatch` — Parallel task scheduling
- `pipeline_accel` — Pipeline-level acceleration
- `fence_timeline` — GPU synchronization primitives
- `ops` — High-level compute operations
- `cache`, `prefetch` — Caching and prefetch strategies
- `accel_profile`, `accel_stats` — Profiling and statistics
- `traits` — Core trait definitions
- `error` — Error types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

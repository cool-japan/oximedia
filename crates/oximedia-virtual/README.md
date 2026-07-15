# oximedia-virtual

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Virtual Production and LED Wall Tools for OxiMedia. Provides comprehensive virtual production capabilities including camera tracking, LED wall rendering, in-camera VFX compositing, and real-time synchronization for professional film and broadcast production.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Camera Tracking
- **6DOF Tracking** - Real-time position and orientation tracking
- **Multi-Sensor Fusion** - Combines optical markers and IMU data
- **Kalman Filtering** - Advanced filtering for smooth, accurate tracking
- **Sub-millimeter Accuracy** - Precise camera positioning
- **120Hz Update Rate** - High-frequency tracking for responsive control

### LED Wall Rendering
- **Perspective Correction** - Accurate rendering based on camera position
- **Multi-Panel Support** - Coordinate multiple LED panels with topology management
- **Color Calibration** - Brightness and color uniformity correction
- **10-bit Processing** - High bit-depth for professional quality
- **Temporal Dithering** - Reduces banding and improves visual quality
- **LED Volume Management** - Full LED volume calibration and projection mapping

### In-Camera VFX
- **Real-time Compositing** - Blend foreground and background elements
- **Depth-based Blending** - Use depth maps for realistic compositing
- **Multiple Blend Modes** - Normal, add, multiply, screen, overlay
- **Layer Management** - Organize composite elements
- **Render Passes** - Multi-pass rendering for complex compositing

### Virtual Studio and Set
- **Virtual Set Management** - Complete virtual studio environment
- **Stage Layout** - Physical stage geometry and coordinate systems
- **Projection Mapping** - Map content to physical surfaces
- **Scene Setup** - Pre-configured production environments

### Synchronization
- **Genlock Support** - Frame-accurate synchronization
- **Sub-millisecond Accuracy** - < 1ms sync tolerance
- **Multi-device Sync** - Coordinate cameras, LED walls, and capture

### Motion Capture
- **MoCap Integration** - Real-time motion capture data processing
- **Motion Path Analysis** - Smooth motion path interpolation
- **Talent Keying** - Real-time keying of talent within the virtual set

### Multi-Camera
- **Simultaneous Tracking** - Track multiple cameras concurrently
- **Coordinated Control** - Manage multi-camera setups
- **Automated Switching** - Intelligent camera selection

### Integration
- **Unreal Engine** - Metadata export, coordinate system conversion, real-time data streaming
- **NDI Bridge** - NDI-compatible signal routing within virtual production pipeline
- **Preview Monitor** - Operator preview of composite output

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-virtual = "0.2.0"
```

```rust
use oximedia_virtual::{VirtualProduction, VirtualProductionConfig, WorkflowType};

let config = VirtualProductionConfig::default()
    .with_workflow(WorkflowType::LedWall)
    .with_target_fps(60.0)
    .with_sync_accuracy_ms(0.5);

let mut vp = VirtualProduction::new(config)?;
```

### LED Wall Workflow

```rust
use oximedia_virtual::workflows::LedWallWorkflow;

let mut workflow = LedWallWorkflow::new()?;
workflow.start_recording("session-001".to_string(), 0)?;
// Process frames...
workflow.stop_recording();
```

### AR Workflow

```rust
use oximedia_virtual::workflows::ArWorkflow;

let mut workflow = ArWorkflow::new()?;
workflow.start("ar-session".to_string(), 0)?;
let result = workflow.overlay(&camera_feed, &virtual_content, timestamp)?;
```

### Configuration Presets

```rust
use oximedia_virtual::constants::presets;

let config = presets::led_wall_high_quality();
let config = presets::realtime_preview();
let config = presets::multi_camera_production();
let config = presets::unreal_integration();
let config = presets::ar_vr();
```

## Performance

- **60 FPS Minimum** - Maintains real-time performance
- **< 20ms Latency** - Total end-to-end latency
- **4K Support** - Handle ultra-high definition content
- **Zero-Copy** - Efficient memory management

## API Overview

- `VirtualProduction` — Main virtual production system: camera_tracker(), led_renderer(), compositor(), color_pipeline(), genlock(), multicam_manager()
- `VirtualProductionConfig` — Configuration builder: with_workflow(), with_target_fps(), with_sync_accuracy_ms(), with_quality(), with_num_cameras()
- `WorkflowType` — LedWall, Hybrid, GreenScreen, AugmentedReality
- `QualityMode` — Draft, Preview, Final
- `VirtualProductionError` — Error types: CameraTracking, LedWall, Calibration, Sync, Color, MotionCapture, Compositing, UnrealIntegration, MultiCamera
- Modules: `background_plate`, `camera_frustum`, `camera_tracking`, `color`, `constants`, `examples`, `frustum`, `genlock`, `greenscreen`, `icvfx`, `keying`, `led`, `led_volume`, `led_wall`, `lens`, `metrics`, `mocap`, `motion_path`, `multicam`, `ndi_bridge`, `panel_topology`, `pixel_mapping`, `preview`, `projection_map`, `render_layer`, `render_output`, `scene`, `scene_setup`, `stage`, `stage_layout`, `stage_manager`, `sync`, `talent_keying`, `tracking`, `tracking_data`, `tracking_session`, `unreal`, `utils`, `virtual_set`, `virtual_studio`, `volume_calibration`, `workflows`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)

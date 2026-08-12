# oximedia-plugin

**Status: [Stable]** | Version: 0.2.1 | Tests: extensively tested | Updated: 2026-08-12

Plugin system for [OxiMedia](https://github.com/cool-japan/oximedia) -- dynamic codec loading.

## Overview

This crate provides a plugin architecture that allows external codec implementations to be
registered with OxiMedia at runtime. Third-party or patent-encumbered codecs can be distributed
as separate shared libraries and loaded on demand, keeping the core OxiMedia framework patent-free.

### Key Components

- **`CodecPlugin` trait** -- The interface all plugins must implement, declaring metadata and codec capabilities
- **`PluginRegistry`** -- Central registry for discovering, loading, and querying plugins
- **`StaticPlugin`** -- Builder-pattern helper for registering plugins without shared libraries
- **`PluginManifest`** -- JSON-based plugin metadata for discovery and validation
- **`declare_plugin!` macro** -- Generates FFI exports for shared library plugins

## Feature Flags

| Feature | Description |
|---------|-------------|
| `dynamic-loading` | Enables loading plugins from shared libraries (.so/.dylib/.dll) via `libloading`. Without this feature, only static plugin registration is available. |

## Usage

### Static Plugin Registration

```rust
use oximedia_plugin::{StaticPlugin, CodecPluginInfo, PluginCapability, PluginRegistry};
use std::sync::Arc;
use std::collections::HashMap;

let info = CodecPluginInfo {
    name: "my-codec".to_string(),
    version: "1.0.0".to_string(),
    author: "Example".to_string(),
    description: "Example codec plugin".to_string(),
    api_version: oximedia_plugin::PLUGIN_API_VERSION,
    license: "Apache-2.0".to_string(),
    patent_encumbered: false,
};

let plugin = StaticPlugin::new(info)
    .add_capability(PluginCapability {
        codec_name: "my-codec".to_string(),
        can_decode: true,
        can_encode: false,
        pixel_formats: vec!["yuv420p".to_string()],
        properties: HashMap::new(),
    });

let registry = PluginRegistry::new();
registry.register(Arc::new(plugin)).expect("registration failed");
```

### Dynamic Loading (feature = "dynamic-loading")

Shared library plugins must export two symbols:

- `oximedia_plugin_api_version() -> u32`
- `oximedia_plugin_create() -> *mut dyn CodecPlugin`

Use the `declare_plugin!` macro to generate these exports automatically.

## License

Apache-2.0

Copyright COOLJAPAN OU (Team Kitasan)

//! WebAssembly bindings for the `oximedia-plugin` system.
//!
//! In the browser, dynamic plugin loading is not available, but this module
//! exposes static plugin information, built-in codec listings, and the
//! plugin API version for querying from JavaScript.

use wasm_bindgen::prelude::*;

use oximedia_plugin::{PluginRegistry, PLUGIN_API_VERSION};

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Get the current plugin API version.
///
/// Returns the API version number that plugins must match to be loaded.
#[wasm_bindgen]
pub fn wasm_plugin_api_version() -> u32 {
    PLUGIN_API_VERSION
}

/// List all built-in (patent-free) codecs as a JSON array.
///
/// Returns a JSON string like:
/// ```json
/// [
///   {"name": "av1", "type": "video", "decode": true, "encode": true},
///   {"name": "vp9", "type": "video", "decode": true, "encode": false},
///   ...
/// ]
/// ```
#[wasm_bindgen]
pub fn wasm_list_builtin_codecs() -> String {
    let codecs = serde_json::json!([
        {"name": "av1", "type": "video", "decode": true, "encode": true, "patent_free": true},
        {"name": "vp9", "type": "video", "decode": true, "encode": false, "patent_free": true},
        {"name": "vp8", "type": "video", "decode": true, "encode": false, "patent_free": true},
        {"name": "theora", "type": "video", "decode": true, "encode": false, "patent_free": true},
        {"name": "opus", "type": "audio", "decode": true, "encode": true, "patent_free": true},
        {"name": "vorbis", "type": "audio", "decode": true, "encode": false, "patent_free": true},
        {"name": "flac", "type": "audio", "decode": true, "encode": true, "patent_free": true},
        {"name": "pcm", "type": "audio", "decode": true, "encode": true, "patent_free": true},
    ]);
    // serde_json::to_string should not fail on a static json! value
    serde_json::to_string(&codecs).unwrap_or_else(|_| "[]".to_string())
}

/// Get plugin system information as a JSON string.
///
/// Returns:
/// ```json
/// {
///   "api_version": 1,
///   "dynamic_loading": false,
///   "search_paths": [],
///   "status": "static_only",
///   "builtin_codecs": 8,
///   "note": "Dynamic plugin loading is not available in WebAssembly"
/// }
/// ```
#[wasm_bindgen]
pub fn wasm_plugin_system_info() -> String {
    let info = serde_json::json!({
        "api_version": PLUGIN_API_VERSION,
        "dynamic_loading": false,
        "search_paths": [],
        "status": "static_only",
        "builtin_codecs": 8,
        "note": "Dynamic plugin loading is not available in WebAssembly"
    });
    serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------
// WasmPluginRegistry
// ---------------------------------------------------------------------------

/// Browser-side plugin registry.
///
/// In WebAssembly, only static/built-in plugins are available.
/// This provides a consistent API for querying codec support.
#[wasm_bindgen]
pub struct WasmPluginRegistry {
    inner: PluginRegistry,
    builtin_registered: bool,
}

#[wasm_bindgen]
impl WasmPluginRegistry {
    /// Create a new plugin registry.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: PluginRegistry::empty(),
            builtin_registered: false,
        }
    }

    /// Register built-in (patent-free) codecs.
    ///
    /// This populates the registry with OxiMedia's built-in codecs
    /// so they can be queried through the standard plugin API.
    pub fn register_builtin(&mut self) {
        if self.builtin_registered {
            return;
        }

        use oximedia_plugin::{CodecPluginInfo, PluginCapability, StaticPlugin};
        use std::collections::HashMap;
        use std::sync::Arc;

        let info = CodecPluginInfo {
            name: "oximedia-builtin".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "COOLJAPAN OU (Team KitaSan)".to_string(),
            description: "Built-in patent-free codecs for OxiMedia".to_string(),
            api_version: PLUGIN_API_VERSION,
            license: "MIT OR Apache-2.0".to_string(),
            patent_encumbered: false,
        };

        let plugin = StaticPlugin::new(info)
            .add_capability(PluginCapability {
                codec_name: "av1".to_string(),
                can_decode: true,
                can_encode: true,
                pixel_formats: vec!["yuv420p".to_string(), "yuv444p".to_string()],
                properties: HashMap::new(),
            })
            .add_capability(PluginCapability {
                codec_name: "vp9".to_string(),
                can_decode: true,
                can_encode: false,
                pixel_formats: vec!["yuv420p".to_string()],
                properties: HashMap::new(),
            })
            .add_capability(PluginCapability {
                codec_name: "vp8".to_string(),
                can_decode: true,
                can_encode: false,
                pixel_formats: vec!["yuv420p".to_string()],
                properties: HashMap::new(),
            })
            .add_capability(PluginCapability {
                codec_name: "opus".to_string(),
                can_decode: true,
                can_encode: true,
                pixel_formats: vec![],
                properties: HashMap::new(),
            })
            .add_capability(PluginCapability {
                codec_name: "vorbis".to_string(),
                can_decode: true,
                can_encode: false,
                pixel_formats: vec![],
                properties: HashMap::new(),
            })
            .add_capability(PluginCapability {
                codec_name: "flac".to_string(),
                can_decode: true,
                can_encode: true,
                pixel_formats: vec![],
                properties: HashMap::new(),
            });

        // Registration should not fail for a valid static plugin
        if self.inner.register(Arc::new(plugin)).is_ok() {
            self.builtin_registered = true;
        }
    }

    /// List all codecs in the registry as a JSON array.
    ///
    /// Returns JSON like:
    /// ```json
    /// [{"codec_name": "av1", "can_decode": true, "can_encode": true, "pixel_formats": ["yuv420p"]}]
    /// ```
    pub fn list_codecs(&self) -> String {
        let codecs: Vec<serde_json::Value> = self
            .inner
            .list_codecs()
            .iter()
            .map(|c| {
                serde_json::json!({
                    "codec_name": c.codec_name,
                    "can_decode": c.can_decode,
                    "can_encode": c.can_encode,
                    "pixel_formats": c.pixel_formats,
                })
            })
            .collect();
        serde_json::to_string(&codecs).unwrap_or_else(|_| "[]".to_string())
    }

    /// Check if any registered plugin provides the given codec.
    pub fn has_codec(&self, name: &str) -> bool {
        self.inner.has_codec(name)
    }

    /// Get the number of codecs available.
    pub fn codec_count(&self) -> u32 {
        self.inner.list_codecs().len() as u32
    }

    /// Get the number of registered plugins.
    pub fn plugin_count(&self) -> u32 {
        self.inner.plugin_count() as u32
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_version_nonzero() {
        assert!(wasm_plugin_api_version() > 0);
    }

    #[test]
    fn test_builtin_codecs_json() {
        let json = wasm_list_builtin_codecs();
        assert!(json.contains("av1"));
        assert!(json.contains("opus"));
        assert!(json.contains("vp9"));
        assert!(json.contains("flac"));
    }

    #[test]
    fn test_plugin_system_info() {
        let json = wasm_plugin_system_info();
        assert!(json.contains("api_version"));
        assert!(json.contains("static_only"));
    }

    #[test]
    fn test_registry_new_empty() {
        let reg = WasmPluginRegistry::new();
        assert_eq!(reg.plugin_count(), 0);
        assert_eq!(reg.codec_count(), 0);
    }

    #[test]
    fn test_registry_register_builtin() {
        let mut reg = WasmPluginRegistry::new();
        reg.register_builtin();
        assert_eq!(reg.plugin_count(), 1);
        assert!(reg.codec_count() > 0);
        assert!(reg.has_codec("av1"));
        assert!(reg.has_codec("opus"));
        assert!(!reg.has_codec("h264"));

        // Registering again should be idempotent
        reg.register_builtin();
        assert_eq!(reg.plugin_count(), 1);
    }

    #[test]
    fn test_registry_list_codecs_json() {
        let mut reg = WasmPluginRegistry::new();
        reg.register_builtin();
        let json = reg.list_codecs();
        assert!(json.contains("av1"));
        assert!(json.contains("can_decode"));
    }
}

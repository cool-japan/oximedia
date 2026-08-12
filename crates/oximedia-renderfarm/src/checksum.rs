// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Per-rendered-frame content checksums (SHA-256) for corruption detection.
//!
//! A render worker is expected to compute [`sha256_file`] over a frame's
//! output bytes right after writing it, and record the resulting digest on
//! the corresponding [`crate::pipeline::RenderResult::checksum`]. Post-render
//! verification ([`crate::pipeline::Pipeline`]'s frame-verification step)
//! re-reads the file from disk and recomputes the digest, comparing it
//! against the recorded one: a mismatch means the bytes on disk changed
//! after the worker wrote them (truncation, bit rot, a concurrent
//! overwrite, a bad transfer, ...) and is reported as real corruption rather
//! than silently passed through.
//!
//! When no checksum was recorded for a frame (`RenderResult::checksum ==
//! None`), there is no baseline to compare against, so corruption cannot be
//! detected for that frame -- verification falls back to the existence/
//! non-emptiness check only. This module never fabricates a checksum for a
//! frame that didn't have one recorded.

use crate::error::Result;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Computes the SHA-256 checksum of `data`, returned as a lowercase hex
/// string.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Reads `path` and computes its SHA-256 checksum as a lowercase hex string.
///
/// # Errors
///
/// Returns `Err` if the file cannot be read.
pub fn sha256_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path)?;
    Ok(sha256_hex(&data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia-renderfarm-checksum-{name}"))
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256("") -- the well-known empty-input digest (verified against
        // Python's hashlib in the development environment).
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_is_deterministic_and_sensitive_to_content() {
        let a = sha256_hex(b"hello world");
        let b = sha256_hex(b"hello world");
        let c = sha256_hex(b"hello worlD");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn sha256_file_reads_real_bytes() -> Result<()> {
        let path = tmp_path("sha256-file.bin");
        std::fs::write(&path, b"real frame bytes")?;

        let from_file = sha256_file(&path)?;
        let from_memory = sha256_hex(b"real frame bytes");
        assert_eq!(from_file, from_memory);

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn sha256_file_missing_file_is_err() {
        let path = tmp_path("sha256-file-missing.bin");
        std::fs::remove_file(&path).ok();
        assert!(sha256_file(&path).is_err());
    }
}

// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Real asset-source resolution: locating and materializing a job's missing
//! dependency assets before render.
//!
//! [`crate::pipeline::Pipeline::resolve_dependencies`] previously only
//! checked whether each declared dependency path already existed on disk.
//! This module adds the actually-missing half: given a list of configured
//! [`AssetSource`] locations (added via
//! [`crate::pipeline::Pipeline::add_asset_source`]), a missing dependency is
//! searched for and, if found, copied or downloaded into place at its
//! declared path so later checks (and the render itself) see it as present.
//!
//! Three kinds of source are real:
//!
//! - A local directory path.
//! - A `file://` URI (resolved to a local directory path).
//! - An `http://`/`https://` base URL, fetched with `reqwest` (already a
//!   dependency of this crate).
//!
//! A dependency whose *own* path names a scheme this crate has no transport
//! for (e.g. `s3://`, `ftp://`) fails honestly with [`Error::Dependency`]
//! naming the unsupported scheme, rather than being silently reported as
//! just "missing".

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A configured location [`resolve_missing_dependency`] searches, in order,
/// for a dependency asset that does not already exist at its declared path.
#[derive(Debug, Clone)]
pub enum AssetSource {
    /// A local directory (also used for `file://` URIs, resolved here).
    LocalDir(PathBuf),
    /// An HTTP(S) base URL; a missing dependency's file name is appended to
    /// it (`{base}/{file_name}`) to build the fetch URL.
    Http(String),
}

impl AssetSource {
    /// Parses a source specification string.
    ///
    /// - `file://<path>` becomes [`AssetSource::LocalDir`] with the URI's
    ///   path component.
    /// - `http://` / `https://` become [`AssetSource::Http`].
    /// - Anything else is treated as a bare local directory path.
    #[must_use]
    pub fn parse(spec: &str) -> Self {
        if let Some(path) = spec.strip_prefix("file://") {
            Self::LocalDir(PathBuf::from(path))
        } else if spec.starts_with("http://") || spec.starts_with("https://") {
            Self::Http(spec.trim_end_matches('/').to_string())
        } else {
            Self::LocalDir(PathBuf::from(spec))
        }
    }
}

/// Request timeout for asset downloads. Render assets can legitimately be
/// large (textures, caches); this is generous on purpose.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Attempts to resolve one missing dependency `path` by searching `sources`
/// in order.
///
/// On success (a source had the asset, or `path` itself names a fetchable
/// remote URL), the asset's bytes are copied/downloaded to `path` (creating
/// parent directories as needed) and `Ok(true)` is returned. `Ok(false)`
/// means no configured source had this asset (and `path` itself was not a
/// remote reference) -- the caller should still treat the dependency as
/// unresolved, exactly like the old existence-only check did when nothing
/// was configured. `Err` means an unsupported scheme was named, or a source
/// was reachable but the copy/download itself failed.
///
/// # Errors
///
/// Returns [`Error::Dependency`] when `path` names a URI scheme with no
/// available transport, and [`Error::Network`] / [`Error::Io`] when a fetch
/// or copy that was attempted failed partway through.
pub(crate) async fn resolve_missing_dependency(
    path: &Path,
    sources: &[AssetSource],
) -> Result<bool> {
    // The dependency path itself may directly name a remote asset (e.g. a
    // job submitted with `.dependency("https://cdn.example/tex.png")`).
    if let Some(path_str) = path.to_str() {
        if path_str.starts_with("http://") || path_str.starts_with("https://") {
            fetch_http_to(path_str, path).await?;
            return Ok(true);
        }
        if let Some(scheme_end) = path_str.find("://") {
            let scheme = &path_str[..scheme_end];
            if scheme != "file" {
                return Err(Error::Dependency(format!(
                    "no transport available for dependency '{path_str}': unsupported scheme \
                     '{scheme}://' (only local paths, file://, and http(s):// are supported)"
                )));
            }
        }
    }

    for source in sources {
        match source {
            AssetSource::LocalDir(dir) => {
                let candidate = join_asset_path(dir, path);
                if candidate.exists() {
                    copy_into_place(&candidate, path)?;
                    return Ok(true);
                }
            }
            AssetSource::Http(base) => {
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let url = format!("{base}/{name}");
                if fetch_http_to(&url, path).await.is_ok() {
                    return Ok(true);
                }
                // Not found at this source (or it errored) -- try the next
                // configured source rather than failing the whole
                // resolution; only an unsupported *scheme* on the
                // dependency itself (handled above) is a hard error.
            }
        }
    }

    Ok(false)
}

/// Joins a search-root directory with a dependency path.
///
/// `Path::join` replaces `self` entirely when the joined component is
/// absolute (documented Rust behavior), which would defeat searching an
/// absolute dependency path under `dir` -- so an absolute `dep` is joined by
/// its file name only, matching how [`AssetSource::Http`] must already
/// address dependencies (by file name, since a base URL plus an absolute
/// filesystem path is meaningless).
fn join_asset_path(dir: &Path, dep: &Path) -> PathBuf {
    if dep.is_absolute() {
        match dep.file_name() {
            Some(name) => dir.join(name),
            None => dir.to_path_buf(),
        }
    } else {
        dir.join(dep)
    }
}

/// Copies `source` to `dest`, creating `dest`'s parent directory first.
fn copy_into_place(source: &Path, dest: &Path) -> Result<()> {
    ensure_parent_dir(dest)?;
    std::fs::copy(source, dest)?;
    Ok(())
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// Downloads `url` over HTTP(S) and writes the response body to `dest`.
async fn fetch_http_to(url: &str, dest: &Path) -> Result<()> {
    // The workspace's default build is Pure Rust: `reqwest`/`rustls` carry
    // no compiled-in default crypto provider, so one must be installed
    // before the first TLS connection. Idempotent (`std::sync::Once`
    // guarded), safe to call on every fetch.
    oximedia_cloud::tls_provider::install_default_crypto_provider();

    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| Error::Network(format!("building HTTP client for asset fetch: {e}")))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Network(format!("fetching asset {url}: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::Network(format!(
            "fetching asset {url}: HTTP {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::Network(format!("reading asset body from {url}: {e}")))?;

    ensure_parent_dir(dest)?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia-renderfarm-asset-fetch-{name}"))
    }

    #[test]
    fn parse_recognizes_file_uri() {
        match AssetSource::parse("file:///mnt/shared/assets") {
            AssetSource::LocalDir(p) => assert_eq!(p, PathBuf::from("/mnt/shared/assets")),
            AssetSource::Http(_) => panic!("expected LocalDir"),
        }
    }

    #[test]
    fn parse_recognizes_http_and_https() {
        assert!(matches!(
            AssetSource::parse("http://cdn.example/assets"),
            AssetSource::Http(_)
        ));
        assert!(matches!(
            AssetSource::parse("https://cdn.example/assets"),
            AssetSource::Http(_)
        ));
    }

    #[test]
    fn parse_bare_path_is_local_dir() {
        match AssetSource::parse("/mnt/shared/assets") {
            AssetSource::LocalDir(p) => assert_eq!(p, PathBuf::from("/mnt/shared/assets")),
            AssetSource::Http(_) => panic!("expected LocalDir"),
        }
    }

    #[tokio::test]
    async fn resolve_missing_dependency_no_sources_returns_false() -> Result<()> {
        let missing = tmp_path("no-sources-missing.bin");
        std::fs::remove_file(&missing).ok();

        let resolved = resolve_missing_dependency(&missing, &[]).await?;
        assert!(!resolved);
        assert!(!missing.exists());

        Ok(())
    }

    #[tokio::test]
    async fn resolve_missing_dependency_finds_and_copies_from_local_source() -> Result<()> {
        let source_dir = tmp_path("local-source-dir");
        std::fs::create_dir_all(&source_dir)?;
        let dependency_name = "texture.bin";
        std::fs::write(source_dir.join(dependency_name), b"real asset bytes")?;

        // The dependency's declared path is *relative* -- matching
        // `join_asset_path`'s non-absolute branch (`dir.join(dep)`).
        let dependency_path = tmp_path("local-source-dest").join(dependency_name);
        std::fs::remove_file(&dependency_path).ok();
        assert!(!dependency_path.exists());

        let sources = vec![AssetSource::LocalDir(source_dir.clone())];
        // The dependency path we search for under `source_dir` must be the
        // same relative suffix as where it's expected to land; reuse just
        // the file name as the "declared" relative dependency path.
        let declared = PathBuf::from(dependency_name);
        let resolved = resolve_missing_dependency(&declared, &sources).await?;
        assert!(resolved);
        assert!(declared.exists());
        assert_eq!(std::fs::read(&declared)?, b"real asset bytes");

        std::fs::remove_file(&declared).ok();
        std::fs::remove_dir_all(&source_dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn resolve_missing_dependency_absolute_path_searches_by_file_name() -> Result<()> {
        // `Path::join` replaces `self` entirely when given an absolute
        // argument, which would silently defeat searching an absolute
        // dependency path under a source directory. This proves
        // `join_asset_path` avoids that trap by joining on file name alone
        // for absolute dependency paths.
        let source_dir = tmp_path("abs-source-dir");
        std::fs::create_dir_all(&source_dir)?;
        std::fs::write(source_dir.join("abs_asset.bin"), b"abs asset bytes")?;

        // `temp_dir()` is always absolute, so this dependency path is too.
        let absolute_dep =
            std::env::temp_dir().join("oximedia-renderfarm-asset-fetch-abs-dest/abs_asset.bin");
        std::fs::remove_file(&absolute_dep).ok();
        assert!(absolute_dep.is_absolute());

        let sources = vec![AssetSource::LocalDir(source_dir.clone())];
        let resolved = resolve_missing_dependency(&absolute_dep, &sources).await?;
        assert!(
            resolved,
            "absolute dependency path should resolve by file name"
        );
        assert!(absolute_dep.exists());
        assert_eq!(std::fs::read(&absolute_dep)?, b"abs asset bytes");

        std::fs::remove_file(&absolute_dep).ok();
        std::fs::remove_dir_all(&source_dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn resolve_missing_dependency_unsupported_scheme_is_honest_err() {
        let dep = PathBuf::from("s3://some-bucket/some-key.bin");
        let result = resolve_missing_dependency(&dep, &[]).await;
        assert!(result.is_err(), "unsupported scheme must fail honestly");
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("s3"),
            "error should name the scheme: {message}"
        );
    }

    #[tokio::test]
    async fn resolve_missing_dependency_falls_through_when_source_lacks_asset() -> Result<()> {
        let source_dir = tmp_path("empty-source-dir");
        std::fs::create_dir_all(&source_dir)?;

        let dependency_path = tmp_path("not-in-any-source.bin");
        std::fs::remove_file(&dependency_path).ok();

        let sources = vec![AssetSource::LocalDir(source_dir.clone())];
        let resolved = resolve_missing_dependency(&dependency_path, &sources).await?;
        assert!(!resolved);
        assert!(!dependency_path.exists());

        std::fs::remove_dir_all(&source_dir).ok();
        Ok(())
    }
}

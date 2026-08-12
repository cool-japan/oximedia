//! IMF package integrity verification.
//!
//! [`ImfIntegrityChecker::verify`] walks the given package directory path and
//! checks that every file referenced in a simple `ASSETMAP.xml`-style manifest
//! (if present) actually exists on disk.  When no manifest is found it reports
//! the absence as a warning rather than a hard error.
//!
//! The checker performs a **file-existence** check only — it does not compute
//! checksums (see `essence_hash.rs` for that).  It is intended as a fast
//! first-pass sanity check during ingest or QC.
//!
//! # Example
//!
//! ```no_run
//! use oximedia_imf::integrity_check::ImfIntegrityChecker;
//!
//! let issues = ImfIntegrityChecker::verify("/path/to/imp");
//! if issues.is_empty() {
//!     println!("Package integrity OK");
//! } else {
//!     for issue in &issues {
//!         eprintln!("{issue}");
//!     }
//! }
//! ```

#![allow(dead_code)]

use std::path::Path;

/// Verifies the integrity of an IMF package at the given path.
pub struct ImfIntegrityChecker;

impl ImfIntegrityChecker {
    /// Verify that the IMF package at `pkg_path` exists and that any
    /// referenced files are present.
    ///
    /// The following checks are performed:
    ///
    /// 1. The package directory itself must exist.
    /// 2. An `ASSETMAP.xml` file should be present (soft warning if absent).
    /// 3. A `PKL*.xml` or `PKL.xml` file should be present (soft warning).
    /// 4. At least one `CPL*.xml` or `CPL.xml` file should be present.
    /// 5. Any `.mxf` files referenced by name (heuristic: files listed in
    ///    `<Path>` tags of `ASSETMAP.xml`) must exist; missing files are errors.
    ///
    /// Returns a `Vec<String>` of human-readable issue descriptions.  An empty
    /// vector means no issues were found.
    #[must_use]
    pub fn verify(pkg_path: &str) -> Vec<String> {
        let mut issues = Vec::new();
        let base = Path::new(pkg_path);

        // 1. Package directory must exist
        if !base.exists() {
            issues.push(format!("Package directory not found: '{pkg_path}'"));
            return issues; // cannot proceed further
        }
        if !base.is_dir() {
            issues.push(format!("Package path is not a directory: '{pkg_path}'"));
            return issues;
        }

        // 2. ASSETMAP.xml
        let assetmap = base.join("ASSETMAP.xml");
        if !assetmap.exists() {
            issues.push(format!("WARNING: ASSETMAP.xml not found in '{pkg_path}'"));
        } else {
            // Parse referenced file paths from ASSETMAP.xml
            let referenced = extract_paths_from_xml(&assetmap);
            for rel_path in referenced {
                let full = base.join(&rel_path);
                if !full.exists() {
                    issues.push(format!(
                        "Referenced file missing: '{}' (declared in ASSETMAP.xml)",
                        rel_path
                    ));
                }
            }
        }

        // 3. PKL*.xml
        let pkl_present = dir_contains_match(base, |name| {
            let lower = name.to_lowercase();
            lower.starts_with("pkl") && lower.ends_with(".xml")
        });
        if !pkl_present {
            issues.push(format!("WARNING: No PKL*.xml found in '{pkg_path}'"));
        }

        // 4. CPL*.xml
        let cpl_present = dir_contains_match(base, |name| {
            let lower = name.to_lowercase();
            lower.starts_with("cpl") && lower.ends_with(".xml")
        });
        if !cpl_present {
            issues.push(format!("WARNING: No CPL*.xml found in '{pkg_path}'"));
        }

        issues
    }

    /// Verify a set of explicitly provided file paths.
    ///
    /// Each path in `file_paths` is checked for existence.  Missing files are
    /// returned as error strings.  This is useful when the caller already has
    /// the asset list from a parsed PKL.
    #[must_use]
    pub fn verify_files(pkg_path: &str, file_paths: &[&str]) -> Vec<String> {
        let mut issues = Vec::new();
        let base = Path::new(pkg_path);

        if !base.exists() {
            issues.push(format!("Package directory not found: '{pkg_path}'"));
            return issues;
        }

        for &rel_path in file_paths {
            let full = base.join(rel_path);
            if !full.exists() {
                issues.push(format!("File not found: '{}'", full.display()));
            }
        }

        issues
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract file paths from `<Path>...</Path>` tags in an XML file.
///
/// This is a minimal line-by-line scanner — not a full XML parser — which is
/// sufficient for ASSETMAP.xml files whose paths appear on dedicated lines.
fn extract_paths_from_xml(xml_path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(xml_path) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(inner) = extract_tag_value(trimmed, "Path") {
            if !inner.is_empty() {
                paths.push(inner.to_string());
            }
        }
    }
    paths
}

/// Extract the text content of a simple `<Tag>value</Tag>` element from a
/// trimmed line.  Returns `None` if the pattern does not match.
fn extract_tag_value<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = line.find(&open)?;
    let end = line.find(&close)?;
    let inner_start = start + open.len();
    if inner_start <= end {
        Some(&line[inner_start..end])
    } else {
        None
    }
}

/// Return `true` if the directory at `dir` contains at least one file whose
/// name satisfies `predicate`.
fn dir_contains_match<F>(dir: &Path, predicate: F) -> bool
where
    F: Fn(&str) -> bool,
{
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .any(|name| predicate(&name))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── verify — missing directory ─────────────────────────────────────────

    #[test]
    fn test_verify_nonexistent_directory() {
        let issues = ImfIntegrityChecker::verify("/nonexistent/path/pkg");
        assert!(!issues.is_empty());
        assert!(issues[0].contains("not found"));
    }

    #[test]
    fn test_verify_file_as_path_is_rejected() {
        // Use a known file that exists on any Unix/macOS system
        let issues = ImfIntegrityChecker::verify("/etc/hosts");
        // Either "not found" or "not a directory"
        assert!(!issues.is_empty());
    }

    // ── verify — real temp directory ──────────────────────────────────────

    /// Create a private temporary package directory for a single test.
    ///
    /// The name combines the process id, a per-process counter and a
    /// nanosecond timestamp.  A timestamp alone is not enough: two tests that
    /// read the clock within the same nanosecond receive the same directory,
    /// and the first one to finish then `remove_dir_all`s the other's fixture
    /// mid-run.  The pid and counter make the name unique both across the
    /// per-test processes `cargo nextest` spawns and across threads inside one
    /// process; the timestamp additionally keeps a recycled pid from reusing a
    /// directory left behind by a panicking earlier run.
    fn temp_pkg() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "oximedia_imf_test_{}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_verify_empty_package_warns_missing_files() {
        let dir = temp_pkg();
        let path = dir.to_str().expect("valid utf8 path").to_string();
        let issues = ImfIntegrityChecker::verify(&path);
        // Should warn about missing ASSETMAP.xml, PKL, CPL
        assert!(!issues.is_empty(), "Empty package should have warnings");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_package_with_xml_files_no_warnings_for_those() {
        let dir = temp_pkg();
        // Create stub files
        std::fs::write(
            dir.join("ASSETMAP.xml"),
            "<AssetMap><AssetList></AssetList></AssetMap>",
        )
        .expect("write ASSETMAP");
        std::fs::write(dir.join("PKL_001.xml"), "<PackingList/>").expect("write PKL");
        std::fs::write(dir.join("CPL_001.xml"), "<CompositionPlaylist/>").expect("write CPL");

        let path = dir.to_str().expect("valid utf8 path").to_string();
        let issues = ImfIntegrityChecker::verify(&path);
        // No warnings about missing ASSETMAP/PKL/CPL
        let has_assetmap_warn = issues.iter().any(|i| i.contains("ASSETMAP.xml"));
        let has_pkl_warn = issues.iter().any(|i| i.contains("PKL"));
        let has_cpl_warn = issues.iter().any(|i| i.contains("CPL"));
        assert!(!has_assetmap_warn);
        assert!(!has_pkl_warn);
        assert!(!has_cpl_warn);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_assetmap_references_missing_file() {
        let dir = temp_pkg();
        let assetmap_content = r#"<AssetMap>
  <AssetList>
    <Asset>
      <ChunkList>
        <Chunk>
          <Path>missing_video.mxf</Path>
        </Chunk>
      </ChunkList>
    </Asset>
  </AssetList>
</AssetMap>"#;
        std::fs::write(dir.join("ASSETMAP.xml"), assetmap_content).expect("write ASSETMAP");
        std::fs::write(dir.join("PKL_001.xml"), "").expect("write PKL");
        std::fs::write(dir.join("CPL_001.xml"), "").expect("write CPL");

        let path = dir.to_str().expect("valid utf8 path").to_string();
        let issues = ImfIntegrityChecker::verify(&path);
        let has_missing = issues.iter().any(|i| i.contains("missing_video.mxf"));
        assert!(has_missing, "Should flag missing referenced file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_assetmap_references_present_file_no_error() {
        let dir = temp_pkg();
        let assetmap_content = r#"<AssetMap>
  <AssetList>
    <Asset>
      <ChunkList>
        <Chunk>
          <Path>video.mxf</Path>
        </Chunk>
      </ChunkList>
    </Asset>
  </AssetList>
</AssetMap>"#;
        std::fs::write(dir.join("ASSETMAP.xml"), assetmap_content).expect("write ASSETMAP");
        std::fs::write(dir.join("video.mxf"), b"MXF" as &[u8]).expect("write MXF");
        std::fs::write(dir.join("PKL_001.xml"), "").expect("write PKL");
        std::fs::write(dir.join("CPL_001.xml"), "").expect("write CPL");

        let path = dir.to_str().expect("valid utf8 path").to_string();
        let issues = ImfIntegrityChecker::verify(&path);
        let has_video_err = issues.iter().any(|i| i.contains("video.mxf"));
        assert!(
            !has_video_err,
            "Present file should not be flagged: {:?}",
            issues
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── verify_files ──────────────────────────────────────────────────────────

    #[test]
    fn test_verify_files_all_present() {
        let dir = temp_pkg();
        std::fs::write(dir.join("a.mxf"), b"").expect("write");
        std::fs::write(dir.join("b.mxf"), b"").expect("write");
        let path = dir.to_str().expect("valid utf8 path").to_string();
        let issues = ImfIntegrityChecker::verify_files(&path, &["a.mxf", "b.mxf"]);
        assert!(issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_files_missing_reported() {
        let dir = temp_pkg();
        let path = dir.to_str().expect("valid utf8 path").to_string();
        let issues = ImfIntegrityChecker::verify_files(&path, &["missing.mxf"]);
        assert!(!issues.is_empty());
        assert!(issues[0].contains("missing.mxf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── extract_tag_value ─────────────────────────────────────────────────────

    #[test]
    fn test_extract_tag_value_simple() {
        let line = "          <Path>video.mxf</Path>";
        assert_eq!(extract_tag_value(line.trim(), "Path"), Some("video.mxf"));
    }

    #[test]
    fn test_extract_tag_value_no_match() {
        let line = "<Size>1024</Size>";
        assert_eq!(extract_tag_value(line, "Path"), None);
    }

    #[test]
    fn test_extract_tag_value_empty_tag() {
        let line = "<Path></Path>";
        assert_eq!(extract_tag_value(line, "Path"), Some(""));
    }
}

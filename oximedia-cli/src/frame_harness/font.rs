//! Font loading for text-rendering harness operations.
//!
//! # No font ships with OxiMedia
//!
//! Typefaces carry their own licences, so none is vendored into this tree and
//! nothing here probes system font directories: a burn-in whose glyphs came
//! from whatever font happened to sit in `/System/Library/Fonts` is not
//! reproducible, and silently picking one would make the output depend on the
//! machine rather than on the command line. Callers must pass `--font <PATH>`;
//! without it, [`load_font`] returns an honest error naming the flag.

use anyhow::{Context, Result};
use std::path::Path;

/// Largest font file the CLI will load, in bytes.
///
/// Comfortably above any real TrueType/OpenType face (CJK families with full
/// coverage run ~20-30 MB); the bound exists so a mistyped `--font` pointing
/// at a video file fails fast instead of buffering gigabytes.
const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

/// Read the font file the user asked for.
///
/// Returns the raw font bytes, ready for `fontdue` (via
/// `oximedia_graph::filters::video::TimecodeFilter::new`).
///
/// # Errors
///
/// Returns an error if `path` is `None` (naming the `--font` flag), if the
/// file cannot be read, if it is empty or implausibly large, or if it does not
/// start with a recognised TrueType/OpenType/WOFF signature.
pub fn load_font(path: Option<&Path>) -> Result<Vec<u8>> {
    let Some(path) = path else {
        anyhow::bail!(
            "no font was supplied: text burn-in needs a TrueType/OpenType font, and OxiMedia \
             ships none (fonts carry their own licences) nor guesses at a system font. Pass \
             `--font <PATH>` pointing at a .ttf/.otf file, \
             e.g. `--font /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`."
        );
    };

    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat font file: {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!(
            "--font must point at a font file, but '{}' is not a regular file",
            path.display()
        );
    }
    if metadata.len() == 0 {
        anyhow::bail!("font file '{}' is empty", path.display());
    }
    if metadata.len() > MAX_FONT_BYTES {
        anyhow::bail!(
            "font file '{}' is {} bytes, above the {MAX_FONT_BYTES}-byte limit; \
             is --font pointing at the right file?",
            path.display(),
            metadata.len()
        );
    }

    let data = std::fs::read(path)
        .with_context(|| format!("Failed to read font file: {}", path.display()))?;
    check_signature(path, &data)?;
    Ok(data)
}

/// Reject files that are clearly not a font before handing them to `fontdue`.
fn check_signature(path: &Path, data: &[u8]) -> Result<()> {
    let head = data.get(..4).unwrap_or_default();
    let known = matches!(
        head,
        // TrueType outlines (0x00010000), 'true', 'ttcf' collection,
        // 'OTTO' CFF outlines, 'wOFF' / 'wOF2' web fonts.
        [0x00, 0x01, 0x00, 0x00]
            | [b't', b'r', b'u', b'e']
            | [b't', b't', b'c', b'f']
            | [b'O', b'T', b'T', b'O']
            | [b'w', b'O', b'F', b'F']
            | [b'w', b'O', b'F', b'2']
    );
    if !known {
        anyhow::bail!(
            "'{}' does not look like a TrueType/OpenType font (unrecognised signature {:02X?}). \
             Pass --font with a .ttf or .otf file.",
            path.display(),
            head
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch path for a fixture; PID-scoped because this module is compiled
    /// into both the binary and the lib target, which nextest runs as two
    /// concurrent processes.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_font_harness_{}_{name}",
            std::process::id()
        ))
    }

    #[test]
    fn missing_font_names_the_flag() {
        let err = load_font(None).expect_err("no font must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("--font"),
            "error must name the flag, got: {msg}"
        );
        assert!(
            msg.contains("ships none"),
            "error must explain why, got: {msg}"
        );
    }

    #[test]
    fn nonexistent_font_errors() {
        let path = temp_path("definitely_missing.ttf");
        let _ = std::fs::remove_file(&path);
        let err = load_font(Some(&path)).expect_err("missing file must error");
        assert!(format!("{err}").contains("font file"), "got: {err}");
    }

    #[test]
    fn non_font_signature_is_rejected() {
        let path = temp_path("not_a_font.ttf");
        std::fs::write(&path, b"just some text, not a font at all").expect("write fixture");
        let err = load_font(Some(&path)).expect_err("non-font must error");
        assert!(format!("{err}").contains("TrueType/OpenType"), "got: {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_font_is_rejected() {
        let path = temp_path("empty.ttf");
        std::fs::write(&path, b"").expect("write fixture");
        let err = load_font(Some(&path)).expect_err("empty file must error");
        assert!(format!("{err}").contains("empty"), "got: {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truetype_signature_is_accepted() {
        let path = temp_path("stub.ttf");
        let mut data = vec![0x00, 0x01, 0x00, 0x00];
        data.extend_from_slice(&[0u8; 64]);
        std::fs::write(&path, &data).expect("write fixture");
        let loaded = load_font(Some(&path)).expect("signature check must pass");
        assert_eq!(loaded.len(), data.len());
        let _ = std::fs::remove_file(&path);
    }
}

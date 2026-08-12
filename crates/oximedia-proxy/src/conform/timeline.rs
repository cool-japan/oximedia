//! Timeline conforming utilities.

use crate::proxy_index::ProxyIndex;
use crate::{ProxyError, Result};
use oximedia_edl::event::TrackType;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Timeline conformer for various timeline formats.
pub struct TimelineConformer {
    /// Original media directory.
    original_dir: Option<PathBuf>,
    /// Proxy-to-original index consulted by [`Self::conform_timeline`] to
    /// resolve timeline clip references to their original media.
    proxy_index: Option<ProxyIndex>,
}

impl TimelineConformer {
    /// Create a new timeline conformer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            original_dir: None,
            proxy_index: None,
        }
    }

    /// Set the original media directory.
    #[must_use]
    pub fn with_original_dir(mut self, dir: PathBuf) -> Self {
        self.original_dir = Some(dir);
        self
    }

    /// Attach a proxy index used to resolve timeline clip references to
    /// their original media during [`Self::conform_timeline`].
    #[must_use]
    pub fn with_proxy_index(mut self, index: ProxyIndex) -> Self {
        self.proxy_index = Some(index);
        self
    }

    /// Conform a timeline file to use original media.
    ///
    /// Extracts media references via [`Self::extract_media_references`] and
    /// resolves each one:
    ///
    /// 1. If a [`ProxyIndex`] was attached via [`Self::with_proxy_index`],
    ///    the reference is matched against every indexed entry's proxy
    ///    path (see `resolve_via_proxy_index` for the exact-vs-file-name
    ///    matching rule).
    /// 2. Otherwise, if an original-media directory was attached via
    ///    [`Self::with_original_dir`], a same-named file under that
    ///    directory is used if it exists on disk.
    ///
    /// References resolved through neither mechanism are collected in
    /// [`TimelineConformResult::unresolved_clips`] and counted in
    /// `clips_failed` -- never silently folded into `clips_relinked`.
    ///
    /// This does **not** write a conformed timeline to `output_path`: no
    /// per-format serializer is implemented (see
    /// [`Self::extract_media_references`] for which formats can even be
    /// read). `output_path` is recorded on the result for caller
    /// bookkeeping only.
    ///
    /// # Errors
    ///
    /// Returns an error if the input timeline cannot be read or is in an
    /// unsupported format (see [`Self::extract_media_references`]).
    pub fn conform_timeline(
        &self,
        timeline_path: &std::path::Path,
        output_path: &std::path::Path,
    ) -> Result<TimelineConformResult> {
        let references = self.extract_media_references(timeline_path)?;
        let clips_total = references.len();

        let proxy_lookup = self.proxy_index.as_ref().map(build_proxy_lookup);

        let mut clips_relinked = 0;
        let mut unresolved_clips = Vec::new();

        for reference in references {
            let resolved = proxy_lookup
                .as_ref()
                .and_then(|lookup| resolve_via_proxy_index(lookup, &reference.path))
                .or_else(|| {
                    resolve_via_original_dir(self.original_dir.as_deref(), &reference.path)
                });

            if resolved.is_some() {
                clips_relinked += 1;
            } else {
                unresolved_clips.push(reference);
            }
        }

        Ok(TimelineConformResult {
            input_path: timeline_path.to_path_buf(),
            output_path: output_path.to_path_buf(),
            clips_relinked,
            clips_failed: unresolved_clips.len(),
            clips_total,
            unresolved_clips,
        })
    }

    /// Extract media references from a timeline.
    ///
    /// Only [`TimelineFormat::Edl`] is parsed for real right now, via
    /// [`oximedia_edl::parse_edl`] (already a dependency of this crate).
    /// Every other format -- including [`TimelineFormat::Unknown`] --
    /// returns [`ProxyError::Unsupported`] naming the format, rather than
    /// silently reporting zero references: `EdlConformer`/`XmlConformer`
    /// (the modules that own EDL-specific and XML-specific conform flows)
    /// do not have real parsers wired in yet either, so claiming generic
    /// FCPXML/Premiere/AAF/OTIO support here would overstate what this
    /// crate can actually read.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, is not valid UTF-8,
    /// fails to parse as an EDL, or is in a format that is not yet
    /// supported.
    pub fn extract_media_references(
        &self,
        timeline_path: &std::path::Path,
    ) -> Result<Vec<MediaReference>> {
        let format = TimelineFormatDetector::detect(timeline_path)?;

        match format {
            TimelineFormat::Edl => {
                let text = std::fs::read_to_string(timeline_path)?;
                let edl = oximedia_edl::parse_edl(&text)
                    .map_err(|e| ProxyError::ConformError(format!("EDL parse error: {e}")))?;
                Ok(edl_to_media_references(&edl))
            }
            other => Err(ProxyError::Unsupported(format!(
                "timeline parsing for {} is not implemented; only EDL is currently supported \
                 (via oximedia_edl::parse_edl)",
                other.name()
            ))),
        }
    }

    /// Validate timeline media references.
    pub fn validate_timeline(&self, timeline_path: &std::path::Path) -> Result<TimelineValidation> {
        let references = self.extract_media_references(timeline_path)?;
        let total = references.len();
        let mut found = 0;
        let mut missing = Vec::new();

        for reference in &references {
            if reference.path.exists() {
                found += 1;
            } else {
                missing.push(reference.clone());
            }
        }

        Ok(TimelineValidation {
            total_references: total,
            found_references: found,
            missing_references: missing,
        })
    }
}

impl Default for TimelineConformer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Private relink helpers ──────────────────────────────────────────────────

/// Map a parsed [`oximedia_edl::Edl`]'s events into generic
/// [`MediaReference`]s.
///
/// The source file name for each event is taken from `clip_name`
/// (populated by [`oximedia_edl::parse_edl`] from `* FROM CLIP NAME:` /
/// `* SOURCE FILE:` comment lines) when present, falling back to the reel
/// identifier (e.g. `"A001"`) since CMX 3600 EDLs do not always carry an
/// explicit source filename. CMX 3600 events likewise carry no explicit
/// multi-track index in this model, so `track` is always `0`.
fn edl_to_media_references(edl: &oximedia_edl::Edl) -> Vec<MediaReference> {
    edl.events
        .iter()
        .map(|event| {
            let source_name = event
                .clip_name
                .clone()
                .unwrap_or_else(|| event.reel.clone());
            let (is_video, is_audio) = track_kind_flags(&event.track);
            MediaReference {
                path: PathBuf::from(&source_name),
                clip_name: source_name,
                in_point: event.source_in.to_frames(),
                out_point: event.source_out.to_frames(),
                track: 0,
                is_video,
                is_audio,
            }
        })
        .collect()
}

/// Classify a CMX 3600 [`TrackType`] into `(is_video, is_audio)` flags.
fn track_kind_flags(track: &TrackType) -> (bool, bool) {
    match track {
        TrackType::Video => (true, false),
        TrackType::Audio(_) | TrackType::AudioPair | TrackType::AudioMulti(_) => (false, true),
        TrackType::AudioWithVideo
        | TrackType::AudioPairWithVideo
        | TrackType::VideoWithAudioMulti(_) => (true, true),
    }
}

/// Build a `proxy_path -> original_path` lookup from every entry in a
/// [`ProxyIndex`] (both directory-qualified and bare-file-name lookups are
/// tried by [`resolve_via_proxy_index`], not by this function).
fn build_proxy_lookup(index: &ProxyIndex) -> HashMap<&str, &str> {
    index
        .all_entries()
        .map(|entry| (entry.proxy_path.as_str(), entry.original_path.as_str()))
        .collect()
}

/// Resolve `reference_path` against a `proxy_path -> original_path`
/// lookup built by [`build_proxy_lookup`].
///
/// Timeline clip references are frequently bare file names (e.g. an EDL
/// `clip_name` of `"shot001.mov"`) even though a `ProxyIndex` stores full
/// paths, so a reference with no directory component is matched by file
/// name against every indexed proxy path; a reference that does carry a
/// directory component must match an indexed proxy path exactly (a bare
/// file name matching every path ending in that name would be far too
/// permissive once a directory is actually specified).
fn resolve_via_proxy_index(lookup: &HashMap<&str, &str>, reference_path: &Path) -> Option<PathBuf> {
    let has_directory = reference_path
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());

    if has_directory {
        let ref_str = reference_path.to_string_lossy();
        return lookup
            .get(ref_str.as_ref())
            .map(|original| PathBuf::from(*original));
    }

    let file_name = reference_path.file_name()?;
    lookup
        .iter()
        .find(|(proxy_path, _)| Path::new(proxy_path).file_name() == Some(file_name))
        .map(|(_, original)| PathBuf::from(*original))
}

/// Resolve `reference_path` to a same-named file under `original_dir`, if
/// one exists on disk.
fn resolve_via_original_dir(original_dir: Option<&Path>, reference_path: &Path) -> Option<PathBuf> {
    let dir = original_dir?;
    let file_name = reference_path.file_name()?;
    let candidate = dir.join(file_name);
    candidate.exists().then_some(candidate)
}

/// Timeline conforming result.
#[derive(Debug, Clone)]
pub struct TimelineConformResult {
    /// Input timeline path.
    pub input_path: PathBuf,

    /// Output timeline path. [`TimelineConformer::conform_timeline`] does
    /// not write a file here (yet) -- this is recorded for caller
    /// bookkeeping only.
    pub output_path: PathBuf,

    /// Number of clips successfully relinked.
    pub clips_relinked: usize,

    /// Number of clips that failed to relink.
    pub clips_failed: usize,

    /// Total number of clips.
    pub clips_total: usize,

    /// The references that could not be resolved to an original file,
    /// preserved for diagnostics -- mirrors
    /// [`TimelineValidation::missing_references`]. Always has
    /// `clips_failed` entries.
    pub unresolved_clips: Vec<MediaReference>,
}

impl TimelineConformResult {
    /// Check if conforming was successful.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.clips_failed == 0
    }

    /// Get success percentage.
    #[must_use]
    pub fn success_percentage(&self) -> f64 {
        if self.clips_total == 0 {
            100.0
        } else {
            (self.clips_relinked as f64 / self.clips_total as f64) * 100.0
        }
    }
}

/// Media reference in a timeline.
#[derive(Debug, Clone)]
pub struct MediaReference {
    /// Media file path.
    pub path: PathBuf,

    /// Clip name.
    pub clip_name: String,

    /// In point in frames.
    pub in_point: u64,

    /// Out point in frames.
    pub out_point: u64,

    /// Track index.
    pub track: usize,

    /// Is video reference.
    pub is_video: bool,

    /// Is audio reference.
    pub is_audio: bool,
}

/// Timeline validation result.
#[derive(Debug, Clone)]
pub struct TimelineValidation {
    /// Total media references.
    pub total_references: usize,

    /// Found references.
    pub found_references: usize,

    /// Missing references.
    pub missing_references: Vec<MediaReference>,
}

impl TimelineValidation {
    /// Check if all references are found.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.missing_references.is_empty()
    }

    /// Get validation percentage.
    #[must_use]
    pub fn validation_percentage(&self) -> f64 {
        if self.total_references == 0 {
            100.0
        } else {
            (self.found_references as f64 / self.total_references as f64) * 100.0
        }
    }
}

/// Timeline format detector.
pub struct TimelineFormatDetector;

impl TimelineFormatDetector {
    /// Detect timeline format from file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn detect(path: &std::path::Path) -> Result<TimelineFormat> {
        // Check file extension
        if let Some(ext) = path.extension() {
            match ext.to_str() {
                Some("fcpxml") => return Ok(TimelineFormat::FinalCutProXml),
                Some("xml") => {
                    // Could be Premiere or FCP
                    // Would need to parse XML to determine
                    return Ok(TimelineFormat::PremiereXml);
                }
                Some("aaf") => return Ok(TimelineFormat::Aaf),
                Some("edl") => return Ok(TimelineFormat::Edl),
                Some("otio") => return Ok(TimelineFormat::Otio),
                _ => {}
            }
        }

        Ok(TimelineFormat::Unknown)
    }

    /// Check if format is supported.
    #[must_use]
    pub fn is_supported(format: &TimelineFormat) -> bool {
        !matches!(format, TimelineFormat::Unknown)
    }
}

/// Timeline format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineFormat {
    /// Final Cut Pro XML.
    FinalCutProXml,

    /// Premiere Pro XML.
    PremiereXml,

    /// AAF (Advanced Authoring Format).
    Aaf,

    /// EDL (Edit Decision List).
    Edl,

    /// OpenTimelineIO.
    Otio,

    /// Unknown format.
    Unknown,
}

impl TimelineFormat {
    /// Get format name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::FinalCutProXml => "Final Cut Pro XML",
            Self::PremiereXml => "Premiere Pro XML",
            Self::Aaf => "AAF",
            Self::Edl => "EDL",
            Self::Otio => "OpenTimelineIO",
            Self::Unknown => "Unknown",
        }
    }

    /// Get file extension.
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::FinalCutProXml => "fcpxml",
            Self::PremiereXml => "xml",
            Self::Aaf => "aaf",
            Self::Edl => "edl",
            Self::Otio => "otio",
            Self::Unknown => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real EDL fixture, modeled directly on `oximedia_edl::parser`'s own
    /// `test_parse_clip_name_comment` test so the format is proven to
    /// parse. Two events: `shot001.mov` will be registered in a
    /// `ProxyIndex` (resolvable), `shot002.mov` will not (unresolved).
    const FIXTURE_EDL: &str =
        "001  AX       V     C        01:00:00:00 01:00:05:00 01:00:00:00 01:00:05:00\n\
* FROM CLIP NAME: shot001.mov\n\
002  AX       V     C        01:00:05:00 01:00:10:00 01:00:05:00 01:00:10:00\n\
* FROM CLIP NAME: shot002.mov\n";

    #[test]
    fn test_timeline_conformer_edl_real_relink() {
        use crate::proxy_index::ProxyEntry;

        let timeline_path =
            std::env::temp_dir().join("oximedia_proxy_test_conform_timeline_relink.edl");
        std::fs::write(&timeline_path, FIXTURE_EDL).expect("write EDL fixture");

        let mut index = ProxyIndex::new();
        index.insert(ProxyEntry::new(
            "/originals/shot001_original.mov",
            "shot001.mov",
            640,
            360,
            500,
        ));
        let conformer = TimelineConformer::new().with_proxy_index(index);
        let output_path = std::env::temp_dir().join("oximedia_proxy_test_conform_timeline_out.edl");

        let result = conformer
            .conform_timeline(&timeline_path, &output_path)
            .expect("conform_timeline should succeed for a real, parseable EDL");

        assert_eq!(result.clips_total, 2);
        assert_eq!(result.clips_relinked, 1, "shot001.mov is in the index");
        assert_eq!(result.clips_failed, 1, "shot002.mov is not in the index");
        assert_eq!(result.unresolved_clips.len(), 1);
        assert_eq!(result.unresolved_clips[0].clip_name, "shot002.mov");
        assert!(!result.is_success());

        let _ = std::fs::remove_file(&timeline_path);
    }

    #[test]
    fn test_timeline_conformer_extract_media_references_edl() {
        let timeline_path = std::env::temp_dir().join("oximedia_proxy_test_extract_refs.edl");
        std::fs::write(&timeline_path, FIXTURE_EDL).expect("write EDL fixture");

        let conformer = TimelineConformer::new();
        let references = conformer
            .extract_media_references(&timeline_path)
            .expect("EDL extraction should succeed");

        assert_eq!(references.len(), 2);
        assert_eq!(references[0].clip_name, "shot001.mov");
        assert_eq!(references[0].path, PathBuf::from("shot001.mov"));
        assert!(references[0].is_video);
        assert!(!references[0].is_audio);
        // "01:00:00:00" is 1 real hour of absolute timecode (a common EDL
        // convention -- starting source reels at 1h avoids ambiguity with
        // the "00:00:00:00" sentinel), i.e. 1*3600*30 = 108_000 frames at
        // the parser's default nominal 30fps (EdlFrameRate::Fps2997NDF).
        assert_eq!(references[0].in_point, 108_000);
        // "01:00:05:00" is 5 real seconds later: 5 * 30 = 150 frames.
        assert_eq!(references[0].out_point, references[0].in_point + 5 * 30);
        assert_eq!(references[1].clip_name, "shot002.mov");

        let _ = std::fs::remove_file(&timeline_path);
    }

    #[test]
    fn test_timeline_conformer_unsupported_format_is_honest_err() {
        // `.xml` is detected as `TimelineFormat::PremiereXml`, which has no
        // real parser wired in -- this must be a named Err, never a
        // silent Ok([]) / Ok(clips_relinked: 0).
        let conformer = TimelineConformer::new();
        let result = conformer.conform_timeline(
            std::path::Path::new("timeline.xml"),
            std::path::Path::new("output.xml"),
        );
        assert!(matches!(result, Err(ProxyError::Unsupported(_))));

        let refs_result = conformer.extract_media_references(std::path::Path::new("timeline.xml"));
        assert!(matches!(refs_result, Err(ProxyError::Unsupported(_))));
    }

    #[test]
    fn test_timeline_conform_result() {
        let result = TimelineConformResult {
            input_path: PathBuf::from("input.xml"),
            output_path: PathBuf::from("output.xml"),
            clips_relinked: 8,
            clips_failed: 2,
            clips_total: 10,
            unresolved_clips: Vec::new(),
        };

        assert!(!result.is_success());
        assert_eq!(result.success_percentage(), 80.0);
    }

    #[test]
    fn test_format_detector() {
        let format = TimelineFormatDetector::detect(std::path::Path::new("test.fcpxml"));
        assert!(format.is_ok());
        assert_eq!(
            format.expect("should succeed in test"),
            TimelineFormat::FinalCutProXml
        );

        let format = TimelineFormatDetector::detect(std::path::Path::new("test.edl"));
        assert!(format.is_ok());
        assert_eq!(format.expect("should succeed in test"), TimelineFormat::Edl);
    }

    #[test]
    fn test_format_name() {
        assert_eq!(TimelineFormat::FinalCutProXml.name(), "Final Cut Pro XML");
        assert_eq!(TimelineFormat::Aaf.name(), "AAF");
    }

    #[test]
    fn test_is_supported() {
        assert!(TimelineFormatDetector::is_supported(
            &TimelineFormat::FinalCutProXml
        ));
        assert!(!TimelineFormatDetector::is_supported(
            &TimelineFormat::Unknown
        ));
    }
}

//! XML importer for Final Cut Pro, Premiere, and Resolve timelines.

use crate::error::{ConformError, ConformResult};
use crate::importers::fcpxml::FcpxmlParser;
use crate::importers::TimelineImporter;
use crate::types::{ClipReference, FrameRate, Timecode, TrackType};
use std::path::Path;

/// XML importer for various NLE formats.
pub struct XmlImporter {
    /// XML format type.
    format: XmlFormat,
}

/// XML format type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlFormat {
    /// Final Cut Pro XML.
    FcpXml,
    /// Adobe Premiere Pro XML.
    PremiereXml,
    /// `DaVinci` Resolve XML.
    ResolveXml,
    /// Auto-detect format.
    Auto,
}

impl XmlImporter {
    /// Create a new XML importer with the specified format.
    #[must_use]
    pub const fn new(format: XmlFormat) -> Self {
        Self { format }
    }

    /// Detect XML format from content by sniffing well-known root-element
    /// signatures.
    ///
    /// Returns [`XmlFormat::FcpXml`]/[`XmlFormat::PremiereXml`] when a
    /// confident signature is found. Returns [`XmlFormat::Auto`] — rather
    /// than guessing — when no known signature is found, so
    /// [`XmlImporter::import`] surfaces an honest "could not auto-detect"
    /// error instead of silently routing unrecognized content through the
    /// wrong parser. An earlier revision always returned `XmlFormat::FcpXml`
    /// regardless of content, which made the `XmlFormat::Auto => Err(..)`
    /// arm in `import()` permanently unreachable dead code.
    fn detect_format(content: &str) -> XmlFormat {
        if content.contains("<fcpxml") {
            XmlFormat::FcpXml
        } else if content.contains("<xmeml") {
            // Adobe Premiere Pro (and legacy Final Cut Pro 7) both export an
            // <xmeml> root; Premiere is the far more common source for this
            // importer today, so classify <xmeml> as Premiere.
            XmlFormat::PremiereXml
        } else {
            XmlFormat::Auto
        }
    }

    /// Parse Final Cut Pro XML (FCPXML).
    ///
    /// Delegates to [`FcpxmlParser`] — the crate's real hand-written FCPXML
    /// parser (also used directly via [`crate::FcpxmlParser`]) — and maps
    /// its `ConformProject { sequences: [ConformSequence { clips:
    /// [ConformClip] }] }` model onto [`ClipReference`]. An earlier
    /// revision ignored `content` entirely and returned `Ok(Vec::new())`,
    /// reporting "0 clips found" even for a well-formed FCPXML file.
    fn parse_fcpxml(&self, content: &str) -> ConformResult<Vec<ClipReference>> {
        let project = FcpxmlParser::parse(content)?;

        let mut clips = Vec::new();
        for sequence in &project.sequences {
            // frame_rate == 0.0 means the parser could not resolve a
            // <format> frameDuration; fall back to a conservative default
            // (matching EdlImporter's convention) rather than dividing by
            // zero when converting seconds to frames below.
            let fps = if sequence.frame_rate > 0.0 {
                FrameRate::Custom(f64::from(sequence.frame_rate))
            } else {
                FrameRate::Fps25
            };

            for clip in &sequence.clips {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("format".to_string(), "fcpxml".to_string());
                metadata.insert("sequence".to_string(), sequence.name.clone());
                // Preserve full sub-frame precision alongside the
                // frame-rounded Timecode fields below.
                metadata.insert("offset_s".to_string(), clip.offset_s.to_string());
                metadata.insert("duration_s".to_string(), clip.duration_s.to_string());

                clips.push(ClipReference {
                    id: clip.name.clone(),
                    source_file: Some(clip.name.clone()),
                    source_in: seconds_to_timecode(clip.src_in_s, fps),
                    source_out: seconds_to_timecode(clip.src_out_s, fps),
                    record_in: seconds_to_timecode(clip.offset_s, fps),
                    record_out: seconds_to_timecode(clip.offset_s + clip.duration_s, fps),
                    // The lean hand-written FCPXML parser does not currently
                    // distinguish video-only / audio-only spine clips; FCP X
                    // `asset-clip` elements are typically synchronized
                    // audio+video, so AudioVideo is the honest default
                    // rather than guessing Video specifically.
                    track: TrackType::AudioVideo,
                    fps,
                    metadata,
                });
            }
        }

        Ok(clips)
    }

    /// Parse Premiere XML.
    ///
    /// Adobe Premiere Pro's "File > Export > Final Cut Pro XML" produces an
    /// `<xmeml>` document — the "Final Cut Pro 7 XML Interchange Format"
    /// DTD, which is the *same* DTD [`Self::parse_resolve_xml`] consumes
    /// (see the [`crate::importers::xmeml`] module docs for why one parser
    /// serves both). This function differs from `parse_resolve_xml` only in
    /// the `metadata["format"]` tag it stamps onto each clip.
    ///
    /// # Errors
    ///
    /// Returns [`ConformError::Xml`] for malformed XML, or
    /// [`ConformError::UnsupportedFormat`] for constructs the flat
    /// [`ClipReference`] model cannot represent (compound/nested-sequence
    /// clips, or unresolvable `-1` transition-sentinel boundaries) — see
    /// the [`crate::importers::xmeml`] module docs.
    fn parse_premiere_xml(&self, content: &str) -> ConformResult<Vec<ClipReference>> {
        let sequences = crate::importers::xmeml::XmemlParser::parse(content)?;
        Ok(xmeml_sequences_to_clips(sequences, "premiere_xmeml"))
    }

    /// Parse Resolve XML.
    ///
    /// `DaVinci` Resolve's timeline export ("Timeline > Export > Timeline >
    /// Final Cut Pro 7 XML") produces the same `<xmeml>` "Final Cut Pro 7
    /// XML Interchange Format" DTD that [`Self::parse_premiere_xml`]
    /// consumes — Resolve does not define a separate XML schema of its own
    /// for timeline export (its *other* export option, "Final Cut Pro XML",
    /// is the modern FCPXML format handled by
    /// [`Self::parse_fcpxml`]/[`crate::importers::fcpxml::FcpxmlParser`]
    /// instead). See the [`crate::importers::xmeml`] module docs for the
    /// shared parser and its rate/track/clip/timecode handling.
    ///
    /// # Errors
    ///
    /// Returns [`ConformError::Xml`] for malformed XML, or
    /// [`ConformError::UnsupportedFormat`] for constructs the flat
    /// [`ClipReference`] model cannot represent — see the
    /// [`crate::importers::xmeml`] module docs.
    fn parse_resolve_xml(&self, content: &str) -> ConformResult<Vec<ClipReference>> {
        let sequences = crate::importers::xmeml::XmemlParser::parse(content)?;
        Ok(xmeml_sequences_to_clips(sequences, "resolve_xmeml"))
    }
}

/// Map parsed xmeml sequences into the crate's flat [`ClipReference`] list,
/// tagging each clip's `metadata["format"]` with `source_format` so callers
/// can tell a Premiere-tagged import from a Resolve-tagged one even though
/// both went through the same [`crate::importers::xmeml::XmemlParser`].
///
/// Source (`<in>`/`<out>`) and record (`<start>`/`<end>`) frame counts are
/// each converted through their own rate (see the xmeml module's "Rate
/// scoping" docs) into real seconds, then re-quantized to a single
/// [`Timecode`] rate (the sequence rate) via [`seconds_to_timecode`] — the
/// same pattern [`XmlImporter::parse_fcpxml`] already uses for FCPXML.
fn xmeml_sequences_to_clips(
    sequences: Vec<crate::importers::xmeml::XmemlSequence>,
    source_format: &str,
) -> Vec<ClipReference> {
    let mut clips = Vec::new();
    for seq in sequences {
        for clip in seq.clips {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("format".to_string(), source_format.to_string());
            metadata.insert("sequence".to_string(), seq.name.clone());
            metadata.insert(
                "sequence_fps".to_string(),
                format!("{:.6}", seq.fps.as_f64()),
            );
            metadata.insert("track_index".to_string(), clip.track_index.to_string());
            metadata.insert(
                "source_fps".to_string(),
                format!("{:.6}", clip.source_fps.as_f64()),
            );
            if let Some(duration) = clip.duration_frames {
                metadata.insert("xmeml_duration_frames".to_string(), duration.to_string());
            }
            if let Some(t) = &clip.transition_in {
                metadata.insert("transition_in".to_string(), t.name.clone());
            }
            if let Some(t) = &clip.transition_out {
                metadata.insert("transition_out".to_string(), t.name.clone());
            }

            let source_in_s = clip.source_in_frames as f64 / clip.source_fps.as_f64();
            let source_out_s = clip.source_out_frames as f64 / clip.source_fps.as_f64();
            let record_in_s = clip.record_in_frames as f64 / clip.record_fps.as_f64();
            let record_out_s = clip.record_out_frames as f64 / clip.record_fps.as_f64();

            let id = if clip.name.is_empty() {
                format!(
                    "clipitem@{}-{}",
                    clip.record_in_frames, clip.record_out_frames
                )
            } else {
                clip.name.clone()
            };

            clips.push(ClipReference {
                id,
                source_file: clip.file_path.clone().or_else(|| Some(clip.name.clone())),
                source_in: seconds_to_timecode(source_in_s, clip.record_fps),
                source_out: seconds_to_timecode(source_out_s, clip.record_fps),
                record_in: seconds_to_timecode(record_in_s, clip.record_fps),
                record_out: seconds_to_timecode(record_out_s, clip.record_fps),
                track: clip.track,
                fps: clip.record_fps,
                metadata,
            });
        }
    }
    clips
}

/// Convert a time offset in seconds to a [`Timecode`] at `fps`.
fn seconds_to_timecode(seconds: f64, fps: FrameRate) -> Timecode {
    let total_frames = (seconds.max(0.0) * fps.as_f64()).round() as u64;
    Timecode::from_frames(total_frames, fps)
}

impl Default for XmlImporter {
    fn default() -> Self {
        Self::new(XmlFormat::Auto)
    }
}

impl TimelineImporter for XmlImporter {
    fn import<P: AsRef<Path>>(&self, path: P) -> ConformResult<Vec<ClipReference>> {
        let content = std::fs::read_to_string(path)?;

        let format = if self.format == XmlFormat::Auto {
            Self::detect_format(&content)
        } else {
            self.format
        };

        match format {
            XmlFormat::FcpXml => self.parse_fcpxml(&content),
            XmlFormat::PremiereXml => self.parse_premiere_xml(&content),
            XmlFormat::ResolveXml => self.parse_resolve_xml(&content),
            XmlFormat::Auto => Err(ConformError::UnsupportedFormat(
                "Could not auto-detect XML format".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_importer_creation() {
        let importer = XmlImporter::new(XmlFormat::FcpXml);
        assert_eq!(importer.format, XmlFormat::FcpXml);
    }

    #[test]
    fn test_xml_importer_default() {
        let importer = XmlImporter::default();
        assert_eq!(importer.format, XmlFormat::Auto);
    }

    const MINIMAL_FCPXML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<fcpxml version="1.9">
  <resources>
    <format id="r1" name="FFVideoFormat1080p24" frameDuration="100/2400s" width="1920" height="1080"/>
  </resources>
  <library>
    <event name="My Event">
      <project name="My Project">
        <sequence format="r1" tcStart="0s" tcFormat="NDF">
          <spine>
            <asset-clip name="Shot_001.mov" offset="0s" duration="96/2400s" start="86400s"/>
            <asset-clip name="Shot_002.mov" offset="96/2400s" duration="144/2400s" start="86400s"/>
          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>"#;

    fn tmp_xml_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia-conform-xml-importer-{name}"))
    }

    #[test]
    fn test_detect_format_sniffs_fcpxml() {
        assert_eq!(
            XmlImporter::detect_format(MINIMAL_FCPXML),
            XmlFormat::FcpXml
        );
    }

    #[test]
    fn test_detect_format_unknown_content_is_auto_not_a_guess() {
        // CHANGED: detect_format() previously always guessed FcpXml
        // regardless of content, which made the `XmlFormat::Auto =>
        // Err(..)` arm in `import()` permanently unreachable dead code.
        // Unrecognized content must now come back as Auto so that arm
        // actually fires instead of silently misrouting the file through
        // the FCPXML parser.
        assert_eq!(
            XmlImporter::detect_format("<not-a-timeline-xml/>"),
            XmlFormat::Auto
        );
    }

    #[test]
    fn test_parse_fcpxml_real_extraction() {
        // CHANGED: parse_fcpxml() previously ignored `content` and returned
        // `Ok(Vec::new())`, reporting "0 clips" for any FCPXML file
        // including well-formed ones. It now delegates to the crate's real
        // FcpxmlParser and must extract the real clips.
        let importer = XmlImporter::new(XmlFormat::FcpXml);

        let clips = importer
            .parse_fcpxml(MINIMAL_FCPXML)
            .expect("real FCPXML content should parse");

        assert_eq!(
            clips.len(),
            2,
            "expected 2 real clips extracted, not a fabricated empty Ok(vec![])"
        );
        assert_eq!(clips[0].id, "Shot_001.mov");
        assert_eq!(clips[1].id, "Shot_002.mov");
        assert_eq!(clips[0].source_file.as_deref(), Some("Shot_001.mov"));
    }

    #[test]
    fn test_import_fcpxml_end_to_end_from_file() -> ConformResult<()> {
        let path = tmp_xml_path("minimal.fcpxml");
        std::fs::write(&path, MINIMAL_FCPXML)?;

        let importer = XmlImporter::new(XmlFormat::FcpXml);
        let clips = importer.import(&path)?;
        assert_eq!(clips.len(), 2);

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    const MINIMAL_PREMIERE_XMEML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmeml version="4">
  <sequence>
    <name>Premiere Sequence</name>
    <rate><timebase>24</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Shot_001.mov</name>
            <start>0</start>
            <end>48</end>
            <in>0</in>
            <out>48</out>
            <file id="file-1">
              <name>Shot_001.mov</name>
              <pathurl>file:///Volumes/Media/Shot_001.mov</pathurl>
            </file>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;

    const MINIMAL_RESOLVE_XMEML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmeml version="5">
  <sequence>
    <name>Resolve Timeline</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Shot_A.mov</name>
            <start>0</start>
            <end>50</end>
            <in>0</in>
            <out>50</out>
            <file id="file-a">
              <name>Shot_A.mov</name>
              <pathurl>file:///Volumes/Media/Shot_A.mov</pathurl>
            </file>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;

    #[test]
    fn test_parse_premiere_xml_real_extraction() {
        // CHANGED: parse_premiere_xml() previously always returned an
        // UnsupportedFormat error regardless of content. It now delegates
        // to XmemlParser and must extract the real clip.
        let importer = XmlImporter::new(XmlFormat::PremiereXml);
        let clips = importer
            .parse_premiere_xml(MINIMAL_PREMIERE_XMEML)
            .expect("real xmeml content should parse");

        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id, "Shot_001.mov");
        assert_eq!(
            clips[0].source_file.as_deref(),
            Some("/Volumes/Media/Shot_001.mov")
        );
        assert_eq!(
            clips[0].metadata.get("format").map(String::as_str),
            Some("premiere_xmeml")
        );
    }

    #[test]
    fn test_parse_resolve_xml_real_extraction() {
        // CHANGED: parse_resolve_xml() previously always returned an
        // UnsupportedFormat error regardless of content. It now delegates
        // to the same XmemlParser Premiere uses (see module docs on why
        // Resolve's timeline-XML export shares Premiere's xmeml DTD) and
        // must extract the real clip.
        let importer = XmlImporter::new(XmlFormat::ResolveXml);
        let clips = importer
            .parse_resolve_xml(MINIMAL_RESOLVE_XMEML)
            .expect("real xmeml content should parse");

        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id, "Shot_A.mov");
        assert_eq!(
            clips[0].metadata.get("format").map(String::as_str),
            Some("resolve_xmeml")
        );
    }

    #[test]
    fn test_parse_premiere_xml_malformed_is_honest_err() {
        let importer = XmlImporter::new(XmlFormat::PremiereXml);
        // Unclosed <sequence> tag: not well-formed XML.
        let result = importer.parse_premiere_xml("<xmeml version=\"4\"><sequence>");
        assert!(
            result.is_err(),
            "malformed xmeml must not silently parse as zero clips"
        );
    }

    #[test]
    fn test_parse_resolve_xml_malformed_is_honest_err() {
        let importer = XmlImporter::new(XmlFormat::ResolveXml);
        let result = importer.parse_resolve_xml("<xmeml version=\"5\"><sequence>");
        assert!(
            result.is_err(),
            "malformed xmeml must not silently parse as zero clips"
        );
    }

    #[test]
    fn test_parse_premiere_xml_empty_sequence_is_ok_zero_clips() {
        // A well-formed but empty document is a real (if empty) result, not
        // an error — consistent with parse_fcpxml's "<fcpxml/>" handling.
        let importer = XmlImporter::new(XmlFormat::PremiereXml);
        let clips = importer
            .parse_premiere_xml("<xmeml version=\"4\"/>")
            .expect("well-formed empty xmeml should parse");
        assert!(clips.is_empty());
    }

    #[test]
    fn test_import_resolve_xml_end_to_end_from_file() -> ConformResult<()> {
        let path = tmp_xml_path("minimal.resolve.xml");
        std::fs::write(&path, MINIMAL_RESOLVE_XMEML)?;

        let importer = XmlImporter::new(XmlFormat::ResolveXml);
        let clips = importer.import(&path)?;
        assert_eq!(clips.len(), 1);

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn test_import_auto_detect_unknown_format_is_honest_err() {
        let path = tmp_xml_path("unknown.xml");
        std::fs::write(&path, "<not-a-timeline-xml/>").expect("write temp file");

        let importer = XmlImporter::default(); // Auto
        let result = importer.import(&path);
        assert!(
            result.is_err(),
            "unrecognized XML must not silently import as FCPXML"
        );

        std::fs::remove_file(&path).ok();
    }
}

//! xmeml (Final Cut Pro 7 XML Interchange Format) parser.
//!
//! `xmeml` is the XML interchange format historically defined by Apple for
//! the legacy "Final Cut Pro 7". It is **not** the modern FCPXML format
//! (see [`crate::importers::fcpxml`]) — both Adobe Premiere Pro
//! ("File > Export > Final Cut Pro XML") and `DaVinci` Resolve
//! ("Timeline > Export > Final Cut Pro 7 XML") implement this same
//! `<xmeml>` DTD as their interchange format rather than a Resolve- or
//! Premiere-specific schema, so a single parser here serves both
//! [`crate::importers::xml::XmlImporter`]'s Premiere and Resolve code
//! paths. Version attributes vary in practice (Premiere commonly emits
//! `version="4"` or `"5"`; Resolve commonly emits `version="5"`), but the
//! element structure consumed below is shared across versions and
//! exporters.
//!
//! # Element structure parsed
//!
//! ```text
//! xmeml
//!   sequence                       (one or more; also found nested under
//!                                   project/children/bin for multi-sequence
//!                                   projects — any nesting depth is
//!                                   accepted as long as it is not inside a
//!                                   <clipitem> or <file>)
//!     name
//!     rate / timebase, ntsc         sequence frame rate
//!     timecode / displayformat      DF vs NDF (drop-frame) flag
//!     media
//!       video | audio
//!         track                     (one or more, in document order)
//!           clipitem
//!             name
//!             rate / timebase, ntsc   clip/source rate, if present
//!             in, out                 source in/out (frames, source rate)
//!             start, end              record in/out (frames, sequence rate)
//!             duration
//!             file id="..."
//!               name, pathurl, rate   (first occurrence defines the file;
//!                                      later clipitems reference it via a
//!                                      self-closing `<file id="..."/>`)
//!           transitionitem
//!             start, end
//!             effect / name
//! ```
//!
//! # Constructs this parser cannot represent
//!
//! [`crate::types::ClipReference`] is a flat per-clip record (no nested
//! timeline, no first-class transition type). Two xmeml constructs fall
//! outside that model and are reported as an honest
//! [`ConformError::UnsupportedFormat`] naming the element path, rather than
//! silently dropping the clip or fabricating data for it:
//!
//! - **Compound / nested-sequence clips** — a `<clipitem>` whose content is
//!   an embedded `<sequence>` rather than a `<file>` reference. There is no
//!   media file to point `source_file` at, so fabricating one would
//!   misrepresent the source.
//! - **`-1` ("determined by the adjacent transition") sentinels** on
//!   `<start>`/`<end>`/`<in>`/`<out>` — real FCP7 XML allows a clipitem
//!   touching a transition to omit its true boundary this way. Deriving
//!   the correct value requires knowing exactly how a given exporter
//!   resolves the adjacent `<transitionitem>` against it, which is not
//!   verifiable from the (informal, exporter-varying) DTD alone —
//!   inventing a resolution formula here risks *silently* producing a
//!   wrong timecode, which is worse than refusing. A missing
//!   `<in>`/`<out>`/`<start>`/`<end>` altogether is treated the same way
//!   (never defaulted to `0`).
//!
//! `<marker>` elements are recognized and intentionally skipped: they carry
//! no clip data, and [`crate::importers::TimelineImporter::import`] has no
//! warnings channel to report benign skips through, so this is documented
//! here rather than surfaced at runtime.
//!
//! Transitions themselves (a `<transitionitem>` with a resolvable adjacent
//! clip) ARE represented, best-effort, as metadata on the neighbouring clip
//! (see `transition_in`/`transition_out` on [`XmemlClip`] and the
//! `transition_in`/`transition_out` metadata keys set by
//! [`crate::importers::xml::XmlImporter`]) rather than dropped, since clip
//! metadata is the closest thing this flat model has to transition
//! support.
//!
//! # Rate scoping
//!
//! `<in>`/`<out>` are expressed in the *source* clip's own rate (its
//! `<clipitem><rate>`, falling back to `<clipitem><file><rate>`, falling
//! back to the sequence rate), while `<start>`/`<end>` are expressed in the
//! *sequence*'s rate. Mixing the two (e.g. a 24p clip cut into a 25p
//! timeline) is handled by converting each pair to real seconds using its
//! own rate before re-quantizing to [`crate::types::Timecode`] — see
//! [`crate::importers::xml`]'s mapping function.
//!
//! `<out>`/`<end>` are exclusive (one frame past the last included frame),
//! matching this crate's existing FCPXML importer convention
//! (`src_out_s = src_in_s + duration_s`).

use crate::error::{ConformError, ConformResult};
use crate::types::{FrameRate, TrackType};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Output data model
// ---------------------------------------------------------------------------

/// A transition parsed from an xmeml `<transitionitem>`.
#[derive(Debug, Clone, Default)]
pub(crate) struct XmemlTransition {
    /// Effect name (from `<effect><name>`), e.g. "Cross Dissolve".
    pub name: String,
    /// Transition start, in sequence-rate frames, if present.
    pub start_frames: Option<i64>,
    /// Transition end, in sequence-rate frames, if present.
    pub end_frames: Option<i64>,
}

/// One clip parsed from an xmeml `<clipitem>`, fully resolved (no
/// unresolved sentinels — those are rejected during parsing; see the
/// module docs).
#[derive(Debug, Clone)]
pub(crate) struct XmemlClip {
    pub name: String,
    pub file_path: Option<String>,
    pub track: TrackType,
    /// 1-based index of this clip's track within its kind (V1, V2, A1, ...).
    pub track_index: u32,
    /// Source in/out, in frames at `source_fps` (exclusive `out`).
    pub source_in_frames: i64,
    pub source_out_frames: i64,
    pub source_fps: FrameRate,
    /// Record (timeline) in/out, in frames at `record_fps` (exclusive `end`).
    pub record_in_frames: i64,
    pub record_out_frames: i64,
    pub record_fps: FrameRate,
    /// Raw `<duration>` value, if present (kept for metadata/audit only).
    pub duration_frames: Option<i64>,
    /// A transition attached to the head/tail of this clip, if any.
    pub transition_in: Option<XmemlTransition>,
    pub transition_out: Option<XmemlTransition>,
}

/// One `<sequence>` parsed from an xmeml document.
#[derive(Debug, Clone)]
pub(crate) struct XmemlSequence {
    pub name: String,
    pub fps: FrameRate,
    pub clips: Vec<XmemlClip>,
}

// ---------------------------------------------------------------------------
// Parser entry point
// ---------------------------------------------------------------------------

/// Parser for xmeml (Adobe Premiere Pro / `DaVinci` Resolve "Final Cut Pro 7
/// XML") documents.
pub(crate) struct XmemlParser;

impl XmemlParser {
    /// Parse an xmeml document string and return every `<sequence>` found.
    ///
    /// # Errors
    ///
    /// Returns [`ConformError::Xml`] for malformed XML, or
    /// [`ConformError::UnsupportedFormat`] for constructs the flat clip
    /// model cannot represent — see the module docs.
    pub(crate) fn parse(xml: &str) -> ConformResult<Vec<XmemlSequence>> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut path: Vec<String> = Vec::new();
        let mut state = ParseState::default();
        let mut skip_depth: u32 = 0;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Eof) => break,
                Err(e) => return Err(ConformError::Xml(e)),
                Ok(Event::Start(ref e)) => {
                    let tag = tag_name(e.name().as_ref());
                    if skip_depth > 0 {
                        skip_depth += 1;
                    } else if state.on_start(&tag, &path, e)? {
                        skip_depth = 1;
                    }
                    path.push(tag);
                }
                Ok(Event::Empty(ref e)) => {
                    let tag = tag_name(e.name().as_ref());
                    if skip_depth == 0 {
                        let skip_this = state.on_start(&tag, &path, e)?;
                        if !skip_this {
                            state.on_end(&tag, &path)?;
                        }
                    }
                }
                Ok(Event::Text(ref e)) => {
                    if skip_depth == 0 {
                        let text = String::from_utf8_lossy(e.as_ref()).into_owned();
                        state.on_text(&path, &text);
                    }
                }
                Ok(Event::End(ref e)) => {
                    let tag = tag_name(e.name().as_ref());
                    path.pop();
                    if skip_depth > 0 {
                        skip_depth -= 1;
                    } else {
                        state.on_end(&tag, &path)?;
                    }
                }
                _ => {}
            }
            buf.clear();
        }

        // Reaching Eof with an element still open means the document was
        // truncated (a mismatched end tag is its own quick_xml::Error, but
        // a *missing* end tag at EOF is not — see test coverage below). Any
        // clip/track/sequence left mid-parse here was never finalized and
        // would otherwise vanish with no trace, which is exactly the
        // "silently drop a clip" outcome this importer must not produce;
        // report it as an honest error instead.
        if state.clip.is_some()
            || state.transition.is_some()
            || state.file.is_some()
            || state.track.is_some()
            || state.seq.is_some()
        {
            return Err(ConformError::UnsupportedFormat(
                "xmeml document ended (EOF) with an element still open — truncated file? \
                 refusing to silently drop the in-progress clip/track/sequence"
                    .to_string(),
            ));
        }

        Ok(state.sequences)
    }
}

fn tag_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Element `from_end` positions back from the end of `path` (0 = last).
fn at<'a>(path: &'a [String], from_end: usize) -> Option<&'a str> {
    path.len()
        .checked_sub(from_end + 1)
        .map(|i| path[i].as_str())
}

/// True if `tag` appears anywhere in `path` (any ancestor depth).
fn contains(path: &[String], tag: &str) -> bool {
    path.iter().any(|t| t == tag)
}

/// Nearest ancestor (scanning from the end of `path`) that is one of
/// `candidates`.
fn nearest<'a>(path: &'a [String], candidates: &[&str]) -> Option<&'a str> {
    path.iter()
        .rev()
        .find(|t| candidates.contains(&t.as_str()))
        .map(String::as_str)
}

fn attr_val(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(std::result::Result::ok)
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

/// Strip a `file://` / `file://localhost` scheme prefix from an xmeml
/// `<pathurl>` value, leaving an absolute filesystem path. Percent-encoded
/// characters are passed through unchanged.
fn strip_file_scheme(url: &str) -> String {
    url.strip_prefix("file://localhost")
        .or_else(|| url.strip_prefix("file://"))
        .unwrap_or(url)
        .to_string()
}

// ---------------------------------------------------------------------------
// Frame-rate resolution
// ---------------------------------------------------------------------------

/// Resolve an xmeml `<rate>` block (`<timebase>` + `<ntsc>`) into a
/// [`FrameRate`]. The NTSC flag maps an integer timebase onto the
/// corresponding NTSC rate (24→23.976, 30→29.97, 60→59.94); drop-frame vs
/// non-drop-frame (for a 30 timebase) is refined separately from the
/// sequence's `<timecode><displayformat>`, since `<rate>` alone cannot
/// distinguish them.
fn resolve_frame_rate(timebase: Option<u32>, ntsc: Option<bool>) -> FrameRate {
    let tb = timebase.unwrap_or(25);
    if ntsc.unwrap_or(false) {
        match tb {
            24 => FrameRate::Fps23976,
            30 => FrameRate::Fps2997NDF,
            60 => FrameRate::Fps5994,
            _ => FrameRate::Custom(f64::from(tb) * 1000.0 / 1001.0),
        }
    } else {
        match tb {
            24 => FrameRate::Fps24,
            25 => FrameRate::Fps25,
            30 => FrameRate::Fps30,
            50 => FrameRate::Fps50,
            60 => FrameRate::Fps60,
            _ => FrameRate::Custom(f64::from(tb)),
        }
    }
}

// ---------------------------------------------------------------------------
// Parse-time accumulators
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SeqAccum {
    name: String,
    fps: Option<FrameRate>,
    display_df: Option<bool>,
    clips: Vec<XmemlClip>,
}

struct TrackAccum {
    kind: TrackType,
    index: u32,
    items: Vec<XmemlItem>,
}

enum XmemlItem {
    Clip(XmemlClip),
    Transition(XmemlTransition),
}

#[derive(Default)]
struct ClipAccum {
    name: String,
    file_path: Option<String>,
    in_frames: Option<i64>,
    out_frames: Option<i64>,
    start_frames: Option<i64>,
    end_frames: Option<i64>,
    duration_frames: Option<i64>,
    clip_fps: Option<FrameRate>,
    file_fps: Option<FrameRate>,
}

#[derive(Default)]
struct FileAccum {
    id: String,
    name: Option<String>,
    pathurl: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct FileInfo {
    name: Option<String>,
    pathurl: Option<String>,
}

#[derive(Default)]
struct ParseState {
    sequences: Vec<XmemlSequence>,
    seq: Option<SeqAccum>,
    track: Option<TrackAccum>,
    clip: Option<ClipAccum>,
    transition: Option<XmemlTransition>,
    file: Option<FileAccum>,
    file_map: HashMap<String, FileInfo>,
    current_media_kind: Option<TrackType>,
    video_track_n: u32,
    audio_track_n: u32,
    pending_timebase: Option<u32>,
    pending_ntsc: Option<bool>,
}

impl ParseState {
    /// Handle an opening (or self-closing) tag. `path` holds ancestors only
    /// (not including `tag`). Returns `Ok(true)` when the caller should
    /// treat `tag` and its whole subtree as unrecognized/ignorable content
    /// (rather than reusing flat sequence/track/clip state that belongs to
    /// an unrelated context).
    fn on_start(&mut self, tag: &str, path: &[String], e: &BytesStart<'_>) -> ConformResult<bool> {
        match tag {
            "sequence" => {
                if at(path, 0) == Some("clipitem") {
                    let name = self.clip.as_ref().map_or("", |c| c.name.as_str());
                    return Err(ConformError::UnsupportedFormat(format!(
                        "{}/sequence: clipitem \"{name}\" contains a nested <sequence> \
                         (compound/nested/multicam clip); this importer represents clips as \
                         a flat list and cannot embed a nested timeline",
                        path.join("/"),
                    )));
                }
                if contains(path, "clipitem") || contains(path, "file") {
                    // Non-standard <sequence> placement that is not the direct
                    // compound-clip shape above (e.g. deep inside a <file>'s own
                    // <media> description). Skip the whole subtree rather than
                    // reusing this parser's flat state for content that was
                    // never meant to start a fresh sequence.
                    return Ok(true);
                }
                self.seq = Some(SeqAccum::default());
                self.video_track_n = 0;
                self.audio_track_n = 0;
            }
            "video" => {
                if at(path, 0) == Some("media") && at(path, 1) == Some("sequence") {
                    self.current_media_kind = Some(TrackType::Video);
                }
            }
            "audio" => {
                if at(path, 0) == Some("media") && at(path, 1) == Some("sequence") {
                    self.current_media_kind = Some(TrackType::Audio);
                }
            }
            "track" => {
                let under_sequence_media = (at(path, 0) == Some("video")
                    || at(path, 0) == Some("audio"))
                    && at(path, 1) == Some("media")
                    && at(path, 2) == Some("sequence");
                if under_sequence_media {
                    if let Some(kind) = self.current_media_kind {
                        let index = if kind == TrackType::Video {
                            self.video_track_n += 1;
                            self.video_track_n
                        } else {
                            self.audio_track_n += 1;
                            self.audio_track_n
                        };
                        self.track = Some(TrackAccum {
                            kind,
                            index,
                            items: Vec::new(),
                        });
                    }
                }
            }
            "clipitem" => {
                if at(path, 0) == Some("track") && self.track.is_some() {
                    self.clip = Some(ClipAccum::default());
                }
            }
            "transitionitem" => {
                if at(path, 0) == Some("track") && self.track.is_some() {
                    self.transition = Some(XmemlTransition::default());
                }
            }
            "file" => {
                if at(path, 0) == Some("clipitem") && self.clip.is_some() {
                    let id = attr_val(e, b"id").unwrap_or_default();
                    self.file = Some(FileAccum {
                        id,
                        name: None,
                        pathurl: None,
                    });
                }
            }
            "rate" => {
                self.pending_timebase = None;
                self.pending_ntsc = None;
            }
            _ => {}
        }
        Ok(false)
    }

    /// Handle character content of the innermost currently-open element.
    /// `path` includes that element itself as its last entry.
    fn on_text(&mut self, path: &[String], text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let leaf = at(path, 0);
        let parent = at(path, 1);

        if leaf == Some("name") {
            self.on_name_text(parent, path, text);
        } else if leaf == Some("pathurl") && parent == Some("file") {
            if let Some(file) = self.file.as_mut() {
                file.pathurl = Some(strip_file_scheme(text));
            }
        } else if leaf == Some("timebase") && parent == Some("rate") {
            self.pending_timebase = text.parse::<u32>().ok();
        } else if leaf == Some("ntsc") && parent == Some("rate") {
            self.pending_ntsc = Some(text.eq_ignore_ascii_case("true"));
        } else if leaf == Some("displayformat")
            && parent == Some("timecode")
            && nearest(path, &["sequence", "clipitem"]) == Some("sequence")
        {
            if let Some(seq) = self.seq.as_mut() {
                seq.display_df = Some(text.eq_ignore_ascii_case("DF"));
            }
        } else if parent == Some("clipitem") {
            if let Some(clip) = self.clip.as_mut() {
                match leaf {
                    Some("in") => clip.in_frames = text.parse::<i64>().ok(),
                    Some("out") => clip.out_frames = text.parse::<i64>().ok(),
                    Some("start") => clip.start_frames = text.parse::<i64>().ok(),
                    Some("end") => clip.end_frames = text.parse::<i64>().ok(),
                    Some("duration") => clip.duration_frames = text.parse::<i64>().ok(),
                    _ => {}
                }
            }
        } else if parent == Some("transitionitem") {
            if let Some(t) = self.transition.as_mut() {
                match leaf {
                    Some("start") => t.start_frames = text.parse::<i64>().ok(),
                    Some("end") => t.end_frames = text.parse::<i64>().ok(),
                    _ => {}
                }
            }
        }
    }

    fn on_name_text(&mut self, parent: Option<&str>, path: &[String], text: &str) {
        match parent {
            Some("sequence") => {
                if let Some(seq) = self.seq.as_mut() {
                    if seq.name.is_empty() {
                        seq.name = text.to_string();
                    }
                }
            }
            Some("clipitem") => {
                if let Some(clip) = self.clip.as_mut() {
                    clip.name = text.to_string();
                }
            }
            Some("file") => {
                if let Some(file) = self.file.as_mut() {
                    if file.name.is_none() {
                        file.name = Some(text.to_string());
                    }
                }
            }
            Some("effect") if contains(path, "transitionitem") => {
                if let Some(t) = self.transition.as_mut() {
                    t.name = text.to_string();
                }
            }
            _ => {}
        }
    }

    /// Handle a closing tag. `path` holds ancestors only (`tag` already
    /// popped), matching `on_start`'s convention.
    fn on_end(&mut self, tag: &str, path: &[String]) -> ConformResult<()> {
        match tag {
            "rate" => self.end_rate(path),
            "file" => self.end_file(),
            "transitionitem" => {
                if let Some(t) = self.transition.take() {
                    if let Some(track) = self.track.as_mut() {
                        track.items.push(XmemlItem::Transition(t));
                    }
                }
            }
            "clipitem" => {
                if let Some(c) = self.clip.take() {
                    let finalized = finalize_clip(c, &self.seq, &self.track)?;
                    if let Some(track) = self.track.as_mut() {
                        track.items.push(XmemlItem::Clip(finalized));
                    }
                }
            }
            "track" => {
                if let Some(mut track) = self.track.take() {
                    attach_transitions(&mut track.items);
                    if let Some(seq) = self.seq.as_mut() {
                        for item in track.items {
                            if let XmemlItem::Clip(clip) = item {
                                seq.clips.push(clip);
                            }
                        }
                    }
                }
            }
            "sequence" => {
                if let Some(seq) = self.seq.take() {
                    self.sequences.push(finalize_sequence(seq));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn end_rate(&mut self, path: &[String]) {
        let owner = nearest(path, &["sequence", "clipitem", "file", "transitionitem"]);
        let fps = resolve_frame_rate(self.pending_timebase, self.pending_ntsc);
        match owner {
            Some("sequence") => {
                if let Some(seq) = self.seq.as_mut() {
                    seq.fps = Some(fps);
                }
            }
            Some("clipitem") => {
                if let Some(clip) = self.clip.as_mut() {
                    clip.clip_fps = Some(fps);
                }
            }
            Some("file") => {
                if let Some(clip) = self.clip.as_mut() {
                    clip.file_fps = Some(fps);
                }
            }
            _ => {}
        }
        self.pending_timebase = None;
        self.pending_ntsc = None;
    }

    fn end_file(&mut self) {
        let Some(f) = self.file.take() else {
            return;
        };
        let resolved = if f.name.is_some() || f.pathurl.is_some() {
            self.file_map.insert(
                f.id.clone(),
                FileInfo {
                    name: f.name.clone(),
                    pathurl: f.pathurl.clone(),
                },
            );
            f
        } else if let Some(info) = self.file_map.get(&f.id) {
            FileAccum {
                id: f.id.clone(),
                name: info.name.clone(),
                pathurl: info.pathurl.clone(),
            }
        } else {
            // Unresolvable forward reference (the id's full definition has
            // not appeared yet). Real exporters always define-then-reference
            // in document order, so this is a rare/malformed-input case; a
            // missing source file is representable (Option<String>), so this
            // is left as None rather than treated as an error.
            f
        };
        if let Some(clip) = self.clip.as_mut() {
            clip.file_path = resolved.pathurl.clone().or_else(|| resolved.name.clone());
        }
    }
}

fn require_frame_field(value: Option<i64>, field: &str, clip_name: &str) -> ConformResult<i64> {
    match value {
        Some(v) if v >= 0 => Ok(v),
        Some(v) => Err(ConformError::UnsupportedFormat(format!(
            "clipitem \"{clip_name}\": <{field}>{v}</{field}> is a \"determined by the \
             adjacent transition\" sentinel; this parser does not attempt to derive the true \
             value from the neighbouring <transitionitem> and will not guess — confirm it \
             manually"
        ))),
        None => Err(ConformError::UnsupportedFormat(format!(
            "clipitem \"{clip_name}\": missing required <{field}> value"
        ))),
    }
}

fn finalize_clip(
    c: ClipAccum,
    seq: &Option<SeqAccum>,
    track: &Option<TrackAccum>,
) -> ConformResult<XmemlClip> {
    let in_f = require_frame_field(c.in_frames, "in", &c.name)?;
    let out_f = require_frame_field(c.out_frames, "out", &c.name)?;
    let start_f = require_frame_field(c.start_frames, "start", &c.name)?;
    let end_f = require_frame_field(c.end_frames, "end", &c.name)?;

    let record_fps = seq.as_ref().and_then(|s| s.fps).unwrap_or(FrameRate::Fps25);
    let source_fps = c.clip_fps.or(c.file_fps).unwrap_or(record_fps);
    let (track_kind, track_index) = track
        .as_ref()
        .map_or((TrackType::Video, 0), |t| (t.kind, t.index));

    Ok(XmemlClip {
        name: c.name,
        file_path: c.file_path,
        track: track_kind,
        track_index,
        source_in_frames: in_f,
        source_out_frames: out_f,
        source_fps,
        record_in_frames: start_f,
        record_out_frames: end_f,
        record_fps,
        duration_frames: c.duration_frames,
        transition_in: None,
        transition_out: None,
    })
}

fn finalize_sequence(seq: SeqAccum) -> XmemlSequence {
    let mut fps = seq.fps.unwrap_or(FrameRate::Fps25);
    if seq.display_df == Some(true) && matches!(fps, FrameRate::Fps2997NDF) {
        fps = FrameRate::Fps2997DF;
    }
    XmemlSequence {
        name: seq.name,
        fps,
        clips: seq.clips,
    }
}

/// Attach each `<transitionitem>` to its immediate clip neighbours
/// (best-effort metadata only — never required for correctness, so an
/// unresolvable/edge-of-track transition is simply left unattached rather
/// than erroring).
fn attach_transitions(items: &mut [XmemlItem]) {
    let n = items.len();
    for i in 0..n {
        let transition = match &items[i] {
            XmemlItem::Transition(t) => t.clone(),
            XmemlItem::Clip(_) => continue,
        };
        if i > 0 {
            if let XmemlItem::Clip(c) = &mut items[i - 1] {
                c.transition_out = Some(transition.clone());
            }
        }
        if let Some(XmemlItem::Clip(c)) = items.get_mut(i + 1) {
            c.transition_in = Some(transition);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // All fixtures below are hand-authored minimal xmeml, written directly
    // against the publicly documented Final Cut Pro 7 XML Interchange
    // Format element structure (not extracted from any real project file).

    const PREMIERE_MINIMAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmeml version="4">
  <sequence>
    <name>Premiere Sequence</name>
    <rate>
      <timebase>25</timebase>
      <ntsc>FALSE</ntsc>
    </rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Shot_001.mov</name>
            <duration>50</duration>
            <rate>
              <timebase>25</timebase>
              <ntsc>FALSE</ntsc>
            </rate>
            <start>0</start>
            <end>50</end>
            <in>100</in>
            <out>150</out>
            <file id="file-1">
              <name>Shot_001.mov</name>
              <pathurl>file://localhost/Volumes/Media/Shot_001.mov</pathurl>
            </file>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;

    #[test]
    fn test_premiere_minimal_clip_count_and_fields() {
        let sequences = XmemlParser::parse(PREMIERE_MINIMAL).expect("should parse");
        assert_eq!(sequences.len(), 1);
        let seq = &sequences[0];
        assert_eq!(seq.name, "Premiere Sequence");
        assert_eq!(seq.fps, FrameRate::Fps25);
        assert_eq!(seq.clips.len(), 1);

        let clip = &seq.clips[0];
        assert_eq!(clip.name, "Shot_001.mov");
        assert_eq!(clip.track, TrackType::Video);
        assert_eq!(clip.track_index, 1);
        assert_eq!(clip.source_in_frames, 100);
        assert_eq!(clip.source_out_frames, 150);
        assert_eq!(clip.record_in_frames, 0);
        assert_eq!(clip.record_out_frames, 50);
        assert_eq!(
            clip.file_path.as_deref(),
            Some("/Volumes/Media/Shot_001.mov")
        );
    }

    #[test]
    fn test_out_and_end_are_exclusive_matches_duration() {
        // Regression guard for the exclusive-<out>/<end> convention: with
        // in=100/out=150 and start=0/end=50, both spans must equal the
        // <duration>50</duration> the fixture declares.
        let sequences = XmemlParser::parse(PREMIERE_MINIMAL).expect("should parse");
        let clip = &sequences[0].clips[0];
        assert_eq!(clip.source_out_frames - clip.source_in_frames, 50);
        assert_eq!(clip.record_out_frames - clip.record_in_frames, 50);
        assert_eq!(clip.duration_frames, Some(50));
    }

    const RESOLVE_NTSC_DF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmeml version="5">
  <sequence>
    <name>Resolve Timeline</name>
    <rate>
      <timebase>30</timebase>
      <ntsc>TRUE</ntsc>
    </rate>
    <timecode>
      <rate>
        <timebase>30</timebase>
        <ntsc>TRUE</ntsc>
      </rate>
      <string>01:00:00;00</string>
      <displayformat>DF</displayformat>
    </timecode>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Shot_A.mov</name>
            <start>0</start>
            <end>90</end>
            <in>0</in>
            <out>90</out>
            <file id="file-a">
              <name>Shot_A.mov</name>
              <pathurl>file:///Volumes/Media/Shot_A.mov</pathurl>
            </file>
          </clipitem>
        </track>
        <track>
          <clipitem id="clipitem-2">
            <name>Shot_B.mov</name>
            <start>0</start>
            <end>60</end>
            <in>200</in>
            <out>260</out>
            <file id="file-b">
              <name>Shot_B.mov</name>
              <pathurl>file:///Volumes/Media/Shot_B.mov</pathurl>
            </file>
          </clipitem>
        </track>
      </video>
      <audio>
        <track>
          <clipitem id="clipitem-3">
            <name>Shot_A.wav</name>
            <start>0</start>
            <end>90</end>
            <in>0</in>
            <out>90</out>
            <file id="file-a-audio">
              <name>Shot_A.wav</name>
              <pathurl>file:///Volumes/Media/Shot_A.wav</pathurl>
            </file>
          </clipitem>
        </track>
      </audio>
    </media>
    <marker>
      <name>Chapter 1</name>
      <in>0</in>
      <out>-1</out>
      <comment>Benign — must not affect clip parsing.</comment>
    </marker>
  </sequence>
</xmeml>"#;

    #[test]
    fn test_resolve_ntsc_drop_frame_resolution() {
        let sequences = XmemlParser::parse(RESOLVE_NTSC_DF).expect("should parse");
        assert_eq!(sequences.len(), 1);
        assert_eq!(sequences[0].fps, FrameRate::Fps2997DF);
    }

    #[test]
    fn test_resolve_multiple_tracks_video_and_audio() {
        let sequences = XmemlParser::parse(RESOLVE_NTSC_DF).expect("should parse");
        let seq = &sequences[0];
        assert_eq!(seq.clips.len(), 3, "2 video-track clips + 1 audio clip");

        let video_clips: Vec<_> = seq
            .clips
            .iter()
            .filter(|c| c.track == TrackType::Video)
            .collect();
        assert_eq!(video_clips.len(), 2);
        // Two separate <track> elements under <video> -> distinct indices.
        let mut indices: Vec<u32> = video_clips.iter().map(|c| c.track_index).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![1, 2]);

        let audio_clips: Vec<_> = seq
            .clips
            .iter()
            .filter(|c| c.track == TrackType::Audio)
            .collect();
        assert_eq!(audio_clips.len(), 1);
        assert_eq!(audio_clips[0].track_index, 1);
    }

    #[test]
    fn test_marker_is_benignly_skipped_without_affecting_clip_count() {
        // RESOLVE_NTSC_DF's trailing <marker> (including its own <out>-1</out>,
        // which would be an error if mistaken for a clipitem field) must not
        // change the clip count or raise an error.
        let sequences = XmemlParser::parse(RESOLVE_NTSC_DF).expect("should parse");
        assert_eq!(sequences[0].clips.len(), 3);
    }

    const TRANSITION_FIXTURE: &str = r#"<xmeml version="4">
  <sequence>
    <name>Transition Seq</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Outgoing.mov</name>
            <start>0</start>
            <end>100</end>
            <in>0</in>
            <out>100</out>
          </clipitem>
          <transitionitem>
            <start>90</start>
            <end>110</end>
            <effect>
              <name>Cross Dissolve</name>
              <effectid>Cross Dissolve</effectid>
            </effect>
          </transitionitem>
          <clipitem id="clipitem-2">
            <name>Incoming.mov</name>
            <start>100</start>
            <end>200</end>
            <in>0</in>
            <out>100</out>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;

    #[test]
    fn test_transition_attached_to_both_neighbouring_clips() {
        let sequences = XmemlParser::parse(TRANSITION_FIXTURE).expect("should parse");
        let clips = &sequences[0].clips;
        assert_eq!(clips.len(), 2, "the transitionitem itself is not a clip");

        let outgoing = clips
            .iter()
            .find(|c| c.name == "Outgoing.mov")
            .expect("outgoing clip present");
        let transition_out = outgoing
            .transition_out
            .as_ref()
            .expect("outgoing clip should have a transition_out");
        assert_eq!(transition_out.name, "Cross Dissolve");
        assert_eq!(transition_out.start_frames, Some(90));
        assert_eq!(transition_out.end_frames, Some(110));

        let incoming = clips
            .iter()
            .find(|c| c.name == "Incoming.mov")
            .expect("incoming clip present");
        let transition_in = incoming
            .transition_in
            .as_ref()
            .expect("incoming clip should have a transition_in");
        assert_eq!(transition_in.name, "Cross Dissolve");
    }

    #[test]
    fn test_nested_compound_clip_is_honest_err() {
        let xml = r#"<xmeml version="4">
  <sequence>
    <name>Compound Seq</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Compound_01</name>
            <start>0</start>
            <end>50</end>
            <sequence>
              <name>Nested Timeline</name>
            </sequence>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;
        let err = XmemlParser::parse(xml).expect_err("nested sequence must be an honest error");
        let message = err.to_string();
        assert!(
            message.contains("Compound_01") && message.contains("sequence"),
            "error should name the offending clip and mention the nested <sequence>: {message}"
        );
    }

    #[test]
    fn test_negative_one_sentinel_is_honest_err_not_zero() {
        let xml = r#"<xmeml version="4">
  <sequence>
    <name>Sentinel Seq</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>Touches_Transition.mov</name>
            <start>0</start>
            <end>-1</end>
            <in>0</in>
            <out>50</out>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;
        let err = XmemlParser::parse(xml).expect_err("-1 sentinel must be an honest error");
        let message = err.to_string();
        assert!(
            message.contains("Touches_Transition.mov"),
            "error must name the clip: {message}"
        );
        assert!(
            message.contains("<end>-1</end>"),
            "error must name the exact offending sentinel field/value, not a generic \
             failure: {message}"
        );
        assert!(
            !message.contains("<end>0</end>"),
            "must not describe this as though it silently resolved to a fabricated 0 \
             timecode: {message}"
        );
    }

    #[test]
    fn test_missing_required_field_is_honest_err() {
        let xml = r#"<xmeml version="4">
  <sequence>
    <name>Missing Field Seq</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="clipitem-1">
            <name>No_Out.mov</name>
            <start>0</start>
            <end>50</end>
            <in>0</in>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;
        let err = XmemlParser::parse(xml).expect_err("missing <out> must be an honest error");
        assert!(matches!(err, ConformError::UnsupportedFormat(_)));
    }

    #[test]
    fn test_mismatched_end_tag_is_honest_err() {
        let xml = "<xmeml version=\"4\"><sequence></notsequence></xmeml>";
        let result = XmemlParser::parse(xml);
        assert!(result.is_err(), "mismatched tags must not parse as Ok");
        assert!(matches!(
            result.expect_err("checked above"),
            ConformError::Xml(_)
        ));
    }

    #[test]
    fn test_truncated_at_eof_is_honest_err_not_silent_zero() {
        // A stream that ends mid-<sequence> reaches Eof without quick_xml
        // itself raising a well-formedness error (a *missing* end tag is not
        // a mismatched one) — so this parser must detect the still-open
        // element itself rather than letting the truncated clip/sequence
        // vanish as a silent `Ok(vec![])`.
        let xml = "<xmeml version=\"4\"><sequence><name>Broken</name>";
        let result = XmemlParser::parse(xml);
        assert!(
            result.is_err(),
            "a document truncated mid-element must not silently report zero sequences"
        );
        assert!(matches!(
            result.expect_err("checked above"),
            ConformError::UnsupportedFormat(_)
        ));
    }

    #[test]
    fn test_empty_xmeml_is_ok_zero_sequences() {
        // Consistent with FcpxmlParser::parse("<fcpxml/>") — a well-formed
        // but empty document is a real (if empty) result, not an error.
        let sequences = XmemlParser::parse("<xmeml version=\"4\"/>").expect("should parse");
        assert!(sequences.is_empty());
    }

    #[test]
    fn test_multiple_top_level_sequences_nested_in_project_bin() {
        let xml = r#"<xmeml version="4">
  <project>
    <children>
      <bin>
        <children>
          <sequence>
            <name>Seq One</name>
            <rate><timebase>24</timebase><ntsc>FALSE</ntsc></rate>
            <media>
              <video>
                <track>
                  <clipitem id="c1">
                    <name>A.mov</name>
                    <start>0</start><end>24</end><in>0</in><out>24</out>
                  </clipitem>
                </track>
              </video>
            </media>
          </sequence>
          <sequence>
            <name>Seq Two</name>
            <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
            <media>
              <video>
                <track>
                  <clipitem id="c2">
                    <name>B.mov</name>
                    <start>0</start><end>25</end><in>0</in><out>25</out>
                  </clipitem>
                  <clipitem id="c3">
                    <name>C.mov</name>
                    <start>25</start><end>50</end><in>0</in><out>25</out>
                  </clipitem>
                </track>
              </video>
            </media>
          </sequence>
        </children>
      </bin>
    </children>
  </project>
</xmeml>"#;
        let sequences = XmemlParser::parse(xml).expect("should parse");
        assert_eq!(sequences.len(), 2);
        assert_eq!(sequences[0].name, "Seq One");
        assert_eq!(sequences[0].clips.len(), 1);
        assert_eq!(sequences[0].fps, FrameRate::Fps24);
        assert_eq!(sequences[1].name, "Seq Two");
        assert_eq!(sequences[1].clips.len(), 2);
        assert_eq!(sequences[1].fps, FrameRate::Fps25);
    }

    #[test]
    fn test_file_reference_dedup_via_self_closing_tag() {
        let xml = r#"<xmeml version="4">
  <sequence>
    <name>Dedup Seq</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="c1">
            <name>Reused.mov</name>
            <start>0</start><end>25</end><in>0</in><out>25</out>
            <file id="file-shared">
              <name>Reused.mov</name>
              <pathurl>file:///Volumes/Media/Reused.mov</pathurl>
            </file>
          </clipitem>
          <clipitem id="c2">
            <name>Reused.mov</name>
            <start>25</start><end>50</end><in>25</in><out>50</out>
            <file id="file-shared"/>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;
        let sequences = XmemlParser::parse(xml).expect("should parse");
        let clips = &sequences[0].clips;
        assert_eq!(clips.len(), 2);
        assert_eq!(
            clips[0].file_path.as_deref(),
            Some("/Volumes/Media/Reused.mov")
        );
        assert_eq!(
            clips[1].file_path.as_deref(),
            Some("/Volumes/Media/Reused.mov"),
            "self-closing <file id=.../> reference must resolve via the earlier full definition"
        );
    }

    #[test]
    fn test_mixed_rate_source_vs_sequence() {
        // A 24fps source clip cut into a 25fps sequence: <in>/<out> resolve
        // via the clip's own <rate>, <start>/<end> via the sequence's.
        let xml = r#"<xmeml version="4">
  <sequence>
    <name>Mixed Rate Seq</name>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <track>
          <clipitem id="c1">
            <name>24p_in_25p.mov</name>
            <rate><timebase>24</timebase><ntsc>FALSE</ntsc></rate>
            <start>0</start>
            <end>25</end>
            <in>0</in>
            <out>24</out>
          </clipitem>
        </track>
      </video>
    </media>
  </sequence>
</xmeml>"#;
        let sequences = XmemlParser::parse(xml).expect("should parse");
        let clip = &sequences[0].clips[0];
        assert_eq!(clip.source_fps, FrameRate::Fps24);
        assert_eq!(clip.record_fps, FrameRate::Fps25);
        // Both spans represent exactly 1 real second, at their own rates.
        assert_eq!(clip.source_out_frames - clip.source_in_frames, 24);
        assert_eq!(clip.record_out_frames - clip.record_in_frames, 25);
    }
}

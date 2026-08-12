//! Timeline editing CLI commands.
//!
//! Provides subcommands for creating, editing, and rendering timelines.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Timeline command subcommands.
#[derive(Subcommand, Debug)]
pub enum TimelineCommand {
    /// Create a new timeline project
    Create {
        /// Output timeline file path (.json)
        #[arg(short, long)]
        output: PathBuf,

        /// Frame rate (fps)
        #[arg(long, default_value = "24")]
        fps: f64,

        /// Video width in pixels
        #[arg(long, default_value = "1920")]
        width: u32,

        /// Video height in pixels
        #[arg(long, default_value = "1080")]
        height: u32,

        /// Timeline project name
        #[arg(long)]
        name: Option<String>,

        /// Number of video tracks to create
        #[arg(long, default_value = "1")]
        video_tracks: u32,

        /// Number of audio tracks to create
        #[arg(long, default_value = "1")]
        audio_tracks: u32,
    },

    /// Add a clip to the timeline
    AddClip {
        /// Timeline project file
        #[arg(short, long)]
        timeline: PathBuf,

        /// Input media file to add as clip
        #[arg(short, long)]
        input: PathBuf,

        /// Target track index (0-based)
        #[arg(long)]
        track: Option<u32>,

        /// Start time on the timeline in seconds
        #[arg(long)]
        start_time: Option<f64>,

        /// Clip duration in seconds
        #[arg(long)]
        duration: Option<f64>,

        /// Source in-point in seconds
        #[arg(long)]
        in_point: Option<f64>,

        /// Source out-point in seconds
        #[arg(long)]
        out_point: Option<f64>,

        /// Playback speed multiplier
        #[arg(long, default_value = "1.0")]
        speed: f64,
    },

    /// Remove a clip from the timeline
    RemoveClip {
        /// Timeline project file
        #[arg(short, long)]
        timeline: PathBuf,

        /// Clip ID to remove
        #[arg(long)]
        clip_id: u64,
    },

    /// Render the timeline to a media file
    Render {
        /// Timeline project file
        #[arg(short, long)]
        timeline: PathBuf,

        /// Output media file
        #[arg(short, long)]
        output: PathBuf,

        /// Video codec: av1, vp9, vp8
        #[arg(long)]
        codec: Option<String>,

        /// Quality (CRF value)
        #[arg(long)]
        quality: Option<u32>,

        /// Preview mode (lower quality, faster)
        #[arg(long)]
        preview: bool,

        /// Start rendering at this time (seconds)
        #[arg(long)]
        range_start: Option<f64>,

        /// End rendering at this time (seconds)
        #[arg(long)]
        range_end: Option<f64>,
    },

    /// Display timeline information
    Info {
        /// Timeline project file
        #[arg(short, long)]
        timeline: PathBuf,
    },

    /// Export timeline as EDL or XML
    Export {
        /// Timeline project file
        #[arg(short, long)]
        timeline: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Export format: edl, xml, fcpxml, otio
        #[arg(long, default_value = "edl")]
        format: String,
    },

    /// Import a timeline from EDL or XML
    Import {
        /// Input EDL/XML file
        #[arg(short, long)]
        input: PathBuf,

        /// Output timeline file
        #[arg(short, long)]
        output: PathBuf,

        /// Frame rate override
        #[arg(long)]
        fps: Option<f64>,
    },
}

/// Handle timeline command dispatch.
pub async fn handle_timeline_command(command: TimelineCommand, json_output: bool) -> Result<()> {
    match command {
        TimelineCommand::Create {
            output,
            fps,
            width,
            height,
            name,
            video_tracks,
            audio_tracks,
        } => {
            handle_create(
                &output,
                fps,
                width,
                height,
                name.as_deref(),
                video_tracks,
                audio_tracks,
                json_output,
            )
            .await
        }
        TimelineCommand::AddClip {
            timeline,
            input,
            track,
            start_time,
            duration,
            in_point,
            out_point,
            speed,
        } => {
            handle_add_clip(
                &timeline,
                &input,
                track,
                start_time,
                duration,
                in_point,
                out_point,
                speed,
                json_output,
            )
            .await
        }
        TimelineCommand::RemoveClip { timeline, clip_id } => {
            handle_remove_clip(&timeline, clip_id, json_output).await
        }
        TimelineCommand::Render {
            timeline,
            output,
            codec,
            quality,
            preview,
            range_start,
            range_end,
        } => {
            handle_render(
                &timeline,
                &output,
                codec.as_deref(),
                quality,
                preview,
                range_start,
                range_end,
                json_output,
            )
            .await
        }
        TimelineCommand::Info { timeline } => handle_info(&timeline, json_output).await,
        TimelineCommand::Export {
            timeline,
            output,
            format,
        } => handle_export(&timeline, &output, &format, json_output).await,
        TimelineCommand::Import { input, output, fps } => {
            handle_import(&input, &output, fps, json_output).await
        }
    }
}

// ---------------------------------------------------------------------------
// Internal timeline data model for serialization
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct TimelineProject {
    name: String,
    fps: f64,
    width: u32,
    height: u32,
    tracks: Vec<TrackData>,
    next_clip_id: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct TrackData {
    index: u32,
    track_type: String,
    name: String,
    muted: bool,
    locked: bool,
    clips: Vec<ClipData>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct ClipData {
    id: u64,
    source_path: String,
    start_time: f64,
    duration: f64,
    in_point: f64,
    out_point: f64,
    speed: f64,
}

impl TimelineProject {
    fn new(name: &str, fps: f64, width: u32, height: u32) -> Self {
        Self {
            name: name.to_string(),
            fps,
            width,
            height,
            tracks: Vec::new(),
            next_clip_id: 1,
        }
    }

    fn add_track(&mut self, track_type: &str) -> u32 {
        let index = self.tracks.len() as u32;
        self.tracks.push(TrackData {
            index,
            track_type: track_type.to_string(),
            name: format!("{} {}", track_type, index + 1),
            muted: false,
            locked: false,
            clips: Vec::new(),
        });
        index
    }

    fn duration_seconds(&self) -> f64 {
        let mut max_end = 0.0_f64;
        for track in &self.tracks {
            for clip in &track.clips {
                let end = clip.start_time + clip.duration;
                if end > max_end {
                    max_end = end;
                }
            }
        }
        max_end
    }

    fn clip_count(&self) -> usize {
        self.tracks.iter().map(|t| t.clips.len()).sum()
    }

    /// Add a clip to a track by index.  Returns the assigned clip id.
    /// Defaults: `in_point = 0.0`, `out_point = duration`, `speed = 1.0`.
    #[allow(dead_code)]
    fn add_clip(
        &mut self,
        track_index: u32,
        source_path: &str,
        start_time: f64,
        duration: f64,
    ) -> u64 {
        let id = self.next_clip_id;
        self.next_clip_id += 1;
        if let Some(track) = self.tracks.iter_mut().find(|t| t.index == track_index) {
            track.clips.push(ClipData {
                id,
                source_path: source_path.to_string(),
                start_time,
                duration,
                in_point: 0.0,
                out_point: duration,
                speed: 1.0,
            });
        }
        id
    }

    fn load(path: &PathBuf) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).context("Failed to read timeline project file")?;
        let project: Self =
            serde_json::from_str(&content).context("Failed to parse timeline project file")?;
        Ok(project)
    }

    fn save(&self, path: &PathBuf) -> Result<()> {
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize timeline project")?;
        std::fs::write(path, content).context("Failed to write timeline project file")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Handler: Create
// ---------------------------------------------------------------------------

async fn handle_create(
    output: &PathBuf,
    fps: f64,
    width: u32,
    height: u32,
    name: Option<&str>,
    video_tracks: u32,
    audio_tracks: u32,
    json_output: bool,
) -> Result<()> {
    if fps <= 0.0 {
        return Err(anyhow::anyhow!("Frame rate must be positive, got {}", fps));
    }
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!(
            "Width and height must be > 0, got {}x{}",
            width,
            height
        ));
    }

    let project_name = name.unwrap_or("Untitled");
    let mut project = TimelineProject::new(project_name, fps, width, height);

    for _ in 0..video_tracks {
        project.add_track("video");
    }
    for _ in 0..audio_tracks {
        project.add_track("audio");
    }

    project.save(output)?;

    if json_output {
        let result = serde_json::json!({
            "action": "create",
            "output": output.display().to_string(),
            "name": project_name,
            "fps": fps,
            "width": width,
            "height": height,
            "video_tracks": video_tracks,
            "audio_tracks": audio_tracks,
            "status": "created",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Timeline Created".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Name:", project_name);
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Frame rate:", fps);
        println!("{:20} {}x{}", "Resolution:", width, height);
        println!("{:20} {}", "Video tracks:", video_tracks);
        println!("{:20} {}", "Audio tracks:", audio_tracks);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: AddClip
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn handle_add_clip(
    timeline_path: &PathBuf,
    input: &PathBuf,
    track: Option<u32>,
    start_time: Option<f64>,
    duration: Option<f64>,
    in_point: Option<f64>,
    out_point: Option<f64>,
    speed: f64,
    json_output: bool,
) -> Result<()> {
    if !timeline_path.exists() {
        return Err(anyhow::anyhow!(
            "Timeline file not found: {}",
            timeline_path.display()
        ));
    }
    if !input.exists() {
        return Err(anyhow::anyhow!(
            "Input media file not found: {}",
            input.display()
        ));
    }
    if speed <= 0.0 {
        return Err(anyhow::anyhow!("Speed must be positive, got {}", speed));
    }

    let mut project = TimelineProject::load(timeline_path)?;

    let track_index = track.unwrap_or(0) as usize;
    if track_index >= project.tracks.len() {
        return Err(anyhow::anyhow!(
            "Track index {} out of range (0..{})",
            track_index,
            project.tracks.len()
        ));
    }

    let clip_start = start_time.unwrap_or_else(|| {
        // Place at end of last clip on this track
        project.tracks[track_index]
            .clips
            .last()
            .map_or(0.0, |c| c.start_time + c.duration)
    });

    let clip_in = in_point.unwrap_or(0.0);
    let clip_out = out_point.unwrap_or(0.0);
    let clip_duration = duration.unwrap_or_else(|| {
        if clip_out > clip_in {
            (clip_out - clip_in) / speed
        } else {
            10.0 // default 10 second clip
        }
    });

    let clip_id = project.next_clip_id;
    project.next_clip_id += 1;

    let clip = ClipData {
        id: clip_id,
        source_path: input.display().to_string(),
        start_time: clip_start,
        duration: clip_duration,
        in_point: clip_in,
        out_point: if clip_out > 0.0 {
            clip_out
        } else {
            clip_in + clip_duration * speed
        },
        speed,
    };

    project.tracks[track_index].clips.push(clip);
    // Sort clips by start time
    project.tracks[track_index].clips.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    project.save(timeline_path)?;

    if json_output {
        let result = serde_json::json!({
            "action": "add_clip",
            "clip_id": clip_id,
            "source": input.display().to_string(),
            "track": track_index,
            "start_time": clip_start,
            "duration": clip_duration,
            "in_point": clip_in,
            "speed": speed,
            "status": "added",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clip Added".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Clip ID:", clip_id);
        println!("{:20} {}", "Source:", input.display());
        println!("{:20} {}", "Track:", track_index);
        println!("{:20} {:.3}s", "Start time:", clip_start);
        println!("{:20} {:.3}s", "Duration:", clip_duration);
        println!("{:20} {:.3}s", "In point:", clip_in);
        println!("{:20} {}x", "Speed:", speed);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: RemoveClip
// ---------------------------------------------------------------------------

async fn handle_remove_clip(
    timeline_path: &PathBuf,
    clip_id: u64,
    json_output: bool,
) -> Result<()> {
    if !timeline_path.exists() {
        return Err(anyhow::anyhow!(
            "Timeline file not found: {}",
            timeline_path.display()
        ));
    }

    let mut project = TimelineProject::load(timeline_path)?;

    let mut found = false;
    for track in &mut project.tracks {
        if let Some(pos) = track.clips.iter().position(|c| c.id == clip_id) {
            track.clips.remove(pos);
            found = true;
            break;
        }
    }

    if !found {
        return Err(anyhow::anyhow!("Clip ID {} not found in timeline", clip_id));
    }

    project.save(timeline_path)?;

    if json_output {
        let result = serde_json::json!({
            "action": "remove_clip",
            "clip_id": clip_id,
            "status": "removed",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clip Removed".green().bold());
        println!("{:20} {}", "Clip ID:", clip_id);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Render
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn handle_render(
    timeline_path: &PathBuf,
    output: &PathBuf,
    codec: Option<&str>,
    quality: Option<u32>,
    preview: bool,
    range_start: Option<f64>,
    range_end: Option<f64>,
    json_output: bool,
) -> Result<()> {
    if !timeline_path.exists() {
        return Err(anyhow::anyhow!(
            "Timeline file not found: {}",
            timeline_path.display()
        ));
    }

    let project = TimelineProject::load(timeline_path)?;

    let selected_codec = codec.unwrap_or("av1");
    match selected_codec {
        "av1" | "vp9" | "vp8" => {}
        other => {
            return Err(anyhow::anyhow!(
                "Unsupported codec '{}'. Use av1, vp9, or vp8",
                other
            ));
        }
    }

    let render_quality = quality.unwrap_or(if preview { 45 } else { 28 });
    let render_start = range_start.unwrap_or(0.0);
    let render_end = range_end.unwrap_or(project.duration_seconds());

    if json_output {
        let result = serde_json::json!({
            "action": "render",
            "timeline": timeline_path.display().to_string(),
            "output": output.display().to_string(),
            "codec": selected_codec,
            "quality": render_quality,
            "preview": preview,
            "range_start": render_start,
            "range_end": render_end,
            "resolution": format!("{}x{}", project.width, project.height),
            "fps": project.fps,
            "clip_count": project.clip_count(),
            "track_count": project.tracks.len(),
            "status": "pending_render_pipeline",
            "message": "Timeline renderer initialized; awaiting frame decoding pipeline integration",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Timeline Render".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Timeline:", timeline_path.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Codec:", selected_codec);
        println!("{:20} CRF {}", "Quality:", render_quality);
        println!("{:20} {}", "Preview mode:", preview);
        println!("{:20} {}x{}", "Resolution:", project.width, project.height);
        println!("{:20} {}", "Frame rate:", project.fps);
        println!("{:20} {:.3}s - {:.3}s", "Range:", render_start, render_end);
        println!("{:20} {}", "Tracks:", project.tracks.len());
        println!("{:20} {}", "Clips:", project.clip_count());
        println!();
        println!(
            "{}",
            "Note: Render pipeline requires frame decoding integration.".yellow()
        );
        println!(
            "{}",
            "Timeline renderer and export settings are ready.".dimmed()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Info
// ---------------------------------------------------------------------------

async fn handle_info(timeline_path: &PathBuf, json_output: bool) -> Result<()> {
    if !timeline_path.exists() {
        return Err(anyhow::anyhow!(
            "Timeline file not found: {}",
            timeline_path.display()
        ));
    }

    let project = TimelineProject::load(timeline_path)?;

    if json_output {
        let result = serde_json::json!({
            "name": project.name,
            "fps": project.fps,
            "width": project.width,
            "height": project.height,
            "duration_seconds": project.duration_seconds(),
            "track_count": project.tracks.len(),
            "clip_count": project.clip_count(),
            "tracks": project.tracks.iter().map(|t| {
                serde_json::json!({
                    "index": t.index,
                    "type": t.track_type,
                    "name": t.name,
                    "muted": t.muted,
                    "locked": t.locked,
                    "clip_count": t.clips.len(),
                })
            }).collect::<Vec<_>>(),
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Timeline Information".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Name:", project.name);
        println!("{:20} {}", "Frame rate:", project.fps);
        println!("{:20} {}x{}", "Resolution:", project.width, project.height);
        println!("{:20} {:.3}s", "Duration:", project.duration_seconds());
        println!("{:20} {}", "Tracks:", project.tracks.len());
        println!("{:20} {}", "Total clips:", project.clip_count());
        println!();

        for track in &project.tracks {
            let track_label = format!(
                "Track {} ({}) - {}",
                track.index, track.track_type, track.name
            );
            println!("{}", track_label.cyan().bold());
            println!("{}", "-".repeat(60));
            println!(
                "  Muted: {}  Locked: {}  Clips: {}",
                track.muted,
                track.locked,
                track.clips.len()
            );

            for clip in &track.clips {
                println!(
                    "  [{}] {} @ {:.3}s ({:.3}s) speed={}x",
                    clip.id, clip.source_path, clip.start_time, clip.duration, clip.speed,
                );
            }
            println!();
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Export
// ---------------------------------------------------------------------------

async fn handle_export(
    timeline_path: &PathBuf,
    output: &PathBuf,
    format: &str,
    json_output: bool,
) -> Result<()> {
    if !timeline_path.exists() {
        return Err(anyhow::anyhow!(
            "Timeline file not found: {}",
            timeline_path.display()
        ));
    }

    let project = TimelineProject::load(timeline_path)?;

    match format {
        "edl" | "xml" | "fcpxml" | "otio" => {}
        other => {
            return Err(anyhow::anyhow!(
                "Unsupported export format '{}'. Use: edl, xml, fcpxml, otio",
                other
            ));
        }
    }

    // Generate export content based on format
    let export_content = match format {
        "edl" => generate_edl(&project),
        "xml" | "fcpxml" => generate_fcpxml(&project),
        _ => generate_otio_json(&project),
    };

    std::fs::write(output, &export_content).context("Failed to write export file")?;

    if json_output {
        let result = serde_json::json!({
            "action": "export",
            "timeline": timeline_path.display().to_string(),
            "output": output.display().to_string(),
            "format": format,
            "clip_count": project.clip_count(),
            "status": "exported",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Timeline Exported".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Timeline:", timeline_path.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Format:", format.to_uppercase());
        println!("{:20} {}", "Clips exported:", project.clip_count());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Import
// ---------------------------------------------------------------------------

async fn handle_import(
    input: &PathBuf,
    output: &PathBuf,
    fps: Option<f64>,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let frame_rate = fps.unwrap_or(24.0);
    if frame_rate <= 0.0 {
        return Err(anyhow::anyhow!(
            "Frame rate must be positive, got {}",
            frame_rate
        ));
    }

    let content = std::fs::read_to_string(input).context("Failed to read input file")?;

    let mut project = TimelineProject::new("Imported Timeline", frame_rate, 1920, 1080);
    project.add_track("video");
    project.add_track("audio");

    // Basic EDL line parsing
    let mut clip_start = 0.0_f64;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("TITLE:") || trimmed.starts_with("FCM:") {
            continue;
        }
        // Try to parse simple EDL event lines (event# reel track transition in out src_in src_out)
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 8 {
            if let (Ok(_event_num), Some(reel)) = (parts[0].parse::<u32>(), parts.get(1)) {
                let clip_id = project.next_clip_id;
                project.next_clip_id += 1;
                let clip_duration = 10.0; // default
                let clip = ClipData {
                    id: clip_id,
                    source_path: reel.to_string(),
                    start_time: clip_start,
                    duration: clip_duration,
                    in_point: 0.0,
                    out_point: clip_duration,
                    speed: 1.0,
                };
                if let Some(track) = project.tracks.first_mut() {
                    track.clips.push(clip);
                }
                clip_start += clip_duration;
            }
        }
    }

    project.save(output)?;

    if json_output {
        let result = serde_json::json!({
            "action": "import",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "fps": frame_rate,
            "clips_imported": project.clip_count(),
            "status": "imported",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Timeline Imported".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Frame rate:", frame_rate);
        println!("{:20} {}", "Clips imported:", project.clip_count());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Export format generators
// ---------------------------------------------------------------------------

fn generate_edl(project: &TimelineProject) -> String {
    let mut edl = String::new();
    edl.push_str(&format!("TITLE: {}\n", project.name));
    edl.push_str("FCM: NON-DROP FRAME\n\n");

    let mut event_num = 1u32;
    for track in &project.tracks {
        for clip in &track.clips {
            let tc_in = seconds_to_tc(clip.in_point, project.fps);
            let tc_out = seconds_to_tc(clip.out_point, project.fps);
            let rec_in = seconds_to_tc(clip.start_time, project.fps);
            let rec_out = seconds_to_tc(clip.start_time + clip.duration, project.fps);

            edl.push_str(&format!(
                "{:03}  {}  V     C        {} {} {} {}\n",
                event_num,
                sanitize_reel_name(&clip.source_path),
                tc_in,
                tc_out,
                rec_in,
                rec_out,
            ));
            event_num += 1;
        }
    }

    edl
}

fn generate_fcpxml(project: &TimelineProject) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<!DOCTYPE fcpxml>\n");
    xml.push_str("<fcpxml version=\"1.9\">\n");
    xml.push_str("  <resources>\n");

    // Resources
    for track in &project.tracks {
        for clip in &track.clips {
            xml.push_str(&format!(
                "    <asset id=\"r{}\" name=\"{}\" src=\"file://{}\" />\n",
                clip.id,
                sanitize_reel_name(&clip.source_path),
                clip.source_path,
            ));
        }
    }

    xml.push_str("  </resources>\n");
    xml.push_str("  <library>\n");
    xml.push_str(&format!("    <event name=\"{}\">\n", project.name));
    xml.push_str(&format!("      <project name=\"{}\">\n", project.name));
    xml.push_str(&format!(
        "        <sequence format=\"r0\" duration=\"{:.3}s\" tcStart=\"0s\">\n",
        project.duration_seconds()
    ));
    xml.push_str("          <spine>\n");

    for track in &project.tracks {
        for clip in &track.clips {
            xml.push_str(&format!(
                "            <clip name=\"{}\" offset=\"{:.3}s\" duration=\"{:.3}s\" start=\"{:.3}s\">\n",
                sanitize_reel_name(&clip.source_path),
                clip.start_time,
                clip.duration,
                clip.in_point,
            ));
            xml.push_str(&format!(
                "              <asset-clip ref=\"r{}\" />\n",
                clip.id,
            ));
            xml.push_str("            </clip>\n");
        }
    }

    xml.push_str("          </spine>\n");
    xml.push_str("        </sequence>\n");
    xml.push_str("      </project>\n");
    xml.push_str("    </event>\n");
    xml.push_str("  </library>\n");
    xml.push_str("</fcpxml>\n");

    xml
}

/// Build a `RationalTime.1` JSON node from seconds and a frame rate.
fn otio_rational_time(seconds: f64, fps: f64) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "value": (seconds * fps).round(),
        "rate": fps,
    })
}

/// Build a `TimeRange.1` JSON node.
fn otio_time_range(start_secs: f64, duration_secs: f64, fps: f64) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": otio_rational_time(start_secs, fps),
        "duration": otio_rational_time(duration_secs, fps),
    })
}

/// Build an `ExternalReference.1` for a clip's source path.
fn otio_external_reference(source_path: &str, duration_secs: f64, fps: f64) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "ExternalReference.1",
        "target_url": source_path,
        "available_range": otio_time_range(0.0, duration_secs, fps),
        "available_image_bounds": serde_json::Value::Null,
        "metadata": {},
    })
}

/// Derive a human-readable clip name from a source path stem.
fn otio_clip_name(source_path: &str) -> &str {
    let path = std::path::Path::new(source_path);
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(source_path)
}

/// Build a `Clip.1` JSON node.
fn otio_clip(clip: &ClipData, fps: f64) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "Clip.1",
        "name": otio_clip_name(&clip.source_path),
        "source_range": otio_time_range(clip.in_point, clip.duration, fps),
        "media_reference": otio_external_reference(&clip.source_path, clip.duration, fps),
        "effects": [],
        "markers": [],
        "metadata": {},
        "enabled": true,
    })
}

/// Build a `Gap.1` JSON node for empty space with the given duration.
fn otio_gap(duration_secs: f64, fps: f64) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "Gap.1",
        "source_range": otio_time_range(0.0, duration_secs, fps),
        "effects": [],
        "markers": [],
        "metadata": {},
        "enabled": true,
    })
}

/// Serialize a `TimelineProject` to an OTIO 0.17-compatible JSON string.
fn generate_otio_json(project: &TimelineProject) -> String {
    let fps = project.fps;

    let track_kind = |track_type: &str| -> &str {
        match track_type {
            "audio" => "Audio",
            _ => "Video",
        }
    };

    let tracks: Vec<serde_json::Value> = project
        .tracks
        .iter()
        .map(|t| {
            // Sort clips by timeline start_time for correct gap detection.
            let mut sorted_clips = t.clips.clone();
            sorted_clips.sort_by(|a, b| {
                a.start_time
                    .partial_cmp(&b.start_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut children: Vec<serde_json::Value> = Vec::new();
            let mut cursor = 0.0_f64;

            for clip in &sorted_clips {
                let gap_secs = clip.start_time - cursor;
                if gap_secs > 1e-9 {
                    children.push(otio_gap(gap_secs, fps));
                }
                children.push(otio_clip(clip, fps));
                cursor = clip.start_time + clip.duration;
            }

            serde_json::json!({
                "OTIO_SCHEMA": "Track.1",
                "name": t.name,
                "kind": track_kind(&t.track_type),
                "children": children,
                "effects": [],
                "markers": [],
                "metadata": {},
                "enabled": true,
                "source_range": serde_json::Value::Null,
            })
        })
        .collect();

    let stack = serde_json::json!({
        "OTIO_SCHEMA": "Stack.1",
        "name": "tracks",
        "children": tracks,
        "effects": [],
        "markers": [],
        "metadata": {},
        "enabled": true,
        "source_range": serde_json::Value::Null,
    });

    let timeline = serde_json::json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": project.name,
        "global_start_time": otio_rational_time(0.0, fps),
        "tracks": stack,
        "metadata": {},
    });

    serde_json::to_string_pretty(&timeline).unwrap_or_else(|_| timeline.to_string())
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn seconds_to_tc(seconds: f64, fps: f64) -> String {
    let total_frames = (seconds * fps).round() as u64;
    let fps_int = fps.round() as u64;
    if fps_int == 0 {
        return "00:00:00:00".to_string();
    }
    let frames = total_frames % fps_int;
    let total_seconds = total_frames / fps_int;
    let secs = total_seconds % 60;
    let mins = (total_seconds / 60) % 60;
    let hours = total_seconds / 3600;
    format!("{:02}:{:02}:{:02}:{:02}", hours, mins, secs, frames)
}

fn sanitize_reel_name(path: &str) -> String {
    std::path::Path::new(path).file_stem().map_or_else(
        || "CLIP".to_string(),
        |s| {
            s.to_string_lossy()
                .chars()
                .take(8)
                .collect::<String>()
                .to_uppercase()
        },
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeline_project_new() {
        let project = TimelineProject::new("Test", 30.0, 1920, 1080);
        assert_eq!(project.name, "Test");
        assert!((project.fps - 30.0).abs() < f64::EPSILON);
        assert_eq!(project.width, 1920);
        assert_eq!(project.height, 1080);
        assert!(project.tracks.is_empty());
    }

    #[test]
    fn test_timeline_add_track() {
        let mut project = TimelineProject::new("Test", 24.0, 1920, 1080);
        let idx = project.add_track("video");
        assert_eq!(idx, 0);
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].track_type, "video");

        let idx2 = project.add_track("audio");
        assert_eq!(idx2, 1);
        assert_eq!(project.tracks.len(), 2);
    }

    #[test]
    fn test_timeline_duration() {
        let mut project = TimelineProject::new("Test", 30.0, 1920, 1080);
        project.add_track("video");

        assert!((project.duration_seconds() - 0.0).abs() < f64::EPSILON);

        project.tracks[0].clips.push(ClipData {
            id: 1,
            source_path: "test.mp4".to_string(),
            start_time: 0.0,
            duration: 5.0,
            in_point: 0.0,
            out_point: 5.0,
            speed: 1.0,
        });

        assert!((project.duration_seconds() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_timeline_clip_count() {
        let mut project = TimelineProject::new("Test", 24.0, 1920, 1080);
        project.add_track("video");
        project.add_track("audio");
        assert_eq!(project.clip_count(), 0);

        project.tracks[0].clips.push(ClipData {
            id: 1,
            source_path: "a.mp4".to_string(),
            start_time: 0.0,
            duration: 3.0,
            in_point: 0.0,
            out_point: 3.0,
            speed: 1.0,
        });
        project.tracks[1].clips.push(ClipData {
            id: 2,
            source_path: "b.wav".to_string(),
            start_time: 0.0,
            duration: 3.0,
            in_point: 0.0,
            out_point: 3.0,
            speed: 1.0,
        });
        assert_eq!(project.clip_count(), 2);
    }

    #[test]
    fn test_seconds_to_tc() {
        assert_eq!(seconds_to_tc(0.0, 24.0), "00:00:00:00");
        assert_eq!(seconds_to_tc(1.0, 24.0), "00:00:01:00");
        assert_eq!(seconds_to_tc(61.0, 30.0), "00:01:01:00");
        assert_eq!(seconds_to_tc(3661.0, 24.0), "01:01:01:00");
    }

    #[test]
    fn test_sanitize_reel_name() {
        assert_eq!(sanitize_reel_name("/path/to/my_clip.mp4"), "MY_CLIP");
        assert_eq!(sanitize_reel_name("test.wav"), "TEST");
        assert_eq!(
            sanitize_reel_name("very_long_filename_here.mp4"),
            "VERY_LON"
        );
    }

    #[test]
    fn test_timeline_save_load() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_timeline_cmd.json");

        let mut project = TimelineProject::new("Save Test", 30.0, 1920, 1080);
        project.add_track("video");
        project.tracks[0].clips.push(ClipData {
            id: 1,
            source_path: "clip.mp4".to_string(),
            start_time: 0.0,
            duration: 5.0,
            in_point: 0.0,
            out_point: 5.0,
            speed: 1.0,
        });
        project.save(&path).expect("save should succeed");

        let loaded = TimelineProject::load(&path).expect("load should succeed");
        assert_eq!(loaded.name, "Save Test");
        assert_eq!(loaded.tracks.len(), 1);
        assert_eq!(loaded.tracks[0].clips.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_generate_edl() {
        let mut project = TimelineProject::new("EDL Test", 24.0, 1920, 1080);
        project.add_track("video");
        project.tracks[0].clips.push(ClipData {
            id: 1,
            source_path: "scene01.mp4".to_string(),
            start_time: 0.0,
            duration: 5.0,
            in_point: 0.0,
            out_point: 5.0,
            speed: 1.0,
        });

        let edl = generate_edl(&project);
        assert!(edl.contains("TITLE: EDL Test"));
        assert!(edl.contains("SCENE01"));
    }

    #[test]
    fn test_otio_json_schema() {
        let mut project = TimelineProject::new("Test", 24.0, 1920, 1080);
        let vi = project.add_track("video");
        let ai = project.add_track("audio");
        project.add_clip(vi, "source.mp4", 0.0, 2.0);
        project.add_clip(vi, "b_roll.mp4", 3.0, 1.5); // gap at 2.0-3.0
        project.add_clip(ai, "dialogue.wav", 0.0, 3.5);

        let json = generate_otio_json(&project);
        let val: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

        assert_eq!(val["OTIO_SCHEMA"], "Timeline.1");
        assert_eq!(val["name"], "Test");
        assert_eq!(val["tracks"]["OTIO_SCHEMA"], "Stack.1");

        let children = &val["tracks"]["children"];
        assert_eq!(children[0]["OTIO_SCHEMA"], "Track.1");
        assert_eq!(children[0]["kind"], "Video");

        // Check RationalTime.1 on first clip's source_range
        let first_clip_start = &children[0]["children"][0]["source_range"]["start_time"];
        assert_eq!(first_clip_start["OTIO_SCHEMA"], "RationalTime.1");
        let rate = first_clip_start["rate"].as_f64().unwrap();
        assert!(
            (rate - 24.0).abs() < f64::EPSILON,
            "rate should be project fps"
        );

        // Check gap was inserted between clips
        let track_children = &children[0]["children"];
        let has_gap = track_children
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["OTIO_SCHEMA"] == "Gap.1");
        assert!(has_gap, "gap should be inserted between clips");
    }

    #[test]
    fn test_otio_json_audio_track_kind() {
        let mut project = TimelineProject::new("Audio", 48000.0, 0, 0);
        let ai = project.add_track("audio");
        project.add_clip(ai, "mono.wav", 0.0, 1.0);

        let json = generate_otio_json(&project);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(val["tracks"]["children"][0]["kind"], "Audio");
    }
}

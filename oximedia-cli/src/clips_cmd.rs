//! Clip extraction, management, and organization CLI commands.
//!
//! Provides commands for creating, listing, exporting, trimming, merging,
//! and tagging video clips for professional logging workflows.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Clips command subcommands.
#[derive(Subcommand, Debug)]
pub enum ClipsCommand {
    /// Create a new clip from a source media file
    Create {
        /// Source media file
        #[arg(short, long)]
        input: PathBuf,

        /// Clip name
        #[arg(short, long)]
        name: String,

        /// In-point timecode (e.g., 00:01:30:00)
        #[arg(long)]
        tc_in: Option<String>,

        /// Out-point timecode (e.g., 00:02:00:00)
        #[arg(long)]
        tc_out: Option<String>,

        /// Rating: 0-5 stars
        #[arg(long)]
        rating: Option<u8>,

        /// Comma-separated keywords
        #[arg(long)]
        keywords: Option<String>,

        /// Clip database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// List clips in the database
    List {
        /// Clip database path (JSON file)
        #[arg(long)]
        db: PathBuf,

        /// Filter by keyword
        #[arg(long)]
        keyword: Option<String>,

        /// Filter by minimum rating (0-5)
        #[arg(long)]
        min_rating: Option<u8>,

        /// Sort by: name, rating, date
        #[arg(long, default_value = "name")]
        sort: String,

        /// Maximum results
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Export clips to a file
    Export {
        /// Clip database path (JSON file)
        #[arg(long)]
        db: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Export format: json, csv, edl
        #[arg(long, default_value = "json")]
        format: String,

        /// Filter by keyword
        #[arg(long)]
        keyword: Option<String>,
    },

    /// Trim an existing clip's in/out points
    Trim {
        /// Clip ID to trim
        #[arg(short, long)]
        clip_id: String,

        /// New in-point timecode
        #[arg(long)]
        tc_in: Option<String>,

        /// New out-point timecode
        #[arg(long)]
        tc_out: Option<String>,

        /// Clip database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Merge multiple clips into a single sequence
    Merge {
        /// Comma-separated clip IDs to merge
        #[arg(short, long)]
        clip_ids: String,

        /// Merged clip name
        #[arg(short, long)]
        name: String,

        /// Clip database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Add or remove tags on a clip
    Tag {
        /// Clip ID
        #[arg(short, long)]
        clip_id: String,

        /// Comma-separated keywords to add
        #[arg(long)]
        add: Option<String>,

        /// Comma-separated keywords to remove
        #[arg(long)]
        remove: Option<String>,

        /// Set rating (0-5)
        #[arg(long)]
        rating: Option<u8>,

        /// Clip database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClipRecord {
    id: String,
    name: String,
    source_path: String,
    tc_in: Option<String>,
    tc_out: Option<String>,
    rating: u8,
    keywords: Vec<String>,
    created_at: String,
    notes: String,
    merged_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ClipDb {
    version: u32,
    clips: Vec<ClipRecord>,
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

fn load_db(path: &PathBuf) -> Result<ClipDb> {
    if !path.exists() {
        return Ok(ClipDb {
            version: 1,
            ..ClipDb::default()
        });
    }
    let data = std::fs::read_to_string(path).context("Failed to read clip database")?;
    let db: ClipDb = serde_json::from_str(&data).context("Failed to parse clip database")?;
    Ok(db)
}

fn save_db(path: &PathBuf, db: &ClipDb) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }
    }
    let data = serde_json::to_string_pretty(db).context("Failed to serialize clip database")?;
    std::fs::write(path, data).context("Failed to write clip database")?;
    Ok(())
}

fn generate_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("clip-{:016x}", now.as_nanos())
}

fn now_timestamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

fn parse_keywords(kw: &Option<String>) -> Vec<String> {
    kw.as_ref()
        .map(|k| {
            k.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle clips command dispatch.
pub async fn handle_clips_command(command: ClipsCommand, json_output: bool) -> Result<()> {
    match command {
        ClipsCommand::Create {
            input,
            name,
            tc_in,
            tc_out,
            rating,
            keywords,
            db,
        } => {
            run_create(
                &input,
                &name,
                &tc_in,
                &tc_out,
                rating,
                &keywords,
                &db,
                json_output,
            )
            .await
        }
        ClipsCommand::List {
            db,
            keyword,
            min_rating,
            sort,
            limit,
        } => run_list(&db, &keyword, min_rating, &sort, limit, json_output).await,
        ClipsCommand::Export {
            db,
            output,
            format,
            keyword,
        } => run_export(&db, &output, &format, &keyword, json_output).await,
        ClipsCommand::Trim {
            clip_id,
            tc_in,
            tc_out,
            db,
        } => run_trim(&clip_id, &tc_in, &tc_out, &db, json_output).await,
        ClipsCommand::Merge { clip_ids, name, db } => {
            run_merge(&clip_ids, &name, &db, json_output).await
        }
        ClipsCommand::Tag {
            clip_id,
            add,
            remove,
            rating,
            db,
        } => run_tag(&clip_id, &add, &remove, rating, &db, json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

async fn run_create(
    input: &PathBuf,
    name: &str,
    tc_in: &Option<String>,
    tc_out: &Option<String>,
    rating: Option<u8>,
    keywords: &Option<String>,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    if !input.exists() {
        return Err(anyhow::anyhow!(
            "Source file not found: {}",
            input.display()
        ));
    }

    let clip_id = generate_id();
    let kw = parse_keywords(keywords);
    let r = rating.unwrap_or(0).min(5);

    let clip = ClipRecord {
        id: clip_id.clone(),
        name: name.to_string(),
        source_path: input.to_string_lossy().to_string(),
        tc_in: tc_in.clone(),
        tc_out: tc_out.clone(),
        rating: r,
        keywords: kw.clone(),
        created_at: now_timestamp(),
        notes: String::new(),
        merged_from: Vec::new(),
    };

    db.clips.push(clip);
    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "clips_create",
            "clip_id": clip_id,
            "name": name,
            "source": input.display().to_string(),
            "tc_in": tc_in,
            "tc_out": tc_out,
            "rating": r,
            "keywords": kw,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clip Created".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Clip ID:", clip_id);
        println!("{:20} {}", "Name:", name);
        println!("{:20} {}", "Source:", input.display());
        if let Some(ref tc) = tc_in {
            println!("{:20} {}", "TC In:", tc);
        }
        if let Some(ref tc) = tc_out {
            println!("{:20} {}", "TC Out:", tc);
        }
        println!("{:20} {}", "Rating:", r);
        if !kw.is_empty() {
            println!("{:20} {}", "Keywords:", kw.join(", "));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

async fn run_list(
    db_path: &PathBuf,
    keyword: &Option<String>,
    min_rating: Option<u8>,
    sort: &str,
    limit: Option<u32>,
    json_output: bool,
) -> Result<()> {
    let db = load_db(db_path)?;
    let max_results = limit.unwrap_or(100) as usize;

    let mut clips: Vec<&ClipRecord> = db
        .clips
        .iter()
        .filter(|c| {
            let kw_ok = keyword.as_ref().map_or(true, |kw| {
                let kwl = kw.to_lowercase();
                c.keywords.iter().any(|k| k.to_lowercase().contains(&kwl))
                    || c.name.to_lowercase().contains(&kwl)
            });
            let rating_ok = min_rating.map_or(true, |mr| c.rating >= mr);
            kw_ok && rating_ok
        })
        .collect();

    match sort {
        "name" => clips.sort_by(|a, b| a.name.cmp(&b.name)),
        "rating" => clips.sort_by(|a, b| b.rating.cmp(&a.rating)),
        "date" => clips.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        _ => {}
    }

    let total = clips.len();
    clips.truncate(max_results);

    if json_output {
        let result = serde_json::json!({
            "command": "clips_list",
            "total": total,
            "returned": clips.len(),
            "clips": clips.iter().map(|c| serde_json::json!({
                "id": c.id,
                "name": c.name,
                "source": c.source_path,
                "tc_in": c.tc_in,
                "tc_out": c.tc_out,
                "rating": c.rating,
                "keywords": c.keywords,
            })).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else {
        println!("{}", "Clip List".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {} (showing {})", "Total clips:", total, clips.len());
        println!();
        for c in &clips {
            let stars = "*".repeat(c.rating as usize);
            println!(
                "  {} {} [{}] {}",
                ">".cyan(),
                c.name,
                stars,
                c.source_path.dimmed()
            );
            if let (Some(ref ti), Some(ref to)) = (&c.tc_in, &c.tc_out) {
                println!("    Range: {} - {}", ti, to);
            }
            if !c.keywords.is_empty() {
                println!("    Keywords: {}", c.keywords.join(", ").dimmed());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

async fn run_export(
    db_path: &PathBuf,
    output: &PathBuf,
    format: &str,
    keyword: &Option<String>,
    json_output: bool,
) -> Result<()> {
    let db = load_db(db_path)?;

    let clips: Vec<&ClipRecord> = db
        .clips
        .iter()
        .filter(|c| {
            keyword.as_ref().map_or(true, |kw| {
                let kwl = kw.to_lowercase();
                c.keywords.iter().any(|k| k.to_lowercase().contains(&kwl))
            })
        })
        .collect();

    let export_data = match format {
        "json" => {
            let data: Vec<serde_json::Value> = clips
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "name": c.name,
                        "source": c.source_path,
                        "tc_in": c.tc_in,
                        "tc_out": c.tc_out,
                        "rating": c.rating,
                        "keywords": c.keywords,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&data).context("Failed to serialize export")?
        }
        "csv" => {
            let mut csv = String::from("id,name,source,tc_in,tc_out,rating,keywords\n");
            for c in &clips {
                csv.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    c.id,
                    c.name.replace(',', ";"),
                    c.source_path.replace(',', ";"),
                    c.tc_in.as_deref().unwrap_or(""),
                    c.tc_out.as_deref().unwrap_or(""),
                    c.rating,
                    c.keywords.join(";")
                ));
            }
            csv
        }
        "edl" => {
            let mut edl = String::from("TITLE: OxiMedia Clip Export\n\n");
            for (i, c) in clips.iter().enumerate() {
                let tc_in = c.tc_in.as_deref().unwrap_or("00:00:00:00");
                let tc_out = c.tc_out.as_deref().unwrap_or("00:00:00:00");
                edl.push_str(&format!(
                    "{:03}  {} V     C        {} {} {} {}\n",
                    i + 1,
                    c.name.replace(' ', "_"),
                    tc_in,
                    tc_out,
                    tc_in,
                    tc_out
                ));
            }
            edl
        }
        _ => return Err(anyhow::anyhow!("Unsupported export format: {format}")),
    };

    if let Some(parent) = output.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed to create output directory")?;
        }
    }
    std::fs::write(output, &export_data).context("Failed to write export file")?;

    if json_output {
        let result = serde_json::json!({
            "command": "clips_export",
            "output": output.display().to_string(),
            "format": format,
            "clip_count": clips.len(),
            "size_bytes": export_data.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clips Exported".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Format:", format);
        println!("{:20} {}", "Clips:", clips.len());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Trim
// ---------------------------------------------------------------------------

async fn run_trim(
    clip_id: &str,
    tc_in: &Option<String>,
    tc_out: &Option<String>,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    let clip = db
        .clips
        .iter_mut()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| anyhow::anyhow!("Clip not found: {clip_id}"))?;

    if let Some(ref tc) = tc_in {
        clip.tc_in = Some(tc.clone());
    }
    if let Some(ref tc) = tc_out {
        clip.tc_out = Some(tc.clone());
    }

    let saved_tc_in = clip.tc_in.clone();
    let saved_tc_out = clip.tc_out.clone();

    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "clips_trim",
            "clip_id": clip_id,
            "tc_in": saved_tc_in,
            "tc_out": saved_tc_out,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clip Trimmed".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Clip ID:", clip_id);
        if let Some(ref tc) = saved_tc_in {
            println!("{:20} {}", "TC In:", tc);
        }
        if let Some(ref tc) = saved_tc_out {
            println!("{:20} {}", "TC Out:", tc);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

async fn run_merge(
    clip_ids_str: &str,
    name: &str,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;
    let ids: Vec<String> = clip_ids_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if ids.len() < 2 {
        return Err(anyhow::anyhow!("Need at least 2 clip IDs to merge"));
    }

    // Verify all clips exist and collect source
    let mut source_path = String::new();
    for id in &ids {
        let clip = db
            .clips
            .iter()
            .find(|c| c.id == *id)
            .ok_or_else(|| anyhow::anyhow!("Clip not found: {id}"))?;
        if source_path.is_empty() {
            source_path.clone_from(&clip.source_path);
        }
    }

    let merged_id = generate_id();
    let merged = ClipRecord {
        id: merged_id.clone(),
        name: name.to_string(),
        source_path,
        tc_in: None,
        tc_out: None,
        rating: 0,
        keywords: vec!["merged".to_string()],
        created_at: now_timestamp(),
        notes: format!("Merged from: {}", ids.join(", ")),
        merged_from: ids.clone(),
    };

    db.clips.push(merged);
    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "clips_merge",
            "merged_id": merged_id,
            "name": name,
            "source_clips": ids,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clips Merged".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Merged ID:", merged_id);
        println!("{:20} {}", "Name:", name);
        println!("{:20} {}", "Source clips:", ids.join(", "));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tag
// ---------------------------------------------------------------------------

async fn run_tag(
    clip_id: &str,
    add: &Option<String>,
    remove: &Option<String>,
    rating: Option<u8>,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    let clip = db
        .clips
        .iter_mut()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| anyhow::anyhow!("Clip not found: {clip_id}"))?;

    let to_add = parse_keywords(add);
    let to_remove = parse_keywords(remove);

    for kw in &to_add {
        if !clip.keywords.contains(kw) {
            clip.keywords.push(kw.clone());
        }
    }
    clip.keywords.retain(|k| !to_remove.contains(k));

    if let Some(r) = rating {
        clip.rating = r.min(5);
    }

    let saved_keywords = clip.keywords.clone();
    let saved_rating = clip.rating;

    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "clips_tag",
            "clip_id": clip_id,
            "keywords": saved_keywords,
            "rating": saved_rating,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Clip Tagged".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Clip ID:", clip_id);
        println!("{:20} {}", "Keywords:", saved_keywords.join(", "));
        println!("{:20} {}", "Rating:", saved_rating);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_id() {
        let id = generate_id();
        assert!(id.starts_with("clip-"));
    }

    #[test]
    fn test_parse_keywords() {
        let kw = Some("interview, raw, john-doe".to_string());
        let result = parse_keywords(&kw);
        assert_eq!(result, vec!["interview", "raw", "john-doe"]);
    }

    #[test]
    fn test_parse_keywords_none() {
        let result = parse_keywords(&None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_clip_record_serialization() {
        let source_path = std::env::temp_dir().join("video.mov").display().to_string();
        let clip = ClipRecord {
            id: "clip-001".to_string(),
            name: "Take 1".to_string(),
            source_path,
            tc_in: Some("01:00:00:00".to_string()),
            tc_out: Some("01:00:30:00".to_string()),
            rating: 4,
            keywords: vec!["interview".to_string()],
            created_at: "12345".to_string(),
            notes: String::new(),
            merged_from: Vec::new(),
        };
        let json = serde_json::to_string(&clip);
        assert!(json.is_ok());
        let parsed: Result<ClipRecord, _> = serde_json::from_str(&json.expect("should serialize"));
        assert!(parsed.is_ok());
        let p = parsed.expect("should deserialize");
        assert_eq!(p.rating, 4);
    }

    #[test]
    fn test_db_roundtrip() {
        let source_path = std::env::temp_dir().join("a.mov").display().to_string();
        let db = ClipDb {
            version: 1,
            clips: vec![ClipRecord {
                id: "c1".to_string(),
                name: "Test".to_string(),
                source_path,
                tc_in: None,
                tc_out: None,
                rating: 3,
                keywords: vec!["raw".to_string()],
                created_at: "0".to_string(),
                notes: String::new(),
                merged_from: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&db).expect("serialize");
        let parsed: ClipDb = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.clips.len(), 1);
    }
}

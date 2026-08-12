//! Review and approval workflow CLI commands.
//!
//! Provides commands for creating review sessions, adding annotations,
//! approving/rejecting content, exporting review data, and checking status.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Review command subcommands.
#[derive(Subcommand, Debug)]
pub enum ReviewCommand {
    /// Create a new review session
    Create {
        /// Title for the review session
        #[arg(short, long)]
        title: String,

        /// Content ID or path being reviewed
        #[arg(short, long)]
        content: String,

        /// Workflow type: simple, multi-stage, parallel, sequential
        #[arg(long, default_value = "simple")]
        workflow: String,

        /// Description
        #[arg(long)]
        description: Option<String>,

        /// Review database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Add an annotation/comment to a review session
    Annotate {
        /// Session ID
        #[arg(short, long)]
        session_id: String,

        /// Author name
        #[arg(long)]
        author: String,

        /// Annotation message
        #[arg(short, long)]
        message: String,

        /// Annotation type: general, issue, suggestion, question
        #[arg(long, default_value = "general")]
        annotation_type: String,

        /// Frame number to attach to
        #[arg(long)]
        frame: Option<u64>,

        /// Timecode to attach to
        #[arg(long)]
        timecode: Option<String>,

        /// Review database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Approve content in a review session
    Approve {
        /// Session ID
        #[arg(short, long)]
        session_id: String,

        /// Approver name
        #[arg(long)]
        approver: String,

        /// Optional approval note
        #[arg(long)]
        note: Option<String>,

        /// Review database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Reject content in a review session
    Reject {
        /// Session ID
        #[arg(short, long)]
        session_id: String,

        /// Reviewer name
        #[arg(long)]
        reviewer: String,

        /// Rejection reason
        #[arg(short, long)]
        reason: String,

        /// Review database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Export review data
    Export {
        /// Session ID
        #[arg(short, long)]
        session_id: String,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Export format: json, csv, pdf
        #[arg(long, default_value = "json")]
        format: String,

        /// Include annotations
        #[arg(long)]
        include_annotations: bool,

        /// Review database path (JSON file)
        #[arg(long)]
        db: PathBuf,
    },

    /// Show status of review sessions
    Status {
        /// Session ID (if omitted, shows all)
        #[arg(short, long)]
        session_id: Option<String>,

        /// Review database path (JSON file)
        #[arg(long)]
        db: PathBuf,

        /// Show detailed status
        #[arg(long)]
        detailed: bool,
    },
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReviewRecord {
    id: String,
    title: String,
    content_id: String,
    workflow: String,
    description: String,
    status: String,
    created_at: String,
    annotations: Vec<AnnotationRecord>,
    approvals: Vec<ApprovalRecord>,
    rejections: Vec<RejectionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnnotationRecord {
    id: String,
    author: String,
    message: String,
    annotation_type: String,
    frame: Option<u64>,
    timecode: Option<String>,
    created_at: String,
    resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApprovalRecord {
    approver: String,
    note: String,
    approved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RejectionRecord {
    reviewer: String,
    reason: String,
    rejected_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ReviewDb {
    version: u32,
    sessions: Vec<ReviewRecord>,
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

fn load_db(path: &PathBuf) -> Result<ReviewDb> {
    if !path.exists() {
        return Ok(ReviewDb {
            version: 1,
            ..ReviewDb::default()
        });
    }
    let data = std::fs::read_to_string(path).context("Failed to read review database")?;
    let db: ReviewDb = serde_json::from_str(&data).context("Failed to parse review database")?;
    Ok(db)
}

fn save_db(path: &PathBuf, db: &ReviewDb) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }
    }
    let data = serde_json::to_string_pretty(db).context("Failed to serialize review database")?;
    std::fs::write(path, data).context("Failed to write review database")?;
    Ok(())
}

fn generate_id(prefix: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{prefix}-{:016x}", now.as_nanos())
}

fn now_timestamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle review command dispatch.
pub async fn handle_review_command(command: ReviewCommand, json_output: bool) -> Result<()> {
    match command {
        ReviewCommand::Create {
            title,
            content,
            workflow,
            description,
            db,
        } => run_create(&title, &content, &workflow, &description, &db, json_output).await,
        ReviewCommand::Annotate {
            session_id,
            author,
            message,
            annotation_type,
            frame,
            timecode,
            db,
        } => {
            run_annotate(
                &session_id,
                &author,
                &message,
                &annotation_type,
                frame,
                &timecode,
                &db,
                json_output,
            )
            .await
        }
        ReviewCommand::Approve {
            session_id,
            approver,
            note,
            db,
        } => run_approve(&session_id, &approver, &note, &db, json_output).await,
        ReviewCommand::Reject {
            session_id,
            reviewer,
            reason,
            db,
        } => run_reject(&session_id, &reviewer, &reason, &db, json_output).await,
        ReviewCommand::Export {
            session_id,
            output,
            format,
            include_annotations,
            db,
        } => {
            run_export(
                &session_id,
                &output,
                &format,
                include_annotations,
                &db,
                json_output,
            )
            .await
        }
        ReviewCommand::Status {
            session_id,
            db,
            detailed,
        } => run_status(&session_id, &db, detailed, json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

async fn run_create(
    title: &str,
    content: &str,
    workflow: &str,
    description: &Option<String>,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    let valid_workflow = match workflow {
        "simple" | "multi-stage" | "parallel" | "sequential" => workflow.to_string(),
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid workflow: {workflow}. Use simple, multi-stage, parallel, or sequential"
            ))
        }
    };

    let session_id = generate_id("review");
    let session = ReviewRecord {
        id: session_id.clone(),
        title: title.to_string(),
        content_id: content.to_string(),
        workflow: valid_workflow.clone(),
        description: description.clone().unwrap_or_default(),
        status: "pending".to_string(),
        created_at: now_timestamp(),
        annotations: Vec::new(),
        approvals: Vec::new(),
        rejections: Vec::new(),
    };

    db.sessions.push(session);
    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "review_create",
            "session_id": session_id,
            "title": title,
            "content_id": content,
            "workflow": valid_workflow,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Review Session Created".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Session ID:", session_id);
        println!("{:20} {}", "Title:", title);
        println!("{:20} {}", "Content:", content);
        println!("{:20} {}", "Workflow:", valid_workflow);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Annotate
// ---------------------------------------------------------------------------

async fn run_annotate(
    session_id: &str,
    author: &str,
    message: &str,
    annotation_type: &str,
    frame: Option<u64>,
    timecode: &Option<String>,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    let session = db
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

    let valid_type = match annotation_type {
        "general" | "issue" | "suggestion" | "question" => annotation_type.to_string(),
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid annotation type: {annotation_type}. Use general, issue, suggestion, or question"
            ))
        }
    };

    let ann_id = generate_id("ann");
    session.annotations.push(AnnotationRecord {
        id: ann_id.clone(),
        author: author.to_string(),
        message: message.to_string(),
        annotation_type: valid_type.clone(),
        frame,
        timecode: timecode.clone(),
        created_at: now_timestamp(),
        resolved: false,
    });

    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "review_annotate",
            "annotation_id": ann_id,
            "session_id": session_id,
            "author": author,
            "type": valid_type,
            "frame": frame,
            "timecode": timecode,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Annotation Added".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Annotation ID:", ann_id);
        println!("{:20} {}", "Session ID:", session_id);
        println!("{:20} {}", "Author:", author);
        println!("{:20} {}", "Type:", valid_type);
        if let Some(f) = frame {
            println!("{:20} {}", "Frame:", f);
        }
        if let Some(ref tc) = timecode {
            println!("{:20} {}", "Timecode:", tc);
        }
        println!("{:20} {}", "Message:", message);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Approve
// ---------------------------------------------------------------------------

async fn run_approve(
    session_id: &str,
    approver: &str,
    note: &Option<String>,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    let session = db
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

    session.approvals.push(ApprovalRecord {
        approver: approver.to_string(),
        note: note.clone().unwrap_or_default(),
        approved_at: now_timestamp(),
    });

    // Update status based on workflow
    let open_issues = session
        .annotations
        .iter()
        .filter(|a| a.annotation_type == "issue" && !a.resolved)
        .count();
    if open_issues == 0 {
        session.status = "approved".to_string();
    } else {
        session.status = "conditionally_approved".to_string();
    }

    let saved_status = session.status.clone();

    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "review_approve",
            "session_id": session_id,
            "approver": approver,
            "status": saved_status,
            "open_issues": open_issues,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Content Approved".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Session ID:", session_id);
        println!("{:20} {}", "Approver:", approver);
        println!("{:20} {}", "Status:", saved_status);
        if open_issues > 0 {
            println!("  {} {} open issue(s) remain", "!".yellow(), open_issues);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Reject
// ---------------------------------------------------------------------------

async fn run_reject(
    session_id: &str,
    reviewer: &str,
    reason: &str,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let mut db = load_db(db_path)?;

    let session = db
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

    session.rejections.push(RejectionRecord {
        reviewer: reviewer.to_string(),
        reason: reason.to_string(),
        rejected_at: now_timestamp(),
    });

    session.status = "rejected".to_string();
    save_db(db_path, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "review_reject",
            "session_id": session_id,
            "reviewer": reviewer,
            "reason": reason,
            "status": "rejected",
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Content Rejected".red().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Session ID:", session_id);
        println!("{:20} {}", "Reviewer:", reviewer);
        println!("{:20} {}", "Reason:", reason);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

async fn run_export(
    session_id: &str,
    output: &PathBuf,
    format: &str,
    include_annotations: bool,
    db_path: &PathBuf,
    json_output: bool,
) -> Result<()> {
    let db = load_db(db_path)?;

    let session = db
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

    let export_data = match format {
        "json" => {
            let mut data = serde_json::json!({
                "session_id": session.id,
                "title": session.title,
                "content_id": session.content_id,
                "workflow": session.workflow,
                "status": session.status,
                "approvals": session.approvals.len(),
                "rejections": session.rejections.len(),
            });
            if include_annotations {
                data["annotations"] = serde_json::json!(session
                    .annotations
                    .iter()
                    .map(|a| {
                        serde_json::json!({
                            "id": a.id,
                            "author": a.author,
                            "message": a.message,
                            "type": a.annotation_type,
                            "frame": a.frame,
                            "timecode": a.timecode,
                            "resolved": a.resolved,
                        })
                    })
                    .collect::<Vec<_>>());
            }
            serde_json::to_string_pretty(&data).context("Failed to serialize")?
        }
        "csv" => {
            let mut csv = String::from("id,author,type,message,frame,timecode,resolved\n");
            if include_annotations {
                for a in &session.annotations {
                    csv.push_str(&format!(
                        "{},{},{},{},{},{},{}\n",
                        a.id,
                        a.author,
                        a.annotation_type,
                        a.message.replace(',', ";"),
                        a.frame.map_or(String::new(), |f| f.to_string()),
                        a.timecode.as_deref().unwrap_or(""),
                        a.resolved,
                    ));
                }
            }
            csv
        }
        _ => return Err(anyhow::anyhow!("Unsupported export format: {format}")),
    };

    if let Some(parent) = output.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed to create output directory")?;
        }
    }
    std::fs::write(output, &export_data).context("Failed to write export")?;

    if json_output {
        let result = serde_json::json!({
            "command": "review_export",
            "session_id": session_id,
            "output": output.display().to_string(),
            "format": format,
            "size_bytes": export_data.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Review Exported".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Session ID:", session_id);
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Format:", format);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

async fn run_status(
    session_id: &Option<String>,
    db_path: &PathBuf,
    detailed: bool,
    json_output: bool,
) -> Result<()> {
    let db = load_db(db_path)?;

    let sessions: Vec<&ReviewRecord> = if let Some(ref id) = session_id {
        db.sessions.iter().filter(|s| s.id == *id).collect()
    } else {
        db.sessions.iter().collect()
    };

    if sessions.is_empty() {
        if session_id.is_some() {
            return Err(anyhow::anyhow!("Session not found"));
        }
        if !json_output {
            println!("{}", "No review sessions".yellow());
        }
        return Ok(());
    }

    if json_output {
        let result = serde_json::json!({
            "command": "review_status",
            "sessions": sessions.iter().map(|s| {
                let mut entry = serde_json::json!({
                    "id": s.id,
                    "title": s.title,
                    "status": s.status,
                    "workflow": s.workflow,
                    "annotations": s.annotations.len(),
                    "approvals": s.approvals.len(),
                    "rejections": s.rejections.len(),
                });
                if detailed {
                    let open_issues = s.annotations.iter()
                        .filter(|a| a.annotation_type == "issue" && !a.resolved)
                        .count();
                    entry["open_issues"] = serde_json::json!(open_issues);
                }
                entry
            }).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else {
        println!("{}", "Review Status".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Total sessions:", sessions.len());
        println!();

        for s in &sessions {
            let status_color = match s.status.as_str() {
                "approved" => s.status.green(),
                "rejected" => s.status.red(),
                "pending" => s.status.yellow(),
                _ => s.status.normal(),
            };
            println!(
                "  {} {} [{}] - {}",
                ">".cyan(),
                s.title,
                status_color,
                s.content_id
            );
            println!(
                "    {} annotation(s), {} approval(s), {} rejection(s)",
                s.annotations.len(),
                s.approvals.len(),
                s.rejections.len()
            );
            if detailed {
                let open_issues = s
                    .annotations
                    .iter()
                    .filter(|a| a.annotation_type == "issue" && !a.resolved)
                    .count();
                if open_issues > 0 {
                    println!("    {} {} open issue(s)", "!".yellow(), open_issues);
                }
                for a in &s.annotations {
                    let resolved = if a.resolved { " (resolved)" } else { "" };
                    println!(
                        "    {} [{}] {}: {}{}",
                        "-".dimmed(),
                        a.annotation_type,
                        a.author,
                        a.message,
                        resolved.dimmed()
                    );
                }
            }
        }
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
        let id = generate_id("review");
        assert!(id.starts_with("review-"));
    }

    #[test]
    fn test_review_record_serialization() {
        let record = ReviewRecord {
            id: "review-001".to_string(),
            title: "Final Cut Review".to_string(),
            content_id: "video-123".to_string(),
            workflow: "simple".to_string(),
            description: "Review the final cut".to_string(),
            status: "pending".to_string(),
            created_at: "12345".to_string(),
            annotations: Vec::new(),
            approvals: Vec::new(),
            rejections: Vec::new(),
        };
        let json = serde_json::to_string(&record);
        assert!(json.is_ok());
        let parsed: Result<ReviewRecord, _> =
            serde_json::from_str(&json.expect("should serialize"));
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_annotation_record() {
        let ann = AnnotationRecord {
            id: "ann-001".to_string(),
            author: "bob".to_string(),
            message: "Fix color grading at this frame".to_string(),
            annotation_type: "issue".to_string(),
            frame: Some(1500),
            timecode: Some("00:01:02:15".to_string()),
            created_at: "12345".to_string(),
            resolved: false,
        };
        let json = serde_json::to_string(&ann).expect("should serialize");
        assert!(json.contains("1500"));
        assert!(json.contains("issue"));
    }

    #[test]
    fn test_db_roundtrip() {
        let db = ReviewDb {
            version: 1,
            sessions: vec![ReviewRecord {
                id: "r1".to_string(),
                title: "Test".to_string(),
                content_id: "c1".to_string(),
                workflow: "simple".to_string(),
                description: String::new(),
                status: "pending".to_string(),
                created_at: "0".to_string(),
                annotations: vec![AnnotationRecord {
                    id: "a1".to_string(),
                    author: "alice".to_string(),
                    message: "Looks good".to_string(),
                    annotation_type: "general".to_string(),
                    frame: None,
                    timecode: None,
                    created_at: "0".to_string(),
                    resolved: false,
                }],
                approvals: Vec::new(),
                rejections: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&db).expect("serialize");
        let parsed: ReviewDb = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.sessions.len(), 1);
        assert_eq!(parsed.sessions[0].annotations.len(), 1);
    }

    #[test]
    fn test_approval_record() {
        let approval = ApprovalRecord {
            approver: "director".to_string(),
            note: "Ship it".to_string(),
            approved_at: "12345".to_string(),
        };
        let json = serde_json::to_string(&approval);
        assert!(json.is_ok());
    }
}

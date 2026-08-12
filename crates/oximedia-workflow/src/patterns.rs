//! Common workflow patterns and templates.
//!
//! # Executability
//!
//! These constructors build workflow *shapes*. Whether a given shape runs to
//! completion under [`crate::executor::DefaultTaskExecutor`] depends on what
//! [`crate::task_exec`] can really do with the caller's files today:
//!
//! - **Transcode** — every preset named here resolves onto an encoder that
//!   exists in this build (see [`crate::task_exec::transcode`]). Inputs must
//!   be a container `oximedia-transcode` demuxes.
//! - **Analysis** — decodes **WAV** and **Y4M** only. A pattern that analyses
//!   an MP4/MKV master (e.g. [`archive_workflow`], [`distribution_workflow`])
//!   fails that step with an honest "cannot decode" error until the crate
//!   grows an A/V demux path; it does not report a fabricated success.
//! - **QualityControl** — needs a container `oximedia-qc` genuinely probes
//!   (ISO-BMFF, Matroska/WebM, AVI, WAV, FLAC, Ogg, MXF).
//! - **Transfer** to a remote protocol and **Notification** delivery are
//!   simulated by the default executor; supply a custom
//!   [`crate::executor::TaskExecutor`] for those.

use crate::task::{Task, TaskPriority, TaskType};
use crate::workflow::Workflow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Watch folder auto-transcode pattern.
///
/// Automatically transcodes files that arrive in a watched folder.
#[allow(dead_code)]
#[must_use]
pub fn watch_folder_transcode(
    input_folder: PathBuf,
    output_folder: PathBuf,
    preset: String,
) -> Workflow {
    let mut workflow = Workflow::new("watch-folder-transcode")
        .with_description("Auto-transcode files from watch folder");

    // This would be integrated with file watcher in practice
    let transcode_task = Task::new(
        "transcode",
        TaskType::Transcode {
            input: input_folder.join("input.mp4"),
            output: output_folder.join("output.mp4"),
            preset,
            params: HashMap::new(),
        },
    )
    .with_priority(TaskPriority::High);

    workflow.add_task(transcode_task);
    workflow
}

/// Multi-pass encoding workflow.
///
/// Source → Low-res proxy → QC → High-res → Delivery
#[allow(dead_code)]
#[must_use]
pub fn multi_pass_encoding(
    source: PathBuf,
    proxy_output: PathBuf,
    final_output: PathBuf,
    qc_profile: String,
) -> Workflow {
    let mut workflow = Workflow::new("multi-pass-encoding")
        .with_description("Multi-pass encoding with QC validation");

    // Step 1: Create low-res proxy
    let proxy_task = Task::new(
        "create-proxy",
        TaskType::Transcode {
            input: source.clone(),
            output: proxy_output.clone(),
            preset: "proxy-lowres".to_string(),
            params: HashMap::new(),
        },
    )
    .with_priority(TaskPriority::High);

    let proxy_id = workflow.add_task(proxy_task);

    // Step 2: QC validation on proxy
    let qc_task = Task::new(
        "qc-validation",
        TaskType::QualityControl {
            input: proxy_output.clone(),
            profile: qc_profile,
            // Left empty on purpose: `rules` means "these checks MUST have
            // run", and the caller-supplied profile decides which rules are
            // installed *and* which of them apply to the file. Demanding a
            // specific rule here would fail QC for reasons unrelated to the
            // media. See `task_exec::qc`.
            rules: Vec::new(),
        },
    );

    let qc_id = workflow.add_task(qc_task);
    workflow.add_edge(proxy_id, qc_id).unwrap_or(());

    // Step 3: High-res encoding (if QC passes)
    let final_task = Task::new(
        "final-encode",
        TaskType::Transcode {
            input: source,
            output: final_output,
            preset: "high-quality".to_string(),
            params: HashMap::new(),
        },
    )
    .with_priority(TaskPriority::Critical)
    .with_timeout(Duration::from_secs(7200)); // 2 hours

    let final_id = workflow.add_task(final_task);
    workflow.add_edge(qc_id, final_id).unwrap_or(());

    workflow
}

/// Validation pipeline pattern.
///
/// Upload → Format check → QC → Archive → Notify
#[allow(dead_code)]
#[must_use]
pub fn validation_pipeline(
    input_file: PathBuf,
    archive_destination: String,
    notification_email: Vec<String>,
) -> Workflow {
    let mut workflow = Workflow::new("validation-pipeline")
        .with_description("Complete validation and archival pipeline");

    // Step 1: Format validation
    let validation_task = Task::new(
        "format-validation",
        TaskType::Analysis {
            input: input_file.clone(),
            analyses: vec![
                crate::task::AnalysisType::VideoQuality,
                crate::task::AnalysisType::AudioLevels,
            ],
            output: None,
        },
    );

    let validation_id = workflow.add_task(validation_task);

    // Step 2: QC checks
    let qc_task = Task::new(
        "quality-check",
        TaskType::QualityControl {
            input: input_file.clone(),
            profile: "broadcast".to_string(),
            // See the note in `multi_pass_encoding`: the Broadcast preset
            // installs the rule set, and rule applicability depends on the
            // file's streams.
            rules: Vec::new(),
        },
    );

    let qc_id = workflow.add_task(qc_task);
    workflow.add_edge(validation_id, qc_id).unwrap_or(());

    // Step 3: Archive transfer
    let archive_task = Task::new(
        "archive",
        TaskType::Transfer {
            source: input_file.to_string_lossy().to_string(),
            destination: archive_destination,
            protocol: crate::task::TransferProtocol::S3,
            options: HashMap::new(),
        },
    )
    .with_timeout(Duration::from_secs(3600));

    let archive_id = workflow.add_task(archive_task);
    workflow.add_edge(qc_id, archive_id).unwrap_or(());

    // Step 4: Notification
    let notify_task = Task::new(
        "notify-completion",
        TaskType::Notification {
            channel: crate::task::NotificationChannel::Email {
                to: notification_email,
                subject: "Validation Pipeline Completed".to_string(),
            },
            message: "File successfully validated and archived".to_string(),
            metadata: HashMap::new(),
        },
    );

    let notify_id = workflow.add_task(notify_task);
    workflow.add_edge(archive_id, notify_id).unwrap_or(());

    workflow
}

/// Distribution workflow pattern.
///
/// Encode → Multiple outputs (web, mobile, broadcast) → Upload
#[allow(dead_code)]
#[must_use]
pub fn distribution_workflow(
    source: PathBuf,
    output_dir: PathBuf,
    upload_destinations: Vec<String>,
) -> Workflow {
    let mut workflow = Workflow::new("distribution-workflow")
        .with_description("Multi-format distribution pipeline");

    let source_task = Task::new(
        "source-analysis",
        TaskType::Analysis {
            input: source.clone(),
            analyses: vec![crate::task::AnalysisType::VideoQuality],
            output: None,
        },
    );

    let source_id = workflow.add_task(source_task);

    // Create multiple format outputs. Presets and scales are the ones
    // `task_exec::transcode` really resolves onto encoders that exist in this
    // build (MJPEG/FFV1 + PCM/FLAC in Matroska); a made-up preset name would
    // now fail the task instead of silently "succeeding".
    let formats = vec![
        ("web-hd", "standard", 1920u32),
        ("web-sd", "standard", 1280),
        ("mobile", "proxy", 854),
        ("broadcast", "high-quality", 1920),
    ];

    let mut encode_ids = Vec::new();

    for (name, preset, width) in formats {
        let mut params = HashMap::new();
        params.insert(
            "scale".to_string(),
            serde_json::json!(format!("{width}x-1")),
        );

        let encode_task = Task::new(
            format!("encode-{name}"),
            TaskType::Transcode {
                input: source.clone(),
                output: output_dir.join(format!("{name}.mkv")),
                preset: preset.to_string(),
                params,
            },
        )
        .with_priority(TaskPriority::High);

        let encode_id = workflow.add_task(encode_task);
        workflow.add_edge(source_id, encode_id).unwrap_or(());
        encode_ids.push(encode_id);
    }

    // Upload all outputs
    for (idx, destination) in upload_destinations.iter().enumerate() {
        let upload_task = Task::new(
            format!("upload-{idx}"),
            TaskType::Transfer {
                source: output_dir.to_string_lossy().to_string(),
                destination: destination.clone(),
                protocol: crate::task::TransferProtocol::S3,
                options: HashMap::new(),
            },
        );

        let upload_id = workflow.add_task(upload_task);

        // Upload depends on all encodes
        for &encode_id in &encode_ids {
            workflow.add_edge(encode_id, upload_id).unwrap_or(());
        }
    }

    workflow
}

/// Archive workflow pattern.
///
/// Ingest → Validate → Create proxies → Deep archive
#[allow(dead_code)]
#[must_use]
pub fn archive_workflow(
    source: PathBuf,
    proxy_dir: PathBuf,
    archive_destination: String,
) -> Workflow {
    let mut workflow = Workflow::new("archive-workflow")
        .with_description("Complete archival workflow with proxies");

    // Step 1: Ingest and validate
    let ingest_task = Task::new(
        "ingest-validate",
        TaskType::Analysis {
            input: source.clone(),
            analyses: vec![
                crate::task::AnalysisType::VideoQuality,
                crate::task::AnalysisType::AudioLevels,
                crate::task::AnalysisType::Color,
            ],
            output: Some(proxy_dir.join("analysis.json")),
        },
    );

    let ingest_id = workflow.add_task(ingest_task);

    // Step 2: Create multiple proxy formats. Every entry names a preset
    // `task_exec::transcode` resolves onto real encoders, with the resolution
    // expressed as a scale parameter.
    let proxy_formats = vec![
        ("proxy-high", 1920u32),
        ("proxy-medium", 1280),
        ("proxy-low", 854),
        ("proxy-thumb", 320),
    ];

    let mut proxy_ids = Vec::new();

    for (name, width) in proxy_formats {
        let mut params = HashMap::new();
        params.insert(
            "scale".to_string(),
            serde_json::json!(format!("{width}x-1")),
        );

        let proxy_task = Task::new(
            format!("create-{name}"),
            TaskType::Transcode {
                input: source.clone(),
                output: proxy_dir.join(format!("{name}.mkv")),
                preset: "proxy".to_string(),
                params,
            },
        );

        let proxy_id = workflow.add_task(proxy_task);
        workflow.add_edge(ingest_id, proxy_id).unwrap_or(());
        proxy_ids.push(proxy_id);
    }

    // Step 3: Deep archive original
    let archive_task = Task::new(
        "deep-archive",
        TaskType::Transfer {
            source: source.to_string_lossy().to_string(),
            destination: archive_destination,
            protocol: crate::task::TransferProtocol::S3,
            options: {
                let mut opts = HashMap::new();
                opts.insert("storage_class".to_string(), "GLACIER".to_string());
                opts
            },
        },
    )
    .with_timeout(Duration::from_secs(7200));

    let archive_id = workflow.add_task(archive_task);

    // Archive depends on all proxies being created
    for proxy_id in proxy_ids {
        workflow.add_edge(proxy_id, archive_id).unwrap_or(());
    }

    workflow
}

/// Parallel processing pattern.
///
/// Creates a workflow with parallel independent tasks.
#[allow(dead_code)]
#[must_use]
pub fn parallel_processing(inputs: Vec<PathBuf>, output_dir: PathBuf, preset: String) -> Workflow {
    let mut workflow =
        Workflow::new("parallel-processing").with_description("Process multiple files in parallel");

    for (idx, input) in inputs.iter().enumerate() {
        let task = Task::new(
            format!("process-{idx}"),
            TaskType::Transcode {
                input: input.clone(),
                output: output_dir.join(format!("output_{idx}.mp4")),
                preset: preset.clone(),
                params: HashMap::new(),
            },
        )
        .with_priority(TaskPriority::Normal);

        workflow.add_task(task);
    }

    workflow
}

/// Sequential processing pattern.
///
/// Creates a linear workflow where each task depends on the previous one.
#[allow(dead_code)]
#[must_use]
pub fn sequential_processing(input: PathBuf, output_dir: PathBuf, stages: Vec<String>) -> Workflow {
    let mut workflow = Workflow::new("sequential-processing")
        .with_description("Sequential multi-stage processing");

    let mut previous_id = None;
    let mut current_input = input;

    for (idx, stage) in stages.iter().enumerate() {
        let output = output_dir.join(format!("stage_{idx}.mp4"));

        let task = Task::new(
            format!("stage-{idx}"),
            TaskType::Transcode {
                input: current_input.clone(),
                output: output.clone(),
                preset: stage.clone(),
                params: HashMap::new(),
            },
        );

        let task_id = workflow.add_task(task);

        if let Some(prev_id) = previous_id {
            workflow.add_edge(prev_id, task_id).unwrap_or(());
        }

        previous_id = Some(task_id);
        current_input = output;
    }

    workflow
}

/// Conditional branching pattern.
///
/// Executes different paths based on conditions.
#[allow(dead_code)]
#[must_use]
pub fn conditional_workflow(input: PathBuf, output_hq: PathBuf, output_lq: PathBuf) -> Workflow {
    let mut workflow = Workflow::new("conditional-workflow")
        .with_description("Conditional processing based on input properties");

    // Analysis task to determine quality
    let analysis_task = Task::new(
        "analyze-input",
        TaskType::Analysis {
            input: input.clone(),
            analyses: vec![crate::task::AnalysisType::VideoQuality],
            output: None,
        },
    );

    let analysis_id = workflow.add_task(analysis_task);

    // High quality path
    let hq_task = Task::new(
        "high-quality-encode",
        TaskType::Transcode {
            input: input.clone(),
            output: output_hq,
            preset: "ultra-hq".to_string(),
            params: HashMap::new(),
        },
    )
    .with_condition("input.bitrate > 10000000"); // Condition example

    let hq_id = workflow.add_task(hq_task);
    workflow.add_edge(analysis_id, hq_id).unwrap_or(());

    // Low quality path
    let lq_task = Task::new(
        "standard-quality-encode",
        TaskType::Transcode {
            input,
            output: output_lq,
            preset: "standard".to_string(),
            params: HashMap::new(),
        },
    )
    .with_condition("input.bitrate <= 10000000");

    let lq_id = workflow.add_task(lq_task);
    workflow.add_edge(analysis_id, lq_id).unwrap_or(());

    workflow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watch_folder_transcode() {
        let workflow = watch_folder_transcode(
            PathBuf::from("/input"),
            PathBuf::from("/output"),
            "h264".to_string(),
        );

        assert_eq!(workflow.name, "watch-folder-transcode");
        assert_eq!(workflow.tasks.len(), 1);
    }

    #[test]
    fn test_multi_pass_encoding() {
        let workflow = multi_pass_encoding(
            PathBuf::from("/source.mp4"),
            PathBuf::from("/proxy.mp4"),
            PathBuf::from("/final.mp4"),
            "broadcast".to_string(),
        );

        assert_eq!(workflow.name, "multi-pass-encoding");
        assert_eq!(workflow.tasks.len(), 3);
        assert_eq!(workflow.edges.len(), 2);
    }

    #[test]
    fn test_validation_pipeline() {
        let workflow = validation_pipeline(
            PathBuf::from("/input.mp4"),
            "s3://bucket/archive".to_string(),
            vec!["admin@example.com".to_string()],
        );

        assert_eq!(workflow.name, "validation-pipeline");
        assert_eq!(workflow.tasks.len(), 4);
        assert!(workflow.validate().is_ok());
    }

    #[test]
    fn test_distribution_workflow() {
        let workflow = distribution_workflow(
            PathBuf::from("/source.mp4"),
            PathBuf::from("/output"),
            vec!["s3://cdn1".to_string(), "s3://cdn2".to_string()],
        );

        assert_eq!(workflow.name, "distribution-workflow");
        assert!(workflow.tasks.len() >= 5); // source + 4 formats + uploads
    }

    #[test]
    fn test_archive_workflow() {
        let workflow = archive_workflow(
            PathBuf::from("/source.mp4"),
            PathBuf::from("/proxies"),
            "s3://archive".to_string(),
        );

        assert_eq!(workflow.name, "archive-workflow");
        assert!(workflow.tasks.len() >= 5); // ingest + proxies + archive
    }

    #[test]
    fn test_parallel_processing() {
        let inputs = vec![
            PathBuf::from("/input1.mp4"),
            PathBuf::from("/input2.mp4"),
            PathBuf::from("/input3.mp4"),
        ];

        let workflow = parallel_processing(inputs, PathBuf::from("/output"), "h264".to_string());

        assert_eq!(workflow.tasks.len(), 3);
        assert_eq!(workflow.edges.len(), 0); // No dependencies - parallel
    }

    #[test]
    fn test_sequential_processing() {
        let stages = vec![
            "denoise".to_string(),
            "color-correct".to_string(),
            "stabilize".to_string(),
        ];

        let workflow = sequential_processing(
            PathBuf::from("/input.mp4"),
            PathBuf::from("/output"),
            stages,
        );

        assert_eq!(workflow.tasks.len(), 3);
        assert_eq!(workflow.edges.len(), 2); // Sequential dependencies
    }

    #[test]
    fn test_conditional_workflow() {
        let workflow = conditional_workflow(
            PathBuf::from("/input.mp4"),
            PathBuf::from("/output_hq.mp4"),
            PathBuf::from("/output_lq.mp4"),
        );

        assert_eq!(workflow.tasks.len(), 3); // analysis + 2 conditional paths
        assert!(workflow.validate().is_ok());
    }
}

// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! End-to-end tests proving that [`DefaultTaskExecutor`] really performs the
//! media work its task types describe — and that a task which cannot be
//! performed fails the DAG instead of reporting a green workflow.
//!
//! Every assertion checks *evidence*, not just a status code:
//!
//! - the Transcode task's FLAC output is re-decoded with the real spec
//!   decoder and compared sample-exact against the source WAV,
//! - the Analysis task's JSON document is read back and its measurements
//!   checked,
//! - the HTTP task runs against a local Axum server bound to port 0,
//! - un-executable tasks must leave the workflow in
//!   [`WorkflowState::Failed`] with the dependent task never completed.
//!
//! Fixtures are written under [`std::env::temp_dir`] with the process id in
//! the file name so parallel test processes never collide.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use oximedia_workflow::executor::{DefaultTaskExecutor, TaskExecutor, WorkflowExecutor};
use oximedia_workflow::task::{AnalysisType, HttpMethod, Task, TaskState, TaskType};
use oximedia_workflow::workflow::{Workflow, WorkflowState};

// ─── Fixtures ────────────────────────────────────────────────────────────────

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("oximedia_wf_e2e_{}_{name}", std::process::id()))
}

/// Deterministic 16-bit PCM sine, interleaved over `channels`.
fn sine_pcm(freq: f64, sample_rate: u32, channels: u16, frames: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * usize::from(channels));
    for i in 0..frames {
        let t = i as f64 / f64::from(sample_rate);
        let v = ((t * freq * std::f64::consts::TAU).sin() * 12_000.0) as i16;
        for _ in 0..channels {
            out.push(v);
        }
    }
    out
}

/// Minimal valid 16-bit PCM WAV bytes.
fn wav_bytes(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&(channels * 2).to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

/// Minimal Y4M (YUV4MPEG2 C420jpeg) with a moving gradient.
fn y4m_bytes(width: usize, height: usize, frames: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(
        format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C420jpeg\n").as_bytes(),
    );
    for t in 0..frames {
        buf.extend_from_slice(b"FRAME\n");
        for y in 0..height {
            for x in 0..width {
                buf.push(((x * 3 + y * 5 + t * 31) % 256) as u8);
            }
        }
        for _ in 0..2 {
            for _ in 0..height.div_ceil(2) {
                for x in 0..width.div_ceil(2) {
                    buf.push(((x * 4 + t * 9) % 256) as u8);
                }
            }
        }
    }
    buf
}

/// Runs one task through the default executor.
async fn run_task(task: &Task) -> oximedia_workflow::task::TaskResult {
    DefaultTaskExecutor
        .execute(task)
        .await
        .expect("DefaultTaskExecutor::execute never returns Err")
}

// ─── (a) Transcode really transcodes ─────────────────────────────────────────

#[tokio::test]
async fn transcode_task_produces_a_decodable_flac_file() {
    let input = temp_path("a_in.wav");
    let output = temp_path("a_out.flac");
    let source = sine_pcm(440.0, 48_000, 2, 24_000);
    std::fs::write(&input, wav_bytes(&source, 48_000, 2)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    let task = Task::new(
        "encode-flac",
        TaskType::Transcode {
            input: input.clone(),
            output: output.clone(),
            preset: "audio-flac".to_string(),
            params: HashMap::new(),
        },
    );

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Completed,
        "transcode must succeed: {:?}",
        result.error
    );
    assert_eq!(result.outputs, vec![output.clone()]);

    let data = result.data.expect("transcode records evidence");
    assert_eq!(data["kind"], "transcode");
    assert_eq!(data["audio_codec"], "flac");
    assert!(data["output_bytes"].as_u64().unwrap_or(0) > 0);

    // Evidence: the artefact really is a FLAC stream carrying the input audio.
    let encoded = std::fs::read(&output).expect("output file exists");
    assert!(!encoded.is_empty(), "output must not be empty");
    let (params, decoded) =
        oximedia_transcode::flac_decode::decode_flac_to_i16(&encoded).expect("output decodes");
    assert_eq!(params.sample_rate, 48_000);
    assert_eq!(params.channels, 2);
    assert_eq!(
        decoded.len(),
        source.len(),
        "FLAC round trip must preserve the sample count"
    );
    assert_eq!(decoded, source, "FLAC round trip must be sample-exact");

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[tokio::test]
async fn transcode_task_reports_the_real_codec_limitation() {
    let input = temp_path("b_in.y4m");
    let output = temp_path("b_out.mkv");
    std::fs::write(&input, y4m_bytes(32, 32, 4)).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    let mut params = HashMap::new();
    params.insert("video_codec".to_string(), serde_json::json!("vp9"));

    let task = Task::new(
        "encode-vp9",
        TaskType::Transcode {
            input: input.clone(),
            output: output.clone(),
            preset: "copy".to_string(),
            params,
        },
    );

    let result = run_task(&task).await;
    assert_eq!(result.status, TaskState::Failed);
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("VP9") || error.contains("vp9"),
        "the limitation must name the codec: {error}"
    );
    assert!(
        !output.exists() || std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0) == 0,
        "a failed transcode must not leave a usable artefact"
    );

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

// ─── (b) Analysis really measures ────────────────────────────────────────────

#[tokio::test]
async fn analysis_task_writes_real_measurements_to_its_output_slot() {
    let input = temp_path("c_in.y4m");
    let output = temp_path("c_out/analysis.json");
    std::fs::write(&input, y4m_bytes(64, 64, 6)).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    let task = Task::new(
        "analyse",
        TaskType::Analysis {
            input: input.clone(),
            analyses: vec![
                AnalysisType::BlackFrames,
                AnalysisType::Motion,
                AnalysisType::VideoQuality,
            ],
            output: Some(output.clone()),
        },
    );

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Completed,
        "analysis must succeed: {:?}",
        result.error
    );
    assert_eq!(result.outputs, vec![output.clone()]);

    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).expect("result document")).expect("JSON");
    assert_eq!(document["source"]["kind"], "y4m");
    assert_eq!(document["source"]["frames_decoded"], 6);
    assert_eq!(
        document["analyses"]["motion"]["frames_compared"]
            .as_u64()
            .unwrap_or(0),
        5,
        "6 frames give 5 frame-to-frame comparisons"
    );
    assert!(
        document["analyses"]["video_quality"]["metrics"]["blur"]["available"]
            .as_bool()
            .unwrap_or(false),
        "blur is measurable on a 64x64 frame"
    );

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(&output);
    if let Some(parent) = output.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

// ─── (c) Quality control really runs the rules ───────────────────────────────

#[tokio::test]
async fn quality_control_task_runs_real_rules_on_a_wav() {
    let input = temp_path("d_in.wav");
    std::fs::write(
        &input,
        wav_bytes(&sine_pcm(1_000.0, 48_000, 2, 48_000), 48_000, 2),
    )
    .expect("write wav");

    let task = Task::new(
        "qc",
        TaskType::QualityControl {
            input: input.clone(),
            profile: "basic".to_string(),
            rules: Vec::new(),
        },
    );

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Completed,
        "a clean 48 kHz stereo PCM WAV must pass the Basic profile: {:?}",
        result.error
    );

    let data = result.data.expect("QC records evidence");
    assert_eq!(data["kind"], "quality_control");
    assert_eq!(data["container"], "WAV");
    assert!(
        data["total_checks"].as_u64().unwrap_or(0) > 0,
        "QC must actually have run checks: {data}"
    );
    assert_eq!(
        data["passed_checks"], data["total_checks"],
        "a passing report must have every check passing"
    );

    // The rules really executed — not a fabricated verdict.
    let evaluated: Vec<String> = data["rules_evaluated"]
        .as_array()
        .expect("rules_evaluated is an array")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    for expected in ["audio_codec_validation", "format_validation"] {
        assert!(
            evaluated.iter().any(|name| name == expected),
            "the Basic profile must have run {expected}, got {evaluated:?}"
        );
    }

    let _ = std::fs::remove_file(input);
}

#[tokio::test]
async fn quality_control_task_refuses_an_unknown_profile() {
    let input = temp_path("e_in.wav");
    std::fs::write(
        &input,
        wav_bytes(&sine_pcm(440.0, 48_000, 1, 4_800), 48_000, 1),
    )
    .expect("write wav");

    let task = Task::new(
        "qc-bogus",
        TaskType::QualityControl {
            input: input.clone(),
            profile: "super-broadcast".to_string(),
            rules: Vec::new(),
        },
    );

    let result = run_task(&task).await;
    assert_eq!(result.status, TaskState::Failed);
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("accepted QC profiles"),
        "an unknown profile must be refused, not silently defaulted: {error}"
    );

    let _ = std::fs::remove_file(input);
}

// ─── (d) HTTP really sends ───────────────────────────────────────────────────

/// Starts a local HTTP server on an ephemeral port and returns its base URL.
async fn start_test_server() -> String {
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::Router;

    let app = Router::new()
        .route("/ok", get(|| async { "workflow-http-ok" }))
        .route(
            "/boom",
            get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "kaboom") }),
        )
        .route("/echo", post(|body: String| async move { body }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn http_task_sends_the_request_and_records_the_status() {
    let base = start_test_server().await;

    let task = Task::new(
        "fetch",
        TaskType::HttpRequest {
            url: format!("{base}/ok"),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body: None,
        },
    )
    .with_timeout(Duration::from_secs(10));

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Completed,
        "GET /ok must succeed: {:?}",
        result.error
    );
    let data = result.data.expect("HTTP records evidence");
    assert_eq!(data["kind"], "http_request");
    assert_eq!(data["status"], 200);
    assert_eq!(data["method"], "GET");
    assert_eq!(data["body_preview"], "workflow-http-ok");
}

#[tokio::test]
async fn http_task_posts_the_configured_body() {
    let base = start_test_server().await;

    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "text/plain".to_string());

    let task = Task::new(
        "post",
        TaskType::HttpRequest {
            url: format!("{base}/echo"),
            method: HttpMethod::Post,
            headers,
            body: Some("payload-from-workflow".to_string()),
        },
    )
    .with_timeout(Duration::from_secs(10));

    let result = run_task(&task).await;
    assert_eq!(result.status, TaskState::Completed, "{:?}", result.error);
    let data = result.data.expect("HTTP records evidence");
    assert_eq!(data["body_preview"], "payload-from-workflow");
}

#[tokio::test]
async fn http_task_fails_on_an_unaccepted_status() {
    let base = start_test_server().await;

    let task = Task::new(
        "fetch-boom",
        TaskType::HttpRequest {
            url: format!("{base}/boom"),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body: None,
        },
    )
    .with_timeout(Duration::from_secs(10));

    let result = run_task(&task).await;
    assert_eq!(result.status, TaskState::Failed);
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("500"),
        "the status must be reported: {error}"
    );
}

#[tokio::test]
async fn http_task_honours_the_allow_status_metadata() {
    let base = start_test_server().await;

    let mut task = Task::new(
        "fetch-boom-allowed",
        TaskType::HttpRequest {
            url: format!("{base}/boom"),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body: None,
        },
    )
    .with_timeout(Duration::from_secs(10));
    task.metadata
        .insert("http.allow_status".to_string(), "2xx,500".to_string());

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Completed,
        "500 is explicitly allowed: {:?}",
        result.error
    );
    let data = result.data.expect("HTTP records evidence");
    assert_eq!(data["status"], 500);
}

#[tokio::test]
async fn http_task_fails_when_the_endpoint_is_unreachable() {
    // Bind and immediately drop a listener to obtain a port nothing serves.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);

    let task = Task::new(
        "fetch-dead",
        TaskType::HttpRequest {
            url: format!("http://{addr}/nothing"),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body: None,
        },
    )
    .with_timeout(Duration::from_secs(5));

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Failed,
        "an unreachable endpoint must fail the task"
    );
}

// ─── (e) The DAG marks un-executable tasks failed, never green ───────────────

#[tokio::test]
async fn a_failing_media_task_fails_the_workflow_and_blocks_its_dependant() {
    let input = temp_path("f_in.y4m");
    let output = temp_path("f_out.mkv");
    std::fs::write(&input, y4m_bytes(16, 16, 2)).expect("write y4m");

    let mut workflow = Workflow::new("failing-transcode");

    let transcode = Task::new(
        "transcode",
        TaskType::Transcode {
            input: input.clone(),
            output: output.clone(),
            // A preset the executor refuses to guess at.
            preset: "make-it-look-nice".to_string(),
            params: HashMap::new(),
        },
    );
    let transcode_id = workflow.add_task(transcode);

    let downstream = Task::new(
        "downstream",
        TaskType::Wait {
            duration: Duration::from_millis(1),
        },
    );
    let downstream_id = workflow.add_task(downstream);
    workflow
        .add_edge(transcode_id, downstream_id)
        .expect("edge must be accepted");

    let executor = WorkflowExecutor::new(std::sync::Arc::new(DefaultTaskExecutor));
    let result = executor
        .execute(&mut workflow)
        .await
        .expect("execution returns a result (fail_fast is off by default)");

    assert_eq!(
        result.state,
        WorkflowState::Failed,
        "a task that could not do its work must not leave the workflow green"
    );

    let transcode_result = result
        .task_results
        .get(&transcode_id)
        .expect("the transcode task reports a result");
    assert_eq!(transcode_result.status, TaskState::Failed);
    let error = transcode_result.error.clone().unwrap_or_default();
    assert!(
        error.contains("accepted presets"),
        "the failure must name the accepted presets: {error}"
    );

    assert!(
        !result.task_results.contains_key(&downstream_id),
        "the dependant task must never run behind a failed dependency"
    );
    assert_eq!(
        workflow
            .get_task(&downstream_id)
            .map(|task| task.state)
            .expect("downstream task exists"),
        TaskState::Skipped,
        "the dependant task must be marked skipped, not completed"
    );

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[tokio::test]
async fn an_undecodable_analysis_input_fails_the_workflow() {
    let input = temp_path("g_in.mkv");
    std::fs::write(&input, b"\x1a\x45\xdf\xa3not-really-a-matroska-file").expect("write");

    let mut workflow = Workflow::new("failing-analysis");
    let analysis_id = workflow.add_task(Task::new(
        "analyse",
        TaskType::Analysis {
            input: input.clone(),
            analyses: vec![AnalysisType::VideoQuality],
            output: None,
        },
    ));

    let executor = WorkflowExecutor::new(std::sync::Arc::new(DefaultTaskExecutor));
    let result = executor
        .execute(&mut workflow)
        .await
        .expect("execution returns a result");

    assert_eq!(result.state, WorkflowState::Failed);
    let analysis_result = result
        .task_results
        .get(&analysis_id)
        .expect("analysis reports a result");
    assert_eq!(analysis_result.status, TaskState::Failed);
    assert!(
        analysis_result
            .error
            .clone()
            .unwrap_or_default()
            .contains("neither RIFF/WAVE nor"),
        "the failure must name the decoding limitation"
    );

    let _ = std::fs::remove_file(input);
}

#[tokio::test]
async fn a_workflow_of_real_media_tasks_completes_green() {
    let input = temp_path("h_in.wav");
    let flac = temp_path("h_out.flac");
    let report = temp_path("h_report.json");
    std::fs::write(
        &input,
        wav_bytes(&sine_pcm(880.0, 48_000, 1, 24_000), 48_000, 1),
    )
    .expect("write wav");
    let _ = std::fs::remove_file(&flac);
    let _ = std::fs::remove_file(&report);

    let mut workflow = Workflow::new("analyse-then-encode");

    let analysis_id = workflow.add_task(Task::new(
        "analyse",
        TaskType::Analysis {
            input: input.clone(),
            analyses: vec![AnalysisType::AudioLevels, AnalysisType::Silence],
            output: Some(report.clone()),
        },
    ));
    let transcode_id = workflow.add_task(Task::new(
        "encode",
        TaskType::Transcode {
            input: input.clone(),
            output: flac.clone(),
            preset: "audio-flac".to_string(),
            params: HashMap::new(),
        },
    ));
    workflow
        .add_edge(analysis_id, transcode_id)
        .expect("edge must be accepted");

    let executor = WorkflowExecutor::new(std::sync::Arc::new(DefaultTaskExecutor));
    let result = executor
        .execute(&mut workflow)
        .await
        .expect("execution returns a result");

    assert_eq!(
        result.state,
        WorkflowState::Completed,
        "both tasks do real work that must succeed: {:?}",
        result
            .task_results
            .values()
            .filter_map(|r| r.error.clone())
            .collect::<Vec<_>>()
    );

    // Both artefacts exist, are non-empty, and carry real content.
    assert!(std::fs::metadata(&report).map(|m| m.len()).unwrap_or(0) > 0);
    let encoded = std::fs::read(&flac).expect("flac written");
    let (params, decoded) =
        oximedia_transcode::flac_decode::decode_flac_to_i16(&encoded).expect("flac decodes");
    assert_eq!(params.sample_rate, 48_000);
    assert_eq!(decoded.len(), 24_000);

    let analysis_result = result
        .task_results
        .get(&analysis_id)
        .expect("analysis result");
    assert_eq!(analysis_result.outputs, vec![report.clone()]);
    let transcode_result = result
        .task_results
        .get(&transcode_id)
        .expect("transcode result");
    assert_eq!(transcode_result.outputs, vec![flac.clone()]);

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(flac);
    let _ = std::fs::remove_file(report);
}

// ─── (f) The preset table maps onto encoders that really run ─────────────────

/// Extracts the first complete JPEG (SOI..EOI) embedded in a byte stream.
fn first_jpeg(data: &[u8]) -> Option<&[u8]> {
    let start = data.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])?;
    let end = data[start..].windows(2).position(|w| w == [0xFF, 0xD9])?;
    Some(&data[start..start + end + 2])
}

#[tokio::test]
async fn video_presets_really_encode_a_video_only_input() {
    // `proxy` / `standard` are *paired* presets (video + audio); a Y4M carries
    // no audio, so the preset-implied audio codec must be dropped rather than
    // making the job fail. `mjpeg` names the codec directly.
    for preset in ["proxy", "standard", "mjpeg"] {
        let input = temp_path(&format!("p_{preset}_in.y4m"));
        let output = temp_path(&format!("p_{preset}_out.mkv"));
        std::fs::write(&input, y4m_bytes(64, 48, 4)).expect("write y4m");
        let _ = std::fs::remove_file(&output);

        let task = Task::new(
            "encode",
            TaskType::Transcode {
                input: input.clone(),
                output: output.clone(),
                preset: preset.to_string(),
                params: HashMap::new(),
            },
        );

        let result = run_task(&task).await;
        assert_eq!(
            result.status,
            TaskState::Completed,
            "preset `{preset}` must encode a video-only input: {:?}",
            result.error
        );

        // Evidence: the Matroska output embeds a real, complete JPEG frame.
        let encoded = std::fs::read(&output).expect("output written");
        let jpeg = first_jpeg(&encoded)
            .unwrap_or_else(|| panic!("preset `{preset}` must embed MJPEG frames"));
        assert!(
            jpeg.len() > 100,
            "preset `{preset}`: embedded JPEG is implausibly small ({} bytes)",
            jpeg.len()
        );

        let data = result.data.expect("transcode records evidence");
        if preset == "proxy" || preset == "standard" {
            let dropped = data["preset_settings_dropped"]
                .as_array()
                .expect("dropped list");
            assert!(
                dropped.iter().any(|v| v == "audio_codec"),
                "preset `{preset}` must report dropping its audio half: {data}"
            );
        }

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }
}

#[tokio::test]
async fn audio_preset_drops_its_video_half_for_a_wav_input() {
    let input = temp_path("q_in.wav");
    let output = temp_path("q_out.flac");
    let source = sine_pcm(660.0, 48_000, 1, 12_000);
    std::fs::write(&input, wav_bytes(&source, 48_000, 1)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    // `standard` is mjpeg + flac; the WAV has no video stream.
    let task = Task::new(
        "encode",
        TaskType::Transcode {
            input: input.clone(),
            output: output.clone(),
            preset: "standard".to_string(),
            params: HashMap::new(),
        },
    );

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Completed,
        "an audio-only input must still encode: {:?}",
        result.error
    );
    let data = result.data.expect("transcode records evidence");
    let dropped = data["preset_settings_dropped"]
        .as_array()
        .expect("dropped list");
    assert!(
        dropped.iter().any(|v| v == "video_codec"),
        "the video half must be reported as dropped: {data}"
    );

    let (_, decoded) =
        oximedia_transcode::flac_decode::decode_flac_to_i16(&std::fs::read(&output).expect("out"))
            .expect("flac decodes");
    assert_eq!(decoded, source, "the audio half must still be sample-exact");

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[tokio::test]
async fn an_explicitly_requested_codec_is_never_dropped() {
    // Unlike a preset default, an explicit `params.audio_codec` on a
    // video-only input must fail loudly instead of being silently ignored.
    let input = temp_path("r_in.y4m");
    let output = temp_path("r_out.mkv");
    std::fs::write(&input, y4m_bytes(32, 32, 2)).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    let mut params = HashMap::new();
    params.insert("audio_codec".to_string(), serde_json::json!("flac"));
    params.insert("video_codec".to_string(), serde_json::json!("mjpeg"));

    let task = Task::new(
        "encode",
        TaskType::Transcode {
            input: input.clone(),
            output: output.clone(),
            preset: "copy".to_string(),
            params,
        },
    );

    let result = run_task(&task).await;
    assert_eq!(
        result.status,
        TaskState::Failed,
        "an impossible explicit request must not be silently dropped"
    );
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("no audio stream"),
        "the failure must explain why: {error}"
    );

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

#[tokio::test]
async fn remuxing_an_unsupported_input_container_fails_honestly() {
    let input = temp_path("s_in.y4m");
    let output = temp_path("s_out.mkv");
    std::fs::write(&input, y4m_bytes(32, 32, 2)).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    let task = Task::new(
        "remux",
        TaskType::Transcode {
            input: input.clone(),
            output: output.clone(),
            preset: "copy".to_string(),
            params: HashMap::new(),
        },
    );

    let result = run_task(&task).await;
    assert_eq!(result.status, TaskState::Failed);
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("Unsupported input container"),
        "stream-copy of a Y4M must report the container limitation: {error}"
    );

    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
}

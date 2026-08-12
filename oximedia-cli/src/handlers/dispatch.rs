//! Subcommand dispatch handlers for monitor, restore, and captions groups.

use anyhow::Result;

use crate::captions_cmd;
use crate::commands::{CaptionsCommand, MonitorCommand, RestoreCommand};
use crate::monitor_cmd;
use crate::restore_cmd;

/// Handle monitor subcommands.
pub(crate) async fn handle_monitor_command(
    command: MonitorCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        MonitorCommand::Start {
            target,
            db_path,
            interval_ms,
            system_metrics,
            quality_metrics,
        } => {
            let opts = monitor_cmd::MonitorStartOptions {
                target,
                db_path,
                interval_ms,
                system_metrics,
                quality_metrics,
            };
            monitor_cmd::run_monitor_start(opts, json_output).await
        }
        MonitorCommand::Status { db_path, detailed } => {
            let opts = monitor_cmd::MonitorStatusOptions { db_path, detailed };
            monitor_cmd::run_monitor_status(opts, json_output).await
        }
        MonitorCommand::Alerts {
            db_path,
            count,
            severity,
        } => {
            let opts = monitor_cmd::MonitorAlertsOptions {
                db_path,
                count,
                severity,
            };
            monitor_cmd::run_monitor_alerts(opts, json_output).await
        }
        MonitorCommand::Config {
            db_path,
            cpu_threshold,
            memory_threshold,
            quality_threshold,
            show,
        } => {
            let opts = monitor_cmd::MonitorConfigOptions {
                db_path,
                cpu_threshold,
                memory_threshold,
                quality_threshold,
                show,
            };
            monitor_cmd::run_monitor_config(opts, json_output).await
        }
        MonitorCommand::Dashboard {
            db_path,
            refresh_secs,
            history_points,
        } => {
            let opts = monitor_cmd::MonitorDashboardOptions {
                db_path,
                refresh_secs,
                history_points,
            };
            monitor_cmd::run_monitor_dashboard(opts, json_output).await
        }
    }
}

/// Handle restore subcommands.
pub(crate) async fn handle_restore_command(
    command: RestoreCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        RestoreCommand::Audio {
            input,
            output,
            mode,
            sample_rate,
            declip,
            decrackle,
            dehum,
            denoise,
            raw,
        } => {
            let opts = restore_cmd::RestoreAudioOptions {
                input,
                output,
                mode,
                sample_rate,
                declip,
                decrackle,
                dehum,
                denoise,
                raw,
            };
            restore_cmd::run_restore_audio(opts, json_output).await
        }
        RestoreCommand::Video {
            input,
            output,
            mode,
            width,
            height,
        } => {
            let opts = restore_cmd::RestoreVideoOptions {
                input,
                output,
                mode,
                width,
                height,
            };
            restore_cmd::run_restore_video(opts, json_output).await
        }
        RestoreCommand::Analyze {
            input,
            analysis_type,
        } => {
            let opts = restore_cmd::RestoreAnalyzeOptions {
                input,
                analysis_type,
            };
            restore_cmd::run_restore_analyze(opts, json_output).await
        }
        RestoreCommand::Batch {
            input_dir,
            output_dir,
            mode,
            extension,
        } => {
            let opts = restore_cmd::RestoreBatchOptions {
                input_dir,
                output_dir,
                mode,
                extension,
            };
            restore_cmd::run_restore_batch(opts, json_output).await
        }
        RestoreCommand::Compare { original, restored } => {
            let opts = restore_cmd::RestoreCompareOptions { original, restored };
            restore_cmd::run_restore_compare(opts, json_output).await
        }
    }
}

/// Handle captions subcommands.
pub(crate) async fn handle_captions_command(
    command: CaptionsCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        CaptionsCommand::Generate {
            input,
            output,
            format,
            language,
            model,
            vocab,
        } => {
            let opts = captions_cmd::CaptionsGenerateOptions {
                input,
                output,
                format,
                language,
                model,
                vocab,
            };
            captions_cmd::run_captions_generate(opts, json_output).await
        }
        CaptionsCommand::Sync {
            input,
            reference,
            output,
            max_shift_ms,
        } => {
            let opts = captions_cmd::CaptionsSyncOptions {
                input,
                reference,
                output,
                max_shift_ms,
            };
            captions_cmd::run_captions_sync(opts, json_output).await
        }
        CaptionsCommand::Convert {
            input,
            output,
            from_format,
            to_format,
        } => {
            let opts = captions_cmd::CaptionsConvertOptions {
                input,
                output,
                from_format,
                to_format,
            };
            captions_cmd::run_captions_convert(opts, json_output).await
        }
        CaptionsCommand::Burn {
            video,
            captions,
            output,
            font_size,
            font_color,
            font,
        } => {
            let opts = captions_cmd::CaptionsBurnOptions {
                video,
                captions,
                output,
                font_size,
                font_color,
                font,
            };
            captions_cmd::run_captions_burn(opts, json_output).await
        }
        CaptionsCommand::Extract {
            input,
            output,
            format,
            track,
        } => {
            let opts = captions_cmd::CaptionsExtractOptions {
                input,
                output,
                format,
                track,
            };
            captions_cmd::run_captions_extract(opts, json_output).await
        }
        CaptionsCommand::Validate {
            input,
            standard,
            report,
        } => {
            let opts = captions_cmd::CaptionsValidateOptions {
                input,
                standard,
                report,
            };
            captions_cmd::run_captions_validate(opts, json_output).await
        }
    }
}

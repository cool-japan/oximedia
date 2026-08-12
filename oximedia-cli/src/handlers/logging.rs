//! Color and tracing-subscriber initialisation for the CLI.

use anyhow::{Context, Result};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum LogFormat {
    /// Human-readable, coloured text (default).
    Plain,
    /// Newline-delimited JSON (machine-readable).
    Json,
}

/// Honour the NO_COLOR convention and CLICOLOR / TERM=dumb environment variables.
///
/// This must be called **before** any coloured output is produced.
/// `force_no_color` (from `--no-color`, `--json`, or `--ndjson`) wins over env.
pub(crate) fn init_color(force_no_color: bool) {
    let env_disables = std::env::var_os("NO_COLOR").is_some()
        || std::env::var("CLICOLOR").map(|v| v == "0").unwrap_or(false)
        || std::env::var("TERM").map(|v| v == "dumb").unwrap_or(false);
    if force_no_color || env_disables {
        colored::control::set_override(false);
    }
}

/// Initialize logging based on verbosity level and log format.
pub(crate) fn init_logging(verbose: u8, quiet: bool, log_format: LogFormat) -> Result<()> {
    if quiet {
        // No logging in quiet mode
        return Ok(());
    }

    let level = match verbose {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };

    match log_format {
        LogFormat::Plain => {
            // Logs are diagnostic output, not program output: write them to
            // stderr so they never interleave with stdout results (`--json`
            // / `--ndjson` payloads, probe data, ...). `tracing_subscriber`'s
            // default writer is stdout, which would otherwise corrupt any
            // `--json` command whose call graph logs during execution (e.g.
            // `distributed start-coordinator`'s background coordinator
            // server logs real `info!` lines while running).
            let subscriber = FmtSubscriber::builder()
                .with_max_level(level)
                .with_target(false)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_writer(std::io::stderr)
                .compact()
                .finish();

            tracing::subscriber::set_global_default(subscriber)
                .context("Failed to set tracing subscriber")?;
        }
        LogFormat::Json => {
            // Same reasoning as the `Plain` arm above: logs go to stderr.
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_max_level(level)
                .with_writer(std::io::stderr)
                .finish();

            tracing::subscriber::set_global_default(subscriber)
                .context("Failed to set tracing subscriber")?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_color_force_no_color_does_not_panic() {
        init_color(true);
    }

    #[test]
    fn init_color_no_force_does_not_panic() {
        init_color(false);
    }
}

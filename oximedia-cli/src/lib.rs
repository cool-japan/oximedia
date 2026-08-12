//! OxiMedia CLI library — re-exports shared modules for use by the
//! `oximedia-ff` binary and other in-crate binaries.
//!
//! The main `oximedia` binary is defined in `main.rs` and declares all modules
//! privately. This lib target re-declares only the modules that auxiliary
//! binaries — and the integration tests — need to share, so they don't have to
//! duplicate code or shell out to the binary.
//!
//! [`frame_harness`] is the decode → process → encode path for pixel-level
//! commands; [`multicam_cmd`], [`timecode_cmd`], [`denoise_cmd`],
//! [`stabilize_cmd`], [`scaling_cmd`], [`subtitle_cmd`] and [`captions_cmd`]
//! are exposed so `tests/frame_harness_e2e.rs` can drive their real entry
//! points in-process.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::unused_async)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::similar_names)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::if_not_else)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::case_sensitive_file_extension_comparisons)]
#![allow(clippy::doc_link_with_quotes)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::needless_continue)]
#![allow(clippy::single_char_pattern)]

pub mod captions_cmd;
pub mod denoise_cmd;
pub mod frame_harness;
pub mod multicam_cmd;
pub mod presets;
pub mod progress;
pub mod scaling_cmd;
pub mod stabilize_cmd;
pub mod subtitle_cmd;
pub mod timecode_cmd;
pub mod transcode;

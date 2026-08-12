# oximedia-cli — Command-Line Interface TODO

**Version: 0.2.1 (active, dev branch `0.2.1`) / 0.2.0 (stable, `master`)**
**Status as of: 2026-08-12**

**0.2.1 summary.** The single root cause behind the largest cluster of gaps —
no CLI-reachable decode → process → encode frame path — is **closed**. A shared
`src/frame_harness/` module now backs **nine** commands with real frame work.
Thirteen further single-command gaps shipped. `TODO(0.2.x)` markers in
`oximedia-cli/src/` are down to **four** (all listed under "Subcommand-level
gaps" below). `--quiet` remains a **partial** rollout — see the honest count
there. `--resume` is **unchanged and still USER-UNDECIDED**.

The `oximedia-cli` crate is the primary user-facing entry point to the OxiMedia
Sovereign Media Framework. It ships **two binaries** from a single crate — the
main `oximedia` multitool with ~85 domain subcommands covering the entire FFmpeg
+ OpenCV feature surface in patent-free, pure-Rust form, and the `oximedia-ff`
drop-in that accepts raw FFmpeg-style argv and executes each translated job
through the native transcode engine. Every subcommand is wired through clap
derive, returns `anyhow::Result<()>`, respects a global `--json` / `--no-color`
/ `-v/-vv/-vvv` / `-q` flag set, and delegates real work to the ~101 workspace
crates. The whole crate weighs in at ~50,900 lines of Rust across 89 source
files (counted on 2026-04-15 via `wc -l src/*.rs src/bin/*.rs`).

---

## Current Status

### Binary targets (`Cargo.toml`)

| Binary         | Entry point                | Purpose                                                  |
|----------------|----------------------------|----------------------------------------------------------|
| `oximedia`     | `src/main.rs` (1005 lines) | Primary multitool; dispatches all domain subcommands.    |
| `oximedia-ff`  | `src/bin/oximedia-ff.rs`   | FFmpeg drop-in — argv → `oximedia-compat-ffmpeg` → exec. |

Both share code via the thin `src/lib.rs` (33 lines) which publicly re-exports
`presets`, `progress`, and `transcode` so the `oximedia-ff` binary does not
duplicate transcode plumbing.

### Source layout (117 files, ~68,007 SLOC)

- **Entry**: `main.rs` (1005), `lib.rs` (33), `bin/oximedia-ff.rs` (348).
- **Command surface**: `commands.rs` (1442) — defines the top-level `Commands`
  enum (~85 variants) and shared `MonitorCommand`, `RestoreCommand`,
  `CaptionsCommand`, `PresetCommand` sub-enums; `handlers.rs` (829) — shared
  handler functions (`probe_file`, `show_info`, `show_version`, logging init,
  monitor/restore/captions/preset dispatch).
- **Domain modules (`*_cmd.rs`)**: 75 files, one per top-level subcommand group
  (aaf, access, align, archive, archivepro, audio, audiopost, auto, batch,
  calibrate, captions, clips, cloud, collab, color, conform, dedup, denoise,
  distributed, dolbyvision, drm, edl, farm, ffcompat, filter, forensics, gaming,
  graphics, image, imf, loudness, lut, mam, mir, mixer, monitor, multicam, ndi,
  normalize, optimize, package, playlist, playout, plugin, profiler, proxy, qc,
  quality, recommend, renderfarm, repair, restore, review, rights, routing,
  scaling, scopes, search, stabilize, stream, subtitle, switcher, timecode,
  timeline, timesync, tui, vfx, videoip, virtual, watermark, workflow).
- **Shared helpers**: `analyze.rs`, `batch.rs`, `benchmark.rs`, `concat.rs`,
  `extract.rs`, `metadata.rs`, `progress.rs`, `scene.rs`, `thumbnail.rs`,
  `transcode.rs`, `validate.rs`, plus `presets/` (builtin, custom, device,
  streaming, validate, web) and `sprite/` (generate, output, timestamps, utils).

### Top-level subcommand taxonomy (from `commands::Commands`)

- **Probing & inspection** — `probe`, `info`, `version`, `validate`, `analyze`,
  `benchmark`, `metadata`, `forensics`.
- **Transcoding & muxing** — `transcode` (alias `convert`), `extract`, `batch`,
  `concat`, `thumbnail`, `sprite`, `package` (HLS/DASH), `optimize`,
  `batch-engine` (SQLite-backed persistent queue).
- **Audio** — `audio`, `loudness`, `normalize`, `mixer`, `audiopost`,
  `mir` (music-info retrieval), `align`.
- **Video processing** — `scene`, `scopes`, `denoise`, `stabilize`, `scaling`,
  `filter`, `lut`, `color`, `dolby-vision`, `multicam`, `vfx`, `image`,
  `graphics`.
- **Subtitles & captions** — `subtitle`, `captions`, `timecode`.
- **Broadcast & production** — `playout`, `switcher`, `ndi`, `videoip` (RTP/
  SRT/RIST), `calibrate`, `virtual`, `routing`, `timeline`, `timesync`, `edl`,
  `playlist`, `conform`, `qc`.
- **Archival & asset management** — `archive`, `archive-pro`, `mam`, `clips`,
  `proxy`, `dedup`, `search`, `watermark`, `drm`, `rights`, `access`,
  `repair`, `restore`.
- **Collaboration & workflow** — `collab`, `review`, `workflow`, `auto`,
  `recommend`, `imf`, `aaf`.
- **Infrastructure** — `distributed`, `farm`, `renderfarm`, `cloud`, `monitor`,
  `profiler`, `stream`, `plugin`, `quality`, `gaming`.
- **Interop & UX** — `ffcompat` (alias `ff`), `tui`, `preset`.

### Global flags (`Cli` in `main.rs`)

- `-v / --verbose` (repeatable, `ArgAction::Count` → `-v`, `-vv`, `-vvv`).
- `-q / --quiet` — suppress everything except errors.
- `--no-color` — disables `colored::control` overrides.
- `--json` — propagates into every subcommand that has a structured-output path.

### FFmpeg compatibility story

Two complementary layers both route through `oximedia-compat-ffmpeg`:

1. **In-tool**: `oximedia ff <ffmpeg-args>` (`Commands::Ffcompat`, alias `ff`)
   handled by `src/ffcompat_cmd.rs` (373 lines). Supports `--dry-run` / `--plan`
   to print translated jobs without executing.
2. **Drop-in replacement**: `oximedia-ff` binary (348 lines) — a standalone
   argv→translate→execute pipeline that can be symlinked as `ffmpeg` so legacy
   scripts transparently retarget onto OxiMedia. Honours `-h/--help`,
   `-version/--version`, `--dry-run`, `--plan` and prints FFmpeg-style
   diagnostics (`warning:`, `info:`, `error:` prefixes with hints).

Both rely on `oximedia-compat-ffmpeg`'s `parse_and_translate()` which emits
`DiagnosticKind::{PatentCodecSubstituted, UnknownOptionIgnored, FilterNotSupported,
UnsupportedFeature, Info, Warning, Error}` so H.264/H.265/AAC invocations are
auto-rewritten into AV1/VP9/Opus and reported back transparently.

### Interactive TUI (`tui_cmd.rs`, 521 lines)

Three-tab ratatui interface (`Files`, `Commands`, `About`) with crossterm input,
panic-safe terminal restoration, and a working file browser that lists cwd
entries by size. Keyboard: `q`/`Ctrl+C` quit, `Tab`/`→`/`←`/`Shift+Tab` change
tabs, `↑`/`↓` navigate, `Enter` show details.

---

## Completed `[x]`

- [x] Two-binary crate layout — `oximedia` (primary) and `oximedia-ff` (FFmpeg
  drop-in) both shipping from the same `oximedia-cli` crate.
- [x] Core inspection suite — `probe` (text/json/csv, chapters, per-stream,
  metadata dump), `info`, `version`.
  - **Correction (2026-07-14 audit):** the `--hash` and `--quality-snapshot`
    flags on `probe` parse successfully but are dead on arrival —
    destructured as `hash: _hash` / `quality_snapshot: _quality_snapshot` in
    `main.rs::run()` and never passed to `probe_file()`, which has no
    parameter for either. Both are silent no-ops today. Tracked as a new
    gap under "Subcommand-level gaps" below.
  - **Update (2026-07-14, later same day):** `--hash` has SHIPPED —
    `main.rs::run()` now destructures `hash` (no underscore) and forwards
    it into `probe_file(..., hash, ...)`, which takes a real
    `compute_hash: bool` parameter (`handlers/inspect.rs:24`). This is
    DONE, not open. `--quality-snapshot` is still discarded
    (`quality_snapshot: _quality_snapshot`) — see "CLI Flag Wiring
    Investigation (2026-07-14)" below for a ready-to-implement design.
- [x] Transcode pipeline — `transcode` with working FFmpeg-compatible
  aliases (`-i`, `-c:v`, `-c:a`, `-b:v`, `-b:a`, `--scale`, `-y`), two-pass,
  CRF, preset names.
  - **Correction (2026-07-14 audit):** `-vf`, `-af`, `-ss`, `-t`, `-r`,
    `--threads`, and `--resume` all parse without error but are never
    read — they land on `TranscodeOptions` fields marked
    `#[allow(dead_code)]` (present unchanged since the very first
    `5e1fd8c7` / 0.1.0 commit, so this is longstanding, not a regression) —
    and `--map` / `--normalize-audio` are dropped even earlier, discarded
    in `main.rs`'s match arm before ever reaching `TranscodeOptions`.
    `print_transcode_plan` doesn't surface any of these fields either, so
    users get no warning that their seek/duration/filter/thread/resume/map/
    normalize flags did nothing. Because `oximedia ff` and `oximedia-ff`
    both build a `TranscodeOptions` and call the same
    `transcode::transcode()` (see `ffcompat_cmd.rs::execute_job` /
    `bin/oximedia-ff.rs::execute_job`), real FFmpeg command lines using
    `-ss`/`-t`/`-vf`/`-af` are silently executed as a full-file, unfiltered
    transcode. Tracked as a new gap under "Subcommand-level gaps" below.
  - **Update (2026-07-14, later same day):** `--normalize-audio` has
    SHIPPED — `main.rs::run()` destructures `normalize_audio` (no
    underscore) straight into `TranscodeOptions.normalize_audio`, and
    `transcode.rs` reads it in both `transcode_single_pass` (~line 528)
    and `transcode_two_pass` (~line 624) to apply a real
    `NormalizationConfig::new(LoudnessStandard::EbuR128)` pass. It is no
    longer part of this gap. `-vf`, `-af`, `-ss`, `-t`, `-r`, `--threads`,
    `--resume`, and `--map` remain open — see "CLI Flag Wiring
    Investigation (2026-07-14)" below for per-flag design notes.
- [x] Frame / thumbnail / sprite generation — `extract`, `thumbnail` (single /
  multiple / grid / auto), `sprite` with WebVTT + JSON manifest, configurable
  sampling strategy (uniform / scene-based / keyframe-only / smart) and layout
  modes.
- [x] Batch processing — both the ad-hoc `batch` subcommand (TOML config, `-j`
  jobs, `--continue-on-error`, `--dry-run`) and the persistent SQLite-backed
  `batch-engine submit/status/list/cancel/report` command.
- [x] 0.1.2 additions: `loudness` (EBU R128 analyze / check / standards / info),
  `quality` (PSNR/SSIM/BRISQUE/NIQE etc. compare/analyze/list/explain),
  `dedup` (scan/report/clean/hash/compare), `timecode` (convert/calculate/
  validate/burn-in), `normalize` (analyze/process/check/targets),
  `batch-engine`, `scopes`, `workflow`, `version`.
- [x] ~85 domain subcommand modules wired end-to-end through `commands.rs` →
  `main.rs` match arms → `*_cmd.rs::handle_*_command` handlers, every one
  honouring the global `--json` flag.
- [x] Broadcast & live production coverage — `playout`, `switcher`, `ndi`,
  `videoip`, `multicam`, `routing`, `virtual`, `calibrate`, `timesync`.
- [x] MAM / archival coverage — `mam`, `search`, `dedup`, `archive`, `archive-pro`,
  `proxy`, `clips`, `review`, `drm`, `rights`, `access`, `watermark`, `repair`,
  `restore`.
- [x] Production post coverage — `timeline`, `edl`, `conform`, `qc`, `vfx`,
  `graphics`, `image`, `audiopost`, `mixer`, `mir`, `subtitle`, `captions`,
  `color`, `lut`, `dolby-vision`.
- [x] Infrastructure — `distributed`, `farm`, `renderfarm`, `cloud`, `monitor`,
  `profiler`, `plugin`, `stream`, `quality`, `recommend`, `scaling`, `optimize`.
- [x] FFmpeg compatibility layer — both `oximedia ff <args>` and the standalone
  `oximedia-ff` binary, both wired to `oximedia-compat-ffmpeg::parse_and_translate`
  with patent-codec auto-substitution, diagnostic forwarding, and `--dry-run`.
- [x] Interactive TUI — `oximedia tui` launches a three-tab ratatui UI with
  cwd file browser, command reference with descriptions, and about panel.
- [x] Shared `handlers.rs` — `init_logging` respects `-v` count and `-q`,
  `probe_file`, `show_info`, `show_version` with feature-gated build info.
- [x] Coloured error output — every failure is reported via `colored` as
  `Error: …` in red + `Caused by: …` chain from `anyhow::Error::source()`.
- [x] Progress reporting harness — `progress.rs` (397 lines) built on
  `indicatif` for transcode / batch / analyze long-running operations.
- [x] Preset system — `presets/` module (builtin, custom, device, streaming,
  validate, web) plus `preset list/show/create/template/import/export/remove`
  subcommand; preset doctest defects fixed during 0.1.2.
- [x] Bug fixes shipped during 0.1.2 — archive_cmd compile errors, farm_cmd
  async issues, search_cmd missing types, and the preset-module doctest.
- [x] `build.rs` minimised to `fn main() {}` after workspace-level linker
  script (`.cargo/config.toml`) took over glibc 2.38+ `__isoc23_*` symbol
  compat (keeps crate-specific build config available for the future).
- [x] WASM / non-CLI surface explicitly NOT linked — the crate only compiles
  for native targets by design; `oximedia-wasm` handles the browser story.
- [x] Pure-Rust TLS crypto provider installed process-wide at startup —
  `oximedia_net::install_default_crypto_provider()` (rustls-rustcrypto) is
  called first thing in all three binaries (`main.rs`, `bin/oximedia-ff.rs`,
  `bin/oximedia-cv2.rs`) before any subcommand can open a TLS connection
  (cloud storage, distributed rendering, monitoring exporters). The
  workspace builds `reqwest`/`rustls` without a compiled-in default
  provider to keep the default build Pure Rust, so without this call the
  first TLS use would panic. Landed 2026-07-10 in commit `eaf9dfdd`
  (untracked in this TODO until the 2026-07-14 content audit).

---

## Enhancements `[ ]`

### Shell integration & packaging

- [x] Add shell completions for bash/zsh/fish/powershell/elvish
  - **Goal:** `oximedia completions <shell>` emits completion script to stdout
  - **Design:** `clap_complete::generate(shell, &mut cmd, "oximedia", &mut io::stdout())` with Generator impls for Bash/Zsh/Fish/PowerShell/Elvish
  - **Files:** `src/completions_cmd.rs` (new), `src/commands.rs`, `src/main.rs`, `Cargo.toml`
  - **Tests:** `tests/completions_smoke.rs` — each shell variant produces non-empty output
- [x] Generate Unix man page (oximedia.1)
  - **Goal:** `oximedia man-page` emits roff to stdout
  - **Design:** `clap_mangen::Man::new(cmd).render(&mut buf)?`
  - **Files:** `src/man_cmd.rs` (new), `src/commands.rs`, `src/main.rs`, `Cargo.toml`
  - **Tests:** `tests/man_smoke.rs` — output starts with `.TH OXIMEDIA`
- [x] Implement `oximedia doctor` environment diagnostics command
  - **Goal:** Reports Rust version, GPU adapters, temp dir space, OXIMEDIA_TEMP writability
  - **Design:** `DoctorReport { rust_version, gpu_adapters, temp_dir_space, temp_writability }` — enumerate_adapters (sync) + statvfs + write-delete probe; `--json` flag
  - **Files:** `src/doctor_cmd.rs` (new), `src/commands.rs`, `src/main.rs`
  - **Tests:** `tests/doctor_smoke.rs` — exits 0, `--json` produces valid JSON
- [ ] `cargo-dist` / GitHub Actions pipeline producing signed pre-built binaries
  (x86_64-linux-gnu, x86_64-linux-musl, aarch64-linux, x86_64-darwin,
  aarch64-darwin, x86_64-windows-msvc) plus shasums and SBOMs.
- [ ] Platform packaging — Homebrew formula, Windows `winget`/Scoop manifest,
  Debian/Ubuntu `.deb`, RPM for Fedora/RHEL, Arch Linux AUR `PKGBUILD`.

### Output & piping

- [x] Audit every *_cmd.rs accepting --json for strict JSON output (no mixed colored text)
  - **Goal:** When --json is set, stdout is pure parseable JSON; colored text goes to stderr only
  - **Design:** For each of scopes_cmd, graphics_cmd, timeline_cmd, workflow_cmd, vfx_cmd, review_cmd, collab_cmd — ensure colored output is behind `!json_output` guards; `colored::control::set_override(false)` when json_output
  - **Files:** `src/output.rs` (new), 7 cmd files, `src/main.rs`
  - **Tests:** `tests/json_strict.rs` — each of 7 commands with --json produces parseable JSON stdout
- [x] Add --ndjson streaming output for probe/quality/loudness/monitor commands
  - **Goal:** One NDJSON record per file/frame/window, flushed immediately
  - **Design:** `NdjsonWriter<W: Write>` with `emit<T: Serialize>` + `flush`; global `--ndjson` flag `conflicts_with("json")`
  - **Files:** `src/output.rs` (new), `src/main.rs`, `src/probe_cmd.rs`, `src/quality_cmd.rs`, `src/loudness_cmd.rs`, `src/monitor_cmd.rs`
  - **Tests:** `tests/ndjson_probe.rs`, `tests/json_ndjson_conflict.rs`
- [x] Standardise exit codes (0=Ok, 1=GenericError, 2=UsageError, 3=IoError, 4=ValidationError)
  - **Goal:** All CLI errors emit the correct numeric exit code
  - **Design:** `pub enum ExitCode { Ok=0, GenericError=1, UsageError=2, IoError=3, ValidationError=4 }` + `From<&anyhow::Error>` classifier
  - **Files:** `src/exit_codes.rs` (new), `src/main.rs`
  - **Tests:** `tests/exit_codes.rs` — nonexistent file → code 3; unknown subcommand → code 2
- [x] Add `--log-format json` global flag via tracing-subscriber JSON layer
  - **Goal:** Structured JSON log output for pipeline/automation consumers
  - **Design:** `LogFormat { Plain, Json }` enum; `init_logging` accepts `log_format`; Json path uses `tracing_subscriber::fmt::layer().json()`
  - **Files:** `src/handlers.rs`, `src/main.rs`
  - **Tests:** inline — `--log-format json` produces valid JSON log lines

### `oximedia-ff` / `ffcompat` coverage

- [x] Expand the FFmpeg flag surface handled by `oximedia-compat-ffmpeg`
  - **Goal:** `-filter_complex`, `-map_metadata`, `-hwaccel`, `-threads`, `-g`/`-keyint_min`, `-hide_banner`, `-stats`, `-progress`, `-nostats`, `-loglevel` all reach the translator; wire existing dead-code `metadata_compat.rs` and `hwaccel_compat.rs` modules
  - **Design:** Add `pub mod metadata_compat; pub mod hwaccel_compat;` to `lib.rs`; wire into `translator.rs`; add `-map_metadata` arm to `arg_parser.rs`; deepen `filter_complex.rs` with quoted-string and escape lexer
  - **Files:** `crates/oximedia-compat-ffmpeg/src/lib.rs`, `translator.rs`, `arg_parser.rs`, `filter_complex.rs`
  - **Tests:** `crates/oximedia-compat-ffmpeg/tests/hwaccel.rs`, `map_metadata.rs`; inline filter_complex parser tests for quoted strings and escapes
- [x] Document and test the 75+ codec / 30 format mappings
  - **Goal:** Rustdoc in `oximedia-compat-ffmpeg/src/lib.rs` lists all codec/format mappings with FFmpeg-side names → OxiMedia codec IDs
  - **Design:** Pure doc expansion; no runtime changes; "test against real ffmpeg" deferred to a follow-up pass (requires real ffmpeg in CI)
  - **Files:** `crates/oximedia-compat-ffmpeg/src/lib.rs`
  - **Tests:** `cargo doc --no-deps -p oximedia-compat-ffmpeg` must succeed with zero warnings
- [x] Add an `oximedia-ff --explain <args>` mode
  - **Goal:** Prints translation table (input flag → OxiMedia option) without executing transcode
  - **Design:** `--explain` flag on top-level Cli; when present, run translation but skip `oximedia_transcode::run`; emit rows like `Input → -i in.mp4 → input_path = "in.mp4"`
  - **Files:** `oximedia-cli/src/ffcompat_cmd.rs`, `oximedia-cli/src/bin/oximedia-ff.rs`
  - **Tests:** `oximedia-cli/tests/explain.rs` — exits 0, contains "Translation table", no file written
- [x] Ship an opt-in `oximedia-cv2` companion binary (planned 2026-05-05, completed 2026-05-05)
  - **Goal:** Companion binary `oximedia-cv2` using the new `oximedia-compat-cv2` crate; 14 named-arg subcommands + introspection flags (`--list-functions`, `--list-constants`, `--explain`)
  - **Design:** clap-derive; subcommands: `Imread`, `CvtColor`, `Resize`, `GaussianBlur`, `Canny`, `Threshold`, `Sobel`, `Erode`, `Dilate`, `FindContours`, `HoughLines`, `EqualizeHist`, `LkFlow`, `Probe`; global `--list-functions` (≥30 entries table), `--list-constants` (≥130 entries), `--explain` (dispatch table, no execution)
  - **Files:** `oximedia-cli/src/bin/oximedia-cv2.rs`, `oximedia-cli/Cargo.toml` (add `[[bin]]` + dep on `oximedia-compat-cv2`), `oximedia-cli/tests/cv2_smoke.rs`, `cv2_list.rs`, `cv2_imread_imwrite.rs`, `cv2_cvt_color.rs`, `cv2_explain.rs`
  - **Tests:** smoke (`--version`/`--help` exit 0), list (functions ≥30 / constants ≥130), imread+imwrite PNG round-trip via assert_cmd, cvt-color BGR2GRAY output is 1-ch, explain does NOT write output file
  - **Risk:** binary size increase; acceptable — users can opt out at build time with `--bins oximedia`
  - **Prerequisite:** `oximedia-compat-cv2` crate (Slices A-E)
  - **Note (verified 2026-07-14):** the binary now ships 22 subcommands
    (grew via Run 4 Slices C/E/F below). 3 of the originally-listed 14 —
    `FindContours`, `HoughLines`, `LkFlow` — are not yet wired as CLI
    subcommands, even though the underlying `oximedia-compat-cv2` functions
    exist (`contour.rs`, `hough.rs`, `optical_flow.rs`); many additional ops
    (`Flip`, `Rotate`, `Sobel`, `Laplacian`, `AdaptiveThreshold`,
    `MorphologyEx`, `FastCorners`, `HarrisCorners`, `OrbDetect`,
    `DnnForward`, `MedianBlur`, `BilateralFilter`) shipped instead. The
    Goal/introspection-flags claims are otherwise accurate.

### TUI polish (`tui_cmd.rs`)

- [x] Replace the placeholder file-info string with a real mini-probe render
  - **Goal:** `App::on_enter()` triggers real probe; right pane shows codec, resolution, duration, bitrate
  - **Design:** Spawn background thread with sync `std::fs::File::read` → `MultiFormatProber::probe()`; send via `std::sync::mpsc::sync_channel`; drain in event loop tick via `poll_probe_result()`; render via `ratatui::widgets::Paragraph`
  - **Files:** `oximedia-cli/src/tui_cmd.rs`
  - **Tests:** `format_probe_info_full`, `format_probe_info_audio_only` inline unit tests
- [x] Add an actionable "run command" tab
  - **Goal:** Commands tab lets user prefill arguments and dispatch into real subcommand; result shown in scrollable pane
  - **Design:** `CommandTabState::Browsing/InputArgs` enum; Enter enters input mode; second Enter spawns `std::process::Command::new(current_exe())` capturing stdout+stderr; shown in scrollable Paragraph
  - **Files:** `oximedia-cli/src/tui_cmd.rs`
  - **Tests:** `command_tab_enters_input_mode_on_enter`, `command_tab_esc_in_input_mode_goes_back`, `command_input_accumulates_chars`
- [x] Mouse support, PgUp/PgDn, `/` incremental search
  - **Goal:** Mouse scroll changes selection; PgUp/PgDn jumps 10; `/` enters search mode filtering file_list case-insensitively; Esc exits search
  - **Design:** `EnableMouseCapture`/`DisableMouseCapture` in setup/teardown and panic hook; `Event::Mouse(ScrollUp/Down)` → `select_previous/next()`; `PageUp/Down` → 10× loop; `search_mode: bool` + `search_query: String`; key routing checks search mode first to prevent 'q'/Tab from leaking
  - **Files:** `oximedia-cli/src/tui_cmd.rs`
  - **Tests:** `search_filter_case_insensitive`, `search_filter_uppercase_query`, `search_esc_clears_filter`, `search_empty_query_shows_all`, `pgup_moves_selection_by_10`, `pgdn_moves_selection_by_10`
- [x] Persist cwd navigation (Enter on directory descends)
  - **Goal:** Enter on a directory entry descends into it; Backspace pops the stack
  - **Design:** `cwd_stack: Vec<PathBuf>` pushed on descend, popped on `ascend_dir()`; `filtered_file_list` refreshed via `refresh_file_list()` → `load_dir_files()`; persisted to `$XDG_STATE_HOME/oximedia/tui.json` (best-effort via `persist_cwd`)
  - **Files:** `oximedia-cli/src/tui_cmd.rs`
  - **Tests:** `descend_pushes_cwd_stack`, `ascend_pops_cwd_stack`

### Subcommand-level gaps (direct `grep` findings)

**0.2.1 status (re-harvested 2026-08-12).** `rg -n "TODO\(0\.2" oximedia-cli/src/`
returns exactly **four** markers, all verified present in the tree:
`archivepro_cmd.rs:514`, `captions_cmd.rs:1061`, `distributed_cmd.rs:228`,
`edl_cmd.rs:281` — detailed at the end of this subsection.

- [x] **The frame-level-pipeline root cause is closed.** A new shared
  `src/frame_harness/` module (`mod`/`adapt`/`ops`/`font`/`scale`/`text`),
  lifted out of `restore_cmd.rs`'s private stabiliser code, provides
  `process_clip` and `process_frames` (the latter genuinely streaming via
  `Y4mDemuxer::read_frame`) over a Y4M-in/Y4M-out contract. Output is
  buffered and written with a single `fs::write` after full success, so a
  failure leaves no partial file. `ClipStats.bytes_changed` is an
  anti-fabrication counter: a command that changed nothing removes its
  output and refuses to report success. `restore_cmd.rs::stabilize_video_y4m`
  was re-pointed at it, deleting 289 lines of duplicated private
  `ChromaLayout`/`stabilize_planar_frames`/`warp_plane`. **Nine commands now
  do real frame work through it:**
  - `scaling scale` / `compare` / `batch` — real `Resampler` per plane
    (u8→f32→resize→f32→u8) with `FilterKernel` from `--mode`, real
    letterbox/crop, and real PSNR/SSIM via `oximedia_scaling::quality_metrics`.
  - `denoise` — real `Denoiser::process` through a `VideoFrame` bridge;
    `denoise_reduces_variance_on_real_noise` asserts both `bytes_changed > 0`
    and a measured local-variance decrease.
  - `stabilize` — config built from the command's own
    `--mode`/`--quality`/`--smoothing`/`--zoom`; `--zoom` genuinely drives a
    real `ZoomOptimizer` (proven by a pixel-level warp-difference test on
    trackable motion, and a companion test that it is correctly a no-op only
    on featureless footage).
  - `multicam color-match` — real per-angle Y4M decode → BT.709 YCbCr→sRGB →
    mean/std → `ColorMatcher::update_stats`/`calculate_corrections`, written
    to `<stem>.colormatch.json`. This replaces `ColorStats::new`, whose
    `[0.5,0.5,0.5]` / 6500 K defaults were the original fabrication.
  - `timecode burn` — `TimecodeFilter` driven through `Node::process` and
    back to planar; verified by hand through the real binary (a flat 256×64
    clip burns legible glyphs, `01:00:00:00` → `01:00:00:01`, with exactly
    191 luma pixels differing between frames — only the last digit).
  - `subtitle burn` and `captions burn` — real cue parsing, layout via
    `oximedia_subtitle::font::{Font, GlyphCache, SimpleLayoutEngine}`, per-cue
    alpha compositing; `captions` additionally positions through
    `CaptionRenderer::render_lines` + `SafeAreaInsets::standard`.

  A new **`--font <PATH>`** flag is required by every text-burning command.
  No font ships in the tree and no system-font probing is done: a missing or
  unreadable font is an honest error that names the flag. Non-Y4M input keeps
  a shared "convert first" message naming `oximedia transcode`. Content-level
  coverage lives in `tests/frame_harness_e2e.rs`; real-glyph pixel tests are
  gated behind `OXIMEDIA_TEST_FONT`. One genuine intermittent test race was
  found and fixed while verifying with a real font: `captions_cmd.rs`'s test
  module built every fixture path from a bare `std::env::temp_dir()` with
  fixed filenames.
- [x] **Thirteen single-command gaps shipped** (0.2.1). Part 1:
  `archivepro migrate` (real PNG/TIFF image migration via `oximedia_image`,
  reusing `image_cmd.rs`'s own codec path; `VideoFfv1Mkv` stays an honest
  refusal — see the marker below), `cloud upload --multipart` (real
  `upload_stream` with a size hint; all three backends treat an unknown size
  as "always chunk", so this genuinely forces multipart/block/resumable, with
  a pure `upload_size_hint()` guard against the 10,000-part ceiling above
  ~50 GB), `collab edit` (a new subcommand plus `EditEventRecord` persistence
  and real `--include-edits` JSON/CSV export), `distributed`, `drm`, `edl`.
  Part 2: `dolbyvision convert` (`--preserve-levels` gating Level-2 stripping
  on the real Profile 8→8.4 PQ→HLG rescale; every other profile pair is an
  honest `Err` naming the missing transform, never a fabricated RPU),
  `mam ingest` (real `ProxyGenerator` proxies for Y4M→MJPEG/MKV and
  WAV→FLAC; any other format is a visible stderr warning with the asset still
  cataloged and `proxy_path: None`), `renderfarm init` (real XDG-conventional
  persistent state dir via `dirs::state_dir()` → `data_local_dir()` → temp,
  with a real manifest written to disk), `switcher record` (an honest-`Err`
  message that correctly distinguishes FFV1 — a real encoder, blocked only on
  a missing capture source — from AV1/VP9/VP8, whose encoders are themselves
  fake), `transcode` preset/`--audio-bitrate` knobs, `validate`
  (EBU R103 + decode coverage), `workflow` SQLite state.
- [x] `captions_cmd.rs:114` — `generate` currently writes a placeholder caption
  track; wire to the real ASR pipeline (OxiMedia has speech alignment in
  `oximedia-caption-gen`). Done: feature-gated `caption-gen` implementation
  wiring `CaptionEncoder` → `greedy_decode` → `build_caption_blocks` →
  `optimal_break` → `CaptionTrack`. Default build returns a clear error.
- [x] `restore_cmd.rs:80,210` — audio path assumes raw PCM f32 LE; video path
  writes the input through unchanged. Wire to `oximedia-restore` decoders. (Wave 2 Slices 4+5)
  - **Goal:** Format-aware WAV/FLAC/MP3 decode for audio restore; frame-level deinterlace/upscale/color-correct for video restore via FramePipelineConfig.
  - **Files:** `restore_cmd.rs`, `commands.rs`, `crates/oximedia-transcode/src/frame_pipeline.rs`
  - **Note (verified 2026-07-14):** WAV and MP3 decode for real. FLAC input
    is honestly rejected with a clear error directing the user to convert
    to WAV or use `--raw` — `oximedia-audio`'s `FlacDecoder` is itself a
    stub upstream (`send_packet`/`receive_frame` are no-ops), so this is a
    deliberate honest-error choice, not a silent-garbage placeholder.
- [x] Wire conform_cmd to single-file QC path (replace byte-copy stub)
  - **Goal:** `conform check/report` use `oximedia-qc::QualityControl` + `oximedia-transcode::TranscodePipeline` for single-file QC. `ConformSession` is reserved for the EDL-timeline matching path (different use case).
  - **Design:** `ConformSession::new(timeline_path)?.match_assets(paths).await?` for check; `compute_statistics()` for report
  - **Files:** `src/conform_cmd.rs`, `Cargo.toml`
  - **Tests:** `tests/conform_check.rs`, inline unit tests for fallback path
- [x] Wire cloud_cmd to real oximedia-storage backends (replace simulation strings)
  - **Goal:** `cloud upload/download/list` use real S3/GCS/Azure/R2/B2 backends; error when creds missing
  - **Design:** `build_backend_for_provider(p)` dispatches to S3Backend/GcsBackend/AzureBackend constructed from env vars
  - **Files:** `src/cloud_cmd.rs`, `Cargo.toml`
  - **Tests:** `tests/cloud_no_creds.rs` — no-creds error names AWS_ACCESS_KEY_ID; inline provider dispatch tests
- [x] Extend scopes_cmd frame extraction beyond Y4M (use oximedia-container/codec)
  - **Goal:** Any supported container/codec works as scopes input, not only Y4M
  - **Design:** `async fn decode_via_demuxer<D: Demuxer>` backed by Mp4/Matroska/MpegTs demuxers + Av1/Vp9/Vp8 decoders; Y4M stays as fast path; stride-aware `repack_plane` handles padded decoder output
  - **Files:** `src/frame_extract.rs`, `src/scopes_cmd.rs`, `src/thumbnail.rs`
  - **Tests:** `tests/scopes_smoke.rs`, inline async unit tests in `frame_extract.rs`
- [x] Replace synthetic silence in normalize_cmd with real audio decode
  - **Goal:** LUFS analysis runs on actual audio content, not zeros
  - **Design:** `AudioSampleIterator::open(input)?` loop feeding `normalizer.analyze_f32(chunk?)`
  - **Files:** `src/decode_helper.rs` (new), `src/normalize_cmd.rs`
  - **Tests:** `tests/normalize_e2e.rs` — 1kHz@0dBFS WAV → LUFS ∈ [-3.5, -2.5]
- [x] `proxy_cmd.rs:302` — proxy generation writes a placeholder file; invoke
  the real low-bitrate transcode path. (Resolved: `generate_proxy_via_proxy_crate`
  now calls `oximedia_proxy::ProxyGenerator` which uses `TranscodePipeline`
  with per-codec settings; placeholder fallback removed.)
- [x] Wire metadata.rs to real oximedia-metadata readers; replace sidecar with format-specific metadata
  - **Goal:** id3v2/exif/quicktime/matroska readers; format_timestamp returns RFC 3339
  - **Design:** `detect_format(path)` → match to id3v2/vorbis/opus_tags/quicktime/matroska/exif readers; `chrono::DateTime::<Utc>::from_timestamp(secs,0).to_rfc3339()`
  - **Files:** `src/metadata.rs`, `Cargo.toml`
  - **Tests:** `tests/metadata_id3v2.rs`, `tests/metadata_exif.rs`, `tests/metadata_sidecar_fallback.rs`, inline timestamp tests
- [x] `timeline_cmd.rs:763,966` — `generate_otio_placeholder` writes minimal JSON missing RationalTime rationals, TimeRange schema, ExternalReference. (Wave 2 Slice 2)
  - **Goal:** Full OTIO 0.17-compatible JSON (Timeline/Stack/Track/Clip/Gap/RationalTime/TimeRange/ExternalReference schemas).
  - **Files:** `oximedia-cli/src/timeline_cmd.rs`
- [x] `extract.rs:296` — all frame extracts currently fall back to PPM as a
  lossless stand-in; finish the PNG/JPEG output encoders.
  (Done: real container probe + demux + codec decode pipeline; PNG via PngEncoder,
  JPEG via JpegEncoder; synthetic frame generator removed; log updated to show actual input path.)
- [x] `transcode.rs` — flag wiring RESOLVED (0.2.0 pass, final state
  verified 2026-07-15): `--map` / `-ss` / `-t` are wired for real through
  `oximedia_transcode::TranscodePipelineBuilder`
  (`stream_map`/`start_time_secs`/`duration_secs`, applied in
  `apply_trim_and_map`, transcode.rs); `-vf scale=W:H` / `-af volume=…` /
  `-r` are wired into the real frame-level pipeline
  (`video_scale`/`audio_gain_db`/`output_fps`; unsupported filters fail
  with a clear error naming the supported set); `--threads` warns and
  proceeds (nothing to parallelize; transcode.rs ~line 434); `--resume`
  was REMOVED from the CLI surface (see the investigation section below).
  Covered by `tests/transcode_trim_map.rs` and transcode.rs unit tests.
  Both `oximedia transcode` and the shared FFmpeg-compat path benefit
  since they call the same `transcode::transcode()`. **Also in the 0.2.0
  pass (2026-07-15):** `--audio-bitrate` and `--preset` — previously
  parsed, echoed in the transcode plan, and silently unused — now warn
  and proceed (no audio-bitrate knob exists in oximedia-transcode's
  lossless audio encoders; no wired encoder has a speed-preset knob), and
  their misleading plan echoes were removed.
- [x] `probe --hash` — SHIPPED. `main.rs::run()` now forwards the real
  `hash` value (previously `hash: _hash`) into `probe_file(..., hash,
  ...)`, which takes a genuine `compute_hash: bool` parameter
  (`handlers/inspect.rs:24`) threaded through to a real hash/fingerprint
  computation. (Implemented after the 2026-07-14 TODO content audit that
  originally found it discarded.)
- [x] `probe --quality-snapshot` — SHIPPED (verified 2026-07-15):
  `main.rs::run()` forwards `quality_snapshot` into `probe_file()`, which
  threads it to the real frame-0 no-reference analysis in
  `src/quality_snapshot.rs` (all 5 `oximedia-quality` metrics with
  per-metric size-guard honesty); covered by
  `tests/probe_quality_snapshot.rs`.

#### Open — the four remaining `TODO(0.2.x)` markers (verified 2026-08-12)

- [ ] `archivepro_cmd.rs:514` — `VideoFfv1Mkv` migration target. Honest
  refusal today, and deliberately so: `oximedia-transcode`'s frame level
  writes FFV1 only in its own private raw framing (`.ffv1`), never into
  Matroska, and labelling that output `.mkv` would be exactly the
  mislabelling this command refuses elsewhere. Blocked on a Matroska muxer
  that can carry FFV1. (UT Video, JPEG 2000, PDF/A and plain text are
  separate honest refusals in the same command.)
- [ ] `captions_cmd.rs:1061` — extend caption extraction to MP4 (tx3g) and
  ASS/SSA-in-Matroska.
- [ ] `distributed_cmd.rs:228` — add a state/persistence-directory field to
  the config. Related finding from the 0.2.1 pass, recorded so it is not
  re-investigated: `--watch` cannot poll a real coordinator because
  `oximedia_distributed`'s `CoordinatorServiceClient` is a stub whose RPC
  methods return a default response without touching the connected channel,
  and `Coordinator::serve` mounts no tonic service at all (it runs a plain-TCP
  text protocol instead). That is an `oximedia-distributed` gap, not a CLI one.
- [ ] `edl_cmd.rs:281` — grow real per-dialect writers in `oximedia-edl`.

### CLI Flag Wiring Investigation (2026-07-14) — CLOSED 2026-07-15

**Status update (2026-07-15, 0.2.0 wave):** every flag investigated below
has been resolved. Final dispositions:

| Flag | Disposition |
|------|-------------|
| `transcode --map` | SHIPPED — real `StreamMap` selection incl. packet `stream_index` remap in the drain loop |
| `transcode -ss` / `-t` | SHIPPED — real seek (seekable demuxers) / read-and-discard fallback + duration cutoff |
| `transcode -vf` | SHIPPED narrow — `scale=W:H` real via frame-level `video_scale`; other filters = clear error |
| `transcode -af` | SHIPPED narrow — `volume=N`/`NdB` real via `audio_gain_db`; other filters = clear error |
| `transcode -r` | SHIPPED — real `output_fps` through `FpsResamplingDecoder` on the frame-level path |
| `transcode --threads` | warn-and-proceed (single-threaded pipeline; stderr warning, transcode.rs) |
| `transcode --preset` | warn-and-proceed for non-default values (no wired encoder has a speed knob); plan echo removed (2026-07-15) |
| `transcode --audio-bitrate` | warn-and-proceed (lossless audio encoders have no bitrate knob); plan echo removed (2026-07-15) |
| `transcode --resume` | REMOVED from the CLI surface — see item 6 below |
| `probe --quality-snapshot` | SHIPPED — real frame-0 no-reference metrics (`src/quality_snapshot.rs`) |

**`--resume` disposition record — decision confirmed by the user,
2026-07-15; closed.** The user explicitly left the a)/b) choice in item 6
undecided at the time and did not want warn-and-proceed auto-applied. The
0.2.0 wiring pass implemented option (b): the flag is gone from
`TranscodeOptions` and the clap surface, and
`tests/transcode_trim_map.rs:326,340` assert clap now rejects it and that
`transcode --help` no longer advertises it. If resume-from-partial support
is ever wanted, re-add the flag together with real checkpoint persistence
(see the `ConversionCheckpoint` precedent noted in item 6). The user was
subsequently shown the actual diff (commit `d3d556f1`) and confirmed on
2026-07-15 that removal (option b) — the current shipped state, where
clap rejects `--resume` as an unrecognized argument — is correct and
final. **Disposition confirmed by user 2026-07-15; closed.**

---

#### Main.rs flag-threading audit — resolved 2026-07-15 (0.2.0 wave)

A second audit covered every other parsed-but-dropped flag reaching the
subcommand handlers. Dispositions (wired-real entries have tests):

| Flag | Disposition |
|------|-------------|
| `validate --loudness-check` | WIRED REAL — decodes WAV via `decode_helper::decode_wav_f32`, meters EBU R128 (`check_loudness`, src/validate.rs), reports deviation/true-peak issues; undecodable audio = visible warning issue. `tests/validate_loudness_check.rs` |
| `validate --gamut-check` | warn-and-proceed — no YUV-level analysis path exists (`src/validate.rs::validate_files`; TODO(0.2.x) for EBU R103 legal-range check) |
| `mam ingest --extract-metadata` | WIRED REAL — `MultiFormatProber` fills container format/codec/dimensions/duration on the asset record (`extract_asset_metadata`, src/mam_cmd.rs) |
| `mam ingest --generate-proxy` | warn-and-proceed — no proxy-output policy on the ingest surface; points at the real `oximedia proxy generate` (TODO(0.2.x) in src/mam_cmd.rs) |
| `mam search --date-from/--date-to` | WIRED REAL — RFC 3339 / date / epoch bounds filter on parsed ingest timestamps; `ingested_at` now written as real RFC 3339 with legacy epoch-string compat (`parse_asset_timestamp`/`parse_date_bound`) |
| `batch-engine submit --priority` | WIRED REAL — `job.set_priority(...)` persisted to SQLite (test: `test_submit_persists_priority_and_operation`) |
| `batch-engine submit --config` | WIRED REAL — `operation` (shorthand string or serde `BatchOperation` object), `inputs`, `outputs` all honoured (`parse_operation`, src/batch_cmd.rs) |
| `batch-engine list --state` | WIRED REAL — filters on the persisted DB status via `get_job_status_string`; listing now shows a State column (`state_matches`) |
| `workflow create --source/--destination` | WIRED REAL — persisted into the workflow definition JSON (`WorkflowDef.source/.destination`, serde-default for old files) and displayed |
| `workflow --db` (submit/status/list/cancel/logs/run) | warn-and-proceed — no persistence layer wired (`warn_db_unwired`, src/workflow_cmd.rs; TODO(0.2.x): back with oximedia-workflow sqlite) |
| `collab export --include-edits` | warn-and-proceed — sessions do not track edit events (TODO(0.2.x) in src/collab_cmd.rs) |
| `edl parse --format` | WIRED as validated hint — unknown names error with the accepted list; a detected-format mismatch is reported on stderr (`parse_edl_format_name`) |
| `edl export --format` | validated + warn-and-proceed for non-cmx3600 — `oximedia_edl::EdlGenerator` has a single CMX 3600 generation path (TODO(0.2.x) in src/edl_cmd.rs) |
| `drm info --license` | warn-and-proceed — no license-metadata reader exists (TODO(0.2.x) in src/drm_cmd.rs) |
| `cloud upload --multipart` | warn-and-proceed — backends auto-select multipart by file size; no force knob exposed (TODO(0.2.x) in src/cloud_cmd.rs) |
| `recommend codec --bitrate/--resolution` | WIRED REAL — resolution validated (WxH), and with both inputs a deterministic bits-per-pixel assessment section is added to text+JSON output (`assess_bitrate`, tested) |
| `distributed start-coordinator --max-workers/--data-dir` | warn-and-proceed — `DistributedConfig` has no such fields (TODO(0.2.x) in src/distributed_cmd.rs) |
| `distributed status --watch` | warn-and-proceed — needs a live coordinator connection; polling the in-process empty job table would fabricate liveness (TODO(0.2.x)) |
| `renderfarm init --data-dir` | warn-and-proceed — `CoordinatorConfig` has no state-directory field (TODO(0.2.x) in src/renderfarm_cmd.rs) |

Related honesty fixes shipped in the same pass (2026-07-15):

- `subtitle extract` now delegates to the real Matroska/WebM subtitle
  demux shared with `captions extract` (was: printed a success-looking
  banner and wrote nothing). `subtitle burn` and `timecode burn` validate
  their inputs, then fail honestly — no compositor path exists (TODO(0.2.x)
  markers in src/subtitle_cmd.rs / src/timecode_cmd.rs). Tests:
  `tests/subtitle_timecode_honesty.rs`.
- `loudness analyze` / `loudness analyze --ndjson` / `loudness check` now
  decode the file's real samples (WAV/PCM) instead of metering a block of
  synthetic silence and presenting the result as the file's
  metrics/compliance; undecodable input is an honest error (src/loudness_cmd.rs).
- `extract`'s completion summary reports the real written-frame count
  returned by `extract_frames_impl` (was: `frames.unwrap_or(100)/every`
  — a fabricated estimate).
- Global `--quiet` is now real: `progress::set_quiet/is_quiet` gate the
  status/banner stdout of transcode (plan/summary/two-pass banner),
  extract (plan/summary), and image convert; probe/audio results and
  `--json`/`--ndjson` payloads are never suppressed. Help text documents
  the exact coverage. Tests: `tests/quiet_flag.rs`.
  **Rollout status, measured 2026-08-12: PARTIAL, and the tracking marker in
  `src/progress.rs` is gone even though the sweep is not finished — do not
  read the absence of a marker as completion.** `is_quiet()` is consulted in
  **38** source files (**31** of the 75 `*_cmd.rs` handlers, plus the seven
  shared helpers `batch.rs`, `concat.rs`, `extract.rs`, `metadata.rs`,
  `progress.rs`, `thumbnail.rs`, `transcode.rs`). The other **44**
  `*_cmd.rs` handlers **do not consult `is_quiet()` at all**, so nothing
  they print can be suppressed — the largest by
  raw `println!` count are `renderfarm_cmd.rs`, `workflow_cmd.rs`,
  `mir_cmd.rs`, `videoip_cmd.rs`, `farm_cmd.rs`, `audiopost_cmd.rs`,
  `auto_cmd.rs`, `routing_cmd.rs`, `rights_cmd.rs` and `align_cmd.rs`.
  Note that a bare `println!` is **not** automatically a defect here: by
  design, command *results* and `--json`/`--ndjson` payloads are never
  suppressed, so closing this properly means classifying each site as
  status-vs-result rather than mechanically wrapping every call.
- Debug-format leaks fixed where a Display path exists: probe text/csv
  container format (src/handlers/inspect.rs), EDL format (src/edl_cmd.rs),
  benchmark codec/preset lists (join), graphics template params (k=v
  join). Left as `{:?}` (clean single-word enum Debug; Display impls
  would require foreign-crate changes outside this pass's ownership):
  src/aaf_cmd.rs:179 (`track.track_type`), src/color_cmd.rs:209-211,
  src/distributed_cmd.rs (`JobStatus`), src/farm_cmd.rs (`Priority`),
  src/repair_cmd.rs:330 (`issue_type`), src/virtual_cmd.rs
  (`WorkflowType`/`QualityMode`), src/profiler_cmd.rs (`ProfilingMode`).
- `Cargo.toml`: `doc = false` on all three `[[bin]]` targets (the
  `oximedia` bin's rustdoc collided with the `oximedia` facade library
  docs — cargo #6313).
- Dependency note: `oximedia-workflow` (already unused before this pass)
  and `oximedia-recommend` (its decorative `RecommendationEngine::new()`
  construction was removed with the bitrate-assessment wiring) are
  currently unreferenced in `src/`; both stay declared as the planned
  backing for the workflow-persistence and recommend-engine TODO(0.2.x)
  items above. Revisit during the next /unused-deps audit if those plans
  change.
- `README.md`: replaced the fabricated sectioned batch-TOML schema with
  the real flat `BatchConfig` schema (`patterns` required), replaced the
  invented exit codes 4 "Unsupported format"/5 "Patent codec" with the
  real `OxiExitCode` table (0-4), rewrote transcode examples to the real
  re-encode matrix (old examples showed VP9/AV1 encodes that error at
  runtime), fixed `probe -V` → `--detail`, documented `--quiet`'s real
  coverage, and refreshed the global-options table.

---

Original investigation notes follow (kept for the record; the design
details below informed the shipped implementations).

Design-level follow-up on the six flags left open above (`transcode`'s
`-vf`/`-af`/`-ss`/`-t`/`-r`/`--threads`/`--resume`/`--map`, and `probe
--quality-snapshot`). Each entry carries enough file:line detail and code
sketches for a future session to implement directly without
re-investigating; every reference below was independently re-verified
against the current source during this pass. (`--normalize-audio` and
`probe --hash` are NOT covered here — both shipped; see "Completed" above.)

#### 1. `--map` (stream selection) — Medium, ready to implement

- New module `crates/oximedia-transcode/src/stream_map.rs`, exported from
  `lib.rs`:
  ```rust
  pub enum StreamKind { Video, Audio, Subtitle }
  pub enum StreamMapSelector { All, Index(usize), Kind(StreamKind), KindIndex(StreamKind, usize) }
  pub struct StreamMap { pub negative: bool, pub selector: StreamMapSelector }
  impl StreamMap { pub fn parse(s: &str) -> Result<Self> }  // ffmpeg-style "0", "0:0", "0:v", "0:a:1", "-0:a"
  pub fn resolve_stream_selection(streams: &[StreamInfo], maps: &[StreamMap]) -> Result<Vec<usize>>;
  ```
  Semantics: empty `maps` = keep all (today's behavior); positive selectors
  union, negative selectors subtract; a positive selector matching nothing
  is an error, as is a final empty selection when `maps` was non-empty.
- Why a new parser instead of reusing
  `oximedia_compat_ffmpeg::arg_parser::parse_map_spec`: that fn is private
  (`arg_parser.rs:746`, confirmed `fn`, not `pub fn`), and
  `oximedia-transcode` cannot depend on `oximedia-compat-ffmpeg` — the
  dependency runs the other way (`oximedia-compat-ffmpeg/Cargo.toml` →
  `oximedia-transcode`), so the reverse edge would cycle. Only the CLI
  sees both crates.
- New fields: `PipelineConfig.stream_map: Vec<StreamMap>` (`pipeline.rs`
  struct, lines 213-234) and a matching `TranscodePipelineBuilder`
  field/setter (struct 1174-1184, setters 1189-1268, `build()`'s
  `PipelineConfig { .. }` literal at 1288-1298 needs `stream_map:
  self.stream_map,` added). Two in-file test literals (`PipelineConfig {`
  at lines 1360 and 1622) also need the new field.
- **Critical correctness detail — the single most important finding of
  this investigation; don't skip it in implementation.** Filtering must
  happen in TWO places:
  1. At the stream-gather point in `remux()` — `pipeline.rs:723`, `let
     streams: Vec<StreamInfo> = demuxer.streams().to_vec();`. Filter via
     `resolve_stream_selection`, then build an `index_remap:
     HashMap<original_index, new_sequential_index>`.
  2. Because **both the Matroska and Ogg muxers route `write_packet` by
     the packet's position in the muxer's own stream list, not by
     original stream identity** — confirmed in both `write_packet` impls:
     `mux/matroska/writer.rs:731` (`if packet.stream_index >=
     self.streams.len()`), `:738` (`&self.streams[packet.stream_index]`),
     `:776` (`(packet.stream_index + 1) as u64` into
     `write_simple_block`); `mux/ogg/writer.rs:260,267,274` (identical
     pattern, plus `self.stream_writers[packet.stream_index]`). Matroska's
     `write_tracks` (`writer.rs:573-588`) numbers tracks via
     `self.streams.iter().enumerate()` — muxer-local position, unrelated
     to the original demuxer stream index. So `drain_packets_with_gain`
     (defined `pipeline.rs:975`, called from the Matroska/Ogg arms of
     `remux()` at `:805` and `:836`) must *also* skip packets whose
     original `stream_index` was filtered out, and rewrite
     `pkt.stream_index` to the new sequential index before
     `muxer.write_packet()` — otherwise the muxer returns `Err("Invalid
     stream index: {n}")` on the first orphaned packet.
- CLI wiring: add `TranscodeOptions.map: Vec<String>`
  (`oximedia-cli/src/transcode.rs`), parsed via `StreamMap::parse` and
  passed to the builder in `transcode_single_pass`/`transcode_two_pass`.
  Currently discarded at `main.rs:426` (`map: _map`) — forward it instead.
  Other `TranscodeOptions { .. }` construction sites (`batch.rs`,
  `ffcompat_cmd.rs`, `bin/oximedia-ff.rs`, the test helper
  `tests/transcode_normalize_audio.rs`) need `map: Vec::new()` added to
  keep compiling.
- Optional follow-up: `oximedia-cli/src/concat.rs:210-223` has an
  identical unwired `StreamSelection` enum (`#[allow(dead_code)]`,
  hardcoded `stream_selection: None` in `main.rs`'s concat arm) that could
  reuse the same `resolve_stream_selection` resolver.
- Test plan: new `crates/oximedia-transcode/tests/remux_map_seek.rs` —
  build a real 2-stream (video+audio) Matroska fixture in
  `std::env::temp_dir()`, run with `stream_map` selecting only audio,
  re-demux the output and assert only 1 stream remains and every packet's
  `stream_index == 0`. That specific assertion proves the remap works —
  without it the muxer would error instead.

#### 2. `-ss` / `-t` (seek / trim) — Medium, ready to implement

- New fields `PipelineConfig.start_time_secs: Option<f64>` /
  `.duration_secs: Option<f64>` — parsed seconds, not raw strings (same
  dependency-direction constraint as `--map` applies).
- CLI-side parsing: reuse `oximedia_compat_ffmpeg::parse_duration(&str) ->
  Result<Duration, SeekError>` (`crates/oximedia-compat-ffmpeg/src/seek.rs:63`,
  re-exported from the crate's `lib.rs`; already handles
  `HH:MM:SS[.ms]`/plain seconds/`Nh`/`Nm`/`Ns`) rather than writing a new
  time parser.
- Seek insertion point: in `remux()`, once streams/remap are resolved and
  before `match out_format` (~`pipeline.rs:789`). If `start_time_secs` is
  set and `demuxer.is_seekable()`, call `demuxer.seek_to_time(s).await` —
  a real trait method (`crates/oximedia-container/src/demux/traits.rs:109`,
  default impl `self.seek(SeekTarget::time(timestamp)).await`), with real
  overrides in Matroska (`demux/matroska/mod.rs:1025` `seek`, `:1030`
  `is_seekable`) and Ogg (`demux/ogg/mod.rs:511` `seek`, `:516`
  `is_seekable`). WAV (`demux/wav/mod.rs`) and FLAC (`demux/flac/mod.rs`)
  override neither — confirmed by grep — so both fall back to the trait
  defaults (`seek` → `Err(unsupported)`, `is_seekable` → `false`,
  `traits.rs:91-95,158-160`). For those, fall back to read-and-discard:
  pass a `start_discard_secs` into the drain loop and `continue` past
  packets with `pkt.timestamp.to_seconds() < start_discard_secs`.
- Duration: compute `end_secs = start_secs.unwrap_or(0) + duration_secs`
  (or just `duration_secs` if no start given); pass into the drain loop;
  `break` once `pkt.timestamp.to_seconds() >= end_secs`.
  `Timestamp::to_seconds()` (`crates/oximedia-core/src/types/timestamp.rs:106`)
  already accounts for each packet's own timebase, so no manual
  per-stream timebase math is needed.
- Test plan: single-video-stream Matroska fixture with known packet PTS
  spacing — assert `duration_secs` alone reduces output packet count;
  assert `start_time_secs` alone (real seek path) raises the minimum
  retained PTS; add a WAV-input case to exercise the read-and-discard
  fallback specifically (WAV packets carry monotonic sample-based PTS, so
  the fallback is sample-accurate there).

#### 3. `probe --quality-snapshot` — Medium, ready to implement

- Decode **frame 0 only** (not middle/last — `frame_extract.rs`'s
  container decode path has no seek and decodes sequentially from the
  start, so anything past frame 0 isn't "quick"). Call the existing
  `crate::frame_extract::extract_video_frame_rgb(path, 0)`
  (`oximedia-cli/src/frame_extract.rs:38`) — already used by
  `scopes_cmd`/thumbnail; real decode (Y4M native; MKV/WebM/TS via
  demuxer + AV1/VP9/VP8).
- **Do not copy** `quality_cmd.rs::make_grey_frame()`
  (`oximedia-cli/src/quality_cmd.rs:160-166` —
  `Frame::new(..).luma_mut().fill(128)`) — it scores a synthetic
  constant-grey frame regardless of real input, and `--quality-snapshot`'s
  help text explicitly promises "no-reference metrics" on the real file.
- Conversion: confirmed by reading each metric's source that all 5
  no-reference metrics (`crates/oximedia-quality/src/{niqe,brisque,
  blockiness,blur,noise}.rs`) read only `frame.planes[0]` (luma) — so no
  full RGB→YUV conversion is needed. Build a `PixelFormat::Gray8`
  single-plane `quality::Frame` (`Frame::new(w, h, Gray8)`,
  `crates/oximedia-quality/src/lib.rs:177`) and fill it with BT.601 luma,
  `y = (299*r + 587*g + 114*b) / 1000` per pixel — matches the existing
  integer formula in `oximedia_convert::color_convert::rgb_to_yuv`
  (`crates/oximedia-convert/src/color_convert.rs:36`); replicate the
  one-line formula locally rather than adding `oximedia-convert` as a
  dependency (`oximedia-cli` doesn't otherwise depend on it).
- Compute all 5 `MetricType` variants (Blur/Noise/Blockiness/Brisque/Niqe)
  via `QualityAssessor::new().assess_no_reference(&frame, metric)`
  (`crates/oximedia-quality/src/lib.rs:391,444`), each **independently
  optional** — different minimum-size guards (Blur/Noise 8×8, Blockiness
  16×16, Brisque 32×32, Niqe 96×96) mean small frames legitimately drop
  some metrics while keeping others.
- New module `oximedia-cli/src/quality_snapshot.rs`: `QualitySnapshot`
  struct (`available: bool`, `reason: Option<String>` for the
  whole-frame-failed case, `frame_index`/`width`/`height`, one
  `MetricOutcome { score: Option<f64>, unavailable: Option<String> }` per
  metric); `compute_quality_snapshot(path) -> QualitySnapshot` is
  **infallible** (never returns `Err` — encodes failure inside the struct
  so `probe` never crashes on e.g. an audio-only file), plus a `to_json()`.
- Wiring: `probe_file()` (`oximedia-cli/src/handlers/inspect.rs:20-29`)
  gains a `quality_snapshot: bool` param, inserted right after the
  existing `compute_hash: bool` param the `--hash` work added.
  `main.rs:385` currently discards it (`quality_snapshot:
  _quality_snapshot`) — forward it instead. Slot the result into all 4
  output branches (ndjson ~`:81`, json ~`:104`, csv ~`:147`, text
  ~`:175`), following the same insertion pattern `--hash`'s
  `hash_hex`/`hash` field used in each branch.
- Test plan: new `oximedia-cli/tests/probe_quality_snapshot.rs` modeled on
  `tests/probe_hash_flag.rs`. Build a small (96×96, so all 5 metrics
  clear their size guards) Y4M fixture with a non-constant gradient
  pattern in `std::env::temp_dir()`; assert real, non-constant scores come
  back. **Key anti-regression assertion**: independently score a
  solid-grey 96×96 frame in-process and assert the fixture's score
  differs from it — proves a real frame was used, not the
  `make_grey_frame` shortcut. Second case: a WAV (audio-only) fixture
  asserting `probe` still exits 0 with `quality_snapshot.available ==
  false` and a clear `reason`.

#### 4. `--threads` — blocked, no live knob exists

`PipelineConfig`/`TranscodePipelineBuilder` (`pipeline.rs`) have **zero**
thread-related fields today (confirmed by grep — zero occurrences of
"thread" in the whole file); the live remux path is a sequential
packet-copy loop with nothing to parallelize. A stored-but-unused field
would just relocate the "silently does nothing" problem. Recommended
treatment once picked up: **warn and proceed** — print a clear message
that threading has no effect until real frame-level encoding exists,
rather than faking support (user's stated general preference for
currently-inert flags).

#### 5. `-vf` / `-af` / `-r` (video/audio filters, frame rate) — Large, architecture-blocked

The most important finding to preserve. The frame-level
decode→filter→encode capability these flags need
(`oximedia_transcode::MultiTrackExecutor` + `PerTrack`,
`crates/oximedia-transcode/src/multi_track.rs`) has real, working pieces —
`FilterGraph::apply` does real pixel-level scale/gain, `FrameRateConverter`
does real frame duplication/dropping — but:
- **All 8 of its tests** (`crates/oximedia-transcode/tests/pipeline_execute.rs`)
  run against `MockDecoder`/`MockEncoder` with hand-built synthetic
  frames, never a real codec or real file.
- **Zero production call sites anywhere in the workspace** — `grep
  "MultiTrackExecutor::new"` across the whole repo returns exactly 8
  test-site hits (`tests/pipeline_execute.rs`) plus 2 comment/doc-example
  hits (`multi_track.rs:280` doc comment, `pipeline.rs:612` inline
  comment) — confirmed, no others.
- The only "wiring sketch" is a 3-line `rust,ignore` doc-comment block
  (`pipeline.rs:602-615`, immediately before `requires_frame_level()`
  gates the error at `:616-624`) referencing `muxer`/`decoder`/`encoder`/
  `streams` bindings that don't exist anywhere in `execute_single_pass` —
  it shows the *shape* of the call, not how to obtain a real
  decoder/encoder/muxer/streams.
- The live path (`Pipeline::execute_single_pass` → `remux()`) is
  packet-level stream-copy only; it explicitly rejects any real codec
  transcode via `requires_frame_level()` (`pipeline.rs:342-357`, checked
  at `:616-624`) for everything except `copy`/`stream-copy` and the intra
  codecs MJPEG/APV — and even those two are validation-only: the encoder
  is constructed and immediately dropped (`pipeline.rs:767`, `let
  _encoder = make_video_encoder(intra_id, &params)?;`), bytes are still
  stream-copied, not re-encoded.

**Conclusion for the record**: making `-vf`/`-af`/`-r` work is not a
flag-wiring fix — it is "build and prove real decode→encode transcoding
in oximedia-cli for the first time," a substantial, currently-unproven-
at-real-file-level undertaking with real correctness risk (nothing like
it has ever run against a real codec anywhere in this workspace).
**User's stated intent: when picked up, attempt via a Fable-model
subagent** (rather than the default model), given the scale and
exploratory nature of proving out an unproven subsystem. Until then: same
"warn and proceed" treatment as `--threads`.

#### 6. `--resume` — no real design exists, disposition confirmed by user 2026-07-15; closed

`TranscodeJob::resume()` (`crates/oximedia-transcode/src/transcode_job.rs:185-189`)
is a bare in-memory status-enum flip (`Paused`→`Running`) with no
persistence — and it's **entirely unrelated** to the CLI's `--resume`
flag, since the CLI's transcode path never constructs a `TranscodeJob` at
all. The closest real precedent in the workspace is `oximedia-convert`'s
`ConversionCheckpoint` (`crates/oximedia-convert/src/pipeline/job.rs:170-279`)
— genuine single-file, disk-persisted JSON checkpoint scaffolding
(`input_path`/`output_path`/`frames_processed`/`total_frames`/
`byte_offset`) — but it currently hardcodes `byte_offset: 0` and only
restores a proportional progress estimate, not actual resumable encoder
state. `oximedia-batch`'s `CheckpointManager`
(`crates/oximedia-batch/src/checkpoint.rs:133-140`, fields
`dir`/`max_retained`/`next_sequence`) is queue/job-ID granularity only,
not adaptable without new per-file fields. `--resume` is undocumented
anywhere user-facing except its own `--help` string ("Resume from
previous incomplete encode").

**Disposition confirmed by the user, 2026-07-15; closed.** The user was
shown the actual diff (commit `d3d556f1`) and confirmed that option (b) —
remove the flag entirely — is correct and final. Both options are
recorded below for historical context:
- (a) Reject with a clear error when passed — consistent with "warn and
  proceed" elsewhere would actually argue for a *warning* here too, but
  `--resume` implies a stronger promise (avoiding redundant work) than
  filters/threads do, so the user did not want this defaulted
  automatically; or
- (b) Remove the flag entirely (breaking CLI change, drops a `--help`
  entry) — cleanest if nobody depends on it.

### Testing

- [x] Build out tests/ directory with assert_cmd integration tests and E2E smoke suite
  - **Goal:** Non-empty tests/ with --help smoke, JSON validation, E2E decode, snapshot tests
  - **Design:** `assert_cmd::Command::cargo_bin("oximedia")` + `tempfile::TempDir`; common/mod.rs with WAV/fixture helpers
  - **Files:** `tests/cli_help.rs`, `tests/probe_json_snapshot.rs`, `tests/normalize_e2e.rs`, `tests/scopes_decode.rs`, `tests/common/mod.rs`
  - **Tests:** ~15 tests covering help smoke, JSON strict, LUFS E2E, decode smoke
- [x] End-to-end test for `oximedia-ff` against a golden set of real FFmpeg
  command lines (both passing and patent-codec-substituted).
  (completed 2026-05-06 — 71 JSON fixtures in `tests/ff_golden/*.json` driven by
  `tests/ff_golden.rs` via the new `oximedia-ff --json` structured-output flag.
  Coverage: 12 patent substitutions, 4 direct codecs + copy, 8 quality flags,
  15 single-chain video/audio filters, 3 filter_complex graphs, 5 stream
  selectors (-map / -vn / -an), 4 seek/duration flags, 3 metadata/movflags,
  2 two-pass phases, 3 hardware acceleration backends, 4 loglevel/banner
  flags, 2 format aliases, 1 overwrite, 2 combo invocations, 2 hard-error
  cases. Partial-match expectations keep the suite resilient to translator
  field additions.)
- [x] Snapshot tests for `oximedia probe --format json` on fixtures shipped
  by `oximedia-io`. (completed 2026-05-06: `tests/probe_format_json_snapshot.rs`
  builds tiny in-process magic-byte fixtures (MP4/WebM/Ogg/FLAC) into per-test
  `tempfile::TempDir`, runs `oximedia --quiet probe -i ... --format json`, and
  compares against `tests/probe_snapshots/*.json` after path/size/f32-confidence
  normalization. 8 snapshots cover 4 containers + the `--streams` / `--chapters`
  / `--metadata` / all-flags schemas. Re-baseline with `OXIMEDIA_UPDATE_SNAPSHOTS=1`.)
- [x] `assert_cmd` + `predicates` based CLI test harness that exercises
  every top-level variant of `Commands` at least once.
  *(completed 2026-05-06: `tests/cli_help_per_command.rs` covers all 90
  variants + `convert`/`ff` aliases via `oximedia <subcmd> --help` smoke
  (92 tests). `tests/cli_smoke_listings.rs` adds 10 listing-style smokes
  (`preset list`, `plugin list`, `loudness standards`, `quality list`,
  `info`, `version`, `doctor`, `doctor --json`, `man-page`,
  `completions bash`). `tests/cli_help.rs` rewritten as the root-level
  `--help` / `--version` / no-args smoke (5 tests). 107 new tests total,
  zero clippy warnings introduced.)
- [x] Add an `examples/` directory demonstrating shell-script workflows that
  chain subcommands (transcode → loudness check → package → upload via cloud).
  *(completed 2026-05-06: 8 reference scripts + README under
  `oximedia-cli/examples/` — `dailies-ingest`, `quality-check`,
  `abr-package`, `loudness-normalize`, `forensics-investigate`,
  `restore-degraded`, `live-broadcast`, `cv2-pipeline`. All scripts pass
  `bash -n` and use only verified-wired flags.)*
- [x] Core CLI smoke-test gate — `tests/cli.rs` (267 lines; landed
  2026-07-10 in commit `eaf9dfdd`, untracked in this TODO until the
  2026-07-14 audit): `--version`/`version` agreement with the workspace
  version (including `version --json`), `--help` mentions key subcommands,
  invalid-subcommand handling (no panic, helpful stderr with usage
  guidance), and a synthetic tiny-WAV `probe` / `probe --format json` round
  trip plus a missing-file probe error path — all hermetic via
  `tempfile::TempDir`.

### Performance & UX

- [ ] Lazy subcommand loading — the single `main.rs` match is 500+ arms; as
  the CLI grows, consider switching to `clap_derive` with `#[command(flatten)]`
  into per-domain enums so `--help` per-domain is faster to render.
  (Deferred with the commands.rs 1442-line split refactor — they belong together.)
- [x] Colour-aware `--no-color` auto-detection
  - **Goal:** `NO_COLOR` (any value), `CLICOLOR=0`, `TERM=dumb`, explicit `--no-color` flag all disable coloured output; flag wins over env
  - **Design:** `pub fn init_color(force_no_color: bool)` in `handlers.rs`; called early in `main` before any `colored::Colorize` use; compatible with existing `--json`/`--ndjson` disable-color logic from Run 1
  - **Files:** `oximedia-cli/src/handlers.rs`, `oximedia-cli/src/main.rs`
  - **Tests:** `tests/no_color.rs` — `NO_COLOR=1 oximedia probe nonexistent` has zero ANSI codes in stderr; `--no-color` flag same; inline handler unit tests
- [x] Parallelise `batch` using rayon work-stealing
  - **Goal:** `oximedia batch -j N` uses rayon ThreadPool; `-j 0` means num_cpus; result order preserved
  - **Design:** `tokio::task::spawn_blocking(move || pool.install(|| jobs.par_iter().map(run_one_job).collect()))` to avoid blocking the async runtime; `rayon.workspace = true` + `num_cpus.workspace = true`
  - **Files:** `oximedia-cli/src/batch.rs`, `oximedia-cli/Cargo.toml`, root `Cargo.toml` [workspace.dependencies]
  - **Tests:** `tests/batch_parallel.rs` — multi-job batch with `-j 0` dry-run; no panic; result order preserved
- [x] Streaming `--progress json` output
  - **Goal:** Each progress tick emits NDJSON to stderr; compatible with `2>&1 | jq` consumers
  - **Design:** `ProgressFormat { Plain, Json }` enum; `set_format()` / `new_with_format()` on `TranscodeProgress` and `BatchProgress`; `--progress` global flag in `Cli`; Json path hides indicatif bar and emits JSON to stderr
  - **Files:** `oximedia-cli/src/progress.rs`, `oximedia-cli/src/main.rs`, `oximedia-cli/src/batch.rs`, `oximedia-cli/src/transcode.rs`
  - **Tests:** `tests/progress_json.rs` — flag accepted; appears in --help; inline progress.rs unit tests

### Documentation

- [x] Expand the crate-level doc comment in `main.rs`
  - **Goal:** 85-subcommand taxonomy grouped by domain in `//!` doc comment; each subcommand gets a one-line description and "see also" link to its `*_cmd.rs` rustdoc
  - **Design:** Groups: Inspection / Transcode+Convert / Audio / Video / Broadcast+Live / MAM / Post / Infrastructure / Compat / Tooling; pure doc change, no runtime effect
  - **Files:** `oximedia-cli/src/main.rs`
  - **Tests:** `cargo doc --no-deps -p oximedia-cli` succeeds with zero warnings
- [ ] Generate a static HTML reference from clap `--help` output and publish
  to `docs.oximedia.rs` / GitHub Pages.
  (Deferred — CI-blocked: would require a new `.github/workflows/docs.yml` which is outside CI policy.)
- [x] Document the plugin search paths honoured by `plugin_cmd`
  - **Goal:** `$OXIMEDIA_PLUGIN_PATH`, loading order, feature-gate requirement documented in rustdoc
  - **Design:** Expand module-level doc comment in `plugin_cmd.rs`; cross-link from `presets/mod.rs`; pure doc change
  - **Files:** `oximedia-cli/src/plugin_cmd.rs`, `oximedia-cli/src/presets/mod.rs`
  - **Tests:** `cargo doc --no-deps -p oximedia-cli` with zero warnings

---

## Known Issues / Gaps

- **Resolved (verified 2026-07-14):** the `monitor_cmd.rs` module (430 lines)
  is dispatched via `handlers::handle_monitor_command`, which now lives in
  `handlers/dispatch.rs` and fully delegates to `monitor_cmd::run_monitor_*`
  for every `MonitorCommand` variant — no duplicate logic remains.
  `handlers.rs` no longer exists as a single file: Run 4 Slice A (below)
  split it into `handlers/{mod,logging,inspect,dispatch,preset_ui,
  reference}.rs` (957 lines total across 6 files, largest 277 lines), well
  inside the 2000-line-per-file policy.
- `commands.rs` was split 2026-05-06 into `commands/mod.rs` (1124 lines as of
  2026-07-14, holding the single 90-variant `Commands` enum) plus four
  per-domain submodules (`infrastructure.rs`/`video.rs`/`subtitle.rs`/
  `compat.rs`) for the nested sub-enums. The top-level enum cannot be
  further split — clap-derive requires one `Subcommand` enum to live in one
  definition.
- Placeholder/stub behaviours enumerated above under "Subcommand-level gaps"
  were re-verified on 2026-07-14 by grepping each referenced function/
  struct — all are now genuinely wired, working code paths (several
  explicitly removed their old placeholder/synthetic fallback). That same
  audit found two **new**, previously-untracked silent-no-op gaps, now
  tracked under "Subcommand-level gaps" above: `transcode`'s
  `-vf`/`-af`/`-ss`/`-t`/`-r`/`--threads`/`--resume`/`--map`/
  `--normalize-audio` flags, and `probe`'s `--hash`/`--quality-snapshot`
  flags — all parse successfully but have no effect on the output.
  **Update (2026-07-14, later same day):** `--normalize-audio` and
  `--hash` have since SHIPPED (see "Completed" above). The remaining open
  flags are `transcode`'s `-vf`/`-af`/`-ss`/`-t`/`-r`/`--threads`/
  `--resume`/`--map` and `probe`'s `--quality-snapshot` — ready-to-
  implement design notes for each are in "CLI Flag Wiring Investigation
  (2026-07-14)" above. **Update (2026-07-15, 0.2.0 wave):** ALL of those
  are now resolved (shipped, warn-and-proceed, or removed) — see the
  closure tables at the top of "CLI Flag Wiring Investigation
  (2026-07-14) — CLOSED 2026-07-15" above, which also cover the second
  audit of every other parsed-but-dropped flag (validate/mam/batch-engine/
  workflow/collab/edl/drm/cloud/recommend/distributed/renderfarm).
- **0.2.1 (2026-08-12).** The frame-level-pipeline root cause is closed
  (nine commands, see "Subcommand-level gaps"), thirteen single-command gaps
  shipped, and `TODO(0.2.x)` markers under `src/` are down to four. Two
  things stay open and are easy to misread as done:
  1. **`--quiet` is a partial rollout** — 38 files consult `is_quiet()`,
     44 `*_cmd.rs` handlers do not consult it at all — and its tracking marker in
     `src/progress.rs` has been removed, so a marker grep will not surface
     it. Honest counts are under "Subcommand-level gaps" above.
  2. **`--resume` is unchanged: USER-UNDECIDED, removed from the CLI
     surface.** Nothing this session touched it. See the "`--resume`
     disposition record" below for the full design question. **Do not
     implement without an explicit user decision.**
- **Resolved:** a dedicated `tests/` directory now exists (36 files,
  verified 2026-07-14) — `assert_cmd`/`predicates`-driven integration tests
  covering help/version smoke, JSON strict output, NDJSON, exit codes,
  FFmpeg golden fixtures, probe snapshots, `oximedia-cv2` subcommands,
  `doctor`, and more (see "Testing" above). In-crate `#[cfg(test)]` modules
  still exist alongside it for unit-level coverage (e.g. `loudness_cmd`,
  `batch_cmd`, `quality_cmd`, `normalize_cmd`), writing scratch files under
  `std::env::temp_dir()` per policy.

---

## Future (Post-0.2.0)

| Item                                | Rationale                                                  |
|-------------------------------------|------------------------------------------------------------|
| Shell completions (bash/zsh/fish)   | Tab-complete on 85 subcommands is a huge discoverability win. |
| Man pages via `clap_mangen`         | Offline reference; required by most Linux distro policies. |
| Signed pre-built binaries           | `cargo-dist` + GitHub Actions for all Tier-1 targets.      |
| Homebrew / winget / apt / AUR       | Distribution-channel parity with FFmpeg / MPV.             |
| Full interactive TUI mode           | Run subcommands from inside the TUI, not just browse them. |
| Expanded `oximedia-ff` flag surface | Cover `-filter_complex`, `-progress`, `-hwaccel`, metadata. |
| `oximedia-cv2` companion binary     | Opt-in OpenCV drop-in layered on `oximedia-compat-cv2`.    |
| `oximedia doctor` diagnostic        | Single-command environment audit for support channels.    |
| NDJSON streaming output             | First-class machine-readable progress everywhere.          |
| `assert_cmd`-driven integration CI  | Guarantees every subcommand stays wired as features grow.  |
| Docs site (docs.oximedia.rs)        | Auto-generated from clap metadata on every release.        |
| Crate split under `commands.rs`     | Keep files < 2000 SLOC as the subcommand count climbs.     |

---

*Last updated: 2026-08-12 — v0.2.1: frame-harness wave (nine commands made real, `--font` added), thirteen single-command gaps shipped, `TODO(0.2.x)` markers down to four, `--quiet` rollout re-measured as partial (38 files gated), `--resume` disposition unchanged (USER-UNDECIDED). Previous entry below.*

*Last updated: 2026-05-06 — v0.1.9, oximedia-cli summary (Run 4 of `/ultra oximedia-cli` LANDED 2026-05-06: handlers.rs split into per-domain submodule, doctor `--full` Phase 1 with codec matrix + plugin path validation + OXICUDA probe, oximedia-compat-cv2 `dnn` module wrapping oxionnx + ORB pipeline followups (BFMatcher / knn_match / mask), 10 new oximedia-cv2 subcommands; bookkeeping flips Refinement 8 done and closes Refinement 5 as WONT-FIX)*

---

## Refinement Proposals (added 2026-05-05 by /ultra)

### Refinement 1 — TUI polish (RESOLVED — see "TUI polish (`tui_cmd.rs`)" above)
Four items (this refinement predates the file's current name; the file is `tui_cmd.rs`, not `interactive_cmd.rs`): real mini-probe render, "run command" tab, mouse + PgUp/PgDn + `/` search, persist cwd nav. All four are implemented and checked off under "TUI polish" above; re-verified present in `tui_cmd.rs` during the 2026-07-14 content audit.

### Refinement 2 — ffcompat coverage expansion (RESOLVED — see "`oximedia-ff` / `ffcompat` coverage" above)
`-filter_complex` depth, `-map_metadata`, `-hwaccel`, `--explain` mode, `oximedia-cv2` binary. All are implemented and checked off under "`oximedia-ff` / `ffcompat` coverage" above; re-verified present during the 2026-07-14 content audit.

### Refinement 3 — Platform packaging (deferred)
`cargo-dist`, Homebrew formula, winget, .deb, RPM, AUR. Outside CI policy (only pypi-publish.yml/npm-publish.yml allowed). Defer to dedicated release-engineering pass.

### Refinement 4 — `commands.rs` 1463-line split
- [x] Done 2026-05-06 — `commands.rs` (1480 lines) replaced by `commands/` module:
  `mod.rs` (1118 lines, the 90-variant `Commands` enum + `parse_key_val`),
  `infrastructure.rs` (102 lines, `MonitorCommand`),
  `video.rs` (118 lines, `RestoreCommand`),
  `subtitle.rs` (137 lines, `CaptionsCommand`),
  `compat.rs` (75 lines, `PresetCommand`).
  All 647 tests pass; clippy clean; `cargo doc` clean.
  The `Commands` enum stays in `mod.rs` as a single 90-variant unit because
  clap-derive `#[derive(Subcommand)]` requires all variants in one definition;
  only the four nested sub-enums were movable. Re-exports keep the
  `crate::commands::Foo` import path stable for `handlers.rs`.

### Refinement 5 — End-to-end ffmpeg golden tests (closed 2026-05-06 — WONT-FIX)
Closed: depending on real `ffmpeg` execution violates the project's Pure Rust Policy (`.cargo/config.toml` and CLAUDE.md). The structured-`TranslateResult` golden suite (71 fixtures in `tests/ff_golden/*.json`, Run 3 Slice B) provides translator-level correctness coverage; pixel-byte comparison against real ffmpeg would require system ffmpeg in CI which is out of scope. If a future user requests interop validation, consider a local-only `tests/ffmpeg_interop.rs` gated by an `OXIMEDIA_FFMPEG_INTEROP=1` env var, but do not auto-run.

### Refinement 6 — Other 6 prior-UU files (verification residue)
- [x] Done 2026-05-06 — verified via `git ls-files --unmerged` (zero output); no remaining unmerged files anywhere in the workspace.
A prior `/ultra` workspace-wide run listed 7 merge-conflict files. Only `image_cmd.rs` was verified clean. The other 6 (`cpu_fallback`, `worker_health_check`, `text`, `aces`, `dash/client`, `context_manager`) were not re-verified. Run `git ls-files --unmerged` to confirm or list any remaining UU files.

### Refinement 7 — oximedia-compat-cv2: `oximedia-py` delegation refactor
After `oximedia-compat-cv2` is stable, a `/ultra cv2-py` pass refactors `crates/oximedia-py/src/cv2_compat` (7,079 LoC across 18 PyO3 modules) to delegate into the new crate. High PyPI compatibility risk. Estimated 800-1,200 LoC glue + parity tests. Defer.

### Refinement 8 — oximedia-compat-cv2: `dnn` module wiring
- [x] (Done 2026-05-06 in Run 4) — `oxionnx = "0.1.2"` already in workspace deps; feature gate `dnn = ["dep:oxionnx"]` already declared in oximedia-compat-cv2/Cargo.toml. Implementation: new `src/dnn.rs` (~480 lines) with `Net` (wrapping `oxionnx::session::Session`), `read_net_from_onnx`, `Net::forward`, `blob_from_image` (BGR→NCHW preprocessing with optional R↔B swap, mean subtract, scale), `nms_boxes` (greedy NMS by score-desc + IoU). 5 tests in `tests/dnn_smoke.rs` covering load-error path, `blob_from_image` correctness, swap_rb correctness, nms IoU correctness, nms empty/no-overlap edge cases. Wire `oximedia-cv2 dnn-forward` CLI subcommand gated `#[cfg(feature = "dnn")]`.

### Refinement 9 — oximedia-compat-cv2: `VideoCapture`/`VideoWriter` for video files
Bridge `oximedia-container::Demuxer` (async) to sync `next_frame()` via `tokio::Handle::current().block_on()`. v1 supports image sequences only. Estimated 300 LoC. Defer.

### Refinement 10 — oximedia-compat-cv2: Real OpenCV interop tests
Pixel-identical comparison against OpenCV reference output. Requires OpenCV in CI — current policy doesn't allow it. Could live in `benches/` as local-only. Defer.

### Refinement 11 — oximedia-compat-cv2: `imshow`/`waitKey` windowing
Feature-gate `windowing = ["winit", "softbuffer"]`. Not core to cv2 API; users typically use external display libs. Estimated 400 LoC. Defer.

### Refinement 12 — oximedia-compat-cv2: Compile-time constants reflection
[x] Done 2026-05-06 — build.rs syn-parses src/constants.rs and emits LIST_CONSTANTS table; binary now iterates the auto-generated table.

### Refinement 13 — oximedia-compat-cv2: ORB BRIEF descriptor
- [x] Done 2026-05-06 — full ORB BRIEF descriptor (256-bit, rotated, Gaussian-smoothed) + Hamming brute-force matcher implemented; orb_create() now returns Ok.
If Slice D's ORB lift has only keypoint detection without BRIEF descriptor extraction + matching, add BRIEF for full ORB feature-parity with cv2. Estimated 200 LoC.

## Run 4 (completed 2026-05-06)

Slice plan from `$HOME/.claude/plans/parallel-scribbling-nebula.md` (approved
2026-05-06). Six slices, all pure-Rust, all in-repo. No CI yaml, no real ffmpeg.

- [x] **Slice A** — `handlers.rs` (927 lines) → `handlers/` submodule split via splitrs
  - Output: `handlers/{mod,logging,inspect,dispatch,preset_ui,reference}.rs`; old
    `handlers.rs` deleted. Re-exports preserve `crate::handlers::Foo` import paths.
- [x] **Slice B** — `oximedia doctor` Phase 1 expansion (`--full` flag)
  - Adds codec availability matrix, `OXIMEDIA_PLUGIN_PATH` validity check, `OXICUDA_HOME`
    probe. Default output unchanged. ~6 new tests.
- [x] **Slice C** — `oximedia-compat-cv2` `dnn` module + `oximedia-cv2 dnn-forward` (Refinement 8)
  - New `crates/oximedia-compat-cv2/src/dnn.rs`; wraps `oxionnx::Session` with cv2-style
    API (`Net`, `read_net_from_onnx`, `blob_from_image`, `nms_boxes`). ~7 tests.
- [x] **Slice E** — ORB pipeline follow-ups + `oximedia-cv2 orb-detect`
  - `BFMatcher` struct (cv2-idiomatic), `knn_match(k)` for Lowe's ratio test, `mask`
    parameter respected in `Orb::detect_and_compute`. ~6 tests.
- [x] **Slice F** — `oximedia-cv2` binary: 8 already-impl'd cv2 ops wired as subcommands
  - flip, rotate, sobel, laplacian, adaptive-threshold, morphology-ex, fast-corners,
    harris-corners. ~10 tests.
- [x] **Slice G** — Bookkeeping (Run 4 marker flips, Refinement 5 close, two CHANGELOGs)

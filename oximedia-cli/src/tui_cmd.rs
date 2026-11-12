//! TUI command — interactive terminal UI for OxiMedia CLI.
//!
//! Launches a ratatui-based interactive interface with file browser,
//! command reference, and system information panels.
//!
//! ## Feature additions (0.1.7)
//!
//! * **Mini-probe** — pressing Enter on a file spawns a background thread
//!   that reads the first 8 KiB and runs `MultiFormatProber::probe()`, then
//!   sends a `ProbeInfo` struct back through an `mpsc` channel that is drained
//!   on each tick.
//! * **Run-command tab** — Commands tab gains an "InputArgs" mode; Enter on a
//!   command opens an argument input line; a second Enter spawns the CLI
//!   binary as a child process (capturing stdout+stderr) and shows the result
//!   in a scrollable pane.
//! * **Mouse + PgUp/PgDn + `/` search** — `EnableMouseCapture` is added to
//!   setup; scroll events move the selection; PageUp/PageDown jump 10 rows;
//!   `/` activates an incremental case-insensitive search overlay that filters
//!   the file list.
//! * **Cwd navigation** — Enter on a directory descends; Backspace pops the
//!   stack.  The current working directory is persisted to
//!   `$XDG_STATE_HOME/oximedia/tui.json` on exit and restored on startup
//!   (best-effort, failures are silently swallowed).

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
    Terminal,
};
use std::io::Stdout;
use std::sync::mpsc;

// ── Tab identifiers ───────────────────────────────────────────────────────────

const TABS: &[&str] = &["  Files  ", "  Commands  ", "  About  "];
const TAB_FILES: usize = 0;
const TAB_COMMANDS: usize = 1;
const TAB_ABOUT: usize = 2;

// ── Probe result sent from background thread ──────────────────────────────────

struct ProbeInfo {
    container: String,
    video_codec: Option<String>,
    resolution: Option<String>,
    audio_codec: Option<String>,
    duration_secs: Option<f64>,
    bitrate_kbps: Option<u32>,
    size_bytes: u64,
}

// ── Commands tab state ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandTabState {
    /// Browsing the command list.
    Browsing,
    /// User is typing arguments for the selected command.
    InputArgs,
}

// ── Application state ─────────────────────────────────────────────────────────

struct App {
    tab_index: usize,

    // --- Files tab ---
    /// Full un-filtered list for the current directory.
    file_list: Vec<FileEntry>,
    /// Filtered view (equals `file_list` when search is inactive).
    filtered_file_list: Vec<FileEntry>,
    file_state: ListState,
    /// Directory navigation stack.
    cwd_stack: Vec<std::path::PathBuf>,
    /// Current working directory being browsed.
    current_cwd: std::path::PathBuf,

    // --- Probe background channel ---
    probe_rx: Option<mpsc::Receiver<ProbeInfo>>,
    /// Text shown in the right-pane "Details" panel.
    selected_file_info: Option<String>,

    // --- Search overlay ---
    search_mode: bool,
    search_query: String,

    // --- Commands tab ---
    command_list: Vec<(&'static str, &'static str)>,
    command_state: ListState,
    command_tab_state: CommandTabState,
    /// Arguments the user is typing for the selected command.
    command_args: String,
    /// Output lines from the last command run.
    command_output: Vec<String>,

    // --- Status bar ---
    status_message: String,
}

#[derive(Clone)]
struct FileEntry {
    name: String,
    size_bytes: u64,
    is_dir: bool,
}

impl App {
    fn new() -> Result<Self> {
        let cwd = load_persisted_cwd();
        let file_list = load_dir_files(&cwd)?;
        let filtered = file_list.clone();
        let command_list = all_commands();

        let mut file_state = ListState::default();
        if !file_list.is_empty() {
            file_state.select(Some(0));
        }

        let mut command_state = ListState::default();
        if !command_list.is_empty() {
            command_state.select(Some(0));
        }

        Ok(Self {
            tab_index: 0,
            file_list,
            filtered_file_list: filtered,
            file_state,
            cwd_stack: Vec::new(),
            current_cwd: cwd,
            probe_rx: None,
            selected_file_info: None,
            search_mode: false,
            search_query: String::new(),
            command_list,
            command_state,
            command_tab_state: CommandTabState::Browsing,
            command_args: String::new(),
            command_output: Vec::new(),
            status_message:
                "q:quit  Tab:tab  \u{2191}\u{2193}:nav  Enter:select  /:search  PgUp/Dn:jump"
                    .to_string(),
        })
    }

    /// Constructor used only in tests — bypasses filesystem access.
    #[cfg(test)]
    fn new_for_test() -> Self {
        Self {
            tab_index: 0,
            file_list: Vec::new(),
            filtered_file_list: Vec::new(),
            file_state: ListState::default(),
            cwd_stack: Vec::new(),
            current_cwd: std::env::temp_dir(),
            probe_rx: None,
            selected_file_info: None,
            search_mode: false,
            search_query: String::new(),
            command_list: all_commands(),
            command_state: {
                let mut s = ListState::default();
                s.select(Some(0));
                s
            },
            command_tab_state: CommandTabState::Browsing,
            command_args: String::new(),
            command_output: Vec::new(),
            status_message: String::new(),
        }
    }

    // ─── Tab switching ─────────────────────────────────────────────────────

    fn next_tab(&mut self) {
        // Exit any input mode before switching.
        self.command_tab_state = CommandTabState::Browsing;
        self.exit_search();
        self.tab_index = (self.tab_index + 1) % TABS.len();
        self.selected_file_info = None;
    }

    fn prev_tab(&mut self) {
        self.command_tab_state = CommandTabState::Browsing;
        self.exit_search();
        self.tab_index = (self.tab_index + TABS.len() - 1) % TABS.len();
        self.selected_file_info = None;
    }

    // ─── Navigation helpers ────────────────────────────────────────────────

    fn select_previous(&mut self) {
        match self.tab_index {
            TAB_FILES => {
                let len = self.filtered_file_list.len();
                if len == 0 {
                    return;
                }
                let i = match self.file_state.selected() {
                    Some(i) if i > 0 => i - 1,
                    Some(_) => len - 1,
                    None => 0,
                };
                self.file_state.select(Some(i));
                self.selected_file_info = None;
            }
            TAB_COMMANDS => {
                let len = self.command_list.len();
                if len == 0 {
                    return;
                }
                let i = match self.command_state.selected() {
                    Some(i) if i > 0 => i - 1,
                    Some(_) => len - 1,
                    None => 0,
                };
                self.command_state.select(Some(i));
            }
            _ => {}
        }
    }

    fn select_next(&mut self) {
        match self.tab_index {
            TAB_FILES => {
                let len = self.filtered_file_list.len();
                if len == 0 {
                    return;
                }
                let i = match self.file_state.selected() {
                    Some(i) => (i + 1) % len,
                    None => 0,
                };
                self.file_state.select(Some(i));
                self.selected_file_info = None;
            }
            TAB_COMMANDS => {
                let len = self.command_list.len();
                if len == 0 {
                    return;
                }
                let i = match self.command_state.selected() {
                    Some(i) => (i + 1) % len,
                    None => 0,
                };
                self.command_state.select(Some(i));
            }
            _ => {}
        }
    }

    // ─── Enter key ────────────────────────────────────────────────────────

    fn on_enter(&mut self) {
        match self.tab_index {
            TAB_FILES => self.on_enter_files(),
            TAB_COMMANDS => self.on_enter_commands(),
            _ => {}
        }
    }

    fn on_enter_files(&mut self) {
        let idx = match self.file_state.selected() {
            Some(i) => i,
            None => return,
        };
        let entry = match self.filtered_file_list.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let path = self.current_cwd.join(&entry.name);

        if entry.is_dir {
            // Descend into directory.
            self.cwd_stack.push(self.current_cwd.clone());
            self.current_cwd = path;
            self.refresh_file_list();
            self.status_message = format!("  Browsing: {}", self.current_cwd.display());
        } else {
            // Trigger background probe.
            self.trigger_probe(path);
            self.status_message = format!("  Probing: {}", entry.name);
        }
    }

    fn on_enter_commands(&mut self) {
        match self.command_tab_state {
            CommandTabState::Browsing => {
                // Enter input-args mode.
                self.command_tab_state = CommandTabState::InputArgs;
                self.command_args.clear();
                self.command_output.clear();
                self.status_message =
                    "  Type arguments then Enter to run, Esc to cancel".to_string();
            }
            CommandTabState::InputArgs => {
                // Run the selected command.
                self.run_selected_command();
                self.command_tab_state = CommandTabState::Browsing;
                self.status_message =
                    "  Command finished - up/down to navigate, Enter to run again".to_string();
            }
        }
    }

    // ─── Directory navigation ──────────────────────────────────────────────

    /// Reload file list from `self.current_cwd`, reset selection and filter.
    fn refresh_file_list(&mut self) {
        match load_dir_files(&self.current_cwd) {
            Ok(list) => {
                self.file_list = list;
            }
            Err(e) => {
                self.file_list.clear();
                self.status_message = format!("  Error reading dir: {e}");
            }
        }
        self.apply_search_filter();
        if !self.filtered_file_list.is_empty() {
            self.file_state.select(Some(0));
        } else {
            self.file_state.select(None);
        }
        self.selected_file_info = None;
        self.probe_rx = None;
    }

    /// Go up one directory level.
    fn ascend_dir(&mut self) {
        if let Some(parent) = self.cwd_stack.pop() {
            self.current_cwd = parent;
            self.refresh_file_list();
            self.status_message = format!("  Browsing: {}", self.current_cwd.display());
        }
    }

    // ─── Search ────────────────────────────────────────────────────────────

    /// Apply `self.search_query` filter to `self.file_list` into `self.filtered_file_list`.
    fn apply_search_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_file_list = self.file_list.clone();
        } else {
            let q = self.search_query.to_lowercase();
            self.filtered_file_list = self
                .file_list
                .iter()
                .filter(|f| f.name.to_lowercase().contains(&q))
                .cloned()
                .collect();
        }
        // Keep selection valid.
        if self.filtered_file_list.is_empty() {
            self.file_state.select(None);
        } else {
            let current = self.file_state.selected().unwrap_or(0);
            let clamped = current.min(self.filtered_file_list.len() - 1);
            self.file_state.select(Some(clamped));
        }
    }

    fn exit_search(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.filtered_file_list = self.file_list.clone();
        if !self.filtered_file_list.is_empty() {
            self.file_state.select(Some(0));
        }
    }

    // ─── Probe ─────────────────────────────────────────────────────────────

    /// Spawn a background thread that probes `path` and sends a `ProbeInfo`.
    fn trigger_probe(&mut self, path: std::path::PathBuf) {
        let (tx, rx) = mpsc::sync_channel(1);
        self.probe_rx = Some(rx);
        self.selected_file_info = Some("  Probing...".to_string());
        std::thread::spawn(move || {
            let info = probe_file_sync(&path);
            // Best-effort send; ignore if receiver is gone.
            let _ = tx.send(info);
        });
    }

    /// Drain the probe receiver if there is a pending result.
    fn poll_probe_result(&mut self) {
        if let Some(rx) = self.probe_rx.take() {
            match rx.try_recv() {
                Ok(info) => {
                    self.selected_file_info = Some(format_probe_info(&info));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Not ready yet — put receiver back.
                    self.probe_rx = Some(rx);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.selected_file_info = Some("  (probe thread error)".to_string());
                }
            }
        }
    }

    // ─── Command execution ─────────────────────────────────────────────────

    /// Run the currently selected CLI command with `self.command_args`.
    fn run_selected_command(&mut self) {
        let cmd_name = match self.command_state.selected() {
            Some(idx) => match self.command_list.get(idx) {
                Some((name, _)) => *name,
                None => return,
            },
            None => return,
        };

        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.command_output = vec![format!("Error: could not find executable: {e}")];
                return;
            }
        };

        // Build argument list: [command_name, ...user_args]
        let mut args = vec![cmd_name.to_string()];
        for token in self.command_args.split_whitespace() {
            args.push(token.to_string());
        }

        let output = std::process::Command::new(&exe).args(&args).output();

        self.command_output = match output {
            Ok(o) => {
                let mut lines: Vec<String> = Vec::new();
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                if !stdout.is_empty() {
                    lines.extend(stdout.lines().map(str::to_owned));
                }
                if !stderr.is_empty() {
                    lines.extend(stderr.lines().map(str::to_owned));
                }
                if lines.is_empty() {
                    lines.push(format!("(exited with status {})", o.status));
                }
                lines
            }
            Err(e) => vec![format!("Error running command: {e}")],
        };
    }
}

// ── File helpers ──────────────────────────────────────────────────────────────

fn load_dir_files(dir: &std::path::Path) -> Result<Vec<FileEntry>> {
    let mut entries: Vec<FileEntry> = Vec::new();

    let read_dir = std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

    for item in read_dir {
        let item = item.context("Failed to read directory entry")?;
        let meta = item.metadata().context("Failed to read file metadata")?;
        let name = item.file_name().to_string_lossy().to_string();
        let size_bytes = if meta.is_file() { meta.len() } else { 0 };
        entries.push(FileEntry {
            name,
            size_bytes,
            is_dir: meta.is_dir(),
        });
    }

    entries.sort_by(|a, b| {
        // Directories first, then alphabetical.
        b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name))
    });

    Ok(entries)
}

fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

// ── Probe helpers (sync, background-thread safe) ──────────────────────────────

fn probe_file_sync(path: &std::path::Path) -> ProbeInfo {
    use std::io::Read;

    // Read first 8 KiB for probing.
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut buf = vec![0u8; 8192];
    let bytes_read = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    buf.truncate(bytes_read);

    let info = oximedia_container::container_probe::MultiFormatProber::probe(&buf);

    let video_stream = info.streams.iter().find(|s| s.stream_type == "video");
    let audio_stream = info.streams.iter().find(|s| s.stream_type == "audio");

    let video_codec = video_stream.map(|s| s.codec.clone());
    let resolution = video_stream.and_then(|s| match (s.width, s.height) {
        (Some(w), Some(h)) => Some(format!("{w}x{h}")),
        _ => None,
    });
    let audio_codec = audio_stream.map(|s| s.codec.clone());
    let duration_secs = info.duration_ms.map(|ms| ms as f64 / 1000.0);

    ProbeInfo {
        container: info.format.clone(),
        video_codec,
        resolution,
        audio_codec,
        duration_secs,
        bitrate_kbps: info.bitrate_kbps,
        size_bytes,
    }
}

fn format_probe_info(info: &ProbeInfo) -> String {
    let mut lines = Vec::new();

    let container_label = if info.container.is_empty() {
        "unknown"
    } else {
        &info.container
    };
    lines.push(format!("Container : {container_label}"));
    lines.push(format!("Size      : {}", format_size(info.size_bytes)));

    if let Some(ref vc) = info.video_codec {
        lines.push(format!("Video     : {vc}"));
    }
    if let Some(ref res) = info.resolution {
        lines.push(format!("Resolution: {res}"));
    }
    if let Some(ref ac) = info.audio_codec {
        lines.push(format!("Audio     : {ac}"));
    }
    if let Some(dur) = info.duration_secs {
        let total_secs = dur as u64;
        let secs = total_secs % 60;
        let mins = (total_secs / 60) % 60;
        let hrs = total_secs / 3600;
        if hrs > 0 {
            lines.push(format!("Duration  : {hrs}:{mins:02}:{secs:02}"));
        } else {
            lines.push(format!("Duration  : {mins}:{secs:02}"));
        }
    }
    if let Some(kbps) = info.bitrate_kbps {
        lines.push(format!("Bitrate   : {kbps} kbps"));
    }

    lines.join("\n")
}

// ── Command list ──────────────────────────────────────────────────────────────

fn all_commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("probe", "Inspect media file format and streams"),
        ("transcode", "Re-encode video/audio to another codec"),
        ("extract", "Pull individual frames from video"),
        ("batch", "Process a whole directory of files"),
        ("scene", "Detect scene cuts and classify shots"),
        ("audio", "Loudness metering, normalisation, beat detection"),
        ("subtitle", "Convert, extract, burn-in subtitles"),
        ("filter", "Apply standalone filter graph"),
        ("lut", "Apply, inspect, or convert LUT files"),
        ("denoise", "Reduce video noise / grain"),
        ("stabilize", "Remove camera shake from video"),
        ("edl", "Parse, validate, and export EDL files"),
        ("package", "HLS / DASH adaptive-bitrate packaging"),
        ("forensics", "Tamper detection and provenance analysis"),
        ("stream", "HLS/DASH serve, ingest, record"),
        (
            "search",
            "Content search: text, visual similarity, fingerprint",
        ),
        ("timecode", "Timecode conversion and calculation"),
        ("repair", "Media file repair and recovery"),
        ("color", "Color management: convert, matrix, Delta E"),
        ("playlist", "Generate, validate, and simulate playlists"),
        ("conform", "QC/conformance checking and fixing"),
        ("archive", "IMF/archive packaging and extraction"),
        ("watermark", "Digital audio watermarking"),
        ("tui", "Launch this interactive terminal UI"),
    ]
}

// ── Cwd persistence ───────────────────────────────────────────────────────────

fn state_file_path() -> Option<std::path::PathBuf> {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .or_else(|| Some(std::env::temp_dir()))?;
    Some(base.join("oximedia").join("tui.json"))
}

fn load_persisted_cwd() -> std::path::PathBuf {
    (|| -> Option<std::path::PathBuf> {
        let path = state_file_path()?;
        let data = std::fs::read_to_string(path).ok()?;
        let val: serde_json::Value = serde_json::from_str(&data).ok()?;
        let cwd_str = val.get("cwd")?.as_str()?;
        let cwd = std::path::PathBuf::from(cwd_str);
        if cwd.is_dir() {
            Some(cwd)
        } else {
            None
        }
    })()
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()))
}

fn persist_cwd(cwd: &std::path::Path) {
    let result: anyhow::Result<()> = (|| {
        let path = state_file_path().context("no state dir")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create state dir")?;
        }
        let json = serde_json::json!({ "cwd": cwd.display().to_string() });
        std::fs::write(&path, json.to_string()).context("write state file")?;
        Ok(())
    })();
    if let Err(e) = result {
        tracing::debug!("Failed to persist TUI cwd: {e}");
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();

        // Top-level layout: tab bar, body, status bar.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // tabs
                Constraint::Min(0),    // content
                Constraint::Length(2), // status bar
            ])
            .split(area);

        render_tabs(frame, chunks[0], app);
        render_body(frame, chunks[1], app);
        render_status(frame, chunks[2], app);
    })?;
    Ok(())
}

fn render_tabs(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let tab_titles: Vec<Line> = TABS
        .iter()
        .map(|t| Line::from(Span::styled(*t, Style::default().fg(Color::White))))
        .collect();

    let tabs = Tabs::new(tab_titles)
        .block(
            Block::default().borders(Borders::ALL).title(Span::styled(
                " OxiMedia TUI ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
        )
        .select(app.tab_index)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);
}

fn render_body(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    match app.tab_index {
        TAB_FILES => render_files_tab(frame, area, app),
        TAB_COMMANDS => render_commands_tab(frame, area, app),
        TAB_ABOUT => render_about_tab(frame, area),
        _ => {}
    }
}

fn render_files_tab(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    // When search is active, reserve a one-line search bar at the bottom.
    let (list_area, search_bar_area) = if app.search_mode {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        (split[0], Some(split[1]))
    } else {
        (area, None)
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(list_area);

    // Build breadcrumb title for the file list.
    let cwd_display = app.current_cwd.display().to_string();
    let stack_depth = app.cwd_stack.len();
    let list_title = if stack_depth > 0 {
        format!(" Files [{cwd_display}] (Backspace=up) ")
    } else {
        format!(" Files [{cwd_display}] ")
    };

    // File list (uses filtered view).
    let items: Vec<ListItem> = app
        .filtered_file_list
        .iter()
        .map(|f| {
            let icon = if f.is_dir { "d " } else { "  " };
            let size_str = if f.is_dir {
                String::new()
            } else {
                format!(" ({})", format_size(f.size_bytes))
            };
            let label = format!("{}{}{}", icon, f.name, size_str);
            let style = if f.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(label).style(style)
        })
        .collect();

    let file_list_widget = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(list_title))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(file_list_widget, chunks[0], &mut app.file_state);

    // Detail / probe panel.
    let detail_text = if let Some(ref info) = app.selected_file_info {
        info.clone()
    } else {
        "  Enter on a file: probe it\n  Enter on a directory: descend\n  Backspace: go up"
            .to_string()
    };

    let detail = Paragraph::new(detail_text)
        .block(Block::default().borders(Borders::ALL).title(" Details "))
        .style(Style::default().fg(Color::Gray))
        .wrap(ratatui::widgets::Wrap { trim: true });

    frame.render_widget(detail, chunks[1]);

    // Search bar overlay.
    if let Some(bar_area) = search_bar_area {
        let search_text = format!("/{}", app.search_query);
        let bar = Paragraph::new(search_text).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(bar, bar_area);
    }
}

fn render_commands_tab(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    // Command name list.
    let cmd_items: Vec<ListItem> = app
        .command_list
        .iter()
        .map(|(name, _)| {
            ListItem::new(format!("  oximedia {name}")).style(Style::default().fg(Color::Green))
        })
        .collect();

    let cmd_list_widget = List::new(cmd_items)
        .block(Block::default().borders(Borders::ALL).title(" Commands "))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(cmd_list_widget, chunks[0], &mut app.command_state);

    // Right pane: description + optional run output.
    let right_area = chunks[1];

    match app.command_tab_state {
        CommandTabState::Browsing => {
            let description = if let Some(idx) = app.command_state.selected() {
                app.command_list
                    .get(idx)
                    .map(|(name, desc)| {
                        let mut text = format!("Command: oximedia {name}\n\n{desc}");
                        if !app.command_output.is_empty() {
                            text.push_str("\n\n--- Last output ---\n");
                            text.push_str(&app.command_output.join("\n"));
                        }
                        text
                    })
                    .unwrap_or_default()
            } else {
                "Select a command to see its description.\n\nPress Enter to enter run mode."
                    .to_string()
            };

            let desc_widget = Paragraph::new(description)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Description / Output "),
                )
                .style(Style::default().fg(Color::Gray))
                .wrap(ratatui::widgets::Wrap { trim: true });

            frame.render_widget(desc_widget, right_area);
        }
        CommandTabState::InputArgs => {
            // Split right pane: output above, input line at bottom.
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(3)])
                .split(right_area);

            let output_text = if app.command_output.is_empty() {
                "  Output will appear here after running.".to_string()
            } else {
                app.command_output.join("\n")
            };

            let output_widget = Paragraph::new(output_text)
                .block(Block::default().borders(Borders::ALL).title(" Output "))
                .style(Style::default().fg(Color::Gray))
                .wrap(ratatui::widgets::Wrap { trim: true });

            frame.render_widget(output_widget, split[0]);

            // Build the command name for the input prompt.
            let cmd_name = app
                .command_state
                .selected()
                .and_then(|i| app.command_list.get(i))
                .map(|(name, _)| *name)
                .unwrap_or("");

            let input_text = format!(" oximedia {cmd_name} {}", app.command_args);
            let input_widget = Paragraph::new(input_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Run (Enter=execute, Esc=cancel) ")
                        .style(Style::default().fg(Color::Yellow)),
                )
                .style(Style::default().fg(Color::White));

            frame.render_widget(input_widget, split[1]);
        }
    }
}

fn render_about_tab(frame: &mut ratatui::Frame, area: Rect) {
    let version = env!("CARGO_PKG_VERSION");
    let text = format!(
        r#"OxiMedia - Sovereign Media Framework
Version: {version}

A patent-free, pure-Rust reconstruction of FFmpeg + OpenCV.

Supported codecs (video): AV1, VP9, VP8, Theora
Supported codecs (audio): Opus, Vorbis, FLAC, PCM
Supported containers:      Matroska, WebM, Ogg, FLAC, WAV

Homepage: https://github.com/cool-japan/oximedia
License:  Apache-2.0
Author:   COOLJAPAN OU (Team KitaSan)

Keyboard shortcuts:
  q / Ctrl+C     Quit
  Tab / ->       Next tab
  <- / Shift+Tab Previous tab
  Up / Down      Navigate list
  PgUp / PgDn    Jump 10 rows
  Enter          Descend directory / probe file / run command
  Backspace      Go up one directory (Files tab)
  /              Start incremental search (Files tab)
  Esc            Cancel search / cancel run input
  Mouse scroll   Navigate list
"#
    );

    let about = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" About OxiMedia "),
        )
        .style(Style::default().fg(Color::White))
        .wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(about, area);
}

fn render_status(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let status = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(&app.status_message, Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::TOP));

    frame.render_widget(status, area);
}

// ── Terminal setup / teardown ─────────────────────────────────────────────────

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("Failed to create terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")
}

// ── Main event loop ───────────────────────────────────────────────────────────

/// Launch the interactive TUI.
pub fn run_tui() -> Result<()> {
    let mut terminal = setup_terminal()?;

    // Install a panic hook that restores the terminal before printing.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort: ignore errors during panic cleanup.
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        original_hook(info);
    }));

    let mut app = App::new().context("Failed to initialise TUI app state")?;
    let tick_duration = std::time::Duration::from_millis(250);
    let mut quit = false;

    loop {
        // Poll any pending probe result before drawing.
        app.poll_probe_result();

        draw(&mut terminal, &mut app)?;

        if event::poll(tick_duration).context("Event poll failed")? {
            match event::read().context("Event read failed")? {
                Event::Key(key) => {
                    // Ctrl+C -> quit regardless of mode.
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        quit = true;
                    } else {
                        handle_key(&mut app, key.code, key.modifiers);
                        if app.status_message == "__QUIT__" {
                            quit = true;
                        }
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => app.select_previous(),
                    MouseEventKind::ScrollDown => app.select_next(),
                    _ => {}
                },
                Event::Resize(_, _) => {
                    // ratatui handles resize automatically on next draw.
                }
                _ => {}
            }
        }

        if quit {
            break;
        }
    }

    persist_cwd(&app.current_cwd.clone());
    restore_terminal(&mut terminal)?;
    Ok(())
}

/// Handle a single key event, routing by current mode first.
fn handle_key(app: &mut App, code: KeyCode, _modifiers: KeyModifiers) {
    // ── Commands tab — InputArgs mode ─────────────────────────────────────
    if app.tab_index == TAB_COMMANDS && app.command_tab_state == CommandTabState::InputArgs {
        match code {
            KeyCode::Esc => {
                app.command_tab_state = CommandTabState::Browsing;
                app.status_message =
                    "  q:quit  Tab:tab  up/down:nav  Enter:run-mode  Esc:cancel".to_string();
            }
            KeyCode::Enter => app.on_enter_commands(),
            KeyCode::Backspace => {
                app.command_args.pop();
            }
            KeyCode::Char(c) => {
                app.command_args.push(c);
            }
            _ => {}
        }
        return;
    }

    // ── Files tab — search mode ────────────────────────────────────────────
    if app.tab_index == TAB_FILES && app.search_mode {
        match code {
            KeyCode::Esc => {
                app.exit_search();
                app.status_message =
                    "  q:quit  Tab:tab  up/down:nav  Enter:select  /:search  PgUp/Dn:jump"
                        .to_string();
            }
            KeyCode::Enter => {
                // Accept current filter, exit search mode but keep the filter active.
                app.search_mode = false;
            }
            KeyCode::Backspace => {
                app.search_query.pop();
                app.apply_search_filter();
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.apply_search_filter();
            }
            _ => {}
        }
        return;
    }

    // ── Normal mode ────────────────────────────────────────────────────────
    match code {
        KeyCode::Char('q') => {
            app.status_message = "__QUIT__".to_string();
        }
        KeyCode::Tab | KeyCode::Right => app.next_tab(),
        KeyCode::BackTab | KeyCode::Left => app.prev_tab(),
        KeyCode::Up => app.select_previous(),
        KeyCode::Down => app.select_next(),
        KeyCode::PageUp => {
            for _ in 0..10 {
                app.select_previous();
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                app.select_next();
            }
        }
        KeyCode::Enter => app.on_enter(),
        KeyCode::Backspace if app.tab_index == TAB_FILES => {
            app.ascend_dir();
        }
        KeyCode::Char('/') if app.tab_index == TAB_FILES => {
            app.search_mode = true;
            app.search_query.clear();
            app.apply_search_filter();
            app.status_message =
                "  Search: type to filter, Enter to accept, Esc to clear".to_string();
        }
        _ => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_app() -> App {
        App::new_for_test()
    }

    fn make_entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|n| FileEntry {
                name: n.to_string(),
                size_bytes: 1024,
                is_dir: false,
            })
            .collect()
    }

    // ── Search filter ──────────────────────────────────────────────────────

    #[test]
    fn search_filter_case_insensitive() {
        let mut app = make_test_app();
        app.file_list = make_entries(&["video_av1.mp4", "audio.flac", "Archive.zip"]);
        app.filtered_file_list = app.file_list.clone();
        app.file_state.select(Some(0));

        app.search_query = "av1".to_string();
        app.apply_search_filter();

        assert_eq!(app.filtered_file_list.len(), 1);
        assert_eq!(app.filtered_file_list[0].name, "video_av1.mp4");
    }

    #[test]
    fn search_filter_uppercase_query() {
        let mut app = make_test_app();
        app.file_list = make_entries(&["documentary.mkv", "ARCHIVE.zip", "test.mp4"]);
        app.filtered_file_list = app.file_list.clone();

        app.search_query = "ARCHIVE".to_string();
        app.apply_search_filter();

        assert_eq!(app.filtered_file_list.len(), 1);
        assert_eq!(app.filtered_file_list[0].name, "ARCHIVE.zip");
    }

    #[test]
    fn search_esc_clears_filter() {
        let mut app = make_test_app();
        app.file_list = make_entries(&["a.mp4", "b.mkv"]);
        app.filtered_file_list = app.file_list.clone();
        app.file_state.select(Some(0));
        app.search_mode = true;
        app.search_query = "a".to_string();
        app.apply_search_filter();
        assert_eq!(app.filtered_file_list.len(), 1);

        // Simulate Esc.
        app.exit_search();
        assert!(!app.search_mode);
        assert!(app.search_query.is_empty());
        assert_eq!(app.filtered_file_list.len(), 2);
    }

    #[test]
    fn search_empty_query_shows_all() {
        let mut app = make_test_app();
        app.file_list = make_entries(&["a.mp4", "b.mkv", "c.avi"]);
        app.filtered_file_list = app.file_list.clone();
        app.search_query = String::new();
        app.apply_search_filter();
        assert_eq!(app.filtered_file_list.len(), 3);
    }

    // ── Cwd navigation ────────────────────────────────────────────────────

    #[test]
    fn descend_pushes_cwd_stack() {
        let mut app = make_test_app();
        let tmp = std::env::temp_dir();
        app.current_cwd = tmp.clone();
        app.cwd_stack = Vec::new();

        let subdir = tmp.join("oximedia_tui_test_descend");
        std::fs::create_dir_all(&subdir).ok();

        // Simulate descend.
        app.cwd_stack.push(app.current_cwd.clone());
        app.current_cwd = subdir.clone();

        assert_eq!(app.cwd_stack.len(), 1);
        assert_eq!(app.cwd_stack[0], tmp);
        assert_eq!(app.current_cwd, subdir);

        std::fs::remove_dir_all(&subdir).ok();
    }

    #[test]
    fn ascend_pops_cwd_stack() {
        let mut app = make_test_app();
        let tmp = std::env::temp_dir();
        let subdir = tmp.join("oximedia_tui_test_ascend");
        std::fs::create_dir_all(&subdir).ok();

        app.current_cwd = subdir.clone();
        app.cwd_stack = vec![tmp.clone()];

        app.ascend_dir();

        assert!(app.cwd_stack.is_empty());
        assert_eq!(app.current_cwd, tmp);

        std::fs::remove_dir_all(&subdir).ok();
    }

    // ── PgUp / PgDn selection ─────────────────────────────────────────────

    #[test]
    fn pgup_moves_selection_by_10() {
        let mut app = make_test_app();
        app.file_list = (0..25)
            .map(|i| FileEntry {
                name: format!("file_{i:02}.mp4"),
                size_bytes: 0,
                is_dir: false,
            })
            .collect();
        app.filtered_file_list = app.file_list.clone();
        app.file_state.select(Some(15));

        for _ in 0..10 {
            app.select_previous();
        }

        assert_eq!(app.file_state.selected(), Some(5));
    }

    #[test]
    fn pgdn_moves_selection_by_10() {
        let mut app = make_test_app();
        app.file_list = (0..25)
            .map(|i| FileEntry {
                name: format!("file_{i:02}.mp4"),
                size_bytes: 0,
                is_dir: false,
            })
            .collect();
        app.filtered_file_list = app.file_list.clone();
        app.file_state.select(Some(0));

        for _ in 0..10 {
            app.select_next();
        }

        assert_eq!(app.file_state.selected(), Some(10));
    }

    // ── Probe info formatting ─────────────────────────────────────────────

    #[test]
    fn format_probe_info_full() {
        let info = ProbeInfo {
            container: "mkv".to_string(),
            video_codec: Some("av1".to_string()),
            resolution: Some("1920x1080".to_string()),
            audio_codec: Some("opus".to_string()),
            duration_secs: Some(125.5),
            bitrate_kbps: Some(2500),
            size_bytes: 40_000_000,
        };
        let rendered = format_probe_info(&info);
        assert!(rendered.contains("Container : mkv"));
        assert!(rendered.contains("Video     : av1"));
        assert!(rendered.contains("Resolution: 1920x1080"));
        assert!(rendered.contains("Audio     : opus"));
        assert!(rendered.contains("Bitrate   : 2500 kbps"));
        // Duration: 2:05
        assert!(rendered.contains("Duration  : 2:05"));
    }

    #[test]
    fn format_probe_info_audio_only() {
        let info = ProbeInfo {
            container: "flac".to_string(),
            video_codec: None,
            resolution: None,
            audio_codec: Some("flac".to_string()),
            duration_secs: Some(3723.0),
            bitrate_kbps: None,
            size_bytes: 100_000,
        };
        let rendered = format_probe_info(&info);
        assert!(rendered.contains("Container : flac"));
        assert!(!rendered.contains("Video"));
        assert!(rendered.contains("Audio     : flac"));
        // 3723s = 1h 2m 3s
        assert!(rendered.contains("1:02:03"));
    }

    // ── Command tab state ─────────────────────────────────────────────────

    #[test]
    fn command_tab_enters_input_mode_on_enter() {
        let mut app = make_test_app();
        app.tab_index = TAB_COMMANDS;
        app.command_state.select(Some(0));
        assert_eq!(app.command_tab_state, CommandTabState::Browsing);

        app.on_enter_commands();

        assert_eq!(app.command_tab_state, CommandTabState::InputArgs);
        assert!(app.command_args.is_empty());
    }

    #[test]
    fn command_tab_esc_in_input_mode_goes_back() {
        let mut app = make_test_app();
        app.tab_index = TAB_COMMANDS;
        app.command_tab_state = CommandTabState::InputArgs;
        app.command_args = "some args".to_string();

        // Simulate Esc.
        handle_key(&mut app, KeyCode::Esc, KeyModifiers::empty());

        assert_eq!(app.command_tab_state, CommandTabState::Browsing);
    }

    #[test]
    fn command_input_accumulates_chars() {
        let mut app = make_test_app();
        app.tab_index = TAB_COMMANDS;
        app.command_tab_state = CommandTabState::InputArgs;

        handle_key(&mut app, KeyCode::Char('a'), KeyModifiers::empty());
        handle_key(&mut app, KeyCode::Char('b'), KeyModifiers::empty());
        handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::empty());
        handle_key(&mut app, KeyCode::Backspace, KeyModifiers::empty());

        assert_eq!(app.command_args, "ab");
    }

    // ── format_size ───────────────────────────────────────────────────────

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kib() {
        assert_eq!(format_size(2048), "2.00 KiB");
    }

    #[test]
    fn format_size_mib() {
        assert_eq!(format_size(5 * 1024 * 1024), "5.00 MiB");
    }
}

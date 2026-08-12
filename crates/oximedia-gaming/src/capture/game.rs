//! Game-specific capture optimization.
//!
//! Provides optimized capture profiles for different game genres.

use crate::{GamingError, GamingResult};

/// Game capture with genre-specific optimizations.
pub struct GameCapture {
    profile: GameProfile,
    window_handle: Option<u64>,
}

/// Game genre profiles with optimized settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameProfile {
    /// First-person shooter (ultra-low latency)
    Fps,
    /// Multiplayer online battle arena
    Moba,
    /// Fighting game (frame-perfect timing)
    Fighting,
    /// Racing game (high motion handling)
    Racing,
    /// Strategy game (large viewport)
    Strategy,
    /// Role-playing game
    Rpg,
    /// Platformer
    Platformer,
    /// Generic game
    Generic,
}

/// Game detection result.
#[derive(Debug, Clone)]
pub struct GameInfo {
    /// Game name
    pub name: String,
    /// Process ID
    pub pid: u32,
    /// Window handle
    pub window_handle: u64,
    /// Detected profile
    pub profile: GameProfile,
    /// Resolution
    pub resolution: (u32, u32),
    /// Supports hardware acceleration
    pub has_hardware_accel: bool,
}

/// Known process executable names mapped to a display title and the
/// [`GameProfile`] [`GameCapture::auto_detect`] recommends for it.
///
/// Keys are lowercase, with any trailing `.exe` already stripped (see
/// [`normalize_process_name`]) -- they are matched exactly, never as a
/// substring, so a short key like `"cs2"` cannot accidentally match an
/// unrelated process whose name merely contains it.
///
/// This is intentionally a small, best-effort sample (one well-known title
/// per genre) rather than an exhaustive registry, in the same spirit as
/// OBS Studio's built-in game-process list: growing it only grows the set
/// of titles `auto_detect` can name, it never changes how the scan itself
/// works.
///
/// `cfg`-gated together with its two lookup helpers below: they exist only
/// to serve the non-`wasm32` branch of [`GameCapture::auto_detect`], so on
/// `wasm32` they would otherwise be dead code.
#[cfg(not(target_arch = "wasm32"))]
const KNOWN_GAMES: &[(&str, &str, GameProfile)] = &[
    ("csgo", "Counter-Strike: Global Offensive", GameProfile::Fps),
    ("cs2", "Counter-Strike 2", GameProfile::Fps),
    ("valorant-win64-shipping", "VALORANT", GameProfile::Fps),
    ("r5apex", "Apex Legends", GameProfile::Fps),
    (
        "fortniteclient-win64-shipping",
        "Fortnite",
        GameProfile::Fps,
    ),
    ("overwatch", "Overwatch 2", GameProfile::Fps),
    ("dota2", "Dota 2", GameProfile::Moba),
    ("leagueclient", "League of Legends", GameProfile::Moba),
    ("streetfighter6", "Street Fighter 6", GameProfile::Fighting),
    ("tekken8", "Tekken 8", GameProfile::Fighting),
    ("forzahorizon5", "Forza Horizon 5", GameProfile::Racing),
    ("sc2", "StarCraft II", GameProfile::Strategy),
    ("eldenring", "Elden Ring", GameProfile::Rpg),
    ("witcher3", "The Witcher 3", GameProfile::Rpg),
    ("celeste", "Celeste", GameProfile::Platformer),
    ("hollowknight", "Hollow Knight", GameProfile::Platformer),
];

/// Normalize a raw OS process name for matching against [`KNOWN_GAMES`]:
/// lowercased, with one trailing `.exe` stripped.
///
/// Process names returned by the OS process table are already bare
/// executable names rather than full paths, so no path stripping is
/// needed here.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
fn normalize_process_name(raw_name: &str) -> String {
    let lower = raw_name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// Look up a raw OS process name in [`KNOWN_GAMES`].
///
/// Returns the display name and recommended [`GameProfile`] on an exact
/// (case-insensitive, `.exe`-insensitive) match, `None` otherwise.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
fn match_known_game(raw_name: &str) -> Option<(&'static str, GameProfile)> {
    let normalized = normalize_process_name(raw_name);
    KNOWN_GAMES
        .iter()
        .find(|(key, _, _)| *key == normalized)
        .map(|(_, display_name, profile)| (*display_name, *profile))
}

impl GameCapture {
    /// Create a new game capture with the given profile.
    #[must_use]
    pub fn new(profile: GameProfile) -> Self {
        Self {
            profile,
            window_handle: None,
        }
    }

    /// Auto-detect running games and select appropriate profile.
    ///
    /// Scans the live OS process table (via `sysinfo`) and matches each
    /// process's executable name against `KNOWN_GAMES`. This performs a
    /// real scan, not a simulation: on a machine with no matching game
    /// running (e.g. CI) it genuinely returns an empty list, and on a
    /// machine that *is* running one of `KNOWN_GAMES` it returns a real
    /// PID pulled from the OS.
    ///
    /// Detected entries carry `window_handle: 0` and `resolution: (0, 0)`:
    /// resolving an actual window handle needs platform-specific window
    /// enumeration (Win32 `HWND` / X11 `XID` / macOS `CGWindowID`), which
    /// this crate does not implement -- callers resolve a real handle via
    /// [`Self::attach`]. `has_hardware_accel` is always `false` because no
    /// real hardware-acceleration probe is wired up (see
    /// `encode::nvenc`/`encode::qsv`/`encode::vce`, whose own
    /// `is_available()` are themselves still simulated).
    ///
    /// # Errors
    ///
    /// Returns [`GamingError::UnsupportedPlatform`] when there is no
    /// process table to scan: on the `wasm32` target, and on any desktop
    /// OS `sysinfo` itself does not support process enumeration for
    /// (`sysinfo::IS_SUPPORTED_SYSTEM == false`).
    pub fn auto_detect() -> GamingResult<Vec<GameInfo>> {
        #[cfg(target_arch = "wasm32")]
        {
            Err(GamingError::UnsupportedPlatform(
                "process enumeration requires an OS process table, which wasm32 does not have; \
                 GameCapture::auto_detect cannot run in this build target"
                    .to_string(),
            ))
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            if !sysinfo::IS_SUPPORTED_SYSTEM {
                return Err(GamingError::UnsupportedPlatform(
                    "sysinfo has no process-enumeration backend for this operating system; \
                     GameCapture::auto_detect is only implemented where sysinfo supports it \
                     (Windows, macOS, Linux, FreeBSD, NetBSD)"
                        .to_string(),
                ));
            }

            // `nothing()` still populates each process's name and PID --
            // those are fetched unconditionally during enumeration. Only
            // the expensive extras (cmdline, environment, cwd, disk I/O,
            // CPU%) are gated by `ProcessRefreshKind`, and this scan needs
            // none of them.
            let refresh = sysinfo::RefreshKind::nothing()
                .with_processes(sysinfo::ProcessRefreshKind::nothing());
            let system = sysinfo::System::new_with_specifics(refresh);

            let mut detected = Vec::new();
            for (pid, process) in system.processes() {
                let raw_name = process.name().to_string_lossy();
                if let Some((display_name, profile)) = match_known_game(&raw_name) {
                    detected.push(GameInfo {
                        name: display_name.to_string(),
                        pid: pid.as_u32(),
                        window_handle: 0,
                        profile,
                        resolution: (0, 0),
                        has_hardware_accel: false,
                    });
                }
            }
            Ok(detected)
        }
    }

    /// Attach to a specific game window.
    ///
    /// # Errors
    ///
    /// Returns error if attachment fails.
    pub fn attach(&mut self, window_handle: u64) -> GamingResult<()> {
        self.window_handle = Some(window_handle);
        Ok(())
    }

    /// Detach from current game window.
    pub fn detach(&mut self) {
        self.window_handle = None;
    }

    /// Get recommended settings for the current profile.
    #[must_use]
    pub fn recommended_settings(&self) -> CaptureSettings {
        match self.profile {
            GameProfile::Fps => CaptureSettings {
                target_latency_ms: 30,
                max_framerate: 144,
                priority: CapturePriority::Latency,
                motion_prediction: true,
            },
            GameProfile::Moba => CaptureSettings {
                target_latency_ms: 50,
                max_framerate: 60,
                priority: CapturePriority::Balanced,
                motion_prediction: false,
            },
            GameProfile::Fighting => CaptureSettings {
                target_latency_ms: 16,
                max_framerate: 60,
                priority: CapturePriority::Latency,
                motion_prediction: false,
            },
            GameProfile::Racing => CaptureSettings {
                target_latency_ms: 40,
                max_framerate: 120,
                priority: CapturePriority::Quality,
                motion_prediction: true,
            },
            GameProfile::Strategy => CaptureSettings {
                target_latency_ms: 100,
                max_framerate: 60,
                priority: CapturePriority::Quality,
                motion_prediction: false,
            },
            GameProfile::Rpg => CaptureSettings {
                target_latency_ms: 80,
                max_framerate: 60,
                priority: CapturePriority::Quality,
                motion_prediction: false,
            },
            GameProfile::Platformer => CaptureSettings {
                target_latency_ms: 50,
                max_framerate: 60,
                priority: CapturePriority::Balanced,
                motion_prediction: true,
            },
            GameProfile::Generic => CaptureSettings {
                target_latency_ms: 60,
                max_framerate: 60,
                priority: CapturePriority::Balanced,
                motion_prediction: false,
            },
        }
    }

    /// Check if game window is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.window_handle.is_some()
    }

    /// Get current profile.
    #[must_use]
    pub fn profile(&self) -> GameProfile {
        self.profile
    }
}

/// Capture settings optimized for game profile.
#[derive(Debug, Clone, Copy)]
pub struct CaptureSettings {
    /// Target latency in milliseconds
    pub target_latency_ms: u32,
    /// Maximum framerate
    pub max_framerate: u32,
    /// Capture priority
    pub priority: CapturePriority,
    /// Enable motion prediction
    pub motion_prediction: bool,
}

/// Capture priority mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePriority {
    /// Prioritize low latency
    Latency,
    /// Balance latency and quality
    Balanced,
    /// Prioritize high quality
    Quality,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_capture_creation() {
        let capture = GameCapture::new(GameProfile::Fps);
        assert_eq!(capture.profile(), GameProfile::Fps);
        assert!(!capture.is_active());
    }

    #[test]
    fn test_fps_profile_settings() {
        let capture = GameCapture::new(GameProfile::Fps);
        let settings = capture.recommended_settings();

        assert_eq!(settings.priority, CapturePriority::Latency);
        assert!(settings.target_latency_ms <= 30);
        assert!(settings.motion_prediction);
    }

    #[test]
    fn test_fighting_game_settings() {
        let capture = GameCapture::new(GameProfile::Fighting);
        let settings = capture.recommended_settings();

        // Fighting games need frame-perfect timing
        assert!(settings.target_latency_ms <= 16);
        assert_eq!(settings.priority, CapturePriority::Latency);
    }

    #[test]
    fn test_strategy_game_settings() {
        let capture = GameCapture::new(GameProfile::Strategy);
        let settings = capture.recommended_settings();

        // Strategy games can tolerate higher latency for better quality
        assert!(settings.target_latency_ms >= 60);
        assert_eq!(settings.priority, CapturePriority::Quality);
    }

    #[test]
    fn test_attach_detach() {
        let mut capture = GameCapture::new(GameProfile::Generic);

        capture.attach(12345).expect("attach should succeed");
        assert!(capture.is_active());

        capture.detach();
        assert!(!capture.is_active());
    }

    #[test]
    fn test_auto_detect() {
        // A real process-table scan cannot be asserted to an exact count:
        // it is legitimately empty on a CI runner but may find a nonzero
        // number of matches on a developer machine that happens to be
        // running one of `KNOWN_GAMES`. Assert the scan succeeds and that
        // whatever it returns is structurally valid, rather than pinning a
        // length.
        let games = GameCapture::auto_detect()
            .expect("auto detect should succeed on a sysinfo-supported platform");
        for game in &games {
            assert!(!game.name.is_empty());
            assert!(game.pid > 0);
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_match_known_game_exact_hits() {
        assert_eq!(
            match_known_game("csgo.exe"),
            Some(("Counter-Strike: Global Offensive", GameProfile::Fps))
        );
        assert_eq!(
            match_known_game("CS2.EXE"),
            Some(("Counter-Strike 2", GameProfile::Fps))
        );
        assert_eq!(
            match_known_game("dota2"),
            Some(("Dota 2", GameProfile::Moba))
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_match_known_game_rejects_unrelated_processes() {
        // Regression guard: these must NOT match despite containing a
        // KNOWN_GAMES key as a substring (e.g. "cs2" inside "cs2ools"),
        // and common dev-machine/CI processes must never false-positive.
        assert_eq!(match_known_game("cargo"), None);
        assert_eq!(match_known_game("rustc"), None);
        assert_eq!(match_known_game("zsh"), None);
        assert_eq!(match_known_game("cs2ools"), None);
        assert_eq!(match_known_game(""), None);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_normalize_process_name() {
        assert_eq!(normalize_process_name("CSGO.EXE"), "csgo");
        assert_eq!(normalize_process_name("dota2"), "dota2");
        assert_eq!(normalize_process_name("Overwatch"), "overwatch");
    }

    #[test]
    fn test_all_profiles_have_settings() {
        let profiles = [
            GameProfile::Fps,
            GameProfile::Moba,
            GameProfile::Fighting,
            GameProfile::Racing,
            GameProfile::Strategy,
            GameProfile::Rpg,
            GameProfile::Platformer,
            GameProfile::Generic,
        ];

        for profile in profiles {
            let capture = GameCapture::new(profile);
            let settings = capture.recommended_settings();
            assert!(settings.target_latency_ms > 0);
            assert!(settings.max_framerate > 0);
        }
    }
}

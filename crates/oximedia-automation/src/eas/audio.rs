//! EAS audio alert insertion.
//!
//! Two audio layers make up a real Emergency Alert System transmission:
//!
//! * **Digital data bursts** — the SAME header and the end-of-message code —
//!   AFSK-modulated per 47 CFR 11.31 by `EasAudioInsertion::afsk_modulate`
//!   and its callers [`EasAudioInsertion::generate_same_header_audio`] /
//!   [`EasAudioInsertion::generate_eom_burst`]. These are what a receiver's
//!   demodulator actually decodes.
//! * **Analog cues** — the two-tone attention signal
//!   ([`EasAudioInsertion::generate_attention_tone`]) and the spoken message
//!   — meant for human ears, not decoded.
//!
//! [`EasAudioInsertion::compose_full_alert`] assembles both into one
//! complete alert.

use crate::{AutomationError, Result};
use chrono::{DateTime, Datelike, Timelike, Utc};
use oximedia_audio::wav::WavReader;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{debug, info};

// ── Constants ────────────────────────────────────────────────────────────────

/// Sample rate used throughout EAS audio synthesis, in Hz.
pub const EAS_SAMPLE_RATE: f32 = 48_000.0;

/// SAME "space" (binary 0) tone frequency, in Hz — 47 CFR 11.31.
pub const SAME_SPACE_HZ: f32 = 1562.5;

/// SAME (Specific Area Message Encoding) data rate, in bits per second —
/// also the AFSK baud rate, since SAME is binary FSK (47 CFR 11.31).
///
/// Derived as [`SAME_SPACE_HZ`]` / 3.0` rather than typed as its own rounded
/// literal (the commonly published figure is "520.83 Bd"), so that
/// [`SAME_MARK_HZ`] below is an *exact* 4x multiple of this rate rather than
/// two independently-rounded decimals that only approximately agree. This
/// also matches how real SAME encoder hardware generates the two tones —
/// divided down from one shared clock, not two free-running oscillators.
pub const SAME_BAUD_RATE: f32 = SAME_SPACE_HZ / 3.0;

/// SAME "mark" (binary 1) tone frequency, in Hz — exactly 4x
/// [`SAME_BAUD_RATE`] (47 CFR 11.31; ≈ 2083.3 Hz).
pub const SAME_MARK_HZ: f32 = SAME_BAUD_RATE * 4.0;

/// Preamble byte prepended to every SAME header/EOM burst, repeated
/// [`SAME_PREAMBLE_LEN`] times so a receiver's AFSK demodulator can acquire
/// bit and byte sync before the ASCII payload begins.
pub const SAME_PREAMBLE_BYTE: u8 = 0xAB;

/// Number of preamble bytes before the ASCII payload.
pub const SAME_PREAMBLE_LEN: usize = 16;

/// Number of times the SAME header and the end-of-message code are each
/// transmitted, per 47 CFR 11.31.
pub const SAME_REPEAT_COUNT: usize = 3;

/// Silence between repeated header/EOM transmissions. 47 CFR 11.31 specifies
/// a one-second pause between repeats of the same burst.
pub const SAME_BURST_GAP: Duration = Duration::from_secs(1);

/// Digital end-of-message payload: four ASCII `N` characters.
pub const SAME_EOM_PAYLOAD: &[u8] = b"NNNN";

/// Maximum number of location codes one SAME header may carry.
pub const SAME_MAX_LOCATIONS: usize = 31;

// ── SAME header ──────────────────────────────────────────────────────────────

/// A SAME (Specific Area Message Encoding) header, per 47 CFR 11.31.
///
/// [`Self::to_same_string`] renders it as the ASCII burst payload —
/// `ZCZC-ORG-EEE-PSSCCC[-PSSCCC...]+TTTT-JJJHHMM-LLLLLLLL-` — that
/// [`EasAudioInsertion::generate_same_header_audio`] AFSK-modulates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SameHeader {
    /// Originator code (`ORG`), exactly 3 uppercase ASCII letters — e.g.
    /// `"WXR"` (National Weather Service), `"PEP"`, `"CIV"`, `"EAS"`.
    pub originator: String,
    /// Event code (`EEE`), exactly 3 uppercase ASCII letters — e.g. `"TOR"`
    /// (tornado warning), `"RWT"` (required weekly test).
    pub event_code: String,
    /// Location codes (`PSSCCC`), each exactly 6 ASCII digits: an optional
    /// county-subdivision digit, a 2-digit state FIPS code, and a 3-digit
    /// county FIPS code. 1 to [`SAME_MAX_LOCATIONS`] entries.
    pub location_codes: Vec<String>,
    /// Valid time in minutes, encoded as the 4-digit `TTTT` (`HHMM`) purge
    /// time. Must fit 2-digit hours + 2-digit minutes (under 6000 minutes).
    pub purge_minutes: u16,
    /// When the header was issued; encodes to `JJJHHMM` (UTC Julian day,
    /// hour, minute).
    pub issued_at: SystemTime,
    /// Station identifier (`LLLLLLLL`), 1 to 8 ASCII characters. Shorter ids
    /// are right-padded with `/` to fill all 8, matching real station ids
    /// such as `"KGYX/NWS"`.
    pub station_id: String,
}

impl SameHeader {
    /// Render the ASCII SAME burst payload.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationError::Eas`] if any field cannot be represented
    /// in its fixed-width SAME encoding: `originator`/`event_code` not
    /// exactly 3 uppercase ASCII letters, `location_codes` empty or over
    /// [`SAME_MAX_LOCATIONS`] or any entry not exactly 6 ASCII digits,
    /// `purge_minutes` too large for `TTTT`, or `station_id` not 1-8 ASCII
    /// characters.
    pub fn to_same_string(&self) -> Result<String> {
        if self.originator.len() != 3 || !self.originator.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(AutomationError::Eas(format!(
                "SAME originator code must be exactly 3 uppercase ASCII letters, got {:?}",
                self.originator
            )));
        }
        if self.event_code.len() != 3 || !self.event_code.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(AutomationError::Eas(format!(
                "SAME event code must be exactly 3 uppercase ASCII letters, got {:?}",
                self.event_code
            )));
        }
        if self.location_codes.is_empty() {
            return Err(AutomationError::Eas(
                "SAME header needs at least one location code".to_string(),
            ));
        }
        if self.location_codes.len() > SAME_MAX_LOCATIONS {
            return Err(AutomationError::Eas(format!(
                "SAME header allows at most {SAME_MAX_LOCATIONS} location codes, got {}",
                self.location_codes.len()
            )));
        }
        for code in &self.location_codes {
            if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
                return Err(AutomationError::Eas(format!(
                    "SAME location code must be exactly 6 ASCII digits, got {code:?}"
                )));
            }
        }
        if self.purge_minutes >= 100 * 60 {
            return Err(AutomationError::Eas(format!(
                "SAME purge time must fit in 2-digit hours (under 6000 minutes), got {}",
                self.purge_minutes
            )));
        }
        if self.station_id.is_empty() || self.station_id.len() > 8 || !self.station_id.is_ascii() {
            return Err(AutomationError::Eas(format!(
                "SAME station id must be 1-8 ASCII characters, got {:?}",
                self.station_id
            )));
        }

        let hours = self.purge_minutes / 60;
        let minutes = self.purge_minutes % 60;

        let issued: DateTime<Utc> = self.issued_at.into();
        let day_of_year = issued.ordinal();
        let hour = issued.hour();
        let minute = issued.minute();

        let mut same = String::from("ZCZC");
        same.push('-');
        same.push_str(&self.originator);
        same.push('-');
        same.push_str(&self.event_code);
        for code in &self.location_codes {
            same.push('-');
            same.push_str(code);
        }
        same.push_str(&format!("+{hours:02}{minutes:02}"));
        same.push('-');
        same.push_str(&format!("{day_of_year:03}{hour:02}{minute:02}"));
        same.push('-');
        same.push_str(&format!("{:/<8}", self.station_id));
        same.push('-');

        Ok(same)
    }
}

/// EAS attention tone configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionToneConfig {
    /// Frequency in Hz (standard: 853 Hz and 960 Hz)
    pub frequencies: Vec<f32>,
    /// Duration in seconds
    pub duration: f32,
    /// Volume (0.0 - 1.0)
    pub volume: f32,
}

impl Default for AttentionToneConfig {
    fn default() -> Self {
        Self {
            frequencies: vec![853.0, 960.0],
            duration: 8.0,
            volume: 0.8,
        }
    }
}

/// Default for [`EasAudioConfig::same_volume`], also used by serde as the
/// value for that field when deserializing a config saved before it existed
/// — see that field's doc comment.
const fn default_same_volume() -> f32 {
    0.8
}

/// EAS audio configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EasAudioConfig {
    /// Attention tone configuration
    pub attention_tone: AttentionToneConfig,
    /// Path to TTS audio files
    pub tts_audio_path: Option<PathBuf>,
    /// Enable end-of-message tones
    pub enable_eom_tones: bool,
    /// Amplitude for AFSK-modulated digital bursts (SAME header, digital
    /// EOM), 0.0-1.0. Independent of `attention_tone.volume`, which only
    /// controls the analog two-tone attention signal.
    ///
    /// `#[serde(default)]`: this field was added after this struct already
    /// shipped, so a previously-saved `EasAudioConfig` (a config file, a
    /// snapshot) has no `same_volume` key at all. Without a default,
    /// deserializing it would fail outright on a field the file's author
    /// never had a chance to set, rather than falling back to the same
    /// value [`EasAudioConfig::default`] uses.
    #[serde(default = "default_same_volume")]
    pub same_volume: f32,
}

impl Default for EasAudioConfig {
    fn default() -> Self {
        Self {
            attention_tone: AttentionToneConfig::default(),
            tts_audio_path: None,
            enable_eom_tones: true,
            same_volume: default_same_volume(),
        }
    }
}

/// EAS audio insertion handler.
pub struct EasAudioInsertion {
    config: EasAudioConfig,
}

impl EasAudioInsertion {
    /// Create a new EAS audio insertion handler.
    pub fn new(config: EasAudioConfig) -> Self {
        info!("Creating EAS audio insertion handler");

        Self { config }
    }

    /// Generate attention tone.
    pub fn generate_attention_tone(&self) -> Result<Vec<f32>> {
        debug!("Generating EAS attention tone");

        let sample_rate = 48000.0;
        let duration = self.config.attention_tone.duration;
        let num_samples = (sample_rate * duration) as usize;

        let mut samples = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let mut sample = 0.0;

            // Generate dual-tone
            for &freq in &self.config.attention_tone.frequencies {
                sample += (2.0 * std::f32::consts::PI * freq * t).sin();
            }

            // Normalize and apply volume
            sample *= self.config.attention_tone.volume
                / self.config.attention_tone.frequencies.len() as f32;

            samples.push(sample);
        }

        Ok(samples)
    }

    /// Generate a plain audible end-of-segment tone: an 853 Hz sine wave.
    ///
    /// # Not the protocol's EOM code
    ///
    /// This is **not** the SAME protocol's digital end-of-message signal —
    /// 47 CFR 11.31 has no "continuous 853 Hz tone" marker at all; 853 Hz is
    /// only meaningful there as one of the two attention-signal frequencies.
    /// The real, machine-decodable EOM is [`Self::generate_eom_burst`]
    /// (preamble + `"NNNN"`, AFSK-modulated, sent [`SAME_REPEAT_COUNT`]
    /// times); this function only produces a simple audible cue.
    pub fn generate_eom_tone(&self) -> Result<Vec<f32>> {
        debug!("Generating EAS end-of-message tone");

        let sample_rate = 48000.0;
        let duration = 3.0; // 3 seconds for EOM
        let num_samples = (sample_rate * duration) as usize;

        let mut samples = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            // EOM is typically 853 Hz
            let sample =
                (2.0 * std::f32::consts::PI * 853.0 * t).sin() * self.config.attention_tone.volume;
            samples.push(sample);
        }

        Ok(samples)
    }

    // ── SAME AFSK modulation ────────────────────────────────────────────────

    /// AFSK-modulate `bytes` at the SAME data rate ([`SAME_BAUD_RATE`]),
    /// using [`SAME_MARK_HZ`] for binary 1 and [`SAME_SPACE_HZ`] for binary
    /// 0, each byte's bits sent least-significant-bit first — exactly the
    /// framing the demodulator in this module's tests expects, and real SAME
    /// decoders use.
    ///
    /// Phase is accumulated continuously across bit and byte boundaries
    /// (rather than restarting `sin` at zero for every bit), so a tone
    /// change never puts a discontinuity — and the audible click / spurious
    /// harmonic it would create — into the waveform. This is what makes the
    /// modulator's output continuous-phase FSK rather than two independently
    /// keyed oscillators.
    fn afsk_modulate(&self, bytes: &[u8]) -> Vec<f32> {
        let samples_per_bit = EAS_SAMPLE_RATE / SAME_BAUD_RATE;
        let total_bits = bytes.len() * 8;
        if total_bits == 0 {
            return Vec::new();
        }
        let total_samples = (samples_per_bit * total_bits as f32).round() as usize;

        let mut out = Vec::with_capacity(total_samples);
        let mut phase = 0.0f32;
        let amplitude = self.config.same_volume.clamp(0.0, 1.0);
        let two_pi = std::f32::consts::TAU;

        for i in 0..total_samples {
            let bit_index = ((i as f32 / samples_per_bit) as usize).min(total_bits - 1);
            let byte = bytes[bit_index / 8];
            let bit = (byte >> (bit_index % 8)) & 1; // LSB first
            let freq = if bit == 1 {
                SAME_MARK_HZ
            } else {
                SAME_SPACE_HZ
            };

            phase += two_pi * freq / EAS_SAMPLE_RATE;
            if phase >= two_pi {
                phase -= two_pi;
            }
            out.push(phase.sin() * amplitude);
        }

        out
    }

    /// Wrap `payload` with the SAME preamble, AFSK-modulate it, and repeat
    /// the whole burst [`SAME_REPEAT_COUNT`] times with [`SAME_BURST_GAP`]
    /// of silence between repeats — the transmission pattern 47 CFR 11.31
    /// specifies for both the header and the end-of-message code.
    fn same_burst(&self, payload: &[u8]) -> Vec<f32> {
        let mut framed = vec![SAME_PREAMBLE_BYTE; SAME_PREAMBLE_LEN];
        framed.extend_from_slice(payload);

        let burst = self.afsk_modulate(&framed);
        let gap_samples = (EAS_SAMPLE_RATE * SAME_BURST_GAP.as_secs_f32()).round() as usize;
        let gap = vec![0.0f32; gap_samples];

        let mut out = Vec::with_capacity((burst.len() + gap.len()) * SAME_REPEAT_COUNT);
        for repeat in 0..SAME_REPEAT_COUNT {
            out.extend_from_slice(&burst);
            if repeat + 1 < SAME_REPEAT_COUNT {
                out.extend_from_slice(&gap);
            }
        }
        out
    }

    /// Generate the SAME header burst — preamble + header text — transmitted
    /// [`SAME_REPEAT_COUNT`] times per 47 CFR 11.31.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationError::Eas`] if `header` cannot be rendered — see
    /// [`SameHeader::to_same_string`].
    pub fn generate_same_header_audio(&self, header: &SameHeader) -> Result<Vec<f32>> {
        let text = header.to_same_string()?;
        Ok(self.same_burst(text.as_bytes()))
    }

    /// Generate the digital end-of-message burst — preamble + `"NNNN"` —
    /// transmitted [`SAME_REPEAT_COUNT`] times per 47 CFR 11.31.
    ///
    /// This is the protocol's real, machine-decodable EOM code, distinct
    /// from [`Self::generate_eom_tone`] (see that method's doc comment).
    #[must_use]
    pub fn generate_eom_burst(&self) -> Vec<f32> {
        self.same_burst(SAME_EOM_PAYLOAD)
    }

    // ── Pre-recorded announcements ──────────────────────────────────────────

    /// Load a pre-recorded announcement from a WAV file, decoded to
    /// [`EAS_SAMPLE_RATE`] mono samples via the real Pure-Rust WAV decoder
    /// (`oximedia_audio::wav::WavReader`) — not a stub.
    ///
    /// This is the documented alternative to [`Self::load_tts_audio`]: until
    /// a TTS engine exists in this workspace, a spoken EAS message must come
    /// from a pre-recorded WAV file rather than from silence.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationError::Eas`] if the file cannot be opened, is not
    /// a valid WAV file, declares zero channels, or its sample rate is not
    /// [`EAS_SAMPLE_RATE`] — resampling on the fly is not implemented, so a
    /// mismatched-rate file is refused rather than played back at the wrong
    /// speed. A multi-channel file is downmixed to mono by averaging its
    /// channels (a real operation on the real decoded samples, not a
    /// fabrication), rather than rejected.
    pub fn load_prerecorded_announcement(&self, path: &Path) -> Result<Vec<f32>> {
        debug!(
            "Loading pre-recorded EAS announcement from {}",
            path.display()
        );

        let file = std::fs::File::open(path).map_err(|error| {
            AutomationError::Eas(format!(
                "cannot open pre-recorded EAS announcement {}: {error}",
                path.display()
            ))
        })?;
        let mut wav = WavReader::new(file).map_err(|error| {
            AutomationError::Eas(format!(
                "pre-recorded EAS announcement {} is not a valid WAV file: {error}",
                path.display()
            ))
        })?;

        let spec = wav.spec();
        let expected_rate = EAS_SAMPLE_RATE as u32;
        if spec.sample_rate != expected_rate {
            return Err(AutomationError::Eas(format!(
                "pre-recorded EAS announcement {} is {} Hz, but EAS audio requires \
                 {expected_rate} Hz (on-the-fly resampling is not implemented)",
                path.display(),
                spec.sample_rate
            )));
        }
        if spec.channels == 0 {
            return Err(AutomationError::Eas(format!(
                "pre-recorded EAS announcement {} declares zero channels",
                path.display()
            )));
        }

        let samples = wav.read_samples_f32().map_err(|error| {
            AutomationError::Eas(format!(
                "failed to decode pre-recorded EAS announcement {}: {error}",
                path.display()
            ))
        })?;

        if spec.channels == 1 {
            return Ok(samples);
        }

        // Downmix to mono by averaging channels — every output sample is
        // the real average of the real decoded samples at that frame.
        let channels = spec.channels as usize;
        Ok(samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect())
    }

    /// Load TTS audio for message.
    ///
    /// # Safety / honesty note
    ///
    /// This is **not implemented**. An earlier revision returned
    /// `Ok(Vec::new())` (silence) for any message, which let
    /// [`Self::compose_message`] ship a complete-looking Emergency Alert
    /// System audio message — correct attention tone, correct
    /// end-of-message tone — around a **silent gap** where the actual
    /// spoken emergency message belongs. For EAS this is worse than a
    /// visible failure: a downstream broadcaster could air a "successful"
    /// alert that tells the public nothing. This now fails loudly instead
    /// of fabricating silence.
    ///
    /// [`Self::load_prerecorded_announcement`] is the documented, working
    /// alternative: load a real WAV file instead of synthesizing speech.
    ///
    /// # Errors
    ///
    /// Always returns [`AutomationError::Eas`]: real TTS synthesis/loading
    /// is not implemented.
    // TODO(0.2.x): real TTS integration — synthesize or load pre-recorded
    // audio for `message` (see `config.tts_audio_path`). An EAS message
    // MUST NOT ship with a silent spoken-message gap.
    pub fn load_tts_audio(&self, message: &str) -> Result<Vec<f32>> {
        debug!("Loading TTS audio for message: {}", message);

        Err(AutomationError::Eas(
            "TTS audio not available: real text-to-speech synthesis/loading is not \
             implemented (use load_prerecorded_announcement for a pre-recorded WAV instead)"
                .to_string(),
        ))
    }

    // ── Assembly ─────────────────────────────────────────────────────────────

    /// Compose complete EAS audio message.
    ///
    /// # Safety / honesty note
    ///
    /// The spoken message is always requested via [`Self::load_tts_audio`]
    /// — composition is **not** gated on `config.tts_audio_path` being
    /// set, because a missing/unset path is not a legitimate reason to
    /// ship an EAS message with a silent body. Until real TTS exists (see
    /// [`Self::load_tts_audio`]), this function always propagates that
    /// error and therefore cannot return `Ok` with a silent gap where the
    /// emergency message belongs.
    ///
    /// This function does **not** include the SAME header or the digital
    /// end-of-message burst — it only assembles the attention tone, the
    /// spoken message, and the audible EOM tone. See
    /// [`Self::compose_full_alert`] for the protocol-shaped assembly
    /// (SAME header ×3 + attention + message + digital EOM ×3).
    ///
    /// # Errors
    ///
    /// Propagates the [`AutomationError::Eas`] from
    /// [`Self::load_tts_audio`] since real TTS is not implemented.
    pub fn compose_message(&self, message: &str) -> Result<Vec<f32>> {
        info!("Composing complete EAS audio message");

        let mut audio = Vec::new();

        // Add attention tone
        audio.extend(self.generate_attention_tone()?);

        // Add silence (1 second)
        audio.extend(vec![0.0; 48000]);

        // Add TTS message. Unconditional (not gated on tts_audio_path being
        // set): an EAS message must never ship with a silently-skipped
        // spoken body.
        audio.extend(self.load_tts_audio(message)?);

        // Add silence (1 second)
        audio.extend(vec![0.0; 48000]);

        // Add end-of-message tone
        if self.config.enable_eom_tones {
            audio.extend(self.generate_eom_tone()?);
        }

        Ok(audio)
    }

    /// Assemble a complete, protocol-shaped EAS alert: the SAME header
    /// (transmitted [`SAME_REPEAT_COUNT`] times), the attention signal, the
    /// spoken message, and the digital end-of-message code (also
    /// transmitted [`SAME_REPEAT_COUNT`] times) — the structure 47 CFR
    /// 11.31 requires.
    ///
    /// The one-second pauses immediately before and after `message_audio`
    /// are this function's own convention for breathing room, **not** a 47
    /// CFR 11.31 requirement; only the pauses *inside*
    /// [`Self::generate_same_header_audio`] and [`Self::generate_eom_burst`]
    /// (between repeats of the same burst) are spec-mandated.
    ///
    /// `message_audio` is the already-decoded spoken message at
    /// [`EAS_SAMPLE_RATE`] — see [`Self::load_prerecorded_announcement`] for
    /// the supported way to obtain it today. [`Self::load_tts_audio`] is
    /// *not* called by this function: synthesizing it is not implemented, so
    /// a caller with no pre-recorded file must go through that method first
    /// and accept its honest `Err`.
    ///
    /// # Errors
    ///
    /// Returns [`AutomationError::Eas`] if `header` cannot be encoded (see
    /// [`SameHeader::to_same_string`]), or if `message_audio` is empty — an
    /// empty spoken-message slice would silently ship the same
    /// complete-looking-but-silent alert this module's history exists to
    /// prevent, so it is rejected here rather than only at the
    /// [`Self::load_tts_audio`] boundary.
    pub fn compose_full_alert(
        &self,
        header: &SameHeader,
        message_audio: &[f32],
    ) -> Result<Vec<f32>> {
        info!("Composing full SAME-protocol EAS alert");

        if message_audio.is_empty() {
            return Err(AutomationError::Eas(
                "EAS message audio must not be empty: an empty slice would ship a \
                 complete-looking alert with a silent spoken body"
                    .to_string(),
            ));
        }

        let pause = vec![0.0f32; EAS_SAMPLE_RATE as usize];

        let mut audio = self.generate_same_header_audio(header)?;
        audio.extend(self.generate_attention_tone()?);
        audio.extend_from_slice(&pause);
        audio.extend_from_slice(message_audio);
        audio.extend_from_slice(&pause);
        audio.extend(self.generate_eom_burst());

        Ok(audio)
    }

    /// Set attention tone volume.
    pub fn set_volume(&mut self, volume: f32) {
        self.config.attention_tone.volume = volume.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use oximedia_audio::wav::{WavSpec, WavWriter};

    // ── Test helpers ─────────────────────────────────────────────────────────

    /// Count sign changes across consecutive samples — each full sine cycle
    /// contributes exactly two.
    fn count_zero_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
            .count()
    }

    /// Goertzel single-bin power at `target_hz` over one bit-period window.
    fn goertzel_power(window: &[f32], target_hz: f32) -> f32 {
        let omega = std::f32::consts::TAU * target_hz / EAS_SAMPLE_RATE;
        let coeff = 2.0 * omega.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &sample in window {
            let s0 = sample + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    /// Minimal non-coherent AFSK demodulator: for each bit period, compares
    /// Goertzel energy at the mark and space frequencies and keeps whichever
    /// is stronger, then packs 8 bits (LSB first, matching `afsk_modulate`'s
    /// framing) into each output byte. Just enough to prove `afsk_modulate`
    /// produces tones a real SAME receiver could lock onto — not a
    /// production demodulator (no bit-sync recovery, no matched filtering).
    fn demodulate_same_afsk(samples: &[f32], num_bytes: usize) -> Vec<u8> {
        let samples_per_bit = EAS_SAMPLE_RATE / SAME_BAUD_RATE;
        let mut bytes = Vec::with_capacity(num_bytes);
        for byte_index in 0..num_bytes {
            let mut byte = 0u8;
            for bit_index in 0..8 {
                let global_bit = byte_index * 8 + bit_index;
                let start = (global_bit as f32 * samples_per_bit).round() as usize;
                let end = (((global_bit + 1) as f32) * samples_per_bit).round() as usize;
                let end = end.min(samples.len());
                if start >= end {
                    continue;
                }
                let mark = goertzel_power(&samples[start..end], SAME_MARK_HZ);
                let space = goertzel_power(&samples[start..end], SAME_SPACE_HZ);
                if mark > space {
                    byte |= 1 << bit_index;
                }
            }
            bytes.push(byte);
        }
        bytes
    }

    fn temp_wav_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "oximedia_eas_audio_test_{name}_{:?}",
            std::thread::current().id()
        ));
        path
    }

    fn write_wav(path: &std::path::Path, spec: WavSpec, samples: &[f32]) {
        let file = std::fs::File::create(path).expect("create temp wav");
        let mut writer = WavWriter::new(file, spec);
        writer.write_samples_f32(samples).expect("write samples");
        // `finalize()` seeks back, patches the RIFF/data sizes, and flushes
        // synchronously, so the file is complete and on disk once this
        // returns — no separate reopen-and-flush dance needed.
        writer.finalize().expect("finalize wav");
    }

    // ── Pre-existing behaviour (unchanged) ──────────────────────────────────

    #[test]
    fn test_audio_insertion_creation() {
        let config = EasAudioConfig::default();
        let insertion = EasAudioInsertion::new(config);
        assert_eq!(insertion.config.attention_tone.frequencies.len(), 2);
    }

    #[test]
    fn eas_audio_config_deserializes_a_pre_same_volume_config_to_the_default() {
        // `same_volume` was added after `EasAudioConfig` already shipped, so
        // a config saved before that (a config file, a snapshot) has no
        // `same_volume` key. Without `#[serde(default)]` this would fail
        // deserialization outright instead of falling back to the same
        // value `EasAudioConfig::default()` uses.
        let legacy_json = serde_json::json!({
            "attention_tone": {
                "frequencies": [853.0, 960.0],
                "duration": 8.0,
                "volume": 0.8
            },
            "tts_audio_path": null,
            "enable_eom_tones": true
        });
        let config: EasAudioConfig =
            serde_json::from_value(legacy_json).expect("legacy config without same_volume");
        assert!(
            (config.same_volume - 0.8).abs() < f32::EPSILON,
            "expected the documented default, got {}",
            config.same_volume
        );
    }

    #[test]
    fn eas_audio_config_round_trips_through_json() {
        let config = EasAudioConfig::default();
        let json = serde_json::to_value(&config).expect("serialize");
        let restored: EasAudioConfig = serde_json::from_value(json).expect("deserialize");
        assert!((restored.same_volume - config.same_volume).abs() < f32::EPSILON);
        assert_eq!(restored.enable_eom_tones, config.enable_eom_tones);
    }

    #[test]
    fn test_generate_attention_tone() {
        let config = EasAudioConfig::default();
        let insertion = EasAudioInsertion::new(config);

        let tone = insertion
            .generate_attention_tone()
            .expect("generate_attention_tone should succeed");
        assert!(!tone.is_empty());
        assert_eq!(tone.len(), 48000 * 8); // 8 seconds at 48kHz
    }

    #[test]
    fn test_generate_eom_tone() {
        let config = EasAudioConfig::default();
        let insertion = EasAudioInsertion::new(config);

        let tone = insertion
            .generate_eom_tone()
            .expect("generate_eom_tone should succeed");
        assert!(!tone.is_empty());
        assert_eq!(tone.len(), 48000 * 3); // 3 seconds at 48kHz
    }

    #[test]
    fn test_compose_message_is_honest_err_not_silent_success() {
        // CHANGED: this test previously pinned the fabricated (and, for an
        // *Emergency Alert System*, unsafe) behavior — compose_message()
        // used to return Ok() with a complete-looking tone/silence/tone
        // structure while the actual spoken emergency message was silent,
        // because load_tts_audio() fabricated empty samples. Real TTS is
        // not implemented, so compose_message() must now fail loudly
        // instead of shipping a silent "successful" alert.
        let config = EasAudioConfig::default();
        let insertion = EasAudioInsertion::new(config);

        let result = insertion.compose_message("Test message");

        assert!(
            result.is_err(),
            "an EAS message with a silent spoken body must never report success"
        );
        assert!(matches!(result.unwrap_err(), AutomationError::Eas(_)));
    }

    #[test]
    fn test_load_tts_audio_is_honest_err() {
        let config = EasAudioConfig::default();
        let insertion = EasAudioInsertion::new(config);

        let result = insertion.load_tts_audio("Test message");

        assert!(
            result.is_err(),
            "load_tts_audio must not fabricate empty/silent samples as if TTS succeeded"
        );
    }

    #[test]
    fn test_compose_message_fails_even_without_configured_tts_path() {
        // The spoken message is mandatory regardless of whether
        // `tts_audio_path` is configured — an unset path is not a
        // legitimate reason to silently skip the emergency message.
        let mut config = EasAudioConfig::default();
        config.tts_audio_path = None;
        let insertion = EasAudioInsertion::new(config);

        assert!(insertion.compose_message("Test message").is_err());
    }

    // ── SameHeader::to_same_string ───────────────────────────────────────────

    fn fixed_issued_at() -> SystemTime {
        // 2024-01-15 14:30:00 UTC: day-of-year 15 (Julian "015").
        chrono::Utc
            .with_ymd_and_hms(2024, 1, 15, 14, 30, 0)
            .single()
            .expect("valid datetime")
            .into()
    }

    fn sample_header() -> SameHeader {
        SameHeader {
            originator: "WXR".to_string(),
            event_code: "TOR".to_string(),
            location_codes: vec!["037183".to_string()],
            purge_minutes: 60,
            issued_at: fixed_issued_at(),
            station_id: "KGYX".to_string(),
        }
    }

    #[test]
    fn same_header_renders_the_documented_format() {
        let text = sample_header().to_same_string().expect("valid header");
        assert_eq!(text, "ZCZC-WXR-TOR-037183+0100-0151430-KGYX////-");
    }

    #[test]
    fn same_header_renders_multiple_locations() {
        let mut header = sample_header();
        header.location_codes = vec!["037183".to_string(), "037101".to_string()];
        let text = header.to_same_string().expect("valid header");
        assert_eq!(text, "ZCZC-WXR-TOR-037183-037101+0100-0151430-KGYX////-");
    }

    #[test]
    fn same_header_full_length_station_id_needs_no_padding() {
        let mut header = sample_header();
        header.station_id = "KGYXNWSX".to_string();
        let text = header.to_same_string().expect("valid header");
        assert!(text.ends_with("KGYXNWSX-"), "{text}");
    }

    #[test]
    fn same_header_rejects_bad_originator() {
        let mut header = sample_header();
        header.originator = "wx".to_string();
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_bad_event_code() {
        let mut header = sample_header();
        header.event_code = "tornado".to_string();
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_empty_locations() {
        let mut header = sample_header();
        header.location_codes.clear();
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_too_many_locations() {
        let mut header = sample_header();
        header.location_codes = (0..=SAME_MAX_LOCATIONS)
            .map(|i| format!("{i:06}"))
            .collect();
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_malformed_location_code() {
        let mut header = sample_header();
        header.location_codes = vec!["37183".to_string()]; // 5 digits, not 6
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_purge_time_over_two_digit_hours() {
        let mut header = sample_header();
        header.purge_minutes = 100 * 60;
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_oversized_station_id() {
        let mut header = sample_header();
        header.station_id = "TOOLONGID".to_string(); // 9 chars
        assert!(header.to_same_string().is_err());
    }

    #[test]
    fn same_header_rejects_empty_station_id() {
        let mut header = sample_header();
        header.station_id = String::new();
        assert!(header.to_same_string().is_err());
    }

    // ── AFSK modulation ──────────────────────────────────────────────────────

    #[test]
    fn afsk_modulate_mark_tone_has_the_exact_cycle_count() {
        // SAME_MARK_HZ is exactly 4x SAME_BAUD_RATE, so an all-1-bits
        // (0xFF) payload of `bits` bits contains exactly `4 * bits` full
        // cycles of the mark tone, i.e. `8 * bits` zero crossings.
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let payload = vec![0xFFu8; 20];
        let bits = payload.len() * 8;
        let samples = insertion.afsk_modulate(&payload);
        let crossings = count_zero_crossings(&samples);
        let expected = 8 * bits;
        assert!(
            (crossings as i64 - expected as i64).abs() <= 2,
            "expected ~{expected} zero crossings for a pure {SAME_MARK_HZ} Hz tone, got \
             {crossings}"
        );
    }

    #[test]
    fn afsk_modulate_space_tone_has_the_exact_cycle_count() {
        // SAME_SPACE_HZ is exactly 3x SAME_BAUD_RATE: `6 * bits` crossings.
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let payload = vec![0x00u8; 20];
        let bits = payload.len() * 8;
        let samples = insertion.afsk_modulate(&payload);
        let crossings = count_zero_crossings(&samples);
        let expected = 6 * bits;
        assert!(
            (crossings as i64 - expected as i64).abs() <= 2,
            "expected ~{expected} zero crossings for a pure {SAME_SPACE_HZ} Hz tone, got \
             {crossings}"
        );
    }

    #[test]
    fn afsk_modulate_empty_payload_is_empty() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        assert!(insertion.afsk_modulate(&[]).is_empty());
    }

    #[test]
    fn same_header_afsk_round_trips_through_a_demodulator() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let text = sample_header().to_same_string().expect("valid header");

        let modulated = insertion.afsk_modulate(text.as_bytes());
        let recovered_bytes = demodulate_same_afsk(&modulated, text.len());
        let recovered = String::from_utf8(recovered_bytes).expect("ascii payload");

        assert_eq!(recovered, text);
    }

    #[test]
    fn eom_payload_afsk_round_trips_through_a_demodulator() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let modulated = insertion.afsk_modulate(SAME_EOM_PAYLOAD);
        let recovered = demodulate_same_afsk(&modulated, SAME_EOM_PAYLOAD.len());
        assert_eq!(recovered, SAME_EOM_PAYLOAD);
    }

    #[test]
    fn generate_same_header_audio_repeats_three_times_with_gaps() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let text = sample_header().to_same_string().expect("valid header");
        let framed_len = SAME_PREAMBLE_LEN + text.len();
        let samples_per_bit = EAS_SAMPLE_RATE / SAME_BAUD_RATE;
        let burst_len = (samples_per_bit * (framed_len * 8) as f32).round() as usize;
        let gap_len = (EAS_SAMPLE_RATE * SAME_BURST_GAP.as_secs_f32()).round() as usize;
        let expected_len = burst_len * SAME_REPEAT_COUNT + gap_len * (SAME_REPEAT_COUNT - 1);

        let audio = insertion
            .generate_same_header_audio(&sample_header())
            .expect("valid header");
        assert_eq!(audio.len(), expected_len);
    }

    #[test]
    fn generate_same_header_audio_propagates_header_errors() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let mut header = sample_header();
        header.originator = "bad".to_string();
        assert!(insertion.generate_same_header_audio(&header).is_err());
    }

    #[test]
    fn generate_eom_burst_is_three_repeats_of_the_nnnn_payload() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let framed_len = SAME_PREAMBLE_LEN + SAME_EOM_PAYLOAD.len();
        let samples_per_bit = EAS_SAMPLE_RATE / SAME_BAUD_RATE;
        let burst_len = (samples_per_bit * (framed_len * 8) as f32).round() as usize;
        let gap_len = (EAS_SAMPLE_RATE * SAME_BURST_GAP.as_secs_f32()).round() as usize;
        let expected_len = burst_len * SAME_REPEAT_COUNT + gap_len * (SAME_REPEAT_COUNT - 1);

        assert_eq!(insertion.generate_eom_burst().len(), expected_len);
    }

    // ── Pre-recorded announcement loading ────────────────────────────────────

    #[test]
    fn load_prerecorded_announcement_decodes_a_real_wav_file() {
        let path = temp_wav_path("mono48k");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            float: true,
        };
        let input: Vec<f32> = (0..500).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        write_wav(&path, spec, &input);

        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let decoded = insertion
            .load_prerecorded_announcement(&path)
            .expect("real WAV decodes");

        assert_eq!(decoded.len(), input.len());
        for (a, b) in input.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_prerecorded_announcement_downmixes_stereo_to_mono() {
        let path = temp_wav_path("stereo48k");
        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            float: true,
        };
        // Interleaved L, R: constant 1.0 / -1.0 so the average is exactly 0.0.
        let input = vec![1.0f32, -1.0, 1.0, -1.0, 1.0, -1.0];
        write_wav(&path, spec, &input);

        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let decoded = insertion
            .load_prerecorded_announcement(&path)
            .expect("real WAV decodes");

        assert_eq!(decoded.len(), 3, "3 stereo frames -> 3 mono samples");
        for sample in decoded {
            assert!(sample.abs() < 1e-5, "expected ~0.0, got {sample}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_prerecorded_announcement_rejects_wrong_sample_rate() {
        let path = temp_wav_path("wrongrate");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            float: false,
        };
        write_wav(&path, spec, &[0.0, 0.1, 0.2]);

        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let error = insertion
            .load_prerecorded_announcement(&path)
            .expect_err("wrong sample rate must be rejected");
        let message = error.to_string();
        assert!(message.contains("44100"), "{message}");
        assert!(message.contains("48000"), "{message}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_prerecorded_announcement_rejects_a_missing_file() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let path = temp_wav_path("does_not_exist");
        let _ = std::fs::remove_file(&path); // ensure it is really absent
        assert!(insertion.load_prerecorded_announcement(&path).is_err());
    }

    // ── Full alert assembly ──────────────────────────────────────────────────

    #[test]
    fn compose_full_alert_rejects_empty_message_audio() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let error = insertion
            .compose_full_alert(&sample_header(), &[])
            .expect_err("empty message audio must not be accepted as a real message");
        assert!(matches!(error, AutomationError::Eas(_)));
    }

    #[test]
    fn compose_full_alert_propagates_header_errors() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let mut header = sample_header();
        header.originator = "bad".to_string();
        let message_audio = vec![0.1f32; 100];
        assert!(insertion
            .compose_full_alert(&header, &message_audio)
            .is_err());
    }

    #[test]
    fn compose_full_alert_contains_the_message_audio_intact_and_in_order() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        // A distinctive constant that will not occur anywhere else in the
        // assembled audio (header/attention/EOM are all AFSK tones or
        // silence, none of which sustain a flat 0.42 run).
        let message_audio = vec![0.42f32; 4_000];

        let alert = insertion
            .compose_full_alert(&sample_header(), &message_audio)
            .expect("valid header and non-empty message");

        let position = alert
            .windows(message_audio.len())
            .position(|window| window == message_audio.as_slice());
        assert!(
            position.is_some(),
            "message_audio must appear intact and contiguous in the assembled alert"
        );
    }

    #[test]
    fn compose_full_alert_orders_header_before_attention_before_eom() {
        let insertion = EasAudioInsertion::new(EasAudioConfig::default());
        let header = sample_header();
        let message_audio = vec![0.1f32; 10];

        let header_audio = insertion
            .generate_same_header_audio(&header)
            .expect("valid header");
        let eom_audio = insertion.generate_eom_burst();
        let alert = insertion
            .compose_full_alert(&header, &message_audio)
            .expect("valid header and non-empty message");

        assert!(alert.len() > header_audio.len() + eom_audio.len());
        assert_eq!(&alert[..header_audio.len()], header_audio.as_slice());
        assert_eq!(
            &alert[alert.len() - eom_audio.len()..],
            eom_audio.as_slice()
        );
    }
}

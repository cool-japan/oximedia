//! A deterministic synthetic capture backend.
//!
//! Available under `cfg(test)` and behind the `mock` feature. It exists so
//! that a pipeline shaped around a live camera can be exercised without one:
//! the frame cadence, the drop policy, the sequence counter and the clock all
//! behave the way a real device makes them behave, on a schedule the test
//! writes down in advance.
//!
//! Everything it produces is synthetic and labelled as such — the device is
//! `mock:0`, [`CaptureDevice::backend`] is [`BackendKind::Mock`], and the
//! pixels are a moving gradient. It never pretends to be hardware, and it only
//! synthesizes formats it can actually fill: ask it for a compressed encoding
//! and it returns [`CaptureError::FormatRejected`] rather than emitting bytes
//! that claim to be a JPEG and are not.
//!
//! ```
//! # #[cfg(feature = "mock")]
//! # fn main() -> Result<(), oximedia_capture::CaptureError> {
//! use std::time::Duration;
//! use oximedia_capture::{mock, CaptureConfig};
//!
//! let script = mock::MockScript::default()
//!     .with_frames(3)
//!     .with_cadence(Duration::ZERO);
//! let mut session = mock::open(CaptureConfig::default(), script)?;
//! let mut stream = session.take_stream().ok_or_else(|| {
//!     oximedia_capture::CaptureError::platform("stream already taken")
//! })?;
//!
//! let mut count = 0;
//! while stream.recv()?.is_some() {
//!     count += 1;
//! }
//! assert_eq!(count, 3);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "mock"))]
//! # fn main() {}
//! ```

use std::time::{Duration, Instant};

use oximedia_codec::VideoFrame;
use oximedia_core::{PixelFormat, Rational, Timestamp};

use crate::backend::{CaptureBackend, CaptureRunner, FrameSink, StopSignal};
use crate::config::CaptureConfig;
use crate::device::{BackendKind, CaptureDevice, CaptureEncoding, CaptureFormat};
use crate::error::CaptureError;
use crate::frame::{CaptureFrame, FramePayload, TimestampSource};
use crate::session::CaptureSession;

/// Identifier of the single device this backend advertises.
pub const MOCK_DEVICE_ID: &str = "mock:0";

/// Product name of the single device this backend advertises.
pub const MOCK_DEVICE_NAME: &str = "OxiMedia Mock Camera";

/// Longest uninterrupted sleep inside the capture loop.
///
/// The loop must notice [`StopSignal`] promptly, so a long cadence is served
/// in slices rather than one `thread::sleep` — otherwise dropping a session
/// scripted at 20 ms x 10 000 frames would block the dropping thread for
/// minutes.
const SLEEP_SLICE: Duration = Duration::from_millis(5);

// ── Script ───────────────────────────────────────────────────────────────────

/// What the mock device should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockScript {
    /// How many frames to emit before the stream ends normally.
    pub frames: usize,
    /// Emit a fatal [`CaptureError::Platform`] instead of this frame index.
    pub fail_at: Option<usize>,
    /// Wall-clock delay between frames. [`Duration::ZERO`] runs flat out.
    pub cadence: Duration,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Encoding to advertise and synthesize.
    ///
    /// Only [`PixelFormat::Gray8`] and [`PixelFormat::Nv12`] can actually be
    /// filled; anything else is advertised but rejected on open.
    pub encoding: CaptureEncoding,
    /// Frame rate to advertise, in whole frames per second.
    pub fps: u32,
    /// Device timestamps to report, one per frame.
    ///
    /// Shorter than `frames` means the remainder is derived from the nominal
    /// frame period. Supplying a decreasing series is the point: it exercises
    /// the clamping in [`crate::CaptureStats::clock_regressions`].
    pub device_timestamps: Vec<Duration>,
    /// Increment applied to the reported sequence number per frame.
    ///
    /// A step greater than one simulates a driver that dropped frames before
    /// this crate saw them, which the sink detects from the sequence gap.
    pub sequence_step: u64,
    /// Frame indices at which to report a recoverable error and carry on.
    ///
    /// Real backends hit these constantly — a short read, a transient
    /// `EAGAIN`, one malformed buffer — and none of them should end a session.
    pub transient_at: Vec<usize>,
    /// Frame indices at which to report one driver-side frame loss explicitly.
    ///
    /// This models a backend that can read a driver overrun counter, as
    /// opposed to inferring loss from a sequence gap.
    pub device_drops_at: Vec<usize>,
}

impl Default for MockScript {
    fn default() -> Self {
        Self {
            frames: 8,
            fail_at: None,
            cadence: Duration::from_millis(1),
            width: 64,
            height: 48,
            encoding: CaptureEncoding::Raw(PixelFormat::Gray8),
            fps: 30,
            device_timestamps: Vec::new(),
            sequence_step: 1,
            transient_at: Vec::new(),
            device_drops_at: Vec::new(),
        }
    }
}

impl MockScript {
    /// Set how many frames to emit.
    pub fn with_frames(mut self, frames: usize) -> Self {
        self.frames = frames;
        self
    }

    /// Fail fatally instead of emitting the frame at `index`.
    pub fn with_fail_at(mut self, index: usize) -> Self {
        self.fail_at = Some(index);
        self
    }

    /// Set the delay between frames.
    pub fn with_cadence(mut self, cadence: Duration) -> Self {
        self.cadence = cadence;
        self
    }

    /// Set the frame size.
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the encoding to advertise and synthesize.
    pub fn with_encoding(mut self, encoding: CaptureEncoding) -> Self {
        self.encoding = encoding;
        self
    }

    /// Set the advertised frame rate.
    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    /// Report these device timestamps instead of a regular cadence.
    pub fn with_device_timestamps(
        mut self,
        timestamps: impl IntoIterator<Item = Duration>,
    ) -> Self {
        self.device_timestamps = timestamps.into_iter().collect();
        self
    }

    /// Advance the reported sequence number by `step` per frame.
    pub fn with_sequence_step(mut self, step: u64) -> Self {
        self.sequence_step = step;
        self
    }

    /// Report a recoverable error before each of these frame indices.
    pub fn with_transient_at(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.transient_at = indices.into_iter().collect();
        self
    }

    /// Report one driver-side frame loss before each of these frame indices.
    pub fn with_device_drops_at(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.device_drops_at = indices.into_iter().collect();
        self
    }

    /// The single format this script's device advertises.
    pub fn format(&self) -> CaptureFormat {
        CaptureFormat::new(self.encoding, self.width, self.height, self.fps, 1)
    }

    /// Nominal frame period derived from [`Self::fps`].
    fn frame_period(&self) -> Duration {
        if self.fps == 0 {
            Duration::ZERO
        } else {
            Duration::from_nanos(1_000_000_000 / u64::from(self.fps))
        }
    }

    /// Device timestamp for frame `index`.
    fn device_timestamp(&self, index: usize) -> Duration {
        if let Some(&explicit) = self.device_timestamps.get(index) {
            return explicit;
        }
        let steps = u32::try_from(index).unwrap_or(u32::MAX);
        self.frame_period()
            .checked_mul(steps)
            .unwrap_or(Duration::MAX)
    }
}

// ── Public entry points ──────────────────────────────────────────────────────

/// The devices a [`MockScript`] advertises: always exactly one.
pub(crate) fn devices(script: &MockScript) -> Vec<CaptureDevice> {
    vec![CaptureDevice {
        id: MOCK_DEVICE_ID.to_owned(),
        name: MOCK_DEVICE_NAME.to_owned(),
        formats: vec![script.format()],
        backend: BackendKind::Mock,
    }]
}

/// Enumerate the mock device using the default script.
///
/// # Errors
///
/// Never fails today; the signature matches [`crate::enumerate`] so callers
/// can swap between them.
pub fn enumerate() -> Result<Vec<CaptureDevice>, CaptureError> {
    Ok(devices(&MockScript::default()))
}

/// Open a session against the synthetic device described by `script`.
///
/// `config` goes through exactly the same path a real device would take —
/// enumeration, selection, [`crate::negotiate()`], thread spawn — so the
/// selector, drop policy and queue depth all behave as they will in
/// production.
///
/// # Errors
///
/// See [`CaptureError`]. In particular, a script whose encoding the mock
/// cannot synthesize yields [`CaptureError::FormatRejected`].
pub fn open(config: CaptureConfig, script: MockScript) -> Result<CaptureSession, CaptureError> {
    CaptureSession::open_with_backend(config, Box::new(MockBackend { script }))
}

// ── Backend ──────────────────────────────────────────────────────────────────

/// The synthetic backend itself.
#[derive(Debug, Clone)]
pub(crate) struct MockBackend {
    script: MockScript,
}

impl CaptureBackend for MockBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Mock
    }

    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(devices(&self.script))
    }

    fn open(
        &self,
        device: &CaptureDevice,
        format: CaptureFormat,
        _config: &CaptureConfig,
    ) -> Result<Box<dyn CaptureRunner>, CaptureError> {
        let pixel = format.pixel().ok_or_else(|| {
            CaptureError::format_rejected(
                &device.id,
                format,
                "the mock backend synthesizes raw frames only, never a compressed bitstream",
            )
        })?;
        if !matches!(pixel, PixelFormat::Gray8 | PixelFormat::Nv12) {
            return Err(CaptureError::format_rejected(
                &device.id,
                format,
                format!("the mock backend can fill Gray8 and Nv12 planes, not {pixel:?}"),
            ));
        }
        Ok(Box::new(MockRunner {
            script: self.script.clone(),
            format,
            pixel,
        }))
    }
}

// ── Runner ───────────────────────────────────────────────────────────────────

/// The synthetic capture loop.
struct MockRunner {
    script: MockScript,
    format: CaptureFormat,
    pixel: PixelFormat,
}

impl MockRunner {
    /// Build the payload for frame `index`: a gradient that moves by frame.
    fn payload(&self, index: usize, device_timestamp: Duration) -> FramePayload {
        let mut video = VideoFrame::new(self.pixel, self.format.width, self.format.height);
        video.allocate();
        fill_gradient(&mut video, index);
        let millis = i64::try_from(device_timestamp.as_millis()).unwrap_or(i64::MAX);
        // `Rational::new` only panics on a zero denominator; 1000 is a literal.
        video.timestamp = Timestamp::new(millis, Rational::new(1, 1000));
        FramePayload::Raw(video)
    }
}

impl CaptureRunner for MockRunner {
    fn run(&mut self, sink: &FrameSink, stop: &StopSignal) -> Result<(), CaptureError> {
        let started = Instant::now();
        for index in 0..self.script.frames {
            if stop.is_stopped() {
                return Ok(());
            }
            if self.script.fail_at == Some(index) {
                return Err(CaptureError::platform(format!(
                    "mock backend: scripted failure at frame {index}"
                )));
            }
            if self.script.transient_at.contains(&index) {
                sink.report_transient(&CaptureError::platform(format!(
                    "mock backend: scripted transient hiccup at frame {index}"
                )));
            }
            if self.script.device_drops_at.contains(&index) {
                sink.report_device_dropped(1);
            }

            let device_timestamp = self.script.device_timestamp(index);
            let sequence = (index as u64).saturating_mul(self.script.sequence_step);
            sink.deliver(CaptureFrame {
                format: self.format,
                payload: self.payload(index, device_timestamp),
                timestamp: device_timestamp,
                host_timestamp: started.elapsed(),
                timestamp_source: TimestampSource::DeviceMonotonic,
                sequence,
            });

            if !sleep_interruptible(self.script.cadence, stop) {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Sleep for `total`, waking every [`SLEEP_SLICE`] to check `stop`.
///
/// Returns `false` if the stop signal was raised.
fn sleep_interruptible(total: Duration, stop: &StopSignal) -> bool {
    let mut remaining = total;
    while !remaining.is_zero() {
        if stop.is_stopped() {
            return false;
        }
        let slice = remaining.min(SLEEP_SLICE);
        std::thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
    !stop.is_stopped()
}

/// Paint a deterministic diagonal gradient that shifts with the frame index.
///
/// Bytes are written by plane and row using each plane's own stride, so the
/// pattern is valid for both the single-plane Gray8 layout and the two-plane
/// NV12 one without either being special-cased.
fn fill_gradient(video: &mut VideoFrame, index: usize) {
    let phase = (index.wrapping_mul(7) & 0xFF) as u8;
    for (plane_index, plane) in video.planes.iter_mut().enumerate() {
        let stride = plane.stride.max(1);
        let plane_bias = (plane_index.wrapping_mul(64) & 0xFF) as u8;
        for (row, chunk) in plane.data.chunks_mut(stride).enumerate() {
            let row_bias = (row & 0xFF) as u8;
            for (column, byte) in chunk.iter_mut().enumerate() {
                *byte = ((column & 0xFF) as u8)
                    .wrapping_add(row_bias)
                    .wrapping_add(phase)
                    .wrapping_add(plane_bias);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceSelector;

    #[test]
    fn default_script_advertises_one_gray8_device() {
        let script = MockScript::default();
        let devices = devices(&script);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, MOCK_DEVICE_ID);
        assert_eq!(devices[0].backend, BackendKind::Mock);
        assert_eq!(devices[0].formats, vec![script.format()]);
        assert_eq!(
            script.format().pixel(),
            Some(PixelFormat::Gray8),
            "default script must be fillable"
        );
    }

    #[test]
    fn enumerate_matches_the_default_script() {
        let listed = enumerate().expect("the mock always enumerates");
        assert_eq!(listed, devices(&MockScript::default()));
    }

    #[test]
    fn scripted_transients_do_not_end_the_session() {
        let mut session = open(
            CaptureConfig::default().with_queue_depth(8),
            MockScript::default()
                .with_frames(4)
                .with_cadence(Duration::ZERO)
                .with_transient_at([1, 2])
                .with_device_drops_at([3]),
        )
        .expect("the mock device exists");
        let mut stream = session.take_stream().expect("first take");
        let mut delivered = 0usize;
        while stream.recv().expect("transients are not fatal").is_some() {
            delivered += 1;
        }
        assert_eq!(delivered, 4, "every frame still arrives");
        let stats = session.stats();
        assert_eq!(stats.errors, 2);
        assert_eq!(stats.device_dropped, 1);
        assert!(stats
            .last_error
            .is_some_and(|message| message.contains("transient hiccup")));
    }

    #[test]
    fn builders_set_every_field() {
        let script = MockScript::default()
            .with_frames(3)
            .with_fail_at(1)
            .with_cadence(Duration::from_millis(7))
            .with_size(320, 240)
            .with_encoding(CaptureEncoding::Raw(PixelFormat::Nv12))
            .with_fps(60)
            .with_device_timestamps([Duration::from_millis(4)])
            .with_sequence_step(2)
            .with_transient_at([0])
            .with_device_drops_at([2]);
        assert_eq!(script.frames, 3);
        assert_eq!(script.fail_at, Some(1));
        assert_eq!(script.cadence, Duration::from_millis(7));
        assert_eq!((script.width, script.height), (320, 240));
        assert_eq!(script.encoding, CaptureEncoding::Raw(PixelFormat::Nv12));
        assert_eq!(script.fps, 60);
        assert_eq!(script.device_timestamps, vec![Duration::from_millis(4)]);
        assert_eq!(script.sequence_step, 2);
        assert_eq!(script.transient_at, vec![0]);
        assert_eq!(script.device_drops_at, vec![2]);
    }

    #[test]
    fn device_timestamps_fall_back_to_the_nominal_frame_period() {
        let script = MockScript::default().with_fps(50);
        assert_eq!(script.device_timestamp(0), Duration::ZERO);
        assert_eq!(script.device_timestamp(4), Duration::from_millis(80));
    }

    #[test]
    fn explicit_device_timestamps_take_precedence() {
        let script = MockScript::default()
            .with_device_timestamps([Duration::from_millis(9), Duration::from_millis(2)]);
        assert_eq!(script.device_timestamp(0), Duration::from_millis(9));
        assert_eq!(script.device_timestamp(1), Duration::from_millis(2));
        // Past the end of the list, the nominal period resumes.
        assert_eq!(script.device_timestamp(2), script.frame_period() * 2);
    }

    #[test]
    fn a_zero_frame_rate_yields_a_zero_frame_period() {
        let script = MockScript::default().with_fps(0);
        assert_eq!(script.frame_period(), Duration::ZERO);
        assert_eq!(script.device_timestamp(10), Duration::ZERO);
    }

    #[test]
    fn nv12_scripts_open() {
        let script = MockScript::default().with_encoding(CaptureEncoding::Raw(PixelFormat::Nv12));
        let backend = MockBackend {
            script: script.clone(),
        };
        let device = devices(&script).remove(0);
        assert!(backend
            .open(&device, script.format(), &CaptureConfig::default())
            .is_ok());
    }

    #[test]
    fn compressed_scripts_are_rejected_rather_than_faked() {
        let script = MockScript::default().with_encoding(CaptureEncoding::Mjpeg);
        let backend = MockBackend {
            script: script.clone(),
        };
        let device = devices(&script).remove(0);
        let error = match backend.open(&device, script.format(), &CaptureConfig::default()) {
            Err(error) => error,
            Ok(_) => panic!("the mock must not claim it can synthesize a JPEG"),
        };
        match error {
            CaptureError::FormatRejected { device, reason, .. } => {
                assert_eq!(device, MOCK_DEVICE_ID);
                assert!(reason.contains("compressed"), "{reason}");
            }
            other => panic!("expected FormatRejected, got {other:?}"),
        }
    }

    #[test]
    fn unfillable_raw_formats_are_rejected() {
        let script = MockScript::default().with_encoding(CaptureEncoding::Raw(PixelFormat::Rgb24));
        let backend = MockBackend {
            script: script.clone(),
        };
        let device = devices(&script).remove(0);
        let error = match backend.open(&device, script.format(), &CaptureConfig::default()) {
            Err(error) => error,
            Ok(_) => panic!("Rgb24 is advertised but not fillable"),
        };
        assert!(matches!(error, CaptureError::FormatRejected { .. }));
    }

    #[test]
    fn the_gradient_moves_between_frames() {
        let mut first = VideoFrame::new(PixelFormat::Gray8, 16, 8);
        first.allocate();
        fill_gradient(&mut first, 0);
        let mut second = VideoFrame::new(PixelFormat::Gray8, 16, 8);
        second.allocate();
        fill_gradient(&mut second, 1);
        assert_ne!(
            first.planes.first().map(|plane| plane.data.clone()),
            second.planes.first().map(|plane| plane.data.clone())
        );
    }

    #[test]
    fn the_gradient_is_reproducible() {
        let render = |index: usize| {
            let mut video = VideoFrame::new(PixelFormat::Nv12, 32, 16);
            video.allocate();
            fill_gradient(&mut video, index);
            video
                .planes
                .iter()
                .map(|plane| plane.data.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(render(5), render(5));
    }

    #[test]
    fn the_gradient_fills_every_plane() {
        let mut video = VideoFrame::new(PixelFormat::Nv12, 32, 16);
        video.allocate();
        fill_gradient(&mut video, 3);
        assert!(video.planes.len() >= 2);
        assert!(
            video
                .planes
                .iter()
                .all(|plane| plane.data.iter().any(|&byte| byte != 0)),
            "every plane should carry pattern"
        );
    }

    #[test]
    fn interruptible_sleep_returns_early_once_stopped() {
        let stop = StopSignal::new();
        stop.stop();
        let started = Instant::now();
        assert!(!sleep_interruptible(Duration::from_secs(30), &stop));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn interruptible_sleep_runs_to_completion_when_not_stopped() {
        let stop = StopSignal::new();
        let started = Instant::now();
        assert!(sleep_interruptible(Duration::from_millis(12), &stop));
        assert!(started.elapsed() >= Duration::from_millis(12));
    }

    #[test]
    fn a_zero_cadence_does_not_sleep() {
        let stop = StopSignal::new();
        let started = Instant::now();
        assert!(sleep_interruptible(Duration::ZERO, &stop));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn open_goes_through_device_selection() {
        let session = open(
            CaptureConfig::default().with_device(DeviceSelector::Id(MOCK_DEVICE_ID.to_owned())),
            MockScript::default()
                .with_frames(1)
                .with_cadence(Duration::ZERO),
        )
        .expect("the mock device exists");
        assert_eq!(session.device().id, MOCK_DEVICE_ID);
        assert_eq!(session.backend(), BackendKind::Mock);
    }

    #[test]
    fn a_zero_frame_script_ends_immediately() {
        let mut session = open(
            CaptureConfig::default(),
            MockScript::default().with_frames(0),
        )
        .expect("the mock device exists");
        let mut stream = session.take_stream().expect("first take");
        assert!(stream.recv().expect("no error").is_none());
        assert!(stream.is_ended());
        assert_eq!(session.stats().delivered, 0);
    }

    #[test]
    fn failing_at_frame_zero_delivers_nothing() {
        let mut session = open(
            CaptureConfig::default(),
            MockScript::default().with_frames(4).with_fail_at(0),
        )
        .expect("the mock device exists");
        let mut stream = session.take_stream().expect("first take");
        let error = stream.recv().expect_err("the scripted failure");
        assert!(matches!(error, CaptureError::Platform(_)));
        assert!(stream.recv().expect("ended").is_none());
    }
}

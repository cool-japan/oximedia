//! The running capture session: one device, one thread, one bounded queue.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};
use parking_lot::Mutex;

use crate::backend::{CaptureBackend, FrameMessage, FrameSink, StopSignal};
use crate::config::CaptureConfig;
use crate::device::{BackendKind, CaptureDevice, CaptureFormat, DeviceSelector};
use crate::error::CaptureError;
use crate::frame::CaptureFrame;
use crate::negotiate::negotiate;

/// Longest thread-name suffix taken from a device id.
const MAX_THREAD_NAME_ID: usize = 32;

/// Prefix of every capture thread's name.
const THREAD_NAME_PREFIX: &str = "oximedia-capture-";

// ── Statistics ───────────────────────────────────────────────────────────────

/// A snapshot of one session's counters.
///
/// The three loss counters answer three different questions, and keeping them
/// apart is the point: [`Self::dropped`] is this crate discarding frames to
/// honour [`crate::DropPolicy`], [`Self::device_dropped`] is the driver losing
/// frames before this crate ever saw them, and [`Self::errors`] is the backend
/// reporting trouble it recovered from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureStats {
    /// Frames that reached the delivery queue.
    pub delivered: u64,
    /// Frames this crate discarded because the queue was full.
    pub dropped: u64,
    /// Frames lost upstream, inferred from gaps in the driver's sequence
    /// counter or reported directly by the backend.
    pub device_dropped: u64,
    /// Times a source clock went backwards and had to be clamped.
    pub clock_regressions: u64,
    /// Recoverable backend errors, plus the one fatal error if there was one.
    pub errors: u64,
    /// The most recent error message, if any.
    pub last_error: Option<String>,
}

/// Shared counters behind the [`CaptureStats`] snapshot.
#[derive(Debug, Default)]
pub(crate) struct StatsInner {
    delivered: AtomicU64,
    dropped: AtomicU64,
    device_dropped: AtomicU64,
    clock_regressions: AtomicU64,
    errors: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl StatsInner {
    pub(crate) fn record_delivered(&self) {
        self.delivered.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_dropped(&self, count: u64) {
        self.dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn record_device_dropped(&self, count: u64) {
        self.device_dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn record_clock_regression(&self, count: u64) {
        self.clock_regressions.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn record_error(&self, message: &str) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        *self.last_error.lock() = Some(message.to_owned());
    }

    pub(crate) fn snapshot(&self) -> CaptureStats {
        CaptureStats {
            delivered: self.delivered.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            device_dropped: self.device_dropped.load(Ordering::Relaxed),
            clock_regressions: self.clock_regressions.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_error: self.last_error.lock().clone(),
        }
    }
}

// ── Stream ───────────────────────────────────────────────────────────────────

/// The consuming end of a capture session.
///
/// Obtained once per session from [`CaptureSession::take_stream`]. Dropping it
/// does not stop capture — the session owns the thread — but with nothing
/// draining the queue the configured [`crate::DropPolicy`] takes over.
#[derive(Debug)]
pub struct CaptureStream {
    receiver: Receiver<FrameMessage>,
    ended: bool,
}

impl CaptureStream {
    fn new(receiver: Receiver<FrameMessage>) -> Self {
        Self {
            receiver,
            ended: false,
        }
    }

    /// Wait for the next frame.
    ///
    /// Returns `Ok(None)` when the session has ended and no further frames will
    /// arrive. A fatal backend failure is delivered as `Err` exactly once, and
    /// the next call then reports the end of the stream.
    ///
    /// # Errors
    ///
    /// Returns the backend's terminal error, if the session ended because of
    /// one.
    pub fn recv(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        if self.ended {
            return Ok(None);
        }
        match self.receiver.recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                self.ended = true;
                Ok(None)
            }
        }
    }

    /// Wait for the next frame, giving up after `timeout`.
    ///
    /// **A timeout and the end of the stream both return `Ok(None)`**, and
    /// [`Self::is_ended`] is what separates them:
    ///
    /// ```text
    /// match stream.recv_timeout(Duration::from_millis(100))? {
    ///     Some(frame) => process(frame),
    ///     None if stream.is_ended() => break,   // finished
    ///     None => continue,                     // just slow; poll again
    /// }
    /// ```
    ///
    /// The alternative — an error variant for "nothing yet" — would make a
    /// routine, expected poll result indistinguishable from a real failure at
    /// every `?` in the caller's code.
    ///
    /// # Errors
    ///
    /// Returns the backend's terminal error, if the session ended because of
    /// one.
    pub fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<CaptureFrame>, CaptureError> {
        if self.ended {
            return Ok(None);
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                self.ended = true;
                Ok(None)
            }
        }
    }

    /// Take a frame if one is already queued.
    ///
    /// As with [`Self::recv_timeout`], an empty queue and the end of the
    /// stream both return `Ok(None)`; [`Self::is_ended`] separates them.
    ///
    /// # Errors
    ///
    /// Returns the backend's terminal error, if the session ended because of
    /// one.
    pub fn try_recv(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        if self.ended {
            return Ok(None);
        }
        match self.receiver.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.ended = true;
                Ok(None)
            }
        }
    }

    /// Whether the end of the stream has been observed.
    ///
    /// This reflects what the receive methods have already seen; it does not
    /// probe. A session that ends between calls reports `false` until the next
    /// [`Self::recv`], [`Self::recv_timeout`] or [`Self::try_recv`] observes
    /// it.
    pub const fn is_ended(&self) -> bool {
        self.ended
    }

    /// Number of frames currently queued.
    pub fn queued(&self) -> usize {
        self.receiver.len()
    }
}

// ── Session ──────────────────────────────────────────────────────────────────

/// A live capture session.
///
/// Owns the capture thread and the device handle behind it. Dropping the
/// session raises the stop flag and joins the thread, so a session cannot
/// outlive its owner or leak a thread.
///
/// # Thread safety
///
/// `CaptureSession` is `Send + Sync` without any unsafe impls, because every
/// member already is: [`CaptureDevice`] and [`CaptureFormat`] are plain data,
/// the queue is a `crossbeam_channel::Receiver<Result<CaptureFrame, _>>` (both
/// payload types are `Send`), the stop flag is an `Arc<AtomicBool>`, the
/// counters are an `Arc` of atomics plus a `parking_lot::Mutex<Option<String>>`,
/// and `JoinHandle<()>` is `Send + Sync`. The crate denies `unsafe_code`
/// outright, so this is checked by the compiler rather than asserted — see the
/// `session_types_are_send_and_sync` test.
#[derive(Debug)]
pub struct CaptureSession {
    device: CaptureDevice,
    format: CaptureFormat,
    backend: BackendKind,
    stream: Option<CaptureStream>,
    handle: Option<JoinHandle<()>>,
    stop: StopSignal,
    stats: Arc<StatsInner>,
}

impl CaptureSession {
    /// Open a capture session on the platform's native backend.
    ///
    /// # Errors
    ///
    /// See [`CaptureError`]. On a target with no backend compiled in this is
    /// always [`CaptureError::UnsupportedPlatform`].
    pub fn open(config: CaptureConfig) -> Result<Self, CaptureError> {
        Self::open_with_backend(config, crate::platform::native())
    }

    /// Open a capture session on a specific backend.
    ///
    /// This is the whole of the open sequence: enumerate, select, negotiate,
    /// configure, spawn.
    pub(crate) fn open_with_backend(
        config: CaptureConfig,
        backend: Box<dyn CaptureBackend>,
    ) -> Result<Self, CaptureError> {
        let devices = backend.enumerate()?;
        let device = select_device(&devices, &config.device)?;

        let format = negotiate(&device.formats, &config).map_err(|e| e.with_device(&device.id))?;
        tracing::info!(
            device = %device.id,
            backend = %backend.kind(),
            requested = %RequestSummary(&config),
            negotiated = %format,
            "capture format negotiated"
        );

        let mut runner = backend.open(&device, format, &config)?;

        let (sender, receiver) =
            crossbeam_channel::bounded::<FrameMessage>(config.effective_queue_depth());
        let stop = StopSignal::new();
        let stats = Arc::new(StatsInner::default());
        let sink = FrameSink::new(
            sender,
            receiver.clone(),
            config.drop_policy,
            stop.clone(),
            Arc::clone(&stats),
        );

        let thread_stop = stop.clone();
        let handle = std::thread::Builder::new()
            .name(thread_name(&device.id))
            .spawn(move || {
                if let Err(error) = runner.run(&sink, &thread_stop) {
                    tracing::error!(error = %error, "capture loop ended with an error");
                    sink.send_fatal(error);
                }
                // Dropping `sink` here closes the channel, which is what the
                // consumer sees as the end of the stream.
            })
            .map_err(|source| CaptureError::io(device.id.clone(), source))?;

        Ok(Self {
            backend: backend.kind(),
            device,
            format,
            stream: Some(CaptureStream::new(receiver)),
            handle: Some(handle),
            stop,
            stats,
        })
    }

    /// The format the device was actually configured for.
    ///
    /// This is the outcome of [`crate::negotiate()`], not the request; it can
    /// differ from the [`CaptureConfig`] in every field.
    pub const fn negotiated_format(&self) -> CaptureFormat {
        self.format
    }

    /// The device this session is reading from.
    pub const fn device(&self) -> &CaptureDevice {
        &self.device
    }

    /// The backend that opened the device.
    pub const fn backend(&self) -> BackendKind {
        self.backend
    }

    /// Take the frame stream.
    ///
    /// Returns `Some` exactly once per session; every later call returns
    /// `None`. A session has a single delivery queue, so handing out two
    /// stream handles would silently split the frames between them.
    pub fn take_stream(&mut self) -> Option<CaptureStream> {
        self.stream.take()
    }

    /// Current counters.
    pub fn stats(&self) -> CaptureStats {
        self.stats.snapshot()
    }

    /// Whether the capture thread is still owned by this session.
    ///
    /// Becomes `false` after [`Self::stop`].
    pub const fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Stop capture and join the capture thread.
    ///
    /// Idempotent, and called automatically on drop. If the capture thread
    /// unwound, that is recorded in [`CaptureStats::last_error`] as
    /// [`CaptureError::ThreadPanic`] rather than re-raised — this runs from
    /// `Drop`, where panicking would abort the process during unwinding.
    pub fn stop(&mut self) {
        self.stop.stop();
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                let error = CaptureError::ThreadPanic;
                tracing::error!(device = %self.device.id, "{error}");
                self.stats.record_error(&error.to_string());
            }
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Pick one device out of an enumeration.
fn select_device(
    devices: &[CaptureDevice],
    selector: &DeviceSelector,
) -> Result<CaptureDevice, CaptureError> {
    let found = match selector {
        DeviceSelector::Default => devices.first(),
        DeviceSelector::Id(id) => devices.iter().find(|device| &device.id == id),
        DeviceSelector::Index(index) => devices.get(*index),
    };
    found
        .cloned()
        .ok_or_else(|| CaptureError::DeviceNotFound(selector.clone()))
}

/// Build a capture thread name from a device id.
///
/// Every byte is forced into `[A-Za-z0-9._-]` and the id is truncated, because
/// `std::thread::Builder::name` panics on an interior NUL and device ids come
/// from the operating system — a Media Foundation symbolic link is neither
/// short nor free of punctuation.
fn thread_name(device_id: &str) -> String {
    let mut name = String::with_capacity(THREAD_NAME_PREFIX.len() + MAX_THREAD_NAME_ID);
    name.push_str(THREAD_NAME_PREFIX);
    for character in device_id.chars() {
        if name.len() >= THREAD_NAME_PREFIX.len() + MAX_THREAD_NAME_ID {
            break;
        }
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            name.push(character);
        } else {
            name.push('_');
        }
    }
    name
}

/// Renders the interesting parts of a request for one log line.
struct RequestSummary<'a>(&'a CaptureConfig);

impl std::fmt::Display for RequestSummary<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.0.width, self.0.height) {
            (Some(width), Some(height)) => write!(f, "{width}x{height}")?,
            (Some(width), None) => write!(f, "{width}x*")?,
            (None, Some(height)) => write!(f, "*x{height}")?,
            (None, None) => f.write_str("*x*")?,
        }
        match self.0.fps {
            Some(fps) => write!(f, "@{fps:.3}")?,
            None => f.write_str("@*")?,
        }
        write!(
            f,
            " prefer=[{}] compressed={} queue={} policy={}",
            self.0
                .encoding_preference()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            self.0.allow_compressed,
            self.0.effective_queue_depth(),
            self.0.drop_policy
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::config::DropPolicy;
    use crate::device::CaptureEncoding;
    use crate::mock::{self, MockScript};

    fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        predicate()
    }

    fn drain(stream: &mut CaptureStream) -> Result<Vec<CaptureFrame>, CaptureError> {
        let mut frames = Vec::new();
        while let Some(frame) = stream.recv()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    // ── Send + Sync ─────────────────────────────────────────────────────────

    #[test]
    fn session_types_are_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        const fn assert_send<T: Send>() {}
        assert_send_sync::<CaptureSession>();
        assert_send_sync::<CaptureStream>();
        assert_send_sync::<CaptureFrame>();
        assert_send_sync::<CaptureError>();
        assert_send_sync::<CaptureStats>();
        assert_send::<Box<dyn CaptureBackend>>();
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    #[test]
    fn thread_names_are_sanitized_and_bounded() {
        let name = thread_name("\\\\?\\usb#vid_046d&pid_0825#6&1e0d2e5c&0&2");
        assert!(name.starts_with(THREAD_NAME_PREFIX));
        assert!(name.len() <= THREAD_NAME_PREFIX.len() + MAX_THREAD_NAME_ID);
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
            "{name}"
        );
    }

    #[test]
    fn thread_names_survive_hostile_ids() {
        // An interior NUL would make `Builder::name` panic.
        let name = thread_name("bad\0id\u{1F4A9}");
        assert!(!name.contains('\0'), "{name}");
        assert!(std::thread::Builder::new()
            .name(name)
            .spawn(|| ())
            .is_ok_and(|handle| handle.join().is_ok()));
    }

    #[test]
    fn selector_default_takes_the_first_device() {
        let devices = mock::devices(&MockScript::default());
        let chosen = select_device(&devices, &DeviceSelector::Default).expect("a device");
        assert_eq!(chosen.id, devices[0].id);
    }

    #[test]
    fn selector_index_is_bounds_checked() {
        let devices = mock::devices(&MockScript::default());
        let error = select_device(&devices, &DeviceSelector::Index(99)).expect_err("out of range");
        assert!(matches!(error, CaptureError::DeviceNotFound(_)));
    }

    #[test]
    fn selector_id_must_match_exactly() {
        let devices = mock::devices(&MockScript::default());
        assert!(select_device(&devices, &DeviceSelector::Id(devices[0].id.clone())).is_ok());
        let error = select_device(&devices, &DeviceSelector::Id("nope".into()))
            .expect_err("no such device");
        assert!(matches!(error, CaptureError::DeviceNotFound(_)));
    }

    #[test]
    fn selector_on_an_empty_enumeration_is_not_found() {
        let error = select_device(&[], &DeviceSelector::Default).expect_err("nothing to pick");
        assert!(matches!(error, CaptureError::DeviceNotFound(_)));
    }

    #[test]
    fn request_summary_renders_an_unconstrained_request() {
        let text = RequestSummary(&CaptureConfig::default()).to_string();
        assert!(text.contains("*x*@*"), "{text}");
        assert!(text.contains("policy=drop-oldest"), "{text}");
    }

    // ── Native platform ─────────────────────────────────────────────────────

    /// On macOS the AVFoundation backend of package A5 is compiled in, so
    /// enumeration is expected to work. It still must not be *silent* about
    /// failure: a machine whose user has refused camera access says so.
    ///
    /// `open()` is deliberately not exercised here. It starts a real capture
    /// session, which on a machine that has never been asked would put a TCC
    /// permission dialog on screen in the middle of a test run.
    #[test]
    #[cfg(target_os = "macos")]
    fn native_backend_enumerates_through_avfoundation() {
        match crate::enumerate() {
            Ok(devices) => {
                for device in &devices {
                    assert_eq!(device.backend, BackendKind::AvFoundation);
                }
            }
            Err(CaptureError::PermissionDenied { .. }) => {}
            other => panic!("unexpected AVFoundation enumeration result: {other:?}"),
        }
        assert_eq!(crate::backend_kind(), BackendKind::AvFoundation);
    }

    /// The V4L2 backend of package A6 is compiled in on Linux, so enumeration
    /// is expected to work and to report every device it finds as this
    /// backend's. A machine whose `/dev/video*` nodes all belong to a group
    /// this user is not in says so, and a container with no `/dev` to walk
    /// reports the I/O failure; neither is "no backend". An empty list is the
    /// honest answer for a machine with no camera attached — which is what CI
    /// is — and is exactly what must not be confused with the unsupported case
    /// below.
    #[test]
    #[cfg(target_os = "linux")]
    fn native_backend_enumerates_through_v4l2() {
        match crate::enumerate() {
            Ok(devices) => {
                for device in &devices {
                    assert_eq!(device.backend, BackendKind::V4l2);
                }
            }
            Err(CaptureError::PermissionDenied { .. }) | Err(CaptureError::Io { .. }) => {}
            other => panic!("unexpected V4L2 enumeration result: {other:?}"),
        }
        assert_eq!(crate::backend_kind(), BackendKind::V4l2);
    }

    /// The Media Foundation backend of package A7 is compiled in on Windows.
    /// Listing devices is not gated by the camera privacy setting, but a
    /// refusal is still an honest outcome and still not "no backend".
    #[test]
    #[cfg(target_os = "windows")]
    fn native_backend_enumerates_through_media_foundation() {
        match crate::enumerate() {
            Ok(devices) => {
                for device in &devices {
                    assert_eq!(device.backend, BackendKind::MediaFoundation);
                }
            }
            Err(CaptureError::PermissionDenied { .. }) => {}
            other => panic!("unexpected Media Foundation enumeration result: {other:?}"),
        }
        assert_eq!(crate::backend_kind(), BackendKind::MediaFoundation);
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn native_backend_reports_unsupported_from_enumerate() {
        match crate::enumerate() {
            Err(CaptureError::UnsupportedPlatform { target }) => {
                assert_eq!(target, std::env::consts::OS);
            }
            Ok(devices) => panic!(
                "an unimplemented backend must not claim {} device(s) were found",
                devices.len()
            ),
            other => panic!("expected UnsupportedPlatform, got {other:?}"),
        }
        assert_eq!(crate::backend_kind(), BackendKind::Unsupported);
    }

    /// On a target with a backend, `open()` on a machine with no camera must
    /// fail — and fail as "nothing to open", not as "this platform cannot
    /// capture". The two are the same sentence to a caller who only checks
    /// `is_err()`, and opposite instructions to one reading the message: plug
    /// a camera in, versus file a bug.
    ///
    /// The check is skipped when a device really is attached, and on macOS
    /// entirely: opening for real is not this test's business, and on macOS it
    /// would raise a TCC dialog in the middle of a test run.
    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn native_open_without_a_camera_reports_nothing_to_open() {
        let Ok(devices) = crate::enumerate() else {
            return;
        };
        if !devices.is_empty() {
            return;
        }
        match crate::open(CaptureConfig::default()) {
            Err(CaptureError::DeviceNotFound(_)) => {}
            Ok(_) => panic!("open must not fabricate a session with no device attached"),
            Err(other) => panic!("expected DeviceNotFound, got {other:?}"),
        }
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn native_backend_reports_unsupported_from_open() {
        let result = crate::open(CaptureConfig::default());
        assert!(
            matches!(result, Err(CaptureError::UnsupportedPlatform { .. })),
            "open must not fabricate a session"
        );
    }

    // ── Session lifecycle ───────────────────────────────────────────────────

    #[test]
    fn scripted_end_yields_ok_none() {
        let script = MockScript::default()
            .with_frames(5)
            .with_cadence(Duration::ZERO);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(16), script).expect("mock opens");
        let mut stream = session.take_stream().expect("first take");
        let frames = drain(&mut stream).expect("no error");
        assert_eq!(frames.len(), 5);
        assert!(stream.is_ended());
        assert!(stream.recv().expect("still ended").is_none());
    }

    #[test]
    fn frames_carry_the_negotiated_format_and_a_sequence() {
        let script = MockScript::default()
            .with_frames(3)
            .with_size(32, 16)
            .with_cadence(Duration::ZERO);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(8), script).expect("mock opens");
        assert_eq!(session.negotiated_format().width, 32);
        assert_eq!(session.backend(), BackendKind::Mock);
        assert_eq!(session.device().backend, BackendKind::Mock);
        let mut stream = session.take_stream().expect("first take");
        let frames = drain(&mut stream).expect("no error");
        assert_eq!(
            frames.iter().map(|f| f.sequence).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        for frame in &frames {
            assert_eq!(frame.format, session.negotiated_format());
            assert_eq!(frame.width(), 32);
            assert!(frame.video().is_some(), "gray8 script yields raw frames");
        }
    }

    #[test]
    fn fail_at_propagates_once_and_then_the_stream_ends() {
        let script = MockScript::default()
            .with_frames(10)
            .with_fail_at(2)
            .with_cadence(Duration::ZERO);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(16), script).expect("mock opens");
        let mut stream = session.take_stream().expect("first take");

        let mut delivered = 0usize;
        let error = loop {
            match stream.recv() {
                Ok(Some(_)) => delivered += 1,
                Ok(None) => panic!("stream ended without surfacing the scripted failure"),
                Err(error) => break error,
            }
        };
        assert_eq!(delivered, 2, "frames before the failure still arrive");
        assert!(matches!(error, CaptureError::Platform(_)), "{error:?}");
        assert!(stream.recv().expect("ended cleanly").is_none());
        assert!(stream.is_ended());
        assert!(session.stats().last_error.is_some());
    }

    #[test]
    fn take_stream_hands_out_exactly_one_handle() {
        let script = MockScript::default()
            .with_frames(1)
            .with_cadence(Duration::ZERO);
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        assert!(session.take_stream().is_some());
        assert!(session.take_stream().is_none());
    }

    #[test]
    fn stop_is_idempotent_and_drop_joins_promptly() {
        // 10_000 frames at 20 ms would run for over three minutes; the sleep
        // has to be interruptible for this to finish at all.
        let script = MockScript::default()
            .with_frames(10_000)
            .with_cadence(Duration::from_millis(20));
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        assert!(session.is_running());
        let started = Instant::now();
        session.stop();
        session.stop();
        assert!(!session.is_running());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stop took {:?}",
            started.elapsed()
        );

        let script = MockScript::default()
            .with_frames(10_000)
            .with_cadence(Duration::from_millis(20));
        let session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        let started = Instant::now();
        drop(session);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "drop took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn recv_timeout_reports_a_timeout_and_the_end_differently() {
        let script = MockScript::default()
            .with_frames(1)
            .with_cadence(Duration::from_millis(400));
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        let mut stream = session.take_stream().expect("first take");

        assert!(stream.recv().expect("no error").is_some(), "first frame");
        // The runner is now sleeping out its cadence, so this times out.
        let timed_out = stream
            .recv_timeout(Duration::from_millis(20))
            .expect("timeout is not an error");
        assert!(timed_out.is_none());
        assert!(!stream.is_ended(), "a timeout is not the end of the stream");

        // Waiting long enough sees the real end.
        loop {
            match stream
                .recv_timeout(Duration::from_secs(5))
                .expect("no error")
            {
                Some(_) => continue,
                None => break,
            }
        }
        assert!(stream.is_ended());
    }

    #[test]
    fn try_recv_distinguishes_empty_from_ended() {
        let script = MockScript::default()
            .with_frames(1)
            .with_cadence(Duration::from_millis(300));
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        let mut stream = session.take_stream().expect("first take");
        // Whether a frame is already queued depends on scheduling, but the
        // stream is certainly not ended yet.
        assert!(!stream.is_ended());

        assert!(wait_until(|| {
            matches!(stream.try_recv(), Ok(None)) && stream.is_ended()
        }));
    }

    // ── Drop policy ─────────────────────────────────────────────────────────

    #[test]
    fn drop_oldest_keeps_producing_and_counts_every_eviction() {
        let script = MockScript::default()
            .with_frames(32)
            .with_cadence(Duration::ZERO);
        let session = mock::open(
            CaptureConfig::default()
                .with_queue_depth(1)
                .with_drop_policy(DropPolicy::DropOldest),
            script,
        )
        .expect("mock opens");
        // Nothing drains, so the producer evicts its way to the end.
        assert!(wait_until(|| session.stats().delivered == 32));
        let stats = session.stats();
        assert_eq!(stats.delivered, 32, "every frame reached the queue");
        assert_eq!(stats.dropped, 31, "all but the newest were evicted");
    }

    #[test]
    fn drop_newest_keeps_the_backlog_and_discards_arrivals() {
        let script = MockScript::default()
            .with_frames(32)
            .with_cadence(Duration::ZERO);
        let session = mock::open(
            CaptureConfig::default()
                .with_queue_depth(1)
                .with_drop_policy(DropPolicy::DropNewest),
            script,
        )
        .expect("mock opens");
        assert!(wait_until(|| {
            let stats = session.stats();
            stats.delivered + stats.dropped == 32
        }));
        let stats = session.stats();
        assert_eq!(stats.delivered, 1, "only the first frame fit");
        assert_eq!(stats.dropped, 31);
    }

    #[test]
    fn block_policy_loses_nothing_when_the_consumer_keeps_up() {
        let script = MockScript::default()
            .with_frames(24)
            .with_cadence(Duration::ZERO);
        let mut session = mock::open(
            CaptureConfig::default()
                .with_queue_depth(1)
                .with_drop_policy(DropPolicy::Block),
            script,
        )
        .expect("mock opens");
        let mut stream = session.take_stream().expect("first take");
        let frames = drain(&mut stream).expect("no error");
        assert_eq!(frames.len(), 24, "back-pressure, not loss");
        let stats = session.stats();
        assert_eq!(stats.delivered, 24);
        assert_eq!(stats.dropped, 0);
    }

    // ── Clocks ──────────────────────────────────────────────────────────────

    #[test]
    fn backwards_device_clocks_are_clamped_and_counted() {
        let script = MockScript::default()
            .with_frames(4)
            .with_cadence(Duration::ZERO)
            .with_device_timestamps([
                Duration::from_millis(0),
                Duration::from_millis(10),
                Duration::from_millis(5),
                Duration::from_millis(20),
            ]);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(8), script).expect("mock opens");
        let mut stream = session.take_stream().expect("first take");
        let frames = drain(&mut stream).expect("no error");

        let stamps: Vec<Duration> = frames.iter().map(|frame| frame.timestamp).collect();
        assert_eq!(
            stamps,
            vec![
                Duration::from_millis(0),
                Duration::from_millis(10),
                Duration::from_millis(10),
                Duration::from_millis(20),
            ]
        );
        assert!(stamps.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(session.stats().clock_regressions, 1);
    }

    #[test]
    fn sequence_gaps_are_reported_as_device_drops() {
        let script = MockScript::default()
            .with_frames(4)
            .with_cadence(Duration::ZERO)
            .with_sequence_step(3);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(8), script).expect("mock opens");
        let mut stream = session.take_stream().expect("first take");
        let frames = drain(&mut stream).expect("no error");
        assert_eq!(
            frames.iter().map(|f| f.sequence).collect::<Vec<_>>(),
            vec![0, 3, 6, 9]
        );
        // Three frames are missing between each delivered pair.
        assert_eq!(session.stats().device_dropped, 6);
    }

    // ── Negotiation wiring ──────────────────────────────────────────────────

    #[test]
    fn open_fills_the_device_id_into_a_negotiation_failure() {
        // The mock advertises exactly one format; make it compressed, then
        // forbid compression, so negotiation has nothing left to choose.
        let script = MockScript::default().with_encoding(CaptureEncoding::Mjpeg);
        let config = CaptureConfig::default().with_allow_compressed(false);
        let error = mock::open(config, script).expect_err("nothing acceptable");
        match error {
            CaptureError::NoMatchingFormat { device, available } => {
                assert_eq!(device, "mock:0", "the session names the device");
                assert_eq!(available, 1);
            }
            other => panic!("expected NoMatchingFormat, got {other:?}"),
        }
    }

    #[test]
    fn open_rejects_a_selector_that_matches_nothing() {
        let error = mock::open(
            CaptureConfig::default().with_device(DeviceSelector::Index(4)),
            MockScript::default(),
        )
        .expect_err("no such device");
        assert!(matches!(error, CaptureError::DeviceNotFound(_)));
    }

    // ── Stats ───────────────────────────────────────────────────────────────

    #[test]
    fn stats_start_at_zero_and_snapshot_independently() {
        let stats = StatsInner::default();
        assert_eq!(stats.snapshot(), CaptureStats::default());
        stats.record_delivered();
        let first = stats.snapshot();
        stats.record_dropped(4);
        assert_eq!(first.delivered, 1);
        assert_eq!(first.dropped, 0, "snapshots do not track later changes");
        assert_eq!(stats.snapshot().dropped, 4);
    }

    #[test]
    fn stats_record_the_most_recent_error() {
        let stats = StatsInner::default();
        stats.record_error("first");
        stats.record_error("second");
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.errors, 2);
        assert_eq!(snapshot.last_error.as_deref(), Some("second"));
    }
}

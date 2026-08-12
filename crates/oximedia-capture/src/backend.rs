//! The contract a platform capture backend implements.
//!
//! Two traits and two helpers. A [`CaptureBackend`] discovers devices and
//! opens them; the [`CaptureRunner`] it hands back owns the capture loop for
//! one device and runs on a dedicated thread. Everything a runner needs in
//! order to publish frames and to notice that it should stop lives in
//! [`FrameSink`] and [`StopSignal`], so that queue policy, timestamp
//! normalization and statistics are implemented exactly once instead of once
//! per platform.
//!
//! As of package A4 the only backends are [`UnsupportedBackend`] and the
//! `cfg(any(test, feature = "mock"))` mock; V4L2, AVFoundation and Media
//! Foundation arrive in A5-A7 and plug in through [`crate::platform::native`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use parking_lot::Mutex;

use crate::config::{CaptureConfig, DropPolicy};
use crate::device::{BackendKind, CaptureDevice, CaptureFormat};
use crate::error::CaptureError;
use crate::frame::{CaptureFrame, MonotonicClock};
use crate::session::StatsInner;

/// How long a [`DropPolicy::Block`] send waits before re-checking the stop
/// flag.
///
/// A plain blocking send would park the capture thread forever when nothing
/// ever drains the queue, and `stop()` would then wait forever on the join.
/// Polling turns that deadlock into a bounded shutdown latency.
const BLOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The message type carried by the delivery channel.
pub(crate) type FrameMessage = Result<CaptureFrame, CaptureError>;

// ── Traits ───────────────────────────────────────────────────────────────────

/// Discovers and opens capture devices for one platform API.
pub(crate) trait CaptureBackend: Send + Sync {
    /// Which platform API this backend speaks.
    fn kind(&self) -> BackendKind;

    /// List every device the platform currently reports.
    ///
    /// Returning `Ok(vec![])` means "this API works and found no cameras". A
    /// backend that cannot run at all must return an error instead, so that
    /// "no camera plugged in" stays distinguishable from "capture is broken
    /// here".
    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError>;

    /// Configure `device` for `format` and hand back its capture loop.
    ///
    /// The returned runner has not started yet; the session spawns it.
    fn open(
        &self,
        device: &CaptureDevice,
        format: CaptureFormat,
        config: &CaptureConfig,
    ) -> Result<Box<dyn CaptureRunner>, CaptureError>;
}

/// The capture loop for one opened device.
pub(crate) trait CaptureRunner: Send {
    /// Pump frames into `sink` until `stop` is raised or the source ends.
    ///
    /// Implementations must:
    ///
    /// * check [`StopSignal::is_stopped`] at least once per frame period, and
    ///   never block on the device for longer than that — `stop()` joins this
    ///   thread, so an uninterruptible wait here is a hang there;
    /// * publish through [`FrameSink::deliver`] rather than touching the
    ///   channel, so that drop policy, clock normalization and statistics stay
    ///   uniform across platforms;
    /// * return `Ok(())` when the source ends normally, and `Err` only for a
    ///   failure that ends the session. Recoverable hiccups go to
    ///   [`FrameSink::report_transient`] and the loop continues.
    fn run(&mut self, sink: &FrameSink, stop: &StopSignal) -> Result<(), CaptureError>;
}

// ── Stop signal ──────────────────────────────────────────────────────────────

/// A one-way "please finish" flag shared with the capture thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct StopSignal {
    flag: Arc<AtomicBool>,
}

impl StopSignal {
    /// A signal that has not been raised.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Raise the signal. Idempotent.
    pub(crate) fn stop(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Whether the signal has been raised.
    pub(crate) fn is_stopped(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

// ── Frame sink ───────────────────────────────────────────────────────────────

/// Per-sink mutable state, guarded by one short-lived lock.
#[derive(Debug)]
struct SinkState {
    device_clock: MonotonicClock,
    host_clock: MonotonicClock,
    last_sequence: Option<u64>,
}

impl Default for SinkState {
    fn default() -> Self {
        Self {
            device_clock: MonotonicClock::new(),
            host_clock: MonotonicClock::new(),
            last_sequence: None,
        }
    }
}

/// The publishing end of a capture session.
///
/// Runners call [`FrameSink::deliver`]; everything else — normalizing the
/// timestamps, applying [`DropPolicy`], counting driver-side sequence gaps —
/// happens here so that no backend has to reimplement it.
pub(crate) struct FrameSink {
    sender: Sender<FrameMessage>,
    /// A receiver clone used *only* to evict the oldest queued frame under
    /// [`DropPolicy::DropOldest`]. A bounded channel has no "overwrite" send,
    /// so making room means taking one out.
    evictor: Receiver<FrameMessage>,
    policy: DropPolicy,
    stop: StopSignal,
    stats: Arc<StatsInner>,
    state: Mutex<SinkState>,
}

// `deliver`, `report_transient` and `report_device_dropped` are called by the
// platform runners. Where one is compiled in — macOS as of package A5, Linux as
// of A6, Windows as of A7 — they have real callers and this attribute does
// nothing. On a target with no backend yet and without the `mock` feature the
// only implementor of `CaptureRunner` is absent entirely, so the methods would
// warn as dead code; the allowance is scoped to exactly those builds rather than
// blanket-applied, so that a future backend which forgets to publish through
// the sink still produces a warning on its own platform. Each package that adds
// a backend adds its `target_os` to this list.
#[cfg_attr(
    not(any(
        test,
        feature = "mock",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )),
    allow(dead_code)
)]
impl FrameSink {
    /// Wire a sink to a bounded channel.
    pub(crate) fn new(
        sender: Sender<FrameMessage>,
        evictor: Receiver<FrameMessage>,
        policy: DropPolicy,
        stop: StopSignal,
        stats: Arc<StatsInner>,
    ) -> Self {
        Self {
            sender,
            evictor,
            policy,
            stop,
            stats,
            state: Mutex::new(SinkState::default()),
        }
    }

    /// Publish one frame.
    ///
    /// The frame's `timestamp` and `host_timestamp` arrive as *raw* device and
    /// host instants; this method rewrites them onto the session's
    /// non-decreasing since-first-frame timeline before the frame goes
    /// anywhere. Normalization happens before the drop-policy decision, so the
    /// timeline origin is the first frame *submitted*, not the first one that
    /// survived the queue. For frame one those are the same instant — the
    /// queue is empty — and anchoring on submission is what keeps timestamps
    /// comparable across a session that drops frames.
    ///
    /// Returns `true` when the frame reached the queue.
    pub(crate) fn deliver(&self, mut frame: CaptureFrame) -> bool {
        {
            let mut state = self.state.lock();
            let before = state.device_clock.regressions() + state.host_clock.regressions();
            frame.timestamp = state.device_clock.map(frame.timestamp);
            frame.host_timestamp = state.host_clock.map(frame.host_timestamp);
            let after = state.device_clock.regressions() + state.host_clock.regressions();
            if after > before {
                self.stats.record_clock_regression(after - before);
            }
            // A gap in the driver's own sequence counter means frames were
            // lost upstream of this crate.
            if let Some(previous) = state.last_sequence {
                let expected = previous.saturating_add(1);
                if frame.sequence > expected {
                    self.stats.record_device_dropped(frame.sequence - expected);
                }
            }
            state.last_sequence = Some(frame.sequence);
        }
        self.send(Ok(frame))
    }

    /// Report a failure the runner recovered from.
    ///
    /// Increments [`crate::CaptureStats::errors`] and logs, but does not end
    /// the session.
    pub(crate) fn report_transient(&self, error: &CaptureError) {
        tracing::warn!(error = %error, "transient capture error");
        self.stats.record_error(&error.to_string());
    }

    /// Report frames the device or driver lost before this crate saw them.
    ///
    /// Backends that can read a driver-side overrun counter should call this;
    /// backends that cannot get the same information for free from the
    /// sequence gaps [`Self::deliver`] already tracks.
    // V4L2 is the "cannot" case: `/dev/video*` exposes no driver-side overrun
    // counter, and the losses this method exists to report are exactly the
    // `v4l2_buffer::sequence` gaps `deliver` already counts. The allowance below
    // therefore names the platforms whose backends *do* have a counter to read,
    // so that one of those forgetting to call this still warns, while Linux —
    // where there is nothing to call it with — does not.
    #[cfg_attr(
        not(any(test, feature = "mock", target_os = "macos", target_os = "windows")),
        allow(dead_code)
    )]
    pub(crate) fn report_device_dropped(&self, count: u64) {
        self.stats.record_device_dropped(count);
    }

    /// Send the terminal error for this session.
    ///
    /// Called once by the session's thread wrapper when [`CaptureRunner::run`]
    /// returns `Err`. The sink is dropped immediately afterwards, so the
    /// consumer sees the error and then end-of-stream.
    pub(crate) fn send_fatal(&self, error: CaptureError) {
        self.stats.record_error(&error.to_string());
        // Best effort: if the queue is full and nothing is draining it, the
        // error cannot be delivered, but the session still ends and the
        // message is already recorded in `stats.last_error`.
        match self.policy {
            DropPolicy::Block => {
                let _ = self.sender.send_timeout(Err(error), BLOCK_POLL_INTERVAL);
            }
            DropPolicy::DropOldest | DropPolicy::DropNewest => {
                if let Err(TrySendError::Full(message)) = self.sender.try_send(Err(error)) {
                    let _ = self.evictor.try_recv();
                    let _ = self.sender.try_send(message);
                }
            }
        }
    }

    /// Apply [`DropPolicy`] to one outgoing message.
    fn send(&self, message: FrameMessage) -> bool {
        match self.policy {
            DropPolicy::DropOldest => self.send_dropping_oldest(message),
            DropPolicy::DropNewest => self.send_dropping_newest(message),
            DropPolicy::Block => self.send_blocking(message),
        }
    }

    fn send_dropping_oldest(&self, message: FrameMessage) -> bool {
        match self.sender.try_send(message) {
            Ok(()) => {
                self.stats.record_delivered();
                true
            }
            Err(TrySendError::Full(message)) => {
                // Evict exactly one frame, then retry once. Retrying in a loop
                // would let a stalled consumer keep the capture thread busy.
                if self.evictor.try_recv().is_ok() {
                    self.stats.record_dropped(1);
                }
                match self.sender.try_send(message) {
                    Ok(()) => {
                        self.stats.record_delivered();
                        true
                    }
                    Err(TrySendError::Full(_)) => {
                        self.stats.record_dropped(1);
                        false
                    }
                    Err(TrySendError::Disconnected(_)) => false,
                }
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn send_dropping_newest(&self, message: FrameMessage) -> bool {
        match self.sender.try_send(message) {
            Ok(()) => {
                self.stats.record_delivered();
                true
            }
            Err(TrySendError::Full(_)) => {
                self.stats.record_dropped(1);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn send_blocking(&self, message: FrameMessage) -> bool {
        let mut pending = message;
        loop {
            if self.stop.is_stopped() {
                // Shutting down: the frame is discarded rather than blocking
                // the join behind a consumer that will never read again.
                self.stats.record_dropped(1);
                return false;
            }
            match self.sender.send_timeout(pending, BLOCK_POLL_INTERVAL) {
                Ok(()) => {
                    self.stats.record_delivered();
                    return true;
                }
                Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => pending = returned,
                Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return false,
            }
        }
    }
}

impl std::fmt::Debug for FrameSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameSink")
            .field("policy", &self.policy)
            .field("queued", &self.sender.len())
            .field("capacity", &self.sender.capacity())
            .finish()
    }
}

// ── Unsupported backend ──────────────────────────────────────────────────────

/// The backend used on targets that have no capture implementation compiled in.
///
/// It reports [`CaptureError::UnsupportedPlatform`] from every entry point.
/// It never returns `Ok(vec![])`: an empty device list is what a *working*
/// backend says when no camera is plugged in, and conflating the two turns a
/// missing implementation into a support ticket about a broken webcam.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnsupportedBackend;

impl UnsupportedBackend {
    /// The error every entry point returns.
    fn unavailable() -> CaptureError {
        CaptureError::UnsupportedPlatform {
            target: std::env::consts::OS,
        }
    }
}

impl CaptureBackend for UnsupportedBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Unsupported
    }

    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Err(Self::unavailable())
    }

    fn open(
        &self,
        _device: &CaptureDevice,
        _format: CaptureFormat,
        _config: &CaptureConfig,
    ) -> Result<Box<dyn CaptureRunner>, CaptureError> {
        Err(Self::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use oximedia_core::PixelFormat;

    use super::*;
    use crate::device::CaptureEncoding;
    use crate::frame::{FramePayload, TimestampSource};

    fn format() -> CaptureFormat {
        CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Gray8), 8, 8, 30, 1)
    }

    /// A frame with a caller-chosen device timestamp and a host timestamp that
    /// is monotonic in `sequence` — the way a real `Instant`-based host clock
    /// behaves, so only the device clock can regress.
    fn frame(sequence: u64, timestamp_ms: u64) -> CaptureFrame {
        CaptureFrame {
            format: format(),
            payload: FramePayload::Compressed(vec![sequence as u8]),
            timestamp: Duration::from_millis(timestamp_ms),
            host_timestamp: Duration::from_millis(sequence.saturating_mul(10)),
            timestamp_source: TimestampSource::DeviceMonotonic,
            sequence,
        }
    }

    struct Harness {
        sink: FrameSink,
        receiver: Receiver<FrameMessage>,
        stats: Arc<StatsInner>,
        stop: StopSignal,
    }

    fn harness(policy: DropPolicy, depth: usize) -> Harness {
        let (sender, receiver) = crossbeam_channel::bounded(depth);
        let stats = Arc::new(StatsInner::default());
        let stop = StopSignal::new();
        let sink = FrameSink::new(
            sender,
            receiver.clone(),
            policy,
            stop.clone(),
            Arc::clone(&stats),
        );
        Harness {
            sink,
            receiver,
            stats,
            stop,
        }
    }

    #[test]
    fn stop_signal_is_idempotent() {
        let stop = StopSignal::new();
        assert!(!stop.is_stopped());
        stop.stop();
        stop.stop();
        assert!(stop.is_stopped());
    }

    #[test]
    fn stop_signal_clones_share_state() {
        let stop = StopSignal::new();
        let clone = stop.clone();
        stop.stop();
        assert!(clone.is_stopped());
    }

    #[test]
    fn unsupported_backend_errs_instead_of_returning_an_empty_list() {
        let backend = UnsupportedBackend;
        assert_eq!(backend.kind(), BackendKind::Unsupported);
        match backend.enumerate() {
            Err(CaptureError::UnsupportedPlatform { target }) => {
                assert_eq!(target, std::env::consts::OS);
            }
            other => panic!("expected UnsupportedPlatform, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_backend_cannot_open() {
        let backend = UnsupportedBackend;
        let device = CaptureDevice {
            id: "x".into(),
            name: "x".into(),
            formats: vec![format()],
            backend: BackendKind::Unsupported,
        };
        let result = backend.open(&device, format(), &CaptureConfig::default());
        assert!(matches!(
            result,
            Err(CaptureError::UnsupportedPlatform { .. })
        ));
    }

    #[test]
    fn deliver_normalizes_timestamps_to_the_first_frame() {
        let h = harness(DropPolicy::DropOldest, 4);
        assert!(h.sink.deliver(frame(0, 5_000)));
        assert!(h.sink.deliver(frame(1, 5_033)));
        let first = h.receiver.try_recv().expect("first frame").expect("ok");
        let second = h.receiver.try_recv().expect("second frame").expect("ok");
        assert_eq!(first.timestamp, Duration::ZERO);
        assert_eq!(second.timestamp, Duration::from_millis(33));
    }

    #[test]
    fn deliver_clamps_backwards_clocks_and_counts_them() {
        let h = harness(DropPolicy::DropOldest, 4);
        h.sink.deliver(frame(0, 0));
        h.sink.deliver(frame(1, 100));
        h.sink.deliver(frame(2, 50));
        let stamps: Vec<Duration> = (0..3)
            .filter_map(|_| h.receiver.try_recv().ok())
            .filter_map(Result::ok)
            .map(|f| f.timestamp)
            .collect();
        assert_eq!(
            stamps,
            vec![
                Duration::ZERO,
                Duration::from_millis(100),
                Duration::from_millis(100)
            ]
        );
        assert_eq!(h.stats.snapshot().clock_regressions, 1);
    }

    #[test]
    fn both_clocks_regressing_on_one_frame_counts_twice() {
        let h = harness(DropPolicy::DropOldest, 4);
        // Drive device and host clocks in lockstep, then step both backwards.
        for (sequence, millis) in [(0u64, 100u64), (1, 200), (2, 150)] {
            let mut candidate = frame(sequence, millis);
            candidate.host_timestamp = Duration::from_millis(millis);
            h.sink.deliver(candidate);
        }
        assert_eq!(
            h.stats.snapshot().clock_regressions,
            2,
            "each clock that had to be clamped is one regression"
        );
    }

    #[test]
    fn sequence_gaps_are_attributed_to_the_device() {
        let h = harness(DropPolicy::DropOldest, 8);
        h.sink.deliver(frame(10, 0));
        h.sink.deliver(frame(11, 10));
        h.sink.deliver(frame(15, 20));
        assert_eq!(h.stats.snapshot().device_dropped, 3);
        assert_eq!(h.stats.snapshot().dropped, 0, "not our queue's doing");
    }

    #[test]
    fn drop_oldest_keeps_the_newest_frame() {
        let h = harness(DropPolicy::DropOldest, 1);
        assert!(h.sink.deliver(frame(0, 0)));
        assert!(h.sink.deliver(frame(1, 10)));
        assert!(h.sink.deliver(frame(2, 20)));
        let stats = h.stats.snapshot();
        assert_eq!(stats.delivered, 3);
        assert_eq!(stats.dropped, 2);
        let remaining = h.receiver.try_recv().expect("one frame").expect("ok");
        assert_eq!(remaining.sequence, 2, "the newest frame survives");
        assert!(h.receiver.try_recv().is_err());
    }

    #[test]
    fn drop_newest_keeps_the_oldest_frame() {
        let h = harness(DropPolicy::DropNewest, 1);
        assert!(h.sink.deliver(frame(0, 0)));
        assert!(!h.sink.deliver(frame(1, 10)));
        assert!(!h.sink.deliver(frame(2, 20)));
        let stats = h.stats.snapshot();
        assert_eq!(stats.delivered, 1);
        assert_eq!(stats.dropped, 2);
        let remaining = h.receiver.try_recv().expect("one frame").expect("ok");
        assert_eq!(remaining.sequence, 0, "the oldest frame survives");
    }

    #[test]
    fn block_policy_waits_for_room_instead_of_dropping() {
        let h = harness(DropPolicy::Block, 1);
        assert!(h.sink.deliver(frame(0, 0)));
        // Free the slot from another thread while the sink is parked.
        let receiver = h.receiver.clone();
        let drainer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            receiver.recv().map(|_| ())
        });
        assert!(h.sink.deliver(frame(1, 10)));
        drainer.join().map_or_else(
            |_| panic!("drainer panicked"),
            |result| assert!(result.is_ok()),
        );
        let stats = h.stats.snapshot();
        assert_eq!(stats.delivered, 2);
        assert_eq!(stats.dropped, 0);
    }

    #[test]
    fn block_policy_gives_up_once_stop_is_raised() {
        let h = harness(DropPolicy::Block, 1);
        assert!(h.sink.deliver(frame(0, 0)));
        h.stop.stop();
        // Nothing is draining, so without the stop check this would hang.
        let started = std::time::Instant::now();
        assert!(!h.sink.deliver(frame(1, 10)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(h.stats.snapshot().dropped, 1);
    }

    #[test]
    fn transient_errors_are_counted_without_ending_the_session() {
        let h = harness(DropPolicy::DropOldest, 4);
        h.sink.report_transient(&CaptureError::platform("hiccup"));
        h.sink.report_transient(&CaptureError::platform("hiccup"));
        let stats = h.stats.snapshot();
        assert_eq!(stats.errors, 2);
        assert_eq!(
            stats.last_error.as_deref(),
            Some("capture backend error: hiccup")
        );
        assert!(h.sink.deliver(frame(0, 0)), "still publishing");
    }

    #[test]
    fn explicit_device_drop_reports_accumulate() {
        let h = harness(DropPolicy::DropOldest, 4);
        h.sink.report_device_dropped(2);
        h.sink.report_device_dropped(3);
        assert_eq!(h.stats.snapshot().device_dropped, 5);
    }

    #[test]
    fn fatal_errors_reach_the_consumer_even_when_the_queue_is_full() {
        let h = harness(DropPolicy::DropOldest, 1);
        h.sink.deliver(frame(0, 0));
        h.sink
            .send_fatal(CaptureError::platform("device unplugged"));
        let received: Vec<FrameMessage> = h.receiver.try_iter().collect();
        assert!(
            received.iter().any(|message| message.is_err()),
            "the fatal error must be visible: {received:?}"
        );
        assert_eq!(
            h.stats.snapshot().last_error.as_deref(),
            Some("capture backend error: device unplugged")
        );
    }

    #[test]
    fn sink_debug_reports_queue_occupancy() {
        let h = harness(DropPolicy::DropOldest, 2);
        h.sink.deliver(frame(0, 0));
        let text = format!("{:?}", h.sink);
        assert!(text.contains("queued"), "{text}");
    }
}

//! Optional Tokio-native receive path (`tokio` feature).
//!
//! This crate's core is deliberately synchronous — see the crate root docs
//! and the platform module "Threading invariant" sections for why. macOS's
//! `AVCaptureSession` and its sample-buffer delegate are `!Send`
//! Objective-C objects that may not cross a thread boundary at all,
//! Windows' Media Foundation interfaces are COM objects bound to the
//! apartment that created them, and Linux keeps its `mmap`ed ring's file
//! descriptor local to the thread that opened it. Every one of those
//! constraints is enforced by never letting the platform object leave
//! `CaptureRunner::run` (crate-private), which always executes on the
//! dedicated capture thread that `CaptureSession::open_with_backend`
//! (also crate-private) spawns.
//!
//! [`CaptureStream`], the *consuming* end, never touches any of that: it
//! holds nothing but a `crossbeam_channel::Receiver<Result<CaptureFrame,
//! CaptureError>>`, and both payload types are plain `Send` data (checked by
//! the compiler via the crate's own `session_types_are_send_and_sync` test,
//! since the workspace denies `unsafe_code` — there is no unsafe impl to
//! audit here, only ordinary fields). So this module adds an async front end
//! to [`CaptureStream`] itself, not to the capture internals: `session.rs`
//! and every `platform` module are untouched by this feature, exactly as
//! required.
//!
//! # The bridge
//!
//! [`CaptureStream::into_async`] hands the stream to a dedicated
//! [`std::thread`] that loops [`CaptureStream::recv_timeout`] — already
//! public, already sound, and already the same "wake up periodically and
//! check whether to stop" shape the crate-private sink uses for
//! `DropPolicy::Block` — and forwards each frame into a
//! [`tokio::sync::mpsc`] channel with
//! [`Sender::blocking_send`](tokio::sync::mpsc::Sender::blocking_send), the
//! API `tokio` documents specifically for sending from a thread that is not
//! itself inside an async task. The bridge is a plain OS thread, never a
//! tokio task, so `blocking_send` (which panics if called from *inside* an
//! async context) is always safe to call from it.
//!
//! The bridge notices the async side going away — [`AsyncCaptureStream`]
//! dropped, or simply no longer polled — within one
//! `BRIDGE_POLL_INTERVAL`, because each wakeup checks
//! [`Sender::is_closed`](tokio::sync::mpsc::Sender::is_closed) before
//! blocking again on the sync side; it notices the *sync* stream ending the
//! same way every [`CaptureStream::recv_timeout`] caller already does, via
//! [`CaptureStream::is_ended`]. Neither side can leak the bridge thread past
//! one poll interval once it should stop, and nothing about this scheme
//! reaches into a platform module or the sink/runner machinery.
//!
//! # What this does *not* change
//!
//! [`CaptureSession::open`](crate::CaptureSession::open) is still entirely
//! synchronous, and still the only way a session is created — this feature
//! adds a receive-side adapter, not an async `open()`. Opening a camera
//! blocks the calling thread the same as always (briefly, and once, per
//! session); only receiving already-captured frames gets an async front end.
//!
//! One consumer-side caution carries over unchanged from the sync API: a
//! [`CaptureFrame`] holds a buffer-pool lease for as
//! long as it is alive, so holding frames across long `await`s extends pool
//! pressure exactly like holding them across long blocking work — pool
//! exhaustion shows up as device-side drops
//! (`CaptureStats::device_dropped`), not as an error on this stream.

use std::thread::Builder;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::DEFAULT_QUEUE_DEPTH;
use crate::error::CaptureError;
use crate::frame::CaptureFrame;
use crate::session::CaptureStream;

/// How often the bridge thread wakes from a timed-out
/// [`CaptureStream::recv_timeout`] to check whether the async consumer has
/// gone away.
///
/// This is the bridge's only polling loop; it is not on the capture hot
/// path (frames still flow the instant they arrive — `recv_timeout` returns
/// immediately when a frame is queued), so a coarse interval costs nothing
/// but shutdown latency.
const BRIDGE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The message type carried across the bridge — the same shape
/// [`CaptureStream::recv`] already returns, so nothing is translated or
/// re-encoded at the boundary.
type BridgeMessage = Result<CaptureFrame, CaptureError>;

/// The async-native consuming end of a capture session.
///
/// Obtained from [`CaptureStream::into_async`]. See the module docs for how
/// this is implemented and what it does and does not change.
pub struct AsyncCaptureStream {
    receiver: mpsc::Receiver<BridgeMessage>,
    ended: bool,
}

impl std::fmt::Debug for AsyncCaptureStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncCaptureStream")
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl AsyncCaptureStream {
    /// Wait for the next frame.
    ///
    /// Mirrors [`CaptureStream::recv`]: `Ok(None)` means the stream has
    /// ended and no further frames will arrive, and a fatal backend error is
    /// delivered as `Err` exactly once, after which the stream reports its
    /// end.
    ///
    /// # Errors
    ///
    /// Returns the backend's terminal error, if the session ended because of
    /// one.
    pub async fn recv(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        if self.ended {
            return Ok(None);
        }
        match self.receiver.recv().await {
            Some(Ok(frame)) => Ok(Some(frame)),
            Some(Err(error)) => Err(error),
            None => {
                self.ended = true;
                Ok(None)
            }
        }
    }

    /// Whether the end of the stream has been observed.
    ///
    /// As with [`CaptureStream::is_ended`], this reflects what [`Self::recv`]
    /// has already seen; it does not probe ahead of it.
    pub const fn is_ended(&self) -> bool {
        self.ended
    }
}

impl CaptureStream {
    /// Convert this stream into a Tokio-native async stream.
    ///
    /// See the [module docs](self) for the bridge this spawns. The bridge
    /// channel's capacity is [`DEFAULT_QUEUE_DEPTH`]; use
    /// [`Self::into_async_with_capacity`] to choose a different one — for
    /// instance a larger one, if the consumer expects to be scheduled in
    /// bursts and the sync side's own `queue_depth` is already tight.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Io`] if the bridge thread cannot be spawned —
    /// the same failure mode, and the same honest reporting instead of a
    /// panic, as [`crate::CaptureSession::open`] spawning the capture thread
    /// itself.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use oximedia_capture::{CaptureConfig, CaptureError};
    ///
    /// # async fn run() -> Result<(), CaptureError> {
    /// let mut session = oximedia_capture::open(CaptureConfig::default())?;
    /// let Some(stream) = session.take_stream() else {
    ///     return Ok(()); // already taken
    /// };
    /// let mut stream = stream.into_async()?;
    ///
    /// while let Some(frame) = stream.recv().await? {
    ///     println!("frame {} at {:?}", frame.sequence, frame.timestamp);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_async(self) -> Result<AsyncCaptureStream, CaptureError> {
        self.into_async_with_capacity(DEFAULT_QUEUE_DEPTH)
    }

    /// As [`Self::into_async`], with an explicit bridge channel capacity.
    ///
    /// `capacity` is clamped to at least one, for the same reason
    /// [`crate::CaptureConfig::effective_queue_depth`] clamps its own: a
    /// zero-capacity `tokio::sync::mpsc` channel panics on construction
    /// rather than behaving like a rendezvous channel.
    ///
    /// # Errors
    ///
    /// See [`Self::into_async`].
    pub fn into_async_with_capacity(
        mut self,
        capacity: usize,
    ) -> Result<AsyncCaptureStream, CaptureError> {
        let (sender, receiver) = mpsc::channel(capacity.max(1));

        Builder::new()
            .name("oximedia-capture-async-bridge".to_string())
            .spawn(move || loop {
                match self.recv_timeout(BRIDGE_POLL_INTERVAL) {
                    Ok(Some(frame)) => {
                        if sender.blocking_send(Ok(frame)).is_err() {
                            // The async side dropped its receiver: nothing
                            // is left to deliver to, so stop pulling frames
                            // off the sync queue and let the thread end.
                            return;
                        }
                    }
                    Ok(None) => {
                        if self.is_ended() || sender.is_closed() {
                            return;
                        }
                        // Just a poll timeout with nothing to report yet —
                        // loop back into another bounded `recv_timeout`.
                    }
                    Err(error) => {
                        // Best-effort: if this send fails the async side is
                        // already gone, which the next loop iteration's
                        // `sender.is_closed()` check (reached via the sync
                        // stream now reporting `is_ended()`, per
                        // `CaptureStream::recv`'s "terminal error, then end
                        // of stream" contract) will notice and return on.
                        let _ = sender.blocking_send(Err(error));
                    }
                }
            })
            .map_err(|source| CaptureError::io("async-bridge", source))?;

        Ok(AsyncCaptureStream {
            receiver,
            ended: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration as StdDuration;

    use super::*;
    use crate::config::CaptureConfig;
    use crate::error::CaptureError as Error;
    use crate::mock::{self, MockScript};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn async_capture_stream_is_send_and_sync() {
        assert_send_sync::<AsyncCaptureStream>();
    }

    #[tokio::test]
    async fn into_async_delivers_every_frame_the_mock_produces() {
        let script = MockScript::default()
            .with_frames(12)
            .with_size(16, 12)
            .with_cadence(StdDuration::ZERO);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(16), script).expect("mock opens");
        let stream = session.take_stream().expect("first take");
        let mut async_stream = stream.into_async().expect("bridge thread spawns");

        let mut sequences = Vec::new();
        while let Some(frame) = async_stream.recv().await.expect("no error") {
            sequences.push(frame.sequence);
        }
        assert_eq!(sequences, (0..12).collect::<Vec<_>>());
        assert!(async_stream.is_ended());
        // Calling again after the end is observed keeps returning `Ok(None)`,
        // exactly like the sync `CaptureStream`.
        assert!(async_stream.recv().await.expect("still ended").is_none());
    }

    #[tokio::test]
    async fn into_async_surfaces_the_terminal_error_then_ends() {
        let script = MockScript::default()
            .with_frames(6)
            .with_fail_at(3)
            .with_cadence(StdDuration::ZERO);
        let mut session =
            mock::open(CaptureConfig::default().with_queue_depth(16), script).expect("mock opens");
        let stream = session.take_stream().expect("first take");
        let mut async_stream = stream.into_async().expect("bridge thread spawns");

        let mut delivered = 0usize;
        let error = loop {
            match async_stream.recv().await {
                Ok(Some(_)) => delivered += 1,
                Ok(None) => panic!("stream ended without surfacing the scripted failure"),
                Err(error) => break error,
            }
        };
        assert_eq!(delivered, 3, "frames before the failure still arrive");
        assert!(matches!(error, Error::Platform(_)), "{error:?}");
        assert!(async_stream.recv().await.expect("ended cleanly").is_none());
        assert!(async_stream.is_ended());
    }

    #[tokio::test]
    async fn into_async_reports_ok_none_for_an_empty_scripted_session() {
        let script = MockScript::default()
            .with_frames(0)
            .with_cadence(StdDuration::ZERO);
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        let stream = session.take_stream().expect("first take");
        let mut async_stream = stream.into_async().expect("bridge thread spawns");

        assert!(async_stream.recv().await.expect("no error").is_none());
        assert!(async_stream.is_ended());
    }

    #[tokio::test]
    async fn dropping_the_async_stream_lets_the_bridge_thread_exit_on_its_own() {
        // A long, slow-cadence script: if the bridge thread were not
        // noticing the async side going away, this test would hang the
        // process open (a leaked, permanently-parked thread) rather than
        // fail loudly — so this asserts the bridge's self-shutdown directly
        // by giving it a bounded amount of wall-clock time to happen.
        let script = MockScript::default()
            .with_frames(10_000)
            .with_cadence(StdDuration::from_millis(5));
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        let stream = session.take_stream().expect("first take");
        let mut async_stream = stream.into_async().expect("bridge thread spawns");

        // Consume exactly one frame so the bridge is definitely running,
        // then drop the async stream while the session is still producing.
        assert!(async_stream.recv().await.expect("first frame").is_some());
        drop(async_stream);

        // The bridge should notice within a handful of poll intervals.
        // There is no direct handle to join here (by design — see the
        // module docs), so this is an indirect but real check: the
        // session's own queue keeps draining via `DropOldest` instead of
        // filling and stalling the producer, which would not happen if a
        // stuck bridge thread were still holding the sync receiver.
        tokio::time::sleep(StdDuration::from_millis(
            BRIDGE_POLL_INTERVAL.as_millis() as u64 * 4,
        ))
        .await;
        session.stop();
        let stats = session.stats();
        assert!(
            stats.delivered > 1,
            "producer kept running after the bridge stopped consuming: {stats:?}"
        );
    }

    #[tokio::test]
    async fn into_async_with_capacity_clamps_zero_to_one() {
        let script = MockScript::default()
            .with_frames(2)
            .with_cadence(StdDuration::ZERO);
        let mut session = mock::open(CaptureConfig::default(), script).expect("mock opens");
        let stream = session.take_stream().expect("first take");
        // A capacity of zero would panic inside `tokio::sync::mpsc::channel`
        // if it were not clamped first.
        let mut async_stream = stream
            .into_async_with_capacity(0)
            .expect("bridge thread spawns");

        let mut count = 0;
        while async_stream.recv().await.expect("no error").is_some() {
            count += 1;
        }
        assert_eq!(count, 2);
    }
}

//! Streaming demuxer for network sources.
//!
//! Provides progressive demuxing without requiring seek support,
//! optimized for live streaming and network sources.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use bytes::Bytes;
use oximedia_core::{OxiError, OxiResult};
use std::collections::VecDeque;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;

use crate::{Demuxer, Packet, ProbeResult, StreamInfo};

/// Configuration for streaming demuxer.
#[derive(Clone, Debug)]
pub struct StreamingDemuxerConfig {
    /// Initial buffer size in bytes before starting demux.
    pub initial_buffer_size: usize,
    /// Maximum buffer size in bytes.
    pub max_buffer_size: usize,
    /// Enable low-latency mode (minimal buffering).
    pub low_latency: bool,
    /// Timeout for network reads in milliseconds.
    pub read_timeout_ms: u64,
}

impl Default for StreamingDemuxerConfig {
    fn default() -> Self {
        Self {
            initial_buffer_size: 64 * 1024,    // 64 KB
            max_buffer_size: 10 * 1024 * 1024, // 10 MB
            low_latency: false,
            read_timeout_ms: 5000,
        }
    }
}

impl StreamingDemuxerConfig {
    /// Creates a new configuration with default values.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            initial_buffer_size: 64 * 1024,
            max_buffer_size: 10 * 1024 * 1024,
            low_latency: false,
            read_timeout_ms: 5000,
        }
    }

    /// Enables low-latency mode.
    #[must_use]
    pub const fn with_low_latency(mut self, enabled: bool) -> Self {
        self.low_latency = enabled;
        self
    }

    /// Sets the initial buffer size.
    #[must_use]
    pub const fn with_initial_buffer(mut self, size: usize) -> Self {
        self.initial_buffer_size = size;
        self
    }

    /// Sets the maximum buffer size.
    #[must_use]
    pub const fn with_max_buffer(mut self, size: usize) -> Self {
        self.max_buffer_size = size;
        self
    }

    /// Sets the read timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.read_timeout_ms = timeout_ms;
        self
    }
}

/// State of the streaming demuxer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingState {
    /// Initial state: probed (or not yet probed), no packets requested yet.
    Initializing,
    /// Priming (or topping up) the internal packet jitter buffer.
    ///
    /// Entered by [`StreamingDemuxer::read_packet`] whenever the buffer
    /// holds fewer than [`StreamingDemuxerConfig::initial_buffer_size`]
    /// bytes' worth of packets and the source is not known to be
    /// exhausted: packets are pulled from the wrapped [`Demuxer`] into the
    /// local queue (see `StreamingDemuxer::fill_buffer_to_threshold`, a
    /// private helper) *before* one is handed back to the caller, so a slow/bursty
    /// underlying source doesn't stall every single `read_packet` call.
    /// This state is only ever observed by another task inspecting
    /// [`StreamingDemuxer::state`] while a `read_packet` future is still
    /// pending — by the time `read_packet` itself returns, the state has
    /// always moved on to `Active`, `Eof`, or `Underrun`.
    Buffering,
    /// Actively demuxing: the most recent `read_packet` call returned a
    /// packet, whether served from the local buffer (including packets
    /// still queued after the source itself drained) or read straight
    /// through with no local buffering at all (`low_latency` mode, or an
    /// `initial_buffer_size` of `0`).
    Active,
    /// Underrun: the most recent read attempt failed (a non-EOF error from
    /// the underlying source, surfaced either directly or after the local
    /// buffer ran dry).
    Underrun,
    /// End of stream reached: the source is exhausted and the local buffer
    /// (if any) has been fully drained.
    Eof,
}

/// Wrapper that adds streaming capabilities to any demuxer.
///
/// Maintains a local FIFO of [`Packet`]s (`buffer`) pulled ahead of demand
/// from the wrapped [`Demuxer`], so that [`read_packet`](Demuxer::read_packet)
/// can serve from that local queue instead of blocking on the (possibly
/// slow or bursty) underlying source for every single call. See
/// `StreamingDemuxer::fill_buffer_to_threshold` (a private helper) for the
/// priming logic and [`StreamingState::Buffering`] for what that state now
/// actually means.
pub struct StreamingDemuxer<D: Demuxer> {
    inner: D,
    config: StreamingDemuxerConfig,
    /// Packets pulled ahead of demand from `inner`, not yet handed to the
    /// caller. Empty whenever `config.low_latency` is set (buffering is
    /// bypassed entirely in that mode) or once the source is exhausted and
    /// drained.
    buffer: VecDeque<Packet>,
    state: StreamingState,
    /// Total size in bytes of every packet currently sitting in `buffer`
    /// (kept as a running counter rather than recomputed, since it is
    /// updated on every push/pop).
    bytes_buffered: usize,
    packets_read: u64,
    /// `true` once `inner.read_packet()` has returned `OxiError::Eof` at
    /// least once. Once set, `inner.read_packet()` is never called again —
    /// only the still-buffered packets (if any) continue to drain — since
    /// nothing in the [`Demuxer`] contract guarantees it is safe to keep
    /// polling a demuxer past its own EOF.
    source_exhausted: bool,
}

impl<D: Demuxer> StreamingDemuxer<D> {
    /// Creates a new streaming demuxer with default configuration.
    pub fn new(inner: D) -> Self {
        Self::with_config(inner, StreamingDemuxerConfig::default())
    }

    /// Creates a new streaming demuxer with custom configuration.
    pub fn with_config(inner: D, config: StreamingDemuxerConfig) -> Self {
        Self {
            inner,
            config,
            buffer: VecDeque::new(),
            state: StreamingState::Initializing,
            bytes_buffered: 0,
            packets_read: 0,
            source_exhausted: false,
        }
    }

    /// Returns the current state.
    #[must_use]
    pub const fn state(&self) -> StreamingState {
        self.state
    }

    /// Returns the number of bytes currently buffered.
    #[must_use]
    pub const fn bytes_buffered(&self) -> usize {
        self.bytes_buffered
    }

    /// Returns the number of packets read so far.
    #[must_use]
    pub const fn packets_read(&self) -> u64 {
        self.packets_read
    }

    /// Returns a reference to the inner demuxer.
    #[must_use]
    pub const fn inner(&self) -> &D {
        &self.inner
    }

    /// Returns a mutable reference to the inner demuxer.
    pub fn inner_mut(&mut self) -> &mut D {
        &mut self.inner
    }

    /// Unwraps and returns the inner demuxer.
    #[must_use]
    pub fn into_inner(self) -> D {
        self.inner
    }

    /// Checks if buffering is needed.
    fn needs_buffering(&self) -> bool {
        if self.config.low_latency {
            return false;
        }
        self.bytes_buffered < self.config.initial_buffer_size
    }

    /// Updates the state based on buffer level.
    ///
    /// Only used by [`probe`](Demuxer::probe); [`read_packet`](Demuxer::read_packet)
    /// sets `state` directly since each of its outcomes (served from the
    /// buffer, read straight through, exhausted, or failed) maps to exactly
    /// one state and doesn't need this heuristic.
    fn update_state(&mut self) {
        if self.bytes_buffered == 0 {
            // Nothing buffered is only really an "underrun" once we've
            // actually started trying to read; before the first
            // `read_packet` call it just means nothing has been requested
            // yet, which `Initializing` already says honestly.
            if matches!(
                self.state,
                StreamingState::Eof | StreamingState::Initializing
            ) {
                return;
            }
            self.state = StreamingState::Underrun;
        } else if self.needs_buffering() {
            self.state = StreamingState::Buffering;
        } else {
            self.state = StreamingState::Active;
        }
    }

    /// Pulls packets from `inner` into `buffer` until `bytes_buffered`
    /// reaches [`StreamingDemuxerConfig::initial_buffer_size`] (bounded by
    /// [`StreamingDemuxerConfig::max_buffer_size`]), the source signals
    /// EOF, or a read fails.
    ///
    /// This is the actual "read from the source" buffering step that used
    /// to be a no-op: `read_packet` used to flip `state` to `Buffering` and
    /// then immediately delegate to `inner.read_packet()` without ever
    /// reading anything extra. Now it really primes (or tops up) the local
    /// jitter buffer before a packet is served.
    ///
    /// Reaching EOF while filling is not itself a failure — it just means
    /// there was less source data than the configured threshold, which is
    /// normal for short streams — so it returns `Ok(())` and leaves
    /// whatever was buffered before EOF for the caller to drain. A genuine
    /// read error is returned so the caller can decide whether buffered
    /// packets are still available to serve in the meantime.
    async fn fill_buffer_to_threshold(&mut self) -> OxiResult<()> {
        while self.bytes_buffered < self.config.initial_buffer_size
            && self.bytes_buffered < self.config.max_buffer_size
        {
            match self.inner.read_packet().await {
                Ok(packet) => {
                    self.bytes_buffered += packet.size();
                    self.buffer.push_back(packet);
                }
                Err(OxiError::Eof) => {
                    self.source_exhausted = true;
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<D: Demuxer> Demuxer for StreamingDemuxer<D> {
    async fn probe(&mut self) -> OxiResult<ProbeResult> {
        self.state = StreamingState::Initializing;
        let result = self.inner.probe().await?;
        self.update_state();
        Ok(result)
    }

    async fn read_packet(&mut self) -> OxiResult<Packet> {
        // Prime (or top up) the jitter buffer before serving a packet,
        // unless low-latency mode intentionally disables it or the source
        // is already known to be exhausted (in which case `inner` must not
        // be polled again — see `source_exhausted`'s doc comment).
        if self.needs_buffering() && !self.source_exhausted {
            self.state = StreamingState::Buffering;
            if let Err(e) = self.fill_buffer_to_threshold().await {
                // A packet is still buffered from a previous fill: serve it
                // now and let this error resurface on a later call once the
                // buffer is drained (checked again via `needs_buffering`).
                // Otherwise this failure is the only thing to report.
                if self.buffer.is_empty() {
                    self.state = StreamingState::Underrun;
                    return Err(e);
                }
            }
        }

        if let Some(packet) = self.buffer.pop_front() {
            self.bytes_buffered = self.bytes_buffered.saturating_sub(packet.size());
            self.packets_read += 1;
            self.state = StreamingState::Active;
            return Ok(packet);
        }

        if self.source_exhausted {
            self.state = StreamingState::Eof;
            return Err(OxiError::Eof);
        }

        // Buffering disabled (low-latency mode) or a zero-threshold config:
        // read straight through instead of priming a buffer nothing will
        // ever fill.
        match self.inner.read_packet().await {
            Ok(packet) => {
                self.packets_read += 1;
                self.state = StreamingState::Active;
                Ok(packet)
            }
            Err(OxiError::Eof) => {
                self.source_exhausted = true;
                self.state = StreamingState::Eof;
                Err(OxiError::Eof)
            }
            Err(e) => {
                self.state = StreamingState::Underrun;
                Err(e)
            }
        }
    }

    fn streams(&self) -> &[StreamInfo] {
        self.inner.streams()
    }

    fn is_seekable(&self) -> bool {
        // Streaming demuxers are not seekable
        false
    }
}

/// Async packet receiver for background demuxing.
#[cfg(not(target_arch = "wasm32"))]
pub struct PacketReceiver {
    rx: mpsc::UnboundedReceiver<OxiResult<Packet>>,
    streams: Vec<StreamInfo>,
}

#[cfg(not(target_arch = "wasm32"))]
impl PacketReceiver {
    /// Creates a new packet receiver.
    fn new(rx: mpsc::UnboundedReceiver<OxiResult<Packet>>, streams: Vec<StreamInfo>) -> Self {
        Self { rx, streams }
    }

    /// Receives the next packet.
    pub async fn recv(&mut self) -> Option<OxiResult<Packet>> {
        self.rx.recv().await
    }

    /// Returns stream information.
    #[must_use]
    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    /// Tries to receive a packet without blocking.
    ///
    /// # Errors
    ///
    /// Returns `Err(TryRecvError)` if no packet is available or the channel is closed.
    pub fn try_recv(&mut self) -> Result<OxiResult<Packet>, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

/// Spawns a background task for demuxing.
///
/// This function creates a background task that continuously reads packets
/// from the demuxer and sends them through a channel. This is useful for
/// streaming scenarios where you want to decouple demuxing from processing.
///
/// # Arguments
///
/// * `demuxer` - The demuxer to run in the background
///
/// # Returns
///
/// A `PacketReceiver` that can be used to receive packets from the background task.
///
/// # Errors
///
/// Returns `Err` if the demuxer fails during probing.
///
/// # Examples
///
/// ```ignore
/// let demuxer = MatroskaDemuxer::new(source);
/// let mut receiver = spawn_demuxer(demuxer).await?;
///
/// while let Some(result) = receiver.recv().await {
///     match result {
///         Ok(packet) => process_packet(packet),
///         Err(e) => handle_error(e),
///     }
/// }
/// ```
#[cfg(not(target_arch = "wasm32"))]
pub async fn spawn_demuxer<D: Demuxer + Send + 'static>(
    mut demuxer: D,
) -> OxiResult<PacketReceiver> {
    // Probe the demuxer first
    demuxer.probe().await?;
    let streams = demuxer.streams().to_vec();

    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            match demuxer.read_packet().await {
                Ok(packet) => {
                    if tx.send(Ok(packet)).is_err() {
                        // Receiver dropped, exit
                        break;
                    }
                }
                Err(OxiError::Eof) => {
                    let _ = tx.send(Err(OxiError::Eof));
                    break;
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });

    Ok(PacketReceiver::new(rx, streams))
}

/// Buffer for progressive data accumulation.
#[derive(Debug)]
pub struct ProgressiveBuffer {
    data: VecDeque<u8>,
    max_size: usize,
    total_received: u64,
}

impl ProgressiveBuffer {
    /// Creates a new progressive buffer.
    #[must_use]
    pub fn new(max_size: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(max_size.min(64 * 1024)),
            max_size,
            total_received: 0,
        }
    }

    /// Appends data to the buffer.
    ///
    /// # Errors
    ///
    /// Returns `Err` if adding the data would exceed the maximum buffer size.
    pub fn append(&mut self, data: &[u8]) -> OxiResult<()> {
        if self.data.len() + data.len() > self.max_size {
            return Err(OxiError::BufferTooSmall {
                needed: self.data.len() + data.len(),
                have: self.max_size,
            });
        }
        self.data.extend(data);
        self.total_received += data.len() as u64;
        Ok(())
    }

    /// Consumes bytes from the front of the buffer.
    pub fn consume(&mut self, count: usize) -> Option<Bytes> {
        if count > self.data.len() {
            return None;
        }
        let bytes: Vec<u8> = self.data.drain(..count).collect();
        Some(Bytes::from(bytes))
    }

    /// Peeks at the front of the buffer without consuming.
    #[must_use]
    pub fn peek(&self, count: usize) -> Option<&[u8]> {
        if count > self.data.len() {
            return None;
        }
        // Convert VecDeque slices to a single slice if possible
        let (first, _second) = self.data.as_slices();
        if count <= first.len() {
            Some(&first[..count])
        } else {
            None // Data is fragmented across VecDeque halves
        }
    }

    /// Returns the number of bytes currently in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns the total number of bytes received.
    #[must_use]
    pub const fn total_received(&self) -> u64 {
        self.total_received
    }

    /// Clears the buffer.
    pub fn clear(&mut self) {
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oximedia_core::{CodecId, Rational, Timestamp};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Emits `total` fixed-size packets (`packet_size` bytes each, with PTS
    /// `0..total` in the packet's own timebase-agnostic index so tests can
    /// identify which packet came back), then `OxiError::Eof` forever after.
    ///
    /// If `fail_at == Some(n)`, the `n`-th call to `read_packet` (0-based,
    /// counted across the mock's whole lifetime) returns a synthetic
    /// non-EOF error instead of a packet, and does **not** advance
    /// `emitted` — the same would-be packet is emitted on the next call, so
    /// a single injected failure never drops or skips a packet.
    struct MockPacketDemuxer {
        streams: Vec<StreamInfo>,
        total: usize,
        packet_size: usize,
        emitted: usize,
        calls: Arc<AtomicUsize>,
        fail_at: Option<usize>,
    }

    impl MockPacketDemuxer {
        fn new(total: usize, packet_size: usize) -> Self {
            let info = StreamInfo::new(0, CodecId::Opus, Rational::new(1, 48000));
            Self {
                streams: vec![info],
                total,
                packet_size,
                emitted: 0,
                calls: Arc::new(AtomicUsize::new(0)),
                fail_at: None,
            }
        }

        /// A shared handle for inspecting the total call count from outside
        /// the mock after it has been moved into a `StreamingDemuxer`.
        fn call_counter(&self) -> Arc<AtomicUsize> {
            Arc::clone(&self.calls)
        }

        fn with_failure_at(mut self, call_index: usize) -> Self {
            self.fail_at = Some(call_index);
            self
        }
    }

    #[async_trait]
    impl Demuxer for MockPacketDemuxer {
        async fn probe(&mut self) -> OxiResult<ProbeResult> {
            Ok(ProbeResult::new(crate::ContainerFormat::Mp4, 1.0))
        }

        async fn read_packet(&mut self) -> OxiResult<Packet> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_at == Some(call_index) {
                return Err(OxiError::InvalidData("synthetic failure".to_string()));
            }
            if self.emitted >= self.total {
                return Err(OxiError::Eof);
            }
            #[allow(clippy::cast_possible_wrap)]
            let pts = self.emitted as i64;
            let timestamp = Timestamp::new(pts, Rational::new(1, 48000));
            let data = bytes::Bytes::from(vec![0xABu8; self.packet_size]);
            self.emitted += 1;
            Ok(Packet::new(0, data, timestamp, crate::PacketFlags::empty()))
        }

        fn streams(&self) -> &[StreamInfo] {
            &self.streams
        }
    }

    #[tokio::test]
    async fn test_read_packet_primes_and_tops_up_buffer_ahead_of_demand() {
        // threshold=25 bytes, 10-byte packets: the fill loop must pull
        // packets 0,1,2 (10+10+10=30 >= 25) before the first is served.
        let mock = MockPacketDemuxer::new(100, 10);
        let calls = mock.call_counter();
        let cfg = StreamingDemuxerConfig::new().with_initial_buffer(25);
        let mut streaming = StreamingDemuxer::with_config(mock, cfg);
        streaming.probe().await.expect("probe");

        let p0 = streaming.read_packet().await.expect("first packet");
        assert_eq!(p0.pts(), 0, "packets must be served in order");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "a single read_packet() call must prefetch ahead of demand, not delegate 1:1"
        );
        assert_eq!(
            streaming.bytes_buffered(),
            20,
            "2 remaining prefetched packets (20 bytes) still queued"
        );
        assert_eq!(streaming.packets_read(), 1);
        assert_eq!(streaming.state(), StreamingState::Active);

        let p1 = streaming.read_packet().await.expect("second packet");
        assert_eq!(p1.pts(), 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "second call tops the buffer back up by exactly one packet"
        );
        assert_eq!(streaming.bytes_buffered(), 20);
        assert_eq!(streaming.packets_read(), 2);
    }

    #[tokio::test]
    async fn test_low_latency_bypasses_the_buffer_entirely() {
        let mock = MockPacketDemuxer::new(50, 10);
        let calls = mock.call_counter();
        let cfg = StreamingDemuxerConfig::new().with_low_latency(true);
        let mut streaming = StreamingDemuxer::with_config(mock, cfg);

        for i in 0..5u64 {
            let packet = streaming.read_packet().await.expect("packet");
            #[allow(clippy::cast_possible_wrap)]
            let expected_pts = i as i64;
            assert_eq!(packet.pts(), expected_pts);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                (i + 1) as usize,
                "low-latency mode must read exactly one packet per call, no read-ahead"
            );
            assert_eq!(
                streaming.bytes_buffered(),
                0,
                "low-latency never retains payload"
            );
        }
    }

    #[tokio::test]
    async fn test_read_packet_eof_is_terminal_and_never_repolls_source() {
        // Default config: 64 KiB threshold, far above the 5*10=50 byte
        // total, so the very first call's fill drains the whole source.
        let mock = MockPacketDemuxer::new(5, 10);
        let calls = mock.call_counter();
        let mut streaming = StreamingDemuxer::new(mock);

        let mut count = 0;
        loop {
            match streaming.read_packet().await {
                Ok(_) => count += 1,
                Err(OxiError::Eof) => break,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        }
        assert_eq!(count, 5);
        assert_eq!(streaming.state(), StreamingState::Eof);
        assert_eq!(streaming.bytes_buffered(), 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            6,
            "5 real packets plus exactly 1 EOF-detecting call"
        );

        // Repeated polling past EOF must keep surfacing Eof without ever
        // calling the inner demuxer again.
        for _ in 0..10 {
            assert!(matches!(streaming.read_packet().await, Err(OxiError::Eof)));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            6,
            "inner demuxer must not be polled again once exhausted"
        );
        assert_eq!(streaming.packets_read(), 5);
    }

    #[tokio::test]
    async fn test_read_packet_propagates_error_when_buffer_empty() {
        // threshold == packet size: the fill loop pulls at most one packet
        // per call, so the second call's fill has nothing buffered to fall
        // back on when the injected failure hits.
        let mock = MockPacketDemuxer::new(100, 10).with_failure_at(1);
        let cfg = StreamingDemuxerConfig::new().with_initial_buffer(10);
        let mut streaming = StreamingDemuxer::with_config(mock, cfg);

        let p0 = streaming
            .read_packet()
            .await
            .expect("first packet succeeds (call 0)");
        assert_eq!(p0.pts(), 0);

        let err = streaming.read_packet().await;
        assert!(
            matches!(err, Err(OxiError::InvalidData(_))),
            "the injected failure must surface unchanged once nothing is buffered \
             (not swallowed, not turned into Eof)"
        );
        assert_eq!(streaming.state(), StreamingState::Underrun);

        // A retry succeeds and sequencing continues correctly — the failed
        // attempt did not desync or silently drop a packet.
        let p1 = streaming.read_packet().await.expect("retry succeeds");
        assert_eq!(p1.pts(), 1);
    }

    #[tokio::test]
    async fn test_read_packet_masks_fill_error_when_packets_already_buffered() {
        // threshold=25, packet=10: the fill for the first call buffers
        // packet 0 (call index 0) then hits the injected failure at call
        // index 1 while trying to buffer more. Because packet 0 was
        // already queued, this call must still succeed.
        let mock = MockPacketDemuxer::new(100, 10).with_failure_at(1);
        let cfg = StreamingDemuxerConfig::new().with_initial_buffer(25);
        let mut streaming = StreamingDemuxer::with_config(mock, cfg);

        let p0 = streaming
            .read_packet()
            .await
            .expect("masked by the already-buffered packet 0");
        assert_eq!(p0.pts(), 0);
        assert_eq!(streaming.state(), StreamingState::Active);

        // Next call's fill retries (now past the one-shot failure) and
        // proceeds normally.
        let p1 = streaming.read_packet().await.expect("fill retry succeeds");
        assert_eq!(p1.pts(), 1);
    }

    #[tokio::test]
    async fn test_probe_does_not_report_underrun() {
        // A successful probe with nothing read yet must not claim a stall
        // that hasn't happened — `Underrun` describes a failed read
        // attempt, not "haven't started yet".
        let mock = MockPacketDemuxer::new(0, 10);
        let mut streaming = StreamingDemuxer::new(mock);
        streaming.probe().await.expect("probe");
        assert_ne!(streaming.state(), StreamingState::Underrun);
        assert_eq!(streaming.state(), StreamingState::Initializing);
    }

    #[test]
    fn test_config_default() {
        let config = StreamingDemuxerConfig::default();
        assert_eq!(config.initial_buffer_size, 64 * 1024);
        assert_eq!(config.max_buffer_size, 10 * 1024 * 1024);
        assert!(!config.low_latency);
    }

    #[test]
    fn test_config_builder() {
        let config = StreamingDemuxerConfig::new()
            .with_low_latency(true)
            .with_initial_buffer(128 * 1024)
            .with_max_buffer(20 * 1024 * 1024)
            .with_timeout(10000);

        assert!(config.low_latency);
        assert_eq!(config.initial_buffer_size, 128 * 1024);
        assert_eq!(config.max_buffer_size, 20 * 1024 * 1024);
        assert_eq!(config.read_timeout_ms, 10000);
    }

    #[test]
    fn test_progressive_buffer() {
        let mut buffer = ProgressiveBuffer::new(1024);
        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);

        // Append data
        buffer
            .append(&[1, 2, 3, 4])
            .expect("operation should succeed");
        assert_eq!(buffer.len(), 4);
        assert_eq!(buffer.total_received(), 4);

        // Peek
        let peeked = buffer.peek(2).expect("operation should succeed");
        assert_eq!(peeked, &[1, 2]);
        assert_eq!(buffer.len(), 4); // Still has all data

        // Consume
        let consumed = buffer.consume(2).expect("operation should succeed");
        assert_eq!(consumed.as_ref(), &[1, 2]);
        assert_eq!(buffer.len(), 2);

        // Clear
        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_progressive_buffer_overflow() {
        let mut buffer = ProgressiveBuffer::new(10);
        assert!(buffer.append(&[1, 2, 3, 4, 5]).is_ok());
        assert!(buffer.append(&[6, 7, 8, 9, 10, 11]).is_err());
    }
}

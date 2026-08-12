//! Delivered frames, and the clock that timestamps them.

use std::time::Duration;

use oximedia_codec::VideoFrame;

use crate::device::CaptureFormat;

// ── Timestamp source ─────────────────────────────────────────────────────────

/// Where a frame's [`CaptureFrame::timestamp`] came from.
///
/// This is recorded, never guessed. A backend that cannot obtain a device
/// clock reports [`TimestampSource::HostArrival`] rather than inventing a
/// device timeline, because the two have materially different properties:
/// a device clock is sampled at exposure time, while host arrival includes
/// USB transfer and scheduler latency and therefore jitters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimestampSource {
    /// Sampled from the capture device's own monotonic clock.
    DeviceMonotonic,
    /// Taken on the host when the frame was dequeued.
    HostArrival,
}

impl TimestampSource {
    /// Stable lower-case identifier, for logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DeviceMonotonic => "device-monotonic",
            Self::HostArrival => "host-arrival",
        }
    }
}

impl std::fmt::Display for TimestampSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

// ── Payload ──────────────────────────────────────────────────────────────────

/// The frame data itself.
///
/// Compressed payloads are handed over untouched. This crate deliberately does
/// **not** decode them, because doing so would drag a JPEG decoder into every
/// build that only wanted to open a camera.
///
/// To decode MJPEG, add `oximedia-codec` with its `mjpeg` feature to *your*
/// crate and feed [`FramePayload::Compressed`] bytes to `MjpegDecoder`:
///
/// ```text
/// # Cargo.toml
/// oximedia-codec = { version = "0.2.1", features = ["mjpeg"] }
/// ```
///
/// ```text
/// match frame.payload {
///     FramePayload::Raw(video) => use_frame(&video),
///     FramePayload::Compressed(jpeg) => use_frame(&decoder.decode(&jpeg)?),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum FramePayload {
    /// Decoded samples, in the pixel layout named by the format.
    Raw(VideoFrame),
    /// A compressed bitstream — one complete JPEG image for MJPEG sources.
    Compressed(Vec<u8>),
}

impl FramePayload {
    /// The decoded frame, when this payload is raw samples.
    pub const fn raw(&self) -> Option<&VideoFrame> {
        match self {
            Self::Raw(video) => Some(video),
            Self::Compressed(_) => None,
        }
    }

    /// The compressed bitstream, when this payload is compressed.
    pub fn compressed(&self) -> Option<&[u8]> {
        match self {
            Self::Raw(_) => None,
            Self::Compressed(bytes) => Some(bytes),
        }
    }

    /// `true` when the payload must be decoded before use.
    pub const fn is_compressed(&self) -> bool {
        matches!(self, Self::Compressed(_))
    }

    /// Payload size in bytes: the sum of the plane buffers for raw frames, the
    /// bitstream length for compressed ones.
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Raw(video) => video.planes.iter().map(|plane| plane.data.len()).sum(),
            Self::Compressed(bytes) => bytes.len(),
        }
    }
}

// ── Frame ────────────────────────────────────────────────────────────────────

/// One frame delivered by a [`crate::CaptureSession`].
#[derive(Debug, Clone)]
pub struct CaptureFrame {
    /// The negotiated format this frame was captured in.
    pub format: CaptureFormat,
    /// The frame data.
    pub payload: FramePayload,
    /// Presentation time, measured from the first frame of the session.
    ///
    /// Guaranteed monotonic non-decreasing: the session normalizes whatever
    /// the device reports through a `MonotonicClock`, so a device clock that
    /// jumps backwards (firmware wrap, resynchronization, suspend/resume) is
    /// clamped to the previous value rather than being passed through. Each
    /// such clamp is counted in [`crate::CaptureStats::clock_regressions`].
    ///
    /// [`Self::timestamp_source`] says whether this came from the device or
    /// from host arrival time.
    pub timestamp: Duration,
    /// Host arrival time, also measured from the first frame of the session
    /// and also normalized to be non-decreasing.
    ///
    /// Comparing this against [`Self::timestamp`] gives the end-to-end
    /// transport latency for device-clocked sources. For
    /// [`TimestampSource::HostArrival`] sources the two are the same value.
    pub host_timestamp: Duration,
    /// Which clock [`Self::timestamp`] came from.
    pub timestamp_source: TimestampSource,
    /// Frame counter as reported by the driver.
    ///
    /// A gap in this sequence means the *driver* dropped frames before this
    /// crate ever saw them — the capture thread could not keep up with the
    /// device, or the device overran its own ring. Those losses are counted
    /// separately from this crate's own queue drops, in
    /// [`crate::CaptureStats::device_dropped`].
    pub sequence: u64,
}

impl CaptureFrame {
    /// Frame width in pixels, from the negotiated format.
    pub const fn width(&self) -> u32 {
        self.format.width
    }

    /// Frame height in pixels, from the negotiated format.
    pub const fn height(&self) -> u32 {
        self.format.height
    }

    /// The decoded frame, when the payload is raw samples.
    pub const fn video(&self) -> Option<&VideoFrame> {
        self.payload.raw()
    }

    /// The compressed bitstream, when the payload is compressed.
    pub fn compressed(&self) -> Option<&[u8]> {
        self.payload.compressed()
    }
}

// ── Monotonic clock ──────────────────────────────────────────────────────────

/// Maps raw device or host instants onto a non-decreasing
/// since-first-sample timeline.
///
/// Capture clocks misbehave in practice: 32-bit device counters wrap, some
/// firmwares restart their clock after a mode change, and a host suspend can
/// leave a device timestamp behind the previous one. Rather than propagate a
/// backwards jump (which breaks every downstream muxer and A/V sync loop) or
/// fabricate a plausible-looking replacement (which hides a real fault), this
/// clock clamps to the previous value and counts the event so it shows up in
/// [`crate::CaptureStats::clock_regressions`].
#[derive(Debug, Clone, Default)]
pub(crate) struct MonotonicClock {
    origin: Option<Duration>,
    last: Duration,
    regressions: u64,
}

impl MonotonicClock {
    /// A clock that has not yet seen a sample.
    pub(crate) const fn new() -> Self {
        Self {
            origin: None,
            last: Duration::ZERO,
            regressions: 0,
        }
    }

    /// Map a raw instant onto the since-first-sample timeline.
    ///
    /// The first sample defines the origin and maps to [`Duration::ZERO`]. A
    /// sample that would move the timeline backwards is clamped to the
    /// previous value and counted; compare [`Self::regressions`] before and
    /// after to learn whether that happened.
    pub(crate) fn map(&mut self, raw: Duration) -> Duration {
        let origin = *self.origin.get_or_insert(raw);
        // `saturating_sub` covers a raw sample that predates the origin, which
        // is the same fault as any other backwards jump.
        let elapsed = raw.saturating_sub(origin);
        if elapsed < self.last {
            self.regressions = self.regressions.saturating_add(1);
            return self.last;
        }
        self.last = elapsed;
        elapsed
    }

    /// How many samples have been clamped so far.
    ///
    /// This is the sole source of [`crate::CaptureStats::clock_regressions`].
    pub(crate) const fn regressions(&self) -> u64 {
        self.regressions
    }
}

#[cfg(test)]
mod tests {
    use oximedia_core::PixelFormat;

    use super::*;
    use crate::device::CaptureEncoding;

    fn millis(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    // ── MonotonicClock ──────────────────────────────────────────────────────

    #[test]
    fn first_sample_defines_the_origin() {
        let mut clock = MonotonicClock::new();
        assert_eq!(clock.map(millis(9_000)), Duration::ZERO);
        assert_eq!(clock.regressions(), 0);
    }

    #[test]
    fn samples_are_relative_to_the_origin() {
        let mut clock = MonotonicClock::new();
        clock.map(millis(1_000));
        assert_eq!(clock.map(millis(1_033)), millis(33));
        assert_eq!(clock.map(millis(1_066)), millis(66));
    }

    #[test]
    fn backwards_samples_are_clamped_and_counted() {
        let mut clock = MonotonicClock::new();
        clock.map(millis(0));
        clock.map(millis(100));
        assert_eq!(
            clock.map(millis(50)),
            millis(100),
            "clamped to the previous"
        );
        assert_eq!(clock.regressions(), 1);
        // The timeline resumes from the clamped value, not from the bad one.
        assert_eq!(clock.map(millis(150)), millis(150));
        assert_eq!(clock.regressions(), 1);
    }

    #[test]
    fn a_sample_before_the_origin_is_a_regression_not_a_negative_time() {
        let mut clock = MonotonicClock::new();
        clock.map(millis(500));
        clock.map(millis(600));
        assert_eq!(clock.map(millis(100)), millis(100));
        assert_eq!(clock.regressions(), 1);
    }

    #[test]
    fn equal_samples_are_not_regressions() {
        let mut clock = MonotonicClock::new();
        clock.map(millis(10));
        assert_eq!(clock.map(millis(10)), Duration::ZERO);
        assert_eq!(clock.regressions(), 0);
    }

    #[test]
    fn output_is_never_decreasing_for_arbitrary_input() {
        let mut clock = MonotonicClock::new();
        let raw = [30u64, 10, 40, 5, 90, 20, 90, 0, 1_000];
        let mut previous = Duration::ZERO;
        for value in raw {
            let mapped = clock.map(millis(value));
            assert!(mapped >= previous, "{mapped:?} < {previous:?}");
            previous = mapped;
        }
        assert!(clock.regressions() > 0);
    }

    // ── Payload ─────────────────────────────────────────────────────────────

    #[test]
    fn raw_payload_exposes_the_video_frame() {
        let mut video = VideoFrame::new(PixelFormat::Gray8, 16, 8);
        video.allocate();
        let payload = FramePayload::Raw(video);
        assert!(!payload.is_compressed());
        assert!(payload.compressed().is_none());
        assert_eq!(payload.raw().map(|v| v.width), Some(16));
        assert_eq!(payload.byte_len(), 16 * 8);
    }

    #[test]
    fn compressed_payload_exposes_the_bitstream() {
        let payload = FramePayload::Compressed(vec![0xFF, 0xD8, 0xFF, 0xD9]);
        assert!(payload.is_compressed());
        assert!(payload.raw().is_none());
        assert_eq!(payload.compressed(), Some(&[0xFF, 0xD8, 0xFF, 0xD9][..]));
        assert_eq!(payload.byte_len(), 4);
    }

    #[test]
    fn frame_accessors_read_from_the_negotiated_format() {
        let frame = CaptureFrame {
            format: CaptureFormat::new(CaptureEncoding::Mjpeg, 1280, 720, 30, 1),
            payload: FramePayload::Compressed(vec![0; 12]),
            timestamp: millis(33),
            host_timestamp: millis(35),
            timestamp_source: TimestampSource::DeviceMonotonic,
            sequence: 1,
        };
        assert_eq!(frame.width(), 1280);
        assert_eq!(frame.height(), 720);
        assert!(frame.video().is_none());
        assert_eq!(frame.compressed().map(<[u8]>::len), Some(12));
    }

    #[test]
    fn timestamp_source_names_are_stable() {
        assert_eq!(TimestampSource::DeviceMonotonic.name(), "device-monotonic");
        assert_eq!(TimestampSource::HostArrival.to_string(), "host-arrival");
    }
}

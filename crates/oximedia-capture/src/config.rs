//! Capture request: what the caller wants, before the device gets a say.
//!
//! A [`CaptureConfig`] is a *request*, not a promise. Every field is a
//! preference that [`crate::negotiate()`] reconciles against what the device
//! actually advertises; nothing here is silently forced onto the driver.

use oximedia_core::PixelFormat;

use crate::device::{CaptureEncoding, DeviceSelector};

// ── Encoding preference ──────────────────────────────────────────────────────

/// Default encoding preference, most preferred first.
///
/// The ordering is not arbitrary:
///
/// 1. [`PixelFormat::Nv12`] — the native output of most modern UVC sensors and
///    of every hardware scaler worth using. Semi-planar 4:2:0, so it is also
///    the cheapest thing to hand to an encoder.
/// 2. [`PixelFormat::Yuyv422`] — the UVC mandatory fallback. Full chroma
///    horizontally, packed, and covered by an in-ecosystem converter.
/// 3. [`PixelFormat::Uyvy422`] — the same 4:2:2 data with the opposite byte
///    order; ranked just below YUYV only because it is rarer on USB cameras.
/// 4. [`PixelFormat::Yuv420p`] — planar 4:2:0. Equivalent information to NV12
///    but needs a plane de-interleave on most capture paths.
/// 5. [`PixelFormat::Rgb24`] — correct but bandwidth-hungry, and almost always
///    produced by an in-camera conversion that has already thrown chroma away.
/// 6. [`CaptureEncoding::Mjpeg`] — last, and only because it costs a full JPEG
///    decode per frame. It is still listed, because on many USB2 cameras it is
///    the only way to get high resolutions at full frame rate.
///
/// Set [`CaptureConfig::preferred`] to override this wholesale.
pub const DEFAULT_ENCODING_PREFERENCE: [CaptureEncoding; 6] = [
    CaptureEncoding::Raw(PixelFormat::Nv12),
    CaptureEncoding::Raw(PixelFormat::Yuyv422),
    CaptureEncoding::Raw(PixelFormat::Uyvy422),
    CaptureEncoding::Raw(PixelFormat::Yuv420p),
    CaptureEncoding::Raw(PixelFormat::Rgb24),
    CaptureEncoding::Mjpeg,
];

// ── Drop policy ──────────────────────────────────────────────────────────────

/// What to do when the delivery queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DropPolicy {
    /// Discard the oldest queued frame to make room for the new one.
    ///
    /// This is the default, and for a live camera it is the only defensible
    /// one. A capture device owns a small ring of DMA buffers; if the capture
    /// thread stops dequeuing them the *driver* runs out of buffers and starts
    /// dropping frames itself — silently, at a layer where nothing can count
    /// them. Discarding here instead keeps the driver ring cycling, keeps
    /// latency bounded at `queue_depth` frames, and makes every loss visible
    /// in [`crate::CaptureStats::dropped`].
    #[default]
    DropOldest,
    /// Discard the frame that just arrived, keeping the queued backlog.
    ///
    /// Useful when a consumer needs a contiguous run of frames more than it
    /// needs recent ones.
    DropNewest,
    /// Block the capture thread until the consumer makes room.
    ///
    /// Applies real back-pressure, at the cost of stalling the driver ring.
    /// Appropriate for offline or file-backed sources, and for consumers that
    /// genuinely must not miss a frame; a poor fit for a live camera.
    Block,
}

impl DropPolicy {
    /// Stable lower-case identifier, for logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DropOldest => "drop-oldest",
            Self::DropNewest => "drop-newest",
            Self::Block => "block",
        }
    }
}

impl std::fmt::Display for DropPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

// ── Config ───────────────────────────────────────────────────────────────────

/// Default number of frames buffered between the capture thread and consumer.
pub const DEFAULT_QUEUE_DEPTH: usize = 4;

/// Default number of driver-side buffers requested from the device.
pub const DEFAULT_BUFFER_COUNT: u32 = 4;

/// A capture request.
///
/// Build one with [`CaptureConfig::default`] and the `with_*` methods, then
/// hand it to [`crate::open`].
///
/// ```
/// use oximedia_capture::{CaptureConfig, DeviceSelector};
///
/// let config = CaptureConfig::default()
///     .with_device(DeviceSelector::Index(0))
///     .with_size(1280, 720)
///     .with_fps(30.0);
///
/// assert_eq!(config.width, Some(1280));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    /// Which device to open.
    pub device: DeviceSelector,
    /// Requested width in pixels. `None` means "whatever is largest".
    pub width: Option<u32>,
    /// Requested height in pixels. `None` means "whatever is largest".
    pub height: Option<u32>,
    /// Requested frame rate. `None` means "whatever is highest".
    pub fps: Option<f64>,
    /// Encoding preference, most preferred first.
    ///
    /// Empty means [`DEFAULT_ENCODING_PREFERENCE`]. Encodings that are not
    /// listed are still eligible, but rank after every listed one.
    pub preferred: Vec<CaptureEncoding>,
    /// Whether compressed encodings (MJPEG) may be selected at all.
    ///
    /// Defaults to `true`. Setting it to `false` removes compressed formats
    /// *before* ranking, so a device that offers nothing else yields an honest
    /// [`crate::CaptureError::NoMatchingFormat`] rather than a surprise decode
    /// requirement.
    pub allow_compressed: bool,
    /// How many frames may sit between the capture thread and the consumer.
    ///
    /// This is the latency budget. Larger values absorb longer consumer
    /// stalls; smaller values keep the newest frame newer.
    pub queue_depth: usize,
    /// How many buffers to request from the driver.
    ///
    /// Backends treat this as a hint and clamp it to what the device allows.
    pub buffer_count: u32,
    /// What to do when the delivery queue is full.
    pub drop_policy: DropPolicy,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            device: DeviceSelector::Default,
            width: None,
            height: None,
            fps: None,
            preferred: Vec::new(),
            allow_compressed: true,
            queue_depth: DEFAULT_QUEUE_DEPTH,
            buffer_count: DEFAULT_BUFFER_COUNT,
            drop_policy: DropPolicy::DropOldest,
        }
    }
}

impl CaptureConfig {
    /// Select the device to open.
    pub fn with_device(mut self, device: DeviceSelector) -> Self {
        self.device = device;
        self
    }

    /// Request a frame size.
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Request a frame rate.
    pub fn with_fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Replace the encoding preference list, most preferred first.
    pub fn with_preferred(mut self, preferred: impl IntoIterator<Item = CaptureEncoding>) -> Self {
        self.preferred = preferred.into_iter().collect();
        self
    }

    /// Allow or forbid compressed encodings.
    pub fn with_allow_compressed(mut self, allow: bool) -> Self {
        self.allow_compressed = allow;
        self
    }

    /// Set the delivery queue depth (clamped to at least 1).
    pub fn with_queue_depth(mut self, depth: usize) -> Self {
        self.queue_depth = depth.max(1);
        self
    }

    /// Set the number of driver-side buffers to request (clamped to at least 1).
    pub fn with_buffer_count(mut self, count: u32) -> Self {
        self.buffer_count = count.max(1);
        self
    }

    /// Set the queue-full behaviour.
    pub fn with_drop_policy(mut self, policy: DropPolicy) -> Self {
        self.drop_policy = policy;
        self
    }

    /// The effective encoding preference: [`Self::preferred`] when non-empty,
    /// otherwise [`DEFAULT_ENCODING_PREFERENCE`].
    pub fn encoding_preference(&self) -> &[CaptureEncoding] {
        if self.preferred.is_empty() {
            &DEFAULT_ENCODING_PREFERENCE
        } else {
            &self.preferred
        }
    }

    /// Queue depth, clamped to the minimum a bounded channel can hold.
    ///
    /// A zero-capacity crossbeam channel is a rendezvous channel, which turns
    /// every `try_send` into a failure unless a receiver is parked at that
    /// exact instant. That would make [`DropPolicy`] meaningless, so a
    /// requested depth of zero is treated as one.
    pub fn effective_queue_depth(&self) -> usize {
        self.queue_depth.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_live_camera_shaped() {
        let cfg = CaptureConfig::default();
        assert_eq!(cfg.device, DeviceSelector::Default);
        assert_eq!(cfg.width, None);
        assert_eq!(cfg.height, None);
        assert_eq!(cfg.fps, None);
        assert!(cfg.preferred.is_empty());
        assert!(cfg.allow_compressed);
        assert_eq!(cfg.queue_depth, DEFAULT_QUEUE_DEPTH);
        assert_eq!(cfg.buffer_count, DEFAULT_BUFFER_COUNT);
        assert_eq!(cfg.drop_policy, DropPolicy::DropOldest);
    }

    #[test]
    fn drop_policy_default_is_drop_oldest() {
        assert_eq!(DropPolicy::default(), DropPolicy::DropOldest);
    }

    #[test]
    fn builders_compose() {
        let cfg = CaptureConfig::default()
            .with_device(DeviceSelector::Id("/dev/video2".into()))
            .with_size(1920, 1080)
            .with_fps(59.94)
            .with_preferred([CaptureEncoding::Mjpeg])
            .with_allow_compressed(false)
            .with_queue_depth(8)
            .with_buffer_count(6)
            .with_drop_policy(DropPolicy::Block);

        assert_eq!(cfg.device, DeviceSelector::Id("/dev/video2".into()));
        assert_eq!(cfg.width, Some(1920));
        assert_eq!(cfg.height, Some(1080));
        assert_eq!(cfg.fps, Some(59.94));
        assert_eq!(cfg.preferred, vec![CaptureEncoding::Mjpeg]);
        assert!(!cfg.allow_compressed);
        assert_eq!(cfg.queue_depth, 8);
        assert_eq!(cfg.buffer_count, 6);
        assert_eq!(cfg.drop_policy, DropPolicy::Block);
    }

    #[test]
    fn zero_queue_depth_is_clamped_to_one() {
        let cfg = CaptureConfig::default().with_queue_depth(0);
        assert_eq!(cfg.queue_depth, 1);
        assert_eq!(cfg.effective_queue_depth(), 1);
    }

    #[test]
    fn effective_queue_depth_clamps_a_directly_assigned_zero() {
        let mut cfg = CaptureConfig::default();
        cfg.queue_depth = 0;
        assert_eq!(cfg.effective_queue_depth(), 1);
    }

    #[test]
    fn zero_buffer_count_is_clamped_to_one() {
        assert_eq!(
            CaptureConfig::default().with_buffer_count(0).buffer_count,
            1
        );
    }

    #[test]
    fn empty_preference_falls_back_to_default_order() {
        let cfg = CaptureConfig::default();
        assert_eq!(cfg.encoding_preference(), &DEFAULT_ENCODING_PREFERENCE);
    }

    #[test]
    fn explicit_preference_wins() {
        let cfg = CaptureConfig::default().with_preferred([CaptureEncoding::Mjpeg]);
        assert_eq!(cfg.encoding_preference(), &[CaptureEncoding::Mjpeg]);
    }

    #[test]
    fn default_preference_ranks_nv12_first_and_mjpeg_last() {
        assert_eq!(
            DEFAULT_ENCODING_PREFERENCE.first(),
            Some(&CaptureEncoding::Raw(PixelFormat::Nv12))
        );
        assert_eq!(
            DEFAULT_ENCODING_PREFERENCE.last(),
            Some(&CaptureEncoding::Mjpeg)
        );
    }

    #[test]
    fn default_preference_has_no_duplicates() {
        for (i, a) in DEFAULT_ENCODING_PREFERENCE.iter().enumerate() {
            for b in &DEFAULT_ENCODING_PREFERENCE[i + 1..] {
                assert_ne!(a, b, "duplicate encoding in default preference");
            }
        }
    }

    #[test]
    fn drop_policy_names_are_stable() {
        assert_eq!(DropPolicy::DropOldest.name(), "drop-oldest");
        assert_eq!(DropPolicy::DropNewest.name(), "drop-newest");
        assert_eq!(DropPolicy::Block.to_string(), "block");
    }
}

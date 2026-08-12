//! Device, encoding and format descriptions shared by every capture backend.
//!
//! Nothing in this module talks to hardware. These are the plain data shapes
//! that [`crate::enumerate`] fills in from a platform backend and that
//! [`crate::negotiate()`] ranks. Keeping them backend-agnostic is what lets the
//! negotiation logic be unit-tested exhaustively without a camera attached.

use std::fmt;

use oximedia_core::PixelFormat;

// ── Backend kind ─────────────────────────────────────────────────────────────

/// Which platform capture API a [`CaptureDevice`] came from.
///
/// [`BackendKind::Unsupported`] is reported when the current target has no
/// capture implementation compiled in. It is deliberately a real variant
/// rather than, say, returning the *intended* backend for the platform:
/// claiming `AvFoundation` on a build that contains no AVFoundation code
/// would be fabricated data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// Linux Video4Linux2 (`/dev/video*`).
    V4l2,
    /// macOS / iOS AVFoundation.
    AvFoundation,
    /// Windows Media Foundation.
    MediaFoundation,
    /// Deterministic synthetic backend used by tests and by downstream crates
    /// that need a camera-shaped source without a camera.
    Mock,
    /// No capture backend is compiled in for the current target.
    Unsupported,
}

impl BackendKind {
    /// Stable lower-case identifier, suitable for logs and device ids.
    pub const fn name(self) -> &'static str {
        match self {
            Self::V4l2 => "v4l2",
            Self::AvFoundation => "avfoundation",
            Self::MediaFoundation => "mediafoundation",
            Self::Mock => "mock",
            Self::Unsupported => "unsupported",
        }
    }

    /// `true` when this backend can actually open a device.
    pub const fn is_available(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ── Encoding ─────────────────────────────────────────────────────────────────

/// How a captured frame arrives from the device.
///
/// This is deliberately **not** [`PixelFormat`]. MJPEG is a bitstream, not a
/// pixel layout: there is no stride, no plane count and no bytes-per-pixel to
/// speak of, so folding it into `PixelFormat` would force every consumer of
/// that enum to handle a value that answers none of its questions. Modelling
/// the distinction here keeps `PixelFormat` meaning exactly "memory layout of
/// decoded samples".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureEncoding {
    /// Uncompressed samples in the given pixel layout.
    Raw(PixelFormat),
    /// Motion-JPEG: one independently coded JPEG image per frame.
    Mjpeg,
}

impl CaptureEncoding {
    /// The pixel layout, when this encoding *is* a pixel layout.
    ///
    /// Returns `None` for compressed encodings.
    pub const fn pixel(self) -> Option<PixelFormat> {
        match self {
            Self::Raw(pixel) => Some(pixel),
            Self::Mjpeg => None,
        }
    }

    /// `true` when frames arrive as a compressed bitstream that must be
    /// decoded before the samples are usable.
    pub const fn is_compressed(self) -> bool {
        matches!(self, Self::Mjpeg)
    }
}

impl fmt::Display for CaptureEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw(pixel) => write!(f, "{pixel:?}"),
            Self::Mjpeg => f.write_str("MJPEG"),
        }
    }
}

// ── Format ───────────────────────────────────────────────────────────────────

/// One concrete mode a device can be driven in.
///
/// The frame rate is kept as an exact rational (`fps_num / fps_den`) because
/// that is what every capture API reports, and because rounding 30000/1001 to
/// `29.97` before comparison makes format selection non-deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureFormat {
    /// Wire encoding of the delivered frames.
    pub encoding: CaptureEncoding,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame-rate numerator, in frames.
    pub fps_num: u32,
    /// Frame-rate denominator, in seconds. Zero means "unknown / variable".
    pub fps_den: u32,
}

impl CaptureFormat {
    /// Build a format descriptor.
    pub const fn new(
        encoding: CaptureEncoding,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
    ) -> Self {
        Self {
            encoding,
            width,
            height,
            fps_num,
            fps_den,
        }
    }

    /// Frame rate as a floating-point value.
    ///
    /// Returns `0.0` when `fps_den` is zero (variable or unreported frame
    /// rate). It never returns `NaN` or an infinity, so the result is always
    /// safe to compare and to feed into a sort key.
    pub fn fps(self) -> f64 {
        if self.fps_den == 0 {
            0.0
        } else {
            f64::from(self.fps_num) / f64::from(self.fps_den)
        }
    }

    /// The pixel layout, or `None` for compressed encodings.
    pub const fn pixel(self) -> Option<PixelFormat> {
        self.encoding.pixel()
    }

    /// Pixel count of one frame, widened so 8K never overflows.
    pub const fn area(self) -> u64 {
        (self.width as u64) * (self.height as u64)
    }

    /// Exact rational frame-rate comparison against another format.
    ///
    /// Compares `self.fps_num * other.fps_den` against
    /// `other.fps_num * self.fps_den` in 64-bit arithmetic, which is exact for
    /// every plausible camera frame rate and avoids the float rounding that
    /// makes `30000/1001` and `29.97` compare unequal in one direction and
    /// equal in the other.
    pub fn cmp_fps(self, other: Self) -> std::cmp::Ordering {
        let lhs = u64::from(self.fps_num) * u64::from(other.fps_den);
        let rhs = u64::from(other.fps_num) * u64::from(self.fps_den);
        lhs.cmp(&rhs)
    }
}

impl fmt::Display for CaptureFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{}@{:.3} {}",
            self.width,
            self.height,
            self.fps(),
            self.encoding
        )
    }
}

// ── Device ───────────────────────────────────────────────────────────────────

/// A capture device discovered by a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureDevice {
    /// Stable, platform-specific identifier.
    ///
    /// The exact shape depends on the backend, and callers should treat it as
    /// an opaque string:
    ///
    /// * **Linux / V4L2** — the device node path, e.g. `/dev/video0`.
    /// * **macOS / AVFoundation** — the `AVCaptureDevice` `uniqueID`, e.g.
    ///   `0x8020000005ac8514`.
    /// * **Windows / Media Foundation** — the device symbolic link, e.g.
    ///   `\\?\usb#vid_046d&pid_0825&mi_00#...`.
    /// * **Mock** — `mock:<n>`.
    ///
    /// Ids are stable across enumerations for as long as the device stays
    /// plugged in; indices are not, which is why [`DeviceSelector::Id`] is the
    /// right choice for anything that has to survive a re-plug.
    pub id: String,
    /// Human-readable product name, as reported by the device.
    pub name: String,
    /// Every mode this device advertises, in enumeration order.
    ///
    /// Enumeration order is preserved because [`crate::negotiate()`] uses it as
    /// its final tie-break, which is what makes selection reproducible.
    pub formats: Vec<CaptureFormat>,
    /// Backend that produced this entry.
    pub backend: BackendKind,
}

impl fmt::Display for CaptureDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}, {} format(s), {})",
            self.name,
            self.id,
            self.formats.len(),
            self.backend
        )
    }
}

// ── Selector ─────────────────────────────────────────────────────────────────

/// How [`crate::open`] picks one device out of the enumerated list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeviceSelector {
    /// The first device the backend enumerates.
    #[default]
    Default,
    /// Exact match on [`CaptureDevice::id`].
    Id(String),
    /// Zero-based position in the enumeration, bounds-checked.
    ///
    /// Indices are *not* stable across re-plug events; prefer
    /// [`DeviceSelector::Id`] for long-lived configuration.
    Index(usize),
}

impl fmt::Display for DeviceSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default device"),
            Self::Id(id) => write!(f, "device id {id:?}"),
            Self::Index(index) => write!(f, "device index {index}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_names_are_stable() {
        assert_eq!(BackendKind::V4l2.name(), "v4l2");
        assert_eq!(BackendKind::AvFoundation.name(), "avfoundation");
        assert_eq!(BackendKind::MediaFoundation.name(), "mediafoundation");
        assert_eq!(BackendKind::Mock.name(), "mock");
        assert_eq!(BackendKind::Unsupported.name(), "unsupported");
    }

    #[test]
    fn only_unsupported_is_unavailable() {
        assert!(BackendKind::V4l2.is_available());
        assert!(BackendKind::Mock.is_available());
        assert!(!BackendKind::Unsupported.is_available());
    }

    #[test]
    fn raw_encoding_exposes_pixel_format() {
        let enc = CaptureEncoding::Raw(PixelFormat::Nv12);
        assert_eq!(enc.pixel(), Some(PixelFormat::Nv12));
        assert!(!enc.is_compressed());
    }

    #[test]
    fn mjpeg_has_no_pixel_format() {
        assert_eq!(CaptureEncoding::Mjpeg.pixel(), None);
        assert!(CaptureEncoding::Mjpeg.is_compressed());
    }

    #[test]
    fn fps_is_zero_when_denominator_is_zero() {
        let fmt = CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 0);
        assert_eq!(fmt.fps(), 0.0);
        assert!(fmt.fps().is_finite());
    }

    #[test]
    fn fps_handles_ntsc_rational() {
        let fmt = CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 30000, 1001);
        assert!((fmt.fps() - 29.970_029_97).abs() < 1e-6);
    }

    #[test]
    fn area_widens_to_u64() {
        let fmt = CaptureFormat::new(CaptureEncoding::Mjpeg, 7680, 4320, 60, 1);
        assert_eq!(fmt.area(), 7680 * 4320);
    }

    #[test]
    fn cmp_fps_is_exact_for_ntsc_rates() {
        let ntsc = CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30000, 1001);
        let thirty = CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 1);
        let also_ntsc = CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 60000, 2002);
        assert_eq!(ntsc.cmp_fps(thirty), std::cmp::Ordering::Less);
        assert_eq!(thirty.cmp_fps(ntsc), std::cmp::Ordering::Greater);
        assert_eq!(ntsc.cmp_fps(also_ntsc), std::cmp::Ordering::Equal);
    }

    #[test]
    fn format_display_is_readable() {
        let fmt = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 1920, 1080, 30, 1);
        let text = fmt.to_string();
        assert!(text.contains("1920x1080"), "{text}");
        assert!(text.contains("Nv12"), "{text}");
    }

    #[test]
    fn selector_default_is_the_default_variant() {
        assert_eq!(DeviceSelector::default(), DeviceSelector::Default);
    }

    #[test]
    fn selector_display_distinguishes_variants() {
        assert!(DeviceSelector::Default.to_string().contains("default"));
        assert!(DeviceSelector::Id("/dev/video0".into())
            .to_string()
            .contains("/dev/video0"));
        assert!(DeviceSelector::Index(3).to_string().contains('3'));
    }

    #[test]
    fn device_display_reports_format_count() {
        let device = CaptureDevice {
            id: "mock:0".into(),
            name: "Mock Camera".into(),
            formats: vec![CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 1)],
            backend: BackendKind::Mock,
        };
        let text = device.to_string();
        assert!(text.contains("Mock Camera"), "{text}");
        assert!(text.contains("1 format"), "{text}");
    }
}

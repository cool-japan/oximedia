//! Video frame types.

use oximedia_core::{PixelFormat, Rational, Timestamp};

/// Decoded video frame.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// Pixel format.
    pub format: PixelFormat,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Plane data.
    pub planes: Vec<Plane>,
    /// Presentation timestamp.
    pub timestamp: Timestamp,
    /// Frame type (I/P/B).
    pub frame_type: FrameType,
    /// Color information.
    pub color_info: ColorInfo,
    /// Frame is corrupt (concealment applied).
    pub corrupt: bool,
}

impl VideoFrame {
    /// Create a new video frame.
    #[must_use]
    pub fn new(format: PixelFormat, width: u32, height: u32) -> Self {
        Self {
            format,
            width,
            height,
            planes: Vec::new(),
            timestamp: Timestamp::new(0, Rational::new(1, 1000)),
            frame_type: FrameType::Key,
            color_info: ColorInfo::default(),
            corrupt: false,
        }
    }

    /// Allocate planes for the frame format.
    ///
    /// Row stride is normally the plane width (historical behavior,
    /// preserved exactly for every pre-existing format to avoid a
    /// regression). Two families instead consult
    /// [`PixelFormat::stride_for_width`] so the buffer is sized correctly:
    /// packed formats that pack more than one byte per pixel into a single
    /// plane ([`PixelFormat::Yuyv422`] / [`PixelFormat::Uyvy422`], both
    /// 2 bytes/pixel 4:2:2), and the interleaved-UV chroma plane of the
    /// 8-bit semi-planar formats ([`PixelFormat::Nv12`] /
    /// [`PixelFormat::Nv21`]), whose byte width is the full luma width --
    /// sizing it from the chroma *pixel* dimensions under-allocated it 2x.
    /// (The 16-bit semi-planar formats `P010`/`P016` still take the legacy
    /// path: their planes were equally under-sized, but no in-tree consumer
    /// allocates them yet and resizing them here would silently change any
    /// downstream code that has compensated for the historical behaviour.)
    pub fn allocate(&mut self) {
        let plane_count = self.format.plane_count();
        self.planes.clear();

        for i in 0..plane_count {
            let (width, height) = self.plane_dimensions(i as usize);
            let use_stride_table =
                matches!(self.format, PixelFormat::Yuyv422 | PixelFormat::Uyvy422)
                    || (matches!(self.format, PixelFormat::Nv12 | PixelFormat::Nv21) && i == 1);
            let stride = if use_stride_table {
                self.format
                    .stride_for_width(self.width, i)
                    .unwrap_or(width as usize)
            } else {
                width as usize
            };
            let size = stride * height as usize;
            let data = vec![0u8; size];

            self.planes.push(Plane {
                data,
                stride,
                width,
                height,
            });
        }
    }

    /// Get plane dimensions.
    ///
    /// Dimensions are in pixels, not bytes — for packed multi-byte-per-pixel
    /// formats (e.g. [`PixelFormat::Yuyv422`], [`PixelFormat::Uyvy422`]) the
    /// extra bytes per pixel are accounted for in the plane's `stride`
    /// (see [`Self::allocate`]), not in these dimensions.
    #[must_use]
    pub fn plane_dimensions(&self, plane_index: usize) -> (u32, u32) {
        let (h_ratio, v_ratio) = self.format.chroma_subsampling();

        if plane_index == 0 {
            // Luma plane
            (self.width, self.height)
        } else {
            // Chroma planes - use ratio directly for division
            let chroma_width = self.width.div_ceil(h_ratio);
            let chroma_height = self.height.div_ceil(v_ratio);
            (chroma_width, chroma_height)
        }
    }

    /// Get total frame size in bytes.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.planes.iter().map(|p| p.data.len()).sum()
    }

    /// Check if frame is a keyframe.
    #[must_use]
    pub fn is_keyframe(&self) -> bool {
        self.frame_type == FrameType::Key
    }

    /// Get reference to plane by index.
    #[must_use]
    pub fn plane(&self, index: usize) -> &Plane {
        &self.planes[index]
    }

    /// Get mutable reference to plane by index.
    #[must_use]
    pub fn plane_mut(&mut self, index: usize) -> &mut Plane {
        &mut self.planes[index]
    }
}

/// Single plane of video data.
#[derive(Clone, Debug)]
pub struct Plane {
    /// Pixel data.
    pub data: Vec<u8>,
    /// Row stride in bytes.
    pub stride: usize,
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
}

impl Plane {
    /// Create a new plane.
    #[must_use]
    pub fn new(data: Vec<u8>, stride: usize) -> Self {
        Self {
            data,
            stride,
            width: 0,
            height: 0,
        }
    }

    /// Create a new plane with width and height.
    #[must_use]
    pub fn with_dimensions(data: Vec<u8>, stride: usize, width: u32, height: u32) -> Self {
        Self {
            data,
            stride,
            width,
            height,
        }
    }

    /// Get row at given y coordinate.
    #[must_use]
    pub fn row(&self, y: usize) -> &[u8] {
        let start = y * self.stride;
        let end = start + self.stride;
        if end <= self.data.len() {
            &self.data[start..end]
        } else {
            &[]
        }
    }

    /// Get plane width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Get plane height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Get plane stride.
    #[must_use]
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Get immutable reference to plane data.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get mutable reference to plane data.
    #[must_use]
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

/// Frame type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameType {
    /// Keyframe (I-frame) - can be decoded independently.
    #[default]
    Key,
    /// Inter frame (P-frame) - references previous frames.
    Inter,
    /// Bidirectional frame (B-frame) - references past and future.
    BiDir,
    /// Switch frame - allows stream switching.
    Switch,
}

impl FrameType {
    /// Check if this is a reference frame.
    #[must_use]
    pub fn is_reference(&self) -> bool {
        matches!(self, Self::Key | Self::Inter)
    }
}

/// Color information for video frames.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColorInfo {
    /// Color primaries.
    pub primaries: ColorPrimaries,
    /// Transfer characteristics.
    pub transfer: TransferCharacteristics,
    /// Matrix coefficients.
    pub matrix: MatrixCoefficients,
    /// Full range (0-255) vs limited range (16-235).
    pub full_range: bool,
}

/// Color primaries (ITU-T H.273).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorPrimaries {
    /// BT.709 (sRGB, HD).
    #[default]
    Bt709 = 1,
    /// Unspecified.
    Unspecified = 2,
    /// BT.470M (obsolete NTSC).
    Bt470M = 4,
    /// BT.470BG (PAL/SECAM).
    Bt470Bg = 5,
    /// SMPTE 170M (NTSC).
    Smpte170M = 6,
    /// SMPTE 240M.
    Smpte240M = 7,
    /// Generic film.
    Film = 8,
    /// BT.2020 (UHD).
    Bt2020 = 9,
    /// SMPTE ST 428-1.
    Smpte428 = 10,
    /// SMPTE RP 431-2.
    Smpte431 = 11,
    /// SMPTE EG 432-1 (P3).
    Smpte432 = 12,
    /// EBU Tech 3213-E.
    Ebu3213 = 22,
}

/// Transfer characteristics (ITU-T H.273).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferCharacteristics {
    /// BT.709.
    #[default]
    Bt709 = 1,
    /// Unspecified.
    Unspecified = 2,
    /// BT.470M.
    Bt470M = 4,
    /// BT.470BG.
    Bt470Bg = 5,
    /// SMPTE 170M.
    Smpte170M = 6,
    /// SMPTE 240M.
    Smpte240M = 7,
    /// Linear.
    Linear = 8,
    /// Logarithmic 100:1.
    Log100 = 9,
    /// Logarithmic 100*sqrt(10):1.
    Log316 = 10,
    /// IEC 61966-2-4.
    Iec619662_4 = 11,
    /// BT.1361.
    Bt1361 = 12,
    /// sRGB/sYCC.
    Srgb = 13,
    /// BT.2020 10-bit.
    Bt202010 = 14,
    /// BT.2020 12-bit.
    Bt202012 = 15,
    /// SMPTE ST 2084 (PQ/HDR10).
    Smpte2084 = 16,
    /// SMPTE ST 428-1.
    Smpte428 = 17,
    /// ARIB STD-B67 (HLG).
    AribStdB67 = 18,
}

/// Matrix coefficients (ITU-T H.273).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatrixCoefficients {
    /// Identity (RGB).
    Identity = 0,
    /// BT.709.
    #[default]
    Bt709 = 1,
    /// Unspecified.
    Unspecified = 2,
    /// FCC.
    Fcc = 4,
    /// BT.470BG.
    Bt470Bg = 5,
    /// SMPTE 170M.
    Smpte170M = 6,
    /// SMPTE 240M.
    Smpte240M = 7,
    /// `YCgCo`.
    Ycgco = 8,
    /// BT.2020 non-constant.
    Bt2020Ncl = 9,
    /// BT.2020 constant.
    Bt2020Cl = 10,
    /// SMPTE 2085.
    Smpte2085 = 11,
    /// Chromaticity-derived non-constant.
    ChromaDerivedNcl = 12,
    /// Chromaticity-derived constant.
    ChromaDerivedCl = 13,
    /// `ICtCp`.
    Ictcp = 14,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_frame_new() {
        let frame = VideoFrame::new(PixelFormat::Yuv420p, 1920, 1080);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.format, PixelFormat::Yuv420p);
    }

    #[test]
    fn test_plane_dimensions() {
        let frame = VideoFrame::new(PixelFormat::Yuv420p, 1920, 1080);
        assert_eq!(frame.plane_dimensions(0), (1920, 1080));
        assert_eq!(frame.plane_dimensions(1), (960, 540));
        assert_eq!(frame.plane_dimensions(2), (960, 540));
    }

    #[test]
    fn test_frame_allocate() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 1920, 1080);
        frame.allocate();
        assert_eq!(frame.planes.len(), 3);
        // Y: 1920 * 1080, U: 960 * 540, V: 960 * 540
        assert_eq!(frame.planes[0].data.len(), 1920 * 1080);
        assert_eq!(frame.planes[1].data.len(), 960 * 540);
    }

    /// Regression: pre-existing formats must allocate identically to
    /// before the packed-4:2:2 stride fix was introduced.
    #[test]
    fn test_frame_allocate_yuv420p_64x48_unchanged() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 48);
        frame.allocate();
        assert_eq!(frame.planes.len(), 3);
        assert_eq!(frame.planes[0].data.len(), 64 * 48); // 3072
        assert_eq!(frame.planes[1].data.len(), 32 * 24); // 768
        assert_eq!(frame.planes[2].data.len(), 32 * 24); // 768
        assert_eq!(frame.planes[0].stride, 64);
        assert_eq!(frame.planes[1].stride, 32);
        assert_eq!(frame.planes[2].stride, 32);
    }

    /// Regression for the packed 4:2:2 allocation bug: the old
    /// `stride = width` computation allocated half the bytes actually
    /// needed (2 bytes/pixel packed into one plane).
    #[test]
    fn test_frame_allocate_yuyv422_packed_stride() {
        let mut frame = VideoFrame::new(PixelFormat::Yuyv422, 64, 48);
        frame.allocate();
        assert_eq!(frame.planes.len(), 1);
        assert_eq!(frame.planes[0].data.len(), 64 * 48 * 2);
        assert_eq!(frame.planes[0].data.len(), 6144);
        assert_eq!(frame.planes[0].stride, 128);
        assert_eq!(
            frame.size_bytes(),
            PixelFormat::Yuyv422.frame_buffer_size(64, 48)
        );
    }

    #[test]
    fn test_frame_allocate_semi_planar_chroma_full_byte_width() {
        // Regression: the interleaved UV plane of NV12/NV21 is `width` BYTES
        // wide (2 components x width/2 samples) at half height. Sizing it
        // from the chroma pixel dimensions under-allocated it 2x, which
        // surfaced as malformed NV12 frames from oximedia-capture's mock
        // backend (the only in-tree caller that allocates semi-planar
        // frames through this path).
        for format in [PixelFormat::Nv12, PixelFormat::Nv21] {
            let mut frame = VideoFrame::new(format, 64, 48);
            frame.allocate();
            assert_eq!(frame.planes.len(), 2, "{format:?}");
            // Luma: unchanged legacy sizing.
            assert_eq!(frame.planes[0].stride, 64, "{format:?}");
            assert_eq!(frame.planes[0].data.len(), 64 * 48, "{format:?}");
            // Chroma: full luma byte width at half height.
            assert_eq!(frame.planes[1].stride, 64, "{format:?}");
            assert_eq!(frame.planes[1].data.len(), 64 * 24, "{format:?}");
            assert_eq!(
                frame.size_bytes(),
                format.frame_buffer_size(64, 48),
                "{format:?}"
            );
        }
    }

    #[test]
    fn test_frame_allocate_uyvy422_packed_stride() {
        let mut frame = VideoFrame::new(PixelFormat::Uyvy422, 64, 48);
        frame.allocate();
        assert_eq!(frame.planes.len(), 1);
        assert_eq!(frame.planes[0].data.len(), 6144);
        assert_eq!(frame.planes[0].stride, 128);
        assert_eq!(
            frame.size_bytes(),
            PixelFormat::Uyvy422.frame_buffer_size(64, 48)
        );
    }

    #[test]
    fn test_frame_type() {
        assert!(FrameType::Key.is_reference());
        assert!(FrameType::Inter.is_reference());
        assert!(!FrameType::BiDir.is_reference());
    }
}

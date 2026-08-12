//! VP8 macroblock modes and partition types.
//!
//! # Deprecated: defective early seed, not RFC 6386-accurate
//!
//! **Every public item in this module is `#[deprecated]`** (since 0.2.1,
//! scheduled for removal in 0.3.0). These enums are an early, H.264-shaped
//! mode taxonomy that predates VP8 inter-frame decode landing in this
//! crate: they do not represent RFC 6386's real near/nearest/zero/new-MV
//! survey (SS16.3) or SPLITMV sub-partitioning (SS16), and nothing in the
//! decoder uses them. The real, bit-exact decode pipeline behind
//! `Vp8Decoder` has its own private mode representation in `vp8::dec::mode`
//! and `vp8::dec::mv`. Kept only for API stability.
//!
//! This module defines the various prediction modes and partition types
//! used in VP8. VP8 operates on 16x16 macroblocks which can be predicted
//! in different ways:
//!
//! - **Intra Prediction**: Predicted from reconstructed pixels in the current frame
//!   - I16 modes: 16x16 prediction for the entire macroblock
//!   - I4 modes: 4x4 prediction for individual sub-blocks
//!   - Chroma modes: Prediction for chroma planes
//!
//! - **Inter Prediction**: Predicted from reference frames using motion vectors
//!   - Various partitioning: 16x16, 16x8, 8x16, 8x8, etc.

#![allow(dead_code)]

/// Number of I16 (16x16 intra) modes.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
pub const NUM_I16_MODES: usize = 4;

/// Number of I4 (4x4 intra) modes.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
pub const NUM_I4_MODES: usize = 10;

/// Number of chroma intra modes.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
pub const NUM_CHROMA_MODES: usize = 4;

/// Number of inter motion vector modes.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
pub const NUM_MV_MODES: usize = 4;

/// 16x16 intra prediction mode.
///
/// These modes predict the entire 16x16 luma macroblock at once.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// `derive(Default)`'s generated `impl Default` names the `#[default]`
// variant directly; that generated code is not covered by any attribute on
// the variant itself, only by one on the enum item. Internal reference site.
#[allow(deprecated)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum IntraMode16 {
    /// DC prediction - average of neighboring pixels.
    #[default]
    DcPred = 0,
    /// Vertical prediction - copy top pixels downward.
    VPred = 1,
    /// Horizontal prediction - copy left pixels rightward.
    HPred = 2,
    /// True motion prediction (planar).
    TmPred = 3,
}

// Internal reference site: this impl block's own methods necessarily name
// `IntraMode16` and its variants, which are deprecated above; the impl
// itself is not deprecated (see the module doc's policy on inherent
// methods), so it needs its own targeted allow.
#[allow(deprecated)]
impl IntraMode16 {
    /// Converts from u8 to `IntraMode16`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::DcPred),
            1 => Some(Self::VPred),
            2 => Some(Self::HPred),
            3 => Some(Self::TmPred),
            _ => None,
        }
    }

    /// Returns the mode index.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// 4x4 intra prediction mode.
///
/// These modes predict individual 4x4 sub-blocks within a macroblock.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// See `IntraMode16`'s identical note on `derive(Default)` above.
#[allow(deprecated)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum IntraMode4 {
    /// DC prediction.
    #[default]
    DcPred = 0,
    /// True motion prediction.
    TmPred = 1,
    /// Vertical prediction.
    VPred = 2,
    /// Horizontal prediction.
    HPred = 3,
    /// Diagonal down-left prediction.
    LdPred = 4,
    /// Diagonal down-right prediction.
    RdPred = 5,
    /// Vertical-right prediction.
    VrPred = 6,
    /// Vertical-left prediction.
    VlPred = 7,
    /// Horizontal-down prediction.
    HdPred = 8,
    /// Horizontal-up prediction.
    HuPred = 9,
}

// Internal reference site: see the identical note on `impl IntraMode16` above.
#[allow(deprecated)]
impl IntraMode4 {
    /// All 4x4 intra modes.
    pub const ALL: [Self; NUM_I4_MODES] = [
        Self::DcPred,
        Self::TmPred,
        Self::VPred,
        Self::HPred,
        Self::LdPred,
        Self::RdPred,
        Self::VrPred,
        Self::VlPred,
        Self::HdPred,
        Self::HuPred,
    ];

    /// Converts from u8 to `IntraMode4`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::DcPred),
            1 => Some(Self::TmPred),
            2 => Some(Self::VPred),
            3 => Some(Self::HPred),
            4 => Some(Self::LdPred),
            5 => Some(Self::RdPred),
            6 => Some(Self::VrPred),
            7 => Some(Self::VlPred),
            8 => Some(Self::HdPred),
            9 => Some(Self::HuPred),
            _ => None,
        }
    }

    /// Returns the mode index.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Returns whether this is a directional mode.
    #[must_use]
    pub const fn is_directional(self) -> bool {
        !matches!(self, Self::DcPred | Self::TmPred)
    }
}

/// Chroma intra prediction mode.
///
/// Used for predicting the U and V chroma planes.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// See `IntraMode16`'s identical note on `derive(Default)` above.
#[allow(deprecated)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ChromaMode {
    /// DC prediction.
    #[default]
    DcPred = 0,
    /// Vertical prediction.
    VPred = 1,
    /// Horizontal prediction.
    HPred = 2,
    /// True motion prediction.
    TmPred = 3,
}

// Internal reference site: see the identical note on `impl IntraMode16` above.
#[allow(deprecated)]
impl ChromaMode {
    /// Converts from u8 to `ChromaMode`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::DcPred),
            1 => Some(Self::VPred),
            2 => Some(Self::HPred),
            3 => Some(Self::TmPred),
            _ => None,
        }
    }

    /// Returns the mode index.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Inter prediction mode (motion vector mode).
///
/// Defines how motion vectors are obtained for inter-predicted macroblocks.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// See `IntraMode16`'s identical note on `derive(Default)` above.
#[allow(deprecated)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum InterMode {
    /// Use motion vector from the nearest neighbor.
    #[default]
    Nearest = 0,
    /// Use motion vector from nearby blocks.
    Near = 1,
    /// Zero motion vector (reference current position).
    Zero = 2,
    /// New motion vector (explicitly coded).
    New = 3,
}

// Internal reference site: see the identical note on `impl IntraMode16` above.
#[allow(deprecated)]
impl InterMode {
    /// Converts from u8 to `InterMode`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Nearest),
            1 => Some(Self::Near),
            2 => Some(Self::Zero),
            3 => Some(Self::New),
            _ => None,
        }
    }

    /// Returns the mode index.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Returns whether this mode requires a motion vector in the bitstream.
    #[must_use]
    pub const fn needs_mv(self) -> bool {
        matches!(self, Self::New)
    }
}

/// Macroblock partition type.
///
/// Defines how a 16x16 macroblock is partitioned for prediction.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// See `IntraMode16`'s identical note on `derive(Default)` above.
#[allow(deprecated)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PartitionType {
    /// 16x16 partition (entire macroblock).
    #[default]
    P16x16 = 0,
    /// Two 16x8 partitions (horizontal split).
    P16x8 = 1,
    /// Two 8x16 partitions (vertical split).
    P8x16 = 2,
    /// Four 8x8 partitions.
    P8x8 = 3,
}

// Internal reference site: see the identical note on `impl IntraMode16` above.
#[allow(deprecated)]
impl PartitionType {
    /// Converts from u8 to `PartitionType`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::P16x16),
            1 => Some(Self::P16x8),
            2 => Some(Self::P8x16),
            3 => Some(Self::P8x8),
            _ => None,
        }
    }

    /// Returns the number of partitions.
    #[must_use]
    pub const fn num_partitions(self) -> usize {
        match self {
            Self::P16x16 => 1,
            Self::P16x8 | Self::P8x16 => 2,
            Self::P8x8 => 4,
        }
    }

    /// Returns the dimensions of a partition (width, height).
    #[must_use]
    pub const fn partition_size(self, index: usize) -> (usize, usize) {
        match self {
            Self::P16x16 => (16, 16),
            Self::P16x8 => (16, 8),
            Self::P8x16 => (8, 16),
            Self::P8x8 => (8, 8),
        }
    }

    /// Returns the position of a partition within the macroblock (x, y).
    #[must_use]
    pub const fn partition_offset(self, index: usize) -> (usize, usize) {
        match self {
            Self::P16x16 => (0, 0),
            Self::P16x8 => match index {
                0 => (0, 0),
                1 => (0, 8),
                _ => (0, 0),
            },
            Self::P8x16 => match index {
                0 => (0, 0),
                1 => (8, 0),
                _ => (0, 0),
            },
            Self::P8x8 => match index {
                0 => (0, 0),
                1 => (8, 0),
                2 => (0, 8),
                3 => (8, 8),
                _ => (0, 0),
            },
        }
    }
}

/// Reference frame type.
///
/// VP8 uses up to 3 reference frames for inter prediction.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// See `IntraMode16`'s identical note on `derive(Default)` above.
#[allow(deprecated)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum RefFrame {
    /// Last decoded frame.
    #[default]
    Last = 0,
    /// Golden reference frame.
    Golden = 1,
    /// Alternate reference frame.
    AltRef = 2,
}

// Internal reference site: see the identical note on `impl IntraMode16` above.
#[allow(deprecated)]
impl RefFrame {
    /// Converts from u8 to `RefFrame`.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Last),
            1 => Some(Self::Golden),
            2 => Some(Self::AltRef),
            _ => None,
        }
    }

    /// Returns the frame index.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Macroblock type.
///
/// Combines prediction mode and partitioning information.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (H.264-shaped mode taxonomy, not RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// Its own variant fields name the other deprecated enums in this module;
// internal reference site, see the note on `impl IntraMode16` above.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroblockType {
    /// Intra macroblock with 16x16 prediction.
    Intra16(IntraMode16, ChromaMode),
    /// Intra macroblock with 4x4 prediction.
    Intra4 {
        /// 4x4 modes for each of the 16 sub-blocks.
        modes: [IntraMode4; 16],
        /// Chroma prediction mode.
        chroma_mode: ChromaMode,
    },
    /// Inter macroblock.
    Inter {
        /// Partition type.
        partition: PartitionType,
        /// Reference frame.
        ref_frame: RefFrame,
        /// Inter mode for each partition.
        modes: [InterMode; 4],
    },
}

// Internal reference site: see the identical note on `impl IntraMode16` above.
#[allow(deprecated)]
impl MacroblockType {
    /// Returns whether this is an intra macroblock.
    #[must_use]
    pub const fn is_intra(&self) -> bool {
        matches!(self, Self::Intra16(_, _) | Self::Intra4 { .. })
    }

    /// Returns whether this is an inter macroblock.
    #[must_use]
    pub const fn is_inter(&self) -> bool {
        matches!(self, Self::Inter { .. })
    }
}

#[cfg(test)]
// This module's entire purpose is exercising the deprecated types declared
// above; targeted (not blanket/crate-wide) allow for exactly that site.
#[allow(deprecated)]
mod tests {
    use super::*;

    #[test]
    fn test_intra_mode_16() {
        assert_eq!(IntraMode16::from_u8(0), Some(IntraMode16::DcPred));
        assert_eq!(IntraMode16::from_u8(3), Some(IntraMode16::TmPred));
        assert_eq!(IntraMode16::from_u8(4), None);

        assert_eq!(IntraMode16::DcPred.index(), 0);
        assert_eq!(IntraMode16::TmPred.index(), 3);
    }

    #[test]
    fn test_intra_mode_4() {
        assert_eq!(IntraMode4::from_u8(0), Some(IntraMode4::DcPred));
        assert_eq!(IntraMode4::from_u8(9), Some(IntraMode4::HuPred));
        assert_eq!(IntraMode4::from_u8(10), None);

        assert!(!IntraMode4::DcPred.is_directional());
        assert!(!IntraMode4::TmPred.is_directional());
        assert!(IntraMode4::VPred.is_directional());
        assert!(IntraMode4::HuPred.is_directional());

        assert_eq!(IntraMode4::ALL.len(), NUM_I4_MODES);
    }

    #[test]
    fn test_chroma_mode() {
        assert_eq!(ChromaMode::from_u8(0), Some(ChromaMode::DcPred));
        assert_eq!(ChromaMode::from_u8(3), Some(ChromaMode::TmPred));
        assert_eq!(ChromaMode::from_u8(4), None);

        assert_eq!(ChromaMode::VPred.index(), 1);
    }

    #[test]
    fn test_inter_mode() {
        assert_eq!(InterMode::from_u8(0), Some(InterMode::Nearest));
        assert_eq!(InterMode::from_u8(3), Some(InterMode::New));
        assert_eq!(InterMode::from_u8(4), None);

        assert!(!InterMode::Nearest.needs_mv());
        assert!(!InterMode::Zero.needs_mv());
        assert!(InterMode::New.needs_mv());
    }

    #[test]
    fn test_partition_type() {
        assert_eq!(PartitionType::from_u8(0), Some(PartitionType::P16x16));
        assert_eq!(PartitionType::from_u8(3), Some(PartitionType::P8x8));
        assert_eq!(PartitionType::from_u8(4), None);

        assert_eq!(PartitionType::P16x16.num_partitions(), 1);
        assert_eq!(PartitionType::P16x8.num_partitions(), 2);
        assert_eq!(PartitionType::P8x8.num_partitions(), 4);
    }

    #[test]
    fn test_partition_size() {
        assert_eq!(PartitionType::P16x16.partition_size(0), (16, 16));
        assert_eq!(PartitionType::P16x8.partition_size(0), (16, 8));
        assert_eq!(PartitionType::P8x16.partition_size(0), (8, 16));
        assert_eq!(PartitionType::P8x8.partition_size(0), (8, 8));
    }

    #[test]
    fn test_partition_offset() {
        assert_eq!(PartitionType::P16x16.partition_offset(0), (0, 0));

        assert_eq!(PartitionType::P16x8.partition_offset(0), (0, 0));
        assert_eq!(PartitionType::P16x8.partition_offset(1), (0, 8));

        assert_eq!(PartitionType::P8x16.partition_offset(0), (0, 0));
        assert_eq!(PartitionType::P8x16.partition_offset(1), (8, 0));

        assert_eq!(PartitionType::P8x8.partition_offset(0), (0, 0));
        assert_eq!(PartitionType::P8x8.partition_offset(1), (8, 0));
        assert_eq!(PartitionType::P8x8.partition_offset(2), (0, 8));
        assert_eq!(PartitionType::P8x8.partition_offset(3), (8, 8));
    }

    #[test]
    fn test_ref_frame() {
        assert_eq!(RefFrame::from_u8(0), Some(RefFrame::Last));
        assert_eq!(RefFrame::from_u8(2), Some(RefFrame::AltRef));
        assert_eq!(RefFrame::from_u8(3), None);

        assert_eq!(RefFrame::Last.index(), 0);
        assert_eq!(RefFrame::Golden.index(), 1);
        assert_eq!(RefFrame::AltRef.index(), 2);
    }

    #[test]
    fn test_macroblock_type() {
        let intra16 = MacroblockType::Intra16(IntraMode16::DcPred, ChromaMode::DcPred);
        assert!(intra16.is_intra());
        assert!(!intra16.is_inter());

        let intra4 = MacroblockType::Intra4 {
            modes: [IntraMode4::DcPred; 16],
            chroma_mode: ChromaMode::DcPred,
        };
        assert!(intra4.is_intra());
        assert!(!intra4.is_inter());

        let inter = MacroblockType::Inter {
            partition: PartitionType::P16x16,
            ref_frame: RefFrame::Last,
            modes: [InterMode::Zero; 4],
        };
        assert!(!inter.is_intra());
        assert!(inter.is_inter());
    }

    #[test]
    fn test_constants() {
        assert_eq!(NUM_I16_MODES, 4);
        assert_eq!(NUM_I4_MODES, 10);
        assert_eq!(NUM_CHROMA_MODES, 4);
        assert_eq!(NUM_MV_MODES, 4);
    }
}

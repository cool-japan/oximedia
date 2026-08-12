//! VP9 Uncompressed header parsing.

#![allow(clippy::match_same_arms)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::if_not_else)]

use crate::error::{CodecError, CodecResult};
use oximedia_io::BitReader;

/// VP9 frame types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Vp9FrameType {
    /// Keyframe (intra-only).
    #[default]
    Key = 0,
    /// Inter frame.
    Inter = 1,
}

impl Vp9FrameType {
    /// Returns true if this is a keyframe.
    #[must_use]
    pub const fn is_keyframe(&self) -> bool {
        matches!(self, Self::Key)
    }
}

/// VP9 color space specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorSpace {
    /// Unknown or unspecified.
    #[default]
    Unknown = 0,
    /// ITU-R BT.601.
    Bt601 = 1,
    /// ITU-R BT.709.
    Bt709 = 2,
    /// SMPTE 170M.
    Smpte170 = 3,
    /// SMPTE 240M.
    Smpte240 = 4,
    /// ITU-R BT.2020.
    Bt2020 = 5,
    /// Reserved.
    Reserved = 6,
    /// sRGB.
    Srgb = 7,
}

impl From<u8> for ColorSpace {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Unknown,
            1 => Self::Bt601,
            2 => Self::Bt709,
            3 => Self::Smpte170,
            4 => Self::Smpte240,
            5 => Self::Bt2020,
            6 => Self::Reserved,
            7 => Self::Srgb,
            _ => Self::Unknown,
        }
    }
}

impl ColorSpace {
    /// Returns the color space name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Bt601 => "bt601",
            Self::Bt709 => "bt709",
            Self::Smpte170 => "smpte170",
            Self::Smpte240 => "smpte240",
            Self::Bt2020 => "bt2020",
            Self::Reserved => "reserved",
            Self::Srgb => "srgb",
        }
    }
}

/// VP9 Uncompressed header.
#[derive(Clone, Debug, Default)]
pub struct UncompressedHeader {
    /// Frame marker (should be 0b10).
    pub frame_marker: u8,
    /// Profile (0-3).
    pub profile: u8,
    /// Show existing frame flag.
    pub show_existing_frame: bool,
    /// Frame to show index.
    pub frame_to_show: u8,
    /// Frame type.
    pub frame_type: Vp9FrameType,
    /// Show frame flag.
    pub show_frame: bool,
    /// Error resilient mode.
    pub error_resilient: bool,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Render width.
    pub render_width: u32,
    /// Render height.
    pub render_height: u32,
    /// Intra-only flag.
    pub intra_only: bool,
    /// Reset frame context.
    pub reset_frame_context: u8,
    /// Refresh frame flags bitmask.
    pub refresh_frame_flags: u8,
    /// Reference frame indices for LAST, GOLDEN, ALTREF.
    pub ref_frame_idx: [u8; 3],
    /// Reference frame sign bias.
    pub ref_frame_sign_bias: [bool; 4],
    /// Allow high precision motion vectors.
    pub allow_high_precision_mv: bool,
    /// Interpolation filter type.
    pub interp_filter: u8,
    /// Color space specification.
    pub color_space: ColorSpace,
    /// Full range color values.
    pub color_range: bool,
    /// Chroma subsampling X.
    pub subsampling_x: bool,
    /// Chroma subsampling Y.
    pub subsampling_y: bool,
    /// Bit depth (8, 10, or 12).
    pub bit_depth: u8,
    /// Refresh frame context flag.
    pub refresh_frame_context: bool,
    /// Frame parallel decoding mode.
    pub frame_parallel_decoding: bool,
    /// Frame context index (0..=3).
    pub frame_context_idx: u8,
    /// Loop filter parameters.
    pub loop_filter: LoopFilterHeader,
    /// Quantization parameters.
    pub quant: QuantHeader,
    /// Segmentation parameters.
    pub seg: SegmentationHeader,
    /// log2 of tile columns.
    pub tile_cols_log2: u8,
    /// log2 of tile rows.
    pub tile_rows_log2: u8,
    /// Which of the three references the frame size was copied from, when
    /// `frame_size_with_refs` signalled `found_ref` (spec §6.2.5; libvpx
    /// `setup_frame_size_with_refs`).
    ///
    /// `Some(i)` means `width`/`height` are the display dimensions of the
    /// reference in slot `ref_frame_idx[i]`, taken from the decoded-picture
    /// buffer passed to [`UncompressedHeader::parse_with_ref_sizes`], not
    /// coded in this frame. `None` means the frame coded its own size.
    pub size_from_ref: Option<usize>,
    /// Size of the compressed header in bytes (`header_size_in_bytes`).
    pub compressed_header_size: u16,
    /// Byte size of the uncompressed header (offset of the compressed
    /// header within the frame payload).
    pub uncompressed_header_bytes: usize,
}

/// Loop filter fields of the uncompressed header (spec `loop_filter_params`).
///
/// The mode/ref deltas are **persistent decoder state**, not per-frame data:
/// libvpx `setup_loopfilter` assigns only the entries a frame individually
/// flags, and only when `delta_enabled && delta_update`, leaving every other
/// entry at whatever the last frame that wrote it left behind. Parsing stays
/// pure — the values here start from the `vp9_setup_past_independence`
/// defaults (`set_default_lf_deltas`: ref `[1, 0, -1, -1]`, mode `[0, 0]`),
/// and [`ref_delta_updated`](Self::ref_delta_updated) /
/// [`mode_delta_updated`](Self::mode_delta_updated) record exactly which
/// entries this frame transmitted, so the decoder can merge them into the
/// persistent set.
#[derive(Clone, Debug)]
pub struct LoopFilterHeader {
    /// Base filter level (0..=63).
    pub filter_level: u8,
    /// Sharpness level (0..=7).
    pub sharpness: u8,
    /// Mode/ref delta enabled flag (`mode_ref_delta_enabled`).
    pub delta_enabled: bool,
    /// Whether this frame transmits any delta update at all
    /// (`mode_ref_delta_update`); always `false` when `delta_enabled` is
    /// clear, since the bit is not coded then.
    pub delta_update: bool,
    /// Reference deltas (INTRA, LAST, GOLDEN, ALTREF).
    pub ref_deltas: [i8; 4],
    /// Mode deltas.
    pub mode_deltas: [i8; 2],
    /// Which reference deltas this frame actually transmitted.
    pub ref_delta_updated: [bool; 4],
    /// Which mode deltas this frame actually transmitted.
    pub mode_delta_updated: [bool; 2],
}

impl Default for LoopFilterHeader {
    fn default() -> Self {
        Self {
            filter_level: 0,
            sharpness: 0,
            delta_enabled: true,
            delta_update: false,
            ref_deltas: [1, 0, -1, -1],
            mode_deltas: [0, 0],
            ref_delta_updated: [false; 4],
            mode_delta_updated: [false; 2],
        }
    }
}

/// Quantization fields of the uncompressed header.
#[derive(Clone, Debug, Default)]
pub struct QuantHeader {
    /// Base quantizer index.
    pub base_q_idx: u8,
    /// Luma DC delta.
    pub y_dc_delta: i32,
    /// Chroma DC delta.
    pub uv_dc_delta: i32,
    /// Chroma AC delta.
    pub uv_ac_delta: i32,
}

impl QuantHeader {
    /// True when the frame is lossless (all-zero quantizer state).
    #[must_use]
    pub fn lossless(&self) -> bool {
        self.base_q_idx == 0
            && self.y_dc_delta == 0
            && self.uv_dc_delta == 0
            && self.uv_ac_delta == 0
    }
}

/// Segmentation fields of the uncompressed header.
///
/// Like the loop-filter deltas, `feature_enabled`/`feature_data`/`abs_delta`
/// are persistent decoder state: libvpx `setup_segmentation` clears and
/// re-reads the whole feature set only when
/// [`update_data`](Self::update_data) is signalled, and leaves it untouched
/// otherwise. The parser fills in this frame's transmitted values;
/// `Vp9DecState::apply_segmentation_data` merges them.
#[derive(Clone, Debug)]
pub struct SegmentationHeader {
    /// Segmentation enabled.
    pub enabled: bool,
    /// Segment map update flag.
    pub update_map: bool,
    /// Whether this frame retransmits the per-segment feature data
    /// (`segmentation_update_data`). `false` means inherit.
    pub update_data: bool,
    /// Segment tree probabilities.
    pub tree_probs: [u8; 7],
    /// Temporal update flag.
    pub temporal_update: bool,
    /// Prediction probabilities (temporal updates).
    pub pred_probs: [u8; 3],
    /// Absolute (vs delta) feature values.
    pub abs_delta: bool,
    /// Per-segment feature enable flags: `[segment][feature]` with features
    /// ALT_Q, ALT_LF, REF_FRAME, SKIP.
    pub feature_enabled: [[bool; 4]; 8],
    /// Per-segment feature data.
    pub feature_data: [[i16; 4]; 8],
}

impl Default for SegmentationHeader {
    fn default() -> Self {
        Self {
            enabled: false,
            update_map: false,
            update_data: false,
            tree_probs: [255; 7],
            temporal_update: false,
            pred_probs: [255; 3],
            abs_delta: false,
            feature_enabled: [[false; 4]; 8],
            feature_data: [[0; 4]; 8],
        }
    }
}

impl UncompressedHeader {
    const SYNC_BYTES: [u8; 3] = [0x49, 0x83, 0x42];

    /// Parses the uncompressed header from bitstream data, with no
    /// decoded-picture buffer available.
    ///
    /// Equivalent to [`Self::parse_with_ref_sizes`] with every reference slot
    /// empty. An inter frame that copies its size from a reference
    /// (`frame_size_with_refs` with `found_ref` set) therefore fails here —
    /// that frame's dimensions are simply not in its own bitstream. Use
    /// [`Self::parse_with_ref_sizes`] whenever a decoder state exists.
    ///
    /// # Errors
    ///
    /// Returns error if the header is invalid.
    pub fn parse(data: &[u8]) -> CodecResult<Self> {
        Self::parse_with_ref_sizes(data, &[None; 8])
    }

    /// Parses the uncompressed header, resolving a `frame_size_with_refs`
    /// size copy against the decoded-picture buffer.
    ///
    /// `ref_sizes[i]` is the display size (`y_crop_width` / `y_crop_height`)
    /// currently held by reference slot `i`, or `None` if the slot is empty;
    /// there are eight slots (`refresh_frame_flags` is `f(8)`).
    ///
    /// The sizes have to be known *during* the parse, not patched in
    /// afterwards: `render_size()` copies `width`/`height` when it signals
    /// "same as frame size", and `tile_info()` derives *how many bits it
    /// reads* from the frame width, so a header parsed with a placeholder
    /// size desynchronises the bitstream from that point on.
    ///
    /// # Errors
    ///
    /// Returns error if the header is invalid, or if it copies its size from
    /// a reference slot that holds no decoded frame.
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    pub fn parse_with_ref_sizes(
        data: &[u8],
        ref_sizes: &[Option<(u32, u32)>; 8],
    ) -> CodecResult<Self> {
        let mut reader = BitReader::new(data);
        let mut header = Self::default();

        header.frame_marker = reader.read_bits(2).map_err(CodecError::Core)? as u8;
        if header.frame_marker != 0b10 {
            return Err(CodecError::InvalidBitstream(
                "Invalid VP9 frame marker".into(),
            ));
        }

        let profile_low = reader.read_bit().map_err(CodecError::Core)?;
        let profile_high = reader.read_bit().map_err(CodecError::Core)?;
        header.profile = (profile_high << 1) | profile_low;

        if header.profile == 3 {
            let reserved = reader.read_bit().map_err(CodecError::Core)?;
            if reserved != 0 {
                return Err(CodecError::InvalidBitstream("Reserved bit not zero".into()));
            }
        }

        header.show_existing_frame = reader.read_bit().map_err(CodecError::Core)? != 0;

        if header.show_existing_frame {
            header.frame_to_show = reader.read_bits(3).map_err(CodecError::Core)? as u8;
            // The three values the specification *assigns* on this path
            // (§6.2 `uncompressed_header`: `refresh_frame_flags = 0`,
            // `loop_filter_level = 0`, `header_size_in_bytes = 0`) plus
            // libvpx's explicit `cm->show_frame = 1`
            // (`vp9_decodeframe.c:2677-2679`). None of them is coded, so
            // every one is set here rather than left to `derive(Default)` —
            // `show_frame` in particular does *not* default to the right
            // value, and it is read downstream: `use_prev_frame_mvs` tests
            // `last_show_frame`, and a caller that mirrors this header into
            // its own bookkeeping would otherwise record a shown frame as
            // hidden.
            header.refresh_frame_flags = 0;
            header.loop_filter.filter_level = 0;
            header.compressed_header_size = 0;
            header.show_frame = true;
            return Ok(header);
        }

        header.frame_type = if reader.read_bit().map_err(CodecError::Core)? != 0 {
            Vp9FrameType::Inter
        } else {
            Vp9FrameType::Key
        };

        header.show_frame = reader.read_bit().map_err(CodecError::Core)? != 0;
        header.error_resilient = reader.read_bit().map_err(CodecError::Core)? != 0;

        if header.frame_type == Vp9FrameType::Key {
            Self::parse_sync_bytes(&mut reader)?;
            Self::parse_color_config(&mut reader, &mut header)?;
            Self::parse_frame_size(&mut reader, &mut header)?;
            Self::parse_render_size(&mut reader, &mut header)?;
            header.refresh_frame_flags = 0xFF;
        } else {
            if !header.show_frame {
                header.intra_only = reader.read_bit().map_err(CodecError::Core)? != 0;
            }

            if !header.error_resilient {
                header.reset_frame_context = reader.read_bits(2).map_err(CodecError::Core)? as u8;
            }

            if header.intra_only {
                Self::parse_sync_bytes(&mut reader)?;
                if header.profile > 0 {
                    Self::parse_color_config(&mut reader, &mut header)?;
                } else {
                    header.color_space = ColorSpace::Bt601;
                    header.subsampling_x = true;
                    header.subsampling_y = true;
                    header.bit_depth = 8;
                }
                header.refresh_frame_flags = reader.read_bits(8).map_err(CodecError::Core)? as u8;
                Self::parse_frame_size(&mut reader, &mut header)?;
                Self::parse_render_size(&mut reader, &mut header)?;
            } else {
                header.refresh_frame_flags = reader.read_bits(8).map_err(CodecError::Core)? as u8;
                for i in 0..3 {
                    header.ref_frame_idx[i] = reader.read_bits(3).map_err(CodecError::Core)? as u8;
                    header.ref_frame_sign_bias[i + 1] =
                        reader.read_bit().map_err(CodecError::Core)? != 0;
                }
                let found_ref =
                    Self::parse_frame_size_with_refs(&mut reader, &mut header, ref_sizes)?;
                if !found_ref {
                    Self::parse_frame_size(&mut reader, &mut header)?;
                }
                Self::parse_render_size(&mut reader, &mut header)?;
                header.allow_high_precision_mv = reader.read_bit().map_err(CodecError::Core)? != 0;
                Self::parse_interp_filter(&mut reader, &mut header)?;
            }
        }

        if header.error_resilient {
            header.refresh_frame_context = false;
            header.frame_parallel_decoding = true;
        } else {
            header.refresh_frame_context = reader.read_bit().map_err(CodecError::Core)? != 0;
            header.frame_parallel_decoding = reader.read_bit().map_err(CodecError::Core)? != 0;
        }
        header.frame_context_idx = reader.read_bits(2).map_err(CodecError::Core)? as u8;

        Self::parse_loop_filter(&mut reader, &mut header)?;
        Self::parse_quantization(&mut reader, &mut header)?;
        Self::parse_segmentation(&mut reader, &mut header)?;
        Self::parse_tile_info(&mut reader, &mut header)?;

        header.compressed_header_size = reader.read_bits(16).map_err(CodecError::Core)? as u16;
        if header.compressed_header_size == 0 {
            return Err(CodecError::InvalidBitstream(
                "VP9: zero compressed header size".into(),
            ));
        }

        // trailing_bits(): the uncompressed header is padded to a byte
        // boundary; the compressed header starts at the next byte.
        header.uncompressed_header_bytes = reader.bits_read().div_ceil(8);

        Ok(header)
    }

    /// Reads `su(bits)`: magnitude then sign (libvpx
    /// `vpx_rb_read_signed_literal`).
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn read_signed_literal(reader: &mut BitReader<'_>, bits: u8) -> CodecResult<i32> {
        let value = reader.read_bits(bits).map_err(CodecError::Core)? as i32;
        Ok(if reader.read_bit().map_err(CodecError::Core)? != 0 {
            -value
        } else {
            value
        })
    }

    /// Spec `loop_filter_params()` (libvpx `setup_loopfilter`).
    #[allow(clippy::cast_possible_truncation)]
    fn parse_loop_filter(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        let lf = &mut header.loop_filter;
        lf.filter_level = reader.read_bits(6).map_err(CodecError::Core)? as u8;
        lf.sharpness = reader.read_bits(3).map_err(CodecError::Core)? as u8;
        lf.delta_enabled = reader.read_bit().map_err(CodecError::Core)? != 0;
        if lf.delta_enabled {
            lf.delta_update = reader.read_bit().map_err(CodecError::Core)? != 0;
            if lf.delta_update {
                for i in 0..4 {
                    if reader.read_bit().map_err(CodecError::Core)? != 0 {
                        lf.ref_deltas[i] = Self::read_signed_literal(reader, 6)? as i8;
                        lf.ref_delta_updated[i] = true;
                    }
                }
                for i in 0..2 {
                    if reader.read_bit().map_err(CodecError::Core)? != 0 {
                        lf.mode_deltas[i] = Self::read_signed_literal(reader, 6)? as i8;
                        lf.mode_delta_updated[i] = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Spec `quantization_params()` (libvpx `setup_quantization`).
    #[allow(clippy::cast_possible_truncation)]
    fn parse_quantization(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        header.quant.base_q_idx = reader.read_bits(8).map_err(CodecError::Core)? as u8;
        header.quant.y_dc_delta = Self::read_delta_q(reader)?;
        header.quant.uv_dc_delta = Self::read_delta_q(reader)?;
        header.quant.uv_ac_delta = Self::read_delta_q(reader)?;
        Ok(())
    }

    fn read_delta_q(reader: &mut BitReader<'_>) -> CodecResult<i32> {
        if reader.read_bit().map_err(CodecError::Core)? != 0 {
            Self::read_signed_literal(reader, 4)
        } else {
            Ok(0)
        }
    }

    /// Spec `segmentation_params()` (libvpx `setup_segmentation`).
    #[allow(clippy::cast_possible_truncation)]
    fn parse_segmentation(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        /// Bits per feature value (`vp9_seg_feature_data_max` bit widths for
        /// ALT_Q=255, ALT_LF=63, REF_FRAME=3, SKIP=0).
        const FEATURE_BITS: [u8; 4] = [8, 6, 2, 0];
        /// Maximum feature values.
        const FEATURE_MAX: [i32; 4] = [255, 63, 3, 0];
        /// Signedness per feature.
        const FEATURE_SIGNED: [bool; 4] = [true, true, false, false];

        let seg = &mut header.seg;
        seg.enabled = reader.read_bit().map_err(CodecError::Core)? != 0;
        if !seg.enabled {
            return Ok(());
        }

        seg.update_map = reader.read_bit().map_err(CodecError::Core)? != 0;
        if seg.update_map {
            for p in &mut seg.tree_probs {
                *p = if reader.read_bit().map_err(CodecError::Core)? != 0 {
                    reader.read_bits(8).map_err(CodecError::Core)? as u8
                } else {
                    255
                };
            }
            seg.temporal_update = reader.read_bit().map_err(CodecError::Core)? != 0;
            for p in &mut seg.pred_probs {
                *p = if seg.temporal_update && reader.read_bit().map_err(CodecError::Core)? != 0 {
                    reader.read_bits(8).map_err(CodecError::Core)? as u8
                } else {
                    255
                };
            }
        }

        seg.update_data = reader.read_bit().map_err(CodecError::Core)? != 0;
        if seg.update_data {
            seg.abs_delta = reader.read_bit().map_err(CodecError::Core)? != 0;
            for s in 0..8 {
                for f in 0..4 {
                    let enabled = reader.read_bit().map_err(CodecError::Core)? != 0;
                    seg.feature_enabled[s][f] = enabled;
                    let mut data = 0i32;
                    if enabled {
                        if FEATURE_BITS[f] > 0 {
                            data = reader
                                .read_bits(FEATURE_BITS[f])
                                .map_err(CodecError::Core)?
                                as i32;
                            if data > FEATURE_MAX[f] {
                                data = FEATURE_MAX[f];
                            }
                        }
                        if FEATURE_SIGNED[f] && reader.read_bit().map_err(CodecError::Core)? != 0 {
                            data = -data;
                        }
                    }
                    seg.feature_data[s][f] = data as i16;
                }
            }
        }
        Ok(())
    }

    /// Spec `tile_info()` (libvpx `setup_tile_info` / `vp9_get_tile_n_bits`).
    fn parse_tile_info(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        let mi_cols = (header.width as usize + 7) >> 3;
        let sb64_cols = (mi_cols + 7) >> 3;

        let mut min_log2 = 0u8;
        while (64usize << min_log2) < sb64_cols {
            min_log2 += 1;
        }
        let mut max_log2 = 1u8;
        while (sb64_cols >> max_log2) >= 4 {
            max_log2 += 1;
        }
        max_log2 -= 1;

        header.tile_cols_log2 = min_log2;
        let mut max_ones = max_log2.saturating_sub(min_log2);
        while max_ones > 0 && reader.read_bit().map_err(CodecError::Core)? != 0 {
            header.tile_cols_log2 += 1;
            max_ones -= 1;
        }
        if header.tile_cols_log2 > 6 {
            return Err(CodecError::InvalidBitstream(
                "VP9: invalid number of tile columns".into(),
            ));
        }

        header.tile_rows_log2 = if reader.read_bit().map_err(CodecError::Core)? != 0 {
            1 + reader.read_bit().map_err(CodecError::Core)? as u8
        } else {
            0
        };
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn parse_sync_bytes(reader: &mut BitReader<'_>) -> CodecResult<()> {
        for expected in Self::SYNC_BYTES {
            let byte = reader.read_bits(8).map_err(CodecError::Core)? as u8;
            if byte != expected {
                return Err(CodecError::InvalidBitstream(
                    "Invalid VP9 sync bytes".into(),
                ));
            }
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn parse_color_config(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        if header.profile >= 2 {
            header.bit_depth = if reader.read_bit().map_err(CodecError::Core)? != 0 {
                12
            } else {
                10
            };
        } else {
            header.bit_depth = 8;
        }

        header.color_space = ColorSpace::from(reader.read_bits(3).map_err(CodecError::Core)? as u8);

        if header.color_space != ColorSpace::Srgb {
            header.color_range = reader.read_bit().map_err(CodecError::Core)? != 0;
            if header.profile == 1 || header.profile == 3 {
                header.subsampling_x = reader.read_bit().map_err(CodecError::Core)? != 0;
                header.subsampling_y = reader.read_bit().map_err(CodecError::Core)? != 0;
                reader.read_bit().map_err(CodecError::Core)?;
            } else {
                header.subsampling_x = true;
                header.subsampling_y = true;
            }
        } else {
            header.color_range = true;
            if header.profile == 1 || header.profile == 3 {
                header.subsampling_x = false;
                header.subsampling_y = false;
                reader.read_bit().map_err(CodecError::Core)?;
            }
        }

        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn parse_frame_size(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        header.width = reader.read_bits(16).map_err(CodecError::Core)? as u32 + 1;
        header.height = reader.read_bits(16).map_err(CodecError::Core)? as u32 + 1;
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn parse_render_size(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        let different = reader.read_bit().map_err(CodecError::Core)? != 0;
        if different {
            header.render_width = reader.read_bits(16).map_err(CodecError::Core)? as u32 + 1;
            header.render_height = reader.read_bits(16).map_err(CodecError::Core)? as u32 + 1;
        } else {
            header.render_width = header.width;
            header.render_height = header.height;
        }
        Ok(())
    }

    /// Spec `frame_size_with_refs()` (libvpx `setup_frame_size_with_refs`).
    ///
    /// Reads up to three `found_ref` bits; the first set bit adopts the
    /// display size of the reference it names and stops the scan:
    ///
    /// ```c
    /// for (i = 0; i < REFS_PER_FRAME; ++i) {
    ///   if (vpx_rb_read_bit(rb)) {
    ///     YV12_BUFFER_CONFIG *const buf = cm->frame_refs[i].buf;
    ///     width = buf->y_crop_width;
    ///     height = buf->y_crop_height;
    ///     found = 1;
    ///     break;
    ///   }
    /// }
    /// ```
    ///
    /// Returns `true` when a reference supplied the size (so the caller must
    /// not read an explicit `frame_size()`).
    fn parse_frame_size_with_refs(
        reader: &mut BitReader<'_>,
        header: &mut Self,
        ref_sizes: &[Option<(u32, u32)>; 8],
    ) -> CodecResult<bool> {
        for i in 0..3 {
            if reader.read_bit().map_err(CodecError::Core)? == 0 {
                continue;
            }
            let slot = usize::from(header.ref_frame_idx[i]);
            let (width, height) = ref_sizes.get(slot).copied().flatten().ok_or_else(|| {
                CodecError::InvalidBitstream(format!(
                    "VP9: frame_size_with_refs takes the frame size from \
                     reference {i} (slot {slot}), which holds no decoded frame"
                ))
            })?;
            if width == 0 || height == 0 {
                return Err(CodecError::InvalidBitstream(format!(
                    "VP9: reference slot {slot} has invalid dimensions {width}x{height}"
                )));
            }
            header.width = width;
            header.height = height;
            header.size_from_ref = Some(i);
            return Ok(true);
        }
        Ok(false)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn parse_interp_filter(reader: &mut BitReader<'_>, header: &mut Self) -> CodecResult<()> {
        let switchable = reader.read_bit().map_err(CodecError::Core)? != 0;
        header.interp_filter = if switchable {
            4
        } else {
            reader.read_bits(2).map_err(CodecError::Core)? as u8
        };
        Ok(())
    }

    /// Returns true if this is a keyframe.
    #[must_use]
    pub fn is_keyframe(&self) -> bool {
        self.frame_type == Vp9FrameType::Key
    }

    /// Returns true if this is an intra-only frame.
    #[must_use]
    pub fn is_intra_only(&self) -> bool {
        self.frame_type == Vp9FrameType::Key || self.intra_only
    }

    /// Returns the chroma subsampling as (x, y).
    #[must_use]
    pub const fn chroma_subsampling(&self) -> (u8, u8) {
        let x = if self.subsampling_x { 2 } else { 1 };
        let y = if self.subsampling_y { 2 } else { 1 };
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_space_from() {
        assert_eq!(ColorSpace::from(2), ColorSpace::Bt709);
        assert_eq!(ColorSpace::from(7), ColorSpace::Srgb);
    }

    #[test]
    fn test_vp9_frame_type() {
        assert!(Vp9FrameType::Key.is_keyframe());
        assert!(!Vp9FrameType::Inter.is_keyframe());
    }

    #[test]
    fn test_invalid_frame_marker() {
        let data = [0x00];
        assert!(UncompressedHeader::parse(&data).is_err());
    }

    #[test]
    fn test_chroma_subsampling() {
        let mut header = UncompressedHeader::default();
        header.subsampling_x = true;
        header.subsampling_y = true;
        assert_eq!(header.chroma_subsampling(), (2, 2));
    }

    /// Real libvpx-vp9 inter frame (frame 1 of a 3-frame 76x42 encode) whose
    /// `frame_size_with_refs` sets `found_ref` on reference 0 — i.e. its
    /// dimensions live in the decoded-picture buffer, not in its own bits.
    const INTER_FRAME_76X42: &[u8] = include_bytes!("dec/testdata/seq76x42.frame1.bin");
    /// Real libvpx-vp9 key frame, 76x42, crf 24.
    const KEYFRAME_76X42: &[u8] = include_bytes!("dec/testdata/kf76x42.frame0.bin");

    #[test]
    fn test_frame_size_with_refs_records_index_and_copies_dimensions() {
        let mut refs = [None; 8];
        refs[0] = Some((76, 42));

        let hdr = UncompressedHeader::parse_with_ref_sizes(INTER_FRAME_76X42, &refs)
            .expect("inter header parses against a populated DPB");

        assert_eq!(hdr.size_from_ref, Some(0), "found_ref on reference 0");
        assert_eq!((hdr.width, hdr.height), (76, 42));
        assert_eq!(
            (hdr.render_width, hdr.render_height),
            (76, 42),
            "render_size() copies the frame size when it signals 'same'"
        );
        assert!(!hdr.is_keyframe() && !hdr.is_intra_only());
        assert!(
            hdr.compressed_header_size > 0 && hdr.uncompressed_header_bytes > 0,
            "the sections after frame_size_with_refs must still parse"
        );
    }

    #[test]
    fn test_frame_size_with_refs_takes_the_slot_dimensions_not_a_guess() {
        // The size genuinely comes from the referenced slot: point that slot
        // at different dimensions and the parsed frame size follows.
        let mut refs = [None; 8];
        refs[0] = Some((320, 240));

        let hdr = UncompressedHeader::parse_with_ref_sizes(INTER_FRAME_76X42, &refs)
            .expect("header parses");

        assert_eq!(hdr.size_from_ref, Some(0));
        assert_eq!((hdr.width, hdr.height), (320, 240));
        assert_eq!((hdr.render_width, hdr.render_height), (320, 240));
    }

    #[test]
    fn test_frame_size_with_refs_without_the_slot_is_an_honest_error() {
        let err = UncompressedHeader::parse(INTER_FRAME_76X42)
            .expect_err("size copied from an absent reference cannot be invented");
        match err {
            CodecError::InvalidBitstream(msg) => {
                assert!(msg.contains("frame_size_with_refs"), "{msg}");
                assert!(msg.contains("no decoded frame"), "{msg}");
            }
            other => panic!("expected InvalidBitstream, got {other:?}"),
        }

        // An empty slot at the referenced index is the same failure.
        let mut refs = [None; 8];
        refs[1] = Some((76, 42));
        assert!(UncompressedHeader::parse_with_ref_sizes(INTER_FRAME_76X42, &refs).is_err());
    }

    #[test]
    fn test_frame_size_with_refs_rejects_a_zero_sized_slot() {
        let mut refs = [None; 8];
        refs[0] = Some((0, 0));
        let err = UncompressedHeader::parse_with_ref_sizes(INTER_FRAME_76X42, &refs)
            .expect_err("a 0x0 reference is not a usable frame size");
        match err {
            CodecError::InvalidBitstream(msg) => {
                assert!(msg.contains("invalid dimensions"), "{msg}");
            }
            other => panic!("expected InvalidBitstream, got {other:?}"),
        }
    }

    #[test]
    fn test_keyframe_size_is_coded_not_copied() {
        let hdr = UncompressedHeader::parse(KEYFRAME_76X42).expect("keyframe parses");
        assert_eq!(hdr.size_from_ref, None, "key frames code their own size");
        assert_eq!((hdr.width, hdr.height), (76, 42));
    }

    #[test]
    fn test_loop_filter_delta_update_flags_track_transmitted_entries() {
        let hdr = UncompressedHeader::parse(KEYFRAME_76X42).expect("keyframe parses");
        let lf = &hdr.loop_filter;

        if !lf.delta_update {
            assert_eq!(
                lf.ref_delta_updated, [false; 4],
                "no delta payload was coded, so nothing may be flagged as transmitted"
            );
            assert_eq!(lf.mode_delta_updated, [false; 2]);
            assert_eq!(
                (lf.ref_deltas, lf.mode_deltas),
                ([1, 0, -1, -1], [0, 0]),
                "un-transmitted deltas stay at the set_default_lf_deltas values"
            );
        }
        if !lf.delta_enabled {
            assert!(
                !lf.delta_update,
                "the update bit is not coded when disabled"
            );
        }
        // Any entry that differs from the default must have been transmitted.
        for (i, (&d, &flagged)) in lf
            .ref_deltas
            .iter()
            .zip(lf.ref_delta_updated.iter())
            .enumerate()
        {
            if d != [1, 0, -1, -1][i] {
                assert!(flagged, "ref delta {i} changed without being flagged");
            }
        }
    }

    /// The one-byte `show_existing_frame` packet from libvpx's
    /// `vp90-2-16-intra-only.webm` (frame marker `10`, profile 0,
    /// `show_existing_frame = 1`, `frame_to_show_map_idx = 0`).
    const SHOW_EXISTING_SLOT0: &[u8] = include_bytes!("dec/testdata/p9io.frame1.bin");

    /// Every field the specification *assigns* on the `show_existing_frame`
    /// path must be assigned, not left to `derive(Default)` — §6.2
    /// `uncompressed_header` sets `refresh_frame_flags = 0`,
    /// `loop_filter_level = 0` and `header_size_in_bytes = 0`, and libvpx
    /// additionally sets `cm->show_frame = 1` (`vp9_decodeframe.c:2677-2679`).
    ///
    /// `show_frame` is the one that bites: its `Default` is `false`, which is
    /// the *opposite* of the truth for a packet whose entire purpose is to
    /// display a frame, and `Vp9DecState::use_prev_frame_mvs` reads a
    /// `last_show_frame` derived from it.
    #[test]
    fn test_show_existing_frame_assigns_every_derived_field() {
        assert_eq!(SHOW_EXISTING_SLOT0, &[0x88], "the fixture is that one byte");
        let hdr = UncompressedHeader::parse(SHOW_EXISTING_SLOT0)
            .expect("a show_existing_frame packet parses from one byte");

        assert!(hdr.show_existing_frame);
        assert_eq!(hdr.frame_to_show, 0);
        assert!(
            hdr.show_frame,
            "the specification forces show_frame on this path (libvpx \
             vp9_decodeframe.c:2679); the struct default would say hidden"
        );
        assert_eq!(hdr.refresh_frame_flags, 0, "no slot is refreshed");
        assert_eq!(hdr.loop_filter.filter_level, 0, "no loop filter is run");
        assert_eq!(
            hdr.compressed_header_size, 0,
            "header_size_in_bytes = 0: there is no compressed header"
        );
    }

    /// All eight slot indices parse, and none of them is confused with a
    /// normal frame header.
    #[test]
    fn test_show_existing_frame_reads_every_slot_index() {
        for slot in 0u8..8 {
            // frame_marker(10) profile(00) show_existing(1) idx(3 bits)
            let byte = 0b1000_1000 | slot;
            let hdr = UncompressedHeader::parse(&[byte]).expect("one-byte header parses");
            assert!(hdr.show_existing_frame);
            assert_eq!(hdr.frame_to_show, slot);
            assert!(hdr.show_frame);
            assert_eq!(hdr.refresh_frame_flags, 0);
            assert!(
                !hdr.is_keyframe() || hdr.show_existing_frame,
                "a show-existing packet codes no frame_type"
            );
        }
    }

    /// A real key frame must not pick up any of the show-existing
    /// assignments — the guard against implementing them in the wrong place.
    #[test]
    fn test_a_real_key_frame_is_untouched_by_the_show_existing_assignments() {
        let hdr = UncompressedHeader::parse(KEYFRAME_76X42).expect("keyframe parses");
        assert!(!hdr.show_existing_frame);
        assert_eq!(hdr.refresh_frame_flags, 0xFF, "a key frame refreshes all");
        assert!(hdr.compressed_header_size > 0);
    }

    #[test]
    fn test_segmentation_update_data_flag_is_recorded() {
        let hdr = UncompressedHeader::parse(KEYFRAME_76X42).expect("keyframe parses");
        if !hdr.seg.enabled {
            assert!(
                !hdr.seg.update_data && !hdr.seg.update_map,
                "no segmentation payload is coded when segmentation is off"
            );
        }
        if !hdr.seg.update_data {
            assert_eq!(
                hdr.seg.feature_data, [[0; 4]; 8],
                "features are only filled in by an update_data pass"
            );
        }
    }
}

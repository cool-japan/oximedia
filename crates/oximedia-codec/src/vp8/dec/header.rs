//! VP8 key-frame header parsing (RFC 6386 §9, §19).
//!
//! The key-frame header consists of:
//! - a 3-byte uncompressed "frame tag" (frame type, version, show flag, first
//!   partition length),
//! - the 7-byte uncompressed key-frame start code (`0x9d 0x01 0x2a`) plus
//!   14-bit width/height with 2-bit upscaling codes,
//! - a boolean-coded section: colour space, clamping, segmentation, loop-filter
//!   parameters, token-partition count, quantiser indices, and the DCT token
//!   probability updates.
//!
//! Ported from the production-verified `oximedia-image` `webp/vp8` decoder
//! (same workspace; a WebP lossy frame is a VP8 key frame), adapted to this
//! crate's [`CodecError`] error type.

use super::bool_decoder::BoolDecoder;
use super::state::{EntropyContext, LoopFilterDeltas, Vp8State};
use super::tables::{COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS};
use super::tables_inter::MV_UPDATE_PROBS;
use crate::error::{CodecError, CodecResult};

/// Number of independently-decodable segments (RFC 6386 §10).
pub const MAX_SEGMENTS: usize = 4;
/// Number of macroblock "reference frame" loop-filter delta slots.
pub const MAX_REF_LF_DELTAS: usize = 4;
/// Number of macroblock "mode" loop-filter delta slots.
pub const MAX_MODE_LF_DELTAS: usize = 4;

/// Per-segment quantiser / loop-filter feature data.
#[derive(Debug, Clone, Default)]
pub struct SegmentHeader {
    /// Whether segmentation is enabled for this frame.
    pub enabled: bool,
    /// Whether the per-MB segment map is transmitted this frame.
    pub update_map: bool,
    /// Absolute (true) vs delta (false) interpretation of segment features.
    pub abs_delta: bool,
    /// Per-segment quantiser values (absolute or delta).
    pub quantizer: [i32; MAX_SEGMENTS],
    /// Per-segment loop-filter levels (absolute or delta).
    pub filter_strength: [i32; MAX_SEGMENTS],
    /// Probabilities for the per-MB segment-id tree.
    pub tree_probs: [u8; 3],
}

/// In-loop deblocking-filter header (RFC 6386 §9.4, §15).
#[derive(Debug, Clone, Default)]
pub struct LoopFilterHeader {
    /// `true` selects the simple filter, `false` the normal filter.
    pub simple: bool,
    /// Base filter level for the frame.
    pub level: i32,
    /// Sharpness control (0..7) feeding the interior-limit derivation.
    pub sharpness: i32,
    /// Whether per-MB loop-filter deltas are in use.
    pub delta_enabled: bool,
    /// Per-reference-frame loop-filter level deltas.
    pub ref_deltas: [i32; MAX_REF_LF_DELTAS],
    /// Per-prediction-mode loop-filter level deltas.
    pub mode_deltas: [i32; MAX_MODE_LF_DELTAS],
}

/// Quantiser-index header (RFC 6386 §9.6).
#[derive(Debug, Clone, Default)]
pub struct QuantHeader {
    /// Base AC quantiser index for the luma plane.
    pub y_ac_qi: i32,
    /// Delta applied to the luma DC quantiser index.
    pub y_dc_delta: i32,
    /// Delta applied to the Y2 (WHT) DC quantiser index.
    pub y2_dc_delta: i32,
    /// Delta applied to the Y2 (WHT) AC quantiser index.
    pub y2_ac_delta: i32,
    /// Delta applied to the chroma DC quantiser index.
    pub uv_dc_delta: i32,
    /// Delta applied to the chroma AC quantiser index.
    pub uv_ac_delta: i32,
}

/// Inter-frame-only header fields (RFC 6386 §9.7, §9.10).
///
/// VP8 key frames do not carry any of this data — for a key-frame-parsed
/// [`Vp8Header`] every field here is left at its [`Default`] (false/0)
/// value, which is never read as meaningful because
/// [`Vp8Header::is_keyframe`] is `true`. Grouped into its own struct rather
/// than flattened onto [`Vp8Header`] to keep the "only meaningful when
/// `!is_keyframe`" scoping explicit at the type level without resorting to
/// `Option` (every field here is unconditionally present in an inter-frame
/// bitstream, so wrapping in `Option` would not add information).
#[derive(Debug, Clone, Default)]
pub struct InterFrameHeader {
    /// Whether this frame refreshes the golden-frame reference buffer.
    pub refresh_golden: bool,
    /// Whether this frame refreshes the altref reference buffer.
    pub refresh_alt: bool,
    /// Buffer-copy selector for the golden frame when `!refresh_golden`
    /// (0 = none, 1 = copy last frame, 2 = copy altref frame).
    pub copy_to_golden: u8,
    /// Buffer-copy selector for the altref frame when `!refresh_alt`
    /// (0 = none, 1 = copy last frame, 2 = copy golden frame).
    pub copy_to_alt: u8,
    /// Sign-bias flag for the golden frame (RFC 6386 §9.7).
    pub sign_bias_golden: bool,
    /// Sign-bias flag for the altref frame (RFC 6386 §9.7).
    pub sign_bias_alt: bool,
    /// Whether this frame's entropy-probability updates persist beyond it.
    /// `false` means the decoder must snapshot-and-restore (RFC 6386 §9.9;
    /// see [`super::state::Vp8State::saved_entropy`]).
    ///
    /// This is the one field of this struct that is **also meaningful for a
    /// key frame**: RFC 6386 §19.2 codes `refresh_entropy_probs` in both
    /// branches of the frame header (rfc6386.txt line 6822 for
    /// `if (key_frame)`, line 6848 for the `else`), and dixie.c's
    /// snapshot/restore pair (lines 8159-8163 / 8205-8209) is not gated on
    /// frame type. [`Vp8Header::parse`] therefore fills it in too, leaving
    /// every other field of this struct at its default; see that function.
    pub refresh_entropy_probs: bool,
    /// Whether this frame refreshes the last-frame reference buffer.
    pub refresh_last: bool,
    /// Probability that a macroblock is intra-predicted (RFC 6386 §9.10).
    pub prob_intra: u8,
    /// Probability that an inter macroblock predicts from the last frame
    /// rather than golden/altref (RFC 6386 §9.10).
    pub prob_last: u8,
    /// Probability that an inter macroblock not predicting from the last
    /// frame predicts from the golden frame rather than altref (RFC 6386
    /// §9.10).
    pub prob_gf: u8,
}

/// Fully-parsed VP8 key-frame header plus decoder-ready probability state.
pub struct Vp8Header {
    /// Decoded frame width in pixels.
    pub width: u32,
    /// Decoded frame height in pixels.
    pub height: u32,
    /// Bitstream version / profile, frame-tag bits 1..3 (RFC 6386 §9.1,
    /// rfc6386.txt lines 1690-1713). Selects the reconstruction filter —
    /// see [`super::mc::ReconFilter::from_version`]. Present in both frame
    /// types' tags, and (unlike the dimensions) *not* re-sent by inter
    /// frames' callers: every frame carries its own.
    pub version: u8,
    /// Horizontal upscaling code (0 = none). Display-time hint only
    /// (RFC 6386 §9.1); reconstruction is unaffected.
    pub horizontal_scale: u8,
    /// Vertical upscaling code (0 = none). Display-time hint only.
    pub vertical_scale: u8,
    /// Colour-space flag (0 = YUV; only 0 is defined by RFC 6386 §9.2).
    pub color_space: u8,
    /// Pixel-clamping flag (0 = decoder must clamp). This intra-only decoder
    /// always clamps, which satisfies both settings.
    pub clamping_required: bool,
    /// Segmentation header.
    pub segment: SegmentHeader,
    /// Loop-filter header.
    pub loop_filter: LoopFilterHeader,
    /// Quantiser header.
    pub quant: QuantHeader,
    /// DCT-token probability table after any header updates.
    pub coeff_probs: [[[[u8; 11]; 3]; 8]; 4],
    /// Whether the per-MB `mb_skip_coeff` flag is coded (skipping enabled).
    pub mb_no_skip_coeff: bool,
    /// Probability used by the `mb_skip_coeff` flag when skipping is enabled.
    pub prob_skip_false: u8,
    /// Byte offset of the first DCT-token partition, relative to the start of
    /// the VP8 payload.
    pub partitions_start: usize,
    /// Length of the first (header) partition in bytes.
    pub first_partition_size: usize,
    /// Number of DCT-token partitions (1, 2, 4 or 8).
    pub num_token_partitions: usize,
    /// `true` for a key frame, `false` for an inter frame. Key frames are
    /// produced by [`Vp8Header::parse`], inter frames by
    /// [`Vp8Header::parse_interframe`].
    pub is_keyframe: bool,
    /// Inter-frame-only fields; see [`InterFrameHeader`]. All-default
    /// (false/0) when `is_keyframe` is `true`.
    pub inter: InterFrameHeader,
}

impl Vp8Header {
    /// Parses a VP8 key-frame header from `data` (a raw VP8 frame payload).
    ///
    /// Returns the parsed header and the boolean decoder positioned
    /// immediately after the header section, ready to decode macroblock
    /// prediction modes.
    ///
    /// # Errors
    /// Fails if the payload is too short, is not a key frame, has a bad start
    /// code, or declares unsupported parameters.
    pub fn parse(data: &[u8]) -> CodecResult<(Self, BoolDecoder<'_>)> {
        if data.len() < 10 {
            return Err(CodecError::InvalidBitstream(
                "VP8: payload too small for a key-frame header".to_string(),
            ));
        }

        // --- 3-byte uncompressed frame tag (RFC 6386 §9.1) ---
        let tag = u32::from(data[0]) | (u32::from(data[1]) << 8) | (u32::from(data[2]) << 16);
        let key_frame = (tag & 1) == 0;
        let version = ((tag >> 1) & 0x7) as u8;
        let _show_frame = ((tag >> 4) & 1) != 0;
        let first_partition_size = ((tag >> 5) & 0x7_FFFF) as usize;

        if !key_frame {
            return Err(CodecError::InvalidBitstream(
                "VP8: not a key frame (key-frame decoder)".to_string(),
            ));
        }
        if version > 3 {
            return Err(CodecError::InvalidBitstream(
                "VP8: unsupported bitstream version".to_string(),
            ));
        }

        // --- 7-byte uncompressed key-frame header (RFC 6386 §9.1) ---
        // Start code: 0x9d 0x01 0x2a.
        if data[3] != 0x9d || data[4] != 0x01 || data[5] != 0x2a {
            return Err(CodecError::InvalidBitstream(
                "VP8: bad key-frame start code".to_string(),
            ));
        }
        let dim0 = u32::from(data[6]) | (u32::from(data[7]) << 8);
        let dim1 = u32::from(data[8]) | (u32::from(data[9]) << 8);
        let width = dim0 & 0x3FFF;
        let horizontal_scale = (dim0 >> 14) as u8;
        let height = dim1 & 0x3FFF;
        let vertical_scale = (dim1 >> 14) as u8;

        if width == 0 || height == 0 {
            return Err(CodecError::InvalidBitstream(
                "VP8: zero frame dimension".to_string(),
            ));
        }

        // First partition starts after the 10-byte uncompressed header.
        let header_end = 10usize;
        let partitions_start = header_end
            .checked_add(first_partition_size)
            .ok_or_else(|| {
                CodecError::InvalidBitstream("VP8: first partition size overflow".to_string())
            })?;
        if first_partition_size == 0 || partitions_start > data.len() {
            return Err(CodecError::InvalidBitstream(
                "VP8: first partition exceeds payload".to_string(),
            ));
        }

        // The boolean decoder operates on the first (header) partition only.
        let header_partition = &data[header_end..partitions_start];
        let mut bd = BoolDecoder::new(header_partition);

        // --- colour space and clamping (RFC 6386 §9.2) ---
        let color_space = u8::from(bd.get_flag());
        let clamping_required = !bd.get_flag(); // 0 => decoder must clamp

        // --- segmentation header (RFC 6386 §9.3, §10) ---
        let segment = parse_segmentation(&mut bd);

        // --- loop-filter header (RFC 6386 §9.4) ---
        let loop_filter = parse_loop_filter(&mut bd);

        // --- token partition count (RFC 6386 §9.5) ---
        let log2_partitions = bd.get_literal(2);
        let num_token_partitions = 1usize << log2_partitions;

        // --- quantiser indices (RFC 6386 §9.6) ---
        let quant = parse_quant(&mut bd);

        // --- refresh flags (key frame: golden / altref refresh forced) ---
        // For a key frame only `refresh_entropy_probs` is coded here (RFC
        // 6386 §19.2, rfc6386.txt line 6822). It is returned rather than
        // discarded: dixie.c snapshots/restores the entropy context on this
        // bit for *both* frame types (lines 8159-8163, 8205-8209), so the
        // caller needs it — see `InterFrameHeader::refresh_entropy_probs`.
        let refresh_entropy_probs = bd.get_flag();

        // --- DCT-token probability updates (RFC 6386 §9.9, §13.4) ---
        // A key frame's persistent entropy context is the RFC defaults
        // (§9.9: "replaced with their defaults at the beginning of every key
        // frame") — this call's `base` argument is that reset, applied
        // before the shared update loop runs.
        let coeff_probs = update_coeff_probs(&mut bd, &DEFAULT_COEFF_PROBS);

        // --- mb_no_skip_coeff / prob_skip_false (RFC 6386 §9.10/§9.11; §19.2
        // lines 6852-6854 — coded identically for both frame types) ---
        let (mb_no_skip_coeff, prob_skip_false) = parse_skip_prob(&mut bd);

        let header = Self {
            width,
            height,
            version,
            horizontal_scale,
            vertical_scale,
            color_space,
            clamping_required,
            segment,
            loop_filter,
            quant,
            coeff_probs,
            mb_no_skip_coeff,
            prob_skip_false,
            partitions_start,
            first_partition_size,
            num_token_partitions,
            is_keyframe: true,
            inter: InterFrameHeader {
                // The only inter-frame-shaped field a key frame codes; every
                // other one keeps its `Default` (see `InterFrameHeader`).
                refresh_entropy_probs,
                ..InterFrameHeader::default()
            },
        };
        Ok((header, bd))
    }
}

/// Parses the segmentation sub-header (RFC 6386 §9.3).
fn parse_segmentation(bd: &mut BoolDecoder<'_>) -> SegmentHeader {
    let mut seg = SegmentHeader {
        tree_probs: [255, 255, 255],
        ..SegmentHeader::default()
    };
    seg.enabled = bd.get_flag();
    if !seg.enabled {
        return seg;
    }
    seg.update_map = bd.get_flag();
    let update_data = bd.get_flag();
    if update_data {
        seg.abs_delta = bd.get_flag();
        // Quantiser feature: signed 7-bit magnitude per segment.
        for q in &mut seg.quantizer {
            *q = if bd.get_flag() {
                bd.get_signed_literal(7)
            } else {
                0
            };
        }
        // Loop-filter feature: signed 6-bit magnitude per segment.
        for f in &mut seg.filter_strength {
            *f = if bd.get_flag() {
                bd.get_signed_literal(6)
            } else {
                0
            };
        }
    }
    if seg.update_map {
        for p in &mut seg.tree_probs {
            *p = if bd.get_flag() {
                bd.get_literal(8) as u8
            } else {
                255
            };
        }
    }
    seg
}

/// Parses the loop-filter sub-header (RFC 6386 §9.4).
fn parse_loop_filter(bd: &mut BoolDecoder<'_>) -> LoopFilterHeader {
    let mut lf = LoopFilterHeader {
        simple: bd.get_flag(),
        level: bd.get_literal(6) as i32,
        sharpness: bd.get_literal(3) as i32,
        ..LoopFilterHeader::default()
    };
    lf.delta_enabled = bd.get_flag();
    if lf.delta_enabled {
        // `loop_filter_delta_update`
        if bd.get_flag() {
            for d in &mut lf.ref_deltas {
                if bd.get_flag() {
                    *d = bd.get_signed_literal(6);
                }
            }
            for d in &mut lf.mode_deltas {
                if bd.get_flag() {
                    *d = bd.get_signed_literal(6);
                }
            }
        }
    }
    lf
}

/// Parses the quantiser sub-header (RFC 6386 §9.6).
fn parse_quant(bd: &mut BoolDecoder<'_>) -> QuantHeader {
    // Reads a flagged signed 4-bit delta (used for every delta field).
    fn delta(bd: &mut BoolDecoder<'_>) -> i32 {
        if bd.get_flag() {
            bd.get_signed_literal(4)
        } else {
            0
        }
    }
    QuantHeader {
        y_ac_qi: bd.get_literal(7) as i32,
        y_dc_delta: delta(bd),
        y2_dc_delta: delta(bd),
        y2_ac_delta: delta(bd),
        uv_dc_delta: delta(bd),
        uv_ac_delta: delta(bd),
    }
}

/// Applies the DCT-token probability update loop on top of `base` (RFC 6386
/// §9.9 `token_prob_update()`; §19.2 bitstream syntax, rfc6386.txt lines
/// 7187-7199; dixie.c `decode_entropy_header` lines 7738-7747). Identical
/// bitstream syntax for both frame types — only what `base` starts from
/// differs: a key frame always starts from [`DEFAULT_COEFF_PROBS`] (§9.9:
/// "replaced with their defaults at the beginning of every key frame"), an
/// inter frame starts from the persistent
/// [`super::state::Vp8State::entropy`]`.coeff_probs` carried over from the
/// previous frame. Shared verbatim by [`Vp8Header::parse`] (keyframe,
/// unchanged behaviour) and [`Vp8Header::parse_interframe`].
fn update_coeff_probs(
    bd: &mut BoolDecoder<'_>,
    base: &[[[[u8; 11]; 3]; 8]; 4],
) -> [[[[u8; 11]; 3]; 8]; 4] {
    let mut coeff_probs = *base;
    for (i, plane) in COEFF_UPDATE_PROBS.iter().enumerate() {
        for (j, band) in plane.iter().enumerate() {
            for (k, ctx) in band.iter().enumerate() {
                for (t, &update_prob) in ctx.iter().enumerate() {
                    if bd.get_bool(update_prob) {
                        coeff_probs[i][j][k][t] = bd.get_literal(8) as u8;
                    }
                }
            }
        }
    }
    coeff_probs
}

/// Parses `mb_no_skip_coeff` and (if set) `prob_skip_false` (RFC 6386
/// §9.10/§9.11; §19.2 bitstream syntax, rfc6386.txt lines 6852-6854 — this
/// pair is coded identically regardless of frame type, immediately after
/// the coefficient-probability update loop and before the `!key_frame`
/// block). Shared by [`Vp8Header::parse`] and
/// [`Vp8Header::parse_interframe`].
fn parse_skip_prob(bd: &mut BoolDecoder<'_>) -> (bool, u8) {
    let mb_no_skip_coeff = bd.get_flag();
    let prob_skip_false = if mb_no_skip_coeff {
        bd.get_literal(8) as u8
    } else {
        0
    };
    (mb_no_skip_coeff, prob_skip_false)
}

/// Parses the segmentation sub-header for an **inter** frame (RFC 6386
/// §9.3), threading the persistent feature-data/tree-probs through
/// `state.segment` instead of resetting them every frame.
///
/// Verified against dixie.c `decode_segmentation_header` (rfc6386.txt lines
/// 7925-7983): `enabled` is read fresh every frame; when enabled,
/// `update_map`/`update_data` gate (respectively) whether `tree_probs` and
/// whether `abs_delta`/`quant_deltas`/`lf_deltas` are touched *at all* this
/// frame — the persistence boundary is that outer gate, not the individual
/// per-item flags. Within an update pass, every item is unconditionally
/// reassigned (`bool_maybe_get_int`, lines 7593-7595: the flag gates the
/// *value* — new data or a hard `0`/`255` default — never "leave the
/// previous value"). That inner per-item behaviour is identical to the
/// key-frame-only [`parse_segmentation`] above (correct there too, since a
/// key frame's "previous" is always the just-reset zero); only the outer
/// gate differs, because a key frame always starts from a freshly zeroed
/// struct, while an inter frame with the outer flag clear must skip the
/// block entirely and keep whatever was already in `state.segment`.
fn parse_segmentation_inter(bd: &mut BoolDecoder<'_>, state: &mut Vp8State) -> SegmentHeader {
    state.segment.enabled = bd.get_flag();
    let mut update_map = false;
    if state.segment.enabled {
        update_map = bd.get_flag();
        let update_data = bd.get_flag();
        if update_data {
            state.segment.abs_delta = bd.get_flag();
            // Quantiser feature: signed 7-bit magnitude per segment.
            for q in &mut state.segment.quant_deltas {
                *q = if bd.get_flag() {
                    bd.get_signed_literal(7) as i8
                } else {
                    0
                };
            }
            // Loop-filter feature: signed 6-bit magnitude per segment.
            for f in &mut state.segment.lf_deltas {
                *f = if bd.get_flag() {
                    bd.get_signed_literal(6) as i8
                } else {
                    0
                };
            }
        }
        if update_map {
            for p in &mut state.segment.tree_probs {
                *p = if bd.get_flag() {
                    bd.get_literal(8) as u8
                } else {
                    255
                };
            }
        }
    }
    // `!enabled`: dixie.c sets update_map = update_data = 0 and reads no
    // further bits (lines 7978-7982) -- `state.segment`'s persistent fields
    // are simply left untouched.
    SegmentHeader {
        enabled: state.segment.enabled,
        update_map,
        abs_delta: state.segment.abs_delta,
        quantizer: state.segment.quant_deltas.map(i32::from),
        filter_strength: state.segment.lf_deltas.map(i32::from),
        tree_probs: state.segment.tree_probs,
    }
}

/// Parses the loop-filter sub-header for an **inter** frame (RFC 6386
/// §9.4), threading the persistent per-index ref/mode deltas through
/// `state.lf_deltas` instead of resetting them every frame (see the module
/// doc on [`super::state::LoopFilterDeltas`]).
///
/// Verified against dixie.c `decode_loopfilter_header` (rfc6386.txt lines
/// 7900-7922): `use_simple`/`level`/`sharpness`/`delta_enabled` are read
/// fresh every frame; `ref_delta`/`mode_delta` are only touched at all when
/// `delta_enabled && mode_ref_lf_delta_update`, and (as with segmentation
/// above) every index within that pass is unconditionally reassigned
/// (`bool_maybe_get_int`: flag clear means the delta becomes `0`, not
/// "unchanged") — the persistence boundary is the outer
/// `mode_ref_lf_delta_update` gate, exactly mirroring
/// [`parse_segmentation_inter`]'s `update_data`/`update_map` gates. This is
/// the fix for the "P1-era code builds a fresh `LoopFilterHeader` per
/// frame" latent bug: [`parse_loop_filter`] above is correct only for key
/// frames, where "previous" always means "just reset to zero".
fn parse_loop_filter_inter(bd: &mut BoolDecoder<'_>, state: &mut Vp8State) -> LoopFilterHeader {
    let simple = bd.get_flag();
    let level = bd.get_literal(6) as i32;
    let sharpness = bd.get_literal(3) as i32;
    state.lf_deltas.enabled = bd.get_flag();
    if state.lf_deltas.enabled {
        let delta_update = bd.get_flag(); // mode_ref_lf_delta_update
        if delta_update {
            for d in &mut state.lf_deltas.ref_deltas {
                *d = if bd.get_flag() {
                    bd.get_signed_literal(6) as i8
                } else {
                    0
                };
            }
            for d in &mut state.lf_deltas.mode_deltas {
                *d = if bd.get_flag() {
                    bd.get_signed_literal(6) as i8
                } else {
                    0
                };
            }
        }
    }
    LoopFilterHeader {
        simple,
        level,
        sharpness,
        delta_enabled: state.lf_deltas.enabled,
        ref_deltas: state.lf_deltas.ref_deltas.map(i32::from),
        mode_deltas: state.lf_deltas.mode_deltas.map(i32::from),
    }
}

impl Vp8Header {
    /// Parses a VP8 **inter**-frame header from `data` (a raw VP8 frame
    /// payload), threading persistent cross-frame state through `state`.
    ///
    /// Unlike a key frame, an inter-frame payload carries no start code or
    /// dimensions (RFC 6386 §9.1/§19.2: the 7-byte start-code+dimensions
    /// block after the 3-byte frame tag is `if (key_frame) { ... }` only,
    /// rfc6386.txt lines 6808-6811) — the caller supplies the current frame
    /// dimensions (known from the most recently decoded key frame).
    ///
    /// Returns the parsed header and the boolean decoder positioned
    /// immediately after the header section, ready to decode macroblock
    /// prediction modes — same contract as [`Vp8Header::parse`].
    ///
    /// Field order verified against RFC 6386 §19.2's bitstream-syntax table
    /// (rfc6386.txt lines 6804-6870, `token_prob_update()` 7187-7199,
    /// `mv_prob_update()` 7206-7219) and cross-checked against the embedded
    /// dixie.c reference decoder (`decode_reference_header` lines
    /// 7791-7808, `decode_entropy_header` lines 7722-7778, and the
    /// independent `split_ivf.py` static parser used to produce this
    /// package's conformance fixtures' `layout.txt` files).
    ///
    /// # Errors
    /// Fails if the payload is too short, is actually a key frame, declares
    /// an unsupported bitstream version, has a zero frame dimension, or the
    /// first partition size is zero or exceeds the payload.
    pub(crate) fn parse_interframe<'d>(
        data: &'d [u8],
        width: u32,
        height: u32,
        state: &mut Vp8State,
    ) -> CodecResult<(Self, BoolDecoder<'d>)> {
        if data.len() < 3 {
            return Err(CodecError::InvalidBitstream(
                "VP8: payload too small for an inter-frame tag".to_string(),
            ));
        }

        // --- 3-byte uncompressed frame tag (RFC 6386 §9.1) ---
        let tag = u32::from(data[0]) | (u32::from(data[1]) << 8) | (u32::from(data[2]) << 16);
        let key_frame = (tag & 1) == 0;
        let version = ((tag >> 1) & 0x7) as u8;
        let _show_frame = ((tag >> 4) & 1) != 0;
        let first_partition_size = ((tag >> 5) & 0x7_FFFF) as usize;

        if key_frame {
            return Err(CodecError::InvalidBitstream(
                "VP8: expected an inter frame, got a key frame".to_string(),
            ));
        }
        if version > 3 {
            return Err(CodecError::InvalidBitstream(
                "VP8: unsupported bitstream version".to_string(),
            ));
        }
        if width == 0 || height == 0 {
            return Err(CodecError::InvalidBitstream(
                "VP8: zero frame dimension".to_string(),
            ));
        }

        // Inter frames have no start code / dimensions block: the header
        // (compressed) partition starts right after the 3-byte tag (dixie.c
        // `FRAME_HEADER_SZ = 3`, only key frames add `KEYFRAME_HEADER_SZ =
        // 7` more, rfc6386.txt lines 7712-7716).
        let header_end = 3usize;
        let partitions_start = header_end
            .checked_add(first_partition_size)
            .ok_or_else(|| {
                CodecError::InvalidBitstream("VP8: first partition size overflow".to_string())
            })?;
        if first_partition_size == 0 || partitions_start > data.len() {
            return Err(CodecError::InvalidBitstream(
                "VP8: first partition exceeds payload".to_string(),
            ));
        }

        state.ensure_segment_map_size(
            (width as usize).div_ceil(16) * (height as usize).div_ceil(16),
        );

        // The boolean decoder operates on the first (header) partition only.
        let header_partition = &data[header_end..partitions_start];
        let mut bd = BoolDecoder::new(header_partition);

        // --- segmentation header (RFC 6386 §9.3, §10) ---
        let segment = parse_segmentation_inter(&mut bd, state);

        // --- loop-filter header (RFC 6386 §9.4) ---
        let loop_filter = parse_loop_filter_inter(&mut bd, state);

        // --- token partition count (RFC 6386 §9.5) ---
        let log2_partitions = bd.get_literal(2);
        let num_token_partitions = 1usize << log2_partitions;

        // --- quantiser indices (RFC 6386 §9.6) --- identical bitstream
        // shape for both frame types (dixie.c `decode_quantizer_header`,
        // lines 7811-7827, does not branch on `is_keyframe` at all: every
        // field is always fully respecified this frame, so there is no
        // persistence to model and the key-frame helper is reused verbatim).
        let quant = parse_quant(&mut bd);

        // --- refresh golden/altref/copy/sign-bias/entropy/last (RFC 6386
        // §9.7-§9.8; §19.2 lines 6839-6849) ---
        let refresh_golden = bd.get_flag();
        let refresh_alt = bd.get_flag();
        let copy_to_golden = if refresh_golden {
            0
        } else {
            bd.get_literal(2) as u8
        };
        let copy_to_alt = if refresh_alt {
            0
        } else {
            bd.get_literal(2) as u8
        };
        let sign_bias_golden = bd.get_flag();
        let sign_bias_alt = bd.get_flag();
        state.sign_bias.golden = sign_bias_golden;
        state.sign_bias.altref = sign_bias_alt;

        let refresh_entropy_probs = bd.get_flag();
        if !refresh_entropy_probs {
            // Snapshot BEFORE any of this frame's own probability updates
            // below (RFC 6386 §9.9; dixie.c lines 8159-8163: the snapshot
            // runs immediately after this bit and before
            // `decode_entropy_header`). The matching restore is
            // `Vp8State::end_of_frame_entropy_restore` -- not called here;
            // see that function's doc comment for the call-site-wiring
            // scope boundary.
            state.saved_entropy = Some(state.entropy.clone());
        }
        let refresh_last = bd.get_flag();

        // --- DCT-token probability updates (RFC 6386 §9.9, §13.4) ---
        // Base is the *persistent* (carried-over) table -- unlike a key
        // frame, an inter frame does not reset to defaults first.
        state.entropy.coeff_probs = update_coeff_probs(&mut bd, &state.entropy.coeff_probs);
        let coeff_probs = state.entropy.coeff_probs;

        // --- mb_no_skip_coeff / prob_skip_false (RFC 6386 §9.10) ---
        let (mb_no_skip_coeff, prob_skip_false) = parse_skip_prob(&mut bd);

        // --- prob_intra / prob_last / prob_gf (RFC 6386 §9.10) ---
        let prob_intra = bd.get_literal(8) as u8;
        let prob_last = bd.get_literal(8) as u8;
        let prob_gf = bd.get_literal(8) as u8;

        // --- intra_16x16 (ymode) probability update (RFC 6386 §9.10) ---
        if bd.get_flag() {
            for p in &mut state.entropy.ymode_prob {
                *p = bd.get_literal(8) as u8;
            }
        }

        // --- intra_chroma (uv mode) probability update (RFC 6386 §9.10) ---
        if bd.get_flag() {
            for p in &mut state.entropy.uv_mode_prob {
                *p = bd.get_literal(8) as u8;
            }
        }

        // --- motion-vector probability update (RFC 6386 §17.2
        // `update_mvcontexts`, rfc6386.txt lines 6239-6256; bitstream syntax
        // `mv_prob_update()` lines 7206-7219) ---
        // `*p = x ? x<<1 : 1` (line 6252): a decoded literal of 0 does NOT
        // mean "leave unchanged" -- once the per-position update flag has
        // selected a new value, that value is unconditionally assigned (the
        // same flag-gates-a-value shape as the segmentation/loop-filter
        // deltas above). 0 doubled is still 0, which is not a legal VP8
        // probability (the boolean coder requires 1..=255), so the RFC
        // substitutes 1 for that one case.
        for c in 0..2 {
            for i in 0..MV_UPDATE_PROBS[c].len() {
                if bd.get_bool(MV_UPDATE_PROBS[c][i]) {
                    let x = bd.get_literal(7) as u8;
                    state.entropy.mv_probs[c][i] = if x != 0 { x << 1 } else { 1 };
                }
            }
        }

        // RFC 6386 §9.2: colour-space/clamping/scale are key-frame-only
        // fields (§9.2: "Information in this subsection does not appear in
        // interframes"); these are inert defaults for an inter-parsed
        // header (display-hint / redundant-safety fields, unused by
        // reconstruction -- see their doc comments above).
        let header = Self {
            width,
            height,
            version,
            horizontal_scale: 0,
            vertical_scale: 0,
            color_space: 0,
            clamping_required: true,
            segment,
            loop_filter,
            quant,
            coeff_probs,
            mb_no_skip_coeff,
            prob_skip_false,
            partitions_start,
            first_partition_size,
            num_token_partitions,
            is_keyframe: false,
            inter: InterFrameHeader {
                refresh_golden,
                refresh_alt,
                copy_to_golden,
                copy_to_alt,
                sign_bias_golden,
                sign_bias_alt,
                refresh_entropy_probs,
                refresh_last,
                prob_intra,
                prob_last,
                prob_gf,
            },
        };
        Ok((header, bd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The test-only VP8 boolean-arithmetic encoder these tests are built on
    // lives in the shared `dec::testutil` module (relocated there verbatim so
    // that `dec::mv`'s motion-vector tests can use it too).
    use crate::vp8::dec::testutil::TestBoolEncoder;

    #[test]
    fn test_parse_rejects_short_payload() {
        assert!(Vp8Header::parse(&[0u8; 4]).is_err());
    }

    #[test]
    fn test_parse_rejects_bad_start_code() {
        // 64-byte payload, key frame tag, wrong start code.
        let mut data = vec![0u8; 64];
        let part_size: u32 = 32;
        let tag = part_size << 5;
        data[0] = (tag & 0xFF) as u8;
        data[1] = ((tag >> 8) & 0xFF) as u8;
        data[2] = ((tag >> 16) & 0xFF) as u8;
        data[3] = 0x00; // wrong start code
        assert!(Vp8Header::parse(&data).is_err());
    }

    #[test]
    fn test_parse_rejects_inter_frame() {
        let mut data = vec![0u8; 64];
        data[0] = 1; // bit0 = 1 => inter frame
        assert!(Vp8Header::parse(&data).is_err());
    }

    #[test]
    fn test_parse_rejects_zero_first_partition() {
        // Valid tag + start code + dims, but first_partition_size = 0.
        let data = [0x10, 0x00, 0x00, 0x9D, 0x01, 0x2A, 0x40, 0x01, 0xF0, 0x00];
        assert!(Vp8Header::parse(&data).is_err());
    }

    #[test]
    fn test_segment_header_default() {
        let seg = SegmentHeader::default();
        assert!(!seg.enabled);
        assert_eq!(seg.quantizer, [0; MAX_SEGMENTS]);
    }

    // === Part 2 diagnostics: what do the two real fixtures actually
    // exercise? Run with `--ignored --nocapture` to inspect; not itself an
    // assertion-bearing test (kept `#[ignore]` so it never runs in CI, but
    // stays compiled so it cannot silently rot).
    #[test]
    #[ignore = "diagnostic only: run with `--ignored --nocapture` to inspect fixture header contents"]
    fn diag_dump_fixture_inter_fields() {
        let p5basic_frame1 = include_bytes!("testdata/p5basic.frame1.bin");
        let refswap_er_frame2 = include_bytes!("testdata/refswap_er.frame2.bin");

        let mut state = Vp8State::new();
        let (h1, _) =
            Vp8Header::parse_interframe(p5basic_frame1, 96, 64, &mut state).expect("parses");
        eprintln!(
            "p5basic.frame1: prob_intra={} prob_last={} prob_gf={} ymode={:?} uv={:?} mv_row0={:?} mv_row1={:?}",
            h1.inter.prob_intra,
            h1.inter.prob_last,
            h1.inter.prob_gf,
            state.entropy.ymode_prob,
            state.entropy.uv_mode_prob,
            state.entropy.mv_probs[0],
            state.entropy.mv_probs[1],
        );

        let mut state2 = Vp8State::new();
        let (h2, _) =
            Vp8Header::parse_interframe(refswap_er_frame2, 96, 64, &mut state2).expect("parses");
        eprintln!(
            "refswap_er.frame2: prob_intra={} prob_last={} prob_gf={} ymode={:?} uv={:?} mv_row0={:?} mv_row1={:?}",
            h2.inter.prob_intra,
            h2.inter.prob_last,
            h2.inter.prob_gf,
            state2.entropy.ymode_prob,
            state2.entropy.uv_mode_prob,
            state2.entropy.mv_probs[0],
            state2.entropy.mv_probs[1],
        );
    }

    // =====================================================================
    // Acceptance tests (against real fixture bytes; see
    // crates/oximedia-codec/src/vp8/dec/testdata/README.md for provenance).
    // =====================================================================

    /// A non-default sentinel entropy context, distinguishable from
    /// [`EntropyContext::defaults`] in every field, used to prove the
    /// `saved_entropy` snapshot captures the *pre-update* value exactly,
    /// not merely `Some(_)`.
    fn sentinel_entropy() -> EntropyContext {
        let mut e = EntropyContext::defaults();
        e.coeff_probs[0][0][0][0] = 7;
        e.coeff_probs[3][7][2][10] = 250;
        e.ymode_prob = [1, 2, 3, 4];
        e.uv_mode_prob = [5, 6, 7];
        e.mv_probs[0][0] = 9;
        e.mv_probs[1][18] = 200;
        e
    }

    // -- acceptance 1: p5basic.frame1.bin parses cleanly, the bool decoder
    // stays inside the first partition, and every statically-visible field
    // matches p5basic.layout.txt's independent parse of frame index 1
    // (`qi=12 seg=0 g=0,a=0,ent=1,last=1 copy_g=0,copy_a=0 sb_g=0,sb_a=0
    // nparts=1`, `mb_lf_adjustments: enabled=1, delta_update=0`). ---------

    #[test]
    fn test_parse_interframe_p5basic_frame1_matches_layout_txt() {
        let data = include_bytes!("testdata/p5basic.frame1.bin");
        let mut state = Vp8State::new();

        let (header, bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state)
            .expect("p5basic.frame1.bin must parse as a valid inter-frame header");

        assert!(!header.is_keyframe);
        assert_eq!(header.width, 96);
        assert_eq!(header.height, 64);

        // The decoder must still be positioned inside the first partition:
        // bytes consumed so far must not exceed the partition's own
        // declared size, and a non-empty token partition must follow it.
        // (RFC 6386 §7.3 permits reading past a partition's end without
        // erroring -- a field-order bug would silently desync rather than
        // fail outright, which is exactly what this catches: the
        // coefficient-probability loop alone gates on 1056 bools, so any
        // upstream field-order error overshoots this bound by a wide
        // margin.)
        assert!(
            bd.position() <= header.first_partition_size,
            "bool decoder overran the first partition: position={} first_partition_size={}",
            bd.position(),
            header.first_partition_size
        );
        assert!(
            header.partitions_start < data.len(),
            "a non-empty token partition must follow the header partition"
        );

        assert_eq!(header.quant.y_ac_qi, 12, "qi=12");
        assert!(!header.segment.enabled, "seg=0");
        assert!(!header.inter.refresh_golden, "g=0");
        assert!(!header.inter.refresh_alt, "a=0");
        assert_eq!(header.inter.copy_to_golden, 0, "copy_g=0");
        assert_eq!(header.inter.copy_to_alt, 0, "copy_a=0");
        assert!(!header.inter.sign_bias_golden, "sb_g=0");
        assert!(!header.inter.sign_bias_alt, "sb_a=0");
        assert!(header.inter.refresh_entropy_probs, "ent=1");
        assert!(header.inter.refresh_last, "last=1");
        assert_eq!(header.num_token_partitions, 1, "nparts=1");
        assert!(
            header.loop_filter.delta_enabled,
            "mb_lf_adjustments.enabled=1"
        );

        assert!(!state.sign_bias.golden);
        assert!(!state.sign_bias.altref);
        assert!(
            state.saved_entropy.is_none(),
            "refresh_entropy_probs=1 must not take a snapshot"
        );
    }

    // -- lf_deltas inheritance (RFC 6386 §9.4): p5basic.frame1's
    // `delta_update=0` means the ref/mode-delta update pass does not run at
    // all, so a correct parser must leave whatever was already in
    // `state.lf_deltas` completely untouched. This is the fix for the
    // "P1-era code builds a fresh `LoopFilterHeader` per frame" latent bug
    // (see `parse_loop_filter_inter`'s doc comment), proven against real
    // encoder bytes, not just the parser's own logic in isolation. --------

    #[test]
    fn test_parse_interframe_p5basic_frame1_inherits_lf_deltas_realistic() {
        let data = include_bytes!("testdata/p5basic.frame1.bin");
        let mut state = Vp8State::new();
        // What p5basic's own key frame (frame 0) actually left behind, per
        // p5basic.layout.txt frame index 0's `mb_lf_adjustments`.
        state.lf_deltas = LoopFilterDeltas {
            ref_deltas: [2, 0, -2, -2],
            mode_deltas: [4, -2, 2, 4],
            enabled: true,
        };

        let (header, _bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state).expect("parses");

        assert_eq!(state.lf_deltas.ref_deltas, [2, 0, -2, -2]);
        assert_eq!(state.lf_deltas.mode_deltas, [4, -2, 2, 4]);
        assert_eq!(header.loop_filter.ref_deltas, [2, 0, -2, -2]);
        assert_eq!(header.loop_filter.mode_deltas, [4, -2, 2, 4]);
    }

    #[test]
    fn test_parse_interframe_p5basic_frame1_inherits_lf_deltas_sentinel() {
        // Same frame, seeded with values no real encoder would ever
        // produce, to prove this is genuine pass-through and not a
        // coincidental re-derivation of the "realistic" values above.
        let data = include_bytes!("testdata/p5basic.frame1.bin");
        let mut state = Vp8State::new();
        state.lf_deltas = LoopFilterDeltas {
            ref_deltas: [7, 7, 7, 7],
            mode_deltas: [-9, -9, -9, -9],
            enabled: true,
        };

        let (header, _bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state).expect("parses");

        assert_eq!(state.lf_deltas.ref_deltas, [7, 7, 7, 7]);
        assert_eq!(state.lf_deltas.mode_deltas, [-9, -9, -9, -9]);
        assert_eq!(header.loop_filter.ref_deltas, [7, 7, 7, 7]);
        assert_eq!(header.loop_filter.mode_deltas, [-9, -9, -9, -9]);
    }

    // -- acceptance 2: refswap_er.frame2.bin. Cross-checked against
    // refswap_er.layout.txt frame index 2 (`qi=12 seg=1 g=0,a=0,ent=0,last=1
    // copy_g=0,copy_a=0 sb_g=0,sb_a=0 nparts=1`; segmentation
    // `update_mb_segmentation_map=1 update_segment_feature_data=1
    // segment_feature_mode=0 quantizer_update=[None,-6,None,None]
    // loop_filter_update=[None,None,None,None] segment_prob=[255,255,255]`;
    // `mb_lf_adjustments: enabled=1 delta_update=1
    // ref_frame_deltas=[2,0,-2,-2] mode_deltas=[4,-2,2,4]`) -- this frame's
    // `update_data=1`/`update_map=1` passes both run, so every segmentation
    // item is unconditionally reassigned this frame (RFC 6386 §9.3 item 5 /
    // dixie.c `bool_maybe_get_int`: an unflagged item within an active
    // update pass becomes a hard `0`/`255`, not "inherited" -- see
    // `parse_segmentation_inter`'s doc comment). `ent=0` is this fixture's
    // reason for existing: the `refresh_entropy_probs == 0` snapshot/
    // restore round trip. -------------------------------------------------

    #[test]
    fn test_parse_interframe_refswap_er_frame2_matches_layout_txt() {
        let data = include_bytes!("testdata/refswap_er.frame2.bin");
        let mut state = Vp8State::new();

        let (header, bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state)
            .expect("refswap_er.frame2.bin must parse as a valid inter-frame header");

        assert!(!header.is_keyframe);
        assert!(
            bd.position() <= header.first_partition_size,
            "bool decoder overran the first partition"
        );
        assert!(header.partitions_start < data.len());

        assert_eq!(header.quant.y_ac_qi, 12, "qi=12");
        assert!(header.segment.enabled, "seg=1");
        assert!(header.segment.update_map, "update_mb_segmentation_map=1");
        assert!(
            !header.segment.abs_delta,
            "segment_feature_mode=0 (raw bit) => delta mode, matching this crate's existing \
             bit-value convention (bit stored directly as `abs_delta`; see dixie.c `hdr->abs = \
             bool_get_bit(bool)` and this file's pre-existing `parse_segmentation`) -- note RFC \
             6386 §9.3 item 4a's prose is inverted versus its own §19.2 field-description table \
             and dixie.c, a known erratum this crate does not repeat"
        );
        assert_eq!(
            header.segment.quantizer,
            [0, -6, 0, 0],
            "quantizer_update=[None,-6,None,None]"
        );
        assert_eq!(
            header.segment.filter_strength,
            [0, 0, 0, 0],
            "loop_filter_update=[None,None,None,None]"
        );
        assert_eq!(
            header.segment.tree_probs,
            [255, 255, 255],
            "segment_prob=[255,255,255]"
        );
        assert!(header.loop_filter.delta_enabled);
        assert_eq!(header.loop_filter.ref_deltas, [2, 0, -2, -2]);
        assert_eq!(header.loop_filter.mode_deltas, [4, -2, 2, 4]);
        assert!(!header.inter.refresh_golden, "g=0");
        assert!(!header.inter.refresh_alt, "a=0");
        assert_eq!(header.inter.copy_to_golden, 0, "copy_g=0");
        assert_eq!(header.inter.copy_to_alt, 0, "copy_a=0");
        assert!(!header.inter.sign_bias_golden, "sb_g=0");
        assert!(!header.inter.sign_bias_alt, "sb_a=0");
        assert!(!header.inter.refresh_entropy_probs, "ent=0");
        assert!(header.inter.refresh_last, "last=1");
        assert_eq!(header.num_token_partitions, 1, "nparts=1");

        assert_eq!(state.segment.quant_deltas, [0, -6, 0, 0]);
        assert_eq!(state.segment.lf_deltas, [0, 0, 0, 0]);
        assert_eq!(state.segment.tree_probs, [255, 255, 255]);
        assert_eq!(state.lf_deltas.ref_deltas, [2, 0, -2, -2]);
        assert_eq!(state.lf_deltas.mode_deltas, [4, -2, 2, 4]);
    }

    #[test]
    fn test_parse_interframe_refswap_er_frame2_entropy_snapshot_restore_roundtrip() {
        let data = include_bytes!("testdata/refswap_er.frame2.bin");
        let mut state = Vp8State::new();
        let pre_update = sentinel_entropy();
        state.entropy = pre_update.clone();

        let (header, _bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state).expect("parses");
        assert!(
            !header.inter.refresh_entropy_probs,
            "ent=0 is the whole reason this fixture was chosen"
        );

        // Must be byte-identical to the *pre-update* sentinel, not merely
        // `Some(_)` -- this is what would catch a snapshot mistakenly taken
        // after (rather than before) this frame's own coeff/ymode/uv/mv
        // probability updates were applied.
        assert_eq!(
            state.saved_entropy,
            Some(pre_update.clone()),
            "saved_entropy must be exactly the pre-update snapshot"
        );

        // The live table was (in general) mutated by this frame's own
        // updates -- expected, decode of *this* frame must use them.
        // Simulate decode continuing to adapt it further, then restore.
        state.entropy.coeff_probs[2][1][0][3] = 77;
        assert_ne!(
            state.entropy, pre_update,
            "sanity: further mutation is visible"
        );

        state.end_of_frame_entropy_restore();

        assert_eq!(
            state.entropy, pre_update,
            "restore must reproduce the pre-update snapshot byte-for-byte"
        );
        assert!(state.saved_entropy.is_none());
    }

    // =====================================================================
    // update_coeff_probs / parse_skip_prob: the two loops shared verbatim
    // between the keyframe and inter-frame paths.
    // =====================================================================

    #[test]
    fn test_update_coeff_probs_zero_input_is_base_pass_through() {
        // An all-zero byte stream makes `value` stay pinned at 0 through any
        // number of renormalisations (RFC 6386 §7.3: `next_byte` returns 0
        // past the end too), so `value < big_split` holds for *every*
        // `prob` in `1..=255` -- every `get_bool` call decodes `false`,
        // regardless of which probability table drives it. No update fires,
        // so the result must equal `base` exactly. Checked against two
        // different bases to also confirm this does not just always emit
        // `DEFAULT_COEFF_PROBS` regardless of what `base` was.
        let zeros = [0u8; 8];

        let mut bd = BoolDecoder::new(&zeros);
        assert_eq!(
            update_coeff_probs(&mut bd, &DEFAULT_COEFF_PROBS),
            DEFAULT_COEFF_PROBS
        );

        let mut other_base = DEFAULT_COEFF_PROBS;
        other_base[1][2][0][5] = 111;
        other_base[3][7][2][10] = 3;
        let mut bd2 = BoolDecoder::new(&zeros);
        assert_eq!(
            update_coeff_probs(&mut bd2, &other_base),
            other_base,
            "must pass a non-default base through untouched, not silently reset to defaults"
        );
    }

    #[test]
    fn test_parse_skip_prob_disabled_reads_only_the_flag() {
        let zeros = [0u8; 8];
        let mut bd = BoolDecoder::new(&zeros);
        let (mb_no_skip_coeff, prob_skip_false) = parse_skip_prob(&mut bd);
        assert!(!mb_no_skip_coeff);
        assert_eq!(prob_skip_false, 0);
    }

    #[test]
    fn test_parse_skip_prob_enabled_reads_the_probability() {
        let mut e = TestBoolEncoder::new();
        e.write_flag(true);
        e.write_literal(0xA5, 8);
        let bytes = e.finish();

        let mut bd = BoolDecoder::new(&bytes);
        let (mb_no_skip_coeff, prob_skip_false) = parse_skip_prob(&mut bd);
        assert!(mb_no_skip_coeff);
        assert_eq!(prob_skip_false, 0xA5);
    }

    #[test]
    fn test_bool_encoder_roundtrips_through_production_bool_decoder() {
        let mut e = TestBoolEncoder::new();
        e.write_flag(true);
        e.write_flag(false);
        e.write_flag(true);
        e.write_literal(0b1011_0110, 8);
        e.write_signed_literal(42, 7, true);
        e.write_signed_literal(0, 7, false);
        e.write_bool(37, true);
        e.write_bool(200, false);
        e.write_bool(1, true);
        e.write_bool(254, false);
        e.write_literal(19, 5);
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        assert!(d.get_flag());
        assert!(!d.get_flag());
        assert!(d.get_flag());
        assert_eq!(d.get_literal(8), 0b1011_0110);
        assert_eq!(d.get_signed_literal(7), -42);
        assert_eq!(d.get_signed_literal(7), 0);
        assert!(d.get_bool(37));
        assert!(!d.get_bool(200));
        assert!(d.get_bool(1));
        assert!(!d.get_bool(254));
        assert_eq!(d.get_literal(5), 19);
    }

    #[test]
    fn test_bool_encoder_roundtrips_coeff_and_mv_update_gate_shapes() {
        // Exactly the two bulk "all-false against real per-position probs"
        // loops `build_synthetic_interframe` relies on, checked directly.
        let mut e = TestBoolEncoder::new();
        for plane in COEFF_UPDATE_PROBS.iter() {
            for band in plane.iter() {
                for ctx in band.iter() {
                    for &p in ctx.iter() {
                        e.write_bool(p, false);
                    }
                }
            }
        }
        for component in MV_UPDATE_PROBS.iter() {
            for &p in component.iter() {
                e.write_bool(p, false);
            }
        }
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        for plane in COEFF_UPDATE_PROBS.iter() {
            for band in plane.iter() {
                for ctx in band.iter() {
                    for &p in ctx.iter() {
                        assert!(!d.get_bool(p));
                    }
                }
            }
        }
        for component in MV_UPDATE_PROBS.iter() {
            for &p in component.iter() {
                assert!(!d.get_bool(p));
            }
        }
    }

    /// Builds a synthetic (hand-encoded) inter-frame payload exercising
    /// exactly the code paths neither real fixture file triggers -- confirmed
    /// empirically via `diag_dump_fixture_inter_fields` above: p5basic's and
    /// refswap_er's chosen frames both leave `ymode_prob`/`uv_mode_prob` at
    /// RFC defaults (their update flags never fire), and while p5basic.frame1
    /// does exercise one real motion-vector probability update (`x=10` at
    /// `mv_probs[1][0]`, observed doubled to `20`), neither hits the `x == 0`
    /// edge of the RFC §17.2 `x ? x<<1 : 1` quirk. Encoded with
    /// [`TestBoolEncoder`] (see its module documentation in `dec::testutil`
    /// for why an encoder is needed and how it is itself validated).
    /// Everything not under test is
    /// written as "no update" / minimal-valid: segmentation and loop-filter
    /// adjustment disabled, a single token partition, a small quantiser
    /// index, `refresh_golden`/`refresh_alt`/`refresh_last`/
    /// `refresh_entropy_probs` all set (so no copy-buffer bits are read and
    /// no snapshot is taken), and every one of the 1056
    /// coefficient-probability and (except `mv_overrides`) 38
    /// motion-vector-probability update gates written `false` against the
    /// *same* per-position probability the decoder will gate on (decode
    /// correctness only requires encoder and decoder to agree on the
    /// probability used at each position, not that it be any particular
    /// value -- see `test_bool_encoder_roundtrips_coeff_and_mv_update_gate_shapes`).
    fn build_synthetic_interframe(
        ymode_update: Option<[u8; 4]>,
        uv_update: Option<[u8; 3]>,
        mv_overrides: &[(usize, usize, u8)],
    ) -> Vec<u8> {
        let mut e = TestBoolEncoder::new();

        // segmentation_enabled = 0
        e.write_flag(false);
        // filter_type, loop_filter_level(6), sharpness_level(3)
        e.write_flag(false);
        e.write_literal(0, 6);
        e.write_literal(0, 3);
        // loop_filter_adj_enable = 0
        e.write_flag(false);
        // log2_nbr_of_dct_partitions = 0 (1 partition)
        e.write_literal(0, 2);
        // quant_indices(): y_ac_qi(7) + 5x (flag + maybe delta)
        e.write_literal(4, 7);
        for _ in 0..5 {
            e.write_flag(false);
        }
        // refresh_golden, refresh_alt (both 1: no copy-buffer bits follow)
        e.write_flag(true);
        e.write_flag(true);
        // sign_bias_golden, sign_bias_alt
        e.write_flag(false);
        e.write_flag(false);
        // refresh_entropy_probs = 1 (no snapshot)
        e.write_flag(true);
        // refresh_last
        e.write_flag(true);
        // token_prob_update(): every gate false.
        for plane in COEFF_UPDATE_PROBS.iter() {
            for band in plane.iter() {
                for ctx in band.iter() {
                    for &update_prob in ctx.iter() {
                        e.write_bool(update_prob, false);
                    }
                }
            }
        }
        // mb_no_skip_coeff = 0
        e.write_flag(false);
        // prob_intra, prob_last, prob_gf (arbitrary valid bytes)
        e.write_literal(200, 8);
        e.write_literal(1, 8);
        e.write_literal(255, 8);
        // intra_16x16 (ymode) update
        match ymode_update {
            Some(probs) => {
                e.write_flag(true);
                for p in probs {
                    e.write_literal(u32::from(p), 8);
                }
            }
            None => e.write_flag(false),
        }
        // intra_chroma (uv) update
        match uv_update {
            Some(probs) => {
                e.write_flag(true);
                for p in probs {
                    e.write_literal(u32::from(p), 8);
                }
            }
            None => e.write_flag(false),
        }
        // mv_prob_update(): every gate false, except the requested overrides.
        for (c, component) in MV_UPDATE_PROBS.iter().enumerate() {
            for (i, &update_prob) in component.iter().enumerate() {
                if let Some(&(_, _, x)) =
                    mv_overrides.iter().find(|&&(oc, oi, _)| oc == c && oi == i)
                {
                    e.write_bool(update_prob, true);
                    e.write_literal(u32::from(x), 7);
                } else {
                    e.write_bool(update_prob, false);
                }
            }
        }

        let partition = e.finish();
        let first_partition_size = partition.len() as u32;
        let tag = (first_partition_size << 5) | (1 << 4) | 1; // inter, shown
        let mut payload = vec![
            (tag & 0xFF) as u8,
            ((tag >> 8) & 0xFF) as u8,
            ((tag >> 16) & 0xFF) as u8,
        ];
        payload.extend_from_slice(&partition);
        payload
    }

    #[test]
    fn test_parse_interframe_applies_ymode_and_uv_update_when_flagged() {
        let ymode = [10, 20, 30, 40];
        let uv = [50, 60, 70];
        let payload = build_synthetic_interframe(Some(ymode), Some(uv), &[]);

        let mut state = Vp8State::new();
        let (header, _bd) = Vp8Header::parse_interframe(&payload, 16, 16, &mut state)
            .expect("synthetic inter-frame header must parse");

        assert!(!header.is_keyframe);
        assert_eq!(state.entropy.ymode_prob, ymode);
        assert_eq!(state.entropy.uv_mode_prob, uv);
    }

    #[test]
    fn test_parse_interframe_leaves_ymode_and_uv_unchanged_when_not_flagged() {
        let payload = build_synthetic_interframe(None, None, &[]);

        let mut state = Vp8State::new();
        state.entropy.ymode_prob = [1, 2, 3, 4];
        state.entropy.uv_mode_prob = [5, 6, 7];

        Vp8Header::parse_interframe(&payload, 16, 16, &mut state)
            .expect("synthetic inter-frame header must parse");

        assert_eq!(
            state.entropy.ymode_prob,
            [1, 2, 3, 4],
            "unflagged ymode update must inherit, not reset to RFC defaults"
        );
        assert_eq!(
            state.entropy.uv_mode_prob,
            [5, 6, 7],
            "unflagged uv update must inherit, not reset to RFC defaults"
        );
    }

    #[test]
    fn test_parse_interframe_mv_prob_update_zero_becomes_one() {
        // RFC 6386 §17.2 `update_mvcontexts`, rfc6386.txt line 6252:
        // `*p = x ? x<<1 : 1;`. Confirmed against real bytes for a nonzero
        // `x` by `diag_dump_fixture_inter_fields` above (p5basic.frame1
        // decodes x=10 at mv_probs[1][0], observed doubled to 20); this
        // test covers the `x == 0` edge neither fixture happens to hit.
        let payload = build_synthetic_interframe(None, None, &[(0, 0, 0), (1, 5, 10)]);

        let mut state = Vp8State::new();
        Vp8Header::parse_interframe(&payload, 16, 16, &mut state).expect("parses");

        assert_eq!(
            state.entropy.mv_probs[0][0], 1,
            "x=0 must decode to probability 1, not 0 (0 is not a legal VP8 probability)"
        );
        assert_eq!(
            state.entropy.mv_probs[1][5], 20,
            "x=10 must decode to 10<<1 = 20"
        );
    }
}

//! Real VP9 frame decoding (8-bit, 4:2:0) and the decoder state that spans
//! frames.
//!
//! Key frames, intra-only frames and inter frames all reconstruct here, as an
//! exact port of libvpx's decode path (`vp9/decoder/*`, `vp9/common/*`,
//! `vpx_dsp/*`, 8-bit build), verified bit-exact against `ffmpeg`/libvpx
//! reference decodes of real encoder output.
//!
//! The proof is `inter_fixture_tests` (this module's `cfg(test)` sibling):
//! ten real `libvpx-vp9` streams —
//! frozen-source and moving content, error-resilient and adapting, one and
//! two tile columns, high-precision motion vectors, hidden ALTREF frames
//! inside real superframes, compound prediction, a `show_existing_frame`
//! redisplay, and non-8-aligned dimensions with a partial last superblock
//! row — driven whole through the public [`Vp9Decoder`](crate::vp9::Vp9Decoder)
//! API, every emitted frame byte-identical to libvpx's own reconstruction.
//! The four single-key-frame tests at the bottom of this file and [`recon`]'s
//! own multi-frame test cover the same ground at a smaller granularity, so a
//! regression is attributable rather than merely detectable.
//!
//! Two features of an inter frame are still refused rather than approximated,
//! each with a precise [`CodecError::UnsupportedFeature`] and each pinned to
//! the exact fixture packet it fires on: **segmentation** (which needs the
//! previous frame's segment map and this frame's write-back — [`modeinfo`];
//! *intra*-frame segmentation is fully implemented) and **reference
//! scaling**, a reference whose display size differs from the frame's
//! ([`interpred`]).
//!
//! Layout:
//! - [`booldec`]: VP9 boolean (range) decoder (`vpx_dsp/bitreader`).
//! - [`state`]: cross-frame decoder state — the four probability contexts,
//!   the decoded-picture buffer, and the loop-filter / segmentation values a
//!   frame may inherit instead of retransmitting.
//! - [`refs`]: reference-frame slots, `refresh_frame_flags` application and
//!   compound-reference derivation.
//! - [`counts`]: symbol counters for backward probability adaptation.
//! - [`hdr`]: compressed-header parse (tx mode, coefficient / skip / mode /
//!   reference / motion-vector probability updates) and the subexp
//!   probability-update machinery.
//! - [`tables`] / [`scan`]: constant tables mechanically extracted from
//!   libvpx sources (kf mode/partition probabilities, default coefficient
//!   probabilities, Pareto model tree, quantizer lookups, scan orders and
//!   neighbor tables, block-size lookups).
//! - [`itx`]: inverse DCT/ADST/WHT transforms.
//! - [`mc`]: motion-compensation interpolation kernels (`vpx_dsp/vpx_convolve.c`)
//!   — copy/average/horizontal/vertical/full-2-D sub-pixel prediction.
//! - [`interpred`]: inter-prediction build (`vp9/decoder/vp9_decodeframe.c`'s
//!   `dec_build_inter_predictors*`, `vp9/common/vp9_reconinter.c`) — the
//!   MC-stage motion-vector clamp, sub-8x8 chroma vector averaging, and the
//!   two-path (direct / edge-replicated) reference fetch that drives [`mc`].
//! - [`mvref`]: motion-vector reference candidate scan
//!   (`vp9/common/vp9_mvref_common.c`) — the spatial and temporal neighbour
//!   search that produces a block's `nearestmv` / `nearmv` predictors and the
//!   context its inter mode is decoded with.
//! - [`pred`]: intra predictors and border construction.
//! - [`predctx`]: prediction contexts (`vp9/common/vp9_pred_common.c`) — the
//!   above/left entropy-context derivations an inter frame needs before it can
//!   read its mode, reference, filter, segment, skip and tx-size symbols.
//! - [`modeinfo`]: inter-frame mode info (`vp9_decodemv.c`'s inter half) —
//!   reference frames, inter modes, interpolation filter and motion vectors,
//!   plus the transform-size read both frame types share.
//! - [`adapt`] / [`counts`]: the symbol counters and the backward
//!   probability adaptation that folds them into the frame context.
//! - [`recon`]: tile/partition/block decode driver, for both frame types.
//! - [`lf`]: loop filter (levels, masks, kernels).

mod adapt;
mod booldec;
mod counts;
mod hdr;
mod interpred;
mod itx;
mod lf;
mod mc;
mod modeinfo;
mod mvref;
mod pred;
mod predctx;
mod recon;
mod refs;
mod scan;
mod state;
mod tables;
mod tables_inter;

#[cfg(test)]
mod boolenc;
#[cfg(test)]
mod inter_fixture_tests;
#[cfg(test)]
mod testutil;

pub(crate) use recon::{decode_frame, DecodedFrame, InterRefs};
pub(crate) use refs::Vp9RefSlot;
pub(crate) use state::Vp9DecState;

use crate::error::{CodecError, CodecResult};
use crate::vp9::uncompressed::UncompressedHeader;

/// Validates the profile / geometry scope shared by every entry point.
fn check_frame_scope(hdr: &UncompressedHeader) -> CodecResult<()> {
    // TODO(0.2.x): profiles 1-3 — 4:2:2 / 4:4:4 subsampling and 10/12-bit
    // depths (highbd transform/predictor/loop-filter variants).
    if hdr.bit_depth != 8 || !hdr.subsampling_x || !hdr.subsampling_y {
        return Err(CodecError::UnsupportedFeature(format!(
            "VP9 decode supports 8-bit 4:2:0 (profile 0); \
             got {}-bit ss_x={} ss_y={} (profile {})",
            hdr.bit_depth, hdr.subsampling_x, hdr.subsampling_y, hdr.profile
        )));
    }
    if hdr.width == 0 || hdr.height == 0 {
        return Err(CodecError::InvalidBitstream(
            "VP9: zero frame dimensions".into(),
        ));
    }
    Ok(())
}

/// Decodes one VP9 intra frame (key or intra-only) against `state`'s working
/// probability context, accumulating the backward-adaptation counters when
/// the frame is one that will adapt.
///
/// There is deliberately no defaults-only shortcut alongside this: it would
/// be right for a key frame (whose `vp9_setup_past_independence` reset
/// installs the defaults in every frame context before the load, making "the
/// loaded context" and "the defaults" the same thing) and silently *wrong*
/// for an intra-only frame, which loads whatever an earlier frame adapted
/// into `frame_contexts[frame_context_idx]`. Both go through the state.
///
/// The caller must have run [`Vp9DecState::begin_frame`] first: that is what
/// performs the past-independence reset, the loop-filter / segmentation
/// merges and the `frame_contexts[frame_context_idx]` load this function then
/// decodes against. On return, `state.fc` holds the context the frame really
/// used (loaded plus its own compressed-header updates) and `state.counts`
/// holds the symbols it coded, which is precisely what
/// [`Vp9DecState::finish_frame`] needs.
///
/// Supported scope: profile 0 (8-bit, 4:2:0). Other profiles and bit depths
/// return an honest [`CodecError::UnsupportedFeature`].
///
/// # Errors
///
/// Returns an error for unsupported profiles/formats or malformed data.
pub(crate) fn decode_intra_frame_with_state(
    state: &mut Vp9DecState,
    hdr: &UncompressedHeader,
    frame_data: &[u8],
) -> CodecResult<DecodedFrame> {
    check_frame_scope(hdr)?;
    if !hdr.is_intra_only() {
        return Err(CodecError::InvalidParameter(
            "VP9: decode_intra_frame_with_state called for an inter frame — \
             use decode_inter_frame_with_state"
                .into(),
        ));
    }
    let Vp9DecState {
        fc,
        counts,
        tx_mode,
        ..
    } = state;
    let counts = if Vp9DecState::counts_enabled(hdr) {
        Some(counts)
    } else {
        None
    };
    let decoded = decode_frame(hdr, frame_data, fc, counts, None)?;
    *tx_mode = decoded.tx_mode;
    Ok(decoded)
}

/// Decodes one VP9 **inter** frame against `state`'s working probability
/// context, its decoded-picture buffer and (when
/// [`Vp9DecState::use_prev_frame_mvs`] holds) the previous frame's motion
/// vectors.
///
/// Like [`decode_intra_frame_with_state`], the caller must have run
/// [`Vp9DecState::begin_frame`] first. On return `state.fc` holds the context
/// the frame really used, `state.counts` the symbols it coded and
/// `state.tx_mode` the transform mode its compressed header chose — which is
/// exactly what [`Vp9DecState::finish_frame`] needs.
///
/// The three active references are resolved here, once:
/// `dpb[hdr.ref_frame_idx[i]]` for LAST / GOLDEN / ALTREF. A slot that the
/// stream names but never wrote stays `None`; motion compensation reports
/// that honestly if a block actually predicts from it, and a frame whose
/// blocks all avoid it decodes normally — which is what libvpx does too.
///
/// # Errors
///
/// * [`CodecError::InvalidParameter`] when `hdr` is not an inter frame.
/// * [`CodecError::UnsupportedFeature`] for an unsupported profile, for
///   inter-frame segmentation, or for a reference whose display dimensions
///   differ from this frame's (reference scaling).
/// * [`CodecError::InvalidBitstream`] on malformed data.
pub(crate) fn decode_inter_frame_with_state(
    state: &mut Vp9DecState,
    hdr: &UncompressedHeader,
    frame_data: &[u8],
) -> CodecResult<DecodedFrame> {
    check_frame_scope(hdr)?;
    if hdr.is_intra_only() {
        return Err(CodecError::InvalidParameter(
            "VP9: decode_inter_frame_with_state called for a key / intra-only \
             frame — use decode_intra_frame_with_state"
                .into(),
        ));
    }
    // `cm->use_prev_frame_mvs` reads `last_*`, which the destructure below
    // borrows away, so it is evaluated first.
    let use_prev_mvs = state.use_prev_frame_mvs(hdr);
    let Vp9DecState {
        fc,
        counts,
        tx_mode,
        dpb,
        prev_frame,
        ..
    } = state;

    let mut active: [Option<&Vp9RefSlot>; refs::REFS_PER_FRAME] = [None; refs::REFS_PER_FRAME];
    for (slot, &idx) in active.iter_mut().zip(hdr.ref_frame_idx.iter()) {
        *slot = dpb.get(usize::from(idx)).and_then(Option::as_ref);
    }
    let prev_mvs = if use_prev_mvs {
        prev_frame.as_ref().map(|f| f.mvs.as_slice())
    } else {
        None
    };

    let counts = if Vp9DecState::counts_enabled(hdr) {
        Some(counts)
    } else {
        None
    };
    let decoded = decode_frame(
        hdr,
        frame_data,
        fc,
        counts,
        Some(InterRefs { active, prev_mvs }),
    )?;
    *tx_mode = decoded.tx_mode;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::testutil::assert_bit_exact_sequence;

    /// Single-keyframe, single-shown-frame call: the multi-frame harness
    /// ([`assert_bit_exact_sequence`], added for VP9 P4's inter fixtures --
    /// see `testutil` and `super::inter_fixture_tests`) generalizes rather
    /// than duplicates the bit-exact check these four tests used before it
    /// existed, so a one-element `frames` slice must reproduce that old
    /// behaviour exactly: header-parse, decode, compare all three planes,
    /// and account for exactly one coded/shown/verified frame.
    fn assert_bit_exact(ivf_frame: &[u8], ref_yuv: &[u8], w: usize, h: usize, label: &str) {
        let check = assert_bit_exact_sequence(&[ivf_frame], ref_yuv, w, h, label);
        assert_eq!(
            (
                check.coded_frames,
                check.shown_frames,
                check.verified_frames
            ),
            (1, 1, 1),
            "{label}: a lone key/intra-only frame must be coded, shown, and verified"
        );
    }

    /// 76x42 keyframe (non-8/64-aligned dimensions), libvpx-vp9 crf 24
    /// (non-zero loop filter level), reference-decoded with ffmpeg's native
    /// VP9 decoder (verified byte-identical to libvpx's own decoder).
    #[test]
    fn keyframe_76x42_crf24_bit_exact_vs_libvpx() {
        assert_bit_exact(
            include_bytes!("testdata/kf76x42.frame0.bin"),
            include_bytes!("testdata/ref76x42.yuv"),
            76,
            42,
            "kf76x42",
        );
    }

    /// 128x128 keyframe, libvpx-vp9 crf 12 (rich texture, multiple
    /// superblocks, all partition shapes).
    #[test]
    fn keyframe_128x128_crf12_bit_exact_vs_libvpx() {
        assert_bit_exact(
            include_bytes!("testdata/kf128.frame0.bin"),
            include_bytes!("testdata/ref128.yuv"),
            128,
            128,
            "kf128",
        );
    }

    /// 64x64 lossless keyframe (WHT transform path, loop filter off).
    #[test]
    fn keyframe_64x64_lossless_bit_exact_vs_libvpx() {
        assert_bit_exact(
            include_bytes!("testdata/kf64ll.frame0.bin"),
            include_bytes!("testdata/ref64ll.yuv"),
            64,
            64,
            "kf64ll",
        );
    }

    /// 512x64 keyframe encoded with two tile columns (`tile_cols_log2` = 1):
    /// exercises tile-size parsing, per-tile bool decoders, and the left
    /// context / intra-availability reset at the tile boundary.
    #[test]
    fn keyframe_512x64_two_tile_columns_bit_exact_vs_libvpx() {
        assert_bit_exact(
            include_bytes!("testdata/kf512tc.frame0.bin"),
            include_bytes!("testdata/ref512tc.yuv"),
            512,
            64,
            "kf512tc",
        );
    }
}

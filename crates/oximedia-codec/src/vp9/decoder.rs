//! VP9 decoder implementation.
//!
//! Key frames, intra-only frames, inter frames and the show-existing-frame
//! mechanism all decode to real pixels via the bit-exact pipeline in
//! [`crate::vp9::dec`] — never a fabricated frame. Two inter-frame features
//! are still refused with a precise
//! [`CodecError::UnsupportedFeature`]: segmentation, and a reference whose
//! display size differs from the frame's (reference scaling).
//!
//! An intra-only frame reconstructs exactly like a key frame (libvpx picks
//! the same `vp9_kf_y_mode_prob` / `vp9_kf_uv_mode_prob` /
//! `vp9_kf_partition_probs` for both — `frame_is_intra_only(cm)` in
//! `vp9_decodemv.c:822` and `set_partition_probs`,
//! `vp9_onyxc_int.h:367-372`), but it does **not** start from the default
//! coefficient/skip/tx probabilities: it loads
//! `frame_contexts[frame_context_idx]`, which an earlier frame may have
//! adapted. That is why the decode goes through
//! [`Vp9DecState`](crate::vp9::dec::Vp9DecState) rather than a
//! defaults-only entry point, and why backward adaptation
//! ([`crate::vp9::dec`]'s `adapt`) had to land with it.
//!
//! An inter frame needs [`Vp9DecState`](crate::vp9::dec::Vp9DecState) for
//! three further reasons, each of which is silent corruption rather than a
//! loud failure if skipped: it names its references by
//! decoded-picture-buffer slot, it may read the *previous* frame's motion
//! vectors as temporal candidates, and it codes no `color_config` at all —
//! its bit depth and chroma subsampling are the ones an earlier frame
//! established.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]

use crate::error::{CodecError, CodecResult};
use crate::frame::{Plane, VideoFrame};
use crate::traits::{DecoderConfig, VideoDecoder};
use crate::vp9::dec::{self, Vp9DecState, Vp9RefSlot};
use crate::vp9::superframe::Superframe;
use crate::vp9::uncompressed::UncompressedHeader;
use oximedia_core::{CodecId, PixelFormat, Rational, Timestamp};

/// VP9 decoder.
///
/// Scope: key-frame, intra-only **and** inter reconstruction is implemented
/// for 8-bit 4:2:0 (profile 0) and verified bit-exact against libvpx. Other
/// profiles, inter-frame segmentation and reference scaling fail honestly.
#[derive(Debug)]
pub struct Vp9Decoder {
    #[allow(dead_code)]
    config: DecoderConfig,
    width: u32,
    height: u32,
    output_format: PixelFormat,
    output_queue: Vec<VideoFrame>,
    /// Everything that outlives a single frame: the decoded-picture buffer,
    /// the four probability contexts and the inherited loop-filter /
    /// segmentation values.
    state: Vp9DecState,
    flushing: bool,
    /// Frame counter.
    frame_count: u64,
}

impl Vp9Decoder {
    /// Creates a new VP9 decoder.
    ///
    /// # Errors
    ///
    /// Returns error if configuration is invalid.
    pub fn new(config: DecoderConfig) -> CodecResult<Self> {
        Ok(Self {
            config,
            width: 0,
            height: 0,
            output_format: PixelFormat::Yuv420p,
            output_queue: Vec::new(),
            state: Vp9DecState::new(),
            flushing: false,
            frame_count: 0,
        })
    }

    /// Decodes a single (non-superframe) VP9 frame payload.
    fn decode_frame(&mut self, data: &[u8], pts: i64) -> CodecResult<()> {
        // The decoded-picture buffer has to be visible *during* the parse: a
        // frame may take its dimensions from a reference slot
        // (`frame_size_with_refs`), and both `render_size()` and the number of
        // bits `tile_info()` reads depend on those dimensions.
        let mut header = UncompressedHeader::parse_with_ref_sizes(data, &self.state.ref_sizes())?;

        if header.show_existing_frame {
            // Re-display a previously decoded reference frame. The header
            // carries nothing but the slot index, so the output geometry
            // comes from the slot itself.
            let idx = header.frame_to_show as usize;
            let Some(slot) = self.state.dpb.get(idx).and_then(Option::as_ref) else {
                return Err(CodecError::InvalidBitstream(format!(
                    "VP9 show_existing_frame: reference slot {idx} holds no decoded frame"
                )));
            };
            let mut output = Self::slot_to_video_frame(slot);
            output.timestamp = Timestamp::new(pts, Rational::new(1, 1000));
            self.output_queue.push(output);
            // No `finish_frame`, and nothing else touched either. A
            // show-existing packet decodes no frame, so the specification
            // gates every end-of-frame update on `show_existing_frame == 0`:
            // the reference refresh (its `refresh_frame_flags` is not even
            // coded — the parser assigns 0), the frame-context save, and
            // §8.10 step 2's previous-motion-vector / `PrevRefFrames` save,
            // which is what `state.prev_frame` and the `last_*` fields
            // stand for. libvpx returns before `swap_frame_buffers`' own
            // `cm->prev_frame` / `cm->last_show_frame` assignments
            // (`vp9_decoder.c:483-486`).
            //
            // `Vp9DecState::finish_frame` rejects a show-existing header
            // outright, so this is enforced there as well as here — the
            // early return is the fast path, not the only guard.
            return Ok(());
        }

        // Real header-parse results: update stream properties.
        if header.width > 0 && header.height > 0 {
            self.width = header.width;
            self.height = header.height;
        }
        let frame_is_intra_only = header.is_intra_only();

        // Cross-frame state: the colour-configuration inheritance, the
        // past-independence reset, the loop-filter and segmentation merges
        // and the probability-context load, in libvpx's order. `header` comes
        // back with the values the decode must use (`frame_context_idx`
        // forced to 0, merged deltas).
        self.state.begin_frame(&mut header)?;

        // *After* `begin_frame`: an inter frame codes no `color_config` at
        // all, so its bit depth and subsampling are the ones the state just
        // replayed into the header, not the parser's zero defaults.
        self.output_format = match (header.bit_depth, header.subsampling_x, header.subsampling_y) {
            (8, true, true) => PixelFormat::Yuv420p,
            (8, true, false) => PixelFormat::Yuv422p,
            (8, false, false) => PixelFormat::Yuv444p,
            (10, true, true) => PixelFormat::Yuv420p10le,
            (12, true, true) => PixelFormat::Yuv420p12le,
            _ => PixelFormat::Yuv420p,
        };

        // Decodes against `state.fc` (the context just loaded, which for an
        // intra-only or inter frame is whatever an earlier frame adapted) and
        // fills `state.counts` for the adaptation `finish_frame` runs.
        let decoded = if frame_is_intra_only {
            dec::decode_intra_frame_with_state(&mut self.state, &header, data)?
        } else {
            dec::decode_inter_frame_with_state(&mut self.state, &header, data)?
        };
        // The reference slot keeps the MI-aligned reconstruction and the
        // per-MI motion-vector records the next frame's temporal candidate
        // scan reads; the output frame is the cropped view of those pixels.
        let slot = Vp9RefSlot::from_decoded_frame(decoded, header.intra_only, header.show_frame)?;
        let mut frame = Self::slot_to_video_frame(&slot);
        frame.timestamp = Timestamp::new(pts, Rational::new(1, 1000));
        self.frame_count += 1;

        // Reference-slot refresh (a key frame's mask is 0xFF), context save,
        // and the `last_*` advance.
        self.state.finish_frame(&header, slot)?;

        if header.show_frame {
            self.output_queue.push(frame);
        }
        Ok(())
    }

    /// Crops a reference slot's MI-aligned planes to display size and
    /// assembles a [`VideoFrame`].
    fn slot_to_video_frame(slot: &Vp9RefSlot) -> VideoFrame {
        let w = slot.width;
        let h = slot.height;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);

        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, w as u32, h as u32);
        let dims = [(w, h), (cw, ch), (cw, ch)];
        for (plane, &(pw, ph)) in slot.planes.iter().zip(dims.iter()) {
            let mut data = Vec::with_capacity(pw * ph);
            for row in 0..ph {
                let start = row * plane.stride;
                data.extend_from_slice(&plane.data[start..start + pw]);
            }
            frame
                .planes
                .push(Plane::with_dimensions(data, pw, pw as u32, ph as u32));
        }
        frame
    }

    /// Returns the number of pending output frames.
    #[must_use]
    pub fn pending_frames(&self) -> usize {
        self.output_queue.len()
    }

    /// Returns true if the decoder has been flushed.
    #[must_use]
    pub fn is_flushing(&self) -> bool {
        self.flushing
    }
}

impl VideoDecoder for Vp9Decoder {
    fn codec(&self) -> CodecId {
        CodecId::Vp9
    }

    #[allow(clippy::cast_possible_wrap)]
    fn send_packet(&mut self, data: &[u8], pts: i64) -> CodecResult<()> {
        if self.flushing {
            return Err(CodecError::InvalidParameter(
                "Cannot send packet while flushing".into(),
            ));
        }

        let superframe = Superframe::parse(data)?;

        for (i, frame_data) in superframe.frames.iter().enumerate() {
            let frame_pts = pts + i as i64;
            self.decode_frame(frame_data, frame_pts)?;
        }

        Ok(())
    }

    fn receive_frame(&mut self) -> CodecResult<Option<VideoFrame>> {
        if self.output_queue.is_empty() {
            if self.flushing {
                return Err(CodecError::Eof);
            }
            return Ok(None);
        }

        Ok(Some(self.output_queue.remove(0)))
    }

    fn flush(&mut self) -> CodecResult<()> {
        self.flushing = true;
        Ok(())
    }

    fn reset(&mut self) {
        self.output_queue.clear();
        // Drops the whole decoded-picture buffer *and* the adapted
        // probability contexts: after a reset the next frame must be a key
        // frame (or an error-resilient one), exactly as for a fresh decoder.
        self.state = Vp9DecState::new();
        self.flushing = false;
    }

    fn output_format(&self) -> Option<PixelFormat> {
        if self.width > 0 && self.height > 0 {
            Some(self.output_format)
        } else {
            None
        }
    }

    fn dimensions(&self) -> Option<(u32, u32)> {
        if self.width > 0 && self.height > 0 {
            Some((self.width, self.height))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vp9_decoder_new() {
        let config = DecoderConfig::default();
        let decoder = Vp9Decoder::new(config).expect("should succeed");
        assert_eq!(decoder.codec(), CodecId::Vp9);
        assert_eq!(decoder.pending_frames(), 0);
        assert!(!decoder.is_flushing());
    }

    #[test]
    fn test_decoder_initial_state() {
        let config = DecoderConfig::default();
        let decoder = Vp9Decoder::new(config).expect("should succeed");
        assert!(decoder.output_format().is_none());
        assert!(decoder.dimensions().is_none());
    }

    #[test]
    fn test_flush() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        assert!(!decoder.is_flushing());
        decoder.flush().expect("should succeed");
        assert!(decoder.is_flushing());
    }

    #[test]
    fn test_reset() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder.flush().expect("should succeed");
        assert!(decoder.is_flushing());
        decoder.reset();
        assert!(!decoder.is_flushing());
    }

    #[test]
    fn test_receive_no_frame() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        let frame = decoder.receive_frame().expect("should succeed");
        assert!(frame.is_none());
    }

    #[test]
    fn test_send_while_flushing() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder.flush().expect("should succeed");
        let result = decoder.send_packet(&[0x80], 0);
        assert!(result.is_err());
    }

    /// Real libvpx-vp9 keyframe (76x42, crf 24) — must decode to real
    /// pixels through the public `VideoDecoder` API and match the
    /// libvpx/ffmpeg reference decode bit-exactly.
    #[test]
    fn test_decode_real_keyframe_bit_exact() {
        const IVF_FRAME: &[u8] = include_bytes!("dec/testdata/kf76x42.frame0.bin");
        const REF_YUV: &[u8] = include_bytes!("dec/testdata/ref76x42.yuv");

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder.send_packet(IVF_FRAME, 0).expect("keyframe decodes");

        assert_eq!(decoder.dimensions(), Some((76, 42)));
        assert_eq!(decoder.output_format(), Some(PixelFormat::Yuv420p));

        let frame = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("one frame output");
        assert_eq!(frame.planes.len(), 3);
        assert_eq!(frame.planes[0].data.len(), 76 * 42);
        assert_eq!(frame.planes[1].data.len(), 38 * 21);
        assert_eq!(frame.planes[2].data.len(), 38 * 21);

        let expected_y = &REF_YUV[..76 * 42];
        let expected_u = &REF_YUV[76 * 42..76 * 42 + 38 * 21];
        let expected_v = &REF_YUV[76 * 42 + 38 * 21..];
        assert_eq!(frame.planes[0].data.as_slice(), expected_y, "Y plane");
        assert_eq!(frame.planes[1].data.as_slice(), expected_u, "U plane");
        assert_eq!(frame.planes[2].data.as_slice(), expected_v, "V plane");
    }

    const KEYFRAME_76X42: &[u8] = include_bytes!("dec/testdata/kf76x42.frame0.bin");
    const REF_YUV_76X42: &[u8] = include_bytes!("dec/testdata/ref76x42.yuv");
    const INTER_FRAME_76X42: &[u8] = include_bytes!("dec/testdata/seq76x42.frame1.bin");

    /// A real libvpx-vp9 INTER frame (frame 1 of a 3-frame encode) decodes
    /// through the public decoder and reaches the output queue at the
    /// sequence's real geometry.
    ///
    /// This fixture has no committed reference reconstruction of its own, so
    /// the assertion here is "the dispatch really runs the inter path and
    /// produces a correctly-shaped frame", not "the pixels are right".
    /// Pixel verification lives in `dec::inter_fixture_tests`, which drives
    /// ten whole real streams through this same public API and requires
    /// every emitted frame to be byte-identical to libvpx's own
    /// reconstruction.
    ///
    /// The key frame goes in first because this inter frame really does take
    /// its dimensions from a reference slot (`frame_size_with_refs` with
    /// `found_ref` set on reference 0) — without a populated decoded-picture
    /// buffer its size is not derivable at all, which the test below covers.
    #[test]
    fn test_inter_frame_decodes_through_the_public_decoder() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(KEYFRAME_76X42, 0)
            .expect("keyframe decodes");
        let _ = decoder.receive_frame().expect("should succeed");

        decoder
            .send_packet(INTER_FRAME_76X42, 1)
            .expect("inter frame decodes");
        let frame = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("the inter frame is shown, so it must be output");
        assert_eq!((frame.width, frame.height), (76, 42));
        assert_eq!(frame.planes.len(), 3);
        assert_eq!(frame.planes[0].data.len(), 76 * 42);
        assert_eq!(frame.planes[1].data.len(), 38 * 21);
        assert_eq!(frame.planes[2].data.len(), 38 * 21);
    }

    /// The same inter frame with no decoded-picture buffer behind it: its
    /// size lives in a reference slot that does not exist, so the parse must
    /// say so instead of silently continuing with a 0x0 frame (the old
    /// `parse_frame_size_with_refs` behaviour, which additionally
    /// desynchronised `tile_info`).
    #[test]
    fn test_inter_frame_without_references_fails_honestly() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        match decoder.send_packet(INTER_FRAME_76X42, 0) {
            Err(CodecError::InvalidBitstream(msg)) => {
                assert!(
                    msg.contains("frame_size_with_refs") && msg.contains("no decoded frame"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected an honest InvalidBitstream, got {other:?}"),
        }
        assert!(decoder.receive_frame().expect("should succeed").is_none());
    }

    /// `frame_size_with_refs` must copy the *referenced slot's* dimensions
    /// into the header — the fix for the parse that recorded `found_ref` and
    /// then left the frame 0x0.
    #[test]
    fn test_frame_size_with_refs_copies_reference_dimensions() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(KEYFRAME_76X42, 0)
            .expect("keyframe decodes");

        let header =
            UncompressedHeader::parse_with_ref_sizes(INTER_FRAME_76X42, &decoder.state.ref_sizes())
                .expect("inter header parses against a populated DPB");

        assert_eq!(
            header.size_from_ref,
            Some(0),
            "this frame copies its size from reference 0"
        );
        assert_eq!((header.width, header.height), (76, 42));
        assert_eq!(
            (header.render_width, header.render_height),
            (76, 42),
            "render_size() copies the frame size, so it must be resolved too"
        );
        assert!(
            header.compressed_header_size > 0,
            "the rest of the header must still parse coherently"
        );

        // Without the DPB the same bits cannot yield a size at all.
        assert!(UncompressedHeader::parse(INTER_FRAME_76X42).is_err());
    }

    /// `show_existing_frame` must re-emit the stored reference slot's pixels
    /// byte for byte, rebuilt from the MI-aligned planes the DPB holds.
    #[test]
    fn test_show_existing_frame_reemits_stored_slot() {
        // One byte: frame_marker=0b10, profile 0, show_existing_frame=1,
        // frame_to_show=0 -> 0b1000_1000.
        const SHOW_EXISTING_SLOT0: [u8; 1] = [0x88];

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(KEYFRAME_76X42, 0)
            .expect("keyframe decodes");
        let first = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("keyframe output");

        decoder
            .send_packet(&SHOW_EXISTING_SLOT0, 1)
            .expect("show_existing_frame decodes");
        let again = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("re-shown output");

        assert_eq!(again.width, first.width);
        assert_eq!(again.height, first.height);
        assert_eq!(again.planes.len(), 3);
        for (i, (a, b)) in again.planes.iter().zip(first.planes.iter()).enumerate() {
            assert_eq!(a.data, b.data, "plane {i} must be byte-identical");
        }
    }

    /// `show_existing_frame` naming an empty slot must fail, not fabricate.
    #[test]
    fn test_show_existing_frame_empty_slot_errors() {
        // frame_to_show = 5 -> 0b1000_1101.
        const SHOW_EXISTING_SLOT5: [u8; 1] = [0x8D];

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        match decoder.send_packet(&SHOW_EXISTING_SLOT5, 0) {
            Err(CodecError::InvalidBitstream(msg)) => {
                assert!(msg.contains("holds no decoded frame"), "{msg}");
            }
            other => panic!("expected InvalidBitstream, got {other:?}"),
        }
        assert!(decoder.receive_frame().expect("should succeed").is_none());
    }

    /// A key frame refreshes all eight reference slots and leaves the
    /// cross-frame state describing itself.
    #[test]
    fn test_keyframe_populates_every_reference_slot_and_state() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(KEYFRAME_76X42, 0)
            .expect("keyframe decodes");

        for (i, slot) in decoder.state.dpb.iter().enumerate() {
            let slot = slot.as_ref().unwrap_or_else(|| panic!("slot {i} filled"));
            assert_eq!(slot.dimensions(), (76, 42));
            // MI-aligned, not cropped: 76 -> 80, 42 -> 48.
            assert_eq!((slot.planes[0].width, slot.planes[0].height), (80, 48));
            assert_eq!((slot.mi_cols, slot.mi_rows), (10, 6));
            assert_eq!(slot.mvs.len(), 60);
        }
        assert_eq!(decoder.state.last_width, 76);
        assert_eq!(decoder.state.last_height, 42);
        assert!(decoder.state.last_show_frame);
        assert!(!decoder.state.last_intra_only);
        assert!(decoder.state.prev_frame.is_some());
        assert!(
            !decoder.state.use_prev_frame_mvs(
                &UncompressedHeader::parse_with_ref_sizes(
                    INTER_FRAME_76X42,
                    &decoder.state.ref_sizes()
                )
                .expect("inter header parses")
            ),
            "a key frame is never a temporal MV source (libvpx vp9_decodeframe.c:2992)"
        );
    }

    // -- intra-only frames --------------------------------------------------

    /// Frame 0 of libvpx's own `vp90-2-16-intra-only.webm` test vector: a
    /// real, hidden (`show_frame == 0`), 352x288 **intra-only** frame with
    /// `reset_frame_context == 2`, `frame_context_idx == 0`,
    /// `refresh_frame_flags == 0x01` and `frame_parallel_decoding_mode == 0`
    /// — so it is also the first fixture in this crate that actually
    /// exercises coefficient counting and backward adaptation (every `kf*`
    /// fixture here codes `frame_parallel_decoding_mode == 1`, which switches
    /// both off). See `dec/testdata/RECIPE-p9io.md` for provenance.
    const INTRA_ONLY_352X288: &[u8] = include_bytes!("dec/testdata/p9io.frame0.bin");
    /// The `show_existing_frame` packet that displays slot 0 — i.e. the
    /// intra-only frame above — taken verbatim from the same test vector.
    const INTRA_ONLY_SHOW_SLOT0: &[u8] = include_bytes!("dec/testdata/p9io.frame1.bin");
    /// libvpx's own reconstruction of that intra-only frame, byte-identical
    /// to ffmpeg's independent native VP9 decoder's.
    const INTRA_ONLY_REF_YUV: &[u8] = include_bytes!("dec/testdata/p9io.ref.yuv");

    /// Asserts a decoded frame matches a planar YUV 4:2:0 golden.
    fn assert_planes_match(frame: &VideoFrame, golden: &[u8], w: usize, h: usize, label: &str) {
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        assert_eq!(golden.len(), w * h + 2 * cw * ch, "{label}: golden size");
        assert_eq!(frame.planes.len(), 3, "{label}: plane count");
        let (y, rest) = golden.split_at(w * h);
        let (u, v) = rest.split_at(cw * ch);
        for (i, (plane, expected)) in frame.planes.iter().zip([y, u, v]).enumerate() {
            assert_eq!(
                plane.data.as_slice(),
                expected,
                "{label}: plane {i} differs from the libvpx reference"
            );
        }
    }

    /// A real intra-only frame must decode to real pixels, bit-exactly.
    ///
    /// It is hidden, so the decode itself emits nothing; the
    /// `show_existing_frame` packet that follows re-displays reference slot
    /// 0 — the slot this frame refreshed — which is what makes the
    /// reconstruction observable at all, and is exactly how the original test
    /// vector displays it.
    #[test]
    fn test_decode_real_intra_only_frame_bit_exact() {
        // The fixture really is what this test claims, parsed rather than
        // assumed.
        let hdr = UncompressedHeader::parse(INTRA_ONLY_352X288).expect("intra-only header parses");
        assert!(!hdr.is_keyframe(), "not a key frame");
        assert!(hdr.intra_only, "intra_only bit set");
        assert!(hdr.is_intra_only(), "and so frame_is_intra_only(cm) holds");
        assert!(!hdr.show_frame, "hidden");
        assert_eq!(hdr.reset_frame_context, 2);
        assert_eq!(hdr.frame_context_idx, 0);
        assert_eq!(hdr.refresh_frame_flags, 0x01, "refreshes slot 0 only");
        assert!(hdr.refresh_frame_context, "saves its adapted context");
        assert!(
            !hdr.frame_parallel_decoding && !hdr.error_resilient,
            "counting and backward adaptation are live for this frame"
        );
        assert_eq!((hdr.width, hdr.height), (352, 288));

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(INTRA_ONLY_352X288, 0)
            .expect("intra-only frame decodes");
        assert!(
            decoder.receive_frame().expect("should succeed").is_none(),
            "a hidden frame produces no output"
        );
        assert_eq!(decoder.dimensions(), Some((352, 288)));
        assert!(
            decoder.state.dpb[0].is_some(),
            "refresh_frame_flags 0x01 fills slot 0"
        );
        assert!(
            decoder.state.dpb[1].is_none(),
            "and only slot 0 — a key frame's 0xFF is not what this frame codes"
        );

        // The instrumentation really ran: this frame coded coefficients, so
        // both counter families must be non-empty, and the saved context must
        // have moved off the defaults the frame started from.
        assert!(
            decoder.state.counts.total_coef_tokens() > 0,
            "decode_coefs must have counted its tokens"
        );
        assert!(
            decoder.state.counts.total_eob_branches() > 0,
            "decode_coefs must have counted its EOB-branch reads"
        );
        // `refresh_frame_context` really did write slot 0. This says the
        // saved context is not the defaults it started from; it does *not*
        // by itself prove backward adaptation ran, because the compressed
        // header's own `diff_update_prob` passes move probabilities too —
        // `dec::state`'s `adaptation_changes_a_real_frames_context_beyond_
        // its_forward_updates` is the test that separates the two, on this
        // same frame.
        assert!(
            !decoder.state.context_is_defaults(0),
            "refresh_frame_context must have saved a non-default context"
        );

        decoder
            .send_packet(INTRA_ONLY_SHOW_SLOT0, 1)
            .expect("show_existing_frame decodes");
        let frame = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("the re-shown intra-only frame");
        assert_eq!((frame.width, frame.height), (352, 288));
        assert_planes_match(&frame, INTRA_ONLY_REF_YUV, 352, 288, "p9io");
    }

    /// Repeating the intra-only frame exercises `setup_past_independence`'s
    /// `reset_frame_context == 2` branch against a genuinely adapted context:
    /// the first copy leaves an adapted context in slot 0, the second must
    /// throw it away and decode from the defaults again — which it can only
    /// be shown to have done by reproducing the same pixels bit for bit.
    #[test]
    fn test_repeated_intra_only_frame_discards_its_own_adapted_context() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");

        decoder
            .send_packet(INTRA_ONLY_352X288, 0)
            .expect("first copy decodes");
        assert!(
            !decoder.state.context_is_defaults(0),
            "there must be an adapted context to discard"
        );

        decoder
            .send_packet(INTRA_ONLY_352X288, 1)
            .expect("second copy decodes");
        decoder
            .send_packet(INTRA_ONLY_SHOW_SLOT0, 2)
            .expect("show_existing_frame decodes");

        let frame = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("the second copy, re-shown");
        assert_planes_match(&frame, INTRA_ONLY_REF_YUV, 352, 288, "p9io repeat");
        assert!(
            decoder.receive_frame().expect("should succeed").is_none(),
            "both intra-only frames are hidden; only the redisplay outputs"
        );
    }

    /// A key frame must discard a probability context an earlier frame
    /// adapted (`vp9_setup_past_independence` resets all four).
    ///
    /// The sequence is a synthetic concatenation of two real fixtures — the
    /// adapting intra-only frame above, then the 76x42 key frame — which is
    /// exactly the situation a key frame exists to handle: it must decode
    /// correctly whatever entropy state preceded it. Without the reset the
    /// key frame would decode against the intra-only frame's adapted
    /// coefficient probabilities and could not come out bit-exact.
    #[test]
    fn test_key_frame_discards_a_previously_adapted_frame_context() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");

        decoder
            .send_packet(INTRA_ONLY_352X288, 0)
            .expect("intra-only frame decodes");
        assert!(
            !decoder.state.context_is_defaults(0),
            "the key frame must have a genuinely adapted context to discard"
        );

        decoder
            .send_packet(KEYFRAME_76X42, 1)
            .expect("key frame decodes");
        let frame = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("key frame output");
        assert_planes_match(&frame, REF_YUV_76X42, 76, 42, "kf76x42 after adaptation");
        for idx in 1..4 {
            assert!(
                decoder.state.context_is_defaults(idx),
                "context {idx} was reset by the key frame and never written since"
            );
        }
    }

    /// The literal two-key-frame sequence: the second key frame resets every
    /// probability context and therefore reproduces the first one exactly.
    ///
    /// What this does *not* prove is stated here rather than implied: both
    /// copies code `frame_parallel_decoding_mode == 1` (asserted below), so
    /// neither counts nor adapts, and the context slot 0 they save is the
    /// unadapted working context. The adapted-context discard is covered by
    /// [`test_key_frame_discards_a_previously_adapted_frame_context`], which
    /// puts a real adapted context in front of this same key frame.
    #[test]
    fn test_two_key_frame_sequence_decodes_both_identically() {
        let hdr = UncompressedHeader::parse(KEYFRAME_76X42).expect("key frame header parses");
        assert!(
            hdr.frame_parallel_decoding,
            "this fixture is frame-parallel, so it neither counts nor adapts"
        );

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(KEYFRAME_76X42, 0)
            .expect("first key frame decodes");
        let first = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("first output");
        assert_planes_match(&first, REF_YUV_76X42, 76, 42, "kf76x42 #1");
        assert!(
            decoder.state.counts.is_zero(),
            "a frame-parallel frame must count nothing (libvpx nulls xd->counts)"
        );

        decoder
            .send_packet(KEYFRAME_76X42, 1)
            .expect("second key frame decodes");
        let second = decoder
            .receive_frame()
            .expect("should succeed")
            .expect("second output");
        assert_planes_match(&second, REF_YUV_76X42, 76, 42, "kf76x42 #2");
    }

    // -- show_existing_frame does not disturb cross-frame state -------------

    /// Spec §8.10 step 2 gates the previous-motion-vector / `PrevRefFrames`
    /// save on `show_existing_frame == 0`, separately from
    /// `refresh_frame_flags`; libvpx returns before `swap_frame_buffers`'
    /// `cm->prev_frame` / `cm->last_show_frame` assignments. So a
    /// show-existing packet must leave *every* cross-frame field exactly as
    /// it found it.
    #[test]
    fn test_show_existing_frame_leaves_all_cross_frame_state_untouched() {
        const SHOW_EXISTING_SLOT0: [u8; 1] = [0x88];

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(KEYFRAME_76X42, 0)
            .expect("keyframe decodes");
        let _ = decoder.receive_frame().expect("should succeed");

        let before = (
            decoder.state.last_frame_type,
            decoder.state.last_width,
            decoder.state.last_height,
            decoder.state.last_show_frame,
            decoder.state.last_intra_only,
            decoder
                .state
                .prev_frame
                .as_ref()
                .map(Vp9RefSlot::dimensions),
            decoder.state.dpb.iter().filter(|s| s.is_some()).count(),
            decoder.state.contexts_initialized,
        );

        decoder
            .send_packet(&SHOW_EXISTING_SLOT0, 1)
            .expect("show_existing_frame decodes");
        let _ = decoder.receive_frame().expect("should succeed");

        let after = (
            decoder.state.last_frame_type,
            decoder.state.last_width,
            decoder.state.last_height,
            decoder.state.last_show_frame,
            decoder.state.last_intra_only,
            decoder
                .state
                .prev_frame
                .as_ref()
                .map(Vp9RefSlot::dimensions),
            decoder.state.dpb.iter().filter(|s| s.is_some()).count(),
            decoder.state.contexts_initialized,
        );
        assert_eq!(
            before, after,
            "a show_existing_frame packet decodes no frame and must not \
             advance last_*, prev_frame, the reference slots or the contexts"
        );
    }

    /// `reset()` must drop *everything* that outlives a frame — the
    /// reference slots, the adapted probability contexts, the `last_*`
    /// fields and the queued output — so the decoder is indistinguishable
    /// from a fresh one and a following inter frame cannot silently predict
    /// from stale pixels or decode against stale probabilities.
    ///
    /// The adapted-context half is what a `dpb`-only assertion would miss:
    /// an inter frame that survived a reset with a stale
    /// `frame_contexts[idx]` would not fail loudly, it would decode to
    /// wrong pixels. The intra-only fixture is used to plant the adapted
    /// context because it is the one committed frame that really adapts
    /// (every `kf*` fixture is frame-parallel and adapts nothing).
    #[test]
    fn test_reset_clears_every_cross_frame_field() {
        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        decoder
            .send_packet(INTRA_ONLY_352X288, 0)
            .expect("intra-only frame decodes");
        decoder
            .send_packet(KEYFRAME_76X42, 1)
            .expect("keyframe decodes");
        // Preconditions: there is genuinely something to clear.
        assert!(decoder.state.dpb.iter().any(Option::is_some));
        assert!(decoder.state.prev_frame.is_some());
        assert_eq!(decoder.state.last_width, 76);
        assert!(decoder.pending_frames() > 0);

        decoder.reset();

        assert!(decoder.state.dpb.iter().all(Option::is_none));
        assert!(decoder.state.prev_frame.is_none());
        assert_eq!(
            (decoder.state.last_width, decoder.state.last_height),
            (0, 0),
            "the last_* geometry must not survive a reset"
        );
        assert!(!decoder.state.last_show_frame);
        assert_eq!(
            decoder.state.contexts_initialized, [false; 4],
            "no saved context may be marked initialized after a reset"
        );
        for idx in 0..4 {
            assert!(
                decoder.state.context_is_defaults(idx),
                "probability context {idx} must be back at the defaults"
            );
        }
        assert_eq!(decoder.pending_frames(), 0);
        assert!(decoder.send_packet(INTER_FRAME_76X42, 0).is_err());
    }

    /// A truncated keyframe header must fail parsing — and must never
    /// produce a fabricated blank frame (regression guard for the old
    /// silent-blank-frame bug).
    #[test]
    fn test_truncated_keyframe_errors_without_blank_frame() {
        // 9 bytes: valid start of a 64x64 keyframe header, truncated before
        // the loop-filter/quant/tile sections.
        const TRUNCATED: [u8; 9] = [0x82, 0x49, 0x83, 0x42, 0x20, 0x03, 0xF0, 0x03, 0xF0];

        let config = DecoderConfig::default();
        let mut decoder = Vp9Decoder::new(config).expect("should succeed");
        assert!(decoder.send_packet(&TRUNCATED, 0).is_err());
        let frame = decoder.receive_frame().expect("should succeed");
        assert!(frame.is_none(), "no fabricated blank frame may be output");
    }
}

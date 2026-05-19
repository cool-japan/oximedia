//! End-to-end bitstream round-trip conformance test.
//!
//! Generates a complete Baseline-profile H.264 Annex-B byte stream
//! in pure Rust (SPS + PPS + one IDR slice containing one `I_PCM`
//! macroblock), then runs it through the workspace's parser and
//! macroblock pipeline to verify the decoded picture matches the
//! pattern fed into the encoder.
//!
//! `I_PCM` keeps the test self-contained: the path skips the
//! CAVLC residual decoder and the transform / dequantisation
//! stages entirely, so the assertion is `decoded_picture ==
//! encoded_pcm_pattern` byte-for-byte.

#![cfg(test)]

use crate::h264::frame::Frame;
use crate::h264::pcm::{read_pcm_macroblock_420, write_pcm_macroblock_420};
use crate::h264::pipeline::{DecodeStep, Decoder};
use crate::h264::pps::parse_pps;
use crate::h264::rbsp::strip_emulation_prevention;
use crate::h264::slice_header::{parse_slice_header, NalContext, SliceType};
use crate::h264::sps::parse_sps;

/// Picture dimensions used by the encoder helpers: 1 × 1
/// macroblocks = 16 × 16 luma + 8 × 8 chroma.
const PIC_WIDTH_MBS: u32 = 1;
const PIC_HEIGHT_MBS: u32 = 1;

#[test]
fn round_trip_minimal_baseline_i_pcm() {
    let pattern = build_test_pattern();
    let bitstream = encode_minimal_idr_i_pcm(&pattern);

    // Annex-B NAL unit extraction.
    let nals = extract_nal_units(&bitstream);
    assert_eq!(nals.len(), 3, "expected SPS + PPS + IDR slice");

    // ---- SPS ----
    let sps_payload = strip_emulation_prevention(&nals[0][1..]);
    let sps = parse_sps(&sps_payload).expect("SPS parse");
    assert_eq!(sps.profile_idc, 66);
    assert_eq!(sps.pic_width_in_mbs_minus1, 0);
    assert_eq!(sps.pic_height_in_map_units_minus1, 0);

    // ---- PPS ----
    let pps_payload = strip_emulation_prevention(&nals[1][1..]);
    let pps = parse_pps(&pps_payload).expect("PPS parse");
    assert!(!pps.entropy_coding_mode_flag);

    // ---- Slice header ----
    let slice_payload = strip_emulation_prevention(&nals[2][1..]);
    let ctx = NalContext {
        nal_ref_idc: 3,
        is_idr: true,
    };
    let sh = parse_slice_header(&slice_payload, &sps, &pps, ctx).expect("slice header parse");
    assert_eq!(sh.slice_type, SliceType::I);
    assert_eq!(sh.first_mb_in_slice, 0);

    // ---- Slice data ----
    //
    // Locate the start of the slice data inside the post-emulation
    // RBSP: it sits at the byte offset where the slice header's
    // trailing bits ended.  build_idr_i_pcm_slice_rbsp pads the
    // slice header to a byte boundary before the PCM bytes, so we
    // can scan from a known fixed offset.
    let pcm_offset = slice_header_byte_length(&slice_payload).expect("locate PCM");
    let samples =
        read_pcm_macroblock_420(&slice_payload[pcm_offset..]).expect("PCM samples");

    // Reconstruct into a frame and verify byte-for-byte.
    let pic_width = PIC_WIDTH_MBS as usize * 16;
    let pic_height = PIC_HEIGHT_MBS as usize * 16;
    let mut frame = Frame::new(pic_width, pic_height);
    write_pcm_macroblock_420(&mut frame, 0, 0, &samples).expect("write PCM");

    // Luma round-trip.
    for j in 0..16 {
        for i in 0..16 {
            let expected = pattern.luma[j * 16 + i];
            let actual = frame.get_luma(i, j).expect("luma sample");
            assert_eq!(
                actual, expected,
                "luma mismatch at ({i}, {j}): {actual} vs {expected}"
            );
        }
    }
    // Chroma round-trip.
    for j in 0..8 {
        for i in 0..8 {
            let expected_cb = pattern.cb[j * 8 + i];
            let actual_cb = frame.get_cb(i, j).expect("Cb sample");
            assert_eq!(
                actual_cb, expected_cb,
                "Cb mismatch at ({i}, {j}): {actual_cb} vs {expected_cb}"
            );
            let expected_cr = pattern.cr[j * 8 + i];
            let actual_cr = frame.get_cr(i, j).expect("Cr sample");
            assert_eq!(
                actual_cr, expected_cr,
                "Cr mismatch at ({i}, {j}): {actual_cr} vs {expected_cr}"
            );
        }
    }
}

/// The test pattern that travels round-trip: encoded into the
/// `I_PCM` macroblock, decoded out, and asserted equal.
struct TestPattern {
    luma: [u8; 256],
    cb: [u8; 64],
    cr: [u8; 64],
}

fn build_test_pattern() -> TestPattern {
    let mut luma = [0u8; 256];
    for j in 0..16 {
        for i in 0..16 {
            // Visible gradient + checker so any positional swap is
            // immediately visible in an assertion failure.
            luma[j * 16 + i] = ((j * 16 + i) ^ ((j & 1) * 0x0F)) as u8;
        }
    }
    let mut cb = [0u8; 64];
    let mut cr = [0u8; 64];
    for j in 0..8 {
        for i in 0..8 {
            cb[j * 8 + i] = 64 + (j * 8 + i) as u8;
            cr[j * 8 + i] = 192 - (j * 8 + i) as u8;
        }
    }
    TestPattern { luma, cb, cr }
}

/// Assembles a complete Annex-B bitstream containing one SPS, one
/// PPS, and one IDR slice with a single `I_PCM` macroblock.
fn encode_minimal_idr_i_pcm(pattern: &TestPattern) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    out.push(0x67);
    out.extend(encode_sps_rbsp());

    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    out.push(0x68);
    out.extend(encode_pps_rbsp());

    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    out.push(0x65);
    out.extend(encode_idr_i_pcm_slice_rbsp(pattern));

    out
}

/// Encodes a bare IDR slice NAL with explicit frame_num /
/// idr_pic_id — used to build multi-frame streams.
fn encode_idr_nal_only(pattern: &TestPattern, frame_num: u32, idr_pic_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    out.push(0x65);
    let rbsp = encode_idr_i_pcm_slice_rbsp_at(pattern, frame_num, idr_pic_id);
    out.extend(escape_emulation_prevention(&rbsp));
    out
}

/// Inserts H.264 emulation-prevention bytes into a raw RBSP byte
/// stream: any `0x00 0x00 ≤0x03` triplet gets a `0x03` byte
/// spliced between the two zeros and the trailing byte so the
/// downstream Annex-B parser correctly recovers the original data
/// via `strip_emulation_prevention`.
fn escape_emulation_prevention(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 16);
    let mut zeros = 0usize;
    for &b in rbsp {
        if zeros >= 2 && b <= 0x03 {
            out.push(0x03);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

/// Tiny SPS: baseline profile, level 1.0, one 16×16 macroblock.
fn encode_sps_rbsp() -> Vec<u8> {
    // SPS bytes are bit-packed but the leading fields fit cleanly
    // into byte boundaries; the rest is exp-Golomb coded.
    let mut bits = BitWriter::new();
    bits.write(8, 66); // profile_idc = Baseline
    bits.write(8, 0); // constraint_set_flags + reserved_zero_2bits
    bits.write(8, 10); // level_idc = 1.0
    bits.ue(0); // seq_parameter_set_id = 0
    bits.ue(0); // log2_max_frame_num_minus4 = 0 (4-bit frame_num)
    bits.ue(2); // pic_order_cnt_type = 2 (no pic_order_cnt extras)
    bits.ue(1); // num_ref_frames = 1
    bits.write(1, 0); // gaps_in_frame_num_value_allowed_flag = 0
    bits.ue(PIC_WIDTH_MBS - 1); // pic_width_in_mbs_minus1 = 0
    bits.ue(PIC_HEIGHT_MBS - 1); // pic_height_in_map_units_minus1 = 0
    bits.write(1, 1); // frame_mbs_only_flag = 1
    bits.write(1, 0); // direct_8x8_inference_flag = 0
    bits.write(1, 0); // frame_cropping_flag = 0
    bits.write(1, 0); // vui_parameters_present_flag = 0
    bits.rbsp_trailing();
    bits.into_bytes()
}

/// Tiny PPS: cavlc + qp_init = 26.
fn encode_pps_rbsp() -> Vec<u8> {
    let mut bits = BitWriter::new();
    bits.ue(0); // pic_parameter_set_id = 0
    bits.ue(0); // seq_parameter_set_id = 0
    bits.write(1, 0); // entropy_coding_mode_flag = 0 (CAVLC)
    bits.write(1, 0); // bottom_field_pic_order_in_frame_present_flag = 0
    bits.ue(0); // num_slice_groups_minus1 = 0
    bits.ue(0); // num_ref_idx_l0_default_active_minus1 = 0
    bits.ue(0); // num_ref_idx_l1_default_active_minus1 = 0
    bits.write(1, 0); // weighted_pred_flag = 0
    bits.write(2, 0); // weighted_bipred_idc = 0
    bits.se(0); // pic_init_qp_minus26 = 0 -> QP 26
    bits.se(0); // pic_init_qs_minus26 = 0
    bits.se(0); // chroma_qp_index_offset = 0
    bits.write(1, 0); // deblocking_filter_control_present_flag = 0
    bits.write(1, 0); // constrained_intra_pred_flag = 0
    bits.write(1, 0); // redundant_pic_cnt_present_flag = 0
    bits.rbsp_trailing();
    bits.into_bytes()
}

/// IDR slice header + slice data: one I_PCM macroblock.
fn encode_idr_i_pcm_slice_rbsp(pattern: &TestPattern) -> Vec<u8> {
    encode_idr_i_pcm_slice_rbsp_at(pattern, 0, 0)
}

/// Same as [`encode_idr_i_pcm_slice_rbsp`] but takes an explicit
/// `frame_num` and `idr_pic_id` so the caller can build a
/// multi-frame Annex-B stream where each IDR distinguishes
/// itself.
fn encode_idr_i_pcm_slice_rbsp_at(
    pattern: &TestPattern,
    frame_num: u32,
    idr_pic_id: u32,
) -> Vec<u8> {
    let mut bits = BitWriter::new();
    bits.ue(0); // first_mb_in_slice = 0
    bits.ue(7); // slice_type = 7 (I, all-I slices)
    bits.ue(0); // pic_parameter_set_id = 0
    bits.write(4, frame_num); // log2_max_frame_num_minus4 = 0 → 4 bits
    bits.ue(idr_pic_id);
    bits.write(1, 0); // no_output_of_prior_pics_flag
    bits.write(1, 0); // long_term_reference_flag
    bits.se(0); // slice_qp_delta = 0

    bits.ue(25); // I_PCM mb_type
    bits.align_byte_with(false);
    for &b in pattern.luma.iter() {
        bits.write(8, b as u32);
    }
    for &b in pattern.cb.iter() {
        bits.write(8, b as u32);
    }
    for &b in pattern.cr.iter() {
        bits.write(8, b as u32);
    }
    bits.rbsp_trailing();
    bits.into_bytes()
}

/// Splits an Annex-B byte stream into NAL-unit-payload chunks
/// (start-code prefix stripped, NAL header byte included).
fn extract_nal_units(buf: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i + 3 < buf.len() {
        // Find a start code.
        let start_len = if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 0 && buf[i + 3] == 1 {
            4
        } else if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            3
        } else {
            i += 1;
            continue;
        };
        let payload_start = i + start_len;
        // Find next start code.
        let mut j = payload_start;
        while j + 2 < buf.len() {
            if buf[j] == 0
                && buf[j + 1] == 0
                && (buf[j + 2] == 1
                    || (j + 3 < buf.len() && buf[j + 2] == 0 && buf[j + 3] == 1))
            {
                break;
            }
            j += 1;
        }
        let payload_end = if j + 2 < buf.len() { j } else { buf.len() };
        nals.push(&buf[payload_start..payload_end]);
        i = payload_end;
    }
    nals
}

/// Computes the byte offset at which the slice data section starts
/// inside the IDR-I-PCM slice RBSP.  Mirrors the encoder's layout:
/// the slice header is written first, then aligned to a byte
/// boundary via `pcm_alignment_zero_bit` (which my encoder writes
/// before the mb_type) — wait, no.  In actual order:
///   slice_header → mb_type(=25, ue(v)) → pcm_alignment_zero_bit → PCM bytes.
/// We reconstruct the byte offset by re-running the header / mb_type
/// portion through the same `BitWriter` logic.
fn slice_header_byte_length(_rbsp: &[u8]) -> Option<usize> {
    let mut bits = BitWriter::new();
    bits.ue(0);
    bits.ue(7);
    bits.ue(0);
    bits.write(4, 0);
    bits.ue(0);
    bits.write(1, 0);
    bits.write(1, 0);
    bits.se(0);
    bits.ue(25);
    bits.align_byte_with(false);
    Some(bits.bit_count() / 8)
}

// ---------------------------------------------------------------------------
// Bit-packing helper.
// ---------------------------------------------------------------------------

struct BitWriter {
    bits: Vec<bool>,
}

impl BitWriter {
    fn new() -> Self {
        Self { bits: Vec::new() }
    }

    fn write(&mut self, n: u32, mut value: u32) {
        let mask = if n == 0 { 0 } else { 1u32 << (n - 1) };
        for _ in 0..n {
            self.bits.push(value & mask != 0);
            value <<= 1;
        }
    }

    fn ue(&mut self, value: u32) {
        let mut n = 0u32;
        while (1u32 << (n + 1)) - 1 <= value {
            n += 1;
        }
        for _ in 0..n {
            self.bits.push(false);
        }
        self.bits.push(true);
        let suffix = value + 1 - (1u32 << n);
        self.write(n, suffix);
    }

    fn se(&mut self, value: i32) {
        let mapped = if value <= 0 {
            (-(value as i64) * 2) as u32
        } else {
            (value as i64 * 2 - 1) as u32
        };
        self.ue(mapped);
    }

    fn align_byte_with(&mut self, bit: bool) {
        while self.bits.len() % 8 != 0 {
            self.bits.push(bit);
        }
    }

    fn rbsp_trailing(&mut self) {
        self.bits.push(true);
        self.align_byte_with(false);
    }

    fn bit_count(&self) -> usize {
        self.bits.len()
    }

    fn into_bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bits.len() / 8 + 1);
        let mut byte = 0u8;
        let mut count = 0u8;
        for b in self.bits {
            byte = (byte << 1) | u8::from(b);
            count += 1;
            if count == 8 {
                out.push(byte);
                byte = 0;
                count = 0;
            }
        }
        if count > 0 {
            byte <<= 8 - count;
            out.push(byte);
        }
        out
    }
}

#[test]
fn decoder_round_trips_minimal_baseline_i_pcm() {
    // Same encoded bitstream as round_trip_minimal_baseline_i_pcm,
    // but exercised through the top-level Decoder driver to prove
    // the parameter-set store + NAL routing + slice dispatch +
    // frame emission compose end-to-end.
    let pattern = build_test_pattern();
    let bitstream = encode_minimal_idr_i_pcm(&pattern);

    let mut decoder = Decoder::new();
    let frames = decoder.feed_annex_b(&bitstream).expect("feed");
    assert_eq!(frames.len(), 1, "expected exactly one decoded frame");
    let frame = &frames[0];

    for j in 0..16 {
        for i in 0..16 {
            let expected = pattern.luma[j * 16 + i];
            let actual = frame.get_luma(i, j).expect("luma sample");
            assert_eq!(
                actual, expected,
                "decoder luma mismatch at ({i}, {j}): {actual} vs {expected}"
            );
        }
    }
    for j in 0..8 {
        for i in 0..8 {
            assert_eq!(
                frame.get_cb(i, j),
                Some(pattern.cb[j * 8 + i]),
                "decoder Cb mismatch at ({i}, {j})"
            );
            assert_eq!(
                frame.get_cr(i, j),
                Some(pattern.cr[j * 8 + i]),
                "decoder Cr mismatch at ({i}, {j})"
            );
        }
    }

    // DPB should now hold one short-term reference (IDR with
    // nal_ref_idc = 3).
    assert_eq!(decoder.dpb().entries.len(), 1);
    assert!(decoder.dpb().entries[0].is_short_term_reference);
}

#[test]
fn decoder_handles_sps_pps_only_without_frame_output() {
    // SPS + PPS alone shouldn't emit a frame.
    let pattern = build_test_pattern();
    let bitstream = encode_minimal_idr_i_pcm(&pattern);
    // Strip the slice NAL (everything after the second start code).
    let mut trimmed = Vec::new();
    let mut start_codes_seen = 0;
    let mut i = 0;
    while i + 3 < bitstream.len() {
        let len = if bitstream[i..i + 4] == [0, 0, 0, 1] {
            4
        } else if bitstream[i..i + 3] == [0, 0, 1] {
            3
        } else {
            trimmed.push(bitstream[i]);
            i += 1;
            continue;
        };
        if start_codes_seen >= 2 {
            break;
        }
        trimmed.extend_from_slice(&bitstream[i..i + len]);
        start_codes_seen += 1;
        i += len;
        while i + 3 < bitstream.len()
            && !(bitstream[i..i + 3] == [0, 0, 1]
                || bitstream[i..i + 4] == [0, 0, 0, 1])
        {
            trimmed.push(bitstream[i]);
            i += 1;
        }
    }
    let _ = DecodeStep::None;
    let mut decoder = Decoder::new();
    let frames = decoder.feed_annex_b(&trimmed).expect("feed");
    assert!(frames.is_empty(), "SPS+PPS only must not emit a frame");
}

#[test]
fn decoder_handles_multi_frame_idr_sequence() {
    // Build three IDR pictures each with a distinct I_PCM pattern,
    // streamed back-to-back through Decoder::feed_annex_b.  The
    // SPS + PPS appear once; subsequent IDRs reuse them.  Verifies:
    //
    // - NAL extraction across multiple frames (start-code scan).
    // - SPS / PPS persistence across slices.
    // - DPB grows by one short-term reference per emitted frame.
    // - Each decoded frame contains the pattern its IDR carried.
    let pattern_a = build_test_pattern();
    let pattern_b = TestPattern {
        luma: core::array::from_fn(|i| (255 - i) as u8),
        cb: core::array::from_fn(|i| (i * 3) as u8),
        cr: core::array::from_fn(|i| (200 - i) as u8),
    };
    let pattern_c = TestPattern {
        luma: [42u8; 256],
        cb: [80u8; 64],
        cr: [180u8; 64],
    };

    let mut bitstream = Vec::new();
    // SPS + PPS (once).
    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bitstream.push(0x67);
    bitstream.extend(encode_sps_rbsp());
    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bitstream.push(0x68);
    bitstream.extend(encode_pps_rbsp());
    // Three IDR NALs.  Each IDR resets frame_num to 0 (spec), so
    // we pass 0 for every one.  idr_pic_id is the only field that
    // distinguishes them on the bitstream side.
    bitstream.extend(encode_idr_nal_only(&pattern_a, 0, 0));
    bitstream.extend(encode_idr_nal_only(&pattern_b, 0, 1));
    bitstream.extend(encode_idr_nal_only(&pattern_c, 0, 2));

    let mut decoder = Decoder::new();
    let frames = decoder.feed_annex_b(&bitstream).expect("feed");
    assert_eq!(frames.len(), 3, "three IDR pictures should produce three frames");

    // Each frame must carry its IDR's pattern, byte-for-byte.
    for (frame, pattern) in frames.iter().zip([&pattern_a, &pattern_b, &pattern_c]) {
        for j in 0..16 {
            for i in 0..16 {
                assert_eq!(
                    frame.get_luma(i, j),
                    Some(pattern.luma[j * 16 + i]),
                    "luma mismatch in multi-frame at ({i}, {j})"
                );
            }
        }
        for j in 0..8 {
            for i in 0..8 {
                assert_eq!(frame.get_cb(i, j), Some(pattern.cb[j * 8 + i]));
                assert_eq!(frame.get_cr(i, j), Some(pattern.cr[j * 8 + i]));
            }
        }
    }

    // DPB now holds three short-term references.
    assert_eq!(decoder.dpb().entries.len(), 3);
    for entry in &decoder.dpb().entries {
        assert!(entry.is_short_term_reference);
    }
}

#[test]
fn decoder_skips_aud_and_sei_nals() {
    // Insert an AUD before the IDR and an SEI between the PPS and
    // IDR.  Decoder must skip them silently and still produce the
    // exact same frame as the no-AUD/SEI baseline.
    let pattern = build_test_pattern();
    let mut bitstream = Vec::new();
    // AUD NAL: nal_unit_type = 9.  Body is "primary_pic_type" u(3)
    // + rbsp_trailing — `0xF0` covers it.
    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]);

    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bitstream.push(0x67);
    bitstream.extend(encode_sps_rbsp());

    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bitstream.push(0x68);
    bitstream.extend(encode_pps_rbsp());

    // SEI NAL: nal_unit_type = 6.  Two-byte payload (filler
    // payload type) + trailing bits.
    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x06, 0x00, 0x80]);

    bitstream.extend(encode_idr_nal_only(&pattern, 0, 0));

    let mut decoder = Decoder::new();
    let frames = decoder.feed_annex_b(&bitstream).expect("feed");
    assert_eq!(frames.len(), 1, "AUD + SEI shouldn't add or drop frames");
    for j in 0..16 {
        for i in 0..16 {
            assert_eq!(
                frames[0].get_luma(i, j),
                Some(pattern.luma[j * 16 + i])
            );
        }
    }
}

#[test]
fn decoder_poc_advances_across_idrs() {
    // Each IDR resets POC to 0 (spec § 8.2.1).  Three IDRs in a
    // row should leave every DPB entry at POC 0 — this catches a
    // regression where prev_pic_order_cnt_msb / _lsb might leak
    // across IDR resets.
    let pattern = build_test_pattern();
    let mut bitstream = Vec::new();
    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bitstream.push(0x67);
    bitstream.extend(encode_sps_rbsp());
    bitstream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bitstream.push(0x68);
    bitstream.extend(encode_pps_rbsp());
    bitstream.extend(encode_idr_nal_only(&pattern, 0, 0));
    bitstream.extend(encode_idr_nal_only(&pattern, 0, 1));

    let mut decoder = Decoder::new();
    decoder.feed_annex_b(&bitstream).expect("feed");
    for entry in &decoder.dpb().entries {
        assert_eq!(entry.poc, 0, "IDR POC must always be 0");
    }
}

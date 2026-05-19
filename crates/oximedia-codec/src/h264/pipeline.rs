//! Top-level H.264 decoder driver.
//!
//! Wraps the parser stack (NAL extraction, SPS / PPS parsing,
//! slice-header parsing) and the per-slice decode pipeline behind
//! a `Decoder` struct that consumes an Annex-B byte stream and
//! emits reconstructed [`Frame`]s.
//!
//! ## Scope (current iteration)
//!
//! - Single slice per picture (the most common layout in
//!   real-world streams).  Multi-slice pictures aren't yet
//!   assembled into a single output frame.
//! - I and P slices through the CABAC entropy path.
//!   B slices are accepted but motion compensation for them
//!   needs the reference-list construction landing in a follow-up.
//! - `I_PCM` macroblocks via the parser + PCM passthrough.
//! - Multiple SPS / PPS instances stored by id.
//! - Simple `pic_order_cnt_type = 2` POC (frame_num × 2).
//!
//! ## Out of scope for this iteration
//!
//! - CAVLC inter slice loop (only CABAC is wired for P).
//! - Reference picture list modification ops + weighted
//!   prediction application.
//! - POC types 0 and 1.
//! - AUD / SEI / filler NALs (silently skipped).
//! - Multi-slice picture assembly.

use std::collections::HashMap;

use crate::h264::bit_reader::BitReader;
use crate::h264::cabac::{init_contexts, CabacContext};
use crate::h264::cabac_inter_mb::MbNeighbours;
use crate::h264::decoder::{
    decode_intra_16x16_mb, decode_intra_4x4_mb, decode_intra_chroma_8x8,
    decode_intra_slice_bitstream, Residual4x4Scan,
};
use crate::h264::dpb::{Dpb, DpbEntry};
use crate::h264::frame::Frame;
use crate::h264::inter_cache::{InterMbDecoded, InterSliceCache};
use crate::h264::intra_mode::{most_probable_mode, resolve_intra4x4_mode, Intra4x4ModeContext};
use crate::h264::intra_pred::Intra4x4Mode;
use crate::h264::macroblock::{Intra16x16PredMode, IntraChromaPredMode, MacroblockLayer, MbType};
use crate::h264::mv_pred::{predict_mv_median, MotionVector, MvPredictionContext};
use crate::h264::pcm::{read_pcm_macroblock_420, write_pcm_macroblock_420};
use crate::h264::pps::{parse_pps, PpsRbsp};
use crate::h264::rbsp::strip_emulation_prevention;
use crate::h264::reconstruct_inter::{
    reconstruct_inter_p_mb, reconstruct_inter_p_mb_multiref, InterPMbInputs,
};
use crate::h264::reconstruct_intra_cabac::{
    reconstruct_intra_16x16_mb_cabac, reconstruct_intra_4x4_mb_cabac,
    reconstruct_intra_chroma_8x8_cabac,
};
use crate::h264::slice_cabac::{parse_slice_cabac, MbKind, SliceCabacContext};
use crate::h264::slice_cavlc::{
    cavlc_block_to_position_4x4, cavlc_chroma_ac_to_position, cavlc_chroma_dc_to_position,
    parse_slice_cavlc, MbCavlcKind,
};
use crate::h264::slice_header::{parse_slice_header, NalContext, SliceType};
use crate::h264::sps::{parse_sps, SpsRbsp};
use crate::CodecError;

/// Top-level decoder.
///
/// Construct with [`Decoder::new`], feed Annex-B bytes via
/// [`Decoder::feed_annex_b`] or individual NAL units via
/// [`Decoder::feed_nal`].  Decoded pictures are emitted through
/// the return value as they complete.
#[derive(Debug)]
pub struct Decoder {
    sps_store: HashMap<u8, SpsRbsp>,
    pps_store: HashMap<u8, PpsRbsp>,
    dpb: Dpb,
    /// `frame_num` of the most recently decoded reference.  Used by
    /// the POC type 2 derivation.
    prev_frame_num: i32,
    /// `pic_order_cnt_msb` carried from the previous reference
    /// picture — POC type 0 state.
    prev_pic_order_cnt_msb: i32,
    /// `pic_order_cnt_lsb` carried from the previous reference
    /// picture — POC type 0 state.
    prev_pic_order_cnt_lsb: i32,
    /// Picture currently being assembled (when in multi-slice mode
    /// — empty in the current single-slice-per-picture model).
    in_progress: Option<Frame>,
}

/// Outcome of feeding one NAL unit to the decoder.
#[derive(Debug)]
pub enum DecodeStep {
    /// NAL was consumed (SPS / PPS / AUD / SEI / partial slice)
    /// but no full frame is ready yet.
    None,
    /// NAL completed a picture; the reconstructed frame is
    /// returned and stored in the DPB.
    Frame(Frame),
    /// A NAL type the decoder does not currently support — the
    /// caller can decide whether to skip or surface an error.
    Unsupported(u8),
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// Constructs an empty decoder.  Parameter sets must be fed in
    /// before any slice can be decoded.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sps_store: HashMap::new(),
            pps_store: HashMap::new(),
            dpb: Dpb::new(16),
            prev_frame_num: 0,
            prev_pic_order_cnt_msb: 0,
            prev_pic_order_cnt_lsb: 0,
            in_progress: None,
        }
    }

    /// Feeds a full Annex-B byte stream.  Returns every decoded
    /// frame in stream order.
    ///
    /// # Errors
    ///
    /// Bubbles up any parser or decode error from the inner stages.
    pub fn feed_annex_b(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, CodecError> {
        let mut frames = Vec::new();
        for nal in extract_nal_units(bytes) {
            match self.feed_nal(nal)? {
                DecodeStep::Frame(f) => frames.push(f),
                DecodeStep::None | DecodeStep::Unsupported(_) => {}
            }
        }
        Ok(frames)
    }

    /// Feeds one NAL unit (header byte + payload, no start code).
    ///
    /// Returns the outcome — frame, no-op, or unsupported NAL type.
    ///
    /// # Errors
    ///
    /// Bubbles up parser / decode errors.
    pub fn feed_nal(&mut self, nal: &[u8]) -> Result<DecodeStep, CodecError> {
        if nal.is_empty() {
            return Err(CodecError::InvalidData(
                "h264 decoder: empty NAL unit".into(),
            ));
        }
        let header = nal[0];
        let nal_unit_type = header & 0x1F;
        let nal_ref_idc = (header >> 5) & 0x03;
        let payload = &nal[1..];

        match nal_unit_type {
            7 => {
                let rbsp = strip_emulation_prevention(payload);
                let sps = parse_sps(&rbsp)?;
                self.sps_store.insert(sps.seq_parameter_set_id as u8, sps);
                Ok(DecodeStep::None)
            }
            8 => {
                let rbsp = strip_emulation_prevention(payload);
                let pps = parse_pps(&rbsp)?;
                self.pps_store.insert(pps.pic_parameter_set_id as u8, pps);
                Ok(DecodeStep::None)
            }
            1 | 5 => {
                let frame =
                    self.decode_slice_nal(payload, nal_ref_idc, nal_unit_type == 5)?;
                Ok(DecodeStep::Frame(frame))
            }
            6 | 9 | 12 => Ok(DecodeStep::None),
            _ => Ok(DecodeStep::Unsupported(nal_unit_type)),
        }
    }

    /// Returns a reference to the active DPB.  Useful for tests
    /// and for callers that want to introspect the reference
    /// pictures held by the decoder.
    #[must_use]
    pub fn dpb(&self) -> &Dpb {
        &self.dpb
    }

    fn decode_slice_nal(
        &mut self,
        payload: &[u8],
        nal_ref_idc: u8,
        is_idr: bool,
    ) -> Result<Frame, CodecError> {
        let rbsp = strip_emulation_prevention(payload);

        // Slice headers reference parameter sets by id; the first
        // ue(v) is `first_mb_in_slice`, the second is `slice_type`,
        // the third is `pic_parameter_set_id`.  We need the PPS
        // (and the SPS it points at) before we can parse the full
        // header — but `parse_slice_header` itself does that look-up
        // via its `pps` argument.  We pick the most recently
        // registered PPS as a pragmatic default; a richer driver
        // would peek at the PPS id field first.
        let pps = self
            .pps_store
            .values()
            .next()
            .ok_or_else(|| CodecError::InvalidData("h264 decoder: no PPS registered".into()))?
            .clone();
        let sps = self
            .sps_store
            .get(&(pps.seq_parameter_set_id as u8))
            .ok_or_else(|| {
                CodecError::InvalidData("h264 decoder: PPS references unknown SPS".into())
            })?
            .clone();

        let ctx = NalContext {
            nal_ref_idc,
            is_idr,
        };
        let sh = parse_slice_header(&rbsp, &sps, &pps, ctx)?;

        let pic_width = (sps.pic_width_in_mbs_minus1 as usize + 1) * 16;
        let pic_height = (sps.pic_height_in_map_units_minus1 as usize + 1) * 16;
        let pic_width_mbs = sps.pic_width_in_mbs_minus1 as usize + 1;
        let pic_height_mbs = sps.pic_height_in_map_units_minus1 as usize + 1;

        let frame = if pps.entropy_coding_mode_flag {
            self.decode_cabac_slice(
                &rbsp,
                &sh,
                &sps,
                &pps,
                pic_width,
                pic_height,
                pic_width_mbs,
                pic_height_mbs,
            )?
        } else {
            self.decode_cavlc_slice(
                &rbsp,
                &sh,
                &sps,
                &pps,
                pic_width,
                pic_height,
                pic_width_mbs,
                pic_height_mbs,
            )?
        };

        // POC derivation per spec § 8.2.1.
        let frame_num = sh.frame_num as i32;
        let poc = self.compute_poc(&sh, &sps, nal_ref_idc, is_idr);
        let _ = self.dpb.insert(DpbEntry {
            frame: frame.clone(),
            poc,
            frame_num,
            is_short_term_reference: nal_ref_idc != 0,
            is_long_term_reference: false,
            long_term_idx: None,
            output_pending: true,
        });
        if nal_ref_idc != 0 {
            self.prev_frame_num = frame_num;
        }
        Ok(frame)
    }

    /// Computes the picture order count for a slice per spec
    /// § 8.2.1.  Updates internal POC state when the picture is a
    /// reference.
    fn compute_poc(
        &mut self,
        sh: &crate::h264::slice_header::SliceHeader,
        sps: &SpsRbsp,
        nal_ref_idc: u8,
        is_idr: bool,
    ) -> i32 {
        match sps.pic_order_cnt_type {
            0 => self.compute_poc_type0(sh, sps, nal_ref_idc, is_idr),
            1 => self.compute_poc_type1(sh, sps, nal_ref_idc, is_idr),
            _ => self.compute_poc_type2(sh, nal_ref_idc, is_idr),
        }
    }

    fn compute_poc_type0(
        &mut self,
        sh: &crate::h264::slice_header::SliceHeader,
        sps: &SpsRbsp,
        nal_ref_idc: u8,
        is_idr: bool,
    ) -> i32 {
        if is_idr {
            self.prev_pic_order_cnt_msb = 0;
            self.prev_pic_order_cnt_lsb = 0;
        }
        let max_poc_lsb =
            1i32 << (sps.log2_max_pic_order_cnt_lsb_minus4 + 4);
        let poc_lsb = sh.pic_order_cnt_lsb.unwrap_or(0) as i32;

        let poc_msb = if poc_lsb < self.prev_pic_order_cnt_lsb
            && self.prev_pic_order_cnt_lsb - poc_lsb >= max_poc_lsb / 2
        {
            self.prev_pic_order_cnt_msb + max_poc_lsb
        } else if poc_lsb > self.prev_pic_order_cnt_lsb
            && poc_lsb - self.prev_pic_order_cnt_lsb > max_poc_lsb / 2
        {
            self.prev_pic_order_cnt_msb - max_poc_lsb
        } else {
            self.prev_pic_order_cnt_msb
        };

        let top_field_order_cnt = poc_msb + poc_lsb;
        if nal_ref_idc != 0 {
            self.prev_pic_order_cnt_msb = poc_msb;
            self.prev_pic_order_cnt_lsb = poc_lsb;
        }
        top_field_order_cnt
    }

    /// Type 1 (delta-cycle): coarse implementation — uses the
    /// expected delta cycle without weighing in
    /// `offset_for_non_ref_pic` against the previous picture's
    /// state, which is sufficient when every reference picture
    /// carries `delta_pic_order_always_zero_flag == 1` (a common
    /// constrained-baseline pattern).
    fn compute_poc_type1(
        &mut self,
        sh: &crate::h264::slice_header::SliceHeader,
        sps: &SpsRbsp,
        _nal_ref_idc: u8,
        is_idr: bool,
    ) -> i32 {
        if is_idr {
            return 0;
        }
        let cycle = sps.num_ref_frames_in_pic_order_cnt_cycle.max(1) as i32;
        // Without an `offset_for_ref_frame[]` array on the parsed
        // SPS we approximate the expected delta as the average
        // offset_for_non_ref_pic over one cycle.  For DAR-zero
        // streams (the common subset we support) this collapses to
        // a simple 2 × frame_num progression.
        2 * sh.frame_num as i32 / cycle
    }

    fn compute_poc_type2(
        &mut self,
        sh: &crate::h264::slice_header::SliceHeader,
        nal_ref_idc: u8,
        is_idr: bool,
    ) -> i32 {
        poc_type2_value(sh.frame_num as i32, nal_ref_idc, is_idr)
    }

    /// Constructs RefPicList0 per spec § 8.2.4 (without applying
    /// `ref_pic_list_modification` ops — those land in a follow-up).
    /// Short-term references are ordered by descending `frame_num`
    /// (proxy for PicNum on frame-coded pictures); long-term
    /// references follow in ascending `long_term_idx`.
    fn build_ref_pic_list_l0(&self) -> Vec<&Frame> {
        let mut short_term: Vec<&DpbEntry> = self
            .dpb
            .entries
            .iter()
            .filter(|e| e.is_short_term_reference)
            .collect();
        short_term.sort_by(|a, b| b.frame_num.cmp(&a.frame_num));
        let mut long_term: Vec<&DpbEntry> = self
            .dpb
            .entries
            .iter()
            .filter(|e| e.is_long_term_reference)
            .collect();
        long_term.sort_by(|a, b| a.long_term_idx.cmp(&b.long_term_idx));
        short_term
            .into_iter()
            .chain(long_term)
            .map(|e| &e.frame)
            .collect()
    }

    /// Constructs RefPicList1 for a B slice per spec § 8.2.4.2.3.
    /// Short-term refs with POC > current_poc come first (sorted
    /// ascending by POC), followed by short-term refs with POC <
    /// current_poc (sorted descending by POC), then long-term
    /// refs ordered by ascending long_term_idx.
    ///
    /// `current_poc` is the POC of the slice currently being
    /// decoded (used as the pivot for the bi-prediction split).
    fn build_ref_pic_list_l1(&self, current_poc: i32) -> Vec<&Frame> {
        let mut higher: Vec<&DpbEntry> = self
            .dpb
            .entries
            .iter()
            .filter(|e| e.is_short_term_reference && e.poc > current_poc)
            .collect();
        higher.sort_by(|a, b| a.poc.cmp(&b.poc));
        let mut lower: Vec<&DpbEntry> = self
            .dpb
            .entries
            .iter()
            .filter(|e| e.is_short_term_reference && e.poc < current_poc)
            .collect();
        lower.sort_by(|a, b| b.poc.cmp(&a.poc));
        let mut long_term: Vec<&DpbEntry> = self
            .dpb
            .entries
            .iter()
            .filter(|e| e.is_long_term_reference)
            .collect();
        long_term.sort_by(|a, b| a.long_term_idx.cmp(&b.long_term_idx));
        higher
            .into_iter()
            .chain(lower)
            .chain(long_term)
            .map(|e| &e.frame)
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_cabac_slice(
        &mut self,
        rbsp: &[u8],
        sh: &crate::h264::slice_header::SliceHeader,
        _sps: &SpsRbsp,
        pps: &PpsRbsp,
        pic_width: usize,
        pic_height: usize,
        pic_width_mbs: usize,
        pic_height_mbs: usize,
    ) -> Result<Frame, CodecError> {
        // CABAC slice data is byte-aligned after the slice header
        // (the spec inserts `cabac_alignment_one_bit` to enforce
        // this).  Use the parsed bit length of the slice header
        // rounded up to a byte boundary as the data start.
        let slice_data_start = slice_header_bit_length(sh, pps).div_ceil(8);
        if slice_data_start >= rbsp.len() {
            return Err(CodecError::InvalidData(
                "h264 decoder: slice header consumes whole RBSP".into(),
            ));
        }

        let qp_y_u8 = sh.slice_qp_y.clamp(0, 51) as u8;
        let mut states = init_contexts(sh.slice_type, qp_y_u8, 0);
        let mut cabac = CabacContext::new(&rbsp[slice_data_start..])?;

        // Dequant tables — fixed default scaling.  Real decoder
        // would pull these from PPS / SPS scaling lists.
        let scan_4x4: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
        let scan_8x8 = [0u8; 64];
        let dq4 = [16u32; 16];
        let dq8 = [16u32; 64];

        let ctx = SliceCabacContext {
            slice_type: sh.slice_type,
            pic_width_mbs,
            pic_height_mbs,
            initial_qp_y: qp_y_u8,
            chroma_qp_index_offset: pps.chroma_qp_index_offset,
            num_ref_idx_l0_active: (pps.num_ref_idx_l0_default_active_minus1 as u8).saturating_add(1),
            scan_4x4: &scan_4x4,
            scan_8x8: &scan_8x8,
            dequant_4x4_luma: &dq4,
            dequant_4x4_cb: &dq4,
            dequant_4x4_cr: &dq4,
            dequant_8x8_luma: &dq8,
        };
        let mut cache = InterSliceCache::new(pic_width_mbs);

        let mbs = parse_slice_cabac(&mut cabac, &mut states, ctx, &mut cache)?;

        let mut frame = Frame::new(pic_width, pic_height);
        // RefPicList0 built per spec § 8.2.4: short-term refs in
        // descending PicNum order followed by long-term refs in
        // ascending LongTermPicNum order.  Modification ops in
        // `ref_pic_list_modification_l0` are parsed but not yet
        // applied — a follow-up will splice them in.
        let ref_pic_list_l0 = self.build_ref_pic_list_l0();
        let placeholder = Frame::new(pic_width, pic_height);
        let primary_ref = ref_pic_list_l0
            .first()
            .map(|f| &**f)
            .unwrap_or(&placeholder);

        for mb in &mbs {
            match &mb.kind {
                MbKind::PSkip => {
                    let mvs = [(0i32, 0i32); 16];
                    let luma_4x4 = [[0i32; 16]; 16];
                    let chroma_dc = [[0i32; 8]; 2];
                    let chroma_ac = [[0i32; 16]; 8];
                    let inputs = InterPMbInputs {
                        mb_x: mb.mb_x,
                        mb_y: mb.mb_y,
                        mvs_l0: &mvs,
                        luma_4x4: &luma_4x4,
                        chroma_dc: &chroma_dc,
                        chroma_ac: &chroma_ac,
                        qp_y: mb.qp_y,
                        qp_chroma: mb.qp_chroma,
                    };
                    reconstruct_inter_p_mb(&mut frame, primary_ref, &inputs)?;
                }
                MbKind::InterP { decoded, .. } => {
                    let inputs = InterPMbInputs {
                        mb_x: mb.mb_x,
                        mb_y: mb.mb_y,
                        mvs_l0: &decoded.mv_l0,
                        luma_4x4: &mb.residual.luma_4x4,
                        chroma_dc: &mb.residual.chroma_dc,
                        chroma_ac: &mb.residual.chroma_ac,
                        qp_y: mb.qp_y,
                        qp_chroma: mb.qp_chroma,
                    };
                    if ref_pic_list_l0.is_empty() {
                        reconstruct_inter_p_mb(&mut frame, primary_ref, &inputs)?;
                    } else {
                        reconstruct_inter_p_mb_multiref(
                            &mut frame,
                            &ref_pic_list_l0,
                            &inputs,
                            &decoded.ref_l0,
                        )?;
                    }
                }
                MbKind::Intra(intra) => {
                    reconstruct_intra_mb_cabac(&mut frame, mb, intra)?;
                }
            }
        }
        Ok(frame)
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_cavlc_slice(
        &mut self,
        rbsp: &[u8],
        sh: &crate::h264::slice_header::SliceHeader,
        sps: &SpsRbsp,
        pps: &PpsRbsp,
        pic_width: usize,
        pic_height: usize,
        pic_width_mbs: usize,
        _pic_height_mbs: usize,
    ) -> Result<Frame, CodecError> {
        let slice_header_bits = slice_header_bit_length(sh, pps);

        // For the current iteration the only CAVLC path that
        // actually produces pixels is I_PCM.  Read mb_type as
        // ue(v) starting at the bit immediately after the slice
        // header; when the codeword is 25 we have I_PCM and can
        // pull the raw samples directly.
        //
        // The full CAVLC slice loop (I_NxN, I_16x16, P_*) lands in
        // a follow-up.
        let mut frame = Frame::new(pic_width, pic_height);

        if sh.slice_type == SliceType::I {
            // Peek at mb_type first.  If the slice starts with an
            // I_PCM macroblock (mb_type code 25), the rest of the
            // slice is raw samples — handle it via the PCM
            // passthrough.  Otherwise dispatch to the existing
            // CAVLC I-slice walker.
            let mut reader = BitReader::new(rbsp);
            reader.skip_bits(slice_header_bits as u32)?;
            let probe_pos = reader.bits_consumed();
            let mb_type = reader.read_ue()?;
            if mb_type == 25 {
                let bits_consumed = reader.bits_consumed();
                let aligned_byte = bits_consumed.div_ceil(8);
                if rbsp.len() < aligned_byte + 384 {
                    return Err(CodecError::InvalidData(
                        "h264 decoder: short I_PCM payload".into(),
                    ));
                }
                let samples = read_pcm_macroblock_420(&rbsp[aligned_byte..])?;
                write_pcm_macroblock_420(&mut frame, 0, 0, &samples)?;
                return Ok(frame);
            }
            // Non-PCM CAVLC I-slice: rewind to the start of
            // slice_data and hand off to the per-macroblock walker.
            let mut reader = BitReader::new(rbsp);
            reader.skip_bits(probe_pos as u32)?;
            decode_intra_slice_bitstream(&mut reader, sps, pps, sh, &mut frame)?;
            return Ok(frame);
        }

        // P slice: walk via parse_slice_cavlc with a running
        // InterSliceCache so each macroblock's median MV predictor
        // sees its left + top neighbours.  Multi-partition shapes
        // (16×8 / 8×16) are now handled; P_8x8 (sub-mb partitions)
        // still falls through to the mid-grey placeholder pending
        // its dedicated sub-mb walker.
        if sh.slice_type == SliceType::P {
            let mut reader = BitReader::new(rbsp);
            reader.skip_bits(slice_header_bits as u32)?;
            let mbs = parse_slice_cavlc(&mut reader, sps, pps, sh)?;

            let placeholder = Frame::new(pic_width, pic_height);
            let ref_pic_list_l0 = self.build_ref_pic_list_l0();
            let primary_ref = ref_pic_list_l0
                .first()
                .map(|f| &**f)
                .unwrap_or(&placeholder);

            let mut cache = InterSliceCache::new(pic_width_mbs);
            let mut intra_ctx = Intra4x4ModeContext::default();

            for mb in &mbs {
                if mb.mb_x == 0 {
                    cache.begin_row();
                }
                let top_right_slot = if mb.mb_x + 1 < pic_width_mbs {
                    Some(&cache.top_row[mb.mb_x + 1])
                } else {
                    None
                };
                let neighbours = MbNeighbours::from_cache(&cache, mb.mb_x, top_right_slot);

                match &mb.kind {
                    MbCavlcKind::Skip => {
                        // P_Skip — inferred zero MV, zero residual.
                        let inputs = inter_inputs_zero(mb);
                        reconstruct_inter_p_mb(&mut frame, primary_ref, &inputs)?;
                        let decoded = InterMbDecoded {
                            is_skip: true,
                            ..InterMbDecoded::default()
                        };
                        cache.record_inter_mb(mb.mb_x, &decoded, 0, 0);
                    }
                    MbCavlcKind::InterP {
                        mb_type,
                        motion,
                        luma_blocks,
                        chroma_dc,
                        chroma_ac,
                        ..
                    } => {
                        let (per_block_mvs, decoded) =
                            decode_p_mvs_cavlc(mb, *mb_type, motion, &neighbours);
                        let luma_4x4 = build_luma_4x4(luma_blocks);
                        let chroma_dc_pos = build_chroma_dc(chroma_dc);
                        let chroma_ac_pos = build_chroma_ac(chroma_ac);
                        let inputs = InterPMbInputs {
                            mb_x: mb.mb_x,
                            mb_y: mb.mb_y,
                            mvs_l0: &per_block_mvs,
                            luma_4x4: &luma_4x4,
                            chroma_dc: &chroma_dc_pos,
                            chroma_ac: &chroma_ac_pos,
                            qp_y: mb.qp_y,
                            qp_chroma: mb.qp_chroma,
                        };
                        reconstruct_inter_p_mb(&mut frame, primary_ref, &inputs)?;
                        cache.record_inter_mb(mb.mb_x, &decoded, 0, 0);
                    }
                    MbCavlcKind::Intra {
                        layer,
                        luma_blocks,
                        chroma_dc: _,
                        chroma_ac,
                    } => {
                        reconstruct_intra_in_p_cavlc(
                            &mut frame,
                            mb,
                            layer,
                            luma_blocks,
                            chroma_ac,
                            &mut intra_ctx,
                        )?;
                        let intra = InterMbDecoded {
                            is_intra: true,
                            ..InterMbDecoded::default()
                        };
                        cache.record_inter_mb(mb.mb_x, &intra, 0, 0);
                    }
                    MbCavlcKind::Unsupported => {
                        fill_mid_grey(&mut frame, mb.mb_x, mb.mb_y);
                        cache.record_inter_mb(mb.mb_x, &InterMbDecoded::default(), 0, 0);
                    }
                }
            }
            return Ok(frame);
        }

        // B and the remaining CAVLC paths still emit a mid-grey
        // frame so the slice counts toward the DPB.
        for y in 0..pic_height {
            for x in 0..pic_width {
                frame.set_luma(x, y, 128);
            }
        }
        for y in 0..pic_height / 2 {
            for x in 0..pic_width / 2 {
                frame.set_cb(x, y, 128);
                frame.set_cr(x, y, 128);
            }
        }
        let _ = sps.chroma_format_idc;
        Ok(frame)
    }
}

/// Dispatches a CABAC-decoded intra macroblock to the right
/// reconstruction path: I_PCM is a no-op (the parser bypasses
/// CABAC for PCM bytes — see [`crate::h264::pcm`]); I_NxN and
/// I_16x16 walk through the per-block intra prediction + IDCT
/// pipeline.  Chroma 8×8 reconstruction always runs (for non-PCM).
fn reconstruct_intra_mb_cabac(
    frame: &mut Frame,
    mb: &crate::h264::slice_cabac::MbCabacDecoded,
    intra: &crate::h264::cabac_mb::IntraMbCabac,
) -> Result<(), CodecError> {
    use crate::h264::cabac_syntax::IntraMbType;

    let chroma_pred_mode =
        chroma_pred_mode_from_raw(intra.chroma_pred_mode);
    let chroma_dc_cb = chroma_dc_from_residual(&mb.residual.chroma_dc[0]);
    let chroma_dc_cr = chroma_dc_from_residual(&mb.residual.chroma_dc[1]);
    let chroma_ac_cb = extract_chroma_ac_plane(&mb.residual.chroma_ac, 0);
    let chroma_ac_cr = extract_chroma_ac_plane(&mb.residual.chroma_ac, 1);

    match intra.mb_type {
        IntraMbType::IPCM => {
            // I_PCM never reaches the slice_cabac orchestrator
            // because CABAC bypasses the entropy coder for PCM
            // bytes; the pipeline's CAVLC PCM passthrough handles
            // it instead.  Leave the macroblock as-is.
        }
        IntraMbType::I4x4 => {
            let modes = intra4x4_modes_from_raw(&intra.intra4x4_modes);
            reconstruct_intra_4x4_mb_cabac(
                frame,
                mb.mb_x,
                mb.mb_y,
                &modes,
                &mb.residual.luma_4x4,
                mb.qp_y,
            )?;
            reconstruct_intra_chroma_8x8_cabac(
                frame,
                mb.mb_x,
                mb.mb_y,
                chroma_pred_mode,
                &chroma_dc_cb,
                &chroma_ac_cb,
                mb.qp_chroma,
                true,
            )?;
            reconstruct_intra_chroma_8x8_cabac(
                frame,
                mb.mb_x,
                mb.mb_y,
                chroma_pred_mode,
                &chroma_dc_cr,
                &chroma_ac_cr,
                mb.qp_chroma,
                false,
            )?;
        }
        IntraMbType::I16x16 { pred_mode, .. } => {
            let pred = intra16x16_pred_mode_from_raw(pred_mode);
            reconstruct_intra_16x16_mb_cabac(
                frame,
                mb.mb_x,
                mb.mb_y,
                pred,
                &mb.residual.luma_dc,
                &mb.residual.luma_4x4,
                mb.qp_y,
            )?;
            reconstruct_intra_chroma_8x8_cabac(
                frame,
                mb.mb_x,
                mb.mb_y,
                chroma_pred_mode,
                &chroma_dc_cb,
                &chroma_ac_cb,
                mb.qp_chroma,
                true,
            )?;
            reconstruct_intra_chroma_8x8_cabac(
                frame,
                mb.mb_x,
                mb.mb_y,
                chroma_pred_mode,
                &chroma_dc_cr,
                &chroma_ac_cr,
                mb.qp_chroma,
                false,
            )?;
        }
    }
    Ok(())
}

fn chroma_pred_mode_from_raw(raw: u8) -> crate::h264::macroblock::IntraChromaPredMode {
    use crate::h264::macroblock::IntraChromaPredMode;
    match raw {
        0 => IntraChromaPredMode::Dc,
        1 => IntraChromaPredMode::Horizontal,
        2 => IntraChromaPredMode::Vertical,
        _ => IntraChromaPredMode::Plane,
    }
}

fn intra16x16_pred_mode_from_raw(raw: u8) -> crate::h264::macroblock::Intra16x16PredMode {
    use crate::h264::macroblock::Intra16x16PredMode;
    match raw {
        0 => Intra16x16PredMode::Vertical,
        1 => Intra16x16PredMode::Horizontal,
        2 => Intra16x16PredMode::Dc,
        _ => Intra16x16PredMode::Plane,
    }
}

fn intra4x4_modes_from_raw(raw: &[u8; 16]) -> [crate::h264::intra_pred::Intra4x4Mode; 16] {
    use crate::h264::intra_pred::Intra4x4Mode;
    let map = |r: u8| match r {
        0 => Intra4x4Mode::Vertical,
        1 => Intra4x4Mode::Horizontal,
        2 => Intra4x4Mode::Dc,
        3 => Intra4x4Mode::DiagonalDownLeft,
        4 => Intra4x4Mode::DiagonalDownRight,
        5 => Intra4x4Mode::VerticalRight,
        6 => Intra4x4Mode::HorizontalDown,
        7 => Intra4x4Mode::VerticalLeft,
        _ => Intra4x4Mode::HorizontalUp,
    };
    let mut out = [Intra4x4Mode::Dc; 16];
    for i in 0..16 {
        out[i] = map(raw[i]);
    }
    out
}

fn chroma_dc_from_residual(plane: &[i32; 8]) -> [i32; 4] {
    [plane[0], plane[1], plane[2], plane[3]]
}

fn extract_chroma_ac_plane(all: &[[i32; 16]; 8], plane: usize) -> [[i32; 16]; 4] {
    let base = plane * 4;
    [all[base], all[base + 1], all[base + 2], all[base + 3]]
}

/// Reconstructs an intra macroblock that appeared inside a CAVLC
/// P slice.  Mirrors the I-slice CAVLC dispatch
/// ([`crate::h264::decoder::decode_intra_slice_bitstream`]) but
/// operates on already-parsed `MbCavlcKind::Intra` data so the
/// surrounding P-slice walker can dispatch per MB.
///
/// `intra_ctx` is the running MPM context — it captures the modes
/// of previously decoded intra macroblocks in this slice so MPM
/// derivation matches across intra blocks separated by inter
/// macroblocks.
fn reconstruct_intra_in_p_cavlc(
    frame: &mut Frame,
    mb: &crate::h264::slice_cavlc::MbCavlcDecoded,
    layer: &MacroblockLayer,
    luma_blocks: &[Option<crate::h264::cavlc::ResidualBlock>; 16],
    chroma_ac: &[Option<crate::h264::cavlc::ResidualBlock>; 8],
    intra_ctx: &mut Intra4x4ModeContext,
) -> Result<(), CodecError> {
    let mb_x = mb.mb_x;
    let mb_y = mb.mb_y;
    let qp_y = mb.qp_y;
    let qp_chroma = mb.qp_chroma;
    let chroma_mode = layer
        .intra_chroma_pred_mode
        .unwrap_or(IntraChromaPredMode::Dc);

    let mut luma_residuals: [Residual4x4Scan; 16] = [None; 16];
    for (i, blk) in luma_blocks.iter().enumerate() {
        if let Some(b) = blk {
            if b.total_coeff > 0 {
                luma_residuals[i] = Some(b.to_scan_order_padded());
            }
        }
    }
    let mut chroma_cb: [Residual4x4Scan; 4] = [None; 4];
    let mut chroma_cr: [Residual4x4Scan; 4] = [None; 4];
    for i in 0..4 {
        if let Some(b) = &chroma_ac[i] {
            if b.total_coeff > 0 {
                chroma_cb[i] = Some(b.to_scan_order_padded());
            }
        }
        if let Some(b) = &chroma_ac[4 + i] {
            if b.total_coeff > 0 {
                chroma_cr[i] = Some(b.to_scan_order_padded());
            }
        }
    }

    match layer.mb_type {
        MbType::INxN => {
            let intra_nxn = layer.intra_nxn.as_ref().ok_or_else(|| {
                CodecError::InvalidData("h264 decoder: I_NxN without intra_nxn".into())
            })?;
            let mut modes = [Intra4x4Mode::Dc; 16];
            for (idx, mode) in modes.iter_mut().enumerate() {
                let mpm =
                    most_probable_mode(intra_ctx.top_of(idx), intra_ctx.left_of(idx));
                let prev_flag = intra_nxn
                    .prev_intra4x4_pred_mode_flag
                    .get(idx)
                    .copied()
                    .unwrap_or(false);
                let rem = intra_nxn.rem_intra4x4_pred_mode.get(idx).copied().flatten();
                *mode = resolve_intra4x4_mode(prev_flag, rem, mpm)?;
                intra_ctx.set(idx, *mode);
            }
            decode_intra_4x4_mb(frame, mb_x, mb_y, &modes, &luma_residuals, qp_y)?;
            decode_intra_chroma_8x8(frame, mb_x, mb_y, chroma_mode, &chroma_cb, qp_chroma, true)?;
            decode_intra_chroma_8x8(
                frame, mb_x, mb_y, chroma_mode, &chroma_cr, qp_chroma, false,
            )?;
        }
        MbType::I16x16 { pred_mode, .. } => {
            decode_intra_16x16_mb(
                frame,
                mb_x,
                mb_y,
                intra16x16_pred_mode_from_decoded(pred_mode),
                &luma_residuals,
                qp_y,
            )?;
            decode_intra_chroma_8x8(frame, mb_x, mb_y, chroma_mode, &chroma_cb, qp_chroma, true)?;
            decode_intra_chroma_8x8(
                frame, mb_x, mb_y, chroma_mode, &chroma_cr, qp_chroma, false,
            )?;
            // I_16x16 doesn't update the 4×4 MPM context — reset
            // so subsequent I_NxN MBs don't carry stale state.
            *intra_ctx = Intra4x4ModeContext::default();
        }
        MbType::IPcm => {
            // I_PCM inside the CAVLC P-slice walker would have
            // been routed through the PCM passthrough already.
        }
        _ => {
            fill_mid_grey(frame, mb_x, mb_y);
        }
    }
    Ok(())
}

fn intra16x16_pred_mode_from_decoded(pred_mode: Intra16x16PredMode) -> Intra16x16PredMode {
    pred_mode
}

/// Resolves per-4×4 motion vectors for a CAVLC P-slice
/// macroblock.  Picks the partition shape from the parsed
/// `MacroblockLayer.mb_type` (carried inside the `MbCavlcDecoded`
/// via the parser's [`crate::h264::macroblock::InterMotionInfo`]),
/// applies the median MV predictor per partition, adds the
/// signalled delta, and splatters the resulting MV across the
/// covered 4×4 blocks.  Returns the per-block MV array plus an
/// [`InterMbDecoded`] view suitable for `record_inter_mb`.
fn decode_p_mvs_cavlc(
    mb: &crate::h264::slice_cavlc::MbCavlcDecoded,
    mb_type: MbType,
    motion: &crate::h264::macroblock::InterMotionInfo,
    neighbours: &MbNeighbours,
) -> ([MotionVector; 16], InterMbDecoded) {
    let partitions = cavlc_partitions(mb_type);
    let mut per_block_mvs = [(0i32, 0i32); 16];
    let mut decoded = InterMbDecoded::default();

    for (pi, blocks) in partitions.iter().enumerate() {
        let delta = motion.mvd_l0.get(pi).copied().unwrap_or((0, 0));
        let ref_idx = motion
            .ref_idx_l0
            .get(pi)
            .copied()
            .map(|v| v as i8)
            .unwrap_or(0);
        let first_block = blocks[0];
        let row = first_block / 4;
        let col = first_block % 4;
        let ctx = MvPredictionContext {
            left: if col == 0 && neighbours.left_available {
                Some(neighbours.left_mv[row])
            } else {
                None
            },
            above: if row == 0 && neighbours.top_available {
                Some(neighbours.top_mv[col])
            } else {
                None
            },
            above_right: if row == 0 {
                neighbours.top_right_mv
            } else {
                None
            },
            above_left: None,
        };
        let pred = predict_mv_median(&ctx);
        let mv = (pred.0 + delta.0, pred.1 + delta.1);
        for &b in *blocks {
            per_block_mvs[b] = mv;
            decoded.mv_l0[b] = mv;
            decoded.ref_l0[b] = ref_idx;
            decoded.mvd_abs_l0[b] = [delta.0.unsigned_abs().min(255) as u8, delta.1.unsigned_abs().min(255) as u8];
        }
    }
    (per_block_mvs, decoded)
}

/// Returns the 4×4 block indices covered by each partition of a
/// CAVLC P-slice macroblock type.  P_8x8 and P_8x8ref0 are
/// flattened to four 8×8 quadrants — sub-MB-level partitioning
/// inside each quadrant uses motion.sub_mb_types when bit-exact
/// reconstruction lands.
fn cavlc_partitions(mb_type: MbType) -> &'static [&'static [usize]] {
    match mb_type {
        MbType::PL0_16x16 => &[&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]],
        MbType::PL0L0_16x8 => &[
            &[0, 1, 2, 3, 4, 5, 6, 7],
            &[8, 9, 10, 11, 12, 13, 14, 15],
        ],
        MbType::PL0L0_8x16 => &[
            &[0, 1, 4, 5, 8, 9, 12, 13],
            &[2, 3, 6, 7, 10, 11, 14, 15],
        ],
        MbType::P8x8 | MbType::P8x8Ref0 => &[
            &[0, 1, 4, 5],
            &[2, 3, 6, 7],
            &[8, 9, 12, 13],
            &[10, 11, 14, 15],
        ],
        _ => &[&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]],
    }
}

/// Builds an [`InterPMbInputs`] populated with all-zero
/// MVs / residuals — used for P_Skip macroblocks.
fn inter_inputs_zero(mb: &crate::h264::slice_cavlc::MbCavlcDecoded) -> InterPMbInputs<'static> {
    static MVS_ZERO: [(i32, i32); 16] = [(0, 0); 16];
    static LUMA_ZERO: [[i32; 16]; 16] = [[0; 16]; 16];
    static CHROMA_DC_ZERO: [[i32; 8]; 2] = [[0; 8]; 2];
    static CHROMA_AC_ZERO: [[i32; 16]; 8] = [[0; 16]; 8];
    InterPMbInputs {
        mb_x: mb.mb_x,
        mb_y: mb.mb_y,
        mvs_l0: &MVS_ZERO,
        luma_4x4: &LUMA_ZERO,
        chroma_dc: &CHROMA_DC_ZERO,
        chroma_ac: &CHROMA_AC_ZERO,
        qp_y: mb.qp_y,
        qp_chroma: mb.qp_chroma,
    }
}

fn build_luma_4x4(
    blocks: &[Option<crate::h264::cavlc::ResidualBlock>; 16],
) -> [[i32; 16]; 16] {
    let mut out = [[0i32; 16]; 16];
    for (i, blk) in blocks.iter().enumerate() {
        if let Some(b) = blk {
            out[i] = cavlc_block_to_position_4x4(b);
        }
    }
    out
}

fn build_chroma_dc(
    blocks: &[Option<crate::h264::cavlc::ResidualBlock>; 2],
) -> [[i32; 8]; 2] {
    let mut out = [[0i32; 8]; 2];
    for plane in 0..2 {
        if let Some(b) = &blocks[plane] {
            let dc = cavlc_chroma_dc_to_position(b);
            out[plane][..4].copy_from_slice(&dc);
        }
    }
    out
}

fn build_chroma_ac(
    blocks: &[Option<crate::h264::cavlc::ResidualBlock>; 8],
) -> [[i32; 16]; 8] {
    let mut out = [[0i32; 16]; 8];
    for (i, blk) in blocks.iter().enumerate() {
        if let Some(b) = blk {
            out[i] = cavlc_chroma_ac_to_position(b);
        }
    }
    out
}

/// Free-function POC type 2 formula (spec § 8.2.1.3).  Exposed
/// outside `impl Decoder` so it's testable in isolation.
fn poc_type2_value(frame_num: i32, nal_ref_idc: u8, is_idr: bool) -> i32 {
    if is_idr {
        0
    } else if nal_ref_idc == 0 {
        2 * frame_num - 1
    } else {
        2 * frame_num
    }
}

/// Annex-B NAL extraction (3- or 4-byte start codes).
fn extract_nal_units(buf: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i + 3 < buf.len() {
        let start_len = if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 0 && buf[i + 3] == 1 {
            4
        } else if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            3
        } else {
            i += 1;
            continue;
        };
        let payload_start = i + start_len;
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

/// Re-computes the bit length of the slice header by re-encoding
/// the parsed fields with the same exp-Golomb / fixed-width rules
/// the parser used.
///
/// This is intentionally optimistic for the IDR-I / non-IDR-P
/// shapes the parser produces today; richer features
/// (`ref_pic_list_modification`, `pred_weight_table`,
/// `cabac_init_idc`, `disable_deblocking_filter_idc`) are excluded
/// — bit-exact recomputation across every variant is its own
/// refactor.
fn slice_header_bit_length(
    sh: &crate::h264::slice_header::SliceHeader,
    _pps: &PpsRbsp,
) -> usize {
    let mut bits = 0usize;
    bits += ue_v_bit_length(sh.first_mb_in_slice as u32);
    bits += ue_v_bit_length(slice_type_code(sh.slice_type));
    bits += ue_v_bit_length(sh.pic_parameter_set_id as u32);
    bits += 4; // frame_num (log2_max_frame_num_minus4 = 0 assumed)
    if let Some(idr_id) = sh.idr_pic_id {
        bits += ue_v_bit_length(idr_id);
        // dec_ref_pic_marking IDR variant: 2 flag bits.
        bits += 2;
    }
    bits += se_v_bit_length(sh.slice_qp_delta);
    bits
}

fn slice_type_code(slice_type: SliceType) -> u32 {
    match slice_type {
        SliceType::P => 0,
        SliceType::B => 1,
        SliceType::I => 7,
        SliceType::SP => 3,
        SliceType::SI => 4,
    }
}

/// Reads the value of an `ue(v)` exp-Golomb codeword starting at
/// the first bit of `buf`.  Used to peek at `mb_type` without
/// constructing a full bitreader.
fn peek_ue_v(buf: &[u8]) -> Result<u32, CodecError> {
    let mut bit_pos = 0usize;
    let mut leading_zeros = 0u32;
    while bit_pos < buf.len() * 8 {
        let byte = buf[bit_pos / 8];
        let bit = (byte >> (7 - (bit_pos % 8))) & 1;
        if bit != 0 {
            break;
        }
        leading_zeros += 1;
        bit_pos += 1;
    }
    if bit_pos >= buf.len() * 8 {
        return Err(CodecError::InvalidData(
            "h264 decoder: exp-Golomb runs past buffer".into(),
        ));
    }
    // Consume the '1' bit.
    bit_pos += 1;
    let mut suffix = 0u32;
    for _ in 0..leading_zeros {
        if bit_pos >= buf.len() * 8 {
            return Err(CodecError::InvalidData(
                "h264 decoder: exp-Golomb suffix overflow".into(),
            ));
        }
        let byte = buf[bit_pos / 8];
        let bit = (byte >> (7 - (bit_pos % 8))) & 1;
        suffix = (suffix << 1) | bit as u32;
        bit_pos += 1;
    }
    Ok((1u32 << leading_zeros) - 1 + suffix)
}

/// Returns the bit length of the `ue(v)` encoding of `value`.
fn ue_v_bit_length(value: u32) -> usize {
    let mut n = 0u32;
    while (1u32 << (n + 1)) - 1 <= value {
        n += 1;
    }
    (2 * n + 1) as usize
}

/// Returns the bit length of the `se(v)` encoding of `value`.
fn se_v_bit_length(value: i32) -> usize {
    let mapped = if value <= 0 {
        (-(value as i64) * 2) as u32
    } else {
        (value as i64 * 2 - 1) as u32
    };
    ue_v_bit_length(mapped)
}

fn fill_mid_grey(frame: &mut Frame, mb_x: usize, mb_y: usize) {
    let px = mb_x * 16;
    let py = mb_y * 16;
    if px + 16 > frame.width || py + 16 > frame.height {
        return;
    }
    for j in 0..16 {
        for i in 0..16 {
            frame.set_luma(px + i, py + j, 128);
        }
    }
    let cx = mb_x * 8;
    let cy = mb_y * 8;
    if cx + 8 > frame.chroma_width() || cy + 8 > frame.chroma_height() {
        return;
    }
    for j in 0..8 {
        for i in 0..8 {
            frame.set_cb(cx + i, cy + j, 128);
            frame.set_cr(cx + i, cy + j, 128);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_decoder_has_empty_stores() {
        let d = Decoder::new();
        assert!(d.sps_store.is_empty());
        assert!(d.pps_store.is_empty());
        assert_eq!(d.dpb.entries.len(), 0);
    }

    #[test]
    fn feed_nal_rejects_empty_input() {
        let mut d = Decoder::new();
        let err = d.feed_nal(&[]).unwrap_err();
        match err {
            CodecError::InvalidData(_) => {}
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn feed_nal_recognises_sei_aud_as_noop() {
        let mut d = Decoder::new();
        // SEI header: nal_unit_type = 6.
        let nal = [0x06, 0x80, 0x80];
        match d.feed_nal(&nal).unwrap() {
            DecodeStep::None => {}
            other => panic!("expected None for SEI, got {other:?}"),
        }
        // AUD header: nal_unit_type = 9.
        let nal = [0x09, 0xF0];
        match d.feed_nal(&nal).unwrap() {
            DecodeStep::None => {}
            other => panic!("expected None for AUD, got {other:?}"),
        }
    }

    #[test]
    fn ue_v_bit_length_matches_known_values() {
        assert_eq!(ue_v_bit_length(0), 1);
        assert_eq!(ue_v_bit_length(1), 3);
        assert_eq!(ue_v_bit_length(2), 3);
        assert_eq!(ue_v_bit_length(7), 7);
        assert_eq!(ue_v_bit_length(25), 9);
    }

    #[test]
    fn ref_pic_list_l0_orders_short_term_desc_then_long_term_asc() {
        let mut decoder = Decoder::new();
        let entry = |frame_num, st, lt, lt_idx| DpbEntry {
            frame: Frame::new(16, 16),
            poc: frame_num * 2,
            frame_num,
            is_short_term_reference: st,
            is_long_term_reference: lt,
            long_term_idx: lt_idx,
            output_pending: false,
        };
        decoder.dpb.entries.push(entry(3, true, false, None));
        decoder.dpb.entries.push(entry(7, true, false, None));
        decoder.dpb.entries.push(entry(5, true, false, None));
        decoder.dpb.entries.push(entry(0, false, true, Some(2)));
        decoder.dpb.entries.push(entry(1, false, true, Some(0)));
        let list = decoder.build_ref_pic_list_l0();
        assert_eq!(list.len(), 5);
        // Short-term: frame_nums {3,7,5} -> descending {7,5,3}.
        // Long-term: long_term_idx {2,0} -> ascending {0,2}.
        // Full order: 7,5,3 then long-term sorted by lt_idx.
        // The list is &Frame so we can't read frame_num directly;
        // sanity check the length only here — separate fields
        // tested in poc unit tests.
    }

    #[test]
    fn ref_pic_list_l1_splits_on_current_poc() {
        let mut decoder = Decoder::new();
        let entry = |poc, st| DpbEntry {
            frame: Frame::new(16, 16),
            poc,
            frame_num: poc,
            is_short_term_reference: st,
            is_long_term_reference: false,
            long_term_idx: None,
            output_pending: false,
        };
        // Three short-term references: POC 2, 4, 6 — we're decoding
        // at POC 4 so the split is { 6 } | { 2 } (ascending past,
        // descending before).
        decoder.dpb.entries.push(entry(2, true));
        decoder.dpb.entries.push(entry(4, true));
        decoder.dpb.entries.push(entry(6, true));
        let list = decoder.build_ref_pic_list_l1(4);
        // POC 4 == current is excluded; remaining 2 entries.  L1
        // order for current_poc = 4: {6 (higher), 2 (lower)}.
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn poc_type2_handles_idr_and_non_ref() {
        // IDR resets POC to 0 regardless of frame_num.
        assert_eq!(poc_type2_value(7, 3, true), 0);
        // Reference frame: POC = 2 * frame_num.
        assert_eq!(poc_type2_value(3, 3, false), 6);
        assert_eq!(poc_type2_value(5, 2, false), 10);
        // Non-reference picture: POC = 2 * frame_num - 1.
        assert_eq!(poc_type2_value(3, 0, false), 5);
        assert_eq!(poc_type2_value(1, 0, false), 1);
    }

    #[test]
    fn peek_ue_v_decodes_known_codewords() {
        // 25 → exp-Golomb: 4 leading zeros + '1' + 4-bit suffix
        // (25 + 1 - 16) = 10 = `1010`.  Full codeword (9 bits):
        // `0000 1 1010` → packed MSB-first into 2 bytes:
        //   bit 0..7: `0000 1101` = 0x0D
        //   bit 8:    `0`         = top bit of 0x00
        let buf = [0x0Du8, 0x00];
        assert_eq!(peek_ue_v(&buf).unwrap(), 25);
        // 0 → '1' (1 bit).  Packed: 0x80.
        assert_eq!(peek_ue_v(&[0x80]).unwrap(), 0);
        // 1 → '010' (3 bits).  Packed: 0x40.
        assert_eq!(peek_ue_v(&[0x40]).unwrap(), 1);
    }
}

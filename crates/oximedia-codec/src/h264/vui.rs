//! H.264 Video Usability Information (VUI) parsing.
//!
//! Implements `vui_parameters()` from H.264 Annex E §E.1.1. VUI carries
//! the metadata that decoders/renderers need to display the picture
//! correctly — sample aspect ratio, color signaling (primaries /
//! transfer / matrix coefficients / range), chroma sample location,
//! frame rate, and HRD (Hypothetical Reference Decoder) buffer
//! parameters.
//!
//! VUI is optional in the bitstream (gated by
//! `vui_parameters_present_flag` in the SPS). When present, downstream
//! consumers care most about the color-signaling fields — the four
//! integers that cause 90% of production color bugs.

use crate::h264::bit_reader::BitReader;
use crate::CodecError;

/// Sample-aspect-ratio data, signaling the per-pixel display shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AspectRatioInfo {
    /// Spec-defined index from H.264 Table E-1. Special value 255
    /// (`Extended_SAR`) means `sar_width` / `sar_height` follow.
    pub aspect_ratio_idc: u8,
    /// Present only when `aspect_ratio_idc == 255` (Extended_SAR).
    pub extended_sar: Option<ExtendedSar>,
}

/// Width and height of an extended sample aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedSar {
    /// Horizontal portion of the SAR.
    pub sar_width: u16,
    /// Vertical portion of the SAR.
    pub sar_height: u16,
}

/// Video signal characteristics: the color-bug-prevention quartet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSignalType {
    /// `video_format` — 0=Component, 1=PAL, 2=NTSC, 3=SECAM, 4=MAC,
    /// 5=unspecified.
    pub video_format: u8,
    /// `video_full_range_flag` — true if the stream uses full-range
    /// quantization (0–255 / 0–1023), false for studio-swing
    /// (16–235 / 64–940).
    pub video_full_range_flag: bool,
    /// Optional color description: primaries, transfer characteristics,
    /// matrix coefficients. Codes per ITU-T H.273.
    pub colour_description: Option<ColourDescription>,
}

/// Color signaling per H.273.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourDescription {
    /// `colour_primaries` (e.g. 1 = BT.709, 9 = BT.2020, 12 = DCI-P3).
    pub colour_primaries: u8,
    /// `transfer_characteristics` (e.g. 1 = BT.709, 16 = PQ, 18 = HLG).
    pub transfer_characteristics: u8,
    /// `matrix_coefficients` (e.g. 1 = BT.709, 9 = BT.2020 NCL).
    pub matrix_coefficients: u8,
}

/// Chroma sample location for top and bottom fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromaLocInfo {
    /// Top-field chroma siting type. Common values: 0 = MPEG-2
    /// (top-left), 2 = MPEG-1 (centered).
    pub top_field: u32,
    /// Bottom-field chroma siting type.
    pub bottom_field: u32,
}

/// Timing model: derived frame rate is
/// `time_scale / (num_units_in_tick * 2)` for progressive content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingInfo {
    /// Number of time units of `time_scale` that pass in one tick.
    pub num_units_in_tick: u32,
    /// Frequency in Hz at which the clock operates.
    pub time_scale: u32,
    /// True if the stream guarantees a constant frame rate.
    pub fixed_frame_rate_flag: bool,
}

/// One per-CPB schedule entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpbSchedule {
    /// Bit rate value indexed by `bit_rate_scale` (final rate in bits/s
    /// is `(bit_rate_value_minus1 + 1) << (6 + bit_rate_scale)`).
    pub bit_rate_value_minus1: u32,
    /// CPB size value indexed by `cpb_size_scale`.
    pub cpb_size_value_minus1: u32,
    /// True if this CPB schedule is constant bitrate.
    pub cbr_flag: bool,
}

/// `hrd_parameters()` from H.264 §E.1.2. Models the abstract decoder's
/// input buffer constraints. Encoders use these to keep bitrate spikes
/// within bounds the spec allows real decoders to handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrdParameters {
    /// Number of alternative CPB specifications minus 1.
    pub cpb_cnt_minus1: u32,
    /// Scale factor applied to `bit_rate_value_minus1`.
    pub bit_rate_scale: u8,
    /// Scale factor applied to `cpb_size_value_minus1`.
    pub cpb_size_scale: u8,
    /// Per-CPB schedule (length `cpb_cnt_minus1 + 1`).
    pub schedule: Vec<CpbSchedule>,
    /// Length in bits of `initial_cpb_removal_delay` syntax elements.
    pub initial_cpb_removal_delay_length_minus1: u8,
    /// Length in bits of `cpb_removal_delay` syntax elements.
    pub cpb_removal_delay_length_minus1: u8,
    /// Length in bits of `dpb_output_delay` syntax elements.
    pub dpb_output_delay_length_minus1: u8,
    /// Length in bits of the `time_offset` syntax element.
    pub time_offset_length: u8,
}

/// `bitstream_restriction()` from H.264 §E.1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitstreamRestriction {
    /// `motion_vectors_over_pic_boundaries_flag`.
    pub motion_vectors_over_pic_boundaries_flag: bool,
    /// `max_bytes_per_pic_denom`.
    pub max_bytes_per_pic_denom: u32,
    /// `max_bits_per_mb_denom`.
    pub max_bits_per_mb_denom: u32,
    /// `log2_max_mv_length_horizontal`.
    pub log2_max_mv_length_horizontal: u32,
    /// `log2_max_mv_length_vertical`.
    pub log2_max_mv_length_vertical: u32,
    /// `max_num_reorder_frames`.
    pub max_num_reorder_frames: u32,
    /// `max_dec_frame_buffering`.
    pub max_dec_frame_buffering: u32,
}

/// Top-level VUI parameters as parsed from the SPS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VuiParameters {
    /// Aspect ratio info, when signalled.
    pub aspect_ratio_info: Option<AspectRatioInfo>,
    /// `overscan_appropriate_flag` value, when signalled.
    pub overscan_appropriate_flag: Option<bool>,
    /// Video signal characteristics, when signalled. Holds the four
    /// color-tag fields plus full-range flag.
    pub video_signal_type: Option<VideoSignalType>,
    /// Chroma sample location, when signalled.
    pub chroma_loc_info: Option<ChromaLocInfo>,
    /// Timing model, when signalled.
    pub timing_info: Option<TimingInfo>,
    /// HRD parameters for NAL HRD, when signalled.
    pub nal_hrd_parameters: Option<HrdParameters>,
    /// HRD parameters for VCL HRD, when signalled.
    pub vcl_hrd_parameters: Option<HrdParameters>,
    /// `low_delay_hrd_flag` — present iff either HRD parameter set is.
    pub low_delay_hrd_flag: Option<bool>,
    /// `pic_struct_present_flag`.
    pub pic_struct_present_flag: bool,
    /// Bitstream restriction parameters, when signalled.
    pub bitstream_restriction: Option<BitstreamRestriction>,
}

/// Parses `vui_parameters()` from the current position of `r`.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] on malformed bitstreams.
pub fn parse_vui(r: &mut BitReader<'_>) -> Result<VuiParameters, CodecError> {
    let mut vui = VuiParameters::default();

    if r.read_bit()? {
        // aspect_ratio_info_present_flag
        let aspect_ratio_idc = r.read_bits(8)? as u8;
        let extended_sar = if aspect_ratio_idc == 255 {
            Some(ExtendedSar {
                sar_width: r.read_bits(16)? as u16,
                sar_height: r.read_bits(16)? as u16,
            })
        } else {
            None
        };
        vui.aspect_ratio_info = Some(AspectRatioInfo {
            aspect_ratio_idc,
            extended_sar,
        });
    }

    if r.read_bit()? {
        // overscan_info_present_flag
        vui.overscan_appropriate_flag = Some(r.read_bit()?);
    }

    if r.read_bit()? {
        // video_signal_type_present_flag
        let video_format = r.read_bits(3)? as u8;
        let video_full_range_flag = r.read_bit()?;
        let colour_description = if r.read_bit()? {
            Some(ColourDescription {
                colour_primaries: r.read_bits(8)? as u8,
                transfer_characteristics: r.read_bits(8)? as u8,
                matrix_coefficients: r.read_bits(8)? as u8,
            })
        } else {
            None
        };
        vui.video_signal_type = Some(VideoSignalType {
            video_format,
            video_full_range_flag,
            colour_description,
        });
    }

    if r.read_bit()? {
        // chroma_loc_info_present_flag
        vui.chroma_loc_info = Some(ChromaLocInfo {
            top_field: r.read_ue()?,
            bottom_field: r.read_ue()?,
        });
    }

    if r.read_bit()? {
        // timing_info_present_flag
        vui.timing_info = Some(TimingInfo {
            num_units_in_tick: r.read_bits(32)?,
            time_scale: r.read_bits(32)?,
            fixed_frame_rate_flag: r.read_bit()?,
        });
    }

    let nal_hrd_present = r.read_bit()?;
    if nal_hrd_present {
        vui.nal_hrd_parameters = Some(parse_hrd_parameters(r)?);
    }
    let vcl_hrd_present = r.read_bit()?;
    if vcl_hrd_present {
        vui.vcl_hrd_parameters = Some(parse_hrd_parameters(r)?);
    }
    if nal_hrd_present || vcl_hrd_present {
        vui.low_delay_hrd_flag = Some(r.read_bit()?);
    }

    vui.pic_struct_present_flag = r.read_bit()?;

    if r.read_bit()? {
        // bitstream_restriction_flag
        vui.bitstream_restriction = Some(BitstreamRestriction {
            motion_vectors_over_pic_boundaries_flag: r.read_bit()?,
            max_bytes_per_pic_denom: r.read_ue()?,
            max_bits_per_mb_denom: r.read_ue()?,
            log2_max_mv_length_horizontal: r.read_ue()?,
            log2_max_mv_length_vertical: r.read_ue()?,
            max_num_reorder_frames: r.read_ue()?,
            max_dec_frame_buffering: r.read_ue()?,
        });
    }

    Ok(vui)
}

fn parse_hrd_parameters(r: &mut BitReader<'_>) -> Result<HrdParameters, CodecError> {
    let cpb_cnt_minus1 = r.read_ue()?;
    let bit_rate_scale = r.read_bits(4)? as u8;
    let cpb_size_scale = r.read_bits(4)? as u8;
    let mut schedule = Vec::with_capacity((cpb_cnt_minus1 + 1) as usize);
    for _ in 0..=cpb_cnt_minus1 {
        schedule.push(CpbSchedule {
            bit_rate_value_minus1: r.read_ue()?,
            cpb_size_value_minus1: r.read_ue()?,
            cbr_flag: r.read_bit()?,
        });
    }
    Ok(HrdParameters {
        cpb_cnt_minus1,
        bit_rate_scale,
        cpb_size_scale,
        schedule,
        initial_cpb_removal_delay_length_minus1: r.read_bits(5)? as u8,
        cpb_removal_delay_length_minus1: r.read_bits(5)? as u8,
        dpb_output_delay_length_minus1: r.read_bits(5)? as u8,
        time_offset_length: r.read_bits(5)? as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_vui_with_color_tags() {
        // Build a VUI with:
        //   aspect_ratio_info_present = 1, aspect_ratio_idc = 1 (square)
        //   overscan_info_present = 0
        //   video_signal_type_present = 1
        //     video_format = 5, video_full_range_flag = 0,
        //     colour_description_present = 1
        //       primaries = 1, transfer = 1, matrix = 1
        //   chroma_loc_info_present = 0
        //   timing_info_present = 0
        //   nal_hrd_present = 0
        //   vcl_hrd_present = 0
        //   pic_struct_present = 0
        //   bitstream_restriction = 0
        let mut bits = Vec::new();
        bits.push(true); // aspect_ratio_info_present_flag
        push_bits_msb(&mut bits, 1, 8); // aspect_ratio_idc = 1
        bits.push(false); // overscan_info_present_flag
        bits.push(true); // video_signal_type_present_flag
        push_bits_msb(&mut bits, 5, 3); // video_format
        bits.push(false); // video_full_range_flag
        bits.push(true); // colour_description_present_flag
        push_bits_msb(&mut bits, 1, 8); // colour_primaries
        push_bits_msb(&mut bits, 1, 8); // transfer_characteristics
        push_bits_msb(&mut bits, 1, 8); // matrix_coefficients
        bits.push(false); // chroma_loc_info_present_flag
        bits.push(false); // timing_info_present_flag
        bits.push(false); // nal_hrd_parameters_present_flag
        bits.push(false); // vcl_hrd_parameters_present_flag
        bits.push(false); // pic_struct_present_flag
        bits.push(false); // bitstream_restriction_flag

        // Pad to a byte boundary so the BitReader has a buffer to read.
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let payload = pack_bits_msb(&bits);

        let mut r = BitReader::new(&payload);
        let vui = parse_vui(&mut r).expect("should parse");
        assert_eq!(
            vui.aspect_ratio_info.as_ref().map(|a| a.aspect_ratio_idc),
            Some(1)
        );
        let signal = vui.video_signal_type.as_ref().unwrap();
        assert_eq!(signal.video_format, 5);
        assert!(!signal.video_full_range_flag);
        let colour = signal.colour_description.as_ref().unwrap();
        assert_eq!(colour.colour_primaries, 1);
        assert_eq!(colour.transfer_characteristics, 1);
        assert_eq!(colour.matrix_coefficients, 1);
    }

    #[test]
    fn extended_sar_carries_width_and_height() {
        let mut bits = Vec::new();
        bits.push(true); // aspect_ratio_info_present_flag
        push_bits_msb(&mut bits, 255, 8); // Extended_SAR
        push_bits_msb(&mut bits, 64, 16); // sar_width
        push_bits_msb(&mut bits, 33, 16); // sar_height
        bits.push(false); // overscan_info_present_flag
        bits.push(false); // video_signal_type_present_flag
        bits.push(false); // chroma_loc_info_present_flag
        bits.push(false); // timing_info_present_flag
        bits.push(false); // nal_hrd
        bits.push(false); // vcl_hrd
        bits.push(false); // pic_struct
        bits.push(false); // bitstream_restriction
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let payload = pack_bits_msb(&bits);
        let mut r = BitReader::new(&payload);
        let vui = parse_vui(&mut r).unwrap();
        let ext = vui.aspect_ratio_info.unwrap().extended_sar.unwrap();
        assert_eq!(ext.sar_width, 64);
        assert_eq!(ext.sar_height, 33);
    }

    #[test]
    fn hrd_parameters_round_trip() {
        // 1 CPB schedule, bit_rate_scale = 4, cpb_size_scale = 5,
        // bit_rate_value_minus1 = 7, cpb_size_value_minus1 = 9, cbr = 1.
        let mut bits = Vec::new();
        push_ue(&mut bits, 0); // cpb_cnt_minus1
        push_bits_msb(&mut bits, 4, 4); // bit_rate_scale
        push_bits_msb(&mut bits, 5, 4); // cpb_size_scale
        push_ue(&mut bits, 7); // bit_rate_value_minus1
        push_ue(&mut bits, 9); // cpb_size_value_minus1
        bits.push(true); // cbr_flag
        push_bits_msb(&mut bits, 23, 5); // initial_cpb_removal_delay_length_minus1
        push_bits_msb(&mut bits, 23, 5); // cpb_removal_delay_length_minus1
        push_bits_msb(&mut bits, 23, 5); // dpb_output_delay_length_minus1
        push_bits_msb(&mut bits, 24, 5); // time_offset_length
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let payload = pack_bits_msb(&bits);
        let mut r = BitReader::new(&payload);
        let hrd = parse_hrd_parameters(&mut r).expect("should parse");
        assert_eq!(hrd.cpb_cnt_minus1, 0);
        assert_eq!(hrd.bit_rate_scale, 4);
        assert_eq!(hrd.cpb_size_scale, 5);
        assert_eq!(hrd.schedule.len(), 1);
        assert_eq!(hrd.schedule[0].bit_rate_value_minus1, 7);
        assert_eq!(hrd.schedule[0].cpb_size_value_minus1, 9);
        assert!(hrd.schedule[0].cbr_flag);
        assert_eq!(hrd.initial_cpb_removal_delay_length_minus1, 23);
        assert_eq!(hrd.time_offset_length, 24);
    }

    // -- bit-building helpers (kept self-contained per module convention) --

    fn push_ue(bits: &mut Vec<bool>, value: u32) {
        let mut n = 0u32;
        while (1u32 << (n + 1)) - 1 <= value {
            n += 1;
            assert!(n <= 31);
        }
        for _ in 0..n {
            bits.push(false);
        }
        bits.push(true);
        let suffix = value + 1 - (1u32 << n);
        push_bits_msb(bits, suffix, n);
    }

    fn push_bits_msb(bits: &mut Vec<bool>, mut value: u32, n: u32) {
        let mask = if n == 0 { 0 } else { 1u32 << (n - 1) };
        for _ in 0..n {
            bits.push(value & mask != 0);
            value <<= 1;
        }
    }

    fn pack_bits_msb(bits: &[bool]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bits.len() / 8 + 1);
        let mut byte = 0u8;
        let mut count = 0u8;
        for &b in bits {
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

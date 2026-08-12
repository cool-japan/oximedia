//! VP8 lossy encoder for WebP.
//!
//! Produces conforming VP8 key frames suitable for embedding in a WebP
//! container: RGB to YUV 4:2:0 (BT.601), 16x16 macroblocks with whole-block
//! DC intra prediction, the forward 4x4 DCT/WHT, quality-mapped
//! quantisation, and boolean arithmetic (range) coding.
//!
//! Every entropy-coded field is written as the exact dual of what the
//! in-crate decoder (`crate::vp8::dec`) reads — the boolean coder itself
//! (RFC 6386 §7.3), the frame header field order (§19.2), the prediction
//! mode trees (§11), and the DCT token trees, bands and contexts (§13). A
//! range coder has no resynchronisation point, so a single field written
//! with the wrong probability, in the wrong order, or omitted makes the
//! entire remainder of the frame unparseable rather than merely degraded.
//!
//! # Limitations
//!
//! - Key frames only (no inter prediction / P-frames)
//! - Whole-block DC prediction exclusively (no B_PRED, V, H or TM modes)
//! - Single DCT token partition
//! - No rate-distortion optimisation, and the loop filter is disabled
//!
//! # References
//!
//! - [RFC 6386: VP8 Data Format and Decoding Guide](https://tools.ietf.org/html/rfc6386)

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use crate::error::{CodecError, CodecResult};

// ---------------------------------------------------------------------------
// VP8 DCT-token probability tables (RFC 6386 §13.4, §13.5)
// ---------------------------------------------------------------------------
//
// Both tables are stored flat rather than as `[[[[u8; 11]; 3]; 8]; 4]`: the
// nested form costs ~2x the source lines for no gain, and every access goes
// through `coeff_prob_offset` anyway.

/// Byte offset of the 11 tree-node probabilities for one
/// `(block_type, coeff_band, prev_token_context)` triple.
///
/// `block_type`: 0 = luma after Y2, 1 = Y2 (WHT), 2 = chroma, 3 = luma
/// without Y2 (RFC 6386 §13.3).
const fn coeff_prob_offset(block_type: usize, band: usize, ctx: usize) -> usize {
    ((block_type * 8 + band) * 3 + ctx) * 11
}

/// Default DCT-token probabilities (RFC 6386 §13.5 `default_coeff_probs`,
/// rfc6386.txt lines 3509+), flattened to
/// `[block_type][coeff_band][prev_token_context][tree_node]` — index with
/// [`coeff_prob_offset`].
///
/// Transcribed from RFC 6386 and verified byte-for-byte against the
/// bit-exact in-crate decoder's `vp8::dec::tables::DEFAULT_COEFF_PROBS`.
static DEFAULT_COEFF_PROBS: [u8; 1056] = [
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 253, 136, 254, 255, 228,
    219, 128, 128, 128, 128, 128, 189, 129, 242, 255, 227, 213, 255, 219, 128, 128, 128, 106, 126,
    227, 252, 214, 209, 255, 255, 128, 128, 128, 1, 98, 248, 255, 236, 226, 255, 255, 128, 128,
    128, 181, 133, 238, 254, 221, 234, 255, 154, 128, 128, 128, 78, 134, 202, 247, 198, 180, 255,
    219, 128, 128, 128, 1, 185, 249, 255, 243, 255, 128, 128, 128, 128, 128, 184, 150, 247, 255,
    236, 224, 128, 128, 128, 128, 128, 77, 110, 216, 255, 236, 230, 128, 128, 128, 128, 128, 1,
    101, 251, 255, 241, 255, 128, 128, 128, 128, 128, 170, 139, 241, 252, 236, 209, 255, 255, 128,
    128, 128, 37, 116, 196, 243, 228, 255, 255, 255, 128, 128, 128, 1, 204, 254, 255, 245, 255,
    128, 128, 128, 128, 128, 207, 160, 250, 255, 238, 128, 128, 128, 128, 128, 128, 102, 103, 231,
    255, 211, 171, 128, 128, 128, 128, 128, 1, 152, 252, 255, 240, 255, 128, 128, 128, 128, 128,
    177, 135, 243, 255, 234, 225, 128, 128, 128, 128, 128, 80, 129, 211, 255, 194, 224, 128, 128,
    128, 128, 128, 1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128, 246, 1, 255, 128, 128, 128,
    128, 128, 128, 128, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 198, 35, 237,
    223, 193, 187, 162, 160, 145, 155, 62, 131, 45, 198, 221, 172, 176, 220, 157, 252, 221, 1, 68,
    47, 146, 208, 149, 167, 221, 162, 255, 223, 128, 1, 149, 241, 255, 221, 224, 255, 255, 128,
    128, 128, 184, 141, 234, 253, 222, 220, 255, 199, 128, 128, 128, 81, 99, 181, 242, 176, 190,
    249, 202, 255, 255, 128, 1, 129, 232, 253, 214, 197, 242, 196, 255, 255, 128, 99, 121, 210,
    250, 201, 198, 255, 202, 128, 128, 128, 23, 91, 163, 242, 170, 187, 247, 210, 255, 255, 128, 1,
    200, 246, 255, 234, 255, 128, 128, 128, 128, 128, 109, 178, 241, 255, 231, 245, 255, 255, 128,
    128, 128, 44, 130, 201, 253, 205, 192, 255, 255, 128, 128, 128, 1, 132, 239, 251, 219, 209,
    255, 165, 128, 128, 128, 94, 136, 225, 251, 218, 190, 255, 255, 128, 128, 128, 22, 100, 174,
    245, 186, 161, 255, 199, 128, 128, 128, 1, 182, 249, 255, 232, 235, 128, 128, 128, 128, 128,
    124, 143, 241, 255, 227, 234, 128, 128, 128, 128, 128, 35, 77, 181, 251, 193, 211, 255, 205,
    128, 128, 128, 1, 157, 247, 255, 236, 231, 255, 255, 128, 128, 128, 121, 141, 235, 255, 225,
    227, 255, 255, 128, 128, 128, 45, 99, 188, 251, 195, 217, 255, 224, 128, 128, 128, 1, 1, 251,
    255, 213, 255, 128, 128, 128, 128, 128, 203, 1, 248, 255, 255, 128, 128, 128, 128, 128, 128,
    137, 1, 177, 255, 224, 255, 128, 128, 128, 128, 128, 253, 9, 248, 251, 207, 208, 255, 192, 128,
    128, 128, 175, 13, 224, 243, 193, 185, 249, 198, 255, 255, 128, 73, 17, 171, 221, 161, 179,
    236, 167, 255, 234, 128, 1, 95, 247, 253, 212, 183, 255, 255, 128, 128, 128, 239, 90, 244, 250,
    211, 209, 255, 255, 128, 128, 128, 155, 77, 195, 248, 188, 195, 255, 255, 128, 128, 128, 1, 24,
    239, 251, 218, 219, 255, 205, 128, 128, 128, 201, 51, 219, 255, 196, 186, 128, 128, 128, 128,
    128, 69, 46, 190, 239, 201, 218, 255, 228, 128, 128, 128, 1, 191, 251, 255, 255, 128, 128, 128,
    128, 128, 128, 223, 165, 249, 255, 213, 255, 128, 128, 128, 128, 128, 141, 124, 248, 255, 255,
    128, 128, 128, 128, 128, 128, 1, 16, 248, 255, 255, 128, 128, 128, 128, 128, 128, 190, 36, 230,
    255, 236, 255, 128, 128, 128, 128, 128, 149, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128, 1,
    226, 255, 128, 128, 128, 128, 128, 128, 128, 128, 247, 192, 255, 128, 128, 128, 128, 128, 128,
    128, 128, 240, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128, 1, 134, 252, 255, 255, 128,
    128, 128, 128, 128, 128, 213, 62, 250, 255, 255, 128, 128, 128, 128, 128, 128, 55, 93, 255,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 202, 24, 213, 235, 186, 191, 220, 160, 240, 175, 255, 126, 38, 182, 232, 169,
    184, 228, 174, 255, 187, 128, 61, 46, 138, 219, 151, 178, 240, 170, 255, 216, 128, 1, 112, 230,
    250, 199, 191, 247, 159, 255, 255, 128, 166, 109, 228, 252, 211, 215, 255, 174, 128, 128, 128,
    39, 77, 162, 232, 172, 180, 245, 178, 255, 255, 128, 1, 52, 220, 246, 198, 199, 249, 220, 255,
    255, 128, 124, 74, 191, 243, 183, 193, 250, 221, 255, 255, 128, 24, 71, 130, 219, 154, 170,
    243, 182, 255, 255, 128, 1, 182, 225, 249, 219, 240, 255, 224, 128, 128, 128, 149, 150, 226,
    252, 216, 205, 255, 171, 128, 128, 128, 28, 108, 170, 242, 183, 194, 254, 223, 255, 255, 128,
    1, 81, 230, 252, 204, 203, 255, 192, 128, 128, 128, 123, 102, 209, 247, 188, 196, 255, 233,
    128, 128, 128, 20, 95, 153, 243, 164, 173, 255, 203, 128, 128, 128, 1, 222, 248, 255, 216, 213,
    128, 128, 128, 128, 128, 168, 175, 246, 252, 235, 205, 255, 255, 128, 128, 128, 47, 116, 215,
    255, 211, 212, 255, 255, 128, 128, 128, 1, 121, 236, 253, 212, 214, 255, 255, 128, 128, 128,
    141, 84, 213, 252, 201, 202, 255, 219, 128, 128, 128, 42, 80, 160, 240, 162, 185, 255, 205,
    128, 128, 128, 1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128, 244, 1, 255, 128, 128, 128,
    128, 128, 128, 128, 128, 238, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
];

/// Per-probability "is this probability updated?" gates (RFC 6386 §13.4
/// `coeff_update_probs`, rfc6386.txt line 3759), same flattening as
/// [`DEFAULT_COEFF_PROBS`].
///
/// The decoder reads all 1056 update flags with *these* probabilities, so an
/// encoder that signals "no update" must write them with these probabilities
/// too — writing them as plain 1/2-probability bits desynchronises the range
/// coder for the whole rest of the frame.
static COEFF_UPDATE_PROBS: [u8; 1056] = [
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 176, 246, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255, 249, 253,
    253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 244, 252, 255, 255, 255, 255, 255, 255, 255,
    255, 234, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 253, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255, 239, 253, 254, 255,
    255, 255, 255, 255, 255, 255, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    248, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251, 255, 254, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 253, 254, 255, 255, 255,
    255, 255, 255, 255, 255, 251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 254,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255,
    250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 217, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 225, 252, 241, 253, 255, 255, 254, 255, 255, 255,
    255, 234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 223, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 238, 253, 254, 254,
    255, 255, 255, 255, 255, 255, 255, 255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255, 249,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 247, 254, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 253, 254,
    255, 255, 255, 255, 255, 255, 255, 255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 254, 255, 255, 255, 255, 255,
    255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255, 250, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 186, 251, 250, 255,
    255, 255, 255, 255, 255, 255, 255, 234, 251, 244, 254, 255, 255, 255, 255, 255, 255, 255, 251,
    251, 243, 253, 254, 255, 254, 255, 255, 255, 255, 255, 253, 254, 255, 255, 255, 255, 255, 255,
    255, 255, 236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251, 253, 253, 254, 254, 255,
    255, 255, 255, 255, 255, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 254, 254, 254,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 254, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 248, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255, 248, 254, 249,
    253, 255, 255, 255, 255, 255, 255, 255, 255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255, 252, 254, 251, 254, 254, 255, 255, 255,
    255, 255, 255, 255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255, 248, 254, 253, 255, 255,
    255, 255, 255, 255, 255, 255, 253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 251,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 245, 251, 254, 255, 255, 255, 255, 255, 255, 255,
    255, 253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 251, 253, 255, 255, 255, 255,
    255, 255, 255, 255, 252, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 249,
    255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 250, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
];

/// VP8 DC quantizer lookup table (RFC 6386 Section 9.6).
///
/// Maps quantizer index (0..127) to the actual DC dequantization factor.
#[rustfmt::skip]
static DC_QUANT_TABLE: [i32; 128] = [
      4,   5,   6,   7,   8,   9,  10,  10,  11,  12,  13,  14,  15,  16,  17,  17,
     18,  19,  20,  20,  21,  21,  22,  22,  23,  23,  24,  25,  25,  26,  27,  28,
     29,  30,  31,  32,  33,  34,  35,  36,  37,  37,  38,  39,  40,  41,  42,  43,
     44,  45,  46,  46,  47,  48,  49,  50,  51,  52,  53,  54,  55,  56,  57,  58,
     59,  60,  61,  62,  63,  64,  65,  66,  67,  68,  69,  70,  71,  72,  73,  74,
     75,  76,  76,  77,  78,  79,  80,  81,  82,  83,  84,  85,  86,  87,  88,  89,
     91,  93,  95,  96,  98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118,
    122, 124, 126, 128, 130, 132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157,
];

/// VP8 AC quantizer lookup table (RFC 6386 Section 9.6).
///
/// Maps quantizer index (0..127) to the actual AC dequantization factor.
#[rustfmt::skip]
static AC_QUANT_TABLE: [i32; 128] = [
      4,   5,   6,   7,   8,   9,  10,  11,  12,  13,  14,  15,  16,  17,  18,  19,
     20,  21,  22,  23,  24,  25,  26,  27,  28,  29,  30,  31,  32,  33,  34,  35,
     36,  37,  38,  39,  40,  41,  42,  43,  44,  45,  46,  47,  48,  49,  50,  51,
     52,  53,  54,  55,  56,  57,  58,  60,  62,  64,  66,  68,  70,  72,  74,  76,
     78,  80,  82,  84,  86,  88,  90,  92,  94,  96,  98, 100, 102, 104, 106, 108,
    110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140, 143, 146, 149, 152,
    155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205, 209,
    213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284,
];

/// VP8 zigzag scan order for 4x4 blocks.
static ZIGZAG_ORDER: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Maps a coefficient's zigzag position to a frequency band (0..7).
///
/// VP8 groups coefficient positions into 8 bands for probability context.
static COEFF_BANDS: [usize; 16] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7];

// ---------------------------------------------------------------------------
// Boolean arithmetic encoder (VP8 range coder)
// ---------------------------------------------------------------------------

/// Boolean arithmetic encoder for VP8 bitstream writing.
///
/// Exact dual of the in-crate `vp8::dec::bool_decoder::BoolDecoder`: a
/// transcription of the reference `write_bool` / `flush_bool_encoder` of
/// RFC 6386 §7.3 (rfc6386.txt lines 1141-1228).
///
/// The renormalisation loop below must stay byte-for-byte faithful to the
/// RFC. Two properties in particular are load-bearing and were previously
/// absent, which made every frame this module produced undecodable:
///
/// 1. **Byte emission timing.** A byte leaves the encoder only when
///    `bit_count` counts down to exactly zero *inside* the shift loop; the
///    retained value is then masked to 24 bits and `bit_count` reset to 8,
///    with no extra shift. Batching the shifts and emitting afterwards (the
///    previous implementation) advances `bottom` eight bits too far after
///    the first byte, so the emitted stream is the correct one with a byte
///    deleted — the decoder then primes its value register from the wrong
///    bytes and mis-parses the very first header fields.
/// 2. **Carry propagation.** `bottom += split` can carry out of bit 31 and
///    that carry belongs to bytes *already written*; `add_one_to_output`
///    propagates it backwards through any trailing `0xFF`s. Widening
///    `bottom` to `u64` (as the previous implementation did) does not help:
///    the carry still has to reach the emitted bytes, and simply dropping it
///    corrupts the stream.
struct BoolEncoder {
    /// Bytes emitted so far.
    output: Vec<u8>,
    /// Current coding range, kept in `[128, 255]` after normalisation.
    range: u32,
    /// Minimum value of the remaining output (RFC 6386 `bottom`).
    bottom: u32,
    /// Number of shifts before the next output byte is available.
    bit_count: i32,
}

impl BoolEncoder {
    /// Creates a new boolean encoder with an empty output buffer.
    fn new() -> Self {
        Self {
            output: Vec::new(),
            range: 255,
            bottom: 0,
            bit_count: 24,
        }
    }

    /// RFC 6386 §7.3 `add_one_to_output`: propagates a carry backwards
    /// through any already-written trailing `0xFF` bytes.
    fn add_one_to_output(&mut self) {
        for byte in self.output.iter_mut().rev() {
            if *byte == 0xFF {
                *byte = 0;
            } else {
                *byte += 1;
                return;
            }
        }
        // RFC 6386 lines 1149-1153: the arithmetic guarantees the
        // propagation never runs past the start of the partition, because
        // those leading bits are the value's own high end.
    }

    /// Encodes a single boolean symbol with the given probability.
    ///
    /// `prob` is the probability that the symbol is **false** (0),
    /// in the range 1..=255. RFC 6386 §7.3 `write_bool`.
    fn encode_bool(&mut self, value: bool, prob: u8) {
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);

        if value {
            // Wrapping matches the C `uint32`'s mod-2^32 semantics; the
            // carry-out it produces is exactly what the bit-31 test below
            // hands to `add_one_to_output`.
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }

        while self.range < 128 {
            self.range <<= 1;
            if self.bottom & (1u32 << 31) != 0 {
                self.add_one_to_output();
            }
            self.bottom <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.output.push((self.bottom >> 24) as u8);
                self.bottom &= (1u32 << 24) - 1;
                self.bit_count = 8;
            }
        }
    }

    /// Encodes a boolean with 50% probability (uniform bit).
    fn encode_bit(&mut self, value: bool) {
        self.encode_bool(value, 128);
    }

    /// Encodes an unsigned integer of `n` bits, MSB first (RFC 6386 `L(n)`).
    fn encode_literal(&mut self, value: u32, n: u8) {
        for i in (0..n).rev() {
            let bit = (value >> i) & 1 != 0;
            self.encode_bit(bit);
        }
    }

    /// Finalizes the encoder and returns the encoded byte stream.
    ///
    /// RFC 6386 §7.3 `flush_bool_encoder` (rfc6386.txt lines 1212-1228):
    /// propagate a final carry, shift the residual value up to the top of
    /// the register, then write four padding bytes.
    fn flush(mut self) -> Vec<u8> {
        let c0 = self.bit_count; // always in 1..=24, see `encode_bool`
        let v0 = self.bottom;
        if v0 & (1u32 << ((32 - c0) as u32)) != 0 {
            self.add_one_to_output();
        }
        let mut v = v0 << ((c0 & 7) as u32);
        let mut c = c0 >> 3;
        while c > 0 {
            v <<= 8;
            c -= 1;
        }
        for _ in 0..4 {
            self.output.push((v >> 24) as u8);
            v <<= 8;
        }
        self.output
    }
}

/// Writes the boolean path a `read_tree(tree, probs)` walk would take to
/// return `value`, entering the tree at node `start`.
///
/// Exact dual of the decoder's `BoolDecoder::read_tree_from` (RFC 6386 §8
/// `treed_read`): a depth-first search for the leaf `-value`, recording the
/// branch taken at each internal node, then emitting those branches
/// root-first. Deriving the path from the tree table itself — rather than
/// hand-unrolling one `encode_bool` chain per symbol — is what makes the two
/// sides provably agree.
fn write_tree(enc: &mut BoolEncoder, tree: &[i8], probs: &[u8], value: i32, start: usize) {
    /// Depth-first search recording `(probability index, branch)` pairs into
    /// `path`, returning the path length. Bounded and allocation-free: this
    /// runs once per coefficient token, so a heap allocation per call would
    /// be the encoder's hottest cost.
    fn find(
        tree: &[i8],
        value: i32,
        node: usize,
        path: &mut [(usize, bool); MAX_TREE_DEPTH],
        depth: usize,
    ) -> Option<usize> {
        if depth >= MAX_TREE_DEPTH {
            return None;
        }
        for branch in 0..2usize {
            let next = tree[node + branch];
            path[depth] = (node >> 1, branch == 1);
            if next <= 0 {
                if i32::from(-next) == value {
                    return Some(depth + 1);
                }
            } else if let Some(len) = find(tree, value, next as usize, path, depth + 1) {
                return Some(len);
            }
        }
        None
    }

    let mut path = [(0usize, false); MAX_TREE_DEPTH];
    if let Some(len) = find(tree, value, start, &mut path, 0) {
        for &(prob_index, branch) in &path[..len] {
            enc.encode_bool(branch, probs[prob_index]);
        }
    }
}

/// Deepest path any tree written here has: RFC 6386's `coeff_tree` reaches
/// its category-5 and category-6 leaves in seven decisions, and the
/// prediction-mode trees are shallower.
const MAX_TREE_DEPTH: usize = 8;

// ---------------------------------------------------------------------------
// Forward and inverse 4x4 transforms
// ---------------------------------------------------------------------------
//
// RFC 6386 normatively specifies only the *inverse* transforms (§14.3 WHT,
// §14.4 DCT); the forward transforms below are their exact duals, i.e. the
// unique integer approximations that the specified inverse undoes.
//
// Writing them as duals is not cosmetic. The inverse DCT of §14.4 applies
// `x1 * sqrt(2)cos(pi/8) + x3 * sqrt(2)sin(pi/8)`, so the forward transform
// must pair the *transposed* multipliers: 2217/4096 with the `x1 - x2`
// butterfly and 5352/4096 with `x0 - x3`. The previous implementation had
// these two constants exchanged, which is a different (non-invertible-by-
// §14.4) transform, and its Walsh-Hadamard pass omitted the factor-of-two
// normalisation the §14.3 inverse expects — so even a perfectly assembled
// bitstream would have decoded to the wrong picture.

/// Fixed-point `sqrt(2) * cos(pi/8)`, scaled by 2^12 (dual of §14.4's 20091
/// at 2^16).
const FDCT_COS_PI8_SQRT2: i32 = 5352;
/// Fixed-point `sqrt(2) * sin(pi/8)`, scaled by 2^12 (dual of §14.4's 35468).
const FDCT_SIN_PI8_SQRT2: i32 = 2217;
/// Fixed-point `sqrt(2) * cos(pi/8) - 1`, scaled by 2^16 (RFC 6386 §14.4).
const COS_PI8_SQRT2_MINUS1: i64 = 20091;
/// Fixed-point `sqrt(2) * sin(pi/8)`, scaled by 2^16 (RFC 6386 §14.4).
const SIN_PI8_SQRT2: i64 = 35468;

/// Performs the forward 4x4 DCT, the dual of [`idct4x4`].
///
/// Takes 16 residual values in raster order and produces 16 DCT
/// coefficients in raster order. Rows are scaled up by 8 so the odd
/// (rotation) outputs keep their precision through the `>> 12` fixed-point
/// multiply; the column pass takes that factor back out again, leaving the
/// overall `1/2` normalisation §14.4's `+4 >> 3` inverse expects.
fn fdct4x4(residual: &[i32; 16], coeffs: &mut [i32; 16]) {
    let mut tmp = [0i32; 16];

    // Row pass.
    for row in 0..4 {
        let base = row * 4;
        let a1 = (residual[base] + residual[base + 3]) * 8;
        let b1 = (residual[base + 1] + residual[base + 2]) * 8;
        let c1 = (residual[base + 1] - residual[base + 2]) * 8;
        let d1 = (residual[base] - residual[base + 3]) * 8;

        tmp[base] = a1 + b1;
        tmp[base + 2] = a1 - b1;
        tmp[base + 1] = (c1 * FDCT_SIN_PI8_SQRT2 + d1 * FDCT_COS_PI8_SQRT2 + 14500) >> 12;
        tmp[base + 3] = (d1 * FDCT_SIN_PI8_SQRT2 - c1 * FDCT_COS_PI8_SQRT2 + 7500) >> 12;
    }

    // Column pass.
    for col in 0..4 {
        let a1 = tmp[col] + tmp[col + 12];
        let b1 = tmp[col + 4] + tmp[col + 8];
        let c1 = tmp[col + 4] - tmp[col + 8];
        let d1 = tmp[col] - tmp[col + 12];

        coeffs[col] = (a1 + b1 + 7) >> 4;
        coeffs[col + 8] = (a1 - b1 + 7) >> 4;
        coeffs[col + 4] = ((c1 * FDCT_SIN_PI8_SQRT2 + d1 * FDCT_COS_PI8_SQRT2 + 12000) >> 16)
            + i32::from(d1 != 0);
        coeffs[col + 12] = (d1 * FDCT_SIN_PI8_SQRT2 - c1 * FDCT_COS_PI8_SQRT2 + 12000) >> 16;
    }
}

/// Performs the forward 4x4 Walsh-Hadamard transform, the dual of
/// [`iwht4x4`].
///
/// Takes the 16 luma DC coefficients of a macroblock's sub-blocks and
/// produces the Y2 block. The Hadamard butterfly is self-inverse up to a
/// factor of 16, and §14.3's inverse already divides by 8, so this pass
/// carries the remaining factor of `1/2` (expressed as `* 4` then
/// `+3 >> 3`, matching the inverse's rounding).
fn fwht4x4(dc_values: &[i32; 16], coeffs: &mut [i32; 16]) {
    let mut tmp = [0i32; 16];

    // Row pass.
    for row in 0..4 {
        let base = row * 4;
        let a1 = (dc_values[base] + dc_values[base + 3]) * 4;
        let b1 = (dc_values[base + 1] + dc_values[base + 2]) * 4;
        let c1 = (dc_values[base + 1] - dc_values[base + 2]) * 4;
        let d1 = (dc_values[base] - dc_values[base + 3]) * 4;

        tmp[base] = a1 + b1;
        tmp[base + 1] = d1 + c1;
        tmp[base + 2] = a1 - b1;
        tmp[base + 3] = d1 - c1;
    }

    // Column pass, with the inverse's `+3 >> 3` rounding.
    for col in 0..4 {
        let a1 = tmp[col] + tmp[col + 12];
        let b1 = tmp[col + 4] + tmp[col + 8];
        let c1 = tmp[col + 4] - tmp[col + 8];
        let d1 = tmp[col] - tmp[col + 12];

        coeffs[col] = (a1 + b1 + 3) >> 3;
        coeffs[col + 4] = (d1 + c1 + 3) >> 3;
        coeffs[col + 8] = (a1 - b1 + 3) >> 3;
        coeffs[col + 12] = (d1 - c1 + 3) >> 3;
    }
}

/// Inverse 4x4 DCT, transcribed from RFC 6386 §14.4 (`idct4x4llm`).
///
/// The encoder needs the *decoder's* exact inverse, not an approximation of
/// it: intra prediction is fed from the reconstruction, so any difference
/// here is a prediction mismatch that accumulates across macroblocks.
fn idct4x4(coeffs: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    let mut out = [0i32; 16];

    // Vertical pass.
    for i in 0..4 {
        let (c0, c1, c2, c3) = (coeffs[i], coeffs[i + 4], coeffs[i + 8], coeffs[i + 12]);
        let a1 = c0 + c2;
        let b1 = c0 - c2;

        let t1 = (i64::from(c1) * SIN_PI8_SQRT2) >> 16;
        let t2 = i64::from(c3) + ((i64::from(c3) * COS_PI8_SQRT2_MINUS1) >> 16);
        let c1_t = (t1 - t2) as i32;

        let t1 = i64::from(c1) + ((i64::from(c1) * COS_PI8_SQRT2_MINUS1) >> 16);
        let t2 = (i64::from(c3) * SIN_PI8_SQRT2) >> 16;
        let d1 = (t1 + t2) as i32;

        tmp[i] = a1 + d1;
        tmp[i + 12] = a1 - d1;
        tmp[i + 4] = b1 + c1_t;
        tmp[i + 8] = b1 - c1_t;
    }

    // Horizontal pass, with the specified `+4 >> 3` rounding.
    for i in 0..4 {
        let base = i * 4;
        let (c0, c1, c2, c3) = (tmp[base], tmp[base + 1], tmp[base + 2], tmp[base + 3]);
        let a1 = c0 + c2;
        let b1 = c0 - c2;

        let t1 = (i64::from(c1) * SIN_PI8_SQRT2) >> 16;
        let t2 = i64::from(c3) + ((i64::from(c3) * COS_PI8_SQRT2_MINUS1) >> 16);
        let c1_t = (t1 - t2) as i32;

        let t1 = i64::from(c1) + ((i64::from(c1) * COS_PI8_SQRT2_MINUS1) >> 16);
        let t2 = (i64::from(c3) * SIN_PI8_SQRT2) >> 16;
        let d1 = (t1 + t2) as i32;

        out[base] = (a1 + d1 + 4) >> 3;
        out[base + 3] = (a1 - d1 + 4) >> 3;
        out[base + 1] = (b1 + c1_t + 4) >> 3;
        out[base + 2] = (b1 - c1_t + 4) >> 3;
    }

    out
}

/// Inverse 4x4 Walsh-Hadamard transform, transcribed from RFC 6386 §14.3
/// (`iwalsh4x4`). Recovers the 16 luma DC coefficients from a Y2 block.
fn iwht4x4(coeffs: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    let mut out = [0i32; 16];

    // Vertical pass.
    for i in 0..4 {
        let a1 = coeffs[i] + coeffs[i + 12];
        let b1 = coeffs[i + 4] + coeffs[i + 8];
        let c1 = coeffs[i + 4] - coeffs[i + 8];
        let d1 = coeffs[i] - coeffs[i + 12];

        tmp[i] = a1 + b1;
        tmp[i + 4] = d1 + c1;
        tmp[i + 8] = a1 - b1;
        tmp[i + 12] = d1 - c1;
    }

    // Horizontal pass, with `+3 >> 3` rounding.
    for i in 0..4 {
        let base = i * 4;
        let a1 = tmp[base] + tmp[base + 3];
        let b1 = tmp[base + 1] + tmp[base + 2];
        let c1 = tmp[base + 1] - tmp[base + 2];
        let d1 = tmp[base] - tmp[base + 3];

        out[base] = (a1 + b1 + 3) >> 3;
        out[base + 1] = (d1 + c1 + 3) >> 3;
        out[base + 2] = (a1 - b1 + 3) >> 3;
        out[base + 3] = (d1 - c1 + 3) >> 3;
    }

    out
}

// ---------------------------------------------------------------------------
// YUV plane representation
// ---------------------------------------------------------------------------

/// YUV 4:2:0 image planes.
struct YuvPlanes {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    y_stride: usize,
    uv_stride: usize,
    width: u32,
    height: u32,
}

/// Converts RGB data to YUV 4:2:0 using BT.601 coefficients.
///
/// The RGB buffer must contain `width * height * 3` bytes in row-major
/// R-G-B order.  The output planes are padded so that the luma plane
/// width/height are multiples of 16 (macroblock alignment).
fn rgb_to_yuv420(data: &[u8], width: u32, height: u32) -> CodecResult<YuvPlanes> {
    let w = width as usize;
    let h = height as usize;

    if data.len() < w * h * 3 {
        return Err(CodecError::InvalidParameter(format!(
            "RGB data too short: need {}, have {}",
            w * h * 3,
            data.len()
        )));
    }

    // Pad to macroblock boundaries
    let mb_w = ((w + 15) / 16) * 16;
    let mb_h = ((h + 15) / 16) * 16;

    let y_stride = mb_w;
    let uv_stride = mb_w / 2;

    let mut y_plane = vec![0u8; y_stride * mb_h];
    let mut u_plane = vec![128u8; uv_stride * (mb_h / 2)];
    let mut v_plane = vec![128u8; uv_stride * (mb_h / 2)];

    // Convert pixel by pixel
    for row in 0..h {
        for col in 0..w {
            let idx = (row * w + col) * 3;
            let r = f64::from(data[idx]);
            let g = f64::from(data[idx + 1]);
            let b = f64::from(data[idx + 2]);

            let y_val = 0.299 * r + 0.587 * g + 0.114 * b;
            y_plane[row * y_stride + col] = y_val.clamp(0.0, 255.0) as u8;
        }
    }

    // Chroma subsampling: average 2x2 blocks
    let ch_w = (w + 1) / 2;
    let ch_h = (h + 1) / 2;

    for row in 0..ch_h {
        for col in 0..ch_w {
            let mut sum_u = 0.0f64;
            let mut sum_v = 0.0f64;
            let mut count = 0.0f64;

            for dy in 0..2 {
                for dx in 0..2 {
                    let sy = row * 2 + dy;
                    let sx = col * 2 + dx;
                    if sy < h && sx < w {
                        let idx = (sy * w + sx) * 3;
                        let r = f64::from(data[idx]);
                        let g = f64::from(data[idx + 1]);
                        let b = f64::from(data[idx + 2]);

                        sum_u += -0.169 * r - 0.331 * g + 0.500 * b + 128.0;
                        sum_v += 0.500 * r - 0.419 * g - 0.081 * b + 128.0;
                        count += 1.0;
                    }
                }
            }

            let u_val = (sum_u / count).clamp(0.0, 255.0) as u8;
            let v_val = (sum_v / count).clamp(0.0, 255.0) as u8;

            u_plane[row * uv_stride + col] = u_val;
            v_plane[row * uv_stride + col] = v_val;
        }
    }

    // Pad remaining pixels by replicating edges
    for row in 0..h {
        for col in w..mb_w {
            y_plane[row * y_stride + col] = y_plane[row * y_stride + w.saturating_sub(1)];
        }
    }
    for row in h..mb_h {
        let src_row = h.saturating_sub(1);
        for col in 0..mb_w {
            y_plane[row * y_stride + col] = y_plane[src_row * y_stride + col.min(mb_w - 1)];
        }
    }
    for row in 0..ch_h {
        for col in ch_w..(mb_w / 2) {
            u_plane[row * uv_stride + col] = u_plane[row * uv_stride + ch_w.saturating_sub(1)];
            v_plane[row * uv_stride + col] = v_plane[row * uv_stride + ch_w.saturating_sub(1)];
        }
    }
    for row in ch_h..(mb_h / 2) {
        let src_row = ch_h.saturating_sub(1);
        for col in 0..(mb_w / 2) {
            u_plane[row * uv_stride + col] = u_plane[src_row * uv_stride + col];
            v_plane[row * uv_stride + col] = v_plane[src_row * uv_stride + col];
        }
    }

    Ok(YuvPlanes {
        y: y_plane,
        u: u_plane,
        v: v_plane,
        y_stride,
        uv_stride,
        width,
        height,
    })
}

// ---------------------------------------------------------------------------
// DCT token coding (RFC 6386 §13)
// ---------------------------------------------------------------------------

// DCT token values (RFC 6386 §13.2 `dct_token`): literal magnitudes 0-4,
// then six categories whose extra bits widen the magnitude (5..=6, 7..=10,
// 11..=18, 19..=34, 35..=66, 67..=2048), then end-of-block.
const DCT_0: i32 = 0;
const DCT_1: i32 = 1;
const DCT_2: i32 = 2;
const DCT_3: i32 = 3;
const DCT_4: i32 = 4;
const DCT_CAT1: i32 = 5;
const DCT_CAT2: i32 = 6;
const DCT_CAT3: i32 = 7;
const DCT_CAT4: i32 = 8;
const DCT_CAT5: i32 = 9;
const DCT_CAT6: i32 = 10;
const DCT_EOB: i32 = 11;

/// DCT coefficient token tree (RFC 6386 §13.2 `coeff_tree`).
///
/// Note that end-of-block hangs off the *root* only: once a literal zero has
/// been coded the tree is re-entered at node 2, which is why
/// [`encode_block`] tracks `skip_eob`.
#[rustfmt::skip]
static COEFF_TREE: [i8; 22] = [
    -(DCT_EOB as i8), 2,
    -(DCT_0 as i8), 4,
    -(DCT_1 as i8), 6,
    8, 12,
    -(DCT_2 as i8), 10,
    -(DCT_3 as i8), -(DCT_4 as i8),
    14, 16,
    -(DCT_CAT1 as i8), -(DCT_CAT2 as i8),
    18, 20,
    -(DCT_CAT3 as i8), -(DCT_CAT4 as i8),
    -(DCT_CAT5 as i8), -(DCT_CAT6 as i8),
];

// Extra-bit probabilities per DCT category (RFC 6386 §13.2 `Pcat1`..`Pcat6`).
static CAT1_PROB: [u8; 1] = [159];
static CAT2_PROB: [u8; 2] = [165, 145];
static CAT3_PROB: [u8; 3] = [173, 148, 140];
static CAT4_PROB: [u8; 4] = [176, 155, 140, 135];
static CAT5_PROB: [u8; 5] = [180, 157, 141, 134, 130];
static CAT6_PROB: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

/// Extra-bit probabilities indexed by category (0-based).
static CAT_PROBS: [&[u8]; 6] = [
    &CAT1_PROB, &CAT2_PROB, &CAT3_PROB, &CAT4_PROB, &CAT5_PROB, &CAT6_PROB,
];

/// Smallest magnitude each category token can represent (RFC 6386 §13.2).
static CAT_BASE: [i32; 6] = [5, 7, 11, 19, 35, 67];

/// Maps a coefficient magnitude to its token and, for category tokens, the
/// 0-based category index whose extra bits must follow (RFC 6386 §13.2).
fn token_for(abs_value: i32) -> (i32, Option<usize>) {
    match abs_value {
        0 => (DCT_0, None),
        1 => (DCT_1, None),
        2 => (DCT_2, None),
        3 => (DCT_3, None),
        4 => (DCT_4, None),
        5..=6 => (DCT_CAT1, Some(0)),
        7..=10 => (DCT_CAT2, Some(1)),
        11..=18 => (DCT_CAT3, Some(2)),
        19..=34 => (DCT_CAT4, Some(3)),
        35..=66 => (DCT_CAT5, Some(4)),
        _ => (DCT_CAT6, Some(5)),
    }
}

/// Encodes one 4x4 sub-block of quantised coefficients (RFC 6386 §13).
///
/// Exact dual of the in-crate decoder's `vp8::dec::residual::decode_block`:
///
/// - `ctx` is the *initial* token context, `above_nz + left_nz` in `0..=2`
///   (RFC 6386 §13.3). The previous implementation always started from 0,
///   which desynchronises the range coder the moment any neighbouring block
///   carries a token.
/// - `first_coeff` is 1 for luma sub-blocks whose DC lives in the Y2 block,
///   else 0.
/// - After a literal zero the token tree is re-entered past the
///   end-of-block branch (`skip_eob`).
///
/// Returns the block's non-zero flag *as the decoder defines it*: "the
/// end-of-block position exceeds `first_coeff`", i.e. at least one token was
/// coded. That is subtly different from "some coefficient is non-zero" — a
/// block coded as literal zeros followed by EOB has the flag set — and it is
/// this value, not the coefficients, that feeds neighbouring blocks'
/// entropy contexts.
fn encode_block(
    enc: &mut BoolEncoder,
    quantized: &[i32; 16],
    block_type: usize,
    ctx: usize,
    first_coeff: usize,
) -> bool {
    // End-of-block position: one past the last coefficient that carries a
    // token. Trailing zeros are never coded, so this is one past the last
    // non-zero coefficient in zig-zag order.
    let mut eob = first_coeff;
    for i in (first_coeff..16).rev() {
        if quantized[ZIGZAG_ORDER[i]] != 0 {
            eob = i + 1;
            break;
        }
    }

    let mut prev_ctx = ctx;
    let mut skip_eob = false;
    let mut i = first_coeff;
    while i < 16 {
        let offset = coeff_prob_offset(block_type, COEFF_BANDS[i], prev_ctx);
        let probs = &DEFAULT_COEFF_PROBS[offset..offset + 11];
        let start = if skip_eob { 2 } else { 0 };

        if i == eob {
            // Reachable only with `skip_eob == false`: the token at `eob - 1`
            // is non-zero by construction, and the all-zero case stops at
            // `i == first_coeff` before any token is written.
            write_tree(enc, &COEFF_TREE, probs, DCT_EOB, start);
            break;
        }

        let coeff = quantized[ZIGZAG_ORDER[i]];
        let abs_value = coeff.abs();
        let (token, category) = token_for(abs_value);
        write_tree(enc, &COEFF_TREE, probs, token, start);

        if let Some(cat) = category {
            // Category extra bits, most significant first.
            let cat_probs = CAT_PROBS[cat];
            let extra = abs_value - CAT_BASE[cat];
            for (bit_index, &prob) in cat_probs.iter().enumerate() {
                let shift = cat_probs.len() - 1 - bit_index;
                enc.encode_bool((extra >> shift) & 1 != 0, prob);
            }
        }

        if abs_value == 0 {
            prev_ctx = 0;
            skip_eob = true;
        } else {
            enc.encode_bit(coeff < 0);
            prev_ctx = if abs_value == 1 { 1 } else { 2 };
            skip_eob = false;
        }
        i += 1;
    }

    eob > first_coeff
}

// ---------------------------------------------------------------------------
// Macroblock encoding
// ---------------------------------------------------------------------------

/// Quantizes a coefficient with the given quantizer step.
fn quantize(coeff: i32, step: i32) -> i32 {
    if step == 0 {
        return coeff;
    }
    let sign = if coeff < 0 { -1 } else { 1 };
    let abs_c = coeff.abs();
    sign * ((abs_c + step / 2) / step)
}

/// Processes a single macroblock: prediction, DCT, quantization.
///
/// Returns the quantized coefficients for all sub-blocks:
///   - 16 luma 4x4 blocks (Y)
///   - 1 DC block (Y2, the WHT of DC values)
///   - 4 U chroma blocks
///   - 4 V chroma blocks
struct MacroblockCoeffs {
    /// 16 luma sub-blocks, each 16 coefficients in raster order.
    y_blocks: [[i32; 16]; 16],
    /// Y2 (WHT of luma DC values), 16 coefficients.
    y2_block: [i32; 16],
    /// 4 U chroma sub-blocks, each 16 coefficients.
    u_blocks: [[i32; 16]; 4],
    /// 4 V chroma sub-blocks, each 16 coefficients.
    v_blocks: [[i32; 16]; 4],
}

/// Encodes a single 16x16 macroblock to produce quantized coefficients.
fn encode_macroblock(
    yuv: &YuvPlanes,
    mb_x: usize,
    mb_y: usize,
    dc_quant: i32,
    ac_quant: i32,
    y2_dc_quant: i32,
    y2_ac_quant: i32,
    uv_dc_quant: i32,
    uv_ac_quant: i32,
    reconstructed_y: &[u8],
    recon_y_stride: usize,
    reconstructed_u: &[u8],
    recon_uv_stride: usize,
    reconstructed_v: &[u8],
) -> MacroblockCoeffs {
    let mut mb = MacroblockCoeffs {
        y_blocks: [[0i32; 16]; 16],
        y2_block: [0i32; 16],
        u_blocks: [[0i32; 16]; 4],
        v_blocks: [[0i32; 16]; 4],
    };

    // --- DC prediction for luma 16x16 ---
    let pred_y = compute_dc_pred(reconstructed_y, recon_y_stride, mb_x, mb_y, 16);

    // Process 16 luma 4x4 sub-blocks
    let mut dc_values = [0i32; 16];

    for sb in 0..16 {
        let sb_row = sb / 4;
        let sb_col = sb % 4;

        let mut residual = [0i32; 16];
        for r in 0..4 {
            for c in 0..4 {
                let py = mb_y * 16 + sb_row * 4 + r;
                let px = mb_x * 16 + sb_col * 4 + c;
                let orig = i32::from(yuv.y[py * yuv.y_stride + px]);
                let pred = i32::from(pred_y);
                residual[r * 4 + c] = orig - pred;
            }
        }

        let mut coeffs = [0i32; 16];
        fdct4x4(&residual, &mut coeffs);

        // Save DC for Y2 block
        dc_values[sb] = coeffs[0];

        // Quantize AC coefficients (DC will be replaced by Y2)
        for i in 1..16 {
            coeffs[i] = quantize(coeffs[i], ac_quant);
        }
        // DC is set to 0 here; it goes through Y2
        coeffs[0] = 0;

        mb.y_blocks[sb] = coeffs;
    }

    // Y2 block: WHT of DC values
    let mut y2_coeffs = [0i32; 16];
    fwht4x4(&dc_values, &mut y2_coeffs);

    // Quantize Y2
    mb.y2_block[0] = quantize(y2_coeffs[0], y2_dc_quant);
    for i in 1..16 {
        mb.y2_block[i] = quantize(y2_coeffs[i], y2_ac_quant);
    }

    // --- Chroma ---
    let pred_u = compute_dc_pred(reconstructed_u, recon_uv_stride, mb_x, mb_y, 8);
    let pred_v = compute_dc_pred(reconstructed_v, recon_uv_stride, mb_x, mb_y, 8);

    for (plane, pred, out) in [
        (&yuv.u, pred_u, &mut mb.u_blocks),
        (&yuv.v, pred_v, &mut mb.v_blocks),
    ] {
        for sb in 0..4 {
            let sb_row = sb / 2;
            let sb_col = sb % 2;

            let mut residual = [0i32; 16];
            for r in 0..4 {
                for c in 0..4 {
                    let py = mb_y * 8 + sb_row * 4 + r;
                    let px = mb_x * 8 + sb_col * 4 + c;
                    let orig = i32::from(plane[py * yuv.uv_stride + px]);
                    residual[r * 4 + c] = orig - i32::from(pred);
                }
            }
            let mut coeffs = [0i32; 16];
            fdct4x4(&residual, &mut coeffs);
            coeffs[0] = quantize(coeffs[0], uv_dc_quant);
            for i in 1..16 {
                coeffs[i] = quantize(coeffs[i], uv_ac_quant);
            }
            out[sb] = coeffs;
        }
    }

    mb
}

/// Computes the DC prediction for one `size` x `size` block (RFC 6386
/// §12.2): the rounded average of the reconstructed row above and column to
/// the left, whichever are available, and 128 when neither is.
///
/// `size` is 16 for luma and 8 for chroma. Rounding falls out of
/// `(sum + count/2) / count`, which for the three reachable counts is
/// exactly the specification's `(sum + 16) >> 5`, `(sum + 8) >> 4` and
/// `(sum + 4) >> 3`.
fn compute_dc_pred(recon: &[u8], stride: usize, mb_x: usize, mb_y: usize, size: usize) -> u8 {
    let mut sum: u32 = 0;
    let mut count: u32 = 0;

    // Row above.
    if mb_y > 0 {
        let top_row = (mb_y * size - 1) * stride + mb_x * size;
        for col in 0..size {
            if top_row + col < recon.len() {
                sum += u32::from(recon[top_row + col]);
                count += 1;
            }
        }
    }

    // Column to the left.
    if mb_x > 0 {
        let left_col = mb_x * size - 1;
        for row in 0..size {
            let idx = (mb_y * size + row) * stride + left_col;
            if idx < recon.len() {
                sum += u32::from(recon[idx]);
                count += 1;
            }
        }
    }

    (sum + count / 2).checked_div(count).unwrap_or(128) as u8
}

/// Reconstructs a macroblock from its quantized coefficients for use as
/// reference in subsequent macroblock predictions.
fn reconstruct_macroblock(
    mb: &MacroblockCoeffs,
    dc_quant: i32,
    ac_quant: i32,
    y2_dc_quant: i32,
    y2_ac_quant: i32,
    uv_dc_quant: i32,
    uv_ac_quant: i32,
    pred_y: u8,
    pred_u: u8,
    pred_v: u8,
    recon_y: &mut [u8],
    recon_y_stride: usize,
    mb_x: usize,
    mb_y: usize,
    recon_u: &mut [u8],
    recon_uv_stride: usize,
    recon_v: &mut [u8],
) {
    // Inverse Y2 (WHT) to recover each luma sub-block's DC coefficient.
    // The dequantised Y2 block goes through the *decoder's* §14.3 inverse so
    // the encoder predicts from exactly what a decoder will see.
    let mut y2_dequant = [0i32; 16];
    y2_dequant[0] = mb.y2_block[0] * y2_dc_quant;
    for i in 1..16 {
        y2_dequant[i] = mb.y2_block[i] * y2_ac_quant;
    }
    let dc_values = iwht4x4(&y2_dequant);

    // Reconstruct each luma 4x4 sub-block
    for sb in 0..16 {
        let sb_row = sb / 4;
        let sb_col = sb % 4;

        // Dequantize AC
        let mut dequant = [0i32; 16];
        dequant[0] = dc_values[sb]; // DC from Y2
        for i in 1..16 {
            dequant[i] = mb.y_blocks[sb][i] * ac_quant;
        }

        // Inverse DCT
        let reconstructed = idct4x4(&dequant);

        // Add prediction and clamp
        for r in 0..4 {
            for c in 0..4 {
                let py = mb_y * 16 + sb_row * 4 + r;
                let px = mb_x * 16 + sb_col * 4 + c;
                let val = reconstructed[r * 4 + c] + i32::from(pred_y);
                recon_y[py * recon_y_stride + px] = val.clamp(0, 255) as u8;
            }
        }
    }

    // Reconstruct chroma (U then V — identical apart from the plane).
    for (blocks, pred, plane) in [
        (&mb.u_blocks, pred_u, &mut *recon_u),
        (&mb.v_blocks, pred_v, &mut *recon_v),
    ] {
        for sb in 0..4 {
            let sb_row = sb / 2;
            let sb_col = sb % 2;

            let mut dequant = [0i32; 16];
            dequant[0] = blocks[sb][0] * uv_dc_quant;
            for i in 1..16 {
                dequant[i] = blocks[sb][i] * uv_ac_quant;
            }
            let recon = idct4x4(&dequant);
            for r in 0..4 {
                for c in 0..4 {
                    let py = mb_y * 8 + sb_row * 4 + r;
                    let px = mb_x * 8 + sb_col * 4 + c;
                    let val = recon[r * 4 + c] + i32::from(pred);
                    plane[py * recon_uv_stride + px] = val.clamp(0, 255) as u8;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VP8 bitstream assembly
// ---------------------------------------------------------------------------

/// Key-frame luma mode tree (RFC 6386 §11.2 `kf_ymode_tree`) and its fixed
/// probabilities (§11.3). `DC_PRED` is *not* the "0" branch — it sits three
/// levels down, behind `B_PRED`.
#[rustfmt::skip]
static KF_YMODE_TREE: [i8; 8] = [
    -(Y_B_PRED as i8), 2,
    4, 6,
    -(Y_DC_PRED as i8), -(Y_V_PRED as i8),
    -(Y_H_PRED as i8), -(Y_TM_PRED as i8),
];

/// Fixed key-frame luma-mode probabilities (RFC 6386 §11.3).
static KF_YMODE_PROB: [u8; 4] = [145, 156, 163, 128];

/// Chroma mode tree (RFC 6386 §11.4 `uv_mode_tree`).
#[rustfmt::skip]
static UV_MODE_TREE: [i8; 6] = [
    -(Y_DC_PRED as i8), 2,
    -(Y_V_PRED as i8), 4,
    -(Y_H_PRED as i8), -(Y_TM_PRED as i8),
];

/// Fixed key-frame chroma-mode probabilities (RFC 6386 §11.4).
static KF_UV_MODE_PROB: [u8; 3] = [142, 114, 183];

// Whole-block prediction modes (RFC 6386 §11.2): DC, vertical, horizontal,
// "TrueMotion", and the per-4x4-subblock mode (luma only).
const Y_DC_PRED: i32 = 0;
const Y_V_PRED: i32 = 1;
const Y_H_PRED: i32 = 2;
const Y_TM_PRED: i32 = 3;
const Y_B_PRED: i32 = 4;

/// Writes the first partition: the frame header (RFC 6386 §9.2-§9.10) and
/// then every macroblock's prediction modes (§11).
///
/// Field order and coding follow the key-frame column of the §19.2 bitstream
/// syntax table verbatim (rfc6386.txt lines 6812-6854). Two fields here were
/// previously mis-coded, and either alone makes the rest of the frame
/// unparseable, because a boolean arithmetic coder has no resynchronisation
/// point:
///
/// - `refresh_entropy_probs` (§9.8, rfc6386.txt line 6822) is coded on key
///   frames too — `if (key_frame) refresh_entropy_probs L(1)` — and was
///   simply absent.
/// - The 1056 `token_prob_update()` gates (§9.9/§13.4) are each coded
///   against their own entry in `coeff_update_probs`, not at probability
///   1/2. Writing "no update" with the wrong probability still writes *a*
///   symbol, but not the one the decoder subtracts.
fn write_frame_header(enc: &mut BoolEncoder, mb_width: u32, mb_height: u32, quant_index: u8) {
    // --- colour space and clamping (RFC 6386 §9.2) ---
    enc.encode_bit(false); // colour_space: 0 = YUV
    enc.encode_bit(false); // clamping_type: 0 = decoder must clamp

    // --- segmentation (RFC 6386 §9.3) ---
    enc.encode_bit(false); // segmentation_enabled

    // --- loop filter (RFC 6386 §9.4) ---
    enc.encode_bit(false); // filter_type: 0 = normal
    enc.encode_literal(0, 6); // loop_filter_level: 0 disables the filter
    enc.encode_literal(0, 3); // sharpness_level
    enc.encode_bit(false); // loop_filter_adj_enable

    // --- token partitions (RFC 6386 §9.5) ---
    // log2(1) = 0: a single partition, for which §9.5 writes no size table.
    enc.encode_literal(0, 2);

    // --- quantiser indices (RFC 6386 §9.6) ---
    enc.encode_literal(u32::from(quant_index), 7);
    for _ in 0..5 {
        // y_dc / y2_dc / y2_ac / uv_dc / uv_ac deltas: all absent.
        enc.encode_bit(false);
    }

    // --- refresh_entropy_probs (RFC 6386 §9.8; §19.2 line 6822) ---
    enc.encode_bit(false);

    // --- DCT-token probability updates (RFC 6386 §9.9, §13.4) ---
    // "No update" for all 1056 probabilities, each gated by its own
    // `coeff_update_probs` entry.
    for &update_prob in COEFF_UPDATE_PROBS.iter() {
        enc.encode_bool(false, update_prob);
    }

    // --- mb_no_skip_coeff (RFC 6386 §9.10) ---
    // Disabled, so no per-macroblock skip flag follows and every macroblock
    // codes its residual.
    enc.encode_bit(false);

    // --- per-macroblock prediction modes (RFC 6386 §11.2-§11.4) ---
    // Every macroblock is whole-block DC-predicted, so no 4x4 submodes
    // follow (those are coded only for B_PRED).
    let total_mbs = mb_width * mb_height;
    for _ in 0..total_mbs {
        write_tree(enc, &KF_YMODE_TREE, &KF_YMODE_PROB, Y_DC_PRED, 0);
        write_tree(enc, &UV_MODE_TREE, &KF_UV_MODE_PROB, Y_DC_PRED, 0);
    }
}

// ---------------------------------------------------------------------------
// Public encoder API
// ---------------------------------------------------------------------------

/// VP8 lossy encoder for WebP.
///
/// Produces valid VP8 keyframe bitstreams that can be embedded in a WebP
/// RIFF container.  The encoder generates intra-only (keyframe) frames
/// using DC prediction and a configurable quality parameter.
///
/// # Examples
///
/// ```
/// use oximedia_codec::webp::encoder::WebPLossyEncoder;
///
/// let encoder = WebPLossyEncoder::new(75);
///
/// // 2x2 red image
/// let rgb = [255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0];
/// let vp8_data = encoder.encode_rgb(&rgb, 2, 2).expect("encode");
///
/// // The output starts with a valid VP8 frame tag
/// assert!(!vp8_data.is_empty());
/// ```
pub struct WebPLossyEncoder {
    quality: u8,
}

impl WebPLossyEncoder {
    /// Creates a new lossy encoder with the given quality (0-100).
    ///
    /// - 0 = lowest quality / smallest size
    /// - 100 = highest quality / largest size
    #[must_use]
    pub fn new(quality: u8) -> Self {
        Self {
            quality: quality.min(100),
        }
    }

    /// Maps quality (0-100) to VP8 quantizer index (0-127).
    fn quality_to_qindex(&self) -> u8 {
        // Linear mapping: quality 100 → qindex 0, quality 0 → qindex 127
        let qindex = 127 - (u32::from(self.quality) * 127 / 100);
        (qindex as u8).min(127)
    }

    /// Encodes RGB data to a VP8 bitstream (without RIFF container).
    ///
    /// The input `data` must contain `width * height * 3` bytes in
    /// row-major R, G, B order (8 bits per component).
    ///
    /// Returns the raw VP8 bitstream bytes suitable for wrapping in a
    /// WebP RIFF container.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidParameter` if dimensions are zero or
    /// the data length does not match `width * height * 3`.
    pub fn encode_rgb(&self, data: &[u8], width: u32, height: u32) -> CodecResult<Vec<u8>> {
        self.validate_dimensions(width, height)?;

        let expected_len = (width as usize) * (height as usize) * 3;
        if data.len() < expected_len {
            return Err(CodecError::InvalidParameter(format!(
                "RGB data too short: expected {expected_len}, got {}",
                data.len()
            )));
        }

        let yuv = rgb_to_yuv420(data, width, height)?;
        self.encode_yuv(&yuv)
    }

    /// Encodes RGBA data to VP8 bitstream + separate alpha channel.
    ///
    /// Returns `(vp8_data, alpha_data)` where `alpha_data` contains
    /// the raw alpha plane bytes (width * height, row-major, uncompressed).
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidParameter` if dimensions are zero or
    /// the data length does not match `width * height * 4`.
    pub fn encode_rgba(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> CodecResult<(Vec<u8>, Vec<u8>)> {
        self.validate_dimensions(width, height)?;

        let w = width as usize;
        let h = height as usize;
        let expected_len = w * h * 4;
        if data.len() < expected_len {
            return Err(CodecError::InvalidParameter(format!(
                "RGBA data too short: expected {expected_len}, got {}",
                data.len()
            )));
        }

        // Extract RGB and alpha
        let pixel_count = w * h;
        let mut rgb = Vec::with_capacity(pixel_count * 3);
        let mut alpha = Vec::with_capacity(pixel_count);

        for i in 0..pixel_count {
            let base = i * 4;
            rgb.push(data[base]);
            rgb.push(data[base + 1]);
            rgb.push(data[base + 2]);
            alpha.push(data[base + 3]);
        }

        let vp8_data = self.encode_rgb(&rgb, width, height)?;
        Ok((vp8_data, alpha))
    }

    /// Validates that width and height are non-zero and within VP8 limits.
    fn validate_dimensions(&self, width: u32, height: u32) -> CodecResult<()> {
        if width == 0 || height == 0 {
            return Err(CodecError::InvalidParameter(
                "Width and height must be non-zero".to_string(),
            ));
        }
        // VP8 maximum dimension is 16383
        if width > 16383 || height > 16383 {
            return Err(CodecError::InvalidParameter(format!(
                "Dimensions {}x{} exceed VP8 maximum of 16383",
                width, height
            )));
        }
        Ok(())
    }

    /// Core encoding: takes YUV planes and produces a VP8 bitstream.
    fn encode_yuv(&self, yuv: &YuvPlanes) -> CodecResult<Vec<u8>> {
        let width = yuv.width;
        let height = yuv.height;
        let mb_width = ((width + 15) / 16) as usize;
        let mb_height = ((height + 15) / 16) as usize;

        // Dequantisation factors, derived exactly as the decoder derives
        // them (RFC 6386 §14.1): Y2 DC doubles, Y2 AC is 155% with a floor
        // of 8 applied to the *product*, and chroma DC saturates at 132.
        // Applying the floor to the table entry instead, and omitting the
        // chroma cap, made the encoder and decoder disagree about the scale
        // of every coefficient at low quality settings.
        let qindex = self.quality_to_qindex();
        let qi = (qindex as usize).min(127);
        let dc_quant = DC_QUANT_TABLE[qi];
        let ac_quant = AC_QUANT_TABLE[qi];
        let y2_dc_quant = DC_QUANT_TABLE[qi] * 2;
        let y2_ac_quant = (AC_QUANT_TABLE[qi] * 155 / 100).max(8);
        let uv_dc_quant = DC_QUANT_TABLE[qi].min(132);
        let uv_ac_quant = AC_QUANT_TABLE[qi];

        // Reconstructed planes for prediction reference
        let recon_y_stride = mb_width * 16;
        let recon_uv_stride = mb_width * 8;
        let mut recon_y = vec![128u8; recon_y_stride * mb_height * 16];
        let mut recon_u = vec![128u8; recon_uv_stride * mb_height * 8];
        let mut recon_v = vec![128u8; recon_uv_stride * mb_height * 8];

        // Encode frame header (Partition 1)
        let mut header_enc = BoolEncoder::new();
        write_frame_header(&mut header_enc, mb_width as u32, mb_height as u32, qindex);

        // Encode DCT tokens (Partition 2)
        let mut token_enc = BoolEncoder::new();

        // Non-zero (entropy) contexts, mirroring the decoder: 9 slots per
        // macroblock column (4 luma + 2 U + 2 V + 1 Y2), with the "left"
        // context reset at the start of every macroblock row.
        let mut above_nz = vec![false; mb_width * 9];

        for mby in 0..mb_height {
            let mut left_nz = [false; 9];
            for mbx in 0..mb_width {
                let mb = encode_macroblock(
                    yuv,
                    mbx,
                    mby,
                    dc_quant,
                    ac_quant,
                    y2_dc_quant,
                    y2_ac_quant,
                    uv_dc_quant,
                    uv_ac_quant,
                    &recon_y,
                    recon_y_stride,
                    &recon_u,
                    recon_uv_stride,
                    &recon_v,
                );

                // --- residual tokens (RFC 6386 §13) ---
                // Sub-block order, block types and entropy contexts must
                // match the decoder's `decode_residuals` exactly. `ctx` is
                // `above_nz + left_nz` for the corresponding slot: 4 luma
                // columns/rows, 2+2 chroma, and slot 8 for Y2.

                // Y2 block: block type 1 (the previous code said 3, which is
                // the type reserved for luma *without* a Y2 block).
                let ctx = usize::from(left_nz[8]) + usize::from(above_nz[mbx * 9 + 8]);
                let nz = encode_block(&mut token_enc, &mb.y2_block, 1, ctx, 0);
                left_nz[8] = nz;
                above_nz[mbx * 9 + 8] = nz;

                // 16 luma sub-blocks in raster order: block type 0 (DC lives
                // in Y2), so coefficient 0 is skipped.
                for row in 0..4 {
                    for col in 0..4 {
                        let sb = row * 4 + col;
                        let ctx = usize::from(above_nz[mbx * 9 + col]) + usize::from(left_nz[row]);
                        let nz = encode_block(&mut token_enc, &mb.y_blocks[sb], 0, ctx, 1);
                        above_nz[mbx * 9 + col] = nz;
                        left_nz[row] = nz;
                    }
                }

                // 4 U then 4 V sub-blocks: block type 2. U uses above/left
                // context slots 4-5, V uses 6-7.
                for (plane, blocks) in [(0usize, &mb.u_blocks), (1usize, &mb.v_blocks)] {
                    let ctx_base = 4 + plane * 2;
                    for row in 0..2 {
                        for col in 0..2 {
                            let sb = row * 2 + col;
                            let above_index = mbx * 9 + ctx_base + col;
                            let left_index = ctx_base + row;
                            let ctx = usize::from(above_nz[above_index])
                                + usize::from(left_nz[left_index]);
                            let nz = encode_block(&mut token_enc, &blocks[sb], 2, ctx, 0);
                            above_nz[above_index] = nz;
                            left_nz[left_index] = nz;
                        }
                    }
                }

                // Reconstruct macroblock for prediction reference
                let pred_y = compute_dc_pred(&recon_y, recon_y_stride, mbx, mby, 16);
                let pred_u = compute_dc_pred(&recon_u, recon_uv_stride, mbx, mby, 8);
                let pred_v = compute_dc_pred(&recon_v, recon_uv_stride, mbx, mby, 8);

                reconstruct_macroblock(
                    &mb,
                    dc_quant,
                    ac_quant,
                    y2_dc_quant,
                    y2_ac_quant,
                    uv_dc_quant,
                    uv_ac_quant,
                    pred_y,
                    pred_u,
                    pred_v,
                    &mut recon_y,
                    recon_y_stride,
                    mbx,
                    mby,
                    &mut recon_u,
                    recon_uv_stride,
                    &mut recon_v,
                );
            }
        }

        let header_data = header_enc.flush();
        let token_data = token_enc.flush();

        // Assemble VP8 bitstream
        self.assemble_bitstream(width, height, &header_data, &token_data)
    }

    /// Assembles the final VP8 bitstream from header and token partitions.
    fn assemble_bitstream(
        &self,
        width: u32,
        height: u32,
        header_data: &[u8],
        token_data: &[u8],
    ) -> CodecResult<Vec<u8>> {
        let first_partition_size = header_data.len() as u32;

        // Total output size: frame_tag(3) + sync(3) + dims(4) + partitions
        let total_size = 3 + 3 + 4 + header_data.len() + token_data.len();
        let mut output = Vec::with_capacity(total_size);

        // --- Frame tag (3 bytes) ---
        // bit 0: frame_type (0 = keyframe)
        // bits 1-3: version (0)
        // bit 4: show_frame (1)
        // bits 5-7 of byte 0 + bytes 1-2: first_partition_size (19 bits)
        let b0: u8 = 0x00  // frame_type = 0 (key)
            | 0x00          // version = 0
            | 0x10          // show_frame = 1
            | ((first_partition_size << 5) as u8 & 0xE0);
        let b1: u8 = (first_partition_size >> 3) as u8;
        let b2: u8 = (first_partition_size >> 11) as u8;

        output.push(b0);
        output.push(b1);
        output.push(b2);

        // --- Sync code ---
        output.push(0x9D);
        output.push(0x01);
        output.push(0x2A);

        // --- Dimensions (4 bytes, LE) ---
        // width: bits 0-13, horizontal_scale: bits 14-15
        let w_le = (width & 0x3FFF) as u16;
        output.push(w_le as u8);
        output.push((w_le >> 8) as u8);

        let h_le = (height & 0x3FFF) as u16;
        output.push(h_le as u8);
        output.push((h_le >> 8) as u8);

        // --- Partition 1 (header) ---
        output.extend_from_slice(header_data);

        // --- Partition 2 (tokens) ---
        output.extend_from_slice(token_data);

        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bool_encoder_basic() {
        let mut enc = BoolEncoder::new();
        enc.encode_bit(false);
        enc.encode_bit(true);
        enc.encode_bit(false);
        let data = enc.flush();
        assert!(!data.is_empty());
    }

    #[test]
    fn test_bool_encoder_literal() {
        let mut enc = BoolEncoder::new();
        enc.encode_literal(42, 8);
        let data = enc.flush();
        assert!(!data.is_empty());
    }

    #[test]
    fn test_bool_encoder_with_prob() {
        let mut enc = BoolEncoder::new();
        // Encode several symbols with different probabilities
        for prob in [1, 50, 128, 200, 255] {
            enc.encode_bool(true, prob);
            enc.encode_bool(false, prob);
        }
        let data = enc.flush();
        assert!(!data.is_empty());
    }

    #[test]
    fn test_fdct4x4_and_idct4x4_are_duals() {
        // The forward DCT must be undone by RFC 6386 §14.4's inverse to
        // within integer rounding. This is the property the old
        // implementation violated: it had the 2217/5352 rotation constants
        // exchanged, so `idct4x4(fdct4x4(x))` was not `x` at all.
        let residual = [
            12i32, -30, 7, 41, -5, 60, -22, 3, 18, -9, 55, -40, 2, 33, -17, 26,
        ];
        let mut coeffs = [0i32; 16];
        fdct4x4(&residual, &mut coeffs);
        let restored = idct4x4(&coeffs);

        for (i, (&want, &got)) in residual.iter().zip(restored.iter()).enumerate() {
            assert!(
                (want - got).abs() <= 2,
                "sample {i}: forward/inverse DCT round trip drifted: want {want}, got {got}"
            );
        }
    }

    #[test]
    fn test_fwht4x4_and_iwht4x4_are_duals() {
        // Same duality requirement for the Y2 (Walsh-Hadamard) transform:
        // the §14.3 inverse divides by 8, so the forward transform must
        // carry the remaining factor of 1/2. Omitting it (as the previous
        // implementation did) scaled every luma DC by two.
        let dc_values = [
            300i32, -120, 44, 900, -640, 15, 208, -77, 1024, 5, -333, 66, 12, -900, 450, 81,
        ];
        let mut y2 = [0i32; 16];
        fwht4x4(&dc_values, &mut y2);
        let restored = iwht4x4(&y2);

        for (i, (&want, &got)) in dc_values.iter().zip(restored.iter()).enumerate() {
            assert!(
                (want - got).abs() <= 2,
                "DC {i}: forward/inverse WHT round trip drifted: want {want}, got {got}"
            );
        }
    }

    #[test]
    fn test_fdct4x4_dc_only() {
        let residual = [10i32; 16]; // Flat residual
        let mut coeffs = [0i32; 16];
        fdct4x4(&residual, &mut coeffs);

        // DC coefficient should be dominant
        assert!(coeffs[0].abs() > 0);
        // AC should be much smaller than DC for flat input
        let dc_abs = coeffs[0].abs();
        for i in 1..16 {
            assert!(
                coeffs[i].abs() < dc_abs / 2,
                "AC coeff[{i}] = {} should be much smaller than DC = {}",
                coeffs[i],
                coeffs[0]
            );
        }
    }

    #[test]
    fn test_fwht4x4_dc_only() {
        let dc_values = [100i32; 16]; // All same DC
        let mut coeffs = [0i32; 16];
        fwht4x4(&dc_values, &mut coeffs);

        // DC = sum / 2 = 16 * 100 / 2. The `/2` is the normalisation RFC
        // 6386 §14.3's inverse (which divides by 8, against the Hadamard
        // butterfly's own factor of 16) expects the forward pass to carry.
        assert_eq!(coeffs[0], 800);
        // AC should be 0
        for i in 1..16 {
            assert_eq!(coeffs[i], 0);
        }
    }

    #[test]
    fn test_quantize() {
        assert_eq!(quantize(100, 10), 10);
        assert_eq!(quantize(-100, 10), -10);
        assert_eq!(quantize(0, 10), 0);
        assert_eq!(quantize(4, 10), 0); // Below threshold
        assert_eq!(quantize(15, 10), 2); // (15+5)/10 = 2
    }

    #[test]
    fn test_quality_to_qindex() {
        let enc_low = WebPLossyEncoder::new(0);
        let enc_mid = WebPLossyEncoder::new(50);
        let enc_high = WebPLossyEncoder::new(100);

        assert_eq!(enc_high.quality_to_qindex(), 0);
        assert_eq!(enc_low.quality_to_qindex(), 127);
        assert!(enc_mid.quality_to_qindex() > 0);
        assert!(enc_mid.quality_to_qindex() < 127);
    }

    #[test]
    fn test_rgb_to_yuv420_basic() {
        // 4x4 white image
        let data = vec![255u8; 4 * 4 * 3];
        let yuv = rgb_to_yuv420(&data, 4, 4).expect("conversion should succeed");

        // White (255,255,255) → Y ≈ 255
        for &y in &yuv.y[..16] {
            assert!(y >= 250, "Y should be near 255 for white, got {y}");
        }
    }

    #[test]
    fn test_rgb_to_yuv420_black() {
        // 4x4 black image
        let data = vec![0u8; 4 * 4 * 3];
        let yuv = rgb_to_yuv420(&data, 4, 4).expect("conversion should succeed");

        // Black (0,0,0) → Y = 0, U = 128, V = 128
        for &y in &yuv.y[..16] {
            assert!(y <= 5, "Y should be near 0 for black, got {y}");
        }
        for &u in &yuv.u[..4] {
            assert!(
                (120..=136).contains(&u),
                "U should be near 128 for black, got {u}"
            );
        }
    }

    #[test]
    fn test_rgb_to_yuv420_short_data() {
        let data = vec![0u8; 10]; // too short for any image
        assert!(rgb_to_yuv420(&data, 4, 4).is_err());
    }

    #[test]
    fn test_encode_rgb_produces_valid_frame_tag() {
        let encoder = WebPLossyEncoder::new(50);
        // 16x16 gray image
        let data = vec![128u8; 16 * 16 * 3];
        let vp8 = encoder
            .encode_rgb(&data, 16, 16)
            .expect("encode should succeed");

        // Check sync code at bytes 3..6
        assert!(vp8.len() >= 10);
        assert_eq!(vp8[3], 0x9D);
        assert_eq!(vp8[4], 0x01);
        assert_eq!(vp8[5], 0x2A);

        // Check frame type (keyframe = bit 0 of byte 0 is 0)
        assert_eq!(vp8[0] & 0x01, 0, "Should be keyframe");

        // Check show_frame (bit 4 of byte 0)
        assert_ne!(vp8[0] & 0x10, 0, "show_frame should be set");

        // Check dimensions
        let w = u16::from(vp8[6]) | (u16::from(vp8[7]) << 8);
        let h = u16::from(vp8[8]) | (u16::from(vp8[9]) << 8);
        assert_eq!(w & 0x3FFF, 16);
        assert_eq!(h & 0x3FFF, 16);
    }

    #[test]
    fn test_encode_rgb_different_qualities() {
        let data = vec![100u8; 32 * 32 * 3];

        let low = WebPLossyEncoder::new(10);
        let high = WebPLossyEncoder::new(90);

        let low_data = low.encode_rgb(&data, 32, 32).expect("low quality encode");
        let high_data = high.encode_rgb(&data, 32, 32).expect("high quality encode");

        // Both should produce valid output
        assert!(!low_data.is_empty());
        assert!(!high_data.is_empty());
    }

    #[test]
    fn test_encode_rgb_non_mb_aligned() {
        // 7x5 image: not aligned to 16x16 macroblock grid
        let encoder = WebPLossyEncoder::new(75);
        let data = vec![200u8; 7 * 5 * 3];
        let vp8 = encoder
            .encode_rgb(&data, 7, 5)
            .expect("non-aligned encode should succeed");

        assert!(!vp8.is_empty());

        // Dimensions in bitstream should match original, not padded
        let w = u16::from(vp8[6]) | (u16::from(vp8[7]) << 8);
        let h = u16::from(vp8[8]) | (u16::from(vp8[9]) << 8);
        assert_eq!(w & 0x3FFF, 7);
        assert_eq!(h & 0x3FFF, 5);
    }

    #[test]
    fn test_encode_rgba_basic() {
        let encoder = WebPLossyEncoder::new(75);
        // 4x4 red with 50% alpha
        let mut rgba = Vec::with_capacity(4 * 4 * 4);
        for _ in 0..16 {
            rgba.extend_from_slice(&[255, 0, 0, 128]);
        }

        let (vp8_data, alpha_data) = encoder
            .encode_rgba(&rgba, 4, 4)
            .expect("RGBA encode should succeed");

        assert!(!vp8_data.is_empty());
        assert_eq!(alpha_data.len(), 16);
        assert!(alpha_data.iter().all(|&a| a == 128));
    }

    #[test]
    fn test_encode_zero_dimensions() {
        let encoder = WebPLossyEncoder::new(50);
        assert!(encoder.encode_rgb(&[], 0, 10).is_err());
        assert!(encoder.encode_rgb(&[], 10, 0).is_err());
    }

    #[test]
    fn test_encode_too_short_data() {
        let encoder = WebPLossyEncoder::new(50);
        let data = vec![0u8; 10];
        assert!(encoder.encode_rgb(&data, 16, 16).is_err());
    }

    #[test]
    fn test_encode_oversized_dimensions() {
        let encoder = WebPLossyEncoder::new(50);
        assert!(encoder.encode_rgb(&[], 20000, 100).is_err());
    }

    #[test]
    fn test_idct4x4_dc_only_is_uniform() {
        // DC-only input
        let mut coeffs = [0i32; 16];
        coeffs[0] = 400;

        let output = idct4x4(&coeffs);
        // All outputs should be roughly equal (DC distributed)
        let avg = output.iter().sum::<i32>() / 16;
        for &v in &output {
            assert!(
                (v - avg).abs() <= 2,
                "DC-only IDCT should produce roughly uniform output"
            );
        }
    }

    #[test]
    fn test_encode_rgb_1x1() {
        // Smallest possible image
        let encoder = WebPLossyEncoder::new(75);
        let data = [128, 128, 128]; // gray pixel
        let vp8 = encoder
            .encode_rgb(&data, 1, 1)
            .expect("1x1 encode should succeed");

        assert!(!vp8.is_empty());
        // Verify it's a keyframe
        assert_eq!(vp8[0] & 0x01, 0);
    }

    #[test]
    fn test_encode_quality_extremes() {
        let data = vec![128u8; 16 * 16 * 3];

        // Quality 0
        let enc0 = WebPLossyEncoder::new(0);
        let out0 = enc0.encode_rgb(&data, 16, 16).expect("q0");
        assert!(!out0.is_empty());

        // Quality 100
        let enc100 = WebPLossyEncoder::new(100);
        let out100 = enc100.encode_rgb(&data, 16, 16).expect("q100");
        assert!(!out100.is_empty());

        // Quality > 100 should clamp
        let enc200 = WebPLossyEncoder::new(200);
        assert_eq!(enc200.quality, 100);
    }

    #[test]
    fn test_encode_rgb_colored_image() {
        // Create a simple gradient image
        let width = 32u32;
        let height = 32u32;
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                data.push((x * 8) as u8); // R
                data.push((y * 8) as u8); // G
                data.push(128); // B
            }
        }

        let encoder = WebPLossyEncoder::new(80);
        let vp8 = encoder
            .encode_rgb(&data, width, height)
            .expect("gradient encode should succeed");

        // Verify valid VP8 header
        assert!(vp8.len() > 10);
        assert_eq!(vp8[3], 0x9D);
        assert_eq!(vp8[4], 0x01);
        assert_eq!(vp8[5], 0x2A);
    }

    #[test]
    fn test_first_partition_size_encoding() {
        // The first_partition_size must be correctly encoded in the frame tag
        let encoder = WebPLossyEncoder::new(50);
        let data = vec![128u8; 16 * 16 * 3];
        let vp8 = encoder.encode_rgb(&data, 16, 16).expect("encode");

        // Extract first_partition_size from frame tag
        let b0 = vp8[0];
        let b1 = vp8[1];
        let b2 = vp8[2];
        let fps = (u32::from(b0 >> 5) & 0x07) | (u32::from(b1) << 3) | (u32::from(b2) << 11);

        // The partition should start after the 10-byte header
        // and its size should be reasonable
        assert!(fps > 0, "first_partition_size should be non-zero");
        assert!(
            (fps as usize) < vp8.len(),
            "first_partition_size ({fps}) should be less than total ({}) ",
            vp8.len()
        );
    }

    #[test]
    fn test_compute_dc_pred_16x16_no_neighbors() {
        let recon = vec![0u8; 16 * 16];
        let pred = compute_dc_pred(&recon, 16, 0, 0, 16);
        assert_eq!(pred, 128); // Default when no neighbors available
    }

    #[test]
    fn test_compute_dc_pred_16x16_with_top() {
        // Place known values in the row above (mb_y=1, top row is row 15)
        let stride = 32;
        let mut recon = vec![0u8; stride * 32];
        for col in 0..16 {
            recon[15 * stride + col] = 200;
        }
        let pred = compute_dc_pred(&recon, stride, 0, 1, 16);
        assert_eq!(pred, 200);
    }

    #[test]
    fn test_fwht_iwht_roundtrip() {
        // Verify forward WHT structural correctness:
        // A uniform input should produce a single DC coefficient.
        let uniform = [50i32; 16];
        let mut wht_coeffs = [0i32; 16];
        fwht4x4(&uniform, &mut wht_coeffs);

        // DC = sum of all / 2 = 50 * 16 / 2 (see `test_fwht4x4_dc_only`).
        assert_eq!(wht_coeffs[0], 400);
        // All AC should be zero for uniform input
        for i in 1..16 {
            assert_eq!(wht_coeffs[i], 0, "AC coeff at index {i} should be 0");
        }

        // Verify that a non-uniform input produces non-zero AC
        let varied = [
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
        ];
        fwht4x4(&varied, &mut wht_coeffs);

        // DC should equal half the sum of all values.
        let total: i32 = varied.iter().sum();
        assert_eq!(wht_coeffs[0], total / 2);

        // At least some AC coefficients should be non-zero
        let nonzero_ac = wht_coeffs[1..].iter().filter(|&&c| c != 0).count();
        assert!(nonzero_ac > 0, "Non-uniform input should have non-zero AC");
    }
}

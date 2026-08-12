//! Test-support utilities shared across `dec/*` test modules (`cfg(test)`
//! only).
//!
//! # [`TestBoolEncoder`]
//!
//! A minimal test-only VP8 boolean-arithmetic ENCODER (RFC 6386 §7.3
//! `write_bool` / `flush_bool_encoder`, rfc6386.txt lines 1141-1228,
//! transcribed essentially verbatim). It exists because most decoder code
//! paths cannot be reached from the handful of real fixture files in
//! `dec/testdata`: a fixture exercises whatever its encoder happened to
//! choose, not the corners of the format. Synthesising the exact bit
//! sequence under test is the only way to cover those corners, and an
//! encoder that is the *provable dual* of the production [`BoolDecoder`] is
//! the only honest way to synthesise it.
//!
//! Every arithmetic line below is byte-identical to the version originally
//! written and validated in `header.rs` for P2's inter-frame header tests;
//! this module is a pure relocation so that P3's motion-vector tests can
//! share it. Its trustworthiness rests on
//! `header.rs::tests::test_bool_encoder_roundtrips_through_production_bool_decoder`
//! and
//! `header.rs::tests::test_bool_encoder_roundtrips_coeff_and_mv_update_gate_shapes`,
//! which round-trip it through the production [`BoolDecoder`]; that pair
//! doubles as an independent encoder/decoder cross-check of [`BoolDecoder`]
//! itself, which `bool_decoder.rs`'s own tests only exercise against fixed,
//! hand-computed byte sequences.
//!
//! # Symbol-level writers
//!
//! On top of the raw bool/literal primitives this module also provides the
//! duals of the *structured* readers: [`TestBoolEncoder::write_tree`] (dual
//! of [`BoolDecoder::read_tree`]) and
//! [`TestBoolEncoder::write_mv_component`] / [`TestBoolEncoder::write_mv`]
//! (duals of `mv::read_mv_component` / `mv::read_mv`, RFC 6386 §17.2).
//!
//! A round-trip against a dual only proves that encoder and decoder agree;
//! it cannot prove that *both* match the RFC. Tests that need to pin the
//! absolute bit order therefore hand-write raw flag sequences with
//! [`TestBoolEncoder::write_flag`] instead (see `mv.rs`'s hand-vector
//! tests).

use super::bool_decoder::BoolDecoder;
use super::tables_inter::{
    MVLONG_WIDTH, MVNUM_SHORT, MVPBITS, MVPIS_SHORT, MVPSHORT, MVPSIGN, MV_PROB_CNT, SMALL_MVTREE,
};

/// Test-only VP8 boolean-arithmetic encoder (RFC 6386 §7.3).
pub(super) struct TestBoolEncoder {
    output: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
}

impl TestBoolEncoder {
    pub(super) fn new() -> Self {
        Self {
            output: Vec::new(),
            range: 255,
            bottom: 0,
            bit_count: 24,
        }
    }

    /// RFC 6386 `add_one_to_output`: propagate a carry backward through
    /// any already-written trailing `0xFF` bytes.
    fn add_one_to_output(&mut self) {
        for byte in self.output.iter_mut().rev() {
            if *byte == 0xFF {
                *byte = 0;
            } else {
                *byte += 1;
                return;
            }
        }
        // RFC 6386 lines 1149-1153: "the arithmetic guarantees that the
        // propagation will never go beyond the beginning of the output"
        // -- a carry has nothing to propagate into before the first output
        // byte exists, because those leading bits are the value's own high
        // end. Rather than lean on that guarantee alone, every payload
        // this helper builds is decoded straight back through the
        // production `BoolDecoder` and asserted, so a carry silently
        // dropped here would surface as a decode mismatch rather than as
        // corrupt bytes. (P2's payloads were hundreds of bits long; P3's
        // motion-vector vectors are as short as four bools, so the
        // justification cannot rest on payload length.)
    }

    /// RFC 6386 `write_bool`. `self.bottom`'s `+=` uses wrapping
    /// arithmetic to match the C `uint32`'s implicit mod-2^32
    /// semantics -- the carry-out this can produce is exactly what the
    /// `(1u32 << 31)` check below is watching for and propagating via
    /// `add_one_to_output`, so wrapping here is intentional, not a bug
    /// worked around.
    pub(super) fn write_bool(&mut self, prob: u8, bit: bool) {
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        if bit {
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

    pub(super) fn write_flag(&mut self, bit: bool) {
        self.write_bool(128, bit);
    }

    /// Mirrors `BoolDecoder::get_literal`: bits high- to low-order,
    /// each at probability 128.
    pub(super) fn write_literal(&mut self, value: u32, num_bits: u32) {
        for i in (0..num_bits).rev() {
            self.write_flag(((value >> i) & 1) != 0);
        }
    }

    /// Mirrors `BoolDecoder::get_signed_literal`: magnitude first, sign
    /// flag last.
    pub(super) fn write_signed_literal(&mut self, magnitude: u32, num_bits: u32, negative: bool) {
        self.write_literal(magnitude, num_bits);
        self.write_flag(negative);
    }

    /// RFC 6386 `flush_bool_encoder`; call exactly once, after the last
    /// `write_bool`.
    pub(super) fn finish(mut self) -> Vec<u8> {
        let c0 = self.bit_count; // always in 1..=24, see `write_bool`
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

    // -----------------------------------------------------------------
    // Symbol-level writers (duals of the structured `BoolDecoder`
    // readers and of `dec::mv`).
    // -----------------------------------------------------------------

    /// Dual of [`BoolDecoder::read_tree`]: writes the boolean path that
    /// makes `read_tree(tree, probs)` return `value`.
    ///
    /// The path is found by depth-first search over the same flat
    /// `tree_index` layout the decoder walks (RFC 6386 §8 `treed_read`),
    /// so the encoded path is derived from the table under test rather
    /// than from a hand-copied bit assignment.
    ///
    /// Panics (test-only) if `value` is not a leaf of `tree`.
    pub(super) fn write_tree(&mut self, tree: &[i8], probs: &[u8], value: i32) {
        let mut path: Vec<(u8, bool)> = Vec::new();
        assert!(
            find_tree_path(tree, probs, value, 0, &mut path),
            "value {value} is not a leaf of the supplied tree"
        );
        for (prob, bit) in path {
            self.write_bool(prob, bit);
        }
    }

    /// Dual of `mv::read_mv_component` (RFC 6386 §17.2 `read_mvcomponent`,
    /// rfc6386.txt lines 6100-6138).
    ///
    /// `value` is a *raw* component in the RFC's own unit (luma
    /// quarter-pels, `-1023..=1023`, rfc6386.txt lines 6037-6041) -- i.e.
    /// the value `read_mv_component` returns, **before** the ×2 scaling
    /// that `mv::read_mv` applies.
    ///
    /// A magnitude of zero deliberately writes no sign bool: the decoder's
    /// `A && read_bool(...)` short-circuits (rfc6386.txt line 6137).
    pub(super) fn write_mv_component(&mut self, probs: &[u8; MV_PROB_CNT], value: i32) {
        assert!(
            (-1023..=1023).contains(&value),
            "component {value} out of the RFC 6386 line 6040-6041 range -1023..=1023"
        );
        let magnitude = value.abs();

        if magnitude < MVNUM_SHORT as i32 {
            // Short form, 0..=7 (rfc6386.txt line 6134-6135).
            self.write_bool(probs[MVPIS_SHORT], false);
            self.write_tree(&SMALL_MVTREE, &probs[MVPSHORT..], magnitude);
        } else {
            // Long form, 8..=1023 (rfc6386.txt lines 6113-6132).
            self.write_bool(probs[MVPIS_SHORT], true);
            // Bits 0, 1, 2 -- ascending (lines 6115-6119).
            for i in 0..3 {
                self.write_bool(probs[MVPBITS + i], (magnitude >> i) & 1 != 0);
            }
            // Bits 9, 8, 7, 6, 5, 4 -- descending, bit 3 skipped
            // (lines 6121-6125).
            for i in (4..MVLONG_WIDTH).rev() {
                self.write_bool(probs[MVPBITS + i], (magnitude >> i) & 1 != 0);
            }
            // Bit 3 is explicit only when some higher bit is set; when
            // none is, the decoder knows the value is in 8..=15 and adds
            // the implicit 8 without reading (lines 6127-6132).
            if magnitude & 0xfff0 != 0 {
                self.write_bool(probs[MVPBITS + 3], (magnitude >> 3) & 1 != 0);
            } else {
                assert!(
                    magnitude & 8 != 0,
                    "magnitude {magnitude} has no high bits and no bit 3, \
                     so it is not representable in the long form"
                );
            }
        }

        if magnitude != 0 {
            self.write_bool(probs[MVPSIGN], value < 0);
        }
    }

    /// Dual of `mv::read_mv`: row component first, then column
    /// (rfc6386.txt lines 6227-6232). Both values are raw quarter-pel
    /// components, as for [`Self::write_mv_component`].
    pub(super) fn write_mv(&mut self, mvc: &[[u8; MV_PROB_CNT]; 2], row: i32, col: i32) {
        self.write_mv_component(&mvc[0], row);
        self.write_mv_component(&mvc[1], col);
    }
}

/// Depth-first search for the decoder path leading to leaf `value`.
///
/// Mirrors [`BoolDecoder::read_tree`]'s conventions exactly: a non-positive
/// entry is the negation of a leaf value, a positive entry is a child node
/// index, and node `i` is driven by `probs[i >> 1]`. The `next <= 0` test
/// (not `< 0`) matters: `SMALL_MVTREE[4]` is `0`, the RFC's `-0`, which
/// encodes leaf value 0.
fn find_tree_path(
    tree: &[i8],
    probs: &[u8],
    value: i32,
    node: usize,
    path: &mut Vec<(u8, bool)>,
) -> bool {
    for branch in 0..2usize {
        let (Some(&next), Some(&prob)) = (tree.get(node + branch), probs.get(node >> 1)) else {
            return false;
        };
        path.push((prob, branch == 1));
        if next <= 0 {
            if -i32::from(next) == value {
                return true;
            }
        } else if find_tree_path(tree, probs, value, next as usize, path) {
            return true;
        }
        path.pop();
    }
    false
}

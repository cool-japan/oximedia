//! Motion-vector entropy decoding (RFC 6386 §17).
//!
//! A VP8 motion vector is a pair of independently coded signed components,
//! **row (vertical) first, then column (horizontal)** (rfc6386.txt line 6031:
//! "Each vector has two pieces: a vertical component (row) followed by a
//! horizontal component (column)"; the two components use separate
//! probability sets, held as `mvc[2]`, "always row, then column", line 6213).
//!
//! # Unit convention (read this before using [`MotionVector`])
//!
//! There are two different units in play and confusing them is the classic
//! VP8 motion-compensation bug:
//!
//! - The unit **on the wire**. A decoded component is "a signed integer V
//!   representing a vertical or horizontal luma displacement of V
//!   quarter-pixels (and a chroma displacement of V eighth-pixels)", with
//!   `-1023 <= V <= 1023` (rfc6386.txt lines 6037-6041). This is what
//!   [`read_mv_component`] returns.
//! - The unit **in storage**, which is what [`MotionVector`] holds. Because
//!   chroma vectors are averages of four luma vectors and therefore have
//!   1/8-pixel resolution, the synthetic-pixel calculation uses eighth-pel
//!   resolution for luma as well; "In accordance, the stored luma motion
//!   vectors are all doubled, each component of each luma vector becoming an
//!   even integer in the range -2046 to +2046, inclusive" (rfc6386.txt lines
//!   6306-6311).
//!
//! [`read_mv`] performs that ×2, so **every [`MotionVector`] in this decoder
//! is numerically in 1/8-pel units**: the integer pixel offset is `v >> 3`
//! and the interpolation-filter phase is `v & 7` (rfc6386.txt line 6424:
//! "an integer between 0 and 7, representing a forward displacement in
//! eighths of a pixel"; dixie `predict.c`, rfc6386.txt lines 12062-12064:
//! `mx = mv->d.x & 7; my = mv->d.y & 7; reference += ((mv->d.y >> 3) *
//! stride) + (mv->d.x >> 3);`). Luma vectors are always even, so their phase
//! is always one of 0, 2, 4, 6 -- i.e. quarter-pel -- while chroma vectors,
//! derived later by averaging, may take any phase 0..=7.
//!
//! The frozen `vp8/motion.rs` seed uses `>> 2` / `& 3` (quarter-pel storage);
//! that is *not* this format's convention and this module is its replacement.
//! Nothing is imported from it.
//!
//! Note on where the ×2 lives: dixie folds it into the component reader
//! (`return x << 1;`, rfc6386.txt line 10090) while §17.2's abridged
//! pseudo-code omits the scaling entirely (lines 6227-6232). This module
//! follows libvpx's shape -- component reader returns the raw wire value,
//! `read_mv` scales -- so that [`read_mv_component`] can be tested directly
//! against the RFC's own `-1023..=1023` range statement.

use super::bool_decoder::BoolDecoder;
use super::tables_inter::{
    MVLONG_WIDTH, MVPBITS, MVPIS_SHORT, MVPSHORT, MVPSIGN, MV_PROB_CNT, SMALL_MVTREE,
};
use crate::error::CodecResult;

/// A motion vector in **1/8-pel numeric units** (see the module
/// documentation).
///
/// `row` is the vertical displacement, `col` the horizontal one; both are
/// stored doubled with respect to the wire value, so a luma component is
/// always even and lies in `-2046..=2046` (rfc6386.txt lines 6306-6311).
///
/// Consumers split the components themselves, `v >> 3` for the whole-pixel
/// displacement and `v & 7` for the eighth-pel interpolation phase (an
/// arithmetic shift, so a negative component rounds *down* and pairs with a
/// non-negative phase); [`super::mc`] does exactly that, once, in its
/// prediction core. Convenience accessors for the same split live in this
/// module's test module, which pins the negative-vector behaviour against
/// dixie's `reference += (mv->d.y >> 3) * stride` (rfc6386.txt line 12064).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) struct MotionVector {
    /// Vertical displacement, eighth-pels.
    pub row: i16,
    /// Horizontal displacement, eighth-pels.
    pub col: i16,
}

/// Reads one motion-vector component at the current decoder position
/// (RFC 6386 §17.2 `read_mvcomponent`, rfc6386.txt lines 6100-6138; dixie
/// `read_mv_component`, lines 10054-10091).
///
/// `probs` is one 19-entry `MV_CONTEXT` row, indexed with the `MVP*` offsets
/// from `tables_inter`. The returned value is the **raw wire component** in
/// luma quarter-pels, `-1023..=1023` (rfc6386.txt lines 6037-6041); the ×2
/// conversion to this module's 1/8-pel storage unit is [`read_mv`]'s job.
///
/// The coding is:
///
/// 1. One bool against `probs[MVPIS_SHORT]`. Despite the RFC's
///    `mvpis_short` name, *true* selects the **long** form: the branch is
///    annotated `/* 8 <= A <= 1023 */` (line 6113) and dixie labels it
///    `/* Large */` (line 10062).
/// 2. Short form (`false`): the magnitude 0..=7 is read from
///    [`SMALL_MVTREE`] with the 7 internal-node probabilities starting at
///    `MVPSHORT` (line 6135).
/// 3. Long form (`true`): bits 0, 1, 2 ascending (lines 6115-6119), then
///    bits 9, 8, 7, 6, 5, 4 descending (lines 6121-6125), each with its own
///    probability `probs[MVPBITS + i]`. Bit 3 is read **only if some higher
///    bit is set**; otherwise the magnitude is known to be in 8..=15, so
///    bit 3 must be 1 and is not transmitted (lines 6127-6132:
///    `if (!(A & 0xfff0) || read_bool(d, p[MVPbits + 3])) A += 8;` -- the C
///    `||` short-circuits, and so does Rust's, which is what makes the bit
///    conditional rather than merely ignored).
/// 4. A sign bool against `probs[MVPSIGN]`, read **only** for a non-zero
///    magnitude (line 6137: `return A && read_bool(r, p[MVPsign]) ? -A : A`).
///
/// # Errors
///
/// Never fails in practice, and the `CodecResult` is for uniformity with the
/// rest of the decode path: [`BoolDecoder`] is infallible (it reads defined
/// zero bits past the end of a partition), and the long form is a bijection
/// onto exactly `8..=1023` by construction -- the maximum is `7` (bits 0-2)
/// `+ 1008` (bits 4-9) `+ 8` (bit 3) `= 1023`, and the minimum is `0 + 0 + 8
/// = 8` -- so no out-of-range magnitude can be produced to reject.
pub(super) fn read_mv_component(
    d: &mut BoolDecoder,
    probs: &[u8; MV_PROB_CNT],
) -> CodecResult<i32> {
    let mut a: i32;

    if d.get_bool(probs[MVPIS_SHORT]) {
        // Long form: 8 <= A <= 1023 (rfc6386.txt lines 6113-6132).
        a = 0;
        // Bits 0, 1, 2.
        for i in 0..3 {
            a += i32::from(d.get_bool(probs[MVPBITS + i])) << i;
        }
        // Bits 9, 8, 7, 6, 5, 4 (bit 3 deliberately skipped here).
        for i in (4..MVLONG_WIDTH).rev() {
            a += i32::from(d.get_bool(probs[MVPBITS + i])) << i;
        }
        // "We know that A >= 8 because it is coded long, so if A <= 15,
        // bit 3 is one and is not explicitly coded." (lines 6127-6129)
        if (a & 0xfff0) == 0 || d.get_bool(probs[MVPBITS + 3]) {
            a += 8;
        }
    } else {
        // Short form: 0 <= A <= 7, tree-coded (line 6135).
        a = d.read_tree(&SMALL_MVTREE, &probs[MVPSHORT..]);
    }

    // Zero has no sign bit (line 6137).
    if a != 0 && d.get_bool(probs[MVPSIGN]) {
        a = -a;
    }

    Ok(a)
}

/// Reads a complete motion vector (RFC 6386 §17.2 `read_mv`, rfc6386.txt
/// lines 6227-6232; dixie `read_mv`, lines 10174-10181).
///
/// `mvc` holds the two component probability rows, "always row, then column"
/// (line 6213); the row component is read first (line 6230, and dixie's
/// `mv->d.y = read_mv_component(bool, mvc[0]); mv->d.x =
/// read_mv_component(bool, mvc[1]);` at lines 10179-10180).
///
/// Each raw component is doubled into the 1/8-pel storage unit described in
/// the module documentation (rfc6386.txt lines 6306-6311). The product is in
/// `-2046..=2046`, so it always fits the `i16` fields.
///
/// # Errors
///
/// Propagates [`read_mv_component`]'s (currently unreachable) error.
pub(super) fn read_mv(
    d: &mut BoolDecoder,
    mvc: &[[u8; MV_PROB_CNT]; 2],
) -> CodecResult<MotionVector> {
    let row = read_mv_component(d, &mvc[0])?;
    let col = read_mv_component(d, &mvc[1])?;
    Ok(MotionVector {
        row: (row * 2) as i16,
        col: (col * 2) as i16,
    })
}

/// Clamps a motion vector to the "unrestricted motion vector" border
/// (RFC 6386 §16.3 `vp8_clamp_mv`, rfc6386.txt lines 5609-5620; dixie
/// `clamp_mv`, lines 9863-9877).
///
/// # Units and who computes the bounds
///
/// All four bounds are **inclusive limits in the same 1/8-pel units as
/// [`MotionVector`]** and are expected to arrive with the UMV border
/// **already folded in** -- they are dixie's `struct mv_clamp_rect`
/// (rfc6386.txt lines 9838-9841), not libvpx's raw `mb_to_*_edge` (where the
/// `LEFT_TOP_MARGIN` / `RIGHT_BOTTOM_MARGIN` adjustment is applied at the
/// clamp site instead, lines 5611-5620). The two formulations are the same
/// clamp; this one keeps the margin arithmetic in one place, at the caller.
///
/// The caller (the per-macroblock mode/MV reader) computes them once per
/// macroblock row and slides them across the row, using a one-macroblock
/// border (rfc6386.txt lines 10605-10609 and 10636-10637; `<< 7` is
/// `16 pixels << 3` eighth-pels):
///
/// ```text
/// to_left   = -((col + 1)      << 7)      // mb_to_left
/// to_right  =  ((mb_cols - col) << 7)     // mb_to_right
/// to_top    = -((row + 1)      << 7)      // mb_to_top
/// to_bottom =  ((mb_rows - row) << 7)     // mb_to_bottom
/// ```
///
/// # Where it is applied
///
/// The near-MV search clamps NEAREST, NEAR and BEST (rfc6386.txt lines
/// 5759-5761), and NEWMV adds its decoded delta to the *clamped* BEST
/// predictor without re-clamping the sum (lines 10507-10511) -- so this is a
/// predictor clamp, not a final-vector clamp.
pub(super) fn clamp_mv(
    mv: MotionVector,
    mb_to_left: i32,
    mb_to_right: i32,
    mb_to_top: i32,
    mb_to_bottom: i32,
) -> MotionVector {
    MotionVector {
        row: clamp_component(mv.row, mb_to_top, mb_to_bottom),
        col: clamp_component(mv.col, mb_to_left, mb_to_right),
    }
}

/// One axis of [`clamp_mv`].
///
/// dixie writes the second test against the *unclamped* input
/// (`newmv.d.x = (raw.d.x > bounds->to_right) ? bounds->to_right :
/// newmv.d.x;`, rfc6386.txt lines 9870-9871), which is equivalent to the
/// `if / else if` chain of libvpx's `vp8_clamp_mv` (lines 5611-5620) because
/// the bounds always satisfy `low <= -128 < 128 <= high` (see [`clamp_mv`]'s
/// formulas). The `if / else if` form is used here as the clearer of the two.
fn clamp_component(v: i16, low: i32, high: i32) -> i16 {
    let x = i32::from(v);
    let clamped = if x < low {
        low
    } else if x > high {
        high
    } else {
        x
    };
    match i16::try_from(clamped) {
        Ok(c) => c,
        // Unreachable while `v` is a real motion vector: the clamp only
        // bites when `v` is outside the bounds, and `v` is an `i16`, so a
        // bound beyond `i16` range can never be selected. Saturate rather
        // than wrap if a caller ever passes nonsense.
        Err(_) => {
            if clamped < 0 {
                i16::MIN
            } else {
                i16::MAX
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // The eighth-pel component split, as a test-only inherent impl: the
    // production consumers ([`super::super::mc`], [`super::super::inter`])
    // work on raw `(i16, i16)` components, so these accessors exist to state
    // the RFC's rounding contract (floor shift, non-negative phase) in the
    // tests that pin it, rather than as an unused production API.
    impl MotionVector {
        const fn new(row: i16, col: i16) -> Self {
            Self { row, col }
        }

        const fn zero() -> Self {
            Self { row: 0, col: 0 }
        }

        const fn is_zero(self) -> bool {
            self.row == 0 && self.col == 0
        }

        /// Whole-pixel part of the vertical displacement, `row >> 3`
        /// (rfc6386.txt line 12064).
        const fn row_int(self) -> i32 {
            (self.row >> 3) as i32
        }

        /// Whole-pixel part of the horizontal displacement, `col >> 3`.
        const fn col_int(self) -> i32 {
            (self.col >> 3) as i32
        }

        /// Eighth-pel interpolation phase of the vertical displacement,
        /// `row & 7`, always `0..=7` (rfc6386.txt line 6424).
        const fn row_frac(self) -> usize {
            (self.row & 7) as usize
        }

        /// Eighth-pel interpolation phase of the horizontal displacement.
        const fn col_frac(self) -> usize {
            (self.col & 7) as usize
        }
    }

    use super::*;
    use crate::vp8::dec::tables_inter::{DEFAULT_MV_CONTEXT, MVNUM_SHORT};
    use crate::vp8::dec::testutil::TestBoolEncoder;

    /// All-128 probabilities, so that `write_flag` (probability 128) is
    /// exactly `write_bool(probs[i], _)` for every position. Used by the
    /// hand-written bit-order vectors, which must not go through
    /// `write_mv_component`.
    const FLAT_PROBS: [u8; MV_PROB_CNT] = [128; MV_PROB_CNT];

    /// Deterministic probability generator (Numerical Recipes LCG), so the
    /// "random" prob sets are reproducible and need no `rand` dependency.
    /// Every probability is forced into `1..=255`: 0 is not a legal VP8
    /// probability.
    fn lcg_prob_sets(seed: u32) -> [[u8; MV_PROB_CNT]; 2] {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let p = ((state >> 16) & 0xFF) as u8;
            p.max(1)
        };
        let mut sets = [[128u8; MV_PROB_CNT]; 2];
        for set in &mut sets {
            for p in set.iter_mut() {
                *p = next();
            }
        }
        sets
    }

    /// Encodes `flags` as raw probability-128 bools, appends a 16-bit
    /// sentinel, then decodes one MV component and the sentinel.
    ///
    /// The sentinel is what makes these vectors sharp: a decoder that reads
    /// one bool too many (or too few) can still return a plausible
    /// magnitude, but it cannot leave the stream in sync.
    fn decode_hand_written(flags: &[bool]) -> (i32, u32) {
        const SENTINEL: u32 = 0xBEEF;
        let mut e = TestBoolEncoder::new();
        for &f in flags {
            e.write_flag(f);
        }
        e.write_literal(SENTINEL, 16);
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        let value = read_mv_component(&mut d, &FLAT_PROBS).expect("component decode is infallible");
        (value, d.get_literal(16))
    }

    // -----------------------------------------------------------------
    // Unit convention
    // -----------------------------------------------------------------

    #[test]
    fn test_motion_vector_is_eighth_pel_with_floor_split() {
        // 11 eighth-pels = 1 whole pixel + 3/8.
        let mv = MotionVector::new(11, -1);
        assert_eq!(mv.row_int(), 1);
        assert_eq!(mv.row_frac(), 3);
        // -1 eighth-pel = one pixel back plus 7/8 forward (arithmetic
        // shift; rfc6386.txt line 12064).
        assert_eq!(mv.col_int(), -1);
        assert_eq!(mv.col_frac(), 7);

        // Whole-pixel vectors have phase 0.
        let whole = MotionVector::new(-16, 24);
        assert_eq!((whole.row_int(), whole.row_frac()), (-2, 0));
        assert_eq!((whole.col_int(), whole.col_frac()), (3, 0));

        assert!(MotionVector::zero().is_zero());
        assert_eq!(MotionVector::default(), MotionVector::zero());
        assert!(!MotionVector::new(0, 1).is_zero());

        // The reconstruction identity every user of these accessors relies
        // on: int * 8 + frac == the stored component, for every phase.
        for v in -2046..=2046i16 {
            let mv = MotionVector::new(v, v);
            assert_eq!(mv.row_int() * 8 + mv.row_frac() as i32, i32::from(v));
            assert!(mv.row_frac() < 8);
        }
    }

    // -----------------------------------------------------------------
    // Hand-written bit sequences: these, not the round-trips, are what
    // pin the absolute bit order against the RFC.
    // -----------------------------------------------------------------

    #[test]
    fn test_long_path_bit_order_hand_vectors() {
        // Flag layout (rfc6386.txt lines 6113-6132), all at probability
        // 128: [is_long] [bit0 bit1 bit2] [bit9 bit8 bit7 bit6 bit5 bit4]
        // [bit3 -- only if bits 4..9 are not all zero] [sign -- only if
        // the magnitude is non-zero].
        const T: bool = true;
        const F: bool = false;

        // (a) bit0 set, no high bits -> A = 1, `A & 0xfff0 == 0`, so bit 3
        //     is implicit: A += 8 = 9. The very next flag is the SIGN, so
        //     a decoder that reads bit 3 unconditionally eats it and
        //     desynchronises the sentinel.
        assert_eq!(
            decode_hand_written(&[T, T, F, F, F, F, F, F, F, F, F]),
            (9, 0xBEEF)
        );
        // Same bits, sign set.
        assert_eq!(
            decode_hand_written(&[T, T, F, F, F, F, F, F, F, F, T]),
            (-9, 0xBEEF)
        );

        // (b) low bits 0, bit4 set (last of the descending run) -> A = 16;
        //     now `A & 0xfff0 != 0`, so bit 3 IS read: 0 here -> 16.
        assert_eq!(
            decode_hand_written(&[T, F, F, F, F, F, F, F, F, T, F, F]),
            (16, 0xBEEF)
        );
        // (c) ... and 1 there -> 24.
        assert_eq!(
            decode_hand_written(&[T, F, F, F, F, F, F, F, F, T, T, F]),
            (24, 0xBEEF)
        );

        // (d) bit9 is the FIRST flag of the descending run. If the high
        //     bits were read ascending (4..9) this would decode as 16+8=24
        //     instead of 512+8=520.
        assert_eq!(
            decode_hand_written(&[T, F, F, F, T, F, F, F, F, F, T, F]),
            (520, 0xBEEF)
        );

        // (e) bit2 is the LAST flag of the ascending low run. If the low
        //     bits were read MSB-first this would decode as 1+8=9 instead
        //     of 4+8=12.
        assert_eq!(
            decode_hand_written(&[T, F, F, T, F, F, F, F, F, F, F]),
            (12, 0xBEEF)
        );

        // (f) All low bits, no high bits -> 7 + implicit 8 = 15, the
        //     largest value whose bit 3 is not transmitted.
        assert_eq!(
            decode_hand_written(&[T, T, T, T, F, F, F, F, F, F, F]),
            (15, 0xBEEF)
        );

        // (g) Everything set -> 7 + 1008 + 8 = 1023, the maximum
        //     (rfc6386.txt line 6041), negated.
        assert_eq!(
            decode_hand_written(&[T, T, T, T, T, T, T, T, T, T, T, T]),
            (-1023, 0xBEEF)
        );
    }

    #[test]
    fn test_short_path_leaf_mapping_hand_vectors() {
        // [is_long = false] then three tree flags, MSB first, per the leaf
        // comments in `small_mvtree` (rfc6386.txt lines 6090-6094:
        // "0 = 000", "1 = 001", "2 = 010", ... "7 = 111").
        const T: bool = true;
        const F: bool = false;

        assert_eq!(decode_hand_written(&[F, F, F, F]), (0, 0xBEEF));
        assert_eq!(decode_hand_written(&[F, F, F, T, F]), (1, 0xBEEF));
        assert_eq!(decode_hand_written(&[F, F, T, F, F]), (2, 0xBEEF));
        assert_eq!(decode_hand_written(&[F, T, F, T, F]), (5, 0xBEEF));
        assert_eq!(decode_hand_written(&[F, T, T, T, F]), (7, 0xBEEF));
        // Sign applies to the short form too.
        assert_eq!(decode_hand_written(&[F, T, T, T, T]), (-7, 0xBEEF));
        assert_eq!(decode_hand_written(&[F, F, F, T, T]), (-1, 0xBEEF));
    }

    #[test]
    fn test_zero_magnitude_consumes_no_sign_bit() {
        // The short-form encoding of 0 is [is_long=false, 0, 0, 0] and
        // nothing else (rfc6386.txt line 6137: `A && read_bool(...)`).
        let sequence = [false, false, false, false];

        // 1. The sentinel proves no extra bool was consumed.
        assert_eq!(decode_hand_written(&sequence), (0, 0xBEEF));

        // 2. Byte-position cross-check via `BoolDecoder::position()`:
        //    `read_mv_component` must leave the decoder exactly where a
        //    manual "is_long bool + short tree" read leaves it, and the
        //    two must then agree on a long run of following flags.
        let mut e = TestBoolEncoder::new();
        for &f in &sequence {
            e.write_flag(f);
        }
        let tail: Vec<bool> = (0..64).map(|i| (i * 7 + 3) % 5 < 2).collect();
        for &f in &tail {
            e.write_flag(f);
        }
        let bytes = e.finish();

        let mut via_fn = BoolDecoder::new(&bytes);
        let value = read_mv_component(&mut via_fn, &FLAT_PROBS).expect("infallible");
        assert_eq!(value, 0);

        let mut manual = BoolDecoder::new(&bytes);
        assert!(!manual.get_bool(FLAT_PROBS[MVPIS_SHORT]));
        assert_eq!(manual.read_tree(&SMALL_MVTREE, &FLAT_PROBS[MVPSHORT..]), 0);

        assert_eq!(
            via_fn.position(),
            manual.position(),
            "decoding a zero component must consume exactly the is_long \
             bool plus the short tree -- no sign bool"
        );
        for (i, &expected) in tail.iter().enumerate() {
            assert_eq!(via_fn.get_flag(), expected, "tail flag {i} desynchronised");
            assert_eq!(manual.get_flag(), expected, "manual tail flag {i}");
        }
        assert_eq!(via_fn.position(), manual.position());
    }

    #[test]
    fn test_short_long_boundary_at_seven_and_eight() {
        // 7 is the largest short-form magnitude, 8 the smallest long-form
        // one (rfc6386.txt lines 6055-6059). The gate bool is
        // `probs[MVPIS_SHORT]`, and *true* means long.
        for (magnitude, expect_long) in [(7i32, false), (8, true), (0, false), (1023, true)] {
            let mut e = TestBoolEncoder::new();
            e.write_mv_component(&DEFAULT_MV_CONTEXT[0], magnitude);
            let bytes = e.finish();

            let mut gate = BoolDecoder::new(&bytes);
            assert_eq!(
                gate.get_bool(DEFAULT_MV_CONTEXT[0][MVPIS_SHORT]),
                expect_long,
                "magnitude {magnitude} must use the {} form",
                if expect_long { "long" } else { "short" }
            );

            let mut d = BoolDecoder::new(&bytes);
            assert_eq!(
                read_mv_component(&mut d, &DEFAULT_MV_CONTEXT[0]).expect("infallible"),
                magnitude
            );
        }

        // The short tree covers exactly 0..=MVNUM_SHORT-1.
        assert_eq!(MVNUM_SHORT, 8);
    }

    #[test]
    fn test_conditional_bit3_edge_around_the_implicit_range() {
        // 8..=15 have no high bits, so bit 3 is implicit; 16..=23 do, so
        // it is explicit and zero; 24..=31 explicit and one. Round-tripping
        // every value across that boundary exercises both sides of
        // `!(A & 0xfff0)` (rfc6386.txt line 6131).
        for magnitude in 0..=32i32 {
            for value in [magnitude, -magnitude] {
                let mut e = TestBoolEncoder::new();
                e.write_mv_component(&DEFAULT_MV_CONTEXT[1], value);
                let bytes = e.finish();
                let mut d = BoolDecoder::new(&bytes);
                assert_eq!(
                    read_mv_component(&mut d, &DEFAULT_MV_CONTEXT[1]).expect("infallible"),
                    value,
                    "round-trip failed for {value}"
                );
            }
        }
    }

    // -----------------------------------------------------------------
    // Exhaustive and pseudo-random round-trips
    // -----------------------------------------------------------------

    /// Encodes every component value in `-1023..=1023` into one stream and
    /// decodes them back in order, so continuation across symbols is
    /// exercised as well as each symbol in isolation.
    fn assert_full_range_roundtrip(probs: &[u8; MV_PROB_CNT], label: &str) {
        let values: Vec<i32> = (-1023..=1023).collect();
        let mut e = TestBoolEncoder::new();
        for &v in &values {
            e.write_mv_component(probs, v);
        }
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        for &v in &values {
            let got = read_mv_component(&mut d, probs).expect("infallible");
            assert_eq!(got, v, "{label}: decoded {got}, expected {v}");
        }
    }

    #[test]
    fn test_exhaustive_roundtrip_under_default_context() {
        // Both components, both signs, magnitudes 0..=1023
        // (rfc6386.txt lines 6040-6041).
        assert_full_range_roundtrip(&DEFAULT_MV_CONTEXT[0], "default row context");
        assert_full_range_roundtrip(&DEFAULT_MV_CONTEXT[1], "default column context");
    }

    #[test]
    fn test_exhaustive_roundtrip_under_pseudorandom_prob_sets() {
        for seed in 0..20u32 {
            let sets = lcg_prob_sets(seed.wrapping_mul(2_654_435_761).wrapping_add(1));
            for (component, probs) in sets.iter().enumerate() {
                assert!(
                    probs.iter().all(|&p| p >= 1),
                    "probabilities must be 1..=255"
                );
                assert_full_range_roundtrip(probs, &format!("seed {seed} component {component}"));
            }
        }
    }

    #[test]
    fn test_long_form_covers_exactly_eight_to_1023() {
        // Every long-form bit pattern must decode into 8..=1023, and every
        // magnitude in that range must be reachable -- the property that
        // makes `read_mv_component`'s infallible signature honest.
        let mut seen = vec![false; 1024];
        for magnitude in 8..=1023i32 {
            let mut e = TestBoolEncoder::new();
            e.write_mv_component(&DEFAULT_MV_CONTEXT[0], magnitude);
            let bytes = e.finish();
            let mut d = BoolDecoder::new(&bytes);
            assert!(d.get_bool(DEFAULT_MV_CONTEXT[0][MVPIS_SHORT]), "long form");
            let mut d = BoolDecoder::new(&bytes);
            let got = read_mv_component(&mut d, &DEFAULT_MV_CONTEXT[0]).expect("infallible");
            assert!((8..=1023).contains(&got));
            seen[got as usize] = true;
        }
        assert!(seen[8..=1023].iter().all(|&s| s), "8..=1023 all reachable");
    }

    // -----------------------------------------------------------------
    // read_mv: ordering and scaling
    // -----------------------------------------------------------------

    #[test]
    fn test_read_mv_reads_row_before_column() {
        // Asymmetric values, and asymmetric probability rows: swapping the
        // order would also swap which probability row is used, so this
        // catches both halves of the ordering contract at once
        // (rfc6386.txt lines 6213, 6227-6232).
        let mut e = TestBoolEncoder::new();
        e.write_mv(&DEFAULT_MV_CONTEXT, 13, -300);
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        let mv = read_mv(&mut d, &DEFAULT_MV_CONTEXT).expect("infallible");
        assert_eq!(mv.row, 26, "row must be the first component, doubled");
        assert_eq!(mv.col, -600, "column must be the second component, doubled");

        // Feeding the same bytes with the rows swapped must NOT reproduce
        // the vector -- proof the two rows are not interchangeable here.
        let swapped = [DEFAULT_MV_CONTEXT[1], DEFAULT_MV_CONTEXT[0]];
        let mut d = BoolDecoder::new(&bytes);
        let wrong = read_mv(&mut d, &swapped).expect("infallible");
        assert_ne!(
            wrong, mv,
            "row/column probability rows must differ in effect"
        );
    }

    #[test]
    fn test_read_mv_scales_both_components_by_two() {
        // rfc6386.txt lines 6306-6311: stored luma vectors are doubled,
        // each component an even integer in -2046..=2046.
        for (row, col) in [
            (0i32, 0i32),
            (1, -1),
            (7, 8),
            (-1023, 1023),
            (1023, -1023),
            (512, -7),
        ] {
            let mut e = TestBoolEncoder::new();
            e.write_mv(&DEFAULT_MV_CONTEXT, row, col);
            let bytes = e.finish();

            let mut d = BoolDecoder::new(&bytes);
            let mv = read_mv(&mut d, &DEFAULT_MV_CONTEXT).expect("infallible");
            assert_eq!(i32::from(mv.row), row * 2);
            assert_eq!(i32::from(mv.col), col * 2);
            assert_eq!(mv.row % 2, 0, "luma components are always even");
            assert_eq!(mv.col % 2, 0);
            assert!((-2046..=2046).contains(&mv.row));
            assert!((-2046..=2046).contains(&mv.col));
            // Doubled quarter-pels are eighth-pels, so the phase of a luma
            // vector is always even.
            assert_eq!(mv.row_frac() % 2, 0);
            assert_eq!(mv.col_frac() % 2, 0);
        }
    }

    #[test]
    fn test_read_mv_zero_vector_consumes_two_gate_and_tree_symbols_only() {
        let mut e = TestBoolEncoder::new();
        e.write_mv(&DEFAULT_MV_CONTEXT, 0, 0);
        e.write_literal(0xCAFE, 16);
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        let mv = read_mv(&mut d, &DEFAULT_MV_CONTEXT).expect("infallible");
        assert!(mv.is_zero());
        assert_eq!(d.get_literal(16), 0xCAFE, "neither component read a sign");
    }

    #[test]
    fn test_read_mv_accepts_the_live_entropy_context_directly() {
        // P6 seam check: `state.entropy.mv_probs` is declared `[[u8; 19];
        // 2]` and this module's signatures say `[[u8; MV_PROB_CNT]; 2]`.
        // Passing the live field proves the two spellings are the same
        // type, so integration needs no conversion -- and that a
        // header-coded probability update is actually honoured by the
        // decoder rather than shadowed by the defaults.
        let mut state = crate::vp8::dec::state::Vp8State::new();
        assert_eq!(state.entropy.mv_probs, DEFAULT_MV_CONTEXT);

        // Retune one position on each component, exactly as an inter-frame
        // header update would (RFC 6386 §17.2 `update_mvcontexts`).
        state.entropy.mv_probs[0][MVPIS_SHORT] = 3;
        state.entropy.mv_probs[1][MVPSIGN] = 250;

        let mut e = TestBoolEncoder::new();
        e.write_mv(&state.entropy.mv_probs, -400, 6);
        let bytes = e.finish();

        let mut d = BoolDecoder::new(&bytes);
        let mv = read_mv(&mut d, &state.entropy.mv_probs).expect("infallible");
        assert_eq!((mv.row, mv.col), (-800, 12));

        // The updated probabilities are load-bearing: decoding the same
        // bytes with the RFC defaults must not reproduce the vector.
        let mut d = BoolDecoder::new(&bytes);
        let stale = read_mv(&mut d, &DEFAULT_MV_CONTEXT).expect("infallible");
        assert_ne!(stale, mv, "updated MV probabilities must affect decoding");
    }

    // -----------------------------------------------------------------
    // clamp_mv
    // -----------------------------------------------------------------

    /// The bounds a caller at macroblock (row 0, col 0) of a 4x3-macroblock
    /// frame would pass (rfc6386.txt lines 10605-10609).
    const TO_LEFT: i32 = -128; // -((0 + 1) << 7)
    const TO_RIGHT: i32 = 512; // ((4 - 0) << 7)
    const TO_TOP: i32 = -128; // -((0 + 1) << 7)
    const TO_BOTTOM: i32 = 384; // ((3 - 0) << 7)

    fn clamp(mv: MotionVector) -> MotionVector {
        clamp_mv(mv, TO_LEFT, TO_RIGHT, TO_TOP, TO_BOTTOM)
    }

    #[test]
    fn test_clamp_mv_leaves_vectors_inside_the_bounds_untouched() {
        for mv in [
            MotionVector::zero(),
            // On the low edges, then on the high edges.
            MotionVector::new(TO_TOP as i16, TO_LEFT as i16),
            MotionVector::new(TO_BOTTOM as i16, TO_RIGHT as i16),
            MotionVector::new(-127, 511),
            MotionVector::new(1, -1),
        ] {
            assert_eq!(clamp(mv), mv, "{mv:?} is inside the bounds");
        }
    }

    #[test]
    fn test_clamp_mv_clamps_each_edge_independently() {
        // Past the left edge only.
        assert_eq!(
            clamp(MotionVector::new(0, -129)),
            MotionVector::new(0, TO_LEFT as i16)
        );
        // Past the right edge only.
        assert_eq!(
            clamp(MotionVector::new(0, 513)),
            MotionVector::new(0, TO_RIGHT as i16)
        );
        // Past the top edge only.
        assert_eq!(
            clamp(MotionVector::new(-129, 0)),
            MotionVector::new(TO_TOP as i16, 0)
        );
        // Past the bottom edge only.
        assert_eq!(
            clamp(MotionVector::new(385, 0)),
            MotionVector::new(TO_BOTTOM as i16, 0)
        );
        // Both axes at once, and in opposite directions, to prove the
        // row/column arguments are not transposed: `mb_to_left` /
        // `mb_to_right` bound the COLUMN and `mb_to_top` / `mb_to_bottom`
        // bound the ROW.
        assert_eq!(
            clamp(MotionVector::new(-2046, 2046)),
            MotionVector::new(TO_TOP as i16, TO_RIGHT as i16)
        );
        assert_eq!(
            clamp(MotionVector::new(2046, -2046)),
            MotionVector::new(TO_BOTTOM as i16, TO_LEFT as i16)
        );
    }

    #[test]
    fn test_clamp_mv_matches_the_dixie_formulation() {
        // dixie tests the second bound against the *unclamped* input
        // (rfc6386.txt lines 9868-9875); assert the equivalence over the
        // whole i16 component range for a representative bound set.
        for v in (-2046..=2046i16).step_by(3) {
            let mv = MotionVector::new(v, v);
            let got = clamp(mv);

            let x = i32::from(v);
            let mut dixie_col = if x < TO_LEFT { TO_LEFT } else { x };
            dixie_col = if x > TO_RIGHT { TO_RIGHT } else { dixie_col };
            let mut dixie_row = if x < TO_TOP { TO_TOP } else { x };
            dixie_row = if x > TO_BOTTOM { TO_BOTTOM } else { dixie_row };

            assert_eq!(i32::from(got.col), dixie_col, "column clamp of {v}");
            assert_eq!(i32::from(got.row), dixie_row, "row clamp of {v}");
        }
    }

    #[test]
    fn test_clamp_mv_saturates_rather_than_wraps_on_absurd_bounds() {
        // Defensive: an i32 bound outside i16 range can never be selected
        // for a real vector, but must not wrap if one is supplied.
        let mv = MotionVector::new(0, 0);
        assert_eq!(
            clamp_mv(mv, 1_000_000, 2_000_000, -2_000_000, -1_000_000).col,
            i16::MAX
        );
        assert_eq!(
            clamp_mv(mv, 1_000_000, 2_000_000, -2_000_000, -1_000_000).row,
            i16::MIN
        );
    }
}

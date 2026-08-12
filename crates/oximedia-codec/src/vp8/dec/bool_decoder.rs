//! VP8 boolean entropy decoder (RFC 6386 §7).
//!
//! The VP8 bitstream is coded with a binary arithmetic (range) coder. Every
//! decoding decision is a single boolean drawn against an 8-bit probability
//! `prob` in the range `[1, 255]`, where `prob` is the (scaled) probability of
//! the boolean being `false` (0).
//!
//! The decoder maintains:
//! - `range`: the current size of the coding interval, always in `[128, 255]`,
//! - `value`: the top bits of the compressed input shifted into a register,
//! - `bit_count`: how many more bits may be consumed from `value` before a
//!   refill is required.
//!
//! This is a faithful, allocation-free implementation of the algorithm in
//! RFC 6386 §7.3 ("Boolean Entropy Decoder pseudo-code").
//!
//! Ported from the production-verified `oximedia-image` `webp/vp8` decoder
//! (same workspace; a WebP lossy frame is a VP8 key frame).

/// Boolean (range / arithmetic) decoder over a VP8 bit partition.
pub struct BoolDecoder<'a> {
    /// Compressed input bytes for this partition.
    input: &'a [u8],
    /// Index of the next byte to be loaded from `input`.
    pos: usize,
    /// Current coding range, kept within `[128, 255]` after normalisation.
    range: u32,
    /// Value register holding bits of the compressed stream.
    value: u32,
    /// Number of valid bits remaining in `value` before a refill is needed.
    bit_count: i32,
}

impl<'a> BoolDecoder<'a> {
    /// Creates a new decoder over `input`, priming the value register.
    ///
    /// RFC 6386 §7.3: initialisation loads the first two bytes into the value
    /// register and sets the range to 255.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        let mut dec = Self {
            input,
            pos: 0,
            range: 255,
            value: 0,
            bit_count: 0,
        };
        // Load the first two bytes (16 bits) into the high part of `value`.
        dec.value = (u32::from(dec.next_byte()) << 8) | u32::from(dec.next_byte());
        dec.bit_count = 0;
        dec
    }

    /// Returns the number of input bytes consumed so far (including the two
    /// bytes the value register was primed with at construction).
    ///
    /// Used by header-parsing tests to confirm a header parse consumed a
    /// plausible number of bytes from its partition (RFC 6386 §7.3: the
    /// decoder may read up to two bytes past the true end of a partition
    /// without erroring, so an out-of-bounds field-order bug would silently
    /// decode garbage rather than fail outright — this accessor lets tests
    /// catch that by cross-checking against the known partition size).
    #[must_use]
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// Returns the next input byte, or 0 once the partition is exhausted.
    ///
    /// VP8 permits reading past the end of a partition; the trailing bits are
    /// defined to be zero. This matches libvpx behaviour.
    fn next_byte(&mut self) -> u8 {
        let b = self.input.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    /// Decodes one boolean against probability `prob` (probability of `false`).
    ///
    /// Implements the core split / renormalisation loop from RFC 6386 §7.3.
    pub fn get_bool(&mut self, prob: u8) -> bool {
        // split is the size of the "0" sub-interval.
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        let big_split = split << 8;

        let retval;
        if self.value >= big_split {
            // The coded symbol is 1.
            retval = true;
            self.range -= split;
            self.value -= big_split;
        } else {
            // The coded symbol is 0.
            retval = false;
            self.range = split;
        }

        // Renormalise: shift `range` back into [128, 255], pulling fresh bits
        // into `value` as needed.
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                self.value |= u32::from(self.next_byte());
            }
        }
        retval
    }

    /// Decodes a literal of `num_bits` bits, each with probability 128 (1/2).
    ///
    /// Bits are read most-significant-first, matching RFC 6386 `read_literal`.
    pub fn get_literal(&mut self, num_bits: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..num_bits {
            v = (v << 1) | u32::from(self.get_bool(128));
        }
        v
    }

    /// Decodes an unsigned literal followed by a sign flag (RFC 6386 §9.2).
    ///
    /// Returns a signed magnitude: the magnitude is `num_bits` wide and the
    /// final flag negates it.
    pub fn get_signed_literal(&mut self, num_bits: u32) -> i32 {
        let magnitude = self.get_literal(num_bits) as i32;
        if self.get_bool(128) {
            -magnitude
        } else {
            magnitude
        }
    }

    /// Decodes a single equiprobable flag (probability 128).
    pub fn get_flag(&mut self) -> bool {
        self.get_bool(128)
    }

    /// Walks a VP8 token tree, returning the decoded leaf value.
    ///
    /// `tree` is a flat array of `i8` pairs: each internal node `i` has its two
    /// children at `tree[i]` and `tree[i + 1]`. A non-positive entry is the
    /// negation of a leaf value; a positive entry is the index of a child
    /// node. `probs` supplies the probability for each internal node (indexed
    /// by `i >> 1`). This is the generic `treed_read` of RFC 6386 §8.
    pub fn read_tree(&mut self, tree: &[i8], probs: &[u8]) -> i32 {
        self.read_tree_from(tree, probs, 0)
    }

    /// Walks a token tree starting at a given node index `start`.
    ///
    /// Used by the coefficient decoder, which re-enters the DCT token tree at a
    /// non-root position after a previous token implied a known prefix.
    pub fn read_tree_from(&mut self, tree: &[i8], probs: &[u8], start: i32) -> i32 {
        let mut i: i32 = start;
        loop {
            let prob_index = (i >> 1) as usize;
            let prob = probs[prob_index];
            let branch = usize::from(self.get_bool(prob));
            let next = tree[i as usize + branch];
            if next <= 0 {
                return i32::from(-next);
            }
            i = i32::from(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_does_not_panic_on_empty() {
        let dec = BoolDecoder::new(&[]);
        assert_eq!(dec.range, 255);
    }

    #[test]
    fn test_get_bool_prob_128_balanced() {
        // With all-zero input every decoded bool with prob 128 must be 0
        // because value stays below big_split.
        let data = [0u8; 16];
        let mut dec = BoolDecoder::new(&data);
        for _ in 0..32 {
            assert!(!dec.get_bool(128));
        }
    }

    #[test]
    fn test_get_bool_prob_1_mostly_one() {
        // prob = 1 means "false" is extremely unlikely; with 0xFF input the
        // value register is large, so booleans decode to 1.
        let data = [0xFFu8; 16];
        let mut dec = BoolDecoder::new(&data);
        let mut ones = 0;
        for _ in 0..32 {
            if dec.get_bool(1) {
                ones += 1;
            }
        }
        assert!(ones > 16, "expected mostly ones, got {ones}");
    }

    #[test]
    fn test_get_literal_zero_input() {
        let data = [0u8; 8];
        let mut dec = BoolDecoder::new(&data);
        assert_eq!(dec.get_literal(8), 0);
    }

    #[test]
    fn test_read_tree_leaf() {
        // Minimal 2-leaf tree: node 0 -> leaf -1 (value 1) / leaf -2 (value 2).
        let tree: [i8; 2] = [-1, -2];
        let probs = [128u8];
        let data = [0u8; 8];
        let mut dec = BoolDecoder::new(&data);
        // Zero input -> branch 0 -> leaf -(-1) = 1.
        assert_eq!(dec.read_tree(&tree, &probs), 1);
    }

    #[test]
    fn test_range_stays_normalised() {
        let data = [0x5Au8; 32];
        let mut dec = BoolDecoder::new(&data);
        for _ in 0..100 {
            let _ = dec.get_bool(120);
            assert!((128..=255).contains(&dec.range));
        }
    }
}

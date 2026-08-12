//! VP9 boolean (range) decoder — exact port of libvpx `vpx_dsp/bitreader.{h,c}`.
//!
//! VP9 bool-coded partitions (the compressed header and each tile) begin
//! with a marker bit that must decode to 0 (libvpx `vpx_reader_init`,
//! mirrored by ffmpeg's `vp9.c` marker-bit check).
//!
//! Reads past the end of the buffer are well-defined (they behave as if the
//! stream were followed by zero bits, exactly like libvpx); the error state
//! is checked once at the end of a partition via [`BoolReader::has_error`].

/// Size of the value accumulator in bits (libvpx `BD_VALUE_SIZE` with a
/// 64-bit `BD_VALUE`).
const BD_VALUE_SIZE: i32 = 64;

/// Marker added to `count` once input is exhausted (libvpx `LOTS_OF_BITS`).
const LOTS_OF_BITS: i32 = 0x4000_0000;

/// VP9 boolean decoder over a byte slice.
#[derive(Debug)]
pub struct BoolReader<'a> {
    data: &'a [u8],
    pos: usize,
    value: u64,
    range: u32,
    count: i32,
}

impl<'a> BoolReader<'a> {
    /// Initializes the decoder and consumes the VP9 marker bit.
    ///
    /// Returns `None` when the marker bit is set (corrupt stream), matching
    /// libvpx `vpx_reader_init` / ffmpeg's explicit marker check.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Option<Self> {
        let mut r = Self {
            data,
            pos: 0,
            value: 0,
            range: 255,
            count: -8,
        };
        r.fill();
        if r.read_bit() {
            return None; // marker bit must be zero
        }
        Some(r)
    }

    /// Refills the value accumulator (port of `vpx_reader_fill`).
    fn fill(&mut self) {
        let mut shift = BD_VALUE_SIZE - 8 - (self.count + 8);
        let bits_left = ((self.data.len() - self.pos) * 8) as i64;
        let bits_over = i64::from(shift) + 8 - bits_left;
        let mut loop_end: i64 = 0;

        if bits_over >= 0 {
            self.count += LOTS_OF_BITS;
            loop_end = bits_over;
        }

        if bits_over < 0 || bits_left > 0 {
            while i64::from(shift) >= loop_end {
                self.count += 8;
                self.value |= u64::from(self.data[self.pos]) << shift;
                self.pos += 1;
                shift -= 8;
            }
        }
    }

    /// Decodes one boolean with the given probability (port of `vpx_read`).
    #[inline]
    pub fn read_bool(&mut self, prob: u8) -> bool {
        let split = (self.range * u32::from(prob) + (256 - u32::from(prob))) >> 8;

        if self.count < 0 {
            self.fill();
        }

        let bigsplit = u64::from(split) << (BD_VALUE_SIZE - 8);
        let bit;
        if self.value >= bigsplit {
            self.range -= split;
            self.value -= bigsplit;
            bit = true;
        } else {
            self.range = split;
            bit = false;
        }

        // Normalize: `vpx_norm[range]` == leading zeros of the low byte.
        let shift = (self.range as u8).leading_zeros();
        self.range <<= shift;
        self.value <<= shift;
        self.count -= shift as i32;
        bit
    }

    /// Decodes one bit with probability 128 (port of `vpx_read_bit`).
    #[inline]
    pub fn read_bit(&mut self) -> bool {
        self.read_bool(128)
    }

    /// Decodes an `n`-bit unsigned literal, MSB first (`vpx_read_literal`).
    #[inline]
    pub fn read_literal(&mut self, bits: u32) -> u32 {
        let mut literal = 0u32;
        for bit in (0..bits).rev() {
            literal |= u32::from(self.read_bit()) << bit;
        }
        literal
    }

    /// Decodes a value from a token tree (`vpx_read_tree`).
    ///
    /// `tree` is a libvpx `vpx_tree_index` array: non-negative entries are
    /// child node offsets, negative entries are `-token` leaves.
    #[inline]
    pub fn read_tree(&mut self, tree: &[i8], probs: &[u8]) -> u8 {
        let mut i: i16 = 0;
        loop {
            let p = probs[(i >> 1) as usize];
            i = i16::from(tree[(i + i16::from(self.read_bool(p))) as usize]);
            if i <= 0 {
                return (-i) as u8;
            }
        }
    }

    /// True if reads went past the end of the buffer (`vpx_reader_has_error`).
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.count > BD_VALUE_SIZE && self.count < LOTS_OF_BITS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bit_zero_required() {
        // 0x00 first byte: marker bit (p=128) decodes to 0 -> Some
        assert!(BoolReader::new(&[0x00, 0x00]).is_some());
        // 0xFF first byte: marker bit decodes to 1 -> None
        assert!(BoolReader::new(&[0xFF, 0x00]).is_none());
    }

    #[test]
    fn literal_roundtrip_against_known_stream() {
        // All-zero stream decodes literals as... deterministic values; just
        // exercise that reads are stable and error flag stays clear while
        // within the buffer.
        let data = [0x00u8; 16];
        let mut r = BoolReader::new(&data).expect("marker");
        let v = r.read_literal(8);
        assert_eq!(v, 0);
        assert!(!r.has_error());
    }

    #[test]
    fn error_flag_after_overread() {
        let data = [0x00u8; 1];
        let mut r = BoolReader::new(&data).expect("marker");
        for _ in 0..128 {
            let _ = r.read_bit();
        }
        assert!(r.has_error());
    }
}

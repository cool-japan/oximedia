//! Entropy-coded segment reading for baseline JPEG scans.
//!
//! Two pieces live here:
//!
//! * [`HuffDecodeTable`] — the canonical Huffman decoding tables of T.81
//!   §F.2.2.3 (`MINCODE`/`MAXCODE`/`VALPTR`), built once per `DHT` segment.
//!   The previous implementation rebuilt a `HashMap` per component per MCU,
//!   which made a 0.5 Mpx photo take seconds.
//! * [`ScanReader`] — an MSB-first bit reader that performs `0xFF 0x00`
//!   de-stuffing inline (T.81 §B.1.1.5) and stops at a marker instead of
//!   consuming it, so restart-interval resynchronisation stays possible.

use crate::error::{ImageError, ImageResult};

/// Bytes of 1-bit padding an entropy segment may consume past its end.
///
/// A conforming encoder pads the *final byte* of a segment with 1-bits
/// (T.81 §F.1.2.3), so a well-formed stream needs no padding bytes at all.
/// Tolerating a couple of them absorbs sloppy encoders; refusing more is what
/// stops a truncated or mis-parsed stream from being decoded into a plausible
/// looking image.
const MAX_PADDING_BYTES: u32 = 2;

/// A canonical Huffman decoding table (T.81 §F.2.2.3).
pub(super) struct HuffDecodeTable {
    /// Smallest code of each length 1..=16.
    min_code: [i32; 17],
    /// Largest code of each length 1..=16, or -1 when no code has that length.
    max_code: [i32; 17],
    /// Index into `values` of the first symbol of each length.
    val_ptr: [usize; 17],
    /// Symbols in canonical (code-length) order.
    values: Vec<u8>,
}

impl HuffDecodeTable {
    /// Build the decoding table from a `DHT` segment's code-length counts and
    /// symbol list.
    pub(super) fn build(counts: &[u8; 16], values: Vec<u8>) -> Self {
        let mut table = Self {
            min_code: [0; 17],
            max_code: [-1; 17],
            val_ptr: [0; 17],
            values,
        };
        let mut code: i32 = 0;
        let mut value_index = 0usize;
        for length in 1..=16usize {
            let count = counts[length - 1] as usize;
            if count == 0 {
                table.max_code[length] = -1;
            } else {
                table.val_ptr[length] = value_index;
                table.min_code[length] = code;
                code += count as i32;
                table.max_code[length] = code - 1;
                value_index += count;
            }
            code <<= 1;
        }
        table
    }

    /// Decode one symbol, consuming 1..=16 bits.
    pub(super) fn decode_symbol(&self, reader: &mut ScanReader<'_>) -> ImageResult<u8> {
        let mut code: i32 = 0;
        for length in 1..=16usize {
            code = (code << 1) | i32::from(reader.read_bit()?);
            let max = self.max_code[length];
            if max >= 0 && code <= max {
                let index = self.val_ptr[length] + (code - self.min_code[length]) as usize;
                return self.values.get(index).copied().ok_or_else(|| {
                    ImageError::invalid_format("JPEG: Huffman symbol out of range")
                });
            }
        }
        Err(ImageError::invalid_format(
            "JPEG: Huffman code not found within 16 bits (corrupt entropy data)",
        ))
    }
}

/// MSB-first reader over an entropy-coded segment.
pub(super) struct ScanReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buffer: u32,
    bits_in_buffer: u32,
    /// Marker byte reached but not consumed, if any.
    hit_marker: Option<u8>,
    /// Padding bytes fabricated since the last restart.
    padding_bytes: u32,
}

impl<'a> ScanReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bits_in_buffer: 0,
            hit_marker: None,
            padding_bytes: 0,
        }
    }

    /// Next entropy byte, de-stuffing `0xFF 0x00` and stopping at markers.
    fn next_byte(&mut self) -> Option<u8> {
        loop {
            if self.pos >= self.data.len() {
                return None;
            }
            let byte = self.data[self.pos];
            if byte != 0xFF {
                self.pos += 1;
                return Some(byte);
            }
            let Some(&following) = self.data.get(self.pos + 1) else {
                // A trailing 0xFF with nothing after it: end of data.
                self.pos = self.data.len();
                return None;
            };
            match following {
                // Stuffed 0xFF data byte.
                0x00 => {
                    self.pos += 2;
                    return Some(0xFF);
                }
                // Fill byte before a marker; skip and keep looking.
                0xFF => self.pos += 1,
                // A real marker: leave it in place for `restart`/`end`.
                _ => {
                    self.hit_marker = Some(following);
                    return None;
                }
            }
        }
    }

    fn read_bit(&mut self) -> ImageResult<u8> {
        if self.bits_in_buffer == 0 {
            let byte = match self.next_byte() {
                Some(byte) => byte,
                None => {
                    self.padding_bytes += 1;
                    if self.padding_bytes > MAX_PADDING_BYTES {
                        return Err(ImageError::invalid_format(
                            "JPEG: entropy-coded data ends before the last MCU (truncated file)",
                        ));
                    }
                    // Conforming encoders pad with 1-bits.
                    0xFF
                }
            };
            self.bit_buffer = u32::from(byte);
            self.bits_in_buffer = 8;
        }
        self.bits_in_buffer -= 1;
        Ok(((self.bit_buffer >> self.bits_in_buffer) & 1) as u8)
    }

    /// Read `n` bits (`n <= 16`) MSB-first.
    pub(super) fn read_bits(&mut self, n: u8) -> ImageResult<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    /// Discard the partial byte and consume the `RST`*n* marker that closes a
    /// restart interval (T.81 §F.2.1.3.1).
    ///
    /// `expected` is the restart marker index the stream should carry next;
    /// a mismatch means the scan has desynchronised, which is reported rather
    /// than decoded around.
    pub(super) fn restart(&mut self, expected: u8) -> ImageResult<()> {
        self.bit_buffer = 0;
        self.bits_in_buffer = 0;
        self.padding_bytes = 0;
        let found = match self.hit_marker.take() {
            Some(marker) if (0xD0..=0xD7).contains(&marker) => {
                self.pos += 2;
                marker
            }
            _ => self.seek_restart_marker()?,
        };
        if found & 0x07 != expected {
            return Err(ImageError::invalid_format(format!(
                "JPEG: restart marker RST{} out of sequence (expected RST{expected})",
                found & 0x07
            )));
        }
        Ok(())
    }

    /// Scan forward to the next `RST`*n*, tolerating stuffed bytes and fill.
    fn seek_restart_marker(&mut self) -> ImageResult<u8> {
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF {
                let marker = self.data[self.pos + 1];
                if (0xD0..=0xD7).contains(&marker) {
                    self.pos += 2;
                    return Ok(marker);
                }
                if marker != 0x00 && marker != 0xFF {
                    return Err(ImageError::invalid_format(
                        "JPEG: expected a restart marker inside the scan",
                    ));
                }
            }
            self.pos += 1;
        }
        Err(ImageError::invalid_format(
            "JPEG: restart marker missing before end of scan",
        ))
    }
}

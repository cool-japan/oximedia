// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Legacy `BitRead2` compatibility trait.

use std::io;
#[cfg(feature = "alloc")]
use std::vec::Vec;

use super::{
    read_to_vec, BitRead, Endianness, FromBitStream, FromBitStreamWith, Integer, Primitive,
    SignedInteger,
};

/// A compatibility trait for older code implementing [`BitRead`]
///
/// This is a trait largely compatible with older code
/// from the 2.X.X version,
/// which one can use with a named import as needed.
///
/// New code should prefer the regular [`BitRead`] trait.
///
/// # Example
/// ```
/// use oximedia_bitstream::BitRead2 as BitRead;
/// use oximedia_bitstream::{BitReader, BigEndian};
/// let byte = &[0b1111_0000];
/// let mut reader = BitReader::endian(byte.as_slice(), BigEndian);
/// assert_eq!(reader.read::<u8>(4).unwrap(), 0b1111);
/// assert_eq!(reader.read_in::<4, u8>().unwrap(), 0b0000);
/// ```
pub trait BitRead2 {
    /// Reads a single bit from the stream.
    /// `true` indicates 1, `false` indicates 0
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_bit(&mut self) -> io::Result<bool>;

    /// Reads an unsigned value from the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Also returns an error if the output type is too small
    /// to hold the requested number of bits.
    fn read<I>(&mut self, bits: u32) -> io::Result<I>
    where
        I: Integer + Sized;

    /// Reads an unsigned value from the stream with
    /// the given constant number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    fn read_in<const BITS: u32, I>(&mut self) -> io::Result<I>
    where
        I: Integer,
    {
        self.read(BITS)
    }

    /// Reads a twos-complement signed value from the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the number of bits is 0,
    /// since one bit is always needed for the sign.
    /// Also returns an error if the output type is too small
    /// to hold the requested number of bits.
    fn read_signed<S>(&mut self, bits: u32) -> io::Result<S>
    where
        S: SignedInteger;

    /// Reads a twos-complement signed value from the stream with
    /// the given constant number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// A compile-time error occurs if the number of bits is 0,
    /// since one bit is always needed for the sign.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    fn read_signed_in<const BITS: u32, S>(&mut self) -> io::Result<S>
    where
        S: SignedInteger,
    {
        self.read_signed(BITS)
    }

    /// Reads whole value from the stream whose size in bits is equal
    /// to its type's size.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_to<V>(&mut self) -> io::Result<V>
    where
        V: Primitive;

    /// Reads whole value from the stream whose size in bits is equal
    /// to its type's size in an endianness that may be different
    /// from the stream's endianness.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_as_to<F, V>(&mut self) -> io::Result<V>
    where
        F: Endianness,
        V: Primitive;

    /// Skips the given number of bits in the stream.
    /// Since this method does not need an accumulator,
    /// it may be slightly faster than reading to an empty variable.
    /// In addition, since there is no accumulator,
    /// there is no upper limit on the number of bits
    /// which may be skipped.
    /// These bits are still read from the stream, however,
    /// and are never skipped via a `seek` method.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn skip(&mut self, bits: u32) -> io::Result<()>;

    /// Completely fills the given buffer with whole bytes.
    /// If the stream is already byte-aligned, it will map
    /// to a faster `read_exact` call.  Otherwise it will read
    /// bytes individually in 8-bit increments.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        for b in buf.iter_mut() {
            *b = self.read_in::<8, _>()?;
        }
        Ok(())
    }

    /// Completely fills a whole buffer with bytes and returns it.
    /// If the stream is already byte-aligned, it will map
    /// to a faster `read_exact` call.  Otherwise it will read
    /// bytes individually in 8-bit increments.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    #[inline(always)]
    #[deprecated(since = "1.8.0", note = "use read_to() method instead")]
    fn read_to_bytes<const SIZE: usize>(&mut self) -> io::Result<[u8; SIZE]> {
        self.read_to()
    }

    /// Completely fills a vector of bytes and returns it.
    /// If the stream is already byte-aligned, it will map
    /// to a faster `read_exact` call.  Otherwise it will read
    /// bytes individually in 8-bit increments.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    fn read_to_vec(&mut self, bytes: usize) -> io::Result<Vec<u8>> {
        read_to_vec(|buf| self.read_bytes(buf), bytes)
    }

    /// Counts the number of 1 bits in the stream until the next
    /// 0 bit and returns the amount read.
    /// Because this field is variably-sized and may be large,
    /// its output is always a `u32` type.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_unary0(&mut self) -> io::Result<u32> {
        let mut unary = 0;
        while self.read_bit()? {
            unary += 1;
        }
        Ok(unary)
    }

    /// Counts the number of 0 bits in the stream until the next
    /// 1 bit and returns the amount read.
    /// Because this field is variably-sized and may be large,
    /// its output is always a `u32` type.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_unary1(&mut self) -> io::Result<u32> {
        let mut unary = 0;
        while !(self.read_bit()?) {
            unary += 1;
        }
        Ok(unary)
    }

    /// Parses and returns complex type
    fn parse<F: FromBitStream>(&mut self) -> Result<F, F::Error>
    where
        Self: BitRead,
    {
        F::from_reader(self)
    }

    /// Parses and returns complex type with context
    fn parse_with<'a, F: FromBitStreamWith<'a>>(
        &mut self,
        context: &F::Context,
    ) -> Result<F, F::Error>
    where
        Self: BitRead,
    {
        F::from_reader(self, context)
    }

    /// Returns true if the stream is aligned at a whole byte.
    fn byte_aligned(&self) -> bool;

    /// Throws away all unread bit values until the next whole byte.
    /// Does nothing if the stream is already aligned.
    fn byte_align(&mut self);

    /// Given a compiled Huffman tree, reads bits from the stream
    /// until the next symbol is encountered.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    #[inline]
    fn read_huffman<T>(&mut self) -> io::Result<T::Symbol>
    where
        T: crate::huffman::FromBits,
    {
        T::from_bits(|| self.read_bit())
    }
}

impl<R: BitRead> BitRead2 for R {
    #[inline(always)]
    fn read_bit(&mut self) -> io::Result<bool> {
        BitRead::read_bit(self)
    }

    #[inline(always)]
    fn read<I>(&mut self, bits: u32) -> io::Result<I>
    where
        I: Integer + Sized,
    {
        self.read_var(bits)
    }

    #[inline(always)]
    fn read_in<const BITS: u32, I>(&mut self) -> io::Result<I>
    where
        I: Integer,
    {
        BitRead::read::<BITS, I>(self)
    }

    #[inline(always)]
    fn read_signed<S>(&mut self, bits: u32) -> io::Result<S>
    where
        S: SignedInteger,
    {
        self.read_signed_var(bits)
    }

    #[inline(always)]
    fn read_signed_in<const BITS: u32, S>(&mut self) -> io::Result<S>
    where
        S: SignedInteger,
    {
        BitRead::read_signed::<BITS, S>(self)
    }

    #[inline(always)]
    fn read_to<V>(&mut self) -> io::Result<V>
    where
        V: Primitive,
    {
        BitRead::read_to::<V>(self)
    }

    #[inline(always)]
    fn read_as_to<F, V>(&mut self) -> io::Result<V>
    where
        F: Endianness,
        V: Primitive,
    {
        BitRead::read_as_to::<F, V>(self)
    }

    #[inline(always)]
    fn skip(&mut self, bits: u32) -> io::Result<()> {
        BitRead::skip(self, bits)
    }

    #[inline(always)]
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        BitRead::read_bytes(self, buf)
    }

    #[inline(always)]
    fn read_unary0(&mut self) -> io::Result<u32> {
        self.read_unary::<0>()
    }

    #[inline(always)]
    fn read_unary1(&mut self) -> io::Result<u32> {
        self.read_unary::<1>()
    }

    #[inline(always)]
    fn byte_aligned(&self) -> bool {
        BitRead::byte_aligned(self)
    }

    #[inline(always)]
    fn byte_align(&mut self) {
        BitRead::byte_align(self);
    }
}

// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Legacy `BitWrite2` compatibility trait.

use std::io;

use crate::endian::write_byte;

use super::{
    BitCount, BitWrite, BitWriter, Checkable, CheckedSigned, CheckedUnsigned, Endianness, Integer,
    Primitive, SignedBitCount, SignedInteger, ToBitStream, ToBitStreamWith, UnsignedInteger,
};

/// A compatibility trait for older code implementing [`BitWrite`]
///
/// This is a trait largely compatible with older code
/// from the 2.X.X version,
/// which one can use with a named import as needed.
///
/// New code should prefer the regular [`BitWrite`] trait.
///
/// # Example
/// ```
/// use oximedia_bitstream::BitWrite2 as BitWrite;
/// use oximedia_bitstream::{BitWriter, BigEndian};
/// let mut byte = vec![];
/// let mut writer = BitWriter::endian(byte, BigEndian);
/// writer.write::<u8>(4, 0b1111).unwrap();
/// writer.write_out::<4, u8>(0b0000).unwrap();
/// assert_eq!(writer.into_writer(), [0b1111_0000]);
/// ```
pub trait BitWrite2 {
    /// Writes a single bit to the stream.
    /// `true` indicates 1, `false` indicates 0
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        self.write_unsigned_out::<1, u8>(u8::from(bit))
    }

    /// Writes a signed or unsigned value to the stream using the given
    /// number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the input type is too small
    /// to hold the given number of bits.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    fn write<I>(&mut self, bits: u32, value: I) -> io::Result<()>
    where
        I: Integer;

    /// Writes a signed or unsigned value to the stream using the given
    /// const number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    fn write_out<const BITS: u32, I>(&mut self, value: I) -> io::Result<()>
    where
        I: Integer;

    /// Writes an unsigned value to the stream using the given
    /// number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the input type is too small
    /// to hold the given number of bits.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    fn write_unsigned<U>(&mut self, bits: u32, value: U) -> io::Result<()>
    where
        U: UnsignedInteger;

    /// Writes an unsigned value to the stream using the given
    /// const number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    #[inline]
    fn write_unsigned_out<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        self.write_unsigned(BITS, value)
    }

    /// Writes a twos-complement signed value to the stream
    /// with the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the input type is too small
    /// to hold the given number of bits.
    /// Returns an error if the number of bits is 0,
    /// since one bit is always needed for the sign.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    fn write_signed<S>(&mut self, bits: u32, value: S) -> io::Result<()>
    where
        S: SignedInteger;

    /// Writes a twos-complement signed value to the stream
    /// with the given const number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    /// A compile-time error occurs if the number of bits is 0,
    /// since one bit is always needed for the sign.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    fn write_signed_out<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        self.write_signed(BITS, value)
    }

    /// Writes whole value to the stream whose size in bits
    /// is equal to its type's size.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn write_from<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive;

    /// Writes whole value to the stream whose size in bits
    /// is equal to its type's size in an endianness that may
    /// be different from the stream's endianness.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive;

    /// Pads the stream by writing 0 over the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn pad(&mut self, mut bits: u32) -> io::Result<()> {
        loop {
            match bits {
                0 => break Ok(()),
                bits @ 1..64 => break self.write(bits, 0u64),
                _ => {
                    self.write_out::<64, u64>(0)?;
                    bits -= 64;
                }
            }
        }
    }

    /// Writes the entirety of a byte buffer to the stream.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_var(8, 0x66u8).unwrap();
    /// writer.write_var(8, 0x6Fu8).unwrap();
    /// writer.write_var(8, 0x6Fu8).unwrap();
    /// writer.write_bytes(b"bar").unwrap();
    /// assert_eq!(writer.into_writer(), b"foobar");
    /// ```
    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        buf.iter()
            .try_for_each(|b| self.write_unsigned_out::<8, _>(*b))
    }

    /// Writes `value` number of 1 bits to the stream
    /// and then writes a 0 bit.  This field is variably-sized.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    fn write_unary0(&mut self, value: u32) -> io::Result<()>;

    /// Writes `value` number of 0 bits to the stream
    /// and then writes a 1 bit.  This field is variably-sized.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    fn write_unary1(&mut self, value: u32) -> io::Result<()>;

    /// Builds and writes complex type
    fn build<T: ToBitStream>(&mut self, build: &T) -> Result<(), T::Error>
    where
        Self: BitWrite,
    {
        build.to_writer(self)
    }

    /// Builds and writes complex type with context
    fn build_with<'a, T: ToBitStreamWith<'a>>(
        &mut self,
        build: &T,
        context: &T::Context,
    ) -> Result<(), T::Error>
    where
        Self: BitWrite,
    {
        build.to_writer(self, context)
    }

    /// Returns true if the stream is aligned at a whole byte.
    fn byte_aligned(&self) -> bool;

    /// Pads the stream with 0 bits until it is aligned at a whole byte.
    /// Does nothing if the stream is already aligned.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_var(1, 0u8).unwrap();
    /// writer.byte_align().unwrap();
    /// writer.write_var(8, 0xFFu8).unwrap();
    /// assert_eq!(writer.into_writer(), [0x00, 0xFF]);
    /// ```
    fn byte_align(&mut self) -> io::Result<()> {
        while !self.byte_aligned() {
            self.write_bit(false)?;
        }
        Ok(())
    }

    /// Given a symbol, writes its representation to the output stream as bits.
    /// Generates no output if the symbol isn't defined in the Huffman tree.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite2};
    /// use oximedia_bitstream::define_huffman_tree;
    /// define_huffman_tree!(TreeName : char = ['a', ['b', ['c', 'd']]]);
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_huffman::<TreeName>('b').unwrap();
    /// writer.write_huffman::<TreeName>('c').unwrap();
    /// writer.write_huffman::<TreeName>('d').unwrap();
    /// assert_eq!(writer.into_writer(), [0b10_110_111]);
    /// ```
    fn write_huffman<T>(&mut self, value: T::Symbol) -> io::Result<()>
    where
        T: crate::huffman::ToBits,
    {
        T::to_bits(value, |b| self.write_bit(b))
    }
}

impl<W: BitWrite> BitWrite2 for W {
    #[inline]
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        BitWrite::write_bit(self, bit)
    }

    #[inline]
    fn write<I>(&mut self, bits: u32, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        BitWrite::write_var(self, bits, value)
    }

    #[inline]
    fn write_out<const BITS: u32, I>(&mut self, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        BitWrite::write::<BITS, I>(self, value)
    }

    #[inline]
    fn write_unsigned<U>(&mut self, bits: u32, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        BitWrite::write_unsigned_var::<U>(self, bits, value)
    }

    #[inline]
    fn write_unsigned_out<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        BitWrite::write_unsigned::<BITS, U>(self, value)
    }

    #[inline]
    fn write_signed<S>(&mut self, bits: u32, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        BitWrite::write_signed_var::<S>(self, bits, value)
    }

    #[inline]
    fn write_signed_out<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        BitWrite::write_signed::<BITS, S>(self, value)
    }

    #[inline]
    fn write_from<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive,
    {
        BitWrite::write_from(self, value)
    }

    #[inline]
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        BitWrite::write_as_from::<F, V>(self, value)
    }

    #[inline]
    fn pad(&mut self, bits: u32) -> io::Result<()> {
        BitWrite::pad(self, bits)
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        BitWrite::write_bytes(self, buf)
    }

    #[inline]
    fn write_unary0(&mut self, value: u32) -> io::Result<()> {
        BitWrite::write_unary::<0>(self, value)
    }

    #[inline]
    fn write_unary1(&mut self, value: u32) -> io::Result<()> {
        BitWrite::write_unary::<1>(self, value)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        BitWrite::byte_aligned(self)
    }

    #[inline]
    fn byte_align(&mut self) -> io::Result<()> {
        BitWrite::byte_align(self)
    }
}

impl<W: io::Write, E: Endianness> BitWrite for BitWriter<W, E> {
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        match E::push_bit_flush(&mut self.value, &mut self.bits, bit) {
            None => Ok(()),
            Some(byte) => write_byte(&mut self.writer, byte),
        }
    }

    #[inline(always)]
    fn write_unsigned<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        let Self {
            value: queue_value,
            bits: queue_bits,
            writer,
            ..
        } = self;

        E::write_bits_checked(
            writer,
            queue_value,
            queue_bits,
            CheckedUnsigned::<BITS, U>::new_fixed::<BITS>(value)?,
        )
    }

    fn write_unsigned_counted<const BITS: u32, U>(
        &mut self,
        count: BitCount<BITS>,
        value: U,
    ) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        let Self {
            value: queue_value,
            bits: queue_bits,
            writer,
            ..
        } = self;

        E::write_bits_checked(
            writer,
            queue_value,
            queue_bits,
            CheckedUnsigned::new(count, value)?,
        )
    }

    #[inline(always)]
    fn write_signed_counted<const BITS: u32, S>(
        &mut self,
        bits: impl TryInto<SignedBitCount<BITS>>,
        value: S,
    ) -> io::Result<()>
    where
        S: SignedInteger,
    {
        E::write_signed_bits_checked(
            &mut self.writer,
            &mut self.value,
            &mut self.bits,
            CheckedSigned::new(
                bits.try_into().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "signed writes need at least 1 bit for sign",
                    )
                })?,
                value,
            )?,
        )
    }

    #[inline]
    fn write_signed<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        E::write_signed_bits_checked(
            &mut self.writer,
            &mut self.value,
            &mut self.bits,
            CheckedSigned::<BITS, _>::new_fixed::<BITS>(value)?,
        )
    }

    #[inline]
    fn write_from<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive,
    {
        E::write_bytes::<8, _>(
            &mut self.writer,
            &mut self.value,
            self.bits,
            E::primitive_to_bytes(value).as_ref(),
        )
    }

    #[inline]
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        F::write_bytes::<8, _>(
            &mut self.writer,
            &mut self.value,
            self.bits,
            F::primitive_to_bytes(value).as_ref(),
        )
    }

    #[inline]
    fn write_checked<C: Checkable>(&mut self, value: C) -> io::Result<()> {
        value.write_endian::<E, _>(&mut self.writer, &mut self.value, &mut self.bits)
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        E::write_bytes::<1024, _>(&mut self.writer, &mut self.value, self.bits, buf)
    }

    #[inline(always)]
    fn byte_aligned(&self) -> bool {
        self.bits == 0
    }
}

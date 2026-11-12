// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! The core `BitWrite` trait and its blanket impls.

use std::io;

use super::{
    BitCount, Checkable, Endianness, Integer, Numeric, Primitive, SignedBitCount, SignedInteger,
    ToBitStream, ToBitStreamUsing, ToBitStreamWith, UnsignedInteger, VBRInteger,
};

/// A trait for anything that can write a variable number of
/// potentially un-aligned values to an output stream
pub trait BitWrite {
    /// Writes a single bit to the stream.
    /// `true` indicates 1, `false` indicates 0
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w  = BitWriter::endian(vec![], BigEndian);
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert_eq!(w.into_writer(), &[0b1000_1110]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, LittleEndian};
    ///
    /// let mut w  = BitWriter::endian(vec![], LittleEndian);
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(false).is_ok());
    /// assert!(w.write_bit(true).is_ok());
    /// assert_eq!(w.into_writer(), &[0b1000_1110]);
    /// ```
    #[inline]
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        self.write_unsigned::<1, u8>(u8::from(bit))
    }

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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // writing unsigned value is ok
    /// assert!(w.write::<4, u8>(1).is_ok());
    /// // writing signed value is ok
    /// assert!(w.write::<4, i8>(-1).is_ok());
    /// // writing an array of bits is ok too
    /// assert!(w.write::<1, [bool; 4]>([true, false, true, true]).is_ok());
    /// // writing an array of any Integer type is ok
    /// assert!(w.write::<2, [u8; 2]>([0b11, 0b00]).is_ok());
    /// // trying to write a value larger than 4 bits in 4 bits is an error
    /// assert!(w.write::<4, u8>(u8::MAX).is_err());
    ///
    /// assert_eq!(w.into_writer(), &[0b0001_1111, 0b1011_11_00]);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // trying to write 9 bits from a u8 is a compile-time error
    /// w.write::<9, u8>(1);
    /// ```
    #[inline]
    fn write<const BITS: u32, I>(&mut self, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        Integer::write::<BITS, Self>(value, self)
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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // writing unsigned value is ok
    /// assert!(w.write_var::<u8>(4, 1).is_ok());
    /// // writing signed value is also ok
    /// assert!(w.write_var::<i8>(4, -1).is_ok());
    /// assert_eq!(w.into_writer(), &[0b0001_1111]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // writing a value larger than 4 bits in 4 bits is a runtime error
    /// assert!(w.write_var::<u8>(4, u8::MAX).is_err());
    /// // writing 9 bits from a u8 is also a runtime error
    /// assert!(w.write_var::<u8>(9, 0).is_err());
    /// ```
    #[inline]
    fn write_var<I>(&mut self, bits: u32, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        self.write_counted(BitCount::unknown(bits), value)
    }

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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
    /// writer.write_unsigned::<1, u8>(0b1).unwrap();
    /// writer.write_unsigned::<2, u8>(0b01).unwrap();
    /// writer.write_unsigned::<5, u8>(0b10111).unwrap();
    /// assert_eq!(writer.into_writer(), [0b1_01_10111]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], LittleEndian);
    /// writer.write_unsigned::<1, u8>(0b1).unwrap();
    /// writer.write_unsigned::<2, u8>(0b11).unwrap();
    /// writer.write_unsigned::<5, u8>(0b10110).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10110_11_1]);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
    /// // trying to write 9 bits from a u8 is a compile-time error
    /// writer.write_unsigned::<9, u8>(1);
    /// ```
    ///
    /// ```
    /// use std::io::{Write, sink};
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut w = BitWriter::endian(sink(), BigEndian);
    /// assert!(w.write_unsigned::<1, u8>(2).is_err());    // can't write   2 in 1 bit
    /// assert!(w.write_unsigned::<2, u8>(4).is_err());    // can't write   4 in 2 bits
    /// assert!(w.write_unsigned::<3, u8>(8).is_err());    // can't write   8 in 3 bits
    /// assert!(w.write_unsigned::<4, u8>(16).is_err());   // can't write  16 in 4 bits
    /// ```
    #[inline]
    fn write_unsigned<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        self.write_unsigned_var(BITS, value)
    }

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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
    /// writer.write_unsigned_var::<u8>(1, 0b1).unwrap();
    /// writer.write_unsigned_var::<u8>(2, 0b01).unwrap();
    /// writer.write_unsigned_var::<u8>(5, 0b10111).unwrap();
    /// assert_eq!(writer.into_writer(), [0b1_01_10111]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], LittleEndian);
    /// writer.write_unsigned_var::<u8>(1, 0b1).unwrap();
    /// writer.write_unsigned_var::<u8>(2, 0b11).unwrap();
    /// writer.write_unsigned_var::<u8>(5, 0b10110).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10110_11_1]);
    /// ```
    ///
    /// ```
    /// use std::io::{Write, sink};
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut w = BitWriter::endian(sink(), BigEndian);
    /// assert!(w.write_unsigned_var::<u8>(9, 0).is_err());    // can't write  u8 in 9 bits
    /// assert!(w.write_unsigned_var::<u16>(17, 0).is_err());  // can't write u16 in 17 bits
    /// assert!(w.write_unsigned_var::<u32>(33, 0).is_err());  // can't write u32 in 33 bits
    /// assert!(w.write_unsigned_var::<u64>(65, 0).is_err());  // can't write u64 in 65 bits
    /// assert!(w.write_unsigned_var::<u8>(1, 2).is_err());    // can't write   2 in 1 bit
    /// assert!(w.write_unsigned_var::<u8>(2, 4).is_err());    // can't write   4 in 2 bits
    /// assert!(w.write_unsigned_var::<u8>(3, 8).is_err());    // can't write   8 in 3 bits
    /// assert!(w.write_unsigned_var::<u8>(4, 16).is_err());   // can't write  16 in 4 bits
    /// ```
    fn write_unsigned_var<U>(&mut self, bits: u32, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        self.write_unsigned_counted(BitCount::unknown(bits), value)
    }

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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
    /// writer.write_signed::<4, i8>(-5).unwrap();
    /// writer.write_signed::<4, i8>(7).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10110111]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], LittleEndian);
    /// writer.write_signed::<4, i8>(7).unwrap();
    /// writer.write_signed::<4, i8>(-5).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10110111]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], LittleEndian);
    /// // writing a value too large for 4 bits in 4 bits is a runtime error
    /// assert!(writer.write_signed::<4, i8>(i8::MAX).is_err());
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], LittleEndian);
    /// // writing 9 bits from an i8 is a compile-time error
    /// assert!(writer.write_signed::<9, i8>(1).is_err());
    /// ```
    fn write_signed<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        self.write_signed_var(BITS, value)
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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
    /// writer.write_signed_var(4, -5).unwrap();
    /// writer.write_signed_var(4, 7).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10110111]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    ///
    /// let mut writer = BitWriter::endian(vec![], LittleEndian);
    /// writer.write_signed_var(4, 7).unwrap();
    /// writer.write_signed_var(4, -5).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10110111]);
    /// ```
    #[inline(always)]
    fn write_signed_var<S>(&mut self, bits: u32, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        self.write_signed_counted(BitCount::unknown(bits), value)
    }

    /// Writes the given bit count to the stream
    /// with the necessary maximum number of bits.
    ///
    /// For example, if the maximum bit count is 15 - or `0b1111` -
    /// writes the bit count to the stream as a 4-bit unsigned value
    /// which can be used in subsequent writes.
    ///
    /// Note that `MAX` must be greater than 0.
    /// Unlike the bit reader, the bit count need not be an exact
    /// power of two when writing.  Any bits higher than the
    /// bit count can reach are simply left 0.
    ///
    /// # Errors
    ///
    /// Passes along an I/O error from the underlying stream.
    ///
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// let count = 4;
    /// w.write::<3, u32>(count).unwrap();
    /// // may need to verify count is not larger than u8 at runtime
    /// w.write_var::<u8>(count, 0b1111).unwrap();
    /// w.byte_align().unwrap();
    /// assert_eq!(w.into_writer(), &[0b100_11110]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite, BitCount};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // a bit count of 4, with a maximum of 7 (0b111)
    /// let count: BitCount<0b111> = BitCount::new::<4>();
    /// w.write_count(count).unwrap();
    /// // maximum size of count is known to be 7 bits at compile-time
    /// // so no need to check that 7 bits is larger than a u8 at runtime
    /// w.write_counted::<0b111, u8>(count, 0b1111).unwrap();
    /// w.byte_align().unwrap();
    /// assert_eq!(w.into_writer(), &[0b100_11110]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite, BitCount};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // a bit count of 4, with a maximum of 6 (0b110)
    /// let count: BitCount<0b110> = BitCount::new::<4>();
    /// w.write_count(count).unwrap();
    /// w.write_counted::<0b110, u8>(count, 0b1111).unwrap();
    /// w.byte_align().unwrap();
    /// // bit count is written in 3 bits
    /// // while actual value is written in 4 bits
    /// assert_eq!(w.into_writer(), &[0b100_11110]);
    /// ```
    fn write_count<const MAX: u32>(&mut self, BitCount { bits }: BitCount<MAX>) -> io::Result<()> {
        const {
            assert!(MAX > 0, "MAX value must be > 0");
        }

        self.write_unsigned_var(
            if MAX == u32::MAX {
                32
            } else if (MAX + 1).is_power_of_two() {
                (MAX + 1).ilog2()
            } else {
                (MAX + 1).ilog2() + 1
            },
            bits,
        )
    }

    /// Writes a signed or unsigned value to the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian, BitCount};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // writing 4 bits with a maximum of 8 will fit into a u8
    /// // so we only need check the value fits into 4 bits
    /// assert!(w.write_counted::<4, u8>(BitCount::new::<4>(), 0b1111).is_ok());
    /// assert!(w.write_counted::<4, u8>(BitCount::new::<4>(), 0b1111 + 1).is_err());
    /// // writing 4 bits with a maximum of 64 might not fit into a u8
    /// // so need to verify this at runtime
    /// assert!(w.write_counted::<64, u8>(BitCount::new::<4>(), 0b0000).is_ok());
    /// assert_eq!(w.into_writer(), &[0b1111_0000]);
    /// ```
    fn write_counted<const MAX: u32, I>(&mut self, bits: BitCount<MAX>, value: I) -> io::Result<()>
    where
        I: Integer + Sized,
    {
        I::write_var::<MAX, _>(value, self, bits)
    }

    /// Writes a signed value to the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian, BitCount};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // writing 4 bits with a maximum of 8 will fit into a u8
    /// // so we only need check the value fits into 4 bits
    /// assert!(w.write_unsigned_counted::<4, u8>(BitCount::new::<4>(), 0b1111).is_ok());
    /// assert!(w.write_unsigned_counted::<4, u8>(BitCount::new::<4>(), 0b1111 + 1).is_err());
    /// // writing 4 bits with a maximum of 64 might not fit into a u8
    /// // so need to verify this at runtime
    /// assert!(w.write_unsigned_counted::<64, u8>(BitCount::new::<4>(), 0b0000).is_ok());
    /// assert_eq!(w.into_writer(), &[0b1111_0000]);
    /// ```
    fn write_unsigned_counted<const BITS: u32, U>(
        &mut self,
        bits: BitCount<BITS>,
        value: U,
    ) -> io::Result<()>
    where
        U: UnsignedInteger;

    /// Writes an unsigned value to the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the value is too large
    /// to fit the given number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian, BitCount};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // writing 4 bits with a maximum of 8 will fit into an i8
    /// // so we only need check the value fits into 4 bits
    /// assert!(w.write_signed_counted::<4, i8>(BitCount::new::<4>(), 0b0111).is_ok());
    /// assert!(w.write_signed_counted::<4, i8>(BitCount::new::<4>(), 0b0111 + 1).is_err());
    /// // writing 4 bits with a maximum of 64 might not fit into a i8
    /// // so need to verify this at runtime
    /// assert!(w.write_signed_counted::<64, i8>(BitCount::new::<4>(), 0b0000).is_ok());
    /// assert_eq!(w.into_writer(), &[0b0111_0000]);
    /// ```
    fn write_signed_counted<const MAX: u32, S>(
        &mut self,
        bits: impl TryInto<SignedBitCount<MAX>>,
        value: S,
    ) -> io::Result<()>
    where
        S: SignedInteger;

    /// Writes the given constant value to the stream with
    /// the given number of bits.
    ///
    /// Due to current limitations of constant parameters,
    /// this is limited to `u32` values.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// A compile-time error occurs if the number of bits is larger
    /// than 32 or if the value is too large too fit the
    /// requested number of bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// assert!(w.write_const::<4, 0b1000>().is_ok());
    /// assert!(w.write_const::<4, 0b1011>().is_ok());
    /// assert_eq!(w.into_writer(), &[0b1000_1011]);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // trying to write a 5 bit value in 4 bits is a compile-time error
    /// w.write_const::<4, 0b11111>();
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// // trying to write a 33 bit value is also a compile-time error
    /// w.write_const::<33, 1>();
    /// ```
    #[inline]
    fn write_const<const BITS: u32, const VALUE: u32>(&mut self) -> io::Result<()> {
        const {
            assert!(
                BITS == 0 || VALUE <= (u32::ALL >> (u32::BITS_SIZE - BITS)),
                "excessive value for bits written"
            );
        }

        self.write::<BITS, u32>(VALUE)
    }

    /// Writes whole value to the stream whose size in bits
    /// is equal to its type's size.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// assert!(w.write_from::<u32>(0x12_34_56_78).is_ok());
    /// assert_eq!(w.into_writer(), &[0x12, 0x34, 0x56, 0x78]);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// assert!(w.write_from::<[u8; 4]>([0x12, 0x34, 0x56, 0x78]).is_ok());
    /// assert_eq!(w.into_writer(), &[0x12, 0x34, 0x56, 0x78]);
    /// ```
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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian, LittleEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// assert!(w.write_as_from::<LittleEndian, u32>(0x12_34_56_78).is_ok());
    /// assert_eq!(w.into_writer(), &[0x78, 0x56, 0x34, 0x12]);
    /// ```
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive;

    /// Pads the stream by writing 0 over the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// assert!(w.write_bit(true).is_ok());
    /// assert!(w.pad(7).is_ok());
    /// assert_eq!(w.into_writer(), &[0b1_0000000]);
    /// ```
    fn pad(&mut self, mut bits: u32) -> io::Result<()> {
        loop {
            match bits {
                0 => break Ok(()),
                bits @ 1..64 => break self.write_var(bits, 0u64),
                _ => {
                    self.write::<64, u64>(0)?;
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
        buf.iter().try_for_each(|b| self.write_unsigned::<8, _>(*b))
    }

    /// Writes `value` number of non `STOP_BIT` bits to the stream
    /// and then writes a `STOP_BIT`.  This field is variably-sized.
    /// `STOP_BIT` must be 0 or 1.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underyling stream.
    ///
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_unary::<0>(0).unwrap();
    /// writer.write_unary::<0>(3).unwrap();
    /// writer.write_unary::<0>(10).unwrap();
    /// assert_eq!(writer.into_writer(), [0b01110111, 0b11111110]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), LittleEndian);
    /// writer.write_unary::<0>(0).unwrap();
    /// writer.write_unary::<0>(3).unwrap();
    /// writer.write_unary::<0>(10).unwrap();
    /// assert_eq!(writer.into_writer(), [0b11101110, 0b01111111]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_unary::<1>(0).unwrap();
    /// writer.write_unary::<1>(3).unwrap();
    /// writer.write_unary::<1>(10).unwrap();
    /// assert_eq!(writer.into_writer(), [0b10001000, 0b00000001]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{LittleEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), LittleEndian);
    /// writer.write_unary::<1>(0).unwrap();
    /// writer.write_unary::<1>(3).unwrap();
    /// writer.write_unary::<1>(10).unwrap();
    /// assert_eq!(writer.into_writer(), [0b00010001, 0b10000000]);
    /// ```
    fn write_unary<const STOP_BIT: u8>(&mut self, mut value: u32) -> io::Result<()> {
        const {
            assert!(matches!(STOP_BIT, 0 | 1), "stop bit must be 0 or 1");
        }

        const MAX: BitCount<32> = BitCount::new::<32>();

        match STOP_BIT {
            0 => {
                while value > 0 {
                    let to_write = MAX.min(value);
                    self.write_checked(to_write.all::<u32>())?;
                    value -= u32::from(to_write);
                }
                self.write_bit(false)
            }
            1 => {
                while value > 0 {
                    let to_write = MAX.min(value);
                    self.write_checked(to_write.none::<u32>())?;
                    value -= u32::from(to_write);
                }
                self.write_bit(true)
            }
            _ => unreachable!(),
        }
    }

    /// Writes checked value that is known to fit a given number of bits
    fn write_checked<C: Checkable>(&mut self, value: C) -> io::Result<()> {
        // a naive default implementation
        value.write(self)
    }

    /// Builds and writes complex type
    fn build<T: ToBitStream>(&mut self, build: &T) -> Result<(), T::Error> {
        build.to_writer(self)
    }

    /// Builds and writes complex type with context
    fn build_with<'a, T: ToBitStreamWith<'a>>(
        &mut self,
        build: &T,
        context: &T::Context,
    ) -> Result<(), T::Error> {
        build.to_writer(self, context)
    }

    /// Builds and writes complex type with owned context
    fn build_using<T: ToBitStreamUsing>(
        &mut self,
        build: &T,
        context: T::Context,
    ) -> Result<(), T::Error> {
        build.to_writer(self, context)
    }

    /// Returns true if the stream is aligned at a whole byte.
    ///
    /// # Example
    /// ```
    /// use std::io::{Write, sink};
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(sink(), BigEndian);
    /// assert_eq!(writer.byte_aligned(), true);
    /// writer.write_var(1, 0u8).unwrap();
    /// assert_eq!(writer.byte_aligned(), false);
    /// writer.write_var(7, 0u8).unwrap();
    /// assert_eq!(writer.byte_aligned(), true);
    /// ```
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
        while !BitWrite::byte_aligned(self) {
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
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// use oximedia_bitstream::define_huffman_tree;
    ///
    /// define_huffman_tree!(TreeName : char = ['a', ['b', ['c', 'd']]]);
    /// // 'a' is 0
    /// // 'b' is 1 -> 0
    /// // 'c' is 1 -> 1 -> 0
    /// // 'd' is 1 -> 1 -> 1
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
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

    /// Writes a number using a variable using a variable width integer.
    /// This optimises the case when the number is small.
    ///
    /// Given a 4-bit VBR field, any 3-bit value (0 through 7) is encoded directly, with the high bit set to zero.
    /// Values larger than N-1 bits emit their bits in a series of N-1 bit chunks, where all but the last set the high bit.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_unsigned_vbr::<4,_>(7u32);
    /// writer.write_unsigned_vbr::<4,_>(100u32);
    /// assert_eq!(writer.into_writer(), [0b0111_1100, 0b1100_0001]);
    /// ```
    fn write_unsigned_vbr<const FIELD_SIZE: u32, U: UnsignedInteger>(
        &mut self,
        value: U,
    ) -> io::Result<()> {
        const { assert!(FIELD_SIZE >= 2 && FIELD_SIZE < U::BITS_SIZE) };
        let payload_bits = FIELD_SIZE - 1;
        let continuation_bit = U::ONE.shl(payload_bits);
        let payload_mask = continuation_bit.sub(U::ONE);
        let mut value = value;

        loop {
            let payload = value & payload_mask;
            value >>= payload_bits;
            if value != U::ZERO {
                self.write_unsigned::<FIELD_SIZE, U>(payload | continuation_bit)?;
            } else {
                self.write_unsigned::<FIELD_SIZE, U>(payload)?;
                break;
            }
        }
        Ok(())
    }

    /// Writes a number using a variable using a variable width integer.
    /// This optimises the case when the number is small.
    ///
    /// The integer is mapped to an unsigned value using zigzag encoding.
    /// For an integer X:
    ///   - if X >= 0 -> 2X
    ///   - else -> -2X + 1
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_signed_vbr::<4,_>(3);
    /// writer.write_signed_vbr::<4,_>(-50);
    /// assert_eq!(writer.into_writer(), [0b0110_1011, 0b1100_0001]);
    /// ```
    #[inline]
    fn write_signed_vbr<const FIELD_SIZE: u32, I: SignedInteger>(
        &mut self,
        value: I,
    ) -> io::Result<()> {
        let zig_zag = value.shl(1).bitxor(value.shr(I::BITS_SIZE - 1));
        self.write_unsigned_vbr::<FIELD_SIZE, _>(zig_zag.as_non_negative())
    }

    /// Writes a signed or unsigned variable width integer to the stream
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};
    /// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
    /// writer.write_vbr::<4,_>(6u32);
    /// writer.write_vbr::<4,_>(-50i32);
    /// assert_eq!(writer.into_writer(), [0b0110_1011, 0b1100_0001]);
    /// ```
    #[inline]
    fn write_vbr<const FIELD_SIZE: u32, I: VBRInteger>(&mut self, value: I) -> io::Result<()> {
        I::write_vbr::<FIELD_SIZE, _>(value, self)
    }

    /// Creates a "by reference" adaptor for this `BitWrite`
    ///
    /// The returned adapter also implements `BitWrite`
    /// and will borrow the current reader.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
    ///
    /// fn build<W: BitWrite>(w: W) {
    ///     // perform some building
    /// }
    ///
    /// let mut writer = BitWriter::endian(vec![], BigEndian);
    /// // performing building by reference
    /// build(writer.by_ref());
    /// // original owned writer still available
    /// writer.write::<8, u8>(0).unwrap();
    /// assert_eq!(writer.into_writer(), &[0]);
    /// ```
    #[inline]
    fn by_ref(&mut self) -> &mut Self {
        self
    }
}

impl<W: BitWrite + ?Sized> BitWrite for &mut W {
    #[inline]
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        (**self).write_bit(bit)
    }

    #[inline]
    fn write<const BITS: u32, I>(&mut self, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        (**self).write::<BITS, I>(value)
    }

    #[inline]
    fn write_const<const BITS: u32, const VALUE: u32>(&mut self) -> io::Result<()> {
        (**self).write_const::<BITS, VALUE>()
    }

    #[inline]
    fn write_var<I>(&mut self, bits: u32, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        (**self).write_var(bits, value)
    }

    #[inline]
    fn write_unsigned<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        (**self).write_unsigned::<BITS, U>(value)
    }

    #[inline]
    fn write_unsigned_var<U>(&mut self, bits: u32, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        (**self).write_unsigned_var(bits, value)
    }

    #[inline]
    fn write_signed<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        (**self).write_signed::<BITS, S>(value)
    }

    #[inline(always)]
    fn write_signed_var<S>(&mut self, bits: u32, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        (**self).write_signed_var(bits, value)
    }

    #[inline]
    fn write_count<const MAX: u32>(&mut self, count: BitCount<MAX>) -> io::Result<()> {
        (**self).write_count::<MAX>(count)
    }

    #[inline]
    fn write_counted<const MAX: u32, I>(&mut self, bits: BitCount<MAX>, value: I) -> io::Result<()>
    where
        I: Integer + Sized,
    {
        (**self).write_counted::<MAX, I>(bits, value)
    }

    #[inline]
    fn write_unsigned_counted<const BITS: u32, U>(
        &mut self,
        bits: BitCount<BITS>,
        value: U,
    ) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        (**self).write_unsigned_counted::<BITS, U>(bits, value)
    }

    #[inline]
    fn write_signed_counted<const MAX: u32, S>(
        &mut self,
        bits: impl TryInto<SignedBitCount<MAX>>,
        value: S,
    ) -> io::Result<()>
    where
        S: SignedInteger,
    {
        (**self).write_signed_counted::<MAX, S>(bits, value)
    }

    #[inline]
    fn write_from<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive,
    {
        (**self).write_from::<V>(value)
    }

    #[inline]
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        (**self).write_as_from::<F, V>(value)
    }

    #[inline]
    fn pad(&mut self, bits: u32) -> io::Result<()> {
        (**self).pad(bits)
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        (**self).write_bytes(buf)
    }

    #[inline]
    fn write_unary<const STOP_BIT: u8>(&mut self, value: u32) -> io::Result<()> {
        (**self).write_unary::<STOP_BIT>(value)
    }

    #[inline]
    fn write_checked<C: Checkable>(&mut self, value: C) -> io::Result<()> {
        (**self).write_checked(value)
    }

    #[inline]
    fn build<T: ToBitStream>(&mut self, build: &T) -> Result<(), T::Error> {
        (**self).build(build)
    }

    #[inline]
    fn build_with<'a, T: ToBitStreamWith<'a>>(
        &mut self,
        build: &T,
        context: &T::Context,
    ) -> Result<(), T::Error> {
        (**self).build_with(build, context)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        (**self).byte_aligned()
    }

    #[inline]
    fn byte_align(&mut self) -> io::Result<()> {
        (**self).byte_align()
    }

    #[inline]
    fn write_huffman<T>(&mut self, value: T::Symbol) -> io::Result<()>
    where
        T: crate::huffman::ToBits,
    {
        (**self).write_huffman::<T>(value)
    }
}

// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! The core `BitRead` trait and its blanket impls.

use std::io;
#[cfg(feature = "alloc")]
use std::vec::Vec;

use super::{
    read_to_vec, BitCount, CheckablePrimitive, Endianness, FromBitStream, FromBitStreamUsing,
    FromBitStreamWith, Integer, Primitive, SignedBitCount, SignedInteger, UnsignedInteger,
    VBRInteger, VariableWidthOverflow,
};

/// A trait for anything that can read a variable number of
/// potentially un-aligned values from an input stream
pub trait BitRead {
    /// Reads a single bit from the stream.
    /// `true` indicates 1, `false` indicates 0
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b1000_1110];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert!(r.read_bit().is_err());  // no more bits to read
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, LittleEndian};
    ///
    /// let bytes: &[u8] = &[0b1000_1110];
    /// let mut r = BitReader::endian(bytes, LittleEndian);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert!(r.read_bit().is_err());  // no more bits to read
    /// ```
    fn read_bit(&mut self) -> io::Result<bool> {
        self.read_unsigned::<1, u8>().map(|b| b == 1)
    }

    /// Reads a signed or unsigned value from the stream with
    /// the given constant number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b0001_1111, 0b1011_11_00];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading unsigned value is ok
    /// assert_eq!(r.read::<4, u8>().unwrap(), 1);
    /// // reading signed value is also ok
    /// assert_eq!(r.read::<4, i8>().unwrap(), -1);
    /// // reading an array of bits is ok too
    /// assert_eq!(r.read::<1, [bool; 4]>().unwrap(), [true, false, true, true]);
    /// // reading an array of any Integer type is ok
    /// assert_eq!(r.read::<2, [u8; 2]>().unwrap(), [0b11, 0b00]);
    /// // reading more bytes than we have is an error
    /// assert!(r.read::<4, u8>().is_err());
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b0001_1111, 0, 0];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading 9 bits to a u8 is a compile-time error
    /// r.read::<9, u8>();
    /// ```
    #[inline]
    fn read<const BITS: u32, I>(&mut self) -> io::Result<I>
    where
        I: Integer,
    {
        I::read::<BITS, _>(self)
    }

    /// Reads a signed or unsigned value from the stream with
    /// the given variable number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Also returns an error if the output type is too small
    /// to hold the requested number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    /// let bytes: &[u8] = &[0b0001_1111];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading unsigned value is ok
    /// assert_eq!(r.read_var::<u8>(4).unwrap(), 1);
    /// // reading signed value is also ok
    /// assert_eq!(r.read_var::<i8>(4).unwrap(), -1);
    /// // reading more bytes than we have is an error
    /// assert!(r.read_var::<u8>(4).is_err());
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    /// let bytes: &[u8] = &[0, 0, 0, 0, 0];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading 9 bits to a u8 is a runtime error
    /// // no matter how much data is left
    /// assert!(r.read_var::<u8>(9).is_err());
    /// ```
    #[inline]
    fn read_var<I>(&mut self, bits: u32) -> io::Result<I>
    where
        I: Integer + Sized,
    {
        I::read_var(self, BitCount::unknown(bits))
    }

    /// Given a desired maximum value for a bit count,
    /// reads the necessary bits to fill up to that amount.
    ///
    /// For example, if the maximum bit count is 15 - or `0b1111` -
    /// reads a 4-bit unsigned value from the stream and returns a [`BitCount`]
    /// which can be used in subsequent reads.
    ///
    /// Note that `MAX` must be greater than 0, and `MAX + 1` must be
    /// an exact power of two.
    ///
    /// # Errors
    ///
    /// Passes along an I/O error from the underlying stream.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    ///
    /// let bytes: &[u8] = &[0b100_11110];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// let count = r.read::<3, u32>().unwrap();
    /// assert_eq!(count, 4);  // reads 0b100 - or 4
    /// // may need to verify count is not larger than u8 at runtime
    /// assert_eq!(r.read_var::<u8>(count).unwrap(), 0b1111);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead, BitCount};
    ///
    /// let bytes: &[u8] = &[0b100_11110];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// let count = r.read_count::<0b111>().unwrap();
    /// assert_eq!(count, BitCount::new::<4>());  // reads 0b100 - or 4
    /// // maximum size of bit count is known to be 7 at compile-time,
    /// // so no runtime check needed to know 7 bits is not larger than a u8
    /// assert_eq!(r.read_counted::<0b111, u8>(count).unwrap(), 0b1111);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    ///
    /// let bytes: &[u8] = &[0b100_11110];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // maximum bit count is 6 (0b110), so we need to read 3 bits
    /// // but no idea what to do if a value of 7 (0b111) is read,
    /// // so this does not compile at all
    /// let count = r.read_count::<0b110>();
    /// ```
    fn read_count<const MAX: u32>(&mut self) -> io::Result<BitCount<MAX>> {
        const {
            assert!(MAX > 0, "MAX value must be > 0");
            assert!(
                MAX == u32::MAX || (MAX + 1).is_power_of_two(),
                "MAX should fill some whole number of bits ('0b111', '0b1111', etc.)"
            );
        }

        self.read_unsigned_var(if MAX < u32::MAX {
            (MAX + 1).ilog2()
        } else {
            32
        })
        .map(|bits| BitCount { bits })
    }

    /// Reads a signed or unsigned value from the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Also returns an error if the output type is too small
    /// to hold the requested number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, BitCount};
    ///
    /// let bytes: &[u8] = &[0b1111_0000];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading 4 bits with a maximum of 4 will fit into a u8
    /// // so no runtime check needed
    /// assert_eq!(r.read_counted::<4, u8>(BitCount::new::<4>()).unwrap(), 0b1111);
    /// // reading 4 bits with a maximum of 64 might not fit into a u8
    /// // so we need to verify this at runtime
    /// assert_eq!(r.read_counted::<64, u8>(BitCount::new::<4>()).unwrap(), 0b0000);
    /// ```
    #[inline(always)]
    fn read_counted<const MAX: u32, I>(&mut self, bits: BitCount<MAX>) -> io::Result<I>
    where
        I: Integer + Sized,
    {
        I::read_var(self, bits)
    }

    /// Reads an unsigned value from the stream with
    /// the given constant number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// A compile-time error occurs if the given number of bits
    /// is larger than the output type.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0b1_01_10111];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// assert_eq!(reader.read_unsigned::<1, u8>().unwrap(), 0b1);
    /// assert_eq!(reader.read_unsigned::<2, u8>().unwrap(), 0b01);
    /// assert_eq!(reader.read_unsigned::<5, u8>().unwrap(), 0b10111);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0b10110_11_1];
    /// let mut reader = BitReader::endian(data, LittleEndian);
    /// assert_eq!(reader.read_unsigned::<1, u8>().unwrap(), 0b1);
    /// assert_eq!(reader.read_unsigned::<2, u8>().unwrap(), 0b11);
    /// assert_eq!(reader.read_unsigned::<5, u8>().unwrap(), 0b10110);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0, 0, 0, 0, 0];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// // doesn't compile at all
    /// reader.read_unsigned::<9, u8>();  // can't read  9 bits to u8
    /// ```
    fn read_unsigned<const BITS: u32, U>(&mut self) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        self.read_unsigned_var(BITS)
    }

    /// Reads an unsigned value from the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Also returns an error if the output type is too small
    /// to hold the requested number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0b1_01_10111];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// assert_eq!(reader.read_unsigned_var::<u8>(1).unwrap(), 0b1);
    /// assert_eq!(reader.read_unsigned_var::<u8>(2).unwrap(), 0b01);
    /// assert_eq!(reader.read_unsigned_var::<u8>(5).unwrap(), 0b10111);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0b10110_11_1];
    /// let mut reader = BitReader::endian(data, LittleEndian);
    /// assert_eq!(reader.read_unsigned_var::<u8>(1).unwrap(), 0b1);
    /// assert_eq!(reader.read_unsigned_var::<u8>(2).unwrap(), 0b11);
    /// assert_eq!(reader.read_unsigned_var::<u8>(5).unwrap(), 0b10110);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0, 0, 0, 0, 0];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// assert!(reader.read_unsigned_var::<u8>(9).is_err());    // can't read  9 bits to u8
    /// assert!(reader.read_unsigned_var::<u16>(17).is_err());  // can't read 17 bits to u16
    /// assert!(reader.read_unsigned_var::<u32>(33).is_err());  // can't read 33 bits to u32
    /// assert!(reader.read_unsigned_var::<u64>(65).is_err());  // can't read 65 bits to u64
    /// ```
    #[inline(always)]
    fn read_unsigned_var<U>(&mut self, bits: u32) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        self.read_unsigned_counted(BitCount::unknown(bits))
    }

    /// Reads an unsigned value from the stream with
    /// the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Also returns an error if the output type is too small
    /// to hold the requested number of bits.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, BitCount};
    ///
    /// let bytes: &[u8] = &[0b1111_0000];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading 4 bits with a maximum of 4 will fit into a u8
    /// // so no runtime check needed
    /// assert_eq!(r.read_unsigned_counted::<4, u8>(BitCount::new::<4>()).unwrap(), 0b1111);
    /// // reading 4 bits with a maximum of 64 might not fit into a u8
    /// // so we need to verify this at runtime
    /// assert_eq!(r.read_unsigned_counted::<64, u8>(BitCount::new::<4>()).unwrap(), 0b0000);
    /// ```
    fn read_unsigned_counted<const MAX: u32, U>(&mut self, bits: BitCount<MAX>) -> io::Result<U>
    where
        U: UnsignedInteger;

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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    ///
    /// let data: &[u8] = &[0b1011_0111];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// assert_eq!(reader.read_signed::<4, i8>().unwrap(), -5);
    /// assert_eq!(reader.read_signed::<4, i8>().unwrap(), 7);
    /// assert!(reader.read_signed::<4, i8>().is_err());
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitReader, BitRead};
    ///
    /// let data: &[u8] = &[0b1011_0111];
    /// let mut reader = BitReader::endian(data, LittleEndian);
    /// assert_eq!(reader.read_signed::<4, i8>().unwrap(), 7);
    /// assert_eq!(reader.read_signed::<4, i8>().unwrap(), -5);
    /// assert!(reader.read_signed::<4, i8>().is_err());
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::{LittleEndian, BitReader, BitRead};
    ///
    /// let data: &[u8] = &[0, 0, 0, 0, 0];
    /// let mut reader = BitReader::endian(data, LittleEndian);
    /// // reading 9 bits to an i8 is a compile-time error
    /// reader.read_signed::<9, i8>();
    /// ```
    fn read_signed<const BITS: u32, S>(&mut self) -> io::Result<S>
    where
        S: SignedInteger,
    {
        self.read_signed_var(BITS)
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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0b1011_0111];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// assert_eq!(reader.read_signed_var::<i8>(4).unwrap(), -5);
    /// assert_eq!(reader.read_signed_var::<i8>(4).unwrap(), 7);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{LittleEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0b1011_0111];
    /// let mut reader = BitReader::endian(data, LittleEndian);
    /// assert_eq!(reader.read_signed_var::<i8>(4).unwrap(), 7);
    /// assert_eq!(reader.read_signed_var::<i8>(4).unwrap(), -5);
    /// ```
    ///
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    /// let mut r = BitReader::endian(data, BigEndian);
    /// assert!(r.read_signed_var::<i8>(9).is_err());   // can't read 9 bits to i8
    /// assert!(r.read_signed_var::<i16>(17).is_err()); // can't read 17 bits to i16
    /// assert!(r.read_signed_var::<i32>(33).is_err()); // can't read 33 bits to i32
    /// assert!(r.read_signed_var::<i64>(65).is_err()); // can't read 65 bits to i64
    /// ```
    fn read_signed_var<S>(&mut self, bits: u32) -> io::Result<S>
    where
        S: SignedInteger,
    {
        self.read_signed_counted(BitCount::unknown(bits))
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
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, BitCount};
    ///
    /// let bytes: &[u8] = &[0b0001_1111];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // reading 4 bits with a maximum of 4 will fit into an i8
    /// // so no runtime check needed
    /// assert_eq!(r.read_signed_counted::<4, i8>(BitCount::new::<4>()).unwrap(), 1);
    /// // reading 4 bits with a maximum of 64 might not fit into an i8
    /// // so we need to verify this at runtime
    /// assert_eq!(r.read_signed_counted::<64, i8>(BitCount::new::<4>()).unwrap(), -1);
    /// ```
    fn read_signed_counted<const MAX: u32, S>(
        &mut self,
        bits: impl TryInto<SignedBitCount<MAX>>,
    ) -> io::Result<S>
    where
        S: SignedInteger;

    /// Reads the given constant value from the stream with the
    /// given number of bits.
    ///
    /// Due to current limitations of constant paramters,
    /// this is limited to `u32` values.
    ///
    /// If the constant read from the stream doesn't match the expected
    /// value, returns the generated error from the closure.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream,
    /// converted to the mismatch error.  Returns the generated
    /// error if the read value doesn't match the expected constant.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// enum Error {
    ///     Mismatch,
    ///     Io,
    /// }
    ///
    /// impl From<std::io::Error> for Error {
    ///     fn from(_err: std::io::Error) -> Self {
    ///         Self::Io
    ///     }
    /// }
    ///
    /// let data: &[u8] = &[0b1000_1011, 0b0000_0001];
    /// let mut r = BitReader::endian(data, BigEndian);
    /// assert!(r.read_const::<4, 0b1000, _>(Error::Mismatch).is_ok());
    /// assert!(r.read_const::<4, 0b1011, _>(Error::Mismatch).is_ok());
    /// // 0b1000 doesn't match 0b0000
    /// assert!(matches!(r.read_const::<4, 0b1000, _>(Error::Mismatch), Err(Error::Mismatch)));
    /// // 0b1011 doesn't match 0b0001
    /// assert!(matches!(r.read_const::<4, 0b1011, _>(Error::Mismatch), Err(Error::Mismatch)));
    /// // run out of bits to check
    /// assert!(matches!(r.read_const::<4, 0b0000, _>(Error::Mismatch), Err(Error::Io)));
    /// ```
    #[inline]
    fn read_const<const BITS: u32, const VALUE: u32, E>(&mut self, err: E) -> Result<(), E>
    where
        E: From<io::Error>,
    {
        use crate::Numeric;

        const {
            assert!(
                BITS == 0 || VALUE <= (u32::ALL >> (u32::BITS_SIZE - BITS)),
                "excessive value for bits read"
            );
        }

        (self.read::<BITS, u32>()? == VALUE)
            .then_some(())
            .ok_or(err)
    }

    /// Reads whole value from the stream whose size in bits is equal
    /// to its type's size.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0x12, 0x34, 0x56, 0x78];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_to::<u32>().unwrap(), 0x12_34_56_78);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0x12, 0x34, 0x56, 0x78];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_to::<[u8; 4]>().unwrap(), [0x12, 0x34, 0x56, 0x78]);
    /// ```
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
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, LittleEndian};
    ///
    /// let bytes: &[u8] = &[0x12, 0x34, 0x56, 0x78];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_as_to::<LittleEndian, u32>().unwrap(), 0x78_56_34_12);
    /// ```
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
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b1_00000_10];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert!(r.skip(5).is_ok());
    /// assert_eq!(r.read_bit().unwrap(), true);
    /// assert_eq!(r.read_bit().unwrap(), false);
    /// assert!(r.read_bit().is_err());
    /// ```
    fn skip(&mut self, bits: u32) -> io::Result<()> {
        (0..bits).try_for_each(|_| self.read_bit().map(|_| ()))
    }

    /// Completely fills the given buffer with whole bytes.
    /// If the stream is already byte-aligned, it will map
    /// to a faster `read_exact` call.  Otherwise it will read
    /// bytes individually in 8-bit increments.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// let mut buf = [0; 3];
    /// assert_eq!(r.read::<8, u8>().unwrap(), 0x00);
    /// assert!(r.read_bytes(&mut buf).is_ok());
    /// assert_eq!(&buf, &[0x01, 0x02, 0x03]);
    /// assert_eq!(r.read::<8, u8>().unwrap(), 0x04);
    /// ```
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        for b in buf.iter_mut() {
            *b = self.read_unsigned::<8, _>()?;
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
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// let mut buf = [0; 3];
    /// assert_eq!(r.read::<8, u8>().unwrap(), 0x00);
    /// assert_eq!(r.read_to_vec(3).unwrap().as_slice(), &[0x01, 0x02, 0x03]);
    /// assert_eq!(r.read::<8, u8>().unwrap(), 0x04);
    /// ```
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    fn read_to_vec(&mut self, bytes: usize) -> io::Result<Vec<u8>> {
        read_to_vec(|buf| self.read_bytes(buf), bytes)
    }

    /// Counts the number of bits in the stream until `STOP_BIT`
    /// and returns the amount read.
    /// `STOP_BIT` must be 0 or 1.
    /// Because this field is variably-sized and may be large,
    /// its output is always a `u32` type.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// May panic if the number of bits exceeds `u32`.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b0_10_11111, 0b10_000000];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // read 1 bits until stop bit of 0, big-endian order
    /// assert_eq!(r.read_unary::<0>().unwrap(), 0);
    /// assert_eq!(r.read_unary::<0>().unwrap(), 1);
    /// assert_eq!(r.read_unary::<0>().unwrap(), 6);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b1_01_00000, 0b01_000000];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// // read 0 bits until stop bit of 1, big-endian order
    /// assert_eq!(r.read_unary::<1>().unwrap(), 0);
    /// assert_eq!(r.read_unary::<1>().unwrap(), 1);
    /// assert_eq!(r.read_unary::<1>().unwrap(), 6);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, LittleEndian};
    ///
    /// let bytes: &[u8] = &[0b11111_01_0, 0b000000_01];
    /// let mut r = BitReader::endian(bytes, LittleEndian);
    /// // read 1 bits until stop bit of 0, little-endian order
    /// assert_eq!(r.read_unary::<0>().unwrap(), 0);
    /// assert_eq!(r.read_unary::<0>().unwrap(), 1);
    /// assert_eq!(r.read_unary::<0>().unwrap(), 6);
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, LittleEndian};
    ///
    /// let bytes: &[u8] = &[0b00000_10_1, 0b111111_10];
    /// let mut r = BitReader::endian(bytes, LittleEndian);
    /// // read 0 bits until stop bit of 1, little-endian order
    /// assert_eq!(r.read_unary::<1>().unwrap(), 0);
    /// assert_eq!(r.read_unary::<1>().unwrap(), 1);
    /// assert_eq!(r.read_unary::<1>().unwrap(), 6);
    /// ```
    fn read_unary<const STOP_BIT: u8>(&mut self) -> io::Result<u32> {
        const {
            assert!(matches!(STOP_BIT, 0 | 1), "stop bit must be 0 or 1");
        }

        // a simple implementation which works anywhere
        let mut unary = 0;
        while self.read::<1, u8>()? != STOP_BIT {
            unary += 1;
        }
        Ok(unary)
    }

    /// Reads to a checked value that is known to fit a given number of bits
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{
    ///     BitRead, BitReader, BigEndian, Checked, CheckedUnsigned, CheckedSigned,
    ///     BitCount, SignedBitCount, BitWrite, BitWriter,
    /// };
    ///
    /// let data: &[u8] = &[0b1001_1111];
    /// let mut r = BitReader::endian(data, BigEndian);
    ///
    /// let bit_count = BitCount::<4>::new::<4>();
    /// let checked_u8 = r.read_checked::<CheckedUnsigned<4, u8>>(bit_count).unwrap();
    /// assert_eq!(checked_u8.into_value(), 0b1001);
    ///
    /// let bit_count = SignedBitCount::<4>::new::<4>();
    /// let checked_i8 = r.read_checked::<CheckedSigned<4, i8>>(bit_count).unwrap();
    /// assert_eq!(checked_i8.into_value(), -1);
    ///
    /// // note that checked values already know their bit count
    /// // so none is required when writing them to a stream
    /// let mut w = BitWriter::endian(vec![], BigEndian);
    /// w.write_checked(checked_u8).unwrap();
    /// w.write_checked(checked_i8).unwrap();
    /// assert_eq!(w.into_writer().as_slice(), data);
    /// ```
    #[inline]
    fn read_checked<C>(&mut self, count: C::CountType) -> io::Result<C>
    where
        C: CheckablePrimitive,
    {
        C::read(self, count)
    }

    /// Parses and returns complex type
    fn parse<F: FromBitStream>(&mut self) -> Result<F, F::Error> {
        F::from_reader(self)
    }

    /// Parses and returns complex type with context
    fn parse_with<'a, F: FromBitStreamWith<'a>>(
        &mut self,
        context: &F::Context,
    ) -> Result<F, F::Error> {
        F::from_reader(self, context)
    }

    /// Parses and returns complex type with owned context
    fn parse_using<F: FromBitStreamUsing>(&mut self, context: F::Context) -> Result<F, F::Error> {
        F::from_reader(self, context)
    }

    /// Returns true if the stream is aligned at a whole byte.
    ///
    /// # Example
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data = [0];
    /// let mut reader = BitReader::endian(data.as_slice(), BigEndian);
    /// assert_eq!(reader.byte_aligned(), true);
    /// assert!(reader.skip(1).is_ok());
    /// assert_eq!(reader.byte_aligned(), false);
    /// assert!(reader.skip(7).is_ok());
    /// assert_eq!(reader.byte_aligned(), true);
    /// ```
    fn byte_aligned(&self) -> bool;

    /// Throws away all unread bit values until the next whole byte.
    /// Does nothing if the stream is already aligned.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data: &[u8] = &[0x00, 0xFF];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// assert_eq!(reader.read::<4, u8>().unwrap(), 0);
    /// reader.byte_align();
    /// assert_eq!(reader.read::<8, u8>().unwrap(), 0xFF);
    /// ```
    fn byte_align(&mut self);

    /// Given a compiled Huffman tree, reads bits from the stream
    /// until the next symbol is encountered.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, define_huffman_tree, huffman::FromBits};
    ///
    /// define_huffman_tree!(TreeName : char = ['a', ['b', ['c', 'd']]]);
    /// // 'a' is 0
    /// // 'b' is 1 -> 0
    /// // 'c' is 1 -> 1 -> 0
    /// // 'd' is 1 -> 1 -> 1
    ///
    /// let data: &[u8] = &[0b0_10_110_11, 0b1_0000000];
    /// let mut r = BitReader::endian(data, BigEndian);
    /// assert_eq!(r.read_huffman::<TreeName>().unwrap(), 'a');
    /// assert_eq!(r.read_huffman::<TreeName>().unwrap(), 'b');
    /// assert_eq!(r.read_huffman::<TreeName>().unwrap(), 'c');
    /// assert_eq!(r.read_huffman::<TreeName>().unwrap(), 'd');
    /// ```
    #[inline]
    fn read_huffman<T>(&mut self) -> io::Result<T::Symbol>
    where
        T: crate::huffman::FromBits,
    {
        T::from_bits(|| self.read_bit())
    }

    /// Reads a number using a variable using a variable width integer.
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
    /// Returns an error if the data read would overflow the size of the result
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b0111_1100, 0b1100_0001];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_unsigned_vbr::<4, u32>().unwrap(), 7);
    /// assert_eq!(r.read_unsigned_vbr::<4, u32>().unwrap(), 100);
    /// ```
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b1111_1111, 0b0011_1000, 0b1000_0100, 0b1000_1000, 0b1000_0000];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_unsigned_vbr::<4, u8>().unwrap(), 255); // Tries to read <011><111><111>
    /// assert!(r.read_unsigned_vbr::<4, u8>().is_err()); // Tries to read a value of <100><000><000>
    /// assert!(r.read_unsigned_vbr::<4, u8>().is_err()); // Tries to read a value of <000><000><000><000>
    /// ```
    fn read_unsigned_vbr<const FIELD_SIZE: u32, U: UnsignedInteger>(&mut self) -> io::Result<U> {
        const { assert!(FIELD_SIZE >= 2 && FIELD_SIZE < U::BITS_SIZE) };
        let payload_bits = FIELD_SIZE - 1;
        let mut value = U::ZERO;
        let mut shift = 0u32;
        loop {
            let (data, continuation) = self.read_unsigned::<FIELD_SIZE, U>().map(|item| {
                (
                    item & ((U::ONE << payload_bits) - U::ONE),
                    (item >> payload_bits) != U::ZERO,
                )
            })?;
            let shifted = data << shift;
            value |= shifted;
            if !continuation {
                if (data << shift) >> shift == data {
                    break Ok(value);
                } else {
                    break Err(VariableWidthOverflow {}.into());
                }
            }
            shift += payload_bits;
            if shift >= U::BITS_SIZE {
                break Err(VariableWidthOverflow {}.into());
            }
        }
    }

    /// Reads a number using a variable using a variable width integer.
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
    /// Returns an error if the data read would overflow the size of the result
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b0110_1011, 0b1100_0001];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_signed_vbr::<4, i32>().unwrap(), 3);
    /// assert_eq!(r.read_signed_vbr::<4, i32>().unwrap(), -50);
    /// ```
    fn read_signed_vbr<const FIELD_SIZE: u32, I: SignedInteger>(&mut self) -> io::Result<I> {
        self.read_unsigned_vbr::<FIELD_SIZE, I::Unsigned>()
            .map(|zig_zag| {
                let shifted = zig_zag >> 1;
                let complimented = zig_zag & <I::Unsigned as crate::Numeric>::ONE;
                let neg = I::ZERO - complimented.as_non_negative();
                shifted.as_non_negative() ^ neg
            })
    }

    /// Reads a signed or unsigned variable width integer from the stream.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if the data read would overflow the size of the result
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// let bytes: &[u8] = &[0b0110_1011, 0b1100_0001];
    /// let mut r = BitReader::endian(bytes, BigEndian);
    /// assert_eq!(r.read_vbr::<4, u32>().unwrap(), 6);
    /// assert_eq!(r.read_vbr::<4, i32>().unwrap(), -50);
    /// ```
    #[inline]
    fn read_vbr<const FIELD_SIZE: u32, I: VBRInteger>(&mut self) -> io::Result<I> {
        I::read_vbr::<FIELD_SIZE, _>(self)
    }

    /// Creates a "by reference" adaptor for this `BitRead`
    ///
    /// The returned adapter also implements `BitRead`
    /// and will borrow the current reader.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
    ///
    /// fn parse<R: BitRead>(r: R) {
    ///     // perform some parsing
    /// }
    ///
    /// let data: &[u8] = &[0];
    /// let mut reader = BitReader::endian(data, BigEndian);
    /// // performing parsing by reference
    /// parse(reader.by_ref());
    /// // original owned reader still available
    /// assert_eq!(reader.read::<8, u8>().unwrap(), 0);
    /// ```
    #[inline]
    fn by_ref(&mut self) -> &mut Self {
        self
    }
}

impl<R: BitRead + ?Sized> BitRead for &mut R {
    #[inline]
    fn read_bit(&mut self) -> io::Result<bool> {
        (**self).read_bit()
    }

    #[inline]
    fn read<const BITS: u32, I>(&mut self) -> io::Result<I>
    where
        I: Integer,
    {
        (**self).read::<BITS, I>()
    }

    #[inline]
    fn read_var<I>(&mut self, bits: u32) -> io::Result<I>
    where
        I: Integer + Sized,
    {
        (**self).read_var(bits)
    }

    #[inline]
    fn read_count<const MAX: u32>(&mut self) -> io::Result<BitCount<MAX>> {
        (**self).read_count::<MAX>()
    }

    #[inline]
    fn read_counted<const MAX: u32, I>(&mut self, bits: BitCount<MAX>) -> io::Result<I>
    where
        I: Integer + Sized,
    {
        (**self).read_counted::<MAX, I>(bits)
    }

    #[inline]
    fn read_unsigned<const BITS: u32, U>(&mut self) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        (**self).read_unsigned::<BITS, U>()
    }

    #[inline]
    fn read_unsigned_var<U>(&mut self, bits: u32) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        (**self).read_unsigned_var(bits)
    }

    #[inline]
    fn read_unsigned_counted<const MAX: u32, U>(&mut self, bits: BitCount<MAX>) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        (**self).read_unsigned_counted::<MAX, U>(bits)
    }

    #[inline]
    fn read_signed<const BITS: u32, S>(&mut self) -> io::Result<S>
    where
        S: SignedInteger,
    {
        (**self).read_signed::<BITS, S>()
    }

    #[inline]
    fn read_signed_var<S>(&mut self, bits: u32) -> io::Result<S>
    where
        S: SignedInteger,
    {
        (**self).read_signed_var(bits)
    }

    #[inline]
    fn read_signed_counted<const MAX: u32, S>(
        &mut self,
        bits: impl TryInto<SignedBitCount<MAX>>,
    ) -> io::Result<S>
    where
        S: SignedInteger,
    {
        (**self).read_signed_counted::<MAX, S>(bits)
    }

    #[inline]
    fn read_to<V>(&mut self) -> io::Result<V>
    where
        V: Primitive,
    {
        (**self).read_to::<V>()
    }

    #[inline]
    fn read_as_to<F, V>(&mut self) -> io::Result<V>
    where
        F: Endianness,
        V: Primitive,
    {
        (**self).read_as_to::<F, V>()
    }

    #[inline]
    fn skip(&mut self, bits: u32) -> io::Result<()> {
        (**self).skip(bits)
    }

    #[inline]
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        (**self).read_bytes(buf)
    }

    #[inline]
    #[cfg(feature = "alloc")]
    fn read_to_vec(&mut self, bytes: usize) -> io::Result<Vec<u8>> {
        (**self).read_to_vec(bytes)
    }

    #[inline]
    fn read_unary<const STOP_BIT: u8>(&mut self) -> io::Result<u32> {
        (**self).read_unary::<STOP_BIT>()
    }

    #[inline]
    fn parse<F: FromBitStream>(&mut self) -> Result<F, F::Error> {
        (**self).parse::<F>()
    }

    #[inline]
    fn parse_with<'a, F: FromBitStreamWith<'a>>(
        &mut self,
        context: &F::Context,
    ) -> Result<F, F::Error> {
        (**self).parse_with::<F>(context)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        (**self).byte_aligned()
    }

    #[inline]
    fn byte_align(&mut self) {
        (**self).byte_align();
    }

    #[inline]
    fn read_huffman<T>(&mut self) -> io::Result<T::Symbol>
    where
        T: crate::huffman::FromBits,
    {
        (**self).read_huffman::<T>()
    }
}

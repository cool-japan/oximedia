// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Numeric and integer traits.
//!
//! Split out from `lib.rs` during the 0.1.4 refactor so that the crate's
//! core trait surface (`Primitive`, `Integer`, `VBRInteger`, `Numeric`,
//! `UnsignedInteger`, `SignedInteger`) and the macros that implement them
//! for the built-in numeric types live in a single focused file.

#![allow(clippy::unreadable_literal)]

use core::num::NonZero;
use core::ops::{
    BitAnd, BitOr, BitOrAssign, BitXor, Not, Rem, RemAssign, Shl, ShlAssign, Shr, ShrAssign, Sub,
};
use core::{fmt::Debug, mem};
use std::io;

use crate::{BitCount, BitRead, BitWrite};

/// A trait intended for simple fixed-length primitives (such as ints and floats)
/// which allows them to be read and written to streams of
/// different endiannesses verbatim.
pub trait Primitive {
    /// The raw byte representation of this numeric type
    type Bytes: AsRef<[u8]> + AsMut<[u8]>;

    /// An empty buffer of this type's size
    fn buffer() -> Self::Bytes;

    /// Our value in big-endian bytes
    fn to_be_bytes(self) -> Self::Bytes;

    /// Our value in little-endian bytes
    fn to_le_bytes(self) -> Self::Bytes;

    /// Convert big-endian bytes to our value
    fn from_be_bytes(bytes: Self::Bytes) -> Self;

    /// Convert little-endian bytes to our value
    fn from_le_bytes(bytes: Self::Bytes) -> Self;
}

macro_rules! define_primitive_numeric {
    ($t:ty) => {
        impl Primitive for $t {
            type Bytes = [u8; mem::size_of::<$t>()];

            #[inline(always)]
            fn buffer() -> Self::Bytes {
                [0; mem::size_of::<$t>()]
            }
            #[inline(always)]
            fn to_be_bytes(self) -> Self::Bytes {
                self.to_be_bytes()
            }
            #[inline(always)]
            fn to_le_bytes(self) -> Self::Bytes {
                self.to_le_bytes()
            }
            #[inline(always)]
            fn from_be_bytes(bytes: Self::Bytes) -> Self {
                <$t>::from_be_bytes(bytes)
            }
            #[inline(always)]
            fn from_le_bytes(bytes: Self::Bytes) -> Self {
                <$t>::from_le_bytes(bytes)
            }
        }
    };
}

impl<const N: usize> Primitive for [u8; N] {
    type Bytes = [u8; N];

    #[inline(always)]
    fn buffer() -> Self::Bytes {
        [0; N]
    }

    #[inline(always)]
    fn to_be_bytes(self) -> Self::Bytes {
        self
    }

    #[inline(always)]
    fn to_le_bytes(self) -> Self::Bytes {
        self
    }

    #[inline(always)]
    fn from_be_bytes(bytes: Self::Bytes) -> Self {
        bytes
    }

    #[inline(always)]
    fn from_le_bytes(bytes: Self::Bytes) -> Self {
        bytes
    }
}

/// This trait is for integer types which can be read or written
/// to a bit stream as a partial amount of bits.
///
/// It unifies signed and unsigned integer types by delegating
/// reads and writes to the signed and unsigned reading
/// and writing methods as appropriate.
pub trait Integer {
    /// Reads a value of ourself from the stream
    /// with the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// A compile-time error occurs if the given number of bits
    /// is larger than our type.
    fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
    where
        Self: Sized;

    /// Reads a value of ourself from the stream
    /// with the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Also returns an error if our type is too small
    /// to hold the requested number of bits.
    fn read_var<const MAX: u32, R>(reader: &mut R, bits: BitCount<MAX>) -> io::Result<Self>
    where
        R: BitRead + ?Sized,
        Self: Sized;

    /// Writes ourself to the stream using the given const number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if our value is too large
    /// to fit the given number of bits.
    /// A compile-time error occurs if the given number of bits
    /// is larger than our type.
    fn write<const BITS: u32, W: BitWrite + ?Sized>(self, writer: &mut W) -> io::Result<()>;

    /// Writes ourself to the stream using the given number of bits.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// Returns an error if our value is too small
    /// to hold the given number of bits.
    /// Returns an error if our value is too large
    /// to fit the given number of bits.
    fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
        self,
        writer: &mut W,
        bits: BitCount<MAX>,
    ) -> io::Result<()>;
}

/// This trait is for integer types which can be read or written
/// to a bit stream as variable-width integers.
///
/// It unifies signed and unsigned integer types by delegating
/// reads and write to the signed and unsigned vbr reading and
/// writing methods as appropriate.
pub trait VBRInteger: Integer {
    /// Reads a value of ourself from the stream using a variable width integer.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_vbr<const FIELD_SIZE: u32, R>(reader: &mut R) -> io::Result<Self>
    where
        R: BitRead + ?Sized,
        Self: Sized;

    /// Writes ourself to the stream using a variable width integer.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn write_vbr<const FIELD_SIZE: u32, W: BitWrite + ?Sized>(
        self,
        writer: &mut W,
    ) -> io::Result<()>;
}

/// Reading and writing booleans as `Integer` requires the number of bits to be 1.
///
/// This is more useful when combined with the fixed array target
/// for reading blocks of bit flags.
///
/// # Example
/// ```
/// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
///
/// #[derive(Debug, PartialEq, Eq)]
/// struct Flags {
///     a: bool,
///     b: bool,
///     c: bool,
///     d: bool,
/// }
///
/// let data: &[u8] = &[0b1011_0000];
/// let mut r = BitReader::endian(data, BigEndian);
/// // note the number of bits must be 1 per read
/// // while the quantity of flags is indicated by the array length
/// let flags = r.read::<1, [bool; 4]>().map(|[a, b, c, d]| Flags { a, b, c, d }).unwrap();
/// assert_eq!(flags, Flags { a: true, b: false, c: true, d: true });
/// ```
impl Integer for bool {
    #[inline(always)]
    fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
    where
        Self: Sized,
    {
        const {
            assert!(BITS == 1, "booleans require exactly 1 bit");
        }

        reader.read_bit()
    }

    fn read_var<const MAX: u32, R>(
        reader: &mut R,
        BitCount { bits }: BitCount<MAX>,
    ) -> io::Result<Self>
    where
        R: BitRead + ?Sized,
        Self: Sized,
    {
        if bits == 1 {
            reader.read_bit()
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "booleans require exactly 1 bit",
            ))
        }
    }

    #[inline(always)]
    fn write<const BITS: u32, W: BitWrite + ?Sized>(self, writer: &mut W) -> io::Result<()> {
        const {
            assert!(BITS == 1, "booleans require exactly 1 bit");
        }

        writer.write_bit(self)
    }

    fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
        self,
        writer: &mut W,
        BitCount { bits }: BitCount<MAX>,
    ) -> io::Result<()> {
        if bits == 1 {
            writer.write_bit(self)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "booleans require exactly 1 bit",
            ))
        }
    }
}

impl<const SIZE: usize, I: Integer + Copy + Default> Integer for [I; SIZE] {
    #[inline]
    fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
    where
        Self: Sized,
    {
        let mut a = [I::default(); SIZE];

        a.iter_mut().try_for_each(|v| {
            *v = reader.read::<BITS, I>()?;
            Ok::<(), io::Error>(())
        })?;

        Ok(a)
    }

    #[inline]
    fn read_var<const MAX: u32, R>(reader: &mut R, count: BitCount<MAX>) -> io::Result<Self>
    where
        R: BitRead + ?Sized,
        Self: Sized,
    {
        let mut a = [I::default(); SIZE];

        a.iter_mut().try_for_each(|v| {
            *v = reader.read_counted(count)?;
            Ok::<(), io::Error>(())
        })?;

        Ok(a)
    }

    #[inline]
    fn write<const BITS: u32, W: BitWrite + ?Sized>(self, writer: &mut W) -> io::Result<()> {
        IntoIterator::into_iter(self).try_for_each(|v| writer.write::<BITS, I>(v))
    }

    #[inline]
    fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
        self,
        writer: &mut W,
        count: BitCount<MAX>,
    ) -> io::Result<()> {
        IntoIterator::into_iter(self).try_for_each(|v| writer.write_counted(count, v))
    }
}

impl<const SIZE: usize, I: VBRInteger + Copy + Default> VBRInteger for [I; SIZE] {
    fn read_vbr<const FIELD_SIZE: u32, R>(reader: &mut R) -> io::Result<Self>
    where
        R: BitRead + ?Sized,
        Self: Sized,
    {
        let mut a = [I::default(); SIZE];

        a.iter_mut().try_for_each(|v| {
            I::read_vbr::<FIELD_SIZE, R>(reader).map(|item| {
                *v = item;
            })
        })?;

        Ok(a)
    }

    fn write_vbr<const FIELD_SIZE: u32, W: BitWrite + ?Sized>(
        self,
        writer: &mut W,
    ) -> io::Result<()> {
        IntoIterator::into_iter(self).try_for_each(|v| I::write_vbr::<FIELD_SIZE, W>(v, writer))
    }
}

/// This trait extends many common integer types (both unsigned and signed)
/// with a few trivial methods so that they can be used
/// with the bitstream handling traits.
pub trait Numeric:
    Primitive
    + Sized
    + Copy
    + Default
    + Debug
    + PartialOrd
    + Shl<u32, Output = Self>
    + ShlAssign<u32>
    + Shr<u32, Output = Self>
    + ShrAssign<u32>
    + Rem<Self, Output = Self>
    + RemAssign<Self>
    + BitAnd<Self, Output = Self>
    + BitOr<Self, Output = Self>
    + BitOrAssign<Self>
    + BitXor<Self, Output = Self>
    + Not<Output = Self>
    + Sub<Self, Output = Self>
{
    /// Size of type in bits
    const BITS_SIZE: u32;

    /// The value of 0 in this type
    const ZERO: Self;

    /// The value of 1 in this type
    const ONE: Self;

    /// Returns a `u8` value in this type
    fn from_u8(u: u8) -> Self;

    /// Assuming 0 <= value < 256, returns this value as a `u8` type
    fn to_u8(self) -> u8;
}

macro_rules! define_numeric {
    ($t:ty) => {
        define_primitive_numeric!($t);

        impl Numeric for $t {
            const BITS_SIZE: u32 = mem::size_of::<$t>() as u32 * 8;

            const ZERO: Self = 0;

            const ONE: Self = 1;

            #[inline(always)]
            fn from_u8(u: u8) -> Self {
                u as $t
            }
            #[inline(always)]
            fn to_u8(self) -> u8 {
                self as u8
            }
        }
    };
}

/// This trait extends many common unsigned integer types
/// so that they can be used with the bitstream handling traits.
pub trait UnsignedInteger: Numeric {
    /// This type's most-significant bit
    const MSB_BIT: Self;

    /// This type's least significant bit
    const LSB_BIT: Self;

    /// This type with all bits set
    const ALL: Self;

    /// The signed variant of ourself
    type Signed: SignedInteger<Unsigned = Self>;

    /// Given a twos-complement value,
    /// return this value is a non-negative signed number.
    /// The location of the sign bit depends on the stream's endianness
    /// and is not stored in the result.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::UnsignedInteger;
    /// assert_eq!(0b00000001u8.as_non_negative(), 1i8);
    /// ```
    fn as_non_negative(self) -> Self::Signed;

    /// Given a two-complement positive value and certain number of bits,
    /// returns this value as a negative signed number.
    /// The location of the sign bit depends on the stream's endianness
    /// and is not stored in the result.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::UnsignedInteger;
    /// assert_eq!(0b01111111u8.as_negative(8), -1i8);
    /// ```
    fn as_negative(self, bits: u32) -> Self::Signed;

    /// Given a two-complement positive value and certain number of bits,
    /// returns this value as a negative number.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::UnsignedInteger;
    /// assert_eq!(0b01111111u8.as_negative_fixed::<8>(), -1i8);
    /// ```
    fn as_negative_fixed<const BITS: u32>(self) -> Self::Signed;

    /// Checked shift left
    fn checked_shl(self, rhs: u32) -> Option<Self>;

    /// Checked shift right
    fn checked_shr(self, rhs: u32) -> Option<Self>;

    /// Shift left up to our length in bits
    ///
    /// If rhs equals our length in bits, returns default
    fn shl_default(self, rhs: u32) -> Self {
        self.checked_shl(rhs).unwrap_or(Self::ZERO)
    }

    /// Shift left up to our length in bits
    ///
    /// If rhs equals our length in bits, returns zero
    fn shr_default(self, rhs: u32) -> Self {
        self.checked_shr(rhs).unwrap_or(Self::ZERO)
    }
}

macro_rules! define_unsigned_integer {
    ($t:ty, $s:ty) => {
        define_numeric!($t);

        impl UnsignedInteger for $t {
            type Signed = $s;

            const MSB_BIT: Self = 1 << (Self::BITS_SIZE - 1);

            const LSB_BIT: Self = 1;

            const ALL: Self = <$t>::MAX;

            #[inline(always)]
            fn as_non_negative(self) -> Self::Signed {
                self as $s
            }
            #[inline(always)]
            fn as_negative(self, bits: u32) -> Self::Signed {
                (self as $s) + (-1 << (bits - 1))
            }
            #[inline(always)]
            fn as_negative_fixed<const BITS: u32>(self) -> Self::Signed {
                (self as $s) + (-1 << (BITS - 1))
            }
            #[inline(always)]
            fn checked_shl(self, rhs: u32) -> Option<Self> {
                self.checked_shl(rhs)
            }
            #[inline(always)]
            fn checked_shr(self, rhs: u32) -> Option<Self> {
                self.checked_shr(rhs)
            }
            // Override the trait default with the primitive intrinsic.
            // `unbounded_shl`/`unbounded_shr` are stable since Rust 1.87 and
            // return 0 when rhs >= BITS — identical semantics to the default
            // `checked_shl(rhs).unwrap_or(ZERO)`, but branchless.
            #[inline(always)]
            fn shl_default(self, rhs: u32) -> Self {
                self.unbounded_shl(rhs)
            }
            #[inline(always)]
            fn shr_default(self, rhs: u32) -> Self {
                self.unbounded_shr(rhs)
            }
        }

        impl Integer for $t {
            #[inline(always)]
            fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
            where
                Self: Sized,
            {
                reader.read_unsigned::<BITS, _>()
            }

            #[inline(always)]
            fn read_var<const MAX: u32, R>(reader: &mut R, bits: BitCount<MAX>) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                reader.read_unsigned_counted::<MAX, _>(bits)
            }

            #[inline(always)]
            fn write<const BITS: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                writer.write_unsigned::<BITS, _>(self)
            }

            #[inline(always)]
            fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
                bits: BitCount<MAX>,
            ) -> io::Result<()> {
                writer.write_unsigned_counted(bits, self)
            }
        }

        impl VBRInteger for $t {
            #[inline(always)]
            fn read_vbr<const FIELD_SIZE: u32, R>(reader: &mut R) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                reader.read_unsigned_vbr::<FIELD_SIZE, _>()
            }

            #[inline(always)]
            fn write_vbr<const FIELD_SIZE: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                writer.write_unsigned_vbr::<FIELD_SIZE, _>(self)
            }
        }

        /// Unsigned NonZero types increment their value by 1
        /// when being read and decrement it by 1
        /// when being written.
        ///
        /// # Examples
        /// ```
        /// use oximedia_bitstream::{BitReader, BitRead, BigEndian};
        /// use core::num::NonZero;
        ///
        /// let data: &[u8] = &[0b001_00000];
        /// // reading a regular u8 in 3 bits yields 1
        /// assert_eq!(BitReader::endian(data, BigEndian).read::<3, u8>().unwrap(), 1);
        /// // reading a NonZero<u8> in 3 bits of the same data yields 2
        /// assert_eq!(BitReader::endian(data, BigEndian).read::<3, NonZero<u8>>().unwrap().get(), 2);
        /// ```
        ///
        /// ```
        /// use oximedia_bitstream::{BitWriter, BitWrite, BigEndian};
        /// use core::num::NonZero;
        ///
        /// let mut w = BitWriter::endian(vec![], BigEndian);
        /// // writing 1 as a regular u8 in 3 bits
        /// w.write::<3, u8>(1).unwrap();
        /// w.byte_align();
        /// assert_eq!(w.into_writer(), &[0b001_00000]);
        ///
        /// let mut w = BitWriter::endian(vec![], BigEndian);
        /// // writing 1 as a NonZero<u8> in 3 bits
        /// w.write::<3, NonZero<u8>>(NonZero::new(1).unwrap()).unwrap();
        /// w.byte_align();
        /// assert_eq!(w.into_writer(), &[0b000_00000]);
        /// ```
        impl Integer for NonZero<$t> {
            #[inline]
            fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
            where
                Self: Sized,
            {
                const {
                    assert!(
                        BITS < <$t>::BITS_SIZE,
                        "BITS must be less than the type's size in bits"
                    );
                }

                <$t as Integer>::read::<BITS, R>(reader).map(|u| {
                    // `BITS < BITS_SIZE` (enforced by `const { assert! }` above),
                    // so `u <= 2^BITS - 1 <= T::MAX - 1`, meaning `u + 1` never
                    // overflows to zero.
                    NonZero::new(u + 1)
                        .expect("u + 1 is nonzero: BITS < BITS_SIZE guarantees no overflow")
                })
            }

            #[inline]
            fn read_var<const MAX: u32, R>(
                reader: &mut R,
                count @ BitCount { bits }: BitCount<MAX>,
            ) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                if MAX < <$t>::BITS_SIZE || bits < <$t>::BITS_SIZE {
                    <$t as Integer>::read_var::<MAX, R>(reader, count).map(|u| {
                        // `bits < BITS_SIZE` is enforced by the guard above, so
                        // `u <= 2^bits - 1 <= T::MAX - 1`, meaning `u + 1` never
                        // overflows to zero.
                        NonZero::new(u + 1)
                            .expect("u + 1 is nonzero: bits < BITS_SIZE guarantees no overflow")
                    })
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "bit count must be less than the type's size in bits",
                    ))
                }
            }

            #[inline]
            fn write<const BITS: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                const {
                    assert!(
                        BITS < <$t>::BITS_SIZE,
                        "BITS must be less than the type's size in bits"
                    );
                }

                <$t as Integer>::write::<BITS, W>(self.get() - 1, writer)
            }

            #[inline]
            fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
                count @ BitCount { bits }: BitCount<MAX>,
            ) -> io::Result<()> {
                if MAX < <$t>::BITS_SIZE || bits < <$t>::BITS_SIZE {
                    <$t as Integer>::write_var::<MAX, W>(self.get() - 1, writer, count)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "bit count must be less than the type's size in bits",
                    ))
                }
            }
        }

        impl VBRInteger for NonZero<$t> {
            #[inline]
            fn read_vbr<const FIELD_SIZE: u32, R>(reader: &mut R) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                <$t as VBRInteger>::read_vbr::<FIELD_SIZE, R>(reader).map(|u| {
                    // VBR groups encode at most `FIELD_SIZE - 1` data bits per chunk,
                    // so the accumulated value fits within the type and `u + 1`
                    // never overflows to zero.
                    NonZero::new(u + 1)
                        .expect("u + 1 is nonzero: VBR field size bounds prevent overflow")
                })
            }

            #[inline]
            fn write_vbr<const FIELD_SIZE: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                <$t as VBRInteger>::write_vbr::<FIELD_SIZE, W>(self.get() - 1, writer)
            }
        }

        impl Integer for Option<NonZero<$t>> {
            #[inline]
            fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
            where
                Self: Sized,
            {
                <$t as Integer>::read::<BITS, R>(reader).map(NonZero::new)
            }

            #[inline]
            fn read_var<const MAX: u32, R>(reader: &mut R, count: BitCount<MAX>) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                <$t as Integer>::read_var::<MAX, R>(reader, count).map(NonZero::new)
            }

            #[inline]
            fn write<const BITS: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                <$t as Integer>::write::<BITS, W>(self.map(|n| n.get()).unwrap_or(0), writer)
            }

            #[inline]
            fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
                count: BitCount<MAX>,
            ) -> io::Result<()> {
                <$t as Integer>::write_var::<MAX, W>(
                    self.map(|n| n.get()).unwrap_or(0),
                    writer,
                    count,
                )
            }
        }

        impl VBRInteger for Option<NonZero<$t>> {
            #[inline(always)]
            fn read_vbr<const FIELD_SIZE: u32, R>(reader: &mut R) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                <$t as VBRInteger>::read_vbr::<FIELD_SIZE, _>(reader).map(NonZero::new)
            }

            #[inline]
            fn write_vbr<const FIELD_SIZE: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                <$t as VBRInteger>::write_vbr::<FIELD_SIZE, W>(
                    self.map(|n| n.get()).unwrap_or(0),
                    writer,
                )
            }
        }
    };
}

/// This trait extends many common signed integer types
/// so that they can be used with the bitstream handling traits.
///
/// This trait was formerly named `SignedNumeric` in 2.X.X code.
/// If backwards-compatibility is needed one can
/// import `SignedInteger` as `SignedNumeric`.
pub trait SignedInteger: Numeric {
    /// The unsigned variant of ourself
    type Unsigned: UnsignedInteger<Signed = Self>;

    /// Returns true if this value is negative
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::SignedInteger;
    /// assert!(!1i8.is_negative());
    /// assert!((-1i8).is_negative());
    /// ```
    fn is_negative(self) -> bool;

    /// Returns ourself as a non-negative value.
    /// The location of the sign bit depends on the stream's endianness
    /// and is not stored in the result.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::SignedInteger;
    /// assert_eq!(1i8.as_non_negative(), 0b00000001u8);
    /// ```
    fn as_non_negative(self) -> Self::Unsigned;

    /// Given a negative value and a certain number of bits,
    /// returns this value as a twos-complement positive number.
    /// The location of the sign bit depends on the stream's endianness
    /// and is not stored in the result.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::SignedInteger;
    /// assert_eq!((-1i8).as_negative(8), 0b01111111u8);
    /// ```
    fn as_negative(self, bits: u32) -> Self::Unsigned;

    /// Given a negative value and a certain number of bits,
    /// returns this value as a twos-complement positive number.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::SignedInteger;
    /// assert_eq!((-1i8).as_negative_fixed::<8>(), 0b01111111u8);
    /// ```
    fn as_negative_fixed<const BITS: u32>(self) -> Self::Unsigned;
}

macro_rules! define_signed_integer {
    ($t:ty, $u:ty) => {
        define_numeric!($t);

        impl SignedInteger for $t {
            type Unsigned = $u;

            #[inline(always)]
            fn is_negative(self) -> bool {
                self.is_negative()
            }
            fn as_non_negative(self) -> Self::Unsigned {
                self as $u
            }
            fn as_negative(self, bits: u32) -> Self::Unsigned {
                (self - (-1 << (bits - 1))) as $u
            }
            fn as_negative_fixed<const BITS: u32>(self) -> Self::Unsigned {
                (self - (-1 << (BITS - 1))) as $u
            }
        }

        impl Integer for $t {
            #[inline(always)]
            fn read<const BITS: u32, R: BitRead + ?Sized>(reader: &mut R) -> io::Result<Self>
            where
                Self: Sized,
            {
                reader.read_signed::<BITS, _>()
            }

            #[inline(always)]
            fn read_var<const MAX: u32, R>(reader: &mut R, bits: BitCount<MAX>) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                reader.read_signed_counted::<MAX, _>(bits)
            }

            #[inline(always)]
            fn write<const BITS: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                writer.write_signed::<BITS, _>(self)
            }

            #[inline(always)]
            fn write_var<const MAX: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
                bits: BitCount<MAX>,
            ) -> io::Result<()> {
                writer.write_signed_counted::<MAX, _>(bits, self)
            }
        }

        impl VBRInteger for $t {
            #[inline(always)]
            fn read_vbr<const FIELD_SIZE: u32, R>(reader: &mut R) -> io::Result<Self>
            where
                R: BitRead + ?Sized,
                Self: Sized,
            {
                reader.read_signed_vbr::<FIELD_SIZE, _>()
            }

            #[inline(always)]
            fn write_vbr<const FIELD_SIZE: u32, W: BitWrite + ?Sized>(
                self,
                writer: &mut W,
            ) -> io::Result<()> {
                writer.write_signed_vbr::<FIELD_SIZE, _>(self)
            }
        }
    };
}

define_unsigned_integer!(u8, i8);
define_unsigned_integer!(u16, i16);
define_unsigned_integer!(u32, i32);
define_unsigned_integer!(u64, i64);
define_unsigned_integer!(u128, i128);

define_signed_integer!(i8, u8);
define_signed_integer!(i16, u16);
define_signed_integer!(i32, u32);
define_signed_integer!(i64, u64);
define_signed_integer!(i128, u128);

define_primitive_numeric!(f32);
define_primitive_numeric!(f64);

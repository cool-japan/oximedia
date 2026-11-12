// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Pre-validated write helpers — `Checked`, `CheckedError`, `FixedBitCount`,
//! `FixedSignedBitCount`, and the `Checkable` / `CheckablePrimitive` traits.
//!
//! Split out of `lib.rs` during the 0.1.4 refactor.

use std::io;

use crate::{private, BitCount, BitRead, BitWrite, SignedBitCount, SignedInteger, UnsignedInteger};

/// An error when converting a value to a [`Checked`] struct
#[derive(Copy, Clone, Debug)]
pub enum CheckedError {
    /// Excessive bits for type
    ExcessiveBits,
    /// Excessive value for bits
    ExcessiveValue,
}

impl core::error::Error for CheckedError {}

impl core::fmt::Display for CheckedError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Self::ExcessiveBits => core::fmt::Display::fmt("excessive bits for type written", f),
            Self::ExcessiveValue => core::fmt::Display::fmt("excessive value for bits written", f),
        }
    }
}

impl From<CheckedError> for io::Error {
    #[inline]
    fn from(error: CheckedError) -> Self {
        match error {
            CheckedError::ExcessiveBits => io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive bits for type written",
            ),
            CheckedError::ExcessiveValue => io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive value for bits written",
            ),
        }
    }
}

/// A type for eliminating redundant validation when writing
///
/// Normally, when writing a value, not only must the number of bits
/// must be checked against the type being written
/// (e.g. writing 9 bits from a `u8` is always an error),
/// but the value must also be checked against the number of bits
/// (e.g. writing a value of 2 in 1 bit is always an error).
///
/// But when the value's range can be checked in advance,
/// the write-time check can be skipped through the use
/// of the [`BitWrite::write_checked`] method.
#[derive(Copy, Clone, Debug)]
pub struct Checked<C, T> {
    pub(crate) count: C,
    pub(crate) value: T,
}

impl<C, T> Checked<C, T> {
    /// Returns our bit count and value
    #[inline]
    pub fn into_count_value(self) -> (C, T) {
        (self.count, self.value)
    }

    /// Returns our value
    #[inline]
    pub fn into_value(self) -> T {
        self.value
    }
}

impl<C, T> AsRef<T> for Checked<C, T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

/// An unsigned type with a verified value
pub type CheckedUnsigned<const MAX: u32, T> = Checked<BitCount<MAX>, T>;

impl<const MAX: u32, U: UnsignedInteger> Checkable for CheckedUnsigned<MAX, U> {
    #[inline]
    fn write<W: BitWrite + ?Sized>(&self, writer: &mut W) -> io::Result<()> {
        // a naive default implementation
        writer.write_unsigned_counted(self.count, self.value)
    }

    #[inline]
    fn written_bits(&self) -> u32 {
        self.count.bits
    }
}

impl<const MAX: u32, U: UnsignedInteger> CheckablePrimitive for CheckedUnsigned<MAX, U> {
    type CountType = BitCount<MAX>;

    #[inline]
    fn read<R: BitRead + ?Sized>(reader: &mut R, count: Self::CountType) -> io::Result<Self> {
        reader
            .read_unsigned_counted(count)
            .map(|value| Self { count, value })
    }
}

impl<const MAX: u32, U: UnsignedInteger> private::Checkable for CheckedUnsigned<MAX, U> {
    fn write_endian<E, W>(
        self,
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<()>
    where
        E: private::Endianness,
        W: io::Write,
    {
        E::write_bits_checked(writer, queue_value, queue_bits, self)
    }
}

impl<const MAX: u32, U: UnsignedInteger> CheckedUnsigned<MAX, U> {
    /// Returns our value if it fits in the given number of bits
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{BitCount, CheckedUnsigned, CheckedError};
    ///
    /// // a value of 7 fits into a 3 bit count
    /// assert!(CheckedUnsigned::<8, u8>::new(3, 0b111).is_ok());
    ///
    /// // a value of 8 does not fit into a 3 bit count
    /// assert!(matches!(
    ///     CheckedUnsigned::<8, u8>::new(3, 0b1000),
    ///     Err(CheckedError::ExcessiveValue),
    /// ));
    ///
    /// // a bit count of 9 is too large for u8
    /// assert!(matches!(
    ///     CheckedUnsigned::<9, _>::new(9, 1u8),
    ///     Err(CheckedError::ExcessiveBits),
    /// ));
    /// ```
    #[inline]
    pub fn new(count: impl TryInto<BitCount<MAX>>, value: U) -> Result<Self, CheckedError> {
        let count @ BitCount { bits } =
            count.try_into().map_err(|_| CheckedError::ExcessiveBits)?;

        if MAX <= U::BITS_SIZE || bits <= U::BITS_SIZE {
            if bits == 0 {
                Ok(Self {
                    count,
                    value: U::ZERO,
                })
            } else if value <= U::ALL >> (U::BITS_SIZE - bits) {
                Ok(Self { count, value })
            } else {
                Err(CheckedError::ExcessiveValue)
            }
        } else {
            Err(CheckedError::ExcessiveBits)
        }
    }

    /// Returns our value if it fits in the given number of const bits
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{CheckedUnsigned, CheckedError};
    ///
    /// // a value of 7 fits into a 3 bit count
    /// assert!(CheckedUnsigned::<8, u8>::new_fixed::<3>(0b111).is_ok());
    ///
    /// // a value of 8 does not fit into a 3 bit count
    /// assert!(matches!(
    ///     CheckedUnsigned::<8, u8>::new_fixed::<3>(0b1000),
    ///     Err(CheckedError::ExcessiveValue),
    /// ));
    /// ```
    ///
    /// ```compile_fail
    /// use oximedia_bitstream::{BitCount, CheckedUnsigned};
    ///
    /// // a bit count of 9 is too large for u8
    ///
    /// // because this is checked at compile-time,
    /// // it does not compile at all
    /// let c = CheckedUnsigned::<16, u8>::new_fixed::<9>(1);
    /// ```
    pub fn new_fixed<const BITS: u32>(value: U) -> Result<Self, CheckedError> {
        const {
            assert!(BITS <= U::BITS_SIZE, "excessive bits for type written");
        }

        if BITS == 0 {
            Ok(Self {
                count: BitCount::new::<0>(),
                value: U::ZERO,
            })
        } else if BITS == U::BITS_SIZE || value <= (U::ALL >> (U::BITS_SIZE - BITS)) {
            Ok(Self {
                // whether BITS is larger than MAX is checked here
                count: BitCount::new::<BITS>(),
                value,
            })
        } else {
            Err(CheckedError::ExcessiveValue)
        }
    }
}

/// A fixed number of bits to be consumed or written
///
/// Analagous to [`BitCount`], this is a zero-sized type
/// whose value is fixed at compile-time and cannot be changed.
///
/// # Example
///
/// ```
/// use oximedia_bitstream::{
///     BigEndian, BitRead, BitReader, BitWrite, BitWriter, CheckedUnsignedFixed, FixedBitCount,
/// };
///
/// type FourBits = CheckedUnsignedFixed<4, u8>;
///
/// let input: &[u8] = &[0b0001_1111, 0b0110_1001];
/// let mut r = BitReader::endian(input, BigEndian);
///
/// // read 4, 4-bit values
/// let v1 = r.read_checked::<FourBits>(FixedBitCount).unwrap();
/// let v2 = r.read_checked::<FourBits>(FixedBitCount).unwrap();
/// let v3 = r.read_checked::<FourBits>(FixedBitCount).unwrap();
/// let v4 = r.read_checked::<FourBits>(FixedBitCount).unwrap();
///
/// assert_eq!(v1.into_value(), 0b0001);
/// assert_eq!(v2.into_value(), 0b1111);
/// assert_eq!(v3.into_value(), 0b0110);
/// assert_eq!(v4.into_value(), 0b1001);
///
/// // write those same values back to disk
/// let mut w = BitWriter::endian(vec![], BigEndian);
/// w.write_checked(v1).unwrap();
/// w.write_checked(v2).unwrap();
/// w.write_checked(v3).unwrap();
/// w.write_checked(v4).unwrap();
///
/// // ensure they're the same
/// assert_eq!(w.into_writer().as_slice(), input);
/// ```
#[derive(Copy, Clone, Debug)]
pub struct FixedBitCount<const BITS: u32>;

impl<const BITS: u32> From<FixedBitCount<BITS>> for BitCount<BITS> {
    fn from(_count: FixedBitCount<BITS>) -> Self {
        BitCount::new::<BITS>()
    }
}

impl<const BITS: u32, const MAX: u32> core::convert::TryFrom<BitCount<MAX>>
    for FixedBitCount<BITS>
{
    type Error = BitCount<MAX>;

    fn try_from(count: BitCount<MAX>) -> Result<Self, Self::Error> {
        (count.bits == BITS).then_some(FixedBitCount).ok_or(count)
    }
}

/// An unsigned type with a verified value for a fixed number of bits
pub type CheckedUnsignedFixed<const BITS: u32, T> = Checked<FixedBitCount<BITS>, T>;

impl<const BITS: u32, U: UnsignedInteger> CheckedUnsignedFixed<BITS, U> {
    /// Returns our checked value if it fits in the given number of const bits
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{CheckedUnsignedFixed, CheckedError};
    ///
    /// // a value of 7 fits into a maximum of 3 bits
    /// assert!(CheckedUnsignedFixed::<3, u8>::new_fixed(0b111).is_ok());
    ///
    /// // a value of 8 does not fit into a maximum of 3 bits
    /// assert!(matches!(
    ///     CheckedUnsignedFixed::<3, u8>::new_fixed(0b1000),
    ///     Err(CheckedError::ExcessiveValue),
    /// ));
    /// ```
    ///
    /// ```compile_fail
    /// use oximedia_bitstream::CheckedUnsignedFixed;
    ///
    /// // a bit count of 9 is too large for u8
    ///
    /// // because this is checked at compile-time,
    /// // it does not compile at all
    /// let c = CheckedUnsignedFixed::<9, u8>::new_fixed(1);
    /// ```
    pub fn new_fixed(value: U) -> Result<Self, CheckedError> {
        const {
            assert!(BITS <= U::BITS_SIZE, "excessive bits for type written");
        }

        if BITS == 0 {
            Ok(Self {
                count: FixedBitCount,
                value: U::ZERO,
            })
        } else if BITS == U::BITS_SIZE || value <= (U::ALL >> (U::BITS_SIZE - BITS)) {
            Ok(Self {
                count: FixedBitCount,
                value,
            })
        } else {
            Err(CheckedError::ExcessiveValue)
        }
    }
}

impl<const BITS: u32, U: UnsignedInteger> Checkable for CheckedUnsignedFixed<BITS, U> {
    #[inline]
    fn write<W: BitWrite + ?Sized>(&self, writer: &mut W) -> io::Result<()> {
        // a naive default implementation
        writer.write_unsigned::<BITS, _>(self.value)
    }

    #[inline]
    fn written_bits(&self) -> u32 {
        BITS
    }
}

impl<const BITS: u32, U: UnsignedInteger> private::Checkable for CheckedUnsignedFixed<BITS, U> {
    fn write_endian<E, W>(
        self,
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<()>
    where
        E: private::Endianness,
        W: io::Write,
    {
        E::write_bits_checked(
            writer,
            queue_value,
            queue_bits,
            Checked {
                value: self.value,
                count: self.count.into(),
            },
        )
    }
}

impl<const BITS: u32, U: UnsignedInteger> CheckablePrimitive for CheckedUnsignedFixed<BITS, U> {
    type CountType = FixedBitCount<BITS>;

    fn read<R: BitRead + ?Sized>(reader: &mut R, count: FixedBitCount<BITS>) -> io::Result<Self> {
        Ok(Self {
            value: reader.read_unsigned::<BITS, _>()?,
            count,
        })
    }
}

/// A signed type with a verified value
pub type CheckedSigned<const MAX: u32, T> = Checked<SignedBitCount<MAX>, T>;

impl<const MAX: u32, S: SignedInteger> Checkable for CheckedSigned<MAX, S> {
    #[inline]
    fn write<W: BitWrite + ?Sized>(&self, writer: &mut W) -> io::Result<()> {
        // a naive default implementation
        writer.write_signed_counted(self.count, self.value)
    }

    #[inline]
    fn written_bits(&self) -> u32 {
        self.count.bits.into()
    }
}

impl<const MAX: u32, S: SignedInteger> CheckablePrimitive for CheckedSigned<MAX, S> {
    type CountType = SignedBitCount<MAX>;

    #[inline]
    fn read<R: BitRead + ?Sized>(reader: &mut R, count: Self::CountType) -> io::Result<Self> {
        reader
            .read_signed_counted(count)
            .map(|value| Self { count, value })
    }
}

impl<const MAX: u32, S: SignedInteger> private::Checkable for CheckedSigned<MAX, S> {
    #[inline]
    fn write_endian<E, W>(
        self,
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<()>
    where
        E: private::Endianness,
        W: io::Write,
    {
        E::write_signed_bits_checked(writer, queue_value, queue_bits, self)
    }
}

impl<const MAX: u32, S: SignedInteger> CheckedSigned<MAX, S> {
    /// Returns our value if it fits in the given number of bits
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{SignedBitCount, CheckedSigned, CheckedError};
    ///
    /// // a value of 3 fits into a 3 bit count
    /// assert!(CheckedSigned::<8, _>::new(3, 3i8).is_ok());
    ///
    /// // a value of 4 does not fit into a 3 bit count
    /// assert!(matches!(
    ///     CheckedSigned::<8, _>::new(3, 4i8),
    ///     Err(CheckedError::ExcessiveValue),
    /// ));
    ///
    /// // a bit count of 9 is too large for i8
    /// assert!(matches!(
    ///     CheckedSigned::<9, _>::new(9, 1i8),
    ///     Err(CheckedError::ExcessiveBits),
    /// ));
    /// ```
    #[inline]
    pub fn new(count: impl TryInto<SignedBitCount<MAX>>, value: S) -> Result<Self, CheckedError> {
        let count @ SignedBitCount {
            bits: BitCount { bits },
            unsigned: BitCount {
                bits: unsigned_bits,
            },
        } = count.try_into().map_err(|_| CheckedError::ExcessiveBits)?;

        if MAX <= S::BITS_SIZE || bits <= S::BITS_SIZE {
            if bits == S::BITS_SIZE
                || (((S::ZERO - S::ONE) << unsigned_bits) <= value
                    && value < (S::ONE << unsigned_bits))
            {
                Ok(Self { count, value })
            } else {
                Err(CheckedError::ExcessiveValue)
            }
        } else {
            Err(CheckedError::ExcessiveBits)
        }
    }

    /// Returns our value if it fits in the given number of const bits
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{CheckedSigned, CheckedError};
    ///
    /// // a value of 3 fits into a 3 bit count
    /// assert!(CheckedSigned::<8, i8>::new_fixed::<3>(3).is_ok());
    ///
    /// // a value of 4 does not fit into a 3 bit count
    /// assert!(matches!(
    ///     CheckedSigned::<8, i8>::new_fixed::<3>(4),
    ///     Err(CheckedError::ExcessiveValue),
    /// ));
    /// ```
    ///
    /// ```compile_fail
    /// use oximedia_bitstream::{BitCount, CheckedSigned};
    ///
    /// // a bit count of 9 is too large for i8
    ///
    /// // because this is checked at compile-time,
    /// // it does not compile at all
    /// let c = CheckedSigned::<16, i8>::new_fixed::<9>(1);
    /// ```
    pub fn new_fixed<const BITS: u32>(value: S) -> Result<Self, CheckedError> {
        const {
            assert!(BITS <= S::BITS_SIZE, "excessive bits for type written");
        }

        if BITS == S::BITS_SIZE
            || (((S::ZERO - S::ONE) << (BITS - 1)) <= value && value < (S::ONE << (BITS - 1)))
        {
            Ok(Self {
                count: SignedBitCount::new::<BITS>(),
                value,
            })
        } else {
            Err(CheckedError::ExcessiveValue)
        }
    }
}

/// A fixed number of bits to be consumed or written
///
/// Analagous to [`SignedBitCount`], this is a zero-sized type
/// whose value is fixed at compile-time and cannot be changed.
///
/// # Example
///
/// ```
/// use oximedia_bitstream::{
///     BigEndian, BitRead, BitReader, BitWrite, BitWriter,
///     CheckedSignedFixed, FixedSignedBitCount,
/// };
///
/// type FourBits = CheckedSignedFixed<4, i8>;
///
/// let input: &[u8] = &[0b0001_1111, 0b0110_1001];
/// let mut r = BitReader::endian(input, BigEndian);
///
/// // read 4, 4-bit values
/// let v1 = r.read_checked::<FourBits>(FixedSignedBitCount).unwrap();
/// let v2 = r.read_checked::<FourBits>(FixedSignedBitCount).unwrap();
/// let v3 = r.read_checked::<FourBits>(FixedSignedBitCount).unwrap();
/// let v4 = r.read_checked::<FourBits>(FixedSignedBitCount).unwrap();
///
/// assert_eq!(v1.into_value(), 1);
/// assert_eq!(v2.into_value(), -1);
/// assert_eq!(v3.into_value(), 6);
/// assert_eq!(v4.into_value(), -7);
///
/// // write those same values back to disk
/// let mut w = BitWriter::endian(vec![], BigEndian);
/// w.write_checked(v1).unwrap();
/// w.write_checked(v2).unwrap();
/// w.write_checked(v3).unwrap();
/// w.write_checked(v4).unwrap();
///
/// // ensure they're the same
/// assert_eq!(w.into_writer().as_slice(), input);
/// ```
#[derive(Copy, Clone, Debug)]
pub struct FixedSignedBitCount<const BITS: u32>;

impl<const BITS: u32> From<FixedSignedBitCount<BITS>> for SignedBitCount<BITS> {
    fn from(_count: FixedSignedBitCount<BITS>) -> Self {
        SignedBitCount::new::<BITS>()
    }
}

impl<const BITS: u32, const MAX: u32> core::convert::TryFrom<SignedBitCount<MAX>>
    for FixedSignedBitCount<BITS>
{
    type Error = SignedBitCount<MAX>;

    fn try_from(count: SignedBitCount<MAX>) -> Result<Self, Self::Error> {
        (count.bits.bits == BITS)
            .then_some(FixedSignedBitCount)
            .ok_or(count)
    }
}

/// A signed type with a verified value for a fixed number of bits
pub type CheckedSignedFixed<const BITS: u32, T> = Checked<FixedSignedBitCount<BITS>, T>;

impl<const BITS: u32, S: SignedInteger> CheckedSignedFixed<BITS, S> {
    /// Returns our checked value if it fits in the given number of const bits
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{SignedBitCount, CheckedSignedFixed, CheckedError};
    ///
    /// // a value of 3 fits into a 3 bit count
    /// assert!(CheckedSignedFixed::<3, _>::new_fixed(3i8).is_ok());
    ///
    /// // a value of 4 does not fit into a 3 bit count
    /// assert!(matches!(
    ///     CheckedSignedFixed::<3, _>::new_fixed(4i8),
    ///     Err(CheckedError::ExcessiveValue),
    /// ));
    /// ```
    ///
    /// ```compile_fail
    /// use oximedia_bitstream::CheckedSignedFixed;
    ///
    /// // a bit count of 9 is too large for i8
    ///
    /// // because this is checked at compile-time,
    /// // it does not compile at all
    /// let c = CheckedSignedFixed::<9, _>::new_fixed(1i8);
    /// ```
    pub fn new_fixed(value: S) -> Result<Self, CheckedError> {
        const {
            assert!(BITS <= S::BITS_SIZE, "excessive bits for type written");
        }

        if BITS == S::BITS_SIZE
            || (((S::ZERO - S::ONE) << (BITS - 1)) <= value && value < (S::ONE << (BITS - 1)))
        {
            Ok(Self {
                count: FixedSignedBitCount,
                value,
            })
        } else {
            Err(CheckedError::ExcessiveValue)
        }
    }
}
impl<const BITS: u32, S: SignedInteger> Checkable for CheckedSignedFixed<BITS, S> {
    #[inline]
    fn write<W: BitWrite + ?Sized>(&self, writer: &mut W) -> io::Result<()> {
        // a naive default implementation
        writer.write_signed::<BITS, _>(self.value)
    }

    #[inline]
    fn written_bits(&self) -> u32 {
        BITS
    }
}

impl<const BITS: u32, S: SignedInteger> private::Checkable for CheckedSignedFixed<BITS, S> {
    #[inline]
    fn write_endian<E, W>(
        self,
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<()>
    where
        E: private::Endianness,
        W: io::Write,
    {
        E::write_signed_bits_checked(
            writer,
            queue_value,
            queue_bits,
            CheckedSigned {
                value: self.value,
                count: self.count.into(),
            },
        )
    }
}

impl<const BITS: u32, S: SignedInteger> CheckablePrimitive for CheckedSignedFixed<BITS, S> {
    type CountType = FixedSignedBitCount<BITS>;

    fn read<R: BitRead + ?Sized>(
        reader: &mut R,
        count: FixedSignedBitCount<BITS>,
    ) -> io::Result<Self> {
        Ok(Self {
            value: reader.read_signed::<BITS, _>()?,
            count,
        })
    }
}

/// A trait for writable types whose values can be validated
///
/// Ordinarily, when writing a value to a stream with a given
/// number of bits, the value must be validated to ensure
/// it will fit within that number of bits.
///
/// # Example 1
///
/// ```
/// use oximedia_bitstream::{BitWrite, BitWriter, BigEndian};
///
/// let mut w = BitWriter::endian(vec![], BigEndian);
///
/// // writing a value of 2 in 1 bit is always an error
/// // which is checked here at write-time
/// assert!(w.write::<1, u8>(2).is_err());
/// ```
///
/// But if the value can be checked beforehand,
/// it doesn't need to be checked at write-time.
///
/// # Example 2
///
/// ```
/// use oximedia_bitstream::{BitWrite, BitWriter, BigEndian, CheckedUnsigned};
///
/// let mut w = BitWriter::endian(vec![], BigEndian);
///
/// // writing a value of 1 in 1 bit is ok
/// // and we're checking that validity at this stage
/// let value = CheckedUnsigned::<1, u8>::new_fixed::<1>(1).unwrap();
///
/// // because we've pre-validated the value beforehand,
/// // it doesn't need to be checked again here
/// // (though the write itself may still fail)
/// assert!(w.write_checked(value).is_ok());
/// ```
///
pub trait Checkable: private::Checkable + Sized {
    /// Write our value to the given stream
    fn write<W: BitWrite + ?Sized>(&self, writer: &mut W) -> io::Result<()>;

    /// The number of written bits
    fn written_bits(&self) -> u32;
}

/// A trait for readable types whose bit counts can be saved
///
/// Because the intent of reading checkable values is
/// to avoid validating their values when being written,
/// implementing the [`Checkable`] trait is required.
pub trait CheckablePrimitive: Checkable {
    /// Our bit count type for reading
    type CountType;

    /// Reads our value from the given stream
    fn read<R: BitRead + ?Sized>(reader: &mut R, count: Self::CountType) -> io::Result<Self>;
}

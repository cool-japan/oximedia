// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! `BitCount` and `SignedBitCount` — runtime bit-count values that track
//! their statically known maximum at the type level.
//!
//! Moved here from `lib.rs` during the 0.1.4 refactor so the crate root
//! stays under the COOLJAPAN 2 000-line guideline.

use crate::{Checked, CheckedUnsigned, Numeric, SignedInteger, UnsignedInteger};

/// A number of bits to be consumed or written, with a known maximum
///
/// Although [`crate::BitRead::read`] and [`crate::BitWrite::write`] should be
/// preferred when the number of bits is fixed and known at compile-time -
/// because they can validate the bit count is less than or equal
/// to the type's size in bits at compile-time -
/// there are many instances where bit count is dynamic and
/// determined by the file format itself.
/// But when using [`crate::BitRead::read_var`] or [`crate::BitWrite::write_var`]
/// we must pessimistically assume any number of bits as an argument
/// and validate that the number of bits is not larger than the
/// type being read or written on every call.
///
/// ```
/// use oximedia_bitstream::{BitRead, BitReader, BigEndian};
///
/// let data: &[u8] = &[0b100_0001_1, 0b111_0110_0];
/// let mut r = BitReader::endian(data, BigEndian);
/// // our bit count is a 3 bit value
/// let count = r.read::<3, u32>().unwrap();
/// // that count indicates we need to read 4 bits (0b100)
/// assert_eq!(count, 4);
/// // read the first 4-bit value
/// assert_eq!(r.read_var::<u8>(count).unwrap(), 0b0001);
/// // read the second 4-bit value
/// assert_eq!(r.read_var::<u8>(count).unwrap(), 0b1111);
/// // read the third 4-bit value
/// assert_eq!(r.read_var::<u8>(count).unwrap(), 0b0110);
/// ```
///
/// In the preceding example, even though we know `count` is a
/// 3 bit value whose maximum value will never be greater than 7,
/// the subsequent `read_var` calls have no way to know that.
/// They must assume `count` could be 9, or `u32::MAX` or any other `u32` value
/// and validate the count is not larger than the `u8` types we're reading.
///
/// But we can convert our example to use the `BitCount` type:
///
/// ```
/// use oximedia_bitstream::{BitRead, BitReader, BigEndian, BitCount};
///
/// let data: &[u8] = &[0b100_0001_1, 0b111_0110_0];
/// let mut r = BitReader::endian(data, BigEndian);
/// // our bit count is a 3 bit value with a maximum value of 7
/// let count = r.read_count::<0b111>().unwrap();
/// // that count indicates we need to read 4 bits (0b100)
/// assert_eq!(count, BitCount::<7>::new::<4>());
/// // read the first 4-bit value
/// assert_eq!(r.read_counted::<7, u8>(count).unwrap(), 0b0001);
/// // read the second 4-bit value
/// assert_eq!(r.read_counted::<7, u8>(count).unwrap(), 0b1111);
/// // read the third 4-bit value
/// assert_eq!(r.read_counted::<7, u8>(count).unwrap(), 0b0110);
/// ```
///
/// Because the [`crate::BitRead::read_counted`] methods know at compile-time
/// that the bit count will be larger than 7, that check can be eliminated
/// simply by taking advantage of information we already know.
///
/// Leveraging the `BitCount` type also allows us to reason about
/// bit counts in a more formal way, and use checked permutation methods
/// to modify them while ensuring they remain constrained by
/// the file format's requirements.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct BitCount<const MAX: u32> {
    // The amount of bits may be less than or equal to the maximum,
    // but never more.
    pub(crate) bits: u32,
}

impl<const MAX: u32> BitCount<MAX> {
    /// Builds a bit count from a constant number
    /// of bits, which must not be greater than `MAX`.
    ///
    /// Intended to be used for defining constants.
    ///
    /// Use `TryFrom` to conditionally build
    /// counts from values at runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, BitCount};
    /// let data: &[u8] = &[0b111_00000];
    /// let mut r = BitReader::endian(data, BigEndian);
    /// // reading 3 bits from a stream out of a maximum of 8
    /// // doesn't require checking that the bit count is larger
    /// // than a u8 at runtime because specifying the maximum of 8
    /// // guarantees our bit count will not be larger than 8
    /// assert_eq!(r.read_counted::<8, u8>(BitCount::new::<3>()).unwrap(), 0b111);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::BitCount;
    /// // trying to build a count of 10 with a maximum of 8
    /// // fails to compile at all
    /// let count = BitCount::<8>::new::<10>();
    /// ```
    pub const fn new<const BITS: u32>() -> Self {
        const {
            assert!(BITS <= MAX, "BITS must be <= MAX");
        }

        Self { bits: BITS }
    }

    /// Add a number of bits to our count,
    /// returning a new count with a new maximum.
    ///
    /// Returns `None` if the new count goes above our new maximum.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// let count = BitCount::<2>::new::<1>();
    /// // adding 2 to 1 and increasing the max to 3 yields a new count of 3
    /// assert_eq!(count.checked_add::<3>(2), Some(BitCount::<3>::new::<3>()));
    /// // adding 2 to 1 without increasing the max yields None
    /// assert_eq!(count.checked_add::<2>(2), None);
    /// ```
    #[inline]
    pub const fn checked_add<const NEW_MAX: u32>(self, bits: u32) -> Option<BitCount<NEW_MAX>> {
        match self.bits.checked_add(bits) {
            Some(bits) if bits <= NEW_MAX => Some(BitCount { bits }),
            _ => None,
        }
    }

    /// Subtracts a number of bits from our count,
    /// returning a new count with a new maximum.
    ///
    /// Returns `None` if the new count goes below 0
    /// or below our new maximum.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::BitCount;
    /// let count = BitCount::<5>::new::<5>();
    /// // subtracting 1 from 5 yields a new count of 4
    /// assert_eq!(count.checked_sub::<5>(1), Some(BitCount::<5>::new::<4>()));
    /// // subtracting 6 from 5 yields None
    /// assert!(count.checked_sub::<5>(6).is_none());
    /// // subtracting 1 with a new maximum of 3 also yields None
    /// // because 4 is larger than the maximum of 3
    /// assert!(count.checked_sub::<3>(1).is_none());
    /// ```
    #[inline]
    pub const fn checked_sub<const NEW_MAX: u32>(self, bits: u32) -> Option<BitCount<NEW_MAX>> {
        match self.bits.checked_sub(bits) {
            Some(bits) if bits <= NEW_MAX => Some(BitCount { bits }),
            _ => None,
        }
    }

    /// Attempt to convert our count to a count with a new
    /// bit count and new maximum.
    ///
    /// Returns `Some(count)` if the updated number of bits
    /// is less than or equal to the new maximum.
    /// Returns `None` if not.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// let count = BitCount::<5>::new::<5>();
    /// // muliplying 5 bits by 2 with a new max of 10 is ok
    /// assert_eq!(
    ///     count.try_map::<10, _>(|i| i.checked_mul(2)),
    ///     Some(BitCount::<10>::new::<10>()),
    /// );
    ///
    /// // multiplying 5 bits by 3 with a new max of 10 overflows
    /// assert_eq!(count.try_map::<10, _>(|i| i.checked_mul(3)), None);
    /// ```
    #[inline]
    pub fn try_map<const NEW_MAX: u32, F>(self, f: F) -> Option<BitCount<NEW_MAX>>
    where
        F: FnOnce(u32) -> Option<u32>,
    {
        f(self.bits)
            .filter(|bits| *bits <= NEW_MAX)
            .map(|bits| BitCount { bits })
    }

    /// Returns our maximum bit count
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// let count = BitCount::<10>::new::<5>();
    /// assert_eq!(count.max(), 10);
    /// ```
    #[inline(always)]
    pub const fn max(&self) -> u32 {
        MAX
    }

    /// Returns signed count if our bit count is greater than 0
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{BitCount, SignedBitCount};
    ///
    /// let count = BitCount::<10>::new::<5>();
    /// assert_eq!(count.signed_count(), Some(SignedBitCount::<10>::new::<5>()));
    ///
    /// let count = BitCount::<10>::new::<0>();
    /// assert_eq!(count.signed_count(), None);
    /// ```
    #[inline(always)]
    pub const fn signed_count(&self) -> Option<SignedBitCount<MAX>> {
        match self.bits.checked_sub(1) {
            Some(bits) => Some(SignedBitCount {
                bits: *self,
                unsigned: BitCount { bits },
            }),
            None => None,
        }
    }

    /// Masks off the least-significant bits for the given type
    ///
    /// Returns closure that takes a value and returns a
    /// pair of the most-significant and least-significant
    /// bits.  Because the least-significant bits cannot
    /// be larger than this bit count, that value is
    /// returned as a [`Checked`] type.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// // create a bit count of 3
    /// let count = BitCount::<8>::new::<3>();
    ///
    /// // create a mask suitable for u8 types
    /// let mask = count.mask_lsb::<u8>();
    ///
    /// let (msb, lsb) = mask(0b11011_110);
    /// assert_eq!(msb, 0b11011);             // the most-significant bits
    /// assert_eq!(lsb.into_value(), 0b110);  // the least-significant bits
    ///
    /// let (msb, lsb) = mask(0b01100_010);
    /// assert_eq!(msb, 0b01100);             // the most-significant bits
    /// assert_eq!(lsb.into_value(), 0b010);  // the least-significant bits
    ///
    /// let (msb, lsb) = mask(0b00000_111);
    /// assert_eq!(msb, 0b00000);             // the most-significant bits
    /// assert_eq!(lsb.into_value(), 0b111);  // the least-significant bits
    /// ```
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// // a mask with a bit count of 0 puts everything in msb
    /// let mask = BitCount::<8>::new::<0>().mask_lsb::<u8>();
    ///
    /// let (msb, lsb) = mask(0b11111111);
    /// assert_eq!(msb, 0b11111111);
    /// assert_eq!(lsb.into_value(), 0);
    ///
    /// // a mask with a bit count larger than the type
    /// // is restricted to that type's size, if possible
    /// let mask = BitCount::<16>::new::<9>().mask_lsb::<u8>();
    ///
    /// let (msb, lsb) = mask(0b11111111);
    /// assert_eq!(msb, 0);
    /// assert_eq!(lsb.into_value(), 0b11111111);
    /// ```
    pub fn mask_lsb<U: UnsignedInteger>(self) -> impl Fn(U) -> (U, CheckedUnsigned<MAX, U>) {
        let (mask, shift, count) = match U::BITS_SIZE.checked_sub(self.bits) {
            Some(mask_bits) => (U::ALL.shr_default(mask_bits), self.bits, self),
            None => (
                U::ALL,
                U::BITS_SIZE,
                // `self.bits > U::BITS_SIZE` (the `checked_sub` returned `None`), and
                // `self.bits <= MAX`, so `MAX > U::BITS_SIZE >= U::BITS_SIZE`, meaning
                // `BitCount::<MAX>` is valid for `U::BITS_SIZE`.  Construct directly to
                // avoid a fallible conversion that cannot actually fail here.
                BitCount { bits: U::BITS_SIZE },
            ),
        };

        move |v| {
            (
                v.shr_default(shift),
                Checked {
                    value: v & mask,
                    count,
                },
            )
        }
    }

    /// Returns this bit count's range for the given unsigned type
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// assert_eq!(BitCount::<9>::new::<0>().range::<u8>(), 0..=0);
    /// assert_eq!(BitCount::<9>::new::<1>().range::<u8>(), 0..=0b1);
    /// assert_eq!(BitCount::<9>::new::<2>().range::<u8>(), 0..=0b11);
    /// assert_eq!(BitCount::<9>::new::<3>().range::<u8>(), 0..=0b111);
    /// assert_eq!(BitCount::<9>::new::<4>().range::<u8>(), 0..=0b1111);
    /// assert_eq!(BitCount::<9>::new::<5>().range::<u8>(), 0..=0b11111);
    /// assert_eq!(BitCount::<9>::new::<6>().range::<u8>(), 0..=0b111111);
    /// assert_eq!(BitCount::<9>::new::<7>().range::<u8>(), 0..=0b1111111);
    /// assert_eq!(BitCount::<9>::new::<8>().range::<u8>(), 0..=0b11111111);
    /// // a count that exceeds the type's size is
    /// // naturally restricted to that type's maximum range
    /// assert_eq!(BitCount::<9>::new::<9>().range::<u8>(), 0..=0b11111111);
    /// ```
    #[inline]
    pub fn range<U: UnsignedInteger>(&self) -> core::ops::RangeInclusive<U> {
        match U::ONE.checked_shl(self.bits) {
            Some(top) => U::ZERO..=(top - U::ONE),
            None => U::ZERO..=U::ALL,
        }
    }

    /// Returns minimum value between ourself and bit count
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// let count = BitCount::<8>::new::<7>();
    /// assert_eq!(count.min(6), BitCount::new::<6>());
    /// assert_eq!(count.min(8), BitCount::new::<7>());
    /// ```
    #[inline(always)]
    pub fn min(self, bits: u32) -> Self {
        // the minimum of ourself and another bit count
        // can never exceed our maximum bit count
        Self {
            bits: self.bits.min(bits),
        }
    }

    /// Returns the minimum value of an unsigned int in this bit count
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// assert_eq!(BitCount::<8>::new::<0>().none::<u8>().into_value(), 0b0);
    /// assert_eq!(BitCount::<8>::new::<1>().none::<u8>().into_value(), 0b0);
    /// assert_eq!(BitCount::<8>::new::<2>().none::<u8>().into_value(), 0b00);
    /// assert_eq!(BitCount::<8>::new::<3>().none::<u8>().into_value(), 0b000);
    /// assert_eq!(BitCount::<8>::new::<4>().none::<u8>().into_value(), 0b0000);
    /// assert_eq!(BitCount::<8>::new::<5>().none::<u8>().into_value(), 0b00000);
    /// assert_eq!(BitCount::<8>::new::<6>().none::<u8>().into_value(), 0b000000);
    /// assert_eq!(BitCount::<8>::new::<7>().none::<u8>().into_value(), 0b0000000);
    /// assert_eq!(BitCount::<8>::new::<8>().none::<u8>().into_value(), 0b00000000);
    /// ```
    #[inline(always)]
    pub fn none<U: UnsignedInteger>(self) -> CheckedUnsigned<MAX, U> {
        CheckedUnsigned {
            value: U::ZERO,
            count: self,
        }
    }

    /// Returns the maximum value of an unsigned int in this bit count
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::BitCount;
    ///
    /// assert_eq!(BitCount::<8>::new::<0>().all::<u8>().into_value(), 0b0);
    /// assert_eq!(BitCount::<8>::new::<1>().all::<u8>().into_value(), 0b1);
    /// assert_eq!(BitCount::<8>::new::<2>().all::<u8>().into_value(), 0b11);
    /// assert_eq!(BitCount::<8>::new::<3>().all::<u8>().into_value(), 0b111);
    /// assert_eq!(BitCount::<8>::new::<4>().all::<u8>().into_value(), 0b1111);
    /// assert_eq!(BitCount::<8>::new::<5>().all::<u8>().into_value(), 0b11111);
    /// assert_eq!(BitCount::<8>::new::<6>().all::<u8>().into_value(), 0b111111);
    /// assert_eq!(BitCount::<8>::new::<7>().all::<u8>().into_value(), 0b1111111);
    /// assert_eq!(BitCount::<8>::new::<8>().all::<u8>().into_value(), 0b11111111);
    /// ```
    #[inline(always)]
    pub fn all<U: UnsignedInteger>(self) -> CheckedUnsigned<MAX, U> {
        CheckedUnsigned {
            value: match U::ONE.checked_shl(self.bits) {
                Some(top) => top - U::ONE,
                None => U::ALL,
            },
            count: self,
        }
    }
}

impl<const MAX: u32> core::convert::TryFrom<u32> for BitCount<MAX> {
    type Error = u32;

    /// Attempts to convert a `u32` bit count to a `BitCount`
    ///
    /// Attempting a bit maximum bit count larger than the
    /// largest supported type is a compile-time error
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::BitCount;
    /// use std::convert::TryInto;
    ///
    /// assert_eq!(8u32.try_into(), Ok(BitCount::<8>::new::<8>()));
    /// assert_eq!(9u32.try_into(), Err::<BitCount<8>, _>(9));
    /// ```
    fn try_from(bits: u32) -> Result<Self, Self::Error> {
        (bits <= MAX).then_some(Self { bits }).ok_or(bits)
    }
}

impl<const MAX: u32> core::fmt::Display for BitCount<MAX> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.bits, f)
    }
}

impl BitCount<{ u32::MAX }> {
    /// Builds a bit count where the maximum bits is unknown.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::BitCount;
    /// assert_eq!(BitCount::unknown(5), BitCount::<{ u32::MAX }>::new::<5>());
    /// ```
    pub const fn unknown(bits: u32) -> Self {
        Self { bits }
    }
}

#[test]
fn test_unknown_bitcount() {
    let count = BitCount::unknown(u32::MAX);
    assert!(u32::from(count) <= count.max());
}

impl<const MAX: u32> From<BitCount<MAX>> for u32 {
    #[inline(always)]
    fn from(BitCount { bits }: BitCount<MAX>) -> u32 {
        bits
    }
}

/// A number of bits to be read or written for signed integers, with a known maximum
///
/// This is closely related to the [`BitCount`] type, but further constrained
/// to have a minimum value of 1 - because signed values require at least
/// 1 bit for the sign.
///
/// Let's start with a basic example:
///
/// ```
/// use oximedia_bitstream::{BitRead, BitReader, BigEndian};
///
/// let data: &[u8] = &[0b100_0001_1, 0b111_0110_0];
/// let mut r = BitReader::endian(data, BigEndian);
/// // our bit count is a 3 bit value
/// let count = r.read::<3, u32>().unwrap();
/// // that count indicates we need to read 4 bits (0b100)
/// assert_eq!(count, 4);
/// // read the first 4-bit signed value
/// assert_eq!(r.read_var::<i8>(count).unwrap(), 1);
/// // read the second 4-bit signed value
/// assert_eq!(r.read_var::<i8>(count).unwrap(), -1);
/// // read the third 4-bit signed value
/// assert_eq!(r.read_var::<i8>(count).unwrap(), 6);
/// ```
///
/// In the preceding example, even though we know `count` is a
/// 3 bit value whose maximum value will never be greater than 7,
/// the subsequent `read_var` calls have no way to know that.
/// They must assume `count` could be 9, or `u32::MAX` or any other `u32` value
/// and validate the count is not larger than the `i8` types we're reading
/// while also greater than 0 because `i8` requires a sign bit.
///
/// But we can convert our example to use the `SignedBitCount` type:
/// ```
/// use oximedia_bitstream::{BitRead, BitReader, BigEndian, SignedBitCount};
///
/// let data: &[u8] = &[0b100_0001_1, 0b111_0110_0];
/// let mut r = BitReader::endian(data, BigEndian);
/// // our bit count is a 3 bit value with a maximum value of 7
/// let count = r.read_count::<0b111>().unwrap();
/// // convert that count to a signed bit count,
/// // which guarantees its value is greater than 0
/// let count = count.signed_count().unwrap();
/// // that count indicates we need to read 4 bits (0b100)
/// assert_eq!(count, SignedBitCount::<7>::new::<4>());
/// // read the first 4-bit value
/// assert_eq!(r.read_signed_counted::<7, i8>(count).unwrap(), 1);
/// // read the second 4-bit value
/// assert_eq!(r.read_signed_counted::<7, i8>(count).unwrap(), -1);
/// // read the third 4-bit value
/// assert_eq!(r.read_signed_counted::<7, i8>(count).unwrap(), 6);
/// ```
///
/// Because the [`crate::BitRead::read_signed_counted`] methods know at compile-time
/// that the bit count will be larger than 7, that check can be eliminated
/// simply by taking advantage of information we already know.
///
/// Leveraging the `SignedBitCount` type also allows us to reason about
/// bit counts in a more formal way, and use checked permutation methods
/// to modify them while ensuring they remain constrained by
/// the file format's requirements.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct SignedBitCount<const MAX: u32> {
    // the whole original bit count
    pub(crate) bits: BitCount<MAX>,
    // a bit count with one bit removed for the sign
    pub(crate) unsigned: BitCount<MAX>,
}

impl<const MAX: u32> SignedBitCount<MAX> {
    /// Builds a signed bit count from a constant number
    /// of bits, which must be greater than 0 and
    /// not be greater than `MAX`.
    ///
    /// Intended to be used for defining constants.
    ///
    /// Use `TryFrom` to conditionally build
    /// counts from values at runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::{BitReader, BitRead, BigEndian, SignedBitCount};
    /// let data: &[u8] = &[0b111_00000];
    /// let mut r = BitReader::endian(data, BigEndian);
    /// // reading 3 bits from a stream out of a maximum of 8
    /// // doesn't require checking that the bit count is larger
    /// // than a u8 at runtime because specifying the maximum of 8
    /// // guarantees our bit count will not be larger than 8
    /// assert_eq!(r.read_signed_counted::<8, i8>(SignedBitCount::new::<3>()).unwrap(), -1);
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::SignedBitCount;
    /// // trying to build a count of 10 with a maximum of 8
    /// // fails to compile at all
    /// let count = SignedBitCount::<8>::new::<10>();
    /// ```
    ///
    /// ```rust,compile_fail
    /// use oximedia_bitstream::SignedBitCount;
    /// // trying to build a count of 0 also fails to compile
    /// let count = SignedBitCount::<8>::new::<0>();
    /// ```
    pub const fn new<const BITS: u32>() -> Self {
        const {
            assert!(BITS > 0, "BITS must be > 0");
        }

        Self {
            bits: BitCount::new::<BITS>(),
            unsigned: BitCount { bits: BITS - 1 },
        }
    }

    /// Add a number of bits to our count,
    /// returning a new count with a new maximum.
    ///
    /// Returns `None` if the new count goes above our new maximum.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_bitstream::SignedBitCount;
    ///
    /// let count = SignedBitCount::<2>::new::<1>();
    /// // adding 2 to 1 and increasing the max to 3 yields a new count of 3
    /// assert_eq!(count.checked_add::<3>(2), Some(SignedBitCount::<3>::new::<3>()));
    /// // adding 2 to 1 without increasing the max yields None
    /// assert_eq!(count.checked_add::<2>(2), None);
    /// ```
    #[inline]
    pub const fn checked_add<const NEW_MAX: u32>(
        self,
        bits: u32,
    ) -> Option<SignedBitCount<NEW_MAX>> {
        match self.bits.checked_add(bits) {
            Some(bits_new) => match self.unsigned.checked_add(bits) {
                Some(unsigned) => Some(SignedBitCount {
                    bits: bits_new,
                    unsigned,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Subtracts a number of bits from our count,
    /// returning a new count with a new maximum.
    ///
    /// Returns `None` if the new count goes below 1
    /// or below our new maximum.
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::SignedBitCount;
    /// let count = SignedBitCount::<5>::new::<5>();
    /// // subtracting 1 from 5 yields a new count of 4
    /// assert_eq!(count.checked_sub::<5>(1), Some(SignedBitCount::<5>::new::<4>()));
    /// // subtracting 6 from 5 yields None
    /// assert!(count.checked_sub::<5>(6).is_none());
    /// // subtracting 1 with a new maximum of 3 also yields None
    /// // because 4 is larger than the maximum of 3
    /// assert!(count.checked_sub::<3>(1).is_none());
    /// // subtracting 5 from 5 also yields None
    /// // because SignedBitCount always requires 1 bit for the sign
    /// assert!(count.checked_sub::<5>(5).is_none());
    /// ```
    #[inline]
    pub const fn checked_sub<const NEW_MAX: u32>(
        self,
        bits: u32,
    ) -> Option<SignedBitCount<NEW_MAX>> {
        match self.bits.checked_sub(bits) {
            Some(bits_new) => match self.unsigned.checked_sub(bits) {
                Some(unsigned) => Some(SignedBitCount {
                    bits: bits_new,
                    unsigned,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Attempt to convert our count to a count with a new
    /// bit count and new maximum.
    ///
    /// Returns `Some(count)` if the updated number of bits
    /// is less than or equal to the new maximum
    /// and greater than 0.
    /// Returns `None` if not.
    ///
    /// # Examples
    /// ```
    /// use oximedia_bitstream::SignedBitCount;
    ///
    /// let count = SignedBitCount::<5>::new::<5>();
    /// // muliplying 5 bits by 2 with a new max of 10 is ok
    /// assert_eq!(
    ///     count.try_map::<10, _>(|i| i.checked_mul(2)),
    ///     Some(SignedBitCount::<10>::new::<10>()),
    /// );
    ///
    /// // multiplying 5 bits by 3 with a new max of 10 overflows
    /// assert_eq!(count.try_map::<10, _>(|i| i.checked_mul(3)), None);
    ///
    /// // multiplying 5 bits by 0 results in 0 bits,
    /// // which isn't value for a SignedBitCount
    /// assert_eq!(count.try_map::<10, _>(|i| Some(i * 0)), None);
    /// ```
    #[inline]
    pub fn try_map<const NEW_MAX: u32, F>(self, f: F) -> Option<SignedBitCount<NEW_MAX>>
    where
        F: FnOnce(u32) -> Option<u32>,
    {
        self.bits.try_map(f).and_then(|b| b.signed_count())
    }

    /// Returns our maximum bit count
    ///
    /// # Example
    /// ```
    /// use oximedia_bitstream::SignedBitCount;
    ///
    /// let count = SignedBitCount::<10>::new::<5>();
    /// assert_eq!(count.max(), 10);
    /// ```
    #[inline(always)]
    pub const fn max(&self) -> u32 {
        MAX
    }

    /// Returns regular unsigned bit count
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{BitCount, SignedBitCount};
    ///
    /// let signed_count = SignedBitCount::<10>::new::<5>();
    /// assert_eq!(signed_count.count(), BitCount::<10>::new::<5>());
    /// ```
    #[inline(always)]
    pub const fn count(&self) -> BitCount<MAX> {
        self.bits
    }

    /// Returns this bit count's range for the given signed type
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::SignedBitCount;
    ///
    /// assert_eq!(SignedBitCount::<9>::new::<1>().range::<i8>(), -1..=0);
    /// assert_eq!(SignedBitCount::<9>::new::<2>().range::<i8>(), -2..=1);
    /// assert_eq!(SignedBitCount::<9>::new::<3>().range::<i8>(), -4..=3);
    /// assert_eq!(SignedBitCount::<9>::new::<4>().range::<i8>(), -8..=7);
    /// assert_eq!(SignedBitCount::<9>::new::<5>().range::<i8>(), -16..=15);
    /// assert_eq!(SignedBitCount::<9>::new::<6>().range::<i8>(), -32..=31);
    /// assert_eq!(SignedBitCount::<9>::new::<7>().range::<i8>(), -64..=63);
    /// assert_eq!(SignedBitCount::<9>::new::<8>().range::<i8>(), -128..=127);
    /// // a count that exceeds the type's size is
    /// // naturally restricted to that type's maximum range
    /// assert_eq!(SignedBitCount::<9>::new::<9>().range::<i8>(), -128..=127);
    /// ```
    pub fn range<S: SignedInteger>(&self) -> core::ops::RangeInclusive<S> {
        // a bit of a hack to get around the somewhat restrictive
        // SignedInteger trait I've created for myself

        if self.bits.bits < S::BITS_SIZE {
            (!S::ZERO << self.unsigned.bits)..=((S::ONE << self.unsigned.bits) - S::ONE)
        } else {
            S::Unsigned::ZERO.as_negative(S::BITS_SIZE)..=(S::Unsigned::ALL >> 1).as_non_negative()
        }
    }
}

impl<const MAX: u32> core::fmt::Display for SignedBitCount<MAX> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.bits, f)
    }
}

impl<const MAX: u32> core::convert::TryFrom<BitCount<MAX>> for SignedBitCount<MAX> {
    type Error = ();

    #[inline]
    fn try_from(count: BitCount<MAX>) -> Result<Self, Self::Error> {
        count.signed_count().ok_or(())
    }
}

impl<const MAX: u32> core::convert::TryFrom<u32> for SignedBitCount<MAX> {
    type Error = u32;

    #[inline]
    fn try_from(count: u32) -> Result<Self, Self::Error> {
        BitCount::<MAX>::try_from(count).and_then(|b| b.signed_count().ok_or(count))
    }
}

impl<const MAX: u32> From<SignedBitCount<MAX>> for u32 {
    #[inline(always)]
    fn from(
        SignedBitCount {
            bits: BitCount { bits },
            ..
        }: SignedBitCount<MAX>,
    ) -> u32 {
        bits
    }
}

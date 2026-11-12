// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Bit-counting writers — `Overflowed`, the `Counter` trait, and the
//! `BitsWritten` / deprecated `BitCounter` accumulators.

use core::{convert::TryFrom, fmt};
use std::io;

use super::{
    BitCount, BitWrite, Checkable, Endianness, Numeric, PhantomData, Primitive, SignedBitCount,
    SignedInteger, UnsignedInteger,
};

/// An error returned if performing math operations would overflow
#[derive(Copy, Clone, Debug)]
pub struct Overflowed;

impl fmt::Display for Overflowed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "overflow occured in counter".fmt(f)
    }
}

impl core::error::Error for Overflowed {}

impl From<Overflowed> for io::Error {
    fn from(Overflowed: Overflowed) -> Self {
        io::Error::new(
            #[cfg(feature = "std")]
            {
                io::ErrorKind::StorageFull
            },
            #[cfg(not(feature = "std"))]
            {
                io::ErrorKind::Other
            },
            "bitstream accumulator overflow",
        )
    }
}

/// A common trait for integer types for performing math operations
/// which may check for overflow.
pub trait Counter: Default + Sized + From<u8> + TryFrom<u32> + TryFrom<usize> {
    /// add rhs to self, returning `Overflowed` if the result is too large
    fn checked_add_assign(&mut self, rhs: Self) -> Result<(), Overflowed>;

    /// multiply self by rhs, returning `Overflowed` if the result is too large
    fn checked_mul(self, rhs: Self) -> Result<Self, Overflowed>;

    /// returns `true` if the number if bits written is divisible by 8
    fn byte_aligned(&self) -> bool;
}

macro_rules! define_counter {
    ($t:ty) => {
        impl Counter for $t {
            fn checked_add_assign(&mut self, rhs: Self) -> Result<(), Overflowed> {
                *self = <$t>::checked_add(*self, rhs).ok_or(Overflowed)?;
                Ok(())
            }

            fn checked_mul(self, rhs: Self) -> Result<Self, Overflowed> {
                <$t>::checked_mul(self, rhs).ok_or(Overflowed)
            }

            fn byte_aligned(&self) -> bool {
                self % 8 == 0
            }
        }
    };
}

define_counter!(u8);
define_counter!(u16);
define_counter!(u32);
define_counter!(u64);
define_counter!(u128);

/// For counting the number of bits written but generating no output.
///
/// # Example
/// ```
/// use oximedia_bitstream::{BigEndian, BitWrite, BitsWritten};
/// let mut writer: BitsWritten<u32> = BitsWritten::new();
/// writer.write_var(1, 0b1u8).unwrap();
/// writer.write_var(2, 0b01u8).unwrap();
/// writer.write_var(5, 0b10111u8).unwrap();
/// assert_eq!(writer.written(), 8);
/// ```
#[derive(Default)]
pub struct BitsWritten<N> {
    bits: N,
}

impl<N: Default> BitsWritten<N> {
    /// Creates new empty BitsWritten value
    #[inline]
    pub fn new() -> Self {
        Self { bits: N::default() }
    }
}

impl<N: Copy> BitsWritten<N> {
    /// Returns number of bits written
    #[inline]
    pub fn written(&self) -> N {
        self.bits
    }
}

impl<N> BitsWritten<N> {
    /// Returns number of bits written
    #[inline]
    pub fn into_written(self) -> N {
        self.bits
    }
}

impl<N: Counter> BitWrite for BitsWritten<N> {
    #[inline]
    fn write_bit(&mut self, _bit: bool) -> io::Result<()> {
        self.bits.checked_add_assign(1u8.into())?;
        Ok(())
    }

    #[inline]
    fn write_const<const BITS: u32, const VALUE: u32>(&mut self) -> io::Result<()> {
        const {
            assert!(
                BITS == 0 || VALUE <= (u32::ALL >> (u32::BITS_SIZE - BITS)),
                "excessive value for bits written"
            );
        }

        self.bits
            .checked_add_assign(BITS.try_into().map_err(|_| Overflowed)?)?;
        Ok(())
    }

    #[inline]
    fn write_unsigned<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        const {
            assert!(BITS <= U::BITS_SIZE, "excessive bits for type written");
        }

        if BITS == 0 {
            Ok(())
        } else if value <= (U::ALL >> (U::BITS_SIZE - BITS)) {
            self.bits
                .checked_add_assign(BITS.try_into().map_err(|_| Overflowed)?)?;
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive value for bits written",
            ))
        }
    }

    #[inline]
    fn write_signed<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        let SignedBitCount {
            bits: BitCount { bits },
            unsigned,
        } = const {
            assert!(BITS <= S::BITS_SIZE, "excessive bits for type written");
            let count = BitCount::<BITS>::new::<BITS>().signed_count();
            match count {
                Some(c) => c,
                None => panic!("signed writes need at least 1 bit for sign"),
            }
        };

        // doesn't matter which side the sign is on
        // so long as it's added to the bit count
        self.bits.checked_add_assign(1u8.into())?;

        self.write_unsigned_counted(
            unsigned,
            if value.is_negative() {
                value.as_negative(bits)
            } else {
                value.as_non_negative()
            },
        )
    }

    #[inline]
    fn write_unsigned_counted<const MAX: u32, U>(
        &mut self,
        BitCount { bits }: BitCount<MAX>,
        value: U,
    ) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        if MAX <= U::BITS_SIZE || bits <= U::BITS_SIZE {
            if bits == 0 {
                Ok(())
            } else if value <= U::ALL >> (U::BITS_SIZE - bits) {
                self.bits
                    .checked_add_assign(bits.try_into().map_err(|_| Overflowed)?)?;
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "excessive value for bits written",
                ))
            }
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive bits for type written",
            ))
        }
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
        let SignedBitCount {
            bits: BitCount { bits },
            unsigned,
        } = bits.try_into().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "signed writes need at least 1 bit for sign",
            )
        })?;

        if MAX <= S::BITS_SIZE || bits <= S::BITS_SIZE {
            // doesn't matter which side the sign is on
            // so long as it's added to the bit count
            self.bits.checked_add_assign(1u8.into())?;

            self.write_unsigned_counted(
                unsigned,
                if value.is_negative() {
                    value.as_negative(bits)
                } else {
                    value.as_non_negative()
                },
            )
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive bits for type written",
            ))
        }
    }

    #[inline]
    fn write_from<V>(&mut self, _: V) -> io::Result<()>
    where
        V: Primitive,
    {
        self.bits.checked_add_assign(
            N::try_from(core::mem::size_of::<V>())
                .map_err(|_| Overflowed)?
                .checked_mul(8u8.into())?,
        )?;
        Ok(())
    }

    #[inline]
    fn write_as_from<F, V>(&mut self, _: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        self.bits.checked_add_assign(
            N::try_from(core::mem::size_of::<V>())
                .map_err(|_| Overflowed)?
                .checked_mul(8u8.into())?,
        )?;
        Ok(())
    }

    #[inline]
    fn pad(&mut self, bits: u32) -> io::Result<()> {
        self.bits
            .checked_add_assign(bits.try_into().map_err(|_| Overflowed)?)?;
        Ok(())
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        self.bits.checked_add_assign(
            N::try_from(buf.len())
                .map_err(|_| Overflowed)?
                .checked_mul(8u8.into())?,
        )?;
        Ok(())
    }

    fn write_unary<const STOP_BIT: u8>(&mut self, value: u32) -> io::Result<()> {
        const {
            assert!(matches!(STOP_BIT, 0 | 1), "stop bit must be 0 or 1");
        }

        self.bits
            .checked_add_assign(value.try_into().map_err(|_| Overflowed)?)?;
        self.bits.checked_add_assign(1u8.into())?;
        Ok(())
    }

    fn write_checked<C: Checkable>(&mut self, value: C) -> io::Result<()> {
        Ok(self
            .bits
            .checked_add_assign(value.written_bits().try_into().map_err(|_| Overflowed)?)?)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        self.bits.byte_aligned()
    }
}

/// For counting the number of bits written but generating no output.
///
/// # Example
/// ```
/// use oximedia_bitstream::{BigEndian, BitWrite, BitCounter};
/// let mut writer: BitCounter<u32, BigEndian> = BitCounter::new();
/// writer.write_var(1, 0b1u8).unwrap();
/// writer.write_var(2, 0b01u8).unwrap();
/// writer.write_var(5, 0b10111u8).unwrap();
/// assert_eq!(writer.written(), 8);
/// ```
#[derive(Default)]
#[deprecated(since = "4.0.0", note = "use of BitsWritten is preferred")]
pub struct BitCounter<N, E: Endianness> {
    bits: BitsWritten<N>,
    phantom: PhantomData<E>,
}

#[allow(deprecated)]
impl<N: Default, E: Endianness> BitCounter<N, E> {
    /// Creates new counter
    #[inline]
    pub fn new() -> Self {
        BitCounter {
            bits: BitsWritten::new(),
            phantom: PhantomData,
        }
    }
}

#[allow(deprecated)]
impl<N: Copy, E: Endianness> BitCounter<N, E> {
    /// Returns number of bits written
    #[inline]
    pub fn written(&self) -> N {
        self.bits.written()
    }
}

#[allow(deprecated)]
impl<N, E: Endianness> BitCounter<N, E> {
    /// Returns number of bits written
    #[inline]
    pub fn into_written(self) -> N {
        self.bits.into_written()
    }
}

#[allow(deprecated)]
impl<N, E> BitWrite for BitCounter<N, E>
where
    E: Endianness,
    N: Counter,
{
    #[inline]
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        BitWrite::write_bit(&mut self.bits, bit)
    }

    #[inline]
    fn write_const<const BITS: u32, const VALUE: u32>(&mut self) -> io::Result<()> {
        BitWrite::write_const::<BITS, VALUE>(&mut self.bits)
    }

    #[inline]
    fn write_unsigned<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        BitWrite::write_unsigned::<BITS, U>(&mut self.bits, value)
    }

    #[inline]
    fn write_signed<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        BitWrite::write_signed::<BITS, S>(&mut self.bits, value)
    }

    #[inline]
    fn write_unsigned_counted<const MAX: u32, U>(
        &mut self,
        count: BitCount<MAX>,
        value: U,
    ) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        BitWrite::write_unsigned_counted::<MAX, U>(&mut self.bits, count, value)
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
        BitWrite::write_signed_counted::<MAX, S>(&mut self.bits, bits, value)
    }

    #[inline]
    fn write_from<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive,
    {
        BitWrite::write_from(&mut self.bits, value)
    }

    #[inline]
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        BitWrite::write_as_from::<F, V>(&mut self.bits, value)
    }

    #[inline]
    fn pad(&mut self, bits: u32) -> io::Result<()> {
        BitWrite::pad(&mut self.bits, bits)
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        BitWrite::write_bytes(&mut self.bits, buf)
    }

    fn write_unary<const STOP_BIT: u8>(&mut self, value: u32) -> io::Result<()> {
        BitWrite::write_unary::<STOP_BIT>(&mut self.bits, value)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        BitWrite::byte_aligned(&self.bits)
    }
}

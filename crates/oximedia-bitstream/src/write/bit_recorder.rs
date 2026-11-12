// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! `BitRecorder` — feature-gated playback-style writer (requires `alloc`).

#![cfg(feature = "alloc")]

use std::{io, vec::Vec};

use super::{
    BitCount, BitWrite, BitWriter, Counter, Endianness, Integer, Overflowed, PhantomData,
    Primitive, SignedBitCount, SignedInteger, ToBitStream, ToBitStreamWith, UnsignedInteger,
};

/// For recording writes in order to play them back on another writer
/// # Example
/// ```
/// use std::io::Write;
/// use oximedia_bitstream::{BigEndian, BitWriter, BitWrite, BitRecorder};
/// let mut recorder: BitRecorder<u32, BigEndian> = BitRecorder::new();
/// recorder.write_var(1, 0b1u8).unwrap();
/// recorder.write_var(2, 0b01u8).unwrap();
/// recorder.write_var(5, 0b10111u8).unwrap();
/// assert_eq!(recorder.written(), 8);
/// let mut writer = BitWriter::endian(Vec::new(), BigEndian);
/// recorder.playback(&mut writer);
/// assert_eq!(writer.into_writer(), [0b10110111]);
/// ```
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub struct BitRecorder<N, E: Endianness> {
    writer: BitWriter<Vec<u8>, E>,
    phantom: PhantomData<N>,
}

impl<N: Counter, E: Endianness> BitRecorder<N, E> {
    /// Creates new recorder
    #[inline]
    pub fn new() -> Self {
        BitRecorder {
            writer: BitWriter::new(Vec::new()),
            phantom: PhantomData,
        }
    }

    /// Creates new recorder sized for the given number of bytes
    #[inline]
    pub fn with_capacity(bytes: usize) -> Self {
        BitRecorder {
            writer: BitWriter::new(Vec::with_capacity(bytes)),
            phantom: PhantomData,
        }
    }

    /// Creates new recorder with the given endianness
    #[inline]
    pub fn endian(endian: E) -> Self {
        BitRecorder {
            writer: BitWriter::endian(Vec::new(), endian),
            phantom: PhantomData,
        }
    }

    /// Returns number of bits written
    ///
    /// # Panics
    ///
    /// Panics if the number of bits written is
    /// larger than the maximum supported by the counter type.
    /// Use [`BitRecorder::written_checked`] for a non-panicking
    /// alternative.
    #[inline]
    pub fn written(&self) -> N {
        self.written_checked()
            .expect("writer maintains checked count when tracking is enabled")
    }

    /// Returns number of bits written
    ///
    /// # Errors
    ///
    /// Returns an error if the number of bits written overflows
    /// our counter type.
    #[inline]
    pub fn written_checked(&self) -> Result<N, Overflowed> {
        let mut written = N::try_from(self.writer.writer.len())
            .map_err(|_| Overflowed)?
            .checked_mul(8u8.into())?;

        written.checked_add_assign(N::try_from(self.writer.bits).map_err(|_| Overflowed)?)?;

        Ok(written)
    }

    /// Plays recorded writes to the given writer
    #[inline]
    pub fn playback<W: BitWrite>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_bytes(self.writer.writer.as_slice())?;
        writer.write_var(self.writer.bits, self.writer.value)?;
        Ok(())
    }

    /// Clears recorder, removing all values
    #[inline]
    pub fn clear(&mut self) {
        self.writer = BitWriter::new({
            let mut v = core::mem::take(&mut self.writer.writer);
            v.clear();
            v
        });
    }
}

impl<N: Counter, E: Endianness> Default for BitRecorder<N, E> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<N, E> BitWrite for BitRecorder<N, E>
where
    E: Endianness,
    N: Counter,
{
    #[inline]
    fn write_bit(&mut self, bit: bool) -> io::Result<()> {
        BitWrite::write_bit(&mut self.writer, bit)
    }

    #[inline]
    fn write<const BITS: u32, I>(&mut self, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        BitWrite::write::<BITS, I>(&mut self.writer, value)
    }

    #[inline]
    fn write_const<const BITS: u32, const VALUE: u32>(&mut self) -> io::Result<()> {
        self.writer.write_const::<BITS, VALUE>()
    }

    #[inline]
    fn write_var<I>(&mut self, bits: u32, value: I) -> io::Result<()>
    where
        I: Integer,
    {
        self.writer.write_var(bits, value)
    }

    #[inline]
    fn write_unsigned<const BITS: u32, U>(&mut self, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        BitWrite::write_unsigned::<BITS, U>(&mut self.writer, value)
    }

    #[inline]
    fn write_unsigned_var<U>(&mut self, bits: u32, value: U) -> io::Result<()>
    where
        U: UnsignedInteger,
    {
        self.writer.write_unsigned_var(bits, value)
    }

    #[inline]
    fn write_signed<const BITS: u32, S>(&mut self, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        BitWrite::write_signed::<BITS, S>(&mut self.writer, value)
    }

    #[inline(always)]
    fn write_signed_var<S>(&mut self, bits: u32, value: S) -> io::Result<()>
    where
        S: SignedInteger,
    {
        self.writer.write_signed_var(bits, value)
    }

    #[inline]
    fn write_count<const MAX: u32>(&mut self, count: BitCount<MAX>) -> io::Result<()> {
        self.writer.write_count::<MAX>(count)
    }

    #[inline]
    fn write_counted<const MAX: u32, I>(&mut self, bits: BitCount<MAX>, value: I) -> io::Result<()>
    where
        I: Integer + Sized,
    {
        self.writer.write_counted::<MAX, I>(bits, value)
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
        self.writer.write_unsigned_counted::<BITS, U>(bits, value)
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
        self.writer.write_signed_counted::<MAX, S>(bits, value)
    }

    #[inline]
    fn write_from<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive,
    {
        BitWrite::write_from::<V>(&mut self.writer, value)
    }

    #[inline]
    fn write_as_from<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        BitWrite::write_as_from::<F, V>(&mut self.writer, value)
    }

    #[inline]
    fn pad(&mut self, bits: u32) -> io::Result<()> {
        BitWrite::pad(&mut self.writer, bits)
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        BitWrite::write_bytes(&mut self.writer, buf)
    }

    #[inline]
    fn write_unary<const STOP_BIT: u8>(&mut self, value: u32) -> io::Result<()> {
        self.writer.write_unary::<STOP_BIT>(value)
    }

    #[inline]
    fn build<T: ToBitStream>(&mut self, build: &T) -> Result<(), T::Error> {
        BitWrite::build(&mut self.writer, build)
    }

    #[inline]
    fn build_with<'a, T: ToBitStreamWith<'a>>(
        &mut self,
        build: &T,
        context: &T::Context,
    ) -> Result<(), T::Error> {
        BitWrite::build_with(&mut self.writer, build, context)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        BitWrite::byte_aligned(&self.writer)
    }

    #[inline]
    fn byte_align(&mut self) -> io::Result<()> {
        BitWrite::byte_align(&mut self.writer)
    }

    #[inline]
    fn write_huffman<T>(&mut self, value: T::Symbol) -> io::Result<()>
    where
        T: crate::huffman::ToBits,
    {
        BitWrite::write_huffman::<T>(&mut self.writer, value)
    }
}

impl<N: PartialOrd + Counter + Copy, E: Endianness> BitRecorder<N, E> {
    /// Returns shortest option between ourself and candidate
    ///
    /// Executes fallible closure on emptied candidate recorder,
    /// compares the lengths of ourself and the candidate,
    /// and returns the shorter of the two.
    ///
    /// If the new candidate is shorter, we swap ourself and
    /// the candidate so any recorder capacity can be reused.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_bitstream::{BitRecorder, BitWrite, BigEndian};
    ///
    /// let mut best = BitRecorder::<u8, BigEndian>::new();
    /// let mut candidate = BitRecorder::new();
    ///
    /// // write an 8 bit value to our initial candidate
    /// best.write::<8, u8>(0);
    /// assert_eq!(best.written(), 8);
    ///
    /// // try another candidate which writes 4 bits
    /// best = best.best(&mut candidate, |w| {
    ///     w.write::<4, u8>(0)
    /// }).unwrap();
    ///
    /// // which becomes our new best candidate
    /// assert_eq!(best.written(), 4);
    ///
    /// // finally, try a not-so-best candidate
    /// // which writes 10 bits
    /// best = best.best(&mut candidate, |w| {
    ///     w.write::<10, u16>(0)
    /// }).unwrap();
    ///
    /// // so our best candidate remains 4 bits
    /// assert_eq!(best.written(), 4);
    /// ```
    pub fn best<F>(
        mut self,
        candidate: &mut Self,
        f: impl FnOnce(&mut Self) -> Result<(), F>,
    ) -> Result<Self, F> {
        candidate.clear();

        f(candidate)?;

        if candidate.written() < self.written() {
            core::mem::swap(&mut self, candidate);
        }

        Ok(self)
    }
}

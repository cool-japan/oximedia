// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Declarative streaming traits (`ToBitStream` / `ToByteStream` and their
//! context-carrying variants).

use core::convert::TryInto;
use std::io;

use super::{BitWrite, BitsWritten, ByteWrite, Counter, Endianness, Overflowed, Primitive};

/// Implemented by complex types that don't require any additional context
/// to build themselves to a writer
///
/// # Example
/// ```
/// use std::io::Read;
/// use oximedia_bitstream::{BigEndian, BitWrite, BitWriter, ToBitStream};
///
/// #[derive(Debug, PartialEq, Eq)]
/// struct BlockHeader {
///     last_block: bool,
///     block_type: u8,
///     block_size: u32,
/// }
///
/// impl ToBitStream for BlockHeader {
///     type Error = std::io::Error;
///
///     fn to_writer<W: BitWrite + ?Sized>(&self, w: &mut W) -> std::io::Result<()> {
///         w.write_bit(self.last_block)?;
///         w.write::<7, _>(self.block_type)?;
///         w.write::<24, _>(self.block_size)
///     }
/// }
///
/// let mut data = Vec::new();
/// let mut writer = BitWriter::endian(&mut data, BigEndian);
/// writer.build(&BlockHeader { last_block: false, block_type: 4, block_size: 122 }).unwrap();
/// assert_eq!(data, b"\x04\x00\x00\x7A");
/// ```
pub trait ToBitStream {
    /// Error generated during building, such as `io::Error`
    type Error;

    /// Generate self to writer
    fn to_writer<W: BitWrite + ?Sized>(&self, w: &mut W) -> Result<(), Self::Error>
    where
        Self: Sized;

    /// Returns length of self in bits, if possible
    fn bits<C: Counter>(&self) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        let mut c: BitsWritten<C> = BitsWritten::default();
        self.to_writer(&mut c)?;
        Ok(c.into_written())
    }

    /// Returns total length of self, if possible
    #[deprecated(since = "4.0.0", note = "use of bits() is preferred")]
    #[inline]
    fn bits_len<C: Counter, E: Endianness>(&self) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        self.bits()
    }
}

/// Implemented by complex types that require additional context
/// to build themselves to a writer
pub trait ToBitStreamWith<'a> {
    /// Some context to use when writing
    type Context: 'a;

    /// Error generated during building, such as `io::Error`
    type Error;

    /// Generate self to writer
    fn to_writer<W: BitWrite + ?Sized>(
        &self,
        w: &mut W,
        context: &Self::Context,
    ) -> Result<(), Self::Error>
    where
        Self: Sized;

    /// Returns length of self in bits, if possible
    fn bits<C: Counter>(&self, context: &Self::Context) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        let mut c: BitsWritten<C> = BitsWritten::default();
        self.to_writer(&mut c, context)?;
        Ok(c.into_written())
    }

    /// Returns total length of self, if possible
    #[deprecated(since = "4.0.0", note = "use of len() is preferred")]
    #[inline]
    fn bits_len<C: Counter, E: Endianness>(&self, context: &Self::Context) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        self.bits(context)
    }
}

/// Implemented by complex types that consume additional context
/// to build themselves to a writer
pub trait ToBitStreamUsing {
    /// Some context to consume when writing
    type Context;

    /// Error generated during building, such as `io::Error`
    type Error;

    /// Generate self to writer
    fn to_writer<W: BitWrite + ?Sized>(
        &self,
        w: &mut W,
        context: Self::Context,
    ) -> Result<(), Self::Error>
    where
        Self: Sized;

    /// Returns length of self in bits, if possible
    fn bits<C: Counter>(&self, context: Self::Context) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        let mut c: BitsWritten<C> = BitsWritten::default();
        self.to_writer(&mut c, context)?;
        Ok(c.into_written())
    }
}

/// Implemented by complex types that don't require any additional context
/// to build themselves to a writer
pub trait ToByteStream {
    /// Error generated during building, such as `io::Error`
    type Error;

    /// Generate self to writer
    fn to_writer<W: ByteWrite + ?Sized>(&self, w: &mut W) -> Result<(), Self::Error>
    where
        Self: Sized;

    /// Returns length of self in bytes, if possible
    fn bytes<C: Counter>(&self) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        let mut counter = ByteCount::default();
        self.to_writer(&mut counter)?;
        Ok(counter.writer.count)
    }
}

/// Implemented by complex types that require additional context
/// to build themselves to a writer
pub trait ToByteStreamWith<'a> {
    /// Some context to use when writing
    type Context: 'a;

    /// Error generated during building, such as `io::Error`
    type Error;

    /// Generate self to writer
    fn to_writer<W: ByteWrite + ?Sized>(
        &self,
        w: &mut W,
        context: &Self::Context,
    ) -> Result<(), Self::Error>
    where
        Self: Sized;

    /// Returns length of self in bytes, if possible
    fn bytes<C: Counter>(&self, context: &Self::Context) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        let mut counter = ByteCount::default();
        self.to_writer(&mut counter, context)?;
        Ok(counter.writer.count)
    }
}

/// Implemented by complex types that consume additional context
/// to build themselves to a writer
pub trait ToByteStreamUsing {
    /// Some context to consume when writing
    type Context;

    /// Error generated during building, such as `io::Error`
    type Error;

    /// Generate self to writer
    fn to_writer<W: ByteWrite + ?Sized>(
        &self,
        w: &mut W,
        context: Self::Context,
    ) -> Result<(), Self::Error>
    where
        Self: Sized;

    /// Returns length of self in bytes, if possible
    fn bytes<C: Counter>(&self, context: Self::Context) -> Result<C, Self::Error>
    where
        Self: Sized,
    {
        let mut counter = ByteCount::default();
        self.to_writer(&mut counter, context)?;
        Ok(counter.writer.count)
    }
}

#[derive(Default)]
struct ByteCounterWriter<C> {
    count: C,
}

impl<C: Counter> io::Write for ByteCounterWriter<C> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.count
            .checked_add_assign(buf.len().try_into().map_err(|_| Overflowed)?)?;

        Ok(buf.len())
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        // nothing to do
        Ok(())
    }
}

#[derive(Default)]
struct ByteCount<C> {
    writer: ByteCounterWriter<C>,
}

impl<C: Counter> ByteWrite for ByteCount<C> {
    fn write<V: Primitive>(&mut self, _value: V) -> io::Result<()> {
        self.writer.count.checked_add_assign(
            V::buffer()
                .as_ref()
                .len()
                .try_into()
                .map_err(|_| Overflowed)?,
        )?;

        Ok(())
    }

    fn write_as<F: Endianness, V: Primitive>(&mut self, _value: V) -> io::Result<()> {
        self.writer.count.checked_add_assign(
            V::buffer()
                .as_ref()
                .len()
                .try_into()
                .map_err(|_| Overflowed)?,
        )?;

        Ok(())
    }

    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        self.writer
            .count
            .checked_add_assign(buf.len().try_into().map_err(|_| Overflowed)?)?;

        Ok(())
    }

    fn pad(&mut self, bytes: u32) -> io::Result<()> {
        self.writer
            .count
            .checked_add_assign(bytes.try_into().map_err(|_| Overflowed)?)?;

        Ok(())
    }

    fn writer_ref(&mut self) -> &mut dyn io::Write {
        &mut self.writer
    }
}

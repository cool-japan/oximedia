// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! `BitWriter` — the concrete partial-byte accumulator.

use std::io;

use super::{BitWrite, ByteWriter, Endianness, PhantomData};

/// For writing bit values to an underlying stream in a given endianness.
///
/// Because this only writes whole bytes to the underlying stream,
/// it is important that output is byte-aligned before the bitstream
/// writer's lifetime ends.
/// **Partial bytes will be lost** if the writer is disposed of
/// before they can be written.
pub struct BitWriter<W: io::Write, E: Endianness> {
    // our underlying writer
    pub(crate) writer: W,
    // our partial byte
    pub(crate) value: u8,
    // the number of bits in our partial byte
    pub(crate) bits: u32,
    // a container for our endianness
    phantom: PhantomData<E>,
}

impl<W: io::Write, E: Endianness> BitWriter<W, E> {
    /// Wraps a BitWriter around something that implements `Write`
    pub fn new(writer: W) -> BitWriter<W, E> {
        BitWriter {
            writer,
            value: 0,
            bits: 0,
            phantom: PhantomData,
        }
    }

    /// Wraps a BitWriter around something that implements `Write`
    /// with the given endianness.
    pub fn endian(writer: W, _endian: E) -> BitWriter<W, E> {
        BitWriter {
            writer,
            value: 0,
            bits: 0,
            phantom: PhantomData,
        }
    }

    /// Unwraps internal writer and disposes of BitWriter.
    ///
    /// # Warning
    ///
    /// Any unwritten partial bits are discarded.
    #[inline]
    pub fn into_writer(self) -> W {
        self.writer
    }

    /// If stream is byte-aligned, provides mutable reference
    /// to internal writer.  Otherwise returns `None`
    #[inline]
    pub fn writer(&mut self) -> Option<&mut W> {
        if BitWrite::byte_aligned(self) {
            Some(&mut self.writer)
        } else {
            None
        }
    }

    /// Returns byte-aligned mutable reference to internal writer.
    ///
    /// Bytes aligns stream if it is not already aligned.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    #[inline]
    pub fn aligned_writer(&mut self) -> io::Result<&mut W> {
        BitWrite::byte_align(self)?;
        Ok(&mut self.writer)
    }

    /// Converts `BitWriter` to `ByteWriter` in the same endianness.
    ///
    /// # Warning
    ///
    /// Any written partial bits are discarded.
    #[inline]
    pub fn into_bytewriter(self) -> ByteWriter<W, E> {
        ByteWriter::new(self.into_writer())
    }

    /// If stream is byte-aligned, provides temporary `ByteWriter`
    /// in the same endianness.  Otherwise returns `None`
    ///
    /// # Warning
    ///
    /// Any unwritten bits left over when `ByteWriter` is dropped are lost.
    #[inline]
    pub fn bytewriter(&mut self) -> Option<ByteWriter<&mut W, E>> {
        self.writer().map(ByteWriter::new)
    }

    /// Flushes output stream to disk, if necessary.
    /// Any partial bytes are not flushed.
    ///
    /// # Errors
    ///
    /// Passes along any errors from the underlying stream.
    #[inline(always)]
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

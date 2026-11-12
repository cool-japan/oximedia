// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! `ByteWriter` and the `ByteWrite` trait for whole-byte output.

use std::io;

use super::{
    BitWriter, Endianness, PhantomData, Primitive, ToByteStream, ToByteStreamUsing,
    ToByteStreamWith,
};

/// For writing aligned bytes to a stream of bytes in a given endianness.
///
/// This only writes aligned values and maintains no internal state.
pub struct ByteWriter<W: io::Write, E: Endianness> {
    phantom: PhantomData<E>,
    writer: W,
}

impl<W: io::Write, E: Endianness> ByteWriter<W, E> {
    /// Wraps a ByteWriter around something that implements `Write`
    pub fn new(writer: W) -> ByteWriter<W, E> {
        ByteWriter {
            phantom: PhantomData,
            writer,
        }
    }

    /// Wraps a BitWriter around something that implements `Write`
    /// with the given endianness.
    pub fn endian(writer: W, _endian: E) -> ByteWriter<W, E> {
        ByteWriter {
            phantom: PhantomData,
            writer,
        }
    }

    /// Unwraps internal writer and disposes of `ByteWriter`.
    /// Any unwritten partial bits are discarded.
    #[inline]
    pub fn into_writer(self) -> W {
        self.writer
    }

    /// Provides mutable reference to internal writer.
    #[inline]
    pub fn writer(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Converts `ByteWriter` to `BitWriter` in the same endianness.
    #[inline]
    pub fn into_bitwriter(self) -> BitWriter<W, E> {
        BitWriter::new(self.into_writer())
    }

    /// Provides temporary `BitWriter` in the same endianness.
    ///
    /// # Warning
    ///
    /// Any unwritten bits left over when `BitWriter` is dropped are lost.
    #[inline]
    pub fn bitwriter(&mut self) -> BitWriter<&mut W, E> {
        BitWriter::new(self.writer())
    }
}

/// A trait for anything that can write aligned values to an output stream
pub trait ByteWrite {
    /// Writes whole numeric value to stream
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, ByteWriter, ByteWrite};
    /// let mut writer = ByteWriter::endian(Vec::new(), BigEndian);
    /// writer.write(0b0000000011111111u16).unwrap();
    /// assert_eq!(writer.into_writer(), [0b00000000, 0b11111111]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{LittleEndian, ByteWriter, ByteWrite};
    /// let mut writer = ByteWriter::endian(Vec::new(), LittleEndian);
    /// writer.write(0b0000000011111111u16).unwrap();
    /// assert_eq!(writer.into_writer(), [0b11111111, 0b00000000]);
    /// ```
    fn write<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive;

    /// Writes whole numeric value to stream in a potentially different endianness
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, ByteWriter, ByteWrite, LittleEndian};
    /// let mut writer = ByteWriter::endian(Vec::new(), BigEndian);
    /// writer.write_as::<LittleEndian, u16>(0b0000000011111111).unwrap();
    /// assert_eq!(writer.into_writer(), [0b11111111, 0b00000000]);
    /// ```
    ///
    /// ```
    /// use std::io::Write;
    /// use oximedia_bitstream::{BigEndian, ByteWriter, ByteWrite, LittleEndian};
    /// let mut writer = ByteWriter::endian(Vec::new(), LittleEndian);
    /// writer.write_as::<BigEndian, u16>(0b0000000011111111).unwrap();
    /// assert_eq!(writer.into_writer(), [0b00000000, 0b11111111]);
    /// ```
    fn write_as<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive;

    /// Writes the entirety of a byte buffer to the stream.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()>;

    /// Pads the stream by writing 0 over the given number of bytes.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn pad(&mut self, bytes: u32) -> io::Result<()>;

    /// Builds and writes complex type
    fn build<T: ToByteStream>(&mut self, build: &T) -> Result<(), T::Error> {
        build.to_writer(self)
    }

    /// Builds and writes complex type with context
    fn build_with<'a, T: ToByteStreamWith<'a>>(
        &mut self,
        build: &T,
        context: &T::Context,
    ) -> Result<(), T::Error> {
        build.to_writer(self, context)
    }

    /// Builds and writes complex type with owned context
    fn build_using<T: ToByteStreamUsing>(
        &mut self,
        build: &T,
        context: T::Context,
    ) -> Result<(), T::Error> {
        build.to_writer(self, context)
    }

    /// Returns mutable reference to underlying writer
    fn writer_ref(&mut self) -> &mut dyn io::Write;
}

impl<W: io::Write, E: Endianness> ByteWrite for ByteWriter<W, E> {
    #[inline]
    fn write<V>(&mut self, value: V) -> io::Result<()>
    where
        V: Primitive,
    {
        self.writer.write_all(E::primitive_to_bytes(value).as_ref())
    }

    #[inline]
    fn write_as<F, V>(&mut self, value: V) -> io::Result<()>
    where
        F: Endianness,
        V: Primitive,
    {
        self.writer.write_all(F::primitive_to_bytes(value).as_ref())
    }

    #[inline]
    fn pad(&mut self, mut bytes: u32) -> io::Result<()> {
        let buf = [0u8; 8];

        while bytes > 0 {
            let to_write = bytes.min(8);
            self.write_bytes(&buf[0..to_write as usize])?;
            bytes -= to_write;
        }
        Ok(())
    }

    #[inline]
    fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        self.writer.write_all(buf)
    }

    #[inline]
    fn writer_ref(&mut self) -> &mut dyn io::Write {
        &mut self.writer
    }
}

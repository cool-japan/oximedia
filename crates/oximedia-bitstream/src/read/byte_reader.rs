// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! `ByteReader` and the `ByteRead` trait for whole-byte input.

use std::io;
#[cfg(feature = "alloc")]
use std::vec::Vec;

use super::{
    read_to_vec, skip_aligned, BitReader, Endianness, FromByteStream, FromByteStreamUsing,
    FromByteStreamWith, PhantomData, Primitive,
};

/// A trait for anything that can read aligned values from an input stream
pub trait ByteRead {
    /// Reads whole numeric value from stream
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, ByteReader, ByteRead};
    /// let data = [0b00000000, 0b11111111];
    /// let mut reader = ByteReader::endian(data.as_slice(), BigEndian);
    /// assert_eq!(reader.read::<u16>().unwrap(), 0b0000000011111111);
    /// ```
    ///
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{LittleEndian, ByteReader, ByteRead};
    /// let data = [0b00000000, 0b11111111];
    /// let mut reader = ByteReader::endian(data.as_slice(), LittleEndian);
    /// assert_eq!(reader.read::<u16>().unwrap(), 0b1111111100000000);
    /// ```
    fn read<V>(&mut self) -> Result<V, io::Error>
    where
        V: Primitive;

    /// Reads whole numeric value from stream in a potentially different endianness
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    ///
    /// # Examples
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, ByteReader, ByteRead, LittleEndian};
    /// let data = [0b00000000, 0b11111111];
    /// let mut reader = ByteReader::endian(data.as_slice(), BigEndian);
    /// assert_eq!(reader.read_as::<LittleEndian, u16>().unwrap(), 0b1111111100000000);
    /// ```
    ///
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, ByteReader, ByteRead, LittleEndian};
    /// let data = [0b00000000, 0b11111111];
    /// let mut reader = ByteReader::endian(data.as_slice(), LittleEndian);
    /// assert_eq!(reader.read_as::<BigEndian, u16>().unwrap(), 0b0000000011111111);
    /// ```
    fn read_as<F, V>(&mut self) -> Result<V, io::Error>
    where
        F: Endianness,
        V: Primitive;

    /// Completely fills the given buffer with whole bytes.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        for b in buf.iter_mut() {
            *b = self.read()?;
        }
        Ok(())
    }

    /// Completely fills a whole buffer with bytes and returns it.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    #[inline(always)]
    #[deprecated(since = "1.8.0", note = "use read() method instead")]
    fn read_to_bytes<const SIZE: usize>(&mut self) -> io::Result<[u8; SIZE]> {
        self.read()
    }

    /// Completely fills a vector of bytes and returns it.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    fn read_to_vec(&mut self, bytes: usize) -> io::Result<Vec<u8>> {
        read_to_vec(|buf| self.read_bytes(buf), bytes)
    }

    /// Skips the given number of bytes in the stream.
    ///
    /// # Errors
    ///
    /// Passes along any I/O error from the underlying stream.
    fn skip(&mut self, bytes: u32) -> io::Result<()>;

    /// Parses and returns complex type
    fn parse<F: FromByteStream>(&mut self) -> Result<F, F::Error> {
        F::from_reader(self)
    }

    /// Parses and returns complex type with context
    fn parse_with<'a, F: FromByteStreamWith<'a>>(
        &mut self,
        context: &F::Context,
    ) -> Result<F, F::Error> {
        F::from_reader(self, context)
    }

    /// Parses and returns complex type with owned context
    fn parse_using<F: FromByteStreamUsing>(&mut self, context: F::Context) -> Result<F, F::Error> {
        F::from_reader(self, context)
    }

    /// Returns mutable reference to underlying reader
    fn reader_ref(&mut self) -> &mut dyn io::Read;
}

/// For reading aligned bytes from a stream of bytes in a given endianness.
///
/// This only reads aligned values and maintains no internal state.
#[derive(Debug)]
pub struct ByteReader<R: io::Read, E: Endianness> {
    phantom: PhantomData<E>,
    reader: R,
}

impl<R: io::Read, E: Endianness> ByteReader<R, E> {
    /// Wraps a ByteReader around something that implements `Read`
    pub fn new(reader: R) -> ByteReader<R, E> {
        ByteReader {
            phantom: PhantomData,
            reader,
        }
    }

    /// Wraps a ByteReader around something that implements `Read`
    /// with the given endianness.
    pub fn endian(reader: R, _endian: E) -> ByteReader<R, E> {
        ByteReader {
            phantom: PhantomData,
            reader,
        }
    }

    /// Unwraps internal reader and disposes of `ByteReader`.
    #[inline]
    pub fn into_reader(self) -> R {
        self.reader
    }

    /// Provides mutable reference to internal reader
    #[inline]
    pub fn reader(&mut self) -> &mut R {
        &mut self.reader
    }

    /// Converts `ByteReader` to `BitReader` in the same endianness.
    #[inline]
    pub fn into_bitreader(self) -> BitReader<R, E> {
        BitReader::new(self.into_reader())
    }

    /// Provides temporary `BitReader` in the same endianness.
    ///
    /// # Warning
    ///
    /// Any unread bits left over when `BitReader` is dropped are lost.
    #[inline]
    pub fn bitreader(&mut self) -> BitReader<&mut R, E> {
        BitReader::new(self.reader())
    }
}

impl<R: io::Read, E: Endianness> ByteRead for ByteReader<R, E> {
    #[inline]
    fn read<V>(&mut self) -> Result<V, io::Error>
    where
        V: Primitive,
    {
        let mut buf = V::buffer();
        self.read_bytes(buf.as_mut())?;
        Ok(E::bytes_to_primitive(buf))
    }

    #[inline]
    fn read_as<F, V>(&mut self) -> Result<V, io::Error>
    where
        F: Endianness,
        V: Primitive,
    {
        let mut buf = V::buffer();
        self.read_bytes(buf.as_mut())?;
        Ok(F::bytes_to_primitive(buf))
    }

    #[inline]
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.reader.read_exact(buf)
    }

    #[inline]
    fn skip(&mut self, bytes: u32) -> io::Result<()> {
        skip_aligned(&mut self.reader, bytes)
    }

    #[inline]
    fn reader_ref(&mut self) -> &mut dyn io::Read {
        &mut self.reader
    }
}

impl<R: io::Read + io::Seek, E: Endianness> io::Seek for ByteReader<R, E> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.reader().seek(pos)
    }
}

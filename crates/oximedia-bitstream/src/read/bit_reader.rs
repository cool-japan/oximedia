// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! The concrete `BitReader` type.

use core::convert::TryInto;
use std::io;

use super::{
    skip_aligned, BitCount, BitRead, ByteReader, Endianness, PhantomData, Primitive,
    SignedBitCount, SignedInteger, UnsignedInteger,
};

/// For reading non-aligned bits from a stream of bytes in a given endianness.
///
/// This will read exactly as many whole bytes needed to return
/// the requested number of bits.  It may cache up to a single partial byte
/// but no more.
#[derive(Clone, Debug)]
pub struct BitReader<R, E: Endianness> {
    // our underlying reader
    reader: R,
    // our partial byte
    value: u8,
    // the number of bits in our partial byte
    bits: u32,
    // a container for our endiannness
    phantom: PhantomData<E>,
}

impl<R, E: Endianness> BitReader<R, E> {
    /// Wraps a BitReader around something that implements `Read`
    pub fn new(reader: R) -> BitReader<R, E> {
        BitReader {
            reader,
            value: 0,
            bits: 0,
            phantom: PhantomData,
        }
    }

    /// Wraps a BitReader around something that implements `Read`
    /// with the given endianness.
    pub fn endian(reader: R, _endian: E) -> BitReader<R, E> {
        BitReader {
            reader,
            value: 0,
            bits: 0,
            phantom: PhantomData,
        }
    }

    /// Unwraps internal reader and disposes of BitReader.
    ///
    /// # Warning
    ///
    /// Any unread partial bits are discarded.
    #[inline]
    pub fn into_reader(self) -> R {
        self.reader
    }
}

impl<R: io::Read, E: Endianness> BitReader<R, E> {
    /// If stream is byte-aligned, provides mutable reference
    /// to internal reader.  Otherwise returns `None`
    #[inline]
    pub fn reader(&mut self) -> Option<&mut R> {
        if BitRead::byte_aligned(self) {
            Some(&mut self.reader)
        } else {
            None
        }
    }

    /// Returns byte-aligned mutable reference to internal reader.
    ///
    /// Bytes aligns stream if it is not already aligned.
    #[inline]
    pub fn aligned_reader(&mut self) -> &mut R {
        BitRead::byte_align(self);
        &mut self.reader
    }

    /// Converts `BitReader` to `ByteReader` in the same endianness.
    ///
    /// # Warning
    ///
    /// Any unread partial bits are discarded.
    #[inline]
    pub fn into_bytereader(self) -> ByteReader<R, E> {
        ByteReader::new(self.into_reader())
    }

    /// If stream is byte-aligned, provides temporary `ByteReader`
    /// in the same endianness.  Otherwise returns `None`
    ///
    /// # Warning
    ///
    /// Any reader bits left over when `ByteReader` is dropped are lost.
    #[inline]
    pub fn bytereader(&mut self) -> Option<ByteReader<&mut R, E>> {
        self.reader().map(ByteReader::new)
    }
}

impl<R: io::Read, E: Endianness> BitRead for BitReader<R, E> {
    #[inline(always)]
    fn read_bit(&mut self) -> io::Result<bool> {
        let Self {
            value,
            bits,
            reader,
            ..
        } = self;
        E::pop_bit_refill(reader, value, bits)
    }

    #[inline(always)]
    fn read_unsigned_counted<const BITS: u32, U>(&mut self, bits: BitCount<BITS>) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        let Self {
            value: queue_value,
            bits: queue_bits,
            reader,
            ..
        } = self;
        E::read_bits(reader, queue_value, queue_bits, bits)
    }

    #[inline]
    fn read_unsigned<const BITS: u32, U>(&mut self) -> io::Result<U>
    where
        U: UnsignedInteger,
    {
        let Self {
            value,
            bits,
            reader,
            ..
        } = self;
        E::read_bits_fixed::<BITS, R, U>(reader, value, bits)
    }

    #[inline(always)]
    fn read_signed_counted<const MAX: u32, S>(
        &mut self,
        bits: impl TryInto<SignedBitCount<MAX>>,
    ) -> io::Result<S>
    where
        S: SignedInteger,
    {
        E::read_signed_counted(
            self,
            bits.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "signed reads need at least 1 bit for sign",
                )
            })?,
        )
    }

    #[inline]
    fn read_signed<const BITS: u32, S>(&mut self) -> io::Result<S>
    where
        S: SignedInteger,
    {
        let count = const {
            assert!(BITS <= S::BITS_SIZE, "excessive bits for type read");
            let count = BitCount::<BITS>::new::<BITS>().signed_count();
            match count {
                Some(c) => c,
                None => panic!("signed reads need at least 1 bit for sign"),
            }
        };

        E::read_signed_counted(self, count)
    }

    #[inline]
    fn read_to<V>(&mut self) -> io::Result<V>
    where
        V: Primitive,
    {
        let mut buffer = V::buffer();
        E::read_bytes::<8, _>(
            &mut self.reader,
            &mut self.value,
            self.bits,
            buffer.as_mut(),
        )?;
        Ok(E::bytes_to_primitive(buffer))
    }

    #[inline]
    fn read_as_to<F, V>(&mut self) -> io::Result<V>
    where
        F: Endianness,
        V: Primitive,
    {
        let mut buffer = V::buffer();
        F::read_bytes::<8, _>(
            &mut self.reader,
            &mut self.value,
            self.bits,
            buffer.as_mut(),
        )?;
        Ok(F::bytes_to_primitive(buffer))
    }

    /// # Examples
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data = [0b10110111];
    /// let mut reader = BitReader::endian(data.as_slice(), BigEndian);
    /// assert!(reader.skip(3).is_ok());
    /// assert_eq!(reader.read::<5, u8>().unwrap(), 0b10111);
    /// ```
    ///
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{LittleEndian, BitReader, BitRead};
    /// let data = [0b10110111];
    /// let mut reader = BitReader::endian(data.as_slice(), LittleEndian);
    /// assert!(reader.skip(3).is_ok());
    /// assert_eq!(reader.read::<5, u8>().unwrap(), 0b10110);
    /// ```
    fn skip(&mut self, mut bits: u32) -> io::Result<()> {
        if BitRead::byte_aligned(self) && bits % 8 == 0 {
            skip_aligned(self.reader.by_ref(), bits / 8)
        } else {
            loop {
                match bits {
                    0 => break Ok(()),
                    bits @ 1..64 => break self.read_var(bits).map(|_: u64| ()),
                    _ => {
                        let _ = BitRead::read::<64, u64>(self)?;
                        bits -= 64;
                    }
                }
            }
        }
    }

    /// # Example
    /// ```
    /// use std::io::Read;
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data = b"foobar";
    /// let mut reader = BitReader::endian(data.as_slice(), BigEndian);
    /// assert!(reader.skip(24).is_ok());
    /// let mut buf = [0;3];
    /// assert!(reader.read_bytes(&mut buf).is_ok());
    /// assert_eq!(&buf, b"bar");
    /// ```
    #[inline]
    fn read_bytes(&mut self, buf: &mut [u8]) -> io::Result<()> {
        E::read_bytes::<1024, _>(&mut self.reader, &mut self.value, self.bits, buf)
    }

    fn read_unary<const STOP_BIT: u8>(&mut self) -> io::Result<u32> {
        let Self {
            value,
            bits,
            reader,
            ..
        } = self;
        E::pop_unary::<STOP_BIT, R>(reader, value, bits)
    }

    #[inline]
    fn byte_aligned(&self) -> bool {
        self.bits == 0
    }

    #[inline]
    fn byte_align(&mut self) {
        self.value = 0;
        self.bits = 0;
    }
}

impl<R, E> BitReader<R, E>
where
    E: Endianness,
    R: io::Read + io::Seek,
{
    /// # Example
    /// ```
    /// use std::io::{Read, Cursor, SeekFrom};
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data = [0x00, 0xFF];
    /// let mut reader = BitReader::endian(Cursor::new(&data), BigEndian);
    /// assert_eq!(reader.position_in_bits().unwrap(), 0);
    ///
    /// let pos = reader.seek_bits(SeekFrom::Start(5)).unwrap();
    /// assert!(pos == 5 && 5 == reader.position_in_bits().unwrap());
    ///
    /// let pos = reader.seek_bits(SeekFrom::Current(-2)).unwrap();
    /// assert!(pos == 3 && 3 == reader.position_in_bits().unwrap());
    ///
    /// let pos = reader.seek_bits(SeekFrom::End(5)).unwrap();
    /// assert!(pos == 11 && 11 == reader.position_in_bits().unwrap());
    /// ```
    pub fn seek_bits(&mut self, from: io::SeekFrom) -> io::Result<u64> {
        match from {
            io::SeekFrom::Start(from_start_pos) => {
                let (bytes, bits) = (from_start_pos / 8, (from_start_pos % 8) as u32);
                BitRead::byte_align(self);
                self.reader.seek(io::SeekFrom::Start(bytes))?;
                BitRead::skip(self, bits)?;
                Ok(from_start_pos)
            }
            io::SeekFrom::End(from_end_pos) => {
                let reader_end = self.reader.seek(io::SeekFrom::End(0))?;
                let new_pos = (reader_end * 8) as i64 - from_end_pos;
                assert!(new_pos >= 0, "The final position should be greater than 0");
                self.seek_bits(io::SeekFrom::Start(new_pos as u64))
            }
            io::SeekFrom::Current(offset) => {
                let new_pos = self.position_in_bits()? as i64 + offset;
                assert!(new_pos >= 0, "The final position should be greater than 0");
                self.seek_bits(io::SeekFrom::Start(new_pos as u64))
            }
        }
    }

    /// # Example
    /// ```
    /// use std::fs::read;
    /// use std::io::{Read, Cursor, SeekFrom};
    /// use oximedia_bitstream::{BigEndian, BitReader, BitRead};
    /// let data = [0x00, 0xFF];
    /// let mut reader = BitReader::endian(Cursor::new(&data), BigEndian);
    /// assert_eq!(reader.position_in_bits().unwrap(), 0);
    ///
    /// let _: i32 = reader.read_signed::<5, _>().unwrap();
    /// assert_eq!(reader.position_in_bits().unwrap(), 5);
    ///
    /// reader.read_bit().unwrap();
    /// assert_eq!(reader.position_in_bits().unwrap(), 6);
    /// ```
    #[inline]
    #[allow(clippy::seek_from_current)]
    pub fn position_in_bits(&mut self) -> io::Result<u64> {
        // core2 doesn't have `seek_from_current`
        let bytes = self.reader.seek(io::SeekFrom::Current(0))?;
        Ok(bytes * 8 - (self.bits as u64))
    }
}

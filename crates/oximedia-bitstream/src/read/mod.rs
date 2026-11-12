// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Traits and implementations for reading bits from a stream.
//!
//! Split across multiple files during the 0.1.4 refactor so that no
//! single source exceeds the COOLJAPAN 2 000-line guideline.

#![warn(missing_docs)]

use std::io;
#[cfg(feature = "alloc")]
use std::vec::Vec;

use super::{
    BitCount, CheckablePrimitive, Endianness, Integer, PhantomData, Primitive, SignedBitCount,
    SignedInteger, UnsignedInteger, VBRInteger,
};

mod bit_read;
mod bit_read2;
mod bit_reader;
mod byte_reader;
mod stream_traits;

pub use bit_read::BitRead;
pub use bit_read2::BitRead2;
pub use bit_reader::BitReader;
pub use byte_reader::{ByteRead, ByteReader};
pub use stream_traits::{
    FromBitStream, FromBitStreamUsing, FromBitStreamWith, FromByteStream, FromByteStreamUsing,
    FromByteStreamWith,
};

/// An error returned if performing VBR read overflows
#[derive(Copy, Clone, Debug)]
pub(crate) struct VariableWidthOverflow;

impl core::fmt::Display for VariableWidthOverflow {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        "variable bit rate overflowed".fmt(f)
    }
}

impl core::error::Error for VariableWidthOverflow {}

impl From<VariableWidthOverflow> for io::Error {
    fn from(VariableWidthOverflow: VariableWidthOverflow) -> Self {
        io::Error::new(
            #[cfg(feature = "std")]
            {
                io::ErrorKind::StorageFull
            },
            #[cfg(not(feature = "std"))]
            {
                io::ErrorKind::Other
            },
            "variable bit rate overflow",
        )
    }
}

/// Chunked skip helper shared between `BitReader` and `ByteReader`.
pub(crate) fn skip_aligned<R>(reader: R, bytes: u32) -> io::Result<()>
where
    R: io::Read,
{
    fn skip_chunks<const SIZE: usize, R>(mut reader: R, mut bytes: usize) -> io::Result<()>
    where
        R: io::Read,
    {
        let mut buf = [0; SIZE];
        while bytes > 0 {
            let to_read = bytes.min(SIZE);
            reader.read_exact(&mut buf[0..to_read])?;
            bytes -= to_read;
        }
        Ok(())
    }

    match bytes {
        0..256 => skip_chunks::<8, R>(reader, bytes as usize),
        256..1024 => skip_chunks::<256, R>(reader, bytes as usize),
        1024..4096 => skip_chunks::<1024, R>(reader, bytes as usize),
        _ => skip_chunks::<4096, R>(reader, bytes as usize),
    }
}

/// Shared helper for `read_to_vec` on both `BitRead` and `ByteRead`.
#[cfg(feature = "alloc")]
pub(crate) fn read_to_vec(
    mut read: impl FnMut(&mut [u8]) -> io::Result<()>,
    bytes: usize,
) -> io::Result<Vec<u8>> {
    const MAX_CHUNK: usize = 4096;

    match bytes {
        0 => Ok(Vec::new()),
        bytes if bytes <= MAX_CHUNK => {
            let mut buf = vec![0; bytes];
            read(&mut buf)?;
            Ok(buf)
        }
        mut bytes => {
            let mut whole = Vec::with_capacity(MAX_CHUNK);
            let mut chunk: [u8; MAX_CHUNK] = [0; MAX_CHUNK];
            while bytes > 0 {
                let chunk_size = bytes.min(MAX_CHUNK);
                let chunk = &mut chunk[0..chunk_size];
                read(chunk)?;
                whole.extend_from_slice(chunk);
                bytes -= chunk_size;
            }
            Ok(whole)
        }
    }
}

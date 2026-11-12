// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Originally dual-licensed "MIT OR Apache-2.0" as part of `bitstream-io`
// 4.9.0 by Brian Langenberger. Redistributed here, per the terms of that
// upstream license, under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0> only. This file may not be
// copied, modified, or distributed except according to those terms.

//! Bit-level I/O for OxiMedia — a `std`-only fork of
//! [`bitstream-io`](https://crates.io/crates/bitstream-io) 4.9.0.
//!
//! `oximedia-bitstream` provides traits and structs for reading and writing
//! signed and unsigned integer values to streams that may not be aligned at
//! a whole byte.  Both big-endian and little-endian streams are supported.
//!
//! The crate is used internally by `oximedia-codec` for entropy coding
//! (FLAC predictor coefficients, Vorbis header parsing, AV1 OBU parsing, etc.)
//! and by `oximedia-container` for MP4 box I/O.
//!
//! # Core traits
//!
//! | Trait | Purpose |
//! |-------|---------|
//! | [`BitRead`] | Read bits from a stream, big- or little-endian |
//! | [`BitWrite`] | Write bits to a stream, big- or little-endian |
//! | [`ByteRead`] | Read whole bytes from a byte source |
//! | [`ByteWrite`] | Write whole bytes to any destination |
//! | [`FromBitStream`] | Deserialise a struct from a bit reader |
//! | [`ToBitStream`] | Serialise a struct to a bit writer |
//!
//! # Concrete types
//!
//! - [`BitReader`] — wraps any `std::io::Read` and exposes bit-level reads
//! - [`BitWriter`] — wraps any `std::io::Write` and exposes bit-level writes
//! - [`ByteReader`] — wraps any `std::io::Read` for whole-byte reads
//! - [`ByteWriter`] — wraps any `std::io::Write` for whole-byte writes
//! - [`BitRecorder`] (feature `alloc`) — records bits written for later replay
//! - [`BitsWritten`] — counts bits written without a backing writer
//!
//! Huffman coding helpers live in the [`huffman`] module via the
//! [`FromBits`](huffman::FromBits) and [`ToBits`](huffman::ToBits) traits.
//!
//! # Quick start
//!
//! ```
//! use std::io::Cursor;
//! use oximedia_bitstream::{BigEndian, BitReader, BitRead};
//!
//! let data = [0b1011_0100u8, 0b1100_1010u8];
//! let mut r = BitReader::endian(Cursor::new(&data), BigEndian);
//!
//! // Constant-bit-count read — validated at compile time (requires Rust 1.79+)
//! let high: u8 = r.read::<4, _>().unwrap();
//! assert_eq!(high, 0b1011);
//!
//! // Variable-bit-count read
//! let low: u8 = r.read_var(4).unwrap();
//! assert_eq!(low, 0b0100);
//! ```
//!
//! # Endianness
//!
//! Pass [`BigEndian`] or [`LittleEndian`] as a zero-sized type parameter
//! (or value) to [`BitReader::endian`] / [`BitWriter::endian`].  The endianness
//! is a compile-time phantom; switching endianness mid-stream requires creating
//! a new reader/writer around the same underlying stream.
//!
//! # Feature flags
//!
//! | Flag | Default | Effect |
//! |------|---------|--------|
//! | `std` | yes | Enables `alloc` |
//! | `alloc` | via `std` | Enables [`BitRecorder`] |
//!
//! # Upstream attribution
//!
//! This crate is derived from
//! [`bitstream-io`](https://crates.io/crates/bitstream-io) 4.9.0 by
//! Brian Langenberger, licensed under Apache-2.0 / MIT.
//! The OxiMedia fork removes the `core2` / `no_std` compatibility shim
//! (OxiMedia targets `std` Rust only) and adapts the crate to the
//! OxiMedia workspace conventions.

//! # Traits and helpers for bitstream handling functionality
//!
//! Bitstream readers are for reading signed and unsigned integer
//! values from a stream whose sizes may not be whole bytes.
//! Bitstream writers are for writing signed and unsigned integer
//! values to a stream, also potentially un-aligned at a whole byte.
//!
//! Both big-endian and little-endian streams are supported.
//!
//! The only requirement for wrapped reader streams is that they must
//! implement the [`io::Read`] trait, and the only requirement
//! for writer streams is that they must implement the [`io::Write`] trait.
//!
//! In addition, reader streams do not consume any more bytes
//! from the underlying reader than necessary, buffering only a
//! single partial byte as needed.
//! Writer streams also write out all whole bytes as they are accumulated.
//!
//! Readers and writers are also designed to work with integer
//! types of any possible size.
//! Many of Rust's built-in integer types are supported by default.

//! # Minimum Compiler Version
//!
//! Beginning with version 2.4, the minimum compiler version has been
//! updated to Rust 1.79.
//!
//! The issue is that reading an excessive number of
//! bits to a type which is too small to hold them,
//! or writing an excessive number of bits from too small of a type,
//! are always errors:
//! ```
//! use std::io::{Read, Cursor};
//! use oximedia_bitstream::{BigEndian, BitReader, BitRead};
//! let data = [0; 10];
//! let mut r = BitReader::endian(Cursor::new(&data), BigEndian);
//! let x: Result<u32, _> = r.read_var(64);  // reading 64 bits to u32 always fails at runtime
//! assert!(x.is_err());
//! ```
//! but those errors will not be caught until the program runs,
//! which is less than ideal for the common case in which
//! the number of bits is already known at compile-time.
//!
//! But starting with Rust 1.79, we can now have read and write methods
//! which take a constant number of bits and can validate the number of bits
//! are small enough for the type being read/written at compile-time:
//! ```rust,compile_fail
//! use std::io::{Read, Cursor};
//! use oximedia_bitstream::{BigEndian, BitReader, BitRead};
//! let data = [0; 10];
//! let mut r = BitReader::endian(Cursor::new(&data), BigEndian);
//! let x: Result<u32, _> = r.read::<64, _>();  // doesn't compile at all
//! ```
//! Since catching potential bugs at compile-time is preferable
//! to encountering errors at runtime, this will hopefully be
//! an improvement in the long run.

//! # Changes From 3.X.X
//!
//! Version 4.0.0 features significant optimizations to the [`BitRecorder`]
//! and deprecates the [`BitCounter`] in favor of [`BitsWritten`],
//! which no longer requires specifying an endianness.
//!
//! In addition, the [`BitRead::read_bytes`] and [`BitWrite::write_bytes`]
//! methods are significantly optimized in the case of non-aligned
//! reads and writes.
//!
//! Finally, the [`Endianness`] traits have been sealed so as not
//! to be implemented by other packages.  Given that other endianness
//! types are extremely rare in file formats and end users should not
//! have to implement this trait themselves, this should not be a
//! concern.
//!
//! # Changes From 2.X.X
//!
//! Version 3.0.0 has made many breaking changes to the [`BitRead`] and
//! [`BitWrite`] traits.
//!
//! The [`BitRead::read`] method takes a constant number of bits,
//! and the [`BitRead::read_var`] method takes a variable number of bits
//! (reversing the older [`BitRead2::read_in`] and [`BitRead2::read`]
//! calling methods to emphasize using the constant-based one,
//! which can do more validation at compile-time).
//! A new [`BitRead2`] trait uses the older calling convention
//! for compatibility with existing code and is available
//! for anything implementing [`BitRead`].
//!
//! In addition, the main reading methods return primitive types which
//! implement a new [`Integer`] trait,
//! which delegates to [`BitRead::read_unsigned`]
//! or [`BitRead::read_signed`] depending on whether the output
//! is an unsigned or signed type.
//!
//! [`BitWrite::write`] and [`BitWrite::write_var`] work
//! similarly to the reader's `read` methods, taking anything
//! that implements [`Integer`] and writing an unsigned or
//! signed value to [`BitWrite::write_unsigned`] or
//! [`BitWrite::write_signed`] as appropriate.
//!
//! And as with reading, a [`BitWrite2`] trait is offered
//! for compatibility.
//!
//! In addition, the Huffman code handling has been rewritten
//! to use a small amount of macro magic to write
//! code to read and write symbols at compile-time.
//! This is significantly faster than the older version
//! and can no longer fail to compile at runtime.
//!
//! Lastly, there's a new [`BitCount`] struct which wraps a humble
//! `u32` but encodes the maximum possible number of bits
//! at the type level.
//! This is intended for file formats which encode the number
//! of bits to be read in the format itself.
//! For example, FLAC's predictor coefficient precision
//! is a 4 bit value which indicates how large each predictor
//! coefficient is in bits
//! (each coefficient might be an `i32` type).
//! By keeping track of the maximum value at compile time
//! (4 bits' worth, in this case), we can eliminate
//! any need to check that coefficients aren't too large
//! for an `i32` at runtime.
//! This is accomplished by using [`BitRead::read_count`] to
//! read a [`BitCount`] and then reading final values with
//! that number of bits using [`BitRead::read_counted`].

//! # Migrating From Pre 1.0.0
//!
//! There are now [`BitRead`] and [`BitWrite`] traits for bitstream
//! reading and writing (analogous to the standard library's
//! `Read` and `Write` traits) which you will also need to import.
//! The upside to this approach is that library consumers
//! can now make functions and methods generic over any sort
//! of bit reader or bit writer, regardless of the underlying
//! stream byte source or endianness.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![forbid(unsafe_code)]

// `PhantomData` is re-used through `super::PhantomData` by the `read` and
// `write` submodules; keep the alias reachable from the crate root.
pub(crate) use core::marker::PhantomData;
use std::io;

pub mod byte_io;
pub mod huffman;
pub mod read;
pub mod write;
pub use read::{
    BitRead, BitRead2, BitReader, ByteRead, ByteReader, FromBitStream, FromBitStreamUsing,
    FromBitStreamWith, FromByteStream, FromByteStreamUsing, FromByteStreamWith,
};
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
#[cfg(feature = "alloc")]
pub use write::BitRecorder;
pub use write::{
    BitWrite, BitWrite2, BitWriter, BitsWritten, ByteWrite, ByteWriter, ToBitStream,
    ToBitStreamUsing, ToBitStreamWith, ToByteStream, ToByteStreamUsing, ToByteStreamWith,
};

#[allow(deprecated)]
pub use write::BitCounter;

// Split-out modules — the bitstream runtime surface is carved into
// focused files so that no single source exceeds the COOLJAPAN 2 000-line
// refactor guideline while preserving the original public API.
mod big_endian;
mod bitcount;
mod checked;
mod endian;
mod integer;
mod little_endian;

pub use big_endian::{BigEndian, BE};
pub use bitcount::{BitCount, SignedBitCount};
pub use checked::{
    Checkable, CheckablePrimitive, Checked, CheckedError, CheckedSigned, CheckedSignedFixed,
    CheckedUnsigned, CheckedUnsignedFixed, FixedBitCount, FixedSignedBitCount,
};
pub use endian::Endianness;
pub use integer::{Integer, Numeric, Primitive, SignedInteger, UnsignedInteger, VBRInteger};
pub use little_endian::{LittleEndian, LE};

mod private {
    use crate::{
        io, BitCount, BitRead, BitWrite, CheckedSigned, CheckedUnsigned, Primitive, SignedBitCount,
        SignedInteger, UnsignedInteger,
    };

    #[test]
    fn test_checked_signed() {
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<8>(), -128i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<8>(), 127i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<7>(), -64i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<7>(), 63i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<6>(), -32i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<6>(), 31i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<5>(), -16i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<5>(), 15i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<4>(), -8i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<4>(), 7i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<3>(), -4i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<3>(), 3i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<2>(), -2i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<2>(), 1i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<1>(), -1i8).is_ok());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<1>(), 0i8).is_ok());

        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<7>(), -65i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<7>(), 64i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<6>(), -33i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<6>(), 32i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<5>(), -17i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<5>(), 16i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<4>(), -9i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<4>(), 8i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<3>(), -5i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<3>(), 4i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<2>(), -3i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<2>(), 2i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<1>(), -2i8).is_err());
        assert!(CheckedSigned::new(SignedBitCount::<8>::new::<1>(), 1i8).is_err());
    }

    pub trait Endianness: Sized {
        /// Pops the next bit from the queue,
        /// repleneshing it from the given reader if necessary
        fn pop_bit_refill<R>(
            reader: &mut R,
            queue_value: &mut u8,
            queue_bits: &mut u32,
        ) -> io::Result<bool>
        where
            R: io::Read;

        /// Pops the next unary value from the source until
        /// `STOP_BIT` is encountered, replenishing it from the given
        /// closure if necessary.
        ///
        /// `STOP_BIT` must be 0 or 1.
        fn pop_unary<const STOP_BIT: u8, R>(
            reader: &mut R,
            queue_value: &mut u8,
            queue_bits: &mut u32,
        ) -> io::Result<u32>
        where
            R: io::Read;

        /// Pushes the next bit into the queue,
        /// and returns `Some` value if the queue is full.
        fn push_bit_flush(queue_value: &mut u8, queue_bits: &mut u32, bit: bool) -> Option<u8>;

        /// For performing bulk reads from a bit source to an output type.
        fn read_bits<const MAX: u32, R, U>(
            reader: &mut R,
            queue_value: &mut u8,
            queue_bits: &mut u32,
            count: BitCount<MAX>,
        ) -> io::Result<U>
        where
            R: io::Read,
            U: UnsignedInteger;

        /// For performing bulk reads from a bit source to an output type.
        fn read_bits_fixed<const BITS: u32, R, U>(
            reader: &mut R,
            queue_value: &mut u8,
            queue_bits: &mut u32,
        ) -> io::Result<U>
        where
            R: io::Read,
            U: UnsignedInteger;

        /// For performing a checked write to a bit sink
        fn write_bits_checked<const MAX: u32, W, U>(
            writer: &mut W,
            queue_value: &mut u8,
            queue_bits: &mut u32,
            value: CheckedUnsigned<MAX, U>,
        ) -> io::Result<()>
        where
            W: io::Write,
            U: UnsignedInteger;

        /// For performing a checked signed write to a bit sink
        fn write_signed_bits_checked<const MAX: u32, W, S>(
            writer: &mut W,
            queue_value: &mut u8,
            queue_bits: &mut u32,
            value: CheckedSigned<MAX, S>,
        ) -> io::Result<()>
        where
            W: io::Write,
            S: SignedInteger;

        /// Reads signed value from reader in this endianness
        fn read_signed_counted<const MAX: u32, R, S>(
            r: &mut R,
            bits: SignedBitCount<MAX>,
        ) -> io::Result<S>
        where
            R: BitRead,
            S: SignedInteger;

        /// Reads whole set of bytes to output buffer
        fn read_bytes<const CHUNK_SIZE: usize, R>(
            reader: &mut R,
            queue_value: &mut u8,
            queue_bits: u32,
            buf: &mut [u8],
        ) -> io::Result<()>
        where
            R: io::Read;

        /// Writes whole set of bytes to output buffer
        fn write_bytes<const CHUNK_SIZE: usize, W>(
            writer: &mut W,
            queue_value: &mut u8,
            queue_bits: u32,
            buf: &[u8],
        ) -> io::Result<()>
        where
            W: io::Write;

        /// Converts a primitive's byte buffer to a primitive
        fn bytes_to_primitive<P: Primitive>(buf: P::Bytes) -> P;

        /// Converts a primitive to a primitive's byte buffer
        fn primitive_to_bytes<P: Primitive>(p: P) -> P::Bytes;

        /// Reads convertable numeric value from reader in this endianness
        #[deprecated(since = "4.0.0")]
        fn read_primitive<R, V>(r: &mut R) -> io::Result<V>
        where
            R: BitRead,
            V: Primitive;

        /// Writes convertable numeric value to writer in this endianness
        #[deprecated(since = "4.0.0")]
        fn write_primitive<W, V>(w: &mut W, value: V) -> io::Result<()>
        where
            W: BitWrite,
            V: Primitive;
    }

    pub trait Checkable {
        fn write_endian<E, W>(
            self,
            writer: &mut W,
            queue_value: &mut u8,
            queue_bits: &mut u32,
        ) -> io::Result<()>
        where
            E: Endianness,
            W: io::Write;
    }
}

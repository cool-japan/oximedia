// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Declarative parsing traits (`FromBitStream` / `FromByteStream` and
//! their context-carrying variants).

use super::{BitRead, ByteRead};

/// Implemented by complex types that don't require any additional context
/// to parse themselves from a reader.  Analogous to [`std::str::FromStr`].
///
/// # Example
/// ```
/// use std::io::Read;
/// use oximedia_bitstream::{BigEndian, BitRead, BitReader, FromBitStream};
///
/// #[derive(Debug, PartialEq, Eq)]
/// struct BlockHeader {
///     last_block: bool,
///     block_type: u8,
///     block_size: u32,
/// }
///
/// impl FromBitStream for BlockHeader {
///     type Error = std::io::Error;
///
///     fn from_reader<R: BitRead + ?Sized>(r: &mut R) -> std::io::Result<Self> {
///         Ok(Self {
///             last_block: r.read_bit()?,
///             block_type: r.read::<7, _>()?,
///             block_size: r.read::<24, _>()?,
///         })
///     }
/// }
///
/// let mut reader = BitReader::endian(b"\x04\x00\x00\x7A".as_slice(), BigEndian);
/// assert_eq!(
///     reader.parse::<BlockHeader>().unwrap(),
///     BlockHeader { last_block: false, block_type: 4, block_size: 122 }
/// );
/// ```
pub trait FromBitStream {
    /// Error generated during parsing, such as `io::Error`
    type Error;

    /// Parse Self from reader
    fn from_reader<R: BitRead + ?Sized>(r: &mut R) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

/// Implemented by complex types that require some immutable context
/// to parse themselves from a reader.
///
/// # Example
/// ```
/// use std::io::Read;
/// use oximedia_bitstream::{BigEndian, BitRead, BitReader, FromBitStreamWith};
///
/// #[derive(Default)]
/// struct Streaminfo {
///     minimum_block_size: u16,
///     maximum_block_size: u16,
///     minimum_frame_size: u32,
///     maximum_frame_size: u32,
///     sample_rate: u32,
///     channels: u8,
///     bits_per_sample: u8,
///     total_samples: u64,
///     md5: [u8; 16],
/// }
///
/// #[derive(Debug, PartialEq, Eq)]
/// struct FrameHeader {
///     variable_block_size: bool,
///     block_size: u32,
///     sample_rate: u32,
///     channel_assignment: u8,
///     sample_size: u8,
///     frame_number: u64,
///     crc8: u8,
/// }
///
/// impl FromBitStreamWith<'_> for FrameHeader {
///     type Context = Streaminfo;
///
///     type Error = FrameHeaderError;
///
///     fn from_reader<R: BitRead + ?Sized>(
///         r: &mut R,
///         streaminfo: &Streaminfo,
///     ) -> Result<Self, Self::Error> {
///         if r.read::<14, u16>()? != 0b11111111111110 {
///             return Err(FrameHeaderError::InvalidSync);
///         }
///
///         if r.read_bit()? != false {
///             return Err(FrameHeaderError::InvalidReservedBit);
///         }
///
///         let variable_block_size = r.read_bit()?;
///
///         let block_size_bits = r.read::<4, u8>()?;
///
///         let sample_rate_bits = r.read::<4, u8>()?;
///
///         let channel_assignment = r.read::<4, u8>()?;
///
///         let sample_size = match r.read::<3, u8>()? {
///             0b000 => streaminfo.bits_per_sample,
///             0b001 => 8,
///             0b010 => 12,
///             0b011 => return Err(FrameHeaderError::InvalidSampleSize),
///             0b100 => 16,
///             0b101 => 20,
///             0b110 => 24,
///             0b111 => 32,
///             _ => unreachable!(),
///         };
///
///         if r.read_bit()? != false {
///             return Err(FrameHeaderError::InvalidReservedBit);
///         }
///
///         let frame_number = read_utf8(r)?;
///
///         Ok(FrameHeader {
///             variable_block_size,
///             block_size: match block_size_bits {
///                 0b0000 => return Err(FrameHeaderError::InvalidBlockSize),
///                 0b0001 => 192,
///                 n @ 0b010..=0b0101 => 576 * (1 << (n - 2)),
///                 0b0110 => r.read::<8, u32>()? + 1,
///                 0b0111 => r.read::<16, u32>()? + 1,
///                 n @ 0b1000..=0b1111 => 256 * (1 << (n - 8)),
///                 _ => unreachable!(),
///             },
///             sample_rate: match sample_rate_bits {
///                 0b0000 => streaminfo.sample_rate,
///                 0b0001 => 88200,
///                 0b0010 => 176400,
///                 0b0011 => 192000,
///                 0b0100 => 8000,
///                 0b0101 => 16000,
///                 0b0110 => 22050,
///                 0b0111 => 24000,
///                 0b1000 => 32000,
///                 0b1001 => 44100,
///                 0b1010 => 48000,
///                 0b1011 => 96000,
///                 0b1100 => r.read::<8, u32>()? * 1000,
///                 0b1101 => r.read::<16, u32>()?,
///                 0b1110 => r.read::<16, u32>()? * 10,
///                 0b1111 => return Err(FrameHeaderError::InvalidSampleRate),
///                 _ => unreachable!(),
///             },
///             channel_assignment,
///             sample_size,
///             frame_number,
///             crc8: r.read::<8, _>()?
///         })
///     }
/// }
///
/// #[derive(Debug)]
/// enum FrameHeaderError {
///     Io(std::io::Error),
///     InvalidSync,
///     InvalidReservedBit,
///     InvalidSampleSize,
///     InvalidBlockSize,
///     InvalidSampleRate,
/// }
///
/// impl From<std::io::Error> for FrameHeaderError {
///     fn from(err: std::io::Error) -> Self {
///         Self::Io(err)
///     }
/// }
///
/// fn read_utf8<R: BitRead + ?Sized>(r: &mut R) -> Result<u64, std::io::Error> {
///     r.read::<8, _>()  // left unimplimented in this example
/// }
///
/// let mut reader = BitReader::endian(b"\xFF\xF8\xC9\x18\x00\xC2".as_slice(), BigEndian);
/// assert_eq!(
///     reader.parse_with::<FrameHeader>(&Streaminfo::default()).unwrap(),
///     FrameHeader {
///         variable_block_size: false,
///         block_size: 4096,
///         sample_rate: 44100,
///         channel_assignment: 1,
///         sample_size: 16,
///         frame_number: 0,
///         crc8: 0xC2,
///     }
/// );
/// ```
///
/// # Example with lifetime-contrained `Context`
///
/// In some cases, the `Context` can depend on a reference to another `struct`.
///
/// ```
/// use std::io::Read;
/// use oximedia_bitstream::{BigEndian, BitRead, BitReader, FromBitStreamWith};
///
/// #[derive(Default)]
/// struct ModeParameters {
///     size_len: u8,
///     index_len: u8,
///     index_delta_len: u8,
///     // ...
/// }
///
/// struct AuHeaderParseContext<'a> {
///     params: &'a ModeParameters,
///     base_index: Option<u32>,
/// }
///
/// #[derive(Debug, PartialEq, Eq)]
/// struct AuHeader {
///     size: u32,
///     index: u32,
///     // ...
/// }
///
/// impl<'a> FromBitStreamWith<'a> for AuHeader {
///     type Context = AuHeaderParseContext<'a>;
///
///     type Error = AuHeaderError;
///
///     fn from_reader<R: BitRead + ?Sized>(
///         r: &mut R,
///         ctx: &AuHeaderParseContext<'a>,
///     ) -> Result<Self, Self::Error> {
///         let size = r.read_var::<u32>(ctx.params.size_len as u32)?;
///         let index = match ctx.base_index {
///             None => r.read_var::<u32>(ctx.params.index_len as u32)?,
///             Some(base_index) => {
///                 base_index
///                 + 1
///                 + r.read_var::<u32>(ctx.params.index_delta_len as u32)?
///             }
///         };
///
///         Ok(AuHeader {
///             size,
///             index,
///             // ...
///         })
///     }
/// }
///
/// #[derive(Debug)]
/// enum AuHeaderError {
///     Io(std::io::Error),
/// }
///
/// impl From<std::io::Error> for AuHeaderError {
///     fn from(err: std::io::Error) -> Self {
///         Self::Io(err)
///     }
/// }
///
/// let mut reader = BitReader::endian(b"\xFF\xEA\xFF\x10".as_slice(), BigEndian);
///
/// let mode_params = ModeParameters {
///     size_len: 10,
///     index_len: 6,
///     index_delta_len: 2,
///     // ...
/// };
///
/// let mut ctx = AuHeaderParseContext {
///     params: &mode_params,
///     base_index: None,
/// };
///
/// let header1 = reader.parse_with::<AuHeader>(&ctx).unwrap();
/// assert_eq!(
///     header1,
///     AuHeader {
///         size: 1023,
///         index: 42,
///     }
/// );
///
/// ctx.base_index = Some(header1.index);
///
/// assert_eq!(
///     reader.parse_with::<AuHeader>(&ctx).unwrap(),
///     AuHeader {
///         size: 1020,
///         index: 44,
///     }
/// );
/// ```
pub trait FromBitStreamWith<'a> {
    /// Some context to use when parsing
    type Context: 'a;

    /// Error generated during parsing, such as `io::Error`
    type Error;

    /// Parse Self from reader with the given context
    fn from_reader<R: BitRead + ?Sized>(
        r: &mut R,
        context: &Self::Context,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

/// Implemented by complex types that consume some immutable context
/// to parse themselves from a reader.
///
/// Like [`FromBitStreamWith`], but consumes its context
/// rather than taking a shared reference to it.
pub trait FromBitStreamUsing {
    /// Some context to consume when parsing
    type Context;

    /// Error generated during parsing, such as `io::Error`
    type Error;

    /// Parse Self from reader with the given context
    fn from_reader<R: BitRead + ?Sized>(
        r: &mut R,
        context: Self::Context,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

/// Implemented by complex types that don't require any additional context
/// to parse themselves from a reader.  Analagous to `FromStr`.
pub trait FromByteStream {
    /// Error generated during parsing, such as `io::Error`
    type Error;

    /// Parse Self from reader
    fn from_reader<R: ByteRead + ?Sized>(r: &mut R) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

/// Implemented by complex types that require some additional context
/// to parse themselves from a reader.  Analagous to `FromStr`.
pub trait FromByteStreamWith<'a> {
    /// Some context to use when parsing
    type Context: 'a;

    /// Error generated during parsing, such as `io::Error`
    type Error;

    /// Parse Self from reader
    fn from_reader<R: ByteRead + ?Sized>(
        r: &mut R,
        context: &Self::Context,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

/// Implemented by complex types that consume some additional context
/// to parse themselves from a reader.
///
/// Like [`FromByteStreamWith`], but consumes the context.
pub trait FromByteStreamUsing {
    /// Some context to use when parsing
    type Context;

    /// Error generated during parsing, such as `io::Error`
    type Error;

    /// Parse Self from reader
    fn from_reader<R: ByteRead + ?Sized>(
        r: &mut R,
        context: Self::Context,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

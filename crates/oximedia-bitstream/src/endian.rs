// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! `Endianness` trait and byte-level helpers.
//!
//! The sealed `Endianness` trait is the bridge between the public
//! BE/LE marker types and the internal bit-queue machinery defined in
//! [`crate::private`]. The tiny I/O helpers [`read_byte`] / [`write_byte`]
//! and the unary decoder [`find_unary`] live here because every endian
//! implementation in the crate depends on them.

use std::io;

use crate::private;
use crate::Numeric;

/// A stream's endianness, or byte order, for determining
/// how bits should be read.
///
/// It comes in `BigEndian` and `LittleEndian` varieties
/// (which may be shortened to `BE` and `LE`)
/// and is not something programmers should implement directly.
pub trait Endianness: private::Endianness {}

#[inline(always)]
pub(crate) fn read_byte<R>(mut reader: R) -> io::Result<u8>
where
    R: io::Read,
{
    let mut byte = 0;
    reader
        .read_exact(core::slice::from_mut(&mut byte))
        .map(|()| byte)
}

#[inline(always)]
pub(crate) fn write_byte<W>(mut writer: W, byte: u8) -> io::Result<()>
where
    W: io::Write,
{
    writer.write_all(core::slice::from_ref(&byte))
}

#[inline]
pub(crate) fn find_unary<R>(
    reader: &mut R,
    queue_value: &mut u8,
    queue_bits: &mut u32,
    leading_bits: impl Fn(u8) -> u32,
    max_bits: impl Fn(&mut u32) -> u32,
    checked_shift: impl Fn(u8, u32) -> Option<u8>,
) -> io::Result<u32>
where
    R: io::Read,
{
    let mut acc = 0;

    loop {
        match leading_bits(*queue_value) {
            bits if bits == max_bits(queue_bits) => {
                // all bits exhausted
                // fetch another byte and keep going
                acc += *queue_bits;
                *queue_value = read_byte(reader.by_ref())?;
                *queue_bits = u8::BITS_SIZE;
            }
            bits => match checked_shift(*queue_value, bits + 1) {
                Some(value) => {
                    // fetch part of source byte
                    *queue_value = value;
                    *queue_bits -= bits + 1;
                    break Ok(acc + bits);
                }
                None => {
                    // fetch all of source byte
                    *queue_value = 0;
                    *queue_bits = 0;
                    break Ok(acc + bits);
                }
            },
        }
    }
}

// Copyright 2017 Brian Langenberger
// Copyright 2024-2026 COOLJAPAN OU (Team KitaSan)
//
// Licensed under the Apache License, Version 2.0
// <http://www.apache.org/licenses/LICENSE-2.0>. See `lib.rs` for upstream
// attribution (this crate derives from `bitstream-io` 4.9.0).

//! Little-endian `Endianness` implementation.
//!
//! Split out of `lib.rs` during the 0.1.4 refactor.

use core::mem;
use std::io;

use crate::endian::{find_unary, read_byte, write_byte};
use crate::{
    private, BitCount, BitRead, BitWrite, Checked, CheckedSigned, CheckedUnsigned, Endianness,
    Numeric, Primitive, SignedBitCount, SignedInteger, UnsignedInteger,
};

/// Little-endian, or least significant bits first
#[derive(Copy, Clone, Debug)]
pub struct LittleEndian;

/// Little-endian, or least significant bits first
pub type LE = LittleEndian;

impl LittleEndian {
    // checked in the sense that we've verified
    // the output type is large enough to hold the
    // requested number of bits
    #[inline]
    fn read_bits_checked<const MAX: u32, R, U>(
        reader: &mut R,
        queue: &mut u8,
        queue_bits: &mut u32,
        BitCount { bits }: BitCount<MAX>,
    ) -> io::Result<U>
    where
        R: io::Read,
        U: UnsignedInteger,
    {
        // reads a whole value with the given number of
        // bytes in our endianness, where the number of bytes
        // must be less than or equal to the type's size in bytes
        #[inline(always)]
        fn read_bytes<R, U>(reader: &mut R, bytes: usize) -> io::Result<U>
        where
            R: io::Read,
            U: UnsignedInteger,
        {
            let mut buf = U::buffer();
            reader
                .read_exact(&mut buf.as_mut()[0..bytes])
                .map(|()| U::from_le_bytes(buf))
        }

        if bits <= *queue_bits {
            // all bits in queue, so no byte needed
            let value = *queue & u8::ALL.shr_default(u8::BITS_SIZE - bits);
            *queue = queue.shr_default(bits);
            *queue_bits -= bits;
            Ok(U::from_u8(value))
        } else {
            // at least one byte needed

            // bits needed beyond what's in the queue
            let needed_bits = bits - *queue_bits;

            match (needed_bits / 8, needed_bits % 8) {
                (0, needed) => {
                    // only one additional byte needed,
                    // which we share between our returned value
                    // and the bit queue
                    let next_byte = read_byte(reader)?;

                    Ok(
                        U::from_u8(mem::replace(queue, next_byte.shr_default(needed)))
                            | (U::from_u8(next_byte & (u8::ALL >> (u8::BITS_SIZE - needed)))
                                << mem::replace(queue_bits, u8::BITS_SIZE - needed)),
                    )
                }
                (bytes, 0) => {
                    // exact number of bytes needed beyond what's
                    // available in the queue

                    // so read a whole value from the reader
                    // and prepend what's left of our queue onto it

                    Ok(U::from_u8(mem::take(queue))
                        | (read_bytes::<R, U>(reader, bytes as usize)? << mem::take(queue_bits)))
                }
                (bytes, needed) => {
                    // read a whole value from the reader
                    // prepend what's in the queue at the front of it
                    // *and* append a partial byte at the end of it
                    // while also updating the queue and its bit count

                    let whole: U = read_bytes(reader, bytes as usize)?;
                    let next_byte = read_byte(reader)?;

                    Ok(
                        U::from_u8(mem::replace(queue, next_byte.shr_default(needed)))
                            | (whole << *queue_bits)
                            | (U::from_u8(next_byte & (u8::ALL >> (u8::BITS_SIZE - needed)))
                                << (mem::replace(queue_bits, u8::BITS_SIZE - needed) + bytes * 8)),
                    )
                }
            }
        }
    }
}

impl Endianness for LittleEndian {}

impl private::Endianness for LittleEndian {
    #[inline]
    fn push_bit_flush(queue_value: &mut u8, queue_bits: &mut u32, bit: bool) -> Option<u8> {
        *queue_value |= u8::from(bit) << *queue_bits;
        *queue_bits = (*queue_bits + 1) % 8;
        (*queue_bits == 0).then(|| mem::take(queue_value))
    }

    #[inline]
    fn read_bits<const MAX: u32, R, U>(
        reader: &mut R,
        queue_value: &mut u8,
        queue_bits: &mut u32,
        count @ BitCount { bits }: BitCount<MAX>,
    ) -> io::Result<U>
    where
        R: io::Read,
        U: UnsignedInteger,
    {
        if MAX <= U::BITS_SIZE || bits <= U::BITS_SIZE {
            Self::read_bits_checked::<MAX, R, U>(reader, queue_value, queue_bits, count)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive bits for type read",
            ))
        }
    }

    #[inline]
    fn read_bits_fixed<const BITS: u32, R, U>(
        reader: &mut R,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<U>
    where
        R: io::Read,
        U: UnsignedInteger,
    {
        const {
            assert!(BITS <= U::BITS_SIZE, "excessive bits for type read");
        }

        Self::read_bits_checked::<BITS, R, U>(
            reader,
            queue_value,
            queue_bits,
            BitCount::new::<BITS>(),
        )
    }

    // checked in the sense that we've verified
    // the input type is large enough to hold the
    // requested number of bits and that the value is
    // not too large for those bits
    #[inline]
    fn write_bits_checked<const MAX: u32, W, U>(
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: &mut u32,
        CheckedUnsigned {
            count: BitCount { bits },
            value,
        }: CheckedUnsigned<MAX, U>,
    ) -> io::Result<()>
    where
        W: io::Write,
        U: UnsignedInteger,
    {
        fn write_bytes<W, U>(writer: &mut W, bytes: usize, value: U) -> io::Result<()>
        where
            W: io::Write,
            U: UnsignedInteger,
        {
            let buf = U::to_le_bytes(value);
            writer.write_all(&buf.as_ref()[0..bytes])
        }

        // the amount of available bits in the queue
        let available_bits = u8::BITS_SIZE - *queue_bits;

        if bits < available_bits {
            // all bits fit in queue, so no write needed
            *queue_value |= U::to_u8(value.shl_default(*queue_bits));
            *queue_bits += bits;
            Ok(())
        } else {
            // at least one byte needs to be written

            // bits beyond what can fit in the queue
            let excess_bits = bits - available_bits;

            match (excess_bits / 8, excess_bits % 8) {
                (0, excess) => {
                    // only one byte to be written,
                    // while the excess bits are shared
                    // between the written byte and the bit queue

                    write_byte(
                        writer,
                        mem::replace(queue_value, U::to_u8(value.shr_default(available_bits)))
                            | U::to_u8(
                                (value << mem::replace(queue_bits, excess)) & U::from_u8(u8::ALL),
                            ),
                    )
                }
                (bytes, 0) => {
                    // no excess bytes beyond what can fit the queue
                    // so write a whole byte and
                    // the remainder of the whole value

                    write_byte(
                        writer.by_ref(),
                        mem::take(queue_value)
                            | U::to_u8((value << mem::take(queue_bits)) & U::from_u8(u8::ALL)),
                    )?;

                    write_bytes(writer, bytes as usize, value >> available_bits)
                }
                (bytes, excess) => {
                    // write what's in the queue along
                    // with the head of our whole value,
                    // write the middle section of our whole value,
                    // while also replacing the queue with
                    // the tail of our whole value

                    write_byte(
                        writer.by_ref(),
                        mem::replace(
                            queue_value,
                            U::to_u8(value.shr_default(available_bits + bytes * 8)),
                        ) | U::to_u8(
                            (value << mem::replace(queue_bits, excess)) & U::from_u8(u8::ALL),
                        ),
                    )?;

                    write_bytes(writer, bytes as usize, value >> available_bits)
                }
            }
        }
    }

    fn write_signed_bits_checked<const MAX: u32, W, S>(
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: &mut u32,
        value: CheckedSigned<MAX, S>,
    ) -> io::Result<()>
    where
        W: io::Write,
        S: SignedInteger,
    {
        // little-endian
        let (
            SignedBitCount {
                bits: BitCount { bits },
                unsigned,
            },
            value,
        ) = value.into_count_value();

        Self::write_bits_checked(
            writer.by_ref(),
            queue_value,
            queue_bits,
            Checked {
                value: if value.is_negative() {
                    value.as_negative(bits)
                } else {
                    value.as_non_negative()
                },
                count: unsigned,
            },
        )?;
        match Self::push_bit_flush(queue_value, queue_bits, value.is_negative()) {
            Some(b) => write_byte(writer, b),
            None => Ok(()),
        }
    }

    #[inline]
    fn pop_bit_refill<R>(
        reader: &mut R,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<bool>
    where
        R: io::Read,
    {
        Ok(if *queue_bits == 0 {
            let value = read_byte(reader)?;
            let lsb = value & u8::LSB_BIT;
            *queue_value = value >> 1;
            *queue_bits = u8::BITS_SIZE - 1;
            lsb
        } else {
            let lsb = *queue_value & u8::LSB_BIT;
            *queue_value >>= 1;
            *queue_bits -= 1;
            lsb
        } != 0)
    }

    #[inline]
    fn pop_unary<const STOP_BIT: u8, R>(
        reader: &mut R,
        queue_value: &mut u8,
        queue_bits: &mut u32,
    ) -> io::Result<u32>
    where
        R: io::Read,
    {
        const {
            assert!(matches!(STOP_BIT, 0 | 1), "stop bit must be 0 or 1");
        }

        match STOP_BIT {
            0 => find_unary(
                reader,
                queue_value,
                queue_bits,
                |v| v.trailing_ones(),
                |q| *q,
                |v, b| v.checked_shr(b),
            ),
            1 => find_unary(
                reader,
                queue_value,
                queue_bits,
                |v| v.trailing_zeros(),
                |_| u8::BITS_SIZE,
                |v, b| v.checked_shr(b),
            ),
            _ => unreachable!(),
        }
    }

    #[inline]
    fn read_signed_counted<const MAX: u32, R, S>(
        r: &mut R,
        SignedBitCount {
            bits: BitCount { bits },
            unsigned,
        }: SignedBitCount<MAX>,
    ) -> io::Result<S>
    where
        R: BitRead,
        S: SignedInteger,
    {
        if MAX <= S::BITS_SIZE || bits <= S::BITS_SIZE {
            let unsigned = r.read_unsigned_counted::<MAX, S::Unsigned>(unsigned)?;
            let is_negative = r.read_bit()?;
            Ok(if is_negative {
                unsigned.as_negative(bits)
            } else {
                unsigned.as_non_negative()
            })
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "excessive bits for type read",
            ))
        }
    }

    fn read_bytes<const CHUNK_SIZE: usize, R>(
        reader: &mut R,
        queue_value: &mut u8,
        queue_bits: u32,
        buf: &mut [u8],
    ) -> io::Result<()>
    where
        R: io::Read,
    {
        if queue_bits == 0 {
            reader.read_exact(buf)
        } else {
            let mut input_chunk: [u8; CHUNK_SIZE] = [0; CHUNK_SIZE];

            for output_chunk in buf.chunks_mut(CHUNK_SIZE) {
                let input_chunk = &mut input_chunk[0..output_chunk.len()];
                reader.read_exact(input_chunk)?;

                output_chunk
                    .iter_mut()
                    .zip(input_chunk.iter())
                    .for_each(|(o, i)| {
                        *o = i << queue_bits;
                    });

                output_chunk[1..]
                    .iter_mut()
                    .zip(input_chunk.iter())
                    .for_each(|(o, i)| {
                        *o |= i >> (u8::BITS_SIZE - queue_bits);
                    });

                // `input_chunk` is `&input_chunk[0..output_chunk.len()]` and
                // `chunks_mut` never yields an empty slice, so `last()` is always `Some`.
                let last_byte = *input_chunk
                    .last()
                    .unwrap_or_else(|| unreachable!("chunks_mut never yields empty slices"));
                output_chunk[0] |=
                    mem::replace(queue_value, last_byte >> (u8::BITS_SIZE - queue_bits));
            }

            Ok(())
        }
    }

    fn write_bytes<const CHUNK_SIZE: usize, W>(
        writer: &mut W,
        queue_value: &mut u8,
        queue_bits: u32,
        buf: &[u8],
    ) -> io::Result<()>
    where
        W: io::Write,
    {
        if queue_bits == 0 {
            writer.write_all(buf)
        } else {
            let mut output_chunk: [u8; CHUNK_SIZE] = [0; CHUNK_SIZE];

            for input_chunk in buf.chunks(CHUNK_SIZE) {
                let output_chunk = &mut output_chunk[0..input_chunk.len()];

                output_chunk
                    .iter_mut()
                    .zip(input_chunk.iter())
                    .for_each(|(o, i)| {
                        *o = i << queue_bits;
                    });

                output_chunk[1..]
                    .iter_mut()
                    .zip(input_chunk.iter())
                    .for_each(|(o, i)| {
                        *o |= i >> (u8::BITS_SIZE - queue_bits);
                    });

                // `input_chunk` is `buf.chunks(CHUNK_SIZE)[i]`; `chunks` never
                // yields an empty slice, so `last()` is always `Some`.
                let last_byte = *input_chunk
                    .last()
                    .unwrap_or_else(|| unreachable!("chunks never yields empty slices"));
                output_chunk[0] |=
                    mem::replace(queue_value, last_byte >> (u8::BITS_SIZE - queue_bits));

                writer.write_all(output_chunk)?;
            }

            Ok(())
        }
    }

    #[inline(always)]
    fn bytes_to_primitive<P: Primitive>(buf: P::Bytes) -> P {
        P::from_le_bytes(buf)
    }

    #[inline(always)]
    fn primitive_to_bytes<P: Primitive>(p: P) -> P::Bytes {
        p.to_le_bytes()
    }

    #[inline]
    fn read_primitive<R, V>(r: &mut R) -> io::Result<V>
    where
        R: BitRead,
        V: Primitive,
    {
        let mut buffer = V::buffer();
        r.read_bytes(buffer.as_mut())?;
        Ok(V::from_le_bytes(buffer))
    }

    #[inline]
    fn write_primitive<W, V>(w: &mut W, value: V) -> io::Result<()>
    where
        W: BitWrite,
        V: Primitive,
    {
        w.write_bytes(value.to_le_bytes().as_ref())
    }
}

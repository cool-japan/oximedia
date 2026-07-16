# oximedia-bitstream

> Bit-level I/O for OxiMedia — a `std`-only fork of bitstream-io 4.9.0.

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) multimedia framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- Read and write individual bits and multi-bit integers from any `std::io::Read` / `std::io::Write`
- Big-endian and little-endian streams, chosen at compile time as zero-cost type parameters
- Constant-bit-count reads/writes validated at compile time (Rust 1.79+)
- Variable-bit-count reads/writes via `read_var` / `write_var`
- Huffman coding traits: [`FromBits`] and [`ToBits`] for encoding/decoding symbols
- Struct-level serialisation via [`FromBitStream`] / [`ToBitStream`] derive targets
- `BitRecorder` (feature `alloc`) for recording a write pass and replaying it
- `BitsWritten` counter for measuring output size without a backing writer
- No unsafe code (`#![forbid(unsafe_code)]`)

## Usage

```toml
[dependencies]
oximedia-bitstream = { version = "0.2.0" }
```

### Reading bits

```rust
use std::io::Cursor;
use oximedia_bitstream::{BigEndian, BitReader, BitRead};

let data = [0b1011_0100u8, 0b1100_1010u8];
let mut r = BitReader::endian(Cursor::new(&data), BigEndian);

// Compile-time validated (requires Rust 1.79+)
let high: u8 = r.read::<4, _>().unwrap();
assert_eq!(high, 0b1011);

// Runtime variable width
let low: u8 = r.read_var(4).unwrap();
assert_eq!(low, 0b0100);
```

### Writing bits

```rust
use oximedia_bitstream::{BigEndian, BitWriter, BitWrite};

let mut output = Vec::new();
let mut w = BitWriter::endian(&mut output, BigEndian);

w.write::<4, _>(0b1011u8).unwrap();
w.write::<4, _>(0b0100u8).unwrap();
w.byte_align().unwrap();

assert_eq!(output, [0b1011_0100]);
```

### Little-endian streams

```rust
use std::io::Cursor;
use oximedia_bitstream::{LittleEndian, BitReader, BitRead};

let data = [0b1011_0100u8];
let mut r = BitReader::endian(Cursor::new(&data), LittleEndian);

// Bits are read LSB first in little-endian mode
let low: u8 = r.read::<4, _>().unwrap();
assert_eq!(low, 0b0100);
```

## API Overview

| Type | Role |
|------|------|
| `BitReader<R, E>` | Bit-level reader wrapping any `io::Read` |
| `BitWriter<W, E>` | Bit-level writer wrapping any `io::Write` |
| `ByteReader<R, E>` | Whole-byte reader wrapping any `io::Read` |
| `ByteWriter<W, E>` | Whole-byte writer wrapping any `io::Write` |
| `BitRecorder<N, E>` | Records bits for replay (requires `alloc`) |
| `BitsWritten` | Counts bits without backing storage |
| `BigEndian` / `LittleEndian` | Zero-sized endianness type parameters |

## Feature Flags

| Flag | Default | Effect |
|------|---------|--------|
| `std` | yes | Enables the `alloc` feature |
| `alloc` | via `std` | Enables `BitRecorder` |

## Upstream Attribution

Derived from [`bitstream-io`](https://crates.io/crates/bitstream-io) 4.9.0
by Brian Langenberger (Apache-2.0 / MIT).
The OxiMedia fork removes the `core2` / `no_std` shim and adopts workspace
conventions (version, authors, lint configuration).

## Status

Alpha — API may change between minor versions.

## License

Apache-2.0 — Copyright 2017 Brian Langenberger; 2024-2026 COOLJAPAN OU (Team Kitasan)

This crate derives from [`bitstream-io`](https://crates.io/crates/bitstream-io) 4.9.0,
originally dual-licensed "MIT OR Apache-2.0" by its author. COOLJAPAN OU redistributes
this derivative work under the Apache-2.0 option, as permitted by that upstream license
grant (see "Upstream Attribution" above).

//! VP9 boolean (range) **encoder** — exact port of libvpx
//! `vpx_dsp/bitwriter.{h,c}` (v1.15.2), test scaffolding only.
//!
//! # Why this exists
//!
//! [`super::hdr::parse_compressed_header`] transcribes a long, strictly
//! ordered sequence of probability updates out of libvpx
//! `vp9/decoder/vp9_decodeframe.c:2866-2914`. Real encoder output exercises
//! that order only through fixtures that must also be decodable end to end,
//! and this crate has **no trustworthy VP9 encoder** to synthesise targeted
//! cases with: `vp9/tile_encoder.rs` emits placeholder bytes, not a valid
//! bitstream. So the only way to unit-test the header parser against
//! hand-chosen probability updates — every field, one field at a time,
//! truncated streams, `allow_hp` on and off — is to encode those cases with
//! a real bool encoder.
//!
//! # Why it is trustworthy
//!
//! [`super::booldec::BoolReader`] is an independently validated oracle: the
//! four `keyframe_*_bit_exact_vs_libvpx` tests in [`super::tests`] decode
//! real libvpx-produced bitstreams through it bit-exactly. Round-tripping
//! this writer against that reader (see [`tests`]) therefore pins the writer
//! to libvpx's arithmetic coder, not merely to itself.
//!
//! This is `#[cfg(test)]`-only and never compiled into the shipped decoder.

#![allow(dead_code)]

/// libvpx `vpx_writer` (`vpx_dsp/bitwriter.h:28-40`), with the fixed output
/// buffer replaced by a growing `Vec` (so `br->error` — the "would write
/// past `size`" flag — can never be set and is not modelled) and with
/// per-probability write counters added for the header-symbol-count tests.
pub(super) struct BoolWriter {
    /// `br->lowvalue`.
    lowvalue: u32,
    /// `br->range`.
    range: u32,
    /// `br->count`.
    count: i32,
    /// `br->buffer`; `br->pos` is `buffer.len()`.
    buffer: Vec<u8>,
    /// Number of [`BoolWriter::write_bool`] calls per probability value.
    prob_writes: [usize; 256],
    /// Number of times the carry-propagation branch
    /// (`bitwriter.h:88-98`) ran.
    carry_events: usize,
    /// Total number of `0xff` bytes rewritten to `0` by that branch —
    /// greater than [`BoolWriter::carry_events`] exactly when a carry
    /// propagated across more than one byte.
    carry_bytes: usize,
}

impl BoolWriter {
    /// `vpx_start_encode` (`vpx_dsp/bitwriter.c:20-31`), including the
    /// trailing `vpx_write_bit(br, 0)` that emits VP9's partition marker
    /// bit — the one [`super::booldec::BoolReader::new`] consumes.
    pub(super) fn new() -> Self {
        let mut w = Self {
            lowvalue: 0,
            range: 255,
            count: -24,
            buffer: Vec::new(),
            prob_writes: [0; 256],
            carry_events: 0,
            carry_bytes: 0,
        };
        w.write_bit(false);
        w
    }

    /// `vpx_write` (`vpx_dsp/bitwriter.h:46-116`).
    ///
    /// The C code is `unsigned int` arithmetic throughout, so the additions
    /// and shifts here are wrapping/`u32` on purpose. Every shift amount is
    /// bounded well below 32: `offset` is `-old_count` with `old_count` in
    /// `-8..=-1` (the block is entered only when `count >= 0`, and `count`
    /// leaves it at `count - 8`), so `offset` is `1..=8`, and the two
    /// normalisation shifts are `vpx_norm[range] <= 7`.
    pub(super) fn write_bool(&mut self, prob: u8, bit: bool) {
        self.prob_writes[prob as usize] += 1;

        let mut count = self.count;
        let mut lowvalue = self.lowvalue;

        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        let mut range = split;
        if bit {
            lowvalue = lowvalue.wrapping_add(split);
            range = self.range - split;
        }

        // `vpx_norm[range]`: leading zeros of the low byte (range is 1..=255).
        let mut shift = (range as u8).leading_zeros() as i32;
        range <<= shift;
        count += shift;

        if count >= 0 {
            let offset = shift - count;

            if (lowvalue << (offset - 1)) & 0x8000_0000 != 0 {
                // Carry: propagate back through any 0xff run.
                self.carry_events += 1;
                let mut x = self.buffer.len().wrapping_sub(1);
                while let Some(byte) = self.buffer.get_mut(x) {
                    if *byte != 0xff {
                        break;
                    }
                    *byte = 0;
                    self.carry_bytes += 1;
                    x = x.wrapping_sub(1);
                }
                if let Some(byte) = self.buffer.get_mut(x) {
                    *byte += 1;
                } else {
                    // libvpx's "TODO(wtc): How to prove x >= 0?" case. A
                    // carry out of byte 0 cannot happen for a stream that
                    // starts with the zero marker bit; make it loud rather
                    // than silently mis-encoding.
                    panic!("VP9 bool encoder: carry out of the first byte");
                }
            }

            self.buffer.push(((lowvalue >> (24 - offset)) & 0xff) as u8);

            lowvalue <<= offset;
            shift = count;
            lowvalue &= 0xff_ffff;
            count -= 8;
        }

        lowvalue <<= shift;
        self.count = count;
        self.lowvalue = lowvalue;
        self.range = range;
    }

    /// `vpx_write_bit` (`vpx_dsp/bitwriter.h:118-120`): probability 128.
    pub(super) fn write_bit(&mut self, bit: bool) {
        self.write_bool(128, bit);
    }

    /// `vpx_write_literal` (`vpx_dsp/bitwriter.h:122-126`): `bits` bits of
    /// `value`, MSB first.
    pub(super) fn write_literal(&mut self, value: u32, bits: u32) {
        for bit in (0..bits).rev() {
            self.write_bit((value >> bit) & 1 != 0);
        }
    }

    /// Encodes `token` with a libvpx `vpx_tree_index` tree, the inverse of
    /// [`super::booldec::BoolReader::read_tree`].
    ///
    /// libvpx's encoder reaches the same bits through a precomputed
    /// `vp9_token` table (`vp9_write_token` / `vpx_write_tree`); walking the
    /// tree for the leaf `-token` produces the identical bit string without
    /// needing every token table transcribed here.
    ///
    /// # Panics
    ///
    /// Panics when `token` is not a leaf of `tree` (a test-authoring bug).
    pub(super) fn write_tree(&mut self, tree: &[i8], probs: &[u8], token: u8) {
        let mut path = Vec::new();
        assert!(
            find_tree_path(tree, 0, token, &mut path),
            "token {token} is not a leaf of the tree"
        );
        for (node, bit) in path {
            self.write_bool(probs[node >> 1], bit);
        }
    }

    /// `vpx_stop_encode` (`vpx_dsp/bitwriter.c:33-55`): 32 flushing zero
    /// bits, then libvpx's superframe-index disambiguation byte.
    ///
    /// That last byte exists so a partition can never end with a byte that
    /// looks like a superframe index marker (`(b & 0xe0) == 0xc0`); it is
    /// pure output-side hygiene and does not affect decoding, but it is
    /// reproduced here so a buffer produced by this writer is
    /// byte-identical to what libvpx would emit for the same symbols.
    pub(super) fn finish(mut self) -> Vec<u8> {
        for _ in 0..32 {
            self.write_bit(false);
        }
        if self.buffer.last().is_some_and(|b| b & 0xe0 == 0xc0) {
            self.buffer.push(0);
        }
        self.buffer
    }

    /// Number of `write_bool` calls made at probability `prob`.
    pub(super) fn writes_at(&self, prob: u8) -> usize {
        self.prob_writes[prob as usize]
    }

    /// Total number of `write_bool` calls, at every probability.
    pub(super) fn total_writes(&self) -> usize {
        self.prob_writes.iter().sum()
    }

    /// How many times the carry-propagation branch ran.
    pub(super) fn carry_events(&self) -> usize {
        self.carry_events
    }

    /// How many `0xff` bytes the carry branch rewrote in total.
    pub(super) fn carry_bytes(&self) -> usize {
        self.carry_bytes
    }
}

/// Depth-first search for the bit path from `node` to the leaf `-token`.
///
/// Mirrors [`super::booldec::BoolReader::read_tree`]'s traversal rule:
/// a child entry `<= 0` is the leaf `-child`, anything positive is the
/// index of the next internal node.
fn find_tree_path(tree: &[i8], node: usize, token: u8, path: &mut Vec<(usize, bool)>) -> bool {
    for bit in [false, true] {
        let child = tree[node + usize::from(bit)];
        path.push((node, bit));
        if child <= 0 {
            if (-i16::from(child)) as u8 == token {
                return true;
            }
        } else if find_tree_path(tree, child as usize, token, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// Deterministic xorshift64* PRNG.
///
/// Deliberately hand-rolled rather than pulled in as a dev-dependency: the
/// workspace pins every dependency centrally, a test-only RNG does not earn
/// a workspace entry, and a fixed seed makes every failure reproducible.
pub(super) struct Rng(u64);

impl Rng {
    /// Seeds the generator (any non-zero seed).
    pub(super) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// Next raw 64-bit output.
    pub(super) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..n` (`n > 0`).
    pub(super) fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() >> 32) as u32 % n
    }

    /// A VP9 probability, i.e. `1..=255` (probability 0 is not representable
    /// — `vpx_write`'s `split` would be 1 for both branches).
    pub(super) fn prob(&mut self) -> u8 {
        (self.below(255) + 1) as u8
    }

    /// A coin flip.
    pub(super) fn bit(&mut self) -> bool {
        self.next_u64() & (1u64 << 33) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::super::booldec::BoolReader;
    use super::super::tables::PARTITION_TREE;
    use super::super::tables_inter::{MV_CLASS_TREE, MV_JOINT_TREE};
    use super::*;

    /// The writer's first act must be the VP9 partition marker bit, so
    /// `BoolReader::new` accepts every buffer this writer produces.
    #[test]
    fn writer_emits_the_marker_bit_reader_requires() {
        let buf = BoolWriter::new().finish();
        assert!(
            BoolReader::new(&buf).is_some(),
            "marker bit must decode to 0"
        );
    }

    /// Random bool sequences at random probabilities must survive the round
    /// trip through the (independently validated) reader.
    #[test]
    fn random_bool_sequences_round_trip() {
        let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
        let mut total_symbols = 0usize;
        for seq in 0..300 {
            let len = 1 + rng.below(400) as usize;
            let mut plan = Vec::with_capacity(len);
            let mut w = BoolWriter::new();
            for _ in 0..len {
                let p = rng.prob();
                let b = rng.bit();
                plan.push((p, b));
                w.write_bool(p, b);
            }
            let buf = w.finish();
            let mut r = BoolReader::new(&buf).expect("marker bit");
            for (i, &(p, b)) in plan.iter().enumerate() {
                assert_eq!(r.read_bool(p), b, "seq {seq} symbol {i} (prob {p})");
            }
            assert!(!r.has_error(), "seq {seq}: reader overran its own encode");
            total_symbols += len;
        }
        assert!(total_symbols > 30_000, "coverage: {total_symbols} symbols");
    }

    /// Extreme probabilities, which produce long `0xff` runs in the output.
    #[test]
    fn extreme_probabilities_round_trip() {
        let mut rng = Rng::new(0x0BAD_C0DE_DEAD_BEEF);
        let mut saw_ff_byte = false;
        for seq in 0..200 {
            let len = 200 + rng.below(200) as usize;
            let mut plan = Vec::with_capacity(len);
            let mut w = BoolWriter::new();
            for _ in 0..len {
                let p = [1u8, 2, 8, 247, 254, 255][rng.below(6) as usize];
                let b = rng.below(8) != 0; // heavily biased -> long runs
                plan.push((p, b));
                w.write_bool(p, b);
            }
            let buf = w.finish();
            saw_ff_byte |= buf.contains(&0xff);
            let mut r = BoolReader::new(&buf).expect("marker bit");
            for (i, &(p, b)) in plan.iter().enumerate() {
                assert_eq!(r.read_bool(p), b, "seq {seq} symbol {i} (prob {p})");
            }
            assert!(!r.has_error(), "seq {seq}");
        }
        assert!(saw_ff_byte, "no 0xff byte ever reached the buffer");
    }

    /// Decodes the compact hex form the golden vectors are stored in.
    fn unhex(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        assert!(digits.len() % 2 == 0, "odd hex length");
        digits
            .chunks(2)
            .map(|pair| {
                let hi = char::from(pair[0]).to_digit(16).expect("hex digit");
                let lo = char::from(pair[1]).to_digit(16).expect("hex digit");
                (hi * 16 + lo) as u8
            })
            .collect()
    }

    /// The (prob, bit) symbol at index `i` of golden sequence `which`.
    ///
    /// Defined by closed-form arithmetic so the C oracle and this test
    /// encode provably identical symbol streams with no shared RNG.
    /// Sequence 3 is the exception: it is xorshift-driven from seed 37801,
    /// found by a brute-force search over the C reference for a stream that
    /// exercises the carry branch (see the test's doc comment).
    fn golden_symbol(which: usize, i: usize, rng: &mut Rng) -> (u8, bool) {
        const EXTREME: [u8; 6] = [1, 2, 8, 247, 254, 255];
        match which {
            0 => ((1 + (i * 37) % 255) as u8, ((i * i + 3 * i) % 7) < 3),
            1 => (EXTREME[i % 6], i % 11 != 0),
            2 => (if i % 23 == 22 { 3 } else { 254 }, i % 23 != 22),
            _ => {
                let r = rng.next_u64();
                ((1 + ((r >> 32) % 255)) as u8, r & 1 != 0)
            }
        }
    }

    /// **External oracle.** These byte strings were produced by libvpx's own
    /// `vpx_write` / `vpx_start_encode` / `vpx_stop_encode`
    /// (`vpx_dsp/bitwriter.{h,c}` v1.15.2, compiled verbatim with the
    /// `vpx_norm` table from `vpx_dsp/prob.c`) fed the symbol sequences
    /// [`golden_symbol`] defines. Matching them byte for byte pins this
    /// writer to the reference implementation, not merely to this crate's
    /// own decoder — including the flush, the `0xc0` superframe-index
    /// disambiguation byte in `finish`, and (sequence 3) the carry branch.
    ///
    /// To regenerate: compile `vpx_write` / `vpx_start_encode` /
    /// `vpx_stop_encode` from libvpx v1.15.2 `vpx_dsp/bitwriter.{h,c}`
    /// together with the 256-entry `vpx_norm` table from
    /// `vpx_dsp/prob.c`, drive it with the same [`golden_symbol`] formulas
    /// (the xorshift64* of [`Rng`] seeded 37801 for sequence 3), and hex-dump
    /// `br->buffer[0..br->pos]` after `vpx_stop_encode`. The sequences are
    /// deliberately short so the expected bytes stay legible.
    #[test]
    fn golden_streams_match_libvpx_byte_for_byte() {
        const GOLDEN: [(usize, usize, &str); 4] = [
            // mixed: 200 symbols, 36 bytes, libvpx carry_events = 0
            (
                0,
                200,
                "027656617affa49bad4db1e27223de771c009ee5043018f0e18aa4a349e09347\
                 ac239500",
            ),
            // extreme: 200 symbols, 78 bytes, libvpx carry_events = 0
            (
                1,
                200,
                "00ffffffffefffffbfffffb7fffff85ffffffffc0fffffffff80ffffffffefff\
                 ffbfffffb7fffff85ffffffffc0fffffffff80ffffffffefffffbfffffb7ffff\
                 f85ffffffffc0fffffffff800200",
            ),
            // runs: 200 symbols, 176 bytes, long 0xff runs, carry_events = 0
            (
                2,
                200,
                "7fffffffffffffffffffffffffffffffffffffe07fffffffffffffffffffffff\
                 ffffffffffffffe07fffffffffffffffffffffffffffffffffffffe07fffffff\
                 ffffffffffffffffffffffffffffffe07fffffffffffffffffffffffffffffff\
                 ffffffe07fffffffffffffffffffffffffffffffffffffe07fffffffffffffff\
                 ffffffffffffffffffffffe07fffffffffffffffffffffffffffffffffffffe0\
                 7fffffffffffffffffffffffffff8000",
            ),
            // carry: 300 symbols from seed 37801, 57 bytes, carry_events = 1
            (
                3,
                300,
                "09814eec00000a20400f3000e1f424453ac0be29b12921b09fb2b349f81ffedd\
                 bb309871200c620760c2d30ab53363818b7e1c55bc2210a800",
            ),
        ];

        let mut total_carries = 0usize;
        for (which, len, expected_hex) in GOLDEN {
            let mut rng = Rng::new(37801);
            let mut plan = Vec::with_capacity(len);
            let mut w = BoolWriter::new();
            for i in 0..len {
                let (p, b) = golden_symbol(which, i, &mut rng);
                plan.push((p, b));
                w.write_bool(p, b);
            }
            total_carries += w.carry_events();
            let buf = w.finish();
            let expected = unhex(expected_hex);
            assert_eq!(
                buf.len(),
                expected.len(),
                "sequence {which}: length differs from libvpx"
            );
            assert_eq!(buf, expected, "sequence {which}: bytes differ from libvpx");

            // ...and the reader must still recover the symbols from it.
            let mut r = BoolReader::new(&buf).expect("marker bit");
            for (i, &(p, b)) in plan.iter().enumerate() {
                assert_eq!(r.read_bool(p), b, "sequence {which} symbol {i}");
            }
            assert!(!r.has_error(), "sequence {which}");
        }
        assert_eq!(
            total_carries, 1,
            "sequence 3 is the only golden that carries, exactly once — \
             libvpx reports the same"
        );
    }

    /// White-box test of the carry-propagation loop
    /// (`vpx_dsp/bitwriter.h:88-98`).
    ///
    /// The branch is genuinely almost unreachable from ordinary symbol
    /// streams — a brute-force search over the C reference found roughly one
    /// carry per nine million random symbols, and never one that propagated
    /// across more than a single byte — so the multi-byte case is set up
    /// directly instead of hunted for. The expected result was produced by
    /// running libvpx's own `vpx_write` on exactly this state: buffer
    /// `37 ff ff`, `lowvalue = 0x80000000`, `range = 255`, `count = -1`,
    /// then `vpx_write(w, 0, 1)`, which reports `carry_events = 1`,
    /// `carry_bytes = 2`, `pos = 4`, `range = 128`, `count = -2`,
    /// `lowvalue = 0`, buffer `38 00 00 00`.
    #[test]
    fn carry_propagates_across_an_0xff_run_like_libvpx() {
        let mut w = BoolWriter {
            lowvalue: 0x8000_0000,
            range: 255,
            count: -1,
            buffer: vec![0x37, 0xff, 0xff],
            prob_writes: [0; 256],
            carry_events: 0,
            carry_bytes: 0,
        };
        w.write_bool(1, false);
        assert_eq!(w.buffer, vec![0x38, 0x00, 0x00, 0x00], "carried buffer");
        assert_eq!(w.carry_events, 1);
        assert_eq!(w.carry_bytes, 2, "the 0xff run must be walked back");
        assert_eq!(w.range, 128);
        assert_eq!(w.count, -2);
        assert_eq!(w.lowvalue, 0);
    }

    /// Literals of every width 1..=24 round-trip MSB-first.
    #[test]
    fn literals_round_trip_at_every_width() {
        let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
        for bits in 1..=24u32 {
            let mask = (1u32 << bits) - 1;
            let mut plan = Vec::new();
            let mut w = BoolWriter::new();
            for _ in 0..40 {
                let v = (rng.next_u64() as u32) & mask;
                plan.push(v);
                w.write_literal(v, bits);
            }
            let buf = w.finish();
            let mut r = BoolReader::new(&buf).expect("marker bit");
            for (i, &v) in plan.iter().enumerate() {
                assert_eq!(r.read_literal(bits), v, "{bits}-bit literal {i}");
            }
            assert!(!r.has_error());
        }
    }

    /// Tree-coded tokens round-trip against `BoolReader::read_tree` for
    /// three real VP9 trees of different shapes.
    #[test]
    fn trees_round_trip() {
        let mut rng = Rng::new(0xFEED_FACE_CAFE_B0BA);
        let cases: [(&[i8], usize); 3] = [
            (&PARTITION_TREE, 4),
            (&MV_JOINT_TREE, 4),
            (&MV_CLASS_TREE, 11),
        ];
        for (tree, tokens) in cases {
            let probs: Vec<u8> = (0..tree.len() / 2).map(|_| rng.prob()).collect();
            let mut plan = Vec::new();
            let mut w = BoolWriter::new();
            for _ in 0..200 {
                let t = rng.below(tokens as u32) as u8;
                plan.push(t);
                w.write_tree(tree, &probs, t);
            }
            let buf = w.finish();
            let mut r = BoolReader::new(&buf).expect("marker bit");
            for (i, &t) in plan.iter().enumerate() {
                assert_eq!(r.read_tree(tree, &probs), t, "token {i}");
            }
            assert!(!r.has_error());
        }
    }

    /// Mixed symbol kinds interleaved, which is what a real header is.
    #[test]
    fn mixed_symbol_kinds_round_trip() {
        #[derive(Clone, Copy)]
        enum Sym {
            Bool(u8, bool),
            Lit(u32, u32),
            Tree(u8),
        }
        let mut rng = Rng::new(0x5EED_0000_0000_0001);
        let probs = [200u8, 60, 130];
        let mut plan = Vec::new();
        let mut w = BoolWriter::new();
        for _ in 0..4000 {
            let sym = match rng.below(3) {
                0 => Sym::Bool(rng.prob(), rng.bit()),
                1 => Sym::Lit(rng.below(1 << 7), 7),
                _ => Sym::Tree(rng.below(4) as u8),
            };
            plan.push(sym);
            match sym {
                Sym::Bool(p, b) => w.write_bool(p, b),
                Sym::Lit(v, n) => w.write_literal(v, n),
                Sym::Tree(t) => w.write_tree(&PARTITION_TREE, &probs, t),
            }
        }
        let buf = w.finish();
        let mut r = BoolReader::new(&buf).expect("marker bit");
        for (i, sym) in plan.iter().enumerate() {
            match *sym {
                Sym::Bool(p, b) => assert_eq!(r.read_bool(p), b, "symbol {i}"),
                Sym::Lit(v, n) => assert_eq!(r.read_literal(n), v, "symbol {i}"),
                Sym::Tree(t) => assert_eq!(r.read_tree(&PARTITION_TREE, &probs), t, "symbol {i}"),
            }
        }
        assert!(!r.has_error());
    }

    /// The write counters the header tests assert against must count every
    /// bool, including the ones inside literals and trees and the marker.
    #[test]
    fn write_counters_are_exact() {
        let mut w = BoolWriter::new();
        assert_eq!(w.writes_at(128), 1, "the marker bit");
        assert_eq!(w.total_writes(), 1);
        w.write_literal(0b1011, 4);
        assert_eq!(w.writes_at(128), 5);
        w.write_bool(252, true);
        w.write_bool(252, false);
        assert_eq!(w.writes_at(252), 2);
        assert_eq!(w.total_writes(), 7);
        // PARTITION_TREE token 0 is one bool; token 3 is three.
        w.write_tree(&PARTITION_TREE, &[100, 100, 100], 0);
        assert_eq!(w.total_writes(), 8);
        w.write_tree(&PARTITION_TREE, &[100, 100, 100], 3);
        assert_eq!(w.total_writes(), 11);
    }

    /// The Rng is only test scaffolding, but a broken one would silently
    /// weaken every randomized test above.
    #[test]
    fn rng_is_deterministic_and_spread() {
        let a: Vec<u64> = (0..8)
            .scan(Rng::new(7), |r, _| Some(r.next_u64()))
            .collect();
        let b: Vec<u64> = (0..8)
            .scan(Rng::new(7), |r, _| Some(r.next_u64()))
            .collect();
        assert_eq!(a, b, "same seed must replay");
        let mut r = Rng::new(99);
        let mut seen = [0usize; 16];
        for _ in 0..16_000 {
            seen[r.below(16) as usize] += 1;
        }
        assert!(
            seen.iter().all(|&c| c > 700),
            "below() is badly skewed: {seen:?}"
        );
        let mut r = Rng::new(1234);
        let mut ones = 0;
        for _ in 0..10_000 {
            ones += usize::from(r.bit());
        }
        assert!(
            (4000..6000).contains(&ones),
            "bit() is skewed: {ones}/10000"
        );
        let mut r = Rng::new(5);
        assert!(
            (0..5000).all(|_| (1..=255).contains(&r.prob())),
            "prob() must stay in 1..=255"
        );
    }
}

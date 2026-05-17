# Apple ProRes 422 — Full Decoder Walk-Through

> **Audience.** A Rust engineer who is new to video codecs and wants to
> understand *exactly* what `crates/oximedia-codec/src/prores/` does,
> stage by stage, so they could fix bugs, extend it, or build a similar
> decoder for another codec.
>
> **Prerequisite.** Skim [`codec_internals.md`](codec_internals.md)
> first — that doc explains *why* every video codec has the
> "predict / transform / quantize / entropy code" pipeline shape.
> This doc explains *how* ProRes specifically does each step.
>
> **Companion code.** Every section below cross-references the exact
> file and function that implements what it describes.

## 1. What ProRes is and why we care

ProRes is an **intra-only block-based video codec** — every frame
decodes independently of every other frame, like a sequence of JPEGs.
It's used everywhere in professional post-production:

| Property | Value | Why it matters |
|---|---|---|
| Frames | Independently decodable (no reference frames) | Fast seeking and scrubbing in editors |
| Chroma | 4:2:2 (4 of 6 profiles) or 4:4:4 (2 profiles) | Higher chroma fidelity than 4:2:0 — important for colour grading |
| Bit depth | 10-bit | Headroom for HDR and grade-without-banding |
| Compression | ~6:1 to ~22:1 depending on profile | Manageable file size, decodable in real time |
| Bitstream | Specified in **SMPTE RDD 36-2015** | Public spec; we can implement from it |

The "5 sub-profiles" (apco / apcs / apcn / apch / ap4h / ap4x) share
the same bitstream syntax — they differ only in default quantization
matrices and target qscale ranges. So a single decoder handles all of
them.

## 2. The big picture

A `.mov` file containing ProRes looks like this once the MP4 demuxer
pulls one sample (= one frame) out:

```text
┌────────────────────────────────────┐
│ 4-byte size + 'icpf' tag           │ ← frame container
├────────────────────────────────────┤
│ frame header                       │ ← resolution, profile, quant matrices
├────────────────────────────────────┤
│ picture header                     │ ← slice count
├────────────────────────────────────┤
│ slice 0:  header + Y + Cb + Cr     │ ← one slice = one horizontal MB strip
│ slice 1:  header + Y + Cb + Cr     │
│ slice 2:  header + Y + Cb + Cr     │
│   …                                │
│ slice N:  header + Y + Cb + Cr     │
└────────────────────────────────────┘
```

A frame is split into **macroblocks** (MBs). Each MB covers a 16×16
patch of luma (Y) and an 8×16 patch of each chroma plane (Cb, Cr).
That's 4 luma 8×8 blocks + 2 Cb 8×8 blocks + 2 Cr 8×8 blocks per MB.

A **slice** is a horizontal strip of macroblocks (typically 8 MBs
wide, configurable per picture). For 1920×1080 video: 1920/16 = 120
MBs per row, 1080/16 = 68 rows, ~8160 MBs total, ~1020 slices.

Why slices? Two reasons:

1. **Parallelism.** Every slice decodes independently — a multi-core
   decoder can fan slices out across cores.
2. **Error resilience.** A bit-flip in one slice doesn't poison the
   whole frame; the rest still decode.

For each slice, we have to:

```text
compressed Y bytes  →  [DSP pipeline]  →  10-bit Y samples
compressed Cb bytes →  [DSP pipeline]  →  10-bit Cb samples
compressed Cr bytes →  [DSP pipeline]  →  10-bit Cr samples
```

The DSP pipeline is run once per 8×8 block within the slice. We'll
walk through that next.

## 3. Frame container — [`prores/frame.rs`](../crates/oximedia-codec/src/prores/frame.rs)

The outermost wrapper is dead-simple. 4 bytes of big-endian size, then
the four ASCII bytes `'icpf'`, then the payload.

```text
size = 50000 bytes (incl. these 4 bytes)
─────
[0x00, 0x00, 0xC3, 0x50,   ← size: 0x0000C350 = 50000
 'i',  'c',  'p',  'f',    ← magic tag
 <49992 bytes of frame payload>]
```

The point of the magic tag is so a demuxer can find frame boundaries
even if it doesn't understand the rest of the frame. Useful for
sample-table-driven seeking inside an MP4.

`FrameContainer::parse` checks the magic, reads the size, and returns
a slice of the payload bytes.

## 4. Frame header — [`prores/frame.rs::parse_frame_header`](../crates/oximedia-codec/src/prores/frame.rs)

The header is metadata about the frame. About 20 bytes (more if it
carries custom quantization matrices).

```text
offset 0..2    header_size              16-bit BE, total header length
offset 2       version                  always 0
offset 3..7    encoder_identifier       'apco' / 'apcs' / 'apcn' / 'apch' / 'ap4h' / 'ap4x'
offset 7..9    width                    16-bit BE
offset 9..11   height                   16-bit BE
offset 11      chroma_format            bits 7..6: 2 = 4:2:2, 3 = 4:4:4
               interlace_mode           bits 3..2: 0 = progressive, 1 = TFF, 2 = BFF
offset 12      aspect_ratio_code        bits 7..4
               frame_rate_code          bits 3..0
offset 13      color_primaries          ITU-T H.273 code (BT.709 = 1, BT.2020 = 9, P3 = 12)
offset 14      transfer_characteristic  H.273 code (sRGB = 13, PQ = 16, HLG = 18)
offset 15      matrix_coefficients      H.273 code
offset 16      source_pixel_format      bits 7..4
               alpha_channel_type       bits 3..0 (0 = none, 1 = 8-bit, 2 = 16-bit)
offset 17      reserved
offset 18      bit 7 = load_luma_quant
               bit 6 = load_chroma_quant
offset 19      reserved
offset 20…     [64 bytes luma_quant_matrix    if load_luma_quant   = 1]
               [64 bytes chroma_quant_matrix  if load_chroma_quant = 1]
```

**Note on custom quant matrices.** Most ProRes files don't carry custom
matrices — they rely on the spec defaults. When custom matrices are
signaled, the rule from RDD 36 is: if only `load_luma_quant` is set,
the chroma matrix also uses the luma one. Both flags set means both
matrices are explicitly given.

## 5. Quantization matrices — [`prores/quant.rs`](../crates/oximedia-codec/src/prores/quant.rs)

A **quantization matrix** is a 64-entry table that says, per spatial
frequency, how much the encoder divided each DCT coefficient by. The
decoder uses it to multiply back.

The default ProRes luma matrix:

```text
 4  4  5  5  6  7  8  9     ← row 0 = low vertical freq
 4  4  5  6  7  8  9 10
 5  5  6  7  8  9 10 12
 5  6  7  8  9 10 12 14
 6  7  8  9 10 12 14 17
 7  8  9 10 12 14 17 21
 8  9 10 12 14 17 21 26
 9 10 12 14 17 21 26 33     ← row 7 = high vertical freq
 ↑                    ↑
 col 0 = low h-freq   col 7 = high h-freq (highest-freq corner)
```

Notice the gradient: 4 at top-left (DC, "the average"), 33 at
bottom-right (the highest spatial frequency). Why this shape?

- Your eye is **very** sensitive to low-frequency luma — gentle
  brightness gradients, big edges. So quantize those lightly (small
  divisor → small loss).
- Your eye is **bad** at high-frequency luma — pixel-level texture and
  noise. Quantize aggressively (big divisor → lots of values round to
  zero → bits saved with no visible loss).

The chroma matrix is similar but more aggressive at high frequencies
(bottom-right is 56 vs luma's 33). Same reasoning: your eye is even
worse at high-frequency colour than at high-frequency brightness.

## 6. Picture header — [`prores/picture.rs::parse_picture_header`](../crates/oximedia-codec/src/prores/picture.rs)

After the frame header is one **picture** per field. Progressive
frames have 1 picture; interlaced (TFF/BFF) frames have 2.

Picture header (8 bytes):

```text
offset 0       picture_header_size      typically 8
offset 1..5    picture_size             32-bit BE: header + all slices
offset 5..7    slice_count              16-bit BE
offset 7       log2_slice_mb_width      4 high bits (default 3 → 8-MB-wide slices)
               reserved                 4 low bits
```

Then `slice_count` slices follow, back-to-back.

## 7. Slice header — [`prores/picture.rs::parse_slice_header`](../crates/oximedia-codec/src/prores/picture.rs)

Each slice starts with a short header — 6 bytes for 4:2:2 / 4:4:4
without alpha, 8 bytes with alpha:

```text
offset 0       slice_header_size        high nibble (= 6 or 8)
               reserved                 low nibble
offset 1       quant_scale              1..=224 — per-slice rate-control knob
offset 2..4    luma_data_size           16-bit BE — bytes of compressed Y
offset 4..6    cb_data_size             16-bit BE — bytes of compressed Cb
offset 6..8    cr_data_size             16-bit BE — bytes of compressed Cr
offset 8..10   alpha_data_size          16-bit BE — only if alpha is enabled
```

**The `quant_scale` field is how rate control works.** Flat regions
(sky, walls) get a high qscale → more compression, lower quality.
Detailed regions (faces, foliage) get a low qscale → less compression,
higher quality. The encoder picks per-slice; the decoder just obeys.

After the header come `luma_data_size + cb_data_size + cr_data_size`
bytes of compressed plane data, packed back-to-back.
`split_slice_planes` in [`decode.rs`](../crates/oximedia-codec/src/prores/decode.rs)
carves them into three `&[u8]` slices.

---

# The DSP pipeline

This is where the real codec work happens. For each 8×8 block of each
plane, we do this:

```text
compressed bytes
      │
      ▼  [entropy decode]
   64 quantized integers in scan order
      │
      ▼  [inverse zigzag]
   64 quantized integers in raster (8×8) order
      │
      ▼  [dequantize × matrix × qscale]
   64 dequantized DCT coefficients
      │
      ▼  [2-D 8×8 inverse DCT]
   64 signed spatial samples
      │
      ▼  [+512, clip to [0, 1023]]
   64 unsigned 10-bit Y/Cb/Cr samples
      │
      ▼  [blit to destination plane]
   pixels in your output buffer
```

Each stage solves a specific problem. Let's walk through them.

## 8. Bit reader — [`prores/bitreader.rs`](../crates/oximedia-codec/src/prores/bitreader.rs)

Codewords are variable-length and don't align to byte boundaries. The
bit reader lets us pull arbitrary numbers of bits from a packed
bytestream.

**Bit order is MSB-first**: bit 7 of byte 0 first, then bit 6, … then
bit 0, then bit 7 of byte 1, etc. This is the universal codec
convention.

Three operations matter:

| Method | Returns | Used for |
|---|---|---|
| `read_bits(n)` | next n bits as a u32 | Reading remainder of Golomb-Rice codes |
| `read_bit()` | next 1 bit | Sign bits, single-bit flags |
| `count_leading_ones()` | length of unary prefix | Quotient of Golomb-Rice codes |

The implementation tracks `(byte_pos, bit_pos)`. Reading a bit
returns `(buffer[byte_pos] >> (7 - bit_pos)) & 1`, then advances
`bit_pos`, rolling over to the next byte when it hits 8.

## 9. Golomb-Rice entropy coding — [`prores/entropy.rs`](../crates/oximedia-codec/src/prores/entropy.rs)

Here's the central question: **how do you store an integer in a
variable number of bits, where small integers use fewer bits?**

### 9.1 Unary coding (naive baseline)

Write `n` ones, then a 0. Decoding is "count ones, stop at the 0."

```text
value 0   → "0"           (1 bit)
value 1   → "10"          (2 bits)
value 2   → "110"         (3 bits)
value 5   → "111110"      (6 bits)
value 100 → "111…10"      (101 bits — really bad)
```

Great for small values, terrible for big ones.

### 9.2 Golomb-Rice — picking a width K

Golomb-Rice fixes the runaway by splitting each value into a quotient
and remainder:

```text
quotient  q = value >> K       (unary)
remainder r = value & ((1<<K) - 1)   (K bits, plain binary)

bitstream: q ones, then a 0, then r in K bits
```

For K = 2:

```text
value 0   → q=0, r=0   → "0" + "00"     = "000"        (3 bits)
value 1   → q=0, r=1   → "0" + "01"     = "001"        (3 bits)
value 2   → q=0, r=2   → "0" + "10"     = "010"        (3 bits)
value 3   → q=0, r=3   → "0" + "11"     = "011"        (3 bits)
value 4   → q=1, r=0   → "1" + "0" + "00" = "1000"     (4 bits)
value 5   → q=1, r=1   → "10" + "01"    = "1001"       (4 bits)
value 13  → q=3, r=1   → "1110" + "01"  = "111001"     (6 bits)
value 100 → q=25, r=0  → "111…11"(25)+"0"+"00" = 28 bits — still long
```

Better than unary for medium values. The catch: K has to match the
typical value range. Too small a K → unary blows up. Too big a K →
remainder bits get wasted on small values.

Decode is `decode_unsigned_codeword(reader, k)`:

```text
q = reader.count_leading_ones()
r = reader.read_bits(k)
value = (q << k) | r
```

For signed values (used for DC differentials and AC levels), there's a
trailing sign bit after the magnitude (only emitted when magnitude is
non-zero). Convention: `0` = positive, `1` = negative. Implemented as
`decode_signed_codeword`.

### 9.3 Adaptive K — the encoder's secret weapon

A single K can't be optimal across a whole stream — different
neighbourhoods have different typical value sizes. **ProRes makes K
adaptive**: the K used for the next codeword depends on the magnitude
of the previous one.

| Just decoded a big value? | Next codeword's K | Why |
|---|---|---|
| Magnitude 0 or 1 | K = 0 | Small values keep coming, unary is best |
| Magnitude ~10 | K = 3-5 | Medium values; 3-5 remainder bits is efficient |
| Magnitude ~100 | K = 7 | Big values; widest remainder for cheaper unary |

The adaptation table is just a lookup: `next_K = TABLE[clamp(prev_magnitude)]`.
ProRes saturates at K = 7. Three tables, one per coding context:

- `next_k_dc(prev_mag)` — for DC differentials
- `next_k_ac_level(prev_mag)` — for AC levels
- `next_k_ac_run(prev_run)` — for AC runs

## 10. Per-block decode — [`prores/entropy.rs::decode_block`](../crates/oximedia-codec/src/prores/entropy.rs)

Each 8×8 block has 64 coefficients. **One is DC** (the block's average
brightness — top-left in 2-D, scan index 0); the **other 63 are AC**
(higher spatial frequencies).

### 10.1 DC differential coding

DC values of neighbouring blocks are correlated — adjacent sky blocks
are all roughly the same brightness. So the encoder transmits not the
absolute DC but the **delta from the previous block's DC**:

```text
slice DC sequence:
  block 0: encoded as     |delta| + sign, where prev_dc = 0      → first DC absolute
  block 1: encoded as     |delta| + sign relative to block 0's DC
  block 2: encoded as     |delta| + sign relative to block 1's DC
   …
```

In code:

```rust
fn decode_block(reader, previous_dc) {
    let dc_delta = decode_signed_codeword(reader, next_k_dc(prev_mag));
    let new_dc = previous_dc + dc_delta;
    coeffs[0] = new_dc;
    …
}
```

The caller threads `previous_dc` from block to block within a slice
(reset to 0 at slice start).

### 10.2 AC run/level coding

Most AC coefficients are zero (the DCT concentrates energy at low
frequencies). Storing them as plain integers would waste enormous
amounts of bits on zeros. Instead ProRes encodes **run/level pairs**:

```text
AC scan order: 1, 2, 3, 4, 5, …, 63

Coefficients:  [0, 0, +5, 0, 0, 0, -2, 0, 0, 0, 0, 0, …rest all zero]

Encoded as:
  run = 2  (skip 2 zeros), level = +5
  run = 3  (skip 3 zeros), level = -2
  run = lots (drives position past 63 → end of block)
```

Each `run` is a Golomb-Rice unsigned codeword (with `next_k_ac_run`
adaptation). Each `level` is a Golomb-Rice unsigned codeword (with
`next_k_ac_level` adaptation) plus a sign bit. ProRes biases the
level magnitude by +1 — value 0 isn't transmitted explicitly because
zero AC values are encoded by extending the next run, not by writing
"level=0".

The decode loop terminates when the running position exceeds 63 —
there's no explicit end-of-block marker.

In code:

```rust
let mut pos = 1;
while pos < 64 {
    let run = decode_unsigned_codeword(reader, k_run);
    pos += run;
    if pos >= 64 { break; }
    let level_mag = decode_unsigned_codeword(reader, k_level) + 1;
    let sign = reader.read_bit();
    coeffs[pos] = if sign == 1 { -level_mag } else { level_mag };
    k_run = next_k_ac_run(run);
    k_level = next_k_ac_level(level_mag);
    pos += 1;
}
```

After this loop, `coeffs` holds 64 signed integers — the quantized
coefficients of this 8×8 block, but **in scan order**, not raster
order. Next stage fixes that.

## 11. Inverse zigzag scan — [`prores/zigzag.rs`](../crates/oximedia-codec/src/prores/zigzag.rs)

The 64 coefficients are in **scan order** — the encoder transmitted
them in a sequence designed to cluster non-zero values at the front.
The standard zigzag scan order looks like:

```text
position in 8×8 block  →  scan-order index

  0   1   5   6  14  15  27  28
  2   4   7  13  16  26  29  42
  3   8  12  17  25  30  41  43
  9  11  18  24  31  40  44  53
 10  19  23  32  39  45  52  54
 20  22  33  38  46  51  55  60
 21  34  37  47  50  56  59  61
 35  36  48  49  57  58  62  63
```

Read by following 0, 1, 2, 3, … in scan order — it traces a zigzag
pattern starting at the top-left, swinging through low frequencies,
and ending at the bottom-right (highest frequency).

To undo: `PROGRESSIVE_ZIGZAG[scan_idx] = raster_position`. Iterate the
64 scan-indexed coefficients and place each at its raster position.

ProRes also defines an **alternate (interlaced) zigzag** for
interlaced frames, weighted differently to exploit vertical correlation
between fields. `ALTERNATE_ZIGZAG` is in the same file.

## 12. Dequantization — [`prores/dequant.rs`](../crates/oximedia-codec/src/prores/dequant.rs)

Now we have 64 coefficients in 8×8 raster order. Each was encoded as
`quantized = round(coeff / (matrix[i] × qscale))`. To partially undo:

```rust
coeff[i] = quantized[i] × matrix[i] × qscale
```

That's the whole stage. Six lines of code.

**This is lossy.** We can't recover the encoder's rounding error.
Coefficients quantized to 0 stay 0 forever. The fact that the encoder
chose small `matrix[i]` for low frequencies (small rounding error) and
big `matrix[i]` for high frequencies (big rounding error, but
visually invisible) is what makes the loss imperceptible.

Numerical example with `matrix[0] = 4` and `qscale = 5`:

```text
Encoder side:
  original DC coefficient = 1000
  divided by 4 × 5 = 20
  rounded = 50 (which is what gets entropy-coded and written to disk)

Decoder side:
  entropy decodes "50"
  multiplies back: 50 × 4 × 5 = 1000
  ✓ recovered exactly (1000 was a multiple of 20)

If original was 1001 instead:
  encoder: round(1001 / 20) = 50
  decoder: 50 × 20 = 1000
  → lost 1 (the rounding error)
```

## 13. The 8×8 Inverse DCT — [`prores/idct.rs`](../crates/oximedia-codec/src/prores/idct.rs)

This is the heart of every block-based video codec. The DCT (Discrete
Cosine Transform) converts pixel values into frequency-domain
coefficients; the **IDCT** does the reverse.

### 13.1 Why DCT at all

Pixel values in a small block are highly correlated — sky pixels look
like other sky pixels, brick pixels like other brick pixels. The DCT
**decorrelates** them, packing most of a block's energy into a few
low-frequency coefficients and leaving most of the high-frequency ones
near zero.

That sparsity is what makes the entropy coding from §9 work magic. A
block of 64 spatial pixels might have all 64 values be non-zero (no
compression possible). After DCT, the same block might have 5 big
coefficients and 59 near-zero ones — almost all of which round to
exactly zero after quantization → a tiny run/level stream.

### 13.2 The 1-D formula

For an 8-point IDCT:

```text
x[n] = X[0]/√2 + Σ_{k=1..8} X[k] · cos((2n+1) · k · π / 16)        for n ∈ [0, 7]
```

Reading this:

- `X[k]` for `k = 0..7` are the 8 frequency-domain coefficients.
- `x[n]` for `n = 0..7` are the 8 spatial-domain samples we're
  reconstructing.
- `X[0]/√2` is the "DC term" — it contributes a constant offset to
  every sample.
- The sum over `k = 1..7` contributes the AC components, each weighted
  by a cosine at increasing frequency.

That's literally just a matrix multiplication: an 8×8 cosine matrix
times the 8-element coefficient vector.

### 13.3 Separable 2-D = 1-D × 2

The full 2-D 8×8 IDCT is mathematically:

```text
x[m,n] = Σ_{i,j} X[i,j] · cos((2m+1)·i·π/16) · cos((2n+1)·j·π/16) · α(i) · α(j)
```

Done naively that's 4096 operations per 8×8 block — way too slow.

The trick: the 2-D transform is **separable**. You can do it as eight
1-D IDCTs across rows, then eight 1-D IDCTs across columns of the
intermediate. Same final answer, but only 128 operations.

```text
       ┌─────────────────┐               ┌─────────────────┐
       │ 8 row IDCTs     │               │ 8 col IDCTs     │
   X → │ on coefficients │ → intermediate → on intermediate │ → x
       └─────────────────┘               └─────────────────┘
```

`idct_8x8` does exactly that: it loops over rows calling `idct_1d`,
stores the intermediate, then loops over columns calling `idct_1d`
again.

### 13.4 Integer arithmetic

Codecs need bit-exact results across implementations — float
arithmetic gives slightly different results on different CPUs, which
breaks reproducibility. So everything is integer.

The cosines are pre-scaled into **Q15 fixed-point** (i.e. multiplied by
`2^15 = 32768` and rounded):

```text
cos(0π/16) ≈ 1.0000 → 32768
cos(1π/16) ≈ 0.9808 → 32138
cos(2π/16) ≈ 0.9239 → 30274
cos(3π/16) ≈ 0.8315 → 27246
cos(4π/16) ≈ 0.7071 → 23170    ← cos(π/4) = 1/√2
cos(5π/16) ≈ 0.5556 → 18205
cos(6π/16) ≈ 0.3827 → 12540
cos(7π/16) ≈ 0.1951 →  6393
cos(8π/16) = 0      →     0
```

A multiplication of two Q15 values produces a Q30 value (`a·2^15 ·
b·2^15 = a·b·2^30`). We right-shift by 15 (with rounding) to get back
to "normal" integer scale.

### 13.5 Worked example: DC-only block

Suppose the dequantized coefficient block has just one non-zero value:
`X = [1000, 0, 0, 0, 0, 0, 0, 0]` (DC = 1000, all AC = 0).

Run the 1-D IDCT on each row of the 8×8 (only the top row matters; the
rest are 0). For each `n`:

```text
x[n] = X[0]/√2 + Σ_{k=1..7} 0 · cos(…)
     = X[0]/√2
     = 1000 · 0.7071
     ≈ 707
```

So the row becomes `[707, 707, 707, 707, 707, 707, 707, 707]` —
constant across the row.

In integer Q15 terms:
- `acc = 1000 × 23170 = 23,170,000`
- `out = (23,170,000 + 16,384) >> 15 ≈ 707`. ✓

Now run the 1-D IDCT down each column of the intermediate. Each column
is `[707, 0, 0, 0, 0, 0, 0, 0]` (because only the top row had any
data). Apply the same math:

```text
x'[m] = 707/√2 ≈ 500
```

So the full 2-D output is **a uniform 8×8 block of 500** — every
sample the same value, proportional to the original DC of 1000. That's
exactly what we expect from a DC-only block.

The test `idct_of_dc_only_is_constant_block` verifies this.

## 14. Finalize to 10-bit — [`prores/idct.rs::finalize_idct_output`](../crates/oximedia-codec/src/prores/idct.rs)

The IDCT output is **signed** — could be negative or positive. Real
pixels are unsigned 10-bit (0..1023). So:

1. **Center.** Add 512 (midgrey for 10-bit) to every sample.
2. **Clip.** Values below 0 → 0. Values above 1023 → 1023.

```rust
for (i, &v) in idct_block.iter().enumerate() {
    let centered = v.saturating_add(512);
    out[i] = centered.clamp(0, 1023) as u16;
}
```

Tests verify: zero block → 512 (midgrey); positive DC → above 512;
negative DC → below 512; huge magnitude → clips to 1023 or 0.

## 15. Block-to-plane blit — [`prores/decode.rs::blit_8x8_to_plane`](../crates/oximedia-codec/src/prores/decode.rs)

We finally have 64 pixel values for one 8×8 block. They need to go
into the destination plane at the right (row, column) offset.

For **luma** (4:2:2), each macroblock contributes 4 blocks in a 2×2
arrangement within a 16×16 tile:

```text
MB at column mb_x:

  ┌─────────┬─────────┐
  │ block 0 │ block 1 │   ← rows 0..8, cols mb_x×16+0..+8 and +8..+16
  │ (16×8)  │ (8×8)   │
  ├─────────┼─────────┤
  │ block 2 │ block 3 │   ← rows 8..16
  │ (8×8)   │ (8×8)   │
  └─────────┴─────────┘
```

For **chroma** (4:2:2 — half horizontal resolution), each MB is two
8×8 blocks stacked vertically in an 8×16 column:

```text
MB at column mb_x:

  ┌─────────┐
  │ block 0 │   ← rows 0..8, cols mb_x×8..mb_x×8+8
  │ (8×8)   │
  ├─────────┤
  │ block 1 │   ← rows 8..16
  │ (8×8)   │
  └─────────┘
```

`Plane::block_offset(mb_x, block_in_mb, stride)` returns the byte
offset; `blit_8x8_to_plane` copies the 8×8 of samples into the
destination at that offset, one row at a time (because the destination
stride is generally ≥ 8).

## 16. Putting it all together — [`prores/decode.rs::decode_slice_to_yuv422`](../crates/oximedia-codec/src/prores/decode.rs)

The slice decoder is just `decode_plane` called three times — once
each for Y, Cb, Cr. Each `decode_plane`:

```rust
let mut reader = BitReader::new(compressed_plane_bytes);
let mut running_dc = 0;

for mb_x in 0..slice_mb_width {
    for block_in_mb in 0..blocks_per_mb(plane) {
        let (scan_coeffs, new_dc) = decode_block(&mut reader, running_dc);
        running_dc = new_dc;

        let raster_coeffs = inverse_scan(&scan_coeffs, &PROGRESSIVE_ZIGZAG);
        let dequantized   = dequantize_block(&raster_coeffs, matrix, qscale);
        let spatial       = idct_8x8(&dequantized);
        let samples       = finalize_idct_output(&spatial);

        let offset = plane.block_offset(mb_x, block_in_mb, dst_stride);
        blit_8x8_to_plane(&samples, dst, dst_stride, offset);
    }
}
```

That's the whole pipeline. For a 1920×1080 P22 Standard slice (8 MBs
wide), this loop runs:

- 8 MBs × 4 luma blocks = 32 luma blocks
- 8 MBs × 2 Cb blocks = 16 Cb blocks
- 8 MBs × 2 Cr blocks = 16 Cr blocks

So ~64 8×8 blocks per slice × ~1020 slices = ~65k IDCT operations per
frame. At 30 fps, ~2M IDCTs per second. With careful integer math and
SIMD, modern CPUs handle this in real time on a single core.

## 17. What's tested vs what's not

| Stage | Status |
|---|---|
| Frame container / frame header / picture header / slice header parsing | Tested with hand-crafted byte sequences for every error path |
| Quantization matrices | Tested for spec invariants (permutation, DC quantizer = 4, diagonal monotonicity) |
| Bit reader | Tested for MSB-first order, byte-boundary handling, errors |
| Golomb-Rice codeword decoder | Tested with hand-traced bit patterns for K=0, K=2, K=3, positive and negative |
| K adaptation tables | Tested for saturation behavior |
| Inverse zigzag scan | Tested as permutation + round-trip identity |
| Dequantization | Tested with synthetic coefficients |
| 8×8 IDCT | Tested with DC-only-produces-uniform-block + zero-in-zero-out + clipping |
| Plane assembly | Tested with synthetic samples → known destination layout |
| **Real-stream bit-exact comparison** | **Not done.** No fixture corpus. |

The last row is the gap. Every algorithm matches the spec and the
reference open-source decoder, and every synthetic test passes. But
nobody has dropped a real Apple-encoded ProRes file in and verified
the decoded pixels match a reference decoder pixel-for-pixel.

If a real file fails to decode correctly, the likely culprits (in
descending probability):

1. **AC end-of-block detection.** Our loop ends when `pos >= 64`; ProRes
   may have specific signalling we're not catching.
2. **K-adaptation tables.** Mine match the open-source reference but
   ProRes profiles (Proxy vs HQ) may use subtly different tables.
3. **IDCT scaling constants.** Our Q15 formulation is morally
   equivalent to RDD 36's, but spec-bit-exact reconstruction may
   require the exact integer constants from RDD 36 §6.5.7.

That's what a Phase 3 with a fixture-driven conformance test would
catch and fix.

## 18. How to extend (Phase 3 ideas)

| Feature | Where to add it | Roughly |
|---|---|---|
| 4444 alpha-plane decode | `decode_slice_to_yuv422` becomes `decode_slice_to_yuv444a`; alpha is just another plane with the same pipeline | ~150 LOC |
| Alternate (interlaced) zigzag | `decode_plane` already takes the scan table by value — pass `ALTERNATE_ZIGZAG` when frame is interlaced | ~30 LOC |
| Multi-slice picture orchestration | New function that loops the slice list from the picture header and calls `decode_slice_to_yuv422` per slice with the right destination offset | ~80 LOC |
| `VideoDecoder` trait integration | Add `CodecId::ProRes422` to `oximedia-core`; new type `ProResDecoder` implementing the codec trait; ties the parsing + decoding pipeline together | ~200 LOC |
| Real-stream conformance test | Drop ProRes test fixtures in `crates/oximedia-codec/tests/fixtures/prores/`; test harness decodes them and asserts plane-buffer SHA-256 against expected hashes | ~50 LOC + fixtures |
| SIMD-accelerated IDCT | Replace `idct_1d` with a NEON/AVX2 version; ~5× faster but needs careful integer correctness checks | ~500 LOC |

The conformance test is the most important one — without it, none of
the others can be confidently extended. The wiring tasks (multi-slice
+ alpha + trait integration) are all small but risk shipping a
"working" decoder that produces wrong pixels.

## 19. Reading order

If you opened this doc cold and want to internalize the codebase:

1. Read this doc end-to-end.
2. Open [`prores/quant.rs`](../crates/oximedia-codec/src/prores/quant.rs)
   — the simplest file. Confirm the matrices match this doc's §5.
3. Open [`prores/bitreader.rs`](../crates/oximedia-codec/src/prores/bitreader.rs)
   and read the tests to see what input → output relationships are
   guaranteed.
4. Open [`prores/entropy.rs`](../crates/oximedia-codec/src/prores/entropy.rs)
   and trace `decode_block` against §10 above.
5. Open [`prores/idct.rs`](../crates/oximedia-codec/src/prores/idct.rs)
   and trace `idct_1d` against §13.
6. Finally [`prores/decode.rs`](../crates/oximedia-codec/src/prores/decode.rs)
   to see the whole loop.

The four "trivial" files
([`zigzag`](../crates/oximedia-codec/src/prores/zigzag.rs),
[`dequant`](../crates/oximedia-codec/src/prores/dequant.rs),
[`frame`](../crates/oximedia-codec/src/prores/frame.rs),
[`picture`](../crates/oximedia-codec/src/prores/picture.rs)) you can
read in 5 minutes each.

Total: ~30 minutes to be productive in the module.

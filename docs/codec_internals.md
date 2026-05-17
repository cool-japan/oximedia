# Codec Internals — Onboarding Reference

> **Audience.** A competent Rust engineer who is new to video and audio
> codecs. By the end of this document you should be able to read the
> code in [`crates/oximedia-codec/`](../crates/oximedia-codec/) and
> [`crates/oximedia-audio/`](../crates/oximedia-audio/) and recognize
> what each module is doing and why, even if you can't yet write a
> new decoder from scratch.
>
> **Companion documents.**
> [`wire_formats.md`](wire_formats.md) covers the layer *underneath*
> the codec (NAL framing, RTP, FFI conventions).
> [`codec_status.md`](codec_status.md) is the per-codec status sheet —
> read it after this doc to know what's actually implemented and what
> needs work.

## 0. Why compression is possible at all

Raw 1080p30 4:2:0 video is 1920 × 1080 × 1.5 × 30 ≈ **93 MB/s**. A
30-minute episode is ~167 GB. Modern codecs (H.264, HEVC, AV1) compress
this by 1000× or more — a 1.5 GB Netflix episode at the same resolution.
That factor of a thousand isn't magic; it falls out of three kinds of
redundancy in real video:

|Kind|Example|Codec mechanism|
|---|---|---|
|**Spatial**|Adjacent pixels in a frame are correlated (sky pixels look like other sky pixels).|Intra prediction + transform + quantization|
|**Temporal**|Adjacent frames look similar (between frame N and N+1, most things didn't move).|Inter prediction (motion compensation)|
|**Perceptual**|Your eye is much worse at high-frequency colour detail than at luminance, and worse still at high-frequency motion.|Chroma subsampling, quantization shaped by human-vision models|

A codec is a system that exploits all three. Compression is *lossy* — the
decoded output doesn't match the input byte-for-byte. The skill is making
the loss invisible.

## 1. Color and pixel fundamentals

### RGB versus YCbCr

Sensors and displays speak RGB: three values per pixel for red, green,
blue. But RGB is bad for compression because neighbouring colour channels
are highly correlated (a blue pixel is usually next to other blue
pixels), and a human can't tell red from green nearly as well as bright
from dark. The standard fix is to convert to **YCbCr** (also called
YUV), a colour space that separates **luminance** (Y, brightness) from
**chrominance** (Cb and Cr, the colour offsets):

```text
 Y  =  0.2126·R + 0.7152·G + 0.0722·B          (BT.709, HD)
 Cb = (B - Y) / 1.8556    →   shifted so 128 = grey
 Cr = (R - Y) / 1.5748    →   shifted so 128 = grey
```

Y carries almost all of the *perceived* information; Cb and Cr can be
compressed much more aggressively without anyone noticing.

The matrix changes per colour space: BT.601 (SD), BT.709 (HD), BT.2020
(UHD), and Apple's display-P3 each use slightly different coefficients.
Get the matrix wrong and skin tones come out green.

### Chroma subsampling

Because Y matters more than CbCr, you can store CbCr at lower resolution:

```text
4:4:4   Y at full resolution, Cb at full, Cr at full.    1× chroma data, lossless
4:2:2   Y at full resolution, Cb at ½ horizontal, ditto. ½× chroma data, broadcast standard
4:2:0   Y at full resolution, CbCr at ½ both axes.       ¼× chroma data, web/streaming default

 4:2:0 sample positions (the "MPEG-2" convention used by H.264 / HEVC / AV1):

   Y . Y . Y . Y .          dot = no sample
   . . . . . . . .          C   = co-located Cb and Cr sample
   Y C Y . Y C Y .
   . . . . . . . .
   Y . Y . Y . Y .
   . . . . . . . .
   Y C Y . Y C Y .          one C sample covers a 2×2 block of Y
```

4:2:0 saves 50% of total samples (one Y per pixel; one Cb + one Cr per
four pixels = 1.5 samples/pixel total instead of 3). It's the format
everyone uses for delivery. **NV12** (covered in
[`wire_formats.md`](wire_formats.md)) is the in-memory layout for 4:2:0:
one Y plane followed by one interleaved CbCr plane.

### Bit depth

Pixel values are usually stored as 8-bit integers (0–255 per channel).
HDR formats use 10-bit (0–1023) or 12-bit. The extra precision matters
because:

1. HDR transfer functions (PQ, HLG) crowd more "perceptually equal"
   steps into the dark end, so 8 bits causes visible banding.
2. Internal arithmetic during decode (motion comp, transforms) needs
   headroom to avoid rounding errors; encoding 8-bit pixels through a
   10-bit pipeline gives cleaner output.

Bit depth lives in pixel format names: `Yuv420p` is 8-bit, `Yuv420p10le`
or `P010` is 10-bit little-endian.

### Limited (video) vs full range

For historical compatibility with analog video, "video range" reserves
some headroom outside the visible range:

|Range|Y values|Cb/Cr values|
|---|---|---|
|Limited / video|16 – 235|16 – 240|
|Full / PC|0 – 255|0 – 255|

The bits 0–15 are "blacker than black" (sync); 236–255 are "whiter than
white" (overshoots). Real content lives in the middle. Modern web
content uses full range; broadcast uses video range. Mixing them up
produces washed-out or crushed pictures.

## 2. The block-coding pipeline

Every modern video codec — H.264, HEVC, AV1, VP9, VP8 — uses
**block-based hybrid coding**. The same pipeline shape applies to all
of them; what differs is the block sizes, prediction modes, transforms,
and entropy coder.

### Encoder side (informational — this project is decode-focused)

```text
   raw frame
       │
       ▼
  ┌──────────────┐
  │    split     │  → 16×16 macroblocks (H.264), 64×64 CTUs (HEVC),
  │  into blocks │    128×128 superblocks (AV1). Recursively subdivided
  └──────────────┘    based on local complexity.
       │
       ▼
  ┌──────────────┐  intra mode (predict from in-frame neighbours)
  │   predict    │  or
  │    block     │  inter mode (predict from a previous decoded frame +
  └──────────────┘                a motion vector)
       │
       ▼
  ┌──────────────────┐
  │ residual = orig  │  ← the small "what's left after prediction"
  │           − pred │     signal that actually gets transmitted
  └──────────────────┘
       │
       ▼
  ┌──────────────┐
  │ forward      │  Decorrelate spatially. 4×4 / 8×8 / 16×16 /
  │ transform    │  32×32 integer-DCT variants.
  └──────────────┘
       │
       ▼
  ┌──────────────┐
  │ quantize     │  ← THE lossy step. Most coefficients become 0.
  └──────────────┘
       │
       ▼
  ┌──────────────┐
  │ entropy code │  CABAC (H.264/HEVC), range coder (AV1).
  └──────────────┘
       │
       ▼
    bitstream

  (side branch: dequantize → inverse transform → add prediction →
  loop filter → store in DPB so this block's reconstruction can be
  used as the "neighbour" for the next block, exactly as the decoder
  will see it. This guarantees encoder/decoder mismatch is zero.)
```

### Decoder side (what `oximedia-codec` actually has to do)

```text
    bitstream
       │
       ▼
  ┌──────────────┐
  │ entropy      │ → quantized coefficients + side info:
  │ decode       │     prediction mode, MV, partition tree, …
  └──────────────┘
       │
       ▼
  ┌──────────────┐
  │ dequantize   │ → coefficients
  └──────────────┘
       │
       ▼
  ┌──────────────┐
  │ inverse      │ → residual block
  │ transform    │
  └──────────────┘
       │
       ▼
  ┌────────────────────────┐
  │  build prediction:     │  intra: read decoded neighbours.
  │  intra or inter        │  inter: fetch from reference frame
  │                        │         using the decoded MV.
  └────────────────────────┘
       │
       ▼
  ┌──────────────────────────┐
  │ reconstruct = pred + res │
  └──────────────────────────┘
       │
       ▼
  ┌──────────────┐
  │ loop filter  │  Smooths block boundaries; in newer codecs also
  │              │  applies SAO / CDEF / restoration.
  └──────────────┘
       │
       ▼
    output frame, stash in DPB
```

This is the architecture mirrored in
[`crates/oximedia-codec/src/reconstruct/pipeline.rs`](../crates/oximedia-codec/src/reconstruct/pipeline.rs).
The stage modules — `entropy_coding.rs`, `intra/`, `motion/`,
`reconstruct/residual.rs`, `reconstruct/loop_filter.rs` — each
implement one box of this diagram. **When the diagram and the modules
line up, codec internals stop being magic.**

### Block sizes across codecs

|Codec|Largest block|Smallest block|Largest transform|
|---|---|---|---|
|H.263|16×16|8×8|8×8 DCT|
|H.264 / AVC|16×16 (macroblock)|4×4|8×8|
|HEVC / H.265|64×64 (CTU)|4×4|32×32|
|AV1|128×128 (superblock)|4×4|64×64|

The bigger blocks let newer codecs spend bits more efficiently on flat
regions (one big block instead of sixteen small ones); the smaller
blocks let them adapt to texture and edges.

## 3. Intra prediction

For the first frame of a sequence (or any keyframe), there's no
"previous frame" to reference, so prediction has to come from somewhere
*inside* the current frame. **Intra prediction** generates a guess for
the current block from the pixels of already-decoded neighbouring
blocks — typically the row above and the column to the left.

### H.264 4×4 luma intra modes

H.264 defines 9 modes for predicting a 4×4 block, identified by an index
0–8. Take this 4×4 block where `A`–`L` are decoded neighbour pixels
already in the output:

```text
 M | A B C D | E F G H
 ──┼─────────┼─────────
 I |         |
 J |  ?  ?   |
 K |  ?  ?   |
 L |         |
```

The nine modes:

|ID|Name|What it predicts|
|---|---|---|
|0|Vertical|Copy A, B, C, D straight down the column|
|1|Horizontal|Copy I, J, K, L straight across the row|
|2|DC|Average all available neighbours, fill the whole block with the mean|
|3|Diagonal Down-Left|Project neighbours along a ↘ direction|
|4|Diagonal Down-Right|Project along ↗|
|5|Vertical-Right|Mostly vertical, slightly tilted|
|6|Horizontal-Down|Mostly horizontal, slightly tilted|
|7|Vertical-Left|Like 5, opposite tilt|
|8|Horizontal-Up|Like 6, opposite tilt|

### Worked example: horizontal mode on a 4×4

Suppose the row-left neighbours are `I=80, J=82, K=85, L=90` (a smooth
brightness ramp). Horizontal prediction (mode 1) fills every row of the
block with its left neighbour:

```text
  predicted block          original block        residual
  ┌──────────────┐         ┌──────────────┐      ┌──────────────┐
  │ 80 80 80 80  │         │ 81 82 83 84  │      │ +1 +2 +3 +4  │
  │ 82 82 82 82  │         │ 83 84 85 86  │  =   │ +1 +2 +3 +4  │
  │ 85 85 85 85  │   →     │ 86 87 88 89  │      │ +1 +2 +3 +4  │
  │ 90 90 90 90  │         │ 91 92 93 94  │      │ +1 +2 +3 +4  │
  └──────────────┘         └──────────────┘      └──────────────┘
```

The residual is small (±4) and has a clear horizontal structure. The
transform stage will turn that into a sparse coefficient block; the
quantizer will turn most coefficients into zero. The compression ratio
on this block is enormous — *because the prediction was good*. Picking
a bad prediction mode would leave a residual with values like ±60 and
no structure, and the bitstream would balloon.

The mode index itself costs bits to transmit; modern codecs use
**most-probable-mode** signalling to predict the predictor — assume the
mode of the block above or to the left, and only transmit a delta.

### HEVC and AV1: more modes, bigger blocks

HEVC defines 35 intra modes (33 directional + DC + planar). AV1 defines
56 (including chroma-from-luma, recursive intra, Paeth-style, palette
mode for screen content). The principle is the same — more modes mean
better predictions on more textures, at the cost of mode-signalling bits
and encoder search complexity.

**In code:**
[`intra/modes.rs`](../crates/oximedia-codec/src/intra/modes.rs) enumerates the
mode kinds. [`intra/directional.rs`](../crates/oximedia-codec/src/intra/directional.rs)
projects neighbour pixels along an angle.
[`intra/dc.rs`](../crates/oximedia-codec/src/intra/dc.rs),
[`intra/smooth.rs`](../crates/oximedia-codec/src/intra/smooth.rs),
[`intra/paeth.rs`](../crates/oximedia-codec/src/intra/paeth.rs),
[`intra/palette.rs`](../crates/oximedia-codec/src/intra/palette.rs) are the
non-directional special modes.

## 4. Inter prediction (motion compensation)

For every frame after the first, instead of predicting from in-frame
neighbours we can predict from a **previously decoded frame**. If
something didn't move between frames N and N+1, we can predict its
block in N+1 by copying it from N — a perfect prediction with zero
residual.

For things that *did* move, the encoder finds the best-matching block
in the reference frame (within some search window) and transmits a
**motion vector** (MV) pointing to it:

```text
 reference frame N                current frame N+1
 ┌────────────────────────────┐   ┌────────────────────────────┐
 │                            │   │                            │
 │     ┌──────────┐           │   │              ┌──────────┐  │
 │     │   ball   │           │   │              │   ball   │  │
 │     └──────────┘           │   │              └──────────┘  │
 │     (x=40, y=70)           │   │              (x=85, y=78)  │
 │                            │   │                            │
 └────────────────────────────┘   └────────────────────────────┘

 Encoder transmits, for the current frame's ball block:
   reference index = N
   motion vector  = (+45, +8)
   residual       = ~0 (the ball didn't deform, just translated)
```

The decoder, given the MV, fetches the same pixel rectangle from the
reference frame and uses it as the prediction. The residual is then
added in the usual way.

### Sub-pixel motion

Real motion isn't whole-pixel. A baseball moving at 30 m/s across a
1080p frame moves ~14 pixels per frame at 30 fps — and that's an
average; individual blocks slide by fractional amounts. So MVs are
sub-pixel: H.264 supports quarter-pixel precision, AV1 supports
1/8-pixel. To fetch a block at a sub-pixel offset the decoder
**interpolates** between integer-grid pixels using a fixed filter:

```text
 horizontal half-pel: H.264's 6-tap filter = (1, -5, 20, 20, -5, 1) / 32
 then vertical half-pel uses the same filter on the half-pel rows
 quarter-pel = bilinear average of integer-pel and half-pel
```

The filter is specified to bit-exact precision so every decoder
produces the same output.

### Reference picture buffer (DPB)

Inter prediction needs the references to actually be there. The
**Decoded Picture Buffer** (DPB) is the small set of past frames the
decoder keeps around for reference. Typical size: 4–16 frames depending
on profile/level. The encoder explicitly signals which references to
keep and which to drop.

### P-frames and B-frames

|Frame type|Predicts from|
|---|---|
|I (intra)|Nothing — only intra prediction|
|P (predicted)|One earlier frame|
|B (bidirectional)|Two reference frames, typically one past and one future — *averaged* (or weighted)|

B-frames give the best compression for non-key content because moving
objects can be predicted from where they are *now* and where they will
be — small wins compound. They also break the "decode order = display
order" assumption: to decode a B-frame the decoder needs both its
references already, which means the bitstream interleaves them
non-monotonically in time. This is why streams carry both **DTS**
(decode timestamp — bitstream order) and **PTS** (presentation
timestamp — display order); see [`wire_formats.md`](wire_formats.md) on
CMTime.

### GOP structure

A **Group of Pictures** is the chunk between two keyframes (IDRs). A
classic GOP looks like:

```text
 display order:  I  B  B  P  B  B  P  B  B  P  …
 frame number:   0  1  2  3  4  5  6  7  8  9
 decode order:   0  3  1  2  6  4  5  9  7  8
```

The decoder receives frames in decode order, parses them, and the
display layer reorders to PTS. Modern encoders use **hierarchical B**
where B-frames at deeper levels reference other B-frames — more
complex, but allows graceful quality degradation if you have to drop
frames under bandwidth pressure.

**In code:**
[`motion/types.rs`](../crates/oximedia-codec/src/motion/types.rs) defines `MotionVector`.
[`motion/predictor.rs`](../crates/oximedia-codec/src/motion/predictor.rs) does the
prediction itself. [`motion/subpel.rs`](../crates/oximedia-codec/src/motion/subpel.rs)
implements the interpolation filters.
[`motion/search.rs`](../crates/oximedia-codec/src/motion/search.rs) and
[`motion/diamond.rs`](../crates/oximedia-codec/src/motion/diamond.rs) are
encoder-side search algorithms. [`gop_structure.rs`](../crates/oximedia-codec/src/gop_structure.rs)
handles I/P/B scheduling.

## 5. Transforms

After prediction, the residual is a small block of values that's
*usually structured* — smooth gradients, a single edge, low-frequency
texture. Transforms convert spatial data into frequency-domain
coefficients, which packs the residual's energy into a small number of
big coefficients with most of the rest near zero.

### Discrete Cosine Transform (DCT)

The DCT is a real-valued cousin of the Fourier transform. For a 1D
signal of length N:

```text
  X[k] = Σ_{n=0}^{N-1} x[n] · cos( π·(2n+1)·k / (2N) )
```

A 2D DCT (used for blocks) just applies the 1D DCT to rows, then to
columns. Low-`k` coefficients capture low-frequency content (smooth
ramps, flat regions); high-`k` coefficients capture high-frequency
content (edges, noise). Smooth residuals have almost all their energy
in the top-left few coefficients.

### H.264's integer 4×4 forward transform

Real-valued DCT means floating-point math, which is *not* bit-exact
across implementations. H.264 instead specifies an integer
approximation: every encoder and decoder uses the same matrix and gets
the same result. The 4×4 forward transform is:

```text
        ┌                  ┐
        │  1   1   1   1   │
   H =  │  2   1  -1  -2   │
        │  1  -1  -1   1   │
        │  1  -2   2  -1   │
        └                  ┘

   Y = H · X · H^T          (X is the residual block, Y is coefficients)
```

The factor-of-2 entries (`2` and `-2`) mean the transform isn't quite
orthonormal — the encoder folds the missing scale into the
quantization step, which is essentially free.

### Worked example: 4×4 transform on a smooth block

Take the horizontal-mode residual from §3:

```text
            ┌                   ┐
            │  +1  +2  +3  +4   │
   X    =   │  +1  +2  +3  +4   │
            │  +1  +2  +3  +4   │
            │  +1  +2  +3  +4   │
            └                   ┘
```

Applying `Y = H·X·H^T` gives (working in the H.264 integer scale):

```text
            ┌                       ┐
            │   40   -20    0    0  │
   Y    =   │    0     0    0    0  │   ← all the residual's
            │    0     0    0    0  │     energy is now in the
            │    0     0    0    0  │     top row
            └                       ┘
```

Three non-zero coefficients out of sixteen — that's *energy compaction*.
The entropy coder will encode this in a handful of bits.

### Bigger transforms

H.264 supports 4×4 and 8×8. HEVC supports 4×4 / 8×8 / 16×16 / 32×32.
AV1 supports rectangular transforms too: 4×8, 8×16, 16×32, plus
DST-variants and identity transforms ("skip") for screen-content cases
where the residual is already sparse.

Larger transforms = better compaction on smooth regions, worse on busy
ones. The encoder picks per-block.

**In code:** transform implementations are scattered through the
per-codec directories (`av1/`, `vp9/`, `ffv1/`, `mjpeg/`,
`jpegxl/`, etc.). The reconstruction pipeline calls them through
[`reconstruct/residual.rs`](../crates/oximedia-codec/src/reconstruct/residual.rs).

## 6. Quantization

This is **the** lossy step. Every other stage of the codec is reversible
modulo integer rounding; quantization is where information is thrown
away on purpose.

The idea: divide each transform coefficient by a step size and round to
the nearest integer.

```text
  q[k] = round( Y[k] / step(k) )
```

When the decoder dequantizes:

```text
  Y'[k] = q[k] · step(k)
```

If `Y[k]` was 23 and the step was 8: `q = round(23/8) = 3`, and the
decoder reconstructs `Y' = 3 · 8 = 24`. The 1-unit error is the
*quantization noise*. If `Y[k]` was 4 and the step was 8: `q = 0`, and
the coefficient is gone — saving the bits but losing the information.

### QP — the quantization parameter

Codecs don't transmit raw step sizes; they transmit a small integer QP
(quantization parameter, 0–51 in H.264) and derive the step from it:

```text
  H.264: step doubles every 6 QP values.
         QP  0 →  step ~0.625
         QP  6 →  step  1.25
         QP 12 →  step  2.5
         QP 18 →  step  5
         QP 24 →  step 10
         QP 30 →  step 20
         QP 36 →  step 40
         QP 42 →  step 80
         QP 48 →  step 160
```

Doubling step ≈ doubling perceived quantization noise ≈ halving bitrate
(very roughly). QP is the single biggest knob a streaming encoder
turns. The same content at QP 22 vs QP 38 is the difference between
"looks like the source" and "watchable but obviously compressed."

### Quantization matrices

Not all coefficients are quantized equally. High-frequency coefficients
(bottom-right of the DCT block) get **larger** step sizes than
low-frequency ones (top-left) — your eye doesn't care about
high-frequency noise as much. Codecs ship default matrices plus
encoder-supplied custom ones for HDR or grain-heavy content.

### Dead-zone

A "dead-zone" quantizer rounds *toward zero* in a wider band around the
origin, instead of regular rounding:

```text
  q[k] = sign(Y[k]) · max( 0, ( |Y[k]| - dead_zone ) / step )
```

So `Y=5` with `step=8, dead_zone=2` rounds to 0 (because the magnitude
is below the dead-zone) instead of 1. This kills low-amplitude noise
and improves compression at the cost of some quality. Almost all
modern codecs use dead-zone quantization.

**In code:** quantization is per-codec (transform and quantize are
tightly coupled). Bitrate-driven QP selection lives in
[`bitrate_model.rs`](../crates/oximedia-codec/src/bitrate_model.rs).

## 7. Entropy coding

After quantization, the encoder has a sparse block of integers (most
zero) plus side information (modes, MVs, partition trees). **Entropy
coding** packs this into the smallest possible bitstream by giving
short codes to frequent values and long codes to rare ones.

### Zigzag scan + run-length

A quantized 4×4 block, scanned in **zigzag** order, tends to put all
non-zero coefficients near the start and a long run of zeros at the
end:

```text
   ┌──────────────┐
   │ 8 -3  0  0   │      zigzag order: 8, -3, 1, 0, 0, 1, 0, 0,
   │ 1  0  1  0   │                    0, 0, 0, 0, 0, 0, 0, 0
   │ 0  0  0  0   │
   │ 0  0  0  0   │      → encode "8, -3, 1, [0×2], 1, [0×9]"
   └──────────────┘
```

The encoder transmits non-zero values plus zero-run lengths. Run-length
makes the tail very cheap.

### Variable-length codes (VLC)

The simplest entropy coder assigns short codes to common values. Example
table from H.264's exp-Golomb code for non-negative integers:

```text
  value  | code
  -------+-------
    0    | 1
    1    | 010
    2    | 011
    3    | 00100
    4    | 00101
    5    | 00110
   …
```

Most coefficients are 0 (1 bit). 1 and 2 are next most common (3 bits).
Big values get long codes but appear rarely, so the average bits-per-
symbol is small. This was good enough for early codecs (H.263, MJPEG).

### CABAC (Context-Adaptive Binary Arithmetic Coding)

H.264 Main profile and HEVC use **CABAC**, which compresses much
better than VLC. Two pieces:

1. **Binarization.** Every syntax element is mapped to a sequence of
   bits.
2. **Arithmetic coding.** Each bit is encoded using a probability
   model that *updates* as you go. If a "0" bit appears 80% of the
   time in this context, encoding it costs ~0.32 bits (= -log₂ 0.8);
   encoding a "1" costs ~2.32 bits.

A "context" is essentially a small state machine that says "I just saw
3 zeros in a row, the probability of the next bit being 0 is now 90%."
Modern codecs maintain hundreds of contexts simultaneously, one per
syntax element kind.

CABAC compresses ~10–15% better than CAVLC (H.264 Baseline's VLC
scheme) at the cost of being serial — each bit depends on the
probability state left by the previous bit. This is why CABAC is hard
to parallelize and why hardware decoders have a dedicated CABAC engine.

### AV1's range coder

AV1 uses a range coder, which is mathematically equivalent to arithmetic
coding but works in finite-precision integer arithmetic from the start.
Same complexity, same compression, slightly different implementation
details.

**In code:**
[`entropy_coding.rs`](../crates/oximedia-codec/src/entropy_coding.rs) and
[`entropy_tables.rs`](../crates/oximedia-codec/src/entropy_tables.rs) hold the
shared entropy infrastructure. Per-codec entropy decoders live with their
codec ([`av1/`](../crates/oximedia-codec/src/av1/), etc.).

## 8. Loop filtering

If you reconstruct a frame block by block — predict, dequantize,
inverse-transform, add — the boundaries between blocks become visible.
Quantization noise that happens to land just inside the left edge of
block B doesn't quite match the quantization noise just inside the
right edge of block A; the result is a thin discontinuity at every
8×8 / 16×16 / 64×64 boundary. At low bitrates this looks like a grid
of soap-bubble blocks.

**Loop filtering** runs as the last step of every decoded frame to
smooth those discontinuities. It runs **inside the prediction loop** —
the filtered output is what gets stored in the DPB and what future
frames predict from — hence the name. Skipping the filter doesn't just
hurt this frame's quality; it propagates error into every later frame.

### Deblocking (H.264, HEVC)

The classic loop filter. For every block edge, look at the values just
either side:

```text
 horizontal edge:
                       │
   p3  p2  p1  p0      │     q0  q1  q2  q3
       (block A)       │       (block B)
                       │
                       ↑ filter operates on these eight samples
```

Decide whether the discontinuity is real content (a true edge in the
picture) or coding artefact (block boundary noise). If artefact, blend
the samples across the boundary with a low-pass filter. The strength of
the blending depends on the local QP — high QP, more noise, stronger
filter.

Deblocking is content-aware: the filter is *off* when a real edge sits
on the block boundary, *on* when the edge is artificial.

### SAO (HEVC) — Sample Adaptive Offset

A post-deblock stage that tweaks individual sample values:

- **Band offset**: add a small offset to all samples whose value
  falls inside a chosen band of the histogram (e.g. all values
  64–67). Fixes banding in smooth gradients.
- **Edge offset**: classify each sample by its local shape (peak,
  valley, monotonic edge, etc.) and add a small offset to peaks and
  valleys to soften them.

The offsets are signaled per-CTU; the decoder applies them.

### CDEF (AV1) — Constrained Directional Enhancement Filter

A directional filter — finds the dominant edge direction in an 8×8
block and filters along that direction. Reduces blockiness and
quantization noise on directional edges while preserving the edges
themselves. Strength is signaled in the bitstream.

### Loop restoration (AV1)

A final stage using either a **Wiener** filter (optimal linear filter
trained against the original) or a **self-guided** filter (computed
from the decoded frame itself). Closes the remaining gap to the
original.

**In code:**
[`reconstruct/deblock.rs`](../crates/oximedia-codec/src/reconstruct/deblock.rs)
covers H.264/HEVC deblocking;
[`reconstruct/cdef_apply.rs`](../crates/oximedia-codec/src/reconstruct/cdef_apply.rs)
applies AV1 CDEF;
[`reconstruct/loop_filter.rs`](../crates/oximedia-codec/src/reconstruct/loop_filter.rs)
is the shared in-loop driver.

## 9. Rate control

Every codec stage above is *reversible* from a bitrate perspective —
quantization is the only knob that trades quality for bits. Rate
control is the encoder algorithm that decides "for this frame, in this
GOP, given my bitrate budget, what QP should I use?"

### CBR vs VBR vs CRF

|Mode|Goal|Bitrate behaviour|
|---|---|---|
|**CBR** (constant bitrate)|hit a fixed bitrate exactly|Steady; quality fluctuates with content|
|**VBR** (variable bitrate)|hit average target, allow short bursts|Bumpy; better quality on hard scenes|
|**CRF** (constant rate factor)|hit a fixed *quality*|Total bitrate varies with content|

CRF is the right default for non-streaming workflows (archival,
on-demand). CBR is the right default for live streaming with strict
budget constraints. VBR is the middle ground for VOD when bitrate
matters but quality matters more.

### Lagrangian RD optimization

The mathematically clean way to make encoding decisions: minimize

```text
  J = D + λ · R

   D = distortion of the candidate choice (e.g. SSE between predicted
       and original block)
   R = bits the candidate would cost
   λ = Lagrange multiplier — set by the target QP
```

A given mode, MV, partition, etc. is "better" than another if its J
is lower. λ ties bits to quality: low λ → spend bits freely for quality;
high λ → save bits. This is the framework every modern encoder uses
internally.

**In code:** [`rate_control.md`](rate_control.md) is the dedicated
deeper reference; [`bitrate_model.rs`](../crates/oximedia-codec/src/bitrate_model.rs)
and [`multipass.rs`](../crates/oximedia-codec/src/multipass.rs) implement the
encoder-side machinery.

## 10. Profile, level, tier

Codec standards are big. Implementations don't support every feature.
**Profiles, levels, tiers** are the standardized way of negotiating
which subset is in play.

### Profile = which features

H.264 profiles, in roughly increasing complexity:

|Profile|Adds|
|---|---|
|Baseline|I and P frames, CAVLC, no B-frames|
|Main|+ B-frames, CABAC|
|High|+ 8×8 transform, custom quant matrices, monochrome support|
|High 4:2:2|+ 4:2:2 chroma|
|High 4:4:4 Predictive|+ 4:4:4 chroma, lossless mode|

A decoder advertising "High profile, level 4.1" is promising to decode
everything in High; it might not handle High 4:4:4.

### Level = constraints on size and rate

Levels cap max resolution, frame rate, bitrate, and DPB size:

|Level|Max resolution × fps|Max bitrate (Main)|
|---|---|---|
|3.0|720×480 × 30|10 Mbit/s|
|3.1|1280×720 × 30|14 Mbit/s|
|4.0|1920×1088 × 30|20 Mbit/s|
|4.1|1920×1088 × 30|50 Mbit/s|
|5.0|2560×1920 × 30|135 Mbit/s|
|5.1|4096×2304 × 30|240 Mbit/s|
|6.0|8192×4320 × 30|240 Mbit/s|

### Tier (HEVC only)

HEVC adds a "Main tier" / "High tier" axis: same features, different
max bitrates. High tier doubles or triples the bitrate ceiling for
professional or archival use.

This metadata lives in the SPS (Sequence Parameter Set) and the
container, and is what `H264FormatDescription` or `CMVideoFormatDescription`
serializes for the decoder.

## 11. Audio coding fundamentals

Video coding dominates this doc because video coding dominates the
problem space, but everything in `oximedia-audio` and the audio side of
`oximedia-codec` is built on a small set of audio primitives.

### PCM as the baseline

**Pulse-Code Modulation** is the uncompressed representation: a stream
of samples at a fixed sample rate (44.1 kHz, 48 kHz, 96 kHz) and bit
depth (16-bit signed, 24-bit signed, 32-bit float). Stereo at 44.1 kHz
16-bit is 1.4 Mbit/s — manageable raw, but big for a download. PCM is
the input/output of every codec.

### Frequency-domain coding (MDCT)

Like video uses DCT to decorrelate space, audio uses the **Modified
DCT** (MDCT) to decorrelate time. The encoder splits the signal into
overlapping windows of (e.g.) 1024 samples and transforms each window
into 512 frequency coefficients. The overlap + windowing avoids
boundary artefacts.

Once in frequency domain, the encoder can quantize each frequency
band independently, allocating bits where they matter.

### Perceptual masking

Two human-ear properties make audio compression possible:

1. **Frequency masking.** A loud tone at 1 kHz makes nearby
   frequencies (e.g. 950 Hz – 1100 Hz) inaudible. Bits spent encoding
   them are wasted.
2. **Temporal masking.** Loud sounds mask quieter sounds for a few
   tens of milliseconds before and after.

A psychoacoustic model computes, per frame, the **mask-to-noise ratio**
in each frequency band — the maximum quantization noise that will go
unnoticed. Bits are allocated to keep noise below the mask everywhere.

### AAC vs Opus

|Codec|Strengths|Typical use|
|---|---|---|
|**AAC**|Mature, ubiquitous, good at music at 96–192 kbps|Streaming (legacy), broadcasting, iTunes|
|**Opus**|Excellent at low bitrate (16–32 kbps for speech), low latency, royalty-free|WebRTC, modern streaming, in-game voice|

The relevant audio modules: [`crates/oximedia-audio/`](../crates/oximedia-audio/)
for codecs already in-tree (Opus, Vorbis, FLAC, MP3, PCM);
[`celt.rs`](../crates/oximedia-codec/src/celt.rs) for the CELT part of Opus.

### Lossless audio (FLAC)

FLAC predicts each sample as a linear combination of previous samples,
then losslessly entropy-codes the small residual. Compression ratios
50–70% are typical; the decoded output is bit-exact to the input.
Conceptually identical to video intra prediction + entropy coding;
audio just has one spatial dimension instead of two.

**In code:** [`flac/`](../crates/oximedia-codec/src/flac/) and
[`flac_codec.rs`](../crates/oximedia-codec/src/flac_codec.rs).

## 12. Container basics

A **container** wraps one or more compressed media streams ("elementary
streams" — H.264 video, AAC audio, etc.) with metadata so a player can
find, sync, and decode them.

### Vocabulary

|Term|Meaning|
|---|---|
|**Sample**|One compressed unit in the container — typically one access unit (one frame for video, one packet for audio).|
|**Access unit** (AU)|The bytes a decoder needs to decode one frame. For H.264, all NALs that share the same PTS.|
|**Frame**|Confusing — in container land it's a sample; in codec land it's the decoded picture.|
|**Track**|One elementary stream (e.g. "the H.264 video track").|
|**Mux**|Combine multiple elementary streams into a container.|
|**Demux**|Pull elementary streams back out.|

### Timestamps

Every sample has at least one timestamp:

- **PTS (presentation timestamp)**: when to display.
- **DTS (decode timestamp)**: when to feed to the decoder.

For streams with no B-frames, PTS = DTS. With B-frames, the decoder
order ≠ display order (see §4), so PTS and DTS diverge.

Timestamps are expressed in a **timebase** — a rational number of seconds
per tick. H.264's typical timebase is 1/90000 (90 kHz), so a PTS of 9000
means 0.1 s.

### Fragmentation, edit lists, chapters

- **Fragmentation** (fragmented MP4 / fMP4): split a single file into
  many small self-contained pieces. Required for adaptive streaming
  (HLS, DASH) and for live recording where you don't know the duration
  upfront.
- **Edit lists**: an in-file map saying "play track 1 from time 5s to
  10s, then track 2 from time 0s to 3s." Lets a container re-time
  content without re-encoding.
- **Chapters / cues**: bookmarks. Player UI only.

**In code:** [`crates/oximedia-container/`](../crates/oximedia-container/) owns
container parsing/emission. The codec-side container helpers live under
[`oximedia-codec/src/container/`](../crates/oximedia-codec/src/container/).

## 13. How this all maps to `oximedia-codec`

```text
                          ┌────────────────────────────────┐
   bitstream (Annex-B,    │     oximedia-container         │
   AVCC, OBU, …)          │     - mux/demux                │
            │             │     - timestamps, tracks       │
            ▼             └────────────────────────────────┘
   ┌─────────────────────────────────────────────────────────────────┐
   │                        oximedia-codec                           │
   │                                                                 │
   │   per-codec front-end:                                          │
   │     av1/        vp9/        h263/        ffv1/        …         │
   │       │                                                         │
   │       ▼                                                         │
   │   entropy_coding.rs  ← bitstream → quantized coeffs + side      │
   │       │                                                         │
   │       ▼                                                         │
   │   intra/             ← in-frame prediction                      │
   │   motion/            ← inter prediction (motion compensation)   │
   │       │                                                         │
   │       ▼                                                         │
   │   reconstruct/                                                  │
   │     residual.rs      ← dequantize + inverse transform           │
   │     deblock.rs       ← H.264/HEVC deblocking                    │
   │     cdef_apply.rs    ← AV1 CDEF                                 │
   │     loop_filter.rs   ← in-loop driver                           │
   │     film_grain.rs    ← AV1 grain synthesis                      │
   │     super_res.rs     ← AV1 frame upscaling                      │
   │     output.rs        ← write to VideoFrame                      │
   │     pipeline.rs      ← orchestrates the stages above            │
   │                                                                 │
   │   frame.rs           ← the VideoFrame type                      │
   │   gop_structure.rs   ← I/P/B scheduling (encoder)               │
   │   bitrate_model.rs   ← rate control (encoder)                   │
   └─────────────────────────────────────────────────────────────────┘
            │
            ▼
   VideoFrame  →  caller (renderer, encoder, transcoder, …)
```

Every box in this diagram corresponds to either a stage of the decoder
pipeline (§2) or a piece of bookkeeping the codec needs.

[`codec_status.md`](codec_status.md) is the place to look for *which*
of these stages are actually working per codec. As of writing, the AV1
decoder has the front-end (OBU parsing, header decode) but the
`reconstruct/pipeline.rs` stages are stubs — that's the "specialist"
work flagged there, and it maps directly to the §3–§8 material in this
document.

## 14. Recommended further reading

The doc above is a primer — every chapter in it is a textbook. If you
want depth on any one part:

|Topic|Source|
|---|---|
|H.264 end-to-end|Iain Richardson, *The H.264 Advanced Video Compression Standard*. The clearest single-volume source.|
|HEVC|Vivienne Sze, Madhukar Budagavi, Gary J. Sullivan (eds.), *High Efficiency Video Coding (HEVC)*.|
|AV1|The AOM AV1 bitstream spec (open) and Daala / Thor papers it descends from.|
|Audio coding|Marina Bosi & Richard Goldberg, *Introduction to Digital Audio Coding and Standards*.|
|Information theory background|Khalid Sayood, *Introduction to Data Compression*. The DCT/quantization/entropy story without codec specifics.|
|The actual standards|ITU-T H.264 / H.265 / H.266 (free PDFs from itu.int); AV1 spec (aomedia.org); RFC 6716 for Opus.|

For an in-tree reading order: start with this doc, then
[`wire_formats.md`](wire_formats.md) for the byte-level framing, then
[`codec_status.md`](codec_status.md) for what's implemented vs stubbed,
then pick a codec module that's at "bitstream-parsing" status and trace
its pipeline against the diagram in §13. That's the fastest path from
"new to codecs" to "able to land a stage of a decoder."

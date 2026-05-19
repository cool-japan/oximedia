# Walking through the H.264 decoder

This is a guided tour of the H.264 decoder that lives in
`crates/oximedia-codec/src/h264/`.  It exists because the commit
history is dense — 47 commits of spec-driven plumbing don't tell
you what the result *does* or how to read it.  This document does.

If you've never opened the H.264 spec (ITU-T Rec. H.264 /
ISO/IEC 14496-10), don't.  Read this first.  Then if you want to
go deeper, the spec's clause numbers are referenced throughout
both this document and the source.

---

## 1. What you can do with this decoder, today

Construct a `Decoder`, feed it an Annex-B byte stream, get
`Frame`s back:

```rust
use oximedia_codec::h264::pipeline::Decoder;

let mut d = Decoder::new();
let frames = d.feed_annex_b(&bytes)?;
// frames: Vec<Frame>, one per IDR/P picture in stream order.
```

What the decoder handles end-to-end:

- **I-slice CABAC** with I_NxN, I_16x16, and I_PCM macroblocks.
- **I-slice CAVLC** with the same set.
- **P-slice CABAC** with all four partition shapes (16×16, 16×8,
  8×16, P_8x8 including sub-mb partitioning).
- **P-slice CAVLC** with 16×16, 16×8, 8×16, and the four 8×8
  quadrant case.  Intra macroblocks inside P slices route through
  the proper intra reconstruction.
- **B-slice CABAC** with explicit-MV partitions (codes 1..21),
  B_Direct (spatial mode), and B_8x8 sub-mb partitioning with
  bi-prediction.
- **DPB management** — short-term reference tracking, POC
  computation across `pic_order_cnt_type` 0, 1, and 2.
- **RefPicList0** + **RefPicList1** construction at slice start.
- **In-loop deblocking** for both luma and chroma 4:2:0 planes.
- **NAL routing** for SPS, PPS, IDR/non-IDR slices, AUD, SEI,
  filler — at the Annex-B byte-stream level.

What's still approximate or stubbed (with the source documenting
each one):

- B-slice **CAVLC** walks without erroring but emits `Unsupported`
  placeholders rather than decoding the bins.
- `ref_pic_list_modification` ops are parsed but not applied.
- Weighted prediction (`pred_weight_table`) is parsed but not
  applied to motion compensation.
- `directZeroPredictionFlag` for B_Direct (the spec's "if
  collocated MV is near zero" shortcut) isn't tracked — full
  median predictor always runs.
- High-profile features (8×8 transform, custom scaling lists,
  4:2:2 / 4:4:4 chroma, MBAFF / field coding, SP/SI slices) are
  explicitly out of scope.

---

## 2. The lay of the land

Every file in `crates/oximedia-codec/src/h264/` has one job.
Here's the map:

### Parser stack (bitstream → structured syntax)

| File | What it parses |
|---|---|
| `bit_reader.rs` | MSB-first bit reads, `ue(v)` / `se(v)` exp-Golomb, RBSP trailing-bit detection. |
| `rbsp.rs` | Emulation-prevention byte stripping (the `0x00 0x00 0x03 → 0x00 0x00` rewrite). |
| `sps.rs` | Sequence Parameter Set — picture dimensions, profile, level, bit depth, frame_num/POC config, scaling-list signalling. |
| `pps.rs` | Picture Parameter Set — entropy mode (CAVLC vs CABAC), QP, deblocking control, weighted-prediction flags. |
| `vui.rs` | Video Usability Information — aspect ratio, colour primaries, HRD timing. |
| `scaling_list.rs` | Custom quantisation matrices when the SPS or PPS signals them. |
| `slice_header.rs` | The 50+ fields of `slice_header` — frame_num, POC, ref-list modification, prediction weight table, decoded reference-picture marking. |
| `pcm.rs` | I_PCM macroblock payload reader / writer. |

### Entropy decoders

| File | What it does |
|---|---|
| `cavlc.rs` | The Context-Adaptive Variable Length Coding decoder.  Level VLC, total_zeros, run_before, coeff_token. |
| `cavlc_tables.rs` | The ~3000 lookup-table entries CAVLC needs (spec Tables 9-5 through 9-10). |
| `cabac.rs` | The CABAC arithmetic coder core: `get`, `get_bypass`, `get_terminate`, plus context-state initialisation. |
| `cabac_tables.rs` | The 1343-byte state-machine lookup (spec Tables 9-43 through 9-45). |
| `cabac_init_tables.rs` | The 4096 `(m, n)` initialisation pairs (spec Tables 9-12 through 9-26). |
| `cabac_syntax.rs` | The 10 per-syntax-element CABAC decoders: `mb_skip`, `mb_type`, `intra4x4_pred_mode`, `cbp_luma`, `cbp_chroma`, `ref_idx`, `mvd`, etc. |
| `cabac_residual.rs` | The CABAC residual block decoder — significance scan, last-coeff scan, level decoder, sign bypass. |
| `cabac_inter.rs` | P/B macroblock-type bin trees + the `P_MB_TYPE_INFO` and `B_MB_TYPE_INFO` lookup tables. |
| `cabac_mb.rs` | Macroblock-level residual dispatch — walks 16 luma 4×4 + 2×2 chroma DC + 8 chroma AC blocks per MB. |
| `cabac_inter_mb.rs` | The P-slice CABAC orchestrator — `decode_p_mb_cabac` calls the per-bin decoders in spec order and produces an `InterMbDecoded`. |
| `cabac_inter_b.rs` | Same idea for B slices, including B_Direct spatial inference and B_8x8 sub-mb partitioning. |

### Reconstruction (decoded coefficients → pixels)

| File | What it does |
|---|---|
| `intra_mode.rs` | Most-probable-mode (MPM) derivation for I_NxN. |
| `intra_pred.rs` | The 9 × 4×4, 4 × 16×16, and 4 × chroma 8×8 intra prediction modes. |
| `transform.rs` | The integer 4×4 inverse transform, 4×4 luma DC Hadamard, 2×2 chroma DC Hadamard, plus dequantisation. |
| `motion.rs` | 6-tap luma quarter-pel filter, bilinear chroma quarter-pel fetch. |
| `mv_pred.rs` | The median MV predictor. |
| `reconstruct_inter.rs` | Inter macroblock reconstruction — fetch from reference + IDCT residual + sum + clip + write.  Single-ref P, multi-ref P, and bi-pred B variants. |
| `reconstruct_intra_cabac.rs` | Intra macroblock reconstruction with the proper Hadamard-then-AC-dequant sequence the spec requires (8.5.6 / 8.5.10). |
| `decoder.rs` | CAVLC I-slice driver (`decode_intra_slice_bitstream`) plus the older per-shape helpers for I_NxN, I_16x16, P 16×16 (used by the CAVLC path). |

### Slice-level walkers + frame-level passes

| File | What it does |
|---|---|
| `inter_cache.rs` | Per-macroblock neighbour state — what the next macroblock needs to see from its left + top + top-right neighbour: ref idx, MV, MVD, non-zero counts. |
| `slice_cabac.rs` | The CABAC slice-data walker.  For each MB in raster order, runs the syntax + residual decoders and emits an `MbCabacDecoded`. |
| `slice_cavlc.rs` | The CAVLC slice-data walker.  Mirrors `slice_cabac.rs` but uses the CAVLC entropy path. |
| `frame.rs` | The YUV 4:2:0 picture buffer + per-block neighbour-sample gathering helpers. |
| `dpb.rs` | The Decoded Picture Buffer — short-term and long-term reference tracking, eviction. |
| `deblock.rs` | The per-edge deblocking primitives: boundary strength, α / β thresholds, normal + strong filter formulas. |
| `deblock_frame.rs` | The frame-level luma deblocking pass — walks every macroblock and applies the filter to all 4×4-aligned edges. |
| `deblock_frame_chroma.rs` | Same for the chroma 4:2:0 planes, with the spec-required narrower strong filter. |

### The driver

| File | What it does |
|---|---|
| `pipeline.rs` | The top-level `Decoder` struct.  Holds the parameter-set stores + DPB.  `feed_annex_b` extracts NALs and routes by type; SPS/PPS get stored, slices get parsed + walked + reconstructed + emitted. |

### Tests

| File | What it covers |
|---|---|
| `bitstream_roundtrip.rs` | End-to-end conformance: generate Annex-B bytes in pure Rust, feed through `Decoder`, assert decoded pixels match. |
| `integration.rs` | Pipeline composition tests (parse → reconstruct → deblock on synthetic input). |

---

## 3. Walking a byte stream through the decoder

Suppose you have a tiny H.264 byte stream:

```
00 00 00 01 67 ... (SPS NAL)
00 00 00 01 68 ... (PPS NAL)
00 00 00 01 65 ... (IDR slice NAL)
```

Here's what happens when you call `Decoder::feed_annex_b`:

### Step 1: Annex-B NAL extraction

`pipeline.rs::extract_nal_units` scans for start-code prefixes
(`0x00 0x00 0x01` or `0x00 0x00 0x00 0x01`) and splits the byte
stream into NAL units.  Each NAL is a single byte of header
followed by a payload.

### Step 2: Per-NAL dispatch

`Decoder::feed_nal` reads the header byte: bits 0..=4 are
`nal_unit_type`, bits 5..=6 are `nal_ref_idc`.  The dispatch:

| `nal_unit_type` | Handler |
|---|---|
| 7 (SPS) | `strip_emulation_prevention` → `parse_sps` → store in `sps_store` by `seq_parameter_set_id` |
| 8 (PPS) | Same, into `pps_store` |
| 1 (non-IDR slice) or 5 (IDR slice) | `decode_slice_nal` |
| 6 (SEI), 9 (AUD), 12 (filler) | Silently skipped — no frame produced |
| anything else | Returned as `Unsupported` |

### Step 3: Slice decode

`decode_slice_nal` pulls the active PPS + SPS, calls
`parse_slice_header`, then branches on
`pps.entropy_coding_mode_flag`:

- **CABAC** → `decode_cabac_slice` → `parse_slice_cabac` →
  per-MB reconstruction via `reconstruct_inter_p_mb` or
  `reconstruct_intra_*_mb_cabac`.
- **CAVLC** → `decode_cavlc_slice` →
  - I-slice + I_PCM macroblock → `pcm.rs` passthrough.
  - I-slice + non-PCM → `decode_intra_slice_bitstream` (in
    `decoder.rs`).
  - P-slice → `parse_slice_cavlc` → per-MB reconstruction.
  - B-slice → walks but emits placeholders.

### Step 4: Frame emission + DPB insert

The reconstructed `Frame` is pushed into the DPB with a computed
POC and `is_short_term_reference = nal_ref_idc != 0`.  Then
returned in the `Vec<Frame>` `feed_annex_b` collects.

---

## 4. The entropy layer

H.264 has two entropy coders.  The PPS signals which one a
picture uses via `entropy_coding_mode_flag`.

### CAVLC — Context-Adaptive Variable Length Coding

Pre-built variable-length code tables (in `cavlc_tables.rs`)
that the decoder looks up against an MSB-first bit stream.
Each residual block is decoded as:

1. `coeff_token` — total non-zero count + trailing-ones count.
2. Level VLC per non-zero coefficient (with adaptive prefix
   length).
3. `total_zeros` — total zero coefficients in the block.
4. `run_before` — zeros between non-zeros, one per non-trailing
   non-zero.

The reverse-order encoding (high-frequency first) means the
decoder reverses the stream into low-to-high scan order before
handing off to dequant + IDCT.  This is in `cavlc.rs`.

### CABAC — Context-Adaptive Binary Arithmetic Coding

Each syntax element is turned into a sequence of binary
decisions (bins).  Each bin is either:

- **Context-coded** — the decoder looks up a per-context
  probability state byte, decodes against it, updates the
  state for next time.  ~460 contexts in baseline / main
  profile, expanding to ~1024 for 8×8 transform CBF + 4:4:4.
- **Bypass-coded** — flat 50/50 probability, no state update.
  Used for sign bits and exp-Golomb suffixes.
- **Terminate-coded** — the "is this the slice's last byte?"
  probe.

The arithmetic coder lives in `cabac.rs`.  Above it sits a
syntax layer (`cabac_syntax.rs`) that knows which context to
pick for each H.264 syntax element.  Above that, a residual
layer (`cabac_residual.rs`) that walks the significance map +
level coder for transform blocks.  At the top, two macroblock
orchestrators (`cabac_inter_mb.rs` for P, `cabac_inter_b.rs`
for B) compose the per-syntax-element decoders in spec order
plus the median MV predictor.

---

## 5. The transform layer

The 4×4 inverse integer transform is in `transform.rs`.  Three
sequences matter:

### Plain 4×4

Used for I_NxN luma and all chroma AC.  Dequantise the entire
4×4 grid, then run the inverse 1-D transform on rows, then
columns.

```
dequantize_4x4(grid, qp);
inverse_transform_4x4(&grid);
```

### I_16x16 luma

The 16 DC coefficients of the 16 4×4 luma sub-blocks are
encoded as a separate 4×4 Hadamard-transformed block.

```
1. Receive 4×4 DC block.
2. inverse_hadamard_4x4_luma_dc(dc_block, qp_y)
   → 4×4 grid of dequantised DC values.
3. For each of 16 luma 4×4 AC sub-blocks:
   - zero position [0][0] (DC slot).
   - dequantize_4x4(grid, qp_y)
   - inject dc_dequant[row][col] into [0][0]
   - inverse_transform_4x4(&grid)
```

The "zero before dequant + inject after" order matters.  The
DC value was already dequantised by the Hadamard step — passing
it through `dequantize_4x4` would scale it twice.  That's
`dequant_ac_then_inject_dc` in `reconstruct_intra_cabac.rs`.

### Chroma DC

Same pattern as luma DC but with a 2×2 Hadamard
(`inverse_hadamard_2x2_chroma_dc`) since 4:2:0 chroma has
only 4 DC slots per plane.

---

## 6. Intra prediction

`intra_pred.rs` implements every direction-of-prediction the
spec defines.  Two stages:

1. Collect samples from the already-reconstructed neighbours of
   the block being predicted (`frame.rs` provides these).
2. Apply the prediction formula for the chosen mode and produce
   a 4×4 (or 16×16, or 8×8 chroma) patch of predicted samples.

### 4×4 modes (I_NxN)

Nine modes, identified by an index 0..=8 — each combines
"reference samples on the top edge" or "...on the left edge"
or both, optionally with diagonal extrapolation.  The Most-
Probable-Mode derivation in `intra_mode.rs` picks the most
likely mode from the two block-aligned neighbours and the
bitstream signals only `prev_intra4x4_pred_mode_flag` (one bit
saying "use MPM") plus an optional 3-bit `rem_intra4x4_pred_mode`
when the encoder picked something different.

### 16×16 modes (I_16x16)

Just four: Vertical, Horizontal, DC, Plane.  Applied to the
whole 16×16 luma block at once.

### Chroma 8×8

Four modes: DC, Horizontal, Vertical, Plane.

After prediction, the residual (after IDCT) is added back and
clipped to [0, 255].

---

## 7. Motion compensation

P / B macroblocks predict from a reference picture.  The
quarter-pel motion vector pulls samples from the reference and
optionally runs a 6-tap filter (luma) or bilinear filter
(chroma) to handle sub-pel positions.

### Per-4×4 dispatch

After the inter MB orchestrator (`cabac_inter_mb.rs` or its
CAVLC equivalent) decodes the partition layout + per-partition
MV deltas, it splatters the resulting absolute MVs across the
16 4×4 sub-blocks of the macroblock.  Motion compensation then
runs uniformly per 4×4:

```
for each 4×4 sub-block at (sub_x, sub_y) in the macroblock:
    (mv_x, mv_y) = mvs_l0[sub]
    int_x = block_x + (mv_x >> 2)
    int_y = block_y + (mv_y >> 2)
    sub_pel_x = mv_x & 3
    sub_pel_y = mv_y & 3
    prediction = fetch_luma_4x4_subpel(ref, int_x, int_y, sub_pel_x, sub_pel_y)
```

For 16×16 partitions all 16 sub-blocks share one MV; for 16×8
the top 8 share one MV and the bottom 8 another; for 8×16 it's
left and right; for P_8x8 each quadrant has its own MV (and
optionally further sub-mb partitioning inside each quadrant).

### Bi-prediction (B slices)

For B macroblock partitions that use both lists, the decoder
fetches from `RefPicList0` and `RefPicList1` separately and
averages per sample: `(a + b + 1) / 2`.  See
`reconstruct_inter_b_mb` in `reconstruct_inter.rs`.

### MV prediction

Each partition's MV is `predicted + delta`.  The predictor uses
the **median rule** (`mv_pred.rs::predict_mv_median`) over the
left, above, and above-right neighbours' MVs.  Special-case
overrides exist for 16×8 / 8×16 partition boundaries.

The slice-level cache (`inter_cache.rs`) tracks the neighbour
state so each macroblock's MV predictor sees the right inputs.

---

## 8. Deblocking

Block-based codecs leave visible discontinuities at block
boundaries.  In-loop deblocking smooths them after
reconstruction but *before* the picture goes into the reference
buffer, so the smoothing carries through to subsequent inter
predictions.

The per-edge formulas (boundary strength derivation, α/β
threshold check, normal vs strong filter) are in `deblock.rs`.
Two frame-level walkers drive them:

- `deblock_frame.rs` walks every 4×4-aligned edge of every
  macroblock's luma plane — one external left edge, one
  external top edge, three internal vertical edges, three
  internal horizontal edges.
- `deblock_frame_chroma.rs` does the same for the chroma 8×8
  block per macroblock (just two edges per axis, since 4:2:0
  chroma is half-resolution).

The chroma strong filter (`bS = 4`) is narrower than luma —
it only rewrites `p0` and `q0` while luma's also rewrites
`p1` / `q1` / `p2` / `q2`.  This is per spec § 8.7.3.3.

---

## 9. DPB and POC

The Decoded Picture Buffer (`dpb.rs`) holds reconstructed
pictures.  Each `DpbEntry` tracks:

- The frame itself.
- POC (picture order count — the display-order index).
- `frame_num` (the bitstream's short-term reference id).
- Whether it's a short-term ref, long-term ref, or evictable.

### POC computation (`pic_order_cnt_type` 0, 1, 2)

The spec has three POC schemes (§ 8.2.1):

- **Type 0** — explicit LSB.  The bitstream sends a low-bits
  value; the decoder tracks the high bits across frames and
  rolls them over when the LSB wraps.
- **Type 1** — delta-cycle.  The SPS encodes a cycle of
  offsets; each frame applies a delta against the previous.
- **Type 2** — `2 * frame_num` for reference pictures,
  `2 * frame_num - 1` for non-references.  Simplest; used in
  baseline + constrained-baseline streams.

All three live in `pipeline.rs::Decoder::compute_poc_*`.

### Reference picture lists

When a slice starts, the decoder builds RefPicList0 (and L1 for
B slices) by sorting DPB entries per spec § 8.2.4:

- L0: short-term refs sorted by descending `frame_num`, then
  long-term refs by ascending `long_term_idx`.
- L1: short-term refs with `POC > current_poc` ascending, then
  refs with `POC < current_poc` descending, then long-term.

`build_ref_pic_list_l0` and `build_ref_pic_list_l1` on
`Decoder` produce these.

---

## 10. Reading the commits

The branch history reads better in topical groups than
chronologically.  If you want to understand a specific
subsystem, here's where to look:

| Topic | Start at file → then commits |
|---|---|
| Bit reader + RBSP | `bit_reader.rs`, `rbsp.rs` — earliest commits |
| SPS / PPS / slice header parsing | `sps.rs`, `pps.rs`, `slice_header.rs`, `vui.rs`, `scaling_list.rs` |
| CAVLC | `cavlc_tables.rs` (the tables) then `cavlc.rs` |
| CABAC core | `cabac_tables.rs` → `cabac_init_tables.rs` → `cabac.rs` |
| CABAC syntax | `cabac_syntax.rs` → `cabac_residual.rs` → `cabac_mb.rs` |
| CABAC inter | `cabac_inter.rs` (tables) → `cabac_inter_mb.rs` (P) → `cabac_inter_b.rs` (B) |
| Intra prediction | `intra_mode.rs` + `intra_pred.rs` + `reconstruct_intra_cabac.rs` |
| Inter reconstruction | `motion.rs` + `mv_pred.rs` + `reconstruct_inter.rs` |
| Slice walkers | `slice_cabac.rs`, `slice_cavlc.rs` |
| Deblocking | `deblock.rs` → `deblock_frame.rs` → `deblock_frame_chroma.rs` |
| Top-level driver | `pipeline.rs` |
| Conformance tests | `bitstream_roundtrip.rs`, `integration.rs` |

Each source file has a module-level doc comment that explains
its scope.  The function-level doc comments cite the relevant
spec clause / table number; pair them with the `H.264 (03/2010)`
PDF if you want to verify a specific procedure.

---

## 11. What this branch isn't

It isn't a production decoder.  It can't compete with x264 +
ffmpeg on conformance vectors.  Specific known gaps:

- B-slice CAVLC walks but doesn't decode.
- High-profile features (8×8 transform, custom scaling lists,
  4:2:2 / 4:4:4 chroma, MBAFF / field coding, SP/SI slices) are
  out of scope.
- No JVT-AVC conformance harness.  The conformance suite
  consists of synthetic round-trip tests — these prove the
  encoder + decoder match each other, not that the decoder
  matches the spec on real-world bitstreams.

What it *is* is a complete-enough scaffolding that every
spec-defined syntax element has a parser, every reconstruction
step has a function, every slice type has a walker, and the
top-level driver wires them together end-to-end.  The remaining
work is bug-fixing against a conformance harness, plus picking
off specific features the scope deliberately skipped.

---

## 12. Where to go from here

If you want to extend the decoder:

- **B CAVLC** — extend `MbType` with the full B-slice variants
  (Table 7-14 mapping) and add `read_b_mb_motion` in
  `macroblock.rs`.  Then update `parse_slice_cavlc` to dispatch.
- **`ref_pic_list_modification`** — the ops are already in
  `SliceHeader.ref_pic_list_modification_lN`.  Apply them inside
  `build_ref_pic_list_l0` / `_l1`.
- **Weighted prediction** — `pred_weight_table` is parsed.
  Apply the weights inside `reconstruct_inter_*_mb` before the
  clip.
- **Conformance vectors** — pull a few `.h264` files from the
  JVT-AVC conformance suite into `tests/data/` and write a test
  that compares decoded YUV against the reference output.

If you want to understand what the spec actually requires:

- **§ 7.3** — Bitstream syntax tables.  Match each one against
  a parser in `sps.rs` / `pps.rs` / `slice_header.rs` /
  `macroblock.rs`.
- **§ 8** — Decoding processes.  § 8.3 is intra prediction;
  § 8.4 is inter prediction; § 8.5 is transform/dequant;
  § 8.7 is deblocking.
- **§ 9** — Parsing process.  § 9.2 is CAVLC; § 9.3 is CABAC.

Pair each section with the matching source file and read them
together.

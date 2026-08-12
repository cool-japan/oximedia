# VP9 inter fixtures (track package P4, corrected by P4-VERIFY)

Seven real `ffmpeg`/`libvpx-vp9`-encoded multi-frame streams, split into
their raw per-packet payloads, each with libvpx's own reconstruction of
every *shown* frame and a real header-level layout dump, **plus three more
real streams imported from a sibling package and one small, fully
hand-built exception** (`p9sef` — see below) added by the P4-VERIFY
correction pass. With that one named exception, nothing here was
hand-authored, hand-edited, or produced by this decoder: every coded byte
of the other nine streams comes from `ffmpeg`/`libvpx`, every `.ref.yuv`
is what **libvpx itself** (or, for `scaled`, `vpxdec` — see "Sibling
import" — still real libvpx, not this decoder) decoded from those same
bytes (cross-checked against ffmpeg's independent native `vp9` decoder
before being committed, for the seven native streams), and every layout
row comes from parsing the committed bytes with this crate's own real
`UncompressedHeader` parser (`src/vp9/uncompressed.rs`) plus `Superframe`
(`src/vp9/superframe.rs`) — the same code the crate ships.

**The one exception, named precisely**: `p9sef.frame1.bin` is one
hand-constructed byte (not encoder output) — see the dedicated "`p9sef`"
section below, which spells out every bit and is the only place in this
document describing bytes not produced by `ffmpeg`/`libvpx`.
`p9sef.frame0.bin` is not hand-authored itself (it is a byte-identical copy
of `p9still.frame0.bin`, real encoder output — only its *use* alongside a
hand-built `frame1` is new), and `p9sef.ref.yuv` is real libvpx-decoded
bytes (`p9still.ref.yuv`'s first frame) reused twice, not a fresh decode of
anything.

**P4-VERIFY correction (see "P4-VERIFY correction pass" below, before the
checksums section, for the full story)**: the original P4 package encoded
all seven streams without an explicit `-frame-parallel` flag. ffmpeg's
libvpx-vp9 wrapper defaults that to `"auto"`, which resolved to
`frame_parallel_decoding_mode=1` on every frame of six of the seven streams
— per spec (`refresh_probs()`, 6.1.2) this disables ALL backward
probability adaptation, so those six fixtures could not exercise the
adaptation machinery they were meant to cover. `p9still`, `p9basic`,
`p9tc`, `p9hp`, `p9seg`, and `p9alt` were re-encoded with `-frame-parallel
0` added (same commands otherwise) and every `.frameN.bin` / `.ref.yuv` /
`.layout.txt` / checksum below reflects the corrected bytes. `p9er` was
**not** touched: it is `error_resilient` on every frame by construction,
which per spec forces `frame_parallel_decoding_mode=1` structurally
(`uncompressed.rs`: `if header.error_resilient { ... frame_parallel_decoding
= true; }`) — not a separate encoder-default issue, and exactly the
"no adaptation" behavior `p9er` exists to demonstrate. Every `.layout.txt`
in this package (all seven streams) also gained two columns,
`refresh_frame_context` and `fpdm`, inserted right after `error_resilient`
— see "Layout derivation" below.

This crate does not implement inter-frame *reconstruction* yet (see
`decode_keyframe`'s doc comment in `../mod.rs`); this package is fixtures
and a verification harness only, no decoder change. `../testutil.rs`'s
`assert_bit_exact_sequence` walks every frame of a sequence for real
(superframe unpack, `frame_size_with_refs` resolution,
`refresh_frame_flags` bookkeeping) and pixel-verifies whichever frames this
crate can currently decode (today: frame 0 of every stream, always the key
frame). `../inter_fixture_tests.rs` is what actually exercises these
fixtures.

## Files

| pattern | what it is |
|---|---|
| `<name>.frameN.bin` | coded packet `N` of `<name>`, **raw VP9 payload only** — no IVF/container framing. May be a superframe (kept as one blob; `p9alt.frame1.bin` is one). Real encoder output for every stream except `p9sef.frame1.bin`, one hand-constructed byte — see "`p9sef`" below |
| `<name>.ref.yuv` | libvpx's reconstruction of every **shown** frame of `<name>`, concatenated, planar YUV 4:2:0, no headers (heterogeneous per-frame dimensions for `scaled` — see "Sibling import" below) |
| `<name>.layout.txt` | one row per coded *sub-frame* (a superframe contributes more than one row for its one `.bin`) of every real header field this package was asked to record, parsed from the exact committed bytes |

**Convention exception**: the three streams under "Sibling import" below
(`compound`/`scaled`/`switch`) use a *different* `.frameN.bin` numbering —
decode order across the whole stream, with superframes already unpacked
and their index bytes discarded, rather than one file per raw IVF packet.
`compound.frame1.bin`/`compound.frame2.bin` are therefore two plain
(non-superframe) VP9 frames, not this package's own one-file-per-packet
convention that keeps a superframe as a single blob (`p9alt.frame1.bin`).
See "Sibling import" for exactly why and what that costs.

Naming note: the pre-existing VP9 **key-frame** fixtures in this directory
(`kf76x42`, `kf128`, `kf64ll`, `kf512tc`, and the lone `seq76x42.frame1.bin`
used by `uncompressed.rs`'s own tests) use `ref<name>.yuv`; this package's
streams use `<name>.ref.yuv` (`vp8/dec/testdata`'s convention: the
`<stream>.<role>.<ext>` shape groups a stream's files together in a
directory listing, and keeps the checked-in name identical to the name the
file was generated and sha256'd under). Neither the four `kf*.frame0.bin` /
`ref*.yuv` pairs nor `seq76x42.frame1.bin` were touched by this package.

## The streams

| stream | dims | coded/shown | flags that make it interesting |
|---|---|---|---|
| `p9still` | 76x42 | 8/8 | frozen source (`loop` filter replays source frame 0 for the whole clip): every inter frame is near-all-skip/ZEROMV (16-byte payloads) |
| `p9basic` | 76x42 | 8/8 | moving `testsrc2`: ordinary NEAREST/NEAR/ZERO/NEWMV + probability-adaptation mix |
| `p9er` | 76x42 | 8/8 | `error_resilient` (`error_resilient_mode` bit `true`) on **every** frame including the key frame — no backward probability adaptation |
| `p9tc` | 512x64 | 5/5 | `tile_cols_log2 == 1` (2 tile columns) on every frame |
| `p9hp` | 76x42 | 8/8 | `-crf 8`: the lowest quantizer depth in this set — see the `allow_high_precision_mv` note below for what this flag does *not* uniquely isolate |
| `p9seg` | 76x42 | 8/8 | `aq-mode 1`: `seg.enabled == true` on **every** frame (key and inter), with real per-segment `ALT_Q` data established on the key frame; every inter frame re-codes `enabled` but leaves `update_map`/`update_data` `false`, inheriting the map/features rather than retransmitting them — the segmentation-inheritance path a later package (P14) needs |
| `p9alt` | 128x128 | 8/9, 8 shown | 2-pass `-auto-alt-ref 1 -lag-in-frames 16`: a genuine **hidden** ALTREF sub-frame packed into a real superframe (`frame1`), plus `ref_frame_sign_bias[ALTREF] == true` on every frame that references it — see `p9alt.layout.txt` |

`p9scale` (a fixture exercising `frame_size_with_refs`' *size-copy* branch —
a mid-sequence resolution change) was attempted and honestly skipped; see
[Skipped: p9scale](#skipped-p9scale) below. **This gap is now filled**: the
`scaled` stream imported from a sibling package (see "Sibling import"
below) exercises exactly this branch — real coded resolution change at
frame 6, `found_ref` legitimately failing against the old (wrong-size)
reference slots.

**`p9seg`'s numbers, checked rather than assumed**: the task's field list
for `.layout.txt` does not include `hdr.seg.*`, so this was checked with a
throwaway probe (parse every `p9seg.frameN.bin` and print `seg.enabled` /
`seg.update_map` / `seg.update_data` / `seg.feature_enabled`, then discard
the probe) rather than left as an assumption from the `-aq-mode 1` flag
alone. The key frame carries `update_map == update_data == true` with
`ALT_Q` enabled on segments 0, 1, 2 and 4; every inter frame carries
`enabled == true`, `update_map == update_data == false`.

**`allow_high_precision_mv` does not distinguish `p9hp` from the other
streams**: every inter frame across all seven streams — including `p9still`
and `p9basic` at `-crf 32` — codes `allow_high_precision_mv == true` (see
any `.layout.txt`). This is plausibly libvpx's quantizer-index threshold
for the flag being generous enough that even `-crf 32` clears it on these
tiny, simple `testsrc2` sources; it was not chased further since it is a
single-bit flag, not a claim this package makes about `p9hp`'s purpose —
`p9hp` still stands as the lowest-quantizer-depth stream in this set, which
is the more consequential difference (finer coefficient magnitudes, not
just this one flag) for whatever exercises it next.

## Tooling

- `ffmpeg 7.1.1` (Homebrew, `/opt/homebrew/bin/ffmpeg`, `--enable-libvpx
  --enable-gpl`, `libavcodec 61.19.101`) with **`libvpx 1.15.2`** (from the
  encoder's own startup log line, `[libvpx-vp9 @ ...] v1.15.2`) for both the
  encoder and the `-c:v libvpx-vp9` decoder; ffmpeg's independent native
  `vp9` decoder (same ffmpeg build) for the cross-check.
- `python3 3.14.6` for the IVF split/reassemble (pure container framing, no
  codec logic — see [Splitting](#splitting-and-reassembling)).
- macOS 26.6.1 (Darwin 25.6.0), arm64. `vpxenc`/`vpxdec` are not installed
  on the generating machine (`which vpxenc vpxdec` empty; no Homebrew `vpx`
  formula), so everything goes through ffmpeg's libvpx wrapper.

All encodes use `-threads 1 -row-mt 0` for determinism, `-pix_fmt yuv420p`
source, and no B-frame reordering beyond VP9's own ALTREF mechanism.

## Encode

**`-frame-parallel 0` is REQUIRED, not cosmetic** (see "P4-VERIFY correction
pass" below): ffmpeg's libvpx-vp9 `-frame-parallel` defaults to `"auto"`,
which does not reliably resolve to `0`. Every command below includes it
explicitly; the six non-`p9er` streams did not originally (that was the
bug this correction pass fixed).

```
BASE=(-c:v libvpx-vp9 -crf 32 -b:v 0 -g 100 -lag-in-frames 0 -auto-alt-ref 0 \
      -error-resilient 0 -tile-columns 0 -aq-mode 0 -threads 1 -row-mt 0 \
      -frame-parallel 0)

# p9still -- loop filter freezes the source on testsrc2's own frame 0
ffmpeg -f lavfi \
  -i "testsrc2=size=76x42:rate=25,format=yuv420p,loop=loop=-1:size=1:start=0" \
  -frames:v 8 "${BASE[@]}" -f ivf p9still.ivf

# p9basic
ffmpeg -f lavfi -i "testsrc2=size=76x42:rate=25,format=yuv420p" \
  -frames:v 8 "${BASE[@]}" -f ivf p9basic.ivf

# p9er (only -error-resilient differs from BASE; -frame-parallel 0 is a
# no-op here since error_resilient_mode=1 already forces
# frame_parallel_decoding_mode=1 structurally -- included anyway for
# command-line consistency with the other six, not because it changes
# anything for this stream)
ffmpeg -f lavfi -i "testsrc2=size=76x42:rate=25,format=yuv420p" \
  -frames:v 8 -c:v libvpx-vp9 -crf 32 -b:v 0 -g 100 -lag-in-frames 0 \
  -auto-alt-ref 0 -error-resilient 1 -tile-columns 0 -aq-mode 0 \
  -threads 1 -row-mt 0 -frame-parallel 0 -f ivf p9er.ivf

# p9tc (only -tile-columns differs; 5 source frames -- see "Size lever" below)
ffmpeg -f lavfi -i "testsrc2=size=512x64:rate=25,format=yuv420p" \
  -frames:v 5 -c:v libvpx-vp9 -crf 32 -b:v 0 -g 100 -lag-in-frames 0 \
  -auto-alt-ref 0 -error-resilient 0 -tile-columns 1 -aq-mode 0 \
  -threads 1 -row-mt 0 -frame-parallel 0 -f ivf p9tc.ivf

# p9hp (only -crf differs)
ffmpeg -f lavfi -i "testsrc2=size=76x42:rate=25,format=yuv420p" \
  -frames:v 8 -c:v libvpx-vp9 -crf 8 -b:v 0 -g 100 -lag-in-frames 0 \
  -auto-alt-ref 0 -error-resilient 0 -tile-columns 0 -aq-mode 0 \
  -threads 1 -row-mt 0 -frame-parallel 0 -f ivf p9hp.ivf

# p9seg (only -aq-mode differs)
ffmpeg -f lavfi -i "testsrc2=size=76x42:rate=25,format=yuv420p" \
  -frames:v 8 -c:v libvpx-vp9 -crf 32 -b:v 0 -g 100 -lag-in-frames 0 \
  -auto-alt-ref 0 -error-resilient 0 -tile-columns 0 -aq-mode 1 \
  -threads 1 -row-mt 0 -frame-parallel 0 -f ivf p9seg.ivf

# p9alt -- 2-pass (see "Why 2-pass" below), 20 source frames, then trimmed
ffmpeg -f lavfi -i "testsrc2=size=128x128:rate=25,format=yuv420p" -frames:v 20 \
  -c:v libvpx-vp9 -auto-alt-ref 1 -lag-in-frames 16 -g 100 -crf 32 -b:v 0 \
  -error-resilient 0 -tile-columns 0 -aq-mode 0 -threads 1 -row-mt 0 \
  -frame-parallel 0 -pass 1 -passlogfile <scratch>/p9alt2pass -f null -
ffmpeg -f lavfi -i "testsrc2=size=128x128:rate=25,format=yuv420p" -frames:v 20 \
  -c:v libvpx-vp9 -auto-alt-ref 1 -lag-in-frames 16 -g 100 -crf 32 -b:v 0 \
  -error-resilient 0 -tile-columns 0 -aq-mode 0 -threads 1 -row-mt 0 \
  -frame-parallel 0 -pass 2 -passlogfile <scratch>/p9alt2pass -f ivf p9alt_2p.ivf
# p9alt_2p.ivf has 20 coded packets; frame1 and frame13 are real superframes
# (hidden ALTREF + the visible frame that references it). Packets 0..8 are
# taken as a fresh 8-packet IVF (see "Splitting" below) -- p9alt.ivf --
# which already contains one full ALTREF group (frame1's superframe) plus
# six more inter frames that reference it via a flipped ALTREF sign_bias.
# Re-verified after adding -frame-parallel 0 (not assumed): the hidden
# superframe still lands at packet 1, still sums exactly
# (1853 + 420 + 6-byte index = 2279 bytes, its whole payload) -- the sizes
# themselves changed from the original 1853+411 (see "P4-VERIFY correction
# pass" below), but the structural shape (superframe at packet 1, 8/9
# coded, 8 shown) did not.
```

### Why 2-pass for `p9alt`

ffmpeg's own `-h encoder=libvpx-vp9` documents `-auto-alt-ref` as
"2-pass only", and this was confirmed empirically, not just taken on faith:
a 1-pass encode of 24 source frames at `-auto-alt-ref 1 -lag-in-frames 25`
(otherwise identical settings) produced 24 coded packets with **no** hidden
frame and **no** superframe anywhere (`ref_frame_sign_bias[ALTREF]` false
on every single frame, verified by parsing every packet with this crate's
real header parser). The 2-pass encode above, run at the smaller
`-lag-in-frames 16` that `vp8/dec/testdata`'s own `refswap` fixture found
sufficient, produced the hidden ALTREF on the very first attempt.

### Size lever

`p9tc` at 512x64 dominates the byte budget (`w*h*1.5` per frame is over 6x
every other stream's), so it is encoded directly at 5 source frames (1 key
+ 4 inter — the task's stated floor) rather than 8; every other stream
encodes its full 8-frame commit directly. `p9alt` needs the long 20-frame,
2-pass encode purely to give the ALTREF lookahead window somewhere to work;
only the first 8 of its 20 coded packets are kept.

## Reference decode — both VP9 decoders ffmpeg exposes, verified not assumed

```
ffmpeg -y -c:v libvpx-vp9 -i <name>.ivf -fps_mode passthrough \
  -pix_fmt yuv420p -f rawvideo <name>.libvpx.yuv
ffmpeg -y -c:v vp9       -i <name>.ivf -fps_mode passthrough \
  -pix_fmt yuv420p -f rawvideo <name>.native.yuv
cmp <name>.libvpx.yuv <name>.native.yuv   # must be identical -- and was, for all 7 streams
```

`-c:v libvpx-vp9` / `-c:v vp9` before `-i` select the decoder, not just the
demuxer; for every one of the seven streams this was confirmed from
ffmpeg's own `-loglevel verbose` stream-mapping line (`vp9 (libvpx-vp9) ->
rawvideo` vs. `vp9 (native) -> rawvideo`), the same discipline
`vp8/dec/testdata/README.md` used. `<name>.libvpx.yuv` is what gets
committed as `<name>.ref.yuv`; `<name>.native.yuv` is discarded once `cmp`
confirms it is identical (no point shipping two copies of the same bytes).

`-fps_mode passthrough` is carried over from the same VP8 fixture set's
lesson (a hidden frame leaves a presentation-timestamp gap that the
rawvideo muxer will otherwise fill by duplicating a picture) — applied here
proactively rather than rediscovered the hard way, and it *is* load-bearing
for `p9alt` specifically (one hidden ALTREF sub-frame).

Cross-decoder agreement, with byte counts (`shown_frames * w*h*3/2`):

| stream | libvpx.yuv / native.yuv bytes | shown frames |
|---|---|---|
| `p9still` | 38304 | 8 |
| `p9basic` | 38304 | 8 |
| `p9er` | 38304 | 8 |
| `p9tc` | 245760 | 5 |
| `p9hp` | 38304 | 8 |
| `p9seg` | 38304 | 8 |
| `p9alt` | 196608 | 8 (of 9 coded sub-frames — `frame1`'s ALTREF sub-frame is hidden) |

## Splitting and reassembling

IVF is a fixed, well-documented container (no codec logic): a 32-byte file
header (`"DKIF"`, `u16` version, `u16` header length, 4-byte fourcc, `u16`
width, `u16` height, `u32` rate, `u32` scale, `u32` frame count, `u32`
unused), then per packet a 12-byte header (`u32le` size, `u64le` pts)
followed by `size` bytes of payload. A throwaway `python3` script (kept with
this generation session, not part of the repo — same posture
`vp8/dec/testdata/README.md`'s `split_ivf.py` takes) parses this and:

1. **splits** an `.ivf` into `<name>.frameN.bin` — each packet's payload
   verbatim, no framing;
2. **reassembles** a prefix of N packets from an `.ivf` (or from a fresh
   list of payload files) back into a standalone, valid `.ivf`, reusing the
   original per-packet `pts` values rather than inventing new ones.

Reassembly is what makes `p9alt`'s prefix trim honest instead of a
guess: the golden for `p9alt` was decoded from the **reassembled 8-packet
IVF that only contains the bytes actually committed**, not from the
untrimmed 20-packet encode — decoding the longer stream and then discarding
12 packets' worth of its `ref.yuv` tail would have desynchronized the
frame-accounting the moment the packet count changed. Truncating a VP9
stream to a coded-order prefix is itself sound: decode is strictly causal
(the `frame_size_with_refs`/`refresh_frame_flags`/ALTREF-reference state
any of packets 0..7 needs is entirely produced by packets 0..7 themselves),
so `p9alt.ivf`'s golden is exactly what a real decoder would reconstruct
for those 8 packets whether or not 12 more packets existed after them.

For the six streams that were *not* trimmed (`p9still`, `p9basic`, `p9er`,
`p9tc`, `p9hp`, `p9seg`), the same reassemble step was still run as a
splitter self-check — `cmp`-identical to the original `.ivf` in every case
— which is the round-trip verification `vp8/dec/testdata/README.md`
performed as a separate step; here it falls out of the same code path used
for `p9alt`'s real trim.

## Layout derivation

Each `<name>.layout.txt` was originally produced by a temporary `#[ignore]`d
`#[test]` inside `vp9/dec/mod.rs` (deleted once every stream's layout was
written — nothing temporary remains in the crate), driven by env vars so
one compiled test binary could be rerun per stream without a rebuild. It
parsed every committed `.frameN.bin` with this crate's real
`Superframe::parse` + `UncompressedHeader::parse_with_ref_sizes`, carrying
the same `ref_sizes: [Option<(u32, u32)>; 8]` bookkeeping across frames
that a real decoder's DPB would. The equivalent walk now lives permanently
in `../testutil.rs`'s `assert_bit_exact_sequence`, which is what actually
gates these fixtures in CI (`../inter_fixture_tests.rs`) — the temporary
test was development-only scaffolding for producing the *documentation*
file, not a second source of truth.

**P4-VERIFY (the `-frame-parallel 0` correction pass)** regenerated every
`.layout.txt` in this package (all seven streams) the same way in spirit,
but via a temporary `examples/vp9_layout_dump.rs` in the crate root
(`cargo run --example vp9_layout_dump --features vp9 -- <name> <n_frames>
[dir]`, deleted once this pass finished — same "development-only
scaffolding, not a second source of truth" posture as the original
`#[ignore]`d test) rather than a test-module helper, since it needed to be
pointed at an out-of-tree sibling fixture directory too (see "Sibling
import" below) without copying anything into the crate first. Same parsing
code path (`Superframe::parse` + `UncompressedHeader::parse_with_ref_sizes`
with threaded `ref_sizes`), same non-source-of-truth relationship to
`testutil.rs`.

Column meaning (`-` where the field is not coded and hence not meaningful,
e.g. every field but `frame_to_show` on a `SHOW_EXISTING` row — see
`p9sef.layout.txt`, added by the same correction pass, for the one stream
in this package that actually has such a row):

```
frame_idx sub_idx frame_type show_frame refresh_frame_flags
ref_frame_idx[L,G,A] sign_bias[L,G,A] interp_filter(raw)
allow_high_precision_mv error_resilient refresh_frame_context fpdm
frame_context_idx reset_frame_context tile_cols_log2 tile_rows_log2 width
height frame_to_show
```

`refresh_frame_context` and `fpdm` (`frame_parallel_decoding_mode`) are the
two columns P4-VERIFY added, inserted right after `error_resilient` to
match the bitstream read order of the three probability-adaptation fields
(`uncompressed.rs`: `error_resilient` gates whether the other two are even
coded at all; when it is `true`, `refresh_frame_context` reads back
`false` and `fpdm` reads back `true` without consuming any bits, exactly
`p9er`'s pattern below). Backward probability adaptation
(`refresh_probs()`, spec 6.1.2) is active on a frame if and only if
`error_resilient == false && fpdm == false`.

`interp_filter(raw)` is the **raw 2-bit literal as coded** (or `4` for
"switchable"), not translated through libvpx's `literal_to_filter`
permutation — this package records only what `uncompressed.rs` itself
exposes, and does not assert a mapping this package did not verify.
`sign_bias[L,G,A]` is `ref_frame_sign_bias[1..=3]` (index 0, INTRA, is
always unused/false). `frame_idx`/`sub_idx` address a `.bin` file and its
position within it if it unpacks to more than one frame (only
`p9alt.frame1.bin` does, `sub_idx` 0 and 1).

Two things worth reading straight out of `p9alt.layout.txt` rather than
taking on faith: `frame1 sub 0` has `show_frame=false` and
`refresh_frame_flags=0x04` (the hidden ALTREF, landing in DPB slot 2), and
`frame1 sub 1` through `frame7` all carry `sign_bias=[false,false,true]` —
the ALTREF slot's backward sign bias that marks it as a genuine "future,
relative to display order" reference, exactly what `-auto-alt-ref` is
meant to produce.

## Skipped: p9scale

A fixture exercising `frame_size_with_refs`' *size-copy* branch (a
mid-sequence resolution change, so a later frame's dimensions come from a
differently-sized reference slot rather than its own bitstream — the branch
`parse_frame_size_with_refs` in `uncompressed.rs` already implements and
`uncompressed.rs`'s own tests already exercise with hand-built ref-size
tables, but that no *real encoder-produced* bitstream in this repository's
fixtures exercises) was in scope for this package and was attempted, then
honestly skipped rather than fabricated:

- `ffmpeg -h encoder=libvpx-vp9` (this ffmpeg build, `libavcodec 61.19.101`)
  was checked in full for every AVOption it exposes; none of them
  (`-h encoder=libvpx-vp9 | grep -i 'resize\|scale'` matches nothing but the
  unrelated `libswscale` banner line) control libvpx's spatial-resize
  mechanism (`rc_resize_allowed` / what `vpxenc --resize-mode` sets).
- The 1-pass, `-auto-alt-ref`-less flag sets used for the other six streams
  never triggered a resolution change either (every `.layout.txt` in this
  directory shows a constant `width`/`height` across its whole sequence —
  checked, not assumed).
- The tool that legitimately does this is libvpx's own `vpxenc
  --resize-mode=1 ...`; `vpxenc`/`vpxdec` are not installed on this machine
  (see [Tooling](#tooling)). A `vpxenc` binary was visible in a *different*,
  concurrently-running Claude Code session's scratch directory for an
  unrelated project; it was deliberately not used, since depending on
  another session's private, mid-build artifact would make this recipe's
  provenance unverifiable and non-reproducible by anyone reading this file.
- Given ffmpeg's wrapper exposes no way to request `rc_resize_allowed`,
  no bitrate/quality setting can trigger it through ffmpeg either — this
  was not spot-tried further, since the discriminating fact (the option
  doesn't exist in this wrapper) was already established.

`p9scale` is therefore not present in this directory. Producing it would
need a `vpxenc`-based (or direct libvpx-API-based) toolchain, which this
package's environment does not have. **P4-VERIFY later filled this gap by
import rather than by finally getting a `vpxenc` toolchain** — see
"Sibling import" immediately below.

## Sibling import: `compound` / `scaled` / `switch` (P4-VERIFY)

A concurrently-run, separately-scoped GATE-reviewed fixture package
("Wave-3 package V0b" per its own `RECIPE.md`; generated fully offline in
a scratch directory, no repo read or modified while producing it) shipped
five more real `ffmpeg`/`libvpx-vp9` streams: `p5single`, `compound`,
`switch`, `segdelta`, `scaled`. P4-VERIFY imported three of the five —
`compound`, `scaled`, `switch` — because each adds coverage this package's
own seven streams lack; `p5single` and `segdelta` were **not** imported
(see "Why `p5single`/`segdelta` were skipped" below).

| imported stream | dims | coded/shown | why it was worth importing |
|---|---|---|---|
| `compound` | 96x64 | 13/12 (1 hidden) | a **second** real hidden-ARF-in-a-superframe stream (`p9alt` is this package's only other one) with a *different* structural detail: the hidden sub-frame uses `frame_context_idx=1` while every shown frame in the chain uses `0` — a genuine separate probability-context slot, not a parser artifact (see its `.layout.txt`) |
| `scaled` | 96x64 → 64x48 at coded frame 6 | 12/12 | the **only** fixture in this directory (imported or native) exercising `frame_size_with_refs`' size-copy branch against a real encoder-produced mid-stream resolution change — fills the `p9scale` gap this package's own generation honestly could not (see "Skipped: p9scale" above) |
| `switch` | 100x68 | 10/10 | frame-level `interpolation_filter` genuinely varies within one stream (coded frame 1 = `SWITCHABLE`, frames 2-9 = fixed `EIGHTTAP`) on non-8-aligned dimensions (partial superblocks/MI at the right/bottom edges) — no native stream in this package varies its interpolation filter *within* a sequence |

### Convention differences from this package's own streams — read before treating these three as drop-in `p9*` fixtures

1. **`.frameN.bin` numbering is decode order across the whole stream, with
   superframes already unpacked**, not this package's one-file-per-IVF-packet
   convention. Verified directly, not assumed: `compound` ships 13
   `.frameN.bin` files for 12 real IVF packets, and `compound.frame1.bin`
   (1161 bytes) / `compound.frame2.bin` (147 bytes) — the hidden ARF and the
   shown frame that follows it, both originally packed into IVF packet 1 as
   one real superframe — each have their *own* last byte checked directly:
   `0x60` (top 3 bits `011`) and `0x00` (top 3 bits `000`) respectively,
   neither matching the `0b110` superframe-marker shape, confirming the
   superframe index bytes were genuinely stripped at import time, not just
   renamed. **Consequence for `SequenceCheck::coded_frames`**: feeding
   `compound`'s 13 pre-split files into `assert_bit_exact_sequence` as 13
   array entries makes `coded_frames == 13` mean "13 `.frameN.bin` files
   fed in", *not* "13 real transport packets" the way it does for every
   other stream in this suite (where one array entry is one real IVF
   packet, occasionally unpacking to more than one VP9 frame via
   `Superframe::parse`). This is flagged explicitly in the doc comment on
   `COMPOUND_FRAMES` in `inter_fixture_tests.rs`.
2. **`compound` therefore does *not* provide a second confirming real-world
   case for the `Superframe::parse` size-sum fix** (see "Known sharp edge"
   below) — only `p9alt.frame1.bin` (this package's own superframe blob,
   index bytes intact) exercises that code path among this directory's
   fixtures. Reconstructing a genuine second blob would need reading
   `compound.ivf` from the sibling scratchpad (not committed here per the
   import brief's "frameN.bin + ref.yuv + layout.txt, NOT .ivf" scope) and
   hand-deriving the true packet boundary; judged not worth the added
   complexity and transcription risk for a property `p9alt` already
   verifies, so this was not pursued.
3. **`scaled.ref.yuv` is heterogeneous**: 82944 bytes total, the first
   55296 bytes (6 × 9216) at 96x64 and the remaining 27648 bytes (6 × 4608)
   at 64x48 — see the sibling `RECIPE.md`'s byte-offset table, reproduced
   in full in the "Provenance" subsection below. A reader (or test) using a
   fixed `frame_index * constant_stride` formula on this file will get
   nonsense from output picture 6 onward. `scaled.layout.txt`'s per-frame
   `width`/`height` columns (or a header re-parse) are the source of truth
   for where each frame's bytes start.
4. **`scaled.ref.yuv` was decoded with `vpxdec`, not ffmpeg** — the
   sibling's own finding was that ffmpeg's `-f rawvideo` muxer silently
   re-normalizes every output frame to the *first* frame's dimensions
   (96x64) for a genuinely variable-resolution decode, producing a
   wrong-but-plausible-looking 12×9216-byte file instead of the correct
   82944-byte heterogeneous layout. `vpxdec` (real libvpx, no
   format-negotiation muxer in the way) does not have this problem. Full
   evidence is in the sibling `RECIPE.md`'s "`scaled`: ffmpeg's rawvideo
   output is NOT trustworthy" section, reproduced in "Provenance" below —
   this is the one file in this whole directory (native or imported) whose
   golden was **not** produced by the `ffmpeg -c:v libvpx-vp9 ... -f
   rawvideo` command every other `.ref.yuv` here uses.
5. **`switch`'s `fpdm` is `1` (adaptation inactive) on every frame** — this
   is *not* a repeat of the bug this package's own `-frame-parallel 0`
   correction pass fixed. `switch` was encoded with ffmpeg's defaults
   specifically to let `interpolation_filter` vary freely (its whole
   purpose); its correctness does not depend on backward adaptation being
   active, and the sibling package's own review confirmed this deliberately
   (see its `RECIPE.md`). Confirmed directly against the imported
   `switch.layout.txt` / by re-parsing with this crate's own parser (see
   "Validation performed on import" below): `fpdm=true` on all 10 frames.

### Why `p5single`/`segdelta` were skipped

Both were in scope to import and the size budget did not require skipping
them (see "Size budget" below), but neither adds coverage this package's
own native streams lack: `p5single` (single-reference LAST-only inter
prediction) covers the same ground as `p9basic`/`p9still`, and `segdelta`
(per-segment quantizer deltas via `aq-mode 1`) covers the same ground as
`p9seg` (both establish real `ALT_Q` segment data on the key frame and
inherit it, `update_map=update_data=false`, on every inter frame). Neither
was fabricated or degraded — they simply were not copied in.

### Validation performed on import

1. **Parser acceptance, before copying anything**: `examples/vp9_layout_dump.rs`
   (see "Layout derivation" above) was pointed directly at the sibling's
   scratch directory — no files copied into this crate yet — and run
   against all 13 `compound`, all 12 `scaled`, and all 10 `switch`
   `.frameN.bin` files. Zero parse errors across all 35 payloads.
2. **`scaled`'s per-frame dimensions cross-checked**: the crate's own
   parser output (frames 0-5 at 96x64, frames 6-11 at 64x48) was diffed
   against the sibling `RECIPE.md`'s independently-derived table (its own
   coded-index -> dims mapping) — exact match.
3. **Copy integrity, both ways**: every copied file's sha256 was
   recomputed after copying and diffed both (a) against the same file
   re-hashed at its sibling-scratch source path, and (b) against the
   sibling `RECIPE.md`'s own checksums section — identical in all 41 cases
   (13+13+2 `compound` files, 12+2 `scaled`, 10+2 `switch` — `frameN.bin` +
   `ref.yuv` + `layout.txt` per stream) both ways. The `.ivf` files were
   deliberately not copied (out of scope per the import brief) and are the
   only sibling-committed files this directory does not also hash-match.
4. **Real end-to-end test**: `cargo test -p oximedia-codec --features vp9`
   — see `inter_fixture_tests.rs`. `compound` and `scaled` pass cleanly.
   **`switch` found a real decoder bug** and its test is `#[ignore]`d
   rather than left failing or weakened: `switch` (100x68) is this crate's
   first committed fixture with a *partial* last 64x64-superblock row
   (`mi_rows=9`, `sb64_rows=ceil(9/8)=2`, only 1 real MI row in the second
   superblock row), and decoding its key frame panics in `pred.rs`/
   `recon.rs` — a plane-buffer write one row past the allocated height, not
   a fixture or header-parsing problem (the header parses fine; the crash
   is in pixel reconstruction). Full diagnosis — exact panic, byte-offset
   arithmetic pinning it to the row direction, and why no other fixture in
   this crate (native or imported) happens to exercise this — is in the
   doc comment on `switch_frame0_key_bit_exact_vs_libvpx` in
   `inter_fixture_tests.rs`. Out of this package's scope to fix (`recon.rs`/
   `pred.rs` belong to a different package, and a concurrent sibling
   session was actively editing that decode pipeline while this package
   ran); flagged here and there for whoever owns it next.

### Provenance: verbatim encode commands (from the sibling package's own `RECIPE.md`)

Tool versions: `ffmpeg 7.1.1` (Homebrew, `--enable-libvpx --enable-gpl`,
`libavcodec 61.19.101`) with **libvpx 1.15.2** for the encoder — the exact
same `ffmpeg`/`libvpx` build this package's own native streams use (see
"Tooling" above) — plus a from-source `vpxenc`/`vpxdec` build (libvpx
v1.15.2 git tag, commit `d168454`) that this package's own environment does
not have, used only for `scaled`'s reference decode and its dedicated C
encoder (see below).

```
# compound -- 2-pass, hidden ARF; -frame-parallel 0 REQUIRED (see the
# sibling RECIPE.md's own review-caught bug: without it, fpdm=1 on every
# frame and compound's whole adaptation purpose is silently defeated)
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 12 \
  -c:v libvpx-vp9 -auto-alt-ref 1 -lag-in-frames 16 -g 250 -keyint_min 250 -cpu-used 0 -frame-parallel 0 \
  -qmin 4 -qmax 32 -crf 24 -b:v 200k \
  -pass 1 -passlogfile <scratch>/passlog -f null /dev/null
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 12 \
  -c:v libvpx-vp9 -auto-alt-ref 1 -lag-in-frames 16 -g 250 -keyint_min 250 -cpu-used 0 -frame-parallel 0 \
  -qmin 4 -qmax 32 -crf 24 -b:v 200k \
  -pass 2 -passlogfile <scratch>/passlog -f ivf compound.ivf

# switch -- encoder defaults (no -auto-alt-ref/-tile-columns/-frame-parallel
# overrides), so switchable interpolation filters vary freely
ffmpeg -f lavfi -i "testsrc2=size=100x68:rate=25,format=yuv420p" -frames:v 10 \
  -c:v libvpx-vp9 -g 100 -keyint_min 100 \
  -qmin 4 -qmax 32 -crf 24 -b:v 200k -f ivf switch.ivf

# scaled -- content generation (96x64 for the first 6 frames, genuinely
# downscaled to 64x48 for the last 6), then a dedicated C encoder
# (scaled_encoder.c, this package's own minimal reproduction of libvpx's
# own test/resize_test.cc PreEncodeFrameHook mechanism -- vpx_codec_enc_
# config_set() with new cfg.g_w/cfg.g_h between vpx_codec_encode() calls)
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 12 -f rawvideo scaled_full96x64.yuv
# first 6 frames -> scaled_big.yuv (used as-is, 96x64)
# last 6 frames -> scaled_tail_96x64.yuv, then genuinely downscaled:
ffmpeg -f rawvideo -pix_fmt yuv420p -s 96x64 -i scaled_tail_96x64.yuv \
  -vf scale=64:48 -pix_fmt yuv420p -f rawvideo scaled_small.yuv
clang -O2 -Wall -Wextra -I<libvpx> -I<vp9_vpxbuild> \
  -o scaled_encoder scaled_encoder.c <vp9_vpxbuild>/libvpx.a -lpthread -lm
./scaled_encoder scaled_big.yuv scaled_small.yuv scaled.ivf

# reference decode (compound, switch -- the standard command this
# package's own native streams also use)
ffmpeg -c:v libvpx-vp9 -i <name>.ivf -fps_mode passthrough -pix_fmt yuv420p -f rawvideo <name>.ref.yuv

# reference decode (scaled ONLY -- vpxdec, not ffmpeg; see point 4 above)
vpxdec --codec=vp9 scaled.ivf --rawvideo -o scaled.ref.yuv
```

`scaled.layout.txt`'s byte-offset table (reproduced from the sibling
`RECIPE.md`, describing `scaled.ref.yuv`'s heterogeneous layout):

| output_idx | byte_offset | dims | frame_bytes |
|---:|---:|---|---:|
| 0-5 | `9216 * output_idx` | 96x64 | 9216 each |
| 6-11 | `55296 + 4608 * (output_idx - 6)` | 64x48 | 4608 each |

(full per-index table: 0→0, 1→9216, 2→18432, 3→27648, 4→36864, 5→46080,
6→55296, 7→59904, 8→64512, 9→69120, 10→73728, 11→78336; total 82944 bytes.)

### Size budget

```
compound: 126,329 bytes (13 frameN.bin + ref.yuv + layout.txt, no .ivf)
scaled:   101,124 bytes
switch:   115,325 bytes
imported subtotal: 342,778 bytes
```

This package's own testdata directory was 943,474 bytes just before this
import (after the `-frame-parallel 0` correction pass, before `p9sef`);
943,474 + 342,778 = 1,286,252 bytes, comfortably under the 1.6MB budget
with room left for `p9sef` (a few KB). `p5single`/`segdelta` (83,984 +
104,001 = 187,985 bytes) were left out on coverage grounds (see above), not
because the budget required it.

### sha256 (recomputed on import, into this directory)

```
### compound
efed03dbfd7b0a50c9efd323d15882aed0480706129ea61a65498566e57778af  compound.frame0.bin
836d89abb5a5cada91b440ff57141de9bd2ae6b938b94b5899d25c842d656d24  compound.frame1.bin
697d69300039cd0af6b2d13e4d2f3a8529d3057d832f57844d85fc7231d4e236  compound.frame2.bin
6babb647730c948295d5e3dd1350147ac6347591746b56e36e1ac19214b98726  compound.frame3.bin
fdd3a88eee56e50ebe6ec8e956f19a64a7797b0c798a5216291aa14a7527458a  compound.frame4.bin
6f1dc4f827b10a055b872ff2bb1d5a8ca4264675dde90c750184f3a82306fb57  compound.frame5.bin
d354fdc1415f00b5f17a13298e912cef2db5441639608ff78a731baf5766a3d7  compound.frame6.bin
2a0456e40e5c266687183cf23ca1f740f0fc68d9b5671bb4c796281412d260d1  compound.frame7.bin
eb1415b8074b9553e1526508b88ec2e1b5fab352c4e9da3f382004a1b4bcb30b  compound.frame8.bin
e2d55ae1c2b6f6a57795b2ba76e2c86492f68d2effeb595668218191c2c47561  compound.frame9.bin
6005b0c98d85b657ce5955c79d1b3617d53efeccfb8ce5da186e8d6aba134f2c  compound.frame10.bin
c7378b1bc26617c15682fee5e44e7285f8e4709be8cf6b6332467f7f6ad6abc2  compound.frame11.bin
0c540eb069048b0a8e88357c3063ae2d947c055c595824e3b702d638926399ce  compound.frame12.bin
5b4e91a82ea63a8ffb835640bf38bea391d60aa52c30d0b2c5ed8f9d7ba3352f  compound.ref.yuv
a71088b68cb54e9af561b47d39bf7bfc510d840b9976cc4c2cb45958a4ab1c8d  compound.layout.txt

### scaled
51872bd821cd561680367988649858fcad80afe5804bce5bf4cb07c8ba5a62c7  scaled.frame0.bin
003a2b7129c6e7ce227ad3c81fc299f8c8f790ab53cd988af39fea3dfcd4ff58  scaled.frame1.bin
98774f668c485f940efc9b449782ded4be569290880f5c6a1428f1a6ab9ecd1a  scaled.frame2.bin
65e89f74bc39434e7e0a90b8ca36b3bf80506868d4c6e75b93ced144f046fb03  scaled.frame3.bin
d8a94657effcee658e556f0c5936509171586b1ea0237f06791cc149a58703c1  scaled.frame4.bin
a7f9def5bd38f13ffa5243d49780bb93f8586f45517f2bac25a595dd0cb7a900  scaled.frame5.bin
aab175649c86f7474adb8e4049a77c17ae287a7539872e0cf73b0d10f6deee06  scaled.frame6.bin
ab694f849da5f598e1de36c3d0a7c4a5a61e233ecf5eaf419d99bb2e5ae96802  scaled.frame7.bin
6de35208dd1351725443d6e810b0ea16f30655455d46ddd38dfd8ac7a5efde0a  scaled.frame8.bin
9400020cca0ddd4456836045ec653a6d8f3029bbb2524b87f5020b95d0cc7a13  scaled.frame9.bin
028b5c137dc8be8d758afb031f4d1bec71c7a3a3cd900be49fc69e7bff38bdd7  scaled.frame10.bin
578cb939a858f634d59411a584013ee683d0dbbcd1261dc8cadf8a5aa5f01a29  scaled.frame11.bin
a7cb5ac7cab5e72a274c82b4f459e1bc86fde539d1fe6871ba1e5e92c0fdf63a  scaled.ref.yuv
eed353488eb5929880b9a6cd861f10f0caef47eed56e821f7a7f6ccfd0e77b8c  scaled.layout.txt

### switch
e9ffbfcce0560b2b6bbf83c6c94d535be303e5258d2fc5dffc8048c03d8c6377  switch.frame0.bin
29f56f5def16cb0071abc547ed32b57f1d487329385998a2b09ebd3d3cafa4c2  switch.frame1.bin
c5f799a2c91f3db305e7b1c01b8014a7feb6298693ba0a814af659e1857408d1  switch.frame2.bin
a2fad591bc35387af43e2384ed6c1c2377d4d4af236965bdaacac3b2731da6f4  switch.frame3.bin
fc5090bf6f43d839dce88f6942c91881597c550793bd1a1090eeb355de202c56  switch.frame4.bin
d9a15a63d179b3f93ff07dcb56e89fdebd4f0464f667d1e2b1dab4f62acc0f1a  switch.frame5.bin
69d54e114949c98cbba6f2012099e05b9c492c2682ccfe05923853fc1879645e  switch.frame6.bin
332fac226d39275add52787c014a85e0d9f31a784df8eab7092ebe5f310c386b  switch.frame7.bin
37e664e461e47116f77eb8ec9118d3ed93b93d347b65aa8b9d1c846dae5e6b36  switch.frame8.bin
4e84c18b7f03316911d3cda164d571dfb9a1bd3d622394f6c2e7ebfa867f2039  switch.frame9.bin
4cf9539bc194d4b70ac90efa285244e4159ab536efc88fb49913b99ec0e9bc2c  switch.ref.yuv
94b2b5bab19e1f3fbcd63c126f9e8611e0ee23fd034cc1fbcae1b945be33b126  switch.layout.txt
```

Cross-verified two ways (see "Validation performed on import" above):
against each file re-hashed at its sibling-scratch source path, and
against the sibling package's own `RECIPE.md` checksums section — identical
in all 41 cases.

## `p9sef`: a hand-built `show_existing_frame` fixture (P4-VERIFY)

Neither this package's own seven streams nor the three imported ones
contain a real `show_existing_frame` occurrence (both `RECIPE.md`s
document a bounded, not-exhaustive search that didn't produce one — see
"Known gap" in the sibling's own document). Rather than leave the
`show_existing_frame` parse path (`uncompressed.rs`: the early-return
branch right after `show_existing_frame = reader.read_bit()...`)
completely unexercised by any real bitstream, `p9sef` hand-builds the
smallest possible two-"frame" sequence that exercises it — **the one
deliberate exception to this whole document's "every coded byte comes from
`ffmpeg`/`libvpx`" claim**, named explicitly here and at the top of this
file.

### `p9sef.frame0.bin`: real encoder output, reused verbatim

A byte-identical copy of `p9still.frame0.bin` (post-`-frame-parallel 0`
correction — see "P4-VERIFY correction pass" above), chosen per the task
brief's own suggestion ("p9still (or smallest stream)"). Verified
byte-identical by direct `cmp`, not just by construction:
`af8ef20cb4458ea45876f1ff3567ed68b234310832c2769af374a7c6cc80d6f5` for
both files (see the checksums section below). A real KEY frame,
`refresh_frame_flags=0xff` — every one of the 8 reference slots holds this
picture after it decodes, which is exactly what lets `frame1` legally
request *any* of them.

### `p9sef.frame1.bin`: one hand-built byte, every bit spelled out

`uncompressed_header()` (spec 6.2, and this crate's own
`UncompressedHeader::parse_with_ref_sizes` in `src/vp9/uncompressed.rs`)
reads, for a `show_existing_frame` frame, exactly this sequence before
returning — no compressed header, no `trailing_bits()`, nothing else:

| field | width | spec/code | value chosen | binary |
|---|---:|---|---|---|
| `frame_marker` | 2 bits | must be `0b10` | `0b10` | `10` |
| `profile_low_bit` | 1 bit | `Profile = (high<<1)\|low` | `0` (profile 0) | `0` |
| `profile_high_bit` | 1 bit | (profile 0 ⇒ no `reserved_zero` bit) | `0` (profile 0) | `0` |
| `show_existing_frame` | 1 bit | must be `1` to take this path at all | `1` | `1` |
| `frame_to_show_map_idx` | 3 bits | which of the 8 DPB slots to redisplay | `5` | `101` |

Concatenated MSB-first (this crate's `BitReader` reads MSB-to-LSB within a
byte — `oximedia-io/src/bits/reader.rs`'s own doc comment and doctest
confirm this, not assumed): `10` + `0` + `0` + `1` + `101` = `10001101` =
**`0x8D`**, exactly 8 bits — the entire "frame" is this one byte, which
matches real VP9 encoder behavior for `show_existing_frame` frames (spec:
`header_size_in_bytes = 0` and the function returns immediately after
`frame_to_show_map_idx`; libvpx's own `write_uncompressed_header` returns
right after writing that same field for this case), not merely
"technically sufficient for this parser."

**Why slot 5, not slot 0**: the key frame's `refresh_frame_flags=0xff`
makes every slot 0..=7 an equally legal choice (all were just refreshed by
`frame0`), so slot 0 would have been simpler to justify but is also the
value every field in this byte would read back as under several *wrong*
bit-order or bit-width assumptions (e.g. reading `frame_to_show_map_idx`
as 2 bits instead of 3, or reading the fields LSB-first, could both
coincidentally still produce 0 from an all-zero-after-the-marker byte).
Slot 5 (`101`) is non-zero and non-all-ones, so a wrong parse is far more
likely to read back some *other* wrong value than to coincidentally still
read `5` — the stronger test.

**No coincidental superframe-marker collision**: `0x8D >> 5 == 0b100`,
not `0b110` — true for *every* possible `frame_to_show_map_idx` value
0..=7 at profile 0, since those 3 bits are the byte's low 3 bits and the
marker check only looks at the top 3 (bits 7-5, always `100` here
regardless of slot choice). `SuperframeIndex::parse` therefore returns
`Ok(None)` for this payload and `Superframe::parse` takes its trivial
single-frame branch — confirmed by the crate's own parser, not just this
arithmetic (see "Validation" below).

### `p9sef.ref.yuv`: the key frame's real pixels, reused twice

9576 bytes = 4788 × 2, where 4788 = `76*42 + 2*38*21` (76x42 4:2:0: a
3192-byte Y plane plus two 798-byte chroma planes) is exactly one
`p9still`/`p9sef`-sized frame. Built by concatenating the **first** 4788
bytes of `p9still.ref.yuv` (libvpx's real decode of the exact key frame
`p9sef.frame0.bin` copies) with itself — not a fresh decode of anything,
and not this crate's own decoder output. The first copy is what
`p9sef.frame0.bin`'s real decode is checked against; the second copy exists
only to keep [`assert_bit_exact_sequence`]'s reference-byte accounting
honest for `frame1`'s redisplay (which this harness counts as shown but
cannot pixel-verify without an inter/DPB decoder — see its module doc
comment) — it is not independently re-verified against anything, since
there is nothing to independently verify it against without an
already-real inter decoder.

### Validation

`examples/vp9_layout_dump.rs` (see "Layout derivation" above) parses
`p9sef.frame0.bin`/`p9sef.frame1.bin` with this crate's real parser and
produces:

```
# p9sef: frame_idx sub_idx frame_type show_frame refresh_frame_flags ref_frame_idx[L,G,A] sign_bias[L,G,A] interp_filter(raw) allow_high_precision_mv error_resilient refresh_frame_context fpdm frame_context_idx reset_frame_context tile_cols_log2 tile_rows_log2 width height frame_to_show
0 0 KEY true 0xff [0,0,0] [false,false,false] 0 false false true false 0 0 0 0 76 42 -
1 0 SEF - - - - - - - - - - - - - - - 5
```

— `frame_to_show=5`, matching the hand-derivation above exactly, and every
other field on the `SEF` row correctly `-` (not coded, per
`uncompressed.rs`'s early return). `inter_fixture_tests.rs`'s
`p9sef_frame0_key_and_frame1_show_existing` is the real gate: it reuses
`assert_bit_exact_sequence` (asserting `(coded_frames, shown_frames,
verified_frames) == (2, 2, 1)` — frame0 pixel-verified, frame1 counted
shown but not verified, exactly the SEF handling that function's module
doc comment describes) and separately asserts `hdr.show_existing_frame &&
hdr.frame_to_show == 5` directly against a fresh parse of
`p9sef.frame1.bin`, since the aggregate `SequenceCheck` returned by
`assert_bit_exact_sequence` does not expose per-frame details. Decode-level
verification of the redisplay itself (actually reconstructing slot 5's
pixels a second time) is out of scope until inter/DPB decode lands — a
later package.

### `p9sef` sha256

```
af8ef20cb4458ea45876f1ff3567ed68b234310832c2769af374a7c6cc80d6f5  p9sef.frame0.bin (== p9still.frame0.bin, byte-identical)
075198bfe61765d35f990debe90959d438a943ceeb9d39440e7db5455d449086  p9sef.frame1.bin (the one hand-built byte, 0x8D)
204860781a0efb6431cb011983fcc9dfaee28ad190bdbb33d02064f8915ae962  p9sef.ref.yuv
b017fa4fa2079e0832f7352d15d7112dc4cce6c39e79f3bee65c80d5200fa2dd  p9sef.layout.txt
```

## sha256 (every byte committed by this package)

Computed with `shasum -a 256` immediately after each file was written into
this directory.

All hashes below for `p9still`/`p9basic`/`p9tc`/`p9hp`/`p9seg`/`p9alt` were
**recomputed after the P4-VERIFY `-frame-parallel 0` correction pass** and
supersede the original package's values; `p9er` is untouched and its
hashes are unchanged from the original P4 package.

```
### p9still
af8ef20cb4458ea45876f1ff3567ed68b234310832c2769af374a7c6cc80d6f5  p9still.frame0.bin
a63a21e355556b8f9a5513b24068911635b3fe88c48535e6dbbe23d36fecb317  p9still.frame1.bin
999afe2b23edd353816a18b43e9391b8b9a0885dbb004025cb1d9ea0ba6d6261  p9still.frame2.bin
817a6bfe5d3266d4d6a498245b5edc83685bdccec016a0eeb20c2b544b9e76d6  p9still.frame3.bin
13104303c4942f672d57b8bef0a9921bb25dcdaa6296c184419667e867187eae  p9still.frame4.bin
666e89126d0f79692852e921b1737c12a9d9255ec3f7ae9fa36eab6ed7319380  p9still.frame5.bin
295ff667416ec01d8b2f917bd90c0ebb1a5e1f5aa5323f2c07e4c214253641a2  p9still.frame6.bin
e315effc6edfd731672da43e9e6e160be9cd59737650aed75dddbb3db529e989  p9still.frame7.bin
c106a8df4339ed9be061579651fb6d19dd79a6f686650b33440a078ebaf6ec27  p9still.ref.yuv
69b827c0083f37688a4096ea9a94515eee2c849e637988a0c851353375ba1f51  p9still.layout.txt

### p9basic
af8ef20cb4458ea45876f1ff3567ed68b234310832c2769af374a7c6cc80d6f5  p9basic.frame0.bin
a63a21e355556b8f9a5513b24068911635b3fe88c48535e6dbbe23d36fecb317  p9basic.frame1.bin
999afe2b23edd353816a18b43e9391b8b9a0885dbb004025cb1d9ea0ba6d6261  p9basic.frame2.bin
a435481b523caf1aef6897b54ee701ade6ef39b5158d133f547abc0e8e513ce5  p9basic.frame3.bin
bfde4eeea263d906ed83ba6523c2d9ea4c031e82d9a703308d417968dc6e9c79  p9basic.frame4.bin
7418aa5aa96e3440af4e666b45db093ef30de46b8c77c7a1cdb3a7b7ed2b51e8  p9basic.frame5.bin
cae42b85f7e0d8c1cd72e4b9450dcbda2786c3f63b6a8f610304e9eeaad588a2  p9basic.frame6.bin
c7293fce92659bcd29349c5b1cef62a1d5fe44c6ff556a72e36593c8c8fde415  p9basic.frame7.bin
a3a3b7a70c844329eacd44d4ad4e486cc3e10d3d5e6910d61ddca036380e5adb  p9basic.ref.yuv
2b178a51c94d40c725bb8f765ef6a3f70f4fdb66320d4b0a115d575ac5a8c009  p9basic.layout.txt

### p9er (unchanged -- error_resilient forces fpdm=1 structurally, not
### regenerated; layout.txt still gained the two new columns)
b685c1534d25ef8dd0864f60449f6d3cfe410d163d40b2eedd792cd2a1d7fcc0  p9er.frame0.bin
34f7c3a9156252a280f2dfa51cbcb6c178631e24ce058697e710413470f4e012  p9er.frame1.bin
12a313f3bc3a0ab586b2080dab5f0f03063803ced7ee7dfd7fb6aecaec7eeb7e  p9er.frame2.bin
2f1a4ece563c3fb7b714197ec9e8edc9d438c29980d6fe0e8c844ec94eea92b3  p9er.frame3.bin
8ef24d1478ac417c622faac907c549951e599498303966346170ac37f1ae1028  p9er.frame4.bin
2486f28f46bd35e47bcd3de5396a376cf652d254e9038e850fd756c09e771295  p9er.frame5.bin
8a1fc9da82277f3096fa433dbe43e5706ad8fb5bcc69aa85f2c38d25115c2de8  p9er.frame6.bin
34f7c3a9156252a280f2dfa51cbcb6c178631e24ce058697e710413470f4e012  p9er.frame7.bin
a3a3b7a70c844329eacd44d4ad4e486cc3e10d3d5e6910d61ddca036380e5adb  p9er.ref.yuv
4f02f1954351f69aa9e2abc6a4334af7ee69184d03dee341c9ae1876ad7526a1  p9er.layout.txt

### p9tc
efb5cd79104368d8c3b53abcc7299d88a957bd9544262a1694d36db2083a04de  p9tc.frame0.bin
ca29a2b544d133472da9938a5759c43328e9193ef2526492bd1bf2b62c4fc63b  p9tc.frame1.bin
a2b3031f964ef4c272e8a0f949fc6cbfc08db1c5462219bced519caaca58cfff  p9tc.frame2.bin
b3d06e06451190ebaa10fa886b71fec4bc9085c61a20ae2a4463a5f89c78acbf  p9tc.frame3.bin
093d1aef8d5a049faf54a6000d24e4eb526ae55d99517b62c1760fb589d4c63f  p9tc.frame4.bin
2789da66981e9177699a23048314aaf41c67cd839dbbf08eec24b39856ed7a7c  p9tc.ref.yuv
7951d1aca1a76c120b348e7c6cd8de56730a4da618df903ded30f3b85df535ef  p9tc.layout.txt

### p9hp
f499cd8f2a48bf378e8885abc21adca7cfde2532af0f419b160b3208fd22c9f4  p9hp.frame0.bin
93f337ea8fc8757c18727b2215d5e45ccea00813634f0f5c009dc2283b331f73  p9hp.frame1.bin
b5eeeba28556578f0066a02a94c42612dfe2cea73bfab20f78d66b868604bc70  p9hp.frame2.bin
a0e6e558398c22fc66c40b4e1f9573b8cbb692b85605f3cd39b6b6e5a606ec13  p9hp.frame3.bin
3aa7e3e8619d1b56b11f5ad9efc4a0e1b2b1267db86148e7495ab7c0ba268b6e  p9hp.frame4.bin
f9b6c5f64de65b8b7bee23da3bcd75f27e95add9838e95822c11dd14519fc9ce  p9hp.frame5.bin
7a8701723c94976ab96223ea77f20637e2de0c648aeac4d06f3096311b7b8ab1  p9hp.frame6.bin
7401ac9d1ec972b380c38f7ab25a9b9e3cc89b2141d44ba652c37c4930e89320  p9hp.frame7.bin
1d00374d874351494de23954d5b53a3661d58aff873332cd5f3659a733e8b701  p9hp.ref.yuv
d60e1ef79c605a7a5182b7f6c211de671b9a63f64f5f4d58340b4ac5c5811032  p9hp.layout.txt

### p9seg
b3ca28c77c9b78bfbfe324f031dd137bff6004d6d7104881901942760103a549  p9seg.frame0.bin
24e295ddf02e069f58f30f22e8192c82dee86db0468155368f2e569f3e99ff82  p9seg.frame1.bin
4ec2f2b53aa07b2ba82d0715bff954c5137397865a68a0f34b0eb0d6249ac88d  p9seg.frame2.bin
6e89791c242031399be2c48f70a79b3e15552b77bf7c72eb76f0ec8aa0191e60  p9seg.frame3.bin
f0390a563ce98d9c429ef3167fe68e5f015204dfdff8a0b655788f7219f1bf92  p9seg.frame4.bin
6087b0bd6bc356d0a5777a4332d3a67854adb39ff426d16320832acde4b4b49b  p9seg.frame5.bin
a8980da6ee415c5031f4e83fee47c41f763ebfd9ec104260e8c8fda435f5b342  p9seg.frame6.bin
7aa2b1bcd3950df44e3c717097df977368d818786e43c223b55141b206438b9a  p9seg.frame7.bin
84f3ff338f09a3988fc649eb19b9094da3d6603dbce2c435b8020eb54ad54283  p9seg.ref.yuv
4979ac38d6a22d2323752043e6c54d6fd2b4058352a337088938bd88a78ec644  p9seg.layout.txt

### p9alt
253ba7d357e04f1fe8ee866148be3ddb78c316dc59eb28194d95eeafbba32fc1  p9alt.frame0.bin
00cd5b133f124248eeb58878005f29b4da7a2506e0e90721b5244bbeaa210b14  p9alt.frame1.bin
ffa0b75ee76cfc4c1b10c63b972efd60c299b0d0f2e0038985f668f616640b09  p9alt.frame2.bin
d4f72177ab26c597008bcff407a2b7c569db9b184285eaa7ff8e406d623ccd0a  p9alt.frame3.bin
4f752a1258fac3607944d3cb5c36fd186ca47a8c2dc0a437687d1845c0956bd1  p9alt.frame4.bin
5d2deaf5498c6cd224a2d0899aa1319d9320553ffb2cb06450d02576918b348e  p9alt.frame5.bin
d02d213343d456caed7a65c14edf49ec19736f833be68e31b0f43809427a92e3  p9alt.frame6.bin
9d1d7ced2d06745ceb76ad773b7d12e77db03ff05e94d344a7f9684d0b9d98b2  p9alt.frame7.bin
5a69f0162c0960966b491acd538babb1b33953c248c911a1f7623fd111b4c5bd  p9alt.ref.yuv
c8ec7abddeeb10d885ac9066ecdce043f967316608bbdb442073f8a399930c55  p9alt.layout.txt
```

**Note on sha256 duplicates — re-derived after the P4-VERIFY
`-frame-parallel 0` correction pass, not carried over from the original
package's (now stale) analysis.** The coded bytes changed for all six
regenerated streams, so every coincidence below was re-checked directly
against the new hashes above, not assumed to still hold:

- `p9still.frame0.bin` and `p9basic.frame0.bin` **still** hash identically
  (`af8ef20c...`), and so do `p9still.frame{1,2}.bin` /
  `p9basic.frame{1,2}.bin` (`a63a21e3...` / `999afe2b...`). This part of the
  original finding survives the correction: the `loop` filter that freezes
  `p9still`'s source only affects frames *after* the first, so both
  streams' key frame (and, since both sources are still "no visible change
  yet" this early in `testsrc2`'s slow drift, their first two inter frames)
  encode to the same bytes.
- **This is where the correction pass's whole point shows up**: the
  original package found `p9still.frame{1,3,5,7}.bin` all hash identically
  to each other (four repeats of one value) because with
  `fpdm=1` (no adaptation), every trivial all-skip "no change" frame reused
  the same static, never-adapted probability context, so semantically
  identical frames encoded to byte-identical bitstreams. **With `fpdm=0`
  (this correction pass), that no longer holds**: `p9still.frame1` through
  `frame7` now hash to seven *different* values
  (`a63a21e3`/`999afe2b`/`817a6bfe`/`13104303`/`666e8912`/`295ff667`/`e315effc`)
  even though the source is still frozen and every one of those frames is
  still semantically "all-skip, no change" — backward probability
  adaptation (`refresh_probs()`) now shifts the entropy-coding context after
  every frame, so the *coded bytes* for an identical semantic decision
  drift frame-to-frame even though the *decoded pixels* do not (next
  bullet). This is direct, checked-not-assumed evidence that these six
  fixtures now actually exercise the adaptation machinery, which was the
  entire purpose of this correction pass.
- `p9still.ref.yuv` is **byte-identical to its own pre-correction value**
  (`c106a8df...`, unchanged) despite every `p9still.frameN.bin` (N>0) now
  coding differently. This makes sense given the previous bullet: a
  skip/ZEROMV inter frame carries no residual and no motion payload, so its
  *reconstructed pixels* are just a copy of the reference frame regardless
  of which probabilities were used to entropy-code the "skip" decision
  itself — adaptation changes the bitstream bytes for this stream but
  cannot change its output pixels.
- `p9basic.ref.yuv` and `p9er.ref.yuv` **still** hash identically
  (`a3a3b7a7...`) after the correction pass, even though `p9basic` now has
  real backward adaptation active and `p9er` (unchanged, `error_resilient`)
  still has none. Re-verified directly (not assumed to survive from the
  original finding): both values were recomputed above and compared byte
  for byte. As before, this says the two streams' *entropy-coding contexts*
  differ (adaptation active vs. not) without their RD-optimal
  mode/motion/coefficient *decisions* differing enough, for this short,
  simple clip, to change the reconstructed pixels — confirmed independently
  by the per-stream `cmp` in
  [Reference decode](#reference-decode--both-vp9-decoders-ffmpeg-exposes-verified-not-assumed)
  succeeding for each stream separately, not by this cross-stream
  coincidence.

## Verification performed at generation time

This list is the *original* P4 package's; see "P4-VERIFY correction pass"
below for what was actually re-run against the new bytes for the six
regenerated streams (`p9still`/`p9basic`/`p9tc`/`p9hp`/`p9seg`/`p9alt`) —
items 1, 2, and 3 below were genuinely repeated (not assumed to still
hold), item 6 is re-scoped honestly, and items 4-5 were genuinely
re-checked against every regenerated and imported payload (not assumed
unaffected just because the coded bytes changed): a `grep`/`awk` sweep of
every `.layout.txt` in this directory (all ten streams, native and
imported) confirms frame 0 `KEY` and every other row `INTER`, and a
Python sweep of all 95 committed `.frameN.bin` files' last bytes against
the `0b110` marker shape found exactly one hit — `p9alt.frame1.bin`, the
one genuine superframe — the same zero-coincidental-hits result the
original package found, now re-confirmed after every byte in this
directory but `p9er` changed or was newly added.

1. **Encode determinism**: every one of the seven encode commands above
   (the `p9alt` 2-pass pair as one unit) was run a second time, from a
   fresh `ffmpeg` process, to a separate path, and `cmp`-compared against
   the first run's output — byte-identical for all seven, including the
   `p9alt` prefix-trim applied to each run's own 20-packet output.
2. **Cross-decoder agreement**: `libvpx-vp9` vs. native `vp9` decode of the
   *exact bytes committed* (the reassembled/trimmed IVF for `p9alt`, the
   direct encode output for the other six) — `cmp`-identical for all seven
   streams (see the table above).
3. **Splitter round-trip**: reassembling `<name>.frameN.bin` back into an
   IVF and `cmp`-ing against the source `.ivf` — identical for all seven
   (a real trim for `p9alt`, a no-op self-check for the other six).
4. **Frame-type sanity**: every `.layout.txt` was read to confirm frame 0
   is `KEY` and every other coded frame is `INTER` — `-g 100` suppresses
   *scheduled* keyframes at this clip length but does not disable
   scene-change-triggered ones, so this was checked, not assumed.
5. **`Superframe::parse` robustness on real payloads**: every committed
   `.bin` file's last byte was checked against the superframe-marker shape
   (`byte >> 5 == 0b110`) before `assert_bit_exact_sequence` was written —
   none of the 53 committed payloads happen to trip it (see
   [Known sharp edge](#known-sharp-edge-superframeparse-on-coincidental-marker-bytes)
   below for why this needed checking at all, and why the harness still
   defends against it).
6. **Copy integrity**: every file in this directory was sha256-compared
   against its just-generated source immediately after being copied in.

## P4-VERIFY correction pass: what was actually re-run against the new bytes

Performed with a Python IVF tool (`ivf_tool.py` — plain packet split/trim/
info, no superframe unpacking; kept scratch-side, same "throwaway, not part
of the repo" posture as the original package's splitter, not committed
here) plus this crate's own `examples/vp9_layout_dump.rs` (deleted after
use — see "Layout derivation" above):

1. **fpdm confirmed broken, then confirmed fixed, by the crate's own real
   parser, not assumed from the encoder flags**: `vp9_layout_dump` was run
   against the *original, committed* bytes for all seven streams before any
   change was made. Result: `p9still`/`p9basic`/`p9tc`/`p9hp`/`p9seg`/`p9alt`
   each had `frame_parallel_decoding_mode=1` on literally every frame
   (key and inter); `p9er` also showed `fpdm=1` on every frame, but that is
   structurally forced by `error_resilient=1`, not the same bug (see the
   top of this file). After regenerating the six with `-frame-parallel 0`
   and re-running the same tool: every frame of all six now shows `fpdm=0`
   (`refresh_frame_context=1` too) — confirmed by literally counting
   `fpdm=false` occurrences against each stream's total frame count (8, 8,
   5, 8, 8, 9 respectively — `p9alt` has 9 because `frame1` unpacks to 2
   sub-frames) and finding an exact match both times.
2. **Encode determinism, re-run for real**: each of the six corrected
   encode commands (the `p9alt` 2-pass pair as one unit) was run a second
   time to a separate scratch path and `cmp`-compared against the first
   run — byte-identical for all six.
3. **Cross-decoder agreement, re-run for real**: `ffmpeg -c:v libvpx-vp9`
   vs. `ffmpeg -c:v vp9` (native) decode of the exact corrected bytes —
   `cmp`-identical for all six (the `p9alt.ref.yuv` byte count, 196608,
   confirms the hidden-ALTREF/8-shown-of-9-coded structure survived the
   correction unchanged).
4. **Splitter round-trip, re-run for real**: `<name>.frameN.bin` as
   actually committed to `testdata/` was reassembled into a fresh IVF (same
   header fields, `pts = frame index` — verified true for all six by
   `ivf_tool.py info` before assuming it) and `cmp`-compared against the
   corrected scratch `.ivf` — byte-identical for all six.
5. **`p9alt` superframe survival, checked not assumed** (flagged by review
   as a real risk, since 2-pass GF-group planning can shift with a
   different entropy-adaptation setting): re-ran the marker-byte +
   size-sum arithmetic against the new `p9alt.frame1.bin` directly. Still a
   superframe, still at packet 1, with new sizes `[1853, 420]` (was `[1853,
   411]`) and a still-6-byte index: `1853 + 420 + 6 = 2279 =
   len(p9alt.frame1.bin)` — sums exactly, confirmed both by this arithmetic
   and by `Superframe::parse` (post this package's own fix, see "Known
   sharp edge" below) accepting it without error.
6. **Copy integrity**: every regenerated file's sha256 in the checksums
   section below was computed directly from the file as committed in this
   directory (`shasum -a 256`, not copied from a scratch computation),
   then independently re-verified by a second, separate `shasum -a 256`
   pass diffed against the values written into this document — this caught
   and fixed one transcription slip (a placeholder value briefly present
   for `p9er.layout.txt`) before this pass was called done.
7. **`inter_fixture_tests.rs`, the real gate**: `cargo test -p
   oximedia-codec --features vp9` re-run after copying the corrected files
   in — all seven `p9*_frame0_key_bit_exact_vs_libvpx` tests (plus the four
   `kf*` tests, unaffected) pass unchanged (`coded_frames`/`shown_frames`/
   `verified_frames` assertions identical to before the correction), and
   the crate's own key-frame decoder still reconstructs frame 0 of every
   corrected stream bit-exactly against the newly-decoded `libvpx`
   reference — the strongest evidence available that the correction didn't
   silently corrupt anything, since it is not a byte-comparison but an
   independent real decode.

## Known sharp edge: `Superframe::parse` on coincidental marker bytes

`vp9/superframe.rs`'s `SuperframeIndex::parse` treats a payload whose last
byte's top 3 bits equal `0b110` (the superframe marker shape) as the start
of a *mandatory* superframe index parse. A real, tiny, entropy-coded single
(non-superframe) frame's last byte can match that 3-bit shape purely by
coincidence -- none of this package's 53 committed payloads happen to
(checked directly against every one, item 5 above), so this did not block
generation, but a harness meant to outlive this package should not assume
every future fixture is equally lucky. Two distinct ways a coincidental
match can then misbehave were identified by the original P4 package:

1. **Loud**: once the length/marker-repeat disambiguation that exists to
   tell "coincidence" from "real index" apart fails, `SuperframeIndex::parse`
   returns `Err`, not `Ok(None)` -- the common outcome of a coincidental
   shape match, and easy to mistake for a real header-parse failure one
   frame later if a caller does not special-case it.
2. **Quiet**: `Superframe::parse`'s `Ok(Some(index))` branch checked that
   each individual claimed frame fits inside the payload, but never checked
   that the claimed sizes *sum* to exactly the payload length minus the
   index. A payload that passed the length/marker-repeat disambiguation by
   coincidence (far less likely than (1), but nothing ruled it out) could
   come back `Ok` with `frames` that silently omitted some of the payload's
   real tail bytes.

**P4-VERIFY fixed (2) at the source.** `Superframe::parse` in
`vp9/superframe.rs` now performs the size-sum check itself (right after
the per-frame fit check, before returning `Ok`) and returns an honest `Err`
when the sizes don't add up -- see the doc comments on `Superframe::parse`
and `SuperframeIndex` for the exact check and its libvpx/spec citation. (1)
was **not** changed: a coincidental marker-shape match whose
length/marker-repeat disambiguation fails is still, correctly, a loud
`Err` rather than a silent `Ok(None)` reinterpretation -- that is
`SuperframeIndex::parse` doing its job (a caller has no way to tell "genuinely
malformed superframe index" from "coincidence" apart at that point, and
guessing wrong would desync every frame after it), not a second instance of
bug (2), and out of this correction pass's scope.

`../testutil.rs`'s `split_superframe_defensively` used to re-derive the
same marker-byte arithmetic `SuperframeIndex::parse` uses and only trust a
`Superframe::parse` result when the frame sizes were self-consistent with
it by hand; now that the check lives in `Superframe::parse` itself, that
function has been simplified to delegate to it directly (`Ok` trusted
as-is, any `Err` -- including a sum mismatch now -- falls back to treating
the payload as one non-superframe frame, the same externally-visible
behavior as before). Every other caller of `Superframe::parse`
(`Vp9Decoder::send_packet` in `vp9/decoder.rs`, production code, not just
this test harness) now gets the same size-sum protection for free.
Verified against `p9alt.frame1.bin` (the one real superframe in this
package's fixtures — after the `-frame-parallel 0` correction pass below,
two frames sized 1853 + 420 bytes, summing exactly to its 2279-byte payload
minus a 6-byte index) to confirm the guard still accepts a genuine
superframe, plus two new synthetic unit tests in `superframe.rs` itself
(`test_superframe_size_sum_mismatch_is_rejected` /
`_overshoot_is_rejected`) pinning both directions of "sizes don't add up"
as honest `Err`s.

## Regenerating

Run the encode commands above (`<scratch>` is any writable directory for
the 2-pass log), then the split/reassemble step, then the reference decode.
`ffmpeg`/`libvpx` version drift can change the coded bytes, so
regeneration means re-establishing every golden and every hash in this file
too — the tests compare against libvpx's reconstruction of *these* bytes,
not against a fixed image.


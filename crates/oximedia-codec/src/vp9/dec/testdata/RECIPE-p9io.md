# `p9io` — VP9 intra-only fixture (track package P5)

One real, encoder-produced VP9 **intra-only** frame plus the real
`show_existing_frame` packet that displays it, and libvpx's own
reconstruction of that frame. Nothing here was hand-authored, hand-edited,
or produced by this decoder.

This file documents only the three `p9io.*` files. The inter fixtures
(`p9still`, `p9basic`, `p9er`, `p9tc`, `p9hp`, `p9seg`, `p9alt`) and the
four `kf*` key-frame fixtures are documented in `RECIPE.md`, which this
package did not touch.

## Why a downloaded test vector rather than a fresh encode

`ffmpeg`'s `libvpx-vp9` wrapper cannot be made to emit an intra-only frame:
libvpx's encoder produces them for spatial-SVC layer keyframes, and the
wrapper exposes no option that reaches that path (`ffmpeg -h
encoder=libvpx-vp9` on the build in `RECIPE.md`'s *Tooling* section lists
none), while `vpxenc` is not installed on this machine. The WebM project
publishes an official libvpx test vector for exactly this feature, so it is
used directly.

## Source

```
https://storage.googleapis.com/downloads.webmproject.org/test_data/libvpx/vp90-2-16-intra-only.webm
sha256  328abd0bbb630f76ed5e6a5ed94e8065f8dda53c8f5ad120fba515a03438512b
size    175807 bytes, 352x288, VP9 profile 0, 7 coded packets
```

Its layout, read with a throwaway probe of the same uncompressed-header bit
layout `../../uncompressed.rs` implements (and re-verified afterwards by
`vp9/decoder.rs`'s own test, which parses the committed bytes with the real
`UncompressedHeader::parse`):

| packet | bytes | what it is |
|---|---|---|
| 0 | 117963 | a **superframe** of 4 sub-frames: three hidden intra-only frames (refreshing slots 0, 1, 2) then one shown inter frame (refreshing slot 3) |
| 1 | 25801 | inter, shown, refreshes slot 4 |
| 2 | 1 | `show_existing_frame`, slot 2 |
| 3 | 1 | `show_existing_frame`, slot 1 |
| 4 | 1 | `show_existing_frame`, slot 0 |
| 5 | 21400 | inter, shown, refreshes slot 0 |
| 6 | 10293 | inter, shown, refreshes slot 1 |

Packet 0's superframe index (last byte `0xd3`: 3 bytes per size, 4 frames)
gives sub-frame sizes `30299 + 35391 + 40875 + 11384 = 117949`, exactly the
117963-byte payload minus the 14-byte index.

## Files committed

| file | what it is | sha256 |
|---|---|---|
| `p9io.frame0.bin` | sub-frame 0 of packet 0, verbatim: a hidden intra-only 352x288 frame | `6a63f382e311a764054a5031ba9de397b84374d1fd5d894b811f96deabcc3c9c` |
| `p9io.frame1.bin` | packet 4, verbatim: the one-byte `show_existing_frame` for slot 0 | `4f362f9093bb8e7012f466224ff1237c0746d8c8f660b16699f5036ccba9c64a` |
| `p9io.ref.yuv` | libvpx's reconstruction of `p9io.frame0.bin`, planar YUV 4:2:0, 152064 bytes (one 352x288 frame) | `52774fc29da2fa2b5dd2854409982b1c6baac745318ab634ec5fa2bdce4d5b85` |

`p9io.frame0.bin`'s header fields, as the crate's own parser reads them
(asserted in `vp9/decoder.rs`'s `test_decode_real_intra_only_frame_bit_exact`
rather than only recorded here):

```
frame_type=INTER  intra_only=1  show_frame=0  error_resilient=0
reset_frame_context=2  refresh_frame_flags=0x01  frame_context_idx=0
refresh_frame_context=1  frame_parallel_decoding_mode=0
width=352 height=288  base_q_idx=53  loop_filter_level=6
```

The last two flags on the third line are why this fixture matters beyond
intra-only support: `frame_parallel_decoding_mode == 0` makes it the **first
fixture in this directory that actually counts symbols and runs backward
probability adaptation**. All four `kf*.frame0.bin` key frames code
`frame_parallel_decoding_mode == 1`, which switches counting and adaptation
off entirely, so they cannot exercise either.

## Why trimming the superframe to sub-frame 0 is sound

Sub-frame 0 is the first frame of the stream and is intra-only with
`reset_frame_context == 2` and `frame_context_idx == 0`, so it resets and
then loads probability context 0 itself; it references no reference frame
for pixels, and nothing that follows it in the superframe can affect its own
reconstruction (decode is strictly causal). A packet containing exactly
those bytes is a legal VP9 packet — its last byte is `0x40`, which does not
have the superframe-marker shape (`byte >> 5 == 0b110`), so it is
unambiguously a single frame.

This was not left as an argument: the golden below was decoded from the
trimmed two-packet stream and `cmp`-compared against the corresponding frame
of the **untrimmed** 7-packet stream, and they are byte-identical (see
*Verification* item 3).

## Reference decode — both VP9 decoders ffmpeg exposes

The two committed payloads were reassembled into a standalone IVF (32-byte
`DKIF` header, 12-byte per-packet header, payload verbatim — pure container
framing, no codec logic) and decoded with each of ffmpeg's two independent
VP9 decoders:

```
ffmpeg -y -c:v libvpx-vp9 -i p9io.ivf -fps_mode passthrough \
  -pix_fmt yuv420p -f rawvideo p9io.libvpx.yuv
ffmpeg -y -c:v vp9        -i p9io.ivf -fps_mode passthrough \
  -pix_fmt yuv420p -f rawvideo p9io.native.yuv
cmp p9io.libvpx.yuv p9io.native.yuv   # identical
```

Both produce exactly 152064 bytes — one frame, from the
`show_existing_frame` packet; the intra-only frame itself is hidden and
emits nothing. `p9io.libvpx.yuv` is what is committed as `p9io.ref.yuv`;
`p9io.native.yuv` is discarded once `cmp` confirms it is identical.

Tooling is the same as `RECIPE.md`'s: `ffmpeg 7.1.1` (Homebrew,
`libavcodec 61.19.101`) built against **libvpx 1.15.2**, macOS 26.6.1
(Darwin 25.6.0), arm64. The WebM parsing and IVF assembly used `python3`
(container framing only).

## Verification performed at generation time

1. **Cross-decoder agreement** on the exact bytes committed —
   `libvpx-vp9` vs. native `vp9`, `cmp`-identical.
2. **Superframe arithmetic** — the four sub-frame sizes sum exactly to the
   packet payload minus its index, and each sub-frame's header parses.
3. **Trim fidelity** — the golden equals output frame 4 of the full
   7-packet stream (`sha256 52774fc2…`), i.e. what a decoder reconstructs
   for slot 0 when the other six packets are present. The trim removes
   packets, not meaning.
4. **Not-a-superframe** — `p9io.frame0.bin`'s last byte (`0x40`) does not
   match the superframe-marker shape, so `Superframe::parse` takes its
   single-frame branch.
5. **Provenance of every byte** — both `.bin` files are byte ranges of the
   downloaded `.webm`'s block payloads, copied verbatim; the `.yuv` is
   ffmpeg/libvpx output, unmodified.

## Regenerating

Re-download the test vector (its sha256 is above), parse the WebM
`SimpleBlock` payloads, split packet 0 at its superframe index, keep
sub-frame 0 and packet 4, and re-run the reference decode above. The test
compares against libvpx's reconstruction of *these* bytes, so a libvpx
version change means re-establishing the golden and its hash too.

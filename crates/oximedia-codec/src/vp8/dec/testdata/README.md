# VP8 conformance fixtures (key + inter frames)

Five real VP8 streams, split into their raw per-frame payloads, plus
libvpx's own reconstruction of every shown frame. They are consumed by
`include_bytes!` from `../inter_fixture_tests.rs` (whole-sequence bit-exact
decode), `../mode_tests.rs` and `../header.rs`'s tests.

Nothing here was hand-authored, hand-edited, or produced by this decoder:
every byte comes from `ffmpeg`/`libvpx`, and the reference YUV is what
**libvpx itself** decoded from the very same bytes.

## Files

| pattern | what it is |
|---|---|
| `<name>.frameN.bin` | coded frame `N` of `<name>`, **raw VP8 payload only** — no IVF/container framing (the `vp9/kf/testdata` convention) |
| `<name>.ref.yuv` | libvpx's reconstruction of every **shown** frame of `<name>`, concatenated, planar YUV 4:2:0, no headers |

Naming note: `vp9/kf/testdata` calls its reference `ref<name>.yuv` while this
set uses `<name>.ref.yuv`. This set keeps the name it was generated with, so
that the checked-in bytes are verifiably the generated ones (sha256-compared
at copy time) with no rename step to get wrong; the `<stream>.<role>.<ext>`
shape also groups a stream's files together in a directory listing.

## The streams

| stream | dims | coded / shown frames | what it is for |
|---|---|---|---|
| `p5basic` | 96x64 | 5 / 5 | LAST-only P frames; NEAREST/NEAR/ZERO/NEWMV mode mix; sub-pixel motion. Every inter frame has `mb_lf_adjustments.delta_update = 0`, so it also gates loop-filter delta **inheritance** from the key frame (RFC 6386 §9.4) |
| `refswap` | 96x64 | 19 / 18 | a genuine **hidden** frame (packet 1: `show_frame = 0`, `refresh_alternate_frame = 1`, `refresh_last = 0`); `sign_bias_alternate = 1` on packets 2-14; packet 15 carries `refresh_golden_frame = 1` **and** `copy_buffer_to_alternate = 2` together, which gates `Dpb::commit`'s sequential (non-swap) update order |
| `refswap_er` | 96x64 | 5 / 5 | `-error-resilient 1`: `refresh_entropy_probs = 0` on **every** frame including the key frame, i.e. the entropy snapshot/restore path; plus real bitstream segmentation with per-segment quantiser deltas |
| `splitmv` | 100x64 | 5 / 5 | strong diagonal pan (10 px/frame horizontally, 6 px/frame vertically) of a detailed fractal, encoded with `-cpu-used 0 -deadline best` to make SPLITMV partitions RD-favourable. 100 px wide is **not** a macroblock multiple, so it also covers the partial right-hand column |
| `segdelta` | 96x64 | 5 / 5 | segmentation enabled with per-segment quantiser deltas on every frame, `refresh_entropy_probs = 0`, and non-trivial `mb_lf_adjustments` (`ref_frame_deltas = [2, 0, -2, -2]`, `mode_deltas = [4, -2, 2, 4]`) |

`refswap`'s hidden frame means **coded index != reference index**: packet 0
is reference frame 0, packet 1 is hidden (no reference frame), and packet
`N >= 2` is reference frame `N - 1`. A naive same-index comparison loop
mis-aligns silently from reference frame 1 onward; the test suite walks the
decoder's reported `show_frame` bit instead.

## Provenance

Tooling, exact versions: `ffmpeg 7.1.1` (Homebrew, `--enable-libvpx
--enable-gpl`) with **`libvpx 1.15.2`**; `python3` 3.14.6 for the IVF split;
macOS/arm64. `vpxenc` is not installed on the generating machine (checked
`PATH`, Homebrew's `libvpx` prefix and cellar, and a filesystem-wide
`find`), so everything below goes through ffmpeg's libvpx wrapper.

### Encode

```
# p5basic
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 5 \
  -c:v libvpx -g 100 -auto-alt-ref 0 -lag-in-frames 0 -qmin 4 -qmax 20 -crf 10 \
  -f ivf p5basic.ivf

# refswap (2-pass: -auto-alt-ref is 2-pass only, and 18 source frames is the
# empirically bisected minimum that makes libvpx emit a hidden altref frame)
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 18 \
  -c:v libvpx -auto-alt-ref 1 -lag-in-frames 16 -g 250 -qmin 4 -qmax 20 -crf 10 -b:v 200k \
  -pass 1 -passlogfile <scratch> -f null /dev/null
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 18 \
  -c:v libvpx -auto-alt-ref 1 -lag-in-frames 16 -g 250 -qmin 4 -qmax 20 -crf 10 -b:v 200k \
  -pass 2 -passlogfile <scratch> -f ivf refswap.ivf

# refswap_er
ffmpeg -f lavfi -i "testsrc2=size=96x64:rate=25,format=yuv420p" -frames:v 5 \
  -c:v libvpx -auto-alt-ref 1 -lag-in-frames 16 -g 250 -qmin 4 -qmax 20 -crf 10 \
  -error-resilient 1 -f ivf refswap_er.ivf

# splitmv
ffmpeg -f lavfi -i "mandelbrot=size=200x150:rate=25,format=yuv420p,crop=100:64:x='10*n':y='6*n'" \
  -frames:v 5 -c:v libvpx -cpu-used 0 -deadline best -qmin 4 -qmax 12 -crf 8 \
  -f ivf splitmv.ivf

# segdelta
ffmpeg -f lavfi -i "gradients=size=96x64:rate=25:seed=42:c0=0x3355ff:c1=0xff8800:nb_colors=2:x0=10:y0=10:x1=86:y1=54:type=0:speed=0.02,format=yuv420p" \
  -frames:v 5 -c:v libvpx -error-resilient 1 -qmin 4 -qmax 20 -crf 10 \
  -f ivf segdelta.ivf
```

An explicit `-crf` inside each stream's `[qmin, qmax]` window is required on
this ffmpeg build: with neither `-crf` nor `-b:v` it defaults to CQ mode at
an implicit CRF of 32, which fails outright against these narrow quantiser
windows ("CQ level 32 must be between minimum and maximum quantizer value").

### Reference decode — libvpx, verified not assumed

```
ffmpeg -c:v libvpx -i <name>.ivf -fps_mode passthrough -pix_fmt yuv420p -f rawvideo <name>.ref.yuv
```

`-c:v libvpx` before `-i` forces the **libvpx** decoder rather than ffmpeg's
independent native `vp8` decoder; for every one of the five streams this was
confirmed from ffmpeg's own `-loglevel verbose` stream mapping line
(`Stream #0:0 -> #0:0 (vp8 (libvpx) -> rawvideo (native))`), which is what
licenses the strong claim "bit-exact vs libvpx" in the test suite.

`-fps_mode passthrough` is **not optional**: without it the rawvideo muxer
fills the presentation-timestamp gap left by `refswap`'s hidden frame by
**duplicating** a picture, silently producing a 19-frame (175104-byte)
reference for an 18-shown-frame stream. This was caught empirically, not
theorised: the first `refswap` decode produced exactly that file before the
flag was added.

### Splitting

`split_ivf.py` (kept with the generation recipe outside the repo) parses the
IVF container, writes each coded frame's payload verbatim to
`<name>.frameN.bin`, and independently walks the VP8 frame header (its own
from-scratch RFC 6386 §7.3 boolean decoder — not this crate's) to dump the
per-frame flag table quoted in the stream descriptions above. It aborts
rather than emit output if either of two static validators fails: a key
frame's uncompressed-header dimensions must equal the IVF container's, and
`y_ac_qi` must fall inside the stream's `[qmin, qmax]`. Zero validation
problems were reported for any frame of any stream.

### Verification performed at generation time

1. **Bool-decoder self-test**: the from-scratch header parser was first run
   against the *pre-existing* `VPX_KEYFRAME_VP8` fixture in
   `tests/vp8_fixtures/mod.rs`, and recovered both its known 48x48
   dimensions and its known 4 token partitions.
2. **Encode determinism**: every `.ivf` was generated twice from a fresh
   process (for `refswap`, a fresh 2-pass log path) and compared with `cmp` —
   byte-identical.
3. **Decode determinism**: every `.ref.yuv` was decoded twice and
   sha256-compared — identical.
4. **Size reconciliation**: every `.ref.yuv` byte count equals
   `shown_frames * width * height * 3 / 2`, with `shown_frames` counted from
   the parsed `show_frame` bits rather than assumed.
5. **Round trip**: the `<name>.frameN.bin` files — the bytes this crate
   actually embeds — were reassembled into a fresh IVF container and decoded
   again with the same libvpx command; the result sha256-matched the shipped
   `.ref.yuv` for all five streams.
6. **Copy integrity**: every file in this directory was sha256-compared
   against its source immediately after copying.

## Regenerating

Run the encode commands, then `split_ivf.py` per stream (qmin/qmax: 4/20 for
`p5basic`, `refswap`, `refswap_er`, `segdelta`; 4/12 for `splitmv`), then the
reference decode. Encoder/decoder version drift can change the output bytes,
so regeneration means re-establishing the reference too — the tests compare
against libvpx's reconstruction of *these* bytes, not against a fixed image.

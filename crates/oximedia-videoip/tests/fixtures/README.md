# `oximedia-videoip` decoder test fixtures

Three **real, third-party-encoded** single-keyframe bitstreams used by
`tests/codec_real.rs` to prove that `oximedia_videoip::codec`'s compressed
video decoders actually reconstruct pixels (rather than fabricating a frame).

They are *not* produced by any OxiMedia encoder — the point of the test is that
OxiMedia's decoders agree, bit for bit, with an independent implementation.

| File | Codec | Encoder | Size |
|------|-------|---------|------|
| `vp8_kf.bin` | VP8 | libvpx (via FFmpeg `libvpx`) | 3805 B |
| `vp9_kf.bin` | VP9 | libvpx (via FFmpeg `libvpx-vp9`) | 3425 B |
| `av1_kf.bin` | AV1 | libaom (via FFmpeg `libaom-av1`) | 1181 B |

All three encode the same 128x96 8-bit 4:2:0 source frame (FFmpeg's synthetic
`testsrc2` pattern), so nothing copyrighted is vendored.

## Provenance / how to regenerate

Produced on 2026-08-11 with `ffmpeg version 7.1.1` (Homebrew, arm64-darwin),
reference decodes cross-checked against `dav1d 1.5.1`:

```sh
# 1. common source frame
ffmpeg -y -f lavfi -i "testsrc2=size=128x96:rate=1:duration=1" \
       -pix_fmt yuv420p -frames:v 1 src.y4m

# 2. VP8 / VP9 (IVF containers; the fixtures are the bare frame payloads,
#    i.e. the IVF 32-byte file header and 12-byte frame header stripped)
ffmpeg -y -i src.y4m -c:v libvpx     -crf 20 -b:v 0 -frames:v 1 -f ivf vp8.ivf
ffmpeg -y -i src.y4m -c:v libvpx-vp9 -crf 20 -b:v 0 -frames:v 1 -f ivf vp9.ivf

# 3. AV1 (bare OBU stream — already exactly what the decoder consumes).
#    `enable-palette=0` is REQUIRED: libaom turns palette mode on by default at
#    this resolution, and oximedia-codec's AV1 keyframe decoder honestly
#    rejects palette blocks with
#    "Unsupported feature: AV1 palette mode (has_palette_y) not implemented".
ffmpeg -y -i src.y4m -c:v libaom-av1 -crf 32 -b:v 0 -cpu-used 4 \
       -aom-params "enable-palette=0" -frames:v 1 -f obu av1_kf.bin

# 4. reference reconstructions the expected pixel values in the test come from
ffmpeg -y -i vp8.ivf   -f rawvideo -pix_fmt yuv420p vp8_ref.yuv
ffmpeg -y -i vp9.ivf   -f rawvideo -pix_fmt yuv420p vp9_ref.yuv
ffmpeg -y -i av1_kf.bin -f rawvideo -pix_fmt yuv420p av1_ref.yuv
```

At the time the fixtures were added, `oximedia-codec`'s VP8, VP9 and AV1
decoders each reproduced the corresponding `*_ref.yuv` with **0 differing
bytes out of 18432** (Y+U+V), which is where the per-codec expected pixel
values asserted in `tests/codec_real.rs` come from.

# AVIF decode test fixtures

Real AVIF files produced by `ffmpeg` + `libaom-av1` (**not** authored by
this crate's own `AvifEncoder`, which only emits a structurally valid
container with a minimal placeholder AV1 payload — see the module docs in
`../mod.rs`). These exercise `AvifDecoder::decode()`'s real AV1-backed
pixel reconstruction against independent ground truth.

Generated with ffmpeg 7.1.1 (`--enable-libaom`, `--enable-libdav1d`).
Regenerate with the commands below from this directory; `-cpu-used 4
-enable-palette 0` avoids AV1 palette-mode coding, which this crate's
keyframe decoder does not implement (`CodecError::UnsupportedFeature`) —
tiny synthetic test patterns at high `-cpu-used` readily trigger it.

## `color_only.avif` / `color_only_ref.yuv`

8-bit 4:2:0, 32x32, no alpha. The one fixture the real decode path is
expected to fully decode. `color_only_ref.yuv` is ffmpeg's own
independent decode (via `libdav1d`) of the same file, in raw planar
`yuv420p` (Y then U then V, no header) — the test compares
`AvifDecoder::decode()`'s output against it plane-by-plane, bit-exact.

```sh
ffmpeg -y -f lavfi -i "testsrc2=size=32x32:rate=1" -frames:v 1 -pix_fmt yuv420p \
  -c:v libaom-av1 -still-picture 1 -crf 20 -cpu-used 4 -enable-palette 0 \
  color_only.avif

ffmpeg -y -i color_only.avif -pix_fmt yuv420p -f rawvideo color_only_ref.yuv
```

## `with_alpha.avif`

8-bit 4:2:0 colour image (32x32) plus an alpha auxiliary item. ffmpeg/libaom
has no direct `yuva420p` encoder input for AV1 — the standard way to
produce an AVIF alpha channel is to split the alpha plane out and encode it
as its own AV1 image item, which libaom encodes as **monochrome**
(`gray` pixel format, i.e. `mono_chrome=1` in the AV1 sequence header).
This crate's AV1 keyframe decoder does not implement monochrome
(`CodecError::UnsupportedFeature`, gated in `av1::kf::recon`), so this
fixture is the "honest Err" case: `decode()` must fail with a precise
message naming the alpha image, even though the colour item alone would
decode fine.

```sh
ffmpeg -y -f lavfi -i "testsrc2=size=32x32:rate=1,format=yuva420p" \
  -filter_complex "[0:v]split=2[main][al];[main]format=yuv420p[color];[al]alphaextract,format=gray[alpha]" \
  -map "[color]" -map "[alpha]" \
  -frames:v 1 -c:v libaom-av1 -still-picture 1 -crf 20 -cpu-used 4 -enable-palette 0 \
  with_alpha.avif
```

## `color_10bit.avif`

10-bit 4:2:0, 32x32, no alpha. This crate's AV1 keyframe decoder only
implements 8-bit reconstruction (`CodecError::UnsupportedFeature`, gated in
`av1::kf::recon::decode_intra_frame`). Used to prove that a *colour*-path
failure (not just the alpha path) surfaces the AV1 decoder's own honest
error through `AvifDecoder::decode()`, with the container itself still
probing correctly (`AvifDecoder::probe(..).bit_depth == 10`).

```sh
ffmpeg -y -f lavfi -i "testsrc2=size=32x32:rate=1" -frames:v 1 -pix_fmt yuv420p10le \
  -c:v libaom-av1 -still-picture 1 -crf 20 -cpu-used 4 -enable-palette 0 \
  color_10bit.avif
```

## Container notes (why `avif/container.rs` parses `iloc` version 0)

ffmpeg's AVIF muxer writes `iloc` **version 0** (no `construction_method`
field per item) — different from the version-1 layout this crate's own
`AvifEncoder` writes. `avif/container.rs::locate_mdat_items` supports both;
see its doc comment for exactly which `iloc` layouts are (and are not)
implemented.

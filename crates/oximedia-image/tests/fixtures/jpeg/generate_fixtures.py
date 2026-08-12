#!/usr/bin/env python3
"""Regenerate the baseline-JPEG decoder fixtures.

Every fixture pairs a JPEG file with the reference decode produced by
Pillow/libjpeg-turbo, stored losslessly as PNG. The Rust test decodes the
JPEG with `oximedia_image::jpeg::JpegDecoder`, decodes the PNG with
`oximedia_image::png::PngDecoder`, and compares per pixel.

Sources are generated from a deterministic formula (no external assets), so
this script reproduces byte-identical fixtures on any machine with the same
Pillow/libjpeg build.

Usage:
    python3 generate_fixtures.py

Requires: Pillow, numpy, and ImageMagick `convert` (only for the 4:4:0
fixture, which Pillow cannot write).
"""

from __future__ import annotations

import io
import os
import subprocess
import sys

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))

# (width, height) — deliberately includes non-multiples of 8 and 16 so the
# MCU-padding and chroma-rounding paths are exercised.
SIZES = [(33, 17), (64, 64), (101, 53)]
# Pillow subsampling codes: 0 = 4:4:4, 1 = 4:2:2, 2 = 4:2:0.
SUBSAMPLINGS = [0, 1, 2]
QUALITIES = [70, 90]

SUB_NAME = {0: "444", 1: "422", 2: "420"}


def source_rgb(width: int, height: int) -> np.ndarray:
    """Deterministic RGB test pattern: smooth gradients plus hard colour edges.

    The hard edges are what make chroma upsampling observable; the gradients
    keep the mean error meaningful.
    """
    yy, xx = np.mgrid[0:height, 0:width]
    fx = xx / max(width - 1, 1)
    fy = yy / max(height - 1, 1)

    r = (30 + 200 * fx).astype(np.float64)
    g = (220 - 180 * fy).astype(np.float64)
    b = (40 + 190 * ((fx + fy) * 0.5)).astype(np.float64)

    # Saturated rectangle (hard chroma edge on both axes).
    x0, x1 = width // 5, max(width // 5 + 1, width // 2)
    y0, y1 = height // 4, max(height // 4 + 1, (3 * height) // 5)
    r[y0:y1, x0:x1] = 240.0
    g[y0:y1, x0:x1] = 20.0
    b[y0:y1, x0:x1] = 30.0

    # Vertical bar (hard horizontal chroma edge only).
    xb0 = (3 * width) // 4
    xb1 = min(width, xb0 + max(2, width // 12))
    r[:, xb0:xb1] = 15.0
    g[:, xb0:xb1] = 60.0
    b[:, xb0:xb1] = 235.0

    # Horizontal bar (hard vertical chroma edge only).
    yb0 = (4 * height) // 5
    yb1 = min(height, yb0 + max(2, height // 10))
    r[yb0:yb1, :] = 250.0
    g[yb0:yb1, :] = 250.0
    b[yb0:yb1, :] = 10.0

    # One-pixel checker in the top-left corner: worst case for subsampling.
    checker = ((xx + yy) % 2 == 0) & (xx < width // 6) & (yy < height // 6)
    r[checker] = 5.0
    g[checker] = 250.0
    b[checker] = 120.0

    return np.stack([r, g, b], axis=-1).clip(0, 255).astype(np.uint8)


def source_photo(width: int, height: int) -> np.ndarray:
    """A larger 'photo-like' pattern: sky gradient, sun, ground, hard shapes."""
    yy, xx = np.mgrid[0:height, 0:width]
    fx = xx / (width - 1)
    fy = yy / (height - 1)

    r = 60 + 150 * fy + 20 * np.sin(fx * 9.0)
    g = 110 + 100 * fy + 25 * np.cos(fy * 7.0 + fx * 3.0)
    b = 210 - 120 * fy + 15 * np.sin((fx + fy) * 11.0)

    # Sun.
    cx, cy, rad = width * 0.72, height * 0.26, min(width, height) * 0.13
    dist = np.hypot(xx - cx, yy - cy)
    sun = dist < rad
    r[sun], g[sun], b[sun] = 252.0, 236.0, 120.0

    # Ground.
    horizon = int(height * 0.62)
    r[horizon:, :] = 70 + 60 * fy[horizon:, :]
    g[horizon:, :] = 130 + 70 * (1.0 - fx[horizon:, :])
    b[horizon:, :] = 55 + 40 * fx[horizon:, :]

    # Hard-edged building blocks (chroma edges at arbitrary offsets).
    for i, (bx, bw, bh, col) in enumerate(
        [
            (0.05, 0.11, 0.30, (185, 40, 45)),
            (0.20, 0.09, 0.22, (35, 55, 165)),
            (0.33, 0.13, 0.35, (240, 240, 245)),
            (0.50, 0.08, 0.18, (25, 25, 30)),
        ]
    ):
        x0 = int(bx * width)
        x1 = min(width, x0 + int(bw * width))
        y1 = horizon + 2 + i
        y0 = max(0, y1 - int(bh * height))
        r[y0:y1, x0:x1] = col[0]
        g[y0:y1, x0:x1] = col[1]
        b[y0:y1, x0:x1] = col[2]

    # Thin high-contrast lines (aliasing stress for chroma upsampling).
    for k in range(6):
        x = int(width * (0.60 + 0.05 * k))
        if x + 1 < width:
            r[horizon:, x : x + 1] = 250.0
            g[horizon:, x : x + 1] = 250.0
            b[horizon:, x : x + 1] = 250.0

    return np.stack([r, g, b], axis=-1).clip(0, 255).astype(np.uint8)


def sof_sampling(path: str) -> list[tuple[int, int, int]]:
    """Return [(component_id, h, v)] read straight out of the SOF0 segment."""
    data = open(path, "rb").read()
    i = 2
    while i + 3 < len(data):
        if data[i] != 0xFF:
            i += 1
            continue
        marker = data[i + 1]
        if marker in (0xD8, 0xD9, 0x01) or 0xD0 <= marker <= 0xD7:
            i += 2
            continue
        seg_len = int.from_bytes(data[i + 2 : i + 4], "big")
        if marker in (0xC0, 0xC1, 0xC2):
            body = data[i + 4 : i + 2 + seg_len]
            n = body[5]
            out = []
            for c in range(n):
                cid = body[6 + c * 3]
                sampling = body[7 + c * 3]
                out.append((cid, sampling >> 4, sampling & 0x0F))
            return out
        i += 2 + seg_len
    return []


def write_pair(name: str, jpeg_bytes: bytes) -> None:
    jpeg_path = os.path.join(HERE, name + ".jpg")
    png_path = os.path.join(HERE, name + ".ref.png")
    with open(jpeg_path, "wb") as fh:
        fh.write(jpeg_bytes)
    ref = Image.open(io.BytesIO(jpeg_bytes))
    ref.load()
    ref.save(png_path, optimize=True)
    print(
        f"  {name:32s} jpeg={len(jpeg_bytes):6d} png={os.path.getsize(png_path):6d} "
        f"mode={ref.mode} sampling={sof_sampling(jpeg_path)}"
    )


def main() -> int:
    print("colour fixtures")
    for width, height in SIZES:
        src = Image.fromarray(source_rgb(width, height), "RGB")
        for sub in SUBSAMPLINGS:
            for quality in QUALITIES:
                buf = io.BytesIO()
                src.save(buf, "JPEG", quality=quality, subsampling=sub, optimize=True)
                write_pair(f"rgb_{width}x{height}_{SUB_NAME[sub]}_q{quality}", buf.getvalue())

    print("grayscale fixture")
    gray = Image.fromarray(source_rgb(64, 64), "RGB").convert("L")
    buf = io.BytesIO()
    gray.save(buf, "JPEG", quality=90, optimize=True)
    write_pair("gray_64x64_q90", buf.getvalue())

    print("4:4:0 fixture (h=1, v=2 — Pillow cannot write it, ImageMagick can)")
    src_path = os.path.join(HERE, "_tmp_440_src.png")
    Image.fromarray(source_rgb(64, 64), "RGB").save(src_path)
    out_path = os.path.join(HERE, "_tmp_440.jpg")
    subprocess.run(
        [
            "convert", src_path,
            "-sampling-factor", "1x2",
            "-quality", "90",
            out_path,
        ],
        check=True,
    )
    payload = open(out_path, "rb").read()
    os.remove(src_path)
    os.remove(out_path)
    factors = None
    tmp = os.path.join(HERE, "_probe.jpg")
    with open(tmp, "wb") as fh:
        fh.write(payload)
    factors = sof_sampling(tmp)
    os.remove(tmp)
    if factors and factors[0][1:] == (1, 2):
        write_pair("rgb_64x64_440_q90", payload)
    else:
        print(f"  SKIPPED: ImageMagick produced sampling {factors}, expected (1, 2)")

    print("restart-marker fixture (DRI/RSTn every MCU row)")
    src = Image.fromarray(source_rgb(101, 53), "RGB")
    buf = io.BytesIO()
    src.save(buf, "JPEG", quality=80, subsampling=2, restart_marker_rows=1)
    payload = buf.getvalue()
    if b"\xff\xdd" in payload:
        write_pair("rgb_101x53_420_q80_rst", payload)
    else:
        print("  SKIPPED: no DRI segment in Pillow output")

    print("progressive fixture (SOF2 — must be rejected, so it needs no reference)")
    buf = io.BytesIO()
    Image.fromarray(source_rgb(64, 64), "RGB").save(
        buf, "JPEG", quality=90, subsampling=2, progressive=True
    )
    payload = buf.getvalue()
    path = os.path.join(HERE, "rgb_64x64_420_q90_progressive.jpg")
    with open(path, "wb") as fh:
        fh.write(payload)
    has_sof2 = bytes([0xFF, 0xC2]) in payload
    print(
        f"  rgb_64x64_420_q90_progressive    jpeg={len(payload):6d} "
        f"sampling={sof_sampling(path)} has_SOF2={has_sof2}"
    )

    print("photo fixture (800x600 4:2:0 q85) + 8x8 block-mean reference")
    photo = Image.fromarray(source_photo(800, 600), "RGB")
    buf = io.BytesIO()
    photo.save(buf, "JPEG", quality=85, subsampling=2, optimize=True)
    payload = buf.getvalue()
    jpeg_path = os.path.join(HERE, "photo_800x600_420_q85.jpg")
    with open(jpeg_path, "wb") as fh:
        fh.write(payload)
    decoded = np.asarray(Image.open(io.BytesIO(payload)).convert("RGB"), dtype=np.float64)
    blocks = decoded.reshape(600 // 8, 8, 800 // 8, 8, 3).mean(axis=(1, 3))
    ref = np.rint(blocks).clip(0, 255).astype(np.uint8)
    ref_path = os.path.join(HERE, "photo_800x600_420_q85.ref8.png")
    Image.fromarray(ref, "RGB").save(ref_path, optimize=True)
    print(
        f"  photo_800x600_420_q85            jpeg={len(payload):6d} "
        f"ref8={os.path.getsize(ref_path):6d} sampling={sof_sampling(jpeg_path)}"
    )

    total = 0
    for entry in os.listdir(HERE):
        if entry.endswith((".jpg", ".png")):
            total += os.path.getsize(os.path.join(HERE, entry))
    print(f"\ntotal fixture bytes: {total} ({total / 1024:.1f} KiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

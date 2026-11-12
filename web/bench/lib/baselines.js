// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Honest pure-JS baselines for the "HONEST BASELINE comparisons" suites.
 *
 * These are not strawmen: single pass over the pixel buffer, typed-array
 * storage throughout, integer fixed-point luma (no per-pixel closures, no
 * intermediate objects, no floating-point luma), grow-once output buffers
 * reused across calls exactly like the wasm wrappers' own discipline. They
 * are, however, plain scalar JS — no manual SIMD, no worker parallelism —
 * because that is what "pure JS" means to the overwhelming majority of
 * developers who would reach for a hand-rolled implementation instead of a
 * wasm module. Label results from this file as what they are: a reasonable
 * single-threaded scalar-JS implementation, not a tuned one.
 *
 * @module lib/baselines
 */

/**
 * BT.709 luma in fixed point, weights `54 + 183 + 19 == 256` so the `>> 8`
 * divide is exact-enough integer luma in `[0, 255]`.
 *
 * @param {number} r
 * @param {number} g
 * @param {number} b
 * @returns {number} Integer luma, `0..255`.
 */
function luma709(r, g, b) {
  return (r * 54 + g * 183 + b * 19) >> 8;
}

/**
 * A reusable luma-waveform baseline bound to a fixed output size, mirroring
 * `ScopeRenderer::waveform`'s `Luma` mode closely enough to be a fair
 * comparison: each input column maps to an output column, each input luma
 * maps to an output row (inverted — bright at the top), and repeated hits
 * accumulate before being normalized to a grayscale intensity image.
 */
export class WaveformBaseline {
  /**
   * @param {number} outWidth Output image width in pixels.
   * @param {number} outHeight Output image height in pixels.
   */
  constructor(outWidth, outHeight) {
    /** @readonly */
    this.outWidth = outWidth;
    /** @readonly */
    this.outHeight = outHeight;
    /** @private Grow-once accumulator, cleared (not reallocated) each run. */
    this._acc = new Uint32Array(outWidth * outHeight);
  }

  /**
   * Computes the waveform for `frame` and writes a grayscale RGBA8 image
   * into `out`.
   *
   * @param {Uint8ClampedArray} frame Tightly packed RGBA8 input,
   *   `width * height * 4` bytes.
   * @param {number} width Input frame width.
   * @param {number} height Input frame height.
   * @param {Uint8ClampedArray} out `outWidth * outHeight * 4` bytes,
   *   overwritten in place.
   */
  run(frame, width, height, out) {
    const acc = this._acc;
    acc.fill(0);
    const outW = this.outWidth;
    const outH = this.outHeight;

    for (let y = 0; y < height; y += 1) {
      const rowOff = y * width * 4;
      for (let x = 0; x < width; x += 1) {
        const o = rowOff + x * 4;
        const luma = luma709(frame[o], frame[o + 1], frame[o + 2]);
        const sx = ((x * outW) / width) | 0;
        const sy = (((255 - luma) * outH) / 256) | 0;
        acc[sy * outW + sx] += 1;
      }
    }

    let maxCount = 1;
    for (let i = 0; i < acc.length; i += 1) {
      if (acc[i] > maxCount) maxCount = acc[i];
    }
    const scale = 255 / maxCount;
    for (let i = 0; i < acc.length; i += 1) {
      const v = Math.min(255, (acc[i] * scale) | 0);
      const o = i * 4;
      out[o] = v;
      out[o + 1] = v;
      out[o + 2] = v;
      out[o + 3] = 255;
    }
  }
}

/**
 * A reusable luma-histogram baseline bound to a fixed output size, mirroring
 * `ScopeRenderer::histogram`'s `Luma` mode: a 256-bucket count of per-pixel
 * luma, rendered as a normalized bar chart.
 */
export class HistogramBaseline {
  /**
   * @param {number} outWidth Output image width in pixels.
   * @param {number} outHeight Output image height in pixels.
   */
  constructor(outWidth, outHeight) {
    /** @readonly */
    this.outWidth = outWidth;
    /** @readonly */
    this.outHeight = outHeight;
    /** @private Grow-once 256-bucket histogram, cleared each run. */
    this._hist = new Uint32Array(256);
  }

  /**
   * Computes the luma histogram for `frame` and renders it as a white
   * bar-chart on black into `out`.
   *
   * @param {Uint8ClampedArray} frame Tightly packed RGBA8 input,
   *   `width * height * 4` bytes.
   * @param {number} width Input frame width.
   * @param {number} height Input frame height.
   * @param {Uint8ClampedArray} out `outWidth * outHeight * 4` bytes,
   *   overwritten in place.
   */
  run(frame, width, height, out) {
    const hist = this._hist;
    hist.fill(0);
    const total = width * height;
    for (let i = 0; i < total; i += 1) {
      const o = i * 4;
      hist[luma709(frame[o], frame[o + 1], frame[o + 2])] += 1;
    }

    let maxCount = 1;
    for (let i = 0; i < 256; i += 1) {
      if (hist[i] > maxCount) maxCount = hist[i];
    }

    const outW = this.outWidth;
    const outH = this.outHeight;
    out.fill(0);
    for (let x = 0; x < outW; x += 1) {
      const bin = ((x * 256) / outW) | 0;
      const barHeight = Math.min(outH, ((hist[bin] / maxCount) * outH) | 0);
      for (let y = outH - barHeight; y < outH; y += 1) {
        const o = (y * outW + x) * 4;
        out[o] = 255;
        out[o + 1] = 255;
        out[o + 2] = 255;
        out[o + 3] = 255;
      }
    }
  }
}

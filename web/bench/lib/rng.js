// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Deterministic synthetic-frame generator for the benchmark harness.
 *
 * Every number this harness ever publishes has to be reproducible by a
 * stranger running `bench/run.sh` — including the *input pixels*. Browsers
 * give no stable "load a real photo" primitive without shipping binary test
 * assets (which would need to live somewhere, drift, and need licensing
 * review), so instead every frame is generated in-page from a fixed integer
 * seed using a public-domain PRNG. Same seed, same browser-independent
 * arithmetic (`Math.imul`, `>>>0` truncation — both exactly specified by
 * ECMA-262), same bytes, on any machine, forever.
 *
 * PRNG: `mulberry32` (public domain; Tommy Ettinger, 2017). A 32-bit
 * state, one `Math.imul`-based mixing step per call, `2^32` period-ish
 * quality — not cryptographic, not needed to be, just needs to be fast,
 * seedable, and identical across engines. Reference:
 * https://gist.github.com/tommyettinger/46a874533244883189143505d203312c
 *
 * @module lib/rng
 */

/**
 * Builds a `mulberry32` generator function from a 32-bit integer seed.
 *
 * @param {number} seed Any integer; only the low 32 bits are used.
 * @returns {() => number} A function returning a fresh float in `[0, 1)`
 *   on every call, advancing the generator's internal state.
 */
export function mulberry32(seed) {
  let a = seed >>> 0;
  return function next() {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Clamps `v` into `[0, 255]` and truncates to an integer, matching how the
 * eventual `Uint8ClampedArray` store would clamp it anyway — done explicitly
 * here so the *values* documented in comments match what lands in memory.
 *
 * @param {number} v
 * @returns {number}
 */
function clamp255(v) {
  if (v <= 0) return 0;
  if (v >= 255) return 255;
  return v | 0;
}

/**
 * Generates a deterministic "photographic-ish" RGBA8 frame: three
 * low-frequency sinusoidal bands (one per channel, out of phase with each
 * other so the result isn't grayscale) standing in for a smooth gradient
 * subject, plus independent per-channel `mulberry32` grain to stand in for
 * sensor noise. Alpha is always opaque (255).
 *
 * This is deliberately *not* trying to look like a real photo — it is
 * trying to be a cheap, dependency-free, exactly-reproducible stand-in with
 * the two properties the benchmarked kernels actually care about: smooth
 * low-frequency structure (so waveform/vectorscope traces aren't a single
 * degenerate point) and non-zero per-pixel variance (so histograms/SSIM
 * aren't measuring a constant-color fast path).
 *
 * @param {number} width Pixel width.
 * @param {number} height Pixel height.
 * @param {number} seed PRNG seed; same seed + dimensions => byte-identical
 *   output, on any machine.
 * @param {number} [noiseAmplitude=22] Peak +/- deviation added per channel
 *   before clamping.
 * @returns {Uint8ClampedArray} `width * height * 4` bytes, row-major RGBA,
 *   no row padding.
 */
export function generateGradientFrame(width, height, seed, noiseAmplitude = 22) {
  const out = new Uint8ClampedArray(width * height * 4);
  const rng = mulberry32(seed);
  const twoPi = Math.PI * 2;
  for (let y = 0; y < height; y += 1) {
    const fy = y / height;
    const rowOff = y * width * 4;
    for (let x = 0; x < width; x += 1) {
      const fx = x / width;
      const o = rowOff + x * 4;

      const rBase = 128 + 105 * Math.sin(twoPi * (fx * 1.3 + fy * 0.4));
      const gBase = 128 + 105 * Math.sin(twoPi * (fx * 0.5 - fy * 1.1 + 0.33));
      const bBase = 128 + 105 * Math.sin(twoPi * (fx * 0.9 + fy * 0.9 + 0.66));

      // Independent RNG draws per channel decorrelate the grain (real
      // sensor noise is not perfectly correlated across channels either).
      out[o] = clamp255(rBase + (rng() - 0.5) * 2 * noiseAmplitude);
      out[o + 1] = clamp255(gBase + (rng() - 0.5) * 2 * noiseAmplitude);
      out[o + 2] = clamp255(bBase + (rng() - 0.5) * 2 * noiseAmplitude);
      out[o + 3] = 255;
    }
  }
  return out;
}

/**
 * Generates a deterministic "worst case" RGBA8 frame: independent uniform
 * `mulberry32` noise on every channel of every pixel, with no spatial or
 * cross-channel structure at all. Maximum entropy, zero coherence for any
 * kernel to exploit — a stress input, not a realistic one.
 *
 * @param {number} width Pixel width.
 * @param {number} height Pixel height.
 * @param {number} seed PRNG seed; same seed + dimensions => byte-identical
 *   output, on any machine.
 * @returns {Uint8ClampedArray} `width * height * 4` bytes, row-major RGBA,
 *   no row padding.
 */
export function generateNoiseFrame(width, height, seed) {
  const out = new Uint8ClampedArray(width * height * 4);
  const rng = mulberry32(seed);
  const total = width * height;
  for (let i = 0; i < total; i += 1) {
    const o = i * 4;
    out[o] = (rng() * 256) | 0;
    out[o + 1] = (rng() * 256) | 0;
    out[o + 2] = (rng() * 256) | 0;
    out[o + 3] = 255;
  }
  return out;
}

/**
 * Derives a deterministic "distorted" sibling of `reference` by adding
 * independent per-channel `mulberry32` noise of up to `+/-amplitude`,
 * clamped. Alpha is copied through unchanged. Used to build a non-trivial
 * (neither bit-identical nor unrelated) reference/distorted pair for the
 * PSNR/SSIM suite — a real quality metric on two identical frames is an
 * infinity/1.0 degenerate case, which is not what most callers measure.
 *
 * @param {Uint8ClampedArray} reference Source RGBA8 buffer (not mutated).
 * @param {number} seed PRNG seed for the perturbation, independent from
 *   whatever seed produced `reference`.
 * @param {number} [amplitude=26] Peak +/- deviation added per channel
 *   before clamping.
 * @returns {Uint8ClampedArray} A new buffer, same length as `reference`.
 */
export function deriveDistorted(reference, seed, amplitude = 26) {
  const out = new Uint8ClampedArray(reference.length);
  const rng = mulberry32(seed);
  for (let i = 0; i < reference.length; i += 4) {
    out[i] = clamp255(reference[i] + (rng() - 0.5) * 2 * amplitude);
    out[i + 1] = clamp255(reference[i + 1] + (rng() - 0.5) * 2 * amplitude);
    out[i + 2] = clamp255(reference[i + 2] + (rng() - 0.5) * 2 * amplitude);
    out[i + 3] = reference[i + 3];
  }
  return out;
}

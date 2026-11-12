// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * PSNR + SSIM video quality metrics, backed by `oximedia-web-quality`.
 *
 * `Quality.create({ width, height })` binds an analyzer to a fixed frame
 * size (every working buffer is preallocated wasm-side; `compare` /
 * `ssimMapToCanvas` never allocate beyond one small result object per
 * call). `compare` accepts the same frame sources as {@link frameToRgba}
 * (`VideoFrame`, `<video>`, canvas, `ImageBitmap`) plus two convenience
 * shapes for already-decoded pixels: a raw `Uint8Array`/`Uint8ClampedArray`
 * of `width * height * 4` bytes, or a `{ data, width, height }` object
 * (an `RgbaFrame`, e.g. one already returned by `frameToRgba`).
 *
 * No JSON on the per-frame path: the wasm `compare()` call returns typed
 * getters (`psnr_rgb`/`psnr_luma`/`ssim`), which this wrapper reshapes into
 * a plain `{ psnrRgb, psnrLuma, ssim }` object — one small allocation per
 * call, no serialization.
 *
 * @module quality
 */

import init, { Quality as WasmQuality } from "./wasm/quality/oximedia_web_quality.js";
import { frameToRgba } from "./_frame.js";

/** Module-scope wasm init promise, shared by every {@link Quality} instance. */
let initPromise = null;

/**
 * Initializes the `oximedia-web-quality` wasm module exactly once, no
 * matter how many {@link Quality} instances are created.
 * @returns {Promise<void>}
 */
function ensureWasmInit() {
  if (!initPromise) {
    initPromise = init();
  }
  return initPromise;
}

/**
 * Validates and asserts positive integer frame dimensions.
 * @param {number} width
 * @param {number} height
 * @throws {Error} If either is not a positive integer.
 */
function assertDims(width, height) {
  if (!Number.isInteger(width) || !Number.isInteger(height) || width <= 0 || height <= 0) {
    throw new Error(
      `Quality: width/height must be positive integers, got ${width}x${height}`,
    );
  }
}

/**
 * Resolves any accepted `compare`/`ssimMapToCanvas` input into a tightly
 * packed RGBA8 view, without allocating when the caller already handed us
 * one.
 *
 * @param {*} source
 * @param {number} width Analyzer's bound width.
 * @param {number} height Analyzer's bound height.
 * @param {import("./_frame.js").FrameState} state Reused across calls with
 *   the same logical source.
 * @param {string} label Used in error messages (e.g. `"reference"`).
 * @returns {Promise<Uint8Array|Uint8ClampedArray>}
 * @throws {Error} If a resolved frame's dimensions don't match `width x height`.
 */
async function resolveRgba(source, width, height, state, label) {
  if (source instanceof Uint8Array || source instanceof Uint8ClampedArray) {
    return source;
  }
  if (
    source &&
    typeof source === "object" &&
    (source.data instanceof Uint8Array || source.data instanceof Uint8ClampedArray) &&
    typeof source.width === "number" &&
    typeof source.height === "number"
  ) {
    if (source.width !== width || source.height !== height) {
      throw new Error(
        `Quality: ${label} is ${source.width}x${source.height}, expected ${width}x${height}`,
      );
    }
    return source.data;
  }

  const frame = await frameToRgba(source, state);
  if (frame.width !== width || frame.height !== height) {
    throw new Error(
      `Quality: ${label} decoded to ${frame.width}x${frame.height}, expected ${width}x${height}`,
    );
  }
  return frame.data;
}

/**
 * PSNR + SSIM comparison result for one {@link Quality#compare} call.
 *
 * @typedef {Object} QualityMetrics
 * @property {number} psnrRgb PSNR (dB) over the RGB channels (alpha
 *   ignored); `Infinity` for bit-identical frames.
 * @property {number} psnrLuma PSNR (dB) over BT.709 full-range luma;
 *   `Infinity` for bit-identical frames.
 * @property {number} ssim Mean windowed SSIM (`1.0` for bit-identical
 *   frames, generally `[0, 1]` for real distortions).
 */

/**
 * PSNR + SSIM analyzer bound to a fixed `width x height` RGBA8 frame size.
 */
export class Quality {
  /** @type {InstanceType<typeof WasmQuality>} */
  #wasm;
  /** @type {number} */
  #width;
  /** @type {number} */
  #height;
  /** @type {import("./_frame.js").FrameState} */
  #stateA = {};
  /** @type {import("./_frame.js").FrameState} */
  #stateB = {};
  /** @type {Uint8ClampedArray|undefined} */
  #heatmap;
  /** @type {ImageData|undefined} */
  #heatmapImageData;

  /**
   * @param {InstanceType<typeof WasmQuality>} wasmQuality
   * @param {number} width
   * @param {number} height
   * @private Use {@link Quality.create} instead.
   */
  constructor(wasmQuality, width, height) {
    this.#wasm = wasmQuality;
    this.#width = width;
    this.#height = height;
  }

  /**
   * Creates an analyzer for `width x height` RGBA8 frames, initializing the
   * wasm module on first call (subsequent calls, for this or any other
   * `Quality` instance, reuse the same initialization).
   *
   * @param {{ width: number, height: number }} options
   * @returns {Promise<Quality>}
   * @throws {Error} If `width`/`height` are not positive integers, or are
   *   too small for the fixed 11-pixel SSIM window (wasm-thrown).
   */
  static async create({ width, height }) {
    assertDims(width, height);
    await ensureWasmInit();
    const wasmQuality = new WasmQuality(width, height);
    return new Quality(wasmQuality, width, height);
  }

  /** Frame width this analyzer was created for. */
  get width() {
    return this.#width;
  }

  /** Frame height this analyzer was created for. */
  get height() {
    return this.#height;
  }

  /**
   * Computes PSNR (RGB + luma) and mean SSIM between two frames.
   *
   * @param {*} sourceA Reference frame — any {@link frameToRgba} source, a
   *   raw `Uint8Array`/`Uint8ClampedArray` of `width * height * 4` bytes, or
   *   an `RgbaFrame`-shaped `{ data, width, height }` object.
   * @param {*} sourceB Distorted frame — same accepted shapes as `sourceA`.
   * @returns {Promise<QualityMetrics>}
   * @throws {Error} If either source doesn't resolve to `width x height`,
   *   or (wasm-thrown) doesn't resolve to exactly `width * height * 4` bytes.
   */
  async compare(sourceA, sourceB) {
    const a = await resolveRgba(sourceA, this.#width, this.#height, this.#stateA, "sourceA");
    const b = await resolveRgba(sourceB, this.#width, this.#height, this.#stateB, "sourceB");
    const result = this.#wasm.compare(a, b);
    try {
      return {
        psnrRgb: result.psnr_rgb,
        psnrLuma: result.psnr_luma,
        ssim: result.ssim,
      };
    } finally {
      result.free();
    }
  }

  /**
   * Computes the SSIM heatmap between two frames (red = dissimilar, green =
   * similar) and paints it into `canvas` via `putImageData`, resizing the
   * canvas to `width x height` first if needed. Returns the mean SSIM (the
   * same value {@link Quality#compare} would report).
   *
   * The caller owns `canvas`: this method never creates DOM nodes, only
   * reads/writes the one it is given.
   *
   * @param {*} sourceA Reference frame — same accepted shapes as {@link Quality#compare}.
   * @param {*} sourceB Distorted frame — same accepted shapes as {@link Quality#compare}.
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas Caller-owned target canvas.
   * @returns {Promise<number>} Mean SSIM.
   * @throws {Error} If `canvas` has no usable 2D context, or (as {@link Quality#compare}).
   */
  async ssimMapToCanvas(sourceA, sourceB, canvas) {
    const a = await resolveRgba(sourceA, this.#width, this.#height, this.#stateA, "sourceA");
    const b = await resolveRgba(sourceB, this.#width, this.#height, this.#stateB, "sourceB");

    if (!this.#heatmap) {
      this.#heatmap = new Uint8ClampedArray(this.#width * this.#height * 4);
      this.#heatmapImageData = new ImageData(this.#heatmap, this.#width, this.#height);
    }
    const mean = this.#wasm.ssim_map(a, b, this.#heatmap);

    if (canvas.width !== this.#width || canvas.height !== this.#height) {
      canvas.width = this.#width;
      canvas.height = this.#height;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      throw new Error("Quality.ssimMapToCanvas: failed to acquire a 2D context on `canvas`");
    }
    ctx.putImageData(this.#heatmapImageData, 0, 0);
    return mean;
  }

  /**
   * Releases the underlying wasm object. The instance must not be used
   * afterwards. Safe to omit if you'd rather let the `FinalizationRegistry`
   * reclaim it (slower, non-deterministic).
   */
  free() {
    this.#wasm.free();
  }
}

// Enables `using quality = await Quality.create(...)` (explicit resource
// management) where the engine supports it; mirrors the guard the
// generated wasm glue itself uses for its own classes.
if (Symbol.dispose) {
  Quality.prototype[Symbol.dispose] = Quality.prototype.free;
}

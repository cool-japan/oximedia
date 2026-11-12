// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * `@cooljapan/oximedia-web/color` — the colour-science pipeline for
 * WebCodecs frames: exposure / contrast / saturation, tone mapping
 * (Reinhard / Filmic-Hable / ACES with peak-nits control), primaries-aware
 * gamut mapping (bt709 / bt2020 / display-p3), transfer functions
 * (sRGB / PQ / HLG / linear) and 3D LUTs (.cube load, trilinear or
 * tetrahedral apply, export), running in WebAssembly.
 *
 * Fixed operator order: input-transfer decode → exposure → contrast →
 * saturation → tone map → gamut → output-transfer encode → 3D LUT.
 *
 * Data-plane discipline: pixels cross the JS/wasm boundary only as
 * `Uint8Array` (SDR) or `Float32Array` (HDR) — one copy in, one copy out —
 * and all buffers on this side are persistent per-instance (grow-once), so a
 * steady render loop does not allocate.
 *
 * @module color
 */

import init, {
  ColorPipeline as WasmColorPipeline,
  CubeLut as WasmCubeLut,
} from "./wasm/color/oximedia_web_color.js";
import { frameToRgba } from "./_frame.js";

/** @type {Promise<unknown>|null} Lazily started wasm init, shared module-wide. */
let initPromise = null;

/** Starts (once) and returns the wasm module initialisation promise. */
function ensureInit() {
  if (!initPromise) {
    initPromise = init();
  }
  return initPromise;
}

/**
 * A parsed `.cube` 3D LUT.
 *
 * Construct via {@link loadCubeLut}; hand instances to
 * {@link ColorPipeline#lut}.
 */
export class CubeLut {
  /**
   * @param {WasmCubeLut} inner Wasm-side LUT (internal; use loadCubeLut).
   */
  constructor(inner) {
    /** @private */
    this._inner = inner;
  }

  /** Lattice size per axis (2–129). @returns {number} */
  get size() {
    return this._inner.size();
  }

  /** LUT title from the `TITLE` line, or `null`. @returns {string|null} */
  get title() {
    const t = this._inner.title();
    return t === undefined ? null : t;
  }

  /**
   * Serialises back to `.cube` text (R-fastest data order — the
   * Adobe/ffmpeg convention).
   * @returns {string}
   */
  export() {
    return this._inner.export();
  }

  /** Releases the wasm-side memory. The instance is unusable afterwards. */
  free() {
    this._inner.free();
  }
}

/**
 * Parses a `.cube` LUT from a string, `Uint8Array` or `ArrayBuffer`
 * (UTF-8). Hostile input rejects with a descriptive `Error` — the parser
 * never crashes the wasm instance.
 *
 * @param {string|Uint8Array|ArrayBuffer} bytesOrString
 * @returns {Promise<CubeLut>}
 */
export async function loadCubeLut(bytesOrString) {
  await ensureInit();
  let text;
  if (typeof bytesOrString === "string") {
    text = bytesOrString;
  } else if (bytesOrString instanceof ArrayBuffer) {
    text = new TextDecoder().decode(new Uint8Array(bytesOrString));
  } else if (ArrayBuffer.isView(bytesOrString)) {
    text = new TextDecoder().decode(bytesOrString);
  } else {
    throw new Error(
      "loadCubeLut: expected a string, Uint8Array or ArrayBuffer",
    );
  }
  return new CubeLut(WasmCubeLut.parse(text));
}

/**
 * The colour pipeline. Create with {@link ColorPipeline.create}; all
 * configuration methods are chainable.
 *
 * ```js
 * const pipe = await ColorPipeline.create();
 * pipe.exposure(0.7).contrast(1.1).saturation(1.0)
 *     .toneMap('aces', { peakNits: 100, inputPeakNits: 1000 })
 *     .gamut('bt2020', 'bt709')
 *     .transfer({ in: 'hlg', out: 'srgb' });
 * const outFrame = await pipe.apply(videoFrame); // → VideoFrame
 * ```
 */
export class ColorPipeline {
  /**
   * @param {WasmColorPipeline} inner Wasm-side pipeline (internal; use create()).
   */
  constructor(inner) {
    /** @private */
    this._inner = inner;
    /** @private frame acquisition state for _frame.js (grow-once). */
    this._state = {};
    /** @private cached ImageData for applyToCanvas (recreated on resize). */
    this._imageData = null;
    /** @private */
    this._imageWidth = 0;
    /** @private */
    this._imageHeight = 0;
  }

  /**
   * Initialises the wasm module (once, shared) and returns a fresh identity
   * pipeline (sRGB in, sRGB out, neutral ops).
   * @returns {Promise<ColorPipeline>}
   */
  static async create() {
    await ensureInit();
    return new ColorPipeline(new WasmColorPipeline());
  }

  /**
   * Sets exposure in photographic stops (gain = 2^stops, applied in linear
   * light). 0 is neutral.
   * @param {number} stops
   * @returns {this}
   */
  exposure(stops) {
    this._inner.set_exposure(stops);
    return this;
  }

  /**
   * Sets contrast (power law around the 0.18 linear pivot). 1.0 is neutral.
   * @param {number} value In (0, 10].
   * @returns {this}
   */
  contrast(value) {
    this._inner.set_contrast(value);
    return this;
  }

  /**
   * Sets saturation (BT.709 luma blend in linear light). 1.0 is neutral,
   * 0 is monochrome.
   * @param {number} value In [0, 10].
   * @returns {this}
   */
  saturation(value) {
    this._inner.set_saturation(value);
    return this;
  }

  /**
   * Enables tone mapping, or disables it when `op` is `null`.
   *
   * Operators: `'reinhard'`, `'reinhard-extended'`, `'hable'` (alias
   * `'filmic'`), `'aces'` and `'aces-odt'`. Honesty note: `'aces'` is the
   * Narkowicz-2015 *fitted* ACES curve (luminance-based, hue-preserving);
   * `'aces-odt'` is the ACES Output-Transform-2.0-*shaped* per-channel RRT
   * with parametric gamut compression ported from OxiMedia's `AcesOt2` —
   * neither is the bit-exact Academy CTL reference.
   *
   * @param {('reinhard'|'reinhard-extended'|'hable'|'filmic'|'aces'|'aces-odt')|null} op
   * @param {{ peakNits?: number, inputPeakNits?: number }} [opts]
   *   `peakNits` — target display peak (default 100);
   *   `inputPeakNits` — luminance meant by linear 1.0 on input
   *   (default 1000; use 10000 for PQ-decoded content).
   * @returns {this}
   */
  toneMap(op, opts = {}) {
    if (op === null) {
      this._inner.clear_tone_map();
      return this;
    }
    const peakNits = opts.peakNits ?? 100;
    const inputPeakNits = opts.inputPeakNits ?? 1000;
    this._inner.set_tone_map(op, inputPeakNits, peakNits);
    return this;
  }

  /**
   * Enables gamut conversion between `'bt709'`, `'bt2020'` and
   * `'display-p3'` (aliases: `'rec709'`, `'srgb'`, `'rec2020'`, `'p3'`, …),
   * or disables it when `src` is `null`.
   *
   * Out-of-gamut colours are fixed hue-preservingly (negative channels are
   * desaturated toward luma); HDR values above 1.0 survive by default. Pass
   * a `softness` in (0, 1] to additionally soft-clip highlights into [0, 1].
   *
   * @param {('bt709'|'bt2020'|'display-p3'|string)|null} src
   * @param {('bt709'|'bt2020'|'display-p3'|string)} [dst]
   * @param {{ softness?: number }} [opts]
   * @returns {this}
   */
  gamut(src, dst, opts = {}) {
    if (src === null) {
      this._inner.clear_gamut();
      return this;
    }
    this._inner.set_gamut(src, dst);
    if (opts.softness !== undefined) {
      this._inner.set_gamut_softness(opts.softness);
    }
    return this;
  }

  /**
   * Enables the 3D-LUT stage (applied on encoded output values, the
   * standard creative-LUT convention), or disables it when `cubeLut` is
   * `null`.
   *
   * @param {CubeLut|null} cubeLut
   * @param {{ interp?: 'trilinear'|'tetrahedral' }} [opts]
   * @returns {this}
   */
  lut(cubeLut, opts = {}) {
    if (cubeLut === null) {
      this._inner.clear_lut();
      return this;
    }
    const interp = opts.interp ?? "tetrahedral";
    this._inner.set_lut(cubeLut._inner, interp);
    return this;
  }

  /**
   * Sets the input/output transfer functions: `'srgb'`, `'pq'`, `'hlg'` or
   * `'linear'`. Omitted sides are left unchanged.
   *
   * Normalisation: PQ linear 1.0 = 10 000 nits; HLG decode applies an
   * OOTF-lite (`x^1.2`); `'linear'` passes floats through unclamped.
   *
   * @param {{ in?: 'srgb'|'pq'|'hlg'|'linear', out?: 'srgb'|'pq'|'hlg'|'linear' }} opts
   * @returns {this}
   */
  transfer(opts) {
    if (opts.in !== undefined) {
      this._inner.set_input_transfer(opts.in);
    }
    if (opts.out !== undefined) {
      this._inner.set_output_transfer(opts.out);
    }
    return this;
  }

  /**
   * Bakes the whole pipeline (including its LUT stage) into `.cube` text —
   * the encoded-in → encoded-out map. Loading the result into a fresh
   * pipeline reproduces this one up to lattice interpolation error.
   *
   * @param {{ size?: number }} [opts] Lattice size 2–129 (default 33).
   * @returns {string}
   */
  exportCube(opts = {}) {
    const size = opts.size ?? 33;
    return this._inner.export_cube(size);
  }

  /**
   * Runs the pipeline on any browser image source and returns the processed
   * RGBA bytes plus dimensions (shared plumbing for apply/applyToCanvas).
   *
   * The frame's persistent acquisition buffer is processed **in place**
   * (one wasm copy in, one copy back — no second JS-side buffer), which is
   * safe because both `VideoFrame` and `putImageData` consumers copy the
   * bytes onward before the next frame reuses the buffer.
   *
   * @private
   * @param {*} source
   * @returns {Promise<{ out: Uint8ClampedArray, width: number, height: number }>}
   */
  async _process(source) {
    const { data, width, height } = await frameToRgba(source, this._state);
    this._inner.apply_in_place(data, width, height);
    return { out: data, width, height };
  }

  /**
   * Processes a frame and returns a new `VideoFrame` (RGBA). The caller
   * owns both the input (never closed here) and the returned frame.
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @returns {Promise<VideoFrame>}
   */
  async apply(source) {
    if (typeof VideoFrame === "undefined") {
      throw new Error(
        "ColorPipeline.apply: VideoFrame is not available in this browser — " +
          "use applyToCanvas(source, canvas) instead",
      );
    }
    const { out, width, height } = await this._process(source);
    return new VideoFrame(out, {
      format: "RGBA",
      codedWidth: width,
      codedHeight: height,
      timestamp: source?.timestamp ?? 0,
    });
  }

  /**
   * Processes a frame and paints it into a caller-provided canvas via
   * `putImageData` (non-WebCodecs path). Never creates DOM nodes.
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @returns {Promise<void>}
   */
  async applyToCanvas(source, canvas) {
    const { out, width, height } = await this._process(source);
    if (canvas.width !== width) {
      canvas.width = width;
    }
    if (canvas.height !== height) {
      canvas.height = height;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      throw new Error("ColorPipeline.applyToCanvas: no 2D context available");
    }
    if (
      !this._imageData ||
      this._imageWidth !== width ||
      this._imageHeight !== height ||
      this._imageData.data.buffer !== out.buffer ||
      this._imageData.data.byteOffset !== out.byteOffset
    ) {
      this._imageData = new ImageData(out, width, height);
      this._imageWidth = width;
      this._imageHeight = height;
    }
    ctx.putImageData(this._imageData, 0, 0);
  }

  /**
   * HDR path: applies the pipeline to a tightly packed RGBA `Float32Array`
   * using the exact transfer curves. Synchronous; the caller provides both
   * buffers (`width × height × 4` floats each) and reuses them across
   * frames.
   *
   * @param {Float32Array} src
   * @param {Float32Array} dst
   * @param {number} width
   * @param {number} height
   * @returns {this}
   */
  applyF32(src, dst, width, height) {
    this._inner.apply_f32(src, dst, width, height);
    return this;
  }

  /** Releases the wasm-side memory. The instance is unusable afterwards. */
  free() {
    this._inner.free();
  }
}

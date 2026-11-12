// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * `@cooljapan/oximedia-web/scopes` — broadcast-grade video scopes (waveform,
 * vectorscope, histogram, false-colour exposure) for WebCodecs / `<video>` /
 * canvas frames.
 *
 * Give it any browser image source and a 2D canvas; it acquires the frame's
 * RGBA8 pixels (via {@link module:_frame.frameToRgba}, one copy), loads them
 * into the wasm renderer once, runs the wasm kernel against that resident
 * frame and paints the result with `putImageData`. Rendering several scopes
 * of the same immutable `VideoFrame` (the standard dashboard layout) pays the
 * acquisition + wasm-boundary copy once per frame, not once per scope.
 * The caller owns the canvas — the wrapper never creates DOM nodes. All
 * per-instance buffers (the input-frame scratch and the output `ImageData`) are
 * reused across frames and only reallocated when the scope size changes, so a
 * steady render loop does not allocate.
 *
 * Import path (matches the `@cooljapan/oximedia-web` package's `./scopes`
 * export). The wasm glue lives beside this file in `dist/`:
 *
 * ```js
 * import { Scopes } from '@cooljapan/oximedia-web/scopes';
 * const scopes = await Scopes.create({ width: 512, height: 256 });
 * scopes.waveform(videoFrame, canvas, { mode: 'rgb-parade', ire: true });
 * ```
 *
 * @module scopes
 */

import init, { Scopes as WasmScopes } from './wasm/scopes/oximedia_web_scopes.js';
import { frameToRgba } from './_frame.js';

/**
 * Module-scope singleton for the wasm module's async initialisation. The first
 * {@link Scopes.create} kicks off `init()`; subsequent calls await the same
 * promise, so the module is fetched and instantiated exactly once.
 * @type {Promise<unknown>|null}
 */
let initPromise = null;

/**
 * Ensures the wasm module is initialised, returning the shared init promise.
 * @returns {Promise<unknown>}
 */
function ensureInit() {
  if (!initPromise) {
    initPromise = init();
  }
  return initPromise;
}

/** Waveform mode string → wasm selector. @type {Record<string, number>} */
const WAVEFORM_MODES = {
  luma: 0,
  'rgb-parade': 1,
  'rgb-overlay': 2,
  ycbcr: 3,
};

/** Histogram mode string → wasm selector. @type {Record<string, number>} */
const HISTOGRAM_MODES = { luma: 0, rgb: 1 };

/** False-colour preset string → wasm selector. @type {Record<string, number>} */
const FALSE_COLOR_PRESETS = { spectrum: 0, arri: 1 };

/**
 * Looks up a mode in a table, throwing a helpful error for typos.
 * @param {Record<string, number>} table
 * @param {string} key
 * @param {string} what
 * @returns {number}
 */
function resolveMode(table, key, what) {
  const v = table[key];
  if (v === undefined) {
    throw new Error(
      `Scopes: unknown ${what} '${key}'; expected one of ${Object.keys(table).join(', ')}`,
    );
  }
  return v;
}

/**
 * A video-scope renderer bound to a fixed output size.
 *
 * Construct with the async {@link Scopes.create} factory (it lazily initialises
 * the wasm module), then call a render method per frame. Each render method
 * takes a frame `source`, the destination `canvas`, and per-call options.
 */
export class Scopes {
  /**
   * @param {WasmScopes} inner Initialised wasm renderer.
   * @param {number} width Scope canvas width.
   * @param {number} height Scope canvas height.
   * @param {boolean} graticule Default graticule preference.
   * @private
   */
  constructor(inner, width, height, graticule) {
    /** @type {WasmScopes} @private */
    this._inner = inner;
    /** @type {number} */
    this.width = width;
    /** @type {number} */
    this.height = height;
    /** @type {boolean} @private */
    this._graticule = graticule;
    /** @type {Record<string, unknown>} @private Reusable frame-acquisition state. */
    this._frameState = {};
    /**
     * @type {VideoFrame|null} @private
     * The `VideoFrame` whose pixels are currently resident in the wasm
     * renderer (see {@link Scopes#_loadFrame}). `VideoFrame`s are immutable,
     * so identity + timestamp equality proves the resident copy is current.
     */
    this._residentSource = null;
    /** @type {number} @private Timestamp of `_residentSource` at load time. */
    this._residentTimestamp = 0;
    /** @type {ImageData} @private Persistent output image (reused each frame). */
    this._image = new ImageData(width, height);
    /** @type {(HTMLCanvasElement|OffscreenCanvas)|null} @private */
    this._canvas = null;
    /** @type {CanvasRenderingContext2D|OffscreenCanvasRenderingContext2D|null} @private */
    this._ctx = null;
  }

  /**
   * Creates a scope renderer, initialising the wasm module on first use.
   *
   * @param {Object} [options]
   * @param {number} [options.width=512] Output canvas width in pixels.
   * @param {number} [options.height=256] Output canvas height in pixels.
   * @param {boolean} [options.graticule=true] Default overlay preference used by
   *   the vectorscope / histogram (waveform's `ire` option overrides per call).
   * @returns {Promise<Scopes>}
   */
  static async create({ width = 512, height = 256, graticule = true } = {}) {
    await ensureInit();
    const inner = new WasmScopes(width, height, graticule);
    return new Scopes(inner, width, height, graticule);
  }

  /**
   * Resolves (and caches) a 2D context for `canvas`, sizing the canvas backing
   * store to the scope dimensions on first bind or a canvas swap.
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @returns {CanvasRenderingContext2D|OffscreenCanvasRenderingContext2D}
   * @private
   */
  _context(canvas) {
    if (canvas !== this._canvas || !this._ctx) {
      this._canvas = canvas;
      this._ctx = canvas.getContext('2d');
      if (!this._ctx) {
        throw new Error('Scopes: failed to acquire a 2D context on the canvas');
      }
    }
    if (canvas.width !== this.width || canvas.height !== this.height) {
      canvas.width = this.width;
      canvas.height = this.height;
    }
    return this._ctx;
  }

  /**
   * Makes `source`'s pixels resident in the wasm renderer, skipping both the
   * browser-side acquisition and the JS→wasm copy when the same immutable
   * `VideoFrame` is already resident.
   *
   * A multi-scope dashboard renders several scopes of the *same* frame per
   * tick; paying `frameToRgba` plus a full-frame boundary copy once per
   * scope quadruples the per-frame frame traffic. `VideoFrame` pixels are
   * immutable, so identity + timestamp equality is a sound cache key. All
   * other source kinds (canvases, `<video>` elements, bitmaps) are mutable
   * in place and are re-acquired on every call, exactly as before.
   *
   * @param {*} source
   * @returns {Promise<void>}
   * @private
   */
  async _loadFrame(source) {
    const isVideoFrame = typeof VideoFrame !== 'undefined' && source instanceof VideoFrame;
    if (
      isVideoFrame &&
      this._residentSource === source &&
      this._residentTimestamp === source.timestamp &&
      this._inner.has_frame
    ) {
      return;
    }
    const frame = await frameToRgba(source, this._frameState);
    this._inner.load_frame(frame.data, frame.width, frame.height);
    if (isVideoFrame) {
      this._residentSource = source;
      this._residentTimestamp = source.timestamp;
    } else {
      this._residentSource = null;
    }
  }

  /**
   * Makes `source` resident, runs `render(out)` against the resident frame,
   * and paints the persistent output image into `canvas`.
   * @param {*} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @param {(out: Uint8ClampedArray) => void} render
   * @returns {Promise<void>}
   * @private
   */
  async _paint(source, canvas, render) {
    const ctx = this._context(canvas);
    await this._loadFrame(source);
    render(this._image.data);
    ctx.putImageData(this._image, 0, 0);
  }

  /**
   * Renders a waveform monitor.
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @param {Object} [options]
   * @param {'luma'|'rgb-parade'|'rgb-overlay'|'ycbcr'} [options.mode='rgb-parade']
   * @param {boolean} [options.ire=true] Draw the IRE graticule + labels.
   * @returns {Promise<void>}
   */
  async waveform(source, canvas, { mode = 'rgb-parade', ire = true } = {}) {
    const m = resolveMode(WAVEFORM_MODES, mode, 'waveform mode');
    await this._paint(source, canvas, (out) => {
      this._inner.waveform_current(m, ire, out);
    });
  }

  /**
   * Renders a vectorscope.
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @param {Object} [options]
   * @param {number} [options.gain=1.0] Trace magnification (zoom).
   * @param {boolean} [options.skinToneLine=true] Draw the 123-degree skin-tone
   *   / +I reference line.
   * @param {boolean} [options.graticule] Overlay the SMPTE graticule (defaults
   *   to the instance's `graticule` preference).
   * @returns {Promise<void>}
   */
  async vectorscope(
    source,
    canvas,
    { gain = 1.0, skinToneLine = true, graticule = this._graticule } = {},
  ) {
    await this._paint(source, canvas, (out) => {
      this._inner.vectorscope_current(gain, skinToneLine, graticule, out);
    });
  }

  /**
   * Renders a histogram.
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @param {Object} [options]
   * @param {'luma'|'rgb'} [options.mode='luma']
   * @param {boolean} [options.graticule] Overlay the graticule (defaults to the
   *   instance's `graticule` preference).
   * @returns {Promise<void>}
   */
  async histogram(source, canvas, { mode = 'luma', graticule = this._graticule } = {}) {
    const m = resolveMode(HISTOGRAM_MODES, mode, 'histogram mode');
    await this._paint(source, canvas, (out) => {
      this._inner.histogram_current(m, graticule, out);
    });
  }

  /**
   * Renders a false-colour exposure map (always at the configured scope size).
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @param {Object} [options]
   * @param {'spectrum'|'arri'} [options.preset='arri']
   * @returns {Promise<void>}
   */
  async falseColor(source, canvas, { preset = 'arri' } = {}) {
    const p = resolveMode(FALSE_COLOR_PRESETS, preset, 'false-colour preset');
    await this._paint(source, canvas, (out) => {
      this._inner.false_color_current(p, out);
    });
  }

  /**
   * Computes luma statistics for a frame (no canvas render).
   *
   * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
   * @returns {Promise<{minLuma: number, maxLuma: number, avgLuma: number, stdDev: number, blackClipPercent: number, whiteClipPercent: number}>}
   */
  async stats(source) {
    await this._loadFrame(source);
    const s = this._inner.stats_current();
    try {
      return {
        minLuma: s.min_luma,
        maxLuma: s.max_luma,
        avgLuma: s.avg_luma,
        stdDev: s.std_dev,
        blackClipPercent: s.black_clip_percent,
        whiteClipPercent: s.white_clip_percent,
      };
    } finally {
      s.free();
    }
  }

  /**
   * Releases the wasm renderer. Call when the scope is no longer needed; the
   * instance must not be used afterwards.
   */
  free() {
    this._inner.free();
    this._ctx = null;
    this._canvas = null;
    this._residentSource = null;
  }
}

export default Scopes;

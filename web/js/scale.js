// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * High-level resampling wrapper around the `oximedia-web-scale` wasm
 * module: professional separable resizing (Lanczos3, Catmull-Rom, Mitchell,
 * bilinear) of any browser video/image source to a fixed destination
 * resolution.
 *
 * There is no default export and no dependency beyond `./_frame.js` and
 * the generated wasm glue: drop it on any static `http.server`, no
 * COOP/COEP headers, no bundler required.
 *
 * @module scale
 */

import init, { Scaler as WasmScaler } from './wasm/scale/oximedia_web_scale.js';
import { frameToRgba } from './_frame.js';

/** Default separable filter kernel (see `ScalerOptions.filter`). */
const DEFAULT_FILTER = 'lanczos3';

/** Default premultiply-alpha setting (see `ScalerOptions.premultiply`). */
const DEFAULT_PREMULTIPLY = true;

/**
 * Upper bound on the number of per-source-resolution wasm `Scaler`
 * instances a single {@link Scaler} wrapper keeps alive at once. Bounds
 * memory for streams that flap between a handful of resolutions (e.g.
 * adaptive-bitrate resolution switches) without rebuilding weight tables on
 * every flap back to a previously-seen size.
 */
const MAX_CACHED_WASM_SCALERS = 4;

/**
 * Module-scope cache of the in-flight/completed wasm module init promise,
 * shared by every {@link Scaler} instance so concurrent construction only
 * fetches/instantiates the module once.
 * @type {Promise<unknown>|null}
 */
let wasmReady = null;

/**
 * Lazily initializes the wasm module. Idempotent and safe to call from
 * every entry point (`create`, `resize`, `resizeToCanvas`): the first call
 * starts the fetch/instantiate, every later call (even concurrent ones)
 * awaits the same cached promise.
 * @returns {Promise<void>}
 */
async function ensureWasm() {
  if (!wasmReady) {
    wasmReady = init();
  }
  await wasmReady;
}

/**
 * @param {unknown} source
 * @returns {source is VideoFrame}
 */
function isVideoFrame(source) {
  return typeof VideoFrame !== 'undefined' && source instanceof VideoFrame;
}

/**
 * @typedef {Object} ScalerOptions
 * @property {number} dstWidth Destination width in pixels. Fixed for this
 *   Scaler's lifetime.
 * @property {number} dstHeight Destination height in pixels. Fixed for this
 *   Scaler's lifetime.
 * @property {'bilinear'|'catmull-rom'|'mitchell'|'lanczos3'} [filter]
 *   Separable filter kernel. Defaults to `'lanczos3'` (sharpest, and the
 *   only one of the four with negative side lobes strong enough to matter
 *   for downscale antialiasing quality).
 * @property {boolean} [premultiply] Premultiply RGB by alpha before
 *   resampling and divide it back out afterwards. Defaults to `true`; only
 *   disable this if the source is known fully opaque, since it is a pure
 *   quality improvement (prevents transparent pixels' color from bleeding
 *   into opaque neighbors on downscale) with the same cost either way. The
 *   *output* is always straight (non-premultiplied) alpha regardless of
 *   this setting, matching `VideoFrame`'s `'RGBA'` pixel format convention
 *   — this only controls the internal resampling math.
 */

/**
 * Professional separable image/video resampler for a fixed destination
 * resolution.
 *
 * Owns three kinds of persistent state, all reused across every
 * `resize()`/`resizeToCanvas()` call so a steady-state render loop never
 * allocates:
 *
 * - a small cache of wasm `Scaler` instances keyed by *source* resolution
 *   (rebuilt only when a new source size is seen — see
 *   {@link MAX_CACHED_WASM_SCALERS}),
 * - a `Uint8ClampedArray` output buffer sized `dstWidth * dstHeight * 4`,
 *   built once since the destination resolution is fixed,
 * - a `_frame.js` `FrameState` for source-frame acquisition, and (on the
 *   `resizeToCanvas` path) a cached `ImageData` view over the output buffer
 *   and the last-used canvas 2D context.
 */
export class Scaler {
  /**
   * Prefer {@link Scaler.create} in application code — it awaits wasm
   * module readiness up front. This constructor is synchronous and does
   * not itself touch wasm (the actual wasm `Scaler` is built lazily, once
   * a source frame's dimensions are known); `resize`/`resizeToCanvas` await
   * module readiness internally regardless of which entry point was used,
   * so direct construction is safe, just less predictable about where the
   * first `await` lands.
   *
   * @param {ScalerOptions} options
   */
  constructor(options) {
    if (!options || !Number.isFinite(options.dstWidth) || !Number.isFinite(options.dstHeight)) {
      throw new Error('Scaler: options.dstWidth and options.dstHeight are required numbers');
    }
    if (options.dstWidth <= 0 || options.dstHeight <= 0) {
      throw new Error('Scaler: options.dstWidth and options.dstHeight must be positive');
    }

    /** @readonly */
    this.dstWidth = options.dstWidth | 0;
    /** @readonly */
    this.dstHeight = options.dstHeight | 0;
    /** @readonly */
    this.filter = options.filter ?? DEFAULT_FILTER;
    /** @readonly */
    this.premultiply = options.premultiply ?? DEFAULT_PREMULTIPLY;

    /** @type {import('./_frame.js').FrameState} */
    this._frameState = {};

    /**
     * Wasm `Scaler` instances keyed by `"${srcWidth}x${srcHeight}"`, in
     * least-recently-used order (re-inserting on hit keeps `Map`'s
     * insertion-order iteration acting as an LRU list).
     * @type {Map<string, WasmScaler>}
     */
    this._scalerCache = new Map();
    /** @type {WasmScaler|null} */
    this._wasm = null;

    // The output buffer's size depends only on (dstWidth, dstHeight),
    // which are fixed for this wrapper's lifetime, so it is safe to
    // allocate once here rather than lazily.
    this._outBuffer = new Uint8ClampedArray(this.dstWidth * this.dstHeight * 4);
    /** Cached `ImageData` aliasing `_outBuffer` (built lazily; canvas-only). */
    this._imageData = null;
    /** @type {HTMLCanvasElement|OffscreenCanvas|null} */
    this._canvas = null;
    /** @type {CanvasRenderingContext2D|OffscreenCanvasRenderingContext2D|null} */
    this._ctx = null;
  }

  /**
   * Creates a `Scaler`, awaiting wasm module readiness before returning.
   * @param {ScalerOptions} options
   * @returns {Promise<Scaler>}
   */
  static async create(options) {
    await ensureWasm();
    return new Scaler(options);
  }

  /**
   * Selects (building and caching if necessary) the wasm `Scaler` for a
   * `(srcWidth, srcHeight)` source resolution, evicting the
   * least-recently-used cached instance if the cache is full.
   *
   * @param {number} srcWidth
   * @param {number} srcHeight
   * @returns {void}
   */
  _selectWasmScaler(srcWidth, srcHeight) {
    const key = `${srcWidth}x${srcHeight}`;
    const cached = this._scalerCache.get(key);
    if (cached) {
      // Refresh LRU order: delete + re-insert moves it to the end of the
      // Map's iteration order.
      this._scalerCache.delete(key);
      this._scalerCache.set(key, cached);
      this._wasm = cached;
      return;
    }

    const built = new WasmScaler(
      srcWidth,
      srcHeight,
      this.dstWidth,
      this.dstHeight,
      this.filter,
      this.premultiply,
    );
    this._scalerCache.set(key, built);
    if (this._scalerCache.size > MAX_CACHED_WASM_SCALERS) {
      const oldestKey = this._scalerCache.keys().next().value;
      const oldest = this._scalerCache.get(oldestKey);
      oldest?.free();
      this._scalerCache.delete(oldestKey);
    }
    this._wasm = built;
  }

  /**
   * Acquires `source`'s pixels and resamples them into `this._outBuffer`,
   * (re)building the wasm `Scaler` first if `source`'s resolution differs
   * from the currently selected one.
   *
   * @param {import('./_frame.js').FrameSource} source
   * @returns {Promise<{width: number, height: number}>} `source`'s
   *   acquired dimensions (the resolution the wasm `Scaler` was selected
   *   for).
   */
  async _resampleInto(source) {
    await ensureWasm();
    const rgba = await frameToRgba(source, this._frameState);
    this._selectWasmScaler(rgba.width, rgba.height);
    // wasm-bindgen's generated `passArray8ToWasm0` copies via
    // `TypedArray.prototype.set`, which accepts any typed-array source, so
    // handing it `rgba.data` (a `Uint8ClampedArray`) directly — no
    // intermediate view or copy — is the one-copy-in this data-plane rule
    // calls for. The copy-back into `this._outBuffer` (the "one copy out")
    // is likewise handled by the generated glue.
    this._wasm.resize(rgba.data, this._outBuffer);
    return { width: rgba.width, height: rgba.height };
  }

  /**
   * Resamples `source` to this Scaler's configured `dstWidth x dstHeight`
   * and returns the result as a new `VideoFrame`.
   *
   * The caller owns the returned `VideoFrame`, including calling
   * `.close()` on it when done; `source` is never closed by this method.
   *
   * @param {import('./_frame.js').FrameSource} source
   * @returns {Promise<VideoFrame>}
   */
  async resize(source) {
    await this._resampleInto(source);

    /** @type {VideoFrameBufferInit} */
    const frameInit = {
      format: 'RGBA',
      codedWidth: this.dstWidth,
      codedHeight: this.dstHeight,
      timestamp: isVideoFrame(source) ? source.timestamp : 0,
    };
    if (isVideoFrame(source)) {
      if (typeof source.duration === 'number') {
        frameInit.duration = source.duration;
      }
      if (source.colorSpace) {
        frameInit.colorSpace = source.colorSpace;
      }
    }
    return new VideoFrame(this._outBuffer, frameInit);
  }

  /**
   * Resamples `source` to this Scaler's configured `dstWidth x dstHeight`
   * and paints it into caller-provided `canvas` via `putImageData`. Never
   * creates DOM nodes or a new canvas — the caller owns `canvas`'s
   * lifecycle; this method only (re)sizes it to `dstWidth x dstHeight` if
   * it does not already match.
   *
   * @param {import('./_frame.js').FrameSource} source
   * @param {HTMLCanvasElement|OffscreenCanvas} canvas
   * @returns {Promise<void>}
   */
  async resizeToCanvas(source, canvas) {
    await this._resampleInto(source);

    if (canvas.width !== this.dstWidth || canvas.height !== this.dstHeight) {
      canvas.width = this.dstWidth;
      canvas.height = this.dstHeight;
      this._ctx = null;
    }
    if (this._ctx === null || this._canvas !== canvas) {
      const ctx = canvas.getContext('2d');
      if (!ctx) {
        throw new Error('Scaler.resizeToCanvas: failed to acquire a 2D context');
      }
      this._ctx = ctx;
      this._canvas = canvas;
    }
    // `ImageData`'s constructor aliases (does not copy) the buffer it is
    // given, and `this._outBuffer` never changes identity or length for
    // this Scaler's lifetime, so the ImageData wrapper can be built once
    // and reused — `putImageData` reads it synchronously on every call, by
    // which point `_resampleInto` has already refreshed the buffer's
    // contents in place.
    if (this._imageData === null) {
      this._imageData = new ImageData(this._outBuffer, this.dstWidth, this.dstHeight);
    }
    this._ctx.putImageData(this._imageData, 0, 0);
  }

  /**
   * Releases every cached wasm `Scaler` instance. Call when done with this
   * wrapper; using it afterwards throws (the wasm side is gone).
   * @returns {void}
   */
  dispose() {
    for (const scaler of this._scalerCache.values()) {
      scaler.free();
    }
    this._scalerCache.clear();
    this._wasm = null;
  }
}

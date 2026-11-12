// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Shared frame acquisition helper for the OxiMedia web modules.
 *
 * WebCodecs (and the `<video>` / canvas APIs) hand you decoded pixels in a
 * handful of shapes; the wasm kernels want exactly one shape: a tightly packed
 * RGBA8 buffer (`width * height * 4` bytes, no row padding). This module turns
 * any browser image source into that, reusing a caller-held `state` object's
 * buffers across frames so a steady-state render loop does not allocate.
 *
 * There is no default export and no dependency: drop it on any static
 * `http.server`, no COOP/COEP headers, no bundler required.
 *
 * @module _frame
 */

/**
 * Per-source reusable buffer bag. Create one plain object per video source and
 * pass the same object to {@link frameToRgba} every frame; the helper attaches
 * and grows its scratch buffers on it. Treat the fields as opaque.
 *
 * @typedef {Object} FrameState
 * @property {Uint8ClampedArray} [buffer] Last returned RGBA8 buffer.
 * @property {Uint8Array} [raw] `copyTo` destination (may include row padding).
 * @property {Uint8ClampedArray} [packed] Tightly repacked buffer (padded path).
 * @property {OffscreenCanvas} [canvas] Persistent canvas for the fallback path.
 * @property {OffscreenCanvasRenderingContext2D} [ctx] Cached 2D context.
 */

/**
 * A tightly packed RGBA8 frame ready to hand to a wasm kernel.
 *
 * @typedef {Object} RgbaFrame
 * @property {Uint8ClampedArray} data `width * height * 4` bytes, row-major RGBA.
 * @property {number} width Pixel width.
 * @property {number} height Pixel height.
 */

/**
 * Module-scope cache of whether `VideoFrame.copyTo(..., { format: 'RGBA' })`
 * actually works here. `null` until the first probe; `true`/`false` afterwards.
 * @type {boolean|null}
 */
let copyToRgbaSupported = null;

/**
 * Module-scope cache of the wasm SIMD (`simd128`) validation result.
 * @type {boolean|null}
 */
let simdSupported = null;

/**
 * Grows (or allocates) a `Uint8Array` field on `state` to at least `len`,
 * reusing the existing allocation when it is already large enough.
 *
 * @param {FrameState} state
 * @param {"raw"} key
 * @param {number} len
 * @returns {Uint8Array}
 */
function ensureU8(state, key, len) {
  let buf = state[key];
  if (!buf || buf.length < len) {
    buf = new Uint8Array(len);
    state[key] = buf;
  }
  return buf;
}

/**
 * Grows (or allocates) a `Uint8ClampedArray` field on `state`.
 *
 * @param {FrameState} state
 * @param {"packed"} key
 * @param {number} len
 * @returns {Uint8ClampedArray}
 */
function ensureU8Clamped(state, key, len) {
  let buf = state[key];
  if (!buf || buf.length < len) {
    buf = new Uint8ClampedArray(len);
    state[key] = buf;
  }
  return buf;
}

/**
 * Returns `true` if `source` is a WebCodecs `VideoFrame`.
 * @param {unknown} source
 * @returns {boolean}
 */
function isVideoFrame(source) {
  return typeof VideoFrame !== "undefined" && source instanceof VideoFrame;
}

/**
 * Resolves the visible pixel dimensions of any supported source.
 *
 * @param {*} source
 * @returns {{ width: number, height: number }}
 * @throws {Error} If the source type is unsupported or has zero size.
 */
function sourceDimensions(source) {
  let width;
  let height;

  if (isVideoFrame(source)) {
    const rect = source.visibleRect;
    width = rect ? rect.width : source.displayWidth;
    height = rect ? rect.height : source.displayHeight;
  } else if (
    typeof HTMLVideoElement !== "undefined" &&
    source instanceof HTMLVideoElement
  ) {
    width = source.videoWidth;
    height = source.videoHeight;
  } else if (
    typeof source.width === "number" &&
    typeof source.height === "number"
  ) {
    // HTMLCanvasElement | OffscreenCanvas | ImageBitmap
    width = source.width;
    height = source.height;
  } else {
    throw new Error(
      "frameToRgba: unsupported source; expected VideoFrame, HTMLVideoElement, " +
        "HTMLCanvasElement, OffscreenCanvas, or ImageBitmap",
    );
  }

  if (!width || !height) {
    throw new Error(
      "frameToRgba: source has zero dimensions — is the video/frame decoded yet?",
    );
  }
  return { width, height };
}

/**
 * True when the copyTo(RGBA) fast path is worth attempting for this frame.
 * @param {VideoFrame} frame
 * @returns {boolean}
 */
function canUseCopyToRgba(frame) {
  return (
    copyToRgbaSupported !== false &&
    typeof frame.copyTo === "function" &&
    typeof frame.allocationSize === "function"
  );
}

/**
 * Fast path: `VideoFrame.copyTo` with RGBA conversion straight into a persistent
 * buffer (a single copy). Repacks only if the returned plane layout is padded.
 *
 * @param {VideoFrame} frame
 * @param {FrameState} state
 * @returns {Promise<RgbaFrame>}
 */
async function copyViaCopyTo(frame, state) {
  const rect = frame.visibleRect;
  const width = rect ? rect.width : frame.codedWidth;
  const height = rect ? rect.height : frame.codedHeight;
  const tightBytes = width * height * 4;

  const options = { format: "RGBA" };
  const size = frame.allocationSize(options);
  const raw = ensureU8(state, "raw", size);

  const layout = await frame.copyTo(raw, options);
  const plane = layout && layout[0];
  const stride = plane ? plane.stride : width * 4;
  const offset = plane ? plane.offset : 0;

  let data;
  if (offset === 0 && stride === width * 4) {
    // Already tightly packed: hand back a zero-copy view of the exact bytes.
    data = new Uint8ClampedArray(raw.buffer, raw.byteOffset, tightBytes);
  } else {
    // Row padding present: repack into a grow-once packed buffer.
    const packed = ensureU8Clamped(state, "packed", tightBytes);
    const rowBytes = width * 4;
    for (let y = 0; y < height; y += 1) {
      const start = offset + y * stride;
      packed.set(raw.subarray(start, start + rowBytes), y * rowBytes);
    }
    data = tightBytes === packed.length ? packed : packed.subarray(0, tightBytes);
  }
  return { data, width, height };
}

/**
 * Ensures a persistent `OffscreenCanvas` + 2D context of the given size,
 * resizing (and dropping the cached context) only when the size changes.
 *
 * @param {FrameState} state
 * @param {number} width
 * @param {number} height
 * @returns {OffscreenCanvasRenderingContext2D}
 * @throws {Error} If `OffscreenCanvas`/2D context is unavailable.
 */
function ensureCanvasContext(state, width, height) {
  if (typeof OffscreenCanvas === "undefined") {
    throw new Error(
      "frameToRgba: OffscreenCanvas is unavailable and VideoFrame.copyTo(RGBA) " +
        "is not supported here — cannot acquire frame pixels",
    );
  }
  let canvas = state.canvas;
  if (!canvas) {
    canvas = new OffscreenCanvas(width, height);
    state.canvas = canvas;
    state.ctx = null;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
    state.ctx = null;
  }
  let ctx = state.ctx;
  if (!ctx) {
    ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) {
      throw new Error("frameToRgba: failed to acquire a 2D context");
    }
    state.ctx = ctx;
  }
  return ctx;
}

/**
 * Fallback path: draw the source onto a persistent canvas and read it back with
 * `getImageData`. Works for every `CanvasImageSource`.
 *
 * @param {*} source
 * @param {FrameState} state
 * @returns {RgbaFrame}
 */
function copyViaCanvas(source, state) {
  const { width, height } = sourceDimensions(source);
  const ctx = ensureCanvasContext(state, width, height);
  ctx.drawImage(source, 0, 0, width, height);
  const image = ctx.getImageData(0, 0, width, height);
  return { data: image.data, width, height };
}

/**
 * Converts any supported browser image source into a tightly packed RGBA8
 * frame, reusing `state`'s buffers across calls.
 *
 * Strategy: for a `VideoFrame`, try `copyTo(buffer, { format: 'RGBA' })` (one
 * copy into a persistent buffer, feature-detected and cached module-wide);
 * if that path is unavailable or fails, fall back to drawing onto a persistent
 * `OffscreenCanvas` and reading it back with `getImageData`.
 *
 * The caller retains ownership of `source` (this helper never calls
 * `frame.close()`), and owns the returned `data` only until the next call with
 * the same `state`.
 *
 * @param {VideoFrame|HTMLVideoElement|HTMLCanvasElement|OffscreenCanvas|ImageBitmap} source
 * @param {FrameState} state Caller-held object reused across frames.
 * @returns {Promise<RgbaFrame>}
 * @throws {Error} With an actionable message on unsupported/zero-size sources.
 */
export async function frameToRgba(source, state) {
  if (!state || typeof state !== "object") {
    throw new Error(
      "frameToRgba: `state` must be a caller-held object reused across frames " +
        "(e.g. `const state = {}` outside your render loop)",
    );
  }
  if (source === null || source === undefined) {
    throw new Error("frameToRgba: `source` is null or undefined");
  }

  if (isVideoFrame(source) && canUseCopyToRgba(source)) {
    try {
      const out = await copyViaCopyTo(source, state);
      copyToRgbaSupported = true;
      state.buffer = out.data;
      return out;
    } catch (err) {
      if (copyToRgbaSupported === true) {
        // Previously worked, so this is a real error, not a capability gap.
        throw new Error(`frameToRgba: VideoFrame.copyTo failed — ${err}`);
      }
      // First probe failed: this engine lacks RGBA copyTo. Remember and fall
      // through to the canvas path for this and all future frames.
      copyToRgbaSupported = false;
    }
  }

  const out = copyViaCanvas(source, state);
  state.buffer = out.data;
  return out;
}

/**
 * Minimal wasm module that declares a function returning a `v128`, used to
 * feature-detect the `simd128` proposal via `WebAssembly.validate`.
 *
 * Byte-for-byte structure (29 bytes total):
 *   0x00 0x61 0x73 0x6d              magic "\0asm"
 *   0x01 0x00 0x00 0x00              version 1
 *   0x01 0x05 0x01 0x60 0x00 0x01 0x7b
 *     section id 1 (type), size 5, 1 type: func() -> v128
 *   0x03 0x02 0x01 0x00
 *     section id 3 (function), size 2, 1 function using type 0
 *   0x0a 0x08 0x01 0x06 0x00 0x41 0x00 0xfd 0x0f 0x0b
 *     section id 10 (code), size 8 (1 body-count byte + 1 body-size byte +
 *     6 body-content bytes), 1 function body of size 6:
 *       0x00            0 locals
 *       0x41 0x00       i32.const 0
 *       0xfd 0x0f       simd prefix (0xfd) + i8x16.splat (0x0f)
 *       0x0b            end
 * The code-section size (byte 20) and function-body size (byte 22) must
 * exactly match their content lengths (8 and 6 respectively) or
 * `WebAssembly.validate` rejects the module on every engine — verify by
 * hand against this comment if these bytes are ever touched again.
 * @type {number[]}
 */
const SIMD_PROBE = [
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00,
  0x01, 0x7b, 0x03, 0x02, 0x01, 0x00, 0x0a, 0x08, 0x01, 0x06, 0x00, 0x41, 0x00,
  0xfd, 0x0f, 0x0b,
];

/**
 * Detects wasm `simd128` support, caching the result module-wide.
 * @returns {boolean}
 */
function detectSimd() {
  if (simdSupported === null) {
    try {
      simdSupported =
        typeof WebAssembly !== "undefined" &&
        typeof WebAssembly.validate === "function" &&
        WebAssembly.validate(new Uint8Array(SIMD_PROBE));
    } catch (_err) {
      simdSupported = false;
    }
  }
  return simdSupported;
}

/**
 * Runtime capability report for the demo's diagnostics panel.
 *
 * @typedef {Object} Capabilities
 * @property {boolean} videoFrame WebCodecs `VideoFrame` is defined.
 * @property {boolean} copyToRgba `VideoFrame.copyTo(RGBA)` is available (best
 *   effort until the first {@link frameToRgba} call confirms it).
 * @property {boolean} rvfc `HTMLVideoElement.requestVideoFrameCallback` exists.
 * @property {boolean} trackProcessor `MediaStreamTrackProcessor` is defined.
 * @property {boolean} simd wasm `simd128` validates in this engine.
 */

/**
 * Probes the browser for the features these modules can take advantage of.
 *
 * @returns {Capabilities}
 */
export function detectCapabilities() {
  const videoFrame = typeof VideoFrame !== "undefined";
  const copyTo =
    videoFrame && typeof VideoFrame.prototype.copyTo === "function";
  const rvfc =
    typeof HTMLVideoElement !== "undefined" &&
    "requestVideoFrameCallback" in HTMLVideoElement.prototype;
  const trackProcessor = typeof MediaStreamTrackProcessor !== "undefined";
  return {
    videoFrame,
    // Prefer the confirmed probe result once frameToRgba has run.
    copyToRgba: copyToRgbaSupported === null ? copyTo : copyToRgbaSupported,
    rvfc,
    trackProcessor,
    simd: detectSimd(),
  };
}

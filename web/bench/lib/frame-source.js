// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Wraps a raw RGBA8 buffer into a `FrameSource` the `scopes`/`color`/`scale`
 * wrappers accept (they all funnel through `_frame.js`'s `frameToRgba`,
 * which only understands `VideoFrame`, `HTMLVideoElement`,
 * `HTMLCanvasElement`, `OffscreenCanvas` and `ImageBitmap` — never a raw
 * typed array).
 *
 * Preference order: `VideoFrame` when the engine supports it (this is the
 * library's primary target — "WebCodecs gives you the frames" — and
 * exercises `frameToRgba`'s `copyTo(RGBA)` fast path), falling back to a
 * persistent `OffscreenCanvas` seeded once via `putImageData` (exercises
 * `frameToRgba`'s `drawImage` + `getImageData` fallback path instead). The
 * built source is reused across every warmup/measured call in a suite — it
 * is built once, outside the timed loop, exactly like a real integrator
 * would hold one frame and call a scope/pipeline method on it repeatedly
 * would not do, but which the source-acquisition cost model does not care
 * about: `copyTo`/`drawImage` do not consume or invalidate their source.
 *
 * @module lib/frame-source
 */

/**
 * A benchmark-owned frame source plus its disposer.
 *
 * @typedef {Object} BenchFrameSource
 * @property {VideoFrame|OffscreenCanvas} source Ready to hand to a
 *   `scopes`/`color`/`scale` wrapper method.
 * @property {'VideoFrame'|'OffscreenCanvas'} kind Which acquisition path
 *   `frameToRgba` will take for this source — recorded in the environment
 *   report so published numbers say which one was measured.
 * @property {() => void} dispose Releases the source. Call once, after the
 *   last suite that uses it has finished (`VideoFrame.close()`; a no-op for
 *   the canvas path).
 */

/**
 * Builds a {@link BenchFrameSource} from a tightly packed RGBA8 buffer.
 *
 * @param {Uint8ClampedArray} buffer `width * height * 4` bytes, row-major
 *   RGBA, no row padding.
 * @param {number} width Pixel width.
 * @param {number} height Pixel height.
 * @returns {BenchFrameSource}
 * @throws {Error} If neither `VideoFrame` nor `OffscreenCanvas` is
 *   available in this engine (headless-Chrome / evergreen desktop browsers
 *   both have at least one).
 */
export function buildFrameSource(buffer, width, height) {
  if (typeof VideoFrame !== "undefined") {
    const videoFrame = new VideoFrame(buffer, {
      format: "RGBA",
      codedWidth: width,
      codedHeight: height,
      timestamp: 0,
    });
    return {
      source: videoFrame,
      kind: "VideoFrame",
      dispose: () => videoFrame.close(),
    };
  }

  if (typeof OffscreenCanvas !== "undefined") {
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      throw new Error("buildFrameSource: failed to acquire a 2D context on the fallback canvas");
    }
    ctx.putImageData(new ImageData(new Uint8ClampedArray(buffer), width, height), 0, 0);
    return {
      source: canvas,
      kind: "OffscreenCanvas",
      dispose: () => {},
    };
  }

  throw new Error(
    "buildFrameSource: neither VideoFrame nor OffscreenCanvas is available in this browser " +
      "— the benchmark harness needs at least one to build a frame source",
  );
}

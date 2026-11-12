// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Shared frame acquisition helper for the OxiMedia web modules.
 *
 * Turns any supported browser image source into a tightly packed RGBA8 buffer
 * that the wasm kernels can consume, reusing a caller-held {@link FrameState}
 * across frames so a steady render loop does not allocate.
 */

/**
 * Per-source reusable buffer bag. Create one plain object per video source and
 * pass the same object to {@link frameToRgba} every frame. Treat the fields as
 * opaque.
 */
export interface FrameState {
  /** Last returned RGBA8 buffer. */
  buffer?: Uint8ClampedArray;
  /** `copyTo` destination (may include row padding). */
  raw?: Uint8Array;
  /** Tightly repacked buffer (used only on the padded copyTo path). */
  packed?: Uint8ClampedArray;
  /** Persistent canvas for the fallback path. */
  canvas?: OffscreenCanvas;
  /** Cached 2D context for the fallback path. */
  ctx?: OffscreenCanvasRenderingContext2D | null;
}

/** A tightly packed RGBA8 frame ready to hand to a wasm kernel. */
export interface RgbaFrame {
  /** `width * height * 4` bytes, row-major RGBA. */
  data: Uint8ClampedArray;
  /** Pixel width. */
  width: number;
  /** Pixel height. */
  height: number;
}

/** Any image source {@link frameToRgba} accepts. */
export type FrameSource =
  | VideoFrame
  | HTMLVideoElement
  | HTMLCanvasElement
  | OffscreenCanvas
  | ImageBitmap;

/** Runtime capability report for the demo's diagnostics panel. */
export interface Capabilities {
  /** WebCodecs `VideoFrame` is defined. */
  videoFrame: boolean;
  /** `VideoFrame.copyTo(RGBA)` is available (confirmed after first use). */
  copyToRgba: boolean;
  /** `HTMLVideoElement.requestVideoFrameCallback` exists. */
  rvfc: boolean;
  /** `MediaStreamTrackProcessor` is defined. */
  trackProcessor: boolean;
  /** wasm `simd128` validates in this engine. */
  simd: boolean;
}

/**
 * Converts any supported browser image source into a tightly packed RGBA8
 * frame, reusing `state`'s buffers across calls.
 *
 * The caller retains ownership of `source` (this helper never calls
 * `frame.close()`), and owns the returned `data` only until the next call with
 * the same `state`.
 *
 * @throws {Error} With an actionable message on unsupported/zero-size sources.
 */
export function frameToRgba(
  source: FrameSource,
  state: FrameState,
): Promise<RgbaFrame>;

/** Probes the browser for the features these modules can take advantage of. */
export function detectCapabilities(): Capabilities;

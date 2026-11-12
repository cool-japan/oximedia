// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * PSNR + SSIM video quality metrics, backed by `oximedia-web-quality`.
 */

import type { FrameSource, RgbaFrame } from "./_frame.js";

/** PSNR + SSIM comparison result for one {@link Quality.compare} call. */
export interface QualityMetrics {
  /** PSNR (dB) over the RGB channels (alpha ignored); `Infinity` for bit-identical frames. */
  psnrRgb: number;
  /** PSNR (dB) over BT.709 full-range luma; `Infinity` for bit-identical frames. */
  psnrLuma: number;
  /** Mean windowed SSIM (`1.0` for bit-identical frames, generally `[0, 1]` for real distortions). */
  ssim: number;
}

/**
 * Anything {@link Quality.compare} / {@link Quality.ssimMapToCanvas} accept:
 * any {@link FrameSource}, a raw tightly packed RGBA8 buffer, or an
 * `RgbaFrame`-shaped object (e.g. one already returned by `frameToRgba`).
 */
export type QualitySource = FrameSource | Uint8Array | Uint8ClampedArray | RgbaFrame;

/**
 * PSNR + SSIM analyzer bound to a fixed `width x height` RGBA8 frame size.
 *
 * Every working buffer is preallocated wasm-side at {@link Quality.create};
 * `compare` / `ssimMapToCanvas` never allocate beyond one small result
 * object per call.
 */
export declare class Quality {
  private constructor();

  /**
   * Creates an analyzer for `width x height` RGBA8 frames, initializing the
   * wasm module on first call (subsequent calls, for this or any other
   * `Quality` instance, reuse the same initialization).
   *
   * @throws {Error} If `width`/`height` are not positive integers, or are
   * too small for the fixed 11-pixel SSIM window.
   */
  static create(options: { width: number; height: number }): Promise<Quality>;

  /** Frame width this analyzer was created for. */
  readonly width: number;
  /** Frame height this analyzer was created for. */
  readonly height: number;

  /**
   * Computes PSNR (RGB + luma) and mean SSIM between two frames.
   *
   * @throws {Error} If either source doesn't resolve to `width x height`.
   */
  compare(sourceA: QualitySource, sourceB: QualitySource): Promise<QualityMetrics>;

  /**
   * Computes the SSIM heatmap between two frames (red = dissimilar, green =
   * similar) and paints it into `canvas` via `putImageData`, resizing the
   * canvas to `width x height` first if needed. Returns the mean SSIM (the
   * same value {@link Quality.compare} would report).
   *
   * The caller owns `canvas`: this method never creates DOM nodes.
   *
   * @throws {Error} If `canvas` has no usable 2D context, or as {@link Quality.compare}.
   */
  ssimMapToCanvas(
    sourceA: QualitySource,
    sourceB: QualitySource,
    canvas: HTMLCanvasElement | OffscreenCanvas,
  ): Promise<number>;

  /**
   * Releases the underlying wasm object. The instance must not be used
   * afterwards. Safe to omit if you'd rather let the `FinalizationRegistry`
   * reclaim it (slower, non-deterministic).
   */
  free(): void;
}

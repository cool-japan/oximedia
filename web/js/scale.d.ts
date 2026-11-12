// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * High-level resampling wrapper around the `oximedia-web-scale` wasm
 * module: professional separable resizing (Lanczos3, Catmull-Rom, Mitchell,
 * bilinear) of any browser video/image source to a fixed destination
 * resolution.
 */

import type { FrameSource } from './_frame.js';

/** Separable filter kernel selector accepted by {@link ScalerOptions.filter}. */
export type ScaleFilter = 'bilinear' | 'catmull-rom' | 'mitchell' | 'lanczos3';

/** Options for {@link Scaler.create} / the {@link Scaler} constructor. */
export interface ScalerOptions {
  /** Destination width in pixels. Fixed for this Scaler's lifetime. */
  dstWidth: number;
  /** Destination height in pixels. Fixed for this Scaler's lifetime. */
  dstHeight: number;
  /** Separable filter kernel. Defaults to `'lanczos3'`. */
  filter?: ScaleFilter;
  /**
   * Premultiply RGB by alpha before resampling and divide it back out
   * afterwards (prevents transparent pixels' color from bleeding into
   * opaque neighbors on downscale). Defaults to `true`. The *output* is
   * always straight (non-premultiplied) alpha regardless of this setting.
   */
  premultiply?: boolean;
}

/**
 * Professional separable image/video resampler for a fixed destination
 * resolution. Rebuilds its internal wasm scaler automatically (cached,
 * keyed by source resolution) whenever the source frame's resolution
 * changes; every other buffer is persistent and reused across calls.
 */
export class Scaler {
  /** Destination width this Scaler was configured for. */
  readonly dstWidth: number;
  /** Destination height this Scaler was configured for. */
  readonly dstHeight: number;
  /** Configured filter kernel. */
  readonly filter: ScaleFilter;
  /** Configured premultiply-alpha setting. */
  readonly premultiply: boolean;

  /**
   * Prefer {@link Scaler.create} in application code — it awaits wasm
   * module readiness up front. Direct construction is safe (`resize`/
   * `resizeToCanvas` await readiness internally regardless), just less
   * predictable about where the first `await` lands.
   */
  constructor(options: ScalerOptions);

  /** Creates a `Scaler`, awaiting wasm module readiness before returning. */
  static create(options: ScalerOptions): Promise<Scaler>;

  /**
   * Resamples `source` to `dstWidth x dstHeight` and returns the result as
   * a new `VideoFrame`. The caller owns the returned `VideoFrame` (must
   * call `.close()` on it); `source` is never closed by this method.
   */
  resize(source: FrameSource): Promise<VideoFrame>;

  /**
   * Resamples `source` to `dstWidth x dstHeight` and paints it into
   * caller-provided `canvas` via `putImageData`. Never creates DOM nodes;
   * resizes `canvas` to `dstWidth x dstHeight` if it does not already
   * match.
   */
  resizeToCanvas(source: FrameSource, canvas: HTMLCanvasElement | OffscreenCanvas): Promise<void>;

  /**
   * Releases every cached wasm `Scaler` instance. Using this wrapper
   * afterwards throws.
   */
  dispose(): void;
}

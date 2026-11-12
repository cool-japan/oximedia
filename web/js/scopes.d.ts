// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Type declarations for `@cooljapan/oximedia-web/scopes`.
 * @module scopes
 */

/** Any browser image source the scopes accept. */
export type FrameSource =
  | VideoFrame
  | HTMLVideoElement
  | HTMLCanvasElement
  | OffscreenCanvas
  | ImageBitmap;

/** A 2D-capable destination canvas owned by the caller. */
export type ScopeCanvas = HTMLCanvasElement | OffscreenCanvas;

/** Waveform display mode. */
export type WaveformMode = 'luma' | 'rgb-parade' | 'rgb-overlay' | 'ycbcr';

/** Histogram display mode. */
export type HistogramMode = 'luma' | 'rgb';

/** False-colour palette. */
export type FalseColorPreset = 'spectrum' | 'arri';

/** Options accepted by {@link Scopes.create}. */
export interface ScopesOptions {
  /** Output canvas width in pixels. Default `512`. */
  width?: number;
  /** Output canvas height in pixels. Default `256`. */
  height?: number;
  /** Default graticule preference for the vectorscope / histogram. Default `true`. */
  graticule?: boolean;
}

/** Options for {@link Scopes.waveform}. */
export interface WaveformOptions {
  /** Waveform mode. Default `'rgb-parade'`. */
  mode?: WaveformMode;
  /** Draw the IRE graticule + labels. Default `true`. */
  ire?: boolean;
}

/** Options for {@link Scopes.vectorscope}. */
export interface VectorscopeOptions {
  /** Trace magnification (zoom). Default `1.0`. */
  gain?: number;
  /** Draw the 123-degree skin-tone / +I line. Default `true`. */
  skinToneLine?: boolean;
  /** Overlay the SMPTE graticule. Defaults to the instance preference. */
  graticule?: boolean;
}

/** Options for {@link Scopes.histogram}. */
export interface HistogramOptions {
  /** Histogram mode. Default `'luma'`. */
  mode?: HistogramMode;
  /** Overlay the graticule. Defaults to the instance preference. */
  graticule?: boolean;
}

/** Options for {@link Scopes.falseColor}. */
export interface FalseColorOptions {
  /** False-colour preset. Default `'arri'`. */
  preset?: FalseColorPreset;
}

/** Luma statistics for one frame. */
export interface ScopeStats {
  /** Minimum luma (0..=255). */
  minLuma: number;
  /** Maximum luma (0..=255). */
  maxLuma: number;
  /** Mean luma. */
  avgLuma: number;
  /** Luma standard deviation. */
  stdDev: number;
  /** Percentage of pixels below legal black (< 16). */
  blackClipPercent: number;
  /** Percentage of pixels above legal white (> 235). */
  whiteClipPercent: number;
}

/**
 * A video-scope renderer bound to a fixed output size. Create it with the async
 * {@link Scopes.create} factory, then call a render method per frame.
 */
export class Scopes {
  /** Output canvas width in pixels. */
  readonly width: number;
  /** Output canvas height in pixels. */
  readonly height: number;

  private constructor();

  /** Creates a scope renderer, initialising the wasm module on first use. */
  static create(options?: ScopesOptions): Promise<Scopes>;

  /** Renders a waveform monitor into `canvas`. */
  waveform(source: FrameSource, canvas: ScopeCanvas, options?: WaveformOptions): Promise<void>;

  /** Renders a vectorscope into `canvas`. */
  vectorscope(
    source: FrameSource,
    canvas: ScopeCanvas,
    options?: VectorscopeOptions,
  ): Promise<void>;

  /** Renders a histogram into `canvas`. */
  histogram(source: FrameSource, canvas: ScopeCanvas, options?: HistogramOptions): Promise<void>;

  /** Renders a false-colour exposure map into `canvas`. */
  falseColor(source: FrameSource, canvas: ScopeCanvas, options?: FalseColorOptions): Promise<void>;

  /** Computes luma statistics for a frame (no canvas render). */
  stats(source: FrameSource): Promise<ScopeStats>;

  /** Releases the underlying wasm renderer. */
  free(): void;
}

export default Scopes;

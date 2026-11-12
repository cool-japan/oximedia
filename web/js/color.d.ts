// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * `@cooljapan/oximedia-web/color` — colour-science pipeline for WebCodecs
 * frames, running in WebAssembly.
 *
 * Fixed operator order: input-transfer decode → exposure → contrast →
 * saturation → tone map → gamut → output-transfer encode → 3D LUT.
 * Alpha passes through untouched.
 */

import type { FrameSource } from "./_frame.js";

/**
 * Tone-map operator names.
 *
 * Honesty note — two different "ACES":
 * - `'aces'`: the Narkowicz-2015 **fitted** ACES filmic curve, applied on
 *   BT.2100 luminance with hue-preserving ratio scaling. The common
 *   real-time approximation.
 * - `'aces-odt'`: an ACES Output-Transform-2.0-**shaped** rendering (the
 *   OxiMedia `AcesOt2` port): per-channel RRT S-curve with a peak-nits
 *   adaptive shoulder plus parametric gamut compression.
 *
 * Neither is the bit-exact Academy CTL reference transform.
 */
export type ToneMapOperator =
  | "reinhard"
  | "reinhard-extended"
  | "hable"
  | "filmic"
  | "aces"
  | "aces-odt";

/** Primaries names (aliases like `'rec709'`, `'srgb'`, `'p3'` also parse). */
export type PrimariesName = "bt709" | "bt2020" | "display-p3";

/**
 * Transfer-function names.
 *
 * Normalisation of linear 1.0: sRGB → SDR reference white; PQ → 10 000 nits
 * (SMPTE ST 2084 absolute); HLG → nominal peak (~1 000 nits), with an
 * OOTF-lite (`x^1.2`) applied on decode; `'linear'` passes floats through
 * unclamped.
 */
export type TransferName = "srgb" | "pq" | "hlg" | "linear";

/** 3D-LUT interpolation kernels. */
export type LutInterp = "trilinear" | "tetrahedral";

/** Options for {@link ColorPipeline.toneMap}. */
export interface ToneMapOptions {
  /** Target display peak luminance in nits. Default 100 (SDR). */
  peakNits?: number;
  /**
   * Luminance meant by linear 1.0 on input, in nits. Default 1000.
   * Use 10000 for PQ-decoded content.
   */
  inputPeakNits?: number;
}

/** Options for {@link ColorPipeline.gamut}. */
export interface GamutOptions {
  /**
   * Soft-clip softness in (0, 1]. When set, highlights are additionally
   * compressed into [0, 1] with a C1-continuous tanh knee. When omitted
   * (default 0), only negative out-of-gamut channels are fixed
   * (hue-preserving) and HDR values above 1.0 survive.
   */
  softness?: number;
}

/** Options for {@link ColorPipeline.lut}. */
export interface LutOptions {
  /** Interpolation kernel. Default `'tetrahedral'`. */
  interp?: LutInterp;
}

/** Options for {@link ColorPipeline.transfer}. */
export interface TransferOptions {
  /** Input transfer (decode). Unchanged when omitted. */
  in?: TransferName;
  /** Output transfer (encode). Unchanged when omitted. */
  out?: TransferName;
}

/** Options for {@link ColorPipeline.exportCube}. */
export interface ExportCubeOptions {
  /** Lattice size per axis, 2–129. Default 33. */
  size?: number;
}

/**
 * A parsed `.cube` 3D LUT. Construct via {@link loadCubeLut}.
 */
export class CubeLut {
  /** Lattice size per axis (2–129). */
  readonly size: number;
  /** LUT title from the `TITLE` line, or `null`. */
  readonly title: string | null;
  /**
   * Serialises back to `.cube` text (R-fastest data order — the
   * Adobe/ffmpeg convention).
   */
  export(): string;
  /** Releases the wasm-side memory. The instance is unusable afterwards. */
  free(): void;
}

/**
 * Parses a `.cube` LUT from a string, `Uint8Array` or `ArrayBuffer`
 * (UTF-8). Hostile input rejects with a descriptive `Error` (the parser
 * enforces `LUT_3D_SIZE` in 2–129, exact data-line counts, finite floats,
 * `DOMAIN_MIN`/`DOMAIN_MAX` sanity, and refuses 1D LUTs) — it never crashes
 * the wasm instance.
 */
export function loadCubeLut(
  bytesOrString: string | Uint8Array | ArrayBuffer,
): Promise<CubeLut>;

/**
 * The colour pipeline. Create with {@link ColorPipeline.create}; all
 * configuration methods are chainable. Per-frame buffers are persistent on
 * the instance (grow-once), so a steady render loop does not allocate.
 */
export class ColorPipeline {
  /**
   * Initialises the wasm module (once, shared per page) and returns a fresh
   * identity pipeline (sRGB in, sRGB out, neutral ops).
   */
  static create(): Promise<ColorPipeline>;

  /**
   * Sets exposure in photographic stops (gain = 2^stops, applied in linear
   * light). 0 is neutral. Throws for non-finite input or |stops| > 32.
   */
  exposure(stops: number): this;

  /**
   * Sets contrast: a power law around the 0.18 linear-light pivot
   * (pivot-preserving, monotonic). 1.0 is neutral; valid range (0, 10].
   */
  contrast(value: number): this;

  /**
   * Sets saturation via BT.709 luma blend in linear light. 1.0 is neutral,
   * 0 is monochrome; valid range [0, 10].
   */
  saturation(value: number): this;

  /**
   * Enables tone mapping (`null` disables). See {@link ToneMapOperator}
   * for the `'aces'` vs `'aces-odt'` distinction.
   */
  toneMap(op: ToneMapOperator | null, opts?: ToneMapOptions): this;

  /**
   * Enables gamut conversion between BT.709, BT.2020 and Display-P3
   * (`null` disables). Conversion runs in linear light via a precomputed
   * 3×3 primaries matrix (CIE-XYZ derived, D65).
   */
  gamut(src: PrimariesName | string | null, dst?: PrimariesName | string, opts?: GamutOptions): this;

  /**
   * Enables the 3D-LUT stage (`null` disables). The LUT is applied on
   * encoded output values — the standard creative-LUT convention — so a
   * LUT baked with {@link exportCube} reproduces the pipeline when loaded
   * into a fresh identity pipeline.
   */
  lut(cubeLut: CubeLut | null, opts?: LutOptions): this;

  /** Sets the input/output transfer functions; omitted sides unchanged. */
  transfer(opts: TransferOptions): this;

  /**
   * Bakes the whole pipeline (including its LUT stage) into `.cube` text —
   * the encoded-in → encoded-out map on a `size³` lattice.
   */
  exportCube(opts?: ExportCubeOptions): string;

  /**
   * Processes a frame and returns a new RGBA `VideoFrame` (timestamp is
   * carried over from the source when present). The caller owns both the
   * input (never closed here) and the returned frame. Throws if the
   * browser lacks `VideoFrame` — use {@link applyToCanvas} there.
   */
  apply(source: FrameSource): Promise<VideoFrame>;

  /**
   * Processes a frame and paints it into a caller-provided canvas via
   * `putImageData` (non-WebCodecs path). Never creates DOM nodes; resizes
   * the canvas only when the source size changes.
   */
  applyToCanvas(
    source: FrameSource,
    canvas: HTMLCanvasElement | OffscreenCanvas,
  ): Promise<void>;

  /**
   * HDR path: applies the pipeline to a tightly packed RGBA `Float32Array`
   * (`width × height × 4` elements) using the exact transfer curves.
   * Synchronous; reuse both buffers across frames.
   */
  applyF32(
    src: Float32Array,
    dst: Float32Array,
    width: number,
    height: number,
  ): this;

  /** Releases the wasm-side memory. The instance is unusable afterwards. */
  free(): void;
}

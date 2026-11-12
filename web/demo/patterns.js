// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Procedural test-pattern generators for the OxiScope demo.
 *
 * Every visitor gets "the moment" with zero shipped media files and zero
 * licensing questions: these draw synthetic, animated frames onto a caller
 * canvas so the scopes and the colour pipeline have something live to chew on
 * even when no footage is at hand.
 *
 * Each generator is a pure function of `(ctx, width, height, timeMs)`; the
 * caller owns the canvas, the animation loop and the `captureStream`. No
 * allocation of note per frame beyond the odd gradient object.
 *
 * @module patterns
 */

/**
 * The seven 75%-amplitude SMPTE/EBU top-bar colours, left to right, as sRGB
 * 8-bit triples. 75% grey is 191, 75% primaries scale their 255 to 191.
 * @type {ReadonlyArray<readonly [number, number, number]>}
 */
const BARS_75 = [
  [191, 191, 191], // grey
  [191, 191, 0], // yellow
  [0, 191, 191], // cyan
  [0, 191, 0], // green
  [191, 0, 191], // magenta
  [191, 0, 0], // red
  [0, 0, 191], // blue
];

/**
 * The reverse-order chroma strip beneath the main bars (blue, black, magenta,
 * black, cyan, black, grey) — the classic SMPTE castellation.
 * @type {ReadonlyArray<readonly [number, number, number]>}
 */
const BARS_CASTELLATION = [
  [0, 0, 191], // blue
  [19, 19, 19], // near-black
  [191, 0, 191], // magenta
  [19, 19, 19],
  [0, 191, 191], // cyan
  [19, 19, 19],
  [191, 191, 191], // grey
];

/**
 * Formats an sRGB triple as a `rgb()` string.
 * @param {readonly [number, number, number]} rgb
 * @returns {string}
 */
function rgbCss(rgb) {
  return `rgb(${rgb[0] | 0}, ${rgb[1] | 0}, ${rgb[2] | 0})`;
}

/**
 * Draws a horizontal run of equal-width colour columns across `[0, w)`.
 * @param {CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D} ctx
 * @param {ReadonlyArray<readonly [number, number, number]>} colors
 * @param {number} y
 * @param {number} w
 * @param {number} h
 */
function drawColumns(ctx, colors, y, w, h) {
  const n = colors.length;
  for (let i = 0; i < n; i += 1) {
    const x0 = Math.round((i * w) / n);
    const x1 = Math.round(((i + 1) * w) / n);
    ctx.fillStyle = rgbCss(colors[i]);
    ctx.fillRect(x0, y, x1 - x0, h);
  }
}

/**
 * SMPTE-style colour bars with a live scanning highlight so the scopes update
 * every frame. Top two-thirds: 75% bars. Middle strip: reverse castellation.
 * Bottom: a PLUGE-ish black region with sub/super-black pluge stripes plus a
 * 0→100% luma ramp on the right, and a moving specular sweep across the top so
 * "video is playing".
 *
 * The 100% white pluge patch and the ramp deliberately reach code 255 so that,
 * with tone-mapping off, the waveform piles up flat at 100 IRE — switch a
 * tone-map operator in and that pile-up rolls off, which is the whole point.
 *
 * @param {CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D} ctx
 * @param {number} w
 * @param {number} h
 * @param {number} t Elapsed time in milliseconds.
 */
export function smpteBars(ctx, w, h, t) {
  const topH = Math.round(h * 0.62);
  const midH = Math.round(h * 0.1);
  const botY = topH + midH;
  const botH = h - botY;

  drawColumns(ctx, BARS_75, 0, w, topH);
  drawColumns(ctx, BARS_CASTELLATION, topH, w, midH);

  // Bottom band: -I / white / +Q / black, then a luma ramp on the right third.
  const twoThirds = Math.round(w * 0.66);
  const seg = twoThirds / 4;
  const bottomPatches = [
    [0, 33, 76], // -I
    [255, 255, 255], // 100% white
    [50, 0, 106], // +Q
    [19, 19, 19], // black
  ];
  for (let i = 0; i < bottomPatches.length; i += 1) {
    const x0 = Math.round(i * seg);
    const x1 = Math.round((i + 1) * seg);
    ctx.fillStyle = rgbCss(bottomPatches[i]);
    ctx.fillRect(x0, botY, x1 - x0, botH);
  }

  // PLUGE stripes inside the black patch (sub-black, black, super-black).
  const plugeX = Math.round(3 * seg);
  const plugeW = Math.round(seg / 3);
  const pluge = [11, 19, 29];
  for (let i = 0; i < pluge.length; i += 1) {
    const v = pluge[i];
    ctx.fillStyle = rgbCss([v, v, v]);
    ctx.fillRect(plugeX + i * plugeW, botY, plugeW, botH);
  }

  // Luma ramp on the right third (0 → 255), the tone-map roll-off target.
  const rampX0 = twoThirds;
  const rampW = w - rampX0;
  const ramp = ctx.createLinearGradient(rampX0, 0, rampX0 + rampW, 0);
  ramp.addColorStop(0, 'rgb(0,0,0)');
  ramp.addColorStop(1, 'rgb(255,255,255)');
  ctx.fillStyle = ramp;
  ctx.fillRect(rampX0, botY, rampW, botH);

  // Live specular sweep across the top bars: a soft moving white highlight.
  const sweepX = (0.5 + 0.5 * Math.sin(t / 1300)) * w;
  const sweepW = Math.max(24, w * 0.08);
  const sweep = ctx.createLinearGradient(sweepX - sweepW, 0, sweepX + sweepW, 0);
  sweep.addColorStop(0, 'rgba(255,255,255,0)');
  sweep.addColorStop(0.5, 'rgba(255,255,255,0.45)');
  sweep.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = sweep;
  ctx.fillRect(0, 0, w, topH);
}

/**
 * A drifting luma sweep on top and a zone-plate-style concentric-ring chirp on
 * the bottom half — high-frequency detail that lights up the waveform and
 * histogram and makes any scaling ringing obvious.
 *
 * The top ramp animates its phase so the waveform trace slides; the zone plate
 * breathes its spatial frequency so aliasing shimmer is visible.
 *
 * @param {CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D} ctx
 * @param {number} w
 * @param {number} h
 * @param {number} t Elapsed time in milliseconds.
 */
export function lumaSweepZonePlate(ctx, w, h, t) {
  const splitY = Math.round(h * 0.42);

  // Top: horizontal luma ramp whose phase drifts, plus a moving bright bar
  // that reaches full white to exercise the tone-map roll-off.
  const phase = (t / 2600) % 1;
  const grad = ctx.createLinearGradient(0, 0, w, 0);
  for (let i = 0; i <= 8; i += 1) {
    const p = i / 8;
    const v = Math.round(255 * (0.5 - 0.5 * Math.cos(2 * Math.PI * (p + phase))));
    grad.addColorStop(p, `rgb(${v},${v},${v})`);
  }
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, w, splitY);

  // Bottom: zone plate. Draw into an ImageData for the radial chirp.
  const zoneH = h - splitY;
  const image = ctx.createImageData(w, zoneH);
  const data = image.data;
  const cx = w / 2;
  const cy = zoneH / 2;
  // Breathing spatial-frequency scale.
  const k = 0.00022 + 0.00012 * (0.5 + 0.5 * Math.sin(t / 1700));
  const norm = 1 / Math.max(1, Math.min(w, zoneH));
  for (let y = 0; y < zoneH; y += 1) {
    for (let x = 0; x < w; x += 1) {
      const dx = x - cx;
      const dy = y - cy;
      const r2 = dx * dx + dy * dy;
      const v = 0.5 + 0.5 * Math.cos(k * r2 * (1 + 40 * norm));
      const c = (v * 255) | 0;
      const idx = (y * w + x) * 4;
      data[idx] = c;
      data[idx + 1] = c;
      data[idx + 2] = c;
      data[idx + 3] = 255;
    }
  }
  ctx.putImageData(image, 0, splitY);
}

/**
 * An animated wall of maximally saturated, rotating hues. Read as BT.2020 and
 * converted to BT.709 these are wildly out of gamut, so with the gamut stage
 * engaged the vectorscope trace visibly pulls back inside the graticule — the
 * gamut-mapping demonstration. Also good for exercising the vectorscope's
 * angular coverage since every hue is present each frame.
 *
 * @param {CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D} ctx
 * @param {number} w
 * @param {number} h
 * @param {number} t Elapsed time in milliseconds.
 */
export function outOfGamutSaturated(ctx, w, h, t) {
  const cols = 24;
  const rows = 14;
  const cw = w / cols;
  const ch = h / rows;
  const rot = t / 40; // degrees
  for (let r = 0; r < rows; r += 1) {
    for (let c = 0; c < cols; c += 1) {
      const hue = (((c / cols) * 360 + (r / rows) * 60 + rot) % 360 + 360) % 360;
      // Full saturation, high lightness → the most gamut-stressing colours.
      const light = 46 + 10 * Math.sin((r + c) * 0.6 + t / 600);
      ctx.fillStyle = `hsl(${hue}, 100%, ${light}%)`;
      ctx.fillRect(Math.floor(c * cw), Math.floor(r * ch), Math.ceil(cw), Math.ceil(ch));
    }
  }
  // A sweeping full-white wedge so there is a clipping target here too.
  const wedge = ((t / 3000) % 1) * w;
  const wedgeW = Math.max(30, w * 0.06);
  const g = ctx.createLinearGradient(wedge - wedgeW, 0, wedge + wedgeW, 0);
  g.addColorStop(0, 'rgba(255,255,255,0)');
  g.addColorStop(0.5, 'rgba(255,255,255,0.9)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);
}

/**
 * @typedef {Object} PatternDef
 * @property {string} key Stable identifier used by the UI.
 * @property {string} label Human-facing name.
 * @property {(ctx: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D, w: number, h: number, t: number) => void} draw
 * @property {string} hint One-line description shown when selected.
 */

/**
 * The registry of built-in patterns, in menu order.
 * @type {ReadonlyArray<PatternDef>}
 */
export const PATTERNS = [
  {
    key: 'bars',
    label: 'SMPTE colour bars',
    draw: smpteBars,
    hint: '75% bars with a 100% white patch + luma ramp — the tone-map clip target.',
  },
  {
    key: 'sweep',
    label: 'Luma sweep + zone plate',
    draw: lumaSweepZonePlate,
    hint: 'Drifting ramp and a breathing zone-plate chirp — high-frequency detail.',
  },
  {
    key: 'gamut',
    label: 'Out-of-gamut saturation',
    draw: outOfGamutSaturated,
    hint: 'Rotating full-saturation hues — engage BT.2020→709 to pull them in.',
  },
];

/**
 * Looks up a pattern definition by key, defaulting to the first pattern.
 * @param {string} key
 * @returns {PatternDef}
 */
export function patternByKey(key) {
  return PATTERNS.find((p) => p.key === key) ?? PATTERNS[0];
}

// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * OxiScope — the OxiMedia web demo.
 *
 * WebCodecs hands you decoded frames; OxiScope shows you what to do with them.
 * A dropped clip (or the webcam, or a built-in procedural pattern) is graded by
 * the `@cooljapan/oximedia-web/color` WebAssembly pipeline and the *graded*
 * output is fed to four live broadcast scopes from `.../scopes` — waveform,
 * vectorscope, histogram and false-colour — every frame.
 *
 * No build step, no bundler, no network beyond this origin: pure static ES
 * modules importing the built wasm wrappers from `../dist`. Nothing is ever
 * uploaded — files become in-page `blob:` URLs, the webcam and patterns stay
 * local — which is what the "uploaded: 0 bytes" badge attests.
 *
 * @module app
 */

import { Scopes } from '../dist/scopes.js';
import { ColorPipeline } from '../dist/color.js';
import { detectCapabilities } from '../dist/_frame.js';
import { PATTERNS } from './patterns.js';
import { InputController } from './sources.js';

/** Fixed render size for each scope canvas (native bitmap; CSS scales it). */
const SCOPE_SIZES = {
  waveform: { w: 512, h: 256 },
  vectorscope: { w: 256, h: 256 },
  histogram: { w: 512, h: 256 },
  falseColor: { w: 480, h: 270 },
};

/** Bounded working resolution the scopes analyse (source is downscaled to it). */
const SCOPE_SOURCE = { w: 640, h: 360 };

/** Resolution the hidden pattern canvas renders at. */
const PATTERN_SIZE = { w: 960, h: 540 };

/**
 * Rolling per-frame cost (ms) above which the scope set alternates to protect
 * the preview's frame rate. Chosen a little under a 30 fps budget.
 */
const ALTERNATE_THRESHOLD_MS = 26;

/**
 * The mutable grade the UI edits and {@link reconfigurePipeline} pushes into the
 * wasm pipeline. All values are neutral at rest (an identity grade).
 * @typedef {Object} GradeState
 * @property {number} exposure Stops.
 * @property {number} contrast Power around the 0.18 pivot; 1 = neutral.
 * @property {number} saturation BT.709 luma blend; 1 = neutral.
 * @property {string|null} toneMap Operator name, or null for off.
 * @property {number} peakNits Target display peak for the tone-map.
 * @property {{src: string, dst: string}|null} gamut Primaries conversion, or null.
 */

/** @returns {GradeState} A fresh identity grade. */
function neutralGrade() {
  return {
    exposure: 0,
    contrast: 1,
    saturation: 1,
    toneMap: null,
    peakNits: 100,
    gamut: null,
  };
}

/** Short-hand `document.getElementById`. @param {string} id */
const $ = (id) => document.getElementById(id);

/**
 * The demo's whole runtime: capabilities, the wasm instances, the input
 * controller, the render loop and the DOM it drives.
 */
class OxiScopeApp {
  constructor() {
    /** @private @type {import('./sources.js').SourceCapabilities & { copyToRgba: boolean, simd: boolean }} */
    this._caps = detectCapabilities();
    /** @private @type {ColorPipeline|null} */
    this._pipe = null;
    /** @private */ this._scopes = /** @type {Record<string, Scopes>} */ ({});
    /** @private @type {InputController|null} */
    this._input = null;
    /** @private */ this._grade = neutralGrade();

    // Canvases.
    /** @private @type {HTMLCanvasElement} */ this._preview = $('preview');
    /** @private @type {HTMLCanvasElement} */ this._scopeSrc = $('scope-src');
    /** @private @type {CanvasRenderingContext2D|null} */ this._scopeSrcCtx = null;

    // Perf bookkeeping.
    /** @private */ this._lastFrameTs = 0;
    /** @private */ this._fps = 0;
    /** @private */ this._gradeMs = 0;
    /** @private */ this._scopeMs = { waveform: 0, vectorscope: 0, histogram: 0, falseColor: 0 };
    /** @private */ this._alternate = false;
    /** @private */ this._phase = 0;
    /** @private */ this._rollingCost = 0;
    /** @private */ this._statsPending = false;
  }

  /** Boots the demo, or paints a fatal panel and marks the DOM on failure. */
  async start() {
    this._renderCapabilities();

    if (!this._caps.simd) {
      this._fatal(
        'wasm-simd-unsupported',
        'This browser cannot run WebAssembly SIMD (simd128).',
        'The OxiMedia modules are compiled with SIMD for speed and will not ' +
          'load without it. Try the latest Chrome, Edge, Firefox or Safari. ' +
          'There is no software-codec fallback — that is the point of the project.',
      );
      return;
    }

    try {
      await this._initWasm();
    } catch (err) {
      this._fatal('wasm-init-failed', 'Failed to initialise the WebAssembly modules.', String(err));
      return;
    }

    this._measureWasm();
    // Resource-timing can populate a beat after the fetch settles.
    setTimeout(() => this._measureWasm(), 400);

    this._buildControls();
    this._wireSourceButtons();
    this._wireDragAndDrop();
    this._setupInput();

    if (!this._caps.videoFrame) {
      this._showFallbackNote();
    }

    // The headless-smoke marker: wasm is up and the UI is live.
    document.body.dataset.oxiscope = 'ready';
    this._setStatus('Ready — drop a clip, start the webcam, or pick a test pattern.');
    this._applyQueryOptions();
  }

  /**
   * Applies start-up options from the URL query string so docs screenshots and
   * headless harnesses can boot straight into a live state:
   *
   * - `?autosource=<pattern key>` starts that procedural pattern exactly as if
   *   the user had picked it (patterns need no gesture or permission, so this
   *   is safe to honour unconditionally);
   * - `?tonemap=<operator>` preselects a tone-map operator from the menu.
   *
   * Unknown values are ignored.
   * @private
   */
  _applyQueryOptions() {
    const params = new URLSearchParams(window.location.search);

    const tone = params.get('tonemap');
    const tonemapSel = /** @type {HTMLSelectElement} */ ($('tonemap'));
    if (tone && Array.from(tonemapSel.options).some((o) => o.value === tone)) {
      tonemapSel.value = tone;
      tonemapSel.dispatchEvent(new Event('change'));
    }

    const auto = params.get('autosource');
    if (auto && PATTERNS.some((p) => p.key === auto)) {
      this._startPattern(auto);
    }
  }

  /** @private Creates the four scope renderers and the colour pipeline. */
  async _initWasm() {
    const [waveform, vectorscope, histogram, falseColor, pipe] = await Promise.all([
      Scopes.create({ width: SCOPE_SIZES.waveform.w, height: SCOPE_SIZES.waveform.h, graticule: true }),
      Scopes.create({ width: SCOPE_SIZES.vectorscope.w, height: SCOPE_SIZES.vectorscope.h, graticule: true }),
      Scopes.create({ width: SCOPE_SIZES.histogram.w, height: SCOPE_SIZES.histogram.h, graticule: true }),
      Scopes.create({ width: SCOPE_SIZES.falseColor.w, height: SCOPE_SIZES.falseColor.h, graticule: true }),
      ColorPipeline.create(),
    ]);
    this._scopes = { waveform, vectorscope, histogram, falseColor };
    this._pipe = pipe;
    this.reconfigurePipeline();

    // Bounded scope-source canvas.
    this._scopeSrc.width = SCOPE_SOURCE.w;
    this._scopeSrc.height = SCOPE_SOURCE.h;
    this._scopeSrcCtx = this._scopeSrc.getContext('2d', { willReadFrequently: true });
  }

  /** @private Wires up the input controller with the demo's frame callback. */
  _setupInput() {
    const patternCanvas = $('pattern-canvas');
    patternCanvas.width = PATTERN_SIZE.w;
    patternCanvas.height = PATTERN_SIZE.h;
    this._input = new InputController({
      video: $('stage-video'),
      patternCanvas,
      capabilities: this._caps,
      onFrame: (source) => this._renderFrame(source),
      onSourceChange: (info) => this._onSourceChange(info),
    });
  }

  /**
   * The per-frame heart of the demo: grade the source, mirror the graded pixels
   * to the preview, then feed a bounded downscale of them to the scopes.
   *
   * The single persistent graded RGBA buffer lives inside the wasm pipeline
   * (grow-once) and is painted straight onto the visible preview canvas by
   * `applyToCanvas`; the scopes read a fixed 640×360 downscale of it, so their
   * cost is bounded regardless of the source resolution. The driver owns and
   * closes `source` — we never do.
   *
   * @private
   * @param {VideoFrame|HTMLVideoElement} source
   * @returns {Promise<void>}
   */
  async _renderFrame(source) {
    const pipe = this._pipe;
    const ctx = this._scopeSrcCtx;
    if (!pipe || !ctx) {
      return;
    }

    const t0 = performance.now();
    // Grade + preview in one shot (works for VideoFrame and <video> sources).
    await pipe.applyToCanvas(source, this._preview);
    const tGrade = performance.now();
    this._gradeMs = tGrade - t0;

    // Bounded downscale of the graded frame becomes the scopes' input.
    ctx.drawImage(this._preview, 0, 0, this._scopeSrc.width, this._scopeSrc.height);

    // Decide which scopes to run this frame (all four, or half when alternating).
    const runAll = !this._alternate;
    const runA = runAll || this._phase === 0;
    const runB = runAll || this._phase === 1;
    this._phase ^= 1;

    const src = this._scopeSrc;
    if (runA) {
      await this._timeScope('waveform', () =>
        this._scopes.waveform.waveform(src, $('wf'), { mode: 'rgb-parade', ire: true }),
      );
      await this._timeScope('vectorscope', () =>
        this._scopes.vectorscope.vectorscope(src, $('vs'), {
          gain: 1.0,
          skinToneLine: true,
          graticule: true,
        }),
      );
    }
    if (runB) {
      await this._timeScope('histogram', () =>
        this._scopes.histogram.histogram(src, $('hist'), { mode: 'rgb' }),
      );
      await this._timeScope('falseColor', () =>
        this._scopes.falseColor.falseColor(src, $('fc'), { preset: 'arri' }),
      );
    }

    const tEnd = performance.now();
    this._updatePerf(t0, tEnd);
  }

  /**
   * Times a single scope render into `this._scopeMs`.
   * @private
   * @param {'waveform'|'vectorscope'|'histogram'|'falseColor'} key
   * @param {() => Promise<void>} run
   */
  async _timeScope(key, run) {
    const s = performance.now();
    await run();
    this._scopeMs[key] = performance.now() - s;
  }

  /**
   * Updates the fps EMA and the alternate-mode decision, then schedules a
   * throttled stats-line repaint.
   * @private
   * @param {number} frameStart
   * @param {number} frameEnd
   */
  _updatePerf(frameStart, frameEnd) {
    if (this._lastFrameTs) {
      const dt = frameStart - this._lastFrameTs;
      if (dt > 0) {
        const inst = 1000 / dt;
        this._fps = this._fps ? this._fps * 0.85 + inst * 0.15 : inst;
      }
    }
    this._lastFrameTs = frameStart;

    const cost = frameEnd - frameStart;
    this._rollingCost = this._rollingCost ? this._rollingCost * 0.8 + cost * 0.2 : cost;
    // Hysteresis so we don't flap in and out of alternate mode.
    if (!this._alternate && this._rollingCost > ALTERNATE_THRESHOLD_MS) {
      this._alternate = true;
    } else if (this._alternate && this._rollingCost < ALTERNATE_THRESHOLD_MS * 0.6) {
      this._alternate = false;
    }

    if (!this._statsPending) {
      this._statsPending = true;
      requestAnimationFrame(() => {
        this._statsPending = false;
        this._paintStats();
      });
    }
  }

  /** @private Paints the honest per-frame perf line. */
  _paintStats() {
    const s = this._scopeMs;
    const scopeTotal = s.waveform + s.vectorscope + s.histogram + s.falseColor;
    const mode = this._alternate ? 'alternating (perf)' : 'full';
    $('stats').textContent =
      `${this._fps.toFixed(0)} fps · grade ${this._gradeMs.toFixed(1)} ms · ` +
      `scopes ${scopeTotal.toFixed(1)} ms ` +
      `(wf ${s.waveform.toFixed(1)} / vec ${s.vectorscope.toFixed(1)} / ` +
      `hist ${s.histogram.toFixed(1)} / fc ${s.falseColor.toFixed(1)}) · ${mode}`;
  }

  /**
   * Rebuilds the wasm pipeline from the current {@link GradeState}. Cheap —
   * just parameter setters — so it is safe to call on every control input.
   */
  reconfigurePipeline() {
    const pipe = this._pipe;
    if (!pipe) {
      return;
    }
    const g = this._grade;
    pipe.exposure(g.exposure).contrast(g.contrast).saturation(g.saturation);
    if (g.toneMap) {
      pipe.toneMap(g.toneMap, { peakNits: g.peakNits, inputPeakNits: 1000 });
    } else {
      pipe.toneMap(null);
    }
    if (g.gamut) {
      pipe.gamut(g.gamut.src, g.gamut.dst, { softness: 0.25 });
    } else {
      pipe.gamut(null);
    }
  }

  // ---- UI construction -----------------------------------------------------

  /** @private Populates the pattern menu and binds every grade control. */
  _buildControls() {
    const patternSel = $('pattern-select');
    for (const p of PATTERNS) {
      const opt = document.createElement('option');
      opt.value = p.key;
      opt.textContent = p.label;
      patternSel.appendChild(opt);
    }

    const exposure = $('exposure');
    const exposureVal = $('exposure-val');
    exposure.addEventListener('input', () => {
      this._grade.exposure = Number(exposure.value);
      exposureVal.textContent = `${this._grade.exposure.toFixed(1)} EV`;
      this.reconfigurePipeline();
    });

    const contrast = $('contrast');
    const contrastVal = $('contrast-val');
    contrast.addEventListener('input', () => {
      this._grade.contrast = Number(contrast.value);
      contrastVal.textContent = this._grade.contrast.toFixed(2);
      this.reconfigurePipeline();
    });

    const saturation = $('saturation');
    const saturationVal = $('saturation-val');
    saturation.addEventListener('input', () => {
      this._grade.saturation = Number(saturation.value);
      saturationVal.textContent = this._grade.saturation.toFixed(2);
      this.reconfigurePipeline();
    });

    const tonemap = $('tonemap');
    tonemap.addEventListener('change', () => {
      this._grade.toneMap = tonemap.value === 'off' ? null : tonemap.value;
      this.reconfigurePipeline();
    });

    const peaknits = $('peaknits');
    const peaknitsVal = $('peaknits-val');
    peaknits.addEventListener('input', () => {
      this._grade.peakNits = Number(peaknits.value);
      peaknitsVal.textContent = `${this._grade.peakNits} nits`;
      this.reconfigurePipeline();
    });

    const gamut = $('gamut');
    gamut.addEventListener('change', () => {
      if (gamut.value === 'off') {
        this._grade.gamut = null;
      } else {
        const [src, dst] = gamut.value.split('>');
        this._grade.gamut = { src, dst };
      }
      this.reconfigurePipeline();
    });

    $('btn-export').addEventListener('click', () => this._exportCube());
    $('btn-reset').addEventListener('click', () => this._resetGrade());
  }

  /** @private Binds the source-selection buttons and the file picker. */
  _wireSourceButtons() {
    const fileInput = $('file-input');
    const openPicker = () => fileInput.click();
    $('btn-file').addEventListener('click', openPicker);
    $('drop-file').addEventListener('click', openPicker);
    fileInput.addEventListener('change', () => {
      const file = fileInput.files && fileInput.files[0];
      if (file) {
        this._startFile(file);
      }
    });

    const webcam = () => this._startWebcam();
    $('btn-webcam').addEventListener('click', webcam);
    $('drop-webcam').addEventListener('click', webcam);

    const pattern = () => this._startPattern($('pattern-select').value || PATTERNS[0].key);
    $('drop-pattern').addEventListener('click', pattern);
    $('pattern-select').addEventListener('change', () => {
      if (this._input && this._input.kind === 'pattern') {
        this._startPattern($('pattern-select').value);
      }
    });
    $('btn-pattern').addEventListener('click', pattern);

    $('btn-stop').addEventListener('click', () => this._stopSource());
  }

  /** @private Whole-page drag-and-drop for video files. */
  _wireDragAndDrop() {
    const zone = $('preview-pane');
    const dz = $('dropzone');
    const activate = (on) => dz.classList.toggle('dragging', on);
    ['dragenter', 'dragover'].forEach((ev) =>
      zone.addEventListener(ev, (e) => {
        e.preventDefault();
        activate(true);
      }),
    );
    ['dragleave', 'dragend'].forEach((ev) =>
      zone.addEventListener(ev, (e) => {
        e.preventDefault();
        activate(false);
      }),
    );
    zone.addEventListener('drop', (e) => {
      e.preventDefault();
      activate(false);
      const dt = e.dataTransfer;
      if (!dt || !dt.files || dt.files.length === 0) {
        return;
      }
      const file = dt.files[0];
      if (!file.type.startsWith('video/')) {
        this._setStatus(`"${file.name}" is not a video file.`, true);
        return;
      }
      this._startFile(file);
    });
  }

  // ---- Source actions ------------------------------------------------------

  /** @private @param {File} file */
  async _startFile(file) {
    try {
      await this._input.useFile(file);
    } catch (err) {
      this._setStatus(`Could not play that file: ${err}`, true);
    }
  }

  /** @private */
  async _startWebcam() {
    this._setStatus('Requesting the webcam…');
    try {
      await this._input.useWebcam();
    } catch (err) {
      this._setStatus(`Webcam unavailable: ${err instanceof Error ? err.message : err}`, true);
    }
  }

  /** @private @param {string} key */
  _startPattern(key) {
    try {
      this._input.usePattern(key);
      $('pattern-select').value = key;
    } catch (err) {
      this._setStatus(`Pattern failed: ${err}`, true);
    }
  }

  /** @private */
  _stopSource() {
    if (this._input) {
      this._input.stop();
    }
    document.body.classList.remove('has-source');
    this._lastFrameTs = 0;
    this._fps = 0;
    this._setStatus('Stopped — pick a source to start again.');
  }

  /**
   * @private
   * @param {{ kind: string, label: string, width: number, height: number }} info
   */
  _onSourceChange(info) {
    document.body.classList.add('has-source');
    const dims = info.width && info.height ? ` · ${info.width}×${info.height}` : '';
    this._setStatus(`Source: ${info.label} (${info.kind}${dims})`);
  }

  // ---- Export / reset ------------------------------------------------------

  /** @private Bakes the current grade to a 33³ `.cube` and downloads it. */
  _exportCube() {
    if (!this._pipe) {
      return;
    }
    let text;
    try {
      text = this._pipe.exportCube({ size: 33 });
    } catch (err) {
      this._setStatus(`Export failed: ${err}`, true);
      return;
    }
    const blob = new Blob([text], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'oxiscope-grade.cube';
    document.body.appendChild(a);
    a.click();
    a.remove();
    // Revoke after the click has been dispatched.
    setTimeout(() => URL.revokeObjectURL(url), 0);
    this._setStatus('Exported oxiscope-grade.cube (33³) — nothing left the browser.');
  }

  /** @private Returns every grade control to neutral. */
  _resetGrade() {
    this._grade = neutralGrade();
    /** @type {[string, string][]} */
    const pairs = [
      ['exposure', '0'],
      ['contrast', '1'],
      ['saturation', '1'],
      ['peaknits', '100'],
    ];
    for (const [id, v] of pairs) {
      const el = /** @type {HTMLInputElement} */ ($(id));
      el.value = v;
    }
    $('exposure-val').textContent = '0.0 EV';
    $('contrast-val').textContent = '1.00';
    $('saturation-val').textContent = '1.00';
    $('peaknits-val').textContent = '100 nits';
    /** @type {HTMLSelectElement} */ ($('tonemap')).value = 'off';
    /** @type {HTMLSelectElement} */ ($('gamut')).value = 'off';
    this.reconfigurePipeline();
    this._setStatus('Grade reset to identity.');
  }

  // ---- Badges, capabilities, footer ---------------------------------------

  /**
   * Sums the transferred size of every `.wasm` resource actually loaded and
   * updates the badge + footer. Never hard-coded — read from Resource Timing.
   * @private
   */
  _measureWasm() {
    const entries = performance
      .getEntriesByType('resource')
      .filter((e) => e.name.endsWith('.wasm'));
    let total = 0;
    /** @type {Record<string, number>} */
    const perModule = {};
    for (const e of entries) {
      const bytes = e.transferSize || e.encodedBodySize || 0;
      total += bytes;
      const m = e.name.match(/\/wasm\/([^/]+)\//);
      if (m) {
        perModule[m[1]] = bytes;
      }
    }
    if (total > 0) {
      $('wasm-badge').textContent = `${(total / 1024).toFixed(1)} kB`;
    }
    const names = Object.keys(perModule).sort();
    if (names.length > 0) {
      const parts = names.map((n) => `${n} ${(perModule[n] / 1024).toFixed(1)} kB`);
      $('footer-modules').textContent = `Modules loaded: ${parts.join(' · ')}`;
    }
  }

  /** @private Fills the capability readout panel. */
  _renderCapabilities() {
    const c = this._caps;
    /** @type {[string, boolean][]} */
    const rows = [
      ['WebCodecs VideoFrame', c.videoFrame],
      ['copyTo(RGBA)', c.copyToRgba],
      ['requestVideoFrameCallback', c.rvfc],
      ['MediaStreamTrackProcessor', c.trackProcessor],
      ['wasm SIMD (simd128)', c.simd],
    ];
    const panel = $('cap-list');
    panel.textContent = '';
    for (const [label, ok] of rows) {
      const li = document.createElement('li');
      li.className = ok ? 'cap-ok' : 'cap-no';
      li.textContent = `${ok ? '✓' : '✗'} ${label}`;
      panel.appendChild(li);
    }
  }

  /** @private Shows the non-fatal WebCodecs fallback note. */
  _showFallbackNote() {
    const note = $('fallback-note');
    note.hidden = false;
    note.textContent =
      'WebCodecs VideoFrame is unavailable here — running the canvas fallback ' +
      'path. Grading and all four scopes still work (via getImageData); only ' +
      'the zero-copy frame path is off.';
  }

  /**
   * Paints a fatal error panel, disables interaction and marks the DOM so the
   * headless smoke test can assert the failure mode too.
   * @private
   * @param {string} code
   * @param {string} title
   * @param {string} detail
   */
  _fatal(code, title, detail) {
    const panel = $('error-panel');
    panel.hidden = false;
    $('error-title').textContent = title;
    $('error-detail').textContent = detail;
    $('dropzone').hidden = true;
    document.body.dataset.oxiscope = `error:${code}`;
  }

  /**
   * @private
   * @param {string} message
   * @param {boolean} [isError]
   */
  _setStatus(message, isError = false) {
    const el = $('status');
    el.textContent = message;
    el.classList.toggle('status-error', isError);
  }
}

// Boot once the DOM is parsed. `defer` on the module script already guarantees
// this, but guard anyway for direct/late injection.
function boot() {
  const app = new OxiScopeApp();
  app.start().catch((err) => {
    document.body.dataset.oxiscope = `error:boot-${err}`;
    const panel = document.getElementById('error-panel');
    if (panel) {
      panel.hidden = false;
      const detail = document.getElementById('error-detail');
      if (detail) {
        detail.textContent = String(err);
      }
    }
  });
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot, { once: true });
} else {
  boot();
}

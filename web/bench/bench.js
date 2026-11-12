// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * oximedia-web benchmark harness (M4a).
 *
 * Reproducibility is the product: every number this page reports is
 * produced by running the actual published `dist/*.js` wrappers against
 * deterministic, in-page-generated synthetic frames (see `lib/rng.js`) —
 * nothing here is a placeholder or an estimate. An empty results table
 * means "nobody has run this on this machine yet," not "numbers pending."
 *
 * Load with `?auto=1` to run automatically and report completion via
 * `console.log('OXIBENCH_RESULT:' + json)` / `console.log('OXIBENCH_ERROR:'
 * + json)` — this is what `run.sh` scrapes from headless Chrome's
 * `--enable-logging=stderr` output. Without `?auto=1`, use the "Run
 * benchmarks" button.
 *
 * @module bench
 */

import { Scopes } from "../dist/scopes.js";
import { ColorPipeline, loadCubeLut } from "../dist/color.js";
import { Scaler } from "../dist/scale.js";
import { Quality } from "../dist/quality.js";
import { detectCapabilities } from "../dist/_frame.js";

import { generateGradientFrame, generateNoiseFrame, deriveDistorted } from "./lib/rng.js";
import { buildFrameSource } from "./lib/frame-source.js";
import { timeSuite } from "./lib/harness.js";
import { WaveformBaseline, HistogramBaseline } from "./lib/baselines.js";

/** Results-JSON schema version (see bench/README.md for the shape). */
const SCHEMA_VERSION = 1;

const FRAME_W = 1920;
const FRAME_H = 1080;
const FRAME4K_W = 3840;
const FRAME4K_H = 2160;
const SCOPE_W = 512;
const SCOPE_H = 256;

// Fixed seeds -> deterministic frames on any machine (see lib/rng.js's
// module doc for why this replaces shipping binary test-image assets).
// Each constant names the one logical frame it produces; re-deriving a
// frame elsewhere with the same seed + dimensions reproduces it
// byte-for-byte, which lets two suites legitimately share "the same frame"
// without literally sharing one JS object (necessary here since some
// suites need the pixels packaged differently — e.g. a VideoFrame for the
// scale suite vs. a plain OffscreenCanvas for the baseline it's compared
// against).
const SEED_GRADIENT_1080 = 0x1080f00d;
const SEED_NOISE_1080 = 0x1080dead;
const SEED_GRADIENT_4K = 0x4000f00d;
const SEED_QUALITY_REF = 0x0a11a11a;
const SEED_QUALITY_DIST = 0x0a11d15d;

const WARMUP = 10;
const MEASURE = 60;

/**
 * Builds every persistent object the suites need (wasm wrapper instances,
 * frame sources, output canvases, baseline scratch buffers), all allocated
 * once, outside any timed loop.
 *
 * @returns {Promise<{
 *   suiteDefs: Array<{name: string, fn: () => (void | Promise<void>)}>,
 *   env: Record<string, unknown>,
 *   dispose: () => void,
 * }>}
 */
async function setupContext() {
  const bufGradient1080 = generateGradientFrame(FRAME_W, FRAME_H, SEED_GRADIENT_1080);
  const bufNoise1080 = generateNoiseFrame(FRAME_W, FRAME_H, SEED_NOISE_1080);
  // Two independent buffers, same seed: byte-identical content, packaged
  // separately for the wasm scale suite (VideoFrame/canvas source) and the
  // Canvas2D baseline suite (its own persistent source canvas) so neither
  // suite can accidentally mutate the other's input.
  const bufGradient4kForScale = generateGradientFrame(FRAME4K_W, FRAME4K_H, SEED_GRADIENT_4K);
  const bufGradient4kForBaseline = generateGradientFrame(FRAME4K_W, FRAME4K_H, SEED_GRADIENT_4K);
  const bufQualityRef = generateGradientFrame(FRAME_W, FRAME_H, SEED_QUALITY_REF);
  const bufQualityDist = deriveDistorted(bufQualityRef, SEED_QUALITY_DIST);

  const frameGradient1080 = buildFrameSource(bufGradient1080, FRAME_W, FRAME_H);
  const frameNoise1080 = buildFrameSource(bufNoise1080, FRAME_W, FRAME_H);
  const frameGradient4k = buildFrameSource(bufGradient4kForScale, FRAME4K_W, FRAME4K_H);

  /** @type {Array<() => void>} Best-effort cleanup, run in `dispose()`. */
  const disposers = [frameGradient1080.dispose, frameNoise1080.dispose, frameGradient4k.dispose];

  // --- scopes ---------------------------------------------------------
  const scopes = await Scopes.create({ width: SCOPE_W, height: SCOPE_H, graticule: true });
  const scopesCanvas = new OffscreenCanvas(SCOPE_W, SCOPE_H);
  disposers.push(() => scopes.free());

  // --- color: exposure + ACES tone map + a real (identity) 33^3 LUT ---
  // The LUT's content doesn't matter for timing purposes (it exists to
  // exercise the tetrahedral-interpolation compute path); baking it from a
  // throwaway neutral pipeline keeps the suite self-contained — no .cube
  // fixture file to keep in sync.
  const colorPipe = await ColorPipeline.create();
  const lutSeedPipe = await ColorPipeline.create();
  const cubeText = lutSeedPipe.exportCube({ size: 33 });
  lutSeedPipe.free();
  const lut33 = await loadCubeLut(cubeText);
  colorPipe
    .exposure(0.7)
    .toneMap("aces", { peakNits: 100, inputPeakNits: 1000 })
    .lut(lut33, { interp: "tetrahedral" });
  const colorCanvas = new OffscreenCanvas(FRAME_W, FRAME_H);
  disposers.push(() => {
    lut33.free();
    colorPipe.free();
  });

  // --- scale: lanczos3, 4K -> 1080p -----------------------------------
  const scaler = await Scaler.create({ dstWidth: FRAME_W, dstHeight: FRAME_H, filter: "lanczos3" });
  const scaleCanvas = new OffscreenCanvas(FRAME_W, FRAME_H);
  disposers.push(() => scaler.dispose());

  // --- quality: PSNR + SSIM, raw-buffer fast path ---------------------
  const quality = await Quality.create({ width: FRAME_W, height: FRAME_H });
  disposers.push(() => quality.free());

  // --- baseline: Canvas2D drawImage + getImageData downscale ----------
  const baselineSrcCanvas = new OffscreenCanvas(FRAME4K_W, FRAME4K_H);
  const baselineSrcCtx = baselineSrcCanvas.getContext("2d");
  if (!baselineSrcCtx) {
    throw new Error("bench: failed to acquire a 2D context for the Canvas2D baseline source");
  }
  baselineSrcCtx.putImageData(new ImageData(bufGradient4kForBaseline, FRAME4K_W, FRAME4K_H), 0, 0);
  const baselineDstCanvas = new OffscreenCanvas(FRAME_W, FRAME_H);
  const baselineDstCtx = baselineDstCanvas.getContext("2d");
  if (!baselineDstCtx) {
    throw new Error("bench: failed to acquire a 2D context for the Canvas2D baseline destination");
  }

  // --- baseline: pure-JS luma waveform / histogram ---------------------
  const waveformBaseline = new WaveformBaseline(SCOPE_W, SCOPE_H);
  const waveformBaselineOut = new Uint8ClampedArray(SCOPE_W * SCOPE_H * 4);
  const waveformBaselineCanvas = new OffscreenCanvas(SCOPE_W, SCOPE_H);
  const waveformBaselineCtx = waveformBaselineCanvas.getContext("2d");
  const waveformBaselineImage = new ImageData(waveformBaselineOut, SCOPE_W, SCOPE_H);
  if (!waveformBaselineCtx) {
    throw new Error("bench: failed to acquire a 2D context for the waveform baseline canvas");
  }

  const histogramBaseline = new HistogramBaseline(SCOPE_W, SCOPE_H);
  const histogramBaselineOut = new Uint8ClampedArray(SCOPE_W * SCOPE_H * 4);
  const histogramBaselineCanvas = new OffscreenCanvas(SCOPE_W, SCOPE_H);
  const histogramBaselineCtx = histogramBaselineCanvas.getContext("2d");
  const histogramBaselineImage = new ImageData(histogramBaselineOut, SCOPE_W, SCOPE_H);
  if (!histogramBaselineCtx) {
    throw new Error("bench: failed to acquire a 2D context for the histogram baseline canvas");
  }

  const suiteDefs = [
    {
      name: "scopes: waveform (1080p)",
      fn: () => scopes.waveform(frameGradient1080.source, scopesCanvas, { mode: "rgb-parade", ire: true }),
    },
    {
      name: "scopes: vectorscope (1080p)",
      fn: () =>
        scopes.vectorscope(frameGradient1080.source, scopesCanvas, { gain: 1.0, skinToneLine: true }),
    },
    {
      name: "scopes: histogram (1080p)",
      fn: () => scopes.histogram(frameGradient1080.source, scopesCanvas, { mode: "rgb" }),
    },
    {
      name: "scopes: falseColor (1080p)",
      fn: () => scopes.falseColor(frameGradient1080.source, scopesCanvas, { preset: "arri" }),
    },
    {
      name: "scopes: all-four combined (1080p)",
      fn: async () => {
        await scopes.waveform(frameGradient1080.source, scopesCanvas, { mode: "rgb-parade", ire: true });
        await scopes.vectorscope(frameGradient1080.source, scopesCanvas, {
          gain: 1.0,
          skinToneLine: true,
        });
        await scopes.histogram(frameGradient1080.source, scopesCanvas, { mode: "rgb" });
        await scopes.falseColor(frameGradient1080.source, scopesCanvas, { preset: "arri" });
      },
    },
    {
      name: "scopes: all-four combined (1080p, worst-case noise)",
      fn: async () => {
        await scopes.waveform(frameNoise1080.source, scopesCanvas, { mode: "rgb-parade", ire: true });
        await scopes.vectorscope(frameNoise1080.source, scopesCanvas, { gain: 1.0, skinToneLine: true });
        await scopes.histogram(frameNoise1080.source, scopesCanvas, { mode: "rgb" });
        await scopes.falseColor(frameNoise1080.source, scopesCanvas, { preset: "arri" });
      },
    },
    {
      name: "color: exposure+aces+lut33 (1080p)",
      fn: () => colorPipe.applyToCanvas(frameGradient1080.source, colorCanvas),
    },
    {
      name: "scale: lanczos3 (4K -> 1080p)",
      fn: () => scaler.resizeToCanvas(frameGradient4k.source, scaleCanvas),
    },
    {
      name: "quality: ssim+psnr (1080p pair)",
      fn: () => quality.compare(bufQualityRef, bufQualityDist),
    },
    {
      name: "baseline-js: luma waveform (1080p)",
      fn: () => {
        waveformBaseline.run(bufGradient1080, FRAME_W, FRAME_H, waveformBaselineOut);
        waveformBaselineCtx.putImageData(waveformBaselineImage, 0, 0);
      },
    },
    {
      name: "baseline-js: luma histogram (1080p)",
      fn: () => {
        histogramBaseline.run(bufGradient1080, FRAME_W, FRAME_H, histogramBaselineOut);
        histogramBaselineCtx.putImageData(histogramBaselineImage, 0, 0);
      },
    },
    {
      name: "baseline-canvas2d: downscale drawImage+getImageData (4K -> 1080p)",
      fn: () => {
        baselineDstCtx.drawImage(baselineSrcCanvas, 0, 0, FRAME_W, FRAME_H);
        baselineDstCtx.getImageData(0, 0, FRAME_W, FRAME_H);
      },
    },
  ];

  const env = captureEnvironment({
    scopesColorFrameSourceKind: frameGradient1080.kind,
    scaleFrameSourceKind: frameGradient4k.kind,
  });

  return {
    suiteDefs,
    env,
    dispose() {
      for (const d of disposers) {
        try {
          d();
        } catch (_err) {
          // Best-effort cleanup only; a teardown failure must not mask the
          // benchmark's own result (or a prior, more informative error).
        }
      }
    },
  };
}

/**
 * Captures the environment metadata attached to every results report:
 * user agent, concurrency, pixel ratio, wasm resource-timing sizes, and a
 * timestamp — enough for a reader to tell which machine/browser produced a
 * given number, per the "reproducibility is the product" principle.
 *
 * @param {Record<string, unknown>} extra Suite-specific extras (e.g. which
 *   frame-acquisition path was used) merged into the returned object.
 * @returns {Record<string, unknown>}
 */
function captureEnvironment(extra) {
  const wasmResources = performance
    .getEntriesByType("resource")
    .filter((e) => e.name.endsWith(".wasm"))
    .map((e) => ({
      name: e.name.split("/").pop(),
      transferSize: e.transferSize,
      encodedBodySize: e.encodedBodySize,
      decodedBodySize: e.decodedBodySize,
    }));

  return {
    userAgent: navigator.userAgent,
    hardwareConcurrency: navigator.hardwareConcurrency ?? null,
    devicePixelRatio: typeof window !== "undefined" ? (window.devicePixelRatio ?? null) : null,
    timestamp: new Date().toISOString(),
    capabilities: detectCapabilities(),
    wasmResources,
    ...extra,
  };
}

/**
 * Runs every suite in order (setup -> timed suites -> teardown, teardown
 * always runs even on failure) and returns a results report matching the
 * committed schema: `{schema_version, env, results}`.
 *
 * @returns {Promise<{schema_version: number, env: Record<string, unknown>, results: Array}>}
 */
export async function runAll() {
  const ctx = await setupContext();
  try {
    const results = [];
    for (const def of ctx.suiteDefs) {
      setStatus(`running: ${def.name} (warmup ${WARMUP}, measure ${MEASURE})`);
      // Suites run sequentially and deliberately un-parallelized: this is a
      // latency harness, and overlapping suites would let the browser's
      // own scheduling noise (and shared wasm-heap contention) bleed
      // between what are supposed to be independent measurements.
      // eslint-disable-next-line no-await-in-loop
      const result = await timeSuite(def.name, def.fn, { warmup: WARMUP, measure: MEASURE });
      results.push(result);
    }
    return { schema_version: SCHEMA_VERSION, env: ctx.env, results };
  } finally {
    ctx.dispose();
  }
}

// ---------------------------------------------------------------------
// DOM wiring (skipped entirely if there is no `document`, so `runAll` and
// everything above it stays usable from a non-browser test harness too).
// ---------------------------------------------------------------------

/** @type {{schema_version: number, env: Record<string, unknown>, results: Array}|null} */
let lastReport = null;

/**
 * @param {string} text
 */
function setStatus(text) {
  if (typeof document === "undefined") return;
  const el = document.getElementById("status");
  if (el) el.textContent = text;
}

/**
 * @param {Record<string, unknown>} env
 */
function renderEnv(env) {
  const el = document.getElementById("env-info");
  if (!el) return;
  const caps = /** @type {Record<string, unknown>} */ (env.capabilities) ?? {};
  const lines = [
    `timestamp: ${env.timestamp}`,
    `userAgent: ${env.userAgent}`,
    `hardwareConcurrency: ${env.hardwareConcurrency}`,
    `devicePixelRatio: ${env.devicePixelRatio}`,
    `scopes/color frame source: ${env.scopesColorFrameSourceKind}`,
    `scale frame source: ${env.scaleFrameSourceKind}`,
    `wasm simd128 validated: ${caps.simd}`,
    `VideoFrame.copyTo(RGBA) confirmed: ${caps.copyToRgba}`,
  ];
  el.textContent = lines.join("\n");
}

/**
 * @param {Array<{name: string, n: number, median_ms: number, p95_ms: number, min_ms: number}>} results
 */
function renderResultsTable(results) {
  const tbody = document.getElementById("results-body");
  if (!tbody) return;
  tbody.textContent = "";
  for (const r of results) {
    const tr = document.createElement("tr");
    const cells = [r.name, String(r.n), r.median_ms.toFixed(3), r.p95_ms.toFixed(3), r.min_ms.toFixed(3)];
    for (const c of cells) {
      const td = document.createElement("td");
      td.textContent = c;
      tr.appendChild(td);
    }
    tbody.appendChild(tr);
  }
}

/**
 * @param {{schema_version: number, env: Record<string, unknown>, results: Array}} report
 */
function renderReport(report) {
  lastReport = report;
  renderEnv(report.env);
  renderResultsTable(report.results);
  const rawEl = document.getElementById("raw-json");
  if (rawEl) rawEl.textContent = JSON.stringify(report, null, 2);
  const downloadBtn = /** @type {HTMLButtonElement|null} */ (document.getElementById("download-btn"));
  if (downloadBtn) downloadBtn.disabled = false;
}

/**
 * Triggers a browser download of `lastReport` as
 * `oxibench-<timestamp>.json`. No-op if nothing has run yet.
 */
function downloadReport() {
  if (!lastReport) return;
  const blob = new Blob([JSON.stringify(lastReport, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  try {
    const a = document.createElement("a");
    a.href = url;
    a.download = `oxibench-${String(lastReport.env.timestamp).replace(/[:.]/g, "-")}.json`;
    document.body.appendChild(a);
    a.click();
    a.remove();
  } finally {
    URL.revokeObjectURL(url);
  }
}

function wireButtons() {
  const runBtn = /** @type {HTMLButtonElement|null} */ (document.getElementById("run-btn"));
  if (runBtn) {
    runBtn.addEventListener("click", async () => {
      runBtn.disabled = true;
      setStatus("running...");
      try {
        const report = await runAll();
        renderReport(report);
        setStatus("done");
      } catch (err) {
        setStatus(`error: ${err instanceof Error ? err.message : String(err)}`);
        console.error(err);
      } finally {
        runBtn.disabled = false;
      }
    });
  }

  const downloadBtn = document.getElementById("download-btn");
  if (downloadBtn) {
    downloadBtn.addEventListener("click", downloadReport);
  }
}

const AUTO_RUN =
  typeof location !== "undefined" && new URLSearchParams(location.search).get("auto") === "1";

async function main() {
  if (typeof document === "undefined") return;
  wireButtons();
  if (AUTO_RUN) {
    setStatus("running (auto)...");
    const report = await runAll();
    renderReport(report);
    setStatus("done (auto)");
    // Scraped by run.sh from headless Chrome's --enable-logging=stderr
    // output — see that script for the extraction logic.
    console.log(`OXIBENCH_RESULT:${JSON.stringify(report)}`);
  }
}

main().catch((err) => {
  console.error(err);
  setStatus(`error: ${err instanceof Error ? err.message : String(err)}`);
  if (AUTO_RUN) {
    // A distinct marker so run.sh's log-scraping loop can fail fast on a
    // genuine error instead of polling for OXIBENCH_RESULT until timeout.
    const message = err instanceof Error ? err.message : String(err);
    console.log(`OXIBENCH_ERROR:${JSON.stringify({ message })}`);
  }
});

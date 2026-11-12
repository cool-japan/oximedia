// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Timing harness: runs a suite function through a warmup phase (JIT warmup,
 * lazy allocation warmup — discarded) followed by a measured phase, timing
 * each measured call individually with `performance.now()` and reducing the
 * samples to median/p95/min.
 *
 * @module lib/harness
 */

/**
 * One suite's timing result, matching the published results-JSON schema.
 *
 * @typedef {Object} SuiteResult
 * @property {string} name Suite name, as shown in the results table.
 * @property {number} n Number of *measured* samples (warmup excluded).
 * @property {number} median_ms Median wall time per call, in milliseconds.
 * @property {number} p95_ms 95th-percentile wall time per call.
 * @property {number} min_ms Fastest observed call.
 */

/**
 * Reduces a list of millisecond samples to `{n, median_ms, p95_ms, min_ms}`.
 * Sorts a copy — never mutates `samples`.
 *
 * @param {number[]} samples
 * @returns {{n: number, median_ms: number, p95_ms: number, min_ms: number}}
 * @throws {Error} If `samples` is empty.
 */
export function reduceSamples(samples) {
  if (samples.length === 0) {
    throw new Error("reduceSamples: at least one sample is required");
  }
  const sorted = samples.slice().sort((a, b) => a - b);
  const n = sorted.length;
  const median =
    n % 2 === 1 ? sorted[(n - 1) / 2] : (sorted[n / 2 - 1] + sorted[n / 2]) / 2;
  const p95Index = Math.min(n - 1, Math.ceil(0.95 * n) - 1);
  return {
    n,
    median_ms: median,
    p95_ms: sorted[p95Index],
    min_ms: sorted[0],
  };
}

/**
 * Runs `fn` `warmup` times (untimed, results discarded — lets the JIT tier
 * up and any grow-once buffers inside the wrapper reach their steady-state
 * size before anything is measured), then `measure` times, timing each
 * measured call individually via `performance.now()`.
 *
 * `fn` may be sync or async; each call is `await`-ed before starting the
 * next timer, so async suites measure their full await chain (including
 * any real microtask/macrotask latency such as `VideoFrame.copyTo`), not
 * just synchronous CPU time.
 *
 * @param {string} name Suite name (carried into the returned result).
 * @param {() => (void | Promise<void>)} fn The operation to time. Must not
 *   throw under normal conditions — a throwing suite aborts the whole run
 *   (the harness has no way to report a "half a table" of results and
 *   would rather fail loudly than publish a partial, silently-misleading
 *   one).
 * @param {{warmup?: number, measure?: number}} [options]
 * @returns {Promise<SuiteResult>}
 */
export async function timeSuite(name, fn, { warmup = 10, measure = 60 } = {}) {
  for (let i = 0; i < warmup; i += 1) {
    // eslint-disable-next-line no-await-in-loop
    await fn();
  }

  const samples = new Array(measure);
  for (let i = 0; i < measure; i += 1) {
    const t0 = performance.now();
    // eslint-disable-next-line no-await-in-loop
    await fn();
    samples[i] = performance.now() - t0;
  }

  return { name, ...reduceSamples(samples) };
}

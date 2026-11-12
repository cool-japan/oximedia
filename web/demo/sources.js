// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

/**
 * Input-source management for the OxiScope demo.
 *
 * Three ways in — a dropped/-picked file, the webcam, or a built-in procedural
 * pattern — all converge on a single per-frame callback, `onFrame(source)`,
 * where `source` is a WebCodecs `VideoFrame` when the engine supports it (the
 * brief's `new VideoFrame(video)` path, and the `MediaStreamTrackProcessor`
 * frames straight off a stream) or a raw `<video>` element when it does not.
 *
 * Frame ownership is strict: the driver owns every `VideoFrame` it produces and
 * closes it the instant `onFrame` resolves, so the consumer must never close
 * the source. Rendering is serialised — the next frame is not fetched until the
 * previous `onFrame` promise settles — which keeps the shared grow-once wasm
 * buffers race-free and applies natural backpressure instead of queueing.
 *
 * Nothing here ever uploads a byte: files become `blob:` object URLs, the
 * webcam and the pattern `captureStream` stay in-page. That is the whole point
 * of the "uploaded: 0 bytes" badge.
 *
 * @module sources
 */

import { patternByKey } from './patterns.js';

/**
 * Feature flags the drivers key off, derived once from `detectCapabilities()`.
 * @typedef {Object} SourceCapabilities
 * @property {boolean} videoFrame WebCodecs `VideoFrame` is constructible.
 * @property {boolean} trackProcessor `MediaStreamTrackProcessor` is available.
 * @property {boolean} rvfc `requestVideoFrameCallback` exists on video elements.
 */

/**
 * Drives frames off an `HTMLVideoElement` via `requestVideoFrameCallback`
 * (falling back to `requestAnimationFrame`). When WebCodecs is present each
 * tick wraps the current picture in a fresh `VideoFrame` and closes it after
 * the consumer returns; otherwise the `<video>` element itself is handed over.
 */
class VideoElementDriver {
  /**
   * @param {HTMLVideoElement} video
   * @param {(source: VideoFrame | HTMLVideoElement) => Promise<void>} onFrame
   * @param {SourceCapabilities} caps
   */
  constructor(video, onFrame, caps) {
    /** @private */ this._video = video;
    /** @private */ this._onFrame = onFrame;
    /** @private */ this._caps = caps;
    /** @private */ this._running = false;
    /** @private */ this._rvfcHandle = 0;
    /** @private */ this._rafHandle = 0;
    /** @private @type {(now: number) => void} */
    this._tick = this._tick.bind(this);
  }

  /** Begins the frame loop. */
  start() {
    if (this._running) {
      return;
    }
    this._running = true;
    this._schedule();
  }

  /** Stops the loop; any in-flight `onFrame` is allowed to finish. */
  stop() {
    this._running = false;
    if (this._rvfcHandle && typeof this._video.cancelVideoFrameCallback === 'function') {
      this._video.cancelVideoFrameCallback(this._rvfcHandle);
    }
    if (this._rafHandle) {
      cancelAnimationFrame(this._rafHandle);
    }
    this._rvfcHandle = 0;
    this._rafHandle = 0;
  }

  /** @private Schedules the next frame via rVFC when available, else rAF. */
  _schedule() {
    if (!this._running) {
      return;
    }
    if (this._caps.rvfc && typeof this._video.requestVideoFrameCallback === 'function') {
      this._rvfcHandle = this._video.requestVideoFrameCallback(this._tick);
    } else {
      this._rafHandle = requestAnimationFrame(this._tick);
    }
  }

  /**
   * @private
   * @param {number} now
   */
  async _tick(now) {
    if (!this._running) {
      return;
    }
    const video = this._video;
    if (video.readyState < 2 || video.videoWidth === 0) {
      this._schedule();
      return;
    }
    let frame = null;
    /** @type {VideoFrame | HTMLVideoElement} */
    let source = video;
    if (this._caps.videoFrame) {
      try {
        frame = new VideoFrame(video, { timestamp: Math.round(now * 1000) });
        source = frame;
      } catch (_err) {
        // Element not paintable this tick; fall back to the element itself.
        frame = null;
        source = video;
      }
    }
    try {
      await this._onFrame(source);
    } catch (err) {
      // Surface once, then keep the loop alive so the preview never freezes.
      console.error('OxiScope frame error:', err);
    } finally {
      if (frame) {
        frame.close();
      }
    }
    this._schedule();
  }
}

/**
 * Drives `VideoFrame`s straight off a `MediaStreamTrack` with
 * `MediaStreamTrackProcessor` — no `<video>` element, no compositor round-trip.
 * The preferred path for webcam and pattern-`captureStream` sources when the
 * engine supports it.
 */
class TrackProcessorDriver {
  /**
   * @param {MediaStreamTrack} track
   * @param {(source: VideoFrame) => Promise<void>} onFrame
   */
  constructor(track, onFrame) {
    /** @private */ this._track = track;
    /** @private */ this._onFrame = onFrame;
    /** @private */ this._running = false;
    /** @private @type {ReadableStreamDefaultReader<VideoFrame> | null} */
    this._reader = null;
  }

  /** Begins the read loop. */
  start() {
    if (this._running) {
      return;
    }
    this._running = true;
    const processor = new MediaStreamTrackProcessor({ track: this._track });
    this._reader = processor.readable.getReader();
    this._pump();
  }

  /** Stops the loop and releases the reader. */
  stop() {
    this._running = false;
    const reader = this._reader;
    this._reader = null;
    if (reader) {
      reader.cancel().catch(() => {});
    }
  }

  /** @private Serial read → render → close pump. */
  async _pump() {
    const reader = this._reader;
    if (!reader) {
      return;
    }
    while (this._running) {
      let result;
      try {
        result = await reader.read();
      } catch (_err) {
        break;
      }
      if (result.done) {
        break;
      }
      const frame = result.value;
      if (!this._running) {
        frame.close();
        break;
      }
      try {
        await this._onFrame(frame);
      } catch (err) {
        console.error('OxiScope frame error:', err);
      } finally {
        frame.close();
      }
    }
  }
}

/**
 * Owns the demo's live input: which source is active, its driver, and the
 * teardown of every stream / object URL / animation loop it spins up.
 */
export class InputController {
  /**
   * @param {Object} opts
   * @param {HTMLVideoElement} opts.video Hidden `<video>` sink, reused across sources.
   * @param {HTMLCanvasElement} opts.patternCanvas Hidden canvas the patterns draw onto.
   * @param {(source: VideoFrame | HTMLVideoElement) => Promise<void>} opts.onFrame
   * @param {(info: { kind: string, label: string, width: number, height: number }) => void} [opts.onSourceChange]
   * @param {SourceCapabilities} opts.capabilities
   */
  constructor({ video, patternCanvas, onFrame, onSourceChange, capabilities }) {
    /** @private */ this._video = video;
    /** @private */ this._patternCanvas = patternCanvas;
    /** @private */ this._onFrame = onFrame;
    /** @private */ this._onSourceChange = onSourceChange ?? (() => {});
    /** @private */ this._caps = capabilities;

    /** @private @type {VideoElementDriver | TrackProcessorDriver | null} */
    this._driver = null;
    /** @private @type {MediaStream | null} */
    this._stream = null;
    /** @private @type {string | null} */
    this._objectUrl = null;
    /** @private @type {number} */
    this._rafHandle = 0;
    /** @private @type {number} */
    this._patternStart = 0;
    /** @private @type {import('./patterns.js').PatternDef | null} */
    this._pattern = null;
    /** @private @type {string} */
    this._kind = 'none';

    // Never leave a wasted upload figure: this is always zero and stays zero.
    /** @private */ this._uploadedBytes = 0;
  }

  /** Bytes this demo has uploaded to any server — always zero, by design. */
  get uploadedBytes() {
    return this._uploadedBytes;
  }

  /** The active source kind: `'file' | 'webcam' | 'pattern' | 'none'`. */
  get kind() {
    return this._kind;
  }

  /**
   * Plays a user-supplied video file entirely in-page (`blob:` URL). Never
   * leaves the browser.
   * @param {File} file
   * @returns {Promise<void>}
   */
  async useFile(file) {
    this.stop();
    const url = URL.createObjectURL(file);
    this._objectUrl = url;
    const video = this._video;
    video.srcObject = null;
    video.src = url;
    video.loop = true;
    video.muted = true;
    video.playsInline = true;
    await this._playAndSettle(video);
    this._kind = 'file';
    this._startVideoDriver();
    this._announce(file.name || 'file');
  }

  /**
   * Opens the webcam (audio-free) and drives its frames. Rejects with a
   * readable message if permission is denied or no camera exists.
   * @returns {Promise<void>}
   */
  async useWebcam() {
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== 'function') {
      throw new Error('This browser exposes no camera API (getUserMedia).');
    }
    this.stop();
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: { ideal: 1280 }, height: { ideal: 720 } },
      audio: false,
    });
    this._stream = stream;
    this._kind = 'webcam';
    await this._driveStream(stream, 'Webcam');
  }

  /**
   * Starts a built-in procedural pattern: draws it onto the hidden canvas each
   * animation frame and drives its `captureStream`, so it shares the exact
   * webcam code path (and the `MediaStreamTrackProcessor` fast lane).
   * @param {string} key Pattern key from {@link module:patterns.PATTERNS}.
   */
  usePattern(key) {
    this.stop();
    const pattern = patternByKey(key);
    this._pattern = pattern;
    this._kind = 'pattern';

    const canvas = this._patternCanvas;
    const ctx = canvas.getContext('2d', { willReadFrequently: false });
    if (!ctx) {
      throw new Error('Failed to acquire a 2D context for the pattern canvas.');
    }
    this._patternStart = performance.now();
    const drawOnce = () => {
      const t = performance.now() - this._patternStart;
      pattern.draw(ctx, canvas.width, canvas.height, t);
    };
    // Prime a first frame so captureStream has content immediately.
    drawOnce();

    const stream = canvas.captureStream(30);
    this._stream = stream;

    const loop = () => {
      if (this._kind !== 'pattern') {
        return;
      }
      drawOnce();
      this._rafHandle = requestAnimationFrame(loop);
    };
    this._rafHandle = requestAnimationFrame(loop);

    // Drive the captured stream exactly like the webcam.
    this._driveStream(stream, pattern.label).catch((err) => {
      console.error('OxiScope pattern driver error:', err);
    });
  }

  /**
   * Tears down whatever source is active: stops the driver, cancels the pattern
   * loop, stops every media track and revokes any object URL.
   */
  stop() {
    if (this._driver) {
      this._driver.stop();
      this._driver = null;
    }
    if (this._rafHandle) {
      cancelAnimationFrame(this._rafHandle);
      this._rafHandle = 0;
    }
    if (this._stream) {
      for (const track of this._stream.getTracks()) {
        track.stop();
      }
      this._stream = null;
    }
    const video = this._video;
    if (video.srcObject) {
      video.srcObject = null;
    }
    if (video.src) {
      video.removeAttribute('src');
      video.load();
    }
    if (this._objectUrl) {
      URL.revokeObjectURL(this._objectUrl);
      this._objectUrl = null;
    }
    this._pattern = null;
    this._kind = 'none';
  }

  /**
   * Drives a `MediaStream`: the `MediaStreamTrackProcessor` fast path when
   * available, else a `<video srcObject>` + rVFC loop.
   * @private
   * @param {MediaStream} stream
   * @param {string} label
   * @returns {Promise<void>}
   */
  async _driveStream(stream, label) {
    const track = stream.getVideoTracks()[0];
    if (!track) {
      throw new Error('The media stream has no video track.');
    }
    if (this._caps.trackProcessor && this._caps.videoFrame) {
      try {
        this._driver = new TrackProcessorDriver(track, this._onFrame);
        this._driver.start();
        const settings = track.getSettings();
        this._announce(label, settings.width, settings.height);
        return;
      } catch (_err) {
        // Fall through to the element path if the processor blows up.
        this._driver = null;
      }
    }
    const video = this._video;
    video.src = '';
    video.srcObject = stream;
    video.muted = true;
    video.playsInline = true;
    await this._playAndSettle(video);
    this._startVideoDriver();
    this._announce(label);
  }

  /** @private Starts the shared `<video>`-element driver. */
  _startVideoDriver() {
    this._driver = new VideoElementDriver(this._video, this._onFrame, this._caps);
    this._driver.start();
  }

  /**
   * @private
   * @param {HTMLVideoElement} video
   * @returns {Promise<void>}
   */
  async _playAndSettle(video) {
    try {
      await video.play();
    } catch (_err) {
      // Autoplay can still be gated; a muted element usually clears it, but if
      // not the rVFC/rAF loop simply idles until data arrives.
    }
    if (video.readyState < 2) {
      await new Promise((resolve) => {
        const done = () => {
          video.removeEventListener('loadeddata', done);
          resolve();
        };
        video.addEventListener('loadeddata', done, { once: true });
        // Guard against streams that never fire the event in headless engines.
        setTimeout(resolve, 1500);
      });
    }
  }

  /**
   * @private
   * @param {string} label
   * @param {number} [width]
   * @param {number} [height]
   */
  _announce(label, width, height) {
    const w = width ?? this._video.videoWidth ?? this._patternCanvas.width;
    const h = height ?? this._video.videoHeight ?? this._patternCanvas.height;
    this._onSourceChange({ kind: this._kind, label, width: w || 0, height: h || 0 });
  }
}

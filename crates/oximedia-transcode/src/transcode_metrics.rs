//! Real-time transcoding metrics collection and reporting.
//!
//! Provides atomic counters, per-frame metrics, rolling-window statistics,
//! CSV export, and Prometheus text-format export.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ─── FrameType ────────────────────────────────────────────────────────────────

/// Video frame type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Intra-coded frame — full reference, largest.
    I,
    /// Predicted frame — depends on one past reference.
    P,
    /// Bi-directionally predicted frame — smallest, best compression.
    B,
}

impl FrameType {
    /// Single-character label for CSV/Prometheus output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::I => "I",
            Self::P => "P",
            Self::B => "B",
        }
    }
}

// ─── FrameMetric ─────────────────────────────────────────────────────────────

/// Per-frame encoding statistics.
#[derive(Debug, Clone)]
pub struct FrameMetric {
    /// 0-based frame number within the stream.
    pub frame_number: u64,
    /// Time spent encoding this frame, in microseconds.
    pub encode_time_us: u32,
    /// PSNR for this frame (dB).
    pub psnr: f32,
    /// Frame coding type.
    pub frame_type: FrameType,
    /// Compressed frame size in bits.
    pub output_bits: u32,
}

impl FrameMetric {
    /// Constructs a new `FrameMetric`.
    #[must_use]
    pub fn new(
        frame_number: u64,
        encode_time_us: u32,
        psnr: f32,
        frame_type: FrameType,
        output_bits: u32,
    ) -> Self {
        Self {
            frame_number,
            encode_time_us,
            psnr,
            frame_type,
            output_bits,
        }
    }

    /// Returns the output size in bytes (rounded up from bits).
    #[must_use]
    pub fn output_bytes(&self) -> u32 {
        (self.output_bits + 7) / 8
    }

    /// Returns the instantaneous bitrate given a frame rate.
    #[must_use]
    pub fn instant_bitrate_kbps(&self, fps: f32) -> f32 {
        if fps <= 0.0 {
            return 0.0;
        }
        self.output_bits as f32 * fps / 1000.0
    }
}

// ─── TranscodeMetrics ────────────────────────────────────────────────────────

/// Thread-safe atomic counters for a transcoding session.
#[derive(Debug)]
pub struct TranscodeMetrics {
    /// Total frames successfully encoded.
    pub frames_encoded: AtomicU64,
    /// Frames that were dropped (not encoded).
    pub frames_dropped: AtomicU64,
    /// Total compressed bytes written to output.
    pub bytes_output: AtomicU64,
    /// Number of encoding errors encountered.
    pub encoding_errors: AtomicU64,
}

impl TranscodeMetrics {
    /// Creates a new zeroed metrics structure.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frames_encoded: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            bytes_output: AtomicU64::new(0),
            encoding_errors: AtomicU64::new(0),
        }
    }

    /// Atomically increments the encoded frame counter.
    pub fn inc_frames_encoded(&self, delta: u64) {
        self.frames_encoded.fetch_add(delta, Ordering::Relaxed);
    }

    /// Atomically increments the dropped frame counter.
    pub fn inc_frames_dropped(&self, delta: u64) {
        self.frames_dropped.fetch_add(delta, Ordering::Relaxed);
    }

    /// Atomically adds to the bytes output counter.
    pub fn add_bytes_output(&self, bytes: u64) {
        self.bytes_output.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Atomically increments the error counter.
    pub fn inc_errors(&self, delta: u64) {
        self.encoding_errors.fetch_add(delta, Ordering::Relaxed);
    }

    /// Snapshot of current values (non-atomic-consistent, for reporting).
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            frames_encoded: self.frames_encoded.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            bytes_output: self.bytes_output.load(Ordering::Relaxed),
            encoding_errors: self.encoding_errors.load(Ordering::Relaxed),
        }
    }
}

impl Default for TranscodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A non-atomic snapshot of `TranscodeMetrics` at a point in time.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Frames encoded.
    pub frames_encoded: u64,
    /// Frames dropped.
    pub frames_dropped: u64,
    /// Bytes written to output.
    pub bytes_output: u64,
    /// Encoding errors.
    pub encoding_errors: u64,
}

// ─── EncodingRate ─────────────────────────────────────────────────────────────

/// Instantaneous or average encoding throughput.
#[derive(Debug, Clone)]
pub struct EncodingRate {
    /// Encoded frames per second.
    pub fps: f32,
    /// Ratio of encoded content time to wall-clock time.
    /// 1.0 = real-time; > 1.0 = faster than real-time.
    pub real_time_factor: f32,
    /// Current output bitrate in kbps.
    pub instant_bitrate_kbps: u32,
}

impl EncodingRate {
    /// Returns `true` if the encoder is keeping up with real-time.
    #[must_use]
    pub fn is_realtime(&self) -> bool {
        self.real_time_factor >= 1.0
    }
}

// ─── QualityMetrics ──────────────────────────────────────────────────────────

/// Aggregate quality metrics for a session or window.
#[derive(Debug, Clone)]
pub struct QualityMetrics {
    /// Average PSNR in dB.
    pub avg_psnr: f32,
    /// Average SSIM (structural similarity, \[0.0, 1.0\]).
    pub avg_ssim: f32,
    /// Average VMAF score (\[0.0, 100.0\]).
    pub avg_vmaf: f32,
}

impl QualityMetrics {
    /// Creates a new zero-initialised quality metrics.
    #[must_use]
    pub fn zero() -> Self {
        Self {
            avg_psnr: 0.0,
            avg_ssim: 0.0,
            avg_vmaf: 0.0,
        }
    }
}

// ─── SessionMetrics ──────────────────────────────────────────────────────────

/// Full metrics for a single transcoding session.
#[derive(Debug)]
pub struct SessionMetrics {
    /// Unique session identifier.
    pub session_id: u64,
    /// Wall-clock start time (milliseconds since Unix epoch).
    pub start_time_ms: i64,
    /// Current encoding throughput.
    pub encoding_rate: EncodingRate,
    /// Current quality measurements.
    pub quality: QualityMetrics,
    /// Ring buffer of the last 100 per-frame metrics.
    pub per_frame_metrics: VecDeque<FrameMetric>,
}

/// Maximum number of recent per-frame metrics retained.
const PER_FRAME_WINDOW: usize = 100;

impl SessionMetrics {
    /// Creates a new session metrics record.
    #[must_use]
    pub fn new(session_id: u64, start_time_ms: i64) -> Self {
        Self {
            session_id,
            start_time_ms,
            encoding_rate: EncodingRate {
                fps: 0.0,
                real_time_factor: 0.0,
                instant_bitrate_kbps: 0,
            },
            quality: QualityMetrics::zero(),
            per_frame_metrics: VecDeque::with_capacity(PER_FRAME_WINDOW),
        }
    }

    /// Pushes a frame metric onto the ring buffer; evicts oldest if full.
    pub fn push_frame(&mut self, metric: FrameMetric) {
        if self.per_frame_metrics.len() >= PER_FRAME_WINDOW {
            self.per_frame_metrics.pop_front();
        }
        self.per_frame_metrics.push_back(metric);
    }

    /// Returns the elapsed time since `start_time_ms` in seconds.
    #[must_use]
    pub fn elapsed_secs(&self, current_time_ms: i64) -> f64 {
        let delta = current_time_ms.saturating_sub(self.start_time_ms);
        delta as f64 / 1000.0
    }
}

// ─── MetricAggregator ────────────────────────────────────────────────────────

/// Aggregates per-frame metrics and computes derived statistics.
#[derive(Debug)]
pub struct MetricAggregator {
    session_metrics: SessionMetrics,
    shared_counters: Arc<TranscodeMetrics>,
    /// Source frame rate (used for real-time-factor calculation).
    source_fps: f32,
}

impl MetricAggregator {
    /// Creates a new aggregator for `session_id` starting at `start_time_ms`.
    #[must_use]
    pub fn new(session_id: u64, start_time_ms: i64, source_fps: f32) -> Self {
        Self {
            session_metrics: SessionMetrics::new(session_id, start_time_ms),
            shared_counters: Arc::new(TranscodeMetrics::new()),
            source_fps,
        }
    }

    /// Returns a shared reference to the atomic counters.
    #[must_use]
    pub fn counters(&self) -> Arc<TranscodeMetrics> {
        Arc::clone(&self.shared_counters)
    }

    /// Records a new frame metric, updates ring buffer and atomic counters.
    pub fn update_frame(&mut self, metric: FrameMetric) {
        let byte_count = metric.output_bytes() as u64;
        self.shared_counters.inc_frames_encoded(1);
        self.shared_counters.add_bytes_output(byte_count);
        self.session_metrics.push_frame(metric);
    }

    /// Computes current `EncodingRate` given the current wall-clock time.
    #[must_use]
    pub fn compute_rate(&self, current_time_ms: i64) -> EncodingRate {
        let elapsed = self.session_metrics.elapsed_secs(current_time_ms);
        if elapsed <= 0.0 {
            return EncodingRate {
                fps: 0.0,
                real_time_factor: 0.0,
                instant_bitrate_kbps: 0,
            };
        }

        let frames_encoded = self.shared_counters.frames_encoded.load(Ordering::Relaxed);
        let bytes_output = self.shared_counters.bytes_output.load(Ordering::Relaxed);

        let fps = frames_encoded as f32 / elapsed as f32;
        let rtf = if self.source_fps > 0.0 {
            fps / self.source_fps
        } else {
            0.0
        };

        let bitrate_kbps = if elapsed > 0.0 {
            (bytes_output as f64 * 8.0 / elapsed / 1000.0) as u32
        } else {
            0
        };

        EncodingRate {
            fps,
            real_time_factor: rtf,
            instant_bitrate_kbps: bitrate_kbps,
        }
    }

    /// Computes the rolling average PSNR over the last `window` frame metrics.
    ///
    /// Clamps `window` to the available ring-buffer size.
    #[must_use]
    pub fn rolling_avg_psnr(&self, window: usize) -> f32 {
        let buf = &self.session_metrics.per_frame_metrics;
        if buf.is_empty() {
            return 0.0;
        }
        let effective_window = window.min(buf.len());
        let start = buf.len() - effective_window;
        let sum: f32 = buf.iter().skip(start).map(|m| m.psnr).sum();
        sum / effective_window as f32
    }

    /// Exports all per-frame metrics in CSV format.
    ///
    /// Columns: `frame_number,frame_type,encode_time_us,output_bits,psnr`
    #[must_use]
    pub fn export_csv(&self) -> String {
        let mut out = String::from("frame_number,frame_type,encode_time_us,output_bits,psnr\n");
        for m in &self.session_metrics.per_frame_metrics {
            out.push_str(&format!(
                "{},{},{},{},{:.4}\n",
                m.frame_number,
                m.frame_type.label(),
                m.encode_time_us,
                m.output_bits,
                m.psnr,
            ));
        }
        out
    }

    /// Exports current session metrics in Prometheus text exposition format.
    ///
    /// All metric names are prefixed `oximedia_transcode_`.
    #[must_use]
    pub fn to_prometheus(&self, session_id: u64) -> String {
        let snapshot = self.shared_counters.snapshot();
        let rate = self.compute_rate(self.session_metrics.start_time_ms); // rate from session start

        let mut buf = String::new();

        buf.push_str(
            "# HELP oximedia_transcode_frames_encoded Total frames successfully encoded\n",
        );
        buf.push_str("# TYPE oximedia_transcode_frames_encoded counter\n");
        buf.push_str(&format!(
            "oximedia_transcode_frames_encoded{{session=\"{session_id}\"}} {}\n",
            snapshot.frames_encoded
        ));

        buf.push_str("# HELP oximedia_transcode_frames_dropped Total frames dropped\n");
        buf.push_str("# TYPE oximedia_transcode_frames_dropped counter\n");
        buf.push_str(&format!(
            "oximedia_transcode_frames_dropped{{session=\"{session_id}\"}} {}\n",
            snapshot.frames_dropped
        ));

        buf.push_str("# HELP oximedia_transcode_bytes_output Total compressed bytes written\n");
        buf.push_str("# TYPE oximedia_transcode_bytes_output counter\n");
        buf.push_str(&format!(
            "oximedia_transcode_bytes_output{{session=\"{session_id}\"}} {}\n",
            snapshot.bytes_output
        ));

        buf.push_str("# HELP oximedia_transcode_encoding_errors Total encoding errors\n");
        buf.push_str("# TYPE oximedia_transcode_encoding_errors counter\n");
        buf.push_str(&format!(
            "oximedia_transcode_encoding_errors{{session=\"{session_id}\"}} {}\n",
            snapshot.encoding_errors
        ));

        buf.push_str("# HELP oximedia_transcode_fps Current encoding frames per second\n");
        buf.push_str("# TYPE oximedia_transcode_fps gauge\n");
        buf.push_str(&format!(
            "oximedia_transcode_fps{{session=\"{session_id}\"}} {:.3}\n",
            rate.fps
        ));

        buf.push_str(
            "# HELP oximedia_transcode_real_time_factor Encoding speed relative to real-time\n",
        );
        buf.push_str("# TYPE oximedia_transcode_real_time_factor gauge\n");
        buf.push_str(&format!(
            "oximedia_transcode_real_time_factor{{session=\"{session_id}\"}} {:.4}\n",
            rate.real_time_factor
        ));

        buf.push_str(
            "# HELP oximedia_transcode_bitrate_kbps Instantaneous output bitrate in kbps\n",
        );
        buf.push_str("# TYPE oximedia_transcode_bitrate_kbps gauge\n");
        buf.push_str(&format!(
            "oximedia_transcode_bitrate_kbps{{session=\"{session_id}\"}} {}\n",
            rate.instant_bitrate_kbps
        ));

        let avg_psnr = self.rolling_avg_psnr(PER_FRAME_WINDOW);
        buf.push_str(
            "# HELP oximedia_transcode_avg_psnr Rolling average PSNR over last 100 frames\n",
        );
        buf.push_str("# TYPE oximedia_transcode_avg_psnr gauge\n");
        buf.push_str(&format!(
            "oximedia_transcode_avg_psnr{{session=\"{session_id}\"}} {:.4}\n",
            avg_psnr
        ));

        buf
    }

    /// Returns a reference to the session metrics.
    #[must_use]
    pub fn session(&self) -> &SessionMetrics {
        &self.session_metrics
    }
}

// ─── Legacy API compatibility types ──────────────────────────────────────────
// Kept for modules that import the old types; new code should use the above.

/// Per-frame encoding metric captured during transcoding (legacy).
#[derive(Debug, Clone)]
pub struct LegacyFrameMetric {
    /// Frame index (0-based).
    pub frame_index: u64,
    /// Encode time for this frame in microseconds.
    pub encode_us: u64,
    /// Compressed size of this frame in bytes.
    pub compressed_bytes: u64,
    /// PSNR value for this frame (dB), if computed.
    pub psnr_db: Option<f64>,
}

impl LegacyFrameMetric {
    /// Creates a new frame metric.
    #[must_use]
    pub fn new(frame_index: u64, encode_us: u64, compressed_bytes: u64) -> Self {
        Self {
            frame_index,
            encode_us,
            compressed_bytes,
            psnr_db: None,
        }
    }

    /// Attaches a PSNR measurement.
    #[must_use]
    pub fn with_psnr(mut self, psnr_db: f64) -> Self {
        self.psnr_db = Some(psnr_db);
        self
    }

    /// Returns the instantaneous bitrate for this frame given a frame rate.
    #[must_use]
    pub fn instantaneous_bitrate_bps(&self, fps: f64) -> f64 {
        self.compressed_bytes as f64 * 8.0 * fps
    }
}

/// Summary statistics over a collection of frame metrics (legacy).
#[derive(Debug, Clone)]
pub struct MetricsSummary {
    /// Total number of frames.
    pub frame_count: u64,
    /// Mean encode time per frame in microseconds.
    pub mean_encode_us: f64,
    /// Peak encode time in microseconds.
    pub peak_encode_us: u64,
    /// Total compressed bytes.
    pub total_bytes: u64,
    /// Mean PSNR in dB (None if not measured).
    pub mean_psnr_db: Option<f64>,
    /// Minimum PSNR in dB (None if not measured).
    pub min_psnr_db: Option<f64>,
}

impl MetricsSummary {
    /// Returns the mean bitrate in bits-per-second given input fps.
    #[must_use]
    pub fn mean_bitrate_bps(&self, fps: f64) -> f64 {
        if self.frame_count == 0 || fps <= 0.0 {
            return 0.0;
        }
        let total_bits = self.total_bytes as f64 * 8.0;
        let duration_secs = self.frame_count as f64 / fps;
        total_bits / duration_secs
    }

    /// Returns the encode throughput in frames per second.
    #[must_use]
    pub fn encode_fps(&self) -> f64 {
        if self.mean_encode_us <= 0.0 {
            return 0.0;
        }
        1_000_000.0 / self.mean_encode_us
    }
}

/// Collects frame-level metrics during a transcode session (legacy).
#[derive(Debug, Default)]
pub struct TranscodeMetricsCollector {
    metrics: Vec<LegacyFrameMetric>,
}

impl TranscodeMetricsCollector {
    /// Creates a new, empty collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a collector with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            metrics: Vec::with_capacity(cap),
        }
    }

    /// Records a frame metric.
    pub fn record(&mut self, metric: LegacyFrameMetric) {
        self.metrics.push(metric);
    }

    /// Returns the number of recorded frame metrics.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.metrics.len()
    }

    /// Returns `true` if no metrics have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.metrics.is_empty()
    }

    /// Computes and returns a summary over all recorded metrics.
    pub fn summarise(&self) -> MetricsSummary {
        let count = self.metrics.len() as u64;
        if count == 0 {
            return MetricsSummary {
                frame_count: 0,
                mean_encode_us: 0.0,
                peak_encode_us: 0,
                total_bytes: 0,
                mean_psnr_db: None,
                min_psnr_db: None,
            };
        }

        let total_encode_us: u64 = self.metrics.iter().map(|m| m.encode_us).sum();
        let peak_encode_us = self.metrics.iter().map(|m| m.encode_us).max().unwrap_or(0);
        let total_bytes: u64 = self.metrics.iter().map(|m| m.compressed_bytes).sum();

        let psnr_values: Vec<f64> = self.metrics.iter().filter_map(|m| m.psnr_db).collect();

        let mean_psnr_db = if psnr_values.is_empty() {
            None
        } else {
            Some(psnr_values.iter().sum::<f64>() / psnr_values.len() as f64)
        };

        let min_psnr_db = psnr_values.iter().copied().reduce(f64::min);

        MetricsSummary {
            frame_count: count,
            mean_encode_us: total_encode_us as f64 / count as f64,
            peak_encode_us,
            total_bytes,
            mean_psnr_db,
            min_psnr_db,
        }
    }

    /// Returns the worst (lowest) PSNR frame, if PSNR data is available.
    #[must_use]
    pub fn worst_psnr_frame(&self) -> Option<&LegacyFrameMetric> {
        self.metrics
            .iter()
            .filter(|m| m.psnr_db.is_some())
            .min_by(|a, b| {
                // `filter` above guarantees `Some` for every item reaching
                // this comparator; the fallback (never actually used) ranks
                // a missing value as "not the worst" rather than panicking.
                let pa = a.psnr_db.unwrap_or(f64::INFINITY);
                let pb = b.psnr_db.unwrap_or(f64::INFINITY);
                pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Returns the slowest (highest encode time) frame metric.
    #[must_use]
    pub fn slowest_frame(&self) -> Option<&LegacyFrameMetric> {
        self.metrics.iter().max_by_key(|m| m.encode_us)
    }

    /// Clears all recorded metrics.
    pub fn clear(&mut self) {
        self.metrics.clear();
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(n: u64, enc_us: u32, psnr: f32, ftype: FrameType, bits: u32) -> FrameMetric {
        FrameMetric::new(n, enc_us, psnr, ftype, bits)
    }

    // ── FrameType ─────────────────────────────────────────────────────────────

    #[test]
    fn test_frame_type_labels() {
        assert_eq!(FrameType::I.label(), "I");
        assert_eq!(FrameType::P.label(), "P");
        assert_eq!(FrameType::B.label(), "B");
    }

    #[test]
    fn test_frame_metric_output_bytes() {
        let m = make_frame(0, 1000, 42.0, FrameType::I, 800); // 800 bits = 100 bytes
        assert_eq!(m.output_bytes(), 100);
    }

    #[test]
    fn test_frame_metric_output_bytes_partial() {
        // 801 bits → 101 bytes (ceiling)
        let m = make_frame(0, 1000, 42.0, FrameType::P, 801);
        assert_eq!(m.output_bytes(), 101);
    }

    #[test]
    fn test_frame_metric_instant_bitrate() {
        // 8000 bits at 30 fps = 240 kbps
        let m = make_frame(0, 5000, 40.0, FrameType::I, 8000);
        let kbps = m.instant_bitrate_kbps(30.0);
        assert!((kbps - 240.0).abs() < 0.01, "kbps={kbps}");
    }

    #[test]
    fn test_frame_metric_zero_fps() {
        let m = make_frame(0, 5000, 40.0, FrameType::I, 8000);
        assert_eq!(m.instant_bitrate_kbps(0.0), 0.0);
    }

    // ── TranscodeMetrics (atomic) ──────────────────────────────────────────────

    #[test]
    fn test_atomic_counters_start_at_zero() {
        let m = TranscodeMetrics::new();
        let s = m.snapshot();
        assert_eq!(s.frames_encoded, 0);
        assert_eq!(s.frames_dropped, 0);
        assert_eq!(s.bytes_output, 0);
        assert_eq!(s.encoding_errors, 0);
    }

    #[test]
    fn test_atomic_counters_increment() {
        let m = TranscodeMetrics::new();
        m.inc_frames_encoded(5);
        m.inc_frames_dropped(2);
        m.add_bytes_output(1024);
        m.inc_errors(1);
        let s = m.snapshot();
        assert_eq!(s.frames_encoded, 5);
        assert_eq!(s.frames_dropped, 2);
        assert_eq!(s.bytes_output, 1024);
        assert_eq!(s.encoding_errors, 1);
    }

    // ── MetricAggregator ─────────────────────────────────────────────────────

    #[test]
    fn test_aggregator_update_frame_increments_counters() {
        let mut agg = MetricAggregator::new(1, 0, 30.0);
        agg.update_frame(make_frame(0, 5000, 42.0, FrameType::I, 8000));
        agg.update_frame(make_frame(1, 4000, 41.0, FrameType::P, 4000));
        let s = agg.counters().snapshot();
        assert_eq!(s.frames_encoded, 2);
        assert_eq!(s.bytes_output, 1000 + 500); // 8000/8=1000, 4000/8=500
    }

    #[test]
    fn test_aggregator_rolling_avg_psnr_empty() {
        let agg = MetricAggregator::new(1, 0, 30.0);
        assert_eq!(agg.rolling_avg_psnr(10), 0.0);
    }

    #[test]
    fn test_aggregator_rolling_avg_psnr_basic() {
        let mut agg = MetricAggregator::new(1, 0, 30.0);
        agg.update_frame(make_frame(0, 1000, 40.0, FrameType::I, 8000));
        agg.update_frame(make_frame(1, 1000, 44.0, FrameType::P, 4000));
        let avg = agg.rolling_avg_psnr(2);
        assert!((avg - 42.0).abs() < 0.01, "avg={avg}");
    }

    #[test]
    fn test_aggregator_rolling_avg_psnr_window_clamp() {
        let mut agg = MetricAggregator::new(1, 0, 30.0);
        agg.update_frame(make_frame(0, 1000, 50.0, FrameType::I, 8000));
        // request window=100 but only 1 frame present
        let avg = agg.rolling_avg_psnr(100);
        assert!((avg - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_aggregator_ring_buffer_eviction() {
        let mut agg = MetricAggregator::new(1, 0, 30.0);
        for i in 0..=100_u64 {
            agg.update_frame(make_frame(i, 1000, i as f32, FrameType::P, 1000));
        }
        // Ring buffer should hold at most 100 entries
        assert_eq!(agg.session().per_frame_metrics.len(), 100);
    }

    #[test]
    fn test_aggregator_compute_rate_zero_elapsed() {
        let agg = MetricAggregator::new(1, 1000, 30.0);
        let rate = agg.compute_rate(1000); // same time → 0 elapsed
        assert_eq!(rate.fps, 0.0);
    }

    #[test]
    fn test_aggregator_compute_rate_basic() {
        let mut agg = MetricAggregator::new(1, 0, 30.0);
        // Encode 30 frames
        for i in 0..30 {
            agg.update_frame(make_frame(i, 1000, 42.0, FrameType::P, 8000));
        }
        // After 1 second
        let rate = agg.compute_rate(1000);
        assert!((rate.fps - 30.0).abs() < 0.01, "fps={}", rate.fps);
        assert!((rate.real_time_factor - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_export_csv_header() {
        let agg = MetricAggregator::new(1, 0, 30.0);
        let csv = agg.export_csv();
        assert!(csv.starts_with("frame_number,frame_type,encode_time_us,output_bits,psnr"));
    }

    #[test]
    fn test_export_csv_rows() {
        let mut agg = MetricAggregator::new(1, 0, 30.0);
        agg.update_frame(make_frame(0, 5000, 42.5, FrameType::I, 8000));
        agg.update_frame(make_frame(1, 3000, 40.0, FrameType::B, 2000));
        let csv = agg.export_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 data rows
        assert!(lines[1].starts_with("0,I,"));
        assert!(lines[2].starts_with("1,B,"));
    }

    #[test]
    fn test_prometheus_export_contains_required_metrics() {
        let mut agg = MetricAggregator::new(42, 0, 30.0);
        agg.update_frame(make_frame(0, 5000, 42.0, FrameType::I, 8000));
        let prom = agg.to_prometheus(42);
        assert!(prom.contains("oximedia_transcode_frames_encoded"));
        assert!(prom.contains("oximedia_transcode_frames_dropped"));
        assert!(prom.contains("oximedia_transcode_bytes_output"));
        assert!(prom.contains("oximedia_transcode_encoding_errors"));
        assert!(prom.contains("oximedia_transcode_fps"));
        assert!(prom.contains("oximedia_transcode_real_time_factor"));
        assert!(prom.contains("oximedia_transcode_bitrate_kbps"));
        assert!(prom.contains("oximedia_transcode_avg_psnr"));
    }

    #[test]
    fn test_prometheus_export_session_label() {
        let agg = MetricAggregator::new(99, 0, 30.0);
        let prom = agg.to_prometheus(99);
        assert!(prom.contains("session=\"99\""));
    }

    #[test]
    fn test_encoding_rate_is_realtime() {
        let fast = EncodingRate {
            fps: 60.0,
            real_time_factor: 2.0,
            instant_bitrate_kbps: 5000,
        };
        let slow = EncodingRate {
            fps: 10.0,
            real_time_factor: 0.5,
            instant_bitrate_kbps: 1000,
        };
        assert!(fast.is_realtime());
        assert!(!slow.is_realtime());
    }

    #[test]
    fn test_quality_metrics_zero() {
        let q = QualityMetrics::zero();
        assert_eq!(q.avg_psnr, 0.0);
        assert_eq!(q.avg_ssim, 0.0);
        assert_eq!(q.avg_vmaf, 0.0);
    }

    // ── Legacy API ────────────────────────────────────────────────────────────

    #[test]
    fn test_legacy_collector_record_and_summarise() {
        let mut c = TranscodeMetricsCollector::new();
        c.record(LegacyFrameMetric::new(0, 1000, 400));
        c.record(LegacyFrameMetric::new(1, 3000, 600));
        let s = c.summarise();
        assert_eq!(s.frame_count, 2);
        assert_eq!(s.total_bytes, 1000);
        assert!((s.mean_encode_us - 2000.0).abs() < 1e-6);
    }

    #[test]
    fn test_legacy_worst_psnr() {
        let mut c = TranscodeMetricsCollector::new();
        c.record(LegacyFrameMetric::new(0, 100, 100).with_psnr(45.0));
        c.record(LegacyFrameMetric::new(1, 100, 100).with_psnr(35.0));
        let worst = c.worst_psnr_frame().expect("should exist");
        assert_eq!(worst.frame_index, 1);
    }

    #[test]
    fn test_legacy_clear() {
        let mut c = TranscodeMetricsCollector::new();
        c.record(LegacyFrameMetric::new(0, 100, 100));
        c.clear();
        assert!(c.is_empty());
    }
}

//! Transcoding engine for real-time stream processing.

use crate::error::{ServerError, ServerResult};
use crate::ingest_depacketizer::{sniff_ingest_codec, IngestCodec};
use crate::transcode::AbrLadder;
use oximedia_net::rtmp::MediaPacket;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

/// Transcode job state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscodeJobState {
    /// Initializing.
    Initializing,
    /// Running.
    Running,
    /// Paused.
    Paused,
    /// Completed.
    Completed,
    /// Failed.
    Failed,
}

/// Transcode job.
pub struct TranscodeJob {
    /// Job ID.
    pub id: Uuid,

    /// Stream key.
    pub stream_key: String,

    /// ABR ladder.
    pub ladder: AbrLadder,

    /// Job state.
    pub state: RwLock<TranscodeJobState>,

    /// Input packet sender.
    pub input_tx: mpsc::UnboundedSender<MediaPacket>,

    /// Output packet receivers (one per quality level).
    pub output_rxs: HashMap<String, mpsc::UnboundedReceiver<MediaPacket>>,

    /// Frames processed.
    pub frames_processed: RwLock<u64>,

    /// Start time.
    pub start_time: std::time::Instant,
}

impl TranscodeJob {
    /// Creates a new transcode job.
    pub fn new(stream_key: impl Into<String>, ladder: AbrLadder) -> Self {
        let (input_tx, _input_rx) = mpsc::unbounded_channel();
        let output_rxs = HashMap::new();

        Self {
            id: Uuid::new_v4(),
            stream_key: stream_key.into(),
            ladder,
            state: RwLock::new(TranscodeJobState::Initializing),
            input_tx,
            output_rxs,
            frames_processed: RwLock::new(0),
            start_time: std::time::Instant::now(),
        }
    }

    /// Gets the current state.
    #[must_use]
    pub fn state(&self) -> TranscodeJobState {
        *self.state.read()
    }

    /// Sets the state.
    pub fn set_state(&self, state: TranscodeJobState) {
        *self.state.write() = state;
    }

    /// Gets frames processed.
    #[must_use]
    pub fn frames_processed(&self) -> u64 {
        *self.frames_processed.read()
    }

    /// Increments frames processed.
    pub fn increment_frames(&self) {
        *self.frames_processed.write() += 1;
    }

    /// Gets processing duration.
    #[must_use]
    pub fn duration(&self) -> std::time::Duration {
        std::time::Instant::now().duration_since(self.start_time)
    }
}

/// Transcoding engine.
#[allow(dead_code)]
pub struct TranscodeEngine {
    /// Active jobs.
    jobs: Arc<RwLock<HashMap<String, Arc<TranscodeJob>>>>,

    /// Default ABR ladder.
    default_ladder: AbrLadder,

    /// Worker pool size.
    workers: usize,
}

impl TranscodeEngine {
    /// Creates a new transcoding engine.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails.
    pub async fn new() -> ServerResult<Self> {
        let default_ladder = AbrLadder::standard();
        let workers = num_cpus::get();

        Ok(Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            default_ladder,
            workers,
        })
    }

    /// Creates a new transcode job.
    pub fn create_job(&self, stream_key: impl Into<String>) -> Arc<TranscodeJob> {
        self.create_job_with_ladder(stream_key, self.default_ladder.clone())
    }

    /// Creates a new transcode job with custom ladder.
    pub fn create_job_with_ladder(
        &self,
        stream_key: impl Into<String>,
        ladder: AbrLadder,
    ) -> Arc<TranscodeJob> {
        let stream_key = stream_key.into();
        let job = Arc::new(TranscodeJob::new(stream_key.clone(), ladder));

        let mut jobs = self.jobs.write();
        jobs.insert(stream_key.clone(), Arc::clone(&job));

        info!("Created transcode job for stream: {}", stream_key);
        job
    }

    /// Gets a transcode job.
    #[must_use]
    pub fn get_job(&self, stream_key: &str) -> Option<Arc<TranscodeJob>> {
        let jobs = self.jobs.read();
        jobs.get(stream_key).cloned()
    }

    /// Removes a transcode job.
    pub fn remove_job(&self, stream_key: &str) {
        let mut jobs = self.jobs.write();
        jobs.remove(stream_key);
        info!("Removed transcode job for stream: {}", stream_key);
    }

    /// Lists all jobs.
    #[must_use]
    pub fn list_jobs(&self) -> Vec<Arc<TranscodeJob>> {
        let jobs = self.jobs.read();
        jobs.values().cloned().collect()
    }

    /// Processes a media packet through the transcode pipeline.
    ///
    /// # Errors
    ///
    /// Always returns an error: no real-time ingest transcode exists for any
    /// codec OxiMedia accepts, and the reason differs per codec, so the
    /// packet's codec is resolved first and named in the message. The ingest
    /// caller is expected to degrade to stream-copy (pass-through), so no
    /// transcode is ever falsely claimed.
    ///
    /// The per-codec blockers, as verified against the actual codec
    /// implementations:
    ///
    /// * **AV1 / VP9** — decode is keyframe-only (no inter-frame decode) and
    ///   the encoders do not produce valid bitstreams, so neither half of a
    ///   decode → encode graph exists.
    /// * **Opus** — neither the encoder nor the decoder is trustworthy, so a
    ///   re-encode would silently corrupt the audio.
    /// * **FLAC** — decode is real, but there is no ABR ladder for lossless
    ///   audio; re-encoding it gains nothing and is not wired.
    // TODO(0.2.x): once inter-frame decode and a real encoder exist for a
    // patent-free codec, implement the per-stream pipeline here — resolve the
    // `TranscodeJob` for the packet's stream, feed the packet into a live
    // decode -> ABR-scale -> encode graph (reuse oximedia-transcode), and emit
    // the encoded output on each quality level's `output_rxs` channel.
    pub async fn process_packet(&self, packet: &MediaPacket) -> ServerResult<()> {
        let reason = match sniff_ingest_codec(packet) {
            Ok(IngestCodec::Av1) => "real-time transcode of AV1 ingest is not implemented: \
                 AV1 inter-frame decode is unavailable (keyframe-only) and the AV1 encoder \
                 does not emit a valid bitstream; caller must stream-copy"
                .to_string(),
            Ok(IngestCodec::Vp9) => "real-time transcode of VP9 ingest is not implemented: \
                 VP9 inter-frame decode is unavailable (keyframe-only) and the VP9 encoder \
                 does not emit a valid bitstream; caller must stream-copy"
                .to_string(),
            Ok(IngestCodec::Opus) => "real-time transcode of Opus ingest is not implemented: \
                 neither the Opus encoder nor the Opus decoder is trustworthy, so a \
                 re-encode would corrupt the audio; caller must stream-copy"
                .to_string(),
            Ok(IngestCodec::Flac) => "real-time transcode of FLAC ingest is not implemented: \
                 FLAC decode is real but no lossless ABR ladder is wired; caller must \
                 stream-copy"
                .to_string(),
            Err(e) => format!(
                "real-time ingest transcoding is not implemented and the packet's codec \
                 could not be resolved ({e}); caller must stream-copy this packet"
            ),
        };
        Err(ServerError::TranscodingFailed(reason))
    }

    /// Starts transcoding for a stream.
    pub async fn start_transcoding(&self, stream_key: &str) -> ServerResult<()> {
        let job = self.get_job(stream_key).ok_or_else(|| {
            ServerError::NotFound(format!("Transcode job not found: {}", stream_key))
        })?;

        job.set_state(TranscodeJobState::Running);
        info!("Started transcoding for stream: {}", stream_key);

        Ok(())
    }

    /// Stops transcoding for a stream.
    pub async fn stop_transcoding(&self, stream_key: &str) -> ServerResult<()> {
        let job = self.get_job(stream_key).ok_or_else(|| {
            ServerError::NotFound(format!("Transcode job not found: {}", stream_key))
        })?;

        job.set_state(TranscodeJobState::Completed);
        info!("Stopped transcoding for stream: {}", stream_key);

        Ok(())
    }

    /// Gets the number of active jobs.
    #[must_use]
    pub fn active_jobs(&self) -> usize {
        let jobs = self.jobs.read();
        jobs.values()
            .filter(|j| j.state() == TranscodeJobState::Running)
            .count()
    }
}

// Stub for num_cpus (in real implementation, would use num_cpus crate)
mod num_cpus {
    #[must_use]
    #[allow(dead_code)]
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(4)
    }
}

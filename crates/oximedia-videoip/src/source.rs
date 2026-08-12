//! Video-over-IP source for broadcasting video/audio streams.

use crate::codec::{create_audio_encoder, create_video_encoder, AudioSamples, VideoFrame};
use crate::discovery::DiscoveryServer;
use crate::error::VideoIpResult;
use crate::fec::FecEncoder;
use crate::metadata::MetadataPacket;
use crate::packet::{PacketBuilder, PacketFlags};
use crate::ptz::PtzMessage;
use crate::stats::StatsTracker;
use crate::tally::TallyMessage;
use crate::transport::UdpTransport;
use crate::types::{AudioConfig, StreamType, VideoConfig};
use bytes::Bytes;
use parking_lot::RwLock;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, Interval};

/// Maximum packets to buffer before sending FEC.
#[allow(dead_code)]
const FEC_GROUP_SIZE: usize = 20;

/// Video-over-IP source for broadcasting streams.
pub struct VideoIpSource {
    /// Source name.
    name: String,
    /// Video configuration.
    video_config: VideoConfig,
    /// Audio configuration.
    audio_config: AudioConfig,
    /// UDP transport.
    transport: UdpTransport,
    /// Discovery server for mDNS announcement.
    discovery: Option<DiscoveryServer>,
    /// Video encoder.
    video_encoder: Box<dyn crate::codec::VideoEncoder>,
    /// Audio encoder.
    audio_encoder: Box<dyn crate::codec::AudioEncoder>,
    /// FEC encoder.
    fec_encoder: Option<FecEncoder>,
    /// Current sequence number.
    sequence: u16,
    /// Destination addresses for broadcasting.
    destinations: Arc<RwLock<Vec<SocketAddr>>>,
    /// Statistics tracker.
    stats: StatsTracker,
    /// Control message receiver.
    control_rx: mpsc::Receiver<ControlMessage>,
    /// Control message sender (for external control).
    control_tx: mpsc::Sender<ControlMessage>,
    /// Frame rate interval.
    frame_interval: Option<Interval>,
}

/// Control messages for the source.
#[derive(Debug, Clone)]
pub enum ControlMessage {
    /// Add a destination address.
    AddDestination(SocketAddr),
    /// Remove a destination address.
    RemoveDestination(SocketAddr),
    /// Send PTZ message.
    Ptz(PtzMessage),
    /// Send tally message.
    Tally(TallyMessage),
    /// Send metadata.
    Metadata(MetadataPacket),
}

impl VideoIpSource {
    /// Creates a new video-over-IP source.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot be created.
    pub async fn new(
        name: impl Into<String>,
        video_config: VideoConfig,
        audio_config: AudioConfig,
    ) -> VideoIpResult<Self> {
        let name = name.into();
        let bind_addr = "0.0.0.0:0".parse().expect("should succeed in test");
        let transport = UdpTransport::bind(bind_addr).await?;

        let video_encoder = create_video_encoder(
            video_config.format.codec,
            video_config.format.resolution.width,
            video_config.format.resolution.height,
            video_config.target_bitrate,
        )?;

        let audio_encoder = create_audio_encoder(
            audio_config.format.codec,
            audio_config.format.sample_rate,
            audio_config.format.channels,
        )?;

        let (control_tx, control_rx) = mpsc::channel(100);

        Ok(Self {
            name,
            video_config,
            audio_config,
            transport,
            discovery: None,
            video_encoder,
            audio_encoder,
            fec_encoder: None,
            sequence: 0,
            destinations: Arc::new(RwLock::new(Vec::new())),
            stats: StatsTracker::new(),
            control_rx,
            control_tx,
            frame_interval: None,
        })
    }

    /// Starts broadcasting by announcing the service via mDNS.
    ///
    /// # Errors
    ///
    /// Returns an error if the announcement fails.
    pub fn start_broadcasting(&mut self) -> VideoIpResult<()> {
        let mut discovery = DiscoveryServer::new()?;
        let port = self.transport.local_addr().port();

        discovery.announce(
            &self.name,
            port,
            &self.video_config.format,
            &self.audio_config.format,
        )?;

        self.discovery = Some(discovery);

        // Set up frame rate interval
        let fps = self.video_config.format.frame_rate.to_float();
        let frame_duration = Duration::from_secs_f64(1.0 / fps);
        self.frame_interval = Some(interval(frame_duration));

        Ok(())
    }

    /// Stops broadcasting and removes the mDNS announcement.
    ///
    /// # Errors
    ///
    /// Returns an error if stopping fails.
    pub fn stop_broadcasting(&mut self) -> VideoIpResult<()> {
        if let Some(mut discovery) = self.discovery.take() {
            discovery.stop_announce()?;
        }
        Ok(())
    }

    /// Enables FEC with the specified ratio.
    ///
    /// # Errors
    ///
    /// Returns an error if FEC cannot be enabled.
    pub fn enable_fec(&mut self, ratio: f32) -> VideoIpResult<()> {
        self.fec_encoder = Some(FecEncoder::with_ratio(ratio)?);
        Ok(())
    }

    /// Adds a destination address for broadcasting.
    pub fn add_destination(&self, addr: SocketAddr) {
        self.destinations.write().push(addr);
    }

    /// Removes a destination address.
    pub fn remove_destination(&self, addr: SocketAddr) {
        self.destinations.write().retain(|a| *a != addr);
    }

    /// Returns a sender for control messages.
    #[must_use]
    pub fn control_sender(&self) -> mpsc::Sender<ControlMessage> {
        self.control_tx.clone()
    }

    /// Sends a video frame with optional audio samples.
    ///
    /// # Errors
    ///
    /// Returns an error if encoding or sending fails.
    pub async fn send_frame(
        &mut self,
        video_frame: VideoFrame,
        audio_samples: Option<AudioSamples>,
    ) -> VideoIpResult<()> {
        // Wait for frame interval if configured
        if let Some(ref mut interval) = self.frame_interval {
            interval.tick().await;
        }

        // Encode video
        let video_data = self.video_encoder.encode(&video_frame)?;
        let is_keyframe = video_frame.is_keyframe;

        // Send video packets
        self.send_video_data(video_data, is_keyframe, video_frame.pts)
            .await?;

        // Encode and send audio if provided
        if let Some(samples) = audio_samples {
            let audio_data = self.audio_encoder.encode(&samples)?;
            self.send_audio_data(audio_data, samples.pts).await?;
        }

        // Process control messages
        self.process_control_messages().await?;

        Ok(())
    }

    /// Sends encoded video data as packets.
    async fn send_video_data(
        &mut self,
        data: Bytes,
        is_keyframe: bool,
        pts: u64,
    ) -> VideoIpResult<()> {
        const MAX_PAYLOAD: usize = 8000;
        let chunks: Vec<_> = data.chunks(MAX_PAYLOAD).collect();
        let chunk_count = chunks.len();

        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut flags = PacketFlags::VIDEO;
            if is_keyframe {
                flags |= PacketFlags::KEYFRAME;
            }
            if i == 0 {
                flags |= PacketFlags::START_OF_FRAME;
            }
            if i == chunk_count - 1 {
                flags |= PacketFlags::END_OF_FRAME;
            }

            let mut builder = PacketBuilder::new(self.sequence)
                .with_timestamp(pts)
                .with_stream_type(StreamType::Program);

            if is_keyframe {
                builder = builder.keyframe();
            }
            if i == 0 {
                builder = builder.start_of_frame();
            }
            if i == chunk_count - 1 {
                builder = builder.end_of_frame();
            }

            let packet = builder.video().build(Bytes::copy_from_slice(chunk))?;

            self.send_packet(&packet).await?;
            self.sequence = self.sequence.wrapping_add(1);
        }

        Ok(())
    }

    /// Sends encoded audio data as packets.
    async fn send_audio_data(&mut self, data: Bytes, pts: u64) -> VideoIpResult<()> {
        let packet = PacketBuilder::new(self.sequence)
            .audio()
            .with_timestamp(pts)
            .with_stream_type(StreamType::Program)
            .build(data)?;

        self.send_packet(&packet).await?;
        self.sequence = self.sequence.wrapping_add(1);

        Ok(())
    }

    /// Sends a packet to all destinations.
    async fn send_packet(&mut self, packet: &crate::packet::Packet) -> VideoIpResult<()> {
        let destinations = self.destinations.read().clone();
        let packet_size = packet.size();

        for dest in &destinations {
            self.transport.send_packet(packet, *dest).await?;
        }

        self.stats.record_sent(packet_size);

        Ok(())
    }

    /// Processes pending control messages.
    async fn process_control_messages(&mut self) -> VideoIpResult<()> {
        while let Ok(msg) = self.control_rx.try_recv() {
            match msg {
                ControlMessage::AddDestination(addr) => {
                    self.add_destination(addr);
                }
                ControlMessage::RemoveDestination(addr) => {
                    self.remove_destination(addr);
                }
                ControlMessage::Ptz(ptz_msg) => {
                    let data = ptz_msg.encode();
                    let packet = PacketBuilder::new(self.sequence)
                        .metadata()
                        .with_current_timestamp()
                        .build(data)?;

                    self.send_packet(&packet).await?;
                    self.sequence = self.sequence.wrapping_add(1);
                }
                ControlMessage::Tally(tally_msg) => {
                    let data = tally_msg.encode();
                    let packet = PacketBuilder::new(self.sequence)
                        .metadata()
                        .with_current_timestamp()
                        .build(data)?;

                    self.send_packet(&packet).await?;
                    self.sequence = self.sequence.wrapping_add(1);
                }
                ControlMessage::Metadata(metadata) => {
                    let data = metadata.encode();
                    let packet = PacketBuilder::new(self.sequence)
                        .metadata()
                        .with_current_timestamp()
                        .build(data)?;

                    self.send_packet(&packet).await?;
                    self.sequence = self.sequence.wrapping_add(1);
                }
            }
        }

        Ok(())
    }

    /// Returns the current statistics.
    #[must_use]
    pub fn stats(&self) -> crate::stats::NetworkStats {
        self.stats.get_stats()
    }

    /// Returns the local socket address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }
}

impl Drop for VideoIpSource {
    fn drop(&mut self) {
        let _ = self.stop_broadcasting();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioCodec, VideoCodec};

    /// A configuration this crate can genuinely transmit: uncompressed video
    /// and uncompressed audio. The compressed codecs have no working encoder
    /// (see [`crate::codec`]) and are rejected by `VideoIpSource::new`.
    fn transmittable_configs(width: u32, height: u32) -> (VideoConfig, AudioConfig) {
        let video = VideoConfig::new(width, height, 30.0)
            .expect("should succeed in test")
            .with_codec(VideoCodec::Uyvy);
        let audio = AudioConfig::new(48000, 2)
            .expect("should succeed in test")
            .with_codec(AudioCodec::Pcm16)
            .expect("should succeed in test");
        (video, audio)
    }

    #[tokio::test]
    async fn test_source_creation() {
        let (video_config, audio_config) = transmittable_configs(1920, 1080);

        let source = VideoIpSource::new("Test Source", video_config, audio_config).await;
        assert!(source.is_ok());
    }

    /// The compressed codecs must be refused at construction rather than
    /// silently broadcasting raw frames labelled as a compressed bitstream.
    #[tokio::test]
    async fn test_source_rejects_codecs_without_a_real_encoder() {
        for codec in [VideoCodec::Vp9, VideoCodec::Av1, VideoCodec::Vp8] {
            let video_config = VideoConfig::new(640, 480, 30.0)
                .expect("should succeed in test")
                .with_codec(codec);
            let audio_config = AudioConfig::new(48000, 2)
                .expect("should succeed in test")
                .with_codec(AudioCodec::Pcm16)
                .expect("should succeed in test");

            let err = VideoIpSource::new("Test", video_config, audio_config)
                .await
                .err()
                .expect("compressed video must not be transmittable");
            assert!(
                matches!(
                    err,
                    crate::error::VideoIpError::CodecUnimplemented {
                        operation: "encode",
                        ..
                    }
                ),
                "{codec:?}: unexpected error {err}"
            );
        }

        // ...and likewise for Opus audio.
        let video_config = VideoConfig::new(640, 480, 30.0)
            .expect("should succeed in test")
            .with_codec(VideoCodec::Uyvy);
        let audio_config = AudioConfig::new(48000, 2).expect("should succeed in test");
        let err = VideoIpSource::new("Test", video_config, audio_config)
            .await
            .err()
            .expect("Opus must not be transmittable");
        assert!(
            matches!(
                err,
                crate::error::VideoIpError::CodecUnimplemented {
                    codec: "Opus",
                    operation: "encode",
                    ..
                }
            ),
            "unexpected error {err}"
        );
    }

    #[tokio::test]
    async fn test_source_add_destination() {
        let (video_config, audio_config) = transmittable_configs(1920, 1080);

        let source = VideoIpSource::new("Test", video_config, audio_config)
            .await
            .expect("should succeed in test");

        let dest = "127.0.0.1:5000".parse().expect("should succeed in test");
        source.add_destination(dest);

        assert_eq!(source.destinations.read().len(), 1);
    }

    #[tokio::test]
    async fn test_source_enable_fec() {
        let (video_config, audio_config) = transmittable_configs(1920, 1080);

        let mut source = VideoIpSource::new("Test", video_config, audio_config)
            .await
            .expect("should succeed in test");

        assert!(source.enable_fec(0.1).is_ok());
        assert!(source.fec_encoder.is_some());
    }

    #[tokio::test]
    async fn test_send_frame() {
        let (video_config, audio_config) = transmittable_configs(64, 48);

        let mut source = VideoIpSource::new("Test", video_config, audio_config)
            .await
            .expect("should succeed in test");

        // A real 64x48 UYVY frame and 256 stereo 16-bit PCM samples.
        let frame = VideoFrame::new(Bytes::from(vec![16u8; 64 * 48 * 2]), 64, 48, true, 0);
        let samples = AudioSamples::new(Bytes::from(vec![0u8; 256 * 2 * 2]), 256, 2, 48000, 0);

        let result = source.send_frame(frame, Some(samples)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_control_messages() {
        let (video_config, audio_config) = transmittable_configs(640, 480);

        let source = VideoIpSource::new("Test", video_config, audio_config)
            .await
            .expect("should succeed in test");

        let control_tx = source.control_sender();
        let dest = "127.0.0.1:5000".parse().expect("should succeed in test");

        control_tx
            .send(ControlMessage::AddDestination(dest))
            .await
            .expect("should succeed in test");
    }
}

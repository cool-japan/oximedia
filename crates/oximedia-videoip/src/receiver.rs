//! Video-over-IP receiver for receiving video/audio streams.

use crate::codec::{create_audio_decoder, create_video_decoder, AudioSamples, VideoFrame};
use crate::discovery::DiscoveryClient;
use crate::error::{VideoIpError, VideoIpResult};
use crate::fec::FecDecoder;
use crate::jitter::JitterBuffer;
use crate::metadata::MetadataPacket;
use crate::packet::{Packet, PacketFlags};
use crate::ptz::PtzMessage;
use crate::stats::StatsTracker;
use crate::tally::TallyMessage;
use crate::transport::UdpTransport;
use crate::types::{AudioFormat, VideoFormat};
use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Maximum time to wait for a complete frame (milliseconds).
const FRAME_TIMEOUT_MS: u64 = 100;

/// Maximum number of decoded audio blocks held while waiting for the video
/// frame they accompany. Older blocks are dropped once this is exceeded.
const MAX_BUFFERED_AUDIO_BLOCKS: usize = 64;

/// Video-over-IP receiver for receiving streams.
#[allow(dead_code)]
pub struct VideoIpReceiver {
    /// UDP transport.
    transport: UdpTransport,
    /// Source address.
    source_addr: Option<SocketAddr>,
    /// Video decoder.
    video_decoder: Box<dyn crate::codec::VideoDecoder>,
    /// Audio decoder.
    audio_decoder: Box<dyn crate::codec::AudioDecoder>,
    /// FEC decoder.
    fec_decoder: Option<FecDecoder>,
    /// Jitter buffer.
    jitter_buffer: JitterBuffer,
    /// Statistics tracker.
    stats: StatsTracker,
    /// Last received sequence number.
    last_sequence: Option<u16>,
    /// Incomplete frames being assembled.
    frame_assembly: HashMap<u64, FrameAssembly>,
    /// Decoded audio waiting to be paired with a video frame.
    audio_buffer: VecDeque<AudioSamples>,
    /// Control message sender.
    control_tx: mpsc::Sender<ControlEvent>,
    /// Control message receiver (for external access).
    control_rx: Arc<RwLock<mpsc::Receiver<ControlEvent>>>,
}

/// Frame assembly state for multi-packet frames.
#[allow(dead_code)]
struct FrameAssembly {
    /// Frame chunks.
    chunks: Vec<Option<Bytes>>,
    /// Total expected chunks.
    total_chunks: usize,
    /// Timestamp of first packet.
    timestamp: u64,
    /// Time when assembly started.
    start_time: Instant,
    /// Whether this is a keyframe.
    _is_keyframe: bool,
}

/// Control events from the receiver.
#[derive(Debug, Clone)]
pub enum ControlEvent {
    /// PTZ message received.
    Ptz(PtzMessage),
    /// Tally message received.
    Tally(TallyMessage),
    /// Metadata received.
    Metadata(MetadataPacket),
}

impl VideoIpReceiver {
    /// Creates a new video-over-IP receiver for the announced stream formats.
    ///
    /// The full [`VideoFormat`] / [`AudioFormat`] is required rather than just
    /// the codec: an uncompressed stream carries no geometry in its payload and
    /// PCM carries no sample count, so without the announced resolution and
    /// sample rate the receiver would have to invent them. Compressed video
    /// takes its dimensions from the bitstream and ignores the announced
    /// resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if the receiver cannot be created, in particular
    /// [`VideoIpError::CodecUnimplemented`] if either format names a codec this
    /// crate cannot honestly decode (see [`crate::codec`]).
    pub async fn new(
        video_format: &VideoFormat,
        audio_format: &AudioFormat,
    ) -> VideoIpResult<Self> {
        let bind_addr = "0.0.0.0:0"
            .parse()
            .map_err(|e: std::net::AddrParseError| VideoIpError::Transport(e.to_string()))?;
        let transport = UdpTransport::bind(bind_addr).await?;

        let video_decoder =
            create_video_decoder(video_format.codec, Some(video_format.resolution))?;
        let audio_decoder = create_audio_decoder(
            audio_format.codec,
            audio_format.sample_rate,
            audio_format.channels,
        )?;

        let jitter_buffer = JitterBuffer::new(100, 20);

        let (control_tx, control_rx) = mpsc::channel(100);

        Ok(Self {
            transport,
            source_addr: None,
            video_decoder,
            audio_decoder,
            fec_decoder: None,
            jitter_buffer,
            stats: StatsTracker::new(),
            last_sequence: None,
            frame_assembly: HashMap::new(),
            audio_buffer: VecDeque::new(),
            control_tx,
            control_rx: Arc::new(RwLock::new(control_rx)),
        })
    }

    /// Discovers and connects to a source by name.
    ///
    /// # Errors
    ///
    /// Returns an error if the source is not found.
    pub async fn discover(name: &str) -> VideoIpResult<Self> {
        let client = DiscoveryClient::new()?;
        let source = client.discover_by_name(name, 5).await?;

        // Use the announced stream formats verbatim.
        let mut receiver = Self::new(&source.video_format, &source.audio_format).await?;
        receiver.source_addr = Some(source.socket_addr());

        Ok(receiver)
    }

    /// Connects to a specific source address.
    ///
    /// # Errors
    ///
    /// Returns an error if connection fails.
    pub async fn connect(
        addr: SocketAddr,
        video_format: &VideoFormat,
        audio_format: &AudioFormat,
    ) -> VideoIpResult<Self> {
        let mut receiver = Self::new(video_format, audio_format).await?;
        receiver.source_addr = Some(addr);
        Ok(receiver)
    }

    /// Starts receiving packets.
    pub fn start_receiving(&self) {
        // In a real implementation, this would start background tasks
    }

    /// Stops receiving packets.
    pub fn stop_receiving(&self) {
        // In a real implementation, this would stop background tasks
    }

    /// Enables FEC decoding.
    ///
    /// # Errors
    ///
    /// Returns an error if FEC cannot be enabled.
    pub fn enable_fec(&mut self, data_shards: usize, parity_shards: usize) -> VideoIpResult<()> {
        self.fec_decoder = Some(FecDecoder::new(data_shards, parity_shards)?);
        Ok(())
    }

    /// Receives a complete frame (video and audio).
    ///
    /// # Errors
    ///
    /// Returns an error if receiving fails or times out.
    pub async fn receive_frame(&mut self) -> VideoIpResult<(VideoFrame, Option<AudioSamples>)> {
        let deadline = Duration::from_millis(FRAME_TIMEOUT_MS);

        timeout(deadline, async {
            loop {
                // Receive packets
                let (packet, _addr) = self.transport.recv_packet().await?;

                // Update stats
                self.stats.record_received(packet.size());

                // Check sequence for packet loss
                self.check_sequence(packet.header.sequence);

                // Handle FEC packets
                if packet.header.flags.contains(PacketFlags::FEC) {
                    if let Some(ref mut fec) = self.fec_decoder {
                        let recovered = fec.add_packet(packet)?;
                        for p in recovered {
                            self.jitter_buffer.add_packet(p)?;
                        }
                    }
                    continue;
                }

                // Add to jitter buffer
                self.jitter_buffer.add_packet(packet)?;

                // Try to get packets from jitter buffer and assemble frames
                while let Some(packet) = self.jitter_buffer.get_packet() {
                    if packet.header.flags.contains(PacketFlags::VIDEO) {
                        if let Some(frame) = self.process_video_packet(packet)? {
                            // We have a complete video frame; hand over the
                            // oldest decoded audio block, if any has arrived.
                            let audio = self.audio_buffer.pop_front();
                            return Ok((frame, audio));
                        }
                    } else if packet.header.flags.contains(PacketFlags::AUDIO) {
                        // Store audio for later retrieval
                        self.process_audio_packet(packet)?;
                    } else if packet.header.flags.contains(PacketFlags::METADATA) {
                        self.process_metadata_packet(packet)?;
                    }
                }

                // Cleanup old incomplete frames
                self.cleanup_old_frames();
            }
        })
        .await
        .map_err(|_| VideoIpError::Timeout)?
    }

    /// Processes a video packet and assembles frames.
    fn process_video_packet(&mut self, packet: Packet) -> VideoIpResult<Option<VideoFrame>> {
        let pts = packet.header.timestamp;
        let is_keyframe = packet.header.flags.contains(PacketFlags::KEYFRAME);
        let is_start = packet.header.flags.contains(PacketFlags::START_OF_FRAME);
        let is_end = packet.header.flags.contains(PacketFlags::END_OF_FRAME);

        // Single-packet frame
        if is_start && is_end {
            return self.decode_video_frame(packet.payload, is_keyframe, pts);
        }

        // Multi-packet frame assembly
        let assembly = self
            .frame_assembly
            .entry(pts)
            .or_insert_with(|| FrameAssembly {
                chunks: Vec::new(),
                total_chunks: 0,
                timestamp: pts,
                start_time: Instant::now(),
                _is_keyframe: is_keyframe,
            });

        if is_start {
            assembly.chunks.clear();
            assembly.chunks.push(Some(packet.payload));
        } else if is_end {
            assembly.chunks.push(Some(packet.payload));
            assembly.total_chunks = assembly.chunks.len();

            // Assemble complete frame
            let complete = assembly.chunks.iter().all(Option::is_some);
            if complete {
                let mut data = BytesMut::new();
                for bytes in assembly.chunks.iter().flatten() {
                    data.extend_from_slice(bytes);
                }

                let frame = self.decode_video_frame(data.freeze(), is_keyframe, pts)?;
                self.frame_assembly.remove(&pts);
                return Ok(frame);
            }
        } else {
            assembly.chunks.push(Some(packet.payload));
        }

        Ok(None)
    }

    /// Decodes a complete video frame.
    ///
    /// The transport's keyframe flag and timestamp are handed to the decoder;
    /// for compressed codecs the decoder overrides the keyframe flag (and the
    /// dimensions) with what the bitstream actually says.
    fn decode_video_frame(
        &mut self,
        data: Bytes,
        is_keyframe: bool,
        pts: u64,
    ) -> VideoIpResult<Option<VideoFrame>> {
        self.video_decoder.decode(&data, pts, is_keyframe)
    }

    /// Decodes an audio packet and queues it for the next video frame.
    fn process_audio_packet(&mut self, packet: Packet) -> VideoIpResult<()> {
        let pts = packet.header.timestamp;
        if let Some(samples) = self.audio_decoder.decode(&packet.payload, pts)? {
            if self.audio_buffer.len() >= MAX_BUFFERED_AUDIO_BLOCKS {
                self.audio_buffer.pop_front();
            }
            self.audio_buffer.push_back(samples);
        }
        Ok(())
    }

    /// Processes a metadata packet.
    fn process_metadata_packet(&mut self, packet: Packet) -> VideoIpResult<()> {
        // Try to parse as different metadata types
        if let Ok(ptz_msg) = PtzMessage::decode(&packet.payload) {
            let _ = self.control_tx.try_send(ControlEvent::Ptz(ptz_msg));
        } else if let Ok(tally_msg) = TallyMessage::decode(&packet.payload) {
            let _ = self.control_tx.try_send(ControlEvent::Tally(tally_msg));
        } else if let Ok(metadata) = MetadataPacket::decode(&packet.payload) {
            let _ = self.control_tx.try_send(ControlEvent::Metadata(metadata));
        }

        Ok(())
    }

    /// Checks for packet loss by comparing sequence numbers.
    fn check_sequence(&mut self, sequence: u16) {
        if let Some(last) = self.last_sequence {
            let expected = last.wrapping_add(1);
            if sequence != expected {
                // Packet loss detected
                let lost = if sequence > expected {
                    u64::from(sequence - expected)
                } else {
                    u64::from((u16::MAX - expected) + sequence + 1)
                };

                for _ in 0..lost {
                    self.stats.record_lost();
                }
            }
        }

        self.last_sequence = Some(sequence);
    }

    /// Cleans up old incomplete frames.
    fn cleanup_old_frames(&mut self) {
        let now = Instant::now();
        let timeout = Duration::from_millis(FRAME_TIMEOUT_MS);

        self.frame_assembly
            .retain(|_, assembly| now.duration_since(assembly.start_time) < timeout);
    }

    /// Returns the current statistics.
    #[must_use]
    pub fn stats(&self) -> crate::stats::NetworkStats {
        self.stats.get_stats()
    }

    /// Returns a receiver for control events.
    #[must_use]
    pub fn control_receiver(&self) -> Arc<RwLock<mpsc::Receiver<ControlEvent>>> {
        Arc::clone(&self.control_rx)
    }

    /// Returns the local socket address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }

    /// Returns the jitter buffer statistics.
    #[must_use]
    pub fn jitter_stats(&self) -> crate::jitter::JitterStats {
        self.jitter_buffer.stats().clone()
    }

    /// Adjusts the jitter buffer delay dynamically.
    pub fn adjust_jitter_buffer(&mut self) {
        self.jitter_buffer.adjust_delay();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioCodec, Resolution, VideoCodec};

    /// VP9 video (a real decoder) plus PCM audio (a real decoder).
    fn receivable_formats() -> (VideoFormat, AudioFormat) {
        let video = VideoFormat::new(
            VideoCodec::Vp9,
            Resolution::HD_1080,
            crate::types::FrameRate::FPS_30,
        );
        let audio = AudioFormat::new(AudioCodec::Pcm16, 48000, 2).expect("should succeed in test");
        (video, audio)
    }

    #[tokio::test]
    async fn test_receiver_creation() {
        let (video, audio) = receivable_formats();
        let receiver = VideoIpReceiver::new(&video, &audio).await;
        assert!(receiver.is_ok());
    }

    /// Opus has no honest decoder, so a receiver cannot claim to accept it.
    #[tokio::test]
    async fn test_receiver_rejects_opus() {
        let (video, _) = receivable_formats();
        let audio = AudioFormat::new(AudioCodec::Opus, 48000, 2).expect("should succeed in test");
        let err = VideoIpReceiver::new(&video, &audio)
            .await
            .err()
            .expect("Opus must not be receivable");
        assert!(
            matches!(
                err,
                VideoIpError::CodecUnimplemented {
                    codec: "Opus",
                    operation: "decode",
                    ..
                }
            ),
            "unexpected error {err}"
        );
    }

    #[tokio::test]
    async fn test_receiver_connect() {
        let addr = "127.0.0.1:5000".parse().expect("should succeed in test");
        let (video, audio) = receivable_formats();
        let receiver = VideoIpReceiver::connect(addr, &video, &audio).await;
        assert!(receiver.is_ok());
    }

    #[tokio::test]
    async fn test_receiver_enable_fec() {
        let (video, audio) = receivable_formats();
        let mut receiver = VideoIpReceiver::new(&video, &audio)
            .await
            .expect("should succeed in test");

        assert!(receiver.enable_fec(20, 2).is_ok());
        assert!(receiver.fec_decoder.is_some());
    }

    /// PCM audio packets are really decoded and queued for the next frame.
    #[tokio::test]
    async fn test_audio_packets_are_decoded_and_buffered() {
        let (video, audio) = receivable_formats();
        let mut receiver = VideoIpReceiver::new(&video, &audio)
            .await
            .expect("should succeed in test");

        // 400 bytes = 100 stereo 16-bit samples per channel.
        let packet = crate::packet::PacketBuilder::new(0)
            .audio()
            .with_timestamp(4242)
            .build(Bytes::from(vec![0u8; 400]))
            .expect("should succeed in test");

        receiver
            .process_audio_packet(packet)
            .expect("PCM decode should succeed");

        let samples = receiver
            .audio_buffer
            .pop_front()
            .expect("decoded audio should be queued");
        assert_eq!(samples.sample_count, 100);
        assert_eq!(samples.channels, 2);
        assert_eq!(samples.sample_rate, 48000);
        assert_eq!(samples.pts, 4242);
    }

    /// A payload that is not a whole number of PCM samples is reported, not
    /// rounded off into a fabricated sample count.
    #[tokio::test]
    async fn test_ragged_audio_packet_is_reported() {
        let (video, audio) = receivable_formats();
        let mut receiver = VideoIpReceiver::new(&video, &audio)
            .await
            .expect("should succeed in test");

        let packet = crate::packet::PacketBuilder::new(0)
            .audio()
            .build(Bytes::from(vec![0u8; 401]))
            .expect("should succeed in test");

        assert!(receiver.process_audio_packet(packet).is_err());
        assert!(receiver.audio_buffer.is_empty());
    }

    #[test]
    fn test_sequence_check() {
        let rt = tokio::runtime::Runtime::new().expect("should succeed in test");
        let (video, audio) = receivable_formats();
        let mut receiver = rt
            .block_on(VideoIpReceiver::new(&video, &audio))
            .expect("should succeed in test");

        receiver.check_sequence(0);
        receiver.check_sequence(1);
        receiver.check_sequence(2);

        let stats = receiver.stats();
        assert_eq!(stats.packets_lost, 0);

        // Skip sequence 3
        receiver.check_sequence(4);
        let stats = receiver.stats();
        assert_eq!(stats.packets_lost, 1);
    }

    #[test]
    fn test_cleanup_old_frames() {
        let rt = tokio::runtime::Runtime::new().expect("should succeed in test");
        let (video, audio) = receivable_formats();
        let mut receiver = rt
            .block_on(VideoIpReceiver::new(&video, &audio))
            .expect("should succeed in test");

        // Add an old frame assembly
        receiver.frame_assembly.insert(
            12345,
            FrameAssembly {
                chunks: vec![],
                total_chunks: 0,
                timestamp: 12345,
                start_time: Instant::now() - Duration::from_secs(1),
                _is_keyframe: false,
            },
        );

        receiver.cleanup_old_frames();
        assert_eq!(receiver.frame_assembly.len(), 0);
    }
}

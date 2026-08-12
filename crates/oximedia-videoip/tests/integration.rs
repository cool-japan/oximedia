//! Integration tests for video-over-IP protocol.

use oximedia_videoip::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::time::{sleep, Duration};

/// Uncompressed video + PCM audio is the only combination this crate can
/// genuinely put on the wire, so it is the one the end-to-end test uses.
fn transmittable_configs(width: u32, height: u32) -> (VideoConfig, AudioConfig) {
    let video = VideoConfig::new(width, height, 30.0)
        .expect("video_config should be valid")
        .with_codec(types::VideoCodec::Uyvy);
    let audio = AudioConfig::new(48000, 2)
        .expect("audio_config should be valid")
        .with_codec(types::AudioCodec::Pcm16)
        .expect("audio_config should be valid");
    (video, audio)
}

#[tokio::test]
async fn test_end_to_end_video_streaming() {
    // 64x48 UYVY is 6144 bytes: one packet, no fragmentation.
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 48;
    let (video_config, audio_config) = transmittable_configs(WIDTH, HEIGHT);
    let video_format = video_config.format.clone();
    let audio_format = audio_config.format.clone();

    let mut source = VideoIpSource::new("Test Camera", video_config, audio_config)
        .await
        .expect("test expectation failed");

    let source_addr = source.local_addr();

    // Create receiver
    let mut receiver = VideoIpReceiver::connect(source_addr, &video_format, &audio_format)
        .await
        .expect("test expectation failed");

    // `local_addr()` reports the wildcard bind (0.0.0.0:port); sending there
    // fails with ENETUNREACH/EHOSTUNREACH, which is why this test never
    // actually delivered anything before. Address the loopback explicitly.
    source.add_destination(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        receiver.local_addr().port(),
    ));

    // Send a real, correctly sized frame and a real PCM block.
    let pixels: Vec<u8> = (0..(WIDTH * HEIGHT * 2) as usize)
        .map(|i| (i % 251) as u8)
        .collect();
    let frame = codec::VideoFrame::new(
        bytes::Bytes::from(pixels.clone()),
        WIDTH,
        HEIGHT,
        true,
        7_000_000,
    );

    let samples = codec::AudioSamples::new(
        bytes::Bytes::from(vec![0u8; 480 * 2 * 2]),
        480,
        2,
        48000,
        7_000_000,
    );

    // Send the same frame repeatedly in the background. `receive_frame` only
    // drains the jitter buffer when a new datagram wakes it up, and the buffer
    // holds each packet for its 20 ms target delay, so a single send would sit
    // in the buffer until the receive timeout expired. Every repetition
    // carries identical content, so whichever one is delivered first satisfies
    // the assertions below.
    tokio::spawn(async move {
        for _ in 0..8u32 {
            sleep(Duration::from_millis(12)).await;
            source
                .send_frame(frame.clone(), Some(samples.clone()))
                .await
                .expect("test expectation failed");
        }
    });

    let result = tokio::time::timeout(Duration::from_millis(500), receiver.receive_frame()).await;

    let (video_frame, audio) = result
        .expect("receive_frame timed out over loopback")
        .expect("receive_frame failed");

    // The receiver must report the geometry and payload that were really
    // sent -- these used to be a hardcoded 1920x1080 and a dropped PTS.
    assert_eq!(video_frame.width, WIDTH);
    assert_eq!(video_frame.height, HEIGHT);
    assert_eq!(video_frame.pts, 7_000_000);
    assert!(video_frame.is_keyframe);
    assert_eq!(&video_frame.data[..], &pixels[..]);

    // Audio may or may not have been dequeued before the video frame
    // completed, but whatever arrives must be really decoded PCM.
    if let Some(audio) = audio {
        assert_eq!(audio.sample_count, 480);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.sample_rate, 48000);
        assert_eq!(audio.pts, 7_000_000);
    }
}

#[tokio::test]
async fn test_fec_recovery() {
    let (video_config, audio_config) = transmittable_configs(64, 48);

    let mut source = VideoIpSource::new("FEC Test", video_config, audio_config)
        .await
        .expect("test expectation failed");

    // Enable FEC
    source.enable_fec(0.1).expect("enable_fec should succeed");

    let frame =
        codec::VideoFrame::new(bytes::Bytes::from(vec![32u8; 64 * 48 * 2]), 64, 48, true, 0);

    // Should not panic
    source
        .send_frame(frame, None)
        .await
        .expect("test expectation failed");
}

#[tokio::test]
async fn test_metadata_transmission() {
    use oximedia_videoip::metadata::*;

    let timecode = Timecode::new(1, 23, 45, 12, false);
    let packet = MetadataPacket::timecode(timecode);

    let encoded = packet.encode();
    let decoded = MetadataPacket::decode(&encoded).expect("decoded should be valid");

    assert_eq!(
        decoded.as_timecode().expect("as_timecode should succeed"),
        timecode
    );
}

#[tokio::test]
async fn test_ptz_control() {
    use oximedia_videoip::ptz::*;

    let mut controller = PtzController::new();

    let msg = PtzMessage::new(1, PtzCommand::AbsolutePosition).with_position(45.0, 30.0, 0.5);

    controller.process_command(&msg, 0.1);

    let (pan, tilt, zoom) = controller.position();
    assert!((pan - 45.0).abs() < 0.001);
    assert!((tilt - 30.0).abs() < 0.001);
    assert!((zoom - 0.5).abs() < 0.001);
}

#[tokio::test]
async fn test_tally_system() {
    use oximedia_videoip::tally::*;

    let mut controller = TallyController::new();

    controller.set_state(1, TallyState::Program);
    controller.set_state(2, TallyState::Preview);

    assert_eq!(controller.get_state(1), TallyState::Program);
    assert_eq!(controller.get_state(2), TallyState::Preview);

    let active = controller.active_tallies();
    assert_eq!(active.len(), 2);
}

#[tokio::test]
async fn test_jitter_buffer() {
    use bytes::Bytes;
    use oximedia_videoip::jitter::JitterBuffer;
    use oximedia_videoip::packet::PacketBuilder;

    let mut buffer = JitterBuffer::new(100, 0);

    // Add packets out of order
    for seq in [2u16, 0, 1, 4, 3] {
        let packet = PacketBuilder::new(seq)
            .video()
            .build(Bytes::from_static(b"test"))
            .expect("test expectation failed");
        buffer
            .add_packet(packet)
            .expect("add_packet should succeed");
    }

    // Should come out in order
    for expected in 0..5 {
        let packet = buffer
            .get_packet_immediate()
            .expect("packet should be valid");
        assert_eq!(packet.header.sequence, expected);
    }
}

#[tokio::test]
async fn test_network_stats() {
    use oximedia_videoip::stats::*;

    let mut tracker = StatsTracker::new();

    tracker.record_sent(1000);
    tracker.record_received(1000);
    tracker.record_lost();
    tracker.update_jitter(5000);

    let stats = tracker.get_stats();
    assert_eq!(stats.packets_sent, 1);
    assert_eq!(stats.packets_received, 1);
    assert_eq!(stats.packets_lost, 1);
}

#[tokio::test]
async fn test_packet_fragmentation() {
    use bytes::Bytes;
    use oximedia_videoip::packet::*;

    // Create a large payload that will need fragmentation
    let large_data = vec![0u8; MAX_PACKET_SIZE * 2];

    // In real implementation, source would fragment automatically
    let chunks: Vec<_> = large_data.chunks(MAX_PAYLOAD_SIZE).collect();
    assert!(chunks.len() > 1);

    // Create packets for each chunk
    for (i, chunk) in chunks.iter().enumerate() {
        let mut flags = PacketFlags::VIDEO;
        if i == 0 {
            flags |= PacketFlags::START_OF_FRAME;
        }
        if i == chunks.len() - 1 {
            flags |= PacketFlags::END_OF_FRAME;
        }

        let packet = PacketBuilder::new(i as u16)
            .with_timestamp(12345)
            .build(Bytes::copy_from_slice(chunk))
            .expect("test expectation failed");

        // Packet should be valid
        assert!(packet.header.validate().is_ok());
    }
}

#[tokio::test]
async fn test_timecode_conversion() {
    use oximedia_videoip::metadata::Timecode;

    let tc = Timecode::new(1, 2, 3, 4, false);

    // Test encoding/decoding
    let bytes = tc.to_bytes();
    let decoded = Timecode::from_bytes(&bytes).expect("decoded should be valid");
    assert_eq!(tc, decoded);

    // Test frame conversion
    let total_frames = tc.to_frames(30);
    let from_frames = Timecode::from_frames(total_frames, 30, false);
    assert_eq!(tc, from_frames);
}

#[tokio::test]
async fn test_discovery_service() {
    use oximedia_videoip::discovery::*;
    use oximedia_videoip::types::*;

    let mut server = DiscoveryServer::new().expect("test expectation failed");

    let video_format = VideoFormat::new(VideoCodec::Vp9, Resolution::HD_1080, FrameRate::FPS_30);
    let audio_format =
        AudioFormat::new(AudioCodec::Opus, 48000, 2).expect("audio_format should be valid");

    let result = server.announce("TestDiscovery", 5000, &video_format, &audio_format);

    // May fail in restricted environments, that's OK
    if result.is_ok() {
        server
            .stop_announce()
            .expect("stop_announce should succeed");
    }
}

#[test]
fn test_video_format_bitrate() {
    use oximedia_videoip::types::*;

    let format = VideoFormat::new(VideoCodec::Uyvy, Resolution::HD_1080, FrameRate::FPS_60);

    let bitrate = format
        .uncompressed_bitrate()
        .expect("bitrate should be valid");
    assert!(bitrate > 0);
}

#[test]
fn test_audio_format_bitrate() {
    use oximedia_videoip::types::*;

    let format = AudioFormat::new(AudioCodec::Pcm16, 48000, 2).expect("format should be valid");
    let bitrate = format
        .uncompressed_bitrate()
        .expect("bitrate should be valid");
    assert_eq!(bitrate, 48000 * 2 * 16);
}

#[test]
fn test_stream_type_ids() {
    use oximedia_videoip::types::StreamType;

    assert_eq!(StreamType::Program.to_id(), 0);
    assert_eq!(StreamType::Preview.to_id(), 1);
    assert_eq!(StreamType::Alpha.to_id(), 2);

    assert_eq!(StreamType::from_id(0), StreamType::Program);
    assert_eq!(StreamType::from_id(1), StreamType::Preview);
    assert_eq!(StreamType::from_id(2), StreamType::Alpha);
}

#[test]
fn test_packet_header_validation() {
    use oximedia_videoip::packet::*;

    let header = PacketHeader::new(PacketFlags::VIDEO, 0, 0, types::StreamType::Program, 100);

    assert!(header.validate().is_ok());

    // Test invalid payload size
    let mut bad_header = header.clone();
    bad_header.payload_size = (MAX_PAYLOAD_SIZE + 1) as u16;
    assert!(bad_header.validate().is_err());

    // Test invalid magic
    let mut bad_magic = header.clone();
    bad_magic.magic = 0xDEADBEEF;
    assert!(bad_magic.validate().is_err());
}

#[test]
fn test_fec_encoder_ratios() {
    use oximedia_videoip::fec::FecEncoder;

    let encoder_5 = FecEncoder::with_ratio(0.05).expect("encoder_5 should be valid");
    let encoder_10 = FecEncoder::with_ratio(0.10).expect("encoder_10 should be valid");
    let encoder_20 = FecEncoder::with_ratio(0.20).expect("encoder_20 should be valid");

    assert!(encoder_5.parity_shards() == 1);
    assert!(encoder_10.parity_shards() == 2);
    assert!(encoder_20.parity_shards() == 4);
}

#[test]
fn test_resolution_constants() {
    use oximedia_videoip::types::Resolution;

    assert_eq!(Resolution::SD.pixel_count(), 640 * 480);
    assert_eq!(Resolution::HD_1080.pixel_count(), 1920 * 1080);
    assert_eq!(Resolution::UHD_4K.pixel_count(), 3840 * 2160);
}

#[test]
fn test_framerate_common_values() {
    use oximedia_videoip::types::FrameRate;

    let fps_60 = FrameRate::FPS_60;
    assert_eq!(fps_60.to_float(), 60.0);

    let fps_29_97 = FrameRate::FPS_29_97;
    assert!((fps_29_97.to_float() - 29.97).abs() < 0.01);

    let fps_23_976 = FrameRate::FPS_23_976;
    assert!((fps_23_976.to_float() - 23.976).abs() < 0.01);
}

#[test]
fn test_color_space() {
    use oximedia_videoip::types::ColorSpace;

    let bt709 = ColorSpace::Bt709;
    let bt2020 = ColorSpace::Bt2020;

    assert_ne!(bt709, bt2020);
}

#[test]
fn test_codec_properties() {
    use oximedia_videoip::types::*;

    assert!(VideoCodec::Vp9.is_compressed());
    assert!(!VideoCodec::V210.is_compressed());

    assert!(AudioCodec::Opus.is_compressed());
    assert!(!AudioCodec::Pcm16.is_compressed());

    assert_eq!(VideoCodec::Uyvy.bytes_per_pixel(), Some(2.0));
    assert_eq!(AudioCodec::Pcm16.bytes_per_sample(), Some(2));
}

#[test]
fn test_metadata_types() {
    use oximedia_videoip::metadata::*;

    assert_eq!(MetadataType::Timecode.to_u8(), 0);
    assert_eq!(MetadataType::from_u8(0), MetadataType::Timecode);

    assert_eq!(MetadataType::ClosedCaptions.to_u8(), 1);
    assert_eq!(MetadataType::from_u8(1), MetadataType::ClosedCaptions);
}

#[test]
fn test_afd_values() {
    use oximedia_videoip::metadata::Afd;

    assert_eq!(Afd::Box16x9 as u8, 10);
    assert_eq!(Afd::Letterbox16x9 as u8, 13);
}

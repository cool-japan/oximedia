//! Simple video sender example.

use bytes::Bytes;
use oximedia_videoip::codec::{AudioSamples, VideoFrame};
use oximedia_videoip::types::{AudioCodec, VideoCodec};
use oximedia_videoip::{AudioConfig, VideoConfig, VideoIpSource};
use std::time::Duration;
use tokio::time::interval;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create video and audio configuration
    // UYVY and PCM are the formats this crate can genuinely put on the
    // wire: it has no working VP8/VP9/AV1 or Opus encoder, and
    // VideoIpSource::new refuses those rather than shipping raw
    // frames labelled as a compressed bitstream.
    let video_config = VideoConfig::new(1920, 1080, 30.0)?.with_codec(VideoCodec::Uyvy);
    let audio_config = AudioConfig::new(48000, 2)?.with_codec(AudioCodec::Pcm16)?;

    // Create source
    let mut source = VideoIpSource::new("Example Camera", video_config, audio_config).await?;

    // Start broadcasting via mDNS
    source.start_broadcasting()?;

    // Enable FEC with 10% overhead
    source.enable_fec(0.1)?;

    println!("Broadcasting on port: {}", source.local_addr().port());
    println!("Service name: Example Camera");
    println!("Press Ctrl+C to stop");

    // Generate and send test frames
    let mut frame_count = 0u64;
    let mut frame_interval = interval(Duration::from_millis(33)); // ~30fps

    loop {
        frame_interval.tick().await;

        // Generate test pattern (black frame)
        let frame_size = 1920 * 1080 * 2; // UYVY format
        let video_data = vec![0u8; frame_size];

        let video_frame = VideoFrame::new(
            Bytes::from(video_data),
            1920,
            1080,
            frame_count % 30 == 0, // Keyframe every 30 frames
            frame_count * 33_333,  // PTS in microseconds
        );

        // Generate silent audio
        let audio_size = 1600 * 2 * 2; // 1600 samples * 2 channels * 2 bytes
        let audio_data = vec![0u8; audio_size];

        let audio_samples = AudioSamples::new(
            Bytes::from(audio_data),
            1600,
            2,
            48000,
            frame_count * 33_333,
        );

        // Send frame
        source.send_frame(video_frame, Some(audio_samples)).await?;

        frame_count += 1;

        if frame_count % 30 == 0 {
            let stats = source.stats();
            println!(
                "Sent {} frames, {} packets, {} bytes",
                frame_count, stats.packets_sent, stats.bytes_sent
            );
        }
    }
}

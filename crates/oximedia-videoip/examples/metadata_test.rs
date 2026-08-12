//! Metadata and timecode example.

use oximedia_videoip::metadata::{CaptionType, ClosedCaptionData, MetadataPacket, Timecode};
use oximedia_videoip::source::ControlMessage;
use oximedia_videoip::types::{AudioCodec, VideoCodec};
use oximedia_videoip::{AudioConfig, VideoConfig, VideoIpSource};
use std::time::Duration;
use tokio::time::interval;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // UYVY + PCM: the formats this crate can actually put on the wire (it has
    // no working VP8/VP9/AV1 or Opus encoder, and VideoIpSource::new refuses
    // those rather than shipping raw frames labelled as compressed).
    let video_config = VideoConfig::new(1920, 1080, 30.0)?.with_codec(VideoCodec::Uyvy);
    let audio_config = AudioConfig::new(48000, 2)?.with_codec(AudioCodec::Pcm16)?;

    let source = VideoIpSource::new("Metadata Test", video_config, audio_config).await?;
    let control_tx = source.control_sender();

    println!("Metadata and Timecode Test");
    println!("==========================");
    println!();
    println!("Sending timecode and metadata...");
    println!();

    let mut frame_count = 0u32;
    let mut metadata_interval = interval(Duration::from_millis(33)); // 30fps

    for _ in 0..300 {
        // Send for 10 seconds
        metadata_interval.tick().await;

        // Calculate timecode
        let total_frames = frame_count;
        let fps = 30;
        let timecode = Timecode::from_frames(total_frames, fps, false);

        println!(
            "Frame {}: Timecode: {} ({} frames)",
            frame_count, timecode, total_frames
        );

        // Send timecode metadata
        let tc_packet = MetadataPacket::timecode(timecode);
        control_tx.send(ControlMessage::Metadata(tc_packet)).await?;

        // Every 5 seconds, send closed caption metadata
        if frame_count % (fps as u32 * 5) == 0 {
            let caption_data = ClosedCaptionData {
                data: b"Test Caption Data".to_vec(),
                caption_type: CaptionType::Cea608,
            };

            let cc_packet = MetadataPacket::closed_caption(caption_data);
            control_tx.send(ControlMessage::Metadata(cc_packet)).await?;

            println!("  -> Sent closed caption metadata");
        }

        frame_count += 1;
    }

    println!();
    println!("Test completed!");

    Ok(())
}

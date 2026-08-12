//! Simple video receiver example.

use oximedia_videoip::types::{
    AudioCodec, AudioFormat, FrameRate, Resolution, VideoCodec, VideoFormat,
};
use oximedia_videoip::VideoIpReceiver;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Discovering sources...");

    // Discover source by name
    let mut receiver = match VideoIpReceiver::discover("Example Camera").await {
        Ok(r) => r,
        Err(_) => {
            println!("Source not found, using default address");
            // The full formats are needed, not just the codecs: an
            // uncompressed stream carries no geometry and PCM carries no
            // sample count, so the receiver would otherwise have to guess.
            let video_format =
                VideoFormat::new(VideoCodec::Vp9, Resolution::HD_1080, FrameRate::FPS_30);
            let audio_format = AudioFormat::new(AudioCodec::Pcm16, 48000, 2)?;
            VideoIpReceiver::connect("127.0.0.1:5000".parse()?, &video_format, &audio_format)
                .await?
        }
    };

    println!("Connected to source");

    // Enable FEC
    receiver.enable_fec(20, 2)?;

    // Start receiving
    receiver.start_receiving();

    let mut frame_count = 0u64;

    loop {
        match receiver.receive_frame().await {
            Ok((video_frame, audio_samples)) => {
                frame_count += 1;

                println!(
                    "Received frame {}: {}x{}, keyframe: {}, PTS: {}",
                    frame_count,
                    video_frame.width,
                    video_frame.height,
                    video_frame.is_keyframe,
                    video_frame.pts
                );

                if let Some(audio) = audio_samples {
                    println!(
                        "  Audio: {} samples, {} channels, {} Hz",
                        audio.sample_count, audio.channels, audio.sample_rate
                    );
                }

                if frame_count % 30 == 0 {
                    let stats = receiver.stats();
                    let jitter_stats = receiver.jitter_stats();

                    println!("\nStatistics:");
                    println!("  Packets received: {}", stats.packets_received);
                    println!("  Packets lost: {}", stats.packets_lost);
                    println!("  Loss rate: {:.2}%", stats.loss_rate * 100.0);
                    println!("  Jitter: {} us", stats.jitter_us);
                    println!("  Buffer occupancy: {}", jitter_stats.buffer_occupancy);
                    println!("  Health score: {:.2}", stats.health_score());
                    println!();

                    // Adjust jitter buffer dynamically
                    receiver.adjust_jitter_buffer();
                }
            }
            Err(e) => {
                eprintln!("Error receiving frame: {}", e);
            }
        }
    }
}

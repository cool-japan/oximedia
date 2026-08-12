//! Real-time statistics monitoring example.

use oximedia_videoip::types::{
    AudioCodec, AudioFormat, FrameRate, Resolution, VideoCodec, VideoFormat,
};
use oximedia_videoip::VideoIpReceiver;
use std::time::Duration;
use tokio::time::interval;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Video Stream Statistics Monitor");
    println!("================================");
    println!();

    let mut receiver = match VideoIpReceiver::discover("Example Camera").await {
        Ok(r) => r,
        Err(_) => {
            println!("Using default address 127.0.0.1:5000");
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

    receiver.enable_fec(20, 2)?;
    receiver.start_receiving();

    let mut stats_interval = interval(Duration::from_secs(1));
    let mut frame_count = 0u64;
    let mut last_frame_count = 0u64;

    loop {
        tokio::select! {
            result = receiver.receive_frame() => {
                match result {
                    Ok((video_frame, _audio)) => {
                        frame_count += 1;

                        if video_frame.is_keyframe {
                            print!("K");
                        } else {
                            print!(".");
                        }

                        if frame_count % 60 == 0 {
                            println!();
                        }

                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                    Err(e) => {
                        eprintln!("\nError: {}", e);
                    }
                }
            }

            _ = stats_interval.tick() => {
                let stats = receiver.stats();
                let jitter_stats = receiver.jitter_stats();
                let fps = frame_count - last_frame_count;
                last_frame_count = frame_count;

                println!();
                println!("┌─────────────────────────────────────────────────────────┐");
                println!("│ Stream Statistics                                       │");
                println!("├─────────────────────────────────────────────────────────┤");
                println!("│ Frames Received      : {:>10}                     │", frame_count);
                println!("│ Current FPS          : {:>10}                     │", fps);
                println!("│ Total Packets        : {:>10}                     │", stats.packets_received);
                println!("│ Packets Lost         : {:>10} ({:>5.2}%)           │",
                    stats.packets_lost,
                    stats.loss_rate * 100.0
                );
                println!("│ Packets Recovered    : {:>10}                     │", stats.packets_recovered);
                println!("│ Out of Order         : {:>10}                     │", stats.packets_out_of_order);
                println!("│ Duplicates           : {:>10}                     │", stats.packets_duplicate);
                println!("├─────────────────────────────────────────────────────────┤");
                println!("│ Bitrate              : {:>10.2} Mbps                │",
                    stats.current_bitrate as f64 / 1_000_000.0
                );
                println!("│ Data Received        : {:>10.2} MB                  │",
                    stats.bytes_received as f64 / 1_000_000.0
                );
                println!("├─────────────────────────────────────────────────────────┤");
                println!("│ Jitter               : {:>10} us                   │", stats.jitter_us);
                println!("│ Avg RTT              : {:>10} us                   │", stats.avg_rtt_us);
                println!("│ Buffer Occupancy     : {:>10}                     │", jitter_stats.buffer_occupancy);
                println!("├─────────────────────────────────────────────────────────┤");
                println!("│ Health Score         : {:>10.1}%                    │",
                    stats.health_score() * 100.0
                );
                println!("│ Stream Health        : {:>10}                     │",
                    if stats.is_healthy() { "GOOD" } else { "POOR" }
                );
                println!("└─────────────────────────────────────────────────────────┘");
                println!();

                // Adjust jitter buffer based on conditions
                receiver.adjust_jitter_buffer();
            }
        }
    }
}

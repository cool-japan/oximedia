//! Multicast streaming example for broadcasting to multiple receivers.

use bytes::Bytes;
use oximedia_videoip::codec::{AudioSamples, VideoFrame};
use oximedia_videoip::types::{AudioCodec, VideoCodec};
use oximedia_videoip::{AudioConfig, VideoConfig, VideoIpSource};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::interval;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // UYVY and PCM are the formats this crate can genuinely put on the
    // wire: it has no working VP8/VP9/AV1 or Opus encoder, and
    // VideoIpSource::new refuses those rather than shipping raw
    // frames labelled as a compressed bitstream.
    let video_config = VideoConfig::new(1920, 1080, 60.0)?.with_codec(VideoCodec::Uyvy);
    let audio_config = AudioConfig::new(48000, 2)?.with_codec(AudioCodec::Pcm16)?;

    let mut source = VideoIpSource::new("Multicast Source", video_config, audio_config).await?;

    source.start_broadcasting()?;
    source.enable_fec(0.15)?; // 15% FEC for better multicast reliability

    println!("Multicast Video Streaming");
    println!("=========================");
    println!("Local address: {}", source.local_addr());
    println!();

    // Add multiple multicast destinations
    let multicast_addrs: Vec<SocketAddr> = vec![
        "239.0.0.1:5000".parse()?,
        "239.0.0.2:5000".parse()?,
        "239.0.0.3:5000".parse()?,
    ];

    for addr in &multicast_addrs {
        source.add_destination(*addr);
        println!("Broadcasting to multicast group: {}", addr);
    }

    println!();
    println!("Streaming at 60 fps...");
    println!("Press Ctrl+C to stop");
    println!();

    let mut frame_count = 0u64;
    let mut frame_interval = interval(Duration::from_micros(16_667)); // 60fps

    loop {
        frame_interval.tick().await;

        // Generate test frame with color bars pattern
        let frame_data = generate_color_bars(1920, 1080);

        let video_frame = VideoFrame::new(
            Bytes::from(frame_data),
            1920,
            1080,
            frame_count % 60 == 0, // Keyframe every second
            frame_count * 16_667,  // PTS in microseconds
        );

        // Generate audio tone
        let audio_data = generate_audio_tone(1600, 48000, frame_count);

        let audio_samples = AudioSamples::new(
            Bytes::from(audio_data),
            1600,
            2,
            48000,
            frame_count * 16_667,
        );

        source.send_frame(video_frame, Some(audio_samples)).await?;

        frame_count += 1;

        if frame_count % 60 == 0 {
            let stats = source.stats();
            println!(
                "[{}s] Sent {} frames | Bitrate: {:.2} Mbps | Packets: {} | Loss: {:.2}%",
                frame_count / 60,
                frame_count,
                stats.current_bitrate as f64 / 1_000_000.0,
                stats.packets_sent,
                stats.loss_rate * 100.0
            );
        }
    }
}

/// Generates a UYVY color bars test pattern.
fn generate_color_bars(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 2) as usize);

    // SMPTE color bars (simplified)
    let bars = [
        (180, 128, 128), // White
        (162, 44, 142),  // Yellow
        (131, 156, 44),  // Cyan
        (112, 72, 58),   // Green
        (84, 184, 198),  // Magenta
        (65, 100, 212),  // Red
        (35, 212, 114),  // Blue
        (16, 128, 128),  // Black
    ];

    let bar_width = width / 8;

    for _y in 0..height {
        for x in 0..width {
            let bar_index = (x / bar_width).min(7) as usize;
            let (y_val, u_val, v_val) = bars[bar_index];

            // UYVY packing: U Y V Y
            if x % 2 == 0 {
                data.push(u_val);
                data.push(y_val);
            } else {
                data.push(v_val);
                data.push(y_val);
            }
        }
    }

    data
}

/// Generates a simple audio tone (sine wave).
fn generate_audio_tone(samples: usize, sample_rate: u32, frame_num: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples * 2 * 2); // stereo PCM16
    let frequency = 440.0; // A4 note
    let amplitude = 0.3;

    for i in 0..samples {
        let t = (frame_num * samples as u64 + i as u64) as f32 / sample_rate as f32;
        let sample =
            (amplitude * (2.0 * std::f32::consts::PI * frequency * t).sin() * 32767.0) as i16;

        // Stereo: same sample for left and right
        for _ in 0..2 {
            data.push((sample & 0xFF) as u8);
            data.push((sample >> 8) as u8);
        }
    }

    data
}

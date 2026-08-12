//! PTZ camera control example.

use oximedia_videoip::ptz::{PtzCommand, PtzMessage};
use oximedia_videoip::source::ControlMessage;
use oximedia_videoip::types::{AudioCodec, VideoCodec};
use oximedia_videoip::{AudioConfig, VideoConfig, VideoIpSource};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // UYVY + PCM: the formats this crate can actually put on the wire (it has
    // no working VP8/VP9/AV1 or Opus encoder, and VideoIpSource::new refuses
    // those rather than shipping raw frames labelled as compressed).
    let video_config = VideoConfig::new(1920, 1080, 30.0)?.with_codec(VideoCodec::Uyvy);
    let audio_config = AudioConfig::new(48000, 2)?.with_codec(AudioCodec::Pcm16)?;

    let source = VideoIpSource::new("PTZ Camera", video_config, audio_config).await?;
    let control_tx = source.control_sender();

    println!("PTZ Camera Control");
    println!("==================");
    println!();
    println!("Commands:");
    println!("  w/s - Tilt up/down");
    println!("  a/d - Pan left/right");
    println!("  z/x - Zoom in/out");
    println!("  h   - Home position");
    println!("  1-9 - Recall preset");
    println!("  q   - Quit");
    println!();

    let mut pan = 0.0f32;
    let mut tilt = 0.0f32;
    let mut zoom = 0.5f32;

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let cmd = input.trim().to_lowercase();

        let ptz_msg = match cmd.as_str() {
            "w" => {
                tilt = (tilt + 5.0).min(90.0);
                Some(
                    PtzMessage::new(1, PtzCommand::TiltUp)
                        .with_position(pan, tilt, zoom)
                        .with_tilt_speed(50),
                )
            }
            "s" => {
                tilt = (tilt - 5.0).max(-90.0);
                Some(
                    PtzMessage::new(1, PtzCommand::TiltDown)
                        .with_position(pan, tilt, zoom)
                        .with_tilt_speed(-50),
                )
            }
            "a" => {
                pan = (pan - 5.0).max(-180.0);
                Some(
                    PtzMessage::new(1, PtzCommand::PanLeft)
                        .with_position(pan, tilt, zoom)
                        .with_pan_speed(-50),
                )
            }
            "d" => {
                pan = (pan + 5.0).min(180.0);
                Some(
                    PtzMessage::new(1, PtzCommand::PanRight)
                        .with_position(pan, tilt, zoom)
                        .with_pan_speed(50),
                )
            }
            "z" => {
                zoom = (zoom + 0.1).min(1.0);
                Some(
                    PtzMessage::new(1, PtzCommand::ZoomIn)
                        .with_position(pan, tilt, zoom)
                        .with_zoom_speed(50),
                )
            }
            "x" => {
                zoom = (zoom - 0.1).max(0.0);
                Some(
                    PtzMessage::new(1, PtzCommand::ZoomOut)
                        .with_position(pan, tilt, zoom)
                        .with_zoom_speed(-50),
                )
            }
            "h" => {
                pan = 0.0;
                tilt = 0.0;
                zoom = 0.5;
                Some(PtzMessage::new(1, PtzCommand::Home).with_position(pan, tilt, zoom))
            }
            preset
                if preset.len() == 1
                    && preset
                        .chars()
                        .next()
                        .expect("next should succeed")
                        .is_ascii_digit() =>
            {
                let preset_num = preset.parse::<u8>().expect("preset_num should be valid");
                Some(PtzMessage::new(1, PtzCommand::GotoPreset).with_preset(preset_num))
            }
            "q" => break,
            _ => {
                println!("Unknown command");
                None
            }
        };

        if let Some(msg) = ptz_msg {
            control_tx.send(ControlMessage::Ptz(msg)).await?;
            println!(
                "Position: Pan={:.1}°, Tilt={:.1}°, Zoom={:.2}",
                pan, tilt, zoom
            );
        }
    }

    Ok(())
}

//! Tally light control example for video switchers.

use oximedia_videoip::source::ControlMessage;
use oximedia_videoip::tally::{TallyController, TallyMessage, TallyState};
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

    let source = VideoIpSource::new("Switcher Control", video_config, audio_config).await?;
    let control_tx = source.control_sender();

    let mut tally_controller = TallyController::new();

    println!("Tally Light Control System");
    println!("===========================");
    println!();
    println!("Commands:");
    println!("  p<N> - Set camera N to Program (red)");
    println!("  v<N> - Set camera N to Preview (green)");
    println!("  c<N> - Clear camera N tally");
    println!("  s    - Show all tallies");
    println!("  q    - Quit");
    println!();
    println!("Example: p1 (sets camera 1 to program)");
    println!();

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let cmd = input.trim().to_lowercase();

        if cmd.is_empty() {
            continue;
        }

        match cmd.chars().next().expect("next should succeed") {
            'p' => {
                if let Ok(camera_id) = cmd[1..].parse::<u16>() {
                    tally_controller.set_state(camera_id, TallyState::Program);
                    let msg = tally_controller.create_message(camera_id, 255);
                    control_tx.send(ControlMessage::Tally(msg)).await?;
                    println!("Camera {} set to PROGRAM (red)", camera_id);
                } else {
                    println!("Invalid camera ID");
                }
            }
            'v' => {
                if let Ok(camera_id) = cmd[1..].parse::<u16>() {
                    tally_controller.set_state(camera_id, TallyState::Preview);
                    let msg = tally_controller.create_message(camera_id, 255);
                    control_tx.send(ControlMessage::Tally(msg)).await?;
                    println!("Camera {} set to PREVIEW (green)", camera_id);
                } else {
                    println!("Invalid camera ID");
                }
            }
            'c' => {
                if let Ok(camera_id) = cmd[1..].parse::<u16>() {
                    tally_controller.clear_state(camera_id);
                    let msg = TallyMessage::new(camera_id, TallyState::Off, 0);
                    control_tx.send(ControlMessage::Tally(msg)).await?;
                    println!("Camera {} tally cleared", camera_id);
                } else {
                    println!("Invalid camera ID");
                }
            }
            's' => {
                println!("\nActive Tallies:");
                let active = tally_controller.active_tallies();
                if active.is_empty() {
                    println!("  (none)");
                } else {
                    for (camera_id, state) in active {
                        let state_str = match state {
                            TallyState::Program => "PROGRAM (red)",
                            TallyState::Preview => "PREVIEW (green)",
                            TallyState::Both => "BOTH (red+green)",
                            TallyState::Off => "OFF",
                        };
                        println!("  Camera {}: {}", camera_id, state_str);
                    }
                }
                println!();
            }
            'q' => break,
            _ => {
                println!("Unknown command");
            }
        }
    }

    Ok(())
}

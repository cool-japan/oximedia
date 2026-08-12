//! Broadcast graphics CLI commands for OxiMedia.
//!
//! Provides lower-third, ticker, overlay, and template rendering commands.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Graphics command subcommands.
#[derive(Subcommand, Debug)]
pub enum GraphicsCommand {
    /// Render a lower-third graphic
    LowerThird {
        /// Output file path for rendered RGBA data
        #[arg(short, long)]
        output: PathBuf,

        /// Primary title text
        #[arg(long)]
        title: String,

        /// Subtitle text
        #[arg(long)]
        subtitle: Option<String>,

        /// Output width in pixels
        #[arg(long, default_value = "1920")]
        width: u32,

        /// Output height in pixels
        #[arg(long, default_value = "1080")]
        height: u32,

        /// Background color as hex (e.g., "000000C8")
        #[arg(long)]
        bg_color: Option<String>,

        /// Text color as hex (e.g., "FFFFFF")
        #[arg(long)]
        text_color: Option<String>,

        /// Duration in seconds for the animation
        #[arg(long)]
        duration: Option<f64>,

        /// Style: classic, modern, minimal, news, sports, corporate
        #[arg(long, default_value = "classic")]
        style: String,
    },

    /// Render a scrolling ticker strip
    Ticker {
        /// Output file path for rendered RGBA data
        #[arg(short, long)]
        output: PathBuf,

        /// Ticker text content
        #[arg(long)]
        text: String,

        /// Output width in pixels
        #[arg(long, default_value = "1920")]
        width: u32,

        /// Height of the ticker strip in pixels
        #[arg(long, default_value = "100")]
        height: u32,

        /// Scroll speed in pixels per second
        #[arg(long)]
        speed: Option<f64>,

        /// Background color as hex (e.g., "141450E6")
        #[arg(long)]
        bg_color: Option<String>,

        /// Text color as hex (e.g., "FFFFFF")
        #[arg(long)]
        text_color: Option<String>,
    },

    /// Composite a graphics overlay onto a base image
    Overlay {
        /// Input base image file (RGBA raw data)
        #[arg(short, long)]
        input: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Overlay image file (RGBA raw data)
        #[arg(long)]
        overlay: PathBuf,

        /// Overlay X position
        #[arg(long, default_value = "0")]
        x: i32,

        /// Overlay Y position
        #[arg(long, default_value = "0")]
        y: i32,

        /// Overlay opacity (0.0-1.0)
        #[arg(long, default_value = "1.0")]
        opacity: f64,

        /// Base image width
        #[arg(long)]
        width: u32,

        /// Base image height
        #[arg(long)]
        height: u32,
    },

    /// Render a named template with parameters
    Template {
        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Template name (lower_third, full_screen_title, bug, watermark, color_bars)
        #[arg(long)]
        template: String,

        /// JSON string of template parameters
        #[arg(long)]
        params: Option<String>,

        /// Output width in pixels
        #[arg(long, default_value = "1920")]
        width: u32,

        /// Output height in pixels
        #[arg(long, default_value = "1080")]
        height: u32,
    },

    /// List available graphics templates
    ListTemplates {},
}

/// Handle graphics command dispatch.
pub async fn handle_graphics_command(command: GraphicsCommand, json_output: bool) -> Result<()> {
    match command {
        GraphicsCommand::LowerThird {
            output,
            title,
            subtitle,
            width,
            height,
            bg_color,
            text_color,
            duration,
            style,
        } => {
            render_lower_third(
                &output,
                &title,
                subtitle.as_deref(),
                width,
                height,
                bg_color.as_deref(),
                text_color.as_deref(),
                duration,
                &style,
                json_output,
            )
            .await
        }
        GraphicsCommand::Ticker {
            output,
            text,
            width,
            height,
            speed,
            bg_color,
            text_color,
        } => {
            render_ticker(
                &output,
                &text,
                width,
                height,
                speed,
                bg_color.as_deref(),
                text_color.as_deref(),
                json_output,
            )
            .await
        }
        GraphicsCommand::Overlay {
            input,
            output,
            overlay,
            x,
            y,
            opacity,
            width,
            height,
        } => {
            render_overlay(
                &input,
                &output,
                &overlay,
                x,
                y,
                opacity,
                width,
                height,
                json_output,
            )
            .await
        }
        GraphicsCommand::Template {
            output,
            template,
            params,
            width,
            height,
        } => {
            render_template(
                &output,
                &template,
                params.as_deref(),
                width,
                height,
                json_output,
            )
            .await
        }
        GraphicsCommand::ListTemplates {} => list_templates(json_output).await,
    }
}

/// Parse a hex color string into an RGBA array.
fn parse_hex_color(hex: &str) -> Result<[u8; 4]> {
    let hex = hex.trim_start_matches('#');
    match hex.len() {
        6 => {
            let r =
                u8::from_str_radix(&hex[0..2], 16).context("Invalid red component in hex color")?;
            let g = u8::from_str_radix(&hex[2..4], 16)
                .context("Invalid green component in hex color")?;
            let b = u8::from_str_radix(&hex[4..6], 16)
                .context("Invalid blue component in hex color")?;
            Ok([r, g, b, 255])
        }
        8 => {
            let r =
                u8::from_str_radix(&hex[0..2], 16).context("Invalid red component in hex color")?;
            let g = u8::from_str_radix(&hex[2..4], 16)
                .context("Invalid green component in hex color")?;
            let b = u8::from_str_radix(&hex[4..6], 16)
                .context("Invalid blue component in hex color")?;
            let a = u8::from_str_radix(&hex[6..8], 16)
                .context("Invalid alpha component in hex color")?;
            Ok([r, g, b, a])
        }
        _ => Err(anyhow::anyhow!(
            "Invalid hex color '{}': expected 6 or 8 hex characters",
            hex
        )),
    }
}

/// Parse a style string into a `LowerThirdStyle`.
fn parse_lower_third_style(style: &str) -> Result<oximedia_graphics::lower_third::LowerThirdStyle> {
    match style {
        "classic" => Ok(oximedia_graphics::lower_third::LowerThirdStyle::Classic),
        "modern" => Ok(oximedia_graphics::lower_third::LowerThirdStyle::Modern),
        "minimal" => Ok(oximedia_graphics::lower_third::LowerThirdStyle::Minimal),
        "news" => Ok(oximedia_graphics::lower_third::LowerThirdStyle::News),
        "sports" => Ok(oximedia_graphics::lower_third::LowerThirdStyle::Sports),
        "corporate" => Ok(oximedia_graphics::lower_third::LowerThirdStyle::Corporate),
        other => Err(anyhow::anyhow!(
            "Unknown lower-third style '{}'. Expected: classic, modern, minimal, news, sports, corporate",
            other
        )),
    }
}

/// Render a lower-third graphic.
async fn render_lower_third(
    output: &PathBuf,
    title: &str,
    subtitle: Option<&str>,
    width: u32,
    height: u32,
    bg_color: Option<&str>,
    text_color: Option<&str>,
    duration: Option<f64>,
    style: &str,
    json_output: bool,
) -> Result<()> {
    let parsed_style = parse_lower_third_style(style)?;

    let mut config = oximedia_graphics::lower_third::LowerThirdConfig {
        name: title.to_string(),
        title: subtitle.unwrap_or("").to_string(),
        subtitle: subtitle.map(String::from),
        style: parsed_style,
        ..oximedia_graphics::lower_third::LowerThirdConfig::default()
    };

    if let Some(bg) = bg_color {
        config.background_color = parse_hex_color(bg)?;
    }
    if let Some(tc) = text_color {
        config.text_color = parse_hex_color(tc)?;
    }

    let dur_secs = duration.unwrap_or(3.0);
    let total_frames = (dur_secs * 30.0) as u32;
    let frame_idx = total_frames / 2; // Render the hold frame (middle)

    let rgba_data = oximedia_graphics::lower_third::LowerThirdRenderer::render(
        &config,
        frame_idx,
        total_frames,
        width,
        height,
    );

    tokio::fs::write(output, &rgba_data)
        .await
        .context("Failed to write output file")?;

    if json_output {
        let result = serde_json::json!({
            "output": output.display().to_string(),
            "width": width,
            "height": height,
            "format": "rgba",
            "bytes": rgba_data.len(),
            "title": title,
            "subtitle": subtitle,
            "style": style,
            "duration_secs": dur_secs,
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Lower-Third Rendered".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}x{}", "Dimensions:", width, height);
        println!("{:20} {}", "Title:", title);
        if let Some(sub) = subtitle {
            println!("{:20} {}", "Subtitle:", sub);
        }
        println!("{:20} {}", "Style:", style);
        println!("{:20} {:.1}s", "Duration:", dur_secs);
        println!("{:20} {} bytes", "Size:", rgba_data.len());
    }

    Ok(())
}

/// Render a ticker strip.
async fn render_ticker(
    output: &PathBuf,
    text: &str,
    width: u32,
    height: u32,
    speed: Option<f64>,
    bg_color: Option<&str>,
    text_color: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let mut config = oximedia_graphics::ticker::TickerConfig::default();
    config.height_px = height;

    if let Some(s) = speed {
        config.scroll_speed_pps = s as f32;
    }
    if let Some(bg) = bg_color {
        config.bg_color = parse_hex_color(bg)?;
    }
    if let Some(tc) = text_color {
        config.text_color = parse_hex_color(tc)?;
    }

    let item = oximedia_graphics::ticker::TickerItem::new(text, None, 128);
    let state = oximedia_graphics::ticker::TickerState::new(vec![item]);

    let rgba_data = oximedia_graphics::ticker::TickerRenderer::render(&state, &config, width);

    tokio::fs::write(output, &rgba_data)
        .await
        .context("Failed to write output file")?;

    if json_output {
        let result = serde_json::json!({
            "output": output.display().to_string(),
            "width": width,
            "height": height,
            "format": "rgba",
            "bytes": rgba_data.len(),
            "text": text,
            "speed_pps": config.scroll_speed_pps,
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Ticker Rendered".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}x{}", "Dimensions:", width, height);
        println!("{:20} {}", "Text:", text);
        println!("{:20} {} px/s", "Speed:", config.scroll_speed_pps);
        println!("{:20} {} bytes", "Size:", rgba_data.len());
    }

    Ok(())
}

/// Composite an overlay onto a base image.
async fn render_overlay(
    input: &PathBuf,
    output: &PathBuf,
    overlay_path: &PathBuf,
    x: i32,
    y: i32,
    opacity: f64,
    width: u32,
    height: u32,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }
    if !overlay_path.exists() {
        return Err(anyhow::anyhow!(
            "Overlay file not found: {}",
            overlay_path.display()
        ));
    }

    let mut base_data = tokio::fs::read(input)
        .await
        .context("Failed to read base image")?;
    let overlay_data = tokio::fs::read(overlay_path)
        .await
        .context("Failed to read overlay image")?;

    let expected_base = (width as usize) * (height as usize) * 4;
    if base_data.len() < expected_base {
        return Err(anyhow::anyhow!(
            "Base image data too small: expected {} bytes for {}x{} RGBA, got {}",
            expected_base,
            width,
            height,
            base_data.len()
        ));
    }

    // Perform alpha-blended overlay compositing
    let ov_pixel_count = overlay_data.len() / 4;
    // Infer overlay dimensions: assume square-ish or use full width
    let ov_w = if ov_pixel_count > 0 {
        let sqrt = (ov_pixel_count as f64).sqrt() as u32;
        if sqrt > 0 {
            sqrt
        } else {
            1
        }
    } else {
        return Err(anyhow::anyhow!("Overlay image is empty"));
    };
    let ov_h = (ov_pixel_count as u32).checked_div(ov_w).unwrap_or(1);

    let alpha = opacity.clamp(0.0, 1.0) as f32;

    for oy in 0..ov_h {
        for ox in 0..ov_w {
            let dst_x = ox as i32 + x;
            let dst_y = oy as i32 + y;

            if dst_x < 0 || dst_x >= width as i32 || dst_y < 0 || dst_y >= height as i32 {
                continue;
            }

            let src_idx = ((oy * ov_w + ox) * 4) as usize;
            let dst_idx = ((dst_y as u32 * width + dst_x as u32) * 4) as usize;

            if src_idx + 3 >= overlay_data.len() || dst_idx + 3 >= base_data.len() {
                continue;
            }

            let oa = (f32::from(overlay_data[src_idx + 3]) / 255.0) * alpha;
            let inv_a = 1.0 - oa;

            base_data[dst_idx] = (f32::from(overlay_data[src_idx]) * oa
                + f32::from(base_data[dst_idx]) * inv_a) as u8;
            base_data[dst_idx + 1] = (f32::from(overlay_data[src_idx + 1]) * oa
                + f32::from(base_data[dst_idx + 1]) * inv_a)
                as u8;
            base_data[dst_idx + 2] = (f32::from(overlay_data[src_idx + 2]) * oa
                + f32::from(base_data[dst_idx + 2]) * inv_a)
                as u8;
            base_data[dst_idx + 3] = 255;
        }
    }

    tokio::fs::write(output, &base_data)
        .await
        .context("Failed to write output file")?;

    if json_output {
        let result = serde_json::json!({
            "output": output.display().to_string(),
            "width": width,
            "height": height,
            "position": { "x": x, "y": y },
            "opacity": opacity,
            "bytes": base_data.len(),
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Overlay Composited".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Overlay:", overlay_path.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} ({}, {})", "Position:", x, y);
        println!("{:20} {:.2}", "Opacity:", opacity);
        println!("{:20} {} bytes", "Size:", base_data.len());
    }

    Ok(())
}

/// Render a named template.
async fn render_template(
    output: &PathBuf,
    template_name: &str,
    params_json: Option<&str>,
    width: u32,
    height: u32,
    json_output: bool,
) -> Result<()> {
    let params: HashMap<String, String> = if let Some(json) = params_json {
        serde_json::from_str(json).context("Failed to parse template params JSON")?
    } else {
        HashMap::new()
    };

    let rgba_data = match template_name {
        "lower_third" => {
            let title = params.get("title").map_or("Title", |s| s.as_str());
            let subtitle = params.get("subtitle").map(|s| s.as_str());
            let mut config = oximedia_graphics::lower_third::LowerThirdConfig {
                name: title.to_string(),
                title: subtitle.unwrap_or("").to_string(),
                subtitle: subtitle.map(String::from),
                ..oximedia_graphics::lower_third::LowerThirdConfig::default()
            };
            if let Some(bg) = params.get("bg_color") {
                config.background_color = parse_hex_color(bg)?;
            }
            oximedia_graphics::lower_third::LowerThirdRenderer::render(
                &config, 45, 90, width, height,
            )
        }
        "full_screen_title" => render_full_screen_title(width, height, &params),
        "bug" => render_bug_template(width, height, &params),
        "watermark" => render_watermark_template(width, height, &params),
        "color_bars" => render_color_bars(width, height),
        other => {
            return Err(anyhow::anyhow!(
                "Unknown template '{}'. Use 'list-templates' to see available templates.",
                other
            ));
        }
    };

    tokio::fs::write(output, &rgba_data)
        .await
        .context("Failed to write output file")?;

    if json_output {
        let result = serde_json::json!({
            "output": output.display().to_string(),
            "template": template_name,
            "width": width,
            "height": height,
            "format": "rgba",
            "bytes": rgba_data.len(),
            "params": params,
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Template Rendered".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Template:", template_name);
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}x{}", "Dimensions:", width, height);
        println!("{:20} {} bytes", "Size:", rgba_data.len());
        if !params.is_empty() {
            let mut pairs: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            pairs.sort();
            println!("{:20} {}", "Parameters:", pairs.join(", "));
        }
    }

    Ok(())
}

/// List all available templates.
async fn list_templates(json_output: bool) -> Result<()> {
    let templates = vec![
        (
            "lower_third",
            "Classic broadcast lower-third with title and subtitle",
        ),
        (
            "full_screen_title",
            "Full-screen title card with centered text",
        ),
        ("bug", "Channel/network bug (corner logo placeholder)"),
        ("watermark", "Semi-transparent watermark overlay"),
        ("color_bars", "SMPTE-style color bars test pattern"),
    ];

    if json_output {
        let items: Vec<serde_json::Value> = templates
            .iter()
            .map(|(name, desc)| serde_json::json!({ "name": name, "description": desc }))
            .collect();
        let json_str =
            serde_json::to_string_pretty(&items).context("Failed to serialize template list")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Available Graphics Templates".green().bold());
        println!("{}", "=".repeat(60));
        for (name, desc) in &templates {
            println!("  {:20} {}", name.cyan(), desc);
        }
        println!();
        println!(
            "{}",
            "Use 'oximedia graphics template --template <name>' to render a template.".dimmed()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Template rendering helpers
// ---------------------------------------------------------------------------

/// Render a full-screen title card.
fn render_full_screen_title(width: u32, height: u32, params: &HashMap<String, String>) -> Vec<u8> {
    let size = (width as usize) * (height as usize) * 4;
    let mut data = vec![0u8; size];

    // Parse background color or use default dark background
    let bg = params
        .get("bg_color")
        .and_then(|c| parse_hex_color(c).ok())
        .unwrap_or([20, 20, 30, 255]);

    // Fill background
    for chunk in data.chunks_exact_mut(4) {
        chunk[0] = bg[0];
        chunk[1] = bg[1];
        chunk[2] = bg[2];
        chunk[3] = bg[3];
    }

    // Draw a centered horizontal rule
    let rule_y = height / 2;
    let rule_thickness = 4u32;
    let margin = width / 8;
    let accent = params
        .get("accent_color")
        .and_then(|c| parse_hex_color(c).ok())
        .unwrap_or([255, 165, 0, 255]);

    for dy in 0..rule_thickness {
        let y = rule_y + dy;
        if y >= height {
            break;
        }
        for x in margin..(width - margin) {
            let idx = ((y * width + x) * 4) as usize;
            if idx + 3 < data.len() {
                data[idx] = accent[0];
                data[idx + 1] = accent[1];
                data[idx + 2] = accent[2];
                data[idx + 3] = accent[3];
            }
        }
    }

    data
}

/// Render a bug (corner logo placeholder).
fn render_bug_template(width: u32, height: u32, params: &HashMap<String, String>) -> Vec<u8> {
    let size = (width as usize) * (height as usize) * 4;
    let mut data = vec![0u8; size]; // Transparent background

    let bug_size = params
        .get("size")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(80);
    let margin = 40u32;
    let bug_color = params
        .get("color")
        .and_then(|c| parse_hex_color(c).ok())
        .unwrap_or([255, 255, 255, 180]);

    // Draw in top-right corner
    let start_x = width.saturating_sub(bug_size + margin);
    let start_y = margin;

    for dy in 0..bug_size {
        for dx in 0..bug_size {
            let x = start_x + dx;
            let y = start_y + dy;
            if x >= width || y >= height {
                continue;
            }

            // Draw a circle-ish shape
            let cx = bug_size as f32 / 2.0;
            let cy = bug_size as f32 / 2.0;
            let dist = ((dx as f32 - cx).powi(2) + (dy as f32 - cy).powi(2)).sqrt();
            if dist <= cx {
                let idx = ((y * width + x) * 4) as usize;
                if idx + 3 < data.len() {
                    data[idx] = bug_color[0];
                    data[idx + 1] = bug_color[1];
                    data[idx + 2] = bug_color[2];
                    data[idx + 3] = bug_color[3];
                }
            }
        }
    }

    data
}

/// Render a watermark template.
fn render_watermark_template(width: u32, height: u32, params: &HashMap<String, String>) -> Vec<u8> {
    let size = (width as usize) * (height as usize) * 4;
    let mut data = vec![0u8; size]; // Transparent background

    let watermark_alpha = params
        .get("alpha")
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(40);

    // Draw diagonal lines as a watermark pattern
    let spacing = 80i32;
    let line_color = [200u8, 200, 200, watermark_alpha];

    for y in 0..height {
        for x in 0..width {
            let diag = (x as i32 + y as i32) % spacing;
            if diag == 0 || diag == 1 {
                let idx = ((y * width + x) * 4) as usize;
                if idx + 3 < data.len() {
                    data[idx] = line_color[0];
                    data[idx + 1] = line_color[1];
                    data[idx + 2] = line_color[2];
                    data[idx + 3] = line_color[3];
                }
            }
        }
    }

    data
}

/// Render SMPTE-style color bars test pattern.
fn render_color_bars(width: u32, height: u32) -> Vec<u8> {
    let size = (width as usize) * (height as usize) * 4;
    let mut data = vec![0u8; size];

    // SMPTE color bars: white, yellow, cyan, green, magenta, red, blue
    let bars: [[u8; 3]; 7] = [
        [235, 235, 235], // white (75%)
        [235, 235, 16],  // yellow
        [16, 235, 235],  // cyan
        [16, 235, 16],   // green
        [235, 16, 235],  // magenta
        [235, 16, 16],   // red
        [16, 16, 235],   // blue
    ];

    let bar_width = width / 7;

    for y in 0..height {
        for x in 0..width {
            let bar_idx = ((x / bar_width) as usize).min(bars.len() - 1);
            let color = bars[bar_idx];
            let idx = ((y * width + x) * 4) as usize;
            if idx + 3 < data.len() {
                data[idx] = color[0];
                data[idx + 1] = color[1];
                data[idx + 2] = color[2];
                data[idx + 3] = 255;
            }
        }
    }

    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color_6() {
        let color = parse_hex_color("FF8800").expect("valid color");
        assert_eq!(color, [255, 136, 0, 255]);
    }

    #[test]
    fn test_parse_hex_color_8() {
        let color = parse_hex_color("#FF8800CC").expect("valid color");
        assert_eq!(color, [255, 136, 0, 204]);
    }

    #[test]
    fn test_parse_hex_color_invalid() {
        assert!(parse_hex_color("XYZ").is_err());
        assert!(parse_hex_color("12345").is_err());
    }

    #[test]
    fn test_parse_lower_third_style() {
        assert!(parse_lower_third_style("classic").is_ok());
        assert!(parse_lower_third_style("modern").is_ok());
        assert!(parse_lower_third_style("unknown").is_err());
    }

    #[test]
    fn test_render_color_bars() {
        let data = render_color_bars(140, 10);
        assert_eq!(data.len(), 140 * 10 * 4);
        // First pixel should be white-ish bar
        assert!(data[0] > 200);
        assert!(data[1] > 200);
        assert!(data[2] > 200);
        assert_eq!(data[3], 255);
    }

    #[test]
    fn test_render_full_screen_title() {
        let params = HashMap::new();
        let data = render_full_screen_title(100, 50, &params);
        assert_eq!(data.len(), 100 * 50 * 4);
    }

    #[test]
    fn test_render_bug_template() {
        let params = HashMap::new();
        let data = render_bug_template(200, 200, &params);
        assert_eq!(data.len(), 200 * 200 * 4);
    }

    #[test]
    fn test_render_watermark_template() {
        let params = HashMap::new();
        let data = render_watermark_template(100, 100, &params);
        assert_eq!(data.len(), 100 * 100 * 4);
    }
}

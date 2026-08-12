//! Image operations — read, convert, sequence, adjust, and histogram.
//!
//! Provides `oximedia image` with subcommands for professional image
//! sequence workflows (DPX, EXR, TIFF) used in cinema and VFX.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subcommands for `oximedia image`.
#[derive(Subcommand, Debug)]
pub enum ImageCommand {
    /// Read and display image information
    Read {
        /// Input image file
        #[arg(short, long)]
        input: PathBuf,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Convert image format (DPX, EXR, TIFF, PNG, JPEG, WebP)
    Convert {
        /// Input image file
        #[arg(short, long)]
        input: PathBuf,

        /// Output image file
        #[arg(short, long)]
        output: PathBuf,

        /// Target bit depth (8, 10, 12, 16, 32)
        #[arg(long)]
        bit_depth: Option<u32>,

        /// Target color space (linear, srgb, rec709, rec2020, dci-p3, log, luma)
        #[arg(long)]
        colorspace: Option<String>,

        /// Compression method (none, rle, zip, zips, piz, lzw, packbits)
        #[arg(long)]
        compression: Option<String>,

        /// JPEG output quality 1-100 (default: 95, only used for JPEG output)
        #[arg(long, default_value = "95")]
        quality: u8,
    },

    /// Process image sequence
    Sequence {
        /// Input pattern (e.g. "frame_%04d.exr" or "render.####.dpx")
        #[arg(short, long)]
        input: String,

        /// Start frame number
        #[arg(long)]
        start: Option<u32>,

        /// End frame number
        #[arg(long)]
        end: Option<u32>,

        /// Show sequence info only (no processing)
        #[arg(long)]
        info: bool,

        /// Output pattern for converted sequence
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Adjust image properties (brightness, contrast, saturation, gamma, exposure)
    Adjust {
        /// Input image file
        #[arg(short, long)]
        input: PathBuf,

        /// Output image file
        #[arg(short, long)]
        output: PathBuf,

        /// Brightness adjustment (-1.0 to 1.0)
        #[arg(long)]
        brightness: Option<f64>,

        /// Contrast adjustment (0.0 to 3.0, 1.0 = no change)
        #[arg(long)]
        contrast: Option<f64>,

        /// Saturation adjustment (0.0 to 3.0, 1.0 = no change)
        #[arg(long)]
        saturation: Option<f64>,

        /// Gamma correction (0.1 to 5.0, 1.0 = no change)
        #[arg(long)]
        gamma: Option<f64>,

        /// Exposure adjustment in stops (-5.0 to 5.0)
        #[arg(long)]
        exposure: Option<f64>,
    },

    /// Generate histogram from image
    Histogram {
        /// Input image file
        #[arg(short, long)]
        input: PathBuf,

        /// Output histogram image (optional, prints text if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Histogram mode: rgb, luma, per-channel
        #[arg(long, default_value = "rgb")]
        mode: String,

        /// Histogram image width
        #[arg(long)]
        width: Option<u32>,

        /// Histogram image height
        #[arg(long)]
        height: Option<u32>,
    },
}

/// Handle `oximedia image` subcommand dispatch.
pub async fn handle_image_command(command: ImageCommand, json_output: bool) -> Result<()> {
    match command {
        ImageCommand::Read { input, format } => read_image(&input, &format, json_output).await,
        ImageCommand::Convert {
            input,
            output,
            bit_depth,
            colorspace,
            compression,
            quality,
        } => convert_image(&input, &output, bit_depth, colorspace, compression, quality).await,
        ImageCommand::Sequence {
            input,
            start,
            end,
            info,
            output,
        } => process_sequence(&input, start, end, info, output, json_output).await,
        ImageCommand::Adjust {
            input,
            output,
            brightness,
            contrast,
            saturation,
            gamma,
            exposure,
        } => {
            adjust_image(
                &input, &output, brightness, contrast, saturation, gamma, exposure,
            )
            .await
        }
        ImageCommand::Histogram {
            input,
            output,
            mode,
            width,
            height,
        } => generate_histogram(&input, output, &mode, width, height, json_output).await,
    }
}

fn parse_colorspace(s: &str) -> Result<oximedia_image::ColorSpace> {
    match s.to_ascii_lowercase().as_str() {
        "linear" | "linear-rgb" | "linearrgb" => Ok(oximedia_image::ColorSpace::LinearRgb),
        "srgb" => Ok(oximedia_image::ColorSpace::Srgb),
        "rec709" | "bt709" => Ok(oximedia_image::ColorSpace::Rec709),
        "rec2020" | "bt2020" => Ok(oximedia_image::ColorSpace::Rec2020),
        "dci-p3" | "dcip3" | "p3" => Ok(oximedia_image::ColorSpace::DciP3),
        "log" | "logarithmic" => Ok(oximedia_image::ColorSpace::Log),
        "luma" | "gray" | "grayscale" => Ok(oximedia_image::ColorSpace::Luma),
        "ycbcr" | "yuv" => Ok(oximedia_image::ColorSpace::YCbCr),
        "cmyk" => Ok(oximedia_image::ColorSpace::Cmyk),
        other => Err(anyhow::anyhow!(
            "Unknown color space '{}'. Valid: linear, srgb, rec709, rec2020, dci-p3, log, luma, ycbcr, cmyk",
            other
        )),
    }
}

fn parse_compression(s: &str) -> Result<oximedia_image::Compression> {
    match s.to_ascii_lowercase().as_str() {
        "none" => Ok(oximedia_image::Compression::None),
        "rle" => Ok(oximedia_image::Compression::Rle),
        "zip" => Ok(oximedia_image::Compression::Zip),
        "zips" | "zip-scanline" => Ok(oximedia_image::Compression::ZipScanline),
        "lzw" => Ok(oximedia_image::Compression::Lzw),
        "packbits" => Ok(oximedia_image::Compression::PackBits),
        "piz" => Ok(oximedia_image::Compression::Piz),
        "pxr24" => Ok(oximedia_image::Compression::Pxr24),
        "b44" => Ok(oximedia_image::Compression::B44),
        "b44a" => Ok(oximedia_image::Compression::B44a),
        "dwaa" => Ok(oximedia_image::Compression::Dwaa),
        "dwab" => Ok(oximedia_image::Compression::Dwab),
        other => Err(anyhow::anyhow!(
            "Unknown compression '{}'. Valid: none, rle, zip, zips, lzw, packbits, piz, pxr24, b44, b44a, dwaa, dwab",
            other
        )),
    }
}

fn pixel_type_from_depth(depth: u32) -> Result<oximedia_image::PixelType> {
    match depth {
        8 => Ok(oximedia_image::PixelType::U8),
        10 => Ok(oximedia_image::PixelType::U10),
        12 => Ok(oximedia_image::PixelType::U12),
        16 => Ok(oximedia_image::PixelType::U16),
        32 => Ok(oximedia_image::PixelType::U32),
        other => Err(anyhow::anyhow!(
            "Unsupported bit depth {}. Valid: 8, 10, 12, 16, 32",
            other
        )),
    }
}

fn colorspace_name(cs: oximedia_image::ColorSpace) -> &'static str {
    match cs {
        oximedia_image::ColorSpace::LinearRgb => "Linear RGB",
        oximedia_image::ColorSpace::Srgb => "sRGB",
        oximedia_image::ColorSpace::Rec709 => "Rec. 709",
        oximedia_image::ColorSpace::Rec2020 => "Rec. 2020",
        oximedia_image::ColorSpace::DciP3 => "DCI-P3",
        oximedia_image::ColorSpace::Log => "Logarithmic",
        oximedia_image::ColorSpace::Luma => "Luma",
        oximedia_image::ColorSpace::YCbCr => "YCbCr",
        oximedia_image::ColorSpace::Cmyk => "CMYK",
    }
}

fn compression_name(c: oximedia_image::Compression) -> &'static str {
    match c {
        oximedia_image::Compression::None => "None",
        oximedia_image::Compression::Rle => "RLE",
        oximedia_image::Compression::Zip => "ZIP",
        oximedia_image::Compression::ZipScanline => "ZIP (scanline)",
        oximedia_image::Compression::Lzw => "LZW",
        oximedia_image::Compression::PackBits => "PackBits",
        oximedia_image::Compression::Piz => "PIZ",
        oximedia_image::Compression::Pxr24 => "PXR24",
        oximedia_image::Compression::B44 => "B44",
        oximedia_image::Compression::B44a => "B44A",
        oximedia_image::Compression::Dwaa => "DWAA",
        oximedia_image::Compression::Dwab => "DWAB",
    }
}

// ---------------------------------------------------------------------------
// read_image
// ---------------------------------------------------------------------------

async fn read_image(input: &PathBuf, format: &str, json_output: bool) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let file_size = std::fs::metadata(input)
        .context("Failed to read file metadata")?
        .len();

    // Detect format from magic bytes
    let header_bytes = {
        let mut buf = vec![0u8; 2048.min(file_size as usize)];
        let mut f = std::fs::File::open(input).context("Failed to open input file")?;
        std::io::Read::read(&mut f, &mut buf).context("Failed to read file header")?;
        buf
    };

    let detected_format = oximedia_image::format_detect::FormatDetector::detect(&header_bytes);
    let format_name = detected_format.name();
    let format_ext = detected_format.extension();
    let is_hdr = detected_format.is_hdr();
    let is_lossless = detected_format.is_lossless();

    // Try to read the image frame for dimensions/metadata
    let frame_info = try_read_frame_info(input, &detected_format);

    let output_format = if json_output { "json" } else { format };

    match output_format {
        "json" => {
            let mut info = serde_json::json!({
                "file": input.display().to_string(),
                "file_size": file_size,
                "format": format_name,
                "extension": format_ext,
                "hdr": is_hdr,
                "lossless": is_lossless,
            });

            if let Some(ref fi) = frame_info {
                info["width"] = serde_json::json!(fi.width);
                info["height"] = serde_json::json!(fi.height);
                info["bit_depth"] = serde_json::json!(fi.bit_depth);
                info["components"] = serde_json::json!(fi.components);
                info["colorspace"] = serde_json::json!(fi.colorspace);
                info["pixel_type"] = serde_json::json!(fi.pixel_type);
            }

            let json_str =
                serde_json::to_string_pretty(&info).context("Failed to serialize result")?;
            println!("{}", json_str);
        }
        _ => {
            println!("{}", "Image Information".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "File:", input.display());
            println!("{:20} {} bytes", "File size:", file_size);
            println!("{:20} {}", "Format:", format_name);
            println!("{:20} .{}", "Extension:", format_ext);
            println!("{:20} {}", "HDR:", if is_hdr { "Yes" } else { "No" });
            println!(
                "{:20} {}",
                "Lossless:",
                if is_lossless { "Yes" } else { "No" }
            );

            if let Some(ref fi) = frame_info {
                println!();
                println!("{}", "Image Properties".cyan().bold());
                println!("{}", "-".repeat(60));
                println!("{:20} {}x{}", "Dimensions:", fi.width, fi.height);
                println!("{:20} {}-bit", "Bit depth:", fi.bit_depth);
                println!("{:20} {}", "Components:", fi.components);
                println!("{:20} {}", "Color space:", fi.colorspace);
                println!("{:20} {}", "Pixel type:", fi.pixel_type);
                let pixel_count = (fi.width as u64) * (fi.height as u64);
                let data_size =
                    pixel_count * (fi.components as u64) * ((fi.bit_depth as u64 + 7) / 8);
                println!("{:20} {}", "Pixel count:", pixel_count);
                println!("{:20} {} bytes", "Uncompressed size:", data_size);
            } else {
                println!();
                println!(
                    "{}",
                    "Note: Detailed pixel properties require format-specific decoder integration."
                        .yellow()
                );
            }
        }
    }

    Ok(())
}

struct FrameInfo {
    width: u32,
    height: u32,
    bit_depth: u32,
    components: u32,
    colorspace: String,
    pixel_type: String,
}

fn try_read_frame_info(
    path: &PathBuf,
    detected: &oximedia_image::format_detect::ImageFormat,
) -> Option<FrameInfo> {
    match detected {
        oximedia_image::format_detect::ImageFormat::Dpx => {
            let frame = oximedia_image::dpx::read_dpx(path, 1).ok()?;
            Some(frame_to_info(&frame))
        }
        oximedia_image::format_detect::ImageFormat::Exr => {
            let frame = oximedia_image::exr::read_exr(path, 1).ok()?;
            Some(frame_to_info(&frame))
        }
        oximedia_image::format_detect::ImageFormat::Tiff => {
            let frame = oximedia_image::tiff::read_tiff(path, 1).ok()?;
            Some(frame_to_info(&frame))
        }
        _ => None,
    }
}

fn frame_to_info(frame: &oximedia_image::ImageFrame) -> FrameInfo {
    FrameInfo {
        width: frame.width,
        height: frame.height,
        bit_depth: u32::from(frame.pixel_type.bit_depth()),
        components: u32::from(frame.components),
        colorspace: colorspace_name(frame.color_space).to_string(),
        pixel_type: format!("{:?}", frame.pixel_type),
    }
}

// ---------------------------------------------------------------------------
// convert_image
// ---------------------------------------------------------------------------

async fn convert_image(
    input: &PathBuf,
    output: &PathBuf,
    bit_depth: Option<u32>,
    colorspace: Option<String>,
    compression: Option<String>,
    quality: u8,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Detect input format via magic bytes
    let file_size = std::fs::metadata(input)
        .context("Failed to read file metadata")?
        .len();
    let header_bytes = {
        let mut buf = vec![0u8; 2048.min(file_size as usize)];
        let mut f = std::fs::File::open(input).context("Failed to open input file")?;
        std::io::Read::read(&mut f, &mut buf).context("Failed to read file header")?;
        buf
    };
    let in_fmt = oximedia_image::format_detect::FormatDetector::detect(&header_bytes);
    let out_fmt = output_format_from_path(output);

    // Parse optional settings
    let target_depth = if let Some(d) = bit_depth {
        Some(pixel_type_from_depth(d)?)
    } else {
        None
    };
    let target_cs = if let Some(ref cs) = colorspace {
        Some(parse_colorspace(cs)?)
    } else {
        None
    };
    let target_compression = if let Some(ref c) = compression {
        Some(parse_compression(c)?)
    } else {
        None
    };

    // Status output; suppressed by --quiet.
    if !crate::progress::is_quiet() {
        println!("{}", "Image Conversion".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {} ({})", "Input:", input.display(), in_fmt.name());
        println!("{:20} {} ({})", "Output:", output.display(), out_fmt.name());
        if let Some(ref pt) = target_depth {
            println!("{:20} {}-bit", "Target bit depth:", pt.bit_depth());
        }
        if let Some(ref cs) = target_cs {
            println!("{:20} {}", "Target colorspace:", colorspace_name(*cs));
        }
        if let Some(ref c) = target_compression {
            println!("{:20} {}", "Compression:", compression_name(*c));
        }
        println!();
    }

    // Read input frame
    let mut frame = read_input_frame(input, &in_fmt)?;

    // Apply bit depth conversion
    if let Some(target_pt) = target_depth {
        frame = convert_bit_depth(frame, target_pt)?;
    }

    // Apply color space label (metadata only, no gamut transform)
    if let Some(cs) = target_cs {
        frame.color_space = cs;
    }

    // Write output
    write_output_frame(output, &frame, &out_fmt, target_compression, quality)?;

    if !crate::progress::is_quiet() {
        println!("{}", "✓ Conversion complete.".green().bold());
    }
    Ok(())
}

fn output_format_from_path(path: &PathBuf) -> oximedia_image::format_detect::ImageFormat {
    use oximedia_image::format_detect::ImageFormat;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "dpx" => ImageFormat::Dpx,
        "exr" => ImageFormat::Exr,
        "tif" | "tiff" => ImageFormat::Tiff,
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        "heic" | "heif" | "avif" => ImageFormat::Heif,
        _ => ImageFormat::Unknown,
    }
}

/// Decode an image file into an in-memory frame.
///
/// `pub(crate)` so other subcommands with a genuine need for the real image
/// codec path (e.g. `archive-pro migrate`'s TIFF/PNG preservation targets)
/// reuse this exact decode logic instead of duplicating it.
pub(crate) fn read_input_frame(
    path: &PathBuf,
    fmt: &oximedia_image::format_detect::ImageFormat,
) -> Result<oximedia_image::ImageFrame> {
    use oximedia_image::format_detect::ImageFormat;
    match fmt {
        ImageFormat::Jpeg => oximedia_image::jpeg::read_jpeg(path).context("Failed to read JPEG"),
        ImageFormat::Png => oximedia_image::png::read_png_frame(path).context("Failed to read PNG"),
        ImageFormat::Tiff => {
            oximedia_image::tiff::read_tiff(path, 1).context("Failed to read TIFF")
        }
        ImageFormat::Exr => {
            oximedia_image::exr::read_exr(path, 1).context("Failed to read OpenEXR")
        }
        ImageFormat::Dpx => oximedia_image::dpx::read_dpx(path, 1).context("Failed to read DPX"),
        ImageFormat::WebP => {
            oximedia_image::webp::read_webp(path, 1).context("Failed to read WebP")
        }
        ImageFormat::Heif => Err(anyhow::anyhow!(
            "HEIC/AVIF pixel decoding is not supported.\n\
             HEVC (H.265) is patent-encumbered; OxiMedia uses only royalty-free codecs."
        )),
        other => Err(anyhow::anyhow!(
            "Unsupported input format: {}. Supported: JPEG, PNG, TIFF, EXR, DPX, WebP",
            other.name()
        )),
    }
}

/// Encode an in-memory frame to an image file.
///
/// `pub(crate)` — see [`read_input_frame`].
pub(crate) fn write_output_frame(
    path: &PathBuf,
    frame: &oximedia_image::ImageFrame,
    fmt: &oximedia_image::format_detect::ImageFormat,
    compression: Option<oximedia_image::Compression>,
    jpeg_quality: u8,
) -> Result<()> {
    use oximedia_image::exr::ExrCompression;
    use oximedia_image::format_detect::ImageFormat;
    use oximedia_image::tiff::TiffCompression;
    use oximedia_image::Endian;

    match fmt {
        ImageFormat::Jpeg => oximedia_image::jpeg::write_jpeg(path, frame, jpeg_quality)
            .context("Failed to write JPEG"),
        ImageFormat::Png => {
            let pixels = frame
                .data
                .as_slice()
                .ok_or_else(|| anyhow::anyhow!("PNG encoder requires interleaved pixel data"))?
                .to_vec();
            let color_type = match frame.components {
                1 => oximedia_image::png::PngColorType::Grayscale,
                2 => oximedia_image::png::PngColorType::GrayscaleAlpha,
                3 => oximedia_image::png::PngColorType::Rgb,
                _ => oximedia_image::png::PngColorType::Rgba,
            };
            let png = oximedia_image::png::PngImage {
                width: frame.width,
                height: frame.height,
                bit_depth: frame.pixel_type.bit_depth(),
                color_type,
                pixels,
                metadata: std::collections::HashMap::new(),
            };
            oximedia_image::png::write_png(path, &png).context("Failed to write PNG")
        }
        ImageFormat::Tiff => {
            let tiff_comp = match compression {
                Some(oximedia_image::Compression::Lzw) => TiffCompression::Lzw,
                Some(oximedia_image::Compression::Zip)
                | Some(oximedia_image::Compression::ZipScanline) => TiffCompression::Deflate,
                Some(oximedia_image::Compression::Rle)
                | Some(oximedia_image::Compression::PackBits) => TiffCompression::PackBits,
                _ => TiffCompression::None,
            };
            oximedia_image::tiff::write_tiff(path, frame, tiff_comp).context("Failed to write TIFF")
        }
        ImageFormat::Exr => {
            let exr_comp = match compression {
                Some(oximedia_image::Compression::None) => ExrCompression::None,
                Some(oximedia_image::Compression::Rle) => ExrCompression::Rle,
                Some(oximedia_image::Compression::ZipScanline) => ExrCompression::Zips,
                Some(oximedia_image::Compression::Piz) => ExrCompression::Piz,
                Some(oximedia_image::Compression::Pxr24) => ExrCompression::Pxr24,
                Some(oximedia_image::Compression::B44) => ExrCompression::B44,
                Some(oximedia_image::Compression::B44a) => ExrCompression::B44a,
                Some(oximedia_image::Compression::Dwaa) => ExrCompression::Dwaa,
                Some(oximedia_image::Compression::Dwab) => ExrCompression::Dwab,
                _ => ExrCompression::Zip,
            };
            oximedia_image::exr::write_exr(path, frame, exr_comp).context("Failed to write OpenEXR")
        }
        ImageFormat::Dpx => {
            oximedia_image::dpx::write_dpx(path, frame, Endian::Big).context("Failed to write DPX")
        }
        ImageFormat::WebP => {
            oximedia_image::webp::write_webp(path, frame).context("Failed to write WebP")
        }
        other => Err(anyhow::anyhow!(
            "Unsupported output format: {}. Supported: JPEG, PNG, TIFF, EXR, DPX, WebP",
            other.name()
        )),
    }
}

/// Convert an IEEE 754 half-precision bit pattern (`u16`) to `f32`.
///
/// Handles ±zero, subnormals, ±infinity, and NaN correctly without any
/// external dependencies.
fn f16_to_f32(half: u16) -> f32 {
    let sign = ((half >> 15) as u32) << 31;
    let exp = ((half >> 10) & 0x1F) as u32;
    let mant = (half & 0x3FF) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign // ±zero
        } else {
            // Subnormal: normalize by shifting mantissa until the implicit bit appears.
            let mut m = mant;
            let mut e = 0u32;
            while (m & 0x400) == 0 {
                m <<= 1;
                e += 1;
            }
            sign | ((127u32 - 14 - e + 1) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 31 {
        sign | (0xFF << 23) | (mant << 13) // ±infinity or NaN
    } else {
        sign | ((exp + 112) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

/// Convert an `f32` to an IEEE 754 half-precision bit pattern (`u16`).
///
/// Overflows to ±infinity; underflows to zero or a subnormal half.
fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 31) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x7F_FFFF;

    if exp == 255 {
        // ±infinity or NaN
        return (sign << 15) | 0x7C00 | if mant != 0 { 0x0200 } else { 0 };
    }

    let new_exp = exp - 127 + 15;
    if new_exp >= 31 {
        // Overflow → ±infinity
        return (sign << 15) | 0x7C00;
    }
    if new_exp <= 0 {
        if new_exp < -10 {
            // Too small even for a subnormal half → zero
            return sign << 15;
        }
        // Subnormal half
        let m = (0x0080_0000 | mant) >> (1 - new_exp);
        return (sign << 15) | ((m + 0x1000) >> 13) as u16;
    }

    let half_mant = ((mant + 0x1000) >> 13) as u16;
    (sign << 15) | ((new_exp as u16) << 10) | (half_mant & 0x3FF)
}

fn convert_bit_depth(
    frame: oximedia_image::ImageFrame,
    target: oximedia_image::PixelType,
) -> Result<oximedia_image::ImageFrame> {
    use oximedia_image::{ImageData, PixelType};

    if frame.pixel_type == target {
        return Ok(frame);
    }

    let pixels = frame
        .data
        .as_slice()
        .ok_or_else(|| anyhow::anyhow!("Bit depth conversion requires interleaved pixel data"))?;

    let converted = match (frame.pixel_type, target) {
        (PixelType::U8, PixelType::U16) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for &p in pixels {
                let v = (p as u16) * 257; // 0-255 → 0-65535
                out.extend_from_slice(&v.to_ne_bytes());
            }
            out
        }
        (PixelType::U16, PixelType::U8) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                out.push((v / 257) as u8);
            }
            out
        }
        // 8-bit integer → 32-bit float (normalise 0–255 to 0.0–1.0)
        (PixelType::U8, PixelType::F32) => {
            let mut out = Vec::with_capacity(pixels.len() * 4);
            for &p in pixels {
                let v: f32 = p as f32 / 255.0;
                out.extend_from_slice(&v.to_ne_bytes());
            }
            out
        }
        // 16-bit integer → 32-bit float (normalise 0–65535 to 0.0–1.0)
        (PixelType::U16, PixelType::F32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f: f32 = v as f32 / 65535.0;
                out.extend_from_slice(&f.to_ne_bytes());
            }
            out
        }
        // 32-bit float → 8-bit integer (clamp then quantise)
        (PixelType::F32, PixelType::U8) => {
            let mut out = Vec::with_capacity(pixels.len() / 4);
            for chunk in pixels.chunks_exact(4) {
                let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push((v.clamp(0.0, 1.0) * 255.0) as u8);
            }
            out
        }
        // 32-bit float → 16-bit integer (clamp then quantise)
        (PixelType::F32, PixelType::U16) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(4) {
                let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let q: u16 = (v.clamp(0.0, 1.0) * 65535.0) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        // Same-to-same: identity copy (frame.pixel_type == target was checked above,
        // but this arm keeps the match exhaustive for any new variants).
        (src, dst) if src == dst => pixels.to_vec(),

        // ── U10 conversions (stored as little-endian u16, values 0–1023) ──
        (PixelType::U10, PixelType::U8) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                out.push(((v as u32 * 255 + 511) / 1023) as u8);
            }
            out
        }
        (PixelType::U10, PixelType::U16) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let expanded: u16 = (v << 6) | (v >> 4);
                out.extend_from_slice(&expanded.to_ne_bytes());
            }
            out
        }
        (PixelType::U10, PixelType::F32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f: f32 = v as f32 / 1023.0;
                out.extend_from_slice(&f.to_ne_bytes());
            }
            out
        }
        (PixelType::U8, PixelType::U10) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for &p in pixels {
                let v: u16 = ((p as u32 * 1023 + 127) / 255) as u16;
                out.extend_from_slice(&v.to_ne_bytes());
            }
            out
        }
        (PixelType::U16, PixelType::U10) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let t: u16 = v >> 6;
                out.extend_from_slice(&t.to_ne_bytes());
            }
            out
        }
        (PixelType::F32, PixelType::U10) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let q: u16 = (v.clamp(0.0, 1.0) * 1023.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }

        // ── U12 conversions (stored as little-endian u16, values 0–4095) ──
        (PixelType::U12, PixelType::U8) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                out.push(((v as u32 * 255 + 2047) / 4095) as u8);
            }
            out
        }
        (PixelType::U12, PixelType::U16) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let expanded: u16 = (v << 4) | (v >> 8);
                out.extend_from_slice(&expanded.to_ne_bytes());
            }
            out
        }
        (PixelType::U12, PixelType::F32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f: f32 = v as f32 / 4095.0;
                out.extend_from_slice(&f.to_ne_bytes());
            }
            out
        }
        (PixelType::U8, PixelType::U12) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for &p in pixels {
                let v: u16 = ((p as u32 * 4095 + 127) / 255) as u16;
                out.extend_from_slice(&v.to_ne_bytes());
            }
            out
        }
        (PixelType::U16, PixelType::U12) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let t: u16 = v >> 4;
                out.extend_from_slice(&t.to_ne_bytes());
            }
            out
        }
        (PixelType::F32, PixelType::U12) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let q: u16 = (v.clamp(0.0, 1.0) * 4095.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }

        // ── U32 conversions (32-bit integer per component) ─────────────────
        (PixelType::U32, PixelType::U8) => {
            let mut out = Vec::with_capacity(pixels.len() / 4);
            for chunk in pixels.chunks_exact(4) {
                let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push((v >> 24) as u8);
            }
            out
        }
        (PixelType::U32, PixelType::U16) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let t: u16 = (v >> 16) as u16;
                out.extend_from_slice(&t.to_ne_bytes());
            }
            out
        }
        (PixelType::U32, PixelType::F32) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(4) {
                let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let f = (v as f64 / u32::MAX as f64) as f32;
                out.extend_from_slice(&f.to_ne_bytes());
            }
            out
        }
        (PixelType::U8, PixelType::U32) => {
            let mut out = Vec::with_capacity(pixels.len() * 4);
            for &p in pixels {
                // Replicate the byte across all four bytes: 0x01010101 * p
                let v: u32 = p as u32 * 0x0101_0101;
                out.extend_from_slice(&v.to_ne_bytes());
            }
            out
        }
        (PixelType::U16, PixelType::U32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                // Replicate the u16 into the high and low words.
                let t: u32 = ((v as u32) << 16) | v as u32;
                out.extend_from_slice(&t.to_ne_bytes());
            }
            out
        }
        (PixelType::F32, PixelType::U32) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(4) {
                let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let q: u32 = (v.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }

        // ── F16 conversions (IEEE 754 half-precision stored as u16 bit pattern) ─
        (PixelType::F16, PixelType::F32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let half = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = f16_to_f32(half);
                out.extend_from_slice(&f.to_ne_bytes());
            }
            out
        }
        (PixelType::F32, PixelType::F16) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let half = f32_to_f16(v);
                out.extend_from_slice(&half.to_ne_bytes());
            }
            out
        }
        (PixelType::F16, PixelType::U8) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(2) {
                let half = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = f16_to_f32(half);
                out.push((f.clamp(0.0, 1.0) * 255.0) as u8);
            }
            out
        }
        (PixelType::F16, PixelType::U16) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let half = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = f16_to_f32(half);
                let q: u16 = (f.clamp(0.0, 1.0) * 65535.0) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U8, PixelType::F16) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for &p in pixels {
                let f = p as f32 / 255.0;
                let half = f32_to_f16(f);
                out.extend_from_slice(&half.to_ne_bytes());
            }
            out
        }
        (PixelType::U16, PixelType::F16) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f32 / 65535.0;
                let half = f32_to_f16(f);
                out.extend_from_slice(&half.to_ne_bytes());
            }
            out
        }

        // ── Cross-pairs: U10 ↔ U12, U10 ↔ U32, U12 ↔ U32, F16 ↔ U10,
        //    F16 ↔ U12, F16 ↔ U32 — convert via normalized f32 ─────────────
        (PixelType::U10, PixelType::U12) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f32 / 1023.0;
                let q: u16 = (f.clamp(0.0, 1.0) * 4095.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U12, PixelType::U10) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f32 / 4095.0;
                let q: u16 = (f.clamp(0.0, 1.0) * 1023.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U10, PixelType::U32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f64 / 1023.0;
                let q: u32 = (f.clamp(0.0, 1.0) * u32::MAX as f64) as u32;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U32, PixelType::U10) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let f = v as f64 / u32::MAX as f64;
                let q: u16 = (f.clamp(0.0, 1.0) * 1023.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U12, PixelType::U32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f64 / 4095.0;
                let q: u32 = (f.clamp(0.0, 1.0) * u32::MAX as f64) as u32;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U32, PixelType::U12) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let f = v as f64 / u32::MAX as f64;
                let q: u16 = (f.clamp(0.0, 1.0) * 4095.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::F16, PixelType::U10) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let half = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = f16_to_f32(half);
                let q: u16 = (f.clamp(0.0, 1.0) * 1023.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U10, PixelType::F16) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f32 / 1023.0;
                let half = f32_to_f16(f);
                out.extend_from_slice(&half.to_ne_bytes());
            }
            out
        }
        (PixelType::F16, PixelType::U12) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let half = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = f16_to_f32(half);
                let q: u16 = (f.clamp(0.0, 1.0) * 4095.0 + 0.5) as u16;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U12, PixelType::F16) => {
            let mut out = Vec::with_capacity(pixels.len());
            for chunk in pixels.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = v as f32 / 4095.0;
                let half = f32_to_f16(f);
                out.extend_from_slice(&half.to_ne_bytes());
            }
            out
        }
        (PixelType::F16, PixelType::U32) => {
            let mut out = Vec::with_capacity(pixels.len() * 2);
            for chunk in pixels.chunks_exact(2) {
                let half = u16::from_ne_bytes([chunk[0], chunk[1]]);
                let f = f16_to_f32(half);
                let q: u32 = (f.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32;
                out.extend_from_slice(&q.to_ne_bytes());
            }
            out
        }
        (PixelType::U32, PixelType::F16) => {
            let mut out = Vec::with_capacity(pixels.len() / 2);
            for chunk in pixels.chunks_exact(4) {
                let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let f = (v as f64 / u32::MAX as f64) as f32;
                let half = f32_to_f16(f);
                out.extend_from_slice(&half.to_ne_bytes());
            }
            out
        }
        // Catch-all: same-type identity (unreachable in practice — early return
        // above handles frame.pixel_type == target — but required for exhaustiveness
        // since the guarded arm above cannot be statically proven exhaustive).
        _ => pixels.to_vec(),
    };

    Ok(oximedia_image::ImageFrame::new(
        frame.frame_number,
        frame.width,
        frame.height,
        target,
        frame.components,
        frame.color_space,
        ImageData::interleaved(converted),
    ))
}

// ---------------------------------------------------------------------------
// process_sequence
// ---------------------------------------------------------------------------

async fn process_sequence(
    input: &str,
    start: Option<u32>,
    end: Option<u32>,
    info: bool,
    output: Option<String>,
    json_output: bool,
) -> Result<()> {
    let pattern = oximedia_image::SequencePattern::parse(input)
        .map_err(|e| anyhow::anyhow!("Invalid sequence pattern '{}': {}", input, e))?;

    let start_frame = start.unwrap_or(1);
    let end_frame = end.unwrap_or(start_frame + 99);

    if start_frame > end_frame {
        return Err(anyhow::anyhow!(
            "Start frame ({}) must be <= end frame ({})",
            start_frame,
            end_frame
        ));
    }

    let sequence =
        oximedia_image::ImageSequence::from_pattern(pattern.clone(), start_frame..=end_frame)
            .map_err(|e| anyhow::anyhow!("Failed to create sequence: {}", e))?;

    let frame_count = end_frame - start_frame + 1;
    let has_gaps = !sequence.gaps.is_empty();

    if info || output.is_none() {
        // Show sequence info
        let first_path = pattern.format(start_frame);
        let last_path = pattern.format(end_frame);

        if json_output {
            let result = serde_json::json!({
                "pattern": input,
                "start_frame": start_frame,
                "end_frame": end_frame,
                "frame_count": frame_count,
                "has_gaps": has_gaps,
                "gaps": sequence.gaps,
                "first_file": first_path.display().to_string(),
                "last_file": last_path.display().to_string(),
            });
            let json_str =
                serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
            println!("{}", json_str);
        } else {
            println!("{}", "Image Sequence".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "Pattern:", input);
            println!("{:20} {}", "Start frame:", start_frame);
            println!("{:20} {}", "End frame:", end_frame);
            println!("{:20} {}", "Frame count:", frame_count);
            println!("{:20} {}", "Has gaps:", if has_gaps { "Yes" } else { "No" });

            if has_gaps {
                let gap_display: Vec<String> =
                    sequence.gaps.iter().map(|g| g.to_string()).collect();
                let display = if gap_display.len() > 10 {
                    format!(
                        "{} ... ({} total)",
                        gap_display[..10].join(", "),
                        gap_display.len()
                    )
                } else {
                    gap_display.join(", ")
                };
                println!("{:20} {}", "Missing frames:", display);
            }

            println!();
            println!("{:20} {}", "First file:", first_path.display());
            println!("{:20} {}", "Last file:", last_path.display());
        }
    }

    if let Some(ref out_pattern) = output {
        println!();
        println!("{}", "Sequence Processing".cyan().bold());
        println!("{}", "-".repeat(60));
        println!("{:20} {}", "Source:", input);
        println!("{:20} {}", "Destination:", out_pattern);
        println!("{:20} {}", "Frames:", frame_count);
        println!();
        println!(
            "{}",
            "Note: Sequence transcoding requires frame read/write pipeline integration.".yellow()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// adjust_image
// ---------------------------------------------------------------------------

async fn adjust_image(
    input: &PathBuf,
    output: &PathBuf,
    brightness: Option<f64>,
    contrast: Option<f64>,
    saturation: Option<f64>,
    gamma: Option<f64>,
    exposure: Option<f64>,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Validate parameter ranges
    if let Some(b) = brightness {
        if !(-1.0..=1.0).contains(&b) {
            return Err(anyhow::anyhow!(
                "Brightness must be between -1.0 and 1.0, got {}",
                b
            ));
        }
    }
    if let Some(c) = contrast {
        if !(0.0..=3.0).contains(&c) {
            return Err(anyhow::anyhow!(
                "Contrast must be between 0.0 and 3.0, got {}",
                c
            ));
        }
    }
    if let Some(s) = saturation {
        if !(0.0..=3.0).contains(&s) {
            return Err(anyhow::anyhow!(
                "Saturation must be between 0.0 and 3.0, got {}",
                s
            ));
        }
    }
    if let Some(g) = gamma {
        if !(0.1..=5.0).contains(&g) {
            return Err(anyhow::anyhow!(
                "Gamma must be between 0.1 and 5.0, got {}",
                g
            ));
        }
    }
    if let Some(e) = exposure {
        if !(-5.0..=5.0).contains(&e) {
            return Err(anyhow::anyhow!(
                "Exposure must be between -5.0 and 5.0, got {}",
                e
            ));
        }
    }

    println!("{}", "Image Adjustment".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", input.display());
    println!("{:20} {}", "Output:", output.display());
    println!();

    println!("{}", "Adjustments".cyan().bold());
    println!("{}", "-".repeat(60));

    let mut any_adjustment = false;

    if let Some(b) = brightness {
        println!("{:20} {:+.3}", "Brightness:", b);
        any_adjustment = true;
    }
    if let Some(c) = contrast {
        println!("{:20} {:.3}x", "Contrast:", c);
        any_adjustment = true;
    }
    if let Some(s) = saturation {
        println!("{:20} {:.3}x", "Saturation:", s);
        any_adjustment = true;
    }
    if let Some(g) = gamma {
        println!("{:20} {:.3}", "Gamma:", g);
        any_adjustment = true;
    }
    if let Some(e) = exposure {
        println!("{:20} {:+.3} stops", "Exposure:", e);
        any_adjustment = true;
    }

    if !any_adjustment {
        println!("  (no adjustments specified)");
    }

    println!();
    println!(
        "{}",
        "Note: Image adjustment pipeline requires frame decode/encode integration.".yellow()
    );
    println!(
        "{}",
        "Color adjustment kernels and filter infrastructure are ready.".dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// generate_histogram
// ---------------------------------------------------------------------------

async fn generate_histogram(
    input: &PathBuf,
    output: Option<PathBuf>,
    mode: &str,
    width: Option<u32>,
    height: Option<u32>,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Validate mode
    let valid_modes = ["rgb", "luma", "per-channel"];
    if !valid_modes.contains(&mode) {
        return Err(anyhow::anyhow!(
            "Invalid histogram mode '{}'. Valid: rgb, luma, per-channel",
            mode
        ));
    }

    let hist_width = width.unwrap_or(512);
    let hist_height = height.unwrap_or(256);

    if json_output {
        let result = serde_json::json!({
            "input": input.display().to_string(),
            "mode": mode,
            "histogram_width": hist_width,
            "histogram_height": hist_height,
            "output": output.as_ref().map(|p| p.display().to_string()),
            "status": "pending_frame_decoding",
            "available_modes": valid_modes,
            "message": "Histogram engine ready; awaiting frame decoding pipeline integration",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Image Histogram".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Mode:", mode);
        println!("{:20} {}x{}", "Histogram size:", hist_width, hist_height);

        if let Some(ref out) = output {
            println!("{:20} {}", "Output:", out.display());
        }

        println!();

        println!("{}", "Histogram Engine".cyan().bold());
        println!("{}", "-".repeat(60));
        println!("  Available histogram operations:");
        println!("    - Per-channel RGB histogram (8-bit bins)");
        println!("    - Luminance histogram (BT.601 / BT.709)");
        println!("    - Cumulative distribution function (CDF)");
        println!("    - Histogram equalization LUT generation");
        println!("    - Histogram matching between images");
        println!("    - Contrast stretch / auto-levels");
        println!();

        println!(
            "{}",
            "Note: Full histogram output requires frame decoding pipeline.".yellow()
        );
        println!(
            "{}",
            "Histogram computation engine (oximedia_image::histogram_ops) is ready.".dimmed()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_colorspace_valid() {
        assert_eq!(
            parse_colorspace("srgb").ok(),
            Some(oximedia_image::ColorSpace::Srgb)
        );
        assert_eq!(
            parse_colorspace("linear").ok(),
            Some(oximedia_image::ColorSpace::LinearRgb)
        );
        assert_eq!(
            parse_colorspace("rec709").ok(),
            Some(oximedia_image::ColorSpace::Rec709)
        );
        assert_eq!(
            parse_colorspace("rec2020").ok(),
            Some(oximedia_image::ColorSpace::Rec2020)
        );
        assert_eq!(
            parse_colorspace("dci-p3").ok(),
            Some(oximedia_image::ColorSpace::DciP3)
        );
        assert_eq!(
            parse_colorspace("log").ok(),
            Some(oximedia_image::ColorSpace::Log)
        );
    }

    #[test]
    fn test_parse_colorspace_invalid() {
        assert!(parse_colorspace("invalid").is_err());
    }

    #[test]
    fn test_parse_compression_valid() {
        assert_eq!(
            parse_compression("none").ok(),
            Some(oximedia_image::Compression::None)
        );
        assert_eq!(
            parse_compression("zip").ok(),
            Some(oximedia_image::Compression::Zip)
        );
        assert_eq!(
            parse_compression("piz").ok(),
            Some(oximedia_image::Compression::Piz)
        );
        assert_eq!(
            parse_compression("rle").ok(),
            Some(oximedia_image::Compression::Rle)
        );
    }

    #[test]
    fn test_parse_compression_invalid() {
        assert!(parse_compression("badcompress").is_err());
    }

    #[test]
    fn test_pixel_type_from_depth() {
        assert_eq!(
            pixel_type_from_depth(8).ok(),
            Some(oximedia_image::PixelType::U8)
        );
        assert_eq!(
            pixel_type_from_depth(10).ok(),
            Some(oximedia_image::PixelType::U10)
        );
        assert_eq!(
            pixel_type_from_depth(16).ok(),
            Some(oximedia_image::PixelType::U16)
        );
        assert!(pixel_type_from_depth(7).is_err());
    }

    #[test]
    fn test_colorspace_name() {
        assert_eq!(colorspace_name(oximedia_image::ColorSpace::Srgb), "sRGB");
        assert_eq!(
            colorspace_name(oximedia_image::ColorSpace::LinearRgb),
            "Linear RGB"
        );
    }

    #[test]
    fn test_compression_name() {
        assert_eq!(compression_name(oximedia_image::Compression::None), "None");
        assert_eq!(compression_name(oximedia_image::Compression::Zip), "ZIP");
    }

    // Helper: build a minimal 1×1 ImageFrame with given pixel data.
    fn make_frame(
        pixel_type: oximedia_image::PixelType,
        data: Vec<u8>,
    ) -> oximedia_image::ImageFrame {
        oximedia_image::ImageFrame::new(
            0,
            1,
            1,
            pixel_type,
            1,
            oximedia_image::ColorSpace::Srgb,
            oximedia_image::ImageData::interleaved(data),
        )
    }

    #[test]
    fn test_convert_bit_depth_u8_to_u16() {
        use oximedia_image::PixelType;
        let frame = make_frame(PixelType::U8, vec![0, 128, 255]);
        let out = convert_bit_depth(frame, PixelType::U16).expect("conversion should succeed");
        assert_eq!(out.pixel_type, PixelType::U16);
        let raw = out.data.as_slice().expect("interleaved data");
        let v0 = u16::from_ne_bytes([raw[0], raw[1]]);
        let v1 = u16::from_ne_bytes([raw[2], raw[3]]);
        let v2 = u16::from_ne_bytes([raw[4], raw[5]]);
        assert_eq!(v0, 0);
        assert_eq!(v1, 128 * 257);
        assert_eq!(v2, 65535);
    }

    #[test]
    fn test_convert_bit_depth_u16_to_u8() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        let v: u16 = 65535;
        data.extend_from_slice(&v.to_ne_bytes());
        let v2: u16 = 0;
        data.extend_from_slice(&v2.to_ne_bytes());
        let frame = make_frame(PixelType::U16, data);
        let out = convert_bit_depth(frame, PixelType::U8).expect("conversion should succeed");
        assert_eq!(out.pixel_type, PixelType::U8);
        let raw = out.data.as_slice().expect("interleaved data");
        assert_eq!(raw[0], 255);
        assert_eq!(raw[1], 0);
    }

    #[test]
    fn test_convert_bit_depth_u8_to_f32() {
        use oximedia_image::PixelType;
        let frame = make_frame(PixelType::U8, vec![0, 255]);
        let out = convert_bit_depth(frame, PixelType::F32).expect("conversion should succeed");
        assert_eq!(out.pixel_type, PixelType::F32);
        let raw = out.data.as_slice().expect("interleaved data");
        let f0 = f32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let f1 = f32::from_ne_bytes([raw[4], raw[5], raw[6], raw[7]]);
        assert!((f0 - 0.0_f32).abs() < 1e-6);
        assert!((f1 - 1.0_f32).abs() < 1e-6);
    }

    #[test]
    fn test_convert_bit_depth_u16_to_f32() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        let v: u16 = 65535;
        data.extend_from_slice(&v.to_ne_bytes());
        let frame = make_frame(PixelType::U16, data);
        let out = convert_bit_depth(frame, PixelType::F32).expect("conversion should succeed");
        let raw = out.data.as_slice().expect("interleaved data");
        let f = f32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
        assert!((f - 1.0_f32).abs() < 1e-6);
    }

    #[test]
    fn test_convert_bit_depth_f32_to_u8() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        for v in [0.0_f32, 1.0_f32, 0.5_f32, -0.1_f32, 1.5_f32] {
            data.extend_from_slice(&v.to_ne_bytes());
        }
        let frame = make_frame(PixelType::F32, data);
        let out = convert_bit_depth(frame, PixelType::U8).expect("conversion should succeed");
        let raw = out.data.as_slice().expect("interleaved data");
        assert_eq!(raw[0], 0);
        assert_eq!(raw[1], 255);
        // 0.5 * 255.0 = 127.5, truncated to 127
        assert_eq!(raw[2], 127);
        // clamped: -0.1 → 0 → 0
        assert_eq!(raw[3], 0);
        // clamped: 1.5 → 1.0 → 255
        assert_eq!(raw[4], 255);
    }

    #[test]
    fn test_convert_bit_depth_f32_to_u16() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        for v in [0.0_f32, 1.0_f32] {
            data.extend_from_slice(&v.to_ne_bytes());
        }
        let frame = make_frame(PixelType::F32, data);
        let out = convert_bit_depth(frame, PixelType::U16).expect("conversion should succeed");
        let raw = out.data.as_slice().expect("interleaved data");
        let v0 = u16::from_ne_bytes([raw[0], raw[1]]);
        let v1 = u16::from_ne_bytes([raw[2], raw[3]]);
        assert_eq!(v0, 0);
        assert_eq!(v1, 65535);
    }

    #[test]
    fn test_convert_bit_depth_same_type_identity() {
        use oximedia_image::PixelType;
        let frame = make_frame(PixelType::U8, vec![10, 20, 30]);
        let out = convert_bit_depth(frame, PixelType::U8).expect("same-type should be identity");
        let raw = out.data.as_slice().expect("interleaved data");
        assert_eq!(raw, &[10, 20, 30]);
    }

    // ── U10 tests ─────────────────────────────────────────────────────────────

    #[test]
    fn test_convert_u10_to_u8_black_white() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_ne_bytes());
        data.extend_from_slice(&1023u16.to_ne_bytes());
        let frame = make_frame(PixelType::U10, data);
        let out = convert_bit_depth(frame, PixelType::U8).expect("U10→U8");
        let raw = out.data.as_slice().expect("data");
        assert_eq!(raw[0], 0);
        assert_eq!(raw[1], 255);
    }

    /// U10 value 512 → U16: (512 << 6) | (512 >> 4) = 32768 | 32 = 32800
    #[test]
    fn test_convert_u10_to_u16() {
        use oximedia_image::PixelType;
        let v: u16 = 512;
        let mut data = Vec::new();
        data.extend_from_slice(&v.to_ne_bytes());
        let frame = make_frame(PixelType::U10, data);
        let out = convert_bit_depth(frame, PixelType::U16).expect("U10→U16");
        let raw = out.data.as_slice().expect("data");
        let result = u16::from_ne_bytes([raw[0], raw[1]]);
        let expected: u16 = (512u16 << 6) | (512u16 >> 4);
        assert_eq!(
            result, expected,
            "U10(512) → U16 should be {expected}, got {result}"
        );
    }

    #[test]
    fn test_convert_u10_to_f32_half() {
        use oximedia_image::PixelType;
        // 512/1023 ≈ 0.50049
        let v: u16 = 512;
        let mut data = Vec::new();
        data.extend_from_slice(&v.to_ne_bytes());
        let frame = make_frame(PixelType::U10, data);
        let out = convert_bit_depth(frame, PixelType::F32).expect("U10→F32");
        let raw = out.data.as_slice().expect("data");
        let f = f32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
        assert!((f - 512.0 / 1023.0).abs() < 1e-5, "got {f}");
    }

    #[test]
    fn test_convert_u8_to_u10_black_white() {
        use oximedia_image::PixelType;
        let frame = make_frame(PixelType::U8, vec![0, 255]);
        let out = convert_bit_depth(frame, PixelType::U10).expect("U8→U10");
        let raw = out.data.as_slice().expect("data");
        let v0 = u16::from_ne_bytes([raw[0], raw[1]]);
        let v1 = u16::from_ne_bytes([raw[2], raw[3]]);
        assert_eq!(v0, 0);
        assert_eq!(v1, 1023);
    }

    #[test]
    fn test_convert_f32_to_u10_clamp() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        for v in [0.0_f32, 1.0_f32] {
            data.extend_from_slice(&v.to_ne_bytes());
        }
        let frame = make_frame(PixelType::F32, data);
        let out = convert_bit_depth(frame, PixelType::U10).expect("F32→U10");
        let raw = out.data.as_slice().expect("data");
        let v0 = u16::from_ne_bytes([raw[0], raw[1]]);
        let v1 = u16::from_ne_bytes([raw[2], raw[3]]);
        assert_eq!(v0, 0);
        assert_eq!(v1, 1023);
    }

    // ── U12 tests ─────────────────────────────────────────────────────────────

    #[test]
    fn test_convert_u12_to_u8_black_white() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_ne_bytes());
        data.extend_from_slice(&4095u16.to_ne_bytes());
        let frame = make_frame(PixelType::U12, data);
        let out = convert_bit_depth(frame, PixelType::U8).expect("U12→U8");
        let raw = out.data.as_slice().expect("data");
        assert_eq!(raw[0], 0);
        assert_eq!(raw[1], 255);
    }

    #[test]
    fn test_convert_u12_to_f32() {
        use oximedia_image::PixelType;
        let v: u16 = 4095;
        let mut data = Vec::new();
        data.extend_from_slice(&v.to_ne_bytes());
        let frame = make_frame(PixelType::U12, data);
        let out = convert_bit_depth(frame, PixelType::F32).expect("U12→F32");
        let raw = out.data.as_slice().expect("data");
        let f = f32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
        assert!((f - 1.0).abs() < 1e-5, "got {f}");
    }

    // ── U32 tests ─────────────────────────────────────────────────────────────

    #[test]
    fn test_convert_u32_to_u8() {
        use oximedia_image::PixelType;
        let mut data = Vec::new();
        data.extend_from_slice(&u32::MAX.to_ne_bytes());
        data.extend_from_slice(&0u32.to_ne_bytes());
        let frame = make_frame(PixelType::U32, data);
        let out = convert_bit_depth(frame, PixelType::U8).expect("U32→U8");
        let raw = out.data.as_slice().expect("data");
        assert_eq!(raw[0], 255);
        assert_eq!(raw[1], 0);
    }

    #[test]
    fn test_convert_u32_to_u16() {
        use oximedia_image::PixelType;
        let v: u32 = u32::MAX;
        let mut data = Vec::new();
        data.extend_from_slice(&v.to_ne_bytes());
        let frame = make_frame(PixelType::U32, data);
        let out = convert_bit_depth(frame, PixelType::U16).expect("U32→U16");
        let raw = out.data.as_slice().expect("data");
        let t = u16::from_ne_bytes([raw[0], raw[1]]);
        assert_eq!(t, u16::MAX);
    }

    #[test]
    fn test_convert_u8_to_u32_replicate() {
        use oximedia_image::PixelType;
        // 255 → 0xFFFF_FFFF
        let frame = make_frame(PixelType::U8, vec![255, 0]);
        let out = convert_bit_depth(frame, PixelType::U32).expect("U8→U32");
        let raw = out.data.as_slice().expect("data");
        let v0 = u32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let v1 = u32::from_ne_bytes([raw[4], raw[5], raw[6], raw[7]]);
        assert_eq!(v0, u32::MAX);
        assert_eq!(v1, 0);
    }

    // ── F16 tests ─────────────────────────────────────────────────────────────

    #[test]
    fn test_f16_to_f32_zero() {
        assert_eq!(f16_to_f32(0x0000), 0.0_f32);
    }

    #[test]
    fn test_f16_to_f32_one() {
        // 1.0 in half precision = 0x3C00
        let f = f16_to_f32(0x3C00);
        assert!((f - 1.0_f32).abs() < 1e-5, "got {f}");
    }

    #[test]
    fn test_f32_to_f16_zero_roundtrip() {
        let half = f32_to_f16(0.0);
        let back = f16_to_f32(half);
        assert!((back - 0.0).abs() < 1e-6, "got {back}");
    }

    /// f32→f16→f32 for common values stays within 1e-3
    #[test]
    fn test_convert_f16_roundtrip() {
        for &v in &[0.0_f32, 1.0_f32, 0.5_f32, 0.25_f32] {
            let half = f32_to_f16(v);
            let back = f16_to_f32(half);
            assert!(
                (back - v).abs() < 1e-3,
                "f16 roundtrip: {} → {} → {} (error {})",
                v,
                half,
                back,
                (back - v).abs()
            );
        }
    }

    #[test]
    fn test_convert_u8_to_f16_and_back() {
        use oximedia_image::PixelType;
        let frame = make_frame(PixelType::U8, vec![0, 255]);
        let f16_frame = convert_bit_depth(frame, PixelType::F16).expect("U8→F16");
        let u8_frame = convert_bit_depth(f16_frame, PixelType::U8).expect("F16→U8");
        let raw = u8_frame.data.as_slice().expect("data");
        assert_eq!(raw[0], 0);
        assert_eq!(raw[1], 255);
    }
}

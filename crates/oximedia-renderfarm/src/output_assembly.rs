// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Real output assembly: combining rendered per-frame image files into a
//! single video file via `oximedia-container` muxing.
//!
//! Two real routes, chosen by sniffing the *first* rendered frame's actual
//! bytes:
//!
//! - **JPEG frames** (`FF D8 FF` signature) are written, byte-for-byte and
//!   without re-encoding, as Motion JPEG packets inside an AVI container via
//!   [`oximedia_container::mux::avi::AviMjpegWriter`]. Real width/height
//!   come from actually decoding each frame (see [`crate::frame_decode`]),
//!   not from a guess.
//! - **Anything else `oximedia_codec::image::ImageDecoder` can decode**
//!   (PNG, WebP, ...) is decoded to real pixel data, normalized to YUV420p,
//!   and written as raw frames into a YUV4MPEG2 (Y4M) stream via
//!   [`oximedia_container::mux::y4m::Y4mMuxerBuilder`].
//!
//! A frame sequence whose frames don't decode consistently (mixed formats
//! within a route, or mismatched dimensions) fails honestly instead of
//! silently dropping frames or reporting success on a partial result.
//!
//! # Frame rate
//!
//! This coordinator has no per-job frame rate field anywhere in
//! [`crate::job::JobSubmission`]. If the submission's `metadata` map has an
//! `"fps"` entry (`"<num>"` or `"<num>/<den>"`), it is used; otherwise a
//! documented default of 24/1 applies. This is an honest, real limitation,
//! not a fabricated frame rate: the metadata convention is the closest
//! thing this crate has to configuration for it today.

use crate::error::{Error, Result};
use crate::job::Job;
use oximedia_codec::frame::VideoFrame;
use oximedia_container::mux::avi::{AviMjpegWriter, VideoCodec};
use oximedia_container::mux::y4m::{Y4mMuxer, Y4mMuxerBuilder};
use std::path::{Path, PathBuf};

/// Default frame rate used when a job submission does not specify one via
/// `metadata["fps"]`.
const DEFAULT_FPS: (u32, u32) = (24, 1);

/// JPEG SOI + first marker byte -- the standard "is this a JPEG file"
/// signature check (matches `oximedia_codec::image::ImageFormat::Jpeg`'s own
/// detection).
const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// Assembles `frame_paths` (already sorted by frame number, `success`
/// results only, each verified to exist -- see
/// [`crate::pipeline::Pipeline::verify_all_frames`]) into a single output
/// file written next to the frames.
///
/// # Errors
///
/// See module docs: fails when frames are absent, of inconsistent formats
/// or dimensions within a route, or in a format neither the MJPEG-in-AVI
/// nor the Y4M route can handle.
pub(crate) fn assemble(job: &Job, frame_paths: &[PathBuf]) -> Result<PathBuf> {
    let Some(first_path) = frame_paths.first() else {
        return Err(Error::Other(format!(
            "output assembly for job {}: no successfully rendered frames to assemble",
            job.id
        )));
    };

    let first_bytes = std::fs::read(first_path)?;
    let (fps_num, fps_den) = read_fps(job);

    if is_jpeg(&first_bytes) {
        assemble_mjpeg_avi(job, frame_paths, fps_num, fps_den)
    } else {
        assemble_y4m(job, frame_paths, fps_num, fps_den)
    }
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&JPEG_SIGNATURE)
}

/// Reads an optional `"fps"` override from the job submission's metadata.
/// Accepts a plain integer (`"30"`) or a rational (`"30000/1001"`). Falls
/// back to [`DEFAULT_FPS`] on anything missing, unparseable, or zero.
fn read_fps(job: &Job) -> (u32, u32) {
    let Some(raw) = job.submission.metadata.get("fps") else {
        return DEFAULT_FPS;
    };

    if let Some((num, den)) = raw.split_once('/') {
        if let (Ok(n), Ok(d)) = (num.trim().parse::<u32>(), den.trim().parse::<u32>()) {
            if n > 0 && d > 0 {
                return (n, d);
            }
        }
    } else if let Ok(n) = raw.trim().parse::<u32>() {
        if n > 0 {
            return (n, 1);
        }
    }

    DEFAULT_FPS
}

fn assemble_mjpeg_avi(
    job: &Job,
    frame_paths: &[PathBuf],
    fps_num: u32,
    fps_den: u32,
) -> Result<PathBuf> {
    let first_bytes = std::fs::read(&frame_paths[0])?;
    let (width, height) = crate::frame_decode::decode_dimensions(&first_bytes)?;

    let mut writer =
        AviMjpegWriter::new(width, height, fps_num, fps_den).with_video_codec(VideoCodec::Mjpeg);

    write_mjpeg_frame(&mut writer, job, &frame_paths[0], first_bytes)?;

    for path in &frame_paths[1..] {
        let bytes = std::fs::read(path)?;
        let dims = crate::frame_decode::decode_dimensions(&bytes)?;
        if dims != (width, height) {
            return Err(Error::Other(format!(
                "output assembly for job {}: frame {} is {}x{}, expected {width}x{height} \
                 (from the first frame) -- inconsistent frame dimensions are not supported",
                job.id,
                path.display(),
                dims.0,
                dims.1
            )));
        }
        write_mjpeg_frame(&mut writer, job, path, bytes)?;
    }

    let avi_bytes = writer
        .finish()
        .map_err(|e| Error::Other(format!("MJPEG AVI finalize failed for job {}: {e}", job.id)))?;

    let output_path = output_path_for(job, &frame_paths[0], "avi");
    std::fs::write(&output_path, avi_bytes)?;
    Ok(output_path)
}

fn write_mjpeg_frame(
    writer: &mut AviMjpegWriter,
    job: &Job,
    path: &Path,
    bytes: Vec<u8>,
) -> Result<()> {
    if !is_jpeg(&bytes) {
        return Err(Error::Other(format!(
            "output assembly for job {}: frame {} is not JPEG while an earlier frame was -- \
             mixed-format frame sequences are not supported",
            job.id,
            path.display()
        )));
    }
    writer.write_frame(bytes).map_err(|e| {
        Error::Other(format!(
            "MJPEG AVI mux failed for job {} frame {}: {e}",
            job.id,
            path.display()
        ))
    })
}

fn assemble_y4m(job: &Job, frame_paths: &[PathBuf], fps_num: u32, fps_den: u32) -> Result<PathBuf> {
    let first_bytes = std::fs::read(&frame_paths[0])?;
    let first_frame = crate::frame_decode::decode_normalized(&first_bytes).map_err(|e| {
        Error::Other(format!(
            "output assembly for job {}: frame {} could not be decoded ({e}) -- neither the \
             MJPEG-in-AVI route (not JPEG) nor the Y4M route (not decodable) applies",
            job.id,
            frame_paths[0].display()
        ))
    })?;
    let (width, height) = (first_frame.width, first_frame.height);

    let mut out_bytes: Vec<u8> = Vec::new();
    {
        let mut muxer = Y4mMuxerBuilder::new(width, height)
            .fps(fps_num, fps_den)
            .build(&mut out_bytes)
            .map_err(|e| Error::Other(format!("Y4M mux init failed for job {}: {e}", job.id)))?;

        write_y4m_frame(&mut muxer, job, &frame_paths[0], &first_frame)?;

        for path in &frame_paths[1..] {
            let bytes = std::fs::read(path)?;
            let frame = crate::frame_decode::decode_normalized(&bytes).map_err(|e| {
                Error::Other(format!(
                    "output assembly for job {}: frame {} could not be decoded: {e}",
                    job.id,
                    path.display()
                ))
            })?;
            if (frame.width, frame.height) != (width, height) {
                return Err(Error::Other(format!(
                    "output assembly for job {}: frame {} is {}x{}, expected {width}x{height} \
                     -- inconsistent frame dimensions are not supported",
                    job.id,
                    path.display(),
                    frame.width,
                    frame.height
                )));
            }
            write_y4m_frame(&mut muxer, job, path, &frame)?;
        }

        muxer
            .finish()
            .map_err(|e| Error::Other(format!("Y4M finalize failed for job {}: {e}", job.id)))?;
    }

    let output_path = output_path_for(job, &frame_paths[0], "y4m");
    std::fs::write(&output_path, &out_bytes)?;
    Ok(output_path)
}

fn write_y4m_frame<W: std::io::Write>(
    muxer: &mut Y4mMuxer<W>,
    job: &Job,
    path: &Path,
    frame: &VideoFrame,
) -> Result<()> {
    let mut frame_bytes = Vec::with_capacity(frame.planes.iter().map(|p| p.data.len()).sum());
    for plane in &frame.planes {
        frame_bytes.extend_from_slice(&plane.data);
    }
    muxer.write_frame(&frame_bytes).map_err(|e| {
        Error::Other(format!(
            "Y4M frame write failed for job {} frame {}: {e}",
            job.id,
            path.display()
        ))
    })
}

/// Picks an output path next to the frame files (same directory as the
/// first frame), named `{job_id}.{ext}`. Falls back to the system temp
/// directory only in the degenerate case where the first frame's path has
/// no parent directory component (e.g. a bare relative file name).
fn output_path_for(job: &Job, first_frame: &Path, ext: &str) -> PathBuf {
    let dir = first_frame
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{}.{ext}", job.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::JobSubmission;
    use oximedia_container::demux::avi::AviMjpegReader;
    use oximedia_container::demux::y4m::Y4mDemuxer;
    use std::io::Cursor;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia-renderfarm-output-assembly-{name}"))
    }

    fn test_job(fps: Option<&str>) -> Result<Job> {
        let mut builder = JobSubmission::builder()
            .project_file(tmp_path("assembly-test.blend"))
            .frame_range(1, 3);
        if let Some(fps) = fps {
            builder = builder.metadata("fps".to_string(), fps.to_string());
        }
        Ok(Job::new(builder.build()?))
    }

    /// Encodes a tiny but real JPEG via `oximedia_image`'s baseline encoder.
    fn make_jpeg(width: u32, height: u32, seed: u8) -> Vec<u8> {
        use oximedia_image::jpeg::{JpegEncoder, JpegQuality};
        use oximedia_image::{ColorSpace, ImageData, ImageFrame, PixelType};

        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.push((x.wrapping_add(u32::from(seed)) % 256) as u8);
                rgb.push((y % 256) as u8);
                rgb.push(seed);
            }
        }
        let frame = ImageFrame::new(
            0,
            width,
            height,
            PixelType::U8,
            3,
            ColorSpace::Srgb,
            ImageData::interleaved(rgb),
        );
        JpegEncoder::new(JpegQuality(90))
            .encode(&frame)
            .expect("JPEG encode should succeed in test")
    }

    /// Encodes a tiny but real PNG via `oximedia_codec`'s own encoder.
    fn make_png(width: u32, height: u32, seed: u8) -> Vec<u8> {
        use oximedia_codec::frame::{Plane, VideoFrame as CodecVideoFrame};
        use oximedia_codec::image::{EncoderConfig, ImageEncoder};
        use oximedia_core::PixelFormat;

        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.push((x.wrapping_add(u32::from(seed)) % 256) as u8);
                rgb.push((y % 256) as u8);
                rgb.push(seed);
            }
        }
        let mut frame = CodecVideoFrame::new(PixelFormat::Rgb24, width, height);
        frame.planes = vec![Plane::with_dimensions(
            rgb,
            (width * 3) as usize,
            width,
            height,
        )];
        ImageEncoder::new(EncoderConfig::png())
            .encode(&frame)
            .expect("PNG encode should succeed in test")
    }

    #[test]
    fn assemble_empty_frames_is_honest_err() -> Result<()> {
        let job = test_job(None)?;
        let result = assemble(&job, &[]);
        assert!(result.is_err(), "no frames must not fabricate an output");
        Ok(())
    }

    #[test]
    fn assemble_undecodable_bytes_is_honest_err() -> Result<()> {
        let job = test_job(None)?;
        let path = tmp_path("garbage.bin");
        std::fs::write(&path, b"not an image at all")?;

        let result = assemble(&job, std::slice::from_ref(&path));
        assert!(
            result.is_err(),
            "garbage bytes must not fabricate an output"
        );

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn read_fps_defaults_when_absent() -> Result<()> {
        let job = test_job(None)?;
        assert_eq!(read_fps(&job), DEFAULT_FPS);
        Ok(())
    }

    #[test]
    fn read_fps_parses_plain_integer() -> Result<()> {
        let job = test_job(Some("30"))?;
        assert_eq!(read_fps(&job), (30, 1));
        Ok(())
    }

    #[test]
    fn read_fps_parses_rational() -> Result<()> {
        let job = test_job(Some("30000/1001"))?;
        assert_eq!(read_fps(&job), (30000, 1001));
        Ok(())
    }

    #[test]
    fn read_fps_falls_back_on_garbage() -> Result<()> {
        let job = test_job(Some("not-a-number"))?;
        assert_eq!(read_fps(&job), DEFAULT_FPS);
        Ok(())
    }

    #[test]
    fn assemble_mjpeg_avi_real_roundtrip_frame_count_and_bytes() -> Result<()> {
        let job = test_job(Some("25"))?;

        let jpeg_frames: Vec<Vec<u8>> = (0..3u8).map(|i| make_jpeg(16, 16, i * 40)).collect();
        let mut frame_paths = Vec::new();
        for (i, bytes) in jpeg_frames.iter().enumerate() {
            let path = tmp_path(&format!("avi-roundtrip-{i}.jpg"));
            std::fs::write(&path, bytes)?;
            frame_paths.push(path);
        }

        let output_path = assemble(&job, &frame_paths)?;
        assert!(output_path.exists());
        assert_eq!(
            output_path.extension().and_then(|e| e.to_str()),
            Some("avi")
        );

        // Real re-demux: read the assembled AVI back and confirm the frame
        // count and bytes actually round-trip, not just that a file exists.
        let assembled_bytes = std::fs::read(&output_path)?;
        let reader = AviMjpegReader::new(assembled_bytes)
            .map_err(|e| Error::Other(format!("re-demux failed: {e}")))?;
        let demuxed_frames = reader
            .frames()
            .map_err(|e| Error::Other(format!("re-demux frames() failed: {e}")))?;

        assert_eq!(
            demuxed_frames.len(),
            jpeg_frames.len(),
            "re-demuxed AVI frame count must match the number of assembled frames"
        );
        for (original, demuxed) in jpeg_frames.iter().zip(demuxed_frames.iter()) {
            assert_eq!(
                original, demuxed,
                "MJPEG-in-AVI is a byte-for-byte passthrough of the original JPEG"
            );
        }

        for path in &frame_paths {
            std::fs::remove_file(path).ok();
        }
        std::fs::remove_file(&output_path).ok();
        Ok(())
    }

    #[test]
    fn assemble_y4m_real_roundtrip_frame_count() -> Result<()> {
        let job = test_job(None)?;

        let png_frames: Vec<Vec<u8>> = (0..4u8).map(|i| make_png(20, 12, i * 30)).collect();
        let mut frame_paths = Vec::new();
        for (i, bytes) in png_frames.iter().enumerate() {
            let path = tmp_path(&format!("y4m-roundtrip-{i}.png"));
            std::fs::write(&path, bytes)?;
            frame_paths.push(path);
        }

        let output_path = assemble(&job, &frame_paths)?;
        assert!(output_path.exists());
        assert_eq!(
            output_path.extension().and_then(|e| e.to_str()),
            Some("y4m")
        );

        // Real re-demux: read the assembled Y4M stream back and count
        // frames via `Y4mDemuxer`, not just check the file is non-empty.
        let assembled_bytes = std::fs::read(&output_path)?;
        let mut demuxer = Y4mDemuxer::new(Cursor::new(assembled_bytes))
            .map_err(|e| Error::Other(format!("re-demux header parse failed: {e}")))?;
        assert_eq!(demuxer.width(), 20);
        assert_eq!(demuxer.height(), 12);

        let mut count = 0usize;
        while demuxer
            .read_frame()
            .map_err(|e| Error::Other(format!("re-demux read_frame failed: {e}")))?
            .is_some()
        {
            count += 1;
        }
        assert_eq!(
            count,
            png_frames.len(),
            "re-demuxed Y4M frame count must match the number of assembled frames"
        );

        for path in &frame_paths {
            std::fs::remove_file(path).ok();
        }
        std::fs::remove_file(&output_path).ok();
        Ok(())
    }

    #[test]
    fn assemble_y4m_real_roundtrip_odd_dimensions() -> Result<()> {
        // 20x12 (the other Y4M test) are both even, which never exercises
        // chroma-plane rounding. `convert_rgb_to_yuv420p` (RGB->YUV420p
        // normalization) and `Y4mChroma::bytes_per_frame` (the muxer's
        // expected frame size) both use ceil division for odd dimensions,
        // but they live in different crates and must agree exactly -- a
        // real, previously-unverified constraint this test exercises
        // directly rather than trusting by inspection.
        let job = test_job(None)?;

        let png_frames: Vec<Vec<u8>> = (0..3u8).map(|i| make_png(21, 13, i * 30)).collect();
        let mut frame_paths = Vec::new();
        for (i, bytes) in png_frames.iter().enumerate() {
            let path = tmp_path(&format!("y4m-odd-roundtrip-{i}.png"));
            std::fs::write(&path, bytes)?;
            frame_paths.push(path);
        }

        let output_path = assemble(&job, &frame_paths)?;
        assert!(output_path.exists());

        let assembled_bytes = std::fs::read(&output_path)?;
        let mut demuxer = Y4mDemuxer::new(Cursor::new(assembled_bytes))
            .map_err(|e| Error::Other(format!("re-demux header parse failed: {e}")))?;
        assert_eq!(demuxer.width(), 21);
        assert_eq!(demuxer.height(), 13);

        let mut count = 0usize;
        while demuxer
            .read_frame()
            .map_err(|e| Error::Other(format!("re-demux read_frame failed: {e}")))?
            .is_some()
        {
            count += 1;
        }
        assert_eq!(
            count,
            png_frames.len(),
            "odd-dimension Y4M frame count must match the number of assembled frames"
        );

        for path in &frame_paths {
            std::fs::remove_file(path).ok();
        }
        std::fs::remove_file(&output_path).ok();
        Ok(())
    }

    #[test]
    fn assemble_rejects_mixed_jpeg_and_png_sequence() -> Result<()> {
        let job = test_job(None)?;

        let jpeg_path = tmp_path("mixed-0.jpg");
        std::fs::write(&jpeg_path, make_jpeg(16, 16, 10))?;
        let png_path = tmp_path("mixed-1.png");
        std::fs::write(&png_path, make_png(16, 16, 20))?;

        let result = assemble(&job, &[jpeg_path.clone(), png_path.clone()]);
        assert!(
            result.is_err(),
            "a JPEG-then-PNG sequence must not silently pick one format"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("not JPEG"),
            "unexpected message: {message}"
        );

        std::fs::remove_file(&jpeg_path).ok();
        std::fs::remove_file(&png_path).ok();
        Ok(())
    }

    #[test]
    fn assemble_rejects_inconsistent_jpeg_dimensions() -> Result<()> {
        let job = test_job(None)?;

        let small_path = tmp_path("dims-0.jpg");
        std::fs::write(&small_path, make_jpeg(16, 16, 5))?;
        let big_path = tmp_path("dims-1.jpg");
        std::fs::write(&big_path, make_jpeg(32, 32, 5))?;

        let result = assemble(&job, &[small_path.clone(), big_path.clone()]);
        assert!(
            result.is_err(),
            "inconsistent frame dimensions must not be silently accepted"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("inconsistent frame dimensions"),
            "unexpected message: {message}"
        );

        std::fs::remove_file(&small_path).ok();
        std::fs::remove_file(&big_path).ok();
        Ok(())
    }
}

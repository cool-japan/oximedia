// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Real quality-metric computation for rendered frame sequences.
//!
//! Full-reference (PSNR/SSIM, via `oximedia_quality`) when the job
//! submission names a reference image in its metadata
//! ([`QUALITY_REFERENCE_METADATA_KEY`] -> a file path); no-reference
//! (blockiness/blur) otherwise. A frame that fails to decode, or whose
//! dimensions don't match the reference (full-reference case), fails the
//! whole computation honestly instead of silently averaging over a partial
//! set and reporting it as if it covered every frame.
//!
//! No score here is ever hardcoded: every returned value is the real output
//! of `oximedia_quality`'s algorithms run against actually-decoded frame
//! pixels.

use crate::error::{Error, Result};
use crate::job::Job;
use oximedia_quality::{
    BlockinessDetector, BlurDetector, Frame as QualityFrame, PsnrCalculator, SsimCalculator,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The `JobSubmission::metadata` key naming an optional reference image
/// path for full-reference PSNR/SSIM.
pub(crate) const QUALITY_REFERENCE_METADATA_KEY: &str = "quality_reference";

/// Computes real quality metrics for `frame_paths` (already `success`,
/// existing, checksum-verified frames for `job`).
///
/// # Errors
///
/// `Err` when there are no frames, a configured reference cannot be read or
/// decoded, or any frame fails to decode or mismatches dimensions.
pub(crate) fn calculate(job: &Job, frame_paths: &[PathBuf]) -> Result<HashMap<String, f64>> {
    if frame_paths.is_empty() {
        return Err(Error::Other(format!(
            "quality metrics for job {}: no successfully rendered frames available",
            job.id
        )));
    }

    match job.submission.metadata.get(QUALITY_REFERENCE_METADATA_KEY) {
        Some(reference_path) => full_reference(job, frame_paths, Path::new(reference_path)),
        None => no_reference(job, frame_paths),
    }
}

fn full_reference(
    job: &Job,
    frame_paths: &[PathBuf],
    reference_path: &Path,
) -> Result<HashMap<String, f64>> {
    let reference_bytes = std::fs::read(reference_path).map_err(|e| {
        Error::Other(format!(
            "quality metrics for job {}: could not read configured reference {}: {e}",
            job.id,
            reference_path.display()
        ))
    })?;
    let reference = crate::frame_decode::decode_normalized(&reference_bytes).map_err(|e| {
        Error::Other(format!(
            "quality metrics for job {}: could not decode configured reference {}: {e}",
            job.id,
            reference_path.display()
        ))
    })?;
    let reference_frame = crate::frame_decode::to_quality_frame(&reference);

    let psnr_calc = PsnrCalculator::new();
    let ssim_calc = SsimCalculator::new();

    let mut psnr_sum = 0.0f64;
    let mut ssim_sum = 0.0f64;

    for path in frame_paths {
        let bytes = std::fs::read(path)?;
        let decoded = crate::frame_decode::decode_normalized(&bytes).map_err(|e| {
            Error::Other(format!(
                "quality metrics for job {}: frame {} could not be decoded: {e}",
                job.id,
                path.display()
            ))
        })?;
        if (decoded.width, decoded.height) != (reference.width, reference.height) {
            return Err(Error::Other(format!(
                "quality metrics for job {}: frame {} is {}x{}, reference {} is {}x{} -- \
                 full-reference metrics require matching dimensions",
                job.id,
                path.display(),
                decoded.width,
                decoded.height,
                reference_path.display(),
                reference.width,
                reference.height
            )));
        }
        let distorted_frame = crate::frame_decode::to_quality_frame(&decoded);

        let psnr = psnr_calc
            .calculate(&reference_frame, &distorted_frame)
            .map_err(|e| {
                Error::Other(format!(
                    "PSNR calculation failed for job {} frame {}: {e}",
                    job.id,
                    path.display()
                ))
            })?;
        let ssim = ssim_calc
            .calculate(&reference_frame, &distorted_frame)
            .map_err(|e| {
                Error::Other(format!(
                    "SSIM calculation failed for job {} frame {}: {e}",
                    job.id,
                    path.display()
                ))
            })?;

        psnr_sum += psnr.score;
        ssim_sum += ssim.score;
    }

    let count = frame_paths.len() as f64;
    let mut metrics = HashMap::new();
    metrics.insert("psnr".to_string(), psnr_sum / count);
    metrics.insert("ssim".to_string(), ssim_sum / count);
    metrics.insert("frame_count".to_string(), count);
    Ok(metrics)
}

fn no_reference(job: &Job, frame_paths: &[PathBuf]) -> Result<HashMap<String, f64>> {
    let blockiness_det = BlockinessDetector::new();
    let blur_det = BlurDetector::new();

    let mut blockiness_sum = 0.0f64;
    let mut blur_sum = 0.0f64;
    let mut first_dims: Option<(u32, u32)> = None;

    for path in frame_paths {
        let bytes = std::fs::read(path)?;
        let decoded = crate::frame_decode::decode_normalized(&bytes).map_err(|e| {
            Error::Other(format!(
                "quality metrics for job {}: frame {} could not be decoded: {e}",
                job.id,
                path.display()
            ))
        })?;

        match first_dims {
            None => first_dims = Some((decoded.width, decoded.height)),
            Some(d) if d != (decoded.width, decoded.height) => {
                return Err(Error::Other(format!(
                    "quality metrics for job {}: frame {} is {}x{}, expected {}x{} (from the \
                     first frame) -- inconsistent frame dimensions are not supported",
                    job.id,
                    path.display(),
                    decoded.width,
                    decoded.height,
                    d.0,
                    d.1
                )));
            }
            Some(_) => {}
        }

        let quality_frame: QualityFrame = crate::frame_decode::to_quality_frame(&decoded);

        let blockiness = blockiness_det.detect(&quality_frame).map_err(|e| {
            Error::Other(format!(
                "blockiness detection failed for job {} frame {}: {e}",
                job.id,
                path.display()
            ))
        })?;
        let blur = blur_det.detect(&quality_frame).map_err(|e| {
            Error::Other(format!(
                "blur detection failed for job {} frame {}: {e}",
                job.id,
                path.display()
            ))
        })?;

        blockiness_sum += blockiness.score;
        blur_sum += blur.score;
    }

    let count = frame_paths.len() as f64;
    let mut metrics = HashMap::new();
    metrics.insert("blockiness".to_string(), blockiness_sum / count);
    metrics.insert("blur".to_string(), blur_sum / count);
    metrics.insert("frame_count".to_string(), count);
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{Job, JobSubmission};

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia-renderfarm-quality-metrics-{name}"))
    }

    /// Encodes an RGB buffer as a real PNG via `oximedia_codec`'s encoder.
    fn encode_png(width: u32, height: u32, rgb: Vec<u8>) -> Vec<u8> {
        use oximedia_codec::frame::{Plane, VideoFrame as CodecVideoFrame};
        use oximedia_codec::image::{EncoderConfig, ImageEncoder};
        use oximedia_core::PixelFormat;

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

    fn make_gradient_rgb(width: u32, height: u32) -> Vec<u8> {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.push((x * 8) as u8);
                rgb.push((y * 8) as u8);
                rgb.push(60);
            }
        }
        rgb
    }

    /// A "known degraded copy": the same gradient with its bottom half
    /// overwritten by a flat, unrelated color -- a deterministic, sizable
    /// difference, not random noise, so the expected direction of the
    /// PSNR/SSIM comparison is unambiguous.
    fn make_degraded_rgb(width: u32, height: u32) -> Vec<u8> {
        let mut rgb = make_gradient_rgb(width, height);
        for y in (height / 2)..height {
            for x in 0..width {
                let idx = ((y * width + x) * 3) as usize;
                rgb[idx] = 255;
                rgb[idx + 1] = 0;
                rgb[idx + 2] = 255;
            }
        }
        rgb
    }

    fn test_job_with_reference(reference_path: Option<&Path>) -> Result<Job> {
        let mut builder = JobSubmission::builder()
            .project_file(tmp_path("quality-test.blend"))
            .frame_range(1, 1);
        if let Some(reference_path) = reference_path {
            builder = builder.metadata(
                QUALITY_REFERENCE_METADATA_KEY.to_string(),
                reference_path.to_string_lossy().to_string(),
            );
        }
        Ok(Job::new(builder.build()?))
    }

    #[test]
    fn calculate_no_frames_is_honest_err() -> Result<()> {
        let job = test_job_with_reference(None)?;
        let result = calculate(&job, &[]);
        assert!(result.is_err(), "no frames must not fabricate a score");
        Ok(())
    }

    #[test]
    fn calculate_undecodable_frame_is_honest_err() -> Result<()> {
        let job = test_job_with_reference(None)?;
        let path = tmp_path("undecodable.bin");
        std::fs::write(&path, b"not an image")?;

        let result = calculate(&job, std::slice::from_ref(&path));
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
        Ok(())
    }

    #[test]
    fn calculate_missing_reference_file_is_honest_err() -> Result<()> {
        let missing_reference = tmp_path("does-not-exist-reference.png");
        std::fs::remove_file(&missing_reference).ok();
        let job = test_job_with_reference(Some(&missing_reference))?;

        let frame_path = tmp_path("frame-with-missing-reference.png");
        std::fs::write(&frame_path, encode_png(20, 20, make_gradient_rgb(20, 20)))?;

        let result = calculate(&job, std::slice::from_ref(&frame_path));
        assert!(result.is_err());

        std::fs::remove_file(&frame_path).ok();
        Ok(())
    }

    #[test]
    fn calculate_no_reference_real_no_reference_metrics() -> Result<()> {
        let job = test_job_with_reference(None)?;

        let frame_path = tmp_path("no-reference-frame.png");
        std::fs::write(&frame_path, encode_png(24, 24, make_gradient_rgb(24, 24)))?;

        let metrics = calculate(&job, std::slice::from_ref(&frame_path))?;
        assert!(metrics.contains_key("blockiness"));
        assert!(metrics.contains_key("blur"));
        assert!(!metrics.contains_key("psnr"), "no reference was configured");
        assert!(!metrics.contains_key("ssim"), "no reference was configured");
        for (name, value) in &metrics {
            assert!(value.is_finite(), "{name} should be finite, got {value}");
        }

        std::fs::remove_file(&frame_path).ok();
        Ok(())
    }

    #[test]
    fn calculate_full_reference_metrics_against_known_degraded_copy() -> Result<()> {
        let width = 32u32;
        let height = 32u32;

        let reference_bytes = encode_png(width, height, make_gradient_rgb(width, height));
        let degraded_bytes = encode_png(width, height, make_degraded_rgb(width, height));

        let reference_path = tmp_path("full-reference-reference.png");
        std::fs::write(&reference_path, &reference_bytes)?;
        let degraded_path = tmp_path("full-reference-degraded.png");
        std::fs::write(&degraded_path, &degraded_bytes)?;

        let job = test_job_with_reference(Some(&reference_path))?;

        // The reference compared against a known-degraded copy of itself.
        let degraded_metrics = calculate(&job, std::slice::from_ref(&degraded_path))?;
        let degraded_psnr = *degraded_metrics
            .get("psnr")
            .ok_or_else(|| Error::Other("expected psnr key".to_string()))?;
        let degraded_ssim = *degraded_metrics
            .get("ssim")
            .ok_or_else(|| Error::Other("expected ssim key".to_string()))?;

        // The reference compared against an *identical* copy of itself, as
        // a control: a real differential measurement must place the
        // degraded copy's score measurably below the identical copy's.
        let identical_metrics = calculate(&job, std::slice::from_ref(&reference_path))?;
        let identical_psnr = *identical_metrics
            .get("psnr")
            .ok_or_else(|| Error::Other("expected psnr key".to_string()))?;
        let identical_ssim = *identical_metrics
            .get("ssim")
            .ok_or_else(|| Error::Other("expected ssim key".to_string()))?;

        assert!(
            identical_psnr > degraded_psnr,
            "identical-frame PSNR ({identical_psnr}) should exceed the known-degraded copy's \
             PSNR ({degraded_psnr})"
        );
        assert!(
            identical_ssim > degraded_ssim,
            "identical-frame SSIM ({identical_ssim}) should exceed the known-degraded copy's \
             SSIM ({degraded_ssim})"
        );
        assert!(degraded_psnr.is_finite() && degraded_psnr > 0.0);
        assert!(degraded_ssim.is_finite());
        assert!(!degraded_metrics.contains_key("blockiness"));
        assert!(!degraded_metrics.contains_key("blur"));

        std::fs::remove_file(&reference_path).ok();
        std::fs::remove_file(&degraded_path).ok();
        Ok(())
    }

    #[test]
    fn calculate_full_reference_dimension_mismatch_is_honest_err() -> Result<()> {
        let reference_path = tmp_path("dim-mismatch-reference.png");
        std::fs::write(
            &reference_path,
            encode_png(32, 32, make_gradient_rgb(32, 32)),
        )?;
        let job = test_job_with_reference(Some(&reference_path))?;

        let frame_path = tmp_path("dim-mismatch-frame.png");
        std::fs::write(&frame_path, encode_png(16, 16, make_gradient_rgb(16, 16)))?;

        let result = calculate(&job, std::slice::from_ref(&frame_path));
        assert!(
            result.is_err(),
            "mismatched dimensions must not be silently averaged/skipped"
        );

        std::fs::remove_file(&reference_path).ok();
        std::fs::remove_file(&frame_path).ok();
        Ok(())
    }
}

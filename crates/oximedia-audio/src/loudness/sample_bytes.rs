//! PCM byte ⇄ normalised `f64` sample conversion for the loudness module.
//!
//! Loudness measurement and normalisation both need to read the raw bytes of an
//! [`AudioFrame`](crate::frame::AudioFrame) as normalised samples in
//! `[-1.0, 1.0]`, and normalisation additionally needs to write the processed
//! samples *back* in the frame's original [`SampleFormat`].
//!
//! Every currently defined `SampleFormat` is supported: `U8`, `S16`/`S16p`,
//! `S24`/`S24p` (3 bytes per sample, little-endian), `S32`/`S32p`,
//! `F32`/`F32p` and `F64`/`F64p`. `SampleFormat` is `#[non_exhaustive]`, so both
//! entry points return `None` for a format added upstream after this module was
//! written rather than silently fabricating or discarding audio.

#![forbid(unsafe_code)]

use bytes::Bytes;
use oximedia_core::SampleFormat;

/// Full-scale divisor for signed 16-bit PCM.
const S16_SCALE: f64 = 32_768.0;

/// Full-scale divisor for signed 24-bit PCM.
const S24_SCALE: f64 = 8_388_608.0;

/// Full-scale divisor for signed 32-bit PCM.
const S32_SCALE: f64 = 2_147_483_648.0;

/// Full-scale divisor for unsigned 8-bit PCM (midpoint 128).
const U8_SCALE: f64 = 128.0;

/// Decode raw PCM bytes into normalised `f64` samples in `[-1.0, 1.0]`.
///
/// A trailing partial sample (fewer bytes than
/// `format.bytes_per_sample()`) is ignored.
///
/// Returns `None` if `format` is not one of the formats listed in the module
/// documentation.
#[must_use]
pub fn decode(bytes: &[u8], format: SampleFormat) -> Option<Vec<f64>> {
    let width = format.bytes_per_sample();
    if width == 0 {
        return Some(Vec::new());
    }
    let count = bytes.len() / width;
    let mut out = Vec::with_capacity(count);

    match format {
        SampleFormat::U8 => {
            for &b in &bytes[..count] {
                out.push((f64::from(b) - U8_SCALE) / U8_SCALE);
            }
        }
        SampleFormat::S16 | SampleFormat::S16p => {
            for chunk in bytes.chunks_exact(2) {
                let v = i16::from_le_bytes([chunk[0], chunk[1]]);
                out.push(f64::from(v) / S16_SCALE);
            }
        }
        SampleFormat::S24 | SampleFormat::S24p => {
            for chunk in bytes.chunks_exact(3) {
                // Sign-extend the 24-bit little-endian value into an i32.
                let raw = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], 0]);
                let v = (raw << 8) >> 8;
                out.push(f64::from(v) / S24_SCALE);
            }
        }
        SampleFormat::S32 | SampleFormat::S32p => {
            for chunk in bytes.chunks_exact(4) {
                let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(f64::from(v) / S32_SCALE);
            }
        }
        SampleFormat::F32 | SampleFormat::F32p => {
            for chunk in bytes.chunks_exact(4) {
                let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(f64::from(v));
            }
        }
        SampleFormat::F64 | SampleFormat::F64p => {
            for chunk in bytes.chunks_exact(8) {
                let v = f64::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]);
                out.push(v);
            }
        }
        _ => return None,
    }

    Some(out)
}

/// Encode normalised `f64` samples back into raw PCM bytes in `format`.
///
/// Integer formats are rounded to nearest and clamped to the representable
/// range; float formats are written verbatim (no clamping, so a downstream
/// limiter — not this function — decides the peak ceiling).
///
/// Returns `None` for formats this module does not know how to write.
#[must_use]
pub fn encode(samples: &[f64], format: SampleFormat) -> Option<Bytes> {
    let width = format.bytes_per_sample();
    let mut out = Vec::with_capacity(samples.len() * width);

    match format {
        SampleFormat::U8 => {
            for &s in samples {
                let v = (s * U8_SCALE + U8_SCALE).round().clamp(0.0, 255.0) as u8;
                out.push(v);
            }
        }
        SampleFormat::S16 | SampleFormat::S16p => {
            for &s in samples {
                let v = (s * S16_SCALE)
                    .round()
                    .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        SampleFormat::S24 | SampleFormat::S24p => {
            for &s in samples {
                let v = (s * S24_SCALE).round().clamp(-S24_SCALE, S24_SCALE - 1.0) as i32;
                let le = v.to_le_bytes();
                out.extend_from_slice(&le[..3]);
            }
        }
        SampleFormat::S32 | SampleFormat::S32p => {
            for &s in samples {
                let v = (s * S32_SCALE)
                    .round()
                    .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        SampleFormat::F32 | SampleFormat::F32p => {
            for &s in samples {
                out.extend_from_slice(&(s as f32).to_le_bytes());
            }
        }
        SampleFormat::F64 | SampleFormat::F64p => {
            for &s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        _ => return None,
    }

    Some(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_FORMATS: [SampleFormat; 11] = [
        SampleFormat::U8,
        SampleFormat::S16,
        SampleFormat::S16p,
        SampleFormat::S24,
        SampleFormat::S24p,
        SampleFormat::S32,
        SampleFormat::S32p,
        SampleFormat::F32,
        SampleFormat::F32p,
        SampleFormat::F64,
        SampleFormat::F64p,
    ];

    /// Worst-case quantisation error for a format, as a normalised amplitude.
    fn tolerance(format: SampleFormat) -> f64 {
        match format {
            SampleFormat::U8 => 1.0 / U8_SCALE,
            SampleFormat::S16 | SampleFormat::S16p => 1.0 / S16_SCALE,
            SampleFormat::S24 | SampleFormat::S24p => 1.0 / S24_SCALE,
            SampleFormat::S32 | SampleFormat::S32p => 1.0 / S32_SCALE,
            SampleFormat::F32 | SampleFormat::F32p => 1e-6,
            _ => 1e-12,
        }
    }

    #[test]
    fn test_round_trip_all_formats() {
        let samples: Vec<f64> = (0..64)
            .map(|i| (i as f64 / 63.0).mul_add(1.6, -0.8))
            .collect();

        for format in ALL_FORMATS {
            let bytes = encode(&samples, format).expect("format must be supported");
            assert_eq!(
                bytes.len(),
                samples.len() * format.bytes_per_sample(),
                "{format:?}: wrong byte length"
            );

            let decoded = decode(&bytes, format).expect("format must be supported");
            assert_eq!(decoded.len(), samples.len(), "{format:?}: wrong count");

            let tol = tolerance(format);
            for (orig, back) in samples.iter().zip(decoded.iter()) {
                assert!(
                    (orig - back).abs() <= tol,
                    "{format:?}: {orig} round-tripped to {back} (tol {tol})"
                );
            }
        }
    }

    #[test]
    fn test_s24_sign_extension() {
        // -1.0 full scale in 24-bit is 0x800000 (little-endian 00 00 80).
        let bytes = [0x00u8, 0x00, 0x80];
        let decoded = decode(&bytes, SampleFormat::S24).expect("supported");
        assert_eq!(decoded.len(), 1);
        assert!((decoded[0] - (-1.0)).abs() < 1e-12);
    }

    #[test]
    fn test_u8_midpoint_is_silence() {
        let decoded = decode(&[128u8, 128, 128], SampleFormat::U8).expect("supported");
        assert_eq!(decoded, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_integer_encode_clamps() {
        let bytes = encode(&[4.0, -4.0], SampleFormat::S16).expect("supported");
        let decoded = decode(&bytes, SampleFormat::S16).expect("supported");
        assert!(decoded[0] <= 1.0 && decoded[0] > 0.99);
        assert!(decoded[1] >= -1.0);
    }

    #[test]
    fn test_partial_trailing_sample_ignored() {
        // 5 bytes = two whole i16 samples plus one stray byte.
        let decoded = decode(&[1u8, 0, 2, 0, 3], SampleFormat::S16).expect("supported");
        assert_eq!(decoded.len(), 2);
    }

    #[test]
    fn test_empty_input() {
        for format in ALL_FORMATS {
            assert!(decode(&[], format).expect("supported").is_empty());
            assert!(encode(&[], format).expect("supported").is_empty());
        }
    }
}

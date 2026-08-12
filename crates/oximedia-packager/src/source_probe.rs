//! Real input probing for automatic bitrate-ladder source info.
//!
//! [`probe_source_info`] is the shared helper behind
//! [`crate::dash::DashPackager`] and [`crate::hls::HlsPackager`]'s
//! `generate_ladder_from_source`: when the configured packaging input names a
//! real, readable media file, this probes it with `oximedia-container`'s
//! [`oximedia_container::MultiFormatProber`] — the same real header-scan
//! prober behind `oximedia-cli`'s `mam ingest --extract-metadata` and TUI
//! mini-probe — and turns the result into a [`SourceInfo`], instead of
//! requiring the caller to supply one by hand.
//!
//! This is deliberately best-effort and never a hard error: an unreadable
//! path, an unrecognized container, or a container with no usable video
//! stream all fall back to `None` so the caller can fall back to configured
//! `source_media` (and, ultimately, the existing "no source info" error).
//! Only the first [`PROBE_BYTES`] of the file are read, so containers that
//! place their index at the end (e.g. non-fast-start MP4) may not yield a
//! usable probe — the same limitation `oximedia-cli`'s existing probe
//! commands have.
//!
//! Known gap (outside this crate's scope): `MultiFormatProber`'s MP4/ISOBMFF
//! walker (`parse_moov`/`parse_trak`) does not currently parse `tkhd` or the
//! `stsd` visual sample entry, so it never populates
//! `DetailedStreamInfo::width`/`height` for MP4 inputs. Real MP4 files will
//! therefore always fall back to configured `source_media` here today; only
//! the EBML/Matroska (`.mkv`/`.webm`) path currently yields pixel dimensions.

use crate::ladder::SourceInfo;
use tracing::{debug, info};

/// Number of header bytes read for probing (matches `oximedia-cli`'s `mam
/// ingest --extract-metadata`, which reads 64 KiB for the same
/// `MultiFormatProber` call).
const PROBE_BYTES: usize = 64 * 1024;

/// Fallback frame rate used when a probed video stream doesn't carry one.
///
/// `MultiFormatProber` does not currently compute frame rate for any
/// container (`DetailedStreamInfo::fps` is always `None`), so this is never a
/// "detected" value — it is a documented assumption applied only when real
/// width/height/codec data was found but frame rate specifically was not, and
/// it is logged as such at the call site so it's never mistaken for probed
/// data.
const DEFAULT_FRAMERATE: f64 = 30.0;

/// Attempts to probe `input` as a real media file and derive a [`SourceInfo`]
/// from it.
///
/// Returns `None` (never an error — probing is an enhancement, not a
/// requirement) when:
/// - `input` cannot be opened (nonexistent path, permissions, or not a plain
///   file at all — e.g. a placeholder identifier used in tests),
/// - the header bytes don't match any format `MultiFormatProber` recognizes,
///   or
/// - the container was recognized but no stream has `stream_type == "video"`
///   with both a known (non-empty) codec and known dimensions.
///
/// All three cases are logged (not erased silently) so a real deployment can
/// see why a given input fell back to configured/absent source info.
pub(crate) async fn probe_source_info(input: &str) -> Option<SourceInfo> {
    let bytes = match read_probe_header(input).await {
        Ok(bytes) => bytes,
        Err(err) => {
            debug!(
                "source probe: could not open '{input}' as a media file ({err}); \
                 falling back to configured source_media"
            );
            return None;
        }
    };

    let probed = oximedia_container::MultiFormatProber::probe(&bytes);
    if probed.format == "unknown" {
        info!(
            "source probe: '{input}' header did not match a known container \
             format; falling back to configured source_media"
        );
        return None;
    }

    let Some(video) = probed.streams.iter().find(|s| {
        s.stream_type == "video" && !s.codec.is_empty() && s.width.is_some() && s.height.is_some()
    }) else {
        info!(
            "source probe: '{input}' probed as '{}' but no video stream with \
             known codec and dimensions was found; falling back to configured \
             source_media",
            probed.format
        );
        return None;
    };

    // `.find()` above already guaranteed both are `Some`.
    let width = video.width.unwrap_or_default();
    let height = video.height.unwrap_or_default();
    let framerate = video.fps.map_or(DEFAULT_FRAMERATE, f64::from);

    let mut source = SourceInfo::new(width, height, framerate, video.codec.clone());
    if let Some(kbps) = probed.bitrate_kbps {
        source = source.with_bitrate(kbps.saturating_mul(1000));
    }

    info!(
        "source probe: '{input}' -> format={} codec={} {width}x{height}{}{}",
        probed.format,
        video.codec,
        if video.fps.is_none() {
            format!(" framerate=assumed({DEFAULT_FRAMERATE})")
        } else {
            String::new()
        },
        probed
            .duration_ms
            .map(|ms| format!(" duration={:.2}s", ms as f64 / 1000.0))
            .unwrap_or_default()
    );

    Some(source)
}

/// Opens `input` and reads up to [`PROBE_BYTES`] bytes for header-based
/// probing (short reads are handled: this loops via `read_to_end` bounded by
/// a `take()` adapter rather than relying on a single `read()` call).
async fn read_probe_header(input: &str) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(input).await?;
    let mut buffer = Vec::new();
    file.take(PROBE_BYTES as u64)
        .read_to_end(&mut buffer)
        .await?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_probe_nonexistent_path_returns_none() {
        let result = probe_source_info("/nonexistent/path/does-not-exist.mkv").await;
        assert!(
            result.is_none(),
            "a nonexistent input must fall back gracefully, not error or panic"
        );
    }

    #[tokio::test]
    async fn test_probe_unrecognized_content_returns_none() {
        let dir = std::env::temp_dir().join("oximedia-pkg-source-probe-test-unknown");
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create temp dir should succeed in test");
        let path = dir.join("not-media.bin");
        tokio::fs::write(&path, b"this is not a media container header at all")
            .await
            .expect("write temp file should succeed in test");

        let result =
            probe_source_info(path.to_str().expect("temp path should be valid UTF-8")).await;
        assert!(
            result.is_none(),
            "content matching no known container magic must fall back gracefully"
        );
    }

    #[tokio::test]
    async fn test_probe_real_mkv_header_yields_source_info() {
        // A minimal but structurally valid EBML/Matroska buffer: EBML header +
        // Segment > Tracks > TrackEntry(video, V_AV1, 640x360). Real bytes the
        // real `MultiFormatProber` parses -- not a fabricated result.
        let dir = std::env::temp_dir().join("oximedia-pkg-source-probe-test-mkv");
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create temp dir should succeed in test");
        let path = dir.join("sample.mkv");
        tokio::fs::write(&path, build_minimal_mkv(640, 360, "V_AV1"))
            .await
            .expect("write temp file should succeed in test");

        let result =
            probe_source_info(path.to_str().expect("temp path should be valid UTF-8")).await;
        let source = result.expect("probing a real, well-formed video track must succeed");
        assert_eq!(source.width, 640);
        assert_eq!(source.height, 360);
        assert_eq!(source.codec, "av1");
        // fps is never signalled by MultiFormatProber today: must fall back to
        // the documented default, not silently become 0 or panic.
        assert!((source.framerate - DEFAULT_FRAMERATE).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_probe_mkv_audio_only_track_returns_none() {
        // A track that's audio-only (no video stream at all) must not
        // fabricate a video-shaped SourceInfo.
        let dir = std::env::temp_dir().join("oximedia-pkg-source-probe-test-audio-only");
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create temp dir should succeed in test");
        let path = dir.join("audio.mkv");
        tokio::fs::write(&path, build_minimal_mkv_audio_only("A_FLAC"))
            .await
            .expect("write temp file should succeed in test");

        let result =
            probe_source_info(path.to_str().expect("temp path should be valid UTF-8")).await;
        assert!(
            result.is_none(),
            "an audio-only container must not yield a fabricated video SourceInfo"
        );
    }

    // --- Minimal hand-rolled EBML/Matroska builders for the tests above -----
    //
    // These construct just enough real EBML structure (correct variable-length
    // ID/size encoding) for `oximedia_container`'s real parser to walk: an
    // EBML header, then Segment > Tracks > TrackEntry(TrackType, CodecID,
    // Video > PixelWidth/PixelHeight).

    /// Encodes one EBML element: `id` bytes, followed by a 1-byte
    /// short-form size (`0x80 | len`, so `body.len()` must stay under 127),
    /// followed by `body`.
    fn ebml_elem(id: &[u8], body: &[u8]) -> Vec<u8> {
        assert!(body.len() < 127, "test helper only supports short bodies");
        let mut out = Vec::with_capacity(id.len() + 1 + body.len());
        out.extend_from_slice(id);
        out.push(0x80 | body.len() as u8);
        out.extend_from_slice(body);
        out
    }

    fn build_minimal_mkv(width: u16, height: u16, codec_id: &str) -> Vec<u8> {
        let pixel_width = ebml_elem(&[0xB0], &width.to_be_bytes());
        let pixel_height = ebml_elem(&[0xBA], &height.to_be_bytes());
        let mut video_body = Vec::new();
        video_body.extend(pixel_width);
        video_body.extend(pixel_height);
        let video_elem = ebml_elem(&[0xE0], &video_body);

        let track_type = ebml_elem(&[0x83], &[1]); // 1 = video
        let codec = ebml_elem(&[0x86], codec_id.as_bytes());

        let mut track_entry_body = Vec::new();
        track_entry_body.extend(track_type);
        track_entry_body.extend(codec);
        track_entry_body.extend(video_elem);
        let track_entry = ebml_elem(&[0xAE], &track_entry_body);

        let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &track_entry);
        let segment = ebml_elem(&[0x18, 0x53, 0x80, 0x67], &tracks);
        let ebml_header = ebml_elem(&[0x1A, 0x45, 0xDF, 0xA3], &[0x00]);

        let mut out = Vec::new();
        out.extend(ebml_header);
        out.extend(segment);
        out
    }

    fn build_minimal_mkv_audio_only(codec_id: &str) -> Vec<u8> {
        let track_type = ebml_elem(&[0x83], &[2]); // 2 = audio
        let codec = ebml_elem(&[0x86], codec_id.as_bytes());

        let mut track_entry_body = Vec::new();
        track_entry_body.extend(track_type);
        track_entry_body.extend(codec);
        let track_entry = ebml_elem(&[0xAE], &track_entry_body);

        let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &track_entry);
        let segment = ebml_elem(&[0x18, 0x53, 0x80, 0x67], &tracks);
        let ebml_header = ebml_elem(&[0x1A, 0x45, 0xDF, 0xA3], &[0x00]);

        let mut out = Vec::new();
        out.extend(ebml_header);
        out.extend(segment);
        out
    }
}

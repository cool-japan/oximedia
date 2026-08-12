//! HLS server implementation.

use super::super::{LiveServerConfig, LiveStream, StreamRegistry};
use crate::hls::{MasterPlaylist, MediaPlaylist, Segment, StreamInf, VariantStream};
use bytes::Bytes;
use http_body_util::Full;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// HLS server.
pub struct HlsServer {
    /// Configuration.
    config: LiveServerConfig,

    /// Stream registry.
    registry: Arc<StreamRegistry>,

    /// Cached playlists.
    playlist_cache: RwLock<HashMap<String, (Bytes, std::time::Instant)>>,

    /// Cache TTL.
    cache_ttl: Duration,
}

impl HlsServer {
    /// Creates a new HLS server.
    #[must_use]
    pub fn new(config: LiveServerConfig, registry: Arc<StreamRegistry>) -> Self {
        Self {
            config,
            registry,
            playlist_cache: RwLock::new(HashMap::new()),
            cache_ttl: Duration::from_secs(1),
        }
    }

    /// Handles an HLS request.
    pub async fn handle_request(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
        response: hyper::http::response::Builder,
    ) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
        let path = req.uri().path();

        // Parse path: /hls/{app}/{stream_key}/{file}
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.len() < 3 || parts[0] != "hls" {
            return Ok(response
                .status(400)
                .body(Full::new(Bytes::from("Invalid path")))
                .unwrap_or_else(|_| {
                    hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
                }));
        }

        let app_name = parts[1];
        let stream_key = parts[2];

        // Get stream
        let stream = match self.registry.get_stream(app_name, stream_key) {
            Some(s) => s,
            None => {
                return Ok(response
                    .status(404)
                    .body(Full::new(Bytes::from("Stream not found")))
                    .unwrap_or_else(|_| {
                        hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
                    }));
            }
        };

        if parts.len() == 3 {
            // Master playlist request: /hls/{app}/{stream_key}
            return self.serve_master_playlist(&stream, response).await;
        }

        let filename = parts[3];

        // Check file type
        if filename.ends_with(".m3u8") {
            // Media playlist
            self.serve_media_playlist(&stream, filename, response).await
        } else if filename.ends_with(".m4s") || filename.ends_with(".mp4") {
            // Media segment
            self.serve_segment(&stream, filename, response).await
        } else {
            Ok(response
                .status(400)
                .body(Full::new(Bytes::from("Invalid file type")))
                .unwrap_or_else(|_| {
                    hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
                }))
        }
    }

    /// Serves master playlist.
    async fn serve_master_playlist(
        &self,
        stream: &Arc<LiveStream>,
        response: hyper::http::response::Builder,
    ) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
        let info = stream.info();

        // Build master playlist
        let mut playlist = MasterPlaylist::new();
        playlist.version = if self.config.enable_ll_hls { 9 } else { 3 };
        playlist.independent_segments = true;

        // Add variants
        for variant in &info.variants {
            let stream_inf = StreamInf {
                bandwidth: variant.bandwidth,
                average_bandwidth: Some(variant.bandwidth),
                codecs: Some(format!("{},{}", variant.video_codec, variant.audio_codec)),
                resolution: Some((variant.width, variant.height)),
                frame_rate: Some(variant.framerate),
                hdcp_level: None,
                audio: None,
                video: None,
                subtitles: None,
                closed_captions: None,
            };

            playlist.variants.push(VariantStream {
                stream_inf,
                uri: format!("{}.m3u8", variant.id),
            });
        }

        let m3u8 = playlist.to_m3u8();

        // Add to viewer count
        stream.add_viewer();

        Ok(response
            .status(200)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Cache-Control", "no-cache")
            .body(Full::new(Bytes::from(m3u8)))
            .unwrap_or_else(|_| {
                hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
            }))
    }

    /// Serves media playlist.
    async fn serve_media_playlist(
        &self,
        stream: &Arc<LiveStream>,
        filename: &str,
        response: hyper::http::response::Builder,
    ) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
        let cache_key = format!("{}_{}", stream.info().id, filename);

        // Check cache
        {
            let cache = self.playlist_cache.read();
            if let Some((cached, timestamp)) = cache.get(&cache_key) {
                if timestamp.elapsed() < self.cache_ttl {
                    return Ok(response
                        .status(200)
                        .header("Content-Type", "application/vnd.apple.mpegurl")
                        .header("Cache-Control", "max-age=1")
                        .body(Full::new(cached.clone()))
                        .unwrap_or_else(|_| {
                            hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
                        }));
                }
            }
        }

        // Build media playlist
        let mut playlist = MediaPlaylist::new();
        playlist.version = if self.config.enable_ll_hls { 9 } else { 3 };
        playlist.target_duration = self.config.segment_duration.as_secs();
        playlist.media_sequence = 0;

        // Get segments from generator
        let segments = stream
            .segment_generator
            .get_video_segments(self.config.hls_segment_count);

        if let Some(first) = segments.first() {
            playlist.media_sequence = first.sequence;
        }

        // Add segments to playlist
        for segment in &segments {
            playlist.segments.push(Segment::new(
                Duration::from_millis(segment.duration),
                segment.hls_filename(),
            ));
        }

        // For live streams, don't add endlist
        if stream.info().state != super::super::StreamState::Stopped {
            playlist.ended = false;
        } else {
            playlist.ended = true;
        }

        let m3u8 = playlist.to_m3u8();
        let bytes = Bytes::from(m3u8);

        // Update cache
        {
            let mut cache = self.playlist_cache.write();
            cache.insert(cache_key, (bytes.clone(), std::time::Instant::now()));

            // Clean old entries
            cache.retain(|_, (_, ts)| ts.elapsed() < self.cache_ttl * 10);
        }

        Ok(response
            .status(200)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Cache-Control", "max-age=1")
            .body(Full::new(bytes))
            .unwrap_or_else(|_| {
                hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
            }))
    }

    /// Serves media segment.
    async fn serve_segment(
        &self,
        stream: &Arc<LiveStream>,
        filename: &str,
        response: hyper::http::response::Builder,
    ) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
        // Server-side serve latency: time to locate the segment and build the
        // response. Real measurement replacing a hardcoded 100 ms placeholder.
        //
        // End-to-end (glass-to-glass) latency additionally needs the
        // client's own timestamps -- see `super::client` for the estimator
        // (segment PDT vs. client fetch/decode wall clock). Its module docs
        // record honestly why it isn't wired to this server end-to-end yet:
        // this handler doesn't emit `#EXT-X-PROGRAM-DATE-TIME` into served
        // playlists (and `crate::hls::playlist::MediaPlaylist::to_m3u8`,
        // outside this module, doesn't serialize it even if a `Segment` had
        // one set).
        let serve_start = std::time::Instant::now();

        // Parse filename to get sequence number
        // Format: seg_{sequence}_{uuid}.m4s or init_{uuid}.mp4

        if filename.starts_with("init_") {
            // Initialization segment
            if let Some(init_segment) = stream.segment_generator.get_video_init() {
                return Ok(response
                    .status(200)
                    .header("Content-Type", "video/mp4")
                    .header("Cache-Control", "max-age=31536000") // Cache init segments
                    .body(Full::new(init_segment.data.clone()))
                    .unwrap_or_else(|_| {
                        hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
                    }));
            }
        } else if filename.starts_with("seg_") {
            // Media segment
            if let Some(seq_str) = filename
                .strip_prefix("seg_")
                .and_then(|s| s.split('_').next())
            {
                if let Ok(sequence) = seq_str.parse::<u64>() {
                    if let Some(segment) = stream.segment_generator.get_video_segment(sequence) {
                        // Update metrics with the real server-side serve time.
                        if let Some(analytics) = stream.analytics() {
                            analytics.record_latency(serve_start.elapsed().as_millis() as u64);
                        }

                        return Ok(response
                            .status(200)
                            .header("Content-Type", "video/mp4")
                            .header("Cache-Control", "max-age=31536000")
                            .body(Full::new(segment.data.clone()))
                            .unwrap_or_else(|_| {
                                hyper::Response::new(Full::new(Bytes::from(
                                    "Internal Server Error",
                                )))
                            }));
                    }
                }
            }
        }

        Ok(response
            .status(404)
            .body(Full::new(Bytes::from("Segment not found")))
            .unwrap_or_else(|_| {
                hyper::Response::new(Full::new(Bytes::from("Internal Server Error")))
            }))
    }

    /// Clears playlist cache.
    pub fn clear_cache(&self) {
        let mut cache = self.playlist_cache.write();
        cache.clear();
    }
}

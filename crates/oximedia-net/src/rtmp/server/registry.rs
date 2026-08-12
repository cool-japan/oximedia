use super::*;

/// Cached codec sequence headers for a published stream.
///
/// A player that subscribes to a live stream mid-publish would otherwise miss
/// the codec configuration entirely: `broadcast::Sender::subscribe` only
/// delivers packets sent *after* the subscription, and sequence headers are
/// sent once, at the very start of the publish. Every subscriber therefore
/// replays this cache immediately after subscribing.
///
/// The cache is deliberately **codec-agnostic**: it mirrors whatever the
/// publisher sent, including legacy H.264/AAC configuration records. Refusing
/// unsupported codecs is the depacketizer's job at the packaging layer; an RTMP
/// relay's job is to hand a subscriber exactly what the publisher sent.
#[derive(Clone, Debug, Default)]
pub struct SeqHeaderCache {
    /// Most recent video sequence header, if the publisher has sent one.
    pub video: Option<MediaPacket>,
    /// Most recent audio sequence header, if the publisher has sent one.
    pub audio: Option<MediaPacket>,
}

impl SeqHeaderCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `packet` if it is a codec sequence header.
    ///
    /// Returns `true` when the packet was captured.
    pub fn capture(&mut self, packet: &MediaPacket) -> bool {
        match packet.packet_type {
            MediaPacketType::Video if is_video_sequence_header(&packet.data) => {
                self.video = Some(packet.clone());
                true
            }
            MediaPacketType::Audio if is_audio_sequence_header(&packet.data) => {
                self.audio = Some(packet.clone());
                true
            }
            _ => false,
        }
    }

    /// Returns the cached headers in the order a subscriber should receive
    /// them: video configuration first, then audio.
    #[must_use]
    pub fn replay_packets(&self) -> Vec<MediaPacket> {
        let mut out = Vec::with_capacity(2);
        if let Some(video) = &self.video {
            out.push(video.clone());
        }
        if let Some(audio) = &self.audio {
            out.push(audio.clone());
        }
        out
    }

    /// Returns `true` when nothing has been cached yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.video.is_none() && self.audio.is_none()
    }
}

/// Returns `true` when an FLV video tag body is a codec sequence header.
///
/// Recognises both framings:
///
/// * **Enhanced RTMP** — the ex-header marker followed by packet type
///   `SequenceStart` (0). Note that [`crate::rtmp::enhanced`] places that
///   marker at bit 3 (`0x08`) rather than the published bit 7, and this check
///   deliberately matches that crate's own encoder/decoder pair.
/// * **Legacy FLV** — codec id 7 (AVC) with `AVCPacketType == 0`.
#[must_use]
pub fn is_video_sequence_header(body: &[u8]) -> bool {
    let Some(&first) = body.first() else {
        return false;
    };
    if first & 0x08 != 0 {
        // Enhanced RTMP: [frame_type:4][is_ex_header:1][packet_type:3]
        // plus a 4-byte FourCC.
        return body.len() >= 5 && (first & 0x07) == 0;
    }
    // Legacy FLV: [frame_type:4][codec_id:4], then AVCPacketType.
    body.len() >= 2 && (first & 0x0F) == 7 && body[1] == 0
}

/// Returns `true` when an FLV audio tag body is a codec sequence header.
///
/// Recognises the Enhanced-RTMP framing (`0x9F` marker + packet type
/// `SequenceStart`) and the legacy AAC sequence header (sound format 10 with
/// `AACPacketType == 0`).
#[must_use]
pub fn is_audio_sequence_header(body: &[u8]) -> bool {
    let Some(&first) = body.first() else {
        return false;
    };
    if first == 0x9F {
        // Enhanced RTMP: 0x9F, packet type, then a 4-byte FourCC.
        return body.len() >= 6 && body[1] == 0;
    }
    body.len() >= 2 && (first >> 4) == 10 && body[1] == 0
}

/// Active stream being published.
#[derive(Clone)]
pub struct ActiveStream {
    /// Stream metadata.
    pub metadata: StreamMetadata,
    /// Publisher connection ID.
    pub publisher_id: u64,
    /// Media broadcast channel.
    pub media_tx: broadcast::Sender<MediaPacket>,
    /// Codec sequence headers cached for replay to late subscribers.
    pub seq_headers: Arc<RwLock<SeqHeaderCache>>,
}

/// Stream registry managing active streams.
pub struct StreamRegistry {
    /// Active streams (key: "app/stream_key").
    streams: RwLock<HashMap<String, ActiveStream>>,
}

impl StreamRegistry {
    /// Creates a new stream registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: RwLock::new(HashMap::new()),
        }
    }

    /// Registers a new stream.
    pub async fn register_stream(
        &self,
        key: String,
        metadata: StreamMetadata,
        publisher_id: u64,
    ) -> NetResult<broadcast::Sender<MediaPacket>> {
        let mut streams = self.streams.write().await;

        if streams.contains_key(&key) {
            return Err(NetError::invalid_state(format!(
                "Stream already exists: {key}"
            )));
        }

        let (tx, _rx) = broadcast::channel(1000);

        let active_stream = ActiveStream {
            metadata,
            publisher_id,
            media_tx: tx.clone(),
            seq_headers: Arc::new(RwLock::new(SeqHeaderCache::new())),
        };

        streams.insert(key, active_stream);
        Ok(tx)
    }

    /// Unregisters a stream.
    pub async fn unregister_stream(&self, key: &str) {
        let mut streams = self.streams.write().await;
        streams.remove(key);
    }

    /// Gets a stream for playback.
    pub async fn get_stream(&self, key: &str) -> Option<ActiveStream> {
        let streams = self.streams.read().await;
        streams.get(key).cloned()
    }

    /// Returns the sequence-header cache of a registered stream.
    ///
    /// The publisher's connection handler fetches this once, right after
    /// registration, so it can populate the cache as configuration packets
    /// arrive without re-locking the registry per packet.
    pub async fn seq_headers(&self, key: &str) -> Option<Arc<RwLock<SeqHeaderCache>>> {
        let streams = self.streams.read().await;
        streams.get(key).map(|s| Arc::clone(&s.seq_headers))
    }

    /// Returns the number of active streams.
    pub async fn stream_count(&self) -> usize {
        let streams = self.streams.read().await;
        streams.len()
    }
}

impl Default for StreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    fn packet(packet_type: MediaPacketType, body: &[u8]) -> MediaPacket {
        MediaPacket {
            packet_type,
            timestamp: 0,
            stream_id: 1,
            data: Bytes::copy_from_slice(body),
        }
    }

    // 1. Enhanced-RTMP AV1 sequence start is recognised.
    #[test]
    fn enhanced_video_sequence_header_is_recognised() {
        // [frame_type=1][ex=1][packet_type=0] = 0x18, then "av01".
        assert!(is_video_sequence_header(&[
            0x18, b'a', b'v', b'0', b'1', 0x81
        ]));
        // Coded frames (packet type 1) are not sequence headers.
        assert!(!is_video_sequence_header(&[
            0x19, b'a', b'v', b'0', b'1', 0, 0, 0
        ]));
    }

    // 2. Legacy AVC sequence header is recognised (relay fidelity).
    #[test]
    fn legacy_avc_sequence_header_is_recognised() {
        assert!(is_video_sequence_header(&[0x17, 0x00, 0, 0, 0]));
        assert!(!is_video_sequence_header(&[0x17, 0x01, 0, 0, 0]));
    }

    // 3. Enhanced and legacy audio sequence headers are recognised.
    #[test]
    fn audio_sequence_headers_are_recognised() {
        assert!(is_audio_sequence_header(&[
            0x9F, 0x00, b'O', b'p', b'u', b's', 0x01
        ]));
        assert!(!is_audio_sequence_header(&[
            0x9F, 0x01, b'O', b'p', b'u', b's'
        ]));
        assert!(is_audio_sequence_header(&[0xAF, 0x00, 0x12, 0x10]));
        assert!(!is_audio_sequence_header(&[0xAF, 0x01, 0x21]));
    }

    // 4. The cache stores headers and ignores coded frames.
    #[test]
    fn cache_captures_only_sequence_headers() {
        let mut cache = SeqHeaderCache::new();
        assert!(cache.is_empty());

        let video = packet(
            MediaPacketType::Video,
            &[0x18, b'a', b'v', b'0', b'1', 0x81],
        );
        assert!(cache.capture(&video));
        let frame = packet(
            MediaPacketType::Video,
            &[0x19, b'a', b'v', b'0', b'1', 0, 0, 0],
        );
        assert!(!cache.capture(&frame));

        let audio = packet(
            MediaPacketType::Audio,
            &[0x9F, 0x00, b'O', b'p', b'u', b's', 0x01],
        );
        assert!(cache.capture(&audio));

        assert!(!cache.is_empty());
        let replay = cache.replay_packets();
        assert_eq!(replay.len(), 2, "video then audio");
        assert_eq!(replay[0].packet_type, MediaPacketType::Video);
        assert_eq!(replay[1].packet_type, MediaPacketType::Audio);
        assert_eq!(replay[0].data, video.data);
        assert_eq!(replay[1].data, audio.data);
    }

    // 5. A later sequence header replaces the cached one.
    #[test]
    fn cache_keeps_the_most_recent_header() {
        let mut cache = SeqHeaderCache::new();
        cache.capture(&packet(
            MediaPacketType::Video,
            &[0x18, b'a', b'v', b'0', b'1', 0x81],
        ));
        cache.capture(&packet(
            MediaPacketType::Video,
            &[0x18, b'v', b'p', b'0', b'9', 0x00],
        ));
        let replay = cache.replay_packets();
        assert_eq!(replay.len(), 1);
        assert_eq!(&replay[0].data[1..5], b"vp09");
    }

    // 6. The registry hands out the cache so publisher and players share it.
    #[tokio::test]
    async fn registry_shares_the_sequence_header_cache() {
        let registry = StreamRegistry::new();
        let metadata = StreamMetadata::new("cache-test", "live");
        registry
            .register_stream("live/cache-test".to_string(), metadata, 7)
            .await
            .expect("register");

        // The publisher side captures a header through its cached handle.
        let publisher_cache = registry
            .seq_headers("live/cache-test")
            .await
            .expect("cache handle");
        publisher_cache.write().await.capture(&packet(
            MediaPacketType::Video,
            &[0x18, b'a', b'v', b'0', b'1', 0x81],
        ));

        // A late subscriber sees it through the ActiveStream it gets on play.
        let stream = registry
            .get_stream("live/cache-test")
            .await
            .expect("stream registered");
        let replay = stream.seq_headers.read().await.replay_packets();
        assert_eq!(
            replay.len(),
            1,
            "the late subscriber sees the cached header"
        );
        assert_eq!(&replay[0].data[1..5], b"av01");

        registry.unregister_stream("live/cache-test").await;
    }
}

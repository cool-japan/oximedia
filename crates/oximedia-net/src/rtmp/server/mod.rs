//! RTMP server implementation.
//!
//! This module provides a fully-featured RTMP server with support for:
//! - Accepting client connections
//! - Server-side handshake
//! - Handling publish and play requests
//! - Connection management
//! - Stream multiplexing
//! - Authentication hooks
//! - Multi-client media distribution

#![allow(dead_code)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::similar_names)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::range_plus_one)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::comparison_chain)]
#![allow(clippy::unused_self)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::if_not_else)]
#![allow(clippy::format_push_string)]
#![allow(clippy::single_match_else)]
#![allow(clippy::redundant_slicing)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::assigning_clones)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::format_collect)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::unused_async)]
#![allow(clippy::identity_op)]

pub use super::{
    amf::{AmfDecoder, AmfEncoder, AmfValue},
    chunk::{AssembledMessage, ChunkStream, MessageHeader},
    handshake::{Handshake, C0_SIZE, HANDSHAKE_SIZE},
    message::{CommandMessage, ControlMessage, DataMessage, MessageType, RtmpMessage},
};
pub use crate::error::{NetError, NetResult};
pub use async_trait::async_trait;
pub use bytes::{Bytes, BytesMut};
pub use std::collections::HashMap;
pub use std::net::SocketAddr;
pub use std::sync::Arc;
pub use std::time::{Duration, SystemTime, UNIX_EPOCH};
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub use tokio::net::{TcpListener, TcpStream};
pub use tokio::sync::{broadcast, mpsc, RwLock};
pub use tokio::time::timeout;

pub mod connection;
pub mod recording;
pub mod registry;
pub mod relay;
pub mod rtmp_server;
pub mod state;
pub mod stream_key;
pub mod types;

#[allow(unused_imports)]
pub use connection::ServerConnection;
#[allow(unused_imports)]
pub use recording::{RecordingRegistry, RecordingSession, RecordingStatus};
#[allow(unused_imports)]
pub use registry::{
    is_audio_sequence_header, is_video_sequence_header, ActiveStream, SeqHeaderCache,
    StreamRegistry,
};
#[allow(unused_imports)]
pub use relay::{RelayManager, RelayTarget};
#[allow(unused_imports)]
pub use rtmp_server::{RtmpServer, RtmpServerBuilder};
#[allow(unused_imports)]
pub use state::{ConnectionInfo, OutgoingMessage, RtmpServerConfig, ServerConnectionState};
#[allow(unused_imports)]
pub use stream_key::{StreamKeyPolicy, StreamKeyValidator};
#[allow(unused_imports)]
pub use types::{
    AllowAllAuth, AuthHandler, AuthResult, MediaPacket, MediaPacketType, PublishType,
    StreamMetadata, DEFAULT_CHUNK_SIZE, DEFAULT_READ_TIMEOUT, DEFAULT_SERVER_PORT,
    DEFAULT_WRITE_TIMEOUT,
};

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_str(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("oximedia-net-rtmp-{name}"))
            .to_string_lossy()
            .into_owned()
    }

    // ── stream key validator ──────────────────────────────────────────────────

    #[test]
    fn test_stream_key_validator_empty_rejected() {
        let policy = StreamKeyPolicy {
            reject_empty: true,
            ..Default::default()
        };
        let v = StreamKeyValidator::new(policy);
        assert!(v.validate("").is_err());
    }

    #[test]
    fn test_stream_key_validator_too_short() {
        let policy = StreamKeyPolicy {
            min_length: 8,
            ..Default::default()
        };
        let v = StreamKeyValidator::new(policy);
        assert!(v.validate("abc").is_err());
        assert!(v.validate("abcdefgh").is_ok());
    }

    #[test]
    fn test_stream_key_validator_too_long() {
        let policy = StreamKeyPolicy {
            max_length: 5,
            ..Default::default()
        };
        let v = StreamKeyValidator::new(policy);
        assert!(v.validate("toolong").is_err());
        assert!(v.validate("ok").is_ok());
    }

    #[test]
    fn test_stream_key_validator_non_ascii_rejected() {
        let v = StreamKeyValidator::new(StreamKeyPolicy::default());
        // Non-ASCII Unicode character
        assert!(v.validate("key\u{00e9}").is_err());
    }

    #[test]
    fn test_stream_key_validator_numeric_only() {
        let policy = StreamKeyPolicy {
            reject_numeric_only: true,
            ..Default::default()
        };
        let v = StreamKeyValidator::new(policy);
        assert!(v.validate("123456").is_err());
        assert!(v.validate("123abc").is_ok());
    }

    #[test]
    fn test_stream_key_validator_denylist_prefix() {
        let mut v = StreamKeyValidator::new(StreamKeyPolicy::default());
        v.add_denied_prefix("test_");
        assert!(v.validate("test_stream").is_err());
        assert!(v.validate("prod_stream").is_ok());
    }

    #[test]
    fn test_stream_key_validator_allowlist() {
        let mut v = StreamKeyValidator::new(StreamKeyPolicy::default());
        v.add_allowed_key("secret123");
        v.add_allowed_key("another_key");
        assert!(v.validate("secret123").is_ok());
        assert!(v.validate("another_key").is_ok());
        assert!(v.validate("unknown_key").is_err());
    }

    #[test]
    fn test_stream_key_validator_allowed_chars() {
        let policy = StreamKeyPolicy {
            allowed_chars: Some("abcdefghijklmnopqrstuvwxyz0123456789_-".to_string()),
            ..Default::default()
        };
        let v = StreamKeyValidator::new(policy);
        assert!(v.validate("my_stream-01").is_ok());
        assert!(v.validate("My Stream").is_err()); // space and uppercase disallowed
    }

    #[test]
    fn test_stream_key_is_valid_convenience() {
        let v = StreamKeyValidator::new(StreamKeyPolicy::default());
        assert!(v.is_valid("good_key"));
        assert!(!v.is_valid(""));
    }

    // ── relay manager ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_relay_manager_add_and_stats() {
        let relay = RelayManager::new();
        relay
            .add_target("live/stream1", "rtmp://relay1.example.com/live/stream1")
            .await;
        relay
            .add_target("live/stream1", "rtmp://relay2.example.com/live/stream1")
            .await;

        let stats = relay.stats("live/stream1").await;
        assert_eq!(stats.len(), 2);
        assert!(stats[0].active);
        assert!(stats[1].active);
    }

    #[tokio::test]
    async fn test_relay_manager_forward_to_unreachable_is_honest() {
        // Forwarding to a target that cannot be reached must NOT fabricate
        // forwarded bytes. The background forwarder attempts a real outbound
        // RTMP connection; when it fails the target is marked unhealthy and
        // `bytes_forwarded` stays 0 (nothing actually left the process).
        let relay = RelayManager::new();
        // Port 1 on localhost is closed → connect fails fast (ECONNREFUSED).
        let url = "rtmp://127.0.0.1:1/live/s";
        relay.add_target("live/s", url).await;

        let packet = MediaPacket {
            packet_type: MediaPacketType::Video,
            timestamp: 1000,
            stream_id: 1,
            data: bytes::Bytes::from(vec![0u8; 500]),
        };
        relay.forward("live/s", &packet).await;

        // Wait for the connection attempt to resolve (and fail).
        let mut marked_unhealthy = false;
        for _ in 0..100 {
            let stats = relay.stats("live/s").await;
            if !stats[0].active {
                marked_unhealthy = true;
                assert_eq!(
                    stats[0].bytes_forwarded, 0,
                    "no bytes may be reported forwarded to an unreachable target"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            marked_unhealthy,
            "forwarding to an unreachable target must mark it unhealthy, not fake success"
        );
    }

    #[tokio::test]
    async fn test_relay_manager_mark_inactive() {
        let relay = RelayManager::new();
        let url = "rtmp://dead.example.com/live/s";
        relay.add_target("live/s", url).await;
        relay.mark_inactive("live/s", url).await;

        let stats = relay.stats("live/s").await;
        assert!(!stats[0].active);
    }

    #[tokio::test]
    async fn test_relay_manager_register_unregister() {
        let relay = RelayManager::new();
        let (tx, _rx) = broadcast::channel::<MediaPacket>(16);
        let _sub = relay.register_stream("live/s", tx).await;
        relay.unregister_stream("live/s").await;
        // No panic = pass
    }

    // ── recording registry ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_recording_session_lifecycle() {
        let registry = RecordingRegistry::new();
        let id = registry
            .start_session("live/cam1", "/recordings/cam1.flv", 1000)
            .await;

        let session = registry
            .get_session(id)
            .await
            .expect("should succeed in test");
        assert_eq!(session.status, RecordingStatus::Recording);
        assert_eq!(session.bytes_written, 0);

        // Ingest a packet
        let packet = MediaPacket {
            packet_type: MediaPacketType::Video,
            timestamp: 0,
            stream_id: 1,
            data: bytes::Bytes::from(vec![0u8; 128]),
        };
        registry.ingest(id, &packet).await;

        let session = registry
            .get_session(id)
            .await
            .expect("should succeed in test");
        assert_eq!(session.bytes_written, 128);
        assert_eq!(session.packet_count, 1);
        assert_eq!(session.first_pts, Some(0));

        // Finish
        registry.finish_session(id, 2000).await;
        let session = registry
            .get_session(id)
            .await
            .expect("should succeed in test");
        assert_eq!(session.status, RecordingStatus::Finished);
        assert_eq!(session.ended_at, 2000);
    }

    #[tokio::test]
    async fn test_recording_session_pause_resume() {
        let registry = RecordingRegistry::new();
        let id = registry.start_session("live/s", tmp_str("s.flv"), 0).await;

        registry.pause_session(id).await;
        let session = registry
            .get_session(id)
            .await
            .expect("should succeed in test");
        assert_eq!(session.status, RecordingStatus::Paused);

        // Ingesting while paused should be a no-op
        let packet = MediaPacket {
            packet_type: MediaPacketType::Audio,
            timestamp: 100,
            stream_id: 1,
            data: bytes::Bytes::from(vec![0u8; 64]),
        };
        registry.ingest(id, &packet).await;
        let session = registry
            .get_session(id)
            .await
            .expect("should succeed in test");
        assert_eq!(session.bytes_written, 0);

        registry.resume_session(id).await;
        registry.ingest(id, &packet).await;
        let session = registry
            .get_session(id)
            .await
            .expect("should succeed in test");
        assert_eq!(session.bytes_written, 64);
    }

    #[tokio::test]
    async fn test_recording_registry_active_count() {
        let registry = RecordingRegistry::new();
        let id1 = registry.start_session("live/a", tmp_str("a.flv"), 0).await;
        let id2 = registry.start_session("live/b", tmp_str("b.flv"), 0).await;
        let _id3 = registry.start_session("live/c", tmp_str("c.flv"), 0).await;

        assert_eq!(registry.active_count().await, 3);

        registry.finish_session(id1, 1).await;
        registry.fail_session(id2, 1).await;
        assert_eq!(registry.active_count().await, 1);
    }

    #[tokio::test]
    async fn test_recording_registry_sessions_for_stream() {
        let registry = RecordingRegistry::new();
        let _ = registry
            .start_session("live/cam", tmp_str("1.flv"), 0)
            .await;
        let _ = registry
            .start_session("live/cam", tmp_str("2.flv"), 0)
            .await;
        let _ = registry
            .start_session("live/other", tmp_str("3.flv"), 0)
            .await;

        let cam_sessions = registry.sessions_for_stream("live/cam").await;
        assert_eq!(cam_sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_recording_registry_prune() {
        let registry = RecordingRegistry::new();
        let id1 = registry.start_session("s1", tmp_str("p1.flv"), 0).await;
        let _id2 = registry.start_session("s2", tmp_str("p2.flv"), 0).await;
        registry.finish_session(id1, 1).await;

        registry.prune_completed().await;
        assert!(registry.get_session(id1).await.is_none());
        assert_eq!(registry.active_count().await, 1);
    }

    // ── stream registry (existing type, extended tests) ───────────────────────

    #[tokio::test]
    async fn test_stream_registry_register_unregister() {
        let registry = StreamRegistry::new();
        let metadata = StreamMetadata::new("key1", "live");
        let _tx = registry
            .register_stream("live/key1".to_string(), metadata, 1)
            .await
            .expect("should succeed in test");

        assert_eq!(registry.stream_count().await, 1);

        registry.unregister_stream("live/key1").await;
        assert_eq!(registry.stream_count().await, 0);
    }

    #[tokio::test]
    async fn test_stream_registry_duplicate_rejected() {
        let registry = StreamRegistry::new();
        let meta1 = StreamMetadata::new("key1", "live");
        let meta2 = StreamMetadata::new("key1", "live");

        let _ = registry
            .register_stream("live/key1".to_string(), meta1, 1)
            .await
            .expect("should succeed in test");
        // Second registration with the same key should fail
        let result = registry
            .register_stream("live/key1".to_string(), meta2, 2)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stream_registry_get_nonexistent() {
        let registry = StreamRegistry::new();
        assert!(registry.get_stream("live/nostream").await.is_none());
    }

    // ── auth handler ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_allow_all_auth_connect() {
        let auth = AllowAllAuth;
        let params = HashMap::new();
        let result = auth
            .authenticate_connect("live", "rtmp://localhost/live", &params)
            .await;
        assert_eq!(result, AuthResult::Success);
    }

    #[tokio::test]
    async fn test_allow_all_auth_publish() {
        let auth = AllowAllAuth;
        let result = auth
            .authenticate_publish("live", "stream1", PublishType::Live)
            .await;
        assert_eq!(result, AuthResult::Success);
    }

    #[tokio::test]
    async fn test_allow_all_auth_play() {
        let auth = AllowAllAuth;
        let result = auth.authenticate_play("live", "stream1").await;
        assert_eq!(result, AuthResult::Success);
    }

    // ── misc types ────────────────────────────────────────────────────────────

    #[test]
    fn test_publish_type_round_trip() {
        for (s, expected) in &[
            ("live", PublishType::Live),
            ("record", PublishType::Record),
            ("append", PublishType::Append),
        ] {
            let pt = PublishType::from_str(s).expect("should succeed in test");
            assert_eq!(pt, *expected);
            assert_eq!(pt.as_str(), *s);
        }
        assert!(PublishType::from_str("bogus").is_none());
    }

    #[test]
    fn test_server_connection_state_is_active() {
        assert!(ServerConnectionState::Connected.is_active());
        assert!(ServerConnectionState::Publishing.is_active());
        assert!(ServerConnectionState::Playing.is_active());
        assert!(!ServerConnectionState::Closing.is_active());
        assert!(!ServerConnectionState::Closed.is_active());
    }

    #[test]
    fn test_recording_session_duration() {
        let mut session = RecordingSession::new(1, "s", tmp_str("rs.flv"), 0);
        assert!(session.duration_ms().is_none());

        let p1 = MediaPacket {
            packet_type: MediaPacketType::Video,
            timestamp: 1000,
            stream_id: 1,
            data: bytes::Bytes::from(vec![0u8; 10]),
        };
        let p2 = MediaPacket {
            packet_type: MediaPacketType::Video,
            timestamp: 3000,
            stream_id: 1,
            data: bytes::Bytes::from(vec![0u8; 10]),
        };
        session.ingest(&p1);
        session.ingest(&p2);
        assert_eq!(session.duration_ms(), Some(2000));
    }

    #[test]
    fn test_rtmp_server_builder() {
        let server = RtmpServerBuilder::new()
            .bind_address("0.0.0.0:9935")
            .max_connections(100)
            .chunk_size(8192)
            .window_ack_size(5_000_000)
            .build();
        assert_eq!(server.config.bind_address, "0.0.0.0:9935");
        assert_eq!(server.config.max_connections, 100);
        assert_eq!(server.config.chunk_size, 8192);
        assert_eq!(server.config.window_ack_size, 5_000_000);
    }
}

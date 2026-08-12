//! RTMP (Real-Time Messaging Protocol) implementation.
//!
//! This module provides types and utilities for working with RTMP streaming,
//! including handshaking, chunking, and message handling.
//!
//! # Key Types
//!
//! - [`Handshake`] - RTMP handshake handler
//! - [`ChunkHeader`] - Chunk stream header
//! - [`ChunkStream`] - Chunk stream multiplexer
//! - [`RtmpMessage`] - RTMP message types
//! - [`AmfValue`] - AMF0 data type
//!
//! # Protocol Overview
//!
//! RTMP uses a handshake followed by chunk-based messaging:
//! 1. Client sends C0+C1, server responds with S0+S1+S2
//! 2. Client sends C2, connection established
//! 3. Messages are split into chunks for multiplexing
//!
//! # Example
//!
//! ```ignore
//! use oximedia_net::rtmp::{Handshake, ChunkStream, RtmpMessage};
//!
//! async fn handle_connection(stream: TcpStream) -> NetResult<()> {
//!     let mut handshake = Handshake::new();
//!     handshake.perform(&mut stream).await?;
//!     // ...
//!     Ok(())
//! }
//! ```

mod amf;
mod chunk;
pub mod chunk_optimizer;
mod client;
pub mod enhanced;
mod handshake;
mod message;
pub mod rtmp_ext;
mod server;

pub use amf::{AmfDecoder, AmfEncoder, AmfValue};
pub use chunk::{
    Amf0Value, AssembledMessage, ChunkDecoder, ChunkEncoder, ChunkHeader, ChunkHeaderType,
    ChunkStream, MessageHeader,
};
pub use chunk_optimizer::{
    RtmpChunkOptimizer, RtmpSessionStats, DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE, MIN_CHUNK_SIZE,
};
pub use client::{
    ConnectionState, RtmpClient, RtmpClientBuilder, RtmpClientConfig, RtmpUrl, SessionInfo,
};
pub use enhanced::{
    EnhancedAudioPacketType, EnhancedAudioTag, EnhancedFrameType, EnhancedRtmpCapabilities,
    EnhancedVideoPacketType, EnhancedVideoTag, FourCC,
};
pub use handshake::{Handshake, HandshakeState};
pub use message::{
    CommandMessage, ControlMessage, DataMessage, MessageType, RtmpMessage, UserControlEvent,
};
pub use rtmp_ext::{ExVideoHeader, ExVideoPacketType, RtmpExtendedCodec, RtmpExtendedPacket};
pub use server::{
    is_audio_sequence_header, is_video_sequence_header, ActiveStream, AllowAllAuth, AuthHandler,
    AuthResult, ConnectionInfo, MediaPacket, MediaPacketType, OutgoingMessage, PublishType,
    RtmpServer, RtmpServerBuilder, RtmpServerConfig, SeqHeaderCache, ServerConnection,
    ServerConnectionState, StreamMetadata, StreamRegistry,
};

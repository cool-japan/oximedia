//! RTSP 1.0 client implementation (RFC 2326).
//!
//! Pure-Rust async client that speaks the IP-camera dialect of RTSP:
//! - `OPTIONS`, `DESCRIBE`, `SETUP`, `PLAY`, `PAUSE`, `GET_PARAMETER`, `TEARDOWN`
//! - HTTP Basic and Digest (MD5, with or without `qop=auth`) authentication
//! - TCP-interleaved transport (`Transport: RTP/AVP/TCP;interleaved=N-N+1`),
//!   which is the only transport that traverses NAT reliably
//! - SDP parsing sufficient to discover tracks and rtpmap/fmtp parameters
//! - RTP packet header parsing with sequence-loss / reorder / duplicate detection
//!
//! # Example
//!
//! ```ignore
//! use oximedia_net::rtsp::{RtspClient, SetupTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = RtspClient::connect("rtsp://admin:secret@10.0.0.5/stream1").await?;
//! let _methods = client.options().await?;
//! let sdp = client.describe().await?;
//! let video = sdp.video().expect("expected a video track");
//! let control = video.control.as_deref().unwrap_or_default();
//! client
//!     .setup(control, &SetupTransport::tcp_interleaved(0))
//!     .await?;
//! client.play().await?;
//!
//! loop {
//!     match client.next_event().await? {
//!         oximedia_net::rtsp::ServerEvent::Packet(pkt) => {
//!             let rtp = oximedia_net::rtsp::RtpPacket::parse(&pkt.data)?;
//!             println!("ch={} pt={} seq={}", pkt.channel, rtp.payload_type, rtp.sequence);
//!         }
//!         oximedia_net::rtsp::ServerEvent::Message(_) => {}
//!     }
//! }
//! # }
//! ```

pub mod auth;
pub mod client;
pub mod message;
pub mod rtp;
pub mod sdp;
pub mod transport;
pub mod url;

pub use auth::{Challenge, Credentials};
pub use client::{
    ClientConfig, RtspClient, ServerEvent, SetupResponse, SetupTransport, DEFAULT_RTSP_PORT,
    USER_AGENT,
};
pub use message::{Method, ParseStatus, Request, Response};
pub use rtp::{RtpPacket, SequenceTracker, RTP_HEADER_MIN};
pub use sdp::{ConnectionInfo, Fmtp, MediaDescription, RtpMap, SessionDescription};
pub use transport::{encode_interleaved, next_frame, FrameStatus, InterleavedPacket};
pub use url::RtspUrl;
